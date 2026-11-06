#!/usr/bin/env python3
"""Check retained exact evidence offline; optionally verify its Git source origin."""

import argparse
from decimal import Decimal
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile
import zipfile

from check_retained_resources import check_cases, check_linux, require

HERE = Path(__file__).resolve().parent
TESTS = {
    "arithmetic": "proof_experiment::proof_round_trips_and_measurements",
    "combined": "proof_experiment::combined::combined_round_trips_and_measurements",
    "short-hold": "proof_experiment::same_factor_short_hold_exact_proof",
}
SHAPE = (
    "inputs",
    "outputs",
    "ring",
    "significant_bits",
    "multipliers",
    "constraints",
    "arithmetic_bytes",
    "clsag_field_bytes",
    "signatures",
)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read(path):
    return json.loads(path.read_bytes())


def check_inventory(directory):
    expected = read(directory / "retention.json")["files_sha256"]
    actual = {
        p.name: digest(p.read_bytes())
        for p in directory.iterdir()
        if p.name != "retention.json"
    }
    require(actual == expected, "retention inventory/hash mismatch")


def check_archive(path, expected):
    with tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        require(
            len(members) == len(expected)
            and {m.name for m in members} == set(expected),
            "source archive inventory",
        )
        for member in members:
            require(
                member.isfile()
                and not Path(member.name).is_absolute()
                and ".." not in Path(member.name).parts,
                "source archive member",
            )
            require(
                digest(archive.extractfile(member).read()) == expected[member.name],
                "source archive hash",
            )


def check_cargo(directory, summary):
    raw = gzip.decompress((directory / "cargo.jsonl.gz").read_bytes())
    require(digest(raw) == summary["cargo_messages_sha256"], "Cargo messages hash")
    items = [json.loads(line) for line in raw.splitlines()]
    artifacts = [
        x
        for x in items
        if x.get("reason") == "compiler-artifact"
        and x.get("target", {}).get("name") == "bth_ct_demurrage_reference"
        and x.get("executable")
        and x.get("profile", {}).get("test")
    ]
    require(
        len(artifacts) == 1 and artifacts[0]["executable"] == summary["executable"],
        "unique Cargo executable",
    )
    require(
        artifacts[0]["profile"]["overflow_checks"] is True
        and artifacts[0]["profile"]["opt_level"] == "3",
        "checked release profile",
    )
    require(
        any(
            x.get("reason") == "build-finished" and x.get("success") is True
            for x in items
        ),
        "Cargo build incomplete",
    )
    return artifacts[0]


def check_supplement(directory, review):
    data = read(directory / "supplemental-sources.json")
    hashes = data["source_sha256"]
    required = {s["path"]: s["sha256"] for s in review["sources"]}
    require(
        set(required) | {"cluster-tax/src/lib.rs"} <= set(hashes),
        "required supplemental source missing",
    )
    require(
        all(hashes[p] == h for p, h in required.items()), "review source hash mismatch"
    )
    require(
        data["reviewed_implementation"] == review["source_commit"],
        "review source identity",
    )
    summary = read(directory / "summary.json")
    require(
        data["capture_commit"] == summary["checkout_commit"],
        "supplement capture identity",
    )
    require(
        all(hashes.get(p) == h for p, h in summary["source_sha256"].items()),
        "collector/supplement source disagreement",
    )
    check_archive(directory / "supplemental-sources.tar.gz", hashes)
    return data


def duration_ns(value):
    match = re.fullmatch(r"([0-9.]+)(ns|µs|ms|s)", value)
    require(match is not None, "duration syntax")
    return int(
        Decimal(match[1])
        * {"ns": 1, "µs": 1000, "ms": 1000000, "s": 1000000000}[match[2]]
    )


