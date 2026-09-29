"""Capacity calibration must measure generation, not divide by the offer schedule."""

import math
import unittest
from discovery import rehearsal_result


class RehearsalCadenceTests(unittest.TestCase):
    def rows(self, cadence=2, jitter=False):
        return [
            dict(
                state="reconciled",
                offered=1000 + 2 * i,
                submitted=1001 + cadence * i + (0.15 * math.sin(i) if jitter else 0),
                finished=1031 + cadence * i + (0.15 * math.sin(i) if jitter else 0),
            )
            for i in range(540)
        ]

    def result(self, rows):
        return rehearsal_result(
            rows, 540, 1080, start=1000, cutoff=2200, receipt_deadline=2500, now=2500
        )

    def test_ten_percent_slower_generator_cannot_pass_using_receipt_drain(self):
        result = self.result(self.rows(2.2))
        self.assertEqual(result["status"], "generator_limited")
        self.assertAlmostEqual(result["submitted_per_minute"], 60 / 2.2)

    def test_nominal_cadence_with_realistic_rpc_jitter_passes(self):
        result = self.result(self.rows(jitter=True))
        self.assertEqual(result["status"], "passed")
        self.assertGreater(result["rolling_windows"]["count"], 400)
        self.assertAlmostEqual(result["submitted_per_minute"], 30, delta=0.02)

    def test_end_catchup_and_midrun_pause_cannot_average_to_a_pass(self):
        rows = self.rows()
        for i in range(300, 330):
            rows[i]["submitted"] = rows[329]["offered"] + 1 + (i - 300) * 0.001
            rows[i]["finished"] = rows[i]["submitted"] + 30
        self.assertEqual(self.result(rows)["status"], "generator_limited")

    def test_missing_invalid_or_future_timestamps_fail(self):
        for field in ("offered", "submitted", "finished"):
            for value in (None, float("nan"), float("inf"), True, "1000", 3000):
                with self.subTest(field=field, value=value):
                    rows = self.rows()
                    rows[1][field] = value
                    self.assertEqual(self.result(rows)["status"], "generator_limited")

    def test_offer_clock_cannot_shift_or_duplicate_and_submission_cannot_precede_offer(
        self,
    ):
        rows = self.rows()
        rows[0]["submitted"] = 999
        self.assertEqual(self.result(rows)["status"], "generator_limited")
        rows = self.rows()
        rows[1]["offered"] = rows[0]["offered"]
        self.assertEqual(self.result(rows)["status"], "generator_limited")
        rows = self.rows()
        for row in rows:
            for key in ("offered", "submitted", "finished"):
                row[key] += 10
        self.assertEqual(self.result(rows)["status"], "generator_limited")

    def test_actual_submission_and_receipt_cutoffs_are_enforced(self):
        rows = self.rows()
        rows[-1]["submitted"] = 2200
        rows[-1]["finished"] = 2230
        self.assertEqual(self.result(rows)["status"], "generator_limited")
        rows = self.rows()
        rows[-1]["finished"] = 2501
        self.assertEqual(self.result(rows)["status"], "generator_limited")

    def test_receipt_drain_does_not_change_achieved_generation_rate(self):
        rows = self.rows()
        rows[-1]["finished"] = 2499
        result = self.result(rows)
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["submitted_per_minute"], 30)

    def test_duplicate_submissions_and_steady_small_slowdown_fail(self):
        rows = self.rows()
        rows[200]["submitted"] = rows[199]["submitted"]
        self.assertEqual(self.result(rows)["status"], "generator_limited")
        self.assertEqual(self.result(self.rows(2.01))["status"], "generator_limited")

    def test_slow_but_fully_reconciled_rehearsal_cannot_activate_t0(self):
        import asyncio
        from unittest.mock import Mock, patch
        from runtime import Gate, HOSTS
        from test_discovery import DiscoveryExecutorTests

        fixture = DiscoveryExecutorTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        controller = fixture.c
        controller.j.set("start", None)
        controller.j.set("status", "setup")
        controller.j.set("opening_verified", True)
        controller.j.set("rehearsal_start", 1000)
        controller.j.set("setup_start", 1000)
        controller.j.set("fee_latest", {host: {"at": 2500} for host in HOSTS})
        controller.inventory_ready = Mock()
        for i, row in enumerate(self.rows(2.2)):
            identifier = f"rehearsal-{i:05d}"
            controller.j.offer(identifier, "rehearsal", row["offered"], 0, 1, 1)
            controller.j.transition(
                identifier,
                row["state"],
                submitted=row["submitted"],
                finished=row["finished"],
            )
        with patch("discovery.time.time", return_value=2500):
            with self.assertRaisesRegex(Gate, "rehearsal incomplete"):
                asyncio.run(controller.setup_tick())
        self.assertIsNone(controller.j.get("start"))
        self.assertEqual(
            controller.j.get("rehearsal_result")["status"], "generator_limited"
        )
        controller.inventory_ready.assert_not_called()
