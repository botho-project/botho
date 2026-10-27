"""#1627: a phase-boundary gate's own wait must not expire the next phase's offers.

x1 (q20261027-x1-endurance2) released the correctness -> sustained gate about
197 s after T0+1h: the previous phase drained, then a full sync and accounting
ran. materialize() kept offering sustained events on time and skipped the
first six (offered 09:33:21-09:34:36Z) as ``generator_slot_expired`` once
they were 120 s old. Those skips made the run unable to complete and would
have held it at the T0+24h boundary ("previous phase delivery incomplete").

These tests drive the real admission_tick -> campaign_tick -> materialize ->
deliver -> prepare -> submit_prepared path and the real Journal on a virtual
clock, as test_admission_throughput does. Only the network, signer, coin
selection, full sync, accounting and wallet restore are faked; the gate wait
is the cost of those fakes plus the real drain of the previous phase.
``LegacyAnchor`` restores the pre-#1627 rule (a slot always starts at
``offered``) to show the failure the fix removes.
"""

import asyncio
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import discovery
from discovery import DiscoveryController
from plan import expand
from runtime import Gate, Journal, Quota
from test_admission_throughput import Fleet, x_series

HOUR = 3600
SUMMARY, EVENTS = expand(x_series())
PHASES = [p["id"] for p in x_series()["phases"]]
D1 = next(s for s in SUMMARY["degrade_steps"] if s["id"] == "D1")
MISSED = ("skipped", "unresolved", "expired", "rejected", "aborted")


class LegacyAnchor(DiscoveryController):
    """The pre-#1627 slot rule: every start slot is measured from ``offered``."""

    def slot_anchor(self, row, releases=None):
        return row["offered"]


class Crash(BaseException):
    """A controller process dying mid-tick (escapes Controller.halt)."""


