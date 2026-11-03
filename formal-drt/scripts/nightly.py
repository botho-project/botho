#!/usr/bin/env python3
"""Bounded nightly DRT orchestration and a data-only, default-branch reporter.

No third-party modules. Artifact data is never executed. A mismatch is finite
correspondence evidence, not a proof. See formal/README.md for the contract.
"""

import argparse
import concurrent.futures
import difflib
import hashlib
import html
import json
import math
import os
from pathlib import Path
import re
import secrets
import signal
import subprocess
import sys
import time

LEG = re.compile(r"[a-z][a-z0-9_]{0,79}\Z")
SHA = re.compile(r"[0-9a-f]{40}\Z")
MARKER = re.compile(r"<!-- botho-drt:v1 sha256:([0-9a-f]{64}) -->\Z")
MAX_JSON = 64 * 1024 * 1024
TITLE = "Nightly DRT differential finding"


def require(ok, message):
    if not ok:
        raise ValueError(message)


def natural(value, limit=2**64 - 1):
    return type(value) is int and 0 <= value <= limit


def canonical(value):
    """Identity uses exact integers/strings, not floats or boolean-as-int equality."""
    if isinstance(value, dict):
        require(all(isinstance(k, str) for k in value), "non-string JSON key")
        for v in value.values():
            canonical(v)
    elif isinstance(value, list):
        for v in value:
            canonical(v)
    else:
        require(
            value is None or type(value) in (str, int, bool),
            "non-integral identity value",
        )
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def fingerprint(leg, value):
    require(isinstance(leg, str) and LEG.fullmatch(leg), "invalid leg")
    return hashlib.sha256(canonical({"leg": leg, "input": value}).encode()).hexdigest()


def unique_object(pairs):
    out = {}
    for k, v in pairs:
        require(k not in out, "duplicate JSON key")
        out[k] = v
    return out


def read_json(path):
    require(not path.is_symlink() and path.is_file(), f"missing/unsafe report: {path}")
    require(path.stat().st_size <= MAX_JSON, "JSON exceeds size limit")
    return json.loads(
        path.read_text(),
        object_pairs_hook=unique_object,
        parse_constant=lambda x: (_ for _ in ()).throw(ValueError(f"invalid {x}")),
    )


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    )


def seed_value(text):
    if text == "random":
        return str(secrets.randbits(64))
    require(
        isinstance(text, str) and re.fullmatch(r"[0-9]+", text),
        "seed must be u64 or random",
    )
    require(int(text) < 2**64, "seed outside u64")
    return str(int(text))


def selection(text, discovered):
    legs = text.split(",") if text else discovered[:]
    require(legs and len(legs) == len(set(legs)), "empty/duplicate leg selection")
    require(
        all(LEG.fullmatch(x) and x in discovered for x in legs),
        "unknown/invalid selected leg",
    )
    return legs


def discover(runner):
    p = subprocess.run(
        [str(runner), "--list"], capture_output=True, text=True, timeout=30, check=True
    )
    legs = p.stdout.splitlines()
    require(
        legs and len(legs) == len(set(legs)) and all(LEG.fullmatch(x) for x in legs),
        "--list returned empty, duplicate or invalid legs",
    )
    return legs


def prepare(args):
    require(1 <= args.cases <= 1_000_000, "cases outside 1..1000000")
    require(SHA.fullmatch(args.sha), "expected full tested commit SHA")
    require(
        re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository),
        "invalid repository",
    )
    require(
        re.fullmatch(r"[0-9]+", args.run_id)
        and re.fullmatch(r"[1-9][0-9]*", args.attempt),
        "invalid run identity",
    )
    require(not (args.out / "metadata.json").exists(), "output already contains a run")
    meta = {
        "version": 1,
        "seed": seed_value(args.seed),
        "cases": args.cases,
        "requested_legs": args.legs,
        "sha": args.sha,
        "ref": args.ref,
        "repository": args.repository,
        "run_id": args.run_id,
        "attempt": args.attempt,
        "run_url": f"https://github.com/{args.repository}/actions/runs/{args.run_id}",
        "selected_legs": [],
        "discovered_legs": [],
        "replay_counts": {},
    }
    write_json(args.out / "metadata.json", meta)
    return 0


