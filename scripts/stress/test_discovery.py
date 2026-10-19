"""Adversarial checks for isolated discovery workloads; no live network traffic."""

import copy
import json
from pathlib import Path
import tempfile
import unittest

from plan import expand
from runtime import Gate, Journal, Quota


class DiscoveryTests(unittest.TestCase):
    def profile(self):
        return json.loads(
            Path(__file__).with_name("testnet-discovery-v2.json").read_text()
        )

    def test_schedule_is_dense_with_exactly_one_long_idle(self):
        plan = self.profile()
        summary, events = expand(plan)
        self.assertGreater(len(events), 15000)
        idle = [p for p in plan["phases"] if not p["segments"]]
        self.assertEqual([(p["start_hour"], p["end_hour"]) for p in idle], [(24, 30)])
        self.assertEqual(summary["nominal_campaign_payments"], len(events))
        self.assertEqual({e["fee_multiplier"] for e in events}, {1, 2, 4})

    def test_mutated_limits_or_public_opt_in_cannot_expand(self):
        for field, value in [
            ("max_inflight", 1000),
            ("max_normal_submissions_per_minute", 999999),
        ]:
            plan = self.profile()
            plan["limits"][field] = value
            with self.assertRaises(ValueError):
                expand(plan)
        plan = self.profile()
        plan["isolated_only"] = False
        with self.assertRaises(ValueError):
            expand(plan)

    def test_rehearsal_requires_real_receipts_and_no_skips(self):
        from discovery import rehearsal_result

        rows = [
            {
                "state": "reconciled",
                "offered": 100 + i * 2,
                "submitted": 101 + i * 2,
                "finished": 103 + i * 2,
            }
            for i in range(30)
        ]
        self.assertEqual(
            rehearsal_result(
                rows, 30, 60, start=100, cutoff=280, receipt_deadline=340, now=340
            )["status"],
            "passed",
        )
        rows[4]["state"] = "skipped"
        self.assertEqual(
            rehearsal_result(
                rows, 30, 60, start=100, cutoff=280, receipt_deadline=340, now=340
            )["status"],
            "generator_limited",
        )
        rows[4]["state"] = "reconciled"
        rows[5]["submitted"] = None
        self.assertEqual(
            rehearsal_result(
                rows, 30, 60, start=100, cutoff=280, receipt_deadline=340, now=340
            )["status"],
            "generator_limited",
        )

    def test_missing_or_mutated_fee_evidence_does_not_pass(self):
        from fee_evidence import signed_fee_evidence, summarize_fees

        info = {
            "fee": 200000003,
            "baseline_fee": 100000000,
            "fee_multiplier": 2,
            "bytes": 1000,
            "input_count": 1,
        }
        self.assertEqual(signed_fee_evidence(info, 2)["dust_absorbed"], 3)
        for broken in (
            {**info, "fee_multiplier": 1},
            {**info, "baseline_fee": 0},
            {**info, "fee": 199999999},
        ):
            with self.assertRaises(Gate):
                signed_fee_evidence(broken, 2)
        self.assertEqual(summarize_fees([], [])["dynamic_activation"], "not_exercised")
        self.assertEqual(summarize_fees([], [])["priority"], "not_exercised")


if __name__ == "__main__":
    unittest.main()


class JournalBoundTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "journal.sqlite"
        self.limits = DiscoveryTests().profile()["limits"]
        self.j = Journal(self.path, lambda: 1000, limits=self.limits)
        self.addCleanup(lambda: self.j.db.close())

    def prepare(self, index, selected=None, fee=100_000_000):
        name = str(index)
        self.j.offer(name, "campaign", 1000, index, index + 1, 100)
        self.j.transition(name, "eligible")
        info = {
            "hash": format(index, "064x"),
            "fee": fee,
            "bytes": 5000,
            "selected": selected or [{"id": name}],
        }
        self.j.prepare(name, info, 32, 1120)

    def test_v2_rate_and_budget_enforced_in_journal(self):
        for i in range(30):
            self.prepare(i)
            self.j.begin_submit(str(i), "seed.botho.io", 10000, False, 1120)
        self.prepare(30)
        with self.assertRaises(Quota):
            self.j.begin_submit("30", "seed.botho.io", 10000, False, 1120)
        self.assertIsNone(self.j.intent("30")["submitted"])

    def test_changed_limits_or_duplicate_inputs_fail_atomically(self):
        self.prepare(0)
        import sqlite3

        with self.assertRaises(sqlite3.IntegrityError):
            self.prepare(1, [{"id": "0"}])
        self.assertEqual(self.j.intent("1")["state"], "eligible")
        with self.assertRaises(Gate):
            self.prepare(2, [{"id": "x"}, {"id": "x"}])
        self.j.db.close()
        changed = {**self.limits, "max_inflight": 31}
        with self.assertRaisesRegex(Gate, "limits changed"):
            Journal(self.path, limits=changed)
        self.j = Journal(self.path, limits=self.limits)

    def test_historical_journal_cannot_be_reinterpreted(self):
        path = Path(self.tmp.name) / "old.sqlite"
        old = Journal(path)
        old.offer("held", "campaign", 100, 0, 1, 1)
        old.db.close()
        with self.assertRaisesRegex(Gate, "historical journal"):
            Journal(path, limits=self.limits)

    def test_actual_fee_and_wire_limits_fail_before_marker(self):
        with self.assertRaises(Gate):
            self.prepare(
                0, fee=int(self.limits["max_single_signed_fee_picocredits"]) + 1
            )
        self.prepare(1)
        with self.assertRaises(Quota):
            self.j.begin_submit(
                "1",
                "seed.botho.io",
                self.limits["max_wire_submission_bytes_per_minute"] + 1,
                False,
                1120,
            )
        self.assertIsNone(self.j.intent("1")["submitted"])


class DiscoverySelectionTests(unittest.TestCase):
    def fixture(self, inputs, decoys):
        from test_multi_input import MultiInputDecoyTests

        helper = MultiInputDecoyTests()
        helper.setUp()
        self.addCleanup(helper.doCleanups)
        helper.pool(inputs, decoys)
        return helper.c

    def test_sparse_large_coin_uses_smaller_mature_combination(self):
        from selection import choose

        c = self.fixture([(0, 100), (0, 140), (0, 140)], [100] * 4 + [140] * 23)
        c.inventory[0][0]["utxo"]["amount"] = 10**12
        for coin in c.inventory[0][1:]:
            coin["utxo"]["amount"] = 6_000_000_000
        selected, _ = choose(c, 0, 170, 6_000_000_000)
        self.assertEqual(len(selected), 2)
        self.assertEqual({o["utxo"]["created_at"] for o in selected}, {140})

    def test_complete_set_exclusion_and_bounded_alternate_selection(self):
        from selection import choose

        c = self.fixture([(0, 100), (0, 100), (0, 140)], [100] * 18 + [140] * 19)
        selected, _ = choose(c, 0, 170, 1, 2)
        self.assertEqual(len(selected), 2)
        self.assertEqual({o["utxo"]["created_at"] for o in selected}, {100, 140})
        c.inventory[0] = c.inventory[0][:2]
        selected, reason = choose(c, 0, 170, 1, 2)
        self.assertEqual(selected, [])
        self.assertIn("decoys", reason["reason"])

    def test_reservation_and_maturity_reasons(self):
        from selection import choose

        c = self.fixture([(0, 165)], [165] * 30)
        selected, reason = choose(c, 0, 170, 1)
        self.assertEqual(selected, [])
        self.assertEqual(reason["reason"], "maturity")
        c = self.fixture([(0, 100)], [100] * 30)
        c.j.db.execute(
            "INSERT INTO reservations VALUES(?,?,?)",
            (c.inventory[0][0]["id"], "old", 0),
        )
        selected, reason = choose(c, 0, 170, 1)
        self.assertEqual(reason["reason"], "reservation_or_spent")

    def test_replay_consumption_eventually_reports_inventory_starvation(self):
        from selection import choose

        c = self.fixture([(0, 100)] * 8, [100] * 24)
        spent = []
        for i in range(8):
            selected, _ = choose(c, 0, 170, 1)
            self.assertEqual(len(selected), 1)
            self.assertNotIn(selected[0]["id"], spent)
            spent.append(selected[0]["id"])
            c.j.db.execute(
                "INSERT INTO reservations VALUES(?,?,?)", (selected[0]["id"], str(i), 0)
            )
        self.assertEqual(choose(c, 0, 170, 1)[0], [])


