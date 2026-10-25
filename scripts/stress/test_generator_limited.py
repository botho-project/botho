"""#1613: honour #1576 decision 1 and size x-series submission headroom.

A plan opt-in (`rehearsal.generator_limited_acceptable`, schema v2 only) may
accept a cadence-only `generator_limited` rehearsal whose ledger is clean; it
is journaled and reported as an acceptance, never as a pass. The fake-clock
model below drives the real Journal rate limiter and the real offer
materializer through one ramp-hour peak window of the x-series plan.
"""

import asyncio
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from discovery import ACCEPTED_AS, DiscoveryController
from plan import expand
from runtime import Gate, HOSTS, Journal, Quota

HERE = Path(__file__).parent


def load(name):
    return json.loads((HERE / name).read_text())


class OptInSchemaTests(unittest.TestCase):
    def test_x_series_opts_in_and_validates_with_the_new_limit(self):
        plan = load("x-series-plan.json")
        self.assertIs(plan["rehearsal"]["generator_limited_acceptable"], True)
        self.assertEqual(plan["limits"]["max_normal_submissions_per_minute"], 40)
        summary, events = expand(plan)
        self.assertEqual(len(events), 22480)
        rehearsal = plan["rehearsal"]
        self.assertGreater(
            plan["limits"]["max_normal_submissions_per_minute"],
            60 // rehearsal["interval_seconds"],
        )

    def test_headroom_keeps_max_size_submissions_inside_the_wire_budget(self):
        limits = load("x-series-plan.json")["limits"]
        hex_bytes = 2 * limits["max_signed_transaction_bytes"]
        envelope = len(
            json.dumps(
                {"jsonrpc": "2.0", "id": 1404, "method": "tx_submit",
                 "params": {"tx_hex": "0" * hex_bytes}}
            ).encode()
        )
        self.assertLessEqual(
            limits["max_normal_submissions_per_minute"] * envelope,
            limits["max_wire_submission_bytes_per_minute"],
        )
        self.assertLessEqual(limits["max_inflight"], 32)

    def test_absent_or_false_opt_in_is_accepted_and_non_boolean_rejected(self):
        plan = load("testnet-discovery-v2.json")
        self.assertNotIn("generator_limited_acceptable", plan["rehearsal"])
        expand(plan)
        plan["rehearsal"]["generator_limited_acceptable"] = False
        expand(plan)
        for value in (1, "true", None):
            plan["rehearsal"]["generator_limited_acceptable"] = value
            with self.assertRaises(ValueError):
                expand(copy.deepcopy(plan))

    def test_schema_v1_rejects_the_key(self):
        v1 = load("testnet-72h-plan.json")
        expand(copy.deepcopy(v1))
        for value in (True, False):
            plan = copy.deepcopy(v1)
            plan["rehearsal"] = {"generator_limited_acceptable": value}
            with self.assertRaises(ValueError):
                expand(plan)


