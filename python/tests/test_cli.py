"""Tests for the ``llmgui-train`` command line interface."""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

from conftest import has_full_training_stack
from llmgui_train import cli


def test_cli_hello(capsys):
    assert cli.main(["hello"]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["server"] == "llmgui_train"
    assert payload["protocol"] == 1


def test_cli_capabilities(capsys):
    assert cli.main(["capabilities"]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert "packages" in payload


def test_cli_estimate_text(capsys):
    assert cli.main(["estimate", "--model", "7b", "--seq-len", "1024"]) == 0
    out = capsys.readouterr().out
    assert "TOTAL:" in out
    assert "fits 24GB" in out


def test_cli_estimate_json(capsys):
    assert cli.main(["estimate", "--model", "14b", "--json"]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["model"]["name"] == "14b"


def test_cli_estimate_invalid(capsys):
    assert cli.main(["estimate", "--model", "banana"]) == 2
    assert "estimate error" in capsys.readouterr().err


def test_cli_train_config_error(capsys, tmp_path: Path):
    assert cli.main(["train", "--config", str(tmp_path / "nope.json")]) == 2
    assert "config error" in capsys.readouterr().err


@pytest.mark.skipif(
    has_full_training_stack(), reason="full training stack installed"
)
def test_cli_train_missing_deps(capsys, config_file: Path):
    assert cli.main(["train", "--config", str(config_file)]) == 3
    assert "missing Python training dependencies" in capsys.readouterr().err


def test_cli_export_gguf_tool_missing(capsys, tmp_path: Path):
    model_dir = tmp_path / "model"
    model_dir.mkdir()
    code = cli.main(
        [
            "export-gguf",
            "--model-dir",
            str(model_dir),
            "--output",
            str(tmp_path / "m.gguf"),
            "--tool",
            str(tmp_path / "missing-tool"),
        ]
    )
    assert code == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["error"]["code"] == "tool_not_found"


def test_cli_merge_invalid_adapter(capsys, tmp_path: Path):
    code = cli.main(
        [
            "merge",
            "--base-model",
            "base",
            "--adapter-dir",
            str(tmp_path / "nope"),
            "--output",
            str(tmp_path / "out"),
        ]
    )
    assert code == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["error"]["code"] == "invalid_argument"


def test_cli_merge_tool_missing(capsys, tmp_path: Path):
    adapter_dir = tmp_path / "adapter"
    adapter_dir.mkdir()
    code = cli.main(
        [
            "merge",
            "--base-model",
            "base",
            "--adapter-dir",
            str(adapter_dir),
            "--output",
            str(tmp_path / "out"),
            "--tool",
            str(tmp_path / "missing-tool"),
        ]
    )
    assert code == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["error"]["code"] == "tool_not_found"


def test_cli_worker_eof(monkeypatch):
    monkeypatch.setattr(sys, "stdin", io.StringIO(""))
    stdout = io.StringIO()
    monkeypatch.setattr(sys, "stdout", stdout)
    assert cli.main(["worker"]) == 0
    assert stdout.getvalue() == ""


def test_cli_version_exits_zero(capsys):
    with pytest.raises(SystemExit) as info:
        cli.main(["--version"])
    assert info.value.code == 0
    assert "llmgui-train" in capsys.readouterr().out


def test_cli_requires_subcommand():
    with pytest.raises(SystemExit) as info:
        cli.main([])
    assert info.value.code == 2
