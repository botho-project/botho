#!/usr/bin/env python3
"""Check a candidate node against the original five-collector x2 minting run."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from fleet_nodes import VALIDATORS
from fleet_provision import PRODUCER, validate_run_id
from sustained_memory import manifest_digest, validate_manifest


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def check_archive_index(archive, expected):
    """Honor a retained checksum index and its outer receipt when present."""
    index = archive / "SHA256SUMS"
    outer = archive.parent / "final-receipt.json" if archive.name == "outcome" else None
    if outer and outer.is_file():
        receipt = json.loads(outer.read_text())
        if receipt.get("checksum_index_sha256") != file_hash(index):
            raise ValueError("checksum index differs from the fleet's final receipt")
        result = json.loads((archive / "result.json").read_text())
        if (
            receipt.get("qualified") is not True
            or result.get("qualified") is not True
            or any(
                receipt.get(k) != result.get(k)
                for k in ("run", "commit", "artifact_sha256", "scope", "status")
            )
        ):
            raise ValueError(
                "fleet's final receipt does not confirm the archived qualification"
            )
    if not index.exists():
        return
    listed = {}
    for line in index.read_text().splitlines():
        digest, name = line.split("  ", 1)
        name = name.removeprefix("./")
        if name in listed:
            raise ValueError(f"duplicate entry in archive index: {name}")
        listed[name] = digest
    exits = [
        f"{name}.exit-status"
        for role in VALIDATORS
        for name in (role, f"{role}-{role}")
    ]
    for name in ("analysis.json", "result.json", *exits):
        if name in listed or (archive / name).is_file():
            expected[name] = file_hash(archive / name)
    for name, digest in expected.items():
        if listed.get(name) != digest:
            raise ValueError(f"{name} differs from the retained checksum index")


def verify(archive, binary, analyzer=None):
    """Replay the checked-out analyzer on the bytes identified by each collector."""
    manifest = archive / "manifest.json"
    plan = json.loads(manifest.read_text())
    validate_manifest(plan)
    digest = file_hash(binary)
    if plan["artifact_sha256"] != digest:
        raise ValueError("candidate node SHA-256 differs from qualification manifest")
    nodes = plan["nodes"]
    if (
        plan["workload_kind"] != "minting"
        or {n["node_id"] for n in nodes} != set(VALIDATORS)
        or len({n["host_id"] for n in nodes}) != len(VALIDATORS)
        or [n["node_id"] for n in nodes if n["producer"]] != [PRODUCER]
        or len({n["chain_id"] for n in nodes}) != 1
    ):
        raise ValueError(
            "qualification requires five dedicated validators on one chain with n3 minting"
        )
    manifest_sha = manifest_digest(plan)
    samples = []
    expected = {}
    for role in VALIDATORS:
        # The exporter prefixes files with their source host. Plain names are
        # also used when copying the five original collector outputs directly.
        paths = [
            p
            for p in (archive / f"{role}-{role}.jsonl", archive / f"{role}.jsonl")
            if p.is_file()
        ]
        if len(paths) != 1:
            raise ValueError(f"{role}: expected one original sample file")
        path = paths[0]
        summary = json.loads(path.with_suffix(".summary.json").read_text())
        if (
            summary.get("complete") is not True
            or summary.get("nodes") != [role]
            or summary.get("manifest_sha256") != manifest_sha
        ):
            raise ValueError(f"{role}: incomplete collector or manifest mismatch")
        exit_status = path.with_suffix(".exit-status")
        if exit_status.exists() and exit_status.read_text().strip() != "0":
            raise ValueError(f"{role}: collector exited unsuccessfully")
        samples.append(path)
        expected[str(path)] = summary["out_sha256"]

    archived = archive / "analysis.json"
    if archived.exists():
        assessment = json.loads(archived.read_text())
        if (
            assessment.get("status") != "passed"
            or assessment.get("scope") != "limited_minting"
            or assessment.get("manifest_sha256") != manifest_sha
        ):
            raise ValueError("archived assessment did not pass for this manifest")

    # The detached driver also checks startup, warmup and collector exit codes.
    # Preserve an explicit hold from this wider check on newer archives.
    driver = archive / "result.json"
    if driver.exists():
        recorded = json.loads(driver.read_text())
        if (
            recorded.get("qualified") is not True
            or recorded.get("run") != plan["campaign_id"]
            or recorded.get("commit") != plan["source_revision"]
            or recorded.get("artifact_sha256") != digest
            or recorded.get("status") != "passed"
            or recorded.get("scope") != "limited_minting"
        ):
            raise ValueError("fleet driver did not qualify this run and node")

    analyzer = analyzer or Path(__file__).with_name("sustained_memory.py")
    replay = subprocess.run(
        [sys.executable, "-I", str(analyzer), str(manifest), *map(str, samples)],
        capture_output=True,
        text=True,
        check=False,
    )
    result = json.loads(replay.stdout)
    if (
        replay.returncode
        or result.get("status") != "passed"
        or result.get("scope") != "limited_minting"
    ):
        reasons = result.get("failures", []) + result.get("incomplete_reasons", [])
        raise ValueError("minting assessment did not pass: " + ", ".join(reasons[:3]))
    observed = {f["path"]: f["sha256"] for f in result["observation_files"]}
    if result["manifest_sha256"] != manifest_sha or observed != expected:
        raise ValueError(
            "replayed inputs differ from the manifest or collector checksums"
        )
    indexed = {"manifest.json": file_hash(manifest)}
    for path in samples:
        indexed[path.name] = observed[str(path)]
        summary = path.with_suffix(".summary.json")
        indexed[summary.name] = file_hash(summary)
    check_archive_index(archive, indexed)
    return {
        "qualification_run": plan["campaign_id"],
        "source_revision": plan["source_revision"],
        "artifact_sha256": digest,
        "manifest_sha256": manifest_sha,
        "scope": result["scope"],
        "samples": {
            role: {
                "count": result["nodes"][role]["sample_count"],
                "sha256": observed[str(path)],
            }
            for role, path in zip(VALIDATORS, samples)
        },
    }


def check_fleet_shape(inventory, run, roles):
    """Compare observed instance types in the completed cohort with the new plan."""
    expected = {r["role"]: r["type"] for r in roles if r["role"] in VALIDATORS}
    hosts = [r for r in inventory if r["role"] in VALIDATORS]
    if (
        set(expected) != set(VALIDATORS)
        or len(hosts) != len(VALIDATORS)
        or len({h["id"] for h in hosts if h["id"]}) != len(VALIDATORS)
        or {h["role"]: h["type"] for h in hosts} != expected
        or any(h["hostname"] != f"{h['role']}.{run}.internal" for h in hosts)
    ):
        raise ValueError(
            "qualification fleet inventory differs from the payment fleet validator types or identity"
        )
    return expected


def bind(fleet, campaign_id, archive=None, binary=None, analyzer=None, *, roles=None):
    """Retain the qualified inputs once. Retries replay them before provisioning."""
    path = fleet / "payment-memory-qualification.json"
    previous = json.loads(path.read_text()) if path.exists() else None
    if archive is None and previous is not None:
        archive = Path(previous["archive"])
    if binary is None and previous is not None:
        binary = Path(previous["binary"])
    if archive is None or binary is None:
        raise ValueError(
            "payment fleet requires --memory-evidence and the candidate --node"
        )
    archive, binary = archive.resolve(), binary.resolve()
    receipt = verify(archive, binary, analyzer)
    if roles is not None:
        run = validate_run_id(receipt["qualification_run"])
        inventory = archive / "inventory.json"
        if not inventory.is_file():
            inventory = fleet.parent / run / "inventory.json"
        receipt["validator_types"] = check_fleet_shape(
            json.loads(inventory.read_text()), run, roles
        )
    binding = {
        "fleet": campaign_id,
        "archive": str(archive),
        "binary": str(binary),
        "receipt": receipt,
    }
    if previous is None:
        # Publish a complete file without replacing a concurrent operator's pin.
        with tempfile.NamedTemporaryFile(dir=fleet, delete=False, mode="w") as tmp:
            try:
                json.dump(binding, tmp, indent=2, sort_keys=True)
                tmp.write("\n")
                tmp.flush()
                os.fsync(tmp.fileno())
                try:
                    os.link(tmp.name, path)
                except FileExistsError:
                    pass
            finally:
                os.unlink(tmp.name)
        previous = json.loads(path.read_text())
    if previous["fleet"] != campaign_id or previous["receipt"] != receipt:
        raise ValueError(
            "qualified inputs differ from the payment fleet's frozen receipt"
        )
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "archive",
        type=Path,
        help="directory containing manifest.json and the five collector outputs",
    )
    parser.add_argument(
        "node", type=Path, help="Linux node executable selected for the payment fleet"
    )
    args = parser.parse_args()
    try:
        receipt = verify(args.archive, args.node)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(2, f"Memory qualification held: {error}\n")
    print(json.dumps(receipt, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
