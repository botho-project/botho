#!/usr/bin/env python3
"""Regenerate the docs/research sim tables and fail on drift (#1702).

Every deterministic `cluster-tax-sim` generator behind a `docs/research/*.md`
report that runs in seconds is listed in GENERATORS. For each one this script:

1. runs the generator and compares its full stdout with the committed golden
   under `docs/research/generated/<name>.txt` (any diff fails), and
2. for docs that paste the generator's markdown tables verbatim
   (`verbatim=True`), checks that every table the generator prints appears
   verbatim in the doc, so the published report cannot drift from the golden.

Docs whose tables are hand-reformatted (bolding, merged columns) are covered by
the golden only: a sim change shows up as a golden diff in review, which is the
prompt to update the prose.

Usage:
    check_research_tables.py --bin target/release/cluster-tax-sim
    check_research_tables.py --bin ... --update   # rewrite the goldens

SKIPPED lists the generators deliberately left out because they take minutes,
not seconds; run them by hand when their sims change.
"""

from __future__ import annotations

import argparse
import difflib
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
GOLDEN_DIR = REPO / "docs" / "research" / "generated"


@dataclass(frozen=True)
class Generator:
    name: str
    args: tuple[str, ...]
    doc: str
    verbatim: bool


GENERATORS = [
    Generator("bridge-import-sweep", ("bridge-import-sweep",),
              "docs/research/bridge-import-calibration.md", True),
    Generator("settlement-horizon-sweep", ("settlement-horizon-sweep",),
              "docs/research/settlement-horizon-calibration.md", True),
    Generator("honest-overcharge-sweep", ("honest-overcharge-sweep",),
              "docs/research/demurrage-downgrade-overcharge.md", True),
    Generator("lottery-reward-cap-sweep", ("lottery-reward-cap-sweep",),
              "docs/research/ct-compatible-lottery-selection.md", True),
    Generator("lottery-reward-cap-sweep-faithful", ("lottery-reward-cap-sweep", "--faithful"),
              "docs/research/ct-compatible-lottery-selection.md", True),
    Generator("lottery-selection-sweep", ("lottery-selection-sweep",),
              "docs/research/ct-compatible-lottery-selection.md", False),
    Generator("lottery-sybil-brake-sweep", ("lottery-sybil-brake-sweep",),
              "docs/research/ct-compatible-lottery-selection.md", False),
    Generator("provenance-factor-sweep", ("provenance-factor-sweep",),
              "docs/research/ct-provenance-factor-calibration.md", False),
] + [
    # consensus-fee-model-sim-verdicts.md summarizes these closed-form runs in
    # its own tables, so they are golden-only.
    Generator(f"{sim}-{model}", (sim, "--fee-model", model),
              "docs/research/consensus-fee-model-sim-verdicts.md", False)
    for sim in ("wash-trading", "structuring", "scenario-whale")
    for model in ("legacy", "consensus", "consensus-relay")
] + [
    Generator("wash-trading-height-1", ("wash-trading", "--height", "1"),
              "docs/research/consensus-fee-model-sim-verdicts.md", False),
    Generator("structuring-elapsed-1y", ("structuring", "--elapsed", "6307200"),
              "docs/research/consensus-fee-model-sim-verdicts.md", False),
]

# Too slow for a PR gate (timed on an Apple M-series release build; GitHub
# runners are slower). Their reports are not checked by CI.
SKIPPED = {
    "m2-production (m2-production-verdicts.md)": "> 200 s per run, several runs",
    "m2-cumulative / m2-decay (consensus-fee-model-sim-verdicts.md)": "~190 s per run",
    "lottery-sweep --seed 1695 (consensus-fee-model-sim-verdicts.md)": "~76 s per fee model",
    "decoy-flood-sweep (decoy-flood-cost.md)": "~49 s; constants pinned by botho/tests/decoy_flood_parity.rs",
}


def table_blocks(text: str) -> list[list[str]]:
    blocks: list[list[str]] = []
    cur: list[str] = []
    for line in text.splitlines():
        if line.lstrip().startswith("|"):
            cur.append(line.rstrip())
        elif cur:
            blocks.append(cur)
            cur = []
    if cur:
        blocks.append(cur)
    return blocks


def run(binary: Path, gen: Generator) -> str:
    result = subprocess.run(
        [str(binary), *gen.args], cwd=REPO, capture_output=True, text=True, timeout=300
    )
    if result.returncode != 0:
        raise SystemExit(
            f"{gen.name}: `{' '.join(gen.args)}` exited {result.returncode}\n{result.stderr}"
        )
    return result.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bin", required=True, type=Path, help="cluster-tax-sim binary")
    parser.add_argument("--update", action="store_true", help="rewrite the goldens")
    opts = parser.parse_args()

    failures: list[str] = []
    GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
    for gen in GENERATORS:
        start = time.monotonic()
        out = run(opts.bin, gen)
        elapsed = time.monotonic() - start
        golden = GOLDEN_DIR / f"{gen.name}.txt"
        if opts.update:
            golden.write_text(out)
        elif not golden.exists():
            failures.append(f"{gen.name}: missing golden {golden.relative_to(REPO)}")
        elif golden.read_text() != out:
            diff = "".join(difflib.unified_diff(
                golden.read_text().splitlines(keepends=True), out.splitlines(keepends=True),
                f"{golden.relative_to(REPO)} (committed)", f"{gen.name} (regenerated)"))
            failures.append(f"{gen.name}: output differs from the golden\n{diff}")

        missing = 0
        if gen.verbatim:
            doc_text = "\n".join(l.rstrip() for l in (REPO / gen.doc).read_text().splitlines())
            for block in table_blocks(out):
                if "\n".join(block) not in doc_text:
                    missing += 1
                    failures.append(
                        f"{gen.name}: table not found verbatim in {gen.doc}:\n"
                        + "\n".join(block))
        print(f"{gen.name:40s} {elapsed:5.1f}s"
              + (f"  verbatim tables missing: {missing}" if gen.verbatim else "  (golden only)"))

    print("\nSkipped as too slow for CI:")
    for name, why in SKIPPED.items():
        print(f"  {name}: {why}")

    if failures:
        print("\nFAIL: research tables drifted. Regenerate with --update, then update the docs.\n",
              file=sys.stderr)
        for failure in failures:
            print(failure + "\n", file=sys.stderr)
        return 1
    print("\nOK: research tables match their generators.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