class AcceptanceControllerTests(unittest.TestCase):
    """setup_tick at the rehearsal boundary with a slow-but-complete ledger."""

    def setUp(self):
        from test_discovery import DiscoveryExecutorTests

        fixture = DiscoveryExecutorTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        c = self.c = fixture.c
        c.plan["execution_mode"] = "rehearsal_only"
        c.plan["rehearsal"]["generator_limited_acceptable"] = True
        c.j.set("start", None)
        c.j.set("status", "setup")
        c.j.set("opening_verified", True)
        c.j.set("rehearsal_start", 1000)
        c.j.set("setup_start", 1000)
        c.j.set("fee_latest", {host: {"at": 2500} for host in HOSTS})
        c.inventory_ready = Mock(return_value=True)
        c.report = Mock()
        (c.state / "artifacts").mkdir(exist_ok=True)
        # The x1 shape: every offer reconciled, submissions lagging the clock.
        for i in range(540):
            identifier = f"rehearsal-{i:05d}"
            offered = 1000 + 2 * i
            submitted = 1001 + 2.2 * i
            c.j.offer(identifier, "rehearsal", offered, i % 8, (i + 1) % 8, 1)
            c.j.transition(
                identifier, "reconciled", prepared=submitted - 0.3,
                submitted=submitted, finished=submitted + 30, fee=1,
                hash=format(i, "064x"),
            )
            (c.state / "artifacts" / (identifier + ".bin")).write_bytes(b"signed")

    def tick(self):
        with patch("discovery.time.time", return_value=2500):
            asyncio.run(self.c.setup_tick())

    def accepted_events(self):
        return self.c.j.db.execute(
            "SELECT detail FROM events WHERE state='rehearsal_generator_limited_accepted'"
        ).fetchall()

    def assert_gates(self):
        with self.assertRaisesRegex(Gate, "rehearsal incomplete"):
            self.tick()
        self.assertIsNone(self.c.j.get("rehearsal_acceptance"))
        self.assertEqual(self.accepted_events(), [])
        self.c.inventory_ready.assert_not_called()
        self.assertEqual(self.c.j.get("status"), "setup")

    def test_clean_ledger_proceeds_past_the_gate_and_is_journaled(self):
        self.tick()
        result = self.c.j.get("rehearsal_result")
        self.assertEqual(result["status"], "generator_limited")
        self.assertIn("submission_offer_lag", result["failures"])
        [(detail,)] = self.accepted_events()
        acceptance = json.loads(detail)
        self.assertEqual(acceptance["accepted_as"], ACCEPTED_AS)
        self.assertEqual(acceptance["result"], result)
        self.assertEqual(acceptance["ledger"]["reconciled"], 540)
        self.assertEqual(self.c.j.get("rehearsal_acceptance"), acceptance)
        # The existing post-rehearsal gates still ran.
        self.c.sync.assert_awaited()
        self.c.accounting.assert_called()
        self.c.inventory_ready.assert_called_once()
        self.assertEqual(self.c.j.get("status"), "rehearsal_complete")
        self.assertIn(ACCEPTED_AS, self.c.j.get("reason"))

    def test_post_rehearsal_gates_still_hold_after_acceptance(self):
        self.c.inventory_ready.return_value = False
        with self.assertRaisesRegex(Gate, "inventory depleted"):
            self.tick()
        self.assertEqual(len(self.accepted_events()), 1)
        self.assertEqual(self.c.j.get("status"), "setup")

    def test_opt_in_absent_or_false_still_gates(self):
        for value in (None, False):
            if value is None:
                del self.c.plan["rehearsal"]["generator_limited_acceptable"]
            else:
                self.c.plan["rehearsal"]["generator_limited_acceptable"] = value
            self.assert_gates()

    def test_unreconciled_offer_gates(self):
        self.c.j.transition("rehearsal-00100", "unknown")
        self.assert_gates()

    def test_signed_unsubmitted_gates(self):
        self.c.j.db.execute(
            "UPDATE intents SET submitted=NULL, state='prepared' WHERE id='rehearsal-00200'"
        )
        self.assert_gates()

    def test_unsigned_skip_gates(self):
        self.c.j.db.execute(
            "UPDATE intents SET prepared=NULL, submitted=NULL, finished=1300, "
            "state='skipped' WHERE id='rehearsal-00150'"
        )
        (self.c.state / "artifacts" / "rehearsal-00150.bin").unlink()
        self.assert_gates()

    def test_orphaned_signed_artifact_gates(self):
        (self.c.state / "artifacts" / "rehearsal-99999.bin").write_bytes(b"x")
        self.assert_gates()

    def test_ambiguous_signed_artifact_gates(self):
        (self.c.state / "artifacts" / "rehearsal-00010.partial").write_bytes(b"x")
        self.assert_gates()

    def test_unavailable_artifact_evidence_gates(self):
        import shutil

        shutil.rmtree(self.c.state / "artifacts")
        self.assert_gates()

    def test_non_cadence_failure_gates_even_when_opted_in(self):
        self.c.j.db.execute(
            "UPDATE intents SET finished=9999 WHERE id='rehearsal-00300'"
        )
        self.assert_gates()
        self.assertIn("receipt_timestamps", self.c.j.get("rehearsal_result")["failures"])

    def test_failed_status_is_never_accepted(self):
        self.assertFalse(
            self.c.accept_generator_limited(
                {"status": "failed", "failures": [], "expected": 540,
                 "offered": 540, "reconciled": 540}
            )
        )

    def test_report_marks_the_acceptance_and_never_a_pass(self):
        del self.c.report
        self.tick()
        with patch("discovery.time.time", return_value=2600):
            report = self.c.report()
        self.assertEqual(report["rehearsal"]["status"], "generator_limited")
        self.assertEqual(report["rehearsal"]["accepted_as"], ACCEPTED_AS)
        self.assertEqual(report["qualification_status"], "accepted_generator_limited")


