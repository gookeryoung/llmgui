"""Extra edge case coverage across the llmgui_train modules."""

from __future__ import annotations

import importlib.metadata
import json
from pathlib import Path

import pytest

from llmgui_train import capabilities, config, data, estimate, export, protocol

# ---------------------------------------------------------------------------
# data edge cases
# ---------------------------------------------------------------------------


def test_example_helpers():
    example = data.Example(
        messages=[data.Message("user", "q"), data.Message("assistant", "a")],
        metadata={"source": "test"},
    )
    assert example.roles == ["user", "assistant"]
    assert [message.content for message in example.assistant_messages] == ["a"]
    payload = example.to_dict()
    assert payload["messages"][1] == {"role": "assistant", "content": "a"}
    assert payload["metadata"] == {"source": "test"}
    assert "text" not in payload


def test_message_from_dict_errors():
    with pytest.raises(data.DataError, match="role"):
        data.Message.from_dict({"role": 1, "content": "x"})
    with pytest.raises(data.DataError, match="role"):
        data.Message.from_dict({"role": "wizard", "content": "x"})


@pytest.mark.parametrize(
    ("content", "expected"),
    [
        (None, ""),
        (42, "42"),
        (True, "True"),
        ({"text": "from dict"}, "from dict"),
        ([{"type": "text", "text": "a"}, "b"], "ab"),
    ],
)
def test_content_conversion(content, expected):
    assert data._content_to_text(content) == expected


def test_content_conversion_errors():
    with pytest.raises(data.DataError, match="part"):
        data._content_to_text([42])
    with pytest.raises(data.DataError, match="content type"):
        data._content_to_text(object())


def test_openai_skips_empty_non_assistant_and_allows_empty_assistant():
    example = data.example_from_raw(
        {
            "messages": [
                {"role": "system", "content": ""},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": ""},
            ]
        }
    )
    assert [message.role for message in example.messages] == ["user", "assistant"]


def test_sharegpt_skips_empty_turns():
    example = data.example_from_raw(
        {
            "conversations": [
                {"from": "human", "value": "hi"},
                {"from": "gpt", "value": "   "},
                {"from": "gpt", "value": "hello"},
            ]
        }
    )
    assert example.messages[-1].content == "hello"


def test_render_chat_unknown_role():
    example = data.Example(messages=[data.Message("tool", "result")])
    text = data.render_text(example)
    assert "<|tool|>" in text


def test_assistant_token_mask_empty_offsets_and_zero_length():
    assert data.assistant_token_mask([(0, 2)], []) == []
    assert data.assistant_token_mask([(0, 2)], [(0, 0)]) == [0]
    assert data.assistant_token_mask([], [(0, 2)]) == [0]


def test_load_dataset_json_variants(tmp_path: Path):
    wrapped = tmp_path / "wrapped.json"
    wrapped.write_text(json.dumps({"data": [{"instruction": "a", "output": "b"}]}))
    assert len(data.load_dataset(wrapped)) == 1

    invalid = tmp_path / "invalid.json"
    invalid.write_text('{"not": "a list"}')
    with pytest.raises(data.DataError, match="array"):
        data.load_dataset(invalid)

    bad_json = tmp_path / "bad.json"
    bad_json.write_text("{oops")
    with pytest.raises(data.DataError, match="invalid JSON"):
        data.load_dataset(bad_json)


def test_load_dataset_negative_limit_and_bad_row(tmp_path: Path):
    path = tmp_path / "rows.jsonl"
    path.write_text(json.dumps({"instruction": "a", "output": "b"}) + "\n", encoding="utf-8")
    with pytest.raises(data.DataError, match="limit"):
        data.load_dataset(path, limit=-1)

    bad = tmp_path / "bad.jsonl"
    bad.write_text('[1, 2]\n', encoding="utf-8")
    with pytest.raises(data.DataError, match="JSON object"):
        data.load_dataset(bad)


def test_iter_examples_limit_json(tmp_path: Path, dataset_file: Path):
    assert len(list(data.iter_examples(dataset_file, limit=1))) == 1
    with pytest.raises(data.DataError, match="not found"):
        list(data.iter_examples(tmp_path / "missing.jsonl"))


def test_packed_structures_to_dict():
    packed = data.pack_sequences([[1, 2], [3]], seq_len=4)
    payload = packed.to_dict(include_attention_mask=False)
    assert payload["chunks"][0]["segment_ids"] == [0, 0, 1, -1]
    assert "attention_mask" not in payload["chunks"][0]
    empty_example = data.Example(messages=[data.Message("user", "x")])
    assert data.assistant_mask_char_spans(empty_example) == []


