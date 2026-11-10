"""#2001: signed drafts are priced against the node's own fee floor."""

import asyncio
from pathlib import Path
import tempfile
import unittest

from fee_quote import MAX_FEE_PASSES, check_quote, sign_priced
from runtime import Gate

MIN_TX_FEE = 100_000_000


def node_quote(fee, floor):
    """What `estimateFee` with `tx_hex` returns for a draft signed at `fee`
    against a relay floor `floor` (`bth_cluster_tax::settle_fee`/`resign_fee`)."""
    headroom = floor // 64
    return {"minimumFee": max(floor, MIN_TX_FEE), "consensusFeeFloor": floor,
            "relayFeeFloor": floor, "settleFee": max(floor + headroom, MIN_TX_FEE),
            "resignFee": max(floor + 2 * headroom, MIN_TX_FEE), "txFee": fee,
            "feeSettles": fee >= max(floor + headroom, MIN_TX_FEE)}


class Signer:
    """The offline signer's fee rule: max(local estimate, min_fee)."""

    def __init__(self, artifact, estimate=MIN_TX_FEE):
        self.artifact, self.estimate, self.calls = artifact, estimate, []

    async def __call__(self, min_fee):
        self.calls.append(min_fee)
        fee = max(self.estimate, min_fee)
        Path(self.artifact).write_bytes(b"draft-%d" % fee)
        return {"hash": "h%d" % len(self.calls), "fee": fee}


class SignPricedTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.artifact = Path(self.tmp.name) / "intent.bin"
        self.discarded = []

    def discard(self):
        self.discarded.append(self.artifact.read_bytes())

    def run_priced(self, signer, floors):
        floors = iter(floors)
        quoted = []

        async def quote(tx_hex):
            fee = int(bytes.fromhex(tx_hex).decode().split("-")[1])
            quoted.append(fee)
            return node_quote(fee, next(floors))

        return asyncio.run(sign_priced(signer, quote, self.discard, self.artifact)), quoted

    def test_background_spend_settles_on_the_first_pass(self):
        signer = Signer(self.artifact)
        info, quoted = self.run_priced(signer, [3_960])
        self.assertEqual(signer.calls, [0])
        self.assertEqual(info["fee"], MIN_TX_FEE)
        self.assertEqual(info["underpaid_drafts"], [])
        self.assertEqual(quoted, [MIN_TX_FEE])
        self.assertEqual(self.discarded, [])

    def test_epoch_zero_exit_charge_is_resigned_at_the_quote(self):
        # stress_node's 10.0.0 case: the local estimate pays MIN_TX_FEE, the
        # node's floor (class-exit charge, #1685) is ~2e9.
        floor = 2_000_000_074
        signer = Signer(self.artifact)
        info, quoted = self.run_priced(signer, [floor, floor])
        resign = floor + 2 * (floor // 64)
        self.assertEqual(signer.calls, [0, resign])
        self.assertEqual(info["fee"], resign)
        self.assertEqual(info["fee_floor_quote"]["settleFee"], floor + floor // 64)
        self.assertEqual([d["fee"] for d in info["underpaid_drafts"]], [MIN_TX_FEE])
        self.assertEqual(self.discarded, [b"draft-%d" % MIN_TX_FEE])

    def test_fresh_ring_jitter_within_one_headroom_still_settles(self):
        floor = 2_000_000_074
        signer = Signer(self.artifact)
        info, _ = self.run_priced(signer, [floor, floor + floor // 128])
        self.assertEqual(len(signer.calls), 2)
        self.assertEqual(len(info["underpaid_drafts"]), 1)

    def test_a_floor_that_keeps_rising_gates_after_the_pass_limit(self):
        signer = Signer(self.artifact)
        floors = [10**9 * 2 ** i for i in range(MAX_FEE_PASSES)]
        with self.assertRaisesRegex(Gate, "did not settle"):
            self.run_priced(signer, floors)
        self.assertEqual(len(signer.calls), MAX_FEE_PASSES)
        self.assertEqual(len(self.discarded), MAX_FEE_PASSES)


class CheckQuoteTests(unittest.TestCase):
    def test_accepts_a_node_quote(self):
        self.assertEqual(check_quote(node_quote(5, 3_960), 5)["settleFee"], MIN_TX_FEE)

    def test_refuses_malformed_or_foreign_quotes(self):
        good = node_quote(7, 2 * 10**9)
        for broken, message in [
            ({**good, "settleFee": None}, "settleFee"),
            ({**good, "resignFee": 0}, "resignFee"),
            ({**good, "txFee": 8}, "different draft"),
            ({**good, "relayFeeFloor": good["consensusFeeFloor"] - 1}, "inconsistent"),
            ({**good, "resignFee": good["settleFee"] - 1}, "inconsistent"),
            ([], "invalid fee quote"),
        ]:
            with self.assertRaisesRegex(Gate, message):
                check_quote(broken, 7)


if __name__ == "__main__":
    unittest.main()
