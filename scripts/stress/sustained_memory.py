#!/usr/bin/env python3
"""Analyze immutable, local sustained-memory observations; never launch a node.

Two independent conditions must both hold for ``passed``:

* The observation evidence itself is complete and within the gates: fixed
  warmup/measurement/follow-up coverage, inter-sample gaps of at most the
  manifest's ``max_gap_s`` (15 s), no errors, identity changes or counter
  regressions, and growth within the thresholds.
* The collector finished cleanly and says so (#1902). Every raw file must end
  with exactly one ``collector_terminal`` record, which
  ``collect_sustained_memory.py`` writes only after sampling the final profile
  tick without interruption. That record must carry the manifest's SHA256,
  the full expected tick count (``ceil(profile / sample_interval_s) + 1``) as
  written, zero missed ticks, zero error rows, a first tick at exactly t0 and
  ``complete: true``. Every manifest node must be covered by exactly one such
  record, and each node's observation count must equal the written ticks.

The 15 s gap tolerance alone admits a 4-12 s late start, a single slow tick or
a short collector restart, all of which the collector already knows were
incomplete. The terminal record makes the analyzer fail closed on them: any
missed tick, a missing or extra terminal record, a terminal record for another
manifest, or rows appended after it, yields ``incomplete``. Observation
failures (growth, resource limits, identity changes) still take precedence
over ``incomplete`` and are never hidden by it.

Threat model: the terminal record is an unauthenticated self-attestation, with
no MAC or signature. It detects incomplete collection and operator error, such
as wrong, missing, extra, truncated or concatenated files or a stale manifest.
It does not detect deliberate fabrication: a hand-built record over clean rows,
or one edited row timestamp with the count unchanged, still passes. Integrity
rests on the raw-file SHA256s, the collector's stdout summary and log, and the
manifest, all archived at collection time.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics

MIB = 1024 * 1024
SLOPE_LIMIT = 10.0
NET_LIMIT = 64.0
THRESHOLD_DEFAULTS = {
    "slope_limit_mib_per_hour": SLOPE_LIMIT,
    "net_growth_limit_mib": NET_LIMIT,
}
IDENTITY = (
    "campaign_id",
    "source_revision",
    "artifact_sha256",
    "host_id",
    "chain_id",
    "cgroup_path",
)
GAUGES = (
    "rss_bytes",
    "anonymous_bytes",
    "swap_bytes",
    "cgroup_memory_bytes",
    "cgroup_swap_bytes",
)
COUNTERS = (
    "cpu_seconds",
    "cgroup_cpu_throttled_seconds",
    "chain_height",
    "confirmed_transfers",
)
# The ``record`` value of the collector's terminal completion row (#1902).
TERMINAL_RECORD = "collector_terminal"


def number(value):
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and 0 <= value < 2**64
        and math.isfinite(value)
    )


def manifest_digest(plan):
    """Bind observations to the exact immutable manifest, independent of whitespace."""
    data = json.dumps(plan, sort_keys=True, separators=(",", ":"), allow_nan=False)
    return hashlib.sha256(data.encode()).hexdigest()


def validate_manifest(plan):
    if not isinstance(plan, dict):
        raise ValueError("manifest must be an object")
    fixed = {"schema": 1, "active_window_s": 300}
    for key, expected in fixed.items():
        if plan.get(key) != expected:
            raise ValueError(
                f"{key} must be {expected}; create a separately reviewed profile to change it"
            )
    for key in ("campaign_id", "source_revision", "artifact_sha256"):
        if not isinstance(plan.get(key), str) or not plan[key]:
            raise ValueError(f"missing {key}")
    if len(plan["artifact_sha256"]) != 64 or any(
        c not in "0123456789abcdef" for c in plan["artifact_sha256"]
    ):
        raise ValueError("artifact_sha256 must be lowercase SHA256")
    for key in (
        "started_at_unix_s",
        "sample_interval_s",
        "max_gap_s",
        "min_blocks_per_active_window",
        "min_confirmed_transfers_per_active_window",
    ):
        if not number(plan.get(key)):
            raise ValueError(f"invalid {key}")
    if (
        not 0 < plan["sample_interval_s"] <= 5
        or not plan["sample_interval_s"] <= plan["max_gap_s"] <= 15
    ):
        raise ValueError("sample interval must be <=5s and max gap <=15s")
    if plan["min_blocks_per_active_window"] < 1:
        raise ValueError("active workload requires positive block progress")
    if plan.get("workload_kind") not in ("minting", "payments"):
        raise ValueError("workload_kind must be minting or payments")
    if (
        plan["workload_kind"] == "payments"
        and plan["min_confirmed_transfers_per_active_window"] < 1
    ):
        raise ValueError("payments require positive confirmed transfer progress")
    if plan.get("phases") != profile_phases(plan["workload_kind"]):
        raise ValueError("phases must match the fixed reviewed workload profile")
    nodes = plan.get("nodes", [])
    if (
        not isinstance(nodes, list)
        or not nodes
        or any(
            not isinstance(n, dict) or not isinstance(n.get("node_id"), str)
            for n in nodes
        )
        or len({n["node_id"] for n in nodes}) != len(nodes)
    ):
        raise ValueError("nonempty unique nodes required")
    processes, groups = set(), set()
    for node in nodes:
        for key in (
            "node_id",
            "host_id",
            "process_start_id",
            "chain_id",
            "cgroup_path",
        ):
            if not isinstance(node.get(key), str) or not node[key]:
                raise ValueError(f"invalid node {key}")
        if (
            not isinstance(node.get("pid"), int)
            or isinstance(node["pid"], bool)
            or node["pid"] <= 0
        ):
            raise ValueError("invalid PID")
        if not isinstance(node.get("producer"), bool):
            raise ValueError("node producer flag required")
        process = (node["host_id"], node["pid"])
        if process in processes:
            raise ValueError("nodes cannot share a process")
        processes.add(process)
        group = (node["host_id"], node["cgroup_path"])
        if group in groups:
            raise ValueError("nodes require distinct per-process cgroups")
        groups.add(group)
    if len({n["chain_id"] for n in nodes}) != 1:
        raise ValueError("all nodes must observe the same declared chain")
    if not any(n["producer"] for n in nodes):
        raise ValueError("at least one declared producer required")
    for key in ("rss_plus_swap_limit_bytes", "cgroup_memory_plus_swap_limit_bytes"):
        if key in plan and (not number(plan[key]) or plan[key] <= 0):
            raise ValueError(f"invalid {key}")
    for key, default in THRESHOLD_DEFAULTS.items():
        # Thresholds are part of the hashed manifest and may only be
        # tightened here. Relaxing one (e.g. far enough to admit the old
        # 49 MiB/hour retention) needs a separately reviewed code change.
        if key in plan and (not number(plan[key]) or not 0 < plan[key] <= default):
            raise ValueError(f"{key} must be positive and at most {default}")
    return plan


def thresholds(plan):
    return {key: plan.get(key, default) for key, default in THRESHOLD_DEFAULTS.items()}


def profile_phases(workload):
    phases = [
        {"name": "warmup", "duration_s": 1800},
        {"name": "sustained", "duration_s": 14400},
    ]
    if workload == "minting":
        phases.extend(
            [
                # #1966: at least one 40 s slot plus the proposal tick. Mints
                # submitted before the deadline stay pending and can land one
                # more block about a slot later (memqual7: +45 s).
                {"name": "settling", "duration_s": 120},
                {"name": "idle", "duration_s": 1800},
            ]
        )
    else:
        for cycle in range(1, 4):
            phases.extend(
                {"name": f"cycle_{cycle}_{phase}", "duration_s": 600}
                for phase in ("active", "idle")
            )
    return phases


def windows(plan):
    cursor = plan["started_at_unix_s"]
    stages = []
    for phase in plan["phases"]:
        end = cursor + phase["duration_s"]
        stages.append((phase["name"], cursor, end))
        cursor = end
    return stages


def expected_ticks(plan):
    """Grid ticks ``started_at + k * sample_interval_s`` through the profile end."""
    start, end = plan["started_at_unix_s"], windows(plan)[-1][2]
    return math.ceil((end - start) / plan["sample_interval_s"]) + 1


def terminal_record(
    plan,
    nodes,
    *,
    first_tick_unix_s,
    ticks_written,
    rows_written,
    error_rows,
    finished_at_unix_s,
):
    """The collector's durable completion record, written once at a clean finish."""
    expected = expected_ticks(plan)
    missed = expected - ticks_written
    return {
        "record": TERMINAL_RECORD,
        "schema": 1,
        "manifest_sha256": manifest_digest(plan),
        "nodes": list(nodes),
        "started_at_unix_s": plan["started_at_unix_s"],
        "first_tick_unix_s": first_tick_unix_s,
        "ticks_expected": expected,
        "ticks_written": ticks_written,
        "ticks_missed": missed,
        "rows_written": rows_written,
        "error_rows": error_rows,
        "finished_at_unix_s": finished_at_unix_s,
        "complete": missed == 0
        and error_rows == 0
        and first_tick_unix_s == plan["started_at_unix_s"],
    }