# ---------------------------------------------------------------------------
# config edge cases
# ---------------------------------------------------------------------------


def test_config_bool_and_optional_fields(tmp_path: Path):
    config_obj = config.parse_config(
        {
            "base_model": "m",
            "dataset": "d.jsonl",
            "bf16": False,
            "gradient_checkpointing": False,
            "mask_prompt": False,
            "seed": 7,
            "logging_steps": 2,
            "save_steps": 5,
            "max_steps": 10,
            "weight_decay": 0.01,
            "max_grad_norm": 0.5,
            "warmup_ratio": 0.05,
            "resume_from": "checkpoint-10",
            "epochs": 0.5,
        }
    )
    assert config_obj.bf16 is False
    assert config_obj.gradient_checkpointing is False
    assert config_obj.mask_prompt is False
    assert config_obj.resume_from == "checkpoint-10"
    assert config_obj.epochs == 0.5
    assert config_obj.to_dict()["resume_from"] == "checkpoint-10"


@pytest.mark.parametrize(
    ("raw", "match"),
    [
        ({"base_model": 1, "dataset": "d"}, "base_model"),
        ({"base_model": "m", "dataset": 1}, "dataset"),
        ({"base_model": "m", "dataset": "d", "method": 1}, "method"),
        ({"base_model": "m", "dataset": "d", "format": 1}, "format"),
        ({"base_model": "m", "dataset": "d", "seq_len": 2.5}, "seq_len"),
        ({"base_model": "m", "dataset": "d", "seq_len": 200000}, "seq_len"),
        ({"base_model": "m", "dataset": "d", "lora_dropout": -0.1}, "lora_dropout"),
        ({"base_model": "m", "dataset": "d", "warmup_ratio": -1}, "warmup_ratio"),
        ({"base_model": "m", "dataset": "d", "target_modules": 1}, "target_modules"),
        ({"base_model": "m", "dataset": "d", "target_modules": [1]}, "target_modules"),
        ({"base_model": "m", "dataset": "d", "resume_from": 1}, "resume_from"),
    ],
)
def test_more_invalid_config(raw, match):
    with pytest.raises(config.ConfigError, match=match):
        config.parse_config(raw)


def test_dataset_path_must_be_file(tmp_path: Path):
    directory = tmp_path / "data"
    directory.mkdir()
    with pytest.raises(config.ConfigError, match="not a file"):
        config.parse_config(
            {"base_model": "m", "dataset": str(directory)}, check_paths=True
        )


def test_load_empty_and_invalid_yaml(tmp_path: Path):
    pytest.importorskip("yaml")
    empty = tmp_path / "empty.yaml"
    empty.write_text("", encoding="utf-8")
    with pytest.raises(config.ConfigError, match="empty"):
        config.load_config_file(empty)

    invalid = tmp_path / "invalid.yaml"
    invalid.write_text("a: [1, 2\n", encoding="utf-8")
    with pytest.raises(config.ConfigError, match="invalid YAML"):
        config.load_config_file(invalid)


def test_load_json_not_an_object(tmp_path: Path):
    path = tmp_path / "list.json"
    path.write_text("[1, 2]", encoding="utf-8")
    with pytest.raises(config.ConfigError, match="object"):
        config.load_config_file(path)


def test_config_path_is_directory(tmp_path: Path):
    with pytest.raises(config.ConfigError, match="not a file"):
        config.load_config_file(tmp_path)


# ---------------------------------------------------------------------------
# estimate edge cases
# ---------------------------------------------------------------------------


def test_resolve_preset_sizes():
    assert estimate.resolve_preset("70b").params_b == 70.0
    assert estimate.resolve_preset("0.5b").params_b == 0.5
    assert estimate.resolve_preset("32b").layers == 64
    assert estimate.resolve_preset(params_b=2.5).name == "2.5b"


def test_estimate_options_and_budget():
    result = estimate.estimate_memory(
        "7b",
        method="lora",
        bits=8,
        grad_accum=2,
        optimizer="adafactor",
        cpu_offload=True,
        total_budget_gb=8.0,
        lora_alpha=64,
        target_modules=("q_proj",),
        dtype_bytes=4,
    )
    assert result["settings"]["lora_alpha"] == 64
    assert result["settings"]["target_modules"] == ["q_proj"]
    assert "8gb" in result["fits"]
    assert "cpu_offload" in " ".join(result["notes"])

    eight_bit = estimate.estimate_memory("7b", method="qlora", bits=8)
    four_bit = estimate.estimate_memory("7b", method="qlora", bits=4)
    assert eight_bit["components"]["weights_bytes"] > four_bit["components"]["weights_bytes"]


