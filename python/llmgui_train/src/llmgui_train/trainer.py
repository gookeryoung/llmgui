"""Minimal QLoRA/LoRA SFT trainer with lazy heavy imports.

torch/transformers/peft/trl are imported **only** inside :func:`train`.  On a
machine without the training stack a :class:`MissingDependency` is raised, which
the worker turns into a structured ``missing_dependency`` JSON-RPC error.

This keeps ``import llmgui_train.trainer`` (and the whole test suite) working
without installing torch.
"""

from __future__ import annotations

import importlib.util
import math
import threading
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .config import TrainConfig
from .export import export_adapter

REQUIRED_PACKAGES = ("torch", "transformers", "peft")
OPTIONAL_PACKAGES = ("trl", "datasets", "accelerate", "safetensors", "bitsandbytes")

INSTALL_HINT = (
    "install the training extras from the repository, e.g. "
    "`pip install -e python/llmgui_train[cuda]` (see python/llmgui_train/pyproject.toml)"
)


class MissingDependency(RuntimeError):
    """Raised when the training stack is not installed."""

    code = "missing_dependency"

    def __init__(self, missing: list[str] | tuple[str, ...], *, hint: str = INSTALL_HINT) -> None:
        self.missing = list(missing)
        self.hint = hint
        super().__init__(
            "missing Python training dependencies: " + ", ".join(self.missing) + f"; {hint}"
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "code": self.code,
            "missing": list(self.missing),
            "message": str(self),
            "hint": self.hint,
        }


class TrainingCancelled(RuntimeError):
    """Raised when a cancel request interrupted training."""

    code = "training_cancelled"


@dataclass
class TrainReport:
    """Summary returned by :func:`train`."""

    adapter_dir: str
    steps: int
    epochs: float
    metrics: dict[str, Any]
    export: dict[str, Any]

    def to_dict(self) -> dict[str, Any]:
        return {
            "ok": True,
            "adapter_dir": self.adapter_dir,
            "steps": self.steps,
            "epochs": self.epochs,
            "metrics": dict(self.metrics),
            "export": self.export,
        }


def _installed(package: str) -> bool:
    try:
        return importlib.util.find_spec(package) is not None
    except (ImportError, ValueError, ModuleNotFoundError):
        return False


def available() -> dict[str, bool]:
    """Return which training related packages are importable."""

    return {
        package: _installed(package)
        for package in (*REQUIRED_PACKAGES, *OPTIONAL_PACKAGES)
    }


def missing_dependencies() -> list[str]:
    """List required packages that are not installed."""

    return [package for package in REQUIRED_PACKAGES if not _installed(package)]


def require_training_dependencies(*, quantization: bool = False) -> None:
    """Raise :class:`MissingDependency` when the stack is incomplete."""

    missing = missing_dependencies()
    if quantization and not _installed("bitsandbytes"):
        missing.append("bitsandbytes")
    if missing:
        raise MissingDependency(missing)


def training_ready() -> bool:
    return not missing_dependencies()


EventCallback = Callable[[dict[str, Any]], None]


def _emit(on_event: EventCallback | None, **event: Any) -> None:
    if on_event is not None:
        on_event(event)


def _resolve_device(torch: Any) -> Any:  # pragma: no cover - needs torch
    if torch.cuda.is_available():
        return torch.device("cuda")
    if getattr(torch.backends, "mps", None) is not None and torch.backends.mps.is_available():
        return torch.device("mps")
    return torch.device("cpu")


def _target_modules(config: TrainConfig) -> list[str] | None:
    if config.target_modules:
        return list(config.target_modules)
    return None


def _build_quantization_config(config: TrainConfig, torch: Any) -> Any:  # pragma: no cover - needs transformers
    from transformers import BitsAndBytesConfig

    compute_dtype = torch.bfloat16 if config.bf16 else torch.float16
    return BitsAndBytesConfig(
        load_in_4bit=config.quant_bits == 4,
        load_in_8bit=config.quant_bits == 8,
        bnb_4bit_quant_type="nf4",
        bnb_4bit_compute_dtype=compute_dtype,
        bnb_4bit_use_double_quant=True,
    )


