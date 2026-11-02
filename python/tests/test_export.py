"""Tests for :mod:`llmgui_train.export`."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

from llmgui_train import export
from llmgui_train.config import parse_config


@pytest.fixture
def config():
    return parse_config(
        {
            "base_model": "Qwen2.5-7B-Instruct",
            "dataset": "data/train.jsonl",
            "method": "qlora",
            "lora_r": 8,
            "lora_alpha": 16,
            "target_modules": ["q_proj", "v_proj"],
        }
    )


def test_export_adapter_manifest_only(tmp_path: Path, config):
    adapter_dir = tmp_path / "adapter"
    result = export.export_adapter(adapter_dir, config=config, metrics={"loss": 0.42})

    assert result["ok"] is True
    assert result["status"] == "manifest_only"
    assert (adapter_dir / export.ADAPTER_CONFIG_FILE).exists()
    assert (adapter_dir / export.MANIFEST_FILE).exists()
    assert result["warnings"]

    adapter_config = json.loads((adapter_dir / export.ADAPTER_CONFIG_FILE).read_text())
    assert adapter_config["peft_type"] == "LORA"
    assert adapter_config["r"] == 8
    assert adapter_config["lora_alpha"] == 16
    assert adapter_config["target_modules"] == ["q_proj", "v_proj"]
    assert adapter_config["base_model_name_or_path"] == "Qwen2.5-7B-Instruct"

    manifest = json.loads((adapter_dir / export.MANIFEST_FILE).read_text())
    assert manifest["schema"] == export.MANIFEST_SCHEMA
    assert manifest["kind"] == "llmgui_train.adapter_manifest"
    assert manifest["base_model"] == "Qwen2.5-7B-Instruct"
    assert manifest["method"] == "qlora"
    assert manifest["metrics"] == {"loss": 0.42}
    assert manifest["dataset"]["path"] == "data/train.jsonl"
    assert manifest["framework"]["name"] == "llmgui-train"
    assert manifest["capabilities"]["training_ready"] in (True, False)


def test_export_adapter_requires_weights_when_torch_missing(tmp_path: Path, config):
    if export._safetensors_available():  # pragma: no cover - env dependent
        pytest.skip("torch + safetensors installed; manifest-only path not applicable")
    result = export.export_adapter(
        tmp_path / "adapter",
        config=config,
        state_dict={"w": object()},
        allow_manifest_only=False,
    )
    assert result["ok"] is False
    assert result["error"]["code"] == "missing_dependency"
    assert "torch" in result["error"]["missing"]
    # Manifests are still written for diagnosability.
    assert (tmp_path / "adapter" / export.MANIFEST_FILE).exists()


def test_export_manifest_from_mapping():
    manifest = export.adapter_manifest({"base_model": "m", "dataset": "d", "method": "lora"})
    assert manifest["base_model"] == "m"
    assert manifest["adapter"]["r"] == 16
    assert isinstance(manifest["created_at"], str)


def test_write_json_creates_parents(tmp_path: Path):
    target = export.write_json(tmp_path / "a" / "b" / "x.json", {"x": 1})
    assert json.loads(target.read_text()) == {"x": 1}


def test_merge_adapter_tool_not_found(tmp_path: Path):
    adapter_dir = tmp_path / "adapter"
    adapter_dir.mkdir()
    result = export.merge_adapter(
        "base-model",
        adapter_dir,
        tmp_path / "merged",
        tool=str(tmp_path / "missing-tool"),
    )
    assert result["ok"] is False
    assert result["error"]["code"] == "tool_not_found"
    assert result["error"]["env_var"] == export.DEFAULT_MERGE_TOOL_ENV


def test_merge_adapter_validates_adapter_dir(tmp_path: Path):
    with pytest.raises(export.ExportError) as info:
        export.merge_adapter("base", tmp_path / "nope", tmp_path / "out", tool="x")
    assert info.value.code == "invalid_argument"


def test_export_gguf_tool_not_found(tmp_path: Path):
    model_dir = tmp_path / "model"
    model_dir.mkdir()
    result = export.export_gguf(model_dir, tmp_path / "model.gguf", tool=str(tmp_path / "nope"))
    assert result["ok"] is False
    assert result["error"]["code"] == "tool_not_found"
    assert result["error"]["env_var"] == export.DEFAULT_GGUF_TOOL_ENV


def test_merge_adapter_with_fake_tool(tmp_path: Path):
    script = tmp_path / "fake_tool.py"
    log = tmp_path / "args.json"
    script.write_text(
        "import json, sys\n"
        "from pathlib import Path\n"
        "Path(sys.argv[1]).write_text(json.dumps(sys.argv[2:]))\n",
        encoding="utf-8",
    )
    adapter_dir = tmp_path / "adapter"
    adapter_dir.mkdir()
    result = export.merge_adapter(
        "base-model",
        adapter_dir,
        tmp_path / "out" / "merged",
        tool=[sys.executable, str(script), str(log)],
    )
    assert result["ok"] is True
    assert result["returncode"] == 0
    assert result["command"][0] == sys.executable
    recorded = json.loads(log.read_text())
    assert "--base" in recorded
    assert "--lora" in recorded
    assert "--output" in recorded


def test_run_tool_timeout(tmp_path: Path):
    script = tmp_path / "slow.py"
    script.write_text("import time\ntime.sleep(5)\n", encoding="utf-8")
    result = export.run_tool([sys.executable, str(script)], timeout=0.1)
    assert result["ok"] is False
    assert result["error"]["code"] == "timeout"


def test_run_tool_missing_binary(tmp_path: Path):
    result = export.run_tool([str(tmp_path / "nope-binary")])
    assert result["ok"] is False
    assert result["error"]["code"] == "tool_not_found"


def test_resolve_tool_env_var_and_sequence(tmp_path: Path, monkeypatch):
    script = tmp_path / "tool.py"
    script.write_text("print('hi')\n", encoding="utf-8")
    monkeypatch.setenv(export.DEFAULT_GGUF_TOOL_ENV, str(script))
    argv, searched = export.resolve_tool(None, env_var=export.DEFAULT_GGUF_TOOL_ENV)
    assert argv == [str(script)]
    assert searched == str(script)

    argv, _ = export.resolve_tool([sys.executable, str(script), "--flag"])
    assert argv[0] == sys.executable
    assert argv[1:] == [str(script), "--flag"]


def test_resolve_tool_none_and_empty():
    argv, searched = export.resolve_tool(None)
    assert argv is None
    assert searched == "<none configured>"
    argv, searched = export.resolve_tool("")
    assert argv is None and searched == "<empty>"


def test_export_error_to_rpc():
    error = export.ExportError("boom", code="tool_not_found", data={"tool": "x"})
    payload = error.to_dict()
    assert payload["ok"] is False
    assert payload["error"]["code"] == "tool_not_found"
    assert payload["error"]["tool"] == "x"
    assert export.export_error_to_rpc(error)["code"] == "tool_not_found"

def test_export_adapter_detects_existing_weights(tmp_path: Path, config):
    adapter_dir = tmp_path / "adapter"
    adapter_dir.mkdir()
    (adapter_dir / export.ADAPTER_WEIGHTS_FILE).write_bytes(b"fake-safetensors")
    result = export.export_adapter(adapter_dir, config=config, state_dict=None)
    assert result["ok"] is True
    assert result["status"] == "safetensors"
    assert result["files"]["weights"].endswith(export.ADAPTER_WEIGHTS_FILE)
    assert result["warnings"] == []
