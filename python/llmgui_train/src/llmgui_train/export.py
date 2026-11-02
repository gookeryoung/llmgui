"""Adapter export helpers.

Two kinds of exports are supported:

* ``export_adapter`` writes a PEFT compatible adapter directory.  When torch and
  safetensors are available the weights are written as safetensors; otherwise a
  manifest is written and the result is explicitly marked ``manifest_only``.
* ``merge_adapter`` / ``export_gguf`` shell out to llama.cpp (or any configured
  executable).  Missing tools produce a structured error instead of a traceback.

No heavy dependency is imported at module import time.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
from collections.abc import Mapping, Sequence
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from . import __version__
from .capabilities import detect

ADAPTER_CONFIG_FILE = "adapter_config.json"
ADAPTER_WEIGHTS_FILE = "adapter_model.safetensors"
MANIFEST_FILE = "training_manifest.json"
MANIFEST_SCHEMA = 1

DEFAULT_MERGE_TOOL_ENV = "LLMGUI_TRAIN_MERGE_TOOL"
DEFAULT_GGUF_TOOL_ENV = "LLMGUI_TRAIN_GGUF_TOOL"
DEFAULT_MERGE_TOOL = "llama-export-lora"
DEFAULT_GGUF_TOOL = "convert_hf_to_gguf.py"


class ExportError(RuntimeError):
    """Structured export failure."""

    def __init__(
        self,
        message: str,
        *,
        code: str = "export_failed",
        data: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.data = dict(data or {})

    def to_dict(self) -> dict[str, Any]:
        return {
            "ok": False,
            "error": {
                "code": self.code,
                "message": str(self),
                **self.data,
            },
        }


def _now_iso() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds")


def _as_text(value: Any) -> str:
    return str(value)


def _get(config: Any, name: str, default: Any = None) -> Any:
    """Read a field from either a dataclass/object or a mapping."""

    if isinstance(config, Mapping):
        return config.get(name, default)
    return getattr(config, name, default)


def _lora_config_dict(config: Any) -> dict[str, Any]:
    return {
        "peft_type": "LORA",
        "task_type": "CAUSAL_LM",
        "r": int(_get(config, "lora_r", 16)),
        "lora_alpha": int(_get(config, "lora_alpha", 32)),
        "lora_dropout": float(_get(config, "lora_dropout", 0.05)),
        "target_modules": list(_get(config, "target_modules", ()) or []),
        "bias": "none",
        "inference_mode": False,
        "base_model_name_or_path": _as_text(_get(config, "base_model", "")),
        "use_dora": False,
        "use_rslora": False,
        "fan_in_fan_out": False,
        "modules_to_save": None,
    }


def _config_to_dict(config: Any) -> dict[str, Any]:
    to_dict = getattr(config, "to_dict", None)
    if callable(to_dict):
        return dict(to_dict())
    if isinstance(config, Mapping):
        return dict(config)
    return {}


def adapter_manifest(
    config: Any,
    *,
    training: Mapping[str, Any] | None = None,
    metrics: Mapping[str, Any] | None = None,
    files: Mapping[str, Any] | None = None,
    created_at: str | None = None,
) -> dict[str, Any]:
    """Build the ``training_manifest.json`` payload."""

    capabilities = detect()
    dataset = {
        "path": _as_text(_get(config, "dataset", "")),
        "format": _as_text(_get(config, "data_format", "auto")),
    }
    if training and isinstance(training.get("dataset"), Mapping):
        dataset.update(dict(training["dataset"]))

    manifest: dict[str, Any] = {
        "schema": MANIFEST_SCHEMA,
        "kind": "llmgui_train.adapter_manifest",
        "created_at": created_at or _now_iso(),
        "base_model": _as_text(_get(config, "base_model", "")),
        "method": _as_text(_get(config, "method", "qlora")),
        "adapter": _lora_config_dict(config),
        "config": _config_to_dict(config),
        "dataset": dataset,
        "metrics": dict(metrics or {}),
        "training": dict(training or {}),
        "framework": {
            "name": "llmgui-train",
            "version": __version__,
            "python": capabilities["python"],
            "torch": capabilities["torch"].get("version"),
            "transformers": capabilities["packages"]["transformers"]["version"],
            "peft": capabilities["packages"]["peft"]["version"],
            "bitsandbytes": capabilities["packages"]["bitsandbytes"]["version"],
        },
        "capabilities": {
            "training_ready": capabilities["training"]["ready"],
            "missing": capabilities["training"]["missing"],
            "cuda": capabilities["accelerators"]["cuda"].get("available", False),
            "rocm": capabilities["accelerators"]["rocm"].get("available", False),
        },
        "files": dict(files or {}),
    }
    return manifest


def write_json(path: str | Path, payload: Mapping[str, Any]) -> Path:
    """Write a JSON file atomically enough for our purposes."""

    target = Path(path)
    try:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(dict(payload), indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    except OSError as exc:
        raise ExportError(
            f"cannot write {target}: {exc}",
            code="io_error",
            data={"path": str(target)},
        ) from exc
    return target


def _safetensors_available() -> bool:
    try:
        import safetensors  # type: ignore[import-not-found]  # noqa: F401
        import torch  # type: ignore[import-not-found]  # noqa: F401
    except ImportError:
        return False
    return True


def _write_safetensors(target: Path, state_dict: Mapping[str, Any]) -> Path:  # pragma: no cover - needs torch
    try:
        from safetensors.torch import save_file  # type: ignore[import-not-found]
    except ImportError as exc:
        raise ExportError(
            "safetensors/torch are not installed; adapter weights cannot be saved "
            "(install llmgui_train[cuda] or pass allow_manifest_only=True)",
            code="missing_dependency",
            data={"missing": ["torch", "safetensors"]},
        ) from exc
    try:
        save_file({key: value for key, value in state_dict.items()}, str(target))
    except Exception as exc:  # pragma: no cover - torch object issues
        raise ExportError(
            f"failed to write safetensors: {exc}",
            code="export_failed",
            data={"path": str(target)},
        ) from exc
    return target


def export_adapter(
    adapter_dir: str | Path,
    *,
    config: Any,
    state_dict: Mapping[str, Any] | None = None,
    training: Mapping[str, Any] | None = None,
    metrics: Mapping[str, Any] | None = None,
    allow_manifest_only: bool = True,
) -> dict[str, Any]:
    """Write a PEFT adapter directory plus ``training_manifest.json``.

    Returns a result dict.  When weights cannot be written and
    ``allow_manifest_only`` is true, the result carries
    ``status="manifest_only"`` and a warning; otherwise an ``error`` object with
    ``code="missing_dependency"`` is returned (the manifests are still written).
    """

    directory = Path(adapter_dir)
    try:
        directory.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise ExportError(
            f"cannot create adapter directory {directory}: {exc}",
            code="io_error",
            data={"path": str(directory)},
        ) from exc

    adapter_config_path = write_json(directory / ADAPTER_CONFIG_FILE, _lora_config_dict(config))

    files: dict[str, Any] = {
        "adapter_config": adapter_config_path.name,
        "weights": None,
        "manifest": MANIFEST_FILE,
    }
    weights_path: Path | None = None
    warning: str | None = None
    if state_dict is not None:
        if not _safetensors_available():
            warning = (
                "torch/safetensors are not installed: wrote manifest only; "
                "adapter_model.safetensors was not created"
            )
            if not allow_manifest_only:
                write_json(
                    directory / MANIFEST_FILE,
                    adapter_manifest(config, training=training, metrics=metrics, files=files),
                )
                return {
                    "ok": False,
                    "error": {
                        "code": "missing_dependency",
                        "message": warning,
                        "missing": ["torch", "safetensors"],
                        "adapter_dir": str(directory),
                    },
                }
        else:
            weights_path = _write_safetensors(
                directory / ADAPTER_WEIGHTS_FILE, state_dict
            )
            files["weights"] = weights_path.name
    else:
        warning = "no state_dict provided: wrote manifest only (manifest_only export)"

    manifest_path = write_json(
        directory / MANIFEST_FILE,
        adapter_manifest(config, training=training, metrics=metrics, files=files),
    )

    result: dict[str, Any] = {
        "ok": True,
        "status": "safetensors" if weights_path else "manifest_only",
        "adapter_dir": str(directory),
        "files": {
            "adapter_config": str(adapter_config_path),
            "manifest": str(manifest_path),
            "weights": str(weights_path) if weights_path else None,
        },
        "warnings": [warning] if warning else [],
    }
    if warning:
        result["warning"] = warning
    return result


# ---------------------------------------------------------------------------
# External tool wrappers (llama.cpp)
# ---------------------------------------------------------------------------


def resolve_tool(
    tool: str | Sequence[str] | None,
    *,
    env_var: str | None = None,
    default: str | None = None,
) -> tuple[list[str] | None, str]:
    """Resolve a configured tool into an argv prefix.

    Returns ``(argv_prefix, searched)``; ``argv_prefix`` is ``None`` when the
    tool cannot be found.  ``tool`` may be a path, a bare executable name
    (resolved through ``PATH``) or a sequence such as
    ``[sys.executable, "convert_hf_to_gguf.py"]``.
    """

    configured: str | Sequence[str] | None = tool
    if configured is None and env_var:
        configured = os.environ.get(env_var) or None
    if configured is None:
        configured = default
    if configured is None:
        return None, "<none configured>"

    if isinstance(configured, str):
        candidate = configured.strip()
        if not candidate:
            return None, "<empty>"
        parts = [candidate]
    elif isinstance(configured, (list, tuple)):
        parts = [str(part) for part in configured]
        if not parts:
            return None, "<empty>"
    else:
        raise ExportError(
            f"tool must be a string or list of strings, got {type(configured).__name__}",
            code="invalid_argument",
        )

    primary = parts[0]
    expanded = os.path.expanduser(os.path.expandvars(primary))
    resolved: str | None = None
    if os.path.isfile(expanded) and os.access(expanded, os.X_OK):
        resolved = os.path.abspath(expanded)
    elif os.path.isfile(expanded):
        # On Windows scripts are not always marked executable; accept files.
        resolved = os.path.abspath(expanded)
    else:
        found = shutil.which(expanded)
        if found:
            resolved = found
    if resolved is None:
        return None, expanded
    return [resolved, *parts[1:]], expanded


def _tool_error(
    *,
    code: str,
    message: str,
    command: list[str] | None = None,
    data: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    error: dict[str, Any] = {"code": code, "message": message}
    if command:
        error["command"] = command
    if data:
        error.update(data)
    return {"ok": False, "error": error}


def run_tool(
    argv: Sequence[str],
    *,
    timeout: float = 3600.0,
    cwd: str | Path | None = None,
) -> dict[str, Any]:
    """Run an external command and return a structured result.

    Never raises for a missing binary, non-zero exit or timeout; all of those are
    represented as ``{"ok": False, "error": {...}}``.
    """

    command = [str(part) for part in argv]
    if not command:
        return _tool_error(code="invalid_argument", message="empty command")
    started = time.monotonic()
    try:
        completed = subprocess.run(  # noqa: S603 - explicit user configured tool
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            cwd=str(cwd) if cwd is not None else None,
            check=False,
        )
    except FileNotFoundError as exc:
        return _tool_error(
            code="tool_not_found",
            message=f"tool not found: {command[0]} ({exc})",
            command=command,
            data={"tool": command[0]},
        )
    except subprocess.TimeoutExpired:
        return _tool_error(
            code="timeout",
            message=f"tool timed out after {timeout:g}s: {command[0]}",
            command=command,
            data={"timeout_seconds": timeout},
        )
    except OSError as exc:
        return _tool_error(
            code="tool_error",
            message=f"cannot run tool {command[0]}: {exc}",
            command=command,
            data={"tool": command[0]},
        )
    duration_ms = int((time.monotonic() - started) * 1000)
    result: dict[str, Any] = {
        "ok": completed.returncode == 0,
        "command": command,
        "returncode": completed.returncode,
        "stdout": (completed.stdout or "").strip(),
        "stderr": (completed.stderr or "").strip(),
        "duration_ms": duration_ms,
    }
    if completed.returncode != 0:
        result["error"] = {
            "code": "tool_failed",
            "message": f"tool exited with code {completed.returncode}: {command[0]}",
            "returncode": completed.returncode,
        }
    return result


def merge_adapter(
    base_model: str,
    adapter_dir: str | Path,
    output_path: str | Path,
    *,
    tool: str | Sequence[str] | None = None,
    timeout: float = 3600.0,
    extra_args: Sequence[str] = (),
) -> dict[str, Any]:
    """Merge a PEFT adapter into the base model through an external tool.

    The default tool is ``llama-export-lora`` (or ``$LLMGUI_TRAIN_MERGE_TOOL``).
    Command layout::

        <tool> --base <base_model> --lora <adapter_dir> --output <output_path>
    """

    if not isinstance(base_model, str) or not base_model.strip():
        raise ExportError("base_model must be a non-empty string", code="invalid_argument")
    adapter_path = Path(adapter_dir)
    if not adapter_path.exists():
        raise ExportError(
            f"adapter directory not found: {adapter_path}",
            code="invalid_argument",
            data={"adapter_dir": str(adapter_path)},
        )
    output = Path(output_path)
    try:
        output.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise ExportError(
            f"cannot create output directory {output.parent}: {exc}", code="io_error"
        ) from exc

    argv_prefix, searched = resolve_tool(
        tool,
        env_var=DEFAULT_MERGE_TOOL_ENV,
        default=DEFAULT_MERGE_TOOL,
    )
    command_preview = [
        searched,
        "--base",
        base_model,
        "--lora",
        str(adapter_path),
        "--output",
        str(output),
        *extra_args,
    ]
    if argv_prefix is None:
        return _tool_error(
            code="tool_not_found",
            message=(
                f"merge tool not found: {searched!r}; configure --tool or "
                f"${DEFAULT_MERGE_TOOL_ENV} to point at a llama.cpp merge binary"
            ),
            command=command_preview,
            data={"tool": searched, "env_var": DEFAULT_MERGE_TOOL_ENV},
        )
    return run_tool(
        [
            *argv_prefix,
            "--base",
            base_model,
            "--lora",
            str(adapter_path),
            "--output",
            str(output),
            *extra_args,
        ],
        timeout=timeout,
    )


def export_gguf(
    model_dir: str | Path,
    output_path: str | Path,
    *,
    outtype: str = "f16",
    tool: str | Sequence[str] | None = None,
    timeout: float = 3600.0,
    extra_args: Sequence[str] = (),
) -> dict[str, Any]:
    """Convert a HF model/adapter directory to GGUF through llama.cpp.

    The default tool is ``convert_hf_to_gguf.py`` (or
    ``$LLMGUI_TRAIN_GGUF_TOOL``).  Set ``tool=[sys.executable, path_to_script]``
    when the script is not executable.  Command layout::

        <tool> --outfile <output_path> --outtype <outtype> <model_dir>
    """

    model_path = Path(model_dir)
    if not model_path.exists():
        raise ExportError(
            f"model directory not found: {model_path}",
            code="invalid_argument",
            data={"model_dir": str(model_path)},
        )
    output = Path(output_path)
    try:
        output.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        raise ExportError(
            f"cannot create output directory {output.parent}: {exc}", code="io_error"
        ) from exc

    argv_prefix, searched = resolve_tool(
        tool,
        env_var=DEFAULT_GGUF_TOOL_ENV,
        default=DEFAULT_GGUF_TOOL,
    )
    command_preview = [
        searched,
        "--outfile",
        str(output),
        "--outtype",
        outtype,
        str(model_path),
        *extra_args,
    ]
    if argv_prefix is None:
        return _tool_error(
            code="tool_not_found",
            message=(
                f"GGUF conversion tool not found: {searched!r}; configure --tool or "
                f"${DEFAULT_GGUF_TOOL_ENV} to point at llama.cpp's convert_hf_to_gguf.py"
            ),
            command=command_preview,
            data={"tool": searched, "env_var": DEFAULT_GGUF_TOOL_ENV},
        )
    return run_tool(
        [
            *argv_prefix,
            "--outfile",
            str(output),
            "--outtype",
            outtype,
            str(model_path),
            *extra_args,
        ],
        timeout=timeout,
    )


def export_error_to_rpc(exc: ExportError) -> dict[str, Any]:
    """Convert an :class:`ExportError` into a JSON-RPC error payload body."""

    return {
        "code": exc.code,
        "message": str(exc),
        **exc.data,
    }


__all__ = [
    "ADAPTER_CONFIG_FILE",
    "ADAPTER_WEIGHTS_FILE",
    "DEFAULT_GGUF_TOOL",
    "DEFAULT_GGUF_TOOL_ENV",
    "DEFAULT_MERGE_TOOL",
    "DEFAULT_MERGE_TOOL_ENV",
    "ExportError",
    "MANIFEST_FILE",
    "adapter_manifest",
    "export_adapter",
    "export_error_to_rpc",
    "export_gguf",
    "merge_adapter",
    "resolve_tool",
    "run_tool",
    "write_json",
]
