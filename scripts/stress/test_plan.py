"""Offline checks for expensive or unsafe campaign planning mistakes."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("planner", Path(__file__).with_name("plan.py"))
planner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(planner)


class PlanTests(unittest.TestCase):
    def setUp(self):
        self.plan = json.loads(planner.DEFAULT_PLAN.read_text())

    def rejects(self, change):
        plan = copy.deepcopy(self.plan)
        change(plan)
        with self.assertRaises(ValueError):
            planner.expand(plan)

    def test_expected_budget_and_unique_in_window_intents(self):
        summary, events = planner.expand(self.plan)
        self.assertEqual(summary["nominal_campaign_payments"], 692)
        self.assertEqual(summary["total_planned_chain_write_ceiling"], 780)
        self.assertEqual(summary["min_initial_mature_outputs"], 32)
        self.assertEqual(len({e["intent"] for e in events}), 692)
        self.assertTrue(all(0 <= e["offset_seconds"] < 72 * 3600 for e in events))
        self.assertEqual(events, sorted(events, key=lambda e: e["offset_seconds"]))

    def test_no_live_activation(self):
        self.rejects(lambda p: p.update(status="active"))
        self.rejects(lambda p: p.update(start_utc="2026-09-23T00:00:00Z"))

    def test_budget_and_fee_overrun(self):
        self.rejects(lambda p: p["limits"].update(max_unique_chain_writes=779))
        self.rejects(lambda p: p["limits"].update(max_single_signed_fee_picocredits="600000000000"))
        self.rejects(lambda p: p["funding"].update(max_principal_picocredits="1000000000000"))

    def test_funding_rate_and_recipient_limit(self):
        self.rejects(lambda p: p["funding"].update(max_grants_per_wallet_24h=2))
        self.rejects(lambda p: p["funding"].update(grant_interval_seconds=60))
        self.rejects(lambda p: p.update(setup_deadline_hours=1))

    def test_phase_overlap_and_out_of_window_event(self):
        self.rejects(lambda p: p["phases"][1].update(start_hour=5))
        self.rejects(lambda p: p["phases"][-1]["segments"][-1].update(start_hour=72))

    def test_burst_and_rate_escalation(self):
        self.rejects(lambda p: p["phases"][3]["segments"][2].update(count=9))
        self.rejects(lambda p: p["phases"][3]["segments"][-1].update(interval_seconds=10))

    def test_maturity_and_same_wallet_concurrency(self):
        self.rejects(lambda p: p["wallets"].update(min_input_age_blocks=1))
        self.rejects(lambda p: p["wallets"].update(max_inflight_per_wallet=2))
        self.rejects(lambda p: p["limits"].update(max_inflight=9))

    def test_public_faults_wrong_chain_and_plaintext_ingress(self):
        self.rejects(lambda p: p["faults"].update(public_partitions=True))
        self.rejects(lambda p: p.update(network="botho-mainnet"))
        self.rejects(lambda p: p["endpoints"].__setitem__(0, "http://seed.botho.io/rpc"))


if __name__ == "__main__":
    unittest.main()
