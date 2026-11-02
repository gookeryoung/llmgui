"""Tests for :mod:`llmgui_train.config`."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from llmgui_train.config import (
    ConfigError,
    TrainConfig,
    load_config_file,
    load_train_config,
    parse_config,
)


def test_parse_minimal_config_defaults(config_dict):
    config = parse_config(config_dict)
    assert isinstance(config, TrainConfig)
    assert config.base_model == "Qwen2.5-7B-Instruct"
    assert config.method == "qlora"
    assert config.seq_len == 512
    assert config.quant_bits == 4
    assert config.output_dir == "outputs/test"
    assert config.target_modules == ()
    assert config.to_dict()["target_modules"] == []


def test_parse_aliases_and_nested_lora():
    config = parse_config(
        {
            "model_name_or_path": "some/model-7B",
            "dataset_path": "data/train.jsonl",
            "finetune_method": "LoRA",
            "max_seq_len": 1024,
            "micro_batch_size": 2,
            "gradient_accumulation_steps": 4,
            "learning_rate": 1e-4,
            "num_train_epochs": 3,
            "lora": {"r": 8, "alpha": 16, "dropout": 0.0, "target_modules": ["q_proj", "v_proj"]},
        }
    )
    assert config.method == "lora"
    assert config.quant_bits is None
    assert config.lora_r == 8
    assert config.lora_alpha == 16
    assert config.lora_dropout == 0.0
    assert config.target_modules == ("q_proj", "v_proj")
    assert config.seq_len == 1024
    assert config.micro_batch == 2
    assert config.grad_accum == 4
    assert config.lr == 1e-4
    assert config.epochs == 3.0


def test_unknown_keys_are_kept_in_extra(config_dict):
    config = parse_config({**config_dict, "flash_attention": True, "custom": {"a": 1}})
    assert config.extra == {"flash_attention": True, "custom": {"a": 1}}


@pytest.mark.parametrize(
    ("patch", "match"),
    [
        ({"base_model": ""}, "base_model"),
        ({"dataset": ""}, "dataset"),
        ({"method": "full"}, "method"),
        ({"format": "csv"}, "format"),
        ({"seq_len": 0}, "seq_len"),
        ({"seq_len": "2048"}, "seq_len"),
        ({"micro_batch": 0}, "micro_batch"),
        ({"grad_accum": -1}, "grad_accum"),
        ({"lora_r": 0}, "lora_r"),
        ({"lora_alpha": 0}, "lora_alpha"),
        ({"lora_dropout": 1.5}, "lora_dropout"),
        ({"quant_bits": 3}, "quant_bits"),
        ({"lr": 0}, "lr"),
        ({"warmup_ratio": 2.0}, "warmup_ratio"),
        ({"bf16": "yes"}, "bf16"),
        ({"target_modules": []}, "target_modules"),
        ({"max_steps": -5}, "max_steps"),
    ],
)
def test_invalid_config_fields(config_dict, patch, match):
    with pytest.raises(ConfigError, match=match):
        parse_config({**config_dict, **patch})


def test_load_in_4bit_false_conflicts_with_qlora(config_dict):
    with pytest.raises(ConfigError, match="load_in_4bit"):
        parse_config({**config_dict, "load_in_4bit": False})


def test_load_in_4bit_true_sets_four_bits(config_dict):
    config = parse_config({**config_dict, "load_in_4bit": True})
    assert config.quant_bits == 4


def test_parse_rejects_non_mapping():
    with pytest.raises(ConfigError, match="object"):
        parse_config(["not", "a", "config"])  # type: ignore[arg-type]


def test_load_json_config_and_relative_dataset(tmp_path: Path, dataset_file: Path, config_dict):
    nested = tmp_path / "nested"
    nested.mkdir()
    payload = {**config_dict, "dataset": "../" + dataset_file.name}
    config_path = nested / "train.json"
    config_path.write_text(json.dumps(payload), encoding="utf-8")

    config = load_train_config(config_path)
    assert Path(config.dataset) == dataset_file.resolve()


def test_load_train_config_missing_dataset(tmp_path: Path, config_dict):
    config_path = tmp_path / "train.json"
    config_path.write_text(
        json.dumps({**config_dict, "dataset": str(tmp_path / "missing.jsonl")}),
        encoding="utf-8",
    )
    with pytest.raises(ConfigError, match="dataset file not found"):
        load_train_config(config_path)


def test_load_train_config_check_paths_false(tmp_path: Path, config_dict):
    config_path = tmp_path / "train.json"
    config_path.write_text(
        json.dumps({**config_dict, "dataset": str(tmp_path / "missing.jsonl")}),
        encoding="utf-8",
    )
    config = load_train_config(config_path, check_paths=False)
    assert config.dataset.endswith("missing.jsonl")


def test_load_yaml_config(tmp_path: Path, dataset_file: Path):
    pytest.importorskip("yaml")
    config_path = tmp_path / "train.yaml"
    config_path.write_text(
        "\n".join(
            [
                "base_model: Qwen2.5-7B-Instruct",
                f"dataset: {dataset_file.as_posix()}",
                "method: qlora",
                "seq_len: 256",
                "lora:",
                "  r: 8",
                "  alpha: 16",
            ]
        ),
        encoding="utf-8",
    )
    config = load_train_config(config_path)
    assert config.seq_len == 256
    assert config.lora_r == 8


def test_load_config_file_rejects_unknown_extension(tmp_path: Path):
    path = tmp_path / "train.toml"
    path.write_text("base_model = 'x'", encoding="utf-8")
    with pytest.raises(ConfigError, match="unsupported config extension"):
        load_config_file(path)


def test_load_config_file_missing_file(tmp_path: Path):
    with pytest.raises(ConfigError, match="not found"):
        load_config_file(tmp_path / "nope.json")


def test_load_config_file_invalid_json(tmp_path: Path):
    path = tmp_path / "bad.json"
    path.write_text("{invalid", encoding="utf-8")
    with pytest.raises(ConfigError, match="invalid JSON"):
        load_config_file(path)


def test_yaml_without_pyyaml_has_clear_error(tmp_path: Path, monkeypatch):
    path = tmp_path / "train.yaml"
    path.write_text("base_model: x\n", encoding="utf-8")
    import builtins
    import sys

    original_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "yaml":
            raise ImportError("No module named 'yaml'")
        return original_import(name, *args, **kwargs)

    monkeypatch.delitem(sys.modules, "yaml", raising=False)
    monkeypatch.setattr(builtins, "__import__", fake_import)
    with pytest.raises(ConfigError, match="PyYAML"):
        load_config_file(path)


def test_target_modules_string_and_empty():
    config = parse_config(
        {
            "base_model": "m",
            "dataset": "d.jsonl",
            "target_modules": "q_proj, v_proj",
        }
    )
    assert config.target_modules == ("q_proj", "v_proj")
    with pytest.raises(ConfigError, match="target_modules"):
        parse_config(
            {
                "base_model": "m",
                "dataset": "d.jsonl",
                "target_modules": ["", "  "],
            }
        )