class CampaignFleet(Fleet):
    """A DiscoveryController running the x-series campaign near one boundary.

    Every event offered before ``begin`` is pre-journaled as reconciled; from
    ``begin`` the real controller offers, admits and drains. ``full_sync``,
    ``accounting`` and ``restore`` are the boundary's costs in seconds.
    """

    def __init__(
        self, tmp, boundary_hour, *, begin=600, full_sync=140, accounting=15,
        restore=10, legacy=False,
    ):
        super().__init__(tmp, interval=None)
        self.full_sync_cost = full_sync
        self.accounting_cost = accounting
        self.restore_cost = restore
        self.full_syncs = 0
        self.restores = []
        self.crash_on_full_sync = False
        self.boundary = boundary_hour * HOUR
        self.clock.now = self.boundary - begin
        c = self.c
        if legacy:
            c.__class__ = LegacyAnchor
        c.j.set("status", "running")
        c.j.set("start", 0.0)
        c.j.set("end", 72.0 * HOUR)
        c.config = {"infrastructure_end": 73.0 * HOUR}
        previous = PHASES[PHASES.index(self.phase_at(self.boundary)) - 1]
        reached = PHASES[: PHASES.index(previous) + 1]
        c.j.set("active_phase", previous)
        c.j.set("phase_gates", {p: True for p in reached})
        c.j.set(
            "phase_release",
            {p: self.phase(p)["start_hour"] * HOUR for p in reached},
        )
        # Only the last hour is journaled (the gate checks journaled rows); the
        # cursor still skips every earlier event, keeping the table small.
        cursor = 0
        with c.j.transaction():
            for event in EVENTS:
                if event["offset_seconds"] >= self.clock.now:
                    break
                cursor += 1
                if event["offset_seconds"] < self.clock.now - HOUR:
                    continue
                c.j.offer(event["intent"], "campaign", event["offset_seconds"], 0, 1, 1)
                c.j.transition(
                    event["intent"], "reconciled",
                    info={"input_count": event["input_count"]},
                )
            c.j.set("offer_cursor:campaign", cursor)
        self.wire(c)

    def wire(self, c):
        c.events = EVENTS
        c.degrade_steps = SUMMARY["degrade_steps"]
        c.sync = self.sync
        c.accounting = self.accounting
        c.restore_wallet = self.restore_wallet

    @staticmethod
    def phase(pid):
        return next(p for p in x_series()["phases"] if p["id"] == pid)

    @staticmethod
    def phase_at(at):
        return next(
            p["id"] for p in x_series()["phases"]
            if p["start_hour"] * HOUR <= at < p["end_hour"] * HOUR
        )

    async def sync(self, full=False, **_):
        if not full:
            return await super().sync()
        self.full_syncs += 1
        if self.crash_on_full_sync:
            self.crash_on_full_sync = False
            self.clock.now += self.full_sync_cost / 2
            raise Crash()
        self.clock.now += self.full_sync_cost
        # The monitor runs concurrently with a full scan in the real controller.
        self.c.fresh = self.clock.now
        return 1000

    def accounting(self):
        self.clock.now += self.accounting_cost
        self.c.fresh = self.clock.now

    async def restore_wallet(self, wallet):
        if any(r["sender"] == wallet for r in self.c.j.pending()):
            raise Gate("cannot hand off a wallet with unresolved inputs")
        self.clock.now += self.restore_cost
        self.c.fresh = self.clock.now
        self.restores.append(wallet)

    def reopen(self, legacy=False):
        """A restarted controller process on the same state directory."""
        old = self.c
        old.j.db.close()
        c = (LegacyAnchor if legacy else DiscoveryController).__new__(
            LegacyAnchor if legacy else DiscoveryController
        )
        for name in ("state", "plan", "config", "wallets", "statuses", "blocks",
                     "spent", "report", "spendable", "native", "rpc"):
            setattr(c, name, getattr(old, name))
        c.j = Journal(c.state / "journal.sqlite", lambda: self.clock.now, c.plan["limits"])
        c.fresh = self.clock.now
        c.admission_lock = asyncio.Lock()
        c.closed = False
        self.wire(c)
        self.c = c
        return c

    def run(self, until):
        async def loop():
            while self.clock.now < until and self.c.j.get("status") == "running":
                await self.tick()

        with (
            patch("discovery.time.time", lambda: self.clock.now),
            patch("controller.time.time", lambda: self.clock.now),
        ):
            asyncio.run(loop())

    def tick_once(self):
        """One campaign_tick outside the admission loop (a later boundary)."""
        self.c.fresh = self.clock.now
        with (
            patch("discovery.time.time", lambda: self.clock.now),
            patch("controller.time.time", lambda: self.clock.now),
        ):
            asyncio.run(self.c.campaign_tick())

    def phase_rows(self, pid):
        p = self.phase(pid)
        return self.c.j.rows(
            "kind='campaign' AND offered>=? AND offered<? ORDER BY offered,id",
            (p["start_hour"] * HOUR, p["end_hour"] * HOUR),
        )

    def outcome(self, pid):
        rows = self.phase_rows(pid)
        released = (self.c.j.get("phase_release") or {}).get(pid)
        submitted = [r for r in rows if r["submitted"] is not None]
        return {
            "status": self.c.j.get("status"),
            "reason": self.c.j.get("reason"),
            "active": self.c.j.get("active_phase"),
            "released": released,
            "gate_wait": None if released is None else released - self.boundary,
            "offered": len(rows),
            "submitted": len(submitted),
            "skipped": [r["id"] for r in rows if r["state"] == "skipped"],
            "expired": [r["id"] for r in rows if r["state"] == "expired"],
            "max_slot_lag": max(
                (r["submitted"] - max(r["offered"], released) for r in submitted),
                default=None,
            ),
            "max_offer_lag": max(
                (r["submitted"] - r["offered"] for r in submitted), default=None
            ),
        }

    def finish_phase_on_paper(self, pid):
        """Journal every later offer of ``pid`` as delivered (hours elided)."""
        p = self.phase(pid)
        c = self.c
        cursor = c.j.get("offer_cursor:campaign")
        with c.j.transaction():
            for event in EVENTS[cursor:]:
                if event["offset_seconds"] >= p["end_hour"] * HOUR:
                    break
                c.j.offer(event["intent"], "campaign", event["offset_seconds"], 0, 1, 1)
                cursor += 1
        c.j.set("offer_cursor:campaign", cursor)
        with c.j.transaction():
            for row in c.j.rows(
                "kind='campaign' AND state!='reconciled' AND state!='skipped'"
            ):
                c.j.transition(row["id"], "reconciled", info={})
        self.confirm_at.clear()