def validate_report(report, status, meta, leg):
    """Return clean/mismatch; any inconsistent field fails the entire leg closed."""
    require(
        isinstance(report, dict) and report.get("seed") == meta["seed"],
        "wrong/missing seed",
    )
    require(
        type(report.get("cases_per_leg")) is int
        and report["cases_per_leg"] == meta["cases"],
        "wrong case budget",
    )
    require(
        isinstance(report.get("legs"), list) and len(report["legs"]) == 1,
        "wrong leg coverage",
    )
    r = report["legs"][0]
    require(isinstance(r, dict) and r.get("leg") == leg, "wrong leg")
    require(natural(r.get("cases"), meta["cases"]), "invalid generated count")
    require(
        natural(r.get("replayed")) and r["replayed"] == meta["replay_counts"][leg],
        "incomplete/wrong replay count",
    )
    require(isinstance(r.get("rust"), str) and r["rust"], "missing Rust target")
    seconds = r.get("seconds")
    require(
        type(seconds) in (int, float) and math.isfinite(seconds) and seconds >= 0,
        "invalid duration",
    )
    classes = r.get("classes")
    require(
        isinstance(classes, dict)
        and all(natural(v, r["cases"]) for v in classes.values())
        and sum(classes.values()) <= r["cases"],
        "invalid class counts",
    )
    failures = r.get("mismatches")
    require(isinstance(failures, list), "missing mismatch list")
    for witness in failures:
        require(
            isinstance(witness, dict) and {"input", "lean", "rust"} <= witness.keys(),
            "incomplete witness",
        )
        fingerprint(leg, witness["input"])
        require(
            canonical(witness["lean"]) != canonical(witness["rust"]),
            "equal witness outputs",
        )
        require(
            not (isinstance(witness["lean"], dict) and "lean_err" in witness["lean"]),
            "lean_err: harness/protocol failure",
        )
        require(
            "replayed_fixture" not in witness
            or isinstance(witness["replayed_fixture"], str),
            "invalid replay provenance",
        )
    require(
        type(status.get("exit")) is int and status.get("timeout") is False,
        "timeout/model failure",
    )
    if status["exit"] == 0:
        require(
            not failures and r["cases"] == meta["cases"],
            "incomplete/inconsistent clean report",
        )
        return "clean"
    require(status["exit"] == 1 and failures, "inconsistent exit/report combination")
    return "mismatch"


def execute(command, log, timeout):
    """Kill the entire runner/model process group on a finite timeout."""
    with log.open("w") as output:
        p = subprocess.Popen(
            command, stdout=output, stderr=subprocess.STDOUT, start_new_session=True
        )
        try:
            return {"exit": p.wait(timeout=timeout), "timeout": False}
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            return {"exit": p.returncode, "timeout": True}


def run_leg(args, meta, leg, deadline):
    folder = args.out / "legs" / leg
    folder.mkdir(parents=True)
    status = {"exit": None, "timeout": False}
    before = args.out / "baseline" / f"{leg}.jsonl"
    replay = args.out / "regressions" / f"{leg}.jsonl"
    try:
        remaining = deadline - time.monotonic()
        require(remaining > 0, "overall deadline reached before this leg started")
        command = [
            str(args.runner.resolve()),
            "--model",
            str(args.model.resolve()),
            "--cases",
            str(meta["cases"]),
            "--seed",
            meta["seed"],
            "--legs",
            leg,
            "--regressions",
            str((args.out / "regressions").resolve()),
            "--json",
            str((folder / "report.json").resolve()),
        ]
        status.update(
            execute(command, folder / "process.log", min(args.leg_timeout, remaining))
        )
        report = read_json(folder / "report.json")
        status["kind"] = validate_report(report, status, meta, leg)
        if status["kind"] == "mismatch":
            fixtures = args.out / "failing" / f"{leg}.jsonl"
            fixtures.parent.mkdir(exist_ok=True)
            fixtures.write_text(
                "".join(canonical(w) + "\n" for w in report["legs"][0]["mismatches"])
            )
    except (OSError, ValueError, KeyError, TypeError) as e:
        status.update(kind="infrastructure", error=str(e))
    try:
        old = before.read_bytes() if before.exists() else b""
        new = replay.read_bytes() if replay.exists() else b""
        require(new.startswith(old), "runner replaced committed regression prefix")
        appended = args.out / "appended"
        appended.mkdir(exist_ok=True)
        (appended / f"{leg}.jsonl").write_bytes(new[len(old) :])
        (appended / f"{leg}.diff").write_text(
            "".join(
                difflib.unified_diff(
                    old.decode().splitlines(keepends=True),
                    new.decode().splitlines(keepends=True),
                    fromfile=f"committed/{leg}.jsonl",
                    tofile=f"regressions/{leg}.jsonl",
                )
            )
        )
    except (OSError, ValueError) as e:
        status.update(kind="infrastructure", error=str(e))
    write_json(folder / "status.json", status)
    return status


