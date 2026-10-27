"""#1618: one admission tick may prepare and submit several due offers.

x1 attempt b held 480 s into its rehearsal: offers arrived at 30/min (every
2 s) but the controller admitted at most one transaction per run-loop tick,
about 2.55 s apart, so the backlog grew until a prepare crossed the 120 s
start-slot grace ("preparation expired or intent already consumed"). These
tests drive the real admission_tick -> setup_tick -> deliver -> prepare ->
submit_prepared path and the real Journal gates on a virtual clock. Only the
network, the native signer and coin selection are faked, with serial costs
calibrated to attempt b.
"""

import asyncio
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import discovery
from discovery import DiscoveryController
from reporting import offer_deferrals
from runtime import HOSTS, Journal, Quota

HERE = Path(__file__).parent

# Attempt b (#1618): prepare-to-prepare gap p50 2.55 s, eligible->prepared
# p50 0.69 s, prepared->submitted p50 0.01 s. The rest of the gap is the
# read-only scan, coin selection and the 0.2 s minimum run-loop sleep.
SCAN = 1.4  # sync(): key-image RPCs, inventory/blocks writes, wallet scans
SELECT = 0.2  # per-intent coin selection
SIGN = 0.69  # fee quote + native signing
SUBMIT = 0.05  # artifact inspect + tx_submit reply


def x_series():
    return json.loads((HERE / "x-series-plan.json").read_text())


# x1 attempt b offered the rehearsal at 30/min (every 2 s). #1620 lowered the
# x-series peak to 20/min (every 3 s); the controller-mechanics tests below
# keep the 2 s cadence they were calibrated against.
ATTEMPT_B_INTERVAL = 2


class Clock:
    def __init__(self):
        self.now = 0.0


class Fleet:
    """A DiscoveryController in rehearsal setup, faked below prepare()."""

    def __init__(self, tmp, limits=None, latency=(50, 75), interval=ATTEMPT_B_INTERVAL):
        self.clock = Clock()
        self.latency = latency
        plan = x_series()
        if limits:
            plan["limits"].update(limits)
        if interval is not None:
            plan["rehearsal"]["interval_seconds"] = interval
        c = DiscoveryController.__new__(DiscoveryController)
        c.state = Path(tmp)
        (c.state / "artifacts").mkdir()
        c.plan = plan
        c.config = {}
        c.j = Journal(c.state / "journal.sqlite", lambda: self.clock.now, plan["limits"])
        c.wallets = [
            {"key": f"wallet-{i}.mnemonic", "address": f"addr-{i}"}
            for i in range(plan["wallets"]["count"])
        ]
        c.statuses = {h: {"synced": True, "chainHeight": 1000} for h in HOSTS}
        c.fresh = 0.0
        c.admission_lock = asyncio.Lock()
        c.closed = False
        c.blocks, c.spent = [], []
        c.report = Mock()
        c.sync = self.sync
        c.spendable = self.spendable
        c.native = self.native
        c.rpc = Mock(call=self.rpc)
        c.j.set("status", "setup")
        c.j.set("setup_start", 0.0)
        c.j.set("opening_verified", True)
        c.j.set("rehearsal_start", 0.0)
        self.c = c
        self.signed = {}
        self.confirm_at = {}
        self.scans = 0
        self.signatures = 0
        self.inputs = 0
        self.quota = 0
        self.ticks = []

    async def sync(self):
        self.scans += 1
        self.clock.now += SCAN
        return 1000

    def spendable(self, wallet, height, amount=0, count=None):
        self.clock.now += SELECT
        self.inputs += 1
        return [{"id": f"input-{self.inputs}"}]

    async def native(self, request):
        if request["operation"] == "prepare":
            self.clock.now += SIGN
            self.signatures += 1
            artifact = Path(request["artifact"])
            name = artifact.stem
            artifact.write_bytes(name.encode())
            self.signed[name] = {
                "artifact": str(artifact),
                "hash": hashlib.sha256(name.encode()).hexdigest(),
                "fee": 4 * 10**8,
                "baseline_fee": 10**8,
                "fee_multiplier": request["fee_multiplier"],
                "bytes": 5000,
                "input_count": len(request["selected"]),
                "outputs": [],
                "key_images": ["image-" + name],
                "selected": [{"id": i} for i in request["selected"]],
            }
            return dict(self.signed[name])
        if request["operation"] == "inspect":
            return dict(self.signed[Path(request["artifact"]).stem])
        raise AssertionError(request["operation"])

    async def rpc(self, host, method, params=None, **_):
        if method == "fee_getRate":
            return {"baseRate": 1}
        if method == "getTransactionStatus":
            return {"status": "unknown"}
        if method == "tx_submit":
            self.clock.now += SUBMIT
            name = bytes.fromhex(params["tx_hex"]).decode()
            low, high = self.latency
            spread = int(hashlib.sha256(name.encode()).hexdigest(), 16) % (high - low + 1)
            self.confirm_at[name] = self.clock.now + low + spread
            return {"txHash": self.signed[name]["hash"]}
        raise AssertionError(method)

    def settle(self):
        """The reconciler: confirmed and scanned after the drawn latency."""
        for name, at in list(self.confirm_at.items()):
            if at <= self.clock.now:
                self.c.j.finish(name, {})
                del self.confirm_at[name]

    async def tick(self):
        """One iteration of Controller.run()'s admission loop."""
        c = self.c
        started = self.clock.now
        self.settle()
        c.fresh = self.clock.now
        before = self.signatures
        try:
            await c.admission_tick()
        except Quota as error:
            self.quota += 1
            c.j.event(None, "admission_quota", str(error))
        except Exception as error:
            c.halt(str(error))
        self.ticks.append(self.signatures - before)
        self.clock.now += max(0.2, 1 - (self.clock.now - started))

    def run(self, until, budget=None):
        budget = discovery.ADMISSION_TICK_BUDGET_SECONDS if budget is None else budget

        async def loop():
            while self.clock.now < until and self.c.j.get("status") == "setup":
                await self.tick()

        with (
            patch("discovery.time.time", lambda: self.clock.now),
            patch("controller.time.time", lambda: self.clock.now),
            patch.object(discovery, "ADMISSION_TICK_BUDGET_SECONDS", budget),
        ):
            asyncio.run(loop())
        return self.result()

    def result(self):
        rows = self.c.j.rows("kind='rehearsal'")
        lags = [r["submitted"] - r["offered"] for r in rows if r["submitted"] is not None]
        return {
            "status": self.c.j.get("status"),
            "reason": self.c.j.get("reason"),
            "offered": len(rows),
            "submitted": len(lags),
            "skipped": sum(r["state"] == "skipped" for r in rows),
            "max_lag": max(lags) if lags else None,
            "deferrals": offer_deferrals(self.c.j, "rehearsal"),
        }


