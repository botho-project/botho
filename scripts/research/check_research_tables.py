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
import hashlib
import json
import os
import shlex
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
    Generator("ring-floor-dilution-sweep",
              ("honest-overcharge-sweep", "--adversarial-dilution"),
              "docs/research/ring-floor-dilution-sweep.md", True),
    Generator("demurrage-quantization-sweep", ("demurrage-quantization-sweep",),
              "docs/research/demurrage-quantization-study.md", True),
    Generator("lottery-reward-cap-sweep", ("lottery-reward-cap-sweep",),
              "docs/research/ct-compatible-lottery-selection.md", True),
    Generator("lottery-reward-cap-sweep-faithful", ("lottery-reward-cap-sweep", "--faithful"),
              "docs/research/ct-compatible-lottery-selection.md", True),
    Generator("redistribution-redesign-splitting",
              ("redistribution-redesign", "--part", "splitting"),
              "docs/research/redistribution-redesign-study.md", True),
    Generator("redistribution-redesign-privacy",
              ("redistribution-redesign", "--part", "privacy"),
              "docs/research/redistribution-redesign-study.md", True),
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
    "redistribution-redesign --part m2 (redistribution-redesign-study.md)":
        "~2 min per run, 304 runs",
    "m2-cumulative / m2-decay (consensus-fee-model-sim-verdicts.md)": "~190 s per run",
    "lottery-sweep --seed 1695 (consensus-fee-model-sim-verdicts.md)": "~76 s per fee model",
    "decoy-flood-sweep (decoy-flood-cost.md)": "~49 s; constants pinned by botho/tests/decoy_flood_parity.rs",
}


# Expected *metadata*, not a new command/mode registry: command and surface
# identity are resolved from the existing inventory, independently of goldens.
LOTTERY_METADATA = {
    "sweep.selection": {
        "Method": "expected-value annual population redistribution; research policies",
        "RNG": "none; SEED 0xB07A_0902 is documentary, not consumed",
        "Fees": "public-fee proxy = hypothetical LOTTERY_BASE_FEE_PICO plus demurrage; annual redistribution of demurrage; no production 800-permille track",
        "Assumptions": "analytic population/annual demurrage and weight model; hypothetical per-output ticket charge is not an enforced consensus floor",
        "Policies": "research Rule variants listed in formal-drt/sim-divergences.json; no production transition",
    },
    "sweep.sybil_brake": {
        "Method": "expected-value stationary capture/window and annual redistribution; research policies",
        "RNG": "none; SEED 0xB07A_0902_2 is documentary, not consumed",
        "Fees": "capture prices recurring hypothetical LOTTERY_BASE_FEE_PICO ticket refreshes; honest track redistributes annual demurrage; no production 800-permille track",
        "Assumptions": "analytic organic stream/window and annual demurrage; hypothetical per-output ticket charge is not an enforced consensus floor",
        "Policies": "research HonestPolicy variants listed in formal-drt/sim-divergences.json; no production transition",
    },
    "sweep.reward_cap": {
        "Method": "expected-value rate-based cap and annual/lump accounting; research policies",
        "RNG": "none; SEED 0xB07A_0902_3 is documentary, not consumed",
        "Fees": "claim1 prices hypothetical LOTTERY_BASE_FEE_PICO per output; throughput uses annual/lump demurrage; no production 800-permille track",
        "Assumptions": "rate-based cap and annual/lump demurrage; hypothetical per-output ticket charge is not an enforced consensus floor",
        "Policies": "research ExcessPolicy/GiniPolicy variants listed in formal-drt/sim-divergences.json; historical neutrality is not current production evidence",
    },
    "sweep.reward_cap.faithful": {
        "Method": "expected-value stationary economics plus candidate-window Monte Carlo; no production payout transitions",
        "RNG": "SplitMix64 synthetic transaction-key ordering and offsets; SEED=0x1682_FA17_0902; mixed seed=SEED ^ ((rho_o as u64) << 32) ^ (k as u64); 400 offsets for each of four settings",
        "Fees": "whale scenarios sweep ConsensusOnly (consensus) and ConsensusPlusRelayMin (consensus+relay); organic_tx_fee always ConsensusPlusRelayMin; 800-permille fee pool",
        "Assumptions": "default Uniform/four winners/age 720/minimum 1000000 pico/circulation 10000 blocks; truncated 10000-candidate window; free-ticket fixed point; backlog/epoch and ProposerSlot/BlockSize capacity; synthetic ordering is not ledger discovery",
        "Policies": "Rules::CurrentCode is an approximation; all other Rules in formal-drt/sim-divergences.json are unimplemented candidate remedies; #1682/#1761 profitability refutation unchanged",
    },
}