def parse_arithmetic(directory):
    logs = {name: (directory / (name + ".stdout")).read_text() for name in TESTS}
    for name, log in logs.items():
        require(
            "test result: ok. 1 passed; 0 failed; 0 ignored;" in log,
            "measurement test did not pass exactly once: " + name,
        )
    single = [
        dict(
            case=n,
            proof_bytes=int(b),
            prove_ns=duration_ns(p),
            verify_ns=duration_ns(v),
        )
        for n, b, p, v in re.findall(
            r"([\w-]+): bytes=(\d+) prove=(\S+) verify=(\S+)", logs["arithmetic"]
        )
    ]
    require(
        [s["case"] for s in single]
        == [
            "ordinary",
            "zero-rate",
            "zero-time",
            "zero-year",
            "floor-boundary",
            "maximum-domain",
            "both-caps-cancel",
            "one-cap-difference-one",
        ],
        "single case coverage/order",
    )
    require(
        all(
            s["proof_bytes"] == 1121 and s["prove_ns"] > 0 and s["verify_ns"] > 0
            for s in single
        ),
        "single proof observations",
    )
    combined = []
    pattern = (
        r"combined ([\w-]+) s=(\d+) sample=(\d+): multipliers=(\d+) constraints=(\d+) "
        r"proof=(\d+) section=(\d+) prove=(\S+) verify=(\S+)"
    )
    for n, s, i, m, c, b, sec, p, v in re.findall(pattern, logs["combined"]):
        combined.append(
            dict(
                case=n,
                significant_bits=int(s),
                sample=int(i),
                multipliers=int(m),
                constraints=int(c),
                proof_bytes=int(b),
                proposed_section_bytes=int(sec),
                prove_ns=duration_ns(p),
                verify_ns=duration_ns(v),
            )
        )
    require(
        [(s["case"], s["significant_bits"], s["sample"]) for s in combined]
        == [
            (n, b, i)
            for b in range(2, 5)
            for n in ["ordinary-1", "ordinary-16"]
            for i in range(3)
        ],
        "combined sample coverage/order",
    )
    for sample in combined:
        n = int(sample["case"].split("-")[1])
        require(
            (sample["multipliers"], sample["constraints"])
            == (1626 * n + 136, 3269 * n + 275),
            "combined exact census",
        )
        require(
            sample["proof_bytes"] == (1121 if n == 1 else 1377)
            and sample["proposed_section_bytes"] == 5 + 32 * n + sample["proof_bytes"]
            and sample["proposed_section_bytes"] <= 32768,
            "combined payload bytes",
        )
        require(sample["prove_ns"] > 0 and sample["verify_ns"] > 0, "combined timing")
    boundaries = re.findall(r"combined boundary ([\w-]+): pass", logs["combined"])
    require(
        boundaries
        == [
            "zero",
            "maximum-sum",
            "caps-cancel",
            "cap-difference-one",
            "one-to-sixteen",
            "sixteen-to-one",
        ],
        "combined boundaries",
    )
    setup = re.search(
        r"combined generator setup=(\S+); capacity=(\d+)", logs["combined"]
    )
    short = re.search(
        r"short-hold exact=(\d+) legacy=(\d+) capital=(\d+) bytes=(\d+) "
        r"multipliers=(\d+) constraints=(\d+) verified",
        logs["short-hold"],
    )
    require(
        setup is not None and int(setup[2]) == 32768 and short is not None,
        "setup or short-hold observation missing",
    )
    require(
        tuple(map(int, short.groups())) == (58494241735, 0, 0, 1121, 1562, 3140),
        "short-hold discriminator",
    )
    return single, combined, boundaries, setup, short


def check_artifact(directory):
    retention = read(directory / "retention.json")
    archive = directory / "workflow-artifact.zip"
    require(
        "sha256:" + digest(archive.read_bytes()) == retention["artifact_digest"],
        "artifact digest",
    )
    metadata = read(directory / "github-artifacts.json")["artifacts"]
    matches = [a for a in metadata if a["id"] == retention["artifact_id"]]
    require(
        len(matches) == 1 and matches[0]["digest"] == retention["artifact_digest"],
        "forge artifact digest",
    )
    run = read(directory / "github-run.json")
    require(
        str(run["id"]) == retention["run_id"]
        and str(run["run_attempt"]) == retention["run_attempt"]
        and run["conclusion"] == "success"
        and run["status"] == "completed"
        and run["head_sha"] == retention["pr_head_sha"],
        "workflow run identity/result",
    )
    with zipfile.ZipFile(archive) as source:
        expected = {
            "ownership-build.jsonl": gzip.decompress(
                (directory / "cargo.jsonl.gz").read_bytes()
            )
        }
        expected.update(
            {
                "ownership-resources/" + p.name: p.read_bytes()
                for p in directory.iterdir()
                if p.name == "summary.json" or p.suffix in (".log", ".txt")
            }
        )
        require(
            len(source.namelist()) == 18 and set(source.namelist()) == set(expected),
            "original artifact inventory",
        )
        require(
            all(source.read(p) == b for p, b in expected.items()),
            "original artifact bytes",
        )


