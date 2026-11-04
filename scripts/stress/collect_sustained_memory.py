#!/usr/bin/env python3
"""Fixed-schedule, read-only collector for the sustained-memory qualification.

This is the tick loop that ``docs/operations/sustained-memory-qualification.md``
previously left to an untracked runner script (#1452). It joins the two
existing per-tick halves in one process on each node host:

* ``rpc_sustained_memory`` turns one read-only ``node_getStatus`` reply into an
  observation (never a write RPC, never a transaction);
* ``sample_sustained_memory`` reads local ``/proc`` and cgroup v2 evidence and
  re-verifies the process identity on every row.

Three subcommands:

``schedule``
    Print the fixed profile timeline for a chosen ``started_at`` (or a chosen
    producer ``--mint-until``): the absolute mint deadline to launch the
    producer with, every phase boundary, the collection end, the independent
    six-hour lab deadline and a recommended ``node_end`` for
    ``fleet_nodes.py``'s ``mint_until`` profile.

``manifest``
    Write the immutable manifest once, from the ``--identity PID`` records
    each host produced. It refuses mismatched artifacts, a ``started_at``
    already in the past (readiness delay may never shorten warmup) and an
    existing output file.

``collect``
    Sample the named local nodes on the absolute grid
    ``started_at + k * sample_interval_s`` until the fixed profile end, and
    append every row (including error rows) to a new, exclusively created
    JSONL file, fsyncing each tick. It never rebases the schedule: a late
    start, a slow tick or a collector restart leaves a gap that the analyzer
    reports as ``incomplete``; missed ticks are never replayed.

The collector never launches, pauses, signals or reconfigures a node.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import ssl
import sys
import threading
import time

from rpc_sustained_memory import (
    RPC_URL_HELP,
    fetch_status,
    observation_from_status,
    read_transfer_source,
    rpc_url_problem,
)
from sample_sustained_memory import identity, sample
from sustained_memory import (
    THRESHOLD_DEFAULTS,
    manifest_digest,
    profile_phases,
    validate_manifest,
    windows,
)

# The issue's independent hard bound on one qualification attempt.
HARD_LAB_DEADLINE_S = 6 * 3600
# A collector may wait at most this long for t0; a longer wait is almost
# certainly a wrong manifest or clock.
MAX_LEAD_S = 3600
# Nodes must outlive the last collection tick; fleet_nodes' node-stop timer
# fires at node_end, so leave this margin after the profile end.
NODE_END_MARGIN_S = 900
SAMPLE_INTERVAL_S = 5
MAX_GAP_S = 15
IDENTITY_KEYS = ("pid", "process_start_id", "artifact_sha256", "host_id", "cgroup_path")


def profile_schedule(workload, started_at, interval=SAMPLE_INTERVAL_S):
    """Absolute timeline of the fixed reviewed profile; never rebased."""
    phases, cursor = [], started_at
    for phase in profile_phases(workload):
        phases.append(
            {
                "name": phase["name"],
                "start_unix_s": cursor,
                "end_unix_s": cursor + phase["duration_s"],
            }
        )
        cursor += phase["duration_s"]
    end = cursor
    return {
        "workload_kind": workload,
        "started_at_unix_s": started_at,
        "phases": phases,
        # The producer's --testnet-mint-until: the end of sustained minting.
        # Payments profiles keep their miner configuration; no deadline.
        "mint_until_unix_s": phases[1]["end_unix_s"] if workload == "minting" else None,
        "profile_end_unix_s": end,
        "tick_count": math.ceil((end - started_at) / interval) + 1,
        "recommended_node_end_unix_s": end + NODE_END_MARGIN_S,
        "hard_lab_deadline_unix_s": started_at + HARD_LAB_DEADLINE_S,
    }


def started_at_for_mint_until(mint_until):
    """t0 such that warmup + sustained ends exactly at the producer's deadline."""
    head = profile_phases("minting")[:2]
    return mint_until - sum(phase["duration_s"] for phase in head)