def is_terminal(row):
    return isinstance(row, dict) and row.get("record") == TERMINAL_RECORD


def _count(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def terminal_problem(plan, record):
    """Why ``record`` does not attest a complete collection, or None."""
    if record.get("schema") != 1 or record.get("manifest_sha256") != manifest_digest(
        plan
    ):
        return "does not match the manifest SHA256"
    nodes = record.get("nodes")
    known = {node["node_id"] for node in plan["nodes"]}
    if (
        not isinstance(nodes, list)
        or not nodes
        or any(not isinstance(n, str) or n not in known for n in nodes)
        or len(set(nodes)) != len(nodes)
    ):
        return "names no, unknown or duplicate manifest nodes"
    fields = ("ticks_expected", "ticks_written", "ticks_missed")
    fields += ("rows_written", "error_rows")
    if any(not _count(record.get(key)) for key in fields):
        return "has missing or invalid tick counts"
    expected = expected_ticks(plan)
    if record["ticks_expected"] != expected:
        return f"expects {record['ticks_expected']} ticks, not the profile's {expected}"
    if record["ticks_missed"] != 0 or record["ticks_written"] != expected:
        return (
            f"missed {record['ticks_missed']} tick(s); "
            f"wrote {record['ticks_written']} of {expected}"
        )
    if record["rows_written"] != record["ticks_written"] * len(nodes):
        return "row count is not one row per node per tick"
    if record["error_rows"] != 0:
        return f"wrote {record['error_rows']} error row(s)"
    if record.get("first_tick_unix_s") != plan["started_at_unix_s"]:
        return "first tick is not t0"
    if record.get("complete") is not True:
        return "is not marked complete"
    return None


def collector_incomplete_reasons(plan, terminals, by_node):
    """Fail closed unless one complete terminal record covers each node (#1902)."""
    reasons, covered = set(), {}
    for record in terminals:
        problem = terminal_problem(plan, record)
        nodes = record.get("nodes")
        label = (
            ",".join(n for n in nodes if isinstance(n, str))
            if isinstance(nodes, list)
            else "?"
        )
        if problem:
            reasons.add(f"collector terminal record for {label or '?'} {problem}")
            continue
        for node in nodes:
            covered[node] = covered.get(node, 0) + 1
    expected = expected_ticks(plan)
    for node in plan["nodes"]:
        name = node["node_id"]
        if covered.get(name, 0) == 0:
            reasons.add(f"{name}: no complete collector terminal record")
        elif covered[name] > 1:
            reasons.add(f"{name}: more than one collector terminal record")
        if len(by_node[name]) != expected:
            reasons.add(
                f"{name}: {len(by_node[name])} observations, "
                f"not the collector's {expected} ticks"
            )
    return reasons


def file_incomplete_reasons(path, rows):
    """Per raw file: exactly one terminal record, as the final line, covering it."""
    indices = [i for i, row in enumerate(rows) if is_terminal(row)]
    if not indices:
        return [f"{path}: missing collector terminal record (no clean finish)"]
    reasons = []
    if len(indices) > 1 or indices[-1] != len(rows) - 1:
        reasons.append(
            f"{path}: collector terminal record is not the single final line"
        )
    nodes = rows[indices[-1]].get("nodes")
    nodes = set(nodes) if isinstance(nodes, list) else set()
    if any(
        row is not None  # an unparseable line is reported separately
        and not is_terminal(row)
        and (not isinstance(row, dict) or row.get("node_id") not in nodes)
        for row in rows
    ):
        reasons.append(f"{path}: rows for a node its terminal record does not cover")
    return reasons


def memory_stats(rows, field):
    times = [
        (r["observed_at_unix_s"] - rows[0]["observed_at_unix_s"]) / 3600 for r in rows
    ]
    values = [(r[field] + r["swap_bytes"]) / MIB for r in rows]
    mt, mv = statistics.mean(times), statistics.mean(values)
    variance = sum((t - mt) ** 2 for t in times)
    slope = (
        sum((t - mt) * (v - mv) for t, v in zip(times, values)) / variance
        if variance
        else 0
    )
    return {
        "slope_mib_per_hour": slope,
        "net_growth_mib": values[-1] - values[0],
        "baseline_mib": values[0],
        "end_mib": values[-1],
        "peak_mib": max(values),
    }


def measurement_is_complete(plan, node, observations, measured):
    """A later missing idle period cannot erase an already observed growth failure."""
    _, left, right = windows(plan)[1]
    gap = plan["max_gap_s"]
    if (
        len(measured) < 2
        or measured[0]["observed_at_unix_s"] > left + gap
        or measured[-1]["observed_at_unix_s"] < right - gap
    ):
        return False
    expected = {**plan, **node, "manifest_sha256": manifest_digest(plan)}
    raw = []
    for row in observations:
        timestamp = row.get("observed_at_unix_s")
        if not number(timestamp):
            return False  # Cannot assign an undated error to a later phase.
        if not left <= timestamp <= right:
            continue
        if (
            row.get("errors") != []
            or row.get("schema") != 1
            or any(
                row.get(k) != expected[k]
                for k in IDENTITY + ("pid", "process_start_id", "manifest_sha256")
            )
            or any(not number(row.get(k)) for k in GAUGES + COUNTERS)
        ):
            return False
        raw.append(row)
    if len(raw) != len(measured):
        return False
    for before, after in zip(raw, raw[1:]):
        if not 0 < after["observed_at_unix_s"] - before["observed_at_unix_s"] <= gap:
            return False
        if any(after[k] < before[k] for k in COUNTERS):
            return False
        if any(
            after["cgroup_memory_events"][k] < before["cgroup_memory_events"][k]
            for k in ("high", "max", "oom", "oom_kill")
        ):
            return False
    return True


def inspect_node(plan, node, observations, failures, incomplete):
    name = node["node_id"]
    stages = windows(plan)
    start, end = stages[0][1], stages[-1][2]
    digest = manifest_digest(plan)
    rows = []
    previous_time = None
    first_events = None
    for row in observations:
        # Independent absolute resource evidence remains a failure even when
        # another field in the same observation is unavailable.
        for limit, resident, swapped in (
            ("rss_plus_swap_limit_bytes", "rss_bytes", "swap_bytes"),
            (
                "cgroup_memory_plus_swap_limit_bytes",
                "cgroup_memory_bytes",
                "cgroup_swap_bytes",
            ),
        ):
            if (
                limit in plan
                and number(row.get(resident))
                and number(row.get(swapped))
                and row[resident] + row[swapped] > plan[limit]
            ):
                failures.add(f"{name}: {limit} exceeded")
        events = row.get("cgroup_memory_events")
        if isinstance(events, dict) and all(
            number(events.get(k)) for k in ("oom", "oom_kill")
        ):
            if first_events is None:
                first_events = events
            elif any(events[k] > first_events[k] for k in ("oom", "oom_kill")):
                failures.add(f"{name}: cgroup OOM events increased")
        expected = {**plan, **node}
        identities = {k: expected[k] for k in IDENTITY}
        identities["manifest_sha256"] = digest
        for key, value in identities.items():
            if key not in row:
                incomplete.add(f"{name}: missing {key}")
            elif row[key] != value:
                failures.add(f"{name}: observation identity or manifest changed")
        if (
            row.get("pid") != node["pid"]
            or row.get("process_start_id") != node["process_start_id"]
        ):
            incomplete.add(f"{name}: process restarted or PID changed")
        if not isinstance(row.get("mining"), bool):
            incomplete.add(f"{name}: missing mining state")
        if row.get("schema") != 1 or row.get("errors") != []:
            incomplete.add(f"{name}: sample errors or schema mismatch")
        timestamp = row.get("observed_at_unix_s")
        if not number(timestamp):
            incomplete.add(f"{name}: invalid timestamp")
            continue
        if previous_time is not None and timestamp <= previous_time:
            incomplete.add(f"{name}: duplicate or nonmonotonic sample time")
        previous_time = timestamp
        if not start <= timestamp <= end + plan["max_gap_s"]:
            incomplete.add(f"{name}: sample outside immutable experiment window")
            continue
        if any(not number(row.get(k)) for k in GAUGES + COUNTERS) or any(
            not isinstance(row.get(k), int) or isinstance(row[k], bool)
            for k in ("chain_height", "confirmed_transfers")
        ):
            incomplete.add(f"{name}: missing or invalid numeric observation")
            continue
        events = row.get("cgroup_memory_events")
        if not isinstance(events, dict) or any(
            not number(events.get(k)) for k in ("high", "max", "oom", "oom_kill")
        ):
            incomplete.add(f"{name}: missing cgroup memory events")
            continue
        block_hash = row.get("chain_hash", "")
        if (
            not isinstance(block_hash, str)
            or len(block_hash) != 64
            or any(c not in "0123456789abcdef" for c in block_hash)
        ):
            incomplete.add(f"{name}: invalid chain hash")
            continue
        rows.append(row)
    rows.sort(key=lambda r: r["observed_at_unix_s"])
    if not rows:
        incomplete.add(f"{name}: no usable observations")
        return {"sample_count": 0}, []
    gap = plan["max_gap_s"]
    if (
        rows[0]["observed_at_unix_s"] > start + gap
        or rows[-1]["observed_at_unix_s"] < end - gap
    ):
        incomplete.add(f"{name}: incomplete fixed warmup/measurement/cycle coverage")
    for previous, row in zip(rows, rows[1:]):
        if row["observed_at_unix_s"] - previous["observed_at_unix_s"] > gap:
            incomplete.add(f"{name}: sample gap exceeds {gap}s")
        for key in COUNTERS:
            if row[key] < previous[key]:
                incomplete.add(f"{name}: {key} counter regressed")
        for key in ("high", "max", "oom", "oom_kill"):
            if row["cgroup_memory_events"][key] < previous["cgroup_memory_events"][key]:
                incomplete.add(f"{name}: cgroup {key} counter regressed")
    result = {
        "sample_count": len(rows),
        "first_sample_unix_s": rows[0]["observed_at_unix_s"],
        "last_sample_unix_s": rows[-1]["observed_at_unix_s"],
        "stages": {},
    }
    for phase, left, right in stages:
        segment = [r for r in rows if left <= r["observed_at_unix_s"] <= right]
        if (
            not segment
            or segment[0]["observed_at_unix_s"] > left + gap
            or segment[-1]["observed_at_unix_s"] < right - gap
        ):
            incomplete.add(f"{name}: {phase} boundary coverage missing")
        if plan["workload_kind"] == "minting" and phase != "settling":
            # Explicit settling absorbs the ten-second control tick and the
            # last in-flight block (#1966); every idle observation must be idle.
            interior = [r for r in segment if left <= r["observed_at_unix_s"] < right]
            expected_mining = phase != "idle" and node.get("producer", False)
            if any(r.get("mining") is not expected_mining for r in interior):
                incomplete.add(
                    f"{name}: {phase} mining state does not match fixed schedule"
                )
        if phase == "idle" or phase.endswith("_idle"):
            # Minting already has an explicit settling phase. A further grace
            # interval here would hide blocks committed after idle began.
            quiet = (
                segment
                if plan["workload_kind"] == "minting"
                else [
                    r for r in segment if left + gap < r["observed_at_unix_s"] < right
                ]
            )
            field = (
                "chain_height"
                if plan["workload_kind"] == "minting"
                else "confirmed_transfers"
            )
            if len(quiet) >= 2 and quiet[-1][field] != quiet[0][field]:
                incomplete.add(f"{name}: {phase} workload did not become idle")
        if len(segment) < 2:
            continue
        result["stages"][phase] = {
            "samples": len(segment),
            "height_delta": segment[-1]["chain_height"] - segment[0]["chain_height"],
            "confirmed_transfer_delta": segment[-1]["confirmed_transfers"]
            - segment[0]["confirmed_transfers"],
            "cpu_seconds_delta": segment[-1]["cpu_seconds"] - segment[0]["cpu_seconds"],
            "throttled_seconds_delta": segment[-1]["cgroup_cpu_throttled_seconds"]
            - segment[0]["cgroup_cpu_throttled_seconds"],
        }
        if phase == "sustained" or phase.endswith("_active"):
            for offset in range(0, int(right - left), plan["active_window_s"]):
                window_start = left + offset
                load = [
                    r
                    for r in segment
                    if window_start
                    <= r["observed_at_unix_s"]
                    <= window_start + plan["active_window_s"]
                ]
                if len(load) < 2 or any(
                    load[-1][field] - load[0][field] < plan[minimum]
                    for field, minimum in (
                        ("chain_height", "min_blocks_per_active_window"),
                        (
                            "confirmed_transfers",
                            "min_confirmed_transfers_per_active_window",
                        ),
                    )
                ):
                    incomplete.add(
                        f"{name}: insufficient {phase} workload in a fixed active window"
                    )
    measured = [
        r for r in rows if stages[1][1] <= r["observed_at_unix_s"] <= stages[1][2]
    ]
    limits = thresholds(plan)
    slope_limit = limits["slope_limit_mib_per_hour"]
    net_limit = limits["net_growth_limit_mib"]
    measurement_complete = measurement_is_complete(plan, node, observations, measured)
    result["measurement_complete"] = measurement_complete
    if len(measured) >= 2:
        for field, label in (
            ("rss_bytes", "rss_plus_swap"),
            ("anonymous_bytes", "anonymous_plus_swap"),
        ):
            stats = memory_stats(measured, field)
            result[label] = stats
            complete = not any(reason.startswith(name + ":") for reason in incomplete)
            if measurement_complete and (
                stats["slope_mib_per_hour"] > slope_limit + 1e-6
                or stats["net_growth_mib"] > net_limit
            ):
                failures.add(f"{name}: {label} sustained growth exceeded")
            for phase, left, right in stages[2:]:
                segment = [r for r in rows if left <= r["observed_at_unix_s"] <= right]
                if segment:
                    retained = (
                        segment[-1][field] + segment[-1]["swap_bytes"]
                    ) / MIB - stats["baseline_mib"]
                    result["stages"].setdefault(phase, {})[
                        label + "_baseline_growth_mib"
                    ] = retained
                    if complete and retained > net_limit:
                        failures.add(
                            f"{name}: {label} cycle retained growth exceeded fixed baseline"
                        )
    result["baseline_cpu_seconds"] = rows[0]["cpu_seconds"]
    result["cgroup_pressure_last"] = rows[-1].get("cgroup_pressure")
    result["peaks_bytes"] = {key: max(r[key] for r in rows) for key in GAUGES}
    result["cgroup_memory_events_delta"] = {
        k: rows[-1]["cgroup_memory_events"][k] - rows[0]["cgroup_memory_events"][k]
        for k in ("high", "max", "oom", "oom_kill")
    }
    return result, rows


def analyze(plan, observations):
    validate_manifest(plan)
    failures, incomplete = set(), set()
    by_node = {node["node_id"]: [] for node in plan["nodes"]}
    terminals = [row for row in observations if is_terminal(row)]
    observations = [row for row in observations if not is_terminal(row)]
    for row in observations:
        if not isinstance(row, dict) or not isinstance(row.get("node_id"), str):
            incomplete.add("malformed observation node")
        elif row["node_id"] not in by_node:
            failures.add("unknown observation node")
        else:
            by_node[row["node_id"]].append(row)
    results, hashes, compared = {}, {}, set()
    for node in plan["nodes"]:
        result, rows = inspect_node(
            plan, node, by_node[node["node_id"]], failures, incomplete
        )
        results[node["node_id"]] = result
        for row in rows:
            if row.get("chain_id") != node["chain_id"]:
                continue
            key = (node["chain_id"], row["chain_height"])
            if key in hashes and hashes[key][0] != row["chain_hash"]:
                failures.add(f"conflicting observed block hashes at height {key[1]}")
            if key in hashes and hashes[key][1] != node["node_id"]:
                compared.add(key)
            hashes[key] = (row["chain_hash"], node["node_id"])
    # After every evidence check, so a collector gap never masks a failure.
    incomplete |= collector_incomplete_reasons(plan, terminals, by_node)
    return {
        "schema": 1,
        "manifest_sha256": manifest_digest(plan),
        "status": "failed" if failures else "incomplete" if incomplete else "passed",
        "scope": "limited_minting"
        if plan["workload_kind"] == "minting"
        else "sustained_payments",
        "failures": sorted(failures),
        "incomplete_reasons": sorted(incomplete),
        "nodes": results,
        "same_height_comparisons": len(compared),
        "collector_terminal_records": len(terminals),
        "thresholds": {
            "slope_mib_per_hour": thresholds(plan)["slope_limit_mib_per_hour"],
            "net_growth_mib": thresholds(plan)["net_growth_limit_mib"],
        },
        "limitations": [
            "Sampled same-height hashes do not establish absence of forks.",
            "Same-host processes do not establish equivalent dedicated-host performance.",
            "This result does not authorize a campaign or recover historical state.",
        ],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument(
        "samples",
        type=Path,
        nargs="+",
        help="raw JSONL files (e.g. one per host), read in the order given",
    )
    args = parser.parse_args(argv)
    try:
        plan = json.loads(args.manifest.read_text())
        rows, parse_errors, files, file_reasons = [], [], [], []
        for path in args.samples:
            data = path.read_bytes()
            # Each collector writes its own file; never merge or rewrite them.
            files.append(
                {"path": str(path), "sha256": hashlib.sha256(data).hexdigest()}
            )
            prefix = f"{path}: " if len(args.samples) > 1 else ""
            file_rows = []
            for line_number, line in enumerate(data.decode().splitlines(), 1):
                try:
                    file_rows.append(json.loads(line))
                except ValueError:
                    file_rows.append(None)  # keeps "final line" positional
                    parse_errors.append(f"{prefix}invalid JSONL line {line_number}")
            file_reasons.extend(file_incomplete_reasons(path, file_rows))
            rows.extend(row for row in file_rows if row is not None)
        if parse_errors:
            # An undecodable row has no trustworthy timestamp. It may belong
            # to measurement, so invalidate trend eligibility conservatively.
            rows.extend(
                {"node_id": n["node_id"], "errors": parse_errors} for n in plan["nodes"]
            )
        result = analyze(plan, rows)
        if len(files) == 1:
            result["observations_sha256"] = files[0]["sha256"]
        result["observation_files"] = files
        if parse_errors or file_reasons:
            result["incomplete_reasons"].extend(parse_errors + file_reasons)
            if result["status"] != "failed":
                result["status"] = "incomplete"
    except (OSError, ValueError, KeyError, TypeError) as error:
        result = {"status": "incomplete", "incomplete_reasons": [str(error)]}
    print(json.dumps(result, indent=2, allow_nan=False))
    return {"passed": 0, "failed": 1, "incomplete": 2}[result["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
