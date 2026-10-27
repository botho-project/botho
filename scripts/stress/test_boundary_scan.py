"""#1629: the phase-boundary scan must not grow with the chain.

x1 (q20261027-x1-endurance2) measured the boundary's ``sync(full=True)`` --
a whole-history refetch, then a native ``restore_check`` over every block for
each of the 64 wallets, one after another, then a full key-image recheck --
at 107 s with about 2,400 chain transactions and 291 s with about 4,800. The
cost is linear in chain content, so from T0+30h the boundary gate would wait
far longer than ``max_queued_intents`` offers and skip them.

These tests run the real admission_tick -> campaign_tick -> Controller.sync
-> accounting path and the real Journal on a virtual clock, over a synthetic
chain sized like the x-series schedule. Only the network and the native
signer are faked; each native wallet scan costs virtual time calibrated to
the x1 measurement, on ``CPUS`` controller cores -- 1, the x1 controller
unit's ``CPUQuota=100%``. ``SerialFullRescan`` restores the pre-#1629
boundary (serial 64-wallet ``restore_check``) to show the failure the fix
removes.
"""

import asyncio
import bisect
import json
from pathlib import Path
import tempfile
from unittest.mock import Mock, patch

from controller import bounded_gather
from discovery import DiscoveryController
from runtime import Gate, HOSTS
from test_phase_boundary_gate import EVENTS, HOUR, CampaignFleet, Crash, Scenario

# x1 calibration: one serial 64-wallet full scan (restore_check of every
# block) took 107 s at ~2,400 chain transactions and 291 s at ~4,800.
CALIBRATION = ((2400, 107.0), (4800, 291.0))
SLOPE = (CALIBRATION[1][1] - CALIBRATION[0][1]) / (CALIBRATION[1][0] - CALIBRATION[0][0])
INTERCEPT = CALIBRATION[0][1] - SLOPE * CALIBRATION[0][0]
WALLETS = 64
SPAWN = 0.02  # one signer subprocess start, per native call
CPUS = 1  # x1 controller unit: CPUQuota=100% (staging.py unit_text)
# Chain shape: prefunding left ~2,400 transactions in the first ~400 blocks;
# x1 then reached height 1245 at T0+8.5h (about one block per 36 s). Every
# campaign offer becomes one chain transaction when it is offered.
PREFUND_TX, PREFUND_HEIGHT, BLOCK_SECONDS = 2400, 400, 36
OFFSETS = [e["offset_seconds"] for e in EVENTS]
# RPC latencies: one chain_getOutputs call per 50 blocks, one key-image call
# per 256 images.
OUTPUTS_CALL, OUTPUTS_PER_TX, IMAGES_CALL = 0.1, 0.0005, 0.1


def full_scan_seconds(tx):
    """The calibrated serial 64-wallet full scan over ``tx`` chain transactions."""
    return SLOPE * tx + INTERCEPT


def native_seconds(blocks):
    """One wallet's native scan: marginal cost per transaction scanned, plus
    the calibration's fixed term for a scan that starts at genesis."""
    if not blocks:
        return SPAWN
    tx = sum(b["txCount"] for b in blocks)
    genesis = INTERCEPT if blocks[0]["height"] == 0 else 0.0
    return SPAWN + max(0.0, SLOPE * tx + genesis) / WALLETS


def chain_tx(at):
    """Chain transactions included by virtual time ``at`` (T0 = 0)."""
    return PREFUND_TX + bisect.bisect_left(OFFSETS, at)


