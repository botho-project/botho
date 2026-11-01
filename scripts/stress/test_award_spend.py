"""The campaign spends mature V2 awards, not just discovers them (#1681)."""

import copy
from pathlib import Path
import sqlite3
import tempfile
import unittest
from types import SimpleNamespace

from controller import Controller
from lottery import (
    AWARD_SPEND_POLICY,
    DEFAULT_MIN_RECEIVED_FOR_SPEND,
    award_spend_expectation,
    spend_expectation_threshold,
)
from reporting import workload_complete
from runtime import Journal
from selection import choose
from test_lottery_accounting import TWEAK, V2Fixture

PRINCIPAL = 10**10
AWARD = 125_000_000


def award_row(tag, height, amount=AWARD):
    return {
        "targetKey": bytes([tag] * 32).hex(), "txHash": bytes([tag] * 32).hex(),
        "outputIndex": 1, "lottery": True, "cryptoOutputIndex": 0,
        "amountCommitment": amount.to_bytes(8, "little").hex(),
        "lotteryContext": {"version": 2, "baseIndex": 0, "tweak": TWEAK,
                           "ordinal": 0, "height": height},
        "winnerOutpoint": "cd" * 32 + ":0",
    }


class AwardSelectionTests(unittest.TestCase):
    """selection.choose with ``award_input``: deterministic and bounded."""

    def setUp(self):
        self.c = Controller.__new__(Controller)
        db = sqlite3.connect(":memory:")
        db.execute("CREATE TABLE reservations (input TEXT)")
        self.addCleanup(db.close)
        self.c.j = SimpleNamespace(
            db=db, limits={"max_single_signed_fee_picocredits": 10**9})
        self.c.inventory = [[]]
        self.c.spent = []
        # Every input is created at height 100: at height 170 the age band is
        # heights 93..107. 30 ordinary decoys leave room to exclude reals.
        self.c.blocks = [{"height": 100, "outputs": [
            {"targetKey": bytes([i] * 32).hex(), "txHash": bytes([i] * 32).hex(),
             "outputIndex": 0} for i in range(100, 130)]}]
        self.principal = [self.own(tag, PRINCIPAL * (5 - tag)) for tag in (1, 2, 3)]

    def own(self, tag, amount, height=100, award=False):
        row = award_row(tag, height, amount) if award else {
            "targetKey": bytes([tag] * 32).hex(), "txHash": bytes([tag] * 32).hex(),
            "outputIndex": 0}
        block = next((b for b in self.c.blocks if b["height"] == height), None)
        if block is None:
            block = {"height": height, "outputs": []}
            self.c.blocks.append(block)
        block["outputs"].append(row)
        output = {
            "id": row["txHash"] + ":" + str(row["outputIndex"]), "key_image": "image-%d" % tag,
            "utxo": {"target_key": [tag] * 32, "amount": amount, "created_at": height,
                     "tx_hash": [tag] * 32, "output_index": row["outputIndex"],
                     "lottery": award,
                     "lottery_context": dict(row["lotteryContext"]) if award else None},
        }
        self.c.inventory[0].append(output)
        self.c.spent.append({"keyImage": output["key_image"], "spent": False, "pending": False})
        return output

    def test_without_the_policy_largest_first_never_reaches_an_award(self):
        self.own(40, AWARD, award=True)
        selected, evidence = choose(self.c, 0, 170, 10**9)
        self.assertEqual(selected, [self.principal[0]])
        self.assertNotIn("award_input", evidence)

    def test_free_shape_payment_adds_the_oldest_mature_award(self):
        newer = self.own(41, 2 * AWARD, height=104, award=True)
        older = self.own(40, AWARD, height=97, award=True)
        selected, evidence = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual(selected, [self.principal[0], older])
        self.assertEqual(evidence["award_input"], older["id"])
        # Deterministic: a replay over the same state picks the same set,
        # whatever the inventory order.
        self.c.inventory[0].reverse()
        again, _ = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual([o["id"] for o in again], [o["id"] for o in selected])
        # Once the older one is spent the next oldest follows.
        next(s for s in self.c.spent if s["keyImage"] == older["key_image"])["spent"] = True
        selected, _ = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual(selected, [self.principal[0], newer])

    def test_immature_award_is_not_added(self):
        self.own(40, AWARD, height=165, award=True)
        selected, evidence = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual(selected, [self.principal[0]])
        self.assertIsNone(evidence["award_input"])

    def test_pinned_shapes_and_probes_are_never_widened(self):
        self.own(40, AWARD, award=True)
        for count in (1, 2, 4):
            with self.subTest(count=count):
                selected, evidence = choose(self.c, 0, 170, 10**9, count, award_input=True)
                self.assertEqual(len(selected), count)
                self.assertNotIn("award_input", evidence)

    def test_bounded_by_max_payment_inputs(self):
        self.own(40, AWARD, award=True)
        # Needs all three principal outputs plus a fourth input of value.
        self.own(4, PRINCIPAL // 2)
        amount = PRINCIPAL * 9 + PRINCIPAL // 4
        selected, evidence = choose(self.c, 0, 170, amount, award_input=True)
        self.assertEqual(len(selected), 4)
        self.assertIsNone(evidence["award_input"])
        self.assertFalse(any(o["utxo"]["lottery"] for o in selected))

    def test_award_never_breaks_complete_set_decoy_pools(self):
        def decoy(tag, height):
            return {"targetKey": bytes([tag] * 32).hex(), "txHash": bytes([tag] * 32).hex(),
                    "outputIndex": 0, "height": height}

        # Principal ring band at height 170: 93..107; the award (created at
        # 104) has band 98..110. Nineteen decoys at 95 serve only principal
        # rings; the award ring holds the three principal keys plus 16 decoys
        # at 109: exactly 19, so selecting principal[0] leaves it 18.
        self.c.blocks[0]["outputs"] = [
            o for o in self.c.blocks[0]["outputs"] if o["outputIndex"] == 0
            and o["targetKey"] in {bytes([t] * 32).hex() for t in (1, 2, 3)}]
        self.c.blocks += [
            {"height": 95, "outputs": [decoy(t, 95) for t in range(100, 119)]},
            {"height": 109, "outputs": [decoy(t, 109) for t in range(150, 166)]},
        ]
        award = self.own(40, AWARD, height=104, award=True)
        selected, evidence = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual(selected, [self.principal[0]])
        self.assertIsNone(evidence["award_input"])
        # One more decoy in the award's band: the complete set holds.
        self.c.blocks[-1]["outputs"].append(decoy(166, 109))
        selected, evidence = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual(selected, [self.principal[0], award])
        self.assertEqual(evidence["award_input"], award["id"])

    def test_award_already_selected_by_value_is_not_doubled(self):
        big = self.own(40, PRINCIPAL * 9, award=True)
        selected, evidence = choose(self.c, 0, 170, 10**9, award_input=True)
        self.assertEqual(selected, [big])
        self.assertIsNone(evidence["award_input"])

    def test_discovery_spendable_opts_in(self):
        from discovery import DiscoveryController

        award = self.own(40, AWARD, award=True)
        recorded = {}
        self.c.__class__ = DiscoveryController
        self.c.j.set = recorded.__setitem__
        self.assertEqual(self.c.spendable(0, 170, 10**9), [self.principal[0], award])
        self.assertEqual(recorded["selection_latest"]["award_input"], award["id"])
        self.assertEqual(len(self.c.spendable(0, 170, 10**9, 1)), 1)


class AwardAccountingTests(V2Fixture, unittest.TestCase):
    """An award spent beside principal keeps ``difference == 0`` (#1681)."""

    def test_award_as_extra_input_balances_exactly(self):
        # source (1000) + award (50) -> 300 to wallet-2, 740 change, fee 10.
        self.state("source").update(spent=True, spentHeight=30)
        self.state("payout").update(spent=True, spentHeight=30)
        self.c.blocks.append({"height": 30, "outputs": [
            self.ordinary([3] * 32, 0, [3] * 32, 300),
            self.ordinary([3] * 32, 1, [4] * 32, 740)]})
        self.own(1, [3] * 32, 0, [3] * 32, 300, 30, "received")
        self.own(0, [3] * 32, 1, [4] * 32, 740, 30, "change")
        self.c.j.offer("spend", "campaign", 500, 0, 1, 300)
        self.c.j.db.execute("UPDATE intents SET state='reconciled',prepared=501,submitted=502,"
                            "finished=503,fee=10 WHERE id='spend'")
        self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 0)
        self.assertEqual(report["balances"], [740, 300])
        v2 = report["lottery_v2"]
        self.assertEqual((v2["received_count"], v2["spent_count"]), (1, 1))
        self.c.j.set("award_spend_policy", {"version": AWARD_SPEND_POLICY, "min_received": 0})
        self.c.j.set("status", "draining")
        self.assertEqual(award_spend_expectation(self.c.j)["status"], "pass")


class AwardSpendExpectationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.j = Journal(Path(self.tmp.name) / "journal.sqlite", lambda: 1000)
        self.addCleanup(self.j.db.close)

    def snapshot(self, received, spent, status="draining", policy=True, threshold=16):
        if policy:
            self.j.set("award_spend_policy",
                       {"version": AWARD_SPEND_POLICY, "min_received": threshold})
        self.j.set("status", status)
        self.j.set("accounting", {"difference": 0, "lottery_v2": {
            "received_count": received, "spent_count": spent, "undiscovered": []}})
        return award_spend_expectation(self.j)

    def test_statuses(self):
        self.assertEqual(self.snapshot(698, 0)["status"], "fail")
        self.assertEqual(self.snapshot(698, 0, status="running")["status"], "pending")
        self.assertEqual(self.snapshot(698, 0, status="incomplete")["status"], "fail")
        self.assertEqual(self.snapshot(698, 3)["status"], "pass")
        self.assertEqual(self.snapshot(16, 0)["status"], "not_applicable")
        self.assertEqual(self.snapshot(17, 0)["status"], "fail")
        self.assertEqual(self.snapshot(3, 0, threshold=2)["status"], "fail")

    def test_older_journal_is_not_judged(self):
        result = self.snapshot(698, 0, policy=False)
        self.assertEqual(result["status"], "not_evaluated")
        self.j.set("award_spend_policy", 1)  # not this policy's record shape
        self.assertEqual(award_spend_expectation(self.j)["status"], "not_evaluated")

    def test_failed_expectation_blocks_workload_complete(self):
        self.snapshot(698, 0)
        self.assertFalse(workload_complete(self.j, [], []))
        self.snapshot(698, 1)
        self.assertTrue(workload_complete(self.j, [], []))
        self.snapshot(698, 0, policy=False)
        self.j.set("award_spend_policy", None)
        self.assertTrue(workload_complete(self.j, [], []))

    def test_threshold_is_plan_pinned_and_validated(self):
        self.assertEqual(spend_expectation_threshold({}), DEFAULT_MIN_RECEIVED_FOR_SPEND)
        self.assertEqual(
            spend_expectation_threshold({"lottery_v2": {"min_received_for_spend": 40}}), 40)
        for bad in ({"lottery_v2": {}}, {"lottery_v2": {"min_received_for_spend": -1}},
                    {"lottery_v2": {"min_received_for_spend": True}},
                    {"lottery_v2": {"min_received_for_spend": 1, "x": 2}},
                    {"lottery_v2": 16}):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                spend_expectation_threshold(bad)

    def test_discovery_plan_rejects_a_malformed_pin(self):
        import test_discovery
        from plan import expand

        plan = test_discovery.DiscoveryTests().profile()
        expand(copy.deepcopy(plan))
        plan["lottery_v2"] = {"min_received_for_spend": "16"}
        with self.assertRaises(ValueError):
            expand(plan)


if __name__ == "__main__":
    unittest.main()
