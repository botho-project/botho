#!/usr/bin/env python3
"""Read one Linux process/cgroup sample and join a fresh runner RPC observation."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

from sustained_memory import manifest_digest, validate_manifest


def process_stat(proc, pid):
    # comm may contain spaces and parentheses; fields after its final ')' start
    # with state (field 3), not PID (field 1).
    fields = (proc / str(pid) / "stat").read_text().rsplit(")", 1)[1].split()
    return fields[19], (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def identity(
    pid,
    proc=Path("/proc"),
    machine_id=Path("/etc/machine-id"),
    cgroups=Path("/sys/fs/cgroup"),
):
    start, _ = process_stat(proc, pid)
    with (proc / str(pid) / "exe").open("rb") as binary:
        hasher = hashlib.sha256()
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            hasher.update(chunk)
        digest = hasher.hexdigest()
    again, _ = process_stat(proc, pid)
    if again != start:
        raise ValueError("process changed while reading executable")
    return {
        "pid": pid,
        "process_start_id": f"{(proc / 'sys/kernel/random/boot_id').read_text().strip()}:{start}",
        "artifact_sha256": digest,
        "host_id": machine_id.read_text().strip(),
        "cgroup_path": str(cgroup_path(pid, proc, cgroups)),
    }


def key_values(path):
    return {
        key: int(value)
        for key, value in (line.split() for line in path.read_text().splitlines())
    }


def cgroup_path(pid, proc=Path("/proc"), cgroups=Path("/sys/fs/cgroup")):
    paths = [
        line[3:]
        for line in (proc / str(pid) / "cgroup").read_text().splitlines()
        if line.startswith("0::")
    ]
    if len(paths) != 1:
        raise ValueError("one unified cgroup v2 path required")
    group = (cgroups / paths[0].lstrip("/")).resolve()
    if not group.is_relative_to(cgroups.resolve()):
        raise ValueError("cgroup path escapes root")
    return group


def process_metrics(pid, proc=Path("/proc"), cgroups=Path("/sys/fs/cgroup")):
    status = {}
    for line in (proc / str(pid) / "status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "RssAnon", "VmSwap"):
            amount, unit = value.split()
            if unit != "kB":
                raise ValueError(f"unexpected /proc unit: {unit}")
            status[key] = int(amount) * 1024
    group = cgroup_path(pid, proc, cgroups)
    cpu = key_values(group / "cpu.stat")
    _, cpu_seconds = process_stat(proc, pid)
    return {
        "rss_bytes": status["VmRSS"],
        "anonymous_bytes": status["RssAnon"],
        "swap_bytes": status["VmSwap"],
        "cpu_seconds": cpu_seconds,
        "cgroup_path": str(group),
        "cgroup_memory_bytes": int((group / "memory.current").read_text()),
        "cgroup_swap_bytes": int((group / "memory.swap.current").read_text()),
        "cgroup_memory_events": key_values(group / "memory.events"),
        "cgroup_cpu_throttled_seconds": cpu["throttled_usec"] / 1_000_000,
        "cgroup_cpu_stat": cpu,
        "cgroup_pressure": {
            kind: (group / f"{kind}.pressure").read_text().strip()
            for kind in ("cpu", "memory", "io")
        },
    }


def sample(
    plan,
    node,
    observation,
    *,
    proc=Path("/proc"),
    cgroups=Path("/sys/fs/cgroup"),
    machine_id=Path("/etc/machine-id"),
    now=time.time,
):
    """Never substitute zeros for missing evidence; preserve errors in the row."""
    started = now()
    row = {
        **node,
        "schema": 1,
        "campaign_id": plan["campaign_id"],
        "source_revision": plan["source_revision"],
        "manifest_sha256": manifest_digest(plan),
        "observed_at_unix_s": started,
        "errors": [],
    }
    try:
        before = identity(node["pid"], proc, machine_id, cgroups)
        row.update(before)
        row.update(process_metrics(node["pid"], proc, cgroups))
        after_start, _ = process_stat(proc, node["pid"])
        if before["process_start_id"].rsplit(":", 1)[-1] != after_start:
            raise ValueError("process changed during sampling")
    except (OSError, ValueError, KeyError, IndexError) as error:
        row["errors"].append(f"process observation: {error}")
    try:
        timestamp = observation["observed_at_unix_s"]
        if (
            not isinstance(timestamp, (int, float))
            or not 0 <= started - timestamp <= plan["max_gap_s"]
        ):
            raise ValueError("RPC observation stale or from the future")
        for key in ("pid", "process_start_id", "chain_id"):
            if observation[key] != row.get(key):
                raise ValueError(f"RPC observation {key} mismatch")
        for key in ("chain_height", "chain_hash", "confirmed_transfers", "mining"):
            row[key] = observation[key]
        row["rpc_observed_at_unix_s"] = timestamp
        row["errors"].extend(observation.get("errors", []))
    except (ValueError, KeyError, TypeError) as error:
        row["errors"].append(f"RPC observation: {error}")
    row["sample_duration_s"] = now() - started
    if row["sample_duration_s"] > plan["max_gap_s"]:
        row["errors"].append("sample acquisition exceeded freshness budget")
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--identity", type=int, metavar="PID")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--node")
    parser.add_argument("--observation", type=Path)
    args = parser.parse_args()
    try:
        if args.identity is not None:
            print(json.dumps(identity(args.identity)))
            return 0
        if not args.manifest or not args.node or not args.observation:
            parser.error("provide --identity PID or --manifest/--node/--observation")
        plan = validate_manifest(json.loads(args.manifest.read_text()))
        node = next(n for n in plan["nodes"] if n["node_id"] == args.node)
        try:
            observation = json.loads(args.observation.read_text())
        except (OSError, ValueError) as error:
            observation = {"errors": [f"observation file: {error}"]}
        row = sample(plan, node, observation)
        print(json.dumps(row, allow_nan=False))
        return 2 if row["errors"] else 0
    except (OSError, ValueError, KeyError, TypeError, StopIteration) as error:
        parser.exit(2, f"sampler configuration/identity error: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
