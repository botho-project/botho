import asyncio
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock

from controller import Controller
from runtime import Gate, HOSTS


class IncrementalScanTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.c = Controller.__new__(Controller)
        self.c.state = Path(self.tmp.name)
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
                self.assertEqual(json.loads((self.c.state/'blocks.json').read_text()),before[0])
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


if __name__=='__main__':
    unittest.main()
