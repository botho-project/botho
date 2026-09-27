"""Synthetic controls for the isolated sustained-memory qualification gate."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from sustained_memory import (
    analyze,
    manifest_digest,
    profile_phases,
    validate_manifest,
    windows,
)

MIB = 1024 * 1024


def manifest(workload="payments"):
    return {
        "schema": 1,
        "campaign_id": "synthetic-control",
        "source_revision": "a" * 40,
        "artifact_sha256": "b" * 64,
        "started_at_unix_s": 1_000_000,
        "phases": profile_phases(workload),
        "sample_interval_s": 5,
        "max_gap_s": 15,
        "active_window_s": 300,
        "min_blocks_per_active_window": 10,
        "min_confirmed_transfers_per_active_window": 1 if workload == "payments" else 0,
        "workload_kind": workload,
        "nodes": [
            {
                "node_id": "n1",
                "host_id": "h1",
                "pid": 42,
                "process_start_id": "boot:123",
                "chain_id": "isolated-control",
                "producer": True,
                "cgroup_path": "/sys/fs/cgroup/lab-n1",
            }
        ],
    }


def samples(plan, slope=0):
    rows = []
    for elapsed in range(
        0, int(windows(plan)[-1][2] - plan["started_at_unix_s"]) + 1, 5
    ):
        growth = slope * MIB * max(0, elapsed - 1800) / 3600
        activity = min(elapsed, 16200)
        if plan["workload_kind"] == "payments":
            activity += sum(
                max(0, min(600, elapsed - (16200 + cycle * 1200))) for cycle in range(3)
            )
        height_time = (
            elapsed if plan["workload_kind"] == "payments" else min(elapsed, 16200)
        )
        rows.append(
            {
                **plan["nodes"][0],
                "schema": 1,
                "campaign_id": plan["campaign_id"],
                "manifest_sha256": manifest_digest(plan),
                "source_revision": plan["source_revision"],
                "artifact_sha256": plan["artifact_sha256"],
                "observed_at_unix_s": plan["started_at_unix_s"] + elapsed,
                "rss_bytes": int(200 * MIB + growth),
                "anonymous_bytes": int(150 * MIB + growth),
                "swap_bytes": 0,
                "cgroup_memory_bytes": int(220 * MIB + growth),
                "cgroup_swap_bytes": 0,
                "cpu_seconds": elapsed * 0.5,
                "cgroup_cpu_throttled_seconds": elapsed * 0.01,
                "cgroup_memory_events": {"high": 0, "max": 0, "oom": 0, "oom_kill": 0},
                "chain_height": height_time // 10,
                "chain_hash": f"{height_time // 10:064x}",
                "confirmed_transfers": activity // 60,
                "errors": [],
                "mining": not (plan["workload_kind"] == "minting" and elapsed >= 16200),
            }
        )
    return rows


class SustainedMemoryTests(unittest.TestCase):
    def setUp(self):
        self.plan = manifest()
        self.rows = samples(self.plan)

    def test_constant_and_plateau_pass(self):
        self.assertEqual(analyze(self.plan, self.rows)["status"], "passed")
        for row in self.rows:
            elapsed = row["observed_at_unix_s"] - self.plan["started_at_unix_s"]
            row["rss_bytes"] += int(min(elapsed / 1800, 1) * 30 * MIB)
        self.assertEqual(analyze(self.plan, self.rows)["status"], "passed")

    def test_prior_49_mib_hour_leak_fails(self):
        result = analyze(self.plan, samples(self.plan, 49))
        self.assertEqual(result["status"], "failed")
        self.assertAlmostEqual(
            result["nodes"]["n1"]["rss_plus_swap"]["slope_mib_per_hour"], 49
        )

    def test_complete_bad_measurement_still_fails_without_later_idle(self):
        rows = [
            r
            for r in samples(self.plan, 49)
            if r["observed_at_unix_s"] <= self.plan["started_at_unix_s"] + 16200
        ]
        result = analyze(self.plan, rows)
        self.assertEqual(result["status"], "failed")
        self.assertTrue(result["incomplete_reasons"])
        self.assertTrue(result["nodes"]["n1"]["measurement_complete"])
        incomplete_measurement = analyze(self.plan, rows[:-100])
        self.assertEqual(incomplete_measurement["status"], "incomplete")
        self.assertFalse(incomplete_measurement["nodes"]["n1"]["measurement_complete"])

    def test_measurement_restart_cannot_establish_slope_failure(self):
        rows = samples(self.plan, 49)
        rows[600]["process_start_id"] = "boot:999"
        self.assertEqual(analyze(self.plan, rows)["status"], "incomplete")

    def test_swap_cannot_hide_retained_growth(self):
        for row in self.rows:
            elapsed = row["observed_at_unix_s"] - self.plan["started_at_unix_s"]
            row["swap_bytes"] = int(49 * MIB * max(0, elapsed - 1800) / 3600)
        self.assertEqual(analyze(self.plan, self.rows)["status"], "failed")

    def test_gap_restart_and_truncation_are_incomplete(self):
        self.assertEqual(
            analyze(self.plan, self.rows[:500] + self.rows[510:])["status"],
            "incomplete",
        )
        self.assertEqual(analyze(self.plan, self.rows[:-500])["status"], "incomplete")
        self.rows[600]["process_start_id"] = "boot:999"
        self.assertEqual(analyze(self.plan, self.rows)["status"], "incomplete")

    def test_identity_change_fails(self):
        for field in (
            "source_revision",
            "artifact_sha256",
            "chain_id",
            "host_id",
            "manifest_sha256",
        ):
            with self.subTest(field=field):
                rows = copy.deepcopy(self.rows)
                rows[600][field] = "wrong"
                self.assertEqual(analyze(self.plan, rows)["status"], "failed")

    def test_measurement_is_never_rebased(self):
        self.assertEqual(analyze(self.plan, self.rows[500:])["status"], "incomplete")

    def test_inactive_or_missing_workload_is_incomplete(self):
        for row in self.rows:
            row["confirmed_transfers"] = 0
        self.assertEqual(analyze(self.plan, self.rows)["status"], "incomplete")

    def test_minting_pass_is_explicitly_limited(self):
        plan = manifest("minting")
        rows = samples(plan)
        for row in rows:
            row["confirmed_transfers"] = 0
        result = analyze(plan, rows)
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["scope"], "limited_minting")

    def test_resource_violation_is_failed_even_with_missing_samples(self):
        self.rows[500]["cgroup_memory_events"]["oom_kill"] = 1
        del self.rows[500]["rss_bytes"]
        self.assertEqual(analyze(self.plan, self.rows[:600])["status"], "failed")

    def test_missing_node_and_same_height_conflict(self):
        self.plan["nodes"].append(
            {
                **self.plan["nodes"][0],
                "node_id": "n2",
                "pid": 43,
                "cgroup_path": "/sys/fs/cgroup/lab-n2",
            }
        )
        for row in self.rows:
            row["manifest_sha256"] = manifest_digest(self.plan)
        self.assertEqual(analyze(self.plan, self.rows)["status"], "incomplete")
        second = [
            {**r, "node_id": "n2", "pid": 43, "cgroup_path": "/sys/fs/cgroup/lab-n2"}
            for r in self.rows
        ]
        second[100]["chain_hash"] = "f" * 64
        self.assertEqual(analyze(self.plan, self.rows + second)["status"], "failed")

    def test_counters_cannot_reset_and_nonfinite_values_are_incomplete(self):
        self.rows[700]["cpu_seconds"] = 0
        self.assertEqual(analyze(self.plan, self.rows)["status"], "incomplete")
        self.rows[700]["rss_bytes"] = float("nan")
        self.assertEqual(analyze(self.plan, self.rows)["status"], "incomplete")

    def test_failed_minting_pause_is_incomplete(self):
        plan = manifest("minting")
        rows = samples(plan)
        for row in rows:
            row["mining"] = True
        self.assertEqual(analyze(plan, rows)["status"], "incomplete")

    def test_explicit_settling_allows_control_tick_but_not_late_idle(self):
        plan = manifest("minting")
        rows = samples(plan)
        for row in rows:
            elapsed = row["observed_at_unix_s"] - plan["started_at_unix_s"]
            row["mining"] = elapsed < 16215
        self.assertEqual(analyze(plan, rows)["status"], "passed")
        next(
            r
            for r in rows
            if r["observed_at_unix_s"] == plan["started_at_unix_s"] + 16230
        )["mining"] = True
        self.assertEqual(analyze(plan, rows)["status"], "incomplete")

    def test_cgroup_move_fails_and_shared_group_manifest_rejected(self):
        self.rows[600]["cgroup_path"] = "/different"
        self.assertEqual(analyze(self.plan, self.rows)["status"], "failed")
        self.plan["nodes"].append({**self.plan["nodes"][0], "node_id": "n2", "pid": 43})
        with self.assertRaisesRegex(ValueError, "distinct"):
            validate_manifest(self.plan)

    def test_analyzer_cli_keeps_resource_failure_with_truncated_json(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            self.plan["rss_plus_swap_limit_bytes"] = MIB
            raw = json.dumps(samples(self.plan)[0]) + '\n{"partial":'
            (path / "manifest.json").write_text(json.dumps(self.plan))
            (path / "samples.jsonl").write_text(raw)
            completed = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("sustained_memory.py")),
                    str(path / "manifest.json"),
                    str(path / "samples.jsonl"),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 1, completed.stderr)
            report = json.loads(completed.stdout)
            self.assertEqual(report["status"], "failed")
            self.assertIn("invalid JSONL line 2", report["incomplete_reasons"])
            self.assertEqual((path / "samples.jsonl").read_text(), raw)

    def test_decimal_start_preserves_exact_windows(self):
        self.plan["started_at_unix_s"] += 0.5
        self.assertEqual(analyze(self.plan, samples(self.plan))["status"], "passed")

    def test_different_heights_do_not_imply_hash_conflict(self):
        self.plan["nodes"].append(
            {
                **self.plan["nodes"][0],
                "node_id": "n2",
                "pid": 43,
                "cgroup_path": "/sys/fs/cgroup/lab-n2",
            }
        )
        rows = samples(self.plan)
        second = [
            {
                **r,
                "node_id": "n2",
                "pid": 43,
                "cgroup_path": "/sys/fs/cgroup/lab-n2",
                "chain_height": r["chain_height"] + 10000,
                "chain_hash": f"{r['chain_height'] + 10000:064x}",
            }
            for r in rows
        ]
        self.assertEqual(analyze(self.plan, rows + second)["status"], "passed")

    def test_missing_identity_is_incomplete_not_mismatch(self):
        del self.rows[100]["artifact_sha256"]
        self.assertEqual(analyze(self.plan, self.rows)["status"], "incomplete")

    def test_explicit_absolute_limit_is_failed(self):
        self.plan["rss_plus_swap_limit_bytes"] = 100 * MIB
        self.assertEqual(
            analyze(self.plan, samples(self.plan)[:20])["status"], "failed"
        )

    def test_windows_cannot_be_shortened(self):
        self.plan["phases"][1]["duration_s"] = 300
        with self.assertRaises(ValueError):
            validate_manifest(self.plan)


if __name__ == "__main__":
    unittest.main()
