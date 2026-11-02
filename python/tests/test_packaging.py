"""Guardrails for the console scripts declared in the root ``pyproject.toml``."""

from __future__ import annotations

import importlib
import tomllib
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
PYPROJECT = REPO_ROOT / "pyproject.toml"

EXPECTED_SCRIPTS = {
    "llmgui-train": "llmgui_train.cli:main",
    "llmgui-train-worker": "llmgui_train.worker:main",
}


def _project() -> dict[str, Any]:
    return tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["project"]


def test_console_scripts_point_at_callables():
    scripts = _project()["scripts"]
    for name, target in EXPECTED_SCRIPTS.items():
        assert scripts.get(name) == target, name
    for target in scripts.values():
        module_name, attribute = target.split(":", 1)
        module = importlib.import_module(module_name)
        assert callable(getattr(module, attribute)), target


def test_wheel_ships_the_training_package():
    config = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
    packages = config["tool"]["hatch"]["build"]["targets"]["wheel"]["packages"]
    assert packages, "wheel must declare packages"
    for package in packages:
        assert (REPO_ROOT / package / "__init__.py").is_file(), package


def test_optional_dependencies_cover_training_stacks():
    extras = _project()["optional-dependencies"]
    assert any(dependency.startswith("torch") for dependency in extras["train"])
    assert any(dependency.startswith("bitsandbytes") for dependency in extras["cuda"])
