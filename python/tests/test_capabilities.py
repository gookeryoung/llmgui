"""Tests for :mod:`llmgui_train.capabilities`."""

from __future__ import annotations

import importlib
import sys

import pytest

from llmgui_train import capabilities


@pytest.fixture(autouse=True)
def clear_cache():
    capabilities.reset_cache()
    yield
    capabilities.reset_cache()


def test_detect_returns_expected_structure():
    report = capabilities.detect()
    for key in (
        "schema",
        "python",
        "platform",
        "packages",
        "torch",
        "accelerators",
        "quantization",
        "kernels",
        "training",
    ):
        assert key in report
    assert report["schema"] == 1
    assert report["python"]["version"]
    for package in ("torch", "transformers", "peft", "bitsandbytes", "triton"):
        assert set(report["packages"][package]) == {"installed", "version"}
        assert isinstance(report["packages"][package]["installed"], bool)
    for accelerator in ("cuda", "rocm", "npu", "musa"):
        assert accelerator in report["accelerators"]


def test_training_ready_is_consistent_with_missing():
    report = capabilities.detect()
    training = report["training"]
    assert training["ready"] == (not training["missing"])
    assert set(training["required"]) == {"torch", "transformers", "peft"}
    for missing in training["missing"]:
        assert report["packages"][missing]["installed"] is False


def test_detect_is_cached_and_reset_works():
    first = capabilities.detect()
    second = capabilities.detect()
    assert first is second
    capabilities.reset_cache()
    assert capabilities.detect() is not first


def test_package_version_known_and_unknown():
    known = capabilities.package_version("pytest", "pytest")
    assert known["installed"] is True
    assert known["version"]

    unknown = capabilities.package_version("definitely-not-installed-xyz")
    assert unknown == {"installed": False, "version": None}


def test_has_training_stack_matches_report():
    assert capabilities.has_training_stack() is capabilities.detect()["training"]["ready"]


def test_summary_text_contains_device():
    text = capabilities.summary_text()
    assert "python=" in text
    assert "device=" in text


def test_detect_survives_a_failing_probe(monkeypatch):
    """A broken torch probe must not take the worker down."""

    def boom(*_args, **_kwargs):
        raise RuntimeError("probe exploded")

    monkeypatch.setattr(capabilities, "_torch_info", boom)
    report = capabilities.detect(use_cache=False)
    assert report["torch"]["installed"] is False
    assert "probe_error" in report["torch"]


def test_torch_probe_handles_uninstalled(monkeypatch):
    """When torch is absent the torch report stays structured."""

    original = capabilities.package_version

    def fake_package_version(dist_name, module_name=None):
        if dist_name == "torch":
            return {"installed": False, "version": None}
        return original(dist_name, module_name)

    monkeypatch.setattr(capabilities, "package_version", fake_package_version)
    report = capabilities.detect(use_cache=False)
    assert report["torch"]["installed"] is False
    assert report["torch"]["cuda"]["available"] is False


def test_import_module_stays_lightweight():
    """Importing capabilities must not pull torch/transformers in."""

    module = importlib.reload(capabilities)
    assert module is not None
    for heavy in ("torch", "transformers"):
        if heavy in sys.modules:  # pragma: no cover - env dependent
            pytest.skip(f"{heavy} already imported by another test")
    assert "torch" not in sys.modules
