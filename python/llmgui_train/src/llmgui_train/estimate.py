"""Training memory estimation.

The estimate follows the ``M = weights + lora + activations + logits + context``
breakdown used by the UI and CLI.  It is intentionally conservative but pure
Python: no torch required, fully unit-testable.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

GIB = 1024**3
GB = 1000**3


class EstimateError(ValueError):
    """Raised for invalid estimation inputs."""


@dataclass(frozen=True)
class ModelPreset:
    """Architecture details used to derive parameter distribution."""

    name: str
    params_b: float
    layers: int
    hidden: int
    heads: int
    kv_heads: int
    intermediate: int
    vocab_size: int
    head_dim: int | None = None

    @property
    def resolved_head_dim(self) -> int:
        if self.head_dim is not None:
            return self.head_dim
        return self.hidden // self.heads


PRESETS: dict[str, ModelPreset] = {
    "7b": ModelPreset("7b", 7.0, 32, 4096, 32, 8, 11008, 152064),
    "14b": ModelPreset("14b", 14.0, 40, 5120, 40, 8, 13824, 152064),
    "32b": ModelPreset("32b", 32.0, 64, 5120, 40, 8, 13824, 152064),
}

#: Number of LoRA target modules that roughly matches the conventional
#: ``q_proj,k_proj,v_proj,o_proj,gate_proj,up_proj,down_proj`` set.
DEFAULT_TARGET_MODULES = (
    "q_proj",
    "k_proj",
    "v_proj",
    "o_proj",
    "gate_proj",
    "up_proj",
    "down_proj",
)

# Bytes per trainable parameter in the adapter: bf16 copy (2) + fp32 grad (4)
# + two fp32 AdamW moments (8) = 14 bytes.
_ADAPTER_PARAM_BYTES = 14
# Optimizer state for full fine-tuning: bf16 weights (2) + fp32 grad (4) +
# two fp32 AdamW moments (8) = 14 bytes, plus a bf16 master copy (2).
_FULL_PARAM_BYTES = 16


def _require_number(value: Any, name: str, *, minimum: float = 0.0) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise EstimateError(f"{name} must be a number, got {value!r}")
    result = float(value)
    if result < minimum:
        raise EstimateError(f"{name} must be >= {minimum}, got {result}")
    return result


def _require_positive_int(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise EstimateError(f"{name} must be a positive integer, got {value!r}")
    return value


def _require_method(value: Any) -> str:
    if not isinstance(value, str):
        raise EstimateError(f"method must be a string, got {type(value).__name__}")
    method = value.strip().lower()
    if method not in ("qlora", "lora", "full"):
        raise EstimateError(f"method must be qlora, lora or full, got {value!r}")
    return method


def resolve_preset(
    model: str = "7b", *, params_b: float | None = None
) -> ModelPreset:
    """Resolve a preset name (``7b``/``14b``/``32b``) or a custom size."""

    if params_b is not None:
        size = _require_number(params_b, "params_b", minimum=0.01)
        return ModelPreset(f"{size:g}b", size, 32, 4096, 32, 8, 11008, 152064)

    key = str(model).strip().lower()
    if key in PRESETS:
        return PRESETS[key]
    # Accept a size embedded in a model name ("Qwen2.5-7B-Instruct" -> 7b).
    extracted = model_size_from_name(str(model))
    if extracted is not None and extracted in PRESETS:
        return PRESETS[extracted]
    # Accept "7", "7b", "7B", "0.5b", "70b" ...
    normalized = (extracted or key).rstrip("b").strip()
    try:
        size = float(normalized)
    except ValueError as exc:
        raise EstimateError(
            f"unknown model preset {model!r}; expected one of {sorted(PRESETS)} "
            "or params_b=<billions>"
        ) from exc
    if size <= 0:
        raise EstimateError(f"params_b must be > 0, got {size}")
    if size >= 100:
        return ModelPreset(key, size, 80, 8192, 64, 8, 28672, 152064)
    if size >= 20:
        return ModelPreset(key, size, 64, 5120, 40, 8, 13824, 152064)
    if size >= 10:
        return ModelPreset(key, size, 40, 5120, 40, 8, 13824, 152064)
    return ModelPreset(key, size, 32, 4096, 32, 8, 11008, 152064)


def _lora_trainable_params(
    preset: ModelPreset,
    rank: int,
    target_modules: tuple[str, ...] | list[str],
) -> int:
    hidden = preset.hidden
    intermediate = preset.intermediate
    kv_dim = preset.kv_heads * preset.resolved_head_dim
    module_shapes = {
        "q_proj": (hidden, hidden),
        "k_proj": (hidden, kv_dim),
        "v_proj": (hidden, kv_dim),
        "o_proj": (hidden, hidden),
        "gate_proj": (hidden, intermediate),
        "up_proj": (hidden, intermediate),
        "down_proj": (intermediate, hidden),
        "lm_head": (hidden, preset.vocab_size),
    }
    per_layer = 0
    for module in target_modules:
        shape = module_shapes.get(module)
        if shape is None:
            # Unknown module name: assume a square hidden x hidden projection.
            shape = (hidden, hidden)
        per_layer += rank * (shape[0] + shape[1])
    return per_layer * preset.layers


def estimate_memory(
    model: str = "7b",
    *,
    method: str = "qlora",
    bits: int = 4,
    seq_len: int = 2048,
    micro_batch: int = 1,
    grad_accum: int = 8,
    gradient_checkpointing: bool = True,
    lora_r: int = 16,
    lora_alpha: int | None = None,
    target_modules: tuple[str, ...] | list[str] | None = None,
    params_b: float | None = None,
    dtype_bytes: int = 2,
    cpu_offload: bool = False,
    optimizer: str = "adamw",
    total_budget_gb: float = 24.0,
) -> dict[str, Any]:
    """Estimate training memory in bytes.

    Parameters mirror the fields of :class:`llmgui_train.config.TrainConfig` so
    the worker can pass its params through directly.
    """

    preset = resolve_preset(model, params_b=params_b)
    method = _require_method(method)
    seq_len = _require_positive_int(seq_len, "seq_len")
    micro_batch = _require_positive_int(micro_batch, "micro_batch")
    grad_accum = _require_positive_int(grad_accum, "grad_accum")
    dtype_bytes = _require_positive_int(dtype_bytes, "dtype_bytes")
    lora_r = _require_positive_int(lora_r, "lora_r")
    if lora_alpha is None:
        lora_alpha = lora_r * 2
    lora_alpha = _require_number(lora_alpha, "lora_alpha", minimum=1)
    if not isinstance(gradient_checkpointing, bool):
        raise EstimateError(
            f"gradient_checkpointing must be a boolean, got {gradient_checkpointing!r}"
        )
    if optimizer.strip().lower() not in ("adamw", "adamw_torch", "adafactor", "sgd"):
        raise EstimateError(f"unsupported optimizer: {optimizer!r}")
    if method == "qlora":
        if isinstance(bits, bool) or bits not in (4, 8):
            raise EstimateError(f"bits must be 4 or 8 for qlora, got {bits!r}")
    target = tuple(target_modules) if target_modules else DEFAULT_TARGET_MODULES

    params = preset.params_b * 1e9

    # 1. Base weights: quantized for QLoRA, bf16/fp16 otherwise.
    if method == "qlora":
        weights_bytes = int(params * bits / 8)
        # bitsandbytes stores per-block absmax/scale (double quantization).
        weights_bytes = int(weights_bytes * 1.03)
    else:
        weights_bytes = int(params * dtype_bytes)

    # 2. Adapter weights + gradients + optimizer state (QLoRA/LoRA only).
    lora_bytes = 0
    trainable_params = 0
    if method in ("qlora", "lora"):
        trainable_params = _lora_trainable_params(preset, int(lora_r), target)
        lora_bytes = int(trainable_params * _ADAPTER_PARAM_BYTES)
    else:
        weights_bytes += int(params * _FULL_PARAM_BYTES)

    # 3. Activations.
    tokens = micro_batch * seq_len
    if gradient_checkpointing:
        # Only one layer's recompute buffer is alive at a time; add attention
        # workspace and temporal buffers.
        activations_bytes = int(tokens * preset.hidden * dtype_bytes * 6)
        activations_bytes += int(micro_batch * seq_len * preset.heads * 64 * dtype_bytes)
    else:
        activations_bytes = int(
            micro_batch * seq_len * preset.hidden * preset.layers * dtype_bytes * 2
        )

    # 4. Logits (and their gradient during backward).
    logits_bytes = int(micro_batch * seq_len * preset.vocab_size * dtype_bytes * 2)

    # 5. Context / KV cache for the full training sequence.
    kv_per_token = (
        2
        * preset.layers
        * preset.kv_heads
        * preset.resolved_head_dim
        * dtype_bytes
    )
    context_bytes = int(kv_per_token * micro_batch * seq_len)

    components = {
        "weights_bytes": weights_bytes,
        "lora_bytes": lora_bytes,
        "activations_bytes": activations_bytes,
        "logits_bytes": logits_bytes,
        "context_bytes": context_bytes,
    }
    total_bytes = sum(components.values())

    notes: list[str] = []
    if method == "qlora":
        notes.append(f"base weights quantized to {bits}-bit (bitsandbytes double quant overhead 3%)")
    if gradient_checkpointing:
        notes.append("gradient checkpointing enabled: activation estimate reduced")
    else:
        notes.append("gradient checkpointing disabled: activation estimate grows with depth")
    if cpu_offload:
        notes.append("cpu_offload requested by caller: GPU total may shrink, not modelled here")

    fits = {
        "24gb": total_bytes <= 24 * GIB,
        "32gb": total_bytes <= 32 * GIB,
        "48gb": total_bytes <= 48 * GIB,
        f"{total_budget_gb:g}gb": total_bytes <= total_budget_gb * GIB,
    }

    return {
        "schema": 1,
        "model": {
            "name": preset.name,
            "params_b": preset.params_b,
            "params": int(params),
            "layers": preset.layers,
            "hidden": preset.hidden,
            "heads": preset.heads,
            "kv_heads": preset.kv_heads,
            "intermediate": preset.intermediate,
            "vocab_size": preset.vocab_size,
        },
        "settings": {
            "method": method,
            "bits": bits if method == "qlora" else None,
            "seq_len": seq_len,
            "micro_batch": micro_batch,
            "grad_accum": grad_accum,
            "gradient_checkpointing": gradient_checkpointing,
            "lora_r": int(lora_r),
            "lora_alpha": int(lora_alpha),
            "target_modules": list(target),
            "trainable_params": int(trainable_params),
            "dtype_bytes": dtype_bytes,
            "cpu_offload": cpu_offload,
            "optimizer": optimizer,
            "tokens_per_step": micro_batch * seq_len * grad_accum,
        },
        "components": components,
        "total_bytes": total_bytes,
        "total_gib": round(total_bytes / GIB, 3),
        "total_gb": round(total_bytes / GB, 3),
        "fits": fits,
        "notes": notes,
    }


def model_size_from_name(name: str) -> str | None:
    """Best effort extraction of a size preset from a model name/path.

    ``"Qwen2.5-7B-Instruct"`` -> ``"7b"``; returns ``None`` when no size token
    is present (callers can then fall back to a default preset).
    """

    import re

    match = re.search(r"(\d+(?:\.\d+)?)\s*[bB](?![a-zA-Z0-9])", str(name))
    if not match:
        return None
    return f"{float(match.group(1)):g}b"


def estimate_from_config(config: Any) -> dict[str, Any]:
    """Estimate memory for a :class:`llmgui_train.config.TrainConfig`."""

    model = model_size_from_name(config.base_model) or "7b"
    return estimate_memory(
        model,
        method=config.method,
        bits=config.quant_bits or 4,
        seq_len=config.seq_len,
        micro_batch=config.micro_batch,
        grad_accum=config.grad_accum,
        gradient_checkpointing=config.gradient_checkpointing,
        lora_r=config.lora_r,
        lora_alpha=config.lora_alpha,
        target_modules=config.target_modules or None,
    )


def estimate_from_mapping(params: Mapping[str, Any]) -> dict[str, Any]:
    """Estimate memory from a JSON params object (worker ``estimate`` method)."""

    if not isinstance(params, Mapping):
        raise EstimateError(f"estimate params must be an object, got {type(params).__name__}")
    allowed = {
        "model",
        "method",
        "bits",
        "seq_len",
        "micro_batch",
        "grad_accum",
        "gradient_checkpointing",
        "lora_r",
        "lora_alpha",
        "target_modules",
        "params_b",
        "dtype_bytes",
        "cpu_offload",
        "optimizer",
        "total_budget_gb",
    }
    unknown = sorted(set(params) - allowed)
    if unknown:
        raise EstimateError(f"unknown estimate parameters: {', '.join(unknown)}")
    kwargs = {key: value for key, value in params.items() if key in allowed}
    if "target_modules" in kwargs and kwargs["target_modules"] is not None:
        value = kwargs["target_modules"]
        if not isinstance(value, (list, tuple)):
            raise EstimateError("target_modules must be a list of module names")
        kwargs["target_modules"] = tuple(value)
    return estimate_memory(**kwargs)


def format_report(estimate: Mapping[str, Any]) -> str:
    """Format an estimate as a short human readable report."""

    model = estimate["model"]
    components = estimate["components"]
    lines = [
        f"model:        {model['name']} ({model['params_b']}B params, {model['layers']} layers)",
        f"method:       {estimate['settings']['method']} "
        f"(bits={estimate['settings']['bits']}, seq={estimate['settings']['seq_len']}, "
        f"batch={estimate['settings']['micro_batch']}x{estimate['settings']['grad_accum']})",
        f"weights:      {_human(components['weights_bytes'])}",
        f"lora:         {_human(components['lora_bytes'])}",
        f"activations:  {_human(components['activations_bytes'])}",
        f"logits:       {_human(components['logits_bytes'])}",
        f"context:      {_human(components['context_bytes'])}",
        f"TOTAL:        {_human(estimate['total_bytes'])}",
    ]
    fits = estimate["fits"]
    lines.append(
        "fits 24GB: "
        + ("yes" if fits["24gb"] else "no")
        + " | fits 32GB: "
        + ("yes" if fits["32gb"] else "no")
    )
    return "\n".join(lines)


def _human(num_bytes: float) -> str:
    value = float(num_bytes)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024 or unit == "TiB":
            if unit == "B":
                return f"{value:.0f} B"
            return f"{value:.2f} {unit}"
        value /= 1024
    return f"{value:.2f} TiB"  # pragma: no cover - unreachable
