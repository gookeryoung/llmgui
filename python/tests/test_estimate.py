"""Tests for :mod:`llmgui_train.estimate`."""

from __future__ import annotations

import pytest

from llmgui_train import estimate

GIB = 1024**3


def test_7b_qlora_fits_under_24gb():
    result = estimate.estimate_memory("7b", method="qlora", seq_len=2048, micro_batch=1)
    assert result["total_bytes"] < 24 * GIB
    assert result["fits"]["24gb"] is True
    assert result["total_gib"] < 24


def test_components_sum_to_total():
    result = estimate.estimate_memory("14b", method="qlora")
    components = result["components"]
    assert set(components) == {
        "weights_bytes",
        "lora_bytes",
        "activations_bytes",
        "logits_bytes",
        "context_bytes",
    }
    assert sum(components.values()) == result["total_bytes"]
    assert all(value > 0 for value in components.values())


def test_presets_scale_with_model_size():
    totals = [estimate.estimate_memory(name)["total_bytes"] for name in ("7b", "14b", "32b")]
    assert totals[0] < totals[1] < totals[2]
    assert estimate.estimate_memory("32b")["model"]["params_b"] == 32.0


def test_seq_len_and_batch_increase_memory():
    base = estimate.estimate_memory("7b", seq_len=1024, micro_batch=1)
    longer = estimate.estimate_memory("7b", seq_len=4096, micro_batch=1)
    batched = estimate.estimate_memory("7b", seq_len=1024, micro_batch=4)
    assert longer["total_bytes"] > base["total_bytes"]
    assert batched["total_bytes"] > base["total_bytes"]


def test_gradient_checkpointing_reduces_activations():
    with_gc = estimate.estimate_memory("7b", gradient_checkpointing=True)
    without_gc = estimate.estimate_memory("7b", gradient_checkpointing=False)
    assert with_gc["components"]["activations_bytes"] < without_gc["components"]["activations_bytes"]
    assert "gradient checkpointing" in " ".join(with_gc["notes"])


def test_qlora_is_smaller_than_lora():
    qlora = estimate.estimate_memory("7b", method="qlora", bits=4)
    lora = estimate.estimate_memory("7b", method="lora")
    assert qlora["components"]["weights_bytes"] < lora["components"]["weights_bytes"]
    assert qlora["total_bytes"] < lora["total_bytes"]
    assert lora["settings"]["bits"] is None


def test_custom_params_and_model_name_parsing():
    assert estimate.resolve_preset("Qwen2.5-7B-Instruct").name == "7b"
    assert estimate.model_size_from_name("Llama-3.1-8B") == "8b"
    assert estimate.model_size_from_name("some/random-model") is None
    custom = estimate.estimate_memory(params_b=1.0)
    assert custom["model"]["params_b"] == 1.0


def test_estimate_from_config(config_dict):
    from llmgui_train.config import parse_config

    result = estimate.estimate_from_config(parse_config(config_dict))
    assert result["settings"]["seq_len"] == 512
    assert result["settings"]["method"] == "qlora"
    assert result["total_gib"] > 0


def test_estimate_from_mapping_and_unknown_params():
    result = estimate.estimate_from_mapping({"model": "14b", "seq_len": 1024})
    assert result["settings"]["seq_len"] == 1024
    with pytest.raises(estimate.EstimateError, match="unknown"):
        estimate.estimate_from_mapping({"model": "7b", "bogus": 1})


@pytest.mark.parametrize(
    ("kwargs", "match"),
    [
        ({"model": "banana"}, "unknown model preset"),
        ({"model": "7b", "method": "magic"}, "method"),
        ({"model": "7b", "seq_len": 0}, "seq_len"),
        ({"model": "7b", "micro_batch": 0}, "micro_batch"),
        ({"model": "7b", "bits": 3}, "bits"),
        ({"model": "7b", "gradient_checkpointing": "yes"}, "gradient_checkpointing"),
        ({"model": "7b", "optimizer": "lbfgs"}, "optimizer"),
    ],
)
def test_invalid_estimates(kwargs, match):
    with pytest.raises(estimate.EstimateError, match=match):
        estimate.estimate_memory(**kwargs)


def test_format_report_contains_breakdown():
    report = estimate.format_report(estimate.estimate_memory("7b"))
    assert "weights:" in report
    assert "TOTAL:" in report
    assert "fits 24GB" in report
