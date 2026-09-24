from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from runtime import HOSTS, Journal


MIB = 1024**2


class IdleMemoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name)/'journal.sqlite'
        self.now = 1000.
        self.j = Journal(self.path, lambda: self.now)
        self.c = Controller.__new__(Controller)
        self.c.j = self.j
        self.c.halt = Mock()
        self.c.statuses = {h: {'synced': True, 'mintingActive': False, 'mempoolSize': 0}
                           for h in HOSTS}
        self.rss = {h: 350*MIB for h in HOSTS}
        for host in HOSTS:
            self.j.set('baseline:'+host, {'at': 900, 'rss': 350*MIB})
        self.j.set('last_write', self.now)

    def tearDown(self):
        self.j.db.close()
        self.tmp.cleanup()

    def sample(self, at, status_age=0, observer_age=0):
        self.now = at
        self.c.fresh = at-status_age
        for host in HOSTS:
            self.j.set('latest:'+host, {'at': at-observer_age, 'rss': self.rss[host]})
        with patch('controller.time.time', return_value=at):
            self.c.memory_cycle()

    def window(self, start):
        for elapsed in range(0, 301, 15):
            self.sample(start+elapsed)

    def test_fixed_warm_baseline_flat_rss_does_not_count_as_growth(self):
        original = self.j.get('baseline:'+HOSTS[0])
        for cycle in range(3):
            start = 1000+cycle*900
            self.j.set('last_write', start)
            self.window(start)
        self.c.halt.assert_not_called()
        self.assertEqual(self.j.get('rss_cycles:'+HOSTS[0]), 0)
        self.assertEqual(self.j.get('baseline:'+HOSTS[0]), original)

    def test_sustained_growth_over_fixed_warm_baseline_still_holds(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        for cycle in range(3):
            start = 1000+cycle*900
            self.j.set('last_write', start)
            self.window(start)
            self.assertEqual(self.j.get('rss_cycles:'+host), cycle+1)
        self.c.halt.assert_called_once_with('post-idle RSS growth across three cycles: '+host)
        self.assertEqual(self.j.get('baseline:'+host)['rss'], 350*MIB)

    def test_active_producer_is_excluded_without_consuming_other_host_cycles(self):
        active, passive = HOSTS[:2]
        self.c.statuses[active]['mintingActive'] = True
        self.rss[active] = 2600*MIB
        self.rss[passive] = 450*MIB
        self.window(1000)
        self.assertIsNone(self.j.get('rss_cycles:'+active))
        self.assertIsNone(self.j.get('memory_cycle:'+active))
        self.assertEqual(self.j.get('rss_cycles:'+passive), 1)
        # Stopping the producer starts its own grace; passive peers cannot be
        # counted twice while waiting for the producer to finish the same write.
        self.c.statuses[active]['mintingActive'] = False
        self.rss[active] = 450*MIB
        self.window(1315)
        self.assertEqual(self.j.get('rss_cycles:'+active), 1)
        self.assertEqual(self.j.get('rss_cycles:'+passive), 1)

    def test_late_mempool_activity_restarts_five_minute_idle_grace(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        for at in range(1000, 1300, 15):
            self.sample(at)
        self.c.statuses[host]['mempoolSize'] = 1
        self.sample(1299)
        self.c.statuses[host]['mempoolSize'] = 0
        for at in range(1300, 1600, 15):
            self.sample(at)
        self.assertIsNone(self.j.get('rss_cycles:'+host))
        self.sample(1600)
        self.assertEqual(self.j.get('rss_cycles:'+host), 1)

    def test_unknown_or_unsynced_status_never_establishes_idle(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        unknowns = ({}, {'synced': True},
                    {'synced': True, 'mintingActive': False, 'mempoolSize': False},
                    {'synced': False, 'mintingActive': False, 'mempoolSize': 0})
        for index, status in enumerate(unknowns):
            with self.subTest(status=status):
                self.c.statuses[host] = status
                self.window(1000+index*900)
                self.assertIsNone(self.j.get('rss_cycles:'+host))
                self.assertIsNone(self.j.get('rss_idle:'+host)['since'])

    def test_pending_unknown_intent_delays_idle_until_reconciled(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        self.j.offer('unknown', 'campaign', 1000, 0, 1, 1)
        self.j.transition('unknown', 'unknown', submitted=1000)
        self.window(1000)
        self.assertIsNone(self.j.get('rss_cycles:'+host))
        self.j.transition('unknown', 'reconciled', finished=1300)
        self.window(1315)
        self.assertEqual(self.j.get('rss_cycles:'+host), 1)

    def test_missing_or_stale_observations_reset_grace(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        for at in range(1000, 1300, 15):
            self.sample(at)
        self.sample(1300, observer_age=61)
        self.assertIsNone(self.j.get('rss_cycles:'+host))
        self.sample(1315, status_age=61)
        self.assertIsNone(self.j.get('rss_idle:'+host)['since'])
        # A monitor outage is not proof of idle, even when the next sample is
        # fresh and the original write is older than five minutes.
        self.sample(1330)
        self.sample(1700)
        self.assertEqual(self.j.get('rss_idle:'+host)['since'], 1700)
        self.assertIsNone(self.j.get('rss_cycles:'+host))
        self.window(1700)
        self.assertEqual(self.j.get('rss_cycles:'+host), 1)

    def test_new_write_restarts_grace_and_restart_does_not_recount(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        for at in range(1000, 1300, 15):
            self.sample(at)
        self.j.set('last_write', 1299)
        self.sample(1300)
        self.assertIsNone(self.j.get('rss_cycles:'+host))
        self.window(1300)
        self.j.db.close()
        self.j = Journal(self.path, lambda: self.now)
        self.c.j = self.j
        self.window(1615)
        self.assertEqual(self.j.get('rss_cycles:'+host), 1)
        self.assertEqual(self.j.get('memory_cycle:'+host), 1299)

    def test_legacy_completed_cycle_is_not_recounted_after_upgrade(self):
        self.j.set('memory_cycle', 1000)
        for host in HOSTS:
            self.rss[host] = 450*MIB
            self.j.set('rss_cycles:'+host, 2)
        self.window(1000)
        self.c.halt.assert_not_called()
        self.assertEqual(self.j.get('rss_cycles:'+HOSTS[0]), 2)

    def test_cycle_count_and_completion_marker_are_atomic(self):
        host = HOSTS[0]
        self.rss[host] = 450*MIB
        for at in range(1000, 1300, 15):
            self.sample(at)
        original_set = self.j.set

        def fail_completion(key, value):
            if key == 'memory_cycle:'+host:
                raise RuntimeError('interrupted completion')
            original_set(key, value)

        with patch.object(self.j, 'set', side_effect=fail_completion):
            with self.assertRaisesRegex(RuntimeError, 'interrupted completion'):
                self.sample(1300)
        self.assertIsNone(self.j.get('rss_cycles:'+host))
        self.sample(1315)
        self.assertEqual(self.j.get('rss_cycles:'+host), 1)

    def test_completed_third_cycle_still_holds_after_interrupted_halt(self):
        host = HOSTS[0]
        self.j.set('rss_cycles:'+host, 3)
        self.j.set('memory_cycle:'+host, 1000)
        self.sample(1300)
        self.c.halt.assert_called_once_with('post-idle RSS growth across three cycles: '+host)


class QualifiedBaselineTests(unittest.IsolatedAsyncioTestCase):
    async def monitor_once(self, changed_pid=False):
        with tempfile.TemporaryDirectory() as name:
            reference = {'at':900,'pid':1,'start':'fixed','restarts':0,'binary':'hash',
                         'config':'config-digest','node_key':'peer-key-digest',
                         'disk_free':10*1024**3,'disk_total':20*1024**3,
                         'mem_available':1024**3,'mem_total':4*1024**3,'swap':0,'rss':350*MIB}
            observed = {**reference,'at':1000,'rss':450*MIB,'pid':2 if changed_pid else 1}
            c = Controller.__new__(Controller)
            c.j = Journal(Path(name)/'journal.sqlite')
            try:
                c.config = {'node_sha256':'hash','resource_baselines':{h:reference for h in HOSTS}}
                c.plan = {'network':'botho-testnet','node_commit':'commit','genesis':'block'}
                c.closed = False
                c.fresh = 0
                c.state = Path(name)
                status = {'network':'botho-testnet','gitCommit':'commit','version':'0.6.0',
                          'synced':True,'chainHeight':100}
                async def rpc(host, method, params=None):
                    return status if method=='node_getStatus' else {'hash':'block'}
                c.rpc = Mock(call=AsyncMock(side_effect=rpc))
                c.observer = AsyncMock(return_value=[observed])
                def finish(*args):c.closed=True
                c.memory_cycle = Mock(side_effect=finish)
                c.halt = Mock(side_effect=finish)
                with patch('controller.time.time',return_value=1000), patch('controller.asyncio.sleep',new_callable=AsyncMock):
                    await c.monitor()
                if changed_pid:
                    c.halt.assert_called_once()
                    self.assertIn('unexpected node restart',str(c.halt.call_args.args[0]))
                    c.memory_cycle.assert_not_called()
                else:
                    c.halt.assert_not_called()
                    for host in HOSTS:
                        self.assertEqual(c.j.get('baseline:'+host),reference)
                        self.assertEqual(c.j.get('latest:'+host)['rss'],450*MIB)
            finally:
                c.j.db.close()

    async def test_supplied_qualified_rss_is_retained_instead_of_latest_sample(self):
        await self.monitor_once()

    async def test_supplied_reference_rejects_changed_pid_instead_of_rebasing(self):
        await self.monitor_once(changed_pid=True)


if __name__ == '__main__':
    unittest.main()
