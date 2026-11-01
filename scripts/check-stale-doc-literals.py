#!/usr/bin/env python3
"""Fail when docs repeat known-stale protocol literals (#1702).

Each pattern below was true of an earlier protocol and is wrong now. A doc
that still states it as current misleads readers. The lint scans tracked
Markdown and LaTeX files.

Historical contexts are allowed:
- whole paths in ALLOWED_PATHS (changelogs, ADRs, audit/review records,
  vendored and tooling docs);
- any line carrying, or directly preceded by a line carrying, the marker
  `stale-literal-ok` with a reason, e.g.
  `<!-- stale-literal-ok: quotes the pre-#1700 claim -->`.

Usage: scripts/check-stale-doc-literals.py [--list]
"""

from __future__ import annotations

import fnmatch
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

# (regex, what is true now)
STALE = [
    (re.compile(r"\bring[- ]size(?: of)? 11\b", re.I),
     "consensus ring size is 20 (MIN_RING_SIZE)"),
    (re.compile(r"\b62 ?KB\b", re.I),
     "a ring-20 CLSAG signature is 736 B, not 62 KB"),
    (re.compile(r"\b0\.25 BTH per output\b", re.I),
     "0.25 BTH is the lottery Sybil-cap base fee, not a per-output fee"),
    (re.compile(r"\bLOTTERY_SEED_V1\b"),
     "protocol 7 draws use the V2 seed (LOTTERY_SEED_V2_DOMAIN)"),
]

MARKER = "stale-literal-ok"

ALLOWED_PATHS = [
    "CHANGELOG.md",
    "**/CHANGELOG.md",
    "**/changelog.md",
    "docs/decisions/*",
    "audits/*",
    "**/*.audit/*",
    "**/*.review/*",
    "vendor/*",
    ".anvil/*",
    ".loom/*",
    ".claude/*",
    ".agents/*",
]


def tracked_docs() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.md", "*.tex"],
        cwd=REPO, check=True, capture_output=True, text=True,
    ).stdout
    return [p for p in out.split("\0") if p]


def allowed(path: str) -> bool:
    return any(fnmatch.fnmatch(path, pat) for pat in ALLOWED_PATHS)


def main() -> int:
    list_only = "--list" in sys.argv[1:]
    hits: list[str] = []
    for rel in tracked_docs():
        if allowed(rel):
            continue
        path = REPO / rel
        if not path.is_file():
            continue
        lines = path.read_text(errors="replace").splitlines()
        for i, line in enumerate(lines):
            for pattern, truth in STALE:
                m = pattern.search(line)
                if not m:
                    continue
                if MARKER in line or (i > 0 and MARKER in lines[i - 1]):
                    continue
                hits.append(f"{rel}:{i + 1}: '{m.group(0)}' is stale: {truth}")
    if hits:
        print("\n".join(hits))
        if not list_only:
            print(
                f"\n{len(hits)} stale literal(s). Fix the text, or, if the context is "
                f"historical, add `<!-- {MARKER}: <reason> -->` on or above the line.",
                file=sys.stderr,
            )
            return 1
    else:
        print("OK: no stale doc literals.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