def lottery_metadata(gen: Generator) -> dict[str, str] | None:
    inventory = json.loads((REPO / "formal-drt/sim-divergences.json").read_text())
    matches = [s for s in inventory["surfaces"]
               if "command" in s and tuple(shlex.split(s["command"])) == gen.args]
    if not matches:
        if gen.name.startswith("lottery-"):
            raise ValueError(f"unknown lottery argv: {gen.args}")
        return None
    if len(matches) != 1 or matches[0]["classification"] != "research":
        raise ValueError("ambiguous or non-research inventory command")
    surface = matches[0]
    return {"Surface": surface["id"], "Command": shlex.join(gen.args),
            "Classification": "research", **LOTTERY_METADATA[surface["id"]]}


def validate_metadata(gen: Generator, out: str) -> dict[str, str] | None:
    expected = lottery_metadata(gen)
    if expected is None:
        return None
    actual = {}
    for line in out.splitlines():
        if line.startswith("Research-"):
            key, separator, value = line.removeprefix("Research-").partition(": ")
            if not separator or key in actual:
                raise ValueError(f"{gen.name}: malformed/duplicate metadata")
            actual[key] = value
    if actual != expected:
        raise ValueError(f"{gen.name}: metadata differs from inventory/algorithm contract: {actual}")
    return actual


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def execution_context() -> dict:
    def git(*args: str) -> str:
        return subprocess.check_output(["git", *args], cwd=REPO, text=True).strip()
    paths = ["Cargo.toml", "Cargo.lock", "rust-toolchain", "cluster-tax/Cargo.toml",
             "botho/Cargo.toml", "botho/tests/sim_code_parity.rs",
             "botho/src/consensus/lottery.rs", "formal-drt/sim-divergences.json",
             "scripts/research/check_research_tables.py", "scripts/research/test_check_research_tables.py",
             ".github/workflows/research-sims.yml", ".github/workflows/workspace-build.yml"]
    paths += [str(p.relative_to(REPO)) for p in (REPO / "cluster-tax/src").rglob("*.rs")]
    paths += [str(p.relative_to(REPO)) for p in (REPO / ".cargo").glob("config*")]
    patch = subprocess.check_output(["git", "diff", "HEAD", "--binary"], cwd=REPO)
    return {"head": git("rev-parse", "HEAD"), "tree": git("rev-parse", "HEAD^{tree}"),
            "dirty": bool(git("status", "--porcelain")), "patch_sha256": digest(patch),
            "source_sha256": {p: digest((REPO / p).read_bytes()) for p in sorted(paths)},
            "toolchain": {tool: subprocess.check_output([tool, "--version"], cwd=REPO, text=True).strip()
                          for tool in ("cargo", "rustc")},
            "profile_environment": {k: os.environ.get(k) for k in (
                "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS",
                "CARGO_TARGET_DIR", "RUSTFLAGS", "RUSTC_WRAPPER")},
            "ci": {k: os.environ.get(k) for k in (
                "GITHUB_SHA", "PR_HEAD_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB")}}


def capture_cargo_build(argv: list[str], target: str, receipt: Path) -> None:
    """Record the actual Cargo artifact from this build, not a guessed binary."""
    before = execution_context()
    receipt.parent.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(argv, cwd=REPO, capture_output=True)
    (receipt.parent / (receipt.stem + ".stdout")).write_bytes(result.stdout)
    (receipt.parent / (receipt.stem + ".stderr")).write_bytes(result.stderr)
    artifacts = []
    for line in result.stdout.splitlines():
        try:
            event = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue
        if (isinstance(event, dict) and event.get("reason") == "compiler-artifact" and event.get("executable")
                and event["target"]["name"] == target):
            artifacts.append(event)
    record = {"command": argv, "exit": result.returncode, "before": before,
              "after": execution_context(), "cargo_artifacts": artifacts,
              "stdout_sha256": digest(result.stdout), "stderr_sha256": digest(result.stderr)}
    if len(artifacts) == 1:
        binary = Path(artifacts[0]["executable"]).resolve()
        record["binary"] = str(binary)
        record["binary_sha256"] = digest(binary.read_bytes())
    write_json(receipt, record)
    if result.returncode or len(artifacts) != 1 or record["after"] != before:
        raise RuntimeError(f"build failed/ambiguous/source changed: {receipt}")


def verified_build(receipt: Path, binary: Path) -> dict:
    record = json.loads(receipt.read_text())
    current = execution_context()
    if record["exit"] != 0 or record["before"] != record["after"]:
        raise ValueError("build did not settle successfully in one checkout")
    # --update may change goldens, never the built source or its configuration.
    for key in ("head", "tree", "source_sha256", "profile_environment", "toolchain"):
        if current[key] != record["after"][key]:
            raise ValueError(f"build/execution {key} changed")
    if (str(binary.resolve()) != record["binary"] or
            digest(binary.read_bytes()) != record["binary_sha256"]):
        raise ValueError("executed binary does not match Cargo build artifact")
    return record


