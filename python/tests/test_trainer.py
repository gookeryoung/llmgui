"""Tests for :mod:`llmgui_train.trainer` (dependency-light paths only)."""

from __future__ import annotations

import sys

import pytest

from llmgui_train import trainer


def test_available_and_missing_consistency():
    availability = trainer.available()
    for package in (*trainer.REQUIRED_PACKAGES, *trainer.OPTIONAL_PACKAGES):
        assert package in availability
        assert isinstance(availability[package], bool)
    assert trainer.missing_dependencies() == [
        package for package in trainer.REQUIRED_PACKAGES if not availability[package]
    ]
    assert trainer.training_ready() == (not trainer.missing_dependencies())


def test_require_training_dependencies_raises_with_details(monkeypatch):
    monkeypatch.setattr(trainer, "_installed", lambda package: False)
    monkeypatch.setattr(trainer, "missing_dependencies", lambda: ["torch", "peft"])
    with pytest.raises(trainer.MissingDependency) as info:
        trainer.require_training_dependencies()
    error = info.value
    assert error.missing == ["torch", "peft"]
    assert error.to_dict()["code"] == "missing_dependency"
    assert payload_for(error)["hint"]


def payload_for(error: trainer.MissingDependency) -> dict:
    return error.to_dict()


def test_require_quantization_adds_bitsandbytes(monkeypatch):
    monkeypatch.setattr(trainer, "missing_dependencies", lambda: [])
    monkeypatch.setattr(trainer, "_installed", lambda package: package != "bitsandbytes")
    with pytest.raises(trainer.MissingDependency) as info:
        trainer.require_training_dependencies(quantization=True)
    assert info.value.missing == ["bitsandbytes"]


def test_missing_dependency_message_mentions_hint():
    error = trainer.MissingDependency(["torch"])
    assert "torch" in str(error)
    assert "install" in str(error).lower()


def test_training_cancelled_has_code():
    assert trainer.TrainingCancelled.code == "training_cancelled"
    assert str(trainer.TrainingCancelled("stopped")) == "stopped"


def test_train_report_to_dict():
    report = trainer.TrainReport(
        adapter_dir="out",
        steps=3,
        epochs=1.0,
        metrics={"loss": 0.1},
        export={"ok": True},
    )
    payload = report.to_dict()
    assert payload["ok"] is True
    assert payload["steps"] == 3
    assert payload["metrics"] == {"loss": 0.1}


def test_target_modules_defaults_and_override(config_dict):
    from llmgui_train.config import parse_config

    config = parse_config(config_dict)
    assert trainer._target_modules(config) is None
    config = parse_config({**config_dict, "target_modules": ["q_proj"]})
    assert trainer._target_modules(config) == ["q_proj"]


def test_lr_schedule():
    assert trainer._lr_at(0, 0, 1e-4, 0.0) == 1e-4
    # Warmup.
    assert trainer._lr_at(0, 100, 1e-4, 0.1) == pytest.approx(1e-5)
    assert trainer._lr_at(4, 100, 1e-4, 0.1) == pytest.approx(5e-5)
    # Cosine decay at the end reaches zero.
    assert trainer._lr_at(100, 100, 1e-4, 0.0) == pytest.approx(0.0)
    # Warmup longer than total steps clamps to base lr.
    assert trainer._lr_at(10, 5, 1e-4, 1.0) == pytest.approx(1e-4)


def test_emit_callback():
    events = []
    trainer._emit(events.append, kind="x", value=1)
    assert events == [{"kind": "x", "value": 1}]
    trainer._emit(None, kind="ignored")


@pytest.mark.skipif(
    not trainer.missing_dependencies(), reason="full training stack installed"
)
def test_train_raises_missing_dependency_before_imports(config_dict):
    from llmgui_train.config import parse_config

    config = parse_config(config_dict)
    with pytest.raises(trainer.MissingDependency):
        trainer.train(config)


def test_import_does_not_load_torch():
    assert "torch" not in sys.modules or trainer.missing_dependencies() == []
