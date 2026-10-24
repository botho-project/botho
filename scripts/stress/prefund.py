#!/usr/bin/env python3
"""Tracked prefunder for isolated x-series v2 fleets. Python standard library only.

Moves exactly ``wallets.count * grants_per_wallet`` grants from the producer's
funding wallet into freshly generated campaign wallets. Then it writes the
``opening_balances.json`` that staging pins into ``launch.json``. Run it on the
controller host, which is the only host with a private route to the fleet RPC,
before staging the controller:

    prefund.py check --manifest M   # local validation only; no RPC or signing
    prefund.py run --manifest M     # prefund, reconcile, then stop

Every grant is one main-signer ``prepare``. Its record is created exclusively
before an exclusive submit marker, and ``tx_submit`` is called once. A grant
with a marker is never prepared or submitted again. Any uncertainty holds the
run for operator review; restarting resumes from the records, never from a
replay.
"""
import argparse
import asyncio
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import stat
import time

from controller import Controller
from plan import expand
from runtime import (Gate, HOSTS, Journal, Quota, Rpc, atomic, campaign_targets,
                     campaign_tls_context, check_identity, pinned_file)
from selection import choose

# main's `botho-stress-wallet` Request enum is `deny_unknown_fields`
# (botho-wallet/src/stress.rs). Every request must carry exactly these keys.
SIGNER_FIELDS = {
    'generate': {'wallet'},
    'scan': {'wallet', 'blocks'},
    'restore_check': {'wallet', 'blocks', 'expected_address'},
    'prepare': {'wallet', 'blocks', 'height', 'selected', 'spent', 'reserved', 'recipient',
                'allowed_recipients', 'amount', 'base_rate', 'fee_multiplier', 'artifact'},
    'inspect': {'artifact'},
}
MANIFEST_FIELDS = {
    'schema_version', 'plan_file', 'plan_sha256', 'targets', 'tls_ca_file', 'tls_ca_sha256',
    'signer', 'signer_sha256', 'producer_role', 'funding_wallet', 'wallets',
    'grants_per_wallet', 'grant_picocredits', 'state_dir', 'deadline', 'infrastructure_end',
}
CAMPAIGN_SECONDS = 72 * 3600
CAMPAIGN_DRAIN_SECONDS = 1800   # discovery.py: start + 72 h + 1800 <= infrastructure_end
MATURITY_SECONDS = 3600         # output maturity, staging and activation slack
UNRESOLVED_SECONDS = 900        # same bound as Controller.reconcile()
POLL_SECONDS = 30
HEALTH_SECONDS = 15
UNSYNCED_SECONDS = 120


def need(condition, message):
    if not condition:
        raise Gate(message)


def private_bytes(path, name, limit=1024 * 1024):
    """An owner-only regular file opened without following symlinks."""
    need(isinstance(path, str) and Path(path).is_absolute(), name + ' requires an absolute path')
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError as error:
        raise Gate(name + ' is unreadable or a symlink') from error
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        need(stat.S_ISREG(info.st_mode), name + ' must be a regular file')
        need(info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0, name + ' must be owner-only')
        data = stream.read(limit + 1)
    need(len(data) <= limit, name + ' exceeds its byte bound')
    return data


def latest_deadline(plan, infrastructure_end):
    """The last grant write that still leaves maturity, rehearsal and campaign time."""
    rehearsal = plan['rehearsal']
    campaign = CAMPAIGN_SECONDS if plan.get('execution_mode', 'campaign') == 'campaign' else 0
    return (infrastructure_end - campaign - CAMPAIGN_DRAIN_SECONDS - rehearsal['duration_seconds']
            - rehearsal['drain_seconds'] - MATURITY_SECONDS)


def check_opening_balances(balances, plan):
    """The exact launch.json rule from DiscoveryController and staging.prepare()."""
    need(isinstance(balances, list) and len(balances) == plan['wallets']['count']
         and all(type(x) is int and x > 0 for x in balances),
         'exact positive prefunded opening balances required')
    need(sum(balances) <= int(plan['funding']['max_principal_picocredits']),
         'opening principal exceeds budget')
    return balances


