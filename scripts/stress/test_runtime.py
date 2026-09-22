import asyncio
import copy
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
from controller import Controller
from plan import expand
from runtime import Gate, HOSTS, Journal, Quota, Rpc, check_identity, check_resources


class JournalTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.path=Path(self.tmp.name)/'journal.sqlite'
        self.now=1000.
        self.j=Journal(self.path,lambda:self.now)

    def tearDown(self):
        self.j.db.close()
        self.tmp.cleanup()

    def offered(self,identifier='a',sender=0):
        self.j.offer(identifier,'campaign',self.now,sender,(sender+1)%8,10_000_000_000)
        self.j.transition(identifier,'eligible')
        return {'hash':identifier.encode().hex().ljust(64,'0'),'fee':100_000_000,'bytes':5000,
                'selected':[{'id':'input-'+identifier}],'input_count':1}

    def test_crash_after_prepare_preserves_inputs_fees_and_original_deadline(self):
        artifact=self.offered()
        self.j.prepare('a',artifact,1,self.now+120)
        self.j.db.close()
        self.j=Journal(self.path,lambda:self.now)
        self.assertEqual(self.j.intent('a')['state'],'prepared')
        self.assertEqual(self.j.db.execute('SELECT COUNT(*) FROM reservations').fetchone()[0],1)
        self.assertEqual(self.j.intent('a')['fee'],100_000_000)
        self.now+=121
        with self.assertRaises(Gate):
            self.j.begin_submit('a',HOSTS[0],10000,False,1120)

    def test_crash_after_submit_marker_never_retries_even_without_reply(self):
        self.j.prepare('a',self.offered(),1,1120)
        self.j.begin_submit('a',HOSTS[0],10000,False,1120)
        self.j.db.close()
        self.j=Journal(self.path,lambda:self.now)
        with self.assertRaises(Gate):
            self.j.begin_submit('a',HOSTS[0],10000,False,1120)
        self.assertEqual(self.j.intent('a')['state'],'submitting')
        self.assertEqual(len(self.j.pending()),1)

    def test_conflicting_input_wallet_and_phase_reservations_are_atomic(self):
        a=self.offered(); self.j.prepare('a',a,8,1120)
        b=self.offered('b',0)
        with self.assertRaises(Gate):self.j.prepare('b',b,8,1120)
        self.j.db.execute('UPDATE intents SET sender=1 WHERE id="b"')
        b['selected']=a['selected']
        with self.assertRaises(sqlite3.IntegrityError):self.j.prepare('b',b,8,1120)
        b['selected']=[{'id':'separate'}]
        with self.assertRaises(Gate):self.j.prepare('b',b,1,1120)
        self.assertEqual(self.j.intent('b')['state'],'eligible')

    def test_actual_fee_and_payload_caps_include_prepared_unknown_intents(self):
        a=self.offered()
        for field,value in [('fee',5_000_000_001),('bytes',262145)]:
            bad={**a,field:value}
            with self.assertRaises(Gate):self.j.prepare('a',bad,1,1120)
        self.j.db.execute('INSERT INTO intents(id,kind,offered,state,recipient,amount,fee,prepared) VALUES(?,?,?,?,?,?,?,?)',
                          ('past','campaign',0,'unknown',1,1,499_999_999_999,1))
        with self.assertRaises(Gate):self.j.prepare('a',a,8,1120)

    def test_submission_rate_and_wire_caps_survive_restart(self):
        for i in range(4):
            identifier=str(i)
            self.j.prepare(identifier,self.offered(identifier,i),8,1120)
            self.j.begin_submit(identifier,HOSTS[i],510000,False,1120)
        self.j.prepare('fifth',self.offered('fifth',4),8,1120)
        with self.assertRaises(Quota):self.j.begin_submit('fifth',HOSTS[4],100000,False,1120)
        with self.assertRaises(Quota):self.j.begin_submit('fifth',HOSTS[4],100000,True,1120)
        self.now+=61
        self.j.begin_submit('fifth',HOSTS[4],100000,False,1120)

    def test_shared_rpc_limit_counts_reads_writes_and_survives_restart(self):
        rpc=Rpc(self.j)
        for i in range(50):rpc.reserve(HOSTS[0],'tx_submit' if i%2 else 'node_getStatus',100)
        with self.assertRaises(Quota):Rpc(self.j).reserve(HOSTS[0],'chain_getOutputs',1)
        with self.assertRaises(Gate):rpc.reserve('localhost','node_getStatus',1)
        self.now+=61
        self.j.set('quota:'+HOSTS[0],4)
        rpc.reserve(HOSTS[0],'read',1); rpc.reserve(HOSTS[0],'read',1)
        with self.assertRaises(Quota):rpc.reserve(HOSTS[0],'read',1)
        self.j.set('backoff:'+HOSTS[1],self.now+120)
        with self.assertRaises(Quota):rpc.reserve(HOSTS[1],'read',1)


