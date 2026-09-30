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
import shutil
import subprocess
import time
from plan import expand
from runtime import (Gate, HOSTS, Journal, PENDING, Quota, Rpc, atomic,
                     check_identity, check_resources, digest, campaign_targets, campaign_tls_context, pinned_file)


class Controller:
    def __init__(self, state):
        self.state = state
        self.config = json.loads((state/'launch.json').read_text())
        self.plan = json.loads((state/'plan.json').read_text())
        _, self.events = expand(self.plan)
        self.targets = campaign_targets(self.config, self.plan)
        tls_context = campaign_tls_context(self.config)
        if 'targets' in self.config:
            pinned_file(self.config.get('known_hosts'), self.config.get('known_hosts_sha256'), 'observer known_hosts')
        if 'hosts_file' in self.config or 'hosts_sha256' in self.config:
            pinned_file(self.config.get('hosts_file'), self.config.get('hosts_sha256'), 'hosts file')
        self.j = Journal(state/'journal.sqlite', limits=self.plan['limits'] if self.plan.get('schema_version') == 2 else None)
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
        self.rpc = Rpc(self.j, self.targets, tls_context, allow_env_proxy='targets' not in self.config)
        self.statuses = {}
        self.fresh = 0
        self.blocks = json.loads((state/'blocks.json').read_text()) if (state/'blocks.json').exists() else []
        self.wallets = self.config['wallets']
        wallet_count = self.plan['wallets']['count'] if self.plan.get('schema_version') == 2 else 8
        if len(self.wallets) != wallet_count or len({w['address'] for w in self.wallets}) != wallet_count:
            raise Gate('manifest requires distinct dedicated wallets')
        self.inventory = [[] for _ in self.wallets]
        # Never trust persisted wallet inventory after a process restart.
        self.inventory_initialized = False
        self.spent = []
        self.scan_lock = asyncio.Lock()
        self.admission_lock = asyncio.Lock()
        self.reconcile_wakeup = asyncio.Event()
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
        deadline = self.j.get('end',self.j.get('setup_start')+self.plan.get('setup_deadline_hours',8)*3600)
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
            '-o','UserKnownHostsFile='+self.config['known_hosts'],self.targets[host]['observer_ssh_target'],
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
                previous_height = min((s['chainHeight'] for s in self.statuses.values()),default=-1)
                self.statuses = dict(zip(HOSTS,statuses))
                self.fresh = time.time()
                self.j.set('fleet_latest',{'at':self.fresh,'nodes':self.statuses})
                self.j.event(None,'fleet',{'height':minimum,'at':self.fresh})
                self.memory_cycle()
                # Controller shares no node resources; still stop if its own
                # state volume approaches exhaustion or its evidence grows too far.
                if shutil.disk_usage(self.state).free < 2*1024**3:
                    raise Gate('controller disk headroom')
                if minimum > previous_height:
                    self.reconcile_wakeup.set()
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

    async def sync(self, full=False, *, draining=False):
        async with self.scan_lock:
            if not self.statuses:
                raise Gate('fleet status unavailable')
            height = min(s['chainHeight'] for s in self.statuses.values())
            start = 0 if full else (self.blocks[-1]['height']+1 if self.blocks else 0)
            new = []
            waited = 0.0

            async def read(host, method, params, timeout=10):
                nonlocal waited
                # Only these idempotent reads wait for the shared durable quota.
                # Reserve ten requests/minute for monitoring (or all but one
                # slot when the advertised endpoint quota is smaller).
                while True:
                    # STOP closes admission, not read-only receipt/final scans.
                    # Shutdown and the quota wait bound still apply to drains.
                    if self.closed or (not draining and (self.state / "STOP").exists()):
                        raise Gate("scan stopped during quota wait")
                    try:
                        now = self.j.clock()
                        count = self.j.db.execute(
                            "SELECT COUNT(*) FROM requests WHERE host=? AND at>?",
                            (host, now - 60),
                        ).fetchone()[0]
                        limit = min(self.j.limits["max_rpc_per_endpoint_per_minute"], self.j.get("quota:" + host, 100) // 2)
                        if count >= max(1, limit - 10):
                            raise Quota("scan reserves monitoring headroom")
                        return await self.rpc.call(
                            host, method, params, timeout=timeout
                        )
                    except Quota:
                        if waited >= 300:
                            raise Gate("scan quota wait exceeded five minutes")
                        before = time.monotonic()
                        await asyncio.sleep(min(1, 300 - waited))
                        waited += time.monotonic() - before

            for first in range(start,height+1,50):
                last = min(height,first+49)
                blocks = await read(
                    HOSTS[(first // 50) % 5],
                    "chain_getOutputs",
                    {"start_height": first, "end_height": last},
                    timeout=20,
                )
                if [b['height'] for b in blocks] != list(range(first,last+1)):
                    raise Gate('incomplete or unordered output range')
                new.extend(blocks)
            previous = self.blocks
            if full:
                if previous and new[:len(previous)] != previous:
                    raise Gate('full rescan disagrees with checkpointed output history')
                blocks = new
            else:
                blocks = previous+new
            incremental = self.inventory_initialized and not full
            scan_blocks = new if incremental else blocks
            if incremental:
                # The native scanner rejects duplicate output IDs within its
                # request. Preserve that rule across separate append requests.
                seen = {(o['txHash'],o['outputIndex']) for b in previous for o in b['outputs']}
                for block in new:
                    for output in block['outputs']:
                        identifier = (output['txHash'],output['outputIndex'])
                        if identifier in seen:
                            raise Gate('duplicate output id across incremental history')
                        seen.add(identifier)
            scans = []
            # Separate signer processes and derivations: restores never reuse key state.
            for index,wallet in enumerate(self.wallets):
                owned = {o['id']:o for o in self.inventory[index]} if incremental else {}
                if scan_blocks or not incremental:
                    scanned = await self.native({'operation':'restore_check' if full else 'scan',
                        'wallet':wallet['key'], 'blocks':scan_blocks,
                        **({'expected_address':wallet['address']} if full else {})})
                    if scanned['address'] != wallet['address']:
                        raise Gate('wallet identity differs from allowlist')
                    for output in scanned['owned']:
                        if output['id'] in owned:
                            raise Gate('duplicate owned output across incremental scans')
                        owned[output['id']] = output
                scans.append(list(owned.values()))
            all_images = list(dict.fromkeys(o['key_image'] for owned in scans for o in owned))
            # Finalized spent images stay spent. Phase/full scans independently
            # recheck them; routine scans reserve RPC capacity for live inputs.
            cached = {} if full else {s['keyImage']:s for s in self.spent if s['spent']}
            query_images = [image for image in all_images if image not in cached]
            checked = dict(cached)
            for first in range(0,len(query_images),256):
                images = query_images[first:first+256]
                host=HOSTS[(first//256+height)%len(HOSTS)]
                found = await read(
                    host, "chain_areKeyImagesSpent", {"keyImages": images}
                )
                if ([s.get('keyImage') for s in found] != images or any('error' in s or
                    type(s.get('spent')) is not bool or type(s.get('pending')) is not bool for s in found)):
                    raise Gate('malformed spent-image response')
                checked.update((s['keyImage'],s) for s in found)
            spent=[checked[image] for image in all_images]
            # Do not advance in-memory scan progress on a failed native/RPC
            # check: retry must scan the same new outputs, never skip them.
            if full or new:
                atomic(self.state/'blocks.json',blocks)
            atomic(self.state/'inventory.json',{'at':time.time(),'height':height,'owned':scans,'spent':spent})
            self.blocks, self.inventory, self.spent = blocks, scans, spent
            self.inventory_initialized = True
            return height

    def canonical_history(self):
        # Match target-only ledger resolution before any maturity/age filtering.
        # History and inventory retain unsupported receipts for accounting.
        canonical = {}
        for block in sorted(self.blocks, key=lambda block: block['height']):
            for output in block['outputs']:
                key = output['targetKey'].lower()
                if not output.get('lottery', False) and key not in canonical:
                    canonical[key] = (block['height'], output)
        return canonical

    def canonical_inventory(self, wallet, canonical=None):
        canonical = self.canonical_history() if canonical is None else canonical
        seen = set()
        outputs = []
        for output in self.inventory[wallet]:
            # The signer inventory is also retained verbatim for accounting.
            # Reject malformed entries here without rewriting that evidence.
            if not isinstance(output, dict) or not isinstance(output.get('utxo'), dict):
                continue
            utxo = output['utxo']
            if (
                not isinstance(output.get('id'), str)
                or not isinstance(output.get('key_image'), str)
                or not output['key_image']
                or utxo.get('lottery', False) is not False
                or any(
                    type(utxo.get(field)) is not int or utxo[field] < 0
                    for field in ('created_at', 'output_index', 'amount')
                )
            ):
                continue
            if any(
                not isinstance(utxo.get(field), list)
                or len(utxo[field]) != 32
                or any(
                    type(byte) is not int or not 0 <= byte <= 255
                    for byte in utxo[field]
                )
                for field in ('target_key', 'tx_hash')
            ):
                continue
            key = bytes(utxo['target_key']).hex()
            tx_hash = bytes(utxo['tx_hash']).hex()
            if key == '0' * 64 or output['id'] != tx_hash + ':' + str(
                utxo['output_index']
            ):
                continue
            original = canonical.get(key)
            if utxo.get('lottery', False) or original is None or key in seen:
                continue
            created, row = original
            if (
                created != utxo['created_at']
                or row.get('txHash', '').lower() != tx_hash
                or row.get('outputIndex') != utxo['output_index']
            ):
                continue
            seen.add(key)
            outputs.append(output)
        return outputs

    def unspent_setup_inputs(self, wallet, canonical=None):
        """Existing distinct inputs, including those awaiting maturity or decoys.

        This count only prevents redundant splits; admission still requires
        spendable's eligibility checks and every wallet's four-input signer probe.
        """
        state = {s['keyImage']: s for s in self.spent}
        reserved = {r[0] for r in self.j.db.execute('SELECT input FROM reservations')}
        outputs, seen_ids, seen_images = [], set(), set()
        for output in self.canonical_inventory(wallet, canonical):
            status = state.get(output['key_image'])
            if (
                not status
                or status.get('spent') is not False
                or status.get('pending') is not False
                or 'error' in status
                or output['id'] in reserved
                or output['id'] in seen_ids
                or output['key_image'] in seen_images
            ):
                continue
            outputs.append(output)
            seen_ids.add(output['id'])
            seen_images.add(output['key_image'])
        return outputs

    def spendable(self, wallet, height, amount=0, count=1):
        canonical = self.canonical_history()
        outputs = []
        decoys = {}
        for output in self.unspent_setup_inputs(wallet, canonical):
            utxo = output['utxo']
            if height-utxo['created_at'] < 10:
                continue
            age = height-utxo['created_at']
            # Match production age_similarity_band: integer +/-10%, ten-block floor.
            delta = age//10
            low,high = max(10,age-delta),age+delta
            keys = {key for key, (created, _) in canonical.items()
                    if low <= height-created <= high and key != '0'*64}
            keys.discard(bytes(utxo['target_key']).hex())
            if len(keys) >= 19:
                outputs.append(output)
                decoys[output['id']] = keys
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
            # Match the native signer: no selected real target may be a decoy
            # in any ring. Keep the deterministic largest-value prefix; wait
            # if it is infeasible rather than searching alternative input sets.
            excluded = {bytes(o['utxo']['target_key']).hex() for o in selected}
            if any(len(decoys[o['id']] - excluded) < 19 for o in selected):
                return []
            return selected
        return outputs

    def report(self):
        from reporting import controller_report
        return controller_report(self)

    def intent_deadline(self, row):
        return row['offered']+self.j.limits['start_slot_grace_seconds']

    async def prepare(self, identifier, count, phase_limit, burst=False, crash=None, fee_multiplier=1, cohort=None):
        row = self.j.intent(identifier)
        self.gate()
        if any(r['sender']==row['sender'] for r in self.j.pending()):
            return False
        height = await self.sync()
        selected = self.spendable(row['sender'],height,row['amount'],count)
        if not selected:
            self.j.event(identifier,'inventory_wait',{'wallet':row['sender'],'height':height,
                **self.j.get('selection_latest',{})})
            return False
        self.gate()
        if time.time() > self.intent_deadline(row):
            return False
        self.j.transition(identifier,'eligible')
        charged=self.j.db.execute('SELECT COALESCE(SUM(fee),0) FROM intents WHERE prepared IS NOT NULL').fetchone()[0]
        if charged+self.j.limits['max_single_signed_fee_picocredits']>self.j.limits['max_signed_fee_picocredits']:
            raise Gate('insufficient remaining fee budget to prepare')
        writes=self.j.db.execute('SELECT COUNT(*) FROM intents WHERE prepared IS NOT NULL OR submitted IS NOT NULL').fetchone()[0]
        if writes>=self.j.limits['max_unique_chain_writes']:
            raise Gate('chain write budget reached before signing')
        artifact = self.state/'artifacts'/(identifier+'.bin')
        if artifact.exists() or artifact.with_suffix('.partial').exists():
            raise Gate('orphaned prepared artifact needs reconciliation: '+identifier)
        ingress = HOSTS[0]
        if self.plan.get('schema_version') == 2:
            ingress = HOSTS[int(hashlib.sha256((str(self.plan['seed'])+identifier).encode()).hexdigest(),16)%len(HOSTS)]
        fee_rate = await self.rpc.call(ingress,'fee_getRate')
        info = await self.native({'operation':'prepare','wallet':self.wallets[row['sender']]['key'],
            'blocks':self.blocks,'height':height,'selected':[o['id'] for o in selected],
            'spent':self.spent,'reserved':[r[0] for r in self.j.db.execute('SELECT input FROM reservations')],
            'recipient':self.wallets[row['recipient']]['address'],
            'allowed_recipients':[w['address'] for w in self.wallets],
            'amount':row['amount'],'base_rate':int(fee_rate['baseRate']),'artifact':str(artifact),
            **({'fee_multiplier':fee_multiplier} if self.plan.get('schema_version') == 2 else {})})
        if self.plan.get('schema_version') == 2:
            from fee_evidence import signed_fee_evidence
            info.update(fee_evidence=signed_fee_evidence(info, fee_multiplier), cohort=cohort,
                        fee_quote=fee_rate, ingress_role=ingress)
        info.update(burst=burst,phase_limit=phase_limit,crash=crash,
                    scan_height=height,prepared_at=time.time())
        self.gate()
        self.j.prepare(identifier,info,phase_limit,self.intent_deadline(row))
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
        if time.time() >= self.intent_deadline(row):
            self.j.transition(identifier,'expired','prepared intent exceeded original slot',finished=time.time())
            # Retain the signed input reservations: a signed artifact still exists.
            self.halt('prepared intent expired; review reserved inputs')
            return
        host = info.get('ingress_role', HOSTS[(sum(identifier.encode()) % len(HOSTS))])
        wire = len(json.dumps({'jsonrpc':'2.0','id':1404,'method':'tx_submit','params':{'tx_hex':data.hex()}}).encode())
        self.j.begin_submit(identifier,host,wire,info['burst'],self.intent_deadline(row))
        # Retain the durable marker's byte charge through receipt transitions.
        info = json.loads(self.j.intent(identifier)['info'])
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
        if row['state'] != 'planned' or time.time()>self.intent_deadline(row):
            return
        faucet = await self.rpc.call('faucet.botho.io','faucet_getStatus')
        if not faucet.get('enabled') or int(faucet.get('amountPerRequest',0)) != 1_000_000_000_000:
            raise Gate('unexpected faucet configuration')
        self.gate()
        if time.time()>self.intent_deadline(row):
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

    async def wait_for_reconcile(self, delay):
        try:
            await asyncio.wait_for(self.reconcile_wakeup.wait(), timeout=delay)
        except asyncio.TimeoutError:
            pass

    async def reconcile(self):
        next_poll = 0
        while not self.closed:
            # Height wakes only complete saved confirmations. Keep the ordinary
            # receipt/status polling deadline independent of monitor cadence.
            poll = time.monotonic() >= next_poll
            if poll:
                next_poll = time.monotonic()+30
            self.reconcile_wakeup.clear()
            try:
                for row in self.j.pending():
                    # Admission alone owns first submission of prepared work.
                    # Quota pressure must not interrupt unrelated receipt reads.
                    if row['state']=='prepared':
                        continue
                    info = json.loads(row['info'])
                    saved = info.get('confirmed_receipts') if row['state']=='confirmed' else None
                    fetch_receipts = saved is None
                    if fetch_receipts:
                        if not poll:
                            continue
                        if row['hash']:
                            statuses = await asyncio.gather(*(self.rpc.call(h,'getTransactionStatus',{'hash':row['hash']}) for h in HOSTS))
                        else:
                            statuses = []
                        age = time.time()-(row['submitted'] or row['prepared'])
                        if not statuses or not all(s.get('confirmed') and s.get('txHash')==row['hash'] for s in statuses):
                            if age>900:
                                self.j.transition(row['id'],'unresolved','not reconciled within fifteen minutes')
                                self.halt('unresolved payment exceeded fifteen minutes')
                            elif age>300:
                                self.halt('payment unconfirmed after five minutes')
                            continue
                        receipts = await asyncio.gather(*(self.rpc.call(h,'getTransaction',{'hash':row['hash']}) for h in HOSTS))
                        saved = dict(zip(HOSTS,receipts))
                    if set(saved) != set(HOSTS):
                        raise Gate('incomplete saved confirmation receipts')
                    receipts = [saved[h] for h in HOSTS]
                    if len({(r.get('blockHeight'),r.get('fee'),r.get('outputCount'),r.get('totalOutput')) for r in receipts}) != 1:
                        raise Gate('fleet transaction receipt disagreement')
                    if any(r.get('txHash',row['hash']) != row['hash'] for r in receipts):
                        raise Gate('fleet transaction receipt hash mismatch')
                    if fetch_receipts:
                        info.update(all_confirmed_at=time.time(),block_height=receipts[0]['blockHeight'],
                                    confirmed_receipts=saved)
                        self.j.transition(row['id'],'confirmed',info=info)
                    if info['block_height'] != receipts[0]['blockHeight']:
                        raise Gate('saved confirmation height mismatch')
                    # Monitor wakeups cannot authorize scans from stale or
                    # incomplete fleet state, including after a process restart.
                    if (set(self.statuses) != set(HOSTS) or time.time()-self.fresh > 60
                        or any(not s.get('synced') for s in self.statuses.values())
                        or min(s['chainHeight'] for s in self.statuses.values()) < info['block_height']):
                        continue
                    await self.sync(draining=True)
                    if time.time()-self.fresh > 60:
                        raise Gate('fleet data stale during reconciliation')
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
            if not self.closed:
                await self.wait_for_reconcile(max(.2,next_poll-time.monotonic()))

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
        opening = self.j.get('opening_balance',0)
        report = {'at':time.time(),'opening':opening,'grants':grants,'lottery_receipts':awards,
                  'balances':balances,'ending':sum(balances),'signed_fees':fees,
                  'difference':opening+grants+awards-self.j.get('opening_lottery',0)-sum(balances)-fees}
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
            selections = [self.spendable(w,height,10_000_000_000,4) for w in range(8)]
            if not all(selections):
                return
            height = await self.sync(full=True)
            self.accounting()
            # Recheck every complete selection at the refreshed height before
            # creating any protected probe artifact. Insufficient inventory is
            # a wait within the existing setup deadline, not a signer failure.
            selections = [self.spendable(w,height,10_000_000_000,4) for w in range(8)]
            if not all(selections):
                return
            # Readiness probes must use the actual signer, including every wallet's
            # four-input shape. Probe artifacts remain protected and never submitted.
            fee_rate = await self.rpc.call(HOSTS[0],'fee_getRate')
            for w in range(8):
                probe = self.state/'probes'/('wallet-'+str(w)+'.bin')
                if probe.exists():
                    raise Gate('setup probe already exists; operator review required')
                selected = selections[w]
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
        next_grant=start+(int((time.time()-start)//900)+1)*900
        if next_grant<=start+23*900 and next_grant-time.time()<240:
            return
        # Self-transfers split mature value while preserving all principal in the
        # allowlist. Start as soon as eligible funding outputs exist, interleaving
        # with the immutable 15-minute faucet schedule.
        for w in sorted(range(8), key=lambda i: counts[i]):
            if len(self.unspent_setup_inputs(w)) >= 4:
                # More splits consume fees and reset ages while these inputs
                # are waiting for maturity or age-matched decoys.
                continue
            if self.spendable(w, height, 100_000_000_000, 1):
                if len(bootstrap) >= 64:
                    raise Gate(
                        'setup inventory exhausted after sixty-four bootstrap offers'
                    )
                identifier = 'bootstrap-' + str(len(bootstrap)).zfill(2)
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

    async def admission_tick(self):
        async with self.admission_lock:
            self.gate()
            waiting = self.j.rows("state='prepared' ORDER BY prepared, offered, id")
            if waiting:
                row = waiting[0]
                # Persisted signatures have no submit marker. Require every
                # ingress to report unknown before their one allowed attempt.
                statuses = await asyncio.gather(*(self.rpc.call(
                    h,'getTransactionStatus',{'hash':row['hash']}) for h in HOSTS))
                if any(s.get('status')!='unknown' for s in statuses):
                    self.halt('unsubmitted prepared hash unexpectedly exists on chain')
                else:
                    await self.submit_prepared(row['id'])
                # Even when quota defers this signature, never let new signing
                # steal its next capacity slot or reserve more wallet inputs.
                return
            if self.j.get('status')=='setup':
                await self.setup_tick()
            else:
                await self.campaign_tick()

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
                await self.sync(draining=True)
            reconciler=asyncio.create_task(self.reconcile())
            while True:
                started=time.monotonic()
                status=self.j.get('status')
                if status in ('complete','incomplete','rehearsal_complete'):
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
                        await self.admission_tick()
                except Quota as error:
                    self.j.event(None,'admission_quota',str(error))
                except Exception as error:
                    self.halt(str(error))
                if time.time()-self.j.get('last_summary_at',0)>21600:
                    self.report()
                    atomic(self.state/'reports'/str(int(time.time())),self.report())
                    self.j.set('last_summary_at',time.time())
                # Include admission/report work in the cadence, but always
                # yield so monitoring and reconciliation can make progress.
                await asyncio.sleep(max(.2,1-(time.monotonic()-started)))
            if self.j.get('status') == 'rehearsal_complete':
                return
            if self.fresh and not self.j.pending():
                await self.sync(full=True, draining=True)
                self.accounting()
            from reporting import workload_complete
            success=workload_complete(self.j,self.events,self.plan['phases'])
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
        controller_type = Controller
        if json.loads((args.state/'plan.json').read_text()).get('schema_version') == 2:
            from discovery import DiscoveryController
            controller_type = DiscoveryController
        controller=controller_type(args.state)
        if args.mode=='report':
            print(json.dumps(controller.report(),indent=2))
        else:
            asyncio.run(controller.run())


if __name__=='__main__':
    main()
