"""Tests for ``python -m llmgui_train`` (the __main__ module)."""

from __future__ import annotations

import json
import subprocess
import sys


def test_python_m_llmgui_train_hello(pythonpath_env):
    process = subprocess.run(
        [sys.executable, "-m", "llmgui_train", "hello"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=pythonpath_env,
        timeout=60,
        check=False,
    )
    assert process.returncode == 0, process.stderr
    payload = json.loads(process.stdout)
    assert payload["server"] == "llmgui_train"


def test_python_m_llmgui_train_worker_version(pythonpath_env):
    process = subprocess.run(
        [sys.executable, "-m", "llmgui_train", "worker", "--help"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=pythonpath_env,
        timeout=60,
        check=False,
    )
    assert process.returncode == 0
    assert "stdio" in process.stdout.lower() or "--dry-run" in process.stdout