class Scenario(unittest.TestCase):
    def fleet(self, boundary_hour, **kwargs):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        fleet = CampaignFleet(tmp.name, boundary_hour, **kwargs)
        self.addCleanup(lambda: fleet.c.j.db.close())
        return fleet

    def cross(self, boundary_hour, after=600, **kwargs):
        fleet = self.fleet(boundary_hour, **kwargs)
        fleet.run(fleet.boundary + after)
        return fleet, fleet.outcome(fleet.phase_at(fleet.boundary))

    def assert_clean(self, fleet, result, rate_per_minute):
        self.assertEqual(result["status"], "running", result["reason"])
        self.assertEqual(result["active"], fleet.phase_at(fleet.boundary))
        # The gate genuinely held the phase for about the x1 wait.
        self.assertGreater(result["gate_wait"], 180)
        self.assertLess(result["gate_wait"], 250)
        held = sum(fleet.boundary <= e["offset_seconds"] < result["released"]
                   for e in EVENTS)
        self.assertGreaterEqual(held, int(result["gate_wait"] * rate_per_minute / 60) - 1)
        self.assertEqual(result["skipped"], [])
        self.assertEqual(result["expired"], [])
        self.assertEqual(result["submitted"], result["offered"])
        # Every slot met its grace measured from max(offered, release).
        self.assertLess(result["max_slot_lag"], 120)
        # offered is the immutable schedule; offer lag includes the gate wait.
        rows = fleet.phase_rows(fleet.phase_at(fleet.boundary))
        self.assertEqual(
            [r["offered"] for r in rows],
            [e["offset_seconds"] for e in EVENTS if e["intent"] in {r["id"] for r in rows}],
        )
        self.assertGreater(result["max_offer_lag"], result["gate_wait"] - 1)
        events = [
            json.loads(d) for (d,) in fleet.c.j.db.execute(
                "SELECT detail FROM events WHERE state='phase_gate_released'")
        ]
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["gate_held_offers"], held)
        self.assertAlmostEqual(events[0]["gate_wait_seconds"], result["gate_wait"])


class FourPerMinuteBoundaryTests(Scenario):
    """correctness -> sustained (T0+1h) at 4/min: the x1 incident."""

    def test_gate_wait_no_longer_expires_the_first_sustained_offers(self):
        fleet, result = self.cross(1)
        self.assert_clean(fleet, result, 4)
        self.assertEqual(fleet.full_syncs, 1)
        report = fleet.c.phase_release_report()
        self.assertEqual(report["sustained"]["released"], result["released"])
        self.assertTrue(report["sustained"]["recorded"])
        self.assertEqual(report["correctness"]["gate_wait_seconds"], 0)

    def test_without_the_fix_the_x1_skips_reproduce_and_hold_t0_plus_24h(self):
        fleet, result = self.cross(1, legacy=True)
        self.assertGreater(result["gate_wait"], 180)
        # x1: sustained-00240..00245 (offered T0+1h .. +75 s) skipped.
        self.assertEqual(result["skipped"], ["sustained-%05d" % i for i in range(240, 246)])
        fleet.finish_phase_on_paper("sustained")
        fleet.clock.now = 24 * HOUR + 5
        with self.assertRaisesRegex(Gate, "previous phase delivery incomplete"):
            fleet.tick_once()
        self.assertEqual(fleet.c.j.get("active_phase"), "sustained")

    def test_with_the_fix_the_t0_plus_24h_boundary_releases(self):
        fleet, result = self.cross(1)
        self.assertEqual(result["skipped"], [])
        fleet.finish_phase_on_paper("sustained")
        fleet.clock.now = 24 * HOUR + 5
        fleet.tick_once()
        self.assertEqual(fleet.c.j.get("active_phase"), "idle")