def run(args):
    require(
        1 <= args.workers <= 4
        and 0 < args.leg_timeout <= 3600
        and 0 < args.total_timeout <= 12_600,
        "invalid process/deadline bounds",
    )
    meta = read_json(args.out / "metadata.json")
    require(not (args.out / "legs").exists(), "refusing to reuse stale leg results")
    legs = discover(args.runner.resolve())
    selected = selection(meta["requested_legs"], legs)
    meta.update(
        discovered_legs=legs,
        selected_legs=selected,
        workers=args.workers,
        leg_timeout=args.leg_timeout,
        total_timeout=args.total_timeout,
    )
    for leg in selected:
        source = args.regressions / f"{leg}.jsonl"
        require(not source.is_symlink(), "symlink in regressions")
        raw = source.read_bytes() if source.exists() else b""
        count = 0
        for line in raw.decode().splitlines():
            if line.strip() and not line.lstrip().startswith("#"):
                fixture = json.loads(line, object_pairs_hook=unique_object)
                require(
                    isinstance(fixture, dict) and "input" in fixture,
                    "invalid committed fixture",
                )
                count += 1
        meta["replay_counts"][leg] = count
        for directory in ("baseline", "regressions"):
            path = args.out / directory / f"{leg}.jsonl"
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(raw)
    write_json(args.out / "metadata.json", meta)
    deadline = time.monotonic() + args.total_timeout
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = {
            leg: pool.submit(run_leg, args, meta, leg, deadline) for leg in selected
        }
        statuses = {}
        for leg, future in futures.items():
            try:
                statuses[leg] = future.result()
            except Exception as e:  # Keep collecting independent legs, including a failed report write.
                statuses[leg] = {"kind": "infrastructure", "error": str(e)}
    write_json(args.out / "collection.json", statuses)
    return int(any(s["kind"] != "clean" for s in statuses.values()))


def bundle_json(out, *parts):
    path = out.joinpath(*parts)
    require(
        path.resolve().is_relative_to(out.resolve()),
        "artifact path escapes its directory",
    )
    return read_json(path)


def inspect_bundle(out):
    meta = bundle_json(out, "metadata.json")
    require(isinstance(meta, dict), "invalid metadata object")
    require(
        type(meta.get("version")) is int
        and meta["version"] == 1
        and isinstance(meta.get("seed"), str)
        and seed_value(meta["seed"]) == meta["seed"],
        "invalid metadata seed/version",
    )
    require(
        natural(meta.get("cases"), 1_000_000) and meta["cases"] > 0,
        "invalid case budget",
    )
    legs = meta["selected_legs"]
    require(
        isinstance(legs, list)
        and selection(",".join(legs), meta["discovered_legs"]) == legs,
        "missing/invalid selected legs",
    )
    require(
        isinstance(meta.get("replay_counts"), dict)
        and set(meta["replay_counts"]) == set(legs)
        and all(natural(x) for x in meta["replay_counts"].values()),
        "invalid replay metadata",
    )
    collection = bundle_json(out, "collection.json")
    require(
        isinstance(collection, dict) and set(collection) == set(legs),
        "missing-leg coverage",
    )
    findings, rows = {}, []
    for leg in legs:
        try:
            status = bundle_json(out, "legs", leg, "status.json")
            require(
                isinstance(status, dict)
                and status == collection[leg]
                and status.get("kind") in ("clean", "mismatch"),
                "infrastructure or inconsistent collection",
            )
            report = bundle_json(out, "legs", leg, "report.json")
            kind = validate_report(report, status, meta, leg)
            require(status["kind"] == kind, "inconsistent result kind")
            r = report["legs"][0]
            rows.append(
                f"{leg}: {kind}; generated {r['cases']}/{meta['cases']}, replayed {r['replayed']}"
            )
            for w in r["mismatches"]:
                findings.setdefault(fingerprint(leg, w["input"]), (leg, w, r["cases"]))
        except (OSError, ValueError, KeyError, TypeError) as e:
            rows.append(f"{leg}: infrastructure ({e})")
    return meta, findings, rows


