"""Offline scan scheduling checks using the durable endpoint reservation budget."""

import asyncio
import io
import json
from pathlib import Path
import tempfile
import unittest
import urllib.error
from unittest.mock import AsyncMock, patch

from controller import Controller
from runtime import Gate, HOSTS, Journal, Quota, Rpc


class LocalRpc(Rpc):
    """Exercise real reservations without sending requests to the network."""

    def __init__(self, journal):
        super().__init__(journal)
        self.calls = []
        self.failure = None

    async def call(self, host, method, params=None, timeout=10, write=False):
        self.reserve(host, method, 1)
        self.calls.append((host, method, timeout, write))
        if self.failure:
            raise self.failure
        if method == "chain_getOutputs":
            return [
                {"height": h, "outputs": []}
                for h in range(params["start_height"], params["end_height"] + 1)
            ]
        if method == "chain_areKeyImagesSpent":
            return [
                {"keyImage": i, "spent": False, "pending": False}
                for i in params["keyImages"]
            ]
        return {}


class ScanQuotaTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        asyncio.get_running_loop().set_debug(False)
        self.tmp = tempfile.TemporaryDirectory()
        self.state = Path(self.tmp.name)
        self.now = 1000.0
        self.elapsed = 0.0
        self.sleeps = []
        self.on_sleep = None
        self.c = object.__new__(Controller)
        self.c.state = self.state
        self.c.j = Journal(self.state / "journal.sqlite", lambda: self.now)
        self.c.rpc = LocalRpc(self.c.j)
        self.c.closed = False
        self.c.scan_lock = asyncio.Lock()
        self.c.blocks = []
        self.c.spent = []
        self.c.wallets = [{"key": "offline", "address": "wallet"}]
        self.c.statuses = {h: {"chainHeight": 0, "synced": True} for h in HOSTS}
        self.c.native = AsyncMock(
            return_value={"address": "wallet", "owned": [{"key_image": "image"}]}
        )
        self.c.j.set("setup_start", 900.0)
        self.c.j.set("start", 950.0)
        self.c.j.set("end", 950.0 + 72 * 3600)
        self.c.j.set("status", "running")
        self.sleep_patch = patch("controller.asyncio.sleep", self.sleep)
        self.clock_patch = patch("controller.time.monotonic", lambda: self.elapsed)
        self.sleep_patch.start()
        self.clock_patch.start()

    async def asyncTearDown(self):
        self.clock_patch.stop()
        self.sleep_patch.stop()
        self.c.j.db.close()
        self.tmp.cleanup()

    async def sleep(self, delay):
        self.assertGreater(delay, 0)
        self.assertLessEqual(delay, 1)
        self.sleeps.append(delay)
        self.elapsed += delay
        self.now += delay
        if self.on_sleep:
            await self.on_sleep()

    def fill(self, count, host=HOSTS[0]):
        for _ in range(count):
            self.c.rpc.reserve(host, "node_getStatus", 1)

    async def test_exhaustion_waits_until_shared_window_replenishes(self):
        self.fill(50)
        self.assertEqual(await self.c.sync(), 0)
        self.assertEqual(sum(self.sleeps), 60)
        self.assertEqual(
            [r[1] for r in self.c.rpc.calls],
            ["chain_getOutputs", "chain_areKeyImagesSpent"],
        )
        self.assertEqual([r[2] for r in self.c.rpc.calls], [20, 10])
        self.assertEqual(self.c.j.get("setup_start"), 900.0)
        self.assertEqual(self.c.j.get("start"), 950.0)
        self.assertEqual(self.c.j.get("end"), 950.0 + 72 * 3600)
        self.assertEqual(self.c.j.rows(), [])

    async def test_long_history_leaves_monitoring_headroom_on_each_endpoint(self):
        for status in self.c.statuses.values():
            status["chainHeight"] = 10049
        monitor_calls = []

        async def monitoring():
            if self.elapsed % 15 == 0:
                for host in HOSTS:
                    for method in ("node_getStatus", "getBlockByHeight"):
                        await self.c.rpc.call(host, method)
                        monitor_calls.append((host, method))

        self.on_sleep = monitoring
        await self.c.sync(full=True)
        self.assertEqual(sum(self.sleeps), 60)
        self.assertEqual(len(monitor_calls), 40)
        self.assertEqual(len(self.c.blocks), 10050)
        # The first wait begins before all fifty shared slots are consumed.
        first_monitor = self.c.j.db.execute(
            "SELECT at FROM requests WHERE method='node_getStatus' ORDER BY id LIMIT 1"
        ).fetchone()[0]
        self.assertEqual(first_monitor, 1015)

    async def test_restart_reuses_request_history_advertised_quota_and_backoff(self):
        self.c.j.set("quota:" + HOSTS[0], 40)
        self.fill(10)
        self.c.j.set("backoff:" + HOSTS[0], 1090.0)
        self.c.j.db.close()
        self.c.j = Journal(self.state / "journal.sqlite", lambda: self.now)
        self.c.rpc = LocalRpc(self.c.j)
        await self.c.sync()
        self.assertEqual(sum(self.sleeps), 90)
        self.assertEqual(self.c.j.get("quota:" + HOSTS[0]), 40)
        self.assertEqual(self.c.j.get("backoff:" + HOSTS[0]), 1090.0)
        self.assertEqual(
            self.c.j.db.execute("SELECT COUNT(*) FROM requests").fetchone()[0], 12
        )

    async def test_lower_advertised_quota_reserves_monitoring_capacity(self):
        self.c.j.set("quota:" + HOSTS[0], 40)
        self.fill(10)
        self.c.j.db.close()
        self.c.j = Journal(self.state / "journal.sqlite", lambda: self.now)
        self.c.rpc = LocalRpc(self.c.j)
        await self.c.sync()
        self.assertEqual(sum(self.sleeps), 60)

    async def test_http_429_read_waits_for_persisted_retry_after(self):
        self.c.rpc = Rpc(self.c.j)
        replies = []

        def respond(request, timeout):
            payload = json.loads(request.data)
            replies.append(payload["method"])
            if len(replies) == 1:
                error = urllib.error.HTTPError(
                    request.full_url,
                    429,
                    "limited",
                    {"Retry-After": "90"},
                    io.BytesIO(),
                )
                self.addCleanup(error.close)
                raise error
            result = (
                [{"height": 0, "outputs": []}]
                if payload["method"] == "chain_getOutputs"
                else [{"keyImage": "image", "spent": False, "pending": False}]
            )
            response = io.BytesIO(
                json.dumps({"jsonrpc": "2.0", "id": 1404, "result": result}).encode()
            )
            response.headers = {"X-RateLimit-Limit": "100"}
            return response

        async def send_locally(send):
            return send()

        with (
            patch.object(self.c.rpc.opener, "open", respond),
            patch("runtime.asyncio.to_thread", send_locally),
        ):
            await self.c.sync()
        self.assertEqual(sum(self.sleeps), 90)
        self.assertEqual(
            replies, ["chain_getOutputs", "chain_getOutputs", "chain_areKeyImagesSpent"]
        )
        self.assertEqual(self.c.j.get("backoff:" + HOSTS[0]), 1090.0)
        self.assertEqual(
            [
                r[0]
                for r in self.c.j.db.execute("SELECT status FROM requests ORDER BY id")
            ],
            ["429", "ok", "ok"],
        )

    async def test_zero_advertised_quota_exhausts_bounded_wait_without_requests(self):
        self.c.j.set("quota:" + HOSTS[0], 0)
        with self.assertRaisesRegex(Gate, "five minutes"):
            await self.c.sync()
        self.assertEqual(sum(self.sleeps), 300)
        self.assertEqual(self.c.rpc.calls, [])

    async def test_wait_budget_is_shared_between_output_and_spent_reads(self):
        self.c.j.set("backoff:" + HOSTS[0], 1180.0)

        async def signer(request):
            self.c.j.set("backoff:" + HOSTS[0], self.now + 180)
            return {"address": "wallet", "owned": [{"key_image": "image"}]}

        self.c.native = signer
        with self.assertRaisesRegex(Gate, "five minutes"):
            await self.c.sync()
        self.assertEqual(sum(self.sleeps), 300)
        self.assertEqual([r[1] for r in self.c.rpc.calls], ["chain_getOutputs"])
        self.assertFalse((self.state / "inventory.json").exists())

    async def test_stop_and_close_interrupt_wait_within_one_second(self):
        for stop in ("STOP", "closed"):
            with self.subTest(stop=stop):
                self.c.j.set("backoff:" + HOSTS[0], self.now + 600)

                async def interrupt():
                    if stop == "STOP":
                        (self.state / "STOP").touch()
                    else:
                        self.c.closed = True

                self.on_sleep = interrupt
                before = self.elapsed
                with self.assertRaisesRegex(Gate, "stopped"):
                    await self.c.sync()
                self.assertEqual(self.elapsed - before, 1)
                (self.state / "STOP").unlink(missing_ok=True)
                self.c.closed = False
        self.assertEqual(self.c.rpc.calls, [])

    async def test_existing_stop_or_close_prevents_first_read(self):
        for stop in ("STOP", "closed"):
            with self.subTest(stop=stop):
                if stop == "STOP":
                    (self.state / "STOP").touch()
                else:
                    self.c.closed = True
                with self.assertRaisesRegex(Gate, "stopped"):
                    await self.c.sync()
                (self.state / "STOP").unlink(missing_ok=True)
                self.c.closed = False
        self.assertEqual(self.sleeps, [])
        self.assertEqual(self.c.rpc.calls, [])

    async def test_nonquota_errors_are_not_retried(self):
        for failure in (Gate("bad response"), OSError("connection lost")):
            with self.subTest(failure=failure):
                self.c.rpc.calls.clear()
                self.c.rpc.failure = failure
                with self.assertRaises(type(failure)) as raised:
                    await self.c.sync()
                self.assertIs(raised.exception, failure)
                self.assertEqual(len(self.c.rpc.calls), 1)
        self.assertEqual(self.sleeps, [])

    async def test_stop_allows_pending_receipt_reconciliation_without_admission(self):
        (self.state / "STOP").touch()
        self.c.j.set("status", "held")
        self.c.j.set("reason", "operator stop marker")
        self.c.j.offer("grant", "funding", self.now, None, 0, 1000)
        tx_hash = "12" * 32
        self.c.j.transition("grant", "accepted", hash=tx_hash, submitted=self.now)
        self.c.native = AsyncMock(
            return_value={
                "address": "wallet",
                "owned": [
                    {
                        "key_image": "image",
                        "utxo": {
                            "tx_hash": list(bytes.fromhex(tx_hash)),
                            "output_index": 0,
                            "amount": 1000,
                        },
                    }
                ],
            }
        )
        self.c.report = lambda: None
        scan_rpc = self.c.rpc.call

        async def receipt_rpc(host, method, params=None, **kwargs):
            result = await scan_rpc(host, method, params, **kwargs)
            if method == "getTransactionStatus":
                return {"confirmed": True, "txHash": tx_hash}
            if method == "getTransaction":
                return {
                    "blockHeight": 0,
                    "fee": 1,
                    "outputCount": 1,
                    "totalOutput": 1000,
                }
            return result

        async def finish_iteration(delay):
            self.assertEqual(delay, 30)
            self.c.closed = True

        self.c.rpc.call = receipt_rpc
        with (
            patch("controller.asyncio.sleep", finish_iteration),
            patch("controller.time.time", lambda: self.now),
        ):
            await self.c.reconcile()
        self.assertEqual(self.c.j.intent("grant")["state"], "reconciled")
        self.assertEqual(self.c.j.get("accounting")["difference"], 0)
        self.assertEqual(self.c.j.get("status"), "held")
        with self.assertRaisesRegex(Gate, "run is held"):
            self.c.gate()
        self.assertFalse(any(call[3] for call in self.c.rpc.calls))

    async def test_stopped_final_scan_can_replenish_but_closed_still_interrupts(self):
        (self.state / "STOP").touch()
        self.c.j.set("status", "held")
        self.fill(50)
        await self.c.sync(full=True, draining=True)
        self.assertEqual(sum(self.sleeps), 60)
        self.c.j.set("backoff:" + HOSTS[0], self.now + 600)

        async def close():
            self.c.closed = True

        self.on_sleep = close
        with self.assertRaisesRegex(Gate, "stopped"):
            await self.c.sync(full=True, draining=True)
        self.assertEqual(sum(self.sleeps), 61)

    async def test_stopped_drain_quota_wait_is_still_bounded(self):
        (self.state / "STOP").touch()
        self.c.j.set("backoff:" + HOSTS[0], self.now + 600)
        with self.assertRaisesRegex(Gate, "five minutes"):
            await self.c.sync(draining=True)
        self.assertEqual(sum(self.sleeps), 300)
        self.assertEqual(self.c.rpc.calls, [])

    async def test_submission_quota_failure_is_not_retried_or_rebroadcast(self):
        artifact = self.state / "synthetic.bin"
        artifact.write_bytes(b"offline test fixture")
        info = {
            "hash": "0" * 64,
            "fee": 1,
            "bytes": 20,
            "outputs": [],
            "key_images": [],
            "artifact": str(artifact),
            "burst": False,
            "selected": [{"id": "input"}],
        }
        self.c.j.offer("intent", "campaign", self.now, 0, 1, 10)
        self.c.j.transition("intent", "eligible")
        self.c.j.prepare("intent", info, 1, self.now + 120)
        self.c.native = AsyncMock(return_value=info)
        self.c.gate = lambda: None
        self.c.report = lambda: None
        self.c.rpc.failure = Quota("HTTP 429; no automatic write retry")
        with patch("controller.time.time", lambda: self.now):
            await self.c.submit_prepared("intent")
            self.assertEqual(self.c.j.intent("intent")["state"], "unknown")
            with self.assertRaisesRegex(Gate, "submission marker already consumed"):
                await self.c.submit_prepared("intent")
        self.assertEqual([r[1] for r in self.c.rpc.calls], ["tx_submit"])
        self.assertTrue(self.c.rpc.calls[0][3])
        self.assertEqual(self.sleeps, [])
        self.assertEqual(
            self.c.j.db.execute("SELECT COUNT(*) FROM reservations").fetchone()[0], 1
        )


if __name__ == "__main__":
    unittest.main()
