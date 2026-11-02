#!/usr/bin/env python
"""Validate ADR index and landing tables without external dependencies.

Checks:
1. `docs/adr/README.md` exists and indexes every `NNNN-*.md` ADR.
2. Each ADR has a status line (`Proposed` or `Accepted`).
3. Each ADR has section 18 "落地状态" with a table containing P0..P3 phases.
4. Every row marked `已落地`/`已完成` has non-empty code/test cells.
5. Related links from ADR to requirement/research files resolve.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ADR_DIR = ROOT / "docs" / "adr"
INDEX = ADR_DIR / "README.md"
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
PHASES = ("P0", "P1", "P2a", "P2b", "P3")


def fail(message: str) -> None:
    print(f"[adr-check] {message}")


def check_links(path: Path) -> int:
    text = path.read_text(encoding="utf-8")
    errors = 0
    for match in LINK_RE.finditer(text):
        target = match.group(1).split("#")[0]
        if not target or target.startswith(("http://", "https://", "mailto:")):
            continue
        if "NNNN" in target:
            continue
        resolved = (path.parent / target).resolve()
        if not resolved.exists():
            fail(f"{path.relative_to(ROOT)}: broken link {target}")
            errors += 1
    return errors


def main() -> int:
    if not INDEX.exists():
        fail("docs/adr/README.md is missing")
        return 1
    errors = 0
    index_text = INDEX.read_text(encoding="utf-8")
    adrs = sorted(ADR_DIR.glob("[0-9][0-9][0-9][0-9]-*.md"))
    if not adrs:
        fail("no ADR files found")
        return 1
    for adr in adrs:
        rel = adr.relative_to(ADR_DIR).as_posix()
        if rel not in index_text:
            fail(f"{adr.name}: not indexed in README.md")
            errors += 1
        text = adr.read_text(encoding="utf-8")
        if not re.search(r"状态：\s*(Proposed|Accepted)", text):
            fail(f"{adr.name}: missing Proposed/Accepted status")
            errors += 1
        if "## 18. 落地状态" not in text:
            fail(f"{adr.name}: missing 落地状态 section")
            errors += 1
            continue
        landing = text.split("## 18. 落地状态", 1)[1].split("\n## ", 1)[0]
        for phase in PHASES:
            if not re.search(rf"\|\s*{re.escape(phase)}\b", landing):
                fail(f"{adr.name}: landing table missing phase {phase}")
                errors += 1
        for line in landing.splitlines():
            if not line.startswith("|") or "已落地" not in line and "已完成" not in line:
                continue
            cells = [cell.strip() for cell in line.strip("|").split("|")]
            if len(cells) >= 5 and (cells[2] in {"", "—", "-"} or cells[3] in {"", "—", "-"}):
                fail(f"{adr.name}: phase {cells[0]} marked landed without code/test")
                errors += 1
        errors += check_links(adr)
    if errors:
        return 1
    print(f"[adr-check] ok ({len(adrs)} ADR, index consistent)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
