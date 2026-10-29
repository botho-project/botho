"""Legacy rewards cannot pass qualification just because raw amounts balance."""
import copy
from pathlib import Path
import tempfile
import unittest

from controller import Controller
from reporting import accounting_status, legacy_lottery_evidence, lottery_v2_summary
from runtime import Gate, Journal


class LotteryAccountingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.c = Controller.__new__(Controller)
        self.c.j = Journal(Path(self.tmp.name) / "journal.sqlite", lambda: 1000)
        self.c.inventory = [[]]
        self.c.blocks = [{"height": 20, "outputs": []}]
        self.c.spent = []
        self.add(1, 10, "source", lottery=False)
        self.c.j.set("opening_balance", 10)

    def tearDown(self):
        self.c.j.db.close()
        self.tmp.cleanup()

    def add(self, tag, amount, image, lottery=True):
        tx = [tag] * 32
        index = 1 if lottery else 0
        self.c.inventory[0].append({
            "key_image": image,
            "utxo": {"tx_hash": tx, "output_index": index, "amount": amount},
        })
        self.c.blocks[0]["outputs"].append({
            "txHash": bytes(tx).hex(), "outputIndex": index,
            "lottery": lottery,
            "amountCommitment": amount.to_bytes(8, "little").hex(),
        })
        if not any(s["keyImage"] == image for s in self.c.spent):
            self.c.spent.append({"keyImage": image, "spent": False, "pending": False})

    def test_unspent_alias_fails_even_when_arithmetic_balances(self):
        self.add(2, 2, "source")
        before = copy.deepcopy((self.c.blocks, self.c.inventory, self.c.spent))
        with self.assertRaisesRegex(Gate, "legacy lottery"):
            self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 0)
        self.assertEqual(report["legacy_lottery"]["aliased_awards_picocredits"], 2)
        self.assertEqual(report["legacy_lottery"]["spent_image_awards_picocredits"], 0)
        self.assertEqual(before, (self.c.blocks, self.c.inventory, self.c.spent))

    def test_spent_source_award_preserves_exact_difference(self):
        self.c.spent[0].update(spent=True, spentHeight=5)
        self.c.j.set("opening_balance", 0)
        self.add(2, 840_000_000, "source")
        with self.assertRaisesRegex(Gate, "legacy lottery"):
            self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 840_000_000)
        evidence = report["legacy_lottery"]
        self.assertEqual(evidence["spent_image_awards_picocredits"], 840_000_000)
        self.assertEqual(evidence["groups"][0]["spent_height"], 5)
        self.assertEqual(len(evidence["groups"][0]["outpoints"]), 2)

    def test_repeated_and_nested_aliases_retained_in_one_group(self):
        for tag, amount in [(2, 2), (3, 3), (4, 4)]:
            self.add(tag, amount, "source")
        with self.assertRaisesRegex(Gate, "legacy lottery"):
            self.c.accounting()
        evidence = self.c.j.get("accounting")["legacy_lottery"]
        self.assertEqual(evidence["aliased_awards_picocredits"], 9)
        self.assertEqual(evidence["award_count"], 3)
        self.assertEqual(len(evidence["groups"]), 1)
        self.assertEqual(len(evidence["groups"][0]["outpoints"]), 4)

    def test_independently_keyed_lottery_and_ordinary_amount_controls(self):
        self.add(2, 2, "independent")
        self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 0)
        self.assertEqual(report["legacy_lottery"]["award_count"], 0)
        self.c.inventory[0][0]["utxo"]["amount"] -= 1
        with self.assertRaisesRegex(Gate, "exact integer accounting mismatch"):
            self.c.accounting()
        self.assertEqual(self.c.j.get("accounting")["difference"], 1)

    def test_final_report_rejects_balanced_unsupported_alias_evidence(self):
        self.c.j.set("status", "complete")
        self.c.j.offer("payment", "campaign", 500, 0, 1, 10)
        self.c.j.db.execute(
            "UPDATE intents SET state='reconciled',prepared=501,submitted=502,finished=503,fee=0"
        )
        self.c.j.set("accounting", {
            "at": 900, "difference": 0, "signed_fees": 0,
            "legacy_lottery": {"award_count": 1},
        })
        result = accounting_status(self.c.j, self.c.j.rows(), 1000)
        self.assertEqual(result["final_reconciliation"], "unverified")
        self.assertIn("unsupported legacy lottery payouts", result["reasons"])

    def test_evidence_is_order_independent_and_read_only(self):
        self.add(3, 3, "source")
        self.add(2, 2, "source")
        self.add(4, 4, "another", lottery=False)
        self.add(5, 5, "another")
        amounts = {
            (o["txHash"], o["outputIndex"]): int.from_bytes(bytes.fromhex(o["amountCommitment"]), "little")
            for o in self.c.blocks[0]["outputs"] if o["lottery"]
        }
        states = {s["keyImage"]: s for s in self.c.spent}
        before = copy.deepcopy((self.c.inventory, amounts, states))
        first = legacy_lottery_evidence(self.c.inventory, amounts, states)
        second = legacy_lottery_evidence([list(reversed(self.c.inventory[0]))], amounts, states)
        self.assertEqual(first, second)
        self.assertEqual(before, (self.c.inventory, amounts, states))
        self.assertEqual(first["award_count"], 3)
        self.assertEqual(first["aliased_awards_picocredits"], 10)