def height_at(at):
    return PREFUND_HEIGHT + int(at // BLOCK_SECONDS)


class SerialFullRescan(DiscoveryController):
    """The pre-#1629 boundary: every wallet restore_checked, one at a time."""

    scan_workers = 1

    async def sync(self, full=False, *, draining=False, rescan_wallets=None):
        return await super().sync(full, draining=draining)


class ChainFleet(CampaignFleet):
    """CampaignFleet with the real Controller.sync over a synthetic chain."""

    def __init__(self, tmp, boundary_hour, *, old=False, accounting=15, **kwargs):
        self.old = old
        self.native_calls = []
        self.lanes = [0.0] * CPUS
        self.crash_on_outputs = False
        self.tamper_scan = None
        self.history_tamper = {}
        self.real_accounting = False
        super().__init__(tmp, boundary_hour, accounting=accounting, **kwargs)
        c = self.c
        if old:
            c.__class__ = SerialFullRescan
        else:
            c.scan_workers = CPUS
        keys = Path(tmp) / "keys"
        keys.mkdir()
        (c.state / "wallets").mkdir()
        for i, wallet in enumerate(c.wallets):
            wallet["key"] = str(keys / f"wallet-{i}.mnemonic")
            Path(wallet["key"]).write_text(f"seed {i}\n")
        self.fresh_process(c)
        # The scan an earlier tick left behind: free, before the window starts.
        begin = self.clock.now
        self.observe()
        asyncio.run(c.sync())
        self.clock.now = begin
        self.lanes = [begin] * CPUS
        self.native_calls.clear()

    # -- chain ---------------------------------------------------------------

    def block(self, k):
        if k < PREFUND_HEIGHT:
            tx = PREFUND_TX // PREFUND_HEIGHT
        else:
            at = (k - PREFUND_HEIGHT) * BLOCK_SECONDS
            tx = chain_tx(at + BLOCK_SECONDS) - chain_tx(at)
        tx_hash = self.history_tamper.get(k, "%064x" % k)
        return {"height": k, "txCount": tx,
                "outputs": [{"txHash": tx_hash, "outputIndex": 0}]}

    @staticmethod
    def owner(k):
        return k % WALLETS

    @staticmethod
    def amount(k):
        return 1000 + k

    def observe(self):
        """The concurrently running monitor: fresh, heights at the clock."""
        height = height_at(self.clock.now)
        self.c.statuses = {h: {"synced": True, "chainHeight": height} for h in HOSTS}
        self.c.fresh = self.clock.now

    def advance(self, seconds):
        self.clock.now += seconds
        self.observe()

    # -- fakes ---------------------------------------------------------------

    def fresh_process(self, c):
        c.inventory = [[] for _ in c.wallets]
        c.inventory_initialized = False
        c.scan_lock = asyncio.Lock()

    def wire(self, c):
        super().wire(c)
        # The real sync and restore; accounting is faked unless asked for.
        for name in ("sync", "restore_wallet"):
            c.__dict__.pop(name, None)
        if self.real_accounting:
            c.__dict__.pop("accounting", None)
        c.native = self.native
        c.rpc = Mock(call=self.rpc)

    def reopen(self, legacy=False):
        c = super().reopen(legacy)
        if self.old:
            c.__class__ = SerialFullRescan
        else:
            c.scan_workers = CPUS
        self.fresh_process(c)
        return c

    @staticmethod
    def wallet_index(key):
        # wallet-<i>.mnemonic, or restore_wallet's restored-<i>.mnemonic copy.
        return int(Path(key).stem.split("-")[1])

    async def native(self, request):
        operation = request["operation"]
        if operation not in ("scan", "restore_check"):
            return await super().native(request)
        index = self.wallet_index(request["wallet"])
        # CPUS signer processes run at once; a call starts on the first free core.
        lane = min(range(CPUS), key=lambda i: self.lanes[i])
        begin = max(self.clock.now, self.lanes[lane])
        self.lanes[lane] = end = begin + native_seconds(request["blocks"])
        self.native_calls.append((operation, index, len(request["blocks"])))
        await asyncio.sleep(0)
        if end > self.clock.now:
            self.advance(end - self.clock.now)
        owned = [
            {"id": b["outputs"][0]["txHash"] + ":0", "key_image": "image-%d" % b["height"],
             "utxo": {"amount": self.amount(b["height"]), "output_index": 0,
                      "tx_hash": list(bytes.fromhex(b["outputs"][0]["txHash"]))}}
            for b in request["blocks"] if self.owner(b["height"]) == index
        ]
        result = {"address": "addr-%d" % index, "owned": owned}
        if self.tamper_scan:
            self.tamper_scan(request, result)
        return result

    async def rpc(self, host, method, params=None, **kwargs):
        if method == "chain_getOutputs":
            if self.crash_on_outputs:
                self.crash_on_outputs = False
                raise Crash()
            blocks = [self.block(k) for k in range(params["start_height"], params["end_height"] + 1)]
            self.advance(OUTPUTS_CALL + OUTPUTS_PER_TX * sum(b["txCount"] for b in blocks))
            return blocks
        if method == "chain_areKeyImagesSpent":
            self.advance(IMAGES_CALL)
            return [{"keyImage": i, "spent": False, "pending": False} for i in params["keyImages"]]
        return await super().rpc(host, method, params, **kwargs)

    async def tick(self):
        self.observe()
        await super().tick()

    def run(self, until):
        self.c.scan_lock = asyncio.Lock()
        super().run(until)

    def gate_event(self):
        rows = [json.loads(d) for (d,) in self.c.j.db.execute(
            "SELECT detail FROM events WHERE state='phase_gate_released'")]
        return rows[-1] if rows else None

    def boundary_restores(self):
        return [c for c in self.native_calls if c[0] == "restore_check"]


class BoundedParallelismTests(Scenario):
    def test_bounded_gather_keeps_order_bounds_concurrency_and_stops_on_failure(self):
        running, peak, started = 0, 0, []

        async def call(i, fail=False):
            nonlocal running, peak
            started.append(i)
            running += 1
            peak = max(peak, running)
            await asyncio.sleep(0)
            running -= 1
            if fail:
                raise Gate("wallet identity differs from allowlist")
            return i

        results = asyncio.run(bounded_gather([lambda i=i: call(i) for i in range(20)], 3))
        self.assertEqual(results, list(range(20)))
        self.assertEqual(peak, 3)
        started.clear()
        with self.assertRaisesRegex(Gate, "wallet identity"):
            asyncio.run(bounded_gather(
                [lambda i=i: call(i, fail=i == 4) for i in range(20)], 3))
        # Calls already running finish; nothing after the failure starts.
        self.assertLess(max(started), 8)


class ChainScenario(Scenario):
    def chain_fleet(self, boundary_hour, **kwargs):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        # blocks.json/inventory.json evidence is not under test here, and
        # serializing a 5,000-block chain on every scan dominates the runtime.
        writes = patch("controller.atomic", lambda path, value: None)
        writes.start()
        self.addCleanup(writes.stop)
        fleet = ChainFleet(tmp.name, boundary_hour, **kwargs)
        self.addCleanup(lambda: fleet.c.j.db.close())
        return fleet

    def cross_chain(self, boundary_hour, after=400, **kwargs):
        fleet = self.chain_fleet(boundary_hour, **kwargs)
        fleet.run(fleet.boundary + after)
        return fleet, fleet.outcome(fleet.phase_at(fleet.boundary))


class XSeriesBoundaryScanTests(ChainScenario):
    """h30 (12/min), h42 (recovery restore) and h48 (D1 opens) at x-series size."""

    BOUNDARIES = (30, 42, 48)

    def test_model_reproduces_the_x1_calibration(self):
        self.assertAlmostEqual(full_scan_seconds(2400), 107.0)
        self.assertAlmostEqual(full_scan_seconds(4800), 291.0)
        blocks = [{"height": 0, "txCount": 4800}]
        self.assertAlmostEqual(native_seconds(blocks) * WALLETS, 291.0 + WALLETS * SPAWN)

    def test_boundary_scan_is_bounded_and_skips_nothing(self):
        waits = {}
        for hour in self.BOUNDARIES:
            with self.subTest(boundary=hour):
                fleet, result = self.cross_chain(hour)
                self.assertEqual(result["status"], "running", result["reason"])
                self.assertEqual(result["active"], fleet.phase_at(fleet.boundary))
                self.assertEqual(result["skipped"], [])
                self.assertEqual(result["expired"], [])
                self.assertEqual(result["submitted"], result["offered"])
                event = fleet.gate_event()
                # Ownership was extended, never restored, at the boundary;
                # only the recovery hand-off restores its one wallet.
                restores = fleet.boundary_restores()
                self.assertEqual([r[1] for r in restores], [0] if hour == 42 else [])
                self.assertLess(event["boundary_scan_seconds"], 60)
                self.assertLess(result["gate_wait"], 200)
                # Far from the old cost at this chain length.
                self.assertGreater(full_scan_seconds(chain_tx(fleet.boundary)),
                                   5 * event["boundary_scan_seconds"])
                waits[hour] = (result["gate_wait"], event["boundary_scan_seconds"])
        # The boundary scan does not grow with the chain the way a full
        # restore does: h48 has twice h30's content, not twice its scan.
        self.assertLess(waits[48][1] - waits[30][1],
                        0.1 * (full_scan_seconds(chain_tx(48 * HOUR))
                               - full_scan_seconds(chain_tx(30 * HOUR))))

    def test_old_serial_full_rescan_skips_offers_at_the_same_boundaries(self):
        for hour in self.BOUNDARIES:
            with self.subTest(boundary=hour):
                fleet = self.chain_fleet(hour, old=True)
                expected = full_scan_seconds(chain_tx(fleet.boundary))
                fleet.run(fleet.boundary + expected + 300)
                result = fleet.outcome(fleet.phase_at(fleet.boundary))
                self.assertEqual(len(fleet.boundary_restores()), WALLETS + (hour == 42))
                self.assertGreater(result["gate_wait"], expected)
                queue_limit = fleet.c.j.rows(
                    "kind='campaign' AND state='skipped' AND offered>=?", (fleet.boundary,))
                self.assertTrue(queue_limit, result)
                self.assertEqual(result["skipped"], [r["id"] for r in queue_limit])


class BoundarySafetyTests(ChainScenario):
    """The incremental boundary keeps every safety check of the full one."""

    def boundary_tick(self, fleet):
        fleet.clock.now = fleet.boundary + 5
        fleet.observe()
        fleet.c.scan_lock = asyncio.Lock()
        fleet.tick_once()

    def exact_fleet(self, hour=30, begin=120):
        fleet = self.chain_fleet(hour, begin=begin)
        fleet.real_accounting = True
        fleet.wire(fleet.c)
        c = fleet.c
        c.j.set("opening_verified", True)
        c.j.set("opening_lottery", 0)
        # Pre-journaled campaign rows carry no fee: opening = every balance.
        top = height_at(fleet.boundary + 5)
        c.j.set("opening_balance", sum(fleet.amount(k) for k in range(top + 1)))
        return fleet

    def test_incremental_boundary_proves_exact_accounting(self):
        fleet = self.exact_fleet()
        before = len(fleet.c.blocks)
        self.boundary_tick(fleet)
        c = fleet.c
        self.assertEqual(c.j.get("active_phase"), "ramp")
        self.assertEqual(c.j.get("accounting")["difference"], 0)
        self.assertGreater(len(c.blocks), before)
        self.assertEqual(len(fleet.native_calls), WALLETS)
        self.assertTrue(all(op == "scan" and n == len(c.blocks) - before
                            for op, _, n in fleet.native_calls))
        # Same inventory and spent state as an independent full restore.
        incremental = (c.inventory, c.spent)
        fleet.native_calls.clear()
        asyncio.run(c.sync(full=True, rescan_wallets=True))
        self.assertEqual(len(fleet.boundary_restores()), WALLETS)
        self.assertEqual((c.inventory, c.spent), incremental)

    def test_boundary_detects_an_accounting_mismatch(self):
        fleet = self.exact_fleet()
        dropped = []

        def drop(request, result):
            if request["operation"] == "scan" and result["owned"] and not dropped:
                dropped.append(result["owned"].pop())

        fleet.tamper_scan = drop
        with self.assertRaisesRegex(Gate, "exact integer accounting mismatch"):
            self.boundary_tick(fleet)
        self.assertTrue(dropped)
        self.assertEqual(fleet.c.j.get("active_phase"), "idle")
        self.assertNotEqual(fleet.c.j.get("accounting")["difference"], 0)

    def test_boundary_detects_a_history_disagreement(self):
        fleet = self.exact_fleet()
        fleet.history_tamper[17] = "ff" * 32  # a checkpointed block rewritten
        with self.assertRaisesRegex(Gate, "full rescan disagrees with checkpointed output history"):
            self.boundary_tick(fleet)
        self.assertEqual(fleet.native_calls, [])
        self.assertEqual(fleet.c.j.get("active_phase"), "idle")

    def test_boundary_detects_duplicate_ids_and_wallet_identity(self):
        fleet = self.exact_fleet()
        top = height_at(fleet.boundary + 5)
        fleet.history_tamper[top] = "%064x" % 3  # a new block reusing an output id
        with self.assertRaisesRegex(Gate, "duplicate output id across incremental history"):
            self.boundary_tick(fleet)
        fleet.history_tamper.clear()

        def impostor(request, result):
            result["address"] = "addr-other"

        fleet.tamper_scan = impostor
        with self.assertRaisesRegex(Gate, "wallet identity differs from allowlist"):
            self.boundary_tick(fleet)
        self.assertEqual(fleet.c.j.get("active_phase"), "idle")


class BoundaryRestartTests(ChainScenario):
    def test_restart_mid_boundary_scan_restores_serially_and_reports_skips(self):
        """A restart at a boundary pays one serial full restore (pre-existing).

        Persisted inventory is never trusted, so the first boundary scan of a
        fresh process falls back to every wallet's full ``restore_check``.
        On the x1 controller unit (1 CPU, ``MemoryMax=1G``) those run one at
        a time, exactly as every boundary did before #1629, so a restart at
        h30 waits about as long as the old boundary and skips offers. That is
        a known pre-existing limitation, not made worse here: the skips are
        bounded by ``max_queued_intents``, journaled as generator limits, and
        the run neither holds nor bursts; the next boundary is incremental.
        """
        fleet = self.chain_fleet(30)
        # Die inside the boundary's history refetch, after the drain.
        original = fleet.c.campaign_tick

        async def arm():
            if fleet.clock.now >= fleet.boundary and not getattr(arm, "armed", False):
                arm.armed = True
                fleet.crash_on_outputs = True
            await original()

        fleet.c.campaign_tick = arm
        with self.assertRaises(Crash):
            fleet.run(fleet.boundary + 600)
        self.assertEqual(fleet.c.j.get("active_phase"), "idle")
        self.assertIsNone((fleet.c.j.get("phase_release") or {}).get("ramp"))
        fleet.clock.now += 20
        fleet.native_calls.clear()
        c = fleet.reopen()
        peak = running = 0
        native = fleet.native

        async def counted(request):
            nonlocal peak, running
            running += 1
            peak = max(peak, running)
            try:
                return await native(request)
            finally:
                running -= 1

        c.native = counted
        expected = full_scan_seconds(chain_tx(fleet.boundary))
        fleet.run(fleet.boundary + expected + 1200)
        result = fleet.outcome("ramp")
        # No unsafe behaviour: a full, serial, identity-checked restore.
        self.assertEqual(result["status"], "running", result["reason"])
        self.assertEqual(result["active"], "ramp")
        self.assertEqual(len(fleet.boundary_restores()), WALLETS)
        self.assertEqual(peak, 1)
        self.assertTrue(c.inventory_initialized)
        event = fleet.gate_event()
        self.assertGreater(event["boundary_scan_seconds"], 0.9 * expected)
        # Skips are bounded and reported as generator limits, never a burst.
        limits = c.j.limits["max_queued_intents"]
        skipped = c.j.rows("kind='campaign' AND state='skipped' AND offered>=?", (fleet.boundary,))
        self.assertTrue(skipped)
        self.assertLessEqual(len(skipped), limits)
        self.assertEqual({r["reason"] for r in skipped} - {"generator_queue_limit", "generator_slot_expired"}, set())
        self.assertEqual(result["skipped"], [r["id"] for r in skipped])
        self.assertEqual(result["expired"], [])
        self.assertEqual(result["submitted"] + len(skipped), result["offered"])

        # The next boundary in the same process is incremental again.
        fleet.native_calls.clear()
        asyncio.run(c.sync(full=True, rescan_wallets=False))
        self.assertEqual(fleet.boundary_restores(), [])


if __name__ == "__main__":
    import unittest

    unittest.main()