def summary(args):
    lines = [
        "## Nightly Lean–Rust DRT",
        "Finite tests provide evidence, not a proof of equivalence.",
    ]
    try:
        meta = read_json(args.out / "metadata.json")
        lines += [
            f"Seed: `{meta['seed']}`; SHA: `{meta['sha']}`; ref: {html.escape(meta['ref'])}",
            f"Run attempt: {meta['attempt']}; requested cases per leg: {meta['cases']}",
        ]
        _, _, rows = inspect_bundle(args.out)
        lines.extend(f"- {html.escape(row)}" for row in rows)
    except (OSError, ValueError, KeyError, TypeError) as e:
        lines.append(f"Infrastructure/setup failure: {html.escape(str(e))}")
    if args.artifact_url:
        lines.append(f"[Complete run artifact]({args.artifact_url})")
    else:
        lines.append(
            "Artifacts are uploaded after collection; see this run's artifact list."
        )
    lines.append(
        "Mismatch counts can stop after a generated batch; they do not certify that every generated case was compared."
    )
    with args.summary.open("a") as f:
        f.write("\n\n".join(lines) + "\n")
    return 0


class GitHub:
    def __init__(self, repository):
        self.repository = repository

    def request(self, method, endpoint, body=None):
        command = [
            "gh",
            "api",
            "--method",
            method,
            f"repos/{self.repository}/{endpoint}",
        ]
        if body is not None:
            command += ["--input", "-"]
        p = subprocess.run(
            command,
            input=json.dumps(body) if body is not None else None,
            text=True,
            capture_output=True,
            timeout=60,
            check=True,
        )
        return json.loads(p.stdout) if p.stdout.strip() else None

    def issues(self):
        page = 1
        while True:
            batch = self.request("GET", f"issues?state=all&per_page=100&page={page}")
            require(isinstance(batch, list), "invalid issue-list response")
            yield from batch
            if len(batch) < 100:
                break
            page += 1


def issue_body(meta, leg, witness, count, digest, artifact_url):
    fixture = canonical(witness)
    truncated = len(html.escape(fixture).encode()) > 24_000
    preview = fixture[:4000] if truncated else fixture
    panic = isinstance(witness["rust"], dict) and "rust_panic" in witness["rust"]
    return f"""## {TITLE}
<!-- botho-drt:v1 sha256:{digest} -->
<!-- botho-drt-run:{meta["run_id"]}:{meta["attempt"]} -->

{"Caught Rust panic" if panic else "Unequal Lean/Rust values"} in `{leg}`. This is finite differential-test evidence, not a proof of equivalence. Generated witnesses use the existing bounded shrinker; replayed witnesses may append no new fixture. Global minimality is not claimed.

Seed: `{meta["seed"]}`; tested model/build SHA: `{meta["sha"]}`.
Ref: {html.escape(meta["ref"])}. Requested cases: {meta["cases"]}; harness-reported generated count: {count}. Early stop may leave part of the final generated batch uncompared.
[Run, attempt {meta["attempt"]}]({meta["run_url"]}) · [Complete fixture, logs and reports]({artifact_url})

{"Fixture preview is TRUNCATED; the complete replayable record is in failing/" + leg + ".jsonl in the artifact." if truncated else "Complete replayable fixture (including observed Lean and Rust outputs):"}
<pre>{html.escape(preview)}</pre>

Reproduce after extracting the artifact into `nightly-artifact` (use this exact source revision for both builds):
```sh
git checkout {meta["sha"]}
(cd formal-drt/lean && lake build)
cp Cargo.lock formal-drt/Cargo.lock
cargo build --release --manifest-path formal-drt/Cargo.toml
formal-drt/target/release/botho-formal-drt --model formal-drt/lean/.lake/build/bin/botho-model --seed {meta["seed"]} --legs {leg} --cases {meta["cases"]} --regressions nightly-artifact/failing --no-write-regressions --json replay-report.json
```
The artifact also preserves original replay fixtures, appended records/diffs, and every per-leg report. Identity v1 hashes only the leg and canonical input; seeds, outputs and run metadata do not change it.
"""


