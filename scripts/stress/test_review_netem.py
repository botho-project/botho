"""Read the evidence produced by an offline runner invocation."""

import copy
import io
import json
import tempfile
import unittest
from pathlib import Path

import faults
from review_netem import review
from test_faults import FakeHost, INVENTORY, PLAN, START, SUMMARY, UNIT, step


class ReviewTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.plan = PLAN
        d4 = step("D4")
        host = FakeHost(START)

        def counters(alias, argv):
            if argv[:4] == ["tc", "-j", "-s", "qdisc"]:
                return (
                    0,
                    json.dumps(
                        [
                            {
                                "kind": "netem",
                                "handle": "40:",
                                "parent": "1:4",
                                "packets": 80,
                                "drops": 20,
                            }
                        ]
                    ),
                    "",
                )
            return None

        host.override = counters
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "faults.jsonl"
            runner = faults.Runner(
                [d4],
                INVENTORY,
                UNIT,
                START,
                host,
                path,
                clock=host.clock,
                sleep=host.sleep,
                out=io.StringIO(),
            )
            assert runner.run() == 0
            cls.rows = [json.loads(line) for line in path.read_text().splitlines()]
        cls.activation = {
            "start": START + 0.25,
            "run_id": "q-attempt-a",
            "plan_sha256": SUMMARY["plan_sha256"],
        }
        cls.report = {
            **cls.activation,
            "status": "incomplete",
            "at": START + d4["window_end_offset"] + 1,
            "degrade": [
                {
                    "step": "D4",
                    "targets": d4["target_hosts"],
                    "action": "netem",
                    "expect": "progress",
                    "verdict": "pass",
                    "failures": [],
                    "awaiting": [],
                    "fault_observed": "unknown",
                }
            ],
        }

    def assess(self, rows=None, report=None, **kwargs):
        return review(
            self.plan,
            self.activation,
            report or self.report,
            self.rows if rows is None else rows,
            **kwargs,
        )

    def test_runner_populates_distinct_argument_hashes_in_execution_order(self):
        setup = [r["command_sha256"] for r in self.rows if r["action"] == "netem"]
        self.assertEqual(len(set(setup)), 4)
        result = self.assess()
        self.assertTrue(result["corroborated"])
        self.assertEqual(
            result["runner"]["command_evidence"], "matched_argument_hashes"
        )
        self.assertEqual(result["campaign_status"], "incomplete")
        self.assertEqual(result["runner"]["queue_counters"]["drops"], 20)
        self.assertEqual(result["runner"]["queue_counters"]["packets"], 80)

    def test_missing_all_hashes_is_explicitly_legacy(self):
        legacy = [
            {k: v for k, v in row.items() if k != "command_sha256"} for row in self.rows
        ]
        self.assertEqual(
            self.assess(legacy)["runner"]["command_evidence"],
            "legacy_action_names_only",
        )

    def test_unavailable_or_unrelated_telemetry_cannot_prove_packet_drops(self):
        for mode in ("unavailable", "outside", "other_host", "malformed", "absent"):
            with self.subTest(mode=mode):
                rows = copy.deepcopy(self.rows)
                sample = next(r for r in rows if r["phase"] == "observe")
                if mode == "absent":
                    rows.remove(sample)
                elif mode == "outside":
                    sample["at"] += 3600
                elif mode == "other_host":
                    sample["role"] = "n1"
                elif mode == "malformed":
                    sample["counters"]["drops"] = True
                else:
                    sample["ok"] = False
                result = self.assess(rows)
                self.assertTrue(result["corroborated"])
                self.assertIsNone(result["runner"]["queue_counters"])

    def test_missing_mixed_substituted_or_reordered_command_records(self):
        idx = [i for i, r in enumerate(self.rows) if r["action"] == "netem"]
        for mode in (
            "missing",
            "partial",
            "replaced",
            "reordered",
            "failed",
            "wrong_target",
        ):
            with self.subTest(mode=mode):
                rows = copy.deepcopy(self.rows)
                if mode == "missing":
                    del rows[idx[1]]
                elif mode == "partial":
                    del rows[idx[0]]["command_sha256"]
                elif mode == "replaced":
                    rows[idx[2]]["command_sha256"] = rows[idx[1]]["command_sha256"]
                elif mode == "reordered":
                    rows[idx[1]], rows[idx[2]] = rows[idx[2]], rows[idx[1]]
                elif mode == "failed":
                    rows[idx[0]]["ok"] = False
                else:
                    rows[idx[0]]["role"] = "n1"
                self.assertFalse(self.assess(rows)["corroborated"])
        self.assertFalse(self.assess(dev="eth0")["corroborated"])

    def test_wrong_attempt_missing_restore_or_late_restore(self):
        with self.assertRaisesRegex(ValueError, "different attempts"):
            self.assess(report={**self.report, "run_id": "q-attempt-b"})
        self.assertFalse(
            self.assess([r for r in self.rows if r["phase"] != "verify"])[
                "corroborated"
            ]
        )
        rows = copy.deepcopy(self.rows)
        for row in rows:
            if row["phase"] in ("restore", "verify", "disarm"):
                row["at"] += 3600
        self.assertFalse(self.assess(rows)["corroborated"])

    def test_timed_out_probe_and_short_step_restore_at_original_deadline(self):
        for short in (False, True):
            with self.subTest(short=short), tempfile.TemporaryDirectory() as tmp:
                host = FakeHost(START)
                d4 = step("D4")
                if short:
                    d4["restore_offset"] = d4["fault_start_offset"] + 120

                def missing(alias, argv):
                    if argv[:4] == ["tc", "-j", "-s", "qdisc"]:
                        host.now += faults.SSH_TIMEOUT
                        return 255, "", "timeout"
                    return None

                host.override = missing
                path = Path(tmp) / "faults.jsonl"
                runner = faults.Runner(
                    [d4],
                    INVENTORY,
                    UNIT,
                    START,
                    host,
                    path,
                    clock=host.clock,
                    sleep=host.sleep,
                    out=io.StringIO(),
                )
                self.assertEqual(runner.run(), 0)
                rows = [json.loads(line) for line in path.read_text().splitlines()]
                probes = [r for r in rows if r["action"] == "qdisc_counters"]
                self.assertEqual(len(probes), 0 if short else 1)
                if probes:
                    self.assertFalse(probes[0]["ok"])
                    self.assertEqual(probes[0]["phase"], "warn")
                    self.assertIsNone(probes[0]["counters"])
                (restore,) = (r for r in rows if r["phase"] == "restore")
                self.assertEqual(restore["at"], START + d4["restore_offset"])

    def test_runner_cannot_supply_a_missing_controller_verdict(self):
        report = copy.deepcopy(self.report)
        report["degrade"][0]["verdict"] = "in_progress"
        result = self.assess(report=report)
        self.assertIn("runner", result)
        self.assertFalse(result["corroborated"])


if __name__ == "__main__":
    unittest.main()
