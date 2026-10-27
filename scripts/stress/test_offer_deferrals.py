"""#1615: x-series sender headroom and the offer-deferral report metric.

Rehearsal attempt d held because 32 senders (one inflight spend each) and a
global cap of 32 bound a 30/min peak at ~50-70 s submit-to-reconciled. The
plan now carries 64 wallets and a cap of 64. The controller journals, once per
offer and reason, every due offer it holds back because its sender was busy or
the inflight cap was full, and the report names the binding limit.
"""

import asyncio
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from discovery import DiscoveryController
from plan import expand
from prefund import latest_deadline, load_manifest
from reporting import offer_deferrals, rehearsal_summary
from runtime import Gate, Journal
from test_targets import targets

HERE = Path(__file__).parent


def x_series():
    return json.loads((HERE / "x-series-plan.json").read_text())


class PlanHeadroomTests(unittest.TestCase):
    def test_x_series_validates_with_64_wallets_and_cap(self):
        plan = x_series()
        self.assertEqual(plan["wallets"]["count"], 64)
        self.assertEqual(plan["wallets"]["max_inflight_per_wallet"], 1)
        self.assertEqual(plan["limits"]["max_inflight"], 64)
        self.assertEqual({p["max_inflight"] for p in plan["phases"]}, {64})
        summary, events = expand(plan)
        self.assertEqual(len(events), 21880)  # #1620: 20/min bursts
        self.assertEqual({e["sender"] for e in events}, {f"wallet-{i}" for i in range(1, 65)})

    def test_every_sender_rests_192_s_at_the_20_per_minute_peak(self):
        _, events = expand(x_series())
        last, gaps = {}, []
        for event in events:
            sender = event["sender"]
            if sender in last:
                gaps.append(event["offset_seconds"] - last[sender])
            last[sender] = event["offset_seconds"]
        # #1620: 64 offers at the 3 s burst cadence, three times attempt d's
        # 64 s reuse and above the ~170 s reconcile maximum seen on x1.
        self.assertGreaterEqual(min(gaps), 192)

    def test_validator_still_caps_wallets_at_64(self):
        plan = x_series()
        plan["wallets"]["count"] = 65
        with self.assertRaises(ValueError):
            expand(plan)
        plan = x_series()
        plan["phases"][1]["max_inflight"] = 65
        with self.assertRaises(ValueError):
            expand(plan)


class PrefundSizingTests(unittest.TestCase):
    """The x-series manifest shape: 64 wallets x 32 grants of 5x10^10."""

    def manifest(self, grants, amount):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)

        def private(name, data):
            path = root / name
            path.write_bytes(data)
            path.chmod(0o600)
            return str(path)

        plan = x_series()
        plan["endpoints"] = [t["rpc_url"] for t in targets()]
        plan_bytes = json.dumps(plan).encode()
        wallets = [
            {"address": f"addr-{i}", "key": private(f"w{i}.mnemonic", b"w%d" % i)}
            for i in range(plan["wallets"]["count"])
        ]
        end = 1_000_000.0 + 400 * 3600
        m = {
            "schema_version": 1,
            "plan_file": private("plan.json", plan_bytes),
            "plan_sha256": hashlib.sha256(plan_bytes).hexdigest(),
            "targets": targets(),
            "tls_ca_file": private("ca.pem", b"ca"),
            "tls_ca_sha256": hashlib.sha256(b"ca").hexdigest(),
            "signer": private("signer", b"signer"),
            "signer_sha256": hashlib.sha256(b"signer").hexdigest(),
            "producer_role": "faucet.botho.io",
            "funding_wallet": {"address": "addr-funding", "key": private("p.mnemonic", b"p")},
            "wallets": wallets,
            "grants_per_wallet": grants,
            "grant_picocredits": amount,
            "state_dir": str(root / "state"),
            "deadline": latest_deadline(plan, end),
            "infrastructure_end": end,
        }
        path = root / "manifest.json"
        path.write_text(json.dumps(m))
        path.chmod(0o600)
        return path

    def test_64_wallets_keep_x1_principal_and_inventory_depth(self):
        m, plan = load_manifest(self.manifest(32, 5 * 10**10))
        grants = plan["wallets"]["count"] * m["grants_per_wallet"]
        self.assertEqual(grants, 2048)
        self.assertEqual(grants * m["grant_picocredits"], 1024 * 10**11)
        self.assertLessEqual(
            grants * int(plan["limits"]["max_single_signed_fee_picocredits"]),
            int(plan["limits"]["max_signed_fee_picocredits"]),
        )

    def test_x1_grant_size_at_64_wallets_exceeds_principal(self):
        with self.assertRaisesRegex(Gate, "max_principal"):
            load_manifest(self.manifest(32, 10**11))