def _encode_message(tokenizer: Any, role: str, content: str) -> list[int]:  # pragma: no cover
    from .data import CHAT_MARKERS

    marker = CHAT_MARKERS.get(role, f"<|{role}|>")
    ids = tokenizer(f"{marker}\n{content}\n", add_special_tokens=False)["input_ids"]
    return list(ids)


def _tokenize_example(  # pragma: no cover - needs tokenizer
    tokenizer: Any,
    example: Any,
    *,
    seq_len: int,
    mask_prompt: bool,
) -> dict[str, list[int]]:
    """Tokenize one example; optionally set prompt tokens to -100."""

    input_ids: list[int] = []
    labels: list[int] = []
    for message in example.messages:
        message_ids = _encode_message(tokenizer, message.role, message.content)
        input_ids.extend(message_ids)
        if mask_prompt and message.role != "assistant":
            labels.extend([-100] * len(message_ids))
        else:
            labels.extend(message_ids)

    if tokenizer.eos_token_id is not None:
        input_ids.append(tokenizer.eos_token_id)
        labels.append(tokenizer.eos_token_id)

    if len(input_ids) > seq_len:
        input_ids = input_ids[:seq_len]
        labels = labels[:seq_len]
    return {"input_ids": input_ids, "labels": labels}


def _collate(  # pragma: no cover - needs torch
    batch: list[Mapping[str, list[int]]],
    pad_token_id: int,
    torch: Any,
) -> dict[str, Any]:
    max_len = max(len(item["input_ids"]) for item in batch)
    input_rows: list[list[int]] = []
    label_rows: list[list[int]] = []
    attention_rows: list[list[int]] = []
    for item in batch:
        padding = max_len - len(item["input_ids"])
        input_rows.append(list(item["input_ids"]) + [pad_token_id] * padding)
        label_rows.append(list(item["labels"]) + [-100] * padding)
        attention_rows.append([1] * len(item["input_ids"]) + [0] * padding)
    return {
        "input_ids": torch.tensor(input_rows, dtype=torch.long),
        "labels": torch.tensor(label_rows, dtype=torch.long),
        "attention_mask": torch.tensor(attention_rows, dtype=torch.long),
    }


def _lr_at(step: int, total_steps: int, base_lr: float, warmup_ratio: float) -> float:
    """Simple linear warmup + cosine decay schedule (pure python, testable)."""

    if total_steps <= 0:
        return base_lr
    warmup_steps = int(total_steps * max(0.0, min(1.0, warmup_ratio)))
    if warmup_steps > 0 and step < warmup_steps:
        return base_lr * (step + 1) / warmup_steps
    if total_steps <= warmup_steps:
        return base_lr
    progress = (step - warmup_steps) / max(1, total_steps - warmup_steps)
    return base_lr * 0.5 * (1.0 + math.cos(math.pi * min(1.0, progress)))