def load_manifest(path):
    """Validate the manifest and its pinned inputs locally; no RPC or signing."""
    data = private_bytes(str(path), 'manifest')
    try:
        m = json.loads(data)
    except ValueError as error:
        raise Gate('manifest is not JSON') from error
    need(isinstance(m, dict) and set(m) == MANIFEST_FIELDS, 'manifest requires exactly the documented fields')
    need(m['schema_version'] == 1, 'unsupported prefund manifest')
    pinned_file(m['plan_file'], m['plan_sha256'], 'plan')
    plan = json.loads(Path(m['plan_file']).read_bytes())
    try:
        expand(plan)
    except (KeyError, TypeError, ValueError) as error:
        raise Gate('plan does not expand: ' + str(error)) from error
    need(plan.get('schema_version') == 2 and plan['funding']['mode'] == 'prefunded_isolated',
         'prefunding is only for isolated v2 plans')
    campaign_targets({'targets': m['targets']}, plan)
    pinned_file(m['tls_ca_file'], m['tls_ca_sha256'], 'TLS CA')
    pinned_file(m['signer'], m['signer_sha256'], 'signer')
    need(m['producer_role'] in HOSTS, 'producer role must be one of the fixed roles')
    count = plan['wallets']['count']
    wallets = m['wallets']
    need(isinstance(wallets, list) and len(wallets) == count, 'campaign wallet count differs from plan')
    everyone = [*wallets, m['funding_wallet']]
    for wallet in everyone:
        need(isinstance(wallet, dict) and set(wallet) == {'address', 'key'}
             and isinstance(wallet['address'], str) and wallet['address'], 'wallet entries are {address, key}')
        private_bytes(wallet['key'], 'wallet key file', 16384)
    need(len({w['address'] for w in everyone}) == count + 1
         and len({w['key'] for w in everyone}) == count + 1,
         'campaign and funding wallets must be distinct')
    limits, wallet_plan = plan['limits'], plan['wallets']
    grants, amount = m['grants_per_wallet'], m['grant_picocredits']
    need(type(grants) is int and grants >= wallet_plan['min_mature_utxos_per_wallet'],
         'grants_per_wallet below min_mature_utxos_per_wallet: every grant is one future input')
    max_fee = int(limits['max_single_signed_fee_picocredits'])
    largest = max(map(int, wallet_plan['transfer_amounts_picocredits']))
    need(type(amount) is int and amount >= largest + max_fee,
         'grant below the largest transfer plus the maximum signed fee')
    need(count * grants * amount <= int(plan['funding']['max_principal_picocredits']),
         'prefund principal exceeds max_principal_picocredits')
    need(count * grants * max_fee <= int(limits['max_signed_fee_picocredits']),
         'worst-case prefund fees exceed max_signed_fee_picocredits')
    need(isinstance(m['state_dir'], str) and Path(m['state_dir']).is_absolute(), 'absolute state_dir required')
    for field in ('deadline', 'infrastructure_end'):
        need(type(m[field]) in (int, float) and math.isfinite(m[field]) and m[field] > 0,
             field + ' must be an absolute UNIX time')
    need(m['deadline'] <= latest_deadline(plan, m['infrastructure_end']),
         'deadline leaves no room for maturity, rehearsal and the 72-hour campaign')
    return m, plan


