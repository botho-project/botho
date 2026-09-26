"""Setup must wait for existing outputs to mature instead of paying to resplit."""

import asyncio
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from runtime import Gate, Journal


class SetupInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.c = Controller.__new__(Controller)
        self.c.state = Path(self.temp.name)
        self.c.config = {"run_id": "test-setup"}
        self.c.plan = {}
        self.c.j = Journal(self.c.state / "journal.sqlite")
        self.addCleanup(self.c.j.db.close)
        self.c.j.set("setup_start", 1000)
        self.c.wallets = [
            {"address": str(w), "key": "private-" + str(w)} for w in range(8)
        ]
        self.c.inventory = [[] for _ in range(8)]
        self.c.spent = []
        self.c.blocks = [{"height": 0, "outputs": []}, {"height": 20, "outputs": []}]
        for w in range(8):
            for n in range(4):
                tag = 4 * w + n + 1
                owned = {
                    "id": bytes([tag] * 32).hex() + ":0",
                    "key_image": "image-" + str(tag),
                    "utxo": {
                        "target_key": [tag] * 32,
                        "tx_hash": [tag] * 32,
                        "output_index": 0,
                        "created_at": 20,
                        "amount": 250_000_000_000,
                    },
                }
                self.c.inventory[w].append(owned)
                self.c.spent.append(
                    {"keyImage": owned["key_image"], "spent": False, "pending": False}
                )
                self.c.blocks[1]["outputs"].append(
                    {
                        "targetKey": bytes([tag] * 32).hex(),
                        "txHash": bytes([tag] * 32).hex(),
                        "outputIndex": 0,
                    }
                )
        for i in range(24):
            self.c.j.offer(
                "funding-" + str(i).zfill(2), "funding", 0, None, i % 8, 10**12
            )
            self.c.j.transition("funding-" + str(i).zfill(2), "skipped")
        self.c.sync = AsyncMock(return_value=20)
        self.c.accounting = Mock()
        self.c.prepare = AsyncMock()
        self.c.fund = AsyncMock(
            side_effect=AssertionError("historical grants must never replay")
        )
        self.c.gate = Mock()
        self.c.report = Mock()
        self.c.native = AsyncMock()
        self.c.rpc = Mock(call=AsyncMock(return_value={"baseRate": 1}))
        self.clock = patch("controller.time.time", return_value=1060)
        self.clock.start()
        self.addCleanup(self.clock.stop)

    def mature(self, output):
        output["utxo"]["created_at"] = 0
        row = next(
            row
            for row in self.c.blocks[1]["outputs"]
            if row["targetKey"] == bytes(output["utxo"]["target_key"]).hex()
        )
        self.c.blocks[1]["outputs"].remove(row)
        self.c.blocks[0]["outputs"].append(row)

    def decoys(self):
        self.c.blocks[0]["outputs"].extend(
            {
                "targetKey": bytes([tag] * 32).hex(),
                "txHash": bytes([tag] * 32).hex(),
                "outputIndex": 0,
            }
            for tag in range(100, 120)
        )

    def test_all_wallets_wait_on_four_immature_outputs_without_spending(self):
        for w in range(8):
            self.assertEqual(len(self.c.unspent_setup_inputs(w)), 4)
            self.assertEqual(self.c.spendable(w, 20), [])
        asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_not_awaited()
        self.c.fund.assert_not_awaited()
        self.c.native.assert_not_awaited()
        self.assertIsNone(self.c.j.get("start"))
        self.assertEqual(
            self.c.j.get("inventory_counts")["mature_with_decoys"], [0] * 8
        )

    def test_spent_pending_reserved_lottery_and_alias_outputs_do_not_count(self):
        base = copy.deepcopy(self.c.inventory[0][0])
        for reason in (
            "spent",
            "pending",
            "missing",
            "error",
            "reserved",
            "lottery",
            "alias",
        ):
            with self.subTest(reason=reason):
                self.c.inventory[0] = [copy.deepcopy(base)]
                self.c.spent = [
                    {"keyImage": base["key_image"], "spent": False, "pending": False}
                ]
                self.c.j.db.execute("DELETE FROM reservations")
                if reason in ("spent", "pending"):
                    self.c.spent[0][reason] = True
                if reason == "missing":
                    self.c.spent = []
                if reason == "error":
                    self.c.spent[0]["error"] = "unavailable"
                if reason == "reserved":
                    self.c.j.db.execute(
                        "INSERT INTO reservations VALUES(?,?,?)",
                        (base["id"], "pending", 0),
                    )
                if reason == "lottery":
                    self.c.inventory[0][0]["utxo"]["lottery"] = True
                if reason == "alias":
                    self.c.inventory[0][0]["id"] = "later-payout:0"
                self.assertEqual(self.c.unspent_setup_inputs(0), [])

    def test_duplicate_ids_or_key_images_do_not_inflate_inventory(self):
        first = self.c.inventory[0][0]
        duplicate = copy.deepcopy(first)
        self.c.inventory[0].append(duplicate)
        self.c.inventory[0][1]["key_image"] = first["key_image"]
        self.assertEqual(len(self.c.unspent_setup_inputs(0)), 3)

    def test_only_wallet_missing_outputs_is_split(self):
        # Mature decoys plus a single mature source can fund the missing split;
        # the other seven wallets already have four newly-created inputs.
        self.c.inventory[6].pop()
        source = self.c.inventory[6][0]
        self.mature(source)
        self.decoys()
        asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_awaited_once_with("bootstrap-00", 1, 1)
        self.assertEqual(self.c.j.intent("bootstrap-00")["sender"], 6)
        self.c.fund.assert_not_awaited()

    def test_mixed_readiness_waits_instead_of_resplitting_mature_source(self):
        self.mature(self.c.inventory[0][0])
        self.decoys()
        self.assertEqual(len(self.c.spendable(0, 20)), 1)
        asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_not_awaited()
        self.c.fund.assert_not_awaited()
        self.assertEqual(self.c.j.rows("kind='bootstrap'"), [])

    def test_four_mature_outputs_wait_for_decoys(self):
        for output in self.c.inventory[0]:
            self.mature(output)
        self.assertEqual(self.c.spendable(0, 20), [])
        asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_not_awaited()
        self.c.fund.assert_not_awaited()
        self.c.native.assert_not_awaited()
        self.assertIsNone(self.c.j.get("start"))

    def test_malformed_inventory_and_non_boolean_statuses_do_not_count(self):
        base = copy.deepcopy(self.c.inventory[0][0])
        bad_outputs = [None, {}, {"utxo": {}}, {**base, "id": "wrong:0"}]
        for field in ("id", "key_image", "utxo"):
            bad = copy.deepcopy(base)
            del bad[field]
            bad_outputs.append(bad)
        for field in ("target_key", "tx_hash", "output_index", "created_at", "amount"):
            bad = copy.deepcopy(base)
            del bad["utxo"][field]
            bad_outputs.append(bad)
        for field, value in (
            ("target_key", [1] * 31),
            ("target_key", [999] * 32),
            ("target_key", None),
            ("tx_hash", [1] * 31),
            ("output_index", False),
            ("created_at", "20"),
            ("amount", -1),
            ("amount", True),
            ("lottery", "false"),
        ):
            bad = copy.deepcopy(base)
            bad["utxo"][field] = value
            bad_outputs.append(bad)
        for bad in bad_outputs:
            with self.subTest(output=bad):
                self.c.inventory[0] = [bad]
                before = copy.deepcopy(self.c.inventory)
                self.assertEqual(self.c.unspent_setup_inputs(0), [])
                self.assertEqual(self.c.spendable(0, 20), [])
                self.assertEqual(self.c.inventory, before)
        self.c.inventory[0] = [base]
        self.mature(base)
        self.decoys()
        for field in ("spent", "pending"):
            for value in (None, 0, "", "false", [], {}):
                with self.subTest(field=field, value=value):
                    status = {
                        "keyImage": base["key_image"],
                        "spent": False,
                        "pending": False,
                    }
                    status[field] = value
                    self.c.spent = [status]
                    self.assertEqual(self.c.unspent_setup_inputs(0), [])
                    self.assertEqual(self.c.spendable(0, 20), [])
            del status[field]
            self.c.spent = [status]
            self.assertEqual(self.c.unspent_setup_inputs(0), [])

    def test_waiting_preserves_only_the_immutable_funding_schedule(self):
        # Readiness does not add requests, but it must not cancel preauthorized
        # grants either. Make the next original slot due, once, in this fixture.
        self.c.j.db.execute("DELETE FROM intents WHERE id='funding-01'")
        self.c.fund.side_effect = None
        with patch("controller.time.time", return_value=1900):
            asyncio.run(self.c.setup_tick())
        self.c.fund.assert_awaited_once_with("funding-01")
        self.c.prepare.assert_not_awaited()
        self.assertEqual(len(self.c.j.rows("kind='funding'")), 24)
        self.assertEqual(self.c.j.get("setup_start"), 1000)

    def exhaust_bootstrap_budget(self):
        for index in range(64):
            identifier = "bootstrap-" + str(index).zfill(2)
            self.c.j.offer(identifier, "bootstrap", 0, 0, 0, 100_000_000_000)
            self.c.j.transition(identifier, "skipped")

    def test_exhausted_split_budget_still_waits_for_existing_inventory(self):
        self.exhaust_bootstrap_budget()
        self.mature(self.c.inventory[0][0])
        self.decoys()
        asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_not_awaited()
        self.assertEqual(len(self.c.j.rows("kind='bootstrap'")), 64)
        self.assertEqual(self.c.j.get("setup_start"), 1000)

    def test_exhausted_split_budget_never_offers_a_sixty_fifth_split(self):
        self.exhaust_bootstrap_budget()
        self.c.inventory[0].pop()
        self.mature(self.c.inventory[0][0])
        self.decoys()
        with self.assertRaisesRegex(Gate, "sixty-four bootstrap offers"):
            asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_not_awaited()
        self.assertEqual(len(self.c.j.rows("kind='bootstrap'")), 64)

    def test_real_four_input_probes_still_gate_all_wallets(self):
        self.c.blocks[0]["outputs"] = self.c.blocks[1]["outputs"]
        self.c.blocks[1]["outputs"] = []
        for owned in self.c.inventory:
            for output in owned:
                output["utxo"]["created_at"] = 0
        asyncio.run(self.c.setup_tick())
        self.c.sync.assert_any_await(full=True)
        self.assertEqual(self.c.native.await_count, 8)
        self.assertEqual(
            [call.args[0]["wallet"] for call in self.c.native.await_args_list],
            [wallet["key"] for wallet in self.c.wallets],
        )
        for call in self.c.native.await_args_list:
            self.assertEqual(call.args[0]["operation"], "prepare")
            self.assertEqual(len(call.args[0]["selected"]), 4)
        self.assertEqual(self.c.j.get("end") - self.c.j.get("start"), 72 * 3600)
        self.c.prepare.assert_not_awaited()
        self.c.fund.assert_not_awaited()

    def test_failed_four_input_probe_prevents_activation(self):
        self.c.blocks[0]["outputs"] = self.c.blocks[1]["outputs"]
        self.c.blocks[1]["outputs"] = []
        for owned in self.c.inventory:
            for output in owned:
                output["utxo"]["created_at"] = 0
        self.c.native.side_effect = [None] * 7 + [Gate("signer refused")]
        with self.assertRaisesRegex(Gate, "signer refused"):
            asyncio.run(self.c.setup_tick())
        self.assertEqual(self.c.native.await_count, 8)
        self.assertIsNone(self.c.j.get("start"))
        self.assertIsNone(self.c.j.get("end"))
        self.assertFalse((self.c.state / "activated.json").exists())
        self.assertEqual(self.c.j.get("setup_start"), 1000)

    def test_waiting_never_extends_setup_deadline(self):
        with patch("controller.time.time", return_value=1000 + 8 * 3600):
            with self.assertRaisesRegex(Gate, "eight-hour setup deadline"):
                asyncio.run(self.c.setup_tick())
        self.c.prepare.assert_not_awaited()
        self.c.fund.assert_not_awaited()
        self.c.sync.assert_not_awaited()
        self.assertEqual(self.c.j.get("setup_start"), 1000)


if __name__ == "__main__":
    unittest.main()