class TwelvePerMinuteBoundaryTests(Scenario):
    """idle -> ramp (T0+30h) at 12/min: ~40 offers arrive while the gate waits."""

    def test_backlog_from_the_gate_is_delivered_within_grace_of_release(self):
        fleet, result = self.cross(30, full_sync=185)
        self.assert_clean(fleet, result, 12)
        # The gate-held backlog stays inside the queue bound.
        self.assertLess(result["gate_wait"] * 12 / 60,
                        fleet.c.j.limits["max_queued_intents"])

    def test_without_the_fix_ramp_offers_expire(self):
        fleet, result = self.cross(30, full_sync=185, legacy=True)
        self.assertGreaterEqual(len(result["skipped"]), 15)
        self.assertTrue(all(r.startswith("ramp-") for r in result["skipped"]))


class RecoveryBoundaryTests(Scenario):
    """ramp -> recovery (T0+42h): drain a 20/min burst, then restore_wallet."""

    def test_restore_is_part_of_the_gate_and_costs_no_offers(self):
        fleet, result = self.cross(42, full_sync=100, accounting=15, restore=30)
        self.assert_clean(fleet, result, 4)
        self.assertEqual(fleet.restores, [0])
        self.assertTrue(fleet.c.j.get("restore_recovery"))
        # The 20/min ramp tail drained before the gate opened.
        ramp = fleet.phase_rows("ramp")
        self.assertTrue(all(r["state"] == "reconciled" for r in ramp))

    def test_without_the_fix_recovery_offers_expire(self):
        fleet, result = self.cross(42, full_sync=100, accounting=15, restore=30,
                                   legacy=True)
        self.assertTrue(result["skipped"])


class DegradeBoundaryTests(Scenario):
    """recovery -> degrade (T0+48h): D1 opens at the boundary itself."""

    def window(self, fleet):
        return next(w for w in fleet.c.degrade_windows() if w["step"] == "D1")

    def test_d1_window_misses_no_offer_while_the_gate_waits(self):
        fleet, result = self.cross(48)
        self.assert_clean(fleet, result, 4)
        window = self.window(fleet)
        self.assertEqual(window["fault_start"], fleet.boundary)
        # The verdict input is unchanged: offers dated in [fault_start,
        # window_end) by their immutable offered time, none of them missed.
        self.assertEqual(fleet.c.missed_offers(window), [])
        in_window = fleet.c.j.rows(
            "kind='campaign' AND offered>=? AND offered<?",
            (window["fault_start"], window["window_end"]),
        )
        scheduled = [e["intent"] for e in EVENTS
                     if window["fault_start"] <= e["offset_seconds"] < window["window_end"]]
        self.assertGreater(len(in_window), 30)
        self.assertEqual([r["id"] for r in in_window], scheduled[: len(in_window)])
        # Admission during D1 never used its stopped host as ingress.
        self.assertTrue(all(
            json.loads(r["info"]).get("ingress_role") not in window["expected_down"]
            for r in in_window if r["info"]
        ))

    def test_without_the_fix_d1_records_missed_payments(self):
        fleet, result = self.cross(48, legacy=True)
        self.assertTrue(result["skipped"])
        missed = fleet.c.missed_offers(self.window(fleet))
        self.assertEqual([r["id"] for r in missed], result["skipped"])


