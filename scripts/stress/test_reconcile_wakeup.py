"""Confirmed receipts must not wait for another unconfirmed-poll period."""
import asyncio
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from runtime import HOSTS, Journal


class ReconcileWakeupTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        asyncio.get_running_loop().set_debug(False)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.now = 1000.0
        self.c = Controller.__new__(Controller)
        self.c.state = Path(self.temp.name)
        self.c.j = Journal(self.c.state / 'journal.sqlite', lambda: self.now)
        self.addCleanup(self.c.j.db.close)
        self.c.j.set('status', 'running')
        self.c.closed = False
        self.c.reconcile_wakeup = asyncio.Event()
        self.c.fresh = self.now
        self.c.statuses = {h: {'chainHeight': 99, 'synced': True} for h in HOSTS}
        self.c.blocks = []
        self.c.inventory = [[], [{'utxo': {'tx_hash': [170] * 32,
            'output_index': 0, 'amount': 1000, 'target_key': [187] * 32}}]]
        self.info = {'outputs': [{'index': 0, 'amount': 1000, 'target_key': 'bb' * 32}],
            'key_images': ['image'], 'wire_bytes': 123}
        self.c.j.offer('confirmed', 'rehearsal', 990, 0, 1, 1000)
        self.c.j.transition('confirmed', 'accepted', submitted=991, hash='aa' * 32,
            fee=10, info=self.info)
        self.c.j.db.execute("INSERT INTO reservations VALUES (?,?,?)", ('input', 'confirmed', 0))
        self.c.j.offer('pending', 'rehearsal', 990, 2, 1, 1000)
        self.c.j.transition('pending', 'accepted', submitted=991, hash='cc' * 32, info={})
        self.c.sync = AsyncMock()
        self.c.accounting = Mock()
        self.c.report = Mock(side_effect=self.finished)
        self.calls = []
        self.bad_receipt = self.bad_spent = False
        self.c.rpc = Mock(call=AsyncMock(side_effect=self.rpc))

    def finished(self):
        if self.c.j.intent('confirmed')['state'] == 'reconciled' or self.c.j.get('status') == 'held':
            self.c.closed = True

    async def rpc(self, host, method, params):
        self.calls.append((self.now, host, method, params))
        if method == 'getTransactionStatus':
            return {'txHash': params['hash'], 'confirmed': params['hash'] == 'aa' * 32}
        if method == 'getTransaction':
            return {'txHash': 'aa' * 32, 'blockHeight': 100,
                'fee': 11 if self.bad_receipt and host == HOSTS[-1] else 10,
                'outputCount': 1, 'totalOutput': 1000}
        if method == 'chain_areKeyImagesSpent':
            return [{'keyImage': 'image', 'spent': not self.bad_spent}]
        raise AssertionError('unexpected RPC/write: ' + method)

    async def run_reconciler(self, advances):
        async def wake(delay):
            if self.c.closed:
                return
            elapsed, height, fresh = advances.pop(0)
            self.assertLessEqual(elapsed, delay)
            self.now += elapsed
            self.c.statuses = {h: {'chainHeight': height, 'synced': True} for h in HOSTS}
            if fresh:
                self.c.fresh = self.now
            if not advances and height < 100:
                self.c.closed = True
        async def old_sleep(delay):
            self.now += delay
            self.c.statuses = {h: {'chainHeight': 100, 'synced': True} for h in HOSTS}
            self.c.fresh = self.now
        with (patch('controller.time.time', lambda: self.now),
              patch('controller.time.monotonic', lambda: self.now),
              patch('controller.asyncio.sleep', side_effect=old_sleep),
              patch.object(self.c, 'wait_for_reconcile', create=True, side_effect=wake)):
            await self.c.reconcile()

    async def test_monitor_catchup_completes_without_repoll_or_duplicate_write(self):
        await self.run_reconciler([(5, 99, True), (5, 100, True)])
        row = self.c.j.intent('confirmed')
        self.assertEqual(row['finished'], 1010)
        self.assertEqual(json.loads(row['info'])['all_confirmed_at'], 1000)
        self.assertEqual(json.loads(row['info'])['wire_bytes'], 123)
        self.assertEqual(len([c for c in self.calls if c[2] == 'getTransactionStatus']), 10)
        self.assertEqual(len([c for c in self.calls if c[2] == 'getTransaction']), 5)
        self.assertEqual(len([c for c in self.calls if c[2] == 'chain_areKeyImagesSpent']), 5)
        self.assertEqual(self.c.j.db.execute('SELECT COUNT(*) FROM reservations').fetchone()[0], 0)
        self.assertEqual(row['submitted'], 991)

    async def test_monitor_stays_behind_retains_reservations_without_extra_reads(self):
        await self.run_reconciler([(5, 99, True), (5, 99, True)])
        self.assertEqual(self.c.j.intent('confirmed')['state'], 'confirmed')
        self.assertEqual(len(self.calls), 15)
        self.c.sync.assert_not_awaited()
        self.assertEqual(self.c.j.db.execute('SELECT COUNT(*) FROM reservations').fetchone()[0], 1)

    async def test_stale_monitor_does_not_release_saved_confirmation(self):
        self.c.fresh = 900
        self.c.statuses = {h: {'chainHeight': 100, 'synced': True} for h in HOSTS}
        await self.run_reconciler([(5, 99, False)])
        self.assertEqual(self.c.j.intent('confirmed')['state'], 'confirmed')
        self.c.sync.assert_not_awaited()
        self.assertEqual(self.c.j.db.execute('SELECT COUNT(*) FROM reservations').fetchone()[0], 1)

    def save_confirmation(self):
        receipt = {'txHash': 'aa' * 32, 'blockHeight': 100, 'fee': 10,
            'outputCount': 1, 'totalOutput': 1000}
        info = {**self.info, 'all_confirmed_at': 999, 'block_height': 100,
            'confirmed_receipts': {h: dict(receipt) for h in HOSTS}}
        self.c.j.transition('confirmed', 'confirmed', info=info)
        return info

    async def test_restart_completes_saved_receipts_without_refetch(self):
        self.save_confirmation()
        self.c.j.db.close()
        self.c.j = Journal(self.c.state / 'journal.sqlite', lambda: self.now)
        self.addCleanup(self.c.j.db.close)
        self.c.statuses = {}
        self.c.fresh = 0
        await self.run_reconciler([(5, 100, True)])
        self.assertEqual(self.c.j.intent('confirmed')['finished'], 1005)
        self.assertFalse(any(c[2] == 'getTransaction' for c in self.calls))
        self.assertTrue(all(c[3]['hash'] == 'cc' * 32 for c in self.calls
            if c[2] == 'getTransactionStatus'))

    async def test_legacy_confirmed_row_fetches_full_receipts(self):
        self.c.j.transition('confirmed', 'confirmed', info={**self.info,
            'block_height': 100, 'all_confirmed_at': 990})
        await self.run_reconciler([(5, 100, True)])
        self.assertEqual(len([c for c in self.calls if c[2] == 'getTransaction']), 5)
        self.assertEqual(self.c.j.intent('confirmed')['state'], 'reconciled')

    async def test_saved_receipt_disagreement_halts(self):
        info = self.save_confirmation()
        info['confirmed_receipts'][HOSTS[-1]]['fee'] = 11
        self.c.j.transition('confirmed', 'confirmed', info=info)
        await self.run_reconciler([])
        self.assertIn('receipt disagreement', self.c.j.get('reason'))
        self.assertEqual(self.calls, [])

    async def test_height_wakes_do_not_shift_unconfirmed_poll_deadline(self):
        await self.run_reconciler([(5, 99, True), (5, 99, True),
            (10, 99, True), (10, 99, True), (5, 100, True)])
        pending_polls = [c[0] for c in self.calls if c[2] == 'getTransactionStatus'
            and c[3]['hash'] == 'cc' * 32]
        self.assertEqual(pending_polls, [1000] * 5 + [1030] * 5)
        self.assertEqual(len([c for c in self.calls if c[2] == 'getTransaction']), 5)
        self.assertEqual(self.c.j.intent('confirmed')['finished'], 1035)

    async def test_wait_preserves_coalesced_wakeup_until_next_pass(self):
        self.c.reconcile_wakeup.set()
        self.c.reconcile_wakeup.set()
        await asyncio.wait_for(self.c.wait_for_reconcile(30), timeout=.5)
        self.assertTrue(self.c.reconcile_wakeup.is_set())
        self.c.reconcile_wakeup.clear()
        await self.c.wait_for_reconcile(.001)
        self.assertFalse(self.c.reconcile_wakeup.is_set())

    async def test_receipt_disagreement_halts_before_caching(self):
        self.bad_receipt = True
        await self.run_reconciler([])
        self.assertEqual(self.c.j.get('status'), 'held')
        self.assertIn('receipt disagreement', self.c.j.get('reason'))
        self.assertEqual(self.c.j.intent('confirmed')['state'], 'accepted')

    async def test_ownership_mismatch_after_height_wake_halts(self):
        self.c.inventory[1][0]['utxo']['amount'] = 999
        await self.run_reconciler([(5, 100, True)])
        self.assertIn('ownership/value', self.c.j.get('reason'))
        self.assertEqual(self.c.j.db.execute('SELECT COUNT(*) FROM reservations').fetchone()[0], 1)

    async def test_spent_mismatch_after_height_wake_halts(self):
        self.bad_spent = True
        await self.run_reconciler([(5, 100, True)])
        self.assertIn('spent-input', self.c.j.get('reason'))
        self.assertEqual(self.c.j.db.execute('SELECT COUNT(*) FROM reservations').fetchone()[0], 1)

if __name__ == '__main__':
    unittest.main()