class FeeCoverageTests(unittest.TestCase):
    def test_quote_recovery_must_follow_same_node_height_and_load(self):
        from fee_evidence import summarize_fees

        hot = {
            "host": "a",
            "at": 100,
            "height": 10,
            "adjustmentActive": True,
            "slot_seconds": 3,
            "congestion": 0.9,
            "baseRate": 2,
            "baseMin": 1,
        }
        cold = {
            "host": "b",
            "at": 101,
            "height": 11,
            "adjustmentActive": False,
            "slot_seconds": 40,
            "congestion": 0.1,
            "baseRate": 1,
            "baseMin": 1,
        }
        self.assertEqual(
            summarize_fees([], [hot, cold])["quote_recovery"], "not_exercised"
        )
        cold["host"] = "a"
        self.assertEqual(summarize_fees([], [hot, cold])["quote_recovery"], "observed")
        cold["height"] = 10
        self.assertEqual(
            summarize_fees([], [hot, cold])["quote_recovery"], "not_exercised"
        )

    def test_quote_without_timing_never_establishes_activation(self):
        from fee_evidence import quote_evidence, summarize_fees

        quote = {
            "baseRate": 2,
            "baseMin": 1,
            "baseMax": 10,
            "congestion": 0.9,
            "adjustmentActive": True,
        }
        sample = quote_evidence(quote, {"chainHeight": 10}, "a", 100)
        self.assertEqual(
            summarize_fees([], [sample])["dynamic_activation"], "not_exercised"
        )
        from runtime import Gate

        with self.assertRaises(Gate):
            quote_evidence(
                {**quote, "congestion": float("nan")}, {"chainHeight": 10}, "a", 100
            )


