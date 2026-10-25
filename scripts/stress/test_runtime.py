import asyncio
import copy
import json
from pathlib import Path
import sqlite3
import shutil
import tempfile
import unittest
from unittest.mock import patch
from controller import Controller
from plan import expand
import fleet_nodes
import runtime
from runtime import Gate, HOSTS, Journal, Quota, Rpc, check_identity, check_quorum, check_resources


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


class QuorumGateTests(unittest.TestCase):
    """check_quorum: the installed explicit 4-of-5, f = 1 quorum (#1605)."""

    GOOD = {'quorumMode':'explicit','quorumSize':5,'quorumThreshold':4,
            'quorumByzantineTolerance':1,'quorumFaultTolerant':True,
            'quorumDegenerate':False,'quorumAutoMembers':0,
            'quorumGateIntersectionRefused':False}

    def test_expected_shape_is_derived_not_restated(self):
        self.assertEqual(runtime.expected_quorum(),(len(HOSTS),fleet_nodes.QUORUM_THRESHOLD,1))
        self.assertEqual(runtime.byzantine_tolerance(5,4),1)
        self.assertEqual(runtime.byzantine_tolerance(4,4),0)
        self.assertEqual(runtime.byzantine_tolerance(5,3),0)
        self.assertEqual(runtime.byzantine_tolerance(4,9),0)

    def test_renderer_and_gate_share_one_threshold(self):
        self.assertIs(fleet_nodes.QUORUM_THRESHOLD,runtime.QUORUM_THRESHOLD)

    def test_gate_runs_with_only_the_staged_controller_files(self):
        # The controller host receives staging.CODE only; check_quorum must not
        # depend on fleet_nodes or anything else outside it.
        import staging,subprocess,sys
        src=Path(__file__).parent
        with tempfile.TemporaryDirectory() as tmp:
            for name in staging.CODE:
                shutil.copy(src/name,Path(tmp)/name)
            script=('import sys;sys.path.insert(0,%r);import controller,runtime;'
                    'runtime.check_quorum(%r);'
                    'print(runtime.expected_quorum())') % (tmp,self.GOOD)
            out=subprocess.run([sys.executable,'-I','-c',script],cwd=tmp,capture_output=True,text=True)
            self.assertEqual(out.returncode,0,out.stderr)
            self.assertEqual(out.stdout.strip(),'(5, 4, 1)')

    def test_accepts_explicit_4_of_5(self):
        check_quorum(dict(self.GOOD))
        check_quorum({**self.GOOD,'network':'botho-testnet','quorumCuratedMembers':4})

    def test_rejects_each_deviation(self):
        bad = {'quorumMode':['recommended','Explicit',None],
               'quorumSize':[4,6,'5',5.0,True],
               'quorumThreshold':[3,5,'4',4.0],
               'quorumByzantineTolerance':[0,2,True],
               'quorumFaultTolerant':[False,1,'true'],
               'quorumDegenerate':[True,0],
               'quorumAutoMembers':[1,False],
               'quorumGateIntersectionRefused':[True,0]}
        self.assertEqual(set(bad),set(self.GOOD))
        for key,values in bad.items():
            for value in values:
                with self.subTest(key=key,value=value),self.assertRaisesRegex(Gate,key):
                    check_quorum({**self.GOOD,key:value})

    def test_rejects_each_missing_or_null_field(self):
        for key in self.GOOD:
            with self.subTest(key=key,missing=True),self.assertRaisesRegex(Gate,key):
                check_quorum({k:v for k,v in self.GOOD.items() if k != key})
            with self.subTest(key=key,null=True),self.assertRaisesRegex(Gate,key):
                check_quorum({**self.GOOD,key:None})
        with self.assertRaises(Gate):
            check_quorum({})

    def test_silent_4_of_4_is_refused(self):
        # The #1605 failure: one dropped roster member, node still fine otherwise.
        with self.assertRaisesRegex(Gate,'quorumSize'):
            check_quorum({**self.GOOD,'quorumSize':4,'quorumByzantineTolerance':0,
                          'quorumFaultTolerant':False,'quorumDegenerate':True})


    # #1608: a restarted node serves RPC before its gate's first evaluation.
    NULL_GATE = {k:None for k in runtime.QUORUM_GATE_KEYS}

    def test_gate_and_config_keys_partition_the_shape(self):
        self.assertEqual(set(runtime.QUORUM_GATE_KEYS)|set(runtime.QUORUM_CONFIG_KEYS),set(self.GOOD))
        self.assertFalse(set(runtime.QUORUM_GATE_KEYS)&set(runtime.QUORUM_CONFIG_KEYS))

    def test_unevaluated_gate_passes_only_when_allowed(self):
        check_quorum({**self.GOOD,**self.NULL_GATE},allow_unevaluated=True)
        with self.assertRaisesRegex(Gate,'quorumSize=None'):
            check_quorum({**self.GOOD,**self.NULL_GATE})
        with self.assertRaisesRegex(Gate,'quorumSize=None'):
            check_quorum({**self.GOOD,**self.NULL_GATE},allow_unevaluated=False)

    def test_unevaluated_rejects_partial_null(self):
        for key in runtime.QUORUM_GATE_KEYS:
            with self.subTest(key=key),self.assertRaisesRegex(Gate,key+'=None'):
                check_quorum({**self.GOOD,key:None},allow_unevaluated=True)

    def test_unevaluated_keeps_config_keys_strict(self):
        for key,value in (('quorumMode','recommended'),('quorumFaultTolerant',False),
                          ('quorumDegenerate',True),('quorumMode',None)):
            with self.subTest(key=key,value=value),self.assertRaisesRegex(Gate,key):
                check_quorum({**self.GOOD,**self.NULL_GATE,key:value},allow_unevaluated=True)
        for key in runtime.QUORUM_CONFIG_KEYS:
            with self.subTest(key=key,missing=True),self.assertRaisesRegex(Gate,key):
                check_quorum({k:v for k,v in {**self.GOOD,**self.NULL_GATE}.items() if k != key},
                             allow_unevaluated=True)

    def test_unevaluated_rejects_missing_gate_keys(self):
        # An older binary without the fields is not "unevaluated".
        config={k:self.GOOD[k] for k in runtime.QUORUM_CONFIG_KEYS}
        with self.assertRaises(Gate):
            check_quorum(config,allow_unevaluated=True)
        for key in runtime.QUORUM_GATE_KEYS:
            partial={k:v for k,v in {**self.GOOD,**self.NULL_GATE}.items() if k != key}
            with self.subTest(key=key),self.assertRaises(Gate):
                check_quorum(partial,allow_unevaluated=True)
        with self.assertRaises(Gate):
            check_quorum({},allow_unevaluated=True)

    def test_unevaluated_still_rejects_each_non_null_deviation(self):
        bad = {'quorumMode':['recommended','Explicit',None],
               'quorumSize':[4,6,'5',5.0,True],
               'quorumThreshold':[3,5,'4',4.0],
               'quorumByzantineTolerance':[0,2,True],
               'quorumFaultTolerant':[False,1,'true'],
               'quorumDegenerate':[True,0],
               'quorumAutoMembers':[1,False],
               'quorumGateIntersectionRefused':[True,0]}
        for key,values in bad.items():
            for value in values:
                with self.subTest(key=key,value=value),self.assertRaisesRegex(Gate,key):
                    check_quorum({**self.GOOD,key:value},allow_unevaluated=True)
        with self.assertRaisesRegex(Gate,'quorumSize'):
            check_quorum({**self.GOOD,'quorumSize':4,'quorumByzantineTolerance':0},allow_unevaluated=True)


