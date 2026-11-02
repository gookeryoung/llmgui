#!/usr/bin/env python
"""Fail when a required directory is empty or a .gitkeep is missing."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REQUIRED = [
    "crates",
    "python/llmgui_train/src/llmgui_train",
    "python/tests",
    "gui",
    "docs/adr",
    "docs/requirements",
    "docs/research",
]


def main() -> int:
    failed = False
    for relative in REQUIRED:
        path = ROOT / relative
        if not path.exists():
            print(f"[gitkeep-check] missing required directory: {relative}")
            failed = True
        elif path.is_dir() and not any(path.iterdir()):
            print(f"[gitkeep-check] empty directory: {relative}")
            failed = True
    if not failed:
        print("[gitkeep-check] ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
