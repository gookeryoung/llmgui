"""Runtime capability detection for the training worker.

Detection is best effort and never raises: every probe is guarded so that the
module can be imported on a machine without torch/transformers/peft installed.
Heavy packages such as torch are only imported when the module is already
installed.
"""

from __future__ import annotations

import importlib.metadata
import importlib.util
import platform
import sys
import threading
from typing import Any

# Distribution name -> importable module name.
_PACKAGES: dict[str, str] = {
    "torch": "torch",
    "transformers": "transformers",
    "peft": "peft",
    "trl": "trl",
    "datasets": "datasets",
    "accelerate": "accelerate",
    "bitsandbytes": "bitsandbytes",
    "safetensors": "safetensors",
    "triton": "triton",
    "liger-kernel": "liger_kernel",
    "xformers": "xformers",
    "deepspeed": "deepspeed",
    "pyyaml": "yaml",
}

_TRAINING_REQUIRED = ("torch", "transformers", "peft")

_cache_lock = threading.Lock()
_cache: dict[str, Any] | None = None


def package_version(dist_name: str, module_name: str | None = None) -> dict[str, Any]:
    """Return ``{"installed": bool, "version": str | None}`` for one package."""

    module = module_name or dist_name.replace("-", "_")
    installed = False
    try:
        installed = importlib.util.find_spec(module) is not None
    except (ImportError, ValueError, ModuleNotFoundError):
        installed = False
    version: str | None = None
    if installed:
        try:
            version = importlib.metadata.version(dist_name)
        except importlib.metadata.PackageNotFoundError:
            version = None
        except Exception:  # pragma: no cover - defensive, metadata can be broken
            version = None
    return {"installed": installed, "version": version}


def _torch_info() -> dict[str, Any]:  # pragma: no cover - requires a torch runtime
    """Probe torch and its accelerator backends; never raises."""

    info: dict[str, Any] = {
        "installed": False,
        "version": None,
        "cuda": {"available": False, "version": None, "device_count": 0, "devices": []},
        "rocm": {"available": False, "version": None},
        "npu": {"available": False},
        "musa": {"available": False},
        "mps": {"available": False},
    }
    torch_info = package_version("torch")
    if not torch_info["installed"]:
        return info

    try:
        import torch  # type: ignore[import-not-found]
    except Exception as exc:  # pragma: no cover - broken install
        info["import_error"] = f"{type(exc).__name__}: {exc}"
        return info

    info["installed"] = True
    info["version"] = str(getattr(torch, "__version__", torch_info["version"] or "unknown"))

    try:
        cuda = torch.cuda
        info["cuda"]["available"] = bool(cuda.is_available())
        info["cuda"]["device_count"] = int(cuda.device_count())
        info["cuda"]["version"] = getattr(getattr(torch, "version", None), "cuda", None)
        devices: list[dict[str, Any]] = []
        if info["cuda"]["available"]:
            for index in range(info["cuda"]["device_count"]):
                try:
                    props = cuda.get_device_properties(index)
                    total = int(getattr(props, "total_memory", 0))
                    major = getattr(props, "major", None)
                    minor = getattr(props, "minor", None)
                    capability = (
                        f"{major}.{minor}" if major is not None and minor is not None else None
                    )
                    devices.append(
                        {
                            "index": index,
                            "name": str(getattr(props, "name", f"cuda:{index}")),
                            "total_memory_bytes": total,
                            "total_memory_gib": round(total / (1024**3), 2) if total else None,
                            "capability": capability,
                        }
                    )
                except Exception:  # pragma: no cover - driver edge cases
                    continue
        info["cuda"]["devices"] = devices
    except Exception as exc:  # pragma: no cover - defensive
        info["cuda"]["error"] = f"{type(exc).__name__}: {exc}"

    try:
        hip = getattr(getattr(torch, "version", None), "hip", None)
        info["rocm"]["available"] = bool(hip)
        info["rocm"]["version"] = hip
    except Exception:  # pragma: no cover - defensive
        pass

    try:
        info["mps"]["available"] = bool(getattr(torch.backends, "mps", None) and torch.backends.mps.is_available())
    except Exception:  # pragma: no cover - defensive
        pass

    # torch_npu / torch_musa are optional add-ons providing extra device types.
    for key, module_name in (("npu", "torch_npu"), ("musa", "torch_musa")):
        present = importlib.util.find_spec(module_name) is not None
        info[key]["module"] = module_name
        info[key]["installed"] = present
        info[key]["available"] = present and info["installed"]

    return info