def build_manifest(
    *,
    campaign_id,
    source_revision,
    workload,
    started_at,
    chain_id,
    identities,
    producers,
    min_blocks,
    min_transfers,
    limits=None,
    now,
):
    """Assemble and validate one manifest from per-host ``--identity`` records.

    ``identities`` maps node id to the JSON ``sample_sustained_memory.py
    --identity PID`` printed on that node's host.
    """
    if not isinstance(started_at, int) or isinstance(started_at, bool):
        raise ValueError("started_at must be an integer UNIX second")
    if started_at < now:
        raise ValueError(
            "started_at is in the past: readiness delay may never shorten warmup; "
            "choose a new experiment"
        )
    if started_at - now > MAX_LEAD_S:
        raise ValueError(f"started_at is more than {MAX_LEAD_S}s ahead")
    unknown = set(producers) - set(identities)
    if unknown:
        raise ValueError(f"producer not among nodes: {sorted(unknown)}")
    artifacts = set()
    nodes = []
    for node_id, record in identities.items():
        if not isinstance(record, dict) or any(
            key not in record for key in IDENTITY_KEYS
        ):
            raise ValueError(f"{node_id}: identity record lacks {IDENTITY_KEYS}")
        artifacts.add(record["artifact_sha256"])
        nodes.append(
            {
                "node_id": node_id,
                "host_id": record["host_id"],
                "pid": record["pid"],
                "process_start_id": record["process_start_id"],
                "chain_id": chain_id,
                "producer": node_id in producers,
                "cgroup_path": record["cgroup_path"],
            }
        )
    if len(artifacts) != 1:
        raise ValueError("all measured processes must run the same artifact SHA256")
    plan = {
        "schema": 1,
        "campaign_id": campaign_id,
        "source_revision": source_revision,
        "artifact_sha256": artifacts.pop(),
        "started_at_unix_s": started_at,
        "phases": profile_phases(workload),
        "sample_interval_s": SAMPLE_INTERVAL_S,
        "max_gap_s": MAX_GAP_S,
        "active_window_s": 300,
        "min_blocks_per_active_window": min_blocks,
        "min_confirmed_transfers_per_active_window": min_transfers,
        "workload_kind": workload,
        "nodes": nodes,
    }
    for key, value in (limits or {}).items():
        if value is not None:
            plan[key] = value
    return validate_manifest(plan)


def preflight(plan, targets, *, proc, cgroups, machine_id):
    """Refuse before writing anything if a target is not the manifest's process."""
    problems = []
    for node, _url, _transfer in targets:
        try:
            actual = identity(node["pid"], proc, machine_id, cgroups)
        except (OSError, ValueError, IndexError) as error:
            problems.append(f"{node['node_id']}: process identity unreadable: {error}")
            continue
        expected = {**node, "artifact_sha256": plan["artifact_sha256"]}
        for key in IDENTITY_KEYS:
            if actual[key] != expected[key]:
                problems.append(
                    f"{node['node_id']}: {key} {actual[key]!r} is not the manifest's {expected[key]!r}"
                )
    return problems


def open_exclusive(path):
    """A new append-only raw file; an existing one is evidence, never reused."""
    descriptor = os.open(
        path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_APPEND, 0o640
    )
    return os.fdopen(descriptor, "a")


def collect(
    plan,
    targets,
    output,
    *,
    now,
    sleep,
    fetch,
    stopped=lambda: False,
    expected_network=None,
    rpc_timeout=5.0,
    context=None,
    proc=Path("/proc"),
    cgroups=Path("/sys/fs/cgroup"),
    machine_id=Path("/etc/machine-id"),
    log=lambda message: None,
    fsync=os.fsync,
):
    """Sample ``targets`` on the fixed grid and append rows to ``output``.

    ``targets`` is ``[(manifest_node, rpc_url, transfer_source_path_or_None)]``.
    Returns a summary; ``complete`` is true only if every grid tick from t0 to
    the profile end produced an error-free row for every target.
    """
    start = plan["started_at_unix_s"]
    interval = plan["sample_interval_s"]
    end = windows(plan)[-1][2]
    last = math.ceil((end - start) / interval)
    hard = min(start + HARD_LAB_DEADLINE_S, end + plan["max_gap_s"])
    summary = {
        "schema": 1,
        "manifest_sha256": manifest_digest(plan),
        "nodes": [node["node_id"] for node, _, _ in targets],
        "ticks_sampled": 0,
        "ticks_skipped": 0,
        "rows_written": 0,
        "error_rows": 0,
        "first_tick_unix_s": None,
        "last_row_unix_s": None,
        "reached_profile_end": False,
        "interrupted": False,
    }
    previous = -1
    while True:
        if stopped():
            summary["interrupted"] = True
            break
        current = now()
        if current > hard:
            log("hard deadline reached before the profile end")
            break
        # The tick that is due now. A tick less than half an interval late is
        # still taken (rows carry their actual timestamps); anything later is
        # skipped, never replayed.
        due = max(
            previous + 1, math.ceil((current - start - interval / 2) / interval), 0
        )
        if due > last:
            break
        if due > previous + 1:
            skipped = due - (previous + 1)
            summary["ticks_skipped"] += skipped
            log(f"skipped {skipped} tick(s) before tick {due}; the gap is not replayed")
        tick_at = start + due * interval
        if tick_at > current:
            sleep(tick_at - current)
            if stopped():
                summary["interrupted"] = True
                break
        for node, url, transfer_path in targets:
            status, errors = fetch(url, rpc_timeout, context)
            observed_at = now()
            transfer = None
            if plan["workload_kind"] == "payments" and transfer_path is not None:
                transfer = read_transfer_source(transfer_path, plan, node, observed_at)
            observation = observation_from_status(
                plan,
                node,
                status,
                observed_at,
                rpc_errors=errors,
                expected_network=expected_network,
                transfer_source=transfer,
            )
            row = sample(
                plan,
                node,
                observation,
                proc=proc,
                cgroups=cgroups,
                machine_id=machine_id,
                now=now,
            )
            output.write(json.dumps(row, allow_nan=False) + "\n")
            summary["rows_written"] += 1
            if row["errors"]:
                summary["error_rows"] += 1
                log(f"tick {due} {node['node_id']}: {'; '.join(row['errors'])}")
            summary["last_row_unix_s"] = row["observed_at_unix_s"]
        output.flush()
        fsync(output.fileno())  # each tick is durable before the next one
        if summary["first_tick_unix_s"] is None:
            summary["first_tick_unix_s"] = tick_at
        summary["ticks_sampled"] += 1
        previous = due
        if due == last:
            summary["reached_profile_end"] = True
            break
    summary["complete"] = (
        summary["reached_profile_end"]
        and not summary["interrupted"]
        and summary["ticks_skipped"] == 0
        and summary["error_rows"] == 0
        and summary["first_tick_unix_s"] == start
    )
    return summary


