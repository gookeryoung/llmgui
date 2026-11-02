"""Dataset parsing and packing utilities.

Supported input formats (one JSON object per line, or a JSON array / file):

* Alpaca:      ``{"instruction", "input", "output"}``
* ShareGPT:    ``{"conversations": [{"from", "value"}, ...]}``
* OpenAI chat: ``{"messages": [{"role", "content"}, ...]}``

Everything is normalised to :class:`Example` (a list of :class:`Message` plus an
optional rendered ``text``).  This module is pure Python and has no model or
torch dependency, which keeps it fully unit-testable.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

SUPPORTED_FORMATS = ("auto", "alpaca", "sharegpt", "openai")

_ROLE_ALIASES = {
    "human": "user",
    "user": "user",
    "gpt": "assistant",
    "assistant": "assistant",
    "system": "system",
    "tool": "tool",
    "function": "tool",
    "developer": "system",
}

#: Chat markers used by :func:`render_chat`.  They are intentionally simple and
#: deterministic so that tests and downstream packing can rely on them.
CHAT_MARKERS = {
    "system": "<|system|>",
    "user": "<|user|>",
    "assistant": "<|assistant|>",
    "tool": "<|tool|>",
}


class DataError(ValueError):
    """Raised for malformed datasets or unsupported formats."""


@dataclass(frozen=True)
class Message:
    role: str
    content: str

    def to_dict(self) -> dict[str, str]:
        return {"role": self.role, "content": self.content}

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> Message:
        role = raw.get("role")
        content = raw.get("content")
        if not isinstance(role, str):
            raise DataError(f"message role must be a string, got {role!r}")
        return cls(role=normalize_role(role), content=_content_to_text(content))


@dataclass
class Example:
    """A normalised chat sample."""

    messages: list[Message]
    text: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        data: dict[str, Any] = {"messages": [message.to_dict() for message in self.messages]}
        if self.text is not None:
            data["text"] = self.text
        if self.metadata:
            data["metadata"] = self.metadata
        return data

    @property
    def roles(self) -> list[str]:
        return [message.role for message in self.messages]

    @property
    def assistant_messages(self) -> list[Message]:
        return [message for message in self.messages if message.role == "assistant"]


def normalize_role(role: str) -> str:
    mapped = _ROLE_ALIASES.get(role.strip().lower())
    if mapped is None:
        raise DataError(f"unsupported message role: {role!r}")
    return mapped


def _content_to_text(content: Any) -> str:
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, (int, float, bool)):
        return str(content)
    if isinstance(content, list):
        parts: list[str] = []
        for part in content:
            if isinstance(part, str):
                parts.append(part)
            elif isinstance(part, Mapping):
                part_type = part.get("type")
                if part_type in (None, "text", "input_text") and isinstance(part.get("text"), str):
                    parts.append(part["text"])
                elif isinstance(part.get("content"), str):
                    parts.append(part["content"])
            else:
                raise DataError(f"unsupported message content part: {part!r}")
        return "".join(parts)
    if isinstance(content, Mapping):
        if isinstance(content.get("text"), str):
            return content["text"]
        return json.dumps(content, ensure_ascii=False)
    raise DataError(f"unsupported message content type: {type(content).__name__}")


def detect_format(raw: Mapping[str, Any], override: str = "auto") -> str:
    """Return ``alpaca``/``sharegpt``/``openai`` for a decoded JSON object."""

    fmt = (override or "auto").strip().lower()
    if fmt != "auto":
        if fmt not in SUPPORTED_FORMATS:
            raise DataError(f"unsupported format {override!r}")
        return fmt

    if "messages" in raw:
        return "openai"
    if "conversations" in raw:
        return "sharegpt"
    if "instruction" in raw or ("output" in raw and ("input" in raw or "response" in raw)):
        return "alpaca"
    raise DataError(
        "cannot detect dataset format: expected 'messages', 'conversations' or 'instruction' keys"
    )


def example_from_raw(raw: Mapping[str, Any], fmt: str = "auto") -> Example:
    """Convert one decoded JSON object into an :class:`Example`."""

    if not isinstance(raw, Mapping):
        raise DataError(f"dataset row must be a JSON object, got {type(raw).__name__}")
    resolved = detect_format(raw, fmt)
    if resolved == "alpaca":
        example = _example_from_alpaca(raw)
    elif resolved == "sharegpt":
        example = _example_from_sharegpt(raw)
    else:
        example = _example_from_openai(raw)
    if example.text is None:
        example.text = render_text(example)
    if not example.messages:
        raise DataError("dataset row produced no messages")
    return example


def _example_from_alpaca(raw: Mapping[str, Any]) -> Example:
    instruction = _content_to_text(raw.get("instruction"))
    if not instruction.strip():
        raise DataError("alpaca row is missing a non-empty 'instruction'")
    extra_input = _content_to_text(raw.get("input", ""))
    prompt = instruction if not extra_input else f"{instruction}\n\n{extra_input}"
    output = _content_to_text(raw.get("output", raw.get("response")))
    if not output.strip():
        raise DataError("alpaca row is missing a non-empty 'output'")
    messages = [Message("user", prompt), Message("assistant", output)]
    metadata = {key: raw[key] for key in raw if key not in ("instruction", "input", "output", "response")}
    return Example(messages=messages, metadata=metadata)


def _example_from_sharegpt(raw: Mapping[str, Any]) -> Example:
    conversations = raw.get("conversations")
    if not isinstance(conversations, list) or not conversations:
        raise DataError("sharegpt row needs a non-empty 'conversations' list")
    messages: list[Message] = []
    for turn in conversations:
        if not isinstance(turn, Mapping):
            raise DataError(f"sharegpt turn must be an object, got {type(turn).__name__}")
        speaker = turn.get("from", turn.get("role"))
        value = turn.get("value", turn.get("content"))
        if not isinstance(speaker, str):
            raise DataError(f"sharegpt turn is missing a string 'from', got {speaker!r}")
        role = normalize_role(speaker)
        text = _content_to_text(value)
        if not text.strip():
            continue
        messages.append(Message(role, text))
    if not messages:
        raise DataError("sharegpt row produced no non-empty messages")
    metadata = {key: raw[key] for key in raw if key != "conversations"}
    return Example(messages=messages, metadata=metadata)


def _example_from_openai(raw: Mapping[str, Any]) -> Example:
    messages_raw = raw.get("messages")
    if not isinstance(messages_raw, list) or not messages_raw:
        raise DataError("openai row needs a non-empty 'messages' list")
    messages: list[Message] = []
    for item in messages_raw:
        if not isinstance(item, Mapping):
            raise DataError(f"openai message must be an object, got {type(item).__name__}")
        message = Message.from_dict(item)
        if not message.content and message.role != "assistant":
            continue
        messages.append(message)
    if not messages:
        raise DataError("openai row produced no non-empty messages")
    metadata = {key: raw[key] for key in raw if key != "messages"}
    return Example(messages=messages, metadata=metadata)


def render_chat(example: Example) -> tuple[str, list[tuple[int, int]]]:
    """Render chat messages into text and return assistant content char spans."""

    chunks: list[str] = []
    spans: list[tuple[int, int]] = []
    offset = 0
    for message in example.messages:
        marker = CHAT_MARKERS.get(message.role, f"<|{message.role}|>")
        prefix = f"{marker}\n"
        chunks.append(prefix)
        offset += len(prefix)
        content = message.content
        chunks.append(content)
        if message.role == "assistant":
            spans.append((offset, offset + len(content)))
        offset += len(content)
        chunks.append("\n")
        offset += 1
    return "".join(chunks), spans


def render_text(example: Example) -> str:
    """Render an example to a single training text string."""

    return render_chat(example)[0]


def assistant_mask(example: Example) -> list[int]:
    """Message level 0/1 mask: 1 for assistant turns, 0 otherwise."""

    return [1 if message.role == "assistant" else 0 for message in example.messages]


def assistant_token_mask(
    assistant_spans: Sequence[tuple[int, int]],
    token_offsets: Sequence[tuple[int, int]],
) -> list[int]:
    """Token level 0/1 mask from assistant char spans and tokenizer offsets.

    ``token_offsets`` uses the same ``(start, end)`` convention as HuggingFace
    fast tokenizers.  A token is labelled 1 when its character range overlaps
    any assistant span.
    """

    if not token_offsets:
        return []
    mask: list[int] = []
    for start, end in token_offsets:
        labelled = 0
        if end > start:
            for span_start, span_end in assistant_spans:
                if start < span_end and span_start < end:
                    labelled = 1
                    break
        mask.append(labelled)
    return mask


def labels_from_token_mask(input_ids: Sequence[int], mask: Sequence[int]) -> list[int]:
    """Apply a 0/1 mask to token ids using ``-100`` for masked positions."""

    if len(input_ids) != len(mask):
        raise DataError(
            f"input ids and mask length mismatch: {len(input_ids)} != {len(mask)}"
        )
    return [token if label else -100 for token, label in zip(input_ids, mask, strict=True)]


def _iter_jsonl(path: Path) -> Iterator[tuple[int, Any]]:
    with path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            stripped = line.strip()
            if not stripped:
                continue
            try:
                yield line_number, json.loads(stripped)
            except json.JSONDecodeError as exc:
                raise DataError(f"{path}:{line_number}: invalid JSON: {exc.msg}") from exc


def load_dataset(
    path: str | Path,
    fmt: str = "auto",
    *,
    limit: int | None = None,
    on_error: str = "raise",
) -> list[Example]:
    """Load a ``.jsonl`` (or ``.json`` array) dataset file.

    ``on_error="skip"`` skips malformed rows instead of raising.
    """

    dataset_path = Path(path)
    if not dataset_path.exists():
        raise DataError(f"dataset file not found: {dataset_path}")
    if not dataset_path.is_file():
        raise DataError(f"dataset path is not a file: {dataset_path}")
    if limit is not None and limit < 0:
        raise DataError(f"limit must be >= 0, got {limit}")

    raw_rows: list[Any] = []
    if dataset_path.suffix.lower() == ".json":
        try:
            text = dataset_path.read_text(encoding="utf-8")
            decoded = json.loads(text)
        except json.JSONDecodeError as exc:
            raise DataError(f"{dataset_path}: invalid JSON: {exc.msg}") from exc
        except OSError as exc:
            raise DataError(f"cannot read dataset {dataset_path}: {exc}") from exc
        if isinstance(decoded, list):
            raw_rows = decoded
        elif isinstance(decoded, Mapping) and isinstance(decoded.get("data"), list):
            raw_rows = decoded["data"]
        else:
            raise DataError(f"{dataset_path}: JSON dataset must be an array of objects")
    else:
        for _line_number, row in _iter_jsonl(dataset_path):
            raw_rows.append(row)
            if limit is not None and len(raw_rows) >= limit:
                break

    examples: list[Example] = []
    for index, row in enumerate(raw_rows):
        if limit is not None and len(examples) >= limit:
            break
        try:
            examples.append(example_from_raw(row, fmt))
        except DataError as exc:
            if on_error == "skip":
                continue
            raise DataError(f"{dataset_path}: row {index}: {exc}") from exc
    return examples


def assistant_mask_char_spans(example: Example) -> list[tuple[int, int]]:
    """Convenience wrapper around :func:`render_chat` spans."""

    return render_chat(example)[1]


@dataclass
class PackedChunk:
    """One fixed-length packed training chunk."""

    input_ids: list[int]
    labels: list[int]
    position_ids: list[int]
    segment_ids: list[int]
    attention_mask: list[list[int]]
    segments: list[dict[str, int]]

    def to_dict(self, *, include_attention_mask: bool = True) -> dict[str, Any]:
        data: dict[str, Any] = {
            "input_ids": list(self.input_ids),
            "labels": list(self.labels),
            "position_ids": list(self.position_ids),
            "segment_ids": list(self.segment_ids),
            "segments": [dict(segment) for segment in self.segments],
        }
        if include_attention_mask:
            data["attention_mask"] = [list(row) for row in self.attention_mask]
        return data


@dataclass
class PackResult:
    """Result of :func:`pack_sequences`."""

    seq_len: int
    pad_id: int
    chunks: list[PackedChunk]
    num_sequences: int
    num_tokens: int
    num_padding: int

    def to_dict(self, *, include_attention_mask: bool = True) -> dict[str, Any]:
        return {
            "seq_len": self.seq_len,
            "pad_id": self.pad_id,
            "num_sequences": self.num_sequences,
            "num_tokens": self.num_tokens,
            "num_padding": self.num_padding,
            "chunks": [
                chunk.to_dict(include_attention_mask=include_attention_mask)
                for chunk in self.chunks
            ],
        }


def _build_chunk(
    token_rows: list[tuple[list[int], list[int]]],
    seq_len: int,
    *,
    pad_id: int,
    dense_mask: bool,
) -> PackedChunk:
    input_ids: list[int] = []
    labels: list[int] = []
    position_ids: list[int] = []
    segment_ids: list[int] = []
    segments: list[dict[str, int]] = []
    for segment_index, (ids, token_labels) in enumerate(token_rows):
        start = len(input_ids)
        length = len(ids)
        input_ids.extend(ids)
        labels.extend(token_labels)
        segment_ids.extend([segment_index] * length)
        position_ids.extend(range(length))
        segments.append({"index": segment_index, "start": start, "end": start + length, "length": length})

    padding = seq_len - len(input_ids)
    if padding > 0:
        input_ids.extend([pad_id] * padding)
        labels.extend([-100] * padding)
        position_ids.extend([0] * padding)
        segment_ids.extend([-1] * padding)

    attention_mask: list[list[int]] = []
    if dense_mask:
        for row_index in range(seq_len):
            row_segment = segment_ids[row_index]
            if row_segment < 0:
                attention_mask.append([0] * seq_len)
                continue
            row: list[int] = []
            for column_index in range(seq_len):
                visible = (
                    column_index <= row_index and segment_ids[column_index] == row_segment
                )
                row.append(1 if visible else 0)
            attention_mask.append(row)

    return PackedChunk(
        input_ids=input_ids,
        labels=labels,
        position_ids=position_ids,
        segment_ids=segment_ids,
        attention_mask=attention_mask,
        segments=segments,
    )


def pack_sequences(
    sequences: Iterable[Sequence[int]],
    seq_len: int,
    *,
    labels: Iterable[Sequence[int]] | None = None,
    pad_id: int = 0,
    truncate: bool = True,
    dense_mask: bool = True,
) -> PackResult:
    """Pack token sequences into fixed length chunks.

    Each chunk contains whole sequences (never split across chunks), padded to
    exactly ``seq_len``.  Position ids restart at 0 for every packed sequence and
    the dense ``attention_mask`` is block diagonal and causal: token ``i`` sees
    token ``j`` only when they belong to the same sequence and ``j <= i``.

    ``labels`` may be supplied in parallel with ``sequences``; masked positions
    (and padding) use ``-100``.
    """

    if not isinstance(seq_len, int) or isinstance(seq_len, bool) or seq_len <= 0:
        raise DataError(f"seq_len must be a positive integer, got {seq_len!r}")

    sequence_list = [list(sequence) for sequence in sequences]
    if labels is None:
        label_list: list[list[int]] | None = None
    else:
        label_list = [list(item) for item in labels]
        if len(label_list) != len(sequence_list):
            raise DataError(
                f"labels length {len(label_list)} does not match sequences length {len(sequence_list)}"
            )

    chunks: list[PackedChunk] = []
    pending: list[tuple[list[int], list[int]]] = []
    pending_tokens = 0
    num_tokens = 0
    num_sequences = 0

    def flush() -> None:
        nonlocal pending, pending_tokens
        if not pending:
            return
        chunks.append(
            _build_chunk(pending, seq_len, pad_id=pad_id, dense_mask=dense_mask)
        )
        pending = []
        pending_tokens = 0

    for index, ids in enumerate(sequence_list):
        token_labels = label_list[index] if label_list is not None else list(ids)
        if len(token_labels) != len(ids):
            raise DataError(
                f"sequence {index}: labels length {len(token_labels)} does not match "
                f"tokens length {len(ids)}"
            )
        if not ids:
            continue
        if len(ids) > seq_len:
            if not truncate:
                raise DataError(
                    f"sequence {index} has {len(ids)} tokens, longer than seq_len={seq_len}; "
                    "enable truncate=True or increase seq_len"
                )
            ids = ids[:seq_len]
            token_labels = token_labels[:seq_len]
        if pending and pending_tokens + len(ids) > seq_len:
            flush()
        pending.append((ids, token_labels))
        pending_tokens += len(ids)
        num_tokens += len(ids)
        num_sequences += 1

    flush()
    if not chunks:
        return PackResult(
            seq_len=seq_len,
            pad_id=pad_id,
            chunks=[],
            num_sequences=0,
            num_tokens=0,
            num_padding=0,
        )

    num_padding = sum(1 for chunk in chunks for segment in chunk.segment_ids if segment == -1)
    return PackResult(
        seq_len=seq_len,
        pad_id=pad_id,
        chunks=chunks,
        num_sequences=num_sequences,
        num_tokens=num_tokens,
        num_padding=num_padding,
    )


def iter_examples(
    path: str | Path,
    fmt: str = "auto",
    *,
    limit: int | None = None,
) -> Iterator[Example]:
    """Stream examples one by one instead of loading the whole file."""

    if Path(path).suffix.lower() == ".json":
        yield from load_dataset(path, fmt, limit=limit)
        return
    dataset_path = Path(path)
    if not dataset_path.exists():
        raise DataError(f"dataset file not found: {dataset_path}")
    count = 0
    for _line_number, row in _iter_jsonl(dataset_path):
        if limit is not None and count >= limit:
            break
        yield example_from_raw(row, fmt)
        count += 1
