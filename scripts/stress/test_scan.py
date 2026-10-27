import asyncio
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller, cgroup_cpu_limit, effective_cpu_count, parse_cpu_max
from runtime import BlockLog, Gate, HOSTS, Journal


class IncrementalScanTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.c = Controller.__new__(Controller)
        self.c.state = Path(self.tmp.name)
        self.c.j = Journal(self.c.state/'journal.sqlite')
        self.c.closed = False
        self.c.wallets = [{'key':str(i),'address':'address-'+str(i)} for i in range(8)]
        self.c.inventory = [[] for _ in self.c.wallets]
        self.c.inventory_initialized = False
        self.c.blocks = []
        self.c.spent = []
        self.c.scan_lock = asyncio.Lock()
        self.chain = [self.block(0),self.block(1)]
        self.statuses = {}
        self.set_height(1)
        self.c.native = AsyncMock(side_effect=self.scan)
        self.c.rpc = Mock(call=AsyncMock(side_effect=self.rpc))

    def tearDown(self):
        self.c.j.db.close()
        self.tmp.cleanup()

    def block(self, height):
        return {'height':height,'outputs':[{'txHash':'tx-'+str(height),'outputIndex':0}]}

    def set_height(self, height):
        self.c.statuses = {h:{'chainHeight':height} for h in HOSTS}

    async def scan(self, request):
        owned = [{'id':o['txHash']+':0','key_image':'image-'+o['txHash'],
                  'utxo':{'amount':100+block['height'],'created_at':block['height']}}
                 for block in request['blocks'] for o in block['outputs']]
        return {'address':'address-'+request['wallet'],
                'owned':owned if request['wallet']=='0' else []}

    async def rpc(self, host, method, params, **kwargs):
        if method=='chain_getOutputs':
            return copy.deepcopy(self.chain[params['start_height']:params['end_height']+1])
        return [{'keyImage':image,'spent':self.statuses.get(image,{}).get('spent',False),
                 'pending':self.statuses.get(image,{}).get('pending',False)}
                for image in params['keyImages']]

    def scan_heights(self):
        return [[b['height'] for b in call.args[0]['blocks']] for call in self.c.native.call_args_list]

    def spent_queries(self):
        return [call.args[2]['keyImages'] for call in self.c.rpc.call.call_args_list
                if call.args[1]=='chain_areKeyImagesSpent']

    async def initialize(self):
        await self.c.sync()
        self.c.native.reset_mock()
        self.c.rpc.call.reset_mock()

    async def test_first_scan_and_cached_restart_rebuild_all_owned_outputs(self):
        await self.initialize()
        expected = copy.deepcopy(self.c.inventory)
        # Simulate process restart: public block cache survives, owned/spent
        # memory does not, and an untrusted saved inventory is ignored.
        self.c.inventory = [[] for _ in self.c.wallets]
        self.c.spent = []
        self.c.inventory_initialized = False
        (self.c.state/'inventory.json').write_text('{"owned":"untrusted"}')
        await self.c.sync()
        self.assertEqual(self.scan_heights(),[[0,1]]*8)
        self.assertEqual(self.c.inventory,expected)
        self.assertEqual(self.spent_queries(),[['image-tx-0','image-tx-1']])

    async def test_incremental_append_preserves_owned_and_finalized_spent_history(self):
        self.statuses['image-tx-0'] = {'spent':True}
        await self.initialize()
        self.chain.append(self.block(2));self.set_height(2)
        await self.c.sync()
        self.assertEqual(self.scan_heights(),[[2]]*8)
        self.assertEqual([o['id'] for o in self.c.inventory[0]],['tx-0:0','tx-1:0','tx-2:0'])
        self.assertEqual(self.spent_queries(),[['image-tx-1','image-tx-2']])
        self.assertTrue(self.c.spent[0]['spent'])
        self.assertEqual(self.c.blocks,self.chain)

    async def test_empty_delta_skips_native_but_refreshes_pending_and_spent(self):
        await self.initialize()
        self.statuses['image-tx-0'] = {'pending':True}
        self.statuses['image-tx-1'] = {'spent':True}
        await self.c.sync()
        self.c.native.assert_not_called()
        self.assertEqual(self.spent_queries(),[['image-tx-0','image-tx-1']])
        self.assertTrue(self.c.spent[0]['pending'])
        self.assertTrue(self.c.spent[1]['spent'])
        self.c.rpc.call.reset_mock()
        self.statuses['image-tx-0'] = {'pending':False}
        await self.c.sync()
        self.assertEqual(self.spent_queries(),[['image-tx-0']])
        self.assertFalse(self.c.spent[0]['pending'])

    async def test_full_restore_rebuilds_independently_and_rechecks_spent_images(self):
        self.statuses['image-tx-0'] = {'spent':True}
        await self.initialize()
        self.c.inventory[0][0]['utxo']['amount'] = -1
        await self.c.sync(full=True)
        self.assertEqual(self.scan_heights(),[[0,1]]*8)
        self.assertTrue(all(c.args[0]['operation']=='restore_check' for c in self.c.native.call_args_list))
        self.assertEqual(self.c.inventory[0][0]['utxo']['amount'],100)
        self.assertEqual(self.spent_queries(),[['image-tx-0','image-tx-1']])

    async def test_failed_scan_or_spent_query_retries_same_delta_without_advancing(self):
        await self.initialize()
        before = copy.deepcopy((self.c.blocks,self.c.inventory,self.c.spent))
        self.chain.append(self.block(2));self.set_height(2)
        async def bad_spent(host,method,params,**kwargs):
            return [] if method=='chain_areKeyImagesSpent' else await self.rpc(host,method,params,**kwargs)
        for failure in ('scan','spent'):
            with self.subTest(failure=failure):
                self.c.native.side_effect = Gate('scan failed') if failure=='scan' else self.scan
                self.c.rpc.call.side_effect = bad_spent if failure=='spent' else self.rpc
                with self.assertRaises(Gate):await self.c.sync()
                self.assertEqual((self.c.blocks,self.c.inventory,self.c.spent),before)
                self.assertEqual(BlockLog(self.c.state).load(),before[0])
        self.c.native.side_effect = self.scan;self.c.native.reset_mock()
        self.c.rpc.call.side_effect = self.rpc
        await self.c.sync()
        self.assertEqual(self.scan_heights(),[[2]]*8)
        self.assertEqual(len(self.c.inventory[0]),3)

    async def test_cross_range_duplicate_output_or_owned_id_is_rejected(self):
        await self.initialize()
        self.chain.append({'height':2,'outputs':copy.deepcopy(self.chain[0]['outputs'])})
        self.set_height(2)
        with self.assertRaisesRegex(Gate,'duplicate output id'):await self.c.sync()
        self.c.native.assert_not_called()
        self.chain[2] = self.block(2)
        async def bad_owned(request):
            result = await self.scan(request)
            if request['wallet']=='0':result['owned'][0]['id']='tx-0:0'
            return result
        self.c.native.side_effect = bad_owned
        with self.assertRaisesRegex(Gate,'duplicate owned output'):await self.c.sync()
        self.assertEqual(len(self.c.blocks),2)

    async def test_boundary_mode_refetches_history_but_scans_only_appended_blocks(self):
        # #1629: full=True, rescan_wallets=False.
        self.statuses['image-tx-0'] = {'spent':True}
        await self.initialize()
        self.chain += [self.block(2),self.block(3)];self.set_height(3)
        await self.c.sync(full=True, rescan_wallets=False)
        self.assertEqual(self.scan_heights(),[[2,3]]*8)
        self.assertTrue(all(c.args[0]['operation']=='scan' and 'expected_address' not in c.args[0]
                            for c in self.c.native.call_args_list))
        outputs = [c.args[2] for c in self.c.rpc.call.call_args_list if c.args[1]=='chain_getOutputs']
        self.assertEqual(outputs,[{'start_height':0,'end_height':3}])
        # Every key image is rechecked, the cached spent one included.
        self.assertEqual(self.spent_queries(),[['image-tx-0','image-tx-1','image-tx-2','image-tx-3']])
        boundary = (copy.deepcopy(self.c.inventory),copy.deepcopy(self.c.spent))
        self.c.native.reset_mock()
        await self.c.sync(full=True)
        self.assertEqual(self.scan_heights(),[[0,1,2,3]]*8)
        self.assertEqual((self.c.inventory,self.c.spent),boundary)

    async def test_boundary_mode_keeps_history_duplicate_and_identity_gates(self):
        await self.initialize()
        before = copy.deepcopy((self.c.blocks,self.c.inventory,self.c.spent))
        self.chain[0]['outputs'][0]['txHash'] = 'rewritten'
        with self.assertRaisesRegex(Gate,'full rescan disagrees'):
            await self.c.sync(full=True, rescan_wallets=False)
        self.chain[0] = self.block(0)
        self.chain.append({'height':2,'outputs':copy.deepcopy(self.chain[1]['outputs'])});self.set_height(2)
        with self.assertRaisesRegex(Gate,'duplicate output id'):
            await self.c.sync(full=True, rescan_wallets=False)
        self.c.native.assert_not_called()
        self.chain[2] = self.block(2)
        async def impostor(request):
            result = await self.scan(request)
            if request['wallet']=='5':result['address']='address-x'
            return result
        self.c.native.side_effect = impostor
        with self.assertRaisesRegex(Gate,'wallet identity differs'):
            await self.c.sync(full=True, rescan_wallets=False)
        self.assertEqual((self.c.blocks,self.c.inventory,self.c.spent),before)

    async def test_boundary_mode_restores_when_no_inventory_was_scanned(self):
        # A restarted process never trusts persisted inventory.
        await self.c.sync(full=True, rescan_wallets=False)
        self.assertEqual(self.scan_heights(),[[0,1]]*8)
        self.assertTrue(all(c.args[0]['operation']=='restore_check'
                            and c.args[0]['expected_address']=='address-'+c.args[0]['wallet']
                            for c in self.c.native.call_args_list))
        with self.assertRaises(ValueError):
            await self.c.sync(rescan_wallets=True)

    async def test_whole_chain_scans_are_serial_and_appends_run_bounded_in_wallet_order(self):
        running = peak = 0
        async def slow(request):
            nonlocal running, peak
            running += 1;peak = max(peak,running)
            await asyncio.sleep(0.01)
            running -= 1
            return await self.scan(request)
        self.c.native.side_effect = slow
        self.c.scan_workers = 3
        # Whole-chain restores stay serial even with an explicit worker count:
        # the x1 controller unit has MemoryMax=1G and no swap (#1629).
        await self.c.sync(full=True)
        self.assertEqual(peak,1)
        # A first routine scan from genesis is whole-chain too.
        self.c.inventory_initialized = False
        peak = 0
        await self.c.sync()
        self.assertEqual(peak,1)
        # Incremental scans of appended blocks may run in parallel.
        self.chain.append(self.block(2));self.set_height(2)
        peak = 0
        await self.c.sync(full=True, rescan_wallets=False)
        self.assertEqual(peak,3)
        self.assertEqual([len(owned) for owned in self.c.inventory],[3,0,0,0,0,0,0,0])

    async def test_incremental_parallelism_defaults_to_the_effective_cpu_limit(self):
        with patch('controller.effective_cpu_count', return_value=1):
            self.assertEqual(self.c.scan_parallelism(True),1)
        with patch('controller.effective_cpu_count', return_value=4):
            self.assertEqual(self.c.scan_parallelism(True),4)
            self.assertEqual(self.c.scan_parallelism(False),1)

    async def test_restore_wallet_runs_its_two_scans_serially(self):
        await self.initialize()
        (self.c.state/'wallets').mkdir()
        key = self.c.state/'key-0'
        key.write_text('seed\n')
        self.c.wallets[0]['key'] = str(key)
        self.c.scan_workers = 8
        running = peak = 0
        async def slow(request):
            nonlocal running, peak
            running += 1;peak = max(peak,running)
            await asyncio.sleep(0.01)
            running -= 1
            return {'address':'address-0','owned':[]}
        self.c.native.side_effect = slow
        await self.c.restore_wallet(0)
        self.assertEqual(peak,1)
        self.assertEqual([c.args[0]['operation'] for c in self.c.native.call_args_list],['scan','restore_check'])