class RestartTests(Scenario):
    """The release time is durable; a restart re-runs only an unreleased gate."""

    def test_restart_mid_boundary_still_skips_nothing(self):
        fleet = self.fleet(1)
        fleet.crash_on_full_sync = True
        with self.assertRaises(Crash):
            fleet.run(fleet.boundary + 600)
        # Died during the full sync: no release, rows of the new phase are
        # queued and none expired, however long the process stays down.
        self.assertIsNone((fleet.c.j.get("phase_release") or {}).get("sustained"))
        self.assertEqual(fleet.c.j.get("active_phase"), "correctness")
        fleet.clock.now += 20
        c = fleet.reopen()
        with patch("discovery.time.time", lambda: fleet.clock.now):
            c.materialize(EVENTS, 0.0, "campaign", fleet.clock.now)
            queued = c.j.rows("kind='campaign' AND state='planned'")
            self.assertTrue(queued)
            self.assertTrue(all(c.slot_anchor(r) is None for r in queued))
            self.assertTrue(all(c.intent_deadline(r) == float("-inf") for r in queued))
        self.assertEqual(c.j.rows("state='skipped'"), [])
        fleet.run(fleet.boundary + 600)
        result = fleet.outcome("sustained")
        self.assertEqual(result["status"], "running", result["reason"])
        self.assertEqual(result["skipped"], [])
        self.assertEqual(result["submitted"], result["offered"])
        self.assertEqual(fleet.full_syncs, 2)  # the gate re-ran from the top
        self.assertLess(result["max_slot_lag"], 120)
        self.assertGreater(result["gate_wait"], 180)

    def test_restart_after_release_keeps_the_recorded_release(self):
        fleet, result = self.cross(1, after=230)
        released = result["released"]
        self.assertIsNotNone(released)
        fleet.clock.now += 30
        c = fleet.reopen()
        self.assertEqual(c.j.get("phase_release")["sustained"], released)
        fleet.run(fleet.boundary + 600)
        after = fleet.outcome("sustained")
        self.assertEqual(after["released"], released)
        self.assertEqual(after["skipped"], [])
        self.assertEqual(fleet.full_syncs, 1)  # no second gate after a restart
        self.assertEqual(c.j.db.execute(
            "SELECT COUNT(*) FROM events WHERE state='phase_gate_released'"
        ).fetchone()[0], 1)

    def test_journal_without_release_records_keeps_the_offer_clock(self):
        # A pre-#1627 journal (phase_gates only) anchors released phases at
        # their boundaries, i.e. exactly the old rule for already-open phases.
        fleet = self.fleet(1)
        c = fleet.c
        c.j.set("phase_release", None)
        releases = c.phase_releases()
        self.assertEqual(releases, {"correctness": 0.0})
        row = {"kind": "campaign", "offered": 10.0}
        self.assertEqual(c.slot_anchor(row), 10.0)
        self.assertEqual(c.slot_anchor({"kind": "campaign", "offered": HOUR + 1}), None)
        self.assertEqual(c.slot_anchor({"kind": "rehearsal", "offered": 5.0}), 5.0)


class StrictnessTests(Scenario):
    """The previous phase stays strict: bounded drain, and skipped -> Gate."""

    def test_skipped_previous_row_still_gates(self):
        fleet = self.fleet(1, begin=30)
        fleet.c.j.transition("correctness-00200", "skipped", "generator_slot_expired")
        fleet.run(fleet.boundary + 60)
        self.assertEqual(fleet.c.j.get("status"), "held")
        self.assertIn("previous phase delivery incomplete", fleet.c.j.get("reason"))

    def test_drain_beyond_grace_still_gates(self):
        fleet = self.fleet(1, begin=30)
        # A previous-phase payment that never confirms.
        fleet.c.j.transition("correctness-00230", "accepted", submitted=fleet.clock.now,
                             info={"input_count": 1})
        fleet.run(fleet.boundary + 300)
        self.assertEqual(fleet.c.j.get("status"), "held")
        self.assertIn("previous phase drain exceeded bounded grace", fleet.c.j.get("reason"))
        # Nothing of the new phase was skipped or expired by the hold itself.
        self.assertEqual(
            fleet.c.j.rows("kind='campaign' AND offered>=? AND state='skipped'",
                           (fleet.boundary,)), [])


class DeferralLagTests(Scenario):
    def test_gate_held_deferrals_report_offer_lag_and_gate_share(self):
        fleet = self.fleet(30, full_sync=185)
        # max_inflight is read per event: a cap of 2 forces deferrals.
        fleet.c.events = [dict(e, max_inflight=2) for e in EVENTS]
        fleet.run(fleet.boundary + 260)
        released = fleet.c.j.get("phase_release")["ramp"]
        details = {
            intent: json.loads(d) for intent, d in fleet.c.j.db.execute(
                "SELECT intent, detail FROM events WHERE state='offer_deferred'")
        }
        self.assertTrue(details)
        offered = {r["id"]: r["offered"] for r in fleet.c.j.rows("kind='campaign'")}
        for intent, detail in details.items():
            gate_share = max(0.0, released - offered[intent])
            if gate_share:
                self.assertAlmostEqual(detail["gate_held_seconds"], gate_share)
                self.assertGreaterEqual(detail["lag_seconds"], gate_share)
            else:
                self.assertNotIn("gate_held_seconds", detail)
        self.assertTrue(any("gate_held_seconds" in d for d in details.values()))


if __name__ == "__main__":
    unittest.main()
