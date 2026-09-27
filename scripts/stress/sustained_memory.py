#!/usr/bin/env python3
"""Analyze immutable, local sustained-memory observations; never launch a node."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics

MIB = 1024 * 1024
SLOPE_LIMIT = 10.0
NET_LIMIT = 64.0
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
    return plan


def profile_phases(workload):
    phases = [
        {"name": "warmup", "duration_s": 1800},
        {"name": "sustained", "duration_s": 14400},
    ]
    if workload == "minting":
        phases.extend(
            [
                {"name": "settling", "duration_s": 30},
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
            # Explicit settling absorbs the documented ten-second control tick;
            # every subsequent idle observation must actually be idle.
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
                stats["slope_mib_per_hour"] > SLOPE_LIMIT + 1e-6
                or stats["net_growth_mib"] > NET_LIMIT
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
                    if complete and retained > NET_LIMIT:
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
        "thresholds": {"slope_mib_per_hour": SLOPE_LIMIT, "net_growth_mib": NET_LIMIT},
        "limitations": [
            "Sampled same-height hashes do not establish absence of forks.",
            "Same-host processes do not establish equivalent dedicated-host performance.",
            "This result does not authorize a campaign or recover historical state.",
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("samples", type=Path)
    args = parser.parse_args()
    try:
        plan = json.loads(args.manifest.read_text())
        rows, parse_errors = [], []
        raw = args.samples.read_bytes()
        for line_number, line in enumerate(raw.decode().splitlines(), 1):
            try:
                rows.append(json.loads(line))
            except ValueError:
                parse_errors.append(f"invalid JSONL line {line_number}")
        if parse_errors:
            # An undecodable row has no trustworthy timestamp. It may belong
            # to measurement, so invalidate trend eligibility conservatively.
            rows.extend(
                {"node_id": n["node_id"], "errors": parse_errors} for n in plan["nodes"]
            )
        result = analyze(plan, rows)
        result["observations_sha256"] = hashlib.sha256(raw).hexdigest()
        if parse_errors:
            result["incomplete_reasons"].extend(parse_errors)
            if result["status"] != "failed":
                result["status"] = "incomplete"
    except (OSError, ValueError, KeyError, TypeError) as error:
        result = {"status": "incomplete", "incomplete_reasons": [str(error)]}
    print(json.dumps(result, indent=2, allow_nan=False))
    return {"passed": 0, "failed": 1, "incomplete": 2}[result["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