class CpuLimitTests(unittest.TestCase):
    def test_parse_cpu_max(self):
        self.assertIsNone(parse_cpu_max('max 100000\n'))
        self.assertEqual(parse_cpu_max('100000 100000\n'),1)
        self.assertEqual(parse_cpu_max('250000 100000\n'),2)
        self.assertEqual(parse_cpu_max('50000 100000'),1)
        for bad in ('', 'garbage', '0 100000', '100000 0', 'a b'):
            self.assertIsNone(parse_cpu_max(bad), bad)

    def cgroup(self, files, membership='0::/system.slice/x1.service\n'):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)/'cgroup'
        root.mkdir()
        proc = Path(tmp.name)/'self-cgroup'
        if membership is not None:
            proc.write_text(membership)
        for relative, text in files.items():
            path = root/relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        return str(root), str(proc)

    def test_cgroup_cpu_limit_reads_the_unit_and_its_ancestors(self):
        root, proc = self.cgroup({'system.slice/x1.service/cpu.max':'100000 100000\n'})
        self.assertEqual(cgroup_cpu_limit(root, proc),1)
        root, proc = self.cgroup({'system.slice/x1.service/cpu.max':'max 100000\n',
                                  'system.slice/cpu.max':'250000 100000\n'})
        self.assertEqual(cgroup_cpu_limit(root, proc),2)
        root, proc = self.cgroup({'system.slice/x1.service/cpu.max':'max 100000\n'})
        self.assertIsNone(cgroup_cpu_limit(root, proc))

    def test_missing_cgroup_files_fall_back(self):
        root, proc = self.cgroup({})
        self.assertIsNone(cgroup_cpu_limit(root, proc))
        root, proc = self.cgroup({'system.slice/x1.service/cpu.max':'100000 100000\n'}, membership=None)
        self.assertIsNone(cgroup_cpu_limit(root, proc))
        # cgroup v1 only: no unified ("0::") line.
        root, proc = self.cgroup({}, membership='4:cpu,cpuacct:/x1\n')
        self.assertIsNone(cgroup_cpu_limit(root, proc))

    def test_effective_cpu_count_prefers_the_quota_and_falls_back(self):
        with patch('controller.os.sched_getaffinity', create=True, return_value=set(range(8))):
            self.assertEqual(effective_cpu_count(1),1)
            self.assertEqual(effective_cpu_count(2),2)
            self.assertEqual(effective_cpu_count(16),8)
            with patch('controller.cgroup_cpu_limit', return_value=None):
                self.assertEqual(effective_cpu_count(),8)
        with patch('controller.os.sched_getaffinity', create=True, side_effect=AttributeError), \
             patch('controller.os.cpu_count', return_value=6), \
             patch('controller.cgroup_cpu_limit', return_value=None):
            self.assertEqual(effective_cpu_count(),6)


if __name__=='__main__':
    unittest.main()