class DiscoveryExecutorTests(unittest.TestCase):
    def setUp(self):
        from test_setup import SetupInventoryTests
        from discovery import DiscoveryController

        helper = SetupInventoryTests()
        helper.setUp()
        self.addCleanup(helper.doCleanups)
        self.c = helper.c
        self.c.__class__ = DiscoveryController
        self.c.plan = DiscoveryTests().profile()
        self.c.plan["wallets"]["count"] = 8
        self.c.j.set("status", "running")
        self.c.j.set("start", 1000)
        self.c.j.set("end", 1000 + 72 * 3600)
        self.c.j.set("active_phase", "correctness")
        self.c.events = []

    def test_phase_boundary_drains_natural_inflight_then_advances(self):
        import asyncio
        from unittest.mock import patch

        for index, shape in enumerate((1, 2, 4)):
            self.c.j.offer(str(index), "campaign", 4590, index, index + 1, 10)
            self.c.j.transition(
                str(index),
                "reconciled" if index < 2 else "accepted",
                info={"input_count": shape},
            )
        with patch("discovery.time.time", return_value=4601):
            asyncio.run(self.c.campaign_tick())
        self.assertEqual(self.c.j.get("active_phase"), "correctness")
        self.c.j.transition("2", "reconciled", info={"input_count": 4})
        with patch("discovery.time.time", return_value=4630):
            asyncio.run(self.c.campaign_tick())
        self.assertEqual(self.c.j.get("active_phase"), "sustained")

    def test_unresolved_or_skipped_previous_phase_cannot_pass(self):
        import asyncio
        from unittest.mock import patch

        for state in ("accepted", "skipped"):
            self.c.j.offer("last", "campaign", 4590, 0, 1, 10)
            self.c.j.transition("last", state)
            with patch("discovery.time.time", return_value=4721):
                with self.assertRaises(Gate):
                    asyncio.run(self.c.campaign_tick())
            self.assertEqual(self.c.j.get("active_phase"), "correctness")

    def test_downtime_materializes_missed_offers_once_without_replay(self):
        events = [
            {
                "intent": str(i),
                "offset_seconds": i,
                "sender": "wallet-1",
                "recipient": "wallet-2",
                "amount_picocredits": "10",
            }
            for i in range(5)
        ]
        self.c.materialize(events, 1000, "campaign", 1200)
        self.c.materialize(events, 1000, "campaign", 1201)
        self.assertEqual(
            len(self.c.j.rows()), 29
        )  # 24 legacy setup fixtures plus five offers
        self.assertTrue(
            all(r["state"] == "skipped" for r in self.c.j.rows("kind='campaign'"))
        )
        self.assertEqual(self.c.j.get("offer_cursor:campaign"), 5)

    def test_opening_accounting_detects_one_picocredit_mutation(self):
        from controller import Controller

        opening = sum(o["utxo"]["amount"] for owned in self.c.inventory for o in owned)
        self.c.j.set("opening_balance", opening)
        Controller.accounting(self.c)
        self.assertEqual(self.c.j.get("accounting")["difference"], 0)
        self.c.inventory[0][0]["utxo"]["amount"] -= 1
        with self.assertRaisesRegex(Gate, "accounting mismatch"):
            Controller.accounting(self.c)

    def test_rehearsal_failure_never_sets_campaign_start(self):
        import asyncio
        from unittest.mock import patch
        from runtime import HOSTS

        self.c.j.set("start", None)
        self.c.j.set("status", "setup")
        self.c.j.set("opening_verified", True)
        self.c.j.set("rehearsal_start", 1000)
        self.c.j.set("setup_start", 1000)
        self.c.plan["rehearsal"] = {
            "duration_seconds": 600,
            "interval_seconds": 60,
            "drain_seconds": 60,
        }
        self.c.j.set("fee_latest", {h: {"at": 1661} for h in HOSTS})
        with patch("discovery.time.time", return_value=1661):
            with self.assertRaisesRegex(Gate, "rehearsal incomplete"):
                asyncio.run(self.c.setup_tick())
        self.assertIsNone(self.c.j.get("start"))
        self.assertEqual(
            self.c.j.get("rehearsal_result")["status"], "generator_limited"
        )

    def test_seed_reproduces_workload_and_changes_amounts_and_wallet_order(self):
        plan = DiscoveryTests().profile()
        _, a = expand(plan)
        _, b = expand(copy.deepcopy(plan))
        self.assertEqual(a, b)
        plan["seed"] += 1
        _, c = expand(plan)
        self.assertNotEqual(
            [(e["sender"], e["amount_picocredits"]) for e in a[:20]],
            [(e["sender"], e["amount_picocredits"]) for e in c[:20]],
        )

    def test_reconciler_waits_for_verified_opening_instead_of_false_mismatch(self):
        from discovery import DiscoveryController

        DiscoveryController.accounting(self.c)
        self.assertIsNone(self.c.j.get("accounting"))
        self.c.j.set("opening_verified", True)
        self.c.j.set("opening_balance", 1)
        with self.assertRaisesRegex(Gate, "accounting mismatch"):
            DiscoveryController.accounting(self.c)

    def test_completed_delivery_does_not_erase_safety_hold_or_missing_gate(self):
        from reporting import workload_complete

        self.c.j.offer("one", "campaign", 1000, 0, 1, 1)
        self.c.j.transition("one", "reconciled")
        events = [{"intent": "one"}]
        phases = [{"id": "correctness"}]
        self.c.j.set("status", "draining")
        self.c.j.set("accounting", {"difference": 0})
        self.c.j.set("phase_gates", {"correctness": True})
        self.assertTrue(workload_complete(self.c.j, events, phases))
        for key, value in [
            ("status", "held"),
            ("phase_gates", {}),
            ("accounting", {"difference": 1}),
        ]:
            original = self.c.j.get(key)
            self.c.j.set(key, value)
            self.assertFalse(workload_complete(self.c.j, events, phases))
            self.c.j.set(key, original)

    def test_fee_rpc_cannot_override_observed_host_time_or_slot(self):
        from fee_evidence import quote_evidence

        quote = {
            "baseRate": 2,
            "baseMin": 1,
            "baseMax": 10,
            "congestion": 0.9,
            "adjustmentActive": True,
            "host": "fake",
            "height": 99,
            "at": 999,
            "slot_seconds": 3,
        }
        result = quote_evidence(
            quote, {"chainHeight": 10, "effectiveSlotDurationSecs": 40}, "real", 100
        )
        self.assertEqual(
            (result["host"], result["height"], result["at"], result["slot_seconds"]),
            ("real", 10, 100, 40),
        )
