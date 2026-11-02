"""Tests for :mod:`llmgui_train.data`."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from llmgui_train import data
from llmgui_train.data import DataError, Example, Message


def make_example(role_contents):
    return Example(messages=[Message(role, content) for role, content in role_contents])


def test_alpaca_parsing_with_and_without_input():
    without = data.example_from_raw({"instruction": "Hi", "output": "Hello"})
    assert [message.role for message in without.messages] == ["user", "assistant"]
    assert without.messages[0].content == "Hi"
    assert without.messages[1].content == "Hello"
    assert without.text is not None and "Hello" in without.text

    with_input = data.example_from_raw(
        {"instruction": "Add", "input": "1+1", "output": "2"}
    )
    assert with_input.messages[0].content == "Add\n\n1+1"


def test_alpaca_requires_output():
    with pytest.raises(DataError, match="output"):
        data.example_from_raw({"instruction": "Hi", "output": "  "})


def test_sharegpt_parsing_and_role_mapping():
    example = data.example_from_raw(
        {
            "conversations": [
                {"from": "system", "value": "You are helpful."},
                {"from": "human", "value": "Hi"},
                {"from": "gpt", "value": "Hello"},
            ]
        }
    )
    assert [message.role for message in example.messages] == ["system", "user", "assistant"]
    assert example.messages[2].content == "Hello"


def test_sharegpt_invalid_turn():
    with pytest.raises(DataError, match="conversations"):
        data.example_from_raw({"conversations": "nope"})


def test_openai_parsing_string_and_parts():
    example = data.example_from_raw(
        {
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": [{"type": "text", "text": "part one"}, {"type": "text", "text": " two"}]},
                {"role": "assistant", "content": "answer"},
            ]
        }
    )
    assert example.messages[1].content == "part one two"
    assert example.messages[2].role == "assistant"


def test_openai_rejects_unknown_role():
    with pytest.raises(DataError, match="role"):
        data.example_from_raw({"messages": [{"role": "wizard", "content": "hi"}]})


def test_detect_format():
    assert data.detect_format({"instruction": "x", "output": "y"}) == "alpaca"
    assert data.detect_format({"conversations": []}) == "sharegpt"
    assert data.detect_format({"messages": []}) == "openai"
    assert data.detect_format({"messages": []}, "openai") == "openai"
    with pytest.raises(DataError, match="detect"):
        data.detect_format({"unknown": 1})
    with pytest.raises(DataError, match="unsupported format"):
        data.detect_format({"messages": []}, "csv")


def test_render_chat_spans_and_assistant_token_mask():
    example = make_example([("system", "sys"), ("user", "question"), ("assistant", "answer")])
    text, spans = data.render_chat(example)
    assert text.index("answer") >= spans[0][0]
    assert text[spans[0][0] : spans[0][1]] == "answer"

    # Fake tokenizer offsets: split the text into three chunks around the spans.
    assistant_start, assistant_end = spans[0]
    token_offsets = [
        (0, assistant_start),
        (assistant_start, assistant_end),
        (assistant_end, len(text)),
    ]
    mask = data.assistant_token_mask(spans, token_offsets)
    assert mask == [0, 1, 0]


def test_assistant_mask_message_level():
    example = make_example([("user", "a"), ("assistant", "b"), ("user", "c"), ("assistant", "d")])
    assert data.assistant_mask(example) == [0, 1, 0, 1]


def test_labels_from_token_mask():
    assert data.labels_from_token_mask([10, 11, 12], [0, 1, 0]) == [-100, 11, -100]
    with pytest.raises(DataError, match="mismatch"):
        data.labels_from_token_mask([1], [1, 1])


def test_pack_sequences_single_chunk_metadata():
    packed = data.pack_sequences([[1, 2, 3], [4, 5]], seq_len=6, pad_id=0)
    assert packed.num_sequences == 2
    assert packed.num_tokens == 5
    assert packed.num_padding == 1
    chunk = packed.chunks[0]
    assert chunk.input_ids == [1, 2, 3, 4, 5, 0]
    assert chunk.position_ids == [0, 1, 2, 0, 1, 0]
    assert chunk.labels == [1, 2, 3, 4, 5, -100]
    assert chunk.segment_ids == [0, 0, 0, 1, 1, -1]
    assert chunk.segments == [
        {"index": 0, "start": 0, "end": 3, "length": 3},
        {"index": 1, "start": 3, "end": 5, "length": 2},
    ]
    # Block diagonal + causal attention.
    assert chunk.attention_mask[0][:3] == [1, 0, 0]
    assert chunk.attention_mask[3][:5] == [0, 0, 0, 1, 0]
    assert chunk.attention_mask[4][:5] == [0, 0, 0, 1, 1]
    assert chunk.attention_mask[5] == [0] * 6


def test_pack_sequences_multiple_chunks_and_padding():
    packed = data.pack_sequences([[1, 2, 3], [4, 5, 6]], seq_len=3)
    assert len(packed.chunks) == 2
    assert packed.chunks[0].input_ids == [1, 2, 3]
    assert packed.chunks[1].input_ids == [4, 5, 6]
    assert packed.num_padding == 0

    padded = data.pack_sequences([[1], [2]], seq_len=4)
    assert len(padded.chunks) == 1
    assert padded.num_padding == 2


def test_pack_sequences_truncation_and_errors():
    truncated = data.pack_sequences([[1, 2, 3, 4]], seq_len=2)
    assert truncated.chunks[0].input_ids == [1, 2]

    with pytest.raises(DataError, match="longer than seq_len"):
        data.pack_sequences([[1, 2, 3, 4]], seq_len=2, truncate=False)
    with pytest.raises(DataError, match="seq_len"):
        data.pack_sequences([], seq_len=0)
    with pytest.raises(DataError, match="labels length"):
        data.pack_sequences([[1, 2]], seq_len=4, labels=[[1]])


def test_pack_sequences_sparse_mask_and_empty():
    packed = data.pack_sequences([[1, 2]], seq_len=4, dense_mask=False)
    assert packed.chunks[0].attention_mask == []
    assert packed.chunks[0].segment_ids == [0, 0, -1, -1]

    empty = data.pack_sequences([], seq_len=4)
    assert empty.chunks == []
    assert empty.to_dict()["num_tokens"] == 0


def test_load_dataset_jsonl(dataset_file: Path):
    examples = data.load_dataset(dataset_file)
    assert len(examples) == 2
    assert examples[1].messages[0].content == "Add one and two\n\n1 + 2"
    limited = data.load_dataset(dataset_file, limit=1)
    assert len(limited) == 1


def test_load_dataset_json_array_and_errors(tmp_path: Path):
    path = tmp_path / "data.json"
    path.write_text(
        json.dumps([{"instruction": "a", "output": "b"}, {"instruction": "c", "output": "d"}]),
        encoding="utf-8",
    )
    assert len(data.load_dataset(path)) == 2

    bad = tmp_path / "bad.jsonl"
    bad.write_text(
        "\n".join(
            [
                json.dumps({"instruction": "a", "output": "b"}),
                "{not json",
                json.dumps({"instruction": "c", "output": "d"}),
            ]
        ),
        encoding="utf-8",
    )
    with pytest.raises(DataError, match="invalid JSON"):
        data.load_dataset(bad)


def test_load_dataset_skip_errors(tmp_path: Path):
    path = tmp_path / "mixed.jsonl"
    path.write_text(
        "\n".join(
            [
                json.dumps({"instruction": "a", "output": "b"}),
                json.dumps({"messages": []}),
                json.dumps({"instruction": "c", "output": "d"}),
            ]
        ),
        encoding="utf-8",
    )
    assert len(data.load_dataset(path, on_error="skip")) == 2


def test_load_dataset_missing_and_not_a_file(tmp_path: Path):
    with pytest.raises(DataError, match="not found"):
        data.load_dataset(tmp_path / "nope.jsonl")
    with pytest.raises(DataError, match="not a file"):
        data.load_dataset(tmp_path)


def test_iter_examples(dataset_file: Path):
    assert [example.messages[1].content for example in data.iter_examples(dataset_file)] == [
        "Hi!",
        "Three.",
    ]