class DegradeWindowResourceTests(unittest.TestCase):
    """check_resources(..., window) for #1585 degrade steps (#1586)."""

    RESTART = 'unexpected node restart or binary change'

    def setUp(self):
        self.base={'at':1000,'pid':1,'start':'fixed','restarts':0,'binary':'hash',
                   'config':'config-digest','node_key':'peer-key-digest',
                   'disk_free':10*1024**3,'disk_total':20*1024**3,
                   'mem_available':1024**3,'mem_total':2*1024**3,'swap':0,'rss':100*1024**2}
        # D1-like step: stop at 2000, restart at 2050, rejoin by 2400.
        self.window={'step':'D1','action':'stop_restart','fault_start':2000,
                     'restore_at':2050,'window_end':2400}

    def row(self, at, pid=1, start='fixed', **extra):
        return {**self.base,'at':at,'pid':pid,'start':start,**extra}

    def error(self, at):
        return {'at':at,'error':'FileNotFoundError: /proc/0/status'}

    def ring(self):
        """30 records as the observer exports: pre-stop, stopped, restarted."""
        return ([self.row(t) for t in range(1955,2000,5)]
                + [self.error(t) for t in range(2000,2055,5)]
                + [self.row(t,pid=2,start='restarted') for t in range(2055,2105,5)])

    def assertRestartGate(self, records, baseline, now, window):
        with self.assertRaises(Gate) as caught:
            check_resources(records,baseline,now,window)
        self.assertEqual(str(caught.exception),self.RESTART)

    def test_three_argument_form_is_unchanged(self):
        self.assertIsNone(check_resources([self.base],self.base,1000))
        self.assertIsNone(check_resources([self.base],self.base,1000,None))

    def test_unscheduled_restart_gates(self):
        ring=self.ring()
        self.assertEqual(len(ring),30)
        # Host A with no step naming it: the caller passes None.
        self.assertRestartGate([self.row(2100,pid=2,start='restarted')],self.base,2100,None)
        # A kill-then-start fast enough that no error row was sampled.
        fast=[self.row(2090),self.row(2095,pid=2,start='restarted'),self.row(2100,pid=2,start='restarted')]
        self.assertRestartGate(fast,self.base,2100,None)
        # The full observer ring: its error rows gate first, as today.
        with self.assertRaises(Gate):check_resources(ring,self.base,2100,None)
        # A step names host B only, so A's own call gets None: B's window
        # never reaches A's gate, and A's restart still halts the run.
        windows={HOSTS[1]:self.window}
        self.assertIsNone(windows.get(HOSTS[0]))
        self.assertRestartGate(fast,self.base,2100,windows.get(HOSTS[0]))
        # The same window, given to the host it names, accepts its restart.
        self.assertIsNotNone(check_resources(ring,self.base,2100,windows[HOSTS[1]]))

    def test_restart_outside_the_window_gates(self):
        # First observed at window_end: too late.
        late=[self.row(2390),self.row(2395),self.row(2400,pid=2,start='restarted')]
        self.assertRestartGate(late,self.base,2400,self.window)
        # First observed before fault_start: too early, even if seen again inside.
        early=[self.row(1990,pid=2,start='restarted'),self.row(2005,pid=2,start='restarted')]
        self.assertRestartGate(early,self.base,2005,self.window)

    def test_scheduled_restart_rebaselines_only_pid_and_start(self):
        ring=self.ring()
        new=check_resources(ring,self.base,2100,self.window)
        self.assertEqual(new,{**self.base,'pid':2,'start':'restarted'})
        self.assertEqual({k for k in self.base if self.base[k]!=new[k]},{'pid','start'})
        self.assertEqual(new['rss'],self.base['rss'])
        self.assertEqual(self.base['pid'],1)  # the stored baseline is not mutated
        # Against the returned baseline the same ring is accepted, no new change.
        self.assertIsNone(check_resources(ring,new,2100,self.window))
        # A restart seen with base gone from the ring (only error rows before it).
        self.assertEqual(check_resources(ring[9:],self.base,2100,self.window),new)

    def test_second_restart_gates_against_returned_baseline(self):
        ring=self.ring()
        new=check_resources(ring,self.base,2100,self.window)
        third=self.row(2105,pid=3,start='again')
        # Inside the window: ring still shows pre-stop, restarted, third.
        self.assertRestartGate(ring[1:]+[third],new,2105,self.window)
        # Inside the window, pre-stop rows rolled out: the replaced process
        # was alive after restore_at, so it was the restored process.
        self.assertRestartGate([r for r in ring if r.get('pid')==2]+[third],new,2105,self.window)
        # And against the original baseline: two changes in one window.
        self.assertRestartGate(ring[1:]+[third],self.base,2105,self.window)
        # Outside the window (the caller passes None again).
        after=[self.row(t,pid=2,start='restarted') for t in range(2400,2450,5)]
        self.assertIsNone(check_resources(after,new,2445,None))
        self.assertRestartGate(after+[self.row(2450,pid=3,start='again')],new,2450,None)
        # A process coming back (A, B, A) is not one restart either.
        self.assertRestartGate(ring+[self.row(2105)],self.base,2105,self.window)

    def test_identity_and_automatic_restart_still_gate_in_window(self):
        ring=self.ring()
        for key,message in [('binary',self.RESTART),('restarts',self.RESTART),
                            ('config','node configuration or peer identity changed'),
                            ('node_key','node configuration or peer identity changed')]:
            with self.subTest(key=key):
                changed=ring[:-1]+[{**ring[-1],key:1 if key=='restarts' else 'new'}]
                with self.assertRaises(Gate) as caught:
                    check_resources(changed,self.base,2100,self.window)
                self.assertEqual(str(caught.exception),message)

    def test_error_rows_tolerated_only_inside_restart_window(self):
        ring=self.ring()
        check_resources(ring,self.base,2100,self.window)
        shifted={**self.window,'fault_start':2010}
        with self.assertRaises(Gate) as caught:
            check_resources(ring,self.base,2100,shifted)
        self.assertEqual(str(caught.exception),'observer cannot read required metrics')
        with self.assertRaises(Gate):check_resources(ring,self.base,2100,None)
        # The node still stopped: only error rows inside the window.
        stopped=[self.error(t) for t in range(2000,2030,5)]
        self.assertIsNone(check_resources(stopped,self.base,2025,self.window))
        # Resource breakers still apply to the live rows around the gap.
        disk=ring[:-1]+[{**ring[-1],'disk_free':1024}]
        with self.assertRaises(Gate):check_resources(disk,self.base,2100,self.window)
        low=ring[:-2]+[{**r,'mem_available':100*1024**2} for r in ring[-2:]]
        with self.assertRaises(Gate):check_resources(low,self.base,2100,self.window)

    def test_every_restart_action_relaxes_the_same_way(self):
        for action in ('stop_restart','kill_restart','stop_restart_pair'):
            with self.subTest(action=action):
                new=check_resources(self.ring(),self.base,2100,{**self.window,'action':action})
                self.assertEqual((new['pid'],new['start']),(2,'restarted'))

    def test_isolate_and_netem_relax_nothing(self):
        for action in ('isolate','netem'):
            window={**self.window,'action':action}
            with self.subTest(action=action):
                self.assertIsNone(check_resources([self.row(2100)],self.base,2100,window))
                self.assertRestartGate([self.row(2100,pid=2,start='restarted')],
                                       self.base,2100,window)
                with self.assertRaises(Gate):
                    check_resources([self.error(2095),self.row(2100)],self.base,2100,window)

    def test_malformed_or_unknown_window_gates(self):
        for window in [{**self.window,'action':'reboot'},
                       {k:v for k,v in self.window.items() if k!='step'},
                       {**self.window,'extra':1},
                       {**self.window,'restore_at':2500}]:
            with self.subTest(window=window),self.assertRaises(Gate):
                check_resources(self.ring(),self.base,2100,window)

    def test_actions_match_the_degrade_schema(self):
        from discovery_plan import DEGRADE_ACTIONS
        from runtime import HOLD_ACTIONS, RESTART_ACTIONS
        self.assertEqual(RESTART_ACTIONS|HOLD_ACTIONS,set(DEGRADE_ACTIONS))
        self.assertFalse(RESTART_ACTIONS&HOLD_ACTIONS)


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