class PeakHeadroomFakeClockTests(unittest.TestCase):
    """Hours 30-42 of x-series: 55 min ramp at 12/min, then 5 min at 30/min.

    The model drives the real Journal (sliding-window submission cap) and the
    real DiscoveryController.materialize (120 s start-slot grace) on a fake
    clock, one admission per second as the controller's loop does. Senders
    stay pending for `latency` seconds after submission (max_inflight_per_wallet
    is 1). Injected stalls stand in for admission dips (sync, signer, RPC).

    Outcomes that would stop the run: a `skipped` offer gates the hour-42
    phase boundary ("previous phase delivery incomplete"; see
    test_discovery.test_unresolved_or_skipped_previous_phase_cannot_pass), and
    a signed row the cap defers past its slot is `expired`, which
    submit_prepared turns into an immediate hold.
    """

    PLAN = load("x-series-plan.json")

    def simulate(self, cap, stalls=(), latency=20, hour=30):
        plan = copy.deepcopy(self.PLAN)
        plan["limits"]["max_normal_submissions_per_minute"] = cap
        _, events = expand(plan)
        low = hour * 3600 + 3000  # five ramp minutes before the 30/min peak
        high = (hour + 1) * 3600 + 600
        window = [
            dict(e, offset_seconds=e["offset_seconds"] - low)
            for e in events
            if low <= e["offset_seconds"] < high
        ]
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        clock = [0.0]
        c = DiscoveryController.__new__(DiscoveryController)
        c.j = Journal(Path(tmp.name) / "j.sqlite", lambda: clock[0], limits=plan["limits"])
        self.addCleanup(c.j.db.close)
        grace = plan["limits"]["start_slot_grace_seconds"]
        finishing, quota, expired = {}, 0, 0
        for t in range(high - low + 300):
            clock[0] = float(t)
            for identifier, at in list(finishing.items()):
                if at <= t:
                    c.j.finish(identifier, {})
                    del finishing[identifier]
            c.materialize(window, 0, "campaign", t)
            if any(a <= t < b for a, b in stalls):
                continue
            target = next(iter(c.j.rows("state='prepared' ORDER BY prepared,offered,id")), None)
            if target is None:
                pending = {p["sender"] for p in c.j.pending()}
                for row in c.j.rows("state IN ('planned','eligible') ORDER BY offered,id"):
                    if row["sender"] in pending or t >= row["offered"] + grace:
                        continue
                    c.j.transition(row["id"], "eligible")
                    c.j.prepare(row["id"], {"hash": row["id"], "fee": 1000, "bytes": 5000,
                                            "selected": [{"id": "in-" + row["id"]}]},
                                32, row["offered"] + grace)
                    target = c.j.intent(row["id"])
                    break
            if target is None:
                continue
            if t >= target["offered"] + grace:
                expired += 1
                c.j.transition(target["id"], "expired")
                continue
            try:
                c.j.begin_submit(target["id"], "h", 6000, False, target["offered"] + grace)
                finishing[target["id"]] = t + latency
            except Quota:
                quota += 1
        rows = c.j.rows("kind='campaign'")
        self.assertEqual(len(rows), len(window))
        lags = [r["submitted"] - r["offered"] for r in rows if r["submitted"] is not None]
        return {
            "skipped": sum(r["state"] == "skipped" for r in rows),
            "expired": expired,
            "quota": quota,
            "max_lag": max(lags),
            "final_lag": lags[-1],
        }

    def test_undisturbed_peak_holds_with_zero_headroom(self):
        for cap in (30, 40):
            outcome = self.simulate(cap)
            self.assertEqual(
                (outcome["skipped"], outcome["expired"], outcome["quota"], outcome["max_lag"]),
                (0, 0, 0, 0), cap,
            )

    def test_single_admission_stall_recovers_at_either_cap(self):
        # A 100 s stall a minute into the peak: the backlog persists through
        # the rest of the peak at 30/min but drains once the ramp resumes.
        for cap in (30, 40):
            outcome = self.simulate(cap, stalls=((360, 460),))
            self.assertEqual((outcome["skipped"], outcome["expired"]), (0, 0), cap)
            self.assertEqual(outcome["final_lag"], 0)

    def test_two_stalls_in_one_peak_hold_only_with_zero_headroom(self):
        # 70 s at peak start, then 60 s two minutes later. At 30/min the first
        # backlog never drains, so the second pushes offers past their slot.
        stalls = ((300, 370), (430, 490))
        zero = self.simulate(30, stalls=stalls)
        self.assertGreater(zero["skipped"] + zero["expired"], 0)
        headroom = self.simulate(40, stalls=stalls)
        self.assertEqual((headroom["skipped"], headroom["expired"]), (0, 0))
        self.assertLess(headroom["quota"], zero["quota"])


if __name__ == "__main__":
    unittest.main()
