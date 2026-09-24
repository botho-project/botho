#!/usr/bin/env python3
"""Bounded native testnet campaign; launch config and secrets stay outside the repo."""
import argparse
import asyncio
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shutil
import subprocess
import time
from plan import expand
from runtime import (Gate, HOSTS, Journal, PENDING, Quota, Rpc, atomic,
                     check_identity, check_resources, digest)


class Controller:
    def __init__(self, state):
        self.state = state
        self.config = json.loads((state/'launch.json').read_text())
        self.plan = json.loads((state/'plan.json').read_text())
        _, self.events = expand(self.plan)
        self.j = Journal(state/'journal.sqlite')
        if self.j.get('plan_digest',digest(self.plan)) != digest(self.plan):
            raise Gate('manifest changed after initialization')
        self.j.set('plan_digest',digest(self.plan))
        if self.j.get('launch_digest',digest(self.config)) != digest(self.config):
            raise Gate('launch configuration changed after initialization')
        self.j.set('launch_digest',digest(self.config))
        for name,expected in self.config['controller_files'].items():
            if hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()!=expected:
                raise Gate('controller source differs from launch pin')
        if hashlib.sha256(Path(self.config['signer']).read_bytes()).hexdigest() != self.config['signer_sha256']:
            raise Gate('signer artifact changed')
        self.rpc = Rpc(self.j)
        self.statuses = {}
        self.fresh = 0
        self.blocks = json.loads((state/'blocks.json').read_text()) if (state/'blocks.json').exists() else []
        self.wallets = self.config['wallets']
        if len(self.wallets) != 8 or len({w['address'] for w in self.wallets}) != 8:
            raise Gate('eight distinct dedicated wallets required')
        self.inventory = [[] for _ in self.wallets]
        self.spent = []
        self.scan_lock = asyncio.Lock()
        self.admission_lock = asyncio.Lock()
        self.signed_tasks = set()
        self.closed = False

    def gate(self):
        if self.j.get('status') not in ('setup','running'):
            raise Gate('run is held: '+str(self.j.get('reason')))
        if (self.state/'STOP').exists():
            raise Gate('operator stop marker')
        if time.time()-self.fresh > 60:
            raise Gate('fleet or observer data stale')
        if any(not s.get('synced') for s in self.statuses.values()):
            raise Quota('fleet is recovering sync')
        deadline = self.j.get('end',self.j.get('setup_start')+8*3600)
        if time.time() >= deadline:
            raise Gate('immutable admission deadline')

    def halt(self, reason):
        if self.j.get('status') in ('setup','running'):
            self.j.set('status','held')
            self.j.set('reason',str(reason))
            self.j.set('stopped_at',time.time())
            self.j.event(None,'held',str(reason))
            self.report()

    async def native(self, request):
        def run():
            result = subprocess.run([self.config['signer']], input=json.dumps(request).encode(),
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=90, check=False)
            if len(result.stdout) > 32*1024*1024:
                raise Gate('signer response bound')
            value = json.loads(result.stdout)
            if result.returncode or not value.get('ok'):
                raise Gate('signer: '+value.get('error','failed'))
            return value['result']
        return await asyncio.to_thread(run)

    async def observer(self, host):
        proc = await asyncio.create_subprocess_exec('ssh','-F','/dev/null','-i',self.config['observer_key'],
            '-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=5',
            '-o','UserKnownHostsFile='+self.config['known_hosts'],'ubuntu@'+host,
            stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE)
        try:
            out, _ = await asyncio.wait_for(proc.communicate(),10)
        except BaseException:
            proc.kill()
            await proc.wait()
            raise
        if proc.returncode or len(out)>128*1024:
            raise Gate('restricted observer export failed: '+host)
        return json.loads(out)

    async def monitor(self):
        while not self.closed:
            started = time.monotonic()
            try:
                statuses = await asyncio.gather(*(self.rpc.call(h,'node_getStatus') for h in HOSTS))
                records = await asyncio.gather(*(self.observer(h) for h in HOSTS))
                for host,status,observed in zip(HOSTS,statuses,records):
                    check_identity(status,self.plan)
                    baseline = self.j.get('baseline:'+host)
                    if baseline is None:
                        if not observed or observed[-1].get('error'):
                            raise Gate('observer baseline unavailable')
                        baseline = self.config.get('resource_baselines',{}).get(host,observed[-1])
                        if baseline['binary'] != self.config['node_sha256']:
                            raise Gate('deployed binary differs from launch pin')
                        self.j.set('baseline:'+host,baseline)
                    check_resources(observed,baseline,time.time())
                    if not status.get('synced'):
                        since = self.j.get('unsynced:'+host) or time.time()
                        self.j.set('unsynced:'+host,since)
                        if time.time()-since > 120:
                            raise Gate('node unsynced for two minutes: '+host)
                    else:
                        self.j.set('unsynced:'+host,None)
                    # Post-idle memory is evaluated once per completed active cycle.
                    self.j.set('latest:'+host,observed[-1])
                minimum = min(s['chainHeight'] for s in statuses)
                if self.j.get('checkpoint_height') != minimum:
                    checkpoints = await asyncio.gather(*(self.rpc.call(h,'getBlockByHeight',{'height':minimum}) for h in HOSTS))
                    if len({b['hash'] for b in checkpoints}) != 1:
                        raise Gate('conflicting block hashes at the same finalized height')
                    self.j.set('checkpoint_height',minimum)
                    self.j.set('checkpoint_hash',checkpoints[0]['hash'])
                if not self.j.get('genesis_verified'):
                    genesis = await asyncio.gather(*(self.rpc.call(h,'getBlockByHeight',{'height':0}) for h in HOSTS))
                    if any(b.get('hash') != self.plan['genesis'] for b in genesis):
                        raise Gate('genesis differs from manifest')
                    self.j.set('genesis_verified',True)
                self.statuses = dict(zip(HOSTS,statuses))
                self.fresh = time.time()
                self.j.set('fleet_latest',{'at':self.fresh,'nodes':self.statuses})
                self.j.event(None,'fleet',{'height':minimum,'at':self.fresh})
                self.memory_cycle()
                # Controller shares no node resources; still stop if its own
                # state volume approaches exhaustion or its evidence grows too far.
                if shutil.disk_usage(self.state).free < 2*1024**3:
                    raise Gate('controller disk headroom')
            except Quota as error:
                self.j.event(None,'monitor_quota',str(error))
            except Gate as error:
                self.halt(error)
            except Exception as error:
                self.j.event(None,'monitor_error',str(error))
                if time.time()-self.fresh > 60:
                    self.halt('monitor unavailable for sixty seconds')
            await asyncio.sleep(max(.2,15-(time.monotonic()-started)))

    def memory_cycle(self):
        last = self.j.get('last_write',0)
        now = time.time()
        pending = bool(self.j.pending())
        for host in HOSTS:
            if self.j.get('rss_cycles:'+host,0) >= 3:
                self.halt('post-idle RSS growth across three cycles: '+host)
            row,base = self.j.get('latest:'+host), self.j.get('baseline:'+host)
            status = self.statuses.get(host,{})
            state = self.j.get('rss_idle:'+host,{})
            # Only continuously observed idle time qualifies. New writes and
            # observation gaps cannot inherit an earlier five-minute window.
            if (state.get('write') != last or state.get('sample_at') is None
                    or not 0 <= now-state['sample_at'] <= 60):
                state['since'] = None
            state.update(write=last,sample_at=now)
            idle = (not pending and 0 <= now-self.fresh <= 60 and row and base
                    and 0 <= now-row['at'] <= 60 and status.get('synced') is True
                    and status.get('mintingActive') is False
                    and type(status.get('mempoolSize')) is int and status['mempoolSize']==0)
            if not idle:
                state['since'] = None
            elif state.get('since') is None:
                state['since'] = now
            self.j.set('rss_idle:'+host,state)
            # Each host completes a write cycle independently: an active
            # producer must not consume a passive peer's idle observation, or
            # be compared against its own fixed idle RSS baseline while mining.
            completed = self.j.get('memory_cycle:'+host,self.j.get('memory_cycle',0))
            if (not last or state['since'] is None or now-state['since'] < 300
                    or completed >= last):
                continue
            high = row['rss']-base['rss'] > 64*1024**2 and row['rss'] > base['rss']*1.2
            count = self.j.get('rss_cycles:'+host,0)+1 if high else 0
            with self.j.transaction():
                self.j.set('rss_cycles:'+host,count)
                self.j.set('memory_cycle:'+host,last)
            if count >= 3:
                self.halt('post-idle RSS growth across three cycles: '+host)

    async def sync(self, full=False):
        async with self.scan_lock:
            if not self.statuses:
                raise Gate('fleet status unavailable')
            height = min(s['chainHeight'] for s in self.statuses.values())
            start = 0 if full else (self.blocks[-1]['height']+1 if self.blocks else 0)
            new = []
            for first in range(start,height+1,50):
                last = min(height,first+49)
                blocks = await self.rpc.call(HOSTS[(first//50)%5],'chain_getOutputs',{'start_height':first,'end_height':last},timeout=20)
                if [b['height'] for b in blocks] != list(range(first,last+1)):
                    raise Gate('incomplete or unordered output range')
                new.extend(blocks)
            previous = self.blocks
            if full:
                if previous and new[:len(previous)] != previous:
                    raise Gate('full rescan disagrees with checkpointed output history')
                self.blocks = new
            else:
                self.blocks.extend(new)
            atomic(self.state/'blocks.json',self.blocks)
            scans = []
            # Separate signer processes and derivations: restores never reuse key state.
            for wallet in self.wallets:
                scanned = await self.native({'operation':'restore_check' if full else 'scan',
                    'wallet':wallet['key'], 'blocks':self.blocks,
                    **({'expected_address':wallet['address']} if full else {})})
                if scanned['address'] != wallet['address']:
                    raise Gate('wallet identity differs from allowlist')
                scans.append(scanned['owned'])
            all_images = list(dict.fromkeys(o['key_image'] for owned in scans for o in owned))
            # Finalized spent images stay spent. Phase/full scans independently
            # recheck them; routine scans reserve RPC capacity for live inputs.
            cached = {} if full else {s['keyImage']:s for s in self.spent if s['spent']}
            query_images = [image for image in all_images if image not in cached]
            checked = dict(cached)
            for first in range(0,len(query_images),256):
                images = query_images[first:first+256]
                host=HOSTS[(first//256+height)%len(HOSTS)]
                found = await self.rpc.call(host,'chain_areKeyImagesSpent',{'keyImages':images})
                if ([s.get('keyImage') for s in found] != images or any('error' in s or
                    type(s.get('spent')) is not bool or type(s.get('pending')) is not bool for s in found)):
                    raise Gate('malformed spent-image response')
                checked.update((s['keyImage'],s) for s in found)
            spent=[checked[image] for image in all_images]
            self.inventory, self.spent = scans, spent
            atomic(self.state/'inventory.json',{'at':time.time(),'height':height,'owned':scans,'spent':spent})
            return height

    def spendable(self, wallet, height, amount=0, count=1):
        state = {s['keyImage']:s for s in self.spent}
        reserved = {r[0] for r in self.j.db.execute('SELECT input FROM reservations')}
        outputs = []
        for output in self.inventory[wallet]:
            status = state.get(output['key_image'])
            utxo = output['utxo']
            if (not status or status['spent'] or status['pending'] or output['id'] in reserved
                    or height-utxo['created_at'] < 10):
                continue
            age = height-utxo['created_at']
            # Match production age_similarity_band: integer +/-10%, ten-block floor.
            delta = age//10
            low,high = max(10,age-delta),age+delta
            keys = {o['targetKey'] for block in self.blocks if low <= height-block['height'] <= high
                    for o in block['outputs']}
            keys.discard(bytes(utxo['target_key']).hex())
            if len(keys) >= 19:
                outputs.append(output)
        outputs.sort(key=lambda u:u['utxo']['amount'],reverse=True)
        # Lottery history can reuse a target key. Distinct RPC IDs do not make
        # two independently spendable inputs when their key images coincide.
        unique=[]
        seen=set()
        for output in outputs:
            if output['key_image'] not in seen:
                unique.append(output)
                seen.add(output['key_image'])
        outputs=unique
        if amount:
            selected = outputs[:count]
            if len(selected)<count or sum(o['utxo']['amount'] for o in selected) < amount+5_000_000_000:
                return []
            return selected
        return outputs

    def report(self):
        rows = self.j.rows()
        counts = {}
        for row in rows:
            label = row['kind']+':'+row['state']
            counts[label] = counts.get(label,0)+1
        complete = [r for r in rows if r['state']=='reconciled' and r['submitted']]
        def latency(field):
            values = sorted(r['finished']-r[field] for r in complete)
            if not values:
                return {'count':0}
            return {'count':len(values),'median':values[len(values)//2],
                    'p95':values[min(len(values)-1,int(len(values)*.95))],'max':values[-1]}
        report = {'at':time.time(),'run_id':self.config['run_id'],'status':self.j.get('status'),
            'reason':self.j.get('reason'),'setup_start':self.j.get('setup_start'),
            'start':self.j.get('start'),'end':self.j.get('end'),'plan_sha256':digest(self.plan),
            'counts':counts,'submitted_to_reconciled_seconds':latency('submitted'),
            'nominal_campaign_offers':692,
            'not_yet_offered':692-sum(r['kind']=='campaign' for r in rows),
            'offered_to_reconciled_seconds':latency('offered'),
            'signed_fees':sum(r['fee'] for r in rows if r['prepared']),
            'faucet_fees':sum(json.loads(r['info']).get('faucet_fee',0) for r in rows if r['kind']=='funding'),
            'accounting':self.j.get('accounting'),'fleet':self.j.get('fleet_latest'),
            'resources':{h:self.j.get('latest:'+h) for h in HOSTS},
            'controller':{'max_rss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                          'user_cpu':resource.getrusage(resource.RUSAGE_SELF).ru_utime},
            'coverage':{'web':False,'snap':False,'confidential_amounts':False,'public_node_faults':False}}
        atomic(self.state/'report.json',report)
        self.j.set('report_at',time.time())
        return report

    async def prepare(self, identifier, count, phase_limit, burst=False, crash=None):
        row = self.j.intent(identifier)
        self.gate()
        if any(r['sender']==row['sender'] for r in self.j.pending()):
            return False
        height = await self.sync()
        selected = self.spendable(row['sender'],height,row['amount'],count)
        if not selected:
            self.j.event(identifier,'inventory_wait',{'wallet':row['sender'],'height':height})
            return False
        self.gate()
        if time.time() > row['offered']+120:
            return False
        self.j.transition(identifier,'eligible')
        charged=self.j.db.execute('SELECT COALESCE(SUM(fee),0) FROM intents WHERE prepared IS NOT NULL').fetchone()[0]
        if charged+5_000_000_000>500_000_000_000:
            raise Gate('insufficient remaining fee budget to prepare')
        writes=self.j.db.execute('SELECT COUNT(*) FROM intents WHERE prepared IS NOT NULL OR submitted IS NOT NULL').fetchone()[0]
        if writes>=800:
            raise Gate('chain write budget reached before signing')
        artifact = self.state/'artifacts'/(identifier+'.bin')
        if artifact.exists() or artifact.with_suffix('.partial').exists():
            raise Gate('orphaned prepared artifact needs reconciliation: '+identifier)
        fee_rate = await self.rpc.call(HOSTS[0],'fee_getRate')
        info = await self.native({'operation':'prepare','wallet':self.wallets[row['sender']]['key'],
            'blocks':self.blocks,'height':height,'selected':[o['id'] for o in selected],
            'spent':self.spent,'reserved':[r[0] for r in self.j.db.execute('SELECT input FROM reservations')],
            'recipient':self.wallets[row['recipient']]['address'],
            'allowed_recipients':[w['address'] for w in self.wallets],
            'amount':row['amount'],'base_rate':int(fee_rate['baseRate']),'artifact':str(artifact)})
        info.update(burst=burst,phase_limit=phase_limit,crash=crash,
                    scan_height=height,prepared_at=time.time())
        self.gate()
        self.j.prepare(identifier,info,phase_limit,row['offered']+120)
        if crash=='prepared' and not self.j.get('crash:prepared'):
            self.j.set('crash:prepared',identifier)
            self.report()
            os._exit(77)
        await self.submit_prepared(identifier)
        return True

    async def submit_prepared(self, identifier):
        row = self.j.intent(identifier)
        info = json.loads(row['info'])
        self.gate()
        data = Path(info['artifact']).read_bytes()
        inspected = await self.native({'operation':'inspect','artifact':info['artifact']})
        for key in ('hash','fee','bytes','outputs','key_images'):
            if inspected[key] != info[key]:
                raise Gate('saved artifact does not match durable journal')
        if time.time() >= row['offered']+120:
            self.j.transition(identifier,'expired','prepared intent exceeded original slot',finished=time.time())
            # Retain the signed input reservations: a signed artifact still exists.
            self.halt('prepared intent expired; review reserved inputs')
            return
        host = HOSTS[(sum(identifier.encode()) % len(HOSTS))]
        wire = len(json.dumps({'jsonrpc':'2.0','id':1404,'method':'tx_submit','params':{'tx_hex':data.hex()}}).encode())
        self.j.begin_submit(identifier,host,wire,info['burst'],row['offered']+120)
        self.j.set('last_write',time.time())
        if info.get('crash')=='submitting' and not self.j.get('crash:submitting'):
            # Crash after the durable marker. No retry is possible on recovery;
            # this intentionally ambiguous intent must become unresolved if absent.
            # Exercised in isolated tests only, never selected by public schedule.
            self.j.set('crash:submitting',identifier)
            os._exit(77)
        try:
            result = await self.rpc.call(host,'tx_submit',{'tx_hex':data.hex()},timeout=30,write=True)
            if result.get('txHash') != row['hash']:
                raise Gate('submitted hash differs from canonical saved hash')
            self.j.transition(identifier,'accepted',info={**info,'response_at':time.time()})
            self.report()
            if info.get('crash')=='accepted' and not self.j.get('crash:accepted'):
                self.j.set('crash:accepted',identifier)
                self.report()
                os._exit(77)
        except Exception as error:
            self.j.transition(identifier,'unknown',str(error))
            self.halt('submission reply uncertain or rejected; retained input reservations')

    async def fund(self, identifier):
        row = self.j.intent(identifier)
        self.gate()
        if row['state'] != 'planned' or time.time()>row['offered']+120:
            return
        faucet = await self.rpc.call('faucet.botho.io','faucet_getStatus')
        if not faucet.get('enabled') or int(faucet.get('amountPerRequest',0)) != 1_000_000_000_000:
            raise Gate('unexpected faucet configuration')
        self.gate()
        if time.time()>row['offered']+120:
            return
        with self.j.transaction():
            rows = self.j.rows("kind='funding' AND submitted IS NOT NULL")
            if len(rows)>=24:
                raise Gate('funding count cap')
            # Fixed offer times can precede the previous actual submission + 900
            # seconds. Wait within this offer's original slot; never replay it.
            if any(r['submitted']>time.time()-900 for r in rows):
                return
            if sum(r['recipient']==row['recipient'] and r['submitted']>time.time()-86400 for r in rows)>=3:
                raise Gate('recipient daily funding cap')
            attempted = self.j.db.execute("SELECT COUNT(*) FROM intents WHERE submitted IS NOT NULL OR prepared IS NOT NULL").fetchone()[0]
            if attempted>=800:
                raise Gate('chain write budget')
            self.j.transition(identifier,'submitting',submitted=time.time(),ingress='faucet.botho.io')
        self.j.set('last_write',time.time())
        try:
            result = await self.rpc.call('faucet.botho.io','faucet_request',
                {'address':self.wallets[row['recipient']]['address']},timeout=90,write=True)
            if not result.get('success') or int(result.get('amount',0)) != row['amount'] or not re.fullmatch(r'[0-9a-f]{64}',result.get('txHash','')):
                raise Gate('faucet reply rejected or unexpected')
            self.j.transition(identifier,'accepted',hash=result['txHash'],info={'response_at':time.time()})
            self.report()
        except Exception as error:
            self.j.transition(identifier,'unknown',str(error))
            self.halt('faucet reply uncertain; funding slot consumed without retry')

    async def reconcile(self):
        while not self.closed:
            started = time.monotonic()
            try:
                for row in self.j.pending():
                    # A persisted prepared intent has never acquired a submit marker.
                    # Query its hash on every ingress before allowing its first submit.
                    if row['hash']:
                        statuses = await asyncio.gather(*(self.rpc.call(h,'getTransactionStatus',{'hash':row['hash']}) for h in HOSTS))
                    else:
                        statuses = []
                    if row['state']=='prepared':
                        if any(s.get('status')!='unknown' for s in statuses):
                            self.halt('unsubmitted prepared hash unexpectedly exists on chain')
                        elif self.j.get('status') in ('setup','running') and not self.admission_lock.locked():
                            async with self.admission_lock:
                                if self.j.intent(row['id'])['state']=='prepared':
                                    await self.submit_prepared(row['id'])
                        continue
                    age = time.time()-(row['submitted'] or row['prepared'])
                    if not statuses or not all(s.get('confirmed') and s.get('txHash')==row['hash'] for s in statuses):
                        if age>900:
                            self.j.transition(row['id'],'unresolved','not reconciled within fifteen minutes')
                            self.halt('unresolved payment exceeded fifteen minutes')
                        elif age>300:
                            self.halt('payment unconfirmed after five minutes')
                        continue
                    receipts = await asyncio.gather(*(self.rpc.call(h,'getTransaction',{'hash':row['hash']}) for h in HOSTS))
                    if len({(r.get('blockHeight'),r.get('fee'),r.get('outputCount'),r.get('totalOutput')) for r in receipts}) != 1:
                        raise Gate('fleet transaction receipt disagreement')
                    info = json.loads(row['info'])
                    info.update(all_confirmed_at=time.time(),block_height=receipts[0]['blockHeight'])
                    self.j.transition(row['id'],'confirmed',info=info)
                    # Wait for monitor to expose the inclusion height to wallet scanning.
                    if not self.statuses or min(s['chainHeight'] for s in self.statuses.values()) < info['block_height']:
                        continue
                    await self.sync()
                    recipient = self.inventory[row['recipient']]
                    received = [o for o in recipient if bytes(o['utxo']['tx_hash']).hex()==row['hash'] and o['utxo']['output_index']==0]
                    if len(received)!=1 or received[0]['utxo']['amount']!=row['amount']:
                        raise Gate('recipient ownership/value reconciliation failed')
                    if row['kind'] != 'funding':
                        if receipts[0]['fee'] != row['fee']:
                            raise Gate('node receipt fee differs from signed fee')
                        for expected in info['outputs']:
                            owner = row['recipient'] if expected['index']==0 else row['sender']
                            found = [o for o in self.inventory[owner] if bytes(o['utxo']['tx_hash']).hex()==row['hash']
                                     and o['utxo']['output_index']==expected['index']]
                            if len(found)!=1 or found[0]['utxo']['amount']!=expected['amount'] or bytes(found[0]['utxo']['target_key']).hex()!=expected['target_key']:
                                raise Gate('recipient/change metadata mismatch')
                        states = await asyncio.gather(*(self.rpc.call(h,'chain_areKeyImagesSpent',{'keyImages':info['key_images']}) for h in HOSTS))
                        if any([s.get('keyImage') for s in results] != info['key_images'] or not all(s.get('spent') for s in results) for results in states):
                            raise Gate('spent-input state disagreement')
                    else:
                        info['faucet_fee'] = receipts[0]['fee']
                    info['recipient_verified_at'] = time.time()
                    self.j.finish(row['id'],info)
                    self.report()
                if not self.j.pending() and self.blocks:
                    self.accounting()
                    self.report()
            except Quota as error:
                self.j.event(None,'reconcile_quota',str(error))
            except Exception as error:
                self.halt('reconciliation: '+str(error))
            await asyncio.sleep(max(.2,30-(time.monotonic()-started)))

    def accounting(self):
        state = {s['keyImage']:s for s in self.spent}
        if any(s['pending'] for s in self.spent):
            return
        balances = [sum(o['utxo']['amount'] for o in owned if not state[o['key_image']]['spent']) for owned in self.inventory]
        grants = sum(r['amount'] for r in self.j.rows("kind='funding' AND state='reconciled'"))
        fees = sum(r['fee'] for r in self.j.rows("kind!='funding' AND state='reconciled'"))
        # Recognize actual scanned lottery awards explicitly; never round a difference.
        lottery = {(o['txHash'],o['outputIndex']):int.from_bytes(bytes.fromhex(o['amountCommitment']),'little')
                   for b in self.blocks for o in b['outputs'] if o.get('lottery')}
        awards = sum(lottery.get((bytes(o['utxo']['tx_hash']).hex(),o['utxo']['output_index']),0)
                     for owned in self.inventory for o in owned)
        report = {'at':time.time(),'opening':0,'grants':grants,'lottery_receipts':awards,
                  'balances':balances,'ending':sum(balances),'signed_fees':fees,
                  'difference':grants+awards-sum(balances)-fees}
        self.j.set('accounting',report)
        if report['difference'] != 0:
            raise Gate('exact integer accounting mismatch')

    async def setup_tick(self):
        start = self.j.get('setup_start')
        if time.time()>=start+8*3600:
            raise Gate('eight-hour setup deadline; inventory not ready')
        for index in range(24):
            due = start+index*900
            if time.time()<due:
                break
            identifier = 'funding-'+str(index).zfill(2)
            self.j.offer(identifier,'funding',due,None,index%8,1_000_000_000_000)
            row = self.j.intent(identifier)
            if row['state']=='planned':
                if time.time()>due+120:
                    self.j.transition(identifier,'skipped','funding slot missed',finished=time.time())
                elif not self.j.pending():
                    await self.fund(identifier)
                return
        if self.j.pending():
            return
        if time.time()-self.j.get('setup_scan_at',0)<30:
            return
        height = await self.sync()
        self.j.set('setup_scan_at',time.time())
        self.accounting()
        counts = [len(self.spendable(w,height)) for w in range(8)]
        self.j.set('inventory_counts',{'at':time.time(),'height':height,'mature_with_decoys':counts})
        if min(counts)>=4:
            await self.sync(full=True)
            self.accounting()
            # Readiness probes must use the actual signer, including every wallet's
            # four-input shape. Probe artifacts remain protected and never submitted.
            fee_rate = await self.rpc.call(HOSTS[0],'fee_getRate')
            for w in range(8):
                probe = self.state/'probes'/('wallet-'+str(w)+'.bin')
                if probe.exists():
                    raise Gate('setup probe already exists; operator review required')
                selected = self.spendable(w,height,10_000_000_000,4)
                if not selected:
                    raise Gate('four-input readiness amount unavailable')
                await self.native({'operation':'prepare','wallet':self.wallets[w]['key'],'blocks':self.blocks,
                    'height':height,'selected':[o['id'] for o in selected],'spent':self.spent,'reserved':[],
                    'recipient':self.wallets[(w+1)%8]['address'],'allowed_recipients':[x['address'] for x in self.wallets],
                    'amount':10_000_000_000,'base_rate':int(fee_rate['baseRate']),'artifact':str(probe)})
            self.gate()
            start = time.time()+60
            with self.j.transaction():
                self.j.set('start',start)
                self.j.set('end',start+72*3600)
                self.j.set('status','running')
                self.j.set('active_phase','correctness')
                self.j.set('phase_gates',{'correctness':True})
            atomic(self.state/'activated.json',{'run_id':self.config['run_id'],'plan_sha256':digest(self.plan),
                'start':start,'end':start+72*3600,'inventory':counts,'height':height})
            self.report()
            return
        bootstrap = self.j.rows("kind='bootstrap'")
        if len(bootstrap)>=64:
            raise Gate('setup inventory exhausted after sixty-four bootstrap offers')
        next_grant=start+(int((time.time()-start)//900)+1)*900
        if next_grant<=start+23*900 and next_grant-time.time()<240:
            return
        # Self-transfers split mature value while preserving all principal in the
        # allowlist. Start as soon as eligible funding outputs exist, interleaving
        # with the immutable 15-minute faucet schedule.
        for w in sorted(range(8),key=lambda i:counts[i]):
            if self.spendable(w,height,100_000_000_000,1):
                identifier='bootstrap-'+str(len(bootstrap)).zfill(2)
                self.j.offer(identifier,'bootstrap',time.time(),w,w,100_000_000_000)
                await self.prepare(identifier,1,1)
                return

    async def campaign_tick(self):
        start = self.j.get('start')
        now = time.time()
        if now >= self.j.get('end'):
            self.j.set('status','draining')
            self.j.set('stopped_at',self.j.get('end'))
            return
        elapsed = now-start
        # Materialize every offered slot before gates so downtime cannot hide
        # missing work at a phase boundary. Only eight live offers may queue.
        for event in self.events:
            offered=start+event['offset_seconds']
            if offered>now:
                break
            sender=int(event['sender'].split('-')[-1])-1
            recipient=int(event['recipient'].split('-')[-1])-1
            self.j.offer(event['intent'],'campaign',offered,sender,recipient,int(event['amount_picocredits']))
            row=self.j.intent(event['intent'])
            if row['state'] in ('planned','eligible') and now>offered+120:
                self.j.transition(event['intent'],'skipped','slot expired; no catch-up',finished=now)
        queued=self.j.rows("kind='campaign' AND state IN ('planned','eligible')")
        for row in queued[8:]:
            self.j.transition(row['id'],'skipped','queue budget',finished=now)
        phase = next((p for p in self.plan['phases'] if p['start_hour']*3600 <= elapsed < p['end_hour']*3600),None)
        if not phase:
            return
        active = self.j.get('active_phase')
        if phase['id'] != active:
            earlier = self.j.rows("kind='campaign' AND offered<?",(start+phase['start_hour']*3600,))
            if self.j.pending() or any(r['state']!='reconciled' for r in earlier):
                raise Gate('previous phase incomplete; escalation held')
            await self.sync(full=True)
            self.accounting()
            if active=='correctness':
                shapes={json.loads(r['info']).get('input_count') for r in earlier}
                if not {1,2,4} <= shapes:
                    raise Gate('correctness input-shape coverage missing')
            counts=[len(self.spendable(w,min(s['chainHeight'] for s in self.statuses.values()))) for w in range(8)]
            if min(counts)<1:
                raise Gate('inventory_exhausted at phase boundary')
            self.j.set('active_phase',phase['id'])
            gates=self.j.get('phase_gates')
            gates[phase['id']]=True
            self.j.set('phase_gates',gates)
            self.report()
        for event in self.events:
            offered=start+event['offset_seconds']
            if offered>now:
                break
            sender=int(event['sender'].split('-')[-1])-1
            recipient=int(event['recipient'].split('-')[-1])-1
            self.j.offer(event['intent'],'campaign',offered,sender,recipient,int(event['amount_picocredits']))
            row=self.j.intent(event['intent'])
            if row['state'] not in ('planned','eligible'):
                continue
            if now>offered+120:
                self.j.transition(event['intent'],'skipped','slot expired; no catch-up',finished=now)
                continue
            if len(self.j.pending())>=event['max_inflight']:
                continue
            index=int(event['intent'].split('-')[-1])
            count = (2 if index==2 else 4 if index==4 else 1)
            if index==6 or (phase['id']=='recovery' and not self.j.get('restore_recovery')):
                await self.restore_wallet(sender)
                if phase['id']=='recovery':
                    self.j.set('restore_recovery',True)
            crash = None
            if phase['id']=='recovery':
                if not self.j.get('crash:prepared'):
                    crash='prepared'
                elif not self.j.get('crash:accepted'):
                    crash='accepted'
            await self.prepare(event['intent'],count,event['max_inflight'],event['scenario']=='burst',crash)
            # Offers remain fixed. A slow signer must not turn expired slots into
            # a backlog; the next tick records all missed offers explicitly.
            return

    async def restore_wallet(self,wallet):
        if any(r['sender']==wallet for r in self.j.pending()):
            raise Gate('cannot hand off a wallet with unresolved inputs')
        old=Path(self.wallets[wallet]['key'])
        restored=self.state/'wallets'/('restored-'+str(wallet)+'.mnemonic')
        if not restored.exists():
            with old.open('rb') as source, restored.open('xb') as target:
                shutil.copyfileobj(source,target)
                target.flush()
                os.fsync(target.fileno())
        before=await self.native({'operation':'scan','wallet':str(old),'blocks':self.blocks})
        after=await self.native({'operation':'restore_check','wallet':str(restored),'blocks':self.blocks,
                                'expected_address':self.wallets[wallet]['address']})
        if before!=after:
            raise Gate('restored wallet inventory differs')
        self.wallets[wallet]['key']=str(restored)
        self.j.set('wallet_paths',[w['key'] for w in self.wallets])
        self.j.event(None,'wallet_restored',{'wallet':wallet})

    async def run(self):
        if not self.j.get('status'):
            self.j.set('setup_start',self.config['setup_start'])
            self.j.set('status','setup')
        paths=self.j.get('wallet_paths')
        if paths:
            for wallet,path in zip(self.wallets,paths):
                wallet['key']=path
        # A crashed signing operation may have left a published artifact before
        # SQLite preparation. Hold rather than sign again or silently reuse it.
        for artifact in (self.state/'artifacts').iterdir():
            row=self.j.intent(artifact.stem)
            if artifact.suffix!='.bin' or not row or not row['prepared']:
                self.halt('orphaned or partial signed artifact after restart')
        monitor=asyncio.create_task(self.monitor())
        reconciler=None
        try:
            startup=time.time()
            while not self.fresh and time.time()-startup<55 and self.j.get('status') in ('setup','running'):
                await asyncio.sleep(1)
            if self.fresh:
                await self.sync()
            reconciler=asyncio.create_task(self.reconcile())
            while True:
                status=self.j.get('status')
                if status in ('complete','incomplete'):
                    break
                if status=='running' and time.time()>=self.j.get('end'):
                    self.j.set('status','draining')
                    self.j.set('stopped_at',self.j.get('end'))
                    status='draining'
                if (self.state/'STOP').exists():
                    self.halt('operator stop marker')
                if status in ('held','draining') and time.time()>self.j.get('stopped_at',time.time())+1800:
                    break
                try:
                    if status in ('setup','running'):
                        self.gate()
                        async with self.admission_lock:
                            if status=='setup':
                                await self.setup_tick()
                            else:
                                await self.campaign_tick()
                except Quota as error:
                    self.j.event(None,'admission_quota',str(error))
                except Exception as error:
                    self.halt(str(error))
                if time.time()-self.j.get('last_summary_at',0)>21600:
                    self.report()
                    atomic(self.state/'reports'/str(int(time.time())),self.report())
                    self.j.set('last_summary_at',time.time())
                await asyncio.sleep(1)
            if self.fresh and not self.j.pending():
                await self.sync(full=True)
                self.accounting()
            rows=self.j.rows("kind='campaign'")
            success=len(rows)==692 and all(r['state']=='reconciled' for r in rows)
            self.j.set('status','complete' if success else 'incomplete')
        finally:
            self.closed=True
            monitor.cancel()
            if reconciler:
                reconciler.cancel()
            self.report()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode',choices=['run','report'])
    parser.add_argument('--state',type=Path,required=True)
    args=parser.parse_args()
    os.umask(0o077)
    with (args.state/'controller.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        controller=Controller(args.state)
        if args.mode=='report':
            print(json.dumps(controller.report(),indent=2))
        else:
            asyncio.run(controller.run())


if __name__=='__main__':
    main()