def train(  # pragma: no cover - only exercised with the full ML stack
    config: TrainConfig,
    *,
    on_event: EventCallback | None = None,
    cancel_event: threading.Event | None = None,
) -> dict[str, Any]:
    """Run a minimal SFT (QLoRA/LoRA) fine-tune and export the adapter.

    Raises :class:`MissingDependency` when torch/transformers/peft are absent and
    :class:`TrainingCancelled` when ``cancel_event`` is set during the loop.
    """

    require_training_dependencies(quantization=config.method == "qlora")

    # --- lazy heavy imports ------------------------------------------------
    import torch
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
    from transformers import AutoModelForCausalLM, AutoTokenizer

    from . import data as data_module

    cancel = cancel_event or threading.Event()
    _emit(
        on_event,
        kind="train.started",
        config={
            "base_model": config.base_model,
            "method": config.method,
            "seq_len": config.seq_len,
            "micro_batch": config.micro_batch,
            "grad_accum": config.grad_accum,
            "epochs": config.epochs,
        },
    )

    torch.manual_seed(config.seed)
    device = _resolve_device(torch)

    tokenizer = AutoTokenizer.from_pretrained(config.base_model, trust_remote_code=True)
    if tokenizer.pad_token_id is None:
        tokenizer.pad_token = tokenizer.eos_token
    pad_token_id = int(tokenizer.pad_token_id or 0)

    model_kwargs: dict[str, Any] = {"trust_remote_code": True}
    if config.method == "qlora":
        model_kwargs["quantization_config"] = _build_quantization_config(config, torch)
        model_kwargs["device_map"] = {"": device} if device.type == "cuda" else None
    else:
        model_kwargs["torch_dtype"] = torch.bfloat16 if config.bf16 else torch.float16
    model = AutoModelForCausalLM.from_pretrained(config.base_model, **model_kwargs)

    if config.method == "qlora":
        model = prepare_model_for_kbit_training(
            model, use_gradient_checkpointing=config.gradient_checkpointing
        )

    lora_config = LoraConfig(
        r=config.lora_r,
        lora_alpha=config.lora_alpha,
        lora_dropout=config.lora_dropout,
        target_modules=_target_modules(config) or "all-linear",
        bias="none",
        task_type="CAUSAL_LM",
    )
    model = get_peft_model(model, lora_config)
    if config.gradient_checkpointing:
        model.gradient_checkpointing_enable()
    model.to(device)
    model.train()

    examples = data_module.load_dataset(config.dataset, config.data_format)
    if not examples:
        raise ValueError(f"dataset is empty: {config.dataset}")
    _emit(on_event, kind="train.dataset_loaded", examples=len(examples))

    tokenized = [
        _tokenize_example(
            tokenizer,
            example,
            seq_len=config.seq_len,
            mask_prompt=config.mask_prompt,
        )
        for example in examples
    ]

    micro_batch = config.micro_batch
    grad_accum = config.grad_accum
    batches_per_epoch = max(1, math.ceil(len(tokenized) / micro_batch))
    planned_steps = batches_per_epoch * max(1.0, config.epochs) / grad_accum
    total_steps = int(planned_steps) if config.max_steps <= 0 else config.max_steps
    total_steps = max(1, total_steps)

    trainable = [param for param in model.parameters() if param.requires_grad]
    optimizer = torch.optim.AdamW(
        trainable,
        lr=config.lr,
        weight_decay=config.weight_decay,
    )

    step = 0
    micro_step = 0
    running_loss = 0.0
    loss_count = 0
    metrics: dict[str, Any] = {}
    optimizer.zero_grad(set_to_none=True)
    stop = False

    while not stop:
        for batch_start in range(0, len(tokenized), micro_batch):
            if cancel.is_set():
                raise TrainingCancelled("training cancelled by request")
            batch = _collate(
                tokenized[batch_start : batch_start + micro_batch],
                pad_token_id,
                torch,
            )
            batch = {key: value.to(device) for key, value in batch.items()}
            outputs = model(**batch)
            loss = outputs.loss
            (loss / grad_accum).backward()
            running_loss += float(loss.detach().float().item())
            loss_count += 1
            micro_step += 1

            if micro_step % grad_accum != 0:
                continue

            lr = _lr_at(step, total_steps, config.lr, config.warmup_ratio)
            for group in optimizer.param_groups:
                group["lr"] = lr
            if config.max_grad_norm > 0:
                torch.nn.utils.clip_grad_norm_(trainable, config.max_grad_norm)
            optimizer.step()
            optimizer.zero_grad(set_to_none=True)

            if loss_count:
                metrics["loss"] = round(running_loss / loss_count, 6)
            metrics["lr"] = lr
            metrics["step"] = step
            step += 1
            _emit(
                on_event,
                kind="train.progress",
                step=step,
                total_steps=total_steps,
                loss=metrics["loss"],
                lr=lr,
            )
            if step >= total_steps or cancel.is_set():
                stop = True
                break

        if cancel.is_set():
            raise TrainingCancelled("training cancelled by request")
        if config.epochs >= 1.0:
            continue
        break

    output_dir = Path(config.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(str(output_dir))
    tokenizer.save_pretrained(str(output_dir))
    _emit(on_event, kind="train.adapter_saved", adapter_dir=str(output_dir))

    export = export_adapter(
        output_dir,
        config=config,
        training={"steps": step, "device": str(device)},
        metrics=metrics,
        allow_manifest_only=True,
    )
    _emit(on_event, kind="train.completed", adapter_dir=str(output_dir))

    return TrainReport(
        adapter_dir=str(output_dir),
        steps=step,
        epochs=float(config.epochs),
        metrics=metrics,
        export=export,
    ).to_dict()
