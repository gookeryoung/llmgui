"""Training configuration loading and validation.

The config may be JSON or YAML.  YAML support is optional: PyYAML is imported
lazily and a clear :class:`ConfigError` is raised when it is missing.  The
returned :class:`TrainConfig` is a plain dataclass, so importing this module
(and validating configs) never touches torch/transformers.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

SUPPORTED_METHODS = ("qlora", "lora")
SUPPORTED_FORMATS = ("auto", "alpaca", "sharegpt", "openai")
SUPPORTED_QUANT_BITS = (4, 8)

MIN_SEQ_LEN = 64
MAX_SEQ_LEN = 131_072
MAX_LORA_R = 1024


class ConfigError(ValueError):
    """Raised when a training config file or mapping is invalid."""


@dataclass(frozen=True)
class TrainConfig:
    """Validated training configuration."""

    base_model: str
    dataset: str
    method: str = "qlora"
    output_dir: str = "outputs/train"
    data_format: str = "auto"
    seq_len: int = 2048
    micro_batch: int = 1
    grad_accum: int = 8
    epochs: float = 1.0
    max_steps: int = -1
    lr: float = 2e-4
    weight_decay: float = 0.0
    max_grad_norm: float = 1.0
    warmup_ratio: float = 0.0
    lora_r: int = 16
    lora_alpha: int = 32
    lora_dropout: float = 0.05
    target_modules: tuple[str, ...] = ()
    quant_bits: int | None = 4
    bf16: bool = True
    gradient_checkpointing: bool = True
    mask_prompt: bool = True
    seed: int = 42
    logging_steps: int = 10
    save_steps: int = 100
    resume_from: str | None = None
    extra: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        """Return a JSON friendly dict (tuples become lists)."""

        data = asdict(self)
        data["target_modules"] = list(self.target_modules)
        data["extra"] = dict(self.extra)
        return data


def _mapping(value: Any, where: str) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise ConfigError(f"{where} must be a JSON/YAML object, got {type(value).__name__}")
    return dict(value)


def load_config_file(path: str | Path) -> dict[str, Any]:
    """Read a JSON or YAML config file into a dict."""

    config_path = Path(path)
    if not config_path.exists():
        raise ConfigError(f"config file not found: {config_path}")
    if not config_path.is_file():
        raise ConfigError(f"config path is not a file: {config_path}")

    suffix = config_path.suffix.lower()
    if suffix == ".json":
        return _load_json(config_path)
    if suffix in (".yaml", ".yml"):
        return _load_yaml(config_path)
    raise ConfigError(
        f"unsupported config extension {suffix or '<none>'!r}: expected .json, .yaml or .yml"
    )


def _load_json(path: Path) -> dict[str, Any]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigError(f"cannot read config file {path}: {exc}") from exc
    try:
        raw = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ConfigError(f"invalid JSON in {path}: {exc.msg} (line {exc.lineno})") from exc
    return _mapping(raw, f"config file {path}")


def _load_yaml(path: Path) -> dict[str, Any]:
    try:
        import yaml  # type: ignore[import-not-found]
    except ImportError as exc:
        raise ConfigError(
            "PyYAML is not installed, so YAML configs cannot be read; "
            "install PyYAML (pip install pyyaml) or use a .json config"
        ) from exc
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigError(f"cannot read config file {path}: {exc}") from exc
    try:
        raw = yaml.safe_load(text)
    except yaml.YAMLError as exc:  # type: ignore[attr-defined]
        raise ConfigError(f"invalid YAML in {path}: {exc}") from exc
    if raw is None:
        raise ConfigError(f"config file {path} is empty")
    return _mapping(raw, f"config file {path}")


def _pick(raw: Mapping[str, Any], *names: str, default: Any = None) -> Any:
    for name in names:
        if name in raw:
            return raw[name]
    return default


def _nested(raw: Mapping[str, Any], section: str, key: str) -> Any:
    value = raw.get(section)
    if isinstance(value, Mapping) and key in value:
        return value[key]
    return None


def _as_int(value: Any, field_name: str, *, minimum: int, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ConfigError(f"{field_name} must be an integer, got {value!r}")
    if isinstance(value, float) and not value.is_integer():
        raise ConfigError(f"{field_name} must be an integer, got {value!r}")
    result = int(value)
    if result < minimum:
        raise ConfigError(f"{field_name} must be >= {minimum}, got {result}")
    if maximum is not None and result > maximum:
        raise ConfigError(f"{field_name} must be <= {maximum}, got {result}")
    return result


def _as_float(value: Any, field_name: str, *, minimum: float | None = None) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ConfigError(f"{field_name} must be a number, got {value!r}")
    result = float(value)
    if minimum is not None and result < minimum:
        raise ConfigError(f"{field_name} must be >= {minimum}, got {result}")
    if result != result:  # NaN
        raise ConfigError(f"{field_name} must be a finite number")
    return result


def _as_bool(value: Any, field_name: str) -> bool:
    if isinstance(value, bool):
        return value
    raise ConfigError(f"{field_name} must be a boolean, got {value!r}")


def _as_str(value: Any, field_name: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str):
        raise ConfigError(f"{field_name} must be a string, got {type(value).__name__}")
    stripped = value.strip()
    if not stripped and not allow_empty:
        raise ConfigError(f"{field_name} must not be empty")
    return stripped


def _parse_target_modules(value: Any) -> tuple[str, ...]:
    if value is None:
        return ()
    if isinstance(value, str):
        items = [part.strip() for part in value.split(",")]
    elif isinstance(value, (list, tuple)):
        items = []
        for item in value:
            if not isinstance(item, str):
                raise ConfigError(f"target_modules entries must be strings, got {item!r}")
            items.append(item.strip())
    else:
        raise ConfigError(f"target_modules must be a string or list, got {type(value).__name__}")
    cleaned = [item for item in items if item]
    if not cleaned:
        raise ConfigError("target_modules must not be empty when provided")
    return tuple(cleaned)


def parse_config(raw: Mapping[str, Any], *, check_paths: bool = False) -> TrainConfig:
    """Validate a config mapping and return a :class:`TrainConfig`."""

    raw = _mapping(raw, "config")

    base_model = _as_str(_pick(raw, "base_model", "model", "model_name_or_path", default=""), "base_model")
    dataset = _as_str(
        _pick(raw, "dataset", "dataset_path", "data_path", "train_file", default=""),
        "dataset",
    )

    method = _as_str(_pick(raw, "method", "finetune_method", default="qlora"), "method").lower()
    if method not in SUPPORTED_METHODS:
        raise ConfigError(
            f"method must be one of {', '.join(SUPPORTED_METHODS)}, got {method!r}"
        )

    data_format = _as_str(_pick(raw, "format", "data_format", default="auto"), "format").lower()
    if data_format not in SUPPORTED_FORMATS:
        raise ConfigError(
            f"format must be one of {', '.join(SUPPORTED_FORMATS)}, got {data_format!r}"
        )

    output_dir = _as_str(_pick(raw, "output_dir", "output", default="outputs/train"), "output_dir")
    seq_len = _as_int(
        _pick(raw, "seq_len", "max_seq_len", default=2048),
        "seq_len",
        minimum=MIN_SEQ_LEN,
        maximum=MAX_SEQ_LEN,
    )
    micro_batch = _as_int(
        _pick(raw, "micro_batch", "micro_batch_size", "batch_size", default=1),
        "micro_batch",
        minimum=1,
        maximum=4096,
    )
    grad_accum = _as_int(
        _pick(raw, "grad_accum", "gradient_accumulation_steps", default=8),
        "grad_accum",
        minimum=1,
        maximum=4096,
    )
    epochs = _as_float(_pick(raw, "epochs", "num_train_epochs", default=1.0), "epochs", minimum=0.0)
    max_steps = _as_int(_pick(raw, "max_steps", default=-1), "max_steps", minimum=-1)
    lr = _as_float(_pick(raw, "lr", "learning_rate", default=2e-4), "lr", minimum=0.0)
    if lr == 0.0:
        raise ConfigError("lr must be > 0")
    weight_decay = _as_float(_pick(raw, "weight_decay", default=0.0), "weight_decay", minimum=0.0)
    max_grad_norm = _as_float(_pick(raw, "max_grad_norm", default=1.0), "max_grad_norm", minimum=0.0)
    warmup_ratio = _as_float(_pick(raw, "warmup_ratio", default=0.0), "warmup_ratio", minimum=0.0)
    if warmup_ratio > 1.0:
        raise ConfigError(f"warmup_ratio must be <= 1.0, got {warmup_ratio}")

    lora_raw = raw.get("lora")
    lora_section = lora_raw if isinstance(lora_raw, Mapping) else {}
    lora_r = _as_int(
        _pick(raw, "lora_r", default=lora_section.get("r", 16)),
        "lora_r",
        minimum=1,
        maximum=MAX_LORA_R,
    )
    lora_alpha = _as_int(
        _pick(raw, "lora_alpha", default=lora_section.get("alpha", 32)),
        "lora_alpha",
        minimum=1,
        maximum=1_000_000,
    )
    lora_dropout = _as_float(
        _pick(raw, "lora_dropout", default=lora_section.get("dropout", 0.05)),
        "lora_dropout",
        minimum=0.0,
    )
    if lora_dropout >= 1.0:
        raise ConfigError(f"lora_dropout must be < 1.0, got {lora_dropout}")
    target_modules = _parse_target_modules(
        _pick(raw, "target_modules", default=lora_section.get("target_modules"))
    )

    quant_bits_value = _pick(raw, "quant_bits", "bits", default=None)
    if "load_in_4bit" in raw:
        load_in_4bit = _as_bool(raw["load_in_4bit"], "load_in_4bit")
        if not load_in_4bit:
            raise ConfigError("load_in_4bit=false conflicts with the 4-bit QLoRA path; use method=lora")
        if quant_bits_value is None:
            quant_bits_value = 4
    if method == "qlora":
        if quant_bits_value is None:
            quant_bits = 4
        else:
            quant_bits = _as_int(quant_bits_value, "quant_bits", minimum=1, maximum=16)
            if quant_bits not in SUPPORTED_QUANT_BITS:
                raise ConfigError(
                    f"quant_bits must be one of {SUPPORTED_QUANT_BITS} for method=qlora, "
                    f"got {quant_bits}"
                )
    else:
        quant_bits = None

    bf16 = _as_bool(_pick(raw, "bf16", default=True), "bf16")
    gradient_checkpointing = _as_bool(
        _pick(raw, "gradient_checkpointing", default=True), "gradient_checkpointing"
    )
    mask_prompt = _as_bool(_pick(raw, "mask_prompt", default=True), "mask_prompt")
    seed = _as_int(_pick(raw, "seed", default=42), "seed", minimum=0)
    logging_steps = _as_int(_pick(raw, "logging_steps", default=10), "logging_steps", minimum=1)
    save_steps = _as_int(_pick(raw, "save_steps", default=100), "save_steps", minimum=1)
    resume_from = _pick(raw, "resume_from", "resume_from_checkpoint")
    if resume_from is not None:
        resume_from = _as_str(resume_from, "resume_from")

    known = {
        "base_model",
        "model",
        "model_name_or_path",
        "dataset",
        "dataset_path",
        "data_path",
        "train_file",
        "method",
        "finetune_method",
        "format",
        "data_format",
        "output_dir",
        "output",
        "seq_len",
        "max_seq_len",
        "micro_batch",
        "micro_batch_size",
        "batch_size",
        "grad_accum",
        "gradient_accumulation_steps",
        "epochs",
        "num_train_epochs",
        "max_steps",
        "lr",
        "learning_rate",
        "weight_decay",
        "max_grad_norm",
        "warmup_ratio",
        "lora_r",
        "lora_alpha",
        "lora_dropout",
        "target_modules",
        "lora",
        "quant_bits",
        "bits",
        "load_in_4bit",
        "bf16",
        "gradient_checkpointing",
        "mask_prompt",
        "seed",
        "logging_steps",
        "save_steps",
        "resume_from",
        "resume_from_checkpoint",
    }
    extra = {key: value for key, value in raw.items() if key not in known}

    config = TrainConfig(
        base_model=base_model,
        dataset=dataset,
        method=method,
        output_dir=output_dir,
        data_format=data_format,
        seq_len=seq_len,
        micro_batch=micro_batch,
        grad_accum=grad_accum,
        epochs=epochs,
        max_steps=max_steps,
        lr=lr,
        weight_decay=weight_decay,
        max_grad_norm=max_grad_norm,
        warmup_ratio=warmup_ratio,
        lora_r=lora_r,
        lora_alpha=lora_alpha,
        lora_dropout=lora_dropout,
        target_modules=target_modules,
        quant_bits=quant_bits,
        bf16=bf16,
        gradient_checkpointing=gradient_checkpointing,
        mask_prompt=mask_prompt,
        seed=seed,
        logging_steps=logging_steps,
        save_steps=save_steps,
        resume_from=resume_from,
        extra=extra,
    )

    if check_paths:
        dataset_path = Path(config.dataset)
        if not dataset_path.exists():
            raise ConfigError(f"dataset file not found: {dataset_path}")
        if not dataset_path.is_file():
            raise ConfigError(f"dataset path is not a file: {dataset_path}")

    return config


def load_train_config(path: str | Path, *, check_paths: bool = True) -> TrainConfig:
    """Load and validate a training config file.

    Relative ``dataset`` paths are resolved against the config file directory so
    the worker behaves the same regardless of its working directory.
    """

    config_path = Path(path)
    raw = load_config_file(config_path)
    dataset_key = next(
        (key for key in ("dataset", "dataset_path", "data_path", "train_file") if key in raw),
        None,
    )
    if dataset_key is not None and isinstance(raw[dataset_key], str):
        dataset = raw[dataset_key]
        if not Path(dataset).is_absolute():
            raw[dataset_key] = str((config_path.parent / dataset).resolve())
    return parse_config(raw, check_paths=check_paths)
