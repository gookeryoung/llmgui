"""Shared fixtures for the llmgui_train test suite."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
PACKAGE_SRC = REPO_ROOT / "python" / "llmgui_train" / "src"


@pytest.fixture
def src_path() -> Path:
    """Absolute path to the package ``src`` directory (for subprocess tests)."""

    return PACKAGE_SRC


@pytest.fixture
def dataset_file(tmp_path: Path) -> Path:
    """A tiny valid Alpaca JSONL dataset."""

    path = tmp_path / "train.jsonl"
    rows = [
        {"instruction": "Say hi", "input": "", "output": "Hi!"},
        {"instruction": "Add one and two", "input": "1 + 2", "output": "Three."},
    ]
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    return path


@pytest.fixture
def config_dict(dataset_file: Path) -> dict:
    return {
        "base_model": "Qwen2.5-7B-Instruct",
        "dataset": str(dataset_file),
        "method": "qlora",
        "seq_len": 512,
        "micro_batch": 1,
        "grad_accum": 4,
        "lora_r": 16,
        "lora_alpha": 32,
        "output_dir": "outputs/test",
    }


@pytest.fixture
def config_file(tmp_path: Path, config_dict: dict) -> Path:
    path = tmp_path / "train.json"
    path.write_text(json.dumps(config_dict), encoding="utf-8")
    return path


@pytest.fixture
def pythonpath_env() -> dict[str, str]:
    """Environment for spawning ``python -m llmgui_train.*`` in subprocesses."""

    import os

    env = dict(os.environ)
    existing = env.get("PYTHONPATH", "")
    parts = [str(PACKAGE_SRC)] + ([existing] if existing else [])
    env["PYTHONPATH"] = os.pathsep.join(parts)
    return env


def has_full_training_stack() -> bool:
    import importlib.util

    return all(
        importlib.util.find_spec(name) is not None
        for name in ("torch", "transformers", "peft")
    )


def run_worker(
    lines: list[str],
    *,
    args: list[str] | None = None,
    env: dict[str, str],
    timeout: float = 60.0,
):
    """Run the worker as a subprocess and return the completed process."""

    import subprocess

    command = [sys.executable, "-m", "llmgui_train.worker", *(args or [])]
    payload = "\n".join(lines) + ("\n" if lines else "")
    return subprocess.run(
        command,
        input=payload,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=timeout,
        check=False,
    )


@pytest.fixture
def worker_runner(pythonpath_env):
    """Run ``python -m llmgui_train.worker`` with the given NDJSON lines."""

    def _run(lines: list[str], *, args: list[str] | None = None, timeout: float = 60.0):
        return run_worker(lines, args=args, env=pythonpath_env, timeout=timeout)

    return _run


def response_by_id(frames, request_id):
    for frame in frames:
        if frame.get("id") == request_id:
            return frame
    raise AssertionError(f"no response for id {request_id!r} in {frames}")


def notifications(frames, method: str):
    return [frame for frame in frames if frame.get("method") == method]