class Clock:
    def __init__(self):
        self.now = 0.0


def controller(tmp, limits):
    clock = Clock()
    c = DiscoveryController.__new__(DiscoveryController)
    c.j = Journal(Path(tmp) / "j.sqlite", lambda: clock.now, limits=limits)
    return c, clock


def event(i, sender, at, cap=64):
    return {
        "intent": f"rehearsal-{i:05d}",
        "offset_seconds": at,
        "sender": f"wallet-{sender + 1}",
        "recipient": f"wallet-{sender + 2}",
        "amount_picocredits": "1000",
        "max_inflight": cap,
        "input_count": None,
        "fee_multiplier": 1,
        "cohort": i // 3,
    }


class FakeSubmitter:
    """Stands in for Controller.prepare: sign and submit at once; the row
    stays pending until ``finish`` after ``latency`` seconds."""

    def __init__(self, c, clock, latency, grace=120):
        self.c, self.clock, self.latency, self.grace = c, clock, latency, grace
        self.finishing = {}

    async def __call__(self, identifier, count, phase_limit, **_):
        j, now = self.c.j, self.clock.now
        row = j.intent(identifier)
        deadline = row["offered"] + self.grace
        j.transition(identifier, "eligible")
        artifact = {"hash": identifier, "fee": 1000, "bytes": 5000,
                    "selected": [{"id": "in-" + identifier}]}
        j.prepare(identifier, artifact, phase_limit, deadline)
        j.begin_submit(identifier, "h", 6000, False, deadline)
        self.finishing[identifier] = now + self.latency
        return True

    def settle(self):
        for identifier, at in list(self.finishing.items()):
            if at <= self.clock.now:
                self.c.j.finish(identifier, {})
                del self.finishing[identifier]


def run(c, clock, events, seconds, submitter, kind="rehearsal"):
    c.prepare = submitter
    for t in range(seconds):
        clock.now = float(t)
        submitter.settle()
        with patch("discovery.time.time", return_value=clock.now):
            asyncio.run(c.deliver(events, 0, kind))


class DeferralMetricTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.limits = copy.deepcopy(x_series()["limits"])

    def test_busy_sender_is_counted_once_and_does_not_block_others(self):
        c, clock = controller(self.tmp.name, self.limits)
        self.addCleanup(c.j.db.close)
        # Wallet 0 offers twice in a row; wallet 1's offer must still go.
        events = [event(0, 0, 0), event(1, 0, 1), event(2, 1, 2)]
        submitter = FakeSubmitter(c, clock, latency=30)
        run(c, clock, events, 60, submitter)
        rows = {r["id"]: r for r in c.j.rows()}
        self.assertEqual(rows["rehearsal-00002"]["submitted"], 2.0)
        self.assertEqual(rows["rehearsal-00001"]["submitted"], 30.0)
        result = offer_deferrals(c.j, "rehearsal")
        self.assertEqual(
            result,
            {"sender_busy": 1, "inflight_cap": 0, "admission_backlog": 0, "offers_deferred": 1,
             "skipped_after_deferral": 0, "binding_limit": "sender_busy"},
        )
        detail = json.loads(c.j.db.execute(
            "SELECT detail FROM events WHERE state='offer_deferred'").fetchone()[0])
        self.assertEqual(detail, {"kind": "rehearsal", "reason": "sender_busy", "lag_seconds": 0.0})

    def test_inflight_cap_is_named_when_it_binds(self):
        c, clock = controller(self.tmp.name, self.limits)
        self.addCleanup(c.j.db.close)
        events = [event(i, i, i, cap=2) for i in range(4)]
        submitter = FakeSubmitter(c, clock, latency=10)
        run(c, clock, events, 40, submitter)
        result = offer_deferrals(c.j, "rehearsal")
        self.assertEqual(result["inflight_cap"], 2)
        self.assertEqual(result["sender_busy"], 0)
        self.assertEqual(result["binding_limit"], "inflight_cap")
        self.assertEqual(
            sum(r["state"] == "reconciled" for r in c.j.rows()), 4
        )

    def test_deferrals_survive_a_restart_without_double_counting(self):
        c, clock = controller(self.tmp.name, self.limits)
        self.addCleanup(c.j.db.close)
        events = [event(0, 0, 0), event(1, 0, 1)]
        submitter = FakeSubmitter(c, clock, latency=100)
        run(c, clock, events, 5, submitter)
        restarted = DiscoveryController.__new__(DiscoveryController)
        restarted.j = c.j
        restarted.prepare = submitter
        for t in range(5, 10):
            clock.now = float(t)
            with patch("discovery.time.time", return_value=clock.now):
                asyncio.run(restarted.deliver(events, 0, "rehearsal"))
        count = c.j.db.execute(
            "SELECT COUNT(*) FROM events WHERE state='offer_deferred'").fetchone()[0]
        self.assertEqual(count, 1)

    def test_no_deferrals_reports_no_binding_limit_and_kinds_are_separate(self):
        c, clock = controller(self.tmp.name, self.limits)
        self.addCleanup(c.j.db.close)
        run(c, clock, [event(0, 0, 0), event(1, 0, 1)], 5,
            FakeSubmitter(c, clock, latency=100), kind="campaign")
        self.assertEqual(offer_deferrals(c.j, "campaign")["sender_busy"], 1)
        self.assertEqual(
            offer_deferrals(c.j, "rehearsal"),
            {"sender_busy": 0, "inflight_cap": 0, "admission_backlog": 0, "offers_deferred": 0,
             "skipped_after_deferral": 0, "binding_limit": None},
        )

    def test_rehearsal_summary_carries_the_metric(self):
        c, clock = controller(self.tmp.name, self.limits)
        self.addCleanup(c.j.db.close)
        run(c, clock, [event(0, 0, 0), event(1, 0, 1)], 5, FakeSubmitter(c, clock, latency=100))
        summary = rehearsal_summary(c.j, x_series())
        self.assertEqual(summary["offer_deferrals"]["sender_busy"], 1)
        self.assertEqual(summary["offer_deferrals"]["binding_limit"], "sender_busy")


