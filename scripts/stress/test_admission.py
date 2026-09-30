"""Submission capacity belongs to waiting signed work before new signatures."""

import asyncio
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from bounds import LEGACY
from controller import Controller
from runtime import HOSTS, Journal, Quota


class AdmissionTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.now = 1029.0
        self.c = object.__new__(Controller)
        self.c.state = Path(self.tmp.name)
        (self.c.state / "artifacts").mkdir()
        self.limits = {
            **LEGACY,
            "max_normal_submissions_per_minute": 30,
            "max_wire_submission_bytes_per_minute": 150,
            "max_automatic_rebroadcasts": 0,
        }
        self.c.j = Journal(
            self.c.state / "journal.sqlite", lambda: self.now, self.limits
        )
        self.addCleanup(lambda: self.c.j.db.close())
        self.c.j.set("status", "setup")
        self.c.j.set("setup_start", 1000)
        self.c.j.set("last_summary_at", self.now)
        self.c.config = {}
        self.c.plan = {}
        self.c.fresh = self.now
        self.c.statuses = {h: {"synced": True, "chainHeight": 100} for h in HOSTS}
        self.c.admission_lock = asyncio.Lock()
        self.c.closed = False
        self.c.reconcile_wakeup = asyncio.Event()
        self.c.blocks = []
        self.c.inventory = [[], []]
        self.c.monitor = AsyncMock()
        self.c.sync = AsyncMock()
        self.c.report = Mock()
        self.c.accounting = Mock()
        self.c.native = AsyncMock(side_effect=self.inspect)
        self.c.rpc = Mock(call=AsyncMock(side_effect=self.rpc))
        self.submitted = []
        self.observed = []
        self.exists_on = None
        self.submit_error = None
        self.prepared("old", 0, 1000)

    def prepared(self, name, sender, offered):
        artifact = self.c.state / "artifacts" / (name + ".bin")
        artifact.write_bytes(name.encode())
        info = {
            "artifact": str(artifact),
            "hash": hashlib.sha256(name.encode()).hexdigest(),
            "fee": 100,
            "bytes": len(name),
            "outputs": [],
            "key_images": ["image-" + name],
            "selected": [{"id": "input-" + name}],
            "burst": False,
        }
        self.c.j.offer(name, "rehearsal", offered, sender, 1, 1000)
        self.c.j.transition(name, "eligible")
        self.c.j.prepare(name, info, 8, offered + 120)
        return info

    async def inspect(self, request):
        name = Path(request["artifact"]).stem
        return json.loads(self.c.j.intent(name)["info"])

    async def rpc(self, host, method, params=None, **kwargs):
        if method == "getTransactionStatus":
            self.observed.append((host, params["hash"]))
            if params["hash"] == "aa" * 32:
                return {"confirmed": True, "txHash": params["hash"]}
            return {"status": "pending" if host == self.exists_on else "unknown"}
        if method == "tx_submit":
            name = bytes.fromhex(params["tx_hex"]).decode()
            self.submitted.append(name)
            if self.submit_error:
                raise self.submit_error
            return {"txHash": self.c.j.intent(name)["hash"]}
        if method == "getTransaction":
            return {"blockHeight": 99, "fee": 10, "outputCount": 1, "totalOutput": 1000}
        raise AssertionError(method)

    def fill_window(self):
        # Real durable markers, not a mocked quota: all thirty expire at t=1030.
        for i in range(self.c.j.limits["max_normal_submissions_per_minute"]):
            name = "past-" + str(i)
            self.c.j.offer(name, "rehearsal", 950, i, 1, 1000)
            self.c.j.transition(
                name, "reconciled", submitted=970, info={"wire_bytes": 100}
            )

    async def admission_ticks(self, count):
        # Exercise the production run-loop dispatch, without its concurrent
        # monitor/reconciler. Their network effects are tested separately.
        ticks = 0

        async def sleep(_):
            nonlocal ticks
            ticks += 1
            self.now += 1
            self.c.fresh = self.now
            if ticks >= count:
                self.c.j.set("status", "rehearsal_complete")

        with (
            patch("controller.time.time", lambda: self.now),
            patch("controller.asyncio.sleep", side_effect=sleep),
            patch.object(self.c, "reconcile", new=AsyncMock()),
        ):
            await self.c.run()

    async def test_waiting_signature_gets_released_rolling_capacity_before_new_work(
        self,
    ):
        self.fill_window()

        async def new_signature():
            name = "new-" + str(int(self.now))
            self.prepared(name, int(self.now) - 1028, self.now)
            await self.c.submit_prepared(name)

        self.c.setup_tick = AsyncMock(side_effect=new_signature)
        await self.admission_ticks(2)
        self.assertEqual(self.submitted, ["old"])
        self.c.setup_tick.assert_not_awaited()
        self.assertEqual(self.c.j.rows("id LIKE 'new-%'"), [])
        self.assertEqual(self.c.j.intent("old")["submitted"], 1030)
        self.assertEqual({h for h, _ in self.observed}, set(HOSTS))
        self.assertEqual(len(self.c.j.rows("submitted > ?", (self.now - 60,))), 1)

    async def test_oldest_preparation_wins_even_when_inserted_later(self):
        self.prepared("second", 1, 1001)
        self.c.j.db.execute("UPDATE intents SET prepared=1028 WHERE id='second'")
        self.c.setup_tick = AsyncMock()
        await self.admission_ticks(1)
        self.assertEqual(self.submitted, ["second"])
        self.assertIsNone(self.c.j.intent("old")["submitted"])

    async def test_receipts_progress_while_prepared_submission_budget_is_full(self):
        self.fill_window()
        self.c.j.offer("received", "funding", 1000, None, 1, 1000)
        self.c.j.transition("received", "accepted", hash="aa" * 32, submitted=1001)
        self.c.inventory[1] = [
            {"utxo": {"tx_hash": [170] * 32, "output_index": 0, "amount": 1000}}
        ]

        async def end_pass(_):
            self.c.closed = True

        with (
            patch("controller.time.time", lambda: self.now),
            patch.object(self.c, "wait_for_reconcile", side_effect=end_pass),
        ):
            await self.c.reconcile()
        self.assertEqual(self.c.j.intent("received")["state"], "reconciled")
        self.assertEqual(self.c.j.intent("old")["state"], "prepared")
        self.assertEqual(self.submitted, [])
        self.c.native.assert_not_awaited()

    async def test_expired_signature_holds_and_retains_reservations(self):
        self.now = 1120
        self.c.fresh = self.now
        self.c.setup_tick = AsyncMock()
        await self.admission_ticks(1)
        self.assertEqual(self.c.j.intent("old")["state"], "expired")
        self.assertIn("prepared intent expired", self.c.j.get("reason"))
        self.assertEqual(
            self.c.j.db.execute("SELECT input FROM reservations").fetchall()[0][0],
            "input-old",
        )
        self.assertIsNone(self.c.j.intent("old")["submitted"])
        self.c.setup_tick.assert_not_awaited()

    async def test_restart_checks_every_ingress_before_first_submission(self):
        self.c.j.db.close()
        self.c.j = Journal(
            self.c.state / "journal.sqlite", lambda: self.now, self.limits
        )
        self.c.setup_tick = AsyncMock()
        self.exists_on = HOSTS[-1]
        await self.admission_ticks(1)
        self.assertEqual({h for h, _ in self.observed}, set(HOSTS))
        self.assertEqual(self.submitted, [])
        self.assertIn("unexpectedly exists", self.c.j.get("reason"))
        self.assertEqual(self.c.j.intent("old")["state"], "prepared")
        self.c.setup_tick.assert_not_awaited()

    async def test_ambiguous_submission_marker_is_never_replayed_after_restart(self):
        self.submit_error = TimeoutError("response lost")
        self.c.setup_tick = AsyncMock()
        await self.admission_ticks(1)
        self.assertEqual(self.submitted, ["old"])
        self.assertEqual(self.c.j.intent("old")["state"], "unknown")
        submitted = self.c.j.intent("old")["submitted"]
        self.c.j.db.close()
        self.c.j = Journal(
            self.c.state / "journal.sqlite", lambda: self.now, self.limits
        )
        # Even an explicit operator return to setup cannot erase the marker.
        self.c.j.set("status", "setup")
        await self.admission_ticks(1)
        self.assertEqual(self.submitted, ["old"])
        self.assertEqual(self.c.j.intent("old")["submitted"], submitted)
        self.assertEqual(
            self.c.j.db.execute("SELECT COUNT(*) FROM reservations").fetchone()[0], 1
        )

    async def test_recovery_read_quota_blocks_new_work_without_consuming_marker(self):
        self.c.setup_tick = AsyncMock()
        self.c.rpc.call.side_effect = Quota("shared endpoint request budget")
        await self.admission_ticks(1)
        self.assertIsNone(self.c.j.intent("old")["submitted"])
        self.assertEqual(self.c.j.intent("old")["state"], "prepared")
        self.c.setup_tick.assert_not_awaited()
        self.c.native.assert_not_awaited()

    async def test_crash_after_submit_marker_cannot_repeat_first_submission(self):
        self.c.j.begin_submit("old", HOSTS[0], 100, False, 1120)
        self.c.j.db.close()
        self.c.j = Journal(
            self.c.state / "journal.sqlite", lambda: self.now, self.limits
        )
        self.c.setup_tick = AsyncMock()
        await self.admission_ticks(1)
        self.assertEqual(self.c.j.intent("old")["state"], "submitting")
        self.assertEqual(self.submitted, [])
        self.assertEqual(
            self.c.j.db.execute("SELECT COUNT(*) FROM reservations").fetchone()[0], 1
        )

    async def test_accepted_wire_bytes_count_until_rolling_window_expires(self):
        with patch("controller.time.time", lambda: self.now):
            await self.c.submit_prepared("old")
            self.prepared("next", 1, self.now)
            with self.assertRaisesRegex(Quota, "wire byte budget"):
                await self.c.submit_prepared("next")
            first = self.c.j.intent("old")
            self.assertEqual(first["state"], "accepted")
            expected_wire = len(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "id": 1404,
                        "method": "tx_submit",
                        "params": {"tx_hex": b"old".hex()},
                    }
                ).encode()
            )
            self.assertEqual(json.loads(first["info"])["wire_bytes"], expected_wire)
            self.assertIsNone(self.c.j.intent("next")["submitted"])
            self.now += 60
            self.c.fresh = self.now
            await self.c.submit_prepared("next")
            self.assertEqual(self.submitted, ["old", "next"])