@pytest.mark.parametrize(
    ("kwargs", "match"),
    [
        ({"params_b": 0}, "params_b"),
        ({"params_b": True}, "params_b"),
        ({"lora_r": 0}, "lora_r"),
        ({"grad_accum": 0}, "grad_accum"),
        ({"dtype_bytes": 0}, "dtype_bytes"),
        ({"lora_alpha": 0}, "lora_alpha"),
        ({"model": "7b", "method": 1}, "method"),
    ],
)
def test_more_invalid_estimates(kwargs, match):
    with pytest.raises(estimate.EstimateError, match=match):
        estimate.estimate_memory(**kwargs)


def test_estimate_mapping_target_modules_validation():
    with pytest.raises(estimate.EstimateError, match="target_modules"):
        estimate.estimate_from_mapping({"target_modules": "q_proj"})
    result = estimate.estimate_from_mapping({"target_modules": ["q_proj"]})
    assert result["settings"]["target_modules"] == ["q_proj"]


def test_human_bytes_units():
    assert estimate._human(512).endswith("B")
    assert estimate._human(1024).endswith("KiB")
    assert estimate._human(5 * 1024**2).endswith("MiB")
    assert estimate._human(5 * 1024**3).endswith("GiB")


# ---------------------------------------------------------------------------
# export edge cases
# ---------------------------------------------------------------------------


def test_resolve_tool_rejects_bad_type():
    with pytest.raises(export.ExportError, match="tool"):
        export.resolve_tool(42)  # type: ignore[arg-type]


def test_run_tool_empty_command():
    result = export.run_tool([])
    assert result["ok"] is False
    assert result["error"]["code"] == "invalid_argument"


def test_export_gguf_validates_model_dir(tmp_path: Path):
    with pytest.raises(export.ExportError, match="model directory"):
        export.export_gguf(tmp_path / "nope", tmp_path / "x.gguf", tool="x")


def test_write_json_reports_io_error(monkeypatch, tmp_path: Path):
    def boom(*_args, **_kwargs):
        raise OSError("disk full")

    monkeypatch.setattr(Path, "write_text", boom)
    with pytest.raises(export.ExportError) as info:
        export.write_json(tmp_path / "x.json", {"a": 1})
    assert info.value.code == "io_error"


def test_adapter_manifest_with_training_metadata():
    config_obj = config.parse_config({"base_model": "m", "dataset": "d.jsonl"})
    manifest = export.adapter_manifest(
        config_obj,
        training={"steps": 3, "dataset": {"examples": 10}},
        metrics={"loss": 0.2},
        files={"weights": None},
        created_at="2024-01-01T00:00:00+00:00",
    )
    assert manifest["dataset"]["examples"] == 10
    assert manifest["created_at"] == "2024-01-01T00:00:00+00:00"
    assert manifest["training"]["steps"] == 3


def test_export_adapter_with_state_dict_manifest_only(tmp_path: Path):
    if export._safetensors_available():  # pragma: no cover - env dependent
        pytest.skip("torch + safetensors installed")
    config_obj = config.parse_config({"base_model": "m", "dataset": "d.jsonl"})
    result = export.export_adapter(
        tmp_path / "adapter", config=config_obj, state_dict={"w": object()}
    )
    assert result["ok"] is True
    assert result["status"] == "manifest_only"
    assert "safetensors" in result["warning"]


# ---------------------------------------------------------------------------
# capabilities / protocol edge cases
# ---------------------------------------------------------------------------


def test_package_version_handles_broken_metadata(monkeypatch):
    monkeypatch.setattr(importlib.metadata, "version", lambda _name: (_ for _ in ()).throw(RuntimeError("broken")))
    info = capabilities.package_version("torch", "sys")
    assert info["installed"] is True
    assert info["version"] is None


def test_package_version_handles_missing_module(monkeypatch):
    def boom(*_args, **_kwargs):
        raise ImportError("nope")

    monkeypatch.setattr(capabilities.importlib.util, "find_spec", boom)
    assert capabilities.package_version("torch")["installed"] is False


def test_parse_frame_invalid_utf8():
    with pytest.raises(protocol.RpcError) as info:
        protocol.parse_frame(b"\xff\xfe")
    assert info.value.code == protocol.ErrorCode.PARSE_ERROR


def test_validate_request_null_params_and_id():
    request = protocol.validate_request({"method": "x", "id": None, "params": None})
    assert request.params == {}
    assert request.has_id is True
    assert request.id is None