class ResourceTests(unittest.TestCase):
    def setUp(self):
        self.base={'at':1000,'pid':1,'start':'fixed','restarts':0,'binary':'hash',
                   'config':'config-digest','node_key':'peer-key-digest',
                   'disk_free':10*1024**3,'disk_total':20*1024**3,
                   'mem_available':1024**3,'mem_total':2*1024**3,'swap':0,'rss':100*1024**2}

    def test_stale_restart_build_disk_memory_and_swap_breakers(self):
        check_resources([self.base],self.base,1000)
        with self.assertRaises(Gate):check_resources([self.base],self.base,1061)
        for key,value in [('pid',2),('start','changed'),('restarts',1),('binary','new'),('disk_free',1024)]:
            with self.subTest(key=key), self.assertRaises(Gate):
                check_resources([{**self.base,key:value}],self.base,1000)
        low={**self.base,'mem_available':100*1024**2}
        check_resources([low],self.base,1000)
        with self.assertRaises(Gate):check_resources([low,low],self.base,1000)
        swap=[{**self.base,'at':880+i*5,'swap':65*1024**2} for i in range(25)]
        with self.assertRaises(Gate):check_resources(swap,self.base,1000)

    def test_network_and_build_identity_are_required(self):
        plan=json.loads(Path(__file__).with_name('testnet-72h-plan.json').read_text())
        status={'network':plan['network'],'gitCommit':plan['node_commit'],'version':'0.6.0'}
        check_identity(status,plan)
        for key in status:
            with self.subTest(key=key),self.assertRaises(Gate):
                check_identity({**status,key:'wrong'},plan)


class CampaignTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.now=1000.
        self.c=object.__new__(Controller)
        self.c.j=Journal(Path(self.tmp.name)/'j.sqlite',lambda:self.now)
        self.c.plan=json.loads(Path(__file__).with_name('testnet-72h-plan.json').read_text())
        _,self.c.events=expand(self.c.plan)
        self.c.j.set('start',1000.)
        self.c.j.set('end',1000.+72*3600)
        self.c.j.set('status','running')
        self.c.j.set('active_phase','correctness')
        self.c.j.set('phase_gates',{'correctness':True})
        self.c.statuses={h:{'chainHeight':500,'synced':True} for h in HOSTS}
        async def sync(full=False):return 500
        async def restore(wallet):self.c.j.event(None,'wallet_restored',wallet)
        self.c.sync=sync
        self.c.restore_wallet=restore
        self.c.accounting=lambda:None
        self.c.report=lambda:None
        self.c.spendable=lambda *args:list(range(8))
        async def prepare(identifier,count,limit,burst=False,crash=None):
            j=self.c.j
            j.transition(identifier,'eligible')
            info={'hash':hashlib_hash(identifier),'fee':100_000_000,'bytes':5000,
                  'selected':[{'id':identifier}],'input_count':count}
            j.prepare(identifier,info,limit,self.now+120)
            j.begin_submit(identifier,HOSTS[0],10000,burst,self.now+120)
            j.finish(identifier,info)
            if crash:j.set('crash:'+crash,identifier)
            return True
        self.c.prepare=prepare

    async def asyncTearDown(self):
        self.c.j.db.close()
        self.tmp.cleanup()

    async def test_fake_clock_full_schedule_visits_all_gates_and_shapes(self):
        with patch('controller.time.time',lambda:self.now):
            for event in self.c.events:
                self.now=1000.+event['offset_seconds']
                await self.c.campaign_tick()
            self.now=1000.+72*3600
            await self.c.campaign_tick()
        rows=self.c.j.rows()
        self.assertEqual(len(rows),692)
        self.assertTrue(all(r['state']=='reconciled' for r in rows))
        self.assertEqual(len(self.c.j.get('phase_gates')),6)
        self.assertEqual(self.c.j.get('end'),260200.)
        self.assertEqual(self.c.j.get('status'),'draining')

    async def test_missed_slots_never_catch_up_and_block_escalation(self):
        self.now=1000.+24*3600
        with patch('controller.time.time',lambda:self.now),self.assertRaises(Gate):
            await self.c.campaign_tick()
        self.assertTrue(any(r['state']=='skipped' for r in self.c.j.rows()))
        self.assertFalse(any(r['submitted'] for r in self.c.j.rows()))
        self.assertEqual(self.c.j.get('end'),260200.)

    async def test_accounting_detects_a_single_picocredit_difference(self):
        self.c.j.offer('grant','funding',self.now,None,0,1000)
        self.c.j.finish('grant',{})
        self.c.blocks=[]
        self.c.inventory=[[{'key_image':'image','utxo':{'amount':999,'tx_hash':[0]*32,'output_index':0}}]]
        self.c.spent=[{'keyImage':'image','spent':False,'pending':False}]
        with self.assertRaises(Gate):Controller.accounting(self.c)
        self.assertEqual(self.c.j.get('accounting')['difference'],1)
        self.c.inventory[0][0]['utxo']['amount']=1000
        Controller.accounting(self.c)
        self.assertEqual(self.c.j.get('accounting')['difference'],0)


def hashlib_hash(value):
    import hashlib
    return hashlib.sha256(value.encode()).hexdigest()


if __name__=='__main__':unittest.main()
