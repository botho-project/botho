"""Legacy rewards cannot pass qualification just because raw amounts balance."""
import copy
from pathlib import Path
import tempfile
import unittest

from controller import Controller
from reporting import accounting_status, legacy_lottery_evidence
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


if __name__ == "__main__":
    unittest.main()