def publish(meta, findings, api, artifact_url):
    index = {}
    for issue in api.issues():
        if (
            "pull_request" in issue
            or issue.get("user", {}).get("login") != "github-actions[bot]"
        ):
            continue
        body = issue.get("body") or ""
        for line in body.splitlines():
            match = MARKER.fullmatch(line)
            if match:
                require(
                    match[1] not in index,
                    "multiple existing issues with the exact fingerprint",
                )
                index[match[1]] = issue
    links = []
    for digest, (leg, witness, count) in sorted(findings.items()):
        body = issue_body(meta, leg, witness, count, digest, artifact_url)
        existing = index.get(digest)
        attempt = f"<!-- botho-drt-run:{meta['run_id']}:{meta['attempt']} -->"
        if existing and attempt in (existing.get("body") or "").splitlines():
            issue = existing
        elif existing:
            issue = api.request(
                "PATCH", f"issues/{existing['number']}", {"body": body, "state": "open"}
            )
        else:
            issue = api.request(
                "POST",
                "issues",
                {
                    "title": f"formal(drt): {leg} differential finding [{digest[:12]}]",
                    "body": body,
                    "labels": ["area:testing", "loom:triage"],
                },
            )
        links.append(issue["html_url"])
    return links


def report(args):
    meta, findings, rows = inspect_bundle(args.out)
    for key in ("repository", "run_id", "attempt", "sha"):
        require(
            meta.get(key) == getattr(args, key),
            f"artifact {key} differs from trusted run context",
        )
    require(
        SHA.fullmatch(meta["sha"])
        and isinstance(meta.get("ref"), str)
        and len(meta["ref"]) <= 1024,
        "invalid SHA/ref",
    )
    expected = f"https://github.com/{args.repository}/actions/runs/{args.run_id}"
    require(
        meta.get("run_url") == expected
        and re.fullmatch(re.escape(expected) + r"/artifacts/[0-9]+", args.artifact_url),
        "invalid run/artifact link",
    )
    links = (
        publish(meta, findings, GitHub(args.repository), args.artifact_url)
        if findings
        else []
    )
    with args.summary.open("a") as f:
        f.write(
            "\n## Differential finding issues\n"
            + ("\n".join(f"- {x}" for x in links) or "No confirmed mismatch issues.")
            + "\n"
        )
    return int(any(": infrastructure" in row for row in rows))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    modes = p.add_subparsers(dest="mode", required=True)
    for name in ("prepare", "run", "summary", "report"):
        mode = modes.add_parser(name)
        mode.add_argument("--out", type=Path, required=True)
        if name in ("prepare", "report"):
            for field, env, default in (
                ("repository", "GITHUB_REPOSITORY", "local/local"),
                ("run-id", "GITHUB_RUN_ID", "0"),
                ("attempt", "GITHUB_RUN_ATTEMPT", "1"),
                ("sha", "GITHUB_SHA", ""),
            ):
                mode.add_argument("--" + field, default=os.environ.get(env, default))
        if name == "prepare":
            mode.add_argument("--seed", default="random")
            mode.add_argument("--cases", type=int, default=1_000_000)
            mode.add_argument("--legs", default="")
            mode.add_argument("--ref", default=os.environ.get("GITHUB_REF", "local"))
        elif name == "run":
            mode.add_argument("--runner", type=Path, required=True)
            mode.add_argument("--model", type=Path, required=True)
            mode.add_argument("--regressions", type=Path, required=True)
            mode.add_argument("--workers", type=int, default=4)
            mode.add_argument("--leg-timeout", type=float, default=3600)
            mode.add_argument("--total-timeout", type=float, default=9600)
        if name in ("summary", "report"):
            mode.add_argument("--summary", type=Path, required=True)
            mode.add_argument("--artifact-url", default="")
    args = p.parse_args()
    try:
        return globals()[args.mode](args)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as e:
        print(f"nightly infrastructure failure: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