def create_exclusive(path, value):
    """Create a JSON file once. An existing file is never overwritten."""
    raw = (json.dumps(value, sort_keys=True) + '\n').encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    directory = os.open(Path(path).parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class Prefunder(Controller):
    """Reuses Controller.sync()/canonical_inventory() and selection.choose().

    Wallets ``0..count-1`` are the campaign wallets; the funding wallet is last.
    """

    def __init__(self, manifest, plan, *, clock=time.time, rpc=None, native=None):
        self.m, self.plan, self.clock = manifest, plan, clock
        self.state = Path(manifest['state_dir'])
        self.state.mkdir(mode=0o700, parents=False, exist_ok=True)
        info = self.state.lstat()
        need(stat.S_ISDIR(info.st_mode) and info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0,
             'state_dir must be an owner-only directory')
        self.grants_dir, self.artifacts = self.state / 'grants', self.state / 'artifacts'
        for directory in (self.grants_dir, self.artifacts):
            directory.mkdir(mode=0o700, exist_ok=True)
        self.config = {'signer': manifest['signer']}
        self.targets = campaign_targets({'targets': manifest['targets']}, plan)
        if native is not None:
            self.signer = native
        self.count = plan['wallets']['count']
        self.funding = self.count
        self.wallets = [dict(w) for w in manifest['wallets']] + [dict(manifest['funding_wallet'])]
        self.total = self.count * manifest['grants_per_wallet']
        self.max_fee = int(plan['limits']['max_single_signed_fee_picocredits'])
        # Never trust persisted blocks or inventory: the first sync is a full rescan.
        self.blocks, self.inventory, self.spent = [], [[] for _ in self.wallets], []
        self.inventory_initialized = False
        self.statuses, self.fresh, self.health_at, self.unsynced_since = {}, 0, 0, None
        self.scan_lock = asyncio.Lock()
        self.closed = False
        self.resumed = False
        self.last_poll, self.last_poll_height = 0, -1
        # Validate every durable record before opening a writable journal.
        self.records = self.load_records()
        opening = self.state / 'opening.json'
        self.opening = json.loads(opening.read_text()) if opening.exists() else None
        need(self.opening is not None or not self.records, 'grant records exist without an opening snapshot')
        self.j = Journal(self.state / 'journal.sqlite', clock, limits=plan['limits'])
        self.rpc = rpc or Rpc(self.j, self.targets, campaign_tls_context(manifest), allow_env_proxy=False)

    # ---- durable records -------------------------------------------------

    def record_path(self, index):
        return self.grants_dir / f'grant-{index:04d}.json'

    def marker_path(self, index):
        return self.grants_dir / f'grant-{index:04d}.submit'

    def artifact_path(self, index):
        return self.artifacts / f'grant-{index:04d}.bin'

    def load_records(self):
        records = []
        names = sorted(p.name for p in self.grants_dir.iterdir())
        for index, name in enumerate(n for n in names if n.endswith('.json')):
            need(name == f'grant-{index:04d}.json', 'grant record sequence has a gap or a foreign file')
            row = json.loads(self.record_path(index).read_text())
            need(row.get('index') == index and row.get('artifact') == str(self.artifact_path(index)),
                 'grant record identity changed')
            need(self.artifact_path(index).exists() and sha256_file(row['artifact']) == row['artifact_sha256'],
                 'saved grant artifact missing or altered: ' + name)
            records.append(row)
        expected = {f'grant-{i:04d}.json' for i in range(len(records))}
        expected |= {f'grant-{i:04d}.submit' for i in range(len(records))}
        need(set(names) <= expected, 'orphan submit marker without a grant record')
        recorded = {f'grant-{i:04d}.bin' for i in range(len(records))}
        orphans = sorted(p.name for p in self.artifacts.iterdir() if p.name not in recorded)
        # A signed artifact without a record may already be anywhere: hold, never re-sign.
        need(not orphans, 'orphan signed artifact without a grant record: ' + ', '.join(orphans))
        need(len({r['hash'] for r in records}) == len(records), 'duplicate grant transaction hash')
        need(len(records) <= self.total, 'more grant records than the manifest allows')
        return records

    def save(self, row):
        atomic(self.record_path(row['index']), row)

    def unconfirmed(self):
        return [r for r in self.records if r['state'] != 'confirmed']

    def reserve_inputs(self):
        """Mirror every unconfirmed grant's inputs into the selector's reservations."""
        reserved = sorted({s['id'] for r in self.unconfirmed() for s in r['selected']})
        with self.j.transaction():
            self.j.db.execute('DELETE FROM reservations')
            self.j.db.executemany('INSERT INTO reservations VALUES(?,?,?)',
                                  [(key, 'prefund', self.funding) for key in reserved])
        return reserved

    # ---- signer and RPC --------------------------------------------------

    async def native(self, request):
        need(set(request) == {'operation'} | SIGNER_FIELDS.get(request.get('operation'), {None}),
             'signer request fields differ from main Request')
        if hasattr(self, 'signer'):
            return await self.signer(request)
        return await super().native(request)

    def gate(self, write=False):
        need(not (self.state / 'STOP').exists(), 'operator stop marker')
        if write:
            need(self.clock() < self.m['deadline'], 'prefund write deadline reached')

    async def all_hosts(self, method, params=None):
        return await asyncio.gather(*(self.rpc.call(h, method, params) for h in HOSTS))

    async def health(self, force=False):
        if not force and self.clock() - self.health_at < HEALTH_SECONDS:
            return
        statuses = await self.all_hosts('node_getStatus')
        for host, status in zip(HOSTS, statuses):
            check_identity(status, self.plan)
            need(status.get('mintingActive') is (host == self.m['producer_role']),
                 'minting must be active only on the producer: ' + host)
            need(type(status.get('chainHeight')) is int, 'malformed node height: ' + host)
        unsynced = [h for h, s in zip(HOSTS, statuses) if s.get('synced') is not True]
        if unsynced:
            # Before the opening snapshot nothing is at stake: refuse outright.
            need(self.opening is not None, 'node unsynced at opening: ' + unsynced[0])
            self.unsynced_since = self.unsynced_since or self.clock()
            need(self.clock() - self.unsynced_since <= UNSYNCED_SECONDS,
                 'node unsynced for two minutes: ' + unsynced[0])
            raise Quota('fleet is recovering sync')
        self.unsynced_since = None
        self.statuses = dict(zip(HOSTS, statuses))
        self.fresh = self.health_at = self.clock()

    def unspent(self, wallet):
        state = {s['keyImage']: s for s in self.spent}
        return [o for o in self.inventory[wallet] if not state[o['key_image']]['spent']]

    # ---- opening ---------------------------------------------------------

    async def opening_snapshot(self):
        need(not self.records, 'opening snapshot after grants')
        genesis = await self.all_hosts('getBlockByHeight', {'height': 0})
        need(all(b.get('hash') == self.plan['genesis'] for b in genesis), 'genesis differs from plan')
        height = await self.sync(full=True)
        need(not any(s['pending'] for s in self.spent), 'opening has pending key images')
        for wallet in range(self.count):
            # Fresh wallets on a fresh chain: any prior output means reuse.
            need(not self.inventory[wallet], 'campaign wallet already has on-chain outputs: wallet-%d' % (wallet + 1))
        owned = self.inventory[self.funding]
        spendable = sum(o['utxo']['amount'] for o in self.unspent_setup_inputs(self.funding))
        principal = self.total * self.m['grant_picocredits']
        need(spendable >= principal + self.total * self.max_fee,
             'funding wallet balance cannot cover principal plus worst-case fees')
        self.opening = {'at': self.clock(), 'height': height, 'owned': sorted(o['id'] for o in owned),
                        'balance': sum(o['utxo']['amount'] for o in self.unspent(self.funding)),
                        'spendable': spendable, 'principal': principal}
        create_exclusive(self.state / 'opening.json', self.opening)

    # ---- grants ----------------------------------------------------------

    async def submit(self, row):
        """Write the exclusive marker, then call tx_submit exactly once."""
        self.gate(write=True)
        producer = self.m['producer_role']
        data = Path(row['artifact']).read_bytes()
        need(hashlib.sha256(data).hexdigest() == row['artifact_sha256'], 'grant artifact altered before submit')
        # Never spend the marker on a request the local quota would refuse.
        used = self.j.db.execute('SELECT COUNT(*) FROM requests WHERE host=? AND at>?',
                                 (producer, self.clock() - 60)).fetchone()[0]
        if used + 1 >= min(self.j.limits['max_rpc_per_endpoint_per_minute'],
                           self.j.get('quota:' + producer, 100) // 2):
            raise Quota('producer request budget before submit')
        create_exclusive(self.marker_path(row['index']), {'at': self.clock(), 'hash': row['hash']})
        row.update(state='submitting', submitted_at=self.clock())
        self.save(row)
        try:
            result = await self.rpc.call(producer, 'tx_submit', {'tx_hex': data.hex()}, timeout=30, write=True)
            need(result.get('txHash') == row['hash'], 'submitted hash differs from signed hash')
        except Exception as error:
            row.update(state='unknown', reason=str(error))
            self.save(row)
            raise Gate('grant %d submit reply uncertain; never resubmitted' % row['index']) from error
        row.update(state='accepted', accepted_at=self.clock())
        self.save(row)

    async def offer(self):
        """Prepare, record and submit at most one new grant."""
        if len(self.records) >= self.total or self.clock() >= self.m['deadline']:
            return False
        self.gate(write=True)
        limits = self.j.limits
        if len(self.unconfirmed()) >= limits['max_inflight']:
            return False
        if any(r['state'] != 'accepted' for r in self.unconfirmed()):
            return False  # Resolve every uncertain grant before signing another.
        recent = [r for r in self.records if r.get('submitted_at', -1e18) > self.clock() - 60]
        if len(recent) >= limits['max_normal_submissions_per_minute']:
            return False
        reserved = self.reserve_inputs()
        height = await self.sync()
        amount = self.m['grant_picocredits']
        selected, evidence = choose(self, self.funding, height, amount)
        self.j.set('selection_latest', evidence)
        if not selected:
            return False  # Funding inputs or change are maturing; wait.
        quote = await self.rpc.call(self.m['producer_role'], 'fee_getRate')
        self.gate(write=True)
        index = len(self.records)
        recipient = index % self.count
        artifact = self.artifact_path(index)
        funding_images = {o['key_image'] for o in self.inventory[self.funding]}
        info = await self.native({
            'operation': 'prepare', 'wallet': self.wallets[self.funding]['key'], 'blocks': self.blocks,
            'height': height, 'selected': [o['id'] for o in selected],
            'spent': [s for s in self.spent if s['keyImage'] in funding_images],
            'reserved': reserved, 'recipient': self.wallets[recipient]['address'],
            'allowed_recipients': [w['address'] for w in self.wallets[:self.count]],
            'amount': amount, 'base_rate': int(quote['baseRate']), 'fee_multiplier': 1,
            'artifact': str(artifact)})
        # From here a signed artifact exists. Any refusal leaves it as an orphan,
        # which holds every later start until an operator reviews it.
        need(0 < info['fee'] <= self.max_fee, 'signed grant fee exceeds max_single_signed_fee')
        need(info['bytes'] <= limits['max_signed_transaction_bytes'], 'signed grant byte budget')
        need([s['id'] for s in info['selected']] == [o['id'] for o in selected]
             and info['key_images'] == [s['key_image'] for s in info['selected']], 'signer changed the inputs')
        need(info['outputs'][0]['index'] == 0 and info['outputs'][0]['amount'] == amount
             and [o['index'] for o in info['outputs']] in ([0], [0, 1]), 'grant output shape')
        need(sum(s['amount'] for s in info['selected']) == sum(o['amount'] for o in info['outputs']) + info['fee'],
             'signed grant does not balance')
        inspected = await self.native({'operation': 'inspect', 'artifact': str(artifact)})
        need(all(inspected[k] == info[k] for k in ('hash', 'fee', 'bytes', 'outputs', 'key_images')),
             'inspected artifact differs from signer response')
        need(info['hash'] not in {r['hash'] for r in self.records}, 'duplicate grant hash')
        row = {'index': index, 'recipient': recipient, 'recipient_address': self.wallets[recipient]['address'],
               'amount': amount, 'hash': info['hash'], 'fee': info['fee'], 'bytes': info['bytes'],
               'outputs': info['outputs'], 'key_images': info['key_images'], 'selected': info['selected'],
               'artifact': str(artifact), 'artifact_sha256': sha256_file(artifact), 'fee_quote': quote,
               'scan_height': height, 'prepared_at': self.clock(), 'state': 'prepared'}
        create_exclusive(self.record_path(index), row)
        self.records.append(row)
        self.reserve_inputs()
        await self.submit(row)
        return True

    async def resume(self):
        """Settle records left by a crash before anything new is offered."""
        # Persisted inventory is never trusted (controller.py): restore-scan everything.
        await self.sync(full=True)
        for row in self.records:
            marker = self.marker_path(row['index']).exists()
            if row['state'] == 'prepared' and not marker:
                # Crash between the record and its marker. One attempt is allowed
                # only if no node has ever seen the hash (Controller.admission_tick()).
                statuses = await self.all_hosts('getTransactionStatus', {'hash': row['hash']})
                need(all(s.get('status') == 'unknown' for s in statuses),
                     'unsubmitted grant %d unexpectedly known to the fleet' % row['index'])
                await self.submit(row)
            elif row['state'] == 'prepared':
                # Crash after the marker: the one attempt may have been sent. Never resend.
                row.update(state='submitting', submitted_at=json.loads(
                    self.marker_path(row['index']).read_text())['at'])
                self.save(row)
            elif row['state'] != 'confirmed':
                need(marker, 'grant %d lost its submit marker' % row['index'])
        self.reserve_inputs()
        await self.reconcile(force=True)
        self.resumed = True

    # ---- reconciliation --------------------------------------------------

    async def reconcile(self, force=False):
        pending = self.unconfirmed()
        if not pending:
            return
        height = min(s['chainHeight'] for s in self.statuses.values())
        if not force and self.clock() - self.last_poll < POLL_SECONDS and height == self.last_poll_height:
            return
        self.last_poll, self.last_poll_height = self.clock(), height
        scanned = False
        for row in pending:
            statuses = await self.all_hosts('getTransactionStatus', {'hash': row['hash']})
            if not all(s.get('confirmed') and s.get('txHash') == row['hash'] for s in statuses):
                need(self.clock() - row.get('submitted_at', row['prepared_at']) <= UNRESOLVED_SECONDS,
                     'grant %d unresolved after fifteen minutes; never resubmitted' % row['index'])
                continue
            receipts = await self.all_hosts('getTransaction', {'hash': row['hash']})
            shapes = {(r.get('blockHeight'), r.get('fee'), r.get('outputCount'), r.get('totalOutput'))
                      for r in receipts}
            need(len(shapes) == 1, 'fleet grant receipt disagreement: grant %d' % row['index'])
            need(all(r.get('txHash', row['hash']) == row['hash'] for r in receipts), 'fleet receipt hash mismatch')
            block, fee, outputs, total = shapes.pop()
            need(type(block) is int and fee == row['fee'] and outputs == len(row['outputs'])
                 and total == sum(o['amount'] for o in row['outputs']), 'grant receipt differs from signed grant')
            spent = await self.all_hosts('chain_areKeyImagesSpent', {'keyImages': row['key_images']})
            need(all([s.get('keyImage') for s in host] == row['key_images'] and all(s.get('spent') is True for s in host)
                     for host in spent), 'grant inputs not spent on every node')
            if min(s['chainHeight'] for s in self.statuses.values()) < block:
                continue
            if not scanned:
                await self.sync()
                scanned = True
            for expected in row['outputs']:
                owner = row['recipient'] if expected['index'] == 0 else self.funding
                found = [o for o in self.inventory[owner] if o['id'] == row['hash'] + ':' + str(expected['index'])]
                need(len(found) == 1 and found[0]['utxo']['amount'] == expected['amount']
                     and bytes(found[0]['utxo']['target_key']).hex() == expected['target_key'],
                     'grant %d recipient/change scan mismatch' % row['index'])
            row.update(state='confirmed', block_height=block, confirmed_at=self.clock(),
                       receipts=dict(zip(HOSTS, receipts)))
            self.save(row)
        self.reserve_inputs()

    def accounting(self):
        """Exact funding-wallet and campaign-wallet accounting over the latest scan."""
        need(not any(s['pending'] for s in self.spent), 'accounting with pending key images')
        hashes = {r['hash'] for r in self.records}
        opening = set(self.opening['owned'])
        inflow, external = 0, []
        for output in self.inventory[self.funding]:
            utxo = output['utxo']
            if output['id'] in opening or bytes(utxo['tx_hash']).hex() in hashes:
                continue
            if not (utxo.get('coinbase') or utxo.get('lottery')):
                external.append(output['id'])
            inflow += utxo['amount']
        need(not external, 'unexpected non-coinbase transfer into the funding wallet')
        principal = sum(r['amount'] for r in self.records)
        fees = sum(r['fee'] for r in self.records)
        remaining = sum(o['utxo']['amount'] for o in self.unspent(self.funding))
        difference = self.opening['balance'] + inflow - remaining - principal - fees
        report = {'at': self.clock(), 'opening': self.opening['balance'], 'coinbase_lottery_inflow': inflow,
                  'remaining': remaining, 'principal': principal, 'fees': fees, 'difference': difference}
        self.j.set('accounting', report)
        need(difference == 0, 'exact funding-wallet accounting mismatch')
        for wallet in range(self.count):
            grants = {r['hash'] + ':0' for r in self.records if r['recipient'] == wallet}
            for output in self.inventory[wallet]:
                need(output['id'] in grants or output['utxo'].get('lottery'),
                     'unexpected transfer into campaign wallet-%d' % (wallet + 1))
        return report

    async def complete(self):
        """Final full scan, exact accounting, then the staging inputs. Then stop."""
        height = await self.sync(full=True)
        report = self.accounting()
        state = {s['keyImage']: s for s in self.spent}
        balances = [sum(o['utxo']['amount'] for o in owned if not state[o['key_image']]['spent'])
                    for owned in self.inventory[:self.count]]
        check_opening_balances(balances, self.plan)
        wallets = self.plan['wallets']
        largest = max(map(int, wallets['transfer_amounts_picocredits']))
        readiness = []
        for w in range(self.count):
            ready, counts = choose(self, w, height, headroom=wallets['decoy_headroom'])
            probe, _ = choose(self, w, height, largest, 4, wallets['decoy_headroom'])
            grants = sum(r['recipient'] == w for r in self.records)
            need(sum(1 for o in self.unspent(w) if not o['utxo'].get('lottery')) == grants,
                 'campaign wallet-%d grant outputs spent or missing' % (w + 1))
            readiness.append({**counts, 'grant_outputs': grants, 'four_input_probe_feasible': bool(probe),
                              'ready': len(ready) >= wallets['min_mature_utxos_per_wallet'] and bool(probe)})
        health = {'at': self.clock(), 'height': height, 'plan_sha256': self.m['plan_sha256'],
                  'grants': len(self.records), 'grant_picocredits': self.m['grant_picocredits'],
                  'principal': report['principal'], 'fees': report['fees'],
                  'max_fee': max(r['fee'] for r in self.records), 'accounting': report,
                  'opening_balances': balances,
                  'completed_before_deadline': self.clock() < self.m['deadline'],
                  'inventory_readiness': {'status': 'ready' if all(r['ready'] for r in readiness) else
                                          'maturing_or_generator_limited', 'wallets': readiness}}
        atomic(self.state / 'funding-health.json', health)
        target = self.state / 'opening_balances.json'
        if target.exists():
            need(json.loads(target.read_text()) == balances, 'existing opening_balances.json differs from scan')
        else:
            create_exclusive(target, balances)
        return health

    # ---- loop ------------------------------------------------------------

    async def tick(self):
        """One bounded step. Returns True once the outputs are written."""
        self.gate()
        if (self.state / 'opening_balances.json').exists() and len(self.records) == self.total:
            return True
        await self.health(force=not self.statuses)
        if self.opening is None:
            await self.opening_snapshot()
        if not self.resumed:
            await self.resume()
        await self.reconcile()
        if len(self.records) == self.total and not self.unconfirmed():
            await self.complete()
            return True
        if not self.unconfirmed():
            await self.sync()
            self.accounting()
        if len(self.records) < self.total:
            need(self.clock() < self.m['deadline'], 'prefund deadline reached before every grant was offered')
            await self.offer()
        return False

    async def run(self, sleep=asyncio.sleep):
        try:
            while True:
                try:
                    if await self.tick():
                        return
                except Quota as error:
                    self.j.event(None, 'prefund_quota', str(error))
                await sleep(1)
        except BaseException as error:
            create_exclusive(self.state / ('held-%d.json' % time.time_ns()), {
                'at': self.clock(), 'error': str(error), 'type': type(error).__name__,
                'grants': len(self.records), 'confirmed': len(self.records) - len(self.unconfirmed()),
                'resubmit_forbidden': True})
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('mode', choices=['check', 'run'])
    parser.add_argument('--manifest', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    manifest, plan = load_manifest(args.manifest)
    if args.mode == 'check':
        print(json.dumps({'manifest': 'valid', 'grants': plan['wallets']['count'] * manifest['grants_per_wallet'],
                          'latest_deadline': latest_deadline(plan, manifest['infrastructure_end']),
                          'network_calls': 0, 'signatures': 0}))
        return
    state = Path(manifest['state_dir'])
    state.mkdir(mode=0o700, exist_ok=True)
    with (state / 'prefund.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        prefunder = Prefunder(manifest, plan)
        asyncio.run(prefunder.run())
        print(json.dumps({'opening_balances': str(state / 'opening_balances.json'),
                          'funding_health': str(state / 'funding-health.json')}))


if __name__ == '__main__':
    main()