def capture_parity(build_receipt: Path, directory: Path) -> None:
    """Execute the full existing test target and retain its opt-in payload."""
    build = json.loads(build_receipt.read_text())
    binary = Path(build["binary"])
    verified_build(build_receipt, binary)
    directory.mkdir(parents=True, exist_ok=True)
    payload = directory / "production-lottery.json"
    payload.unlink(missing_ok=True)  # a previous invocation cannot satisfy this run
    argv = ["cargo", "test", "--locked", "-p", "botho", "--test", "sim_code_parity"]
    result = subprocess.run(argv, cwd=REPO, capture_output=True,
                            env={**os.environ, "BOTHO_LOTTERY_PARITY_PAYLOAD": str(payload)})
    (directory / "parity.stdout").write_bytes(result.stdout)
    (directory / "parity.stderr").write_bytes(result.stderr)
    data = json.loads(payload.read_text()) if payload.exists() else None
    inputs = ({"config": data["config"], "traces": [{k: t[k] for k in
        ("initial_carryover_pico", "initial_outputs", "blocks")} for t in data["traces"]]}
        if data else None)
    write_json(directory / "parity-receipt.json", {
        "context": execution_context(), "command": argv, "exit": result.returncode,
        "build_receipt_sha256": digest(build_receipt.read_bytes()),
        "binary_sha256": digest(binary.read_bytes()),
        "payload_produced": payload.is_file(),
        "payload_sha256": digest(payload.read_bytes()) if payload.exists() else None,
        "inputs_sha256": digest(json.dumps(inputs, sort_keys=True).encode()) if inputs else None,
        "config_sha256": digest(json.dumps(data["config"], sort_keys=True).encode()) if data else None,
        "assumptions": data["assumptions"] if data else None,
        "rng": data["rng"] if data else None,
        "stdout_sha256": digest(result.stdout), "stderr_sha256": digest(result.stderr),
        "claim": "synthetic actual-entry behavior/parity only; not economic evidence"})
    print(result.stdout.decode(errors="replace"))
    print(result.stderr.decode(errors="replace"), file=sys.stderr)
    verified_build(build_receipt, binary)
    if result.returncode or not payload.exists():
        raise RuntimeError("parity execution failed or bounded payload missing")


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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bin", required=True, type=Path, help="cluster-tax-sim binary")
    parser.add_argument("--update", action="store_true", help="rewrite the goldens")
    parser.add_argument("--receipt", type=Path, help="retain execution receipt and raw outputs")
    parser.add_argument("--build-receipt", type=Path, help="actual Cargo build receipt from this checkout")
    opts = parser.parse_args(argv)
    if opts.receipt and not opts.build_receipt:
        parser.error("--receipt requires --build-receipt")
    context = execution_context() if opts.receipt else None
    build = verified_build(opts.build_receipt, opts.bin) if opts.receipt else None
    records = []

    failures: list[str] = []
    GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
    for gen in GENERATORS:
        start = time.monotonic()
        out = run(opts.bin, gen)
        elapsed = time.monotonic() - start
        golden = GOLDEN_DIR / f"{gen.name}.txt"
        previous = golden.read_bytes() if golden.exists() else b""
        record = None
        if opts.receipt:
            raw = opts.receipt.parent / "outputs" / f"{gen.name}.txt"
            raw.parent.mkdir(parents=True, exist_ok=True)
            raw.write_text(out)
            if opts.update:
                original = opts.receipt.parent / "original-goldens" / golden.name
                original.parent.mkdir(parents=True, exist_ok=True)
                original.write_bytes(previous)
            record = {"generator": gen.name, "command": [str(opts.bin), *gen.args],
                            "exit": 0, "seconds": elapsed, "metadata": None,
                            "output_sha256": digest(out.encode()),
                            "previous_golden_sha256": digest(previous),
                            "previous_tables_sha256": digest(json.dumps(table_blocks(previous.decode())).encode()),
                            "tables_sha256": digest(json.dumps(table_blocks(out)).encode()),
                            "historical_binary_and_runtime_config": "unavailable; prior bytes only"}
            records.append(record)
        try:
            metadata = validate_metadata(gen, out)
        except (ValueError, KeyError) as error:
            failures.append(str(error))
            if record is not None:
                record["validation_error"] = str(error)
            continue  # critically, invalid metadata never reaches --update writes
        if record is not None:
            record["metadata"] = metadata
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

    if opts.receipt:
        write_json(opts.receipt, {"context": context, "after": execution_context(),
                   "checker_command": [str(Path(__file__)), *(argv if argv is not None else sys.argv[1:])],
                   "build_receipt_sha256": digest(opts.build_receipt.read_bytes()),
                   "binary_sha256": build["binary_sha256"], "update": opts.update,
                   "exit": int(bool(failures)), "records": records, "failures": failures})
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
