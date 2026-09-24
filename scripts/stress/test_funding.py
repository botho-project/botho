import asyncio
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from runtime import Gate, Journal


class FundingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.now = 1800.0
        self.j = Journal(Path(self.tmp.name)/'journal.sqlite', lambda: self.now)
        self.j.set('setup_start', 0)
        self.c = Controller.__new__(Controller)
        self.c.j = self.j
        self.c.gate = Mock()
        self.c.report = Mock()
        self.c.wallets = [{'address': str(i)} for i in range(8)]
        self.c.rpc = Mock()
        self.c.rpc.call = AsyncMock(side_effect=self.rpc)
        self.clock = patch('controller.time.time', lambda: self.now)
        self.clock.start()

    def tearDown(self):
        self.clock.stop()
        self.j.db.close()
        self.tmp.cleanup()

    async def rpc(self, host, method, *args, **kwargs):
        if method == 'faucet_getStatus':
            return {'enabled': True, 'amountPerRequest': 1_000_000_000_000}
        return {'success': True, 'amount': 1_000_000_000_000, 'txHash': 'a'*64}

    def prior(self, identifier, submitted, recipient=0, kind='funding'):
        self.j.offer(identifier, kind, submitted, None, recipient, 1_000_000_000_000)
        self.j.transition(identifier, 'reconciled', submitted=submitted)

    def offered(self):
        self.j.offer('funding-02', 'funding', 1800, None, 2, 1_000_000_000_000)

    def writes(self):
        return [c for c in self.c.rpc.call.call_args_list if c.args[1] == 'faucet_request']

    def test_submission_jitter_waits_then_submits_once_within_original_slot(self):
        self.prior('funding-00', 8)
        self.prior('funding-01', 911, 1)
        self.offered()
        for now in (1800, 1810.9):
            self.now = now
            asyncio.run(self.c.setup_tick())
            self.assertEqual(self.j.intent('funding-02')['state'], 'planned')
            self.assertFalse(self.writes())
        self.now = 1811
        asyncio.run(self.c.setup_tick())
        self.assertEqual(self.j.intent('funding-02')['submitted'], 1811)
        self.assertEqual(self.j.intent('funding-02')['offered'], 1800)
        self.assertEqual(len(self.writes()), 1)
        asyncio.run(self.c.fund('funding-02'))
        self.assertEqual(len(self.writes()), 1)

    def test_expired_deferred_offer_is_skipped_without_replay(self):
        self.prior('funding-00', 8)
        self.prior('funding-01', 1030, 1)
        self.offered()
        asyncio.run(self.c.setup_tick())
        self.now = 1921
        asyncio.run(self.c.setup_tick())
        self.assertEqual(self.j.intent('funding-02')['state'], 'skipped')
        self.now = 1931
        asyncio.run(self.c.fund('funding-02'))
        self.assertFalse(self.writes())

    def test_status_rpc_cannot_extend_original_admission_slot(self):
        self.offered()
        async def delayed(*args, **kwargs):
            self.now = 1921
            return await self.rpc(*args, **kwargs)
        self.c.rpc.call.side_effect = delayed
        asyncio.run(self.c.fund('funding-02'))
        self.assertIsNone(self.j.intent('funding-02')['submitted'])
        self.assertFalse(self.writes())

    def test_funding_count_cap_remains_hard_even_while_pacing(self):
        for i in range(24):
            self.prior('old-'+str(i), 1799)
        self.offered()
        with self.assertRaisesRegex(Gate, 'funding.*cap'):
            asyncio.run(self.c.fund('funding-02'))
        self.assertFalse(self.writes())

    def test_recipient_daily_cap_remains_hard(self):
        for i in range(3):
            self.prior('old-'+str(i), 0, 2)
        self.offered()
        with self.assertRaisesRegex(Gate, 'recipient daily funding cap'):
            asyncio.run(self.c.fund('funding-02'))
        self.assertFalse(self.writes())

    def test_chain_write_cap_remains_hard(self):
        for i in range(800):
            self.prior('old-'+str(i), 0, kind='campaign')
        self.offered()
        with self.assertRaisesRegex(Gate, 'chain write budget'):
            asyncio.run(self.c.fund('funding-02'))
        self.assertFalse(self.writes())