class RehearsalThroughputTests(unittest.TestCase):
    """x1 attempt b's rehearsal: 1200 s, one offer every 2 s (540 offers)."""

    def simulate(self, budget, interval=ATTEMPT_B_INTERVAL):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        fleet = Fleet(tmp.name, interval=interval)
        self.addCleanup(fleet.c.j.db.close)
        # Stop before the rehearsal deadline (start + 1200 s) evaluates it.
        result = fleet.run(until=1195, budget=budget)
        settings = fleet.c.plan["rehearsal"]
        self.assertEqual(
            (settings["duration_seconds"], settings["interval_seconds"]),
            (1200, interval or x_series()["rehearsal"]["interval_seconds"]),
        )
        return fleet, result

    def test_one_admission_per_tick_reproduces_the_x1_backlog(self):
        # Budget 0 is the pre-#1618 pass: stop after the first attempt. The
        # backlog still overruns the 120 s grace, but since #1635 an intent
        # that runs out of slot is skipped, never a campaign hold.
        fleet, before = self.simulate(budget=0)
        self.assertEqual(max(fleet.ticks), 1)
        self.assertEqual(before["status"], "setup")
        self.assertGreater(before["max_lag"], 100)
        self.assertLess(before["submitted"], 540)
        self.assertGreater(before["skipped"], 0)

    def test_multi_offer_ticks_hold_the_30_per_minute_offer_clock(self):
        fleet, after = self.simulate(budget=None)
        self.assertEqual(after["status"], "setup")
        self.assertEqual(after["offered"], 540)
        self.assertEqual(after["submitted"], 540)
        self.assertEqual(after["skipped"], 0)
        self.assertLess(after["max_lag"], 30)
        # Never more than the pre-#1618 bound of four attempts in one pass.
        self.assertLessEqual(max(fleet.ticks), discovery.MAX_ATTEMPTS_PER_PASS)
        self.assertGreater(max(fleet.ticks), 1)
        self.assertEqual(after["deferrals"]["sender_busy"], 0)
        self.assertEqual(after["deferrals"]["inflight_cap"], 0)


