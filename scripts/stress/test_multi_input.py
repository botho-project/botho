"""Whole-transaction decoy eligibility, including setup and admission paths."""

import asyncio
from unittest.mock import patch
import unittest

from controller import Controller
from runtime import Gate
import test_setup


class MultiInputDecoyTests(unittest.TestCase):
    setUp = test_setup.SetupInventoryTests.setUp

    def pool(self, inputs, decoys):
        """Build canonical outputs at explicit heights, without signer mocks."""
        self.c.inventory = [[] for _ in range(8)]
        self.c.spent = []
        blocks = {}
        for tag, (wallet, height) in enumerate(inputs + [(None, h) for h in decoys], 1):
            key = tag.to_bytes(32, "big")
            blocks.setdefault(height, []).append(
                {
                    "targetKey": key.hex(),
                    "txHash": key.hex(),
                    "outputIndex": 0,
                }
            )
            if wallet is not None:
                output = {
                    "id": key.hex() + ":0",
                    "key_image": str(tag),
                    "utxo": {
                        "target_key": list(key),
                        "tx_hash": list(key),
                        "output_index": 0,
                        "created_at": height,
                        "amount": 250_000_000_000,
                    },
                }
                self.c.inventory[wallet].append(output)
                self.c.spent.append(
                    {"keyImage": str(tag), "spent": False, "pending": False}
                )
        self.c.blocks = [
            {"height": h, "outputs": rows} for h, rows in sorted(blocks.items())
        ]
        self.c.sync.return_value = 170

    def test_shared_band_excludes_all_real_inputs_at_exact_threshold(self):
        for count in (1, 2, 4):
            for decoys in (18, 19):
                with self.subTest(count=count, decoys=decoys):
                    self.pool([(0, 100)] * count, [100] * decoys)
                    if count > 1:
                        self.assertEqual(len(self.c.spendable(0, 170)), count)
                    selected = self.c.spendable(0, 170, 10_000_000_000, count)
                    self.assertEqual(len(selected), count if decoys == 19 else 0)

    def test_disjoint_bands_do_not_subtract_unrelated_selected_inputs(self):
        self.pool([(0, 100), (0, 140)], [100] * 19 + [140] * 19)
        self.assertEqual(len(self.c.spendable(0, 170, 1, 2)), 2)

    def test_partially_overlapping_bands_check_each_ring(self):
        # Ages 70 and 60 have bands 63..77 and 54..66. The age-60
        # real input lies outside the first band, but the age-70 input lies
        # outside the second too: their overlapping decoys can be reused.
        self.pool([(0, 100), (0, 110)], [106] * 18 + [100, 110])
        self.assertEqual(len(self.c.spendable(0, 170, 1, 2)), 2)
        self.pool([(0, 100), (0, 110)], [106] * 18 + [100])
        self.assertEqual(self.c.spendable(0, 170, 1, 2), [])

    def test_overlap_checks_each_ring_not_only_the_larger_pool(self):
        # Age 70's upper edge is 77; age 77's lower edge is 70.
        # Both real inputs count as decoys alone but neither may enter a ring.
        self.pool([(0, 100), (0, 93)], [100] * 18 + [86])
        self.assertEqual(len(self.c.spendable(0, 170)), 2)
        self.assertEqual(self.c.spendable(0, 170, 1, 2), [])

    def test_conservative_prefix_waits_even_when_alternate_set_is_feasible(self):
        self.pool([(0, 100)] * 2 + [(0, 140)], [100] * 18 + [140] * 19)
        # The first two equally valuable outputs share a deficient band. A
        # mixed-band pair could pass, but selection deliberately does not search.
        self.assertEqual(self.c.spendable(0, 170, 1, 2), [])
        self.c.inventory[0][2]["utxo"]["amount"] += 1
        self.assertEqual(len(self.c.spendable(0, 170, 1, 2)), 2)

    def test_setup_uses_full_rescan_height_for_selection_and_signer(self):
        self.pool([(w, 100) for w in range(8) for _ in range(4)], [])
        self.c.sync.side_effect = [170, 171]
        asyncio.run(self.c.setup_tick())
        self.assertEqual(self.c.native.await_count, 8)
        self.assertEqual(
            {call.args[0]["height"] for call in self.c.native.await_args_list}, {171}
        )

    def test_full_rescan_age_boundary_waits_without_signing(self):
        # Wallet zero has nineteen decoys on its band's younger edge.
        self.pool(
            [(0, 100)] * 4 + [(w, 140) for w in range(1, 8) for _ in range(4)],
            [107] * 19,
        )
        self.c.sync.side_effect = [170, 169]
        # A lower common scan height is plausible across peers, and changes
        # the age band from 63..77 to 63..75: the age-62 decoys no longer fit.
        asyncio.run(self.c.setup_tick())
        self.c.native.assert_not_awaited()
        self.assertIsNone(self.c.j.get("start"))

    def test_setup_waits_before_any_probe_then_activates_when_pool_grows(self):
        # Wallet zero has 4 real inputs + 18 decoys; other wallets have an
        # independent abundant pool. All eight independently report >=4.
        self.pool(
            [(0, 100)] * 4 + [(w, 140) for w in range(1, 8) for _ in range(4)],
            [100] * 18,
        )
        self.assertTrue(all(len(self.c.spendable(w, 170)) >= 4 for w in range(8)))
        asyncio.run(self.c.setup_tick())
        self.c.native.assert_not_awaited()
        self.c.prepare.assert_not_awaited()
        self.assertIsNone(self.c.j.get("start"))
        self.assertEqual(self.c.j.get("setup_start"), 1000)
        self.pool(
            [(0, 100)] * 4 + [(w, 140) for w in range(1, 8) for _ in range(4)],
            [100] * 19,
        )
        with patch("controller.time.time", return_value=1091):
            asyncio.run(self.c.setup_tick())
        self.assertEqual(self.c.native.await_count, 8)
        self.assertEqual(self.c.j.get("end") - self.c.j.get("start"), 72 * 3600)

    def test_full_rescan_rechecks_all_wallets_before_writing_probe_artifacts(self):
        self.pool([(w, 100) for w in range(8) for _ in range(4)], [])

        async def sync(full=False):
            if full:
                self.pool(
                    [(7, 100)] * 4 + [(w, 140) for w in range(7) for _ in range(4)],
                    [100] * 18,
                )
            return 170

        self.c.sync.side_effect = sync
        asyncio.run(self.c.setup_tick())
        self.c.native.assert_not_awaited()
        self.assertIsNone(self.c.j.get("start"))

    def test_campaign_prepare_waits_without_reserving_or_signing(self):
        self.pool([(0, 100)] * 4, [100] * 18)
        self.c.j.offer("campaign-0", "campaign", 1060, 0, 1, 10_000_000_000)
        self.c.native.side_effect = AssertionError(
            "infeasible selection reached signer"
        )
        self.assertFalse(asyncio.run(Controller.prepare(self.c, "campaign-0", 4, 1)))
        self.c.native.assert_not_awaited()
        self.assertEqual(self.c.j.intent("campaign-0")["state"], "planned")
        self.assertEqual(list(self.c.j.db.execute("SELECT * FROM reservations")), [])

    def test_single_input_bootstrap_keeps_other_owned_outputs_as_decoys(self):
        self.pool([(0, 100)] * 4, [100] * 18)
        self.c.j.offer("bootstrap-0", "bootstrap", 1060, 0, 0, 100_000_000_000)
        self.c.native.side_effect = Gate("stop at signer boundary")
        with self.assertRaisesRegex(Gate, "stop at signer boundary"):
            asyncio.run(Controller.prepare(self.c, "bootstrap-0", 1, 1))
        self.assertEqual(len(self.c.native.await_args.args[0]["selected"]), 1)


if __name__ == "__main__":
    unittest.main()