class AttemptDShapeTests(unittest.TestCase):
    """Attempt d's rehearsal profile (30/min, 2 s, 1080 s of offers) through
    the real deliver/materialize path and Journal gates, at a constant latency
    near attempt d's p90 (71.6 s). Constant latency is optimistic: attempt d's
    spread (max 165 s) crossed the grace sooner. #1620 lowered the plan's
    rehearsal to 20/min (3 s); `interval=None` runs the plan's own cadence."""

    def simulate(self, wallets, cap, latency, seconds=1080, interval=2):
        plan = x_series()
        plan["limits"]["max_inflight"] = cap
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        c, clock = controller(tmp.name, plan["limits"])
        self.addCleanup(c.j.db.close)
        interval = interval or plan["rehearsal"]["interval_seconds"]
        events = [event(i, i % wallets, i * interval, cap) for i in range(seconds // interval)]
        run(c, clock, events, seconds + 200, FakeSubmitter(c, clock, latency))
        rows = c.j.rows("kind='rehearsal'")
        lags = [r["submitted"] - r["offered"] for r in rows if r["submitted"] is not None]
        return {
            "skipped": sum(r["state"] == "skipped" for r in rows),
            "max_lag": max(lags),
            "deferrals": offer_deferrals(c.j, "rehearsal"),
        }

    def test_32_wallets_skip_offers_and_report_sender_reuse(self):
        # 32 senders recur every 64 s: at 75 s each cycle adds ~11 s of lag.
        before = self.simulate(32, 32, latency=75)
        self.assertGreater(before["skipped"], 0)
        self.assertGreater(before["max_lag"], 100)
        self.assertEqual(before["deferrals"]["binding_limit"], "sender_busy")
        self.assertGreater(before["deferrals"]["skipped_after_deferral"], 0)

    def test_64_wallets_hold_the_offer_clock(self):
        # 64 senders recur every 128 s: no deferral up to ~125 s latency.
        for latency in (75, 125):
            after = self.simulate(64, 64, latency=latency)
            self.assertEqual(after["skipped"], 0, latency)
            self.assertEqual(after["max_lag"], 0, latency)
            self.assertEqual(after["deferrals"]["offers_deferred"], 0, latency)

    def test_64_wallets_hold_the_20_per_minute_plan_cadence(self):
        # #1620: 64 senders at 3 s recur every 192 s, past the ~170 s
        # reconcile maximum seen on x1.
        self.assertEqual(x_series()["rehearsal"]["interval_seconds"], 3)
        for latency in (75, 125, 170):
            after = self.simulate(64, 64, latency=latency, interval=None)
            self.assertEqual(after["skipped"], 0, latency)
            self.assertEqual(after["max_lag"], 0, latency)
            self.assertEqual(after["deferrals"]["offers_deferred"], 0, latency)


if __name__ == "__main__":
    unittest.main()
