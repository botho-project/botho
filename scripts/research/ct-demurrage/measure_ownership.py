#!/usr/bin/env python3
"""Bounded fresh-process measurements of the exact ignored research case only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
TEST = "proof_experiment::combined::ownership::resources::ownership_resource_case"
CASES = {"generators": 0, "ordinary-1": 3, "ordinary-4": 3, "ordinary-16": 3,
         "zero": 1, "both-caps-cancel": 1, "one-cap-difference": 1, "maximum-sum": 1}
CASE_SECONDS = 120
TOTAL_SECONDS = 300


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def executable(messages):
    candidates = []
    for line in messages.read_text().splitlines():
        item = json.loads(line)
        if (item.get("reason") == "compiler-artifact"
                and item.get("target", {}).get("name") == "bth_ct_demurrage_reference"
                and item.get("profile", {}).get("test") and item.get("executable")):
            candidates.append(Path(item["executable"]).resolve())
    if len(set(candidates)) != 1 or not candidates[0].is_file():
        raise ValueError("expected exactly one existing research test executable")
    return candidates[0]


def rss_bytes(raw, system):
    if system == "Darwin":
        match = re.search(r"^\s*(\d+)\s+maximum resident set size\s*$", raw, re.M)
        scale = 1
    elif system == "Linux":
        match = re.search(r"^\s*Maximum resident set size \(kbytes\):\s*(\d+)\s*$", raw, re.M)
        scale = 1024
    else:
        raise ValueError("RSS collection supports only macOS and Linux")
    if not match or int(match[1]) <= 0:
        raise ValueError("missing or zero whole-case RSS observation")
    return int(match[1]) * scale


def records(output, case, count):
    parsed = {"setup": [], "fixture": [], "samples": []}
    for line in output.splitlines():
        # libtest can prefix the first marker with 'test <name> ... '.
        match = re.search(r"RESOURCE_(SETUP|FIXTURE|SAMPLE) (.*)$", line)
        if not match:
            continue
        fields = dict(part.split("=", 1) for part in match[2].split())
        if fields.pop("case") != case:
            raise ValueError("case identity mismatch")
        values = {key: int(value) for key, value in fields.items()}
        if any(value < 0 for value in values.values()):
            raise ValueError("negative resource value")
        parsed[{"SETUP": "setup", "FIXTURE": "fixture", "SAMPLE": "samples"}[match[1]]].append(values)
    if len(parsed["setup"]) != 1 or len(parsed["fixture"]) != bool(count):
        raise ValueError("incomplete setup/fixture records")
    if (set(parsed["setup"][0]) != {"capacity", "setup_ns"}
            or parsed["setup"][0]["capacity"] != 32768
            or parsed["setup"][0]["setup_ns"] <= 0):
        raise ValueError("invalid generator setup record")
    if count and (set(parsed["fixture"][0]) != {"fixture_ns"}
                  or parsed["fixture"][0]["fixture_ns"] <= 0):
        raise ValueError("invalid fixture record")
    if len(parsed["samples"]) != count:
        raise ValueError("incomplete sample records")
    if "test result: ok. 1 passed; 0 failed; 0 ignored;" not in output:
        raise ValueError("exact measurement test did not run successfully")
    expected_n = int(case.split("-")[-1]) if case.startswith("ordinary-") else (16 if case == "maximum-sum" else 1)
    expected_keys = {"sample", "inputs", "outputs", "ring", "significant_bits", "multipliers",
                     "constraints", "arithmetic_bytes", "clsag_field_bytes", "signatures", "prove_ns", "verify_ns"}
    for index, sample in enumerate(parsed["samples"]):
        if set(sample) != expected_keys or sample["sample"] != index:
            raise ValueError("unexpected or out-of-order sample fields")
        if any(sample[key] != expected_n for key in ("inputs", "outputs", "signatures")):
            raise ValueError("unexpected input/output/signature count")
        if sample["ring"] != 20 or sample["significant_bits"] != 2:
            raise ValueError("unexpected ring or bucket configuration")
        if sample["clsag_field_bytes"] != 736 * expected_n:
            raise ValueError("unexpected signature field length")
        if not (0 < sample["multipliers"] <= 65536 and 0 < sample["constraints"] <= 262144):
            raise ValueError("arithmetic allocation exceeds proposed ceiling")
        if not all(sample[key] > 0 for key in ("prove_ns", "verify_ns", "arithmetic_bytes")):
            raise ValueError("missing operation observation")
    return parsed


def execute(command, env, log, deadline):
    """Kill the whole process group on timeout, including /usr/bin/time's child."""
    with log.open("w") as out:
        process = subprocess.Popen(command, env=env, stdout=out, stderr=subprocess.STDOUT,
                                   start_new_session=True, cwd=ROOT)
        try:
            return process.wait(timeout=deadline), False
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            return process.returncode, True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo-messages", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    binary = executable(args.cargo_messages)
    output = args.output_dir.resolve()
    # Never overwrite/merge old measurements or recursively delete cache paths.
    output.mkdir(parents=True, exist_ok=False)
    system = platform.system()
    if system == "Darwin":
        cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
    elif system == "Linux":
        cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                    if line.startswith("model name")), "unreported")
    else:
        raise ValueError("RSS collection supports only macOS and Linux")
    sources = sorted((ROOT / "scripts/research/ct-demurrage/src").rglob("*.rs")) + [
        ROOT / "Cargo.lock", ROOT / "Cargo.toml", ROOT / "rust-toolchain",
        Path(__file__).resolve(), ROOT / "scripts/research/ct-demurrage/Cargo.toml",
        ROOT / ".github/workflows/ownership-resources.yml",
        ROOT / "crypto/ring-signature/src/ring_signature/clsag.rs",
        ROOT / "vendor/bulletproofs-og/src/r1cs/prover.rs",
        ROOT / "vendor/bulletproofs-og/src/r1cs/verifier.rs",
    ]
    result = {"schema": 1, "status": "incomplete", "cases": [],
              "checkout_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "source_sha256": {str(path.relative_to(ROOT)): sha(path) for path in sources},
              "git_status_porcelain": subprocess.check_output(
                  ["git", "status", "--porcelain=v1", "--untracked-files=all"], cwd=ROOT, text=True),
              "tracked_diff_sha256": hashlib.sha256(subprocess.check_output(
                  ["git", "diff", "HEAD", "--binary"], cwd=ROOT)).hexdigest(),
              "compile_recipe": ["cargo", "test", "--locked", "--release", "-p",
                                 "bth-ct-demurrage-reference", "--no-run", "--message-format=json"],
              "build_evidence_note": "Recipe used by workflow/reproduction command; Cargo stdout selects executable. Hashes identify supplied output/binary/source, not a hermetic build attestation.",
              "executable": str(binary), "executable_sha256": sha(binary),
              "cargo_messages": str(args.cargo_messages.resolve()), "cargo_messages_sha256": sha(args.cargo_messages),
              "environment": {"system": system, "release": platform.release(), "machine": platform.machine(),
                              "processor": cpu, "python": platform.python_version(),
                              "rustc": subprocess.check_output(["rustc", "--version"], cwd=ROOT, text=True).strip()},
              "ci": {key: os.environ.get(key) for key in ("GITHUB_REPOSITORY", "GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")},
              "limits_seconds": {"per_case": CASE_SECONDS, "matrix": TOTAL_SECONDS},
              "scope": "Whole-case peak RSS includes generators, fixtures and all samples; no per-proof subtraction. Field bytes are not a wire envelope."}
    summary = output / "summary.json"
    def save():
        summary.write_text(json.dumps(result, indent=2) + "\n")
    save()
    started = time.monotonic()
    try:
        for case, count in CASES.items():
            remaining = TOTAL_SECONDS - (time.monotonic() - started)
            if remaining <= 0:
                raise TimeoutError("matrix deadline exhausted")
            rss = output / f"{case}.rss.txt"
            log = output / f"{case}.log"
            timing = ["/usr/bin/time", "-l" if system == "Darwin" else "-v", "-o", str(rss)]
            command = timing + [str(binary), TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"]
            row = {"case": case, "command": command, "status": "incomplete"}
            result["cases"].append(row)
            save()
            env = dict(os.environ, BOTHO_OWNERSHIP_CASE=case)
            rc, timed_out = execute(command, env, log, min(CASE_SECONDS, remaining))
            row.update(exit_code=rc, timed_out=timed_out, output_sha256=sha(log))
            if rc != 0 or timed_out:
                row["status"] = "failed"
                raise RuntimeError(f"{case}: exit={rc}, timeout={timed_out}")
            row.update(records(log.read_text(), case, count))
            row["whole_case_peak_rss_bytes"] = rss_bytes(rss.read_text(), system)
            row["rss_raw_sha256"] = sha(rss)
            row["status"] = "passed"
            save()
        result["status"] = "passed"
    except (ValueError, KeyError, RuntimeError, TimeoutError, OSError) as error:
        result["status"] = "failed"
        result["error"] = str(error)
    result["elapsed_seconds"] = time.monotonic() - started
    save()
    print(f"{result['status']}: {summary}")
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