class XSeriesRehearsalTests(unittest.TestCase):
    """#1620: the x-series rehearsal as planned, 1200 s at 20/min (360 offers)."""

    def simulate(self, budget):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        fleet = Fleet(tmp.name, interval=None)  # the plan's own cadence
        self.addCleanup(fleet.c.j.db.close)
        return fleet, fleet.run(until=1195, budget=budget)

    def test_plan_rehearsal_is_20_per_minute(self):
        settings = x_series()["rehearsal"]
        self.assertEqual(settings["interval_seconds"], 3)
        self.assertEqual(60 // settings["interval_seconds"], 20)

    def test_20_per_minute_keeps_lag_well_inside_the_cadence_bound(self):
        interval = x_series()["rehearsal"]["interval_seconds"]
        # rehearsal_result's lag bound: min(grace, max(1, min(5, 2 x interval))).
        bound = min(120, max(1.0, min(5.0, 2 * interval)))
        for budget in (None, 0):  # multi-offer pass, and the pre-#1618 single pass
            with self.subTest(budget=budget):
                fleet, result = self.simulate(budget)
                self.assertEqual(result["status"], "setup")
                self.assertEqual(result["offered"], 360)
                self.assertEqual(result["submitted"], 360)
                self.assertEqual(result["skipped"], 0)
                self.assertLess(result["max_lag"], 4)
                self.assertLess(result["max_lag"], bound)
                self.assertIsNone(result["deferrals"]["binding_limit"])


class PassBoundTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.fleet = Fleet(tmp.name)
        self.addCleanup(self.fleet.c.j.db.close)
        self.c = self.fleet.c

    def tick_at(self, now, budget=None):
        self.fleet.clock.now = now
        return self.fleet.run(until=now + 0.001, budget=budget)

    def test_one_pass_shares_one_scan_and_respects_the_attempt_bound(self):
        # Six offers due at once (offsets 0..10 s), no cap or sender limit.
        result = self.tick_at(10.0, budget=1000)
        self.assertEqual(self.fleet.ticks, [discovery.MAX_ATTEMPTS_PER_PASS])
        self.assertEqual(self.fleet.scans, 1)
        self.assertEqual(result["submitted"], 4)
        # The two admissible offers left behind name controller throughput.
        self.assertEqual(result["deferrals"]["admission_backlog"], 2)
        self.assertEqual(result["deferrals"]["binding_limit"], "admission_backlog")

    def test_wall_clock_budget_ends_the_pass(self):
        # SCAN + 2 x (SELECT + SIGN + SUBMIT) = 3.28 s < 3.5 <= third start.
        self.tick_at(10.0, budget=3.5)
        self.assertEqual(self.fleet.ticks, [3])
        self.tick_at(30.0, budget=2.0)
        self.assertEqual(self.fleet.ticks, [3, 1])

    def test_inflight_cap_and_busy_senders_stay_binding_mid_pass(self):
        self.c.plan["limits"]["max_inflight"] = 2
        result = self.tick_at(10.0, budget=1000)
        self.assertEqual(result["submitted"], 2)
        self.assertEqual(result["deferrals"]["inflight_cap"], 4)
        self.assertEqual(result["deferrals"]["admission_backlog"], 0)

    def test_quota_stops_new_signing_and_the_signature_keeps_its_slot(self):
        # A rolling window with room for exactly two more submissions.
        quota = self.c.j.limits["max_normal_submissions_per_minute"]
        for i in range(quota - 2):
            self.c.j.offer(f"past-{i}", "funding", 0, None, 1, 1)
            self.c.j.transition(f"past-{i}", "reconciled", submitted=5.0, info={})
        self.tick_at(10.0, budget=1000)
        # Third signature hit the quota: it waits, nothing after it signs.
        self.assertEqual(self.fleet.ticks, [3])
        self.assertEqual(self.fleet.quota, 1)
        waiting = self.c.j.rows("state='prepared'")
        self.assertEqual([r["id"] for r in waiting], ["rehearsal-00002"])
        # Still inside the window: the waiting row retries, no new signature.
        self.tick_at(20.0, budget=1000)
        self.assertEqual(self.fleet.ticks, [3, 0])
        self.assertEqual(self.fleet.quota, 2)
        # Window released: the waiting row submits first, and that tick ends.
        self.tick_at(66.0, budget=1000)
        self.assertEqual(self.fleet.ticks, [3, 0, 0])
        self.assertEqual(self.c.j.intent("rehearsal-00002")["state"], "accepted")
        self.assertEqual(self.c.j.rows("state='prepared'"), [])
        self.tick_at(67.0, budget=1000)
        self.assertGreater(self.fleet.ticks[-1], 0)


if __name__ == "__main__":
    unittest.main()