def _key_value(text, what):
    key, separator, value = text.partition("=")
    if not separator or not key or not value:
        raise argparse.ArgumentTypeError(f"{what} must be NODE=VALUE")
    return key, value


def _read_json(path):
    return json.loads(Path(path).read_text())


def schedule_command(args, now):
    if (args.started_at is None) == (args.mint_until is None):
        raise ValueError("give exactly one of --started-at or --mint-until")
    if args.mint_until is not None:
        if args.workload != "minting":
            raise ValueError("--mint-until applies only to the minting profile")
        started_at = started_at_for_mint_until(args.mint_until)
    else:
        started_at = args.started_at
    result = profile_schedule(args.workload, started_at)
    result["seconds_until_start"] = started_at - now()
    print(json.dumps(result, indent=2))
    return 0


def manifest_command(args, now):
    identities = {}
    for spec in args.node:
        node_id, path = _key_value(spec, "--node")
        if node_id in identities:
            raise ValueError(f"duplicate node {node_id}")
        identities[node_id] = _read_json(path)
    plan = build_manifest(
        campaign_id=args.campaign_id,
        source_revision=args.source_revision,
        workload=args.workload,
        started_at=args.started_at,
        chain_id=args.chain_id,
        identities=identities,
        producers=set(args.producer),
        min_blocks=args.min_blocks,
        min_transfers=args.min_transfers,
        limits={
            "slope_limit_mib_per_hour": args.slope_limit,
            "net_growth_limit_mib": args.net_growth_limit,
            "rss_plus_swap_limit_bytes": args.rss_plus_swap_limit_bytes,
            "cgroup_memory_plus_swap_limit_bytes": args.cgroup_memory_plus_swap_limit_bytes,
        },
        now=now(),
    )
    data = json.dumps(plan, indent=2, sort_keys=True) + "\n"
    descriptor = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(descriptor, "w") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    print(
        json.dumps(
            {
                "manifest": str(args.out),
                "manifest_sha256": manifest_digest(plan),
                "file_sha256": hashlib.sha256(data.encode()).hexdigest(),
                "schedule": profile_schedule(
                    plan["workload_kind"], plan["started_at_unix_s"]
                ),
            },
            indent=2,
        )
    )
    return 0