def validate(here=HERE, verify_git=False):
    index = read(here / "exact-measurements.json")
    review_path = here / "exact-relation-review-sources.json"
    review = read(review_path)
    require(
        digest(review_path.read_bytes()) == index["review_manifest_sha256"],
        "review manifest hash",
    )
    require(
        (index["implementation_commit"], index["implementation_tree"])
        == (review["source_commit"], review["source_tree"]),
        "implementation identity",
    )
    directories = [
        here / "evidence" / ("exact-9-" + n) for n in ["arithmetic", "macos", "linux"]
    ]
    require(
        {
            str(p.relative_to(here)): digest(p.read_bytes())
            for d in directories
            for p in d.iterdir()
        }
        == index["evidence_sha256"],
        "exact evidence inventory/hash mismatch",
    )
    for directory in directories:
        check_inventory(directory)
    arithmetic, mac_dir, linux_dir = directories
    mac, linux = check_cases(mac_dir), check_linux(linux_dir)
    supplements = []
    for directory, summary, platform, commit in [
        (mac_dir, mac, "Darwin", index["macos_capture_commit"]),
        (linux_dir, linux, "Linux", index["linux_capture_commit"]),
    ]:
        require(
            summary["environment"]["system"] == platform
            and summary["checkout_commit"] == commit,
            "platform/capture identity",
        )
        require(
            summary["limits_seconds"] == {"per_case": 120, "matrix": 300}
            and summary["elapsed_seconds"] <= 300,
            "matrix limits",
        )
        check_cargo(directory, summary)
        check_archive(directory / "sources.tar.gz", summary["source_sha256"])
        supplement = check_supplement(directory, review)
        require(
            len(supplement["source_sha256"]) == index["supplemental_source_count"],
            "supplement count",
        )
        supplements.append(supplement)
        for case in summary["cases"]:
            for sample in case["samples"]:
                n = sample["inputs"]
                require(
                    (
                        sample["multipliers"],
                        sample["constraints"],
                        sample["arithmetic_bytes"],
                    )
                    == (
                        1626 * n + 136,
                        3269 * n + 275,
                        {1: 1121, 4: 1249, 16: 1377}[n],
                    ),
                    "ownership exact census/bytes",
                )
    require(
        supplements[0]["source_sha256"] == supplements[1]["source_sha256"],
        "platform source mismatch",
    )
    for a, b in zip(mac["cases"], linux["cases"]):
        require(
            [{k: s[k] for k in SHAPE} for s in a["samples"]]
            == [{k: s[k] for k in SHAPE} for s in b["samples"]],
            "cross-platform shape",
        )
    check_artifact(linux_dir)
    single, combined, boundaries, setup, short = parse_arithmetic(arithmetic)
    observed = read(arithmetic / "measurements.json")
    require(
        (observed["single"], observed["combined"], observed["combined_boundary_passes"])
        == (single, combined, boundaries),
        "arithmetic index/raw disagreement",
    )
    require(
        observed["combined_generator"]
        == {"capacity": int(setup[2]), "setup_ns": duration_ns(setup[1])}
        and observed["short_hold"]
        == dict(
            zip(
                [
                    "exact",
                    "legacy",
                    "capital",
                    "proof_bytes",
                    "multipliers",
                    "constraints",
                ],
                map(int, short.groups()),
            )
        ),
        "arithmetic summary",
    )
    require(
        gzip.decompress((arithmetic / "cargo.jsonl.gz").read_bytes())
        == gzip.decompress((mac_dir / "cargo.jsonl.gz").read_bytes()),
        "same local Cargo capture",
    )
    identity = read(arithmetic / "build-identity.json")
    require(
        identity == read(mac_dir / "build-identity.json")
        and identity["executable"] == mac["executable"]
        and identity["executable_sha256"] == mac["executable_sha256"]
        and identity["checkout_commit"] == mac["checkout_commit"]
        and identity["git_status_porcelain"] == mac["git_status_porcelain"]
        and identity["tracked_diff_sha256"] == mac["tracked_diff_sha256"],
        "local binary/source identity",
    )
    commands = read(arithmetic / "commands.json")
    for name, test in TESTS.items():
        rows = [r for r in commands if r["name"] == name]
        require(len(rows) == 1, "measurement command inventory")
        row = rows[0]
        require(
            row["exit_code"] == 0
            and row["timed_out"] is False
            and row["argv"]
            == [mac["executable"], test, "--exact", "--nocapture", "--test-threads=1"],
            "measurement command",
        )
        for stream in ["stdout", "stderr"]:
            require(
                row[stream + "_sha256"]
                == digest((arithmetic / (name + "." + stream)).read_bytes()),
                "measurement command/log hash",
            )
    if verify_git:
        root = here.parents[2]
        for supplement in supplements:
            for commit in [
                supplement["capture_commit"],
                index["implementation_commit"],
            ]:
                tree = subprocess.check_output(
                    ["git", "rev-parse", commit + "^{tree}"], cwd=root, text=True
                ).strip()
                expected_tree = (
                    supplement["capture_tree"]
                    if commit == supplement["capture_commit"]
                    else index["implementation_tree"]
                )
                require(tree == expected_tree, "Git tree identity")
                for path, expected in supplement["source_sha256"].items():
                    require(
                        digest(
                            subprocess.check_output(
                                ["git", "show", commit + ":" + path], cwd=root
                            )
                        )
                        == expected,
                        "Git source bytes: " + path,
                    )
    return index


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--verify-git",
        action="store_true",
        help="requires retained commits in local Git history",
    )
    args = parser.parse_args()
    result = validate(verify_git=args.verify_git)
    print(
        f"PASS: exact evidence; {len(result['evidence_sha256'])} files; "
        f"{result['supplemental_source_count']} supplemental sources; 8 processes/13 proofs per platform."
    )
    print("CT1 acceptance and activation are not established.")


if __name__ == "__main__":
    main()