# A canonical nonzero scalar (3), little-endian, as the signer requires.
TWEAK = "03" + "00" * 31


def v2_row(tx, index, target, amount, winner, base=0, **overrides):
    """A chain_getOutputs V2 payout row in the frozen #1641 contract shape."""
    row = {
        "txHash": bytes(tx).hex(), "outputIndex": index, "targetKey": bytes(target).hex(),
        "lottery": True, "amountCommitment": amount.to_bytes(8, "little").hex(),
        "cryptoOutputIndex": base,
        "lotteryContext": {"version": 2, "baseIndex": base, "tweak": TWEAK,
                           "ordinal": 0, "height": 20},
        "winnerOutpoint": winner,
    }
    row.update(overrides)
    return row


class V2Fixture:
    """One ordinary source output and the V2 award it won, both in wallet-1."""

    SOURCE = bytes([1] * 32).hex() + ":0"
    PAYOUT = bytes([2] * 32).hex() + ":1"

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.c = Controller.__new__(Controller)
        self.c.j = Journal(Path(self.tmp.name) / "journal.sqlite", lambda: 1000)
        self.c.inventory = [[], []]
        self.c.spent = []
        self.c.blocks = [
            {"height": 10, "outputs": [self.ordinary([1] * 32, 0, [1] * 32, 1000)]},
            {"height": 20, "outputs": [v2_row([2] * 32, 1, [2] * 32, 50, self.SOURCE)]},
        ]
        self.own(0, [1] * 32, 0, [1] * 32, 1000, 10, "source")
        self.own(0, [2] * 32, 1, [2] * 32, 50, 20, "payout", lottery=True,
                 context=dict(self.c.blocks[1]["outputs"][0]["lotteryContext"]))
        self.c.j.set("opening_balance", 1000)

    def tearDown(self):
        self.c.j.db.close()
        self.tmp.cleanup()

    @staticmethod
    def ordinary(tx, index, target, amount):
        return {"txHash": bytes(tx).hex(), "outputIndex": index, "targetKey": bytes(target).hex(),
                "lottery": False, "amountCommitment": amount.to_bytes(8, "little").hex()}

    def own(self, wallet, tx, index, target, amount, height, image, lottery=False, context=None):
        # Shaped like the P5 signer's scan/restore ``owned[]`` entry (#1642).
        output = {
            "id": bytes(tx).hex() + ":" + str(index), "key_image": image,
            "utxo": {"tx_hash": list(tx), "output_index": index, "target_key": list(target),
                     "amount": amount, "created_at": height, "lottery": lottery,
                     "lottery_context": context},
        }
        self.c.inventory[wallet].append(output)
        if not any(s["keyImage"] == image for s in self.c.spent):
            self.c.spent.append({"keyImage": image, "spent": False, "pending": False})
        return output

    def state(self, image):
        return next(s for s in self.c.spent if s["keyImage"] == image)