def _accelerator_reports() -> dict[str, dict[str, Any]]:
    """Summarise hardware backends independently of torch."""

    npu_pkg = package_version("torch-npu", "torch_npu")
    musa_pkg = package_version("torch-musa", "torch_musa")
    return {
        "cuda": {
            "available": False,
            "kind": "cuda",
            "note": "determined from torch; see torch.cuda",
        },
        "rocm": {
            "available": False,
            "kind": "rocm",
            "note": "determined from torch; see torch.version.hip",
        },
        "npu": {
            "available": bool(npu_pkg["installed"]),
            "kind": "npu",
            "module": npu_pkg,
        },
        "musa": {
            "available": bool(musa_pkg["installed"]),
            "kind": "musa",
            "module": musa_pkg,
        },
    }


def detect(*, use_cache: bool = True) -> dict[str, Any]:
    """Return a JSON serialisable capability report.  Never raises."""

    global _cache
    if use_cache:
        with _cache_lock:
            if _cache is not None:
                return _cache

    packages = {name: package_version(name, module) for name, module in _PACKAGES.items()}
    try:
        torch_info = _torch_info()
    except Exception as exc:  # pragma: no cover - probes are defensive anyway
        torch_info = {
            "installed": False,
            "version": None,
            "cuda": {"available": False, "version": None, "device_count": 0, "devices": []},
            "rocm": {"available": False, "version": None},
            "npu": {"available": False},
            "musa": {"available": False},
            "mps": {"available": False},
            "probe_error": f"{type(exc).__name__}: {exc}",
        }
    accelerators = _accelerator_reports()
    accelerators["cuda"]["available"] = bool(torch_info.get("cuda", {}).get("available"))
    accelerators["cuda"]["devices"] = torch_info.get("cuda", {}).get("devices", [])
    accelerators["rocm"]["available"] = bool(torch_info.get("rocm", {}).get("available"))

    missing = [name for name in _TRAINING_REQUIRED if not packages[name]["installed"]]
    report: dict[str, Any] = {
        "schema": 1,
        "python": {
            "version": platform.python_version(),
            "implementation": platform.python_implementation(),
            "executable": sys.executable,
        },
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
        },
        "packages": packages,
        "torch": torch_info,
        "accelerators": accelerators,
        "quantization": {
            "bitsandbytes": bool(packages["bitsandbytes"]["installed"]),
            "bits": [4, 8] if packages["bitsandbytes"]["installed"] else [],
        },
        "kernels": {
            "triton": bool(packages["triton"]["installed"]),
            "liger_kernel": bool(packages["liger-kernel"]["installed"]),
            "xformers": bool(packages["xformers"]["installed"]),
            "flash_attention": None,
        },
        "training": {
            "ready": not missing,
            "missing": missing,
            "required": list(_TRAINING_REQUIRED),
            "optional": ["trl", "datasets", "accelerate", "safetensors", "bitsandbytes"],
        },
    }

    if use_cache:
        with _cache_lock:
            _cache = report
    return report


def reset_cache() -> None:
    """Clear the cached detection report (used by tests)."""

    global _cache
    with _cache_lock:
        _cache = None


def has_training_stack() -> bool:
    """True when torch/transformers/peft are importable."""

    return bool(detect()["training"]["ready"])


def summary_text() -> str:
    """Human readable one-liner used by the CLI."""

    report = detect()
    torch_info = report["torch"]
    if torch_info.get("installed"):
        device = "cuda" if torch_info.get("cuda", {}).get("available") else "cpu"
    else:
        device = "unavailable"
    return (
        f"python={report['python']['version']} torch={torch_info.get('version')} "
        f"device={device} training_ready={report['training']['ready']}"
    )