def collect_command(args, now, sleep, fetch, stopped, paths):
    plan = validate_manifest(_read_json(args.manifest))
    by_id = {node["node_id"]: node for node in plan["nodes"]}
    transfers = dict(
        _key_value(spec, "--transfer-source") for spec in args.transfer_source
    )
    if transfers and plan["workload_kind"] != "payments":
        raise ValueError("--transfer-source is only valid for a payments manifest")
    targets, seen = [], set()
    for spec in args.node:
        node_id, url = _key_value(spec, "--node")
        if node_id not in by_id:
            raise ValueError(f"{node_id} is not a manifest node")
        if node_id in seen:
            raise ValueError(f"duplicate node {node_id}")
        seen.add(node_id)
        problem = rpc_url_problem(url, args.ca_file)
        if problem:
            raise ValueError(f"{node_id}: {problem}")
        transfer = transfers.pop(node_id, None)
        targets.append((by_id[node_id], url, Path(transfer) if transfer else None))
    if transfers:
        raise ValueError(
            f"--transfer-source for an uncollected node: {sorted(transfers)}"
        )
    if not 0 < args.rpc_timeout <= plan["max_gap_s"]:
        raise ValueError("--rpc-timeout must be positive and within max_gap_s")
    current = now()
    end = windows(plan)[-1][2]
    if current > end:
        raise ValueError("the fixed profile has already ended; start a new experiment")
    if plan["started_at_unix_s"] - current > MAX_LEAD_S:
        raise ValueError(f"t0 is more than {MAX_LEAD_S}s ahead")
    problems = preflight(plan, targets, **paths)
    if problems:
        raise ValueError("preflight identity mismatch: " + "; ".join(problems))
    if current > plan["started_at_unix_s"] + plan["max_gap_s"]:
        print(
            "warning: started after t0; warmup coverage is already missing and "
            "the run will be incomplete (evidence is still recorded)",
            file=sys.stderr,
        )
    context = None
    if any(url.startswith("https://") for _, url, _ in targets):
        context = ssl.create_default_context(cafile=str(args.ca_file))
    with open_exclusive(args.out) as output:
        summary = collect(
            plan,
            targets,
            output,
            now=now,
            sleep=sleep,
            fetch=fetch,
            stopped=stopped,
            expected_network=args.expected_network,
            rpc_timeout=args.rpc_timeout,
            context=context,
            log=lambda message: print(message, file=sys.stderr, flush=True),
            **paths,
        )
    summary["out"] = str(args.out)
    summary["out_sha256"] = hashlib.sha256(Path(args.out).read_bytes()).hexdigest()
    print(json.dumps(summary, indent=2))
    return 0 if summary["complete"] else 2


def parser():
    top = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = top.add_subparsers(dest="command", required=True)

    sched = sub.add_parser("schedule", help="print the fixed absolute timeline")
    sched.add_argument("--workload", choices=("minting", "payments"), default="minting")
    sched.add_argument("--started-at", type=int)
    sched.add_argument("--mint-until", type=int, help="producer's --testnet-mint-until")

    man = sub.add_parser("manifest", help="write the immutable manifest once")
    man.add_argument("--out", type=Path, required=True)
    man.add_argument("--campaign-id", required=True)
    man.add_argument("--source-revision", required=True)
    man.add_argument("--workload", choices=("minting", "payments"), default="minting")
    man.add_argument("--started-at", type=int, required=True)
    man.add_argument("--chain-id", required=True)
    man.add_argument(
        "--node", action="append", required=True, metavar="NODE=IDENTITY_JSON"
    )
    man.add_argument("--producer", action="append", required=True, metavar="NODE")
    man.add_argument(
        "--min-blocks", type=int, required=True, help="per fixed 300 s active window"
    )
    man.add_argument("--min-transfers", type=int, default=0)
    man.add_argument(
        "--slope-limit",
        type=float,
        help="tighten only (<= %s)" % THRESHOLD_DEFAULTS["slope_limit_mib_per_hour"],
    )
    man.add_argument(
        "--net-growth-limit",
        type=float,
        help="tighten only (<= %s)" % THRESHOLD_DEFAULTS["net_growth_limit_mib"],
    )
    man.add_argument("--rss-plus-swap-limit-bytes", type=int)
    man.add_argument("--cgroup-memory-plus-swap-limit-bytes", type=int)

    col = sub.add_parser("collect", help="sample local nodes on the fixed grid")
    col.add_argument("--manifest", type=Path, required=True)
    col.add_argument(
        "--node",
        action="append",
        required=True,
        metavar="NODE=RPC_URL",
        help=RPC_URL_HELP,
    )
    col.add_argument(
        "--out", type=Path, required=True, help="new raw JSONL; must not exist"
    )
    col.add_argument("--expected-network")
    col.add_argument("--ca-file", type=Path)
    col.add_argument("--rpc-timeout", type=float, default=4.0)
    col.add_argument(
        "--transfer-source", action="append", default=[], metavar="NODE=FILE"
    )
    return top


def main(
    argv=None,
    *,
    now=time.time,
    sleep=None,
    fetch=fetch_status,
    stopped=None,
    paths=None,
):
    args = parser().parse_args(argv)
    paths = paths or {
        "proc": Path("/proc"),
        "cgroups": Path("/sys/fs/cgroup"),
        "machine_id": Path("/etc/machine-id"),
    }
    try:
        if args.command == "schedule":
            return schedule_command(args, now)
        if args.command == "manifest":
            return manifest_command(args, now)
        if sleep is None or stopped is None:
            # SIGTERM/SIGINT end collection after the current tick; the raw
            # file keeps every written row and the summary says interrupted.
            event = threading.Event()
            for number in (signal.SIGTERM, signal.SIGINT):
                signal.signal(number, lambda *_: event.set())
            sleep = sleep or event.wait
            stopped = stopped or event.is_set
        return collect_command(args, now, sleep, fetch, stopped, paths)
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        argparse.ArgumentTypeError,
    ) as error:
        print(f"{args.command}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