class V2LotteryAccountingTests(V2Fixture, unittest.TestCase):
    """V2 awards are income, spendable, and must all be discovered (#1643)."""

    def test_v2_award_received_and_accounted_as_income(self):
        self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 0)
        self.assertEqual(report["lottery_receipts"], 50)
        self.assertEqual(report["ending"], 1050)
        self.assertEqual(report["legacy_lottery"]["award_count"], 0)
        v2 = report["lottery_v2"]
        self.assertEqual((v2["received_count"], v2["received_picocredits"]), (1, 50))
        self.assertEqual((v2["spent_count"], v2["spent_picocredits"]), (0, 0))
        self.assertEqual(v2["undiscovered"], [])
        self.assertEqual(lottery_v2_summary(report)["unspent_picocredits"], 50)
        # The payout is ordinary canonical inventory, not a held receipt.
        self.assertIn(self.PAYOUT, {o["id"] for o in self.c.canonical_inventory(0)})

    def test_v2_award_spent_as_input_balances_exactly(self):
        # The campaign spends the 50-picocredit award: 40 back to wallet-2
        # (its own change is folded into the recipient here), fee 10.
        self.state("payout").update(spent=True, spentHeight=30)
        self.c.blocks.append({"height": 30, "outputs": [self.ordinary([3] * 32, 0, [3] * 32, 40)]})
        self.own(1, [3] * 32, 0, [3] * 32, 40, 30, "received")
        self.c.j.offer("spend", "campaign", 500, 0, 1, 40)
        self.c.j.db.execute("UPDATE intents SET state='reconciled',prepared=501,submitted=502,"
                            "finished=503,fee=10 WHERE id='spend'")
        self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 0)
        self.assertEqual(report["balances"], [1000, 40])
        v2 = report["lottery_v2"]
        self.assertEqual((v2["received_count"], v2["spent_count"]), (1, 1))
        self.assertEqual((v2["spent_picocredits"], v2["unspent_picocredits"]), (50, 0))

    def assertUndiscovered(self):
        with self.assertRaisesRegex(Gate, "undiscovered lottery award: " + self.PAYOUT):
            self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual([u["payout"] for u in report["lottery_v2"]["undiscovered"]], [self.PAYOUT])
        self.assertEqual(report["lottery_v2"]["undiscovered"][0]["winner"], self.SOURCE)
        self.c.j.set("status", "complete")
        result = accounting_status(self.c.j, self.c.j.rows(), 1000)
        self.assertIn("undiscovered lottery award", result["reasons"])
        self.assertEqual(result["final_reconciliation"], "unverified")

    def test_missing_award_of_a_campaign_winner_is_rejected(self):
        # Silent loss: amounts balance because the signer never found it.
        self.c.inventory[0].pop()
        self.assertUndiscovered()
        self.assertEqual(self.c.j.get("accounting")["difference"], 0)

    def test_award_aliasing_another_key_image_is_rejected(self):
        self.c.inventory[0][1]["key_image"] = "source"
        self.assertUndiscovered()

    def test_award_in_another_wallet_is_rejected(self):
        self.c.inventory[1].append(self.c.inventory[0].pop())
        self.assertUndiscovered()

    def test_non_canonical_award_is_rejected(self):
        self.c.inventory[0][1]["utxo"]["created_at"] = 21
        self.assertUndiscovered()

    def test_payout_of_a_foreign_winner_is_not_required(self):
        self.c.blocks[1]["outputs"][0]["winnerOutpoint"] = "cd" * 32 + ":4"
        self.c.inventory[0].pop()
        self.c.accounting()
        self.assertEqual(self.c.j.get("accounting")["lottery_v2"]["received_count"], 0)

    def test_legacy_alias_still_holds_beside_v2_awards(self):
        # A context-less payout reusing the source key image: the #1450 hold
        # is unchanged, and the V2 award is neither held nor counted as legacy.
        self.c.blocks[1]["outputs"].append({
            "txHash": bytes([4] * 32).hex(), "outputIndex": 1, "targetKey": bytes([1] * 32).hex(),
            "lottery": True, "amountCommitment": (7).to_bytes(8, "little").hex()})
        self.own(0, [4] * 32, 1, [1] * 32, 7, 20, "source", lottery=True)
        with self.assertRaisesRegex(Gate, "legacy lottery"):
            self.c.accounting()
        report = self.c.j.get("accounting")
        self.assertEqual(report["difference"], 0)
        self.assertEqual(report["legacy_lottery"]["award_count"], 1)
        self.assertEqual(report["legacy_lottery"]["aliased_awards_picocredits"], 7)
        self.assertEqual(report["lottery_v2"]["received_count"], 1)
        ids = {o["id"] for o in self.c.canonical_inventory(0)}
        self.assertIn(self.PAYOUT, ids)
        self.assertNotIn(bytes([4] * 32).hex() + ":1", ids)

    def test_malformed_v2_rows_fail_closed(self):
        row = self.c.blocks[1]["outputs"][0]
        for change in ({"cryptoOutputIndex": 1}, {"lottery": False}, {"winnerOutpoint": "x:1"},
                       {"lotteryContext": {**row["lotteryContext"], "version": 1}},
                       {"lotteryContext": {**row["lotteryContext"], "version": True}},
                       {"lotteryContext": {**row["lotteryContext"], "tweak": "0A" + "00" * 31}},
                       # Exactly the frozen keys: the signer is deny_unknown_fields.
                       {"lotteryContext": {**row["lotteryContext"], "extra": 1}},
                       {"lotteryContext": {k: v for k, v in row["lotteryContext"].items()
                                           if k != "ordinal"}},
                       # Signer wire types: baseIndex/ordinal u32, height u64.
                       {"lotteryContext": {**row["lotteryContext"], "baseIndex": 1 << 32},
                        "cryptoOutputIndex": 1 << 32},
                       {"lotteryContext": {**row["lotteryContext"], "ordinal": 1 << 32}},
                       {"lotteryContext": {**row["lotteryContext"], "height": 1 << 64}},
                       {"lotteryContext": {**row["lotteryContext"], "height": -1}}):
            with self.subTest(change=change):
                self.c.blocks[1]["outputs"][0] = {**row, **change}
                with self.assertRaisesRegex(Gate, "malformed V2 lottery row"):
                    self.c.canonical_history()

    def test_tweak_must_be_a_nonzero_canonical_scalar(self):
        from lottery import GROUP_ORDER, canonical_tweak, lottery_context

        le = lambda n: n.to_bytes(32, "little").hex()
        for tweak in (le(1), le(3), le(GROUP_ORDER - 1)):
            self.assertTrue(canonical_tweak(tweak), tweak)
        for tweak in ("00" * 32, le(GROUP_ORDER), le(GROUP_ORDER + 1), "ff" * 32,
                      "ab" * 32, "09", None, 3):
            self.assertFalse(canonical_tweak(tweak), tweak)
        row = self.c.blocks[1]["outputs"][0]
        for tweak in ("00" * 32, le(GROUP_ORDER)):
            with self.subTest(tweak=tweak):
                bad = {**row, "lotteryContext": {**row["lotteryContext"], "tweak": tweak}}
                with self.assertRaisesRegex(Gate, "malformed V2 lottery row"):
                    lottery_context(bad)
        good = {**row, "lotteryContext": {**row["lotteryContext"], "tweak": le(GROUP_ORDER - 1)}}
        self.assertEqual(lottery_context(good)["tweak"], le(GROUP_ORDER - 1))

    def test_signer_lottery_context_cross_checks_the_chain_row(self):
        from controller import compact_owned

        payout = self.c.inventory[0][1]
        ids = lambda: {o["id"] for o in self.c.canonical_inventory(0)}
        # Tolerated and retained verbatim in the controller's inventory.
        self.assertEqual(compact_owned(payout)["utxo"]["lottery_context"],
                         self.c.blocks[1]["outputs"][0]["lotteryContext"])
        self.assertIn(self.PAYOUT, ids())
        context = payout["utxo"]["lottery_context"]
        for reported in (None, {**context, "tweak": "05" + "00" * 31},
                         {**context, "ordinal": 1}, "not-a-context"):
            with self.subTest(reported=reported):
                payout["utxo"]["lottery_context"] = reported
                self.assertNotIn(self.PAYOUT, ids())
        del payout["utxo"]["lottery_context"]  # a pre-P5 signer: legacy to it
        self.assertNotIn(self.PAYOUT, ids())
        # The signer never discovered it as V2: the completeness gate fires.
        self.assertUndiscovered()

    def test_signer_context_on_an_ordinary_row_is_rejected(self):
        source = self.c.inventory[0][0]
        source["utxo"]["lottery_context"] = dict(self.c.blocks[1]["outputs"][0]["lotteryContext"])
        self.assertNotIn(self.SOURCE, {o["id"] for o in self.c.canonical_inventory(0)})

    def test_signer_lottery_flag_never_reclassifies_the_chain_row(self):
        payout = self.c.inventory[0][1]
        payout["utxo"]["lottery"] = False  # a signer that clears the flag for V2
        self.assertIn(self.PAYOUT, {o["id"] for o in self.c.canonical_inventory(0)})
        payout["utxo"]["crypto_output_index"] = 0  # the context's base index
        self.assertIn(self.PAYOUT, {o["id"] for o in self.c.canonical_inventory(0)})
        payout["utxo"]["crypto_output_index"] = 1  # the payout outpoint index: wrong
        self.assertNotIn(self.PAYOUT, {o["id"] for o in self.c.canonical_inventory(0)})
        source = self.c.inventory[0][0]
        source["utxo"]["lottery"] = True  # an ordinary row claimed as lottery
        self.assertNotIn(self.SOURCE, {o["id"] for o in self.c.canonical_inventory(0)})


class DiscoveryOpeningTests(V2Fixture, unittest.TestCase):
    """The launch pin is principal: a V2 award after prefund is opening income."""

    def opening(self, pinned, expected):
        import asyncio
        from unittest.mock import AsyncMock, Mock, patch
        from discovery import DiscoveryController

        c = self.c
        c.__class__ = DiscoveryController
        c.config = {"opening_balances": pinned}
        c.plan = {"setup_deadline_hours": 8, "rehearsal": {}}
        c.j.set("status", "setup")
        c.j.set("setup_start", 900)
        c.sync = AsyncMock(return_value=40)
        # Stop right after the opening check: inventory readiness is not under test.
        c.inventory_ready = Mock(return_value=False)
        with patch("discovery.time.time", return_value=1000):
            with self.assertRaisesRegex(Gate, expected):
                asyncio.run(c.setup_tick())

    def test_award_after_prefund_passes_the_pin_and_is_opening_lottery(self):
        self.c.inventory[1].append({"id": bytes([9] * 32).hex() + ":0", "key_image": "x2", "utxo": {
            "tx_hash": [9] * 32, "output_index": 0, "target_key": [9] * 32,
            "amount": 5, "created_at": 10, "lottery": False}})
        self.c.spent.append({"keyImage": "x2", "spent": False, "pending": False})
        self.c.blocks[0]["outputs"].append(self.ordinary([9] * 32, 0, [9] * 32, 5))
        self.opening([1000, 5], "inventory/decoy headroom")
        self.assertTrue(self.c.j.get("opening_verified"))
        self.assertEqual(self.c.j.get("opening_balance"), 1055)
        self.assertEqual(self.c.j.get("opening_lottery"), 50)
        self.assertEqual(self.c.j.get("accounting")["difference"], 0)

    def test_principal_mismatch_still_refuses(self):
        self.opening([1050], "differ from pinned")  # pinned as if the award were principal
        self.assertIsNone(self.c.j.get("opening_verified"))


if __name__ == "__main__":
    unittest.main()
