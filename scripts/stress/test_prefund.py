"""Offline prefunder tests: a fake five-node fleet and a fake main signer, no network."""
import asyncio
import contextlib
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

import prefund
from controller import SPENT_AUDIT_IMAGES
from lottery import PROTOCOL_VERSION
from test_admission_throughput import settling_quote
from prefund import Prefunder, SIGNER_FIELDS, SIGNER_OPTIONAL, latest_deadline, load_manifest
from runtime import Gate, HOSTS, Quota, Rpc
from selection import choose
from test_targets import targets

PRODUCER = HOSTS[2]  # n3 is the sole producer (#1576 decision 4).
GRANT = 10**11
FILLERS = 26
ROOT = Path(__file__).resolve().parents[2]


class Crash(BaseException):
    """A process death: never caught by `except Exception`."""


def key(*parts):
    return hashlib.sha256(repr(parts).encode()).hexdigest()


class Fleet:
    """Five agreeing nodes over one fake chain, plus the main signer's behavior."""

    def __init__(self, plan, addresses):
        self.plan, self.addresses = plan, addresses  # key path -> address
        self.height, self.blocks, self.outputs = 0, {0: []}, {}
        self.spent, self.mempool, self.txs = {}, {}, {}
        self.status_override, self.receipt_override, self.known_override = {}, {}, {}
        self.chain_info_override = {}
        self.pending_override = set()  # mined images a lagging mempool still reports (#1956)
        self.submits, self.requests, self.submit_hook = [], [], None
        self.quotes, self.quote_hook = [], None
        self.counter = 0
        self.coinbase_to = None
        # Decoy density: unrelated outputs per block, and the funding wallet's
        # coinbase cadence and value.
        self.fillers, self.coinbase_every, self.coinbase_amount = FILLERS, 25, 5 * 10**9
        self.batch_fees = []

    def output(self, height, owner, amount, tx=None, index=0, coinbase=False, lottery=False, target=None):
        self.counter += 1
        tx = tx or key('tx', self.counter)
        target = target or key('target', self.counter)
        row = {'targetKey': target, 'txHash': tx, 'outputIndex': index}
        if lottery:
            row['lottery'] = True
        self.blocks.setdefault(height, []).append(row)
        self.outputs[(tx, index)] = {'owner': owner, 'amount': amount, 'coinbase': coinbase,
                                     'lottery': lottery, 'height': height, 'target': target}
        return tx + ':' + str(index)

    def mine(self, count=1):
        for _ in range(count):
            self.height += 1
            self.blocks.setdefault(self.height, [])
            for _ in range(self.fillers):
                self.output(self.height, None, 1)
            if self.coinbase_to and self.height % self.coinbase_every == 0:
                self.output(self.height, self.coinbase_to, self.coinbase_amount, coinbase=True)
            for tx_hash, tx in list(self.mempool.items()):
                for image in tx['key_images']:
                    self.spent[image] = self.height
                for out in tx['outputs']:
                    self.output(self.height, tx['owners'][out['index']], out['amount'], tx_hash,
                                out['index'], target=out['target_key'])
                self.txs[tx_hash] = {**tx, 'block': self.height}
                del self.mempool[tx_hash]

    def pending_images(self):
        return {i for tx in self.mempool.values() for i in tx['key_images']}

    async def call(self, host, method, params=None, timeout=10, write=False):
        params = params or {}
        self.requests.append((host, method))
        if method == 'getChainInfo':
            return {'lotteryRules': 2, 'protocolVersion': PROTOCOL_VERSION, **self.chain_info_override.get(host, {})}
        if method == 'node_getStatus':
            return {'network': self.plan['network'], 'gitCommit': self.plan['node_commit'], 'version': '0.6.0',
                    'synced': True, 'mintingActive': host == PRODUCER, 'chainHeight': self.height,
                    **self.status_override.get(host, {})}
        if method == 'getBlockByHeight':
            return {'hash': self.plan['genesis'] if params['height'] == 0 else key('block', params['height'])}
        if method == 'chain_getOutputs':
            return [{'height': h, 'outputs': copy.deepcopy(self.blocks.get(h, []))}
                    for h in range(params['start_height'], params['end_height'] + 1)]
        if method == 'chain_areKeyImagesSpent':
            pending = self.pending_images() | self.pending_override
            return [{'keyImage': i, 'spent': i in self.spent, 'pending': i in pending,
                     'spentHeight': self.spent.get(i)} for i in params['keyImages']]
        if method == 'getTransactionStatus':
            h = params['hash']
            status = 'confirmed' if h in self.txs else 'pending' if h in self.mempool else 'unknown'
            status = self.known_override.get((host, h), status)
            return {'txHash': h, 'status': status, 'confirmed': status == 'confirmed'}
        if method == 'getTransaction':
            tx = self.txs[params['hash']]
            return {'txHash': params['hash'], 'blockHeight': tx['block'], 'fee': tx['fee'],
                    'outputCount': len(tx['outputs']), 'totalOutput': sum(o['amount'] for o in tx['outputs']),
                    **self.receipt_override.get(host, {})}
        if method == 'fee_getRate':
            return {'baseRate': 1}
        if method == 'estimateFee':
            # #2001: the producer quotes the draft (an artifact is the tx JSON).
            assert host == PRODUCER
            fee = json.loads(bytes.fromhex(params['tx_hex']))['fee']
            self.quotes.append(fee)
            return self.quote_hook(fee) if self.quote_hook else settling_quote(fee)
        if method == 'tx_submit':
            assert write and host == PRODUCER
            tx = json.loads(bytes.fromhex(params['tx_hex']))
            self.submits.append(tx['hash'])
            if self.submit_hook:
                self.submit_hook(tx)
            self.mempool[tx['hash']] = tx
            return {'txHash': tx['hash']}
        raise AssertionError('unexpected RPC ' + method)

    def scan(self, wallet, blocks):
        address = self.addresses[wallet]
        owned = []
        for block in blocks:
            for row in block['outputs']:
                meta = self.outputs[(row['txHash'], row['outputIndex'])]
                if meta['owner'] != address:
                    continue
                identifier = row['txHash'] + ':' + str(row['outputIndex'])
                owned.append({'id': identifier, 'key_image': 'ki-' + identifier, 'utxo': {
                    'target_key': list(bytes.fromhex(meta['target'])), 'tx_hash': list(bytes.fromhex(row['txHash'])),
                    'output_index': row['outputIndex'], 'created_at': block['height'], 'amount': meta['amount'],
                    'coinbase': meta['coinbase'], 'lottery': meta['lottery'],
                    # The P5 signer echoes a V2 row's context (#1642).
                    'lottery_context': copy.deepcopy(row.get('lotteryContext'))}})
        return address, owned

    async def signer(self, request):
        operation = request['operation']
        required = {'operation'} | SIGNER_FIELDS[operation]
        assert required <= set(request) <= required | SIGNER_OPTIONAL.get(operation, set()), request.keys()
        self.requests.append(('signer', operation))
        if operation in ('scan', 'restore_check'):
            address, owned = self.scan(request['wallet'], request['blocks'])
            if operation == 'restore_check':
                assert address == request['expected_address']
            return {'address': address, 'owned': owned}
        if operation in ('inspect', 'inspect_grants'):
            tx = json.loads(Path(request['artifact']).read_bytes())
            # Main's campaign `inspect` refuses more than two outputs.
            assert operation == 'inspect_grants' or len(tx['outputs']) <= 2
            return {k: tx[k] for k in ('hash', 'fee', 'bytes', 'outputs', 'key_images')}
        if operation == 'prepare_grants':
            return self.prepare_grants(request)
        assert operation == 'prepare'
        _, owned = self.scan(request['wallet'], request['blocks'])
        owned = {o['id']: o for o in owned}
        spent = {s['keyImage']: s for s in request['spent']}
        assert request['recipient'] in request['allowed_recipients']
        assert request['fee_multiplier'] == 1
        selected = []
        for identifier in request['selected']:
            output = owned[identifier]
            assert identifier not in request['reserved']
            state = spent[output['key_image']]
            assert not state['spent'] and not state['pending'] and state['spentHeight'] is None
            assert request['height'] - output['utxo']['created_at'] >= 10
            selected.append(output)
        fee = 10**6 * len(selected)
        total = sum(o['utxo']['amount'] for o in selected)
        assert total >= request['amount'] + fee
        self.counter += 1
        outputs = [{'index': 0, 'amount': request['amount'], 'target_key': key('target', 'r', self.counter),
                    'public_key': '00'}]
        owners = [request['recipient']]
        if total - request['amount'] - fee:
            outputs.append({'index': 1, 'amount': total - request['amount'] - fee,
                            'target_key': key('target', 'c', self.counter), 'public_key': '00'})
            owners.append(self.addresses[request['wallet']])
        tx = {'hash': key('grant', self.counter), 'fee': fee, 'bytes': 4000, 'outputs': outputs,
              'key_images': [o['key_image'] for o in selected]}
        tx['owners'] = owners
        Path(request['artifact']).write_bytes(json.dumps(tx).encode())
        return {**{k: tx[k] for k in ('hash', 'fee', 'bytes', 'outputs', 'key_images')},
                'height': request['height'], 'input_count': len(selected), 'baseline_fee': fee,
                'fee_multiplier': 1, 'artifact': request['artifact'],
                'selected': [{'id': o['id'], 'key_image': o['key_image'], 'amount': o['utxo']['amount']}
                             for o in selected]}


    def prepare_grants(self, request):
        """Main's `prepare_grants` (#1937): grant i is output i, change follows."""
        _, owned = self.scan(request['wallet'], request['blocks'])
        owned = {o['id']: o for o in owned}
        spent = {s['keyImage']: s for s in request['spent']}
        grants = request['outputs']
        assert 1 <= len(grants) <= 15 and 1 <= len(request['selected']) <= 4
        assert all(g['recipient'] in request['allowed_recipients'] for g in grants)
        selected = []
        for identifier in request['selected']:
            output = owned[identifier]
            assert identifier not in request['reserved']
            state = spent[output['key_image']]
            assert not state['spent'] and not state['pending'] and state['spentHeight'] is None
            assert request['height'] - output['utxo']['created_at'] >= 10
            selected.append(output)
        # A re-sign carries the producer's quoted fee as its floor (#2001).
        fee = max(10**6 * (len(selected) + len(grants)), request.get('min_fee', 0))
        self.batch_fees.append(fee)
        total = sum(o['utxo']['amount'] for o in selected)
        principal = sum(g['amount'] for g in grants)
        assert total >= principal + fee
        self.counter += 1
        outputs = [{'index': i, 'amount': g['amount'], 'target_key': key('target', 'r', self.counter, i),
                    'public_key': '00'} for i, g in enumerate(grants)]
        owners = [g['recipient'] for g in grants]
        if total - principal - fee:
            outputs.append({'index': len(grants), 'amount': total - principal - fee,
                            'target_key': key('target', 'c', self.counter), 'public_key': '00'})
            owners.append(self.addresses[request['wallet']])
        tx = {'hash': key('grant', self.counter), 'fee': fee, 'bytes': 4000 + 1200 * len(outputs),
              'outputs': outputs, 'key_images': [o['key_image'] for o in selected], 'owners': owners}
        Path(request['artifact']).write_bytes(json.dumps(tx).encode())
        return {**{k: tx[k] for k in ('hash', 'fee', 'bytes', 'outputs', 'key_images')},
                'height': request['height'], 'input_count': len(selected), 'baseline_fee': fee,
                'fee_multiplier': 1, 'grants': len(grants), 'artifact': request['artifact'],
                'selected': [{'id': o['id'], 'key_image': o['key_image'], 'amount': o['utxo']['amount']}
                             for o in selected]}


class PrefundBase(unittest.TestCase):
    PLAN = 'testnet-discovery-v2.json'

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.now = 1_000_000.0
        plan = json.loads(Path(__file__).with_name(self.PLAN).read_text())
        plan['endpoints'] = [t['rpc_url'] for t in targets()]
        self.plan = plan
        def private(name, data):
            path = self.root / name
            path.write_bytes(data)
            path.chmod(0o600)
            return str(path)
        plan_bytes = json.dumps(plan).encode()
        wallets = [{'address': 'addr-%d' % i, 'key': private('wallet-%d.mnemonic' % i, b'w%d' % i)}
                   for i in range(plan['wallets']['count'])]
        funding = {'address': 'addr-funding', 'key': private('producer.mnemonic', b'producer')}
        end = self.now + 400 * 3600
        self.m = {'schema_version': 1, 'plan_file': private('plan.json', plan_bytes),
                  'plan_sha256': hashlib.sha256(plan_bytes).hexdigest(), 'targets': targets(),
                  'tls_ca_file': private('ca.pem', b'pinned test CA'),
                  'tls_ca_sha256': hashlib.sha256(b'pinned test CA').hexdigest(),
                  'signer': private('botho-stress-wallet', b'signer'),
                  'signer_sha256': hashlib.sha256(b'signer').hexdigest(), 'producer_role': PRODUCER,
                  'funding_wallet': funding, 'wallets': wallets, 'grants_per_wallet': 16,
                  'grant_picocredits': GRANT, 'state_dir': str(self.root / 'state'),
                  'deadline': latest_deadline(plan, end), 'infrastructure_end': end}
        self.fleet = Fleet(plan, {w['key']: w['address'] for w in [*wallets, funding]})
        self.fleet.coinbase_to = funding['address']
        # 36 large producer outputs: enough for 512 grants plus worst-case fees,
        # each covering one full 15-grant batch (#1937).
        for _ in range(36):
            self.fleet.output(1, funding['address'], 16 * 10**11, coinbase=True)
        self.fleet.mine(11)

    def manifest_file(self, m=None):
        path = self.root / 'manifest.json'
        path.write_text(json.dumps(self.m if m is None else m))
        path.chmod(0o600)
        return path

    def prefunder(self):
        m, plan = load_manifest(self.manifest_file())
        p = Prefunder(m, plan, clock=lambda: self.now, rpc=self.fleet, native=self.fleet.signer)
        self.addCleanup(p.j.db.close)
        return p

    def tick(self, p):
        return asyncio.run(p.tick())

    def records(self, p):
        return len(p.records)

    def advance(self, blocks=1):
        self.fleet.mine(blocks)
        self.now += 61 * blocks

    def offer_one(self, p):
        """Ticks until one new grant is submitted."""
        before = len(self.fleet.submits)
        for _ in range(30):
            self.tick(p)
            if len(self.fleet.submits) > before:
                return
            self.advance()
        self.fail('no grant offered')


class SignerContractTests(unittest.TestCase):
    def test_signer_fields_match_main_request_enum(self):
        source = (ROOT / 'botho-wallet/src/stress.rs').read_text()
        body = re.search(r'pub enum Request \{(.*?)\n\}', source, re.S).group(1)
        self.assertIn('deny_unknown_fields', source[:source.index('pub enum Request')][-200:])
        found = {}
        for name, fields in re.findall(r'(\w+) \{(.*?)\n    \}', body, re.S):
            snake = re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()
            found[snake] = set(re.findall(r'^\s*(\w+):', fields, re.M))
        self.assertEqual(found, {op: fields | SIGNER_OPTIONAL.get(op, set())
                                 for op, fields in SIGNER_FIELDS.items()})

    def test_campaign_transfers_keep_the_single_recipient_signer_path(self):
        # #1937 batches are prefund-only: the controller still signs every
        # campaign transfer with main's one-recipient `prepare` and `inspect`.
        source = (ROOT / 'scripts/stress/controller.py').read_text()
        self.assertNotIn('_grants', ''.join(re.findall(r"'operation':'(\w+)'", source)))
        self.assertIn("'operation':'prepare'", source)
        self.assertIn("'operation':'inspect'", source)
        self.assertEqual(SIGNER_FIELDS['prepare'] - SIGNER_FIELDS['prepare_grants'],
                         {'recipient', 'amount', 'fee_multiplier'})
        self.assertEqual(prefund.GRANTS_PER_BATCH, 15)

    def test_imports_only_tracked_harness_modules(self):
        source = Path(prefund.__file__).read_text()
        imported = set(re.findall(r'^(?:from|import) (\w+)', source, re.M))
        local = {p.stem for p in Path(prefund.__file__).parent.glob('*.py')}
        self.assertLessEqual(imported & local, {'controller', 'fee_quote', 'lottery', 'plan', 'runtime', 'selection'})
        self.assertFalse({'qualification', 'fast_timing', 'manifest_limits'} & imported)
        for module in imported - local:
            __import__(module)  # standard library only


class FeeQuoteTests(PrefundBase):
    """#2001: every grant batch is priced against the producer's fee floor."""

    def test_underpaying_draft_is_moved_aside_and_resigned_at_the_quote(self):
        floor = 5 * 10**8  # under the plan's max_single_signed_fee

        def quote(fee):
            # The producer's 10.0.0 floor (#1685/#1687) is far above the
            # signer's local estimate until a re-sign carries it.
            return {'minimumFee': floor, 'consensusFeeFloor': floor, 'relayFeeFloor': floor,
                    'settleFee': floor + floor // 64, 'resignFee': floor + 2 * (floor // 64),
                    'txFee': fee, 'feeSettles': fee >= floor + floor // 64}
        self.fleet.quote_hook = quote
        p = self.prefunder()
        self.offer_one(p)
        row = p.records[0]
        resign = floor + 2 * (floor // 64)
        self.assertEqual(row['fee'], resign)
        self.assertEqual(self.fleet.quotes[:2], [self.fleet.batch_fees[0], resign])
        self.assertEqual(len(row['underpaid_drafts']), 1)
        self.assertEqual(row['underpaid_drafts'][0]['fee'], self.fleet.batch_fees[0])
        self.assertEqual(row['fee_floor_quote']['settleFee'], floor + floor // 64)
        state = Path(self.m['state_dir'])
        # The underpaid draft is kept for audit, outside artifacts/, and was
        # never submitted; only the settled batch was.
        self.assertEqual(sorted(x.name for x in (state / 'underpaid').iterdir()), ['batch-0000.bin'])
        self.assertEqual(self.fleet.submits, [row['hash']])
        # A restart finds no orphan signed artifact.
        self.assertEqual(len(self.prefunder().records), 1)

    def test_a_floor_above_the_single_fee_limit_holds(self):
        floor = int(self.plan['limits']['max_single_signed_fee_picocredits']) + 1

        def quote(fee):
            return {'minimumFee': floor, 'consensusFeeFloor': floor, 'relayFeeFloor': floor,
                    'settleFee': floor, 'resignFee': floor, 'txFee': fee}
        self.fleet.quote_hook = quote
        p = self.prefunder()
        with self.assertRaisesRegex(Gate, 'exceeds max_single_signed_fee'):
            for _ in range(30):
                self.tick(p)
                self.advance()
        self.assertFalse(self.fleet.submits)


class ManifestTests(PrefundBase):
    def refused(self, message, **changes):
        m = {**self.m, **changes}
        with self.assertRaisesRegex(Gate, message):
            load_manifest(self.manifest_file(m))

    def test_valid_manifest_loads(self):
        m, plan = load_manifest(self.manifest_file())
        self.assertEqual(plan['wallets']['count'], 32)
        self.assertEqual(m['grants_per_wallet'], 16)

    def test_refusals(self):
        self.refused('below min_mature', grants_per_wallet=15)
        self.refused('largest transfer plus', grant_picocredits=10**10 + 10**9 - 1)
        self.refused('max_principal', grants_per_wallet=64)
        self.refused('72-hour campaign', deadline=self.m['deadline'] + 1)
        self.refused('differs from launch pin', plan_sha256='0' * 64)
        self.refused('wallet count', wallets=self.m['wallets'][:31])
        self.refused('exactly the documented fields', extra=1)

    def test_deadline_rule_arithmetic(self):
        end = self.m['infrastructure_end']
        self.assertEqual(latest_deadline(self.plan, end), end - 72 * 3600 - 1800 - 1200 - 300 - 3600)

    def test_non_owner_only_key_file_is_refused(self):
        os.chmod(self.m['wallets'][3]['key'], 0o644)
        with self.assertRaisesRegex(Gate, 'owner-only'):
            load_manifest(self.manifest_file())
        os.chmod(self.m['wallets'][3]['key'], 0o600)
        os.chmod(self.m['funding_wallet']['key'], 0o640)
        with self.assertRaisesRegex(Gate, 'owner-only'):
            load_manifest(self.manifest_file())

    def test_manifest_itself_must_be_owner_only(self):
        path = self.manifest_file()
        path.chmod(0o644)
        with self.assertRaisesRegex(Gate, 'owner-only'):
            load_manifest(path)


class OpeningTests(PrefundBase):
    def assertOpeningRefused(self, message):
        p = self.prefunder()
        with self.assertRaisesRegex(Gate, message):
            self.tick(p)
        self.assertFalse(self.fleet.submits)
        self.assertFalse((Path(self.m['state_dir']) / 'opening.json').exists())

    def test_campaign_wallet_with_outputs(self):
        self.fleet.output(self.fleet.height, 'addr-7', 5)
        self.assertOpeningRefused('wallet-8')

    def test_unsynced_node(self):
        self.fleet.status_override[HOSTS[4]] = {'synced': False}
        self.assertOpeningRefused('unsynced at opening')

    def test_v1_lottery_node(self):
        self.fleet.chain_info_override[HOSTS[2]] = {'lotteryRules': 1}
        self.assertOpeningRefused('V2 lottery rules')

    def test_pre_7_protocol_node(self):
        self.fleet.chain_info_override[HOSTS[0]] = {'protocolVersion': '6.0.0'}
        self.assertOpeningRefused('protocol is not ' + re.escape(PROTOCOL_VERSION))

    def test_wrong_commit(self):
        self.fleet.status_override[HOSTS[1]] = {'gitCommit': 'f' * 40}
        self.assertOpeningRefused('software identity')

    def test_minting_on_non_producer(self):
        self.fleet.status_override[HOSTS[0]] = {'mintingActive': True}
        self.assertOpeningRefused('only on the producer')

    def quiet_tick(self, p):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            done = self.tick(p)
        return done, out.getvalue()

    def waits(self, p):
        return [json.loads(r[0]) for r in
                p.j.db.execute("SELECT detail FROM events WHERE state='prefund_funding_wait'")]

    def test_insufficient_funding_balance_waits_then_holds_at_deadline(self):
        self.m['grants_per_wallet'] = 32  # 1024 grants needs more than the 64 x 9e11 opening.
        p = self.prefunder()
        opening = Path(self.m['state_dir']) / 'opening.json'
        done, out = self.quiet_tick(p)
        self.assertFalse(done)
        self.assertIn('waiting for the funding wallet to mature', out)
        (wait,) = self.waits(p)
        self.assertLess(wait['spendable'], wait['needed'])
        self.assertEqual(wait['height'], self.fleet.height)
        self.assertFalse(opening.exists())
        # Within the wait interval nothing is rescanned or journalled.
        self.now += prefund.FUNDING_WAIT_SECONDS - 1
        self.assertEqual(self.quiet_tick(p), (False, ''))
        self.assertEqual(len(self.waits(p)), 1)
        self.now = self.m['deadline']
        with self.assertRaisesRegex(Gate, 'cannot cover principal plus worst-case fees by the prefund deadline'):
            self.quiet_tick(p)
        self.assertFalse(self.fleet.submits)
        self.assertFalse(opening.exists())

    def test_opening_waits_for_funding_to_mature_then_proceeds(self):
        self.m['grants_per_wallet'] = 32
        p = self.prefunder()
        self.assertEqual(self.quiet_tick(p)[0], False)
        self.fleet.mine(1)  # A new block, never one the first rescan already saw.
        for _ in range(64):  # The producer keeps minting; the new outputs mature.
            self.fleet.output(self.fleet.height, 'addr-funding', 9 * 10**11, coinbase=True)
        self.advance(11)
        self.now += prefund.FUNDING_WAIT_SECONDS
        self.quiet_tick(p)
        opening = json.loads((Path(self.m['state_dir']) / 'opening.json').read_text())
        self.assertGreaterEqual(opening['spendable'], self.waits(p)[0]['needed'])
        self.assertEqual(len(self.waits(p)), 1)


class HostsTests(PrefundBase):
    """`--hosts`: the fleet *.internal names resolve only through the bound hosts file (#1610)."""

    def hosts_file(self, text):
        path = self.root / 'hosts'
        path.write_text(text)
        return path

    def fleet_hosts(self):
        return ''.join('10.0.0.%d %s\n' % (i + 11, name) for i, name in enumerate(prefund.target_names(self.m)))

    def test_check_hosts_and_unit_properties(self):
        path = self.hosts_file('# fleet\n' + self.fleet_hosts() + '10.0.0.10 controller.internal\n')
        addresses = prefund.check_hosts(self.m, path)
        self.assertEqual(list(addresses), prefund.target_names(self.m))
        props = prefund.unit_properties(self.m, path, addresses)
        self.assertEqual(props[0], 'BindReadOnlyPaths=%s:/etc/hosts' % path.resolve())
        self.assertIn('ReadWritePaths=' + self.m['state_dir'], props)
        self.assertIn('IPAddressDeny=any', props)
        self.assertEqual([p for p in props if p.startswith('IPAddressAllow=10.')],
                         ['IPAddressAllow=10.0.0.%d/32' % (i + 11) for i in range(5)])

    def test_check_hosts_refusals(self):
        names = prefund.target_names(self.m)
        for text, needle in (('10.0.0.11 %s\n' % names[0], 'does not resolve target'),
                             (self.fleet_hosts().replace('10.0.0.11', '8.8.8.8'), 'private addresses'),
                             (self.fleet_hosts() + '10.0.0.99 %s\n' % names[2], 'duplicate'),
                             ('not-an-ip %s\n' % names[0], 'malformed')):
            with self.subTest(needle=needle), self.assertRaisesRegex(Gate, needle):
                prefund.check_hosts(self.m, self.hosts_file(text))

    def test_run_refuses_unresolvable_or_misresolved_targets(self):
        import socket
        def missing(name, port, proto=0):
            raise socket.gaierror('nodename nor servname provided')
        with self.assertRaisesRegex(Gate, 'bound over /etc/hosts'):
            prefund.check_resolution(self.m, resolve=missing)
        addresses = prefund.check_hosts(self.m, self.hosts_file(self.fleet_hosts()))
        def right(name, port, proto=0):
            return [(2, 1, 6, '', (addresses[name], port))]
        prefund.check_resolution(self.m, addresses, resolve=right)
        prefund.check_resolution(self.m, resolve=right)
        def wrong(name, port, proto=0):
            return [(2, 1, 6, '', ('192.168.9.9', port))]
        with self.assertRaisesRegex(Gate, 'not the hosts-file address'):
            prefund.check_resolution(self.m, addresses, resolve=wrong)


class EndToEndTests(PrefundBase):
    def test_v2_award_during_prefund_is_income_not_principal(self):
        """A campaign wallet wins a V2 award mid-prefund (#1643).

        The pin stays exact principal, so the launch check and the principal
        budget never count lottery inflow; the award is reported apart.
        """
        p = self.prefunder()
        winner_address = self.fleet.addresses[self.m['wallets'][0]['key']]
        award = None
        done = False
        for _ in range(400):
            for _ in range(40):
                before = len(self.fleet.submits)
                done = self.tick(p)
                if done or len(self.fleet.submits) == before:
                    break
            if done:
                break
            self.advance()
            owned = [(tx, i) for (tx, i), meta in self.fleet.outputs.items()
                     if meta['owner'] == winner_address and not meta['lottery']]
            if award is None and owned:
                tx, index = owned[0]
                award = self.fleet.output(self.fleet.height, winner_address, 7 * 10**9, index=1, lottery=True)
                self.fleet.blocks[self.fleet.height][-1].update(
                    amountCommitment=(7 * 10**9).to_bytes(8, 'little').hex(), cryptoOutputIndex=0,
                    lotteryContext={'version': 2, 'baseIndex': 0, 'tweak': '03' + '00' * 31,
                                    'ordinal': 0, 'height': self.fleet.height},
                    winnerOutpoint=tx + ':' + str(index))
        self.assertTrue(done)
        self.assertIsNotNone(award)
        state = Path(self.m['state_dir'])
        self.assertEqual(json.loads((state / 'opening_balances.json').read_text()), [16 * GRANT] * 32)
        health = json.loads((state / 'funding-health.json').read_text())
        self.assertEqual(health['opening_lottery_balances'], [7 * 10**9] + [0] * 31)
        self.assertEqual(health['accounting']['difference'], 0)
        self.assertEqual(health['accounting']['lottery_v2']['received_count'], 1)
        self.assertEqual(health['accounting']['lottery_v2']['undiscovered'], [])
        self.assertEqual([w['grant_outputs'] for w in health['inventory_readiness']['wallets']], [16] * 32)

    def test_prefund_writes_exact_opening_balances(self):
        # Batches finish in a few blocks: mint often enough that coinbase
        # inflow lands mid-prefund and is accounted for.
        self.fleet.coinbase_every = 3
        p = self.prefunder()
        done = False
        for _ in range(400):
            for _ in range(40):
                before = len(self.fleet.submits)
                done = self.tick(p)
                if done or len(self.fleet.submits) == before:
                    break
            if done:
                break
            self.advance()
        self.assertTrue(done)
        state = Path(self.m['state_dir'])
        balances = json.loads((state / 'opening_balances.json').read_text())
        self.assertEqual(balances, [16 * GRANT] * 32)
        # Every grant batch was submitted exactly once, to the producer only:
        # 34 full batches of 15 and one of 2 (#1937).
        self.assertEqual(len(self.fleet.submits), 35)
        self.assertEqual(len(set(self.fleet.submits)), 35)
        self.assertEqual([len(r['grants']) for r in p.records], [15] * 34 + [2])
        self.assertEqual([g['index'] for r in p.records for g in r['grants']], list(range(512)))
        for row in p.records:
            # Grant i of a batch is output i; change (if any) is the next one.
            self.assertEqual([g['output_index'] for g in row['grants']], list(range(len(row['grants']))))
            self.assertIn(len(row['outputs']), (len(row['grants']), len(row['grants']) + 1))
        # The prefunder only ever signs batches; main's campaign `prepare` and
        # `inspect` are left to the controller, unchanged.
        operations = {r[1] for r in self.fleet.requests if r[0] == 'signer'}
        self.assertLessEqual({'prepare_grants', 'inspect_grants'}, operations)
        self.assertFalse({'prepare', 'inspect'} & operations)
        # Equals the scanned per-wallet unspent sums.
        scanned = []
        for wallet in self.m['wallets']:
            _, owned = self.fleet.scan(wallet['key'], [{'height': h, 'outputs': rows}
                                                         for h, rows in sorted(self.fleet.blocks.items())])
            scanned.append(sum(o['utxo']['amount'] for o in owned if o['key_image'] not in self.fleet.spent))
        self.assertEqual(balances, scanned)
        health = json.loads((state / 'funding-health.json').read_text())
        self.assertEqual(health['accounting']['difference'], 0)
        self.assertEqual(health['grants'], 512)
        self.assertEqual(health['batches'], 35)
        self.assertEqual(health['fees'], sum(self.fleet.batch_fees))
        self.assertGreater(health['accounting']['coinbase_lottery_inflow'], 0)
        self.assertEqual([w['grant_outputs'] for w in health['inventory_readiness']['wallets']], [16] * 32)
        # The tracked controller and staging accept the output as launch opening_balances.
        self.assertDiscoveryAccepts(balances)
        self.assertTrue(self.tick(p))  # a finished run stays finished
        self.assertEqual(len(self.fleet.submits), 35)

    def assertDiscoveryAccepts(self, balances):
        import discovery
        state = self.root / 'controller'
        state.mkdir()
        (state / 'plan.json').write_text(json.dumps(self.plan))
        launch = {'isolated_discovery': True, 'targets': targets(), 'setup_start': 1000,
                  'infrastructure_end': 1000 + 81 * 3600}
        with patch('controller.Controller.__init__', lambda self, state: None):
            for value, ok in ((balances, True), (balances[:-1], False), ([0] + balances[1:], False),
                              ([10**15] * 32, False)):
                (state / 'launch.json').write_text(json.dumps({**launch, 'opening_balances': value}))
                if ok:
                    discovery.DiscoveryController(state)
                else:
                    with self.assertRaisesRegex(Gate, 'opening'):
                        discovery.DiscoveryController(state)


class ResumeTests(PrefundBase):
    def crash_on_submit_call(self, p):
        async def crash(row):
            raise Crash()
        p.submit = crash

    def test_record_without_marker_submits_once_when_all_nodes_unknown(self):
        p = self.prefunder()
        self.crash_on_submit_call(p)
        with self.assertRaises(Crash):
            self.tick(p)
        state = Path(self.m['state_dir'])
        self.assertTrue((state / 'grants/batch-0000.json').exists())
        self.assertFalse((state / 'grants/batch-0000.submit').exists())
        self.assertFalse(self.fleet.submits)
        p = self.prefunder()
        self.tick(p)
        self.assertEqual(self.fleet.submits[0], p.records[0]['hash'])
        self.assertEqual(self.fleet.submits.count(p.records[0]['hash']), 1)
        self.assertTrue((state / 'grants/batch-0000.submit').exists())
        self.assertEqual(self.prefunder().records[0]['state'], 'accepted')

    def test_record_without_marker_holds_when_any_node_knows_hash(self):
        p = self.prefunder()
        self.crash_on_submit_call(p)
        with self.assertRaises(Crash):
            self.tick(p)
        h = p.records[0]['hash']
        self.fleet.known_override[(HOSTS[3], h)] = 'pending'
        p = self.prefunder()
        with self.assertRaisesRegex(Gate, 'unexpectedly known'):
            self.tick(p)
        self.assertFalse(self.fleet.submits)

    def crash_after_marker(self, dispatched):
        def hook(tx):
            if dispatched:
                self.fleet.mempool[tx['hash']] = tx
            raise Crash()
        self.fleet.submit_hook = hook
        p = self.prefunder()
        with self.assertRaises(Crash):
            self.tick(p)
        self.fleet.submit_hook = None
        state = Path(self.m['state_dir'])
        self.assertTrue((state / 'grants/batch-0000.submit').exists())
        return p.records[0]['hash']

    def test_crash_after_marker_never_resubmits(self):
        h = self.crash_after_marker(dispatched=False)
        p = self.prefunder()
        self.tick(p)
        self.assertEqual(self.fleet.submits, [h])
        self.assertEqual(len(p.records), 1)  # nothing new while uncertain
        self.advance(5)
        self.tick(p)
        self.assertEqual(self.fleet.submits, [h])
        self.now += 900
        with self.assertRaisesRegex(Gate, 'never resubmitted'):
            self.tick(p)
        with self.assertRaisesRegex(Gate, 'never resubmitted'):
            self.tick(self.prefunder())
        self.assertEqual(self.fleet.submits, [h])

    def test_crash_after_dispatched_marker_confirms_without_resubmit(self):
        h = self.crash_after_marker(dispatched=True)
        self.advance()
        p = self.prefunder()
        self.tick(p)
        self.assertEqual(p.records[0]['state'], 'confirmed')
        self.assertEqual(self.fleet.submits.count(h), 1)

    def test_uncertain_reply_holds_as_unknown(self):
        def hook(tx):
            raise OSError('connection reset')
        self.fleet.submit_hook = hook
        p = self.prefunder()
        with self.assertRaisesRegex(Gate, 'uncertain'):
            self.tick(p)
        self.assertEqual(p.records[0]['state'], 'unknown')
        self.fleet.submit_hook = None
        self.tick(self.prefunder())
        self.assertEqual(len(self.fleet.submits), 1)

    def test_orphan_artifact_holds(self):
        p = self.prefunder()
        self.offer_one(p)
        (Path(self.m['state_dir']) / 'artifacts/batch-0001.bin').write_bytes(b'signed, never recorded')
        with self.assertRaisesRegex(Gate, 'orphan signed artifact'):
            self.prefunder()

    def test_altered_artifact_holds(self):
        p = self.prefunder()
        self.offer_one(p)
        Path(p.records[0]['artifact']).write_bytes(b'changed')
        with self.assertRaisesRegex(Gate, 'altered'):
            self.prefunder()

    def test_persisted_inventory_is_rescanned(self):
        p = self.prefunder()
        self.offer_one(p)
        state = Path(self.m['state_dir'])
        (state / 'inventory.json').write_text(json.dumps({'owned': [[{'id': 'forged'}]] * 33, 'spent': []}))
        (state / 'blocks.json').write_text('[]')
        self.fleet.requests.clear()
        p = self.prefunder()
        self.assertFalse(p.inventory_initialized)
        self.tick(p)
        self.assertIn(('signer', 'restore_check'), self.fleet.requests)
        self.assertNotIn('forged', [o['id'] for owned in p.inventory for o in owned])
        self.assertTrue(p.inventory[p.funding])


class BatchTests(PrefundBase):
    """Grant batches (#1937): exact-once per batch and fixed grant coverage."""

    def test_uncertain_batch_submit_never_resubmits_and_confirms_every_grant(self):
        def hook(tx):
            self.fleet.mempool[tx['hash']] = tx  # It reached the producer...
            raise OSError('connection reset')    # ...but the reply was lost.
        self.fleet.submit_hook = hook
        p = self.prefunder()
        with self.assertRaisesRegex(Gate, 'uncertain; never resubmitted'):
            self.tick(p)
        self.fleet.submit_hook = None
        row = p.records[0]
        self.assertEqual((row['state'], len(row['grants'])), ('unknown', 15))
        h = row['hash']
        # A restart re-signs nothing and sends nothing until the batch resolves.
        p = self.prefunder()
        self.tick(p)
        self.assertEqual((len(p.records), self.fleet.submits), (1, [h]))
        self.advance()
        self.tick(p)
        self.assertEqual(p.records[0]['state'], 'confirmed')
        # Every one of the batch's grants reached its wallet as output i.
        for grant in p.records[0]['grants']:
            found = [o for o in p.inventory[grant['recipient']] if o['id'] == '%s:%d' % (h, grant['output_index'])]
            self.assertEqual([o['utxo']['amount'] for o in found], [GRANT])
        self.assertEqual(self.fleet.submits.count(h), 1)
        self.assertEqual(p.j.get('accounting')['difference'], 0)

    def test_restart_mid_run_continues_coverage_exactly_once(self):
        p = self.prefunder()
        self.offer_one(p)
        self.offer_one(p)
        q = self.prefunder()
        self.assertEqual([g['index'] for r in q.records for g in r['grants']], list(range(30)))
        self.offer_one(q)
        self.assertEqual(q.records[2]['grants'][0]['index'], 30)
        self.assertEqual(q.records[2]['grants'][0]['recipient'], 30 % 32)
        self.assertEqual(len(set(self.fleet.submits)), 3)

    def test_altered_batch_coverage_holds(self):
        p = self.prefunder()
        self.offer_one(p)
        path = p.record_path(0)
        for change in ({'recipient': 5}, {'amount': GRANT + 1}, {'output_index': 3}, {'index': 1}):
            row = json.loads(path.read_text())
            row['grants'][0] = {**row['grants'][0], **change}
            path.write_text(json.dumps(row))
            with self.subTest(change=change), self.assertRaisesRegex(Gate, 'grant batch coverage changed'):
                self.prefunder()
        row['grants'] = p.records[0]['grants'][:14]
        path.write_text(json.dumps(row))
        with self.assertRaisesRegex(Gate, 'grant batch coverage changed'):
            self.prefunder()

    def test_per_grant_state_dir_from_before_batching_holds(self):
        state = Path(self.m['state_dir'])
        (state / 'grants').mkdir(mode=0o700, parents=True)
        state.chmod(0o700)
        (state / 'grants' / 'grant-0000.json').write_text('{}')
        with self.assertRaisesRegex(Gate, 'pre-batch prefunder'):
            self.prefunder()


class DeadlineFitTests(PrefundBase):
    """x-series on a fresh protocol-9.0.0 chain (#1937).

    One coinbase per 40 s block is the only other decoy source, so a funding
    input is spendable only about 220 blocks after the chain starts (the
    #1809 random-offset window needs 19 decoys in its thinnest window, and
    the rehearsal saw the first grant at height 221). One input per grant
    then needs 2048 eligible inputs, about 24 h at one per block; batches
    need 137. Since #2023 the plan pins 40 grants of 2e10 per wallet (2560
    grants, 171 batches) so a payment moves little value under the 10.0.0
    exit charge.
    """
    PLAN = 'x-series-plan.json'
    BLOCK_SECONDS = 40
    X2_GRANT = 2 * 10**10
    X2_GRANTS = 40
    TOTAL = 64 * X2_GRANTS
    BATCHES = -(-TOTAL // 15)

    def run_fresh_chain(self, tick_seconds):
        """Prefund the plan's grants from height 0; returns the prefunder and first/last batch heights."""
        self.fleet = Fleet(self.plan, self.fleet.addresses)
        self.fleet.coinbase_to = self.m['funding_wallet']['address']
        self.fleet.fillers, self.fleet.coinbase_every, self.fleet.coinbase_amount = 0, 1, 5 * 10**13
        self.assertEqual(self.plan['prefund'], {'grants_per_wallet': self.X2_GRANTS,
                         'grant_picocredits': str(self.X2_GRANT), 'max_batch_fee_picocredits': '100000000000'})
        self.m.update(grants_per_wallet=self.X2_GRANTS, grant_picocredits=self.X2_GRANT,
                      deadline=self.now + 6 * 3600)
        p = self.prefunder()
        self.assertEqual(self.plan['wallets']['count'] * self.m['grants_per_wallet'], self.TOTAL)
        done = False
        with contextlib.redirect_stdout(io.StringIO()):  # funding-wait lines
            while not done:
                self.fleet.mine(1)
                for _ in range(self.BLOCK_SECONDS // tick_seconds):
                    self.now += tick_seconds
                    try:
                        done = self.tick(p)
                    except Quota:
                        continue
                    if done:
                        break
        self.assertLess(self.now, self.m['deadline'])
        heights = [r['block_height'] for r in p.records]
        return p, heights[0], heights[-1]

    def test_x_series_fits_its_window_at_one_batch_per_block(self):
        # One tick per block: at most one new batch per block, the issue's
        # "one eligible input per block" pace, and the sparsest decoys.
        p, first, last = self.run_fresh_chain(self.BLOCK_SECONDS)
        count = self.plan['wallets']['count']
        self.assertGreater(first, 200)  # Decoy-starved until then (#1809).
        self.assertEqual((p.granted(), len(p.records)), (self.TOTAL, self.BATCHES))  # ceil(2560 / 15)
        self.assertEqual(len(set(self.fleet.submits)), self.BATCHES)
        self.assertLessEqual(last - first, self.BATCHES)
        # 6 h is 540 blocks; one grant per transaction would need ~2560 more.
        self.assertLess(last * self.BLOCK_SECONDS, 6 * 3600)
        self.assertLessEqual(max(r['fee'] for r in p.records), prefund.batch_fee_cap(self.plan))
        state = Path(self.m['state_dir'])
        self.assertEqual(json.loads((state / 'opening_balances.json').read_text()), [self.X2_GRANTS * self.X2_GRANT] * count)
        health = json.loads((state / 'funding-health.json').read_text())
        self.assertEqual(health['accounting']['difference'], 0)
        self.assertEqual(health['grants'], self.TOTAL)
        self.assertEqual([w['grant_outputs'] for w in health['inventory_readiness']['wallets']],
                         [self.X2_GRANTS] * count)
        # Activation. latest_deadline() reserves MATURITY_SECONDS for output
        # maturity, staging and activation. At the earliest activation, one
        # such slack after completion with the chain back to one coinbase per
        # block, DiscoveryController.inventory_ready()'s exact probe holds for
        # every wallet: min_mature_utxos_per_wallet eligible with
        # decoy_headroom, and a four-input largest-transfer selection. A
        # batch's 16 outputs are decoys for each other, so every grant output
        # sits in a block of at least 17 decoy-eligible outputs.
        completed = self.fleet.height
        self.fleet.mine(prefund.MATURITY_SECONDS // self.BLOCK_SECONDS)
        self.now += prefund.MATURITY_SECONDS
        asyncio.run(p.health(force=True))
        height = asyncio.run(p.sync(full=True))
        self.assertEqual(height, completed + 90)
        wallets = self.plan['wallets']
        largest = max(map(int, wallets['transfer_amounts_picocredits']))
        for w in range(count):
            ready, evidence = choose(p, w, height, headroom=wallets['decoy_headroom'])
            probe, _ = choose(p, w, height, largest, 4, wallets['decoy_headroom'])
            self.assertGreaterEqual(len(ready), wallets['min_mature_utxos_per_wallet'], evidence)
            self.assertTrue(probe, w)

    def test_x_series_change_recycling_finishes_sooner(self):
        # Ticking every 4 s, a batch's change (in a 16-output block) is
        # itself spendable ~20 blocks later, so several batches land per block.
        p, first, last = self.run_fresh_chain(4)
        self.assertEqual((p.granted(), len(p.records)), (self.TOTAL, self.BATCHES))
        self.assertLess(last - first, self.BATCHES)
        self.assertEqual(json.loads((Path(self.m['state_dir']) / 'funding-health.json').read_text())
                         ['accounting']['difference'], 0)


class PinnedShapeTests(PrefundBase):
    """#2023: an x-series plan pins the prefund shape it was fee-checked for."""
    PLAN = 'x-series-plan.json'

    def test_manifest_must_match_the_plan_prefund_block(self):
        pinned = self.plan['prefund']
        with self.assertRaisesRegex(Gate, 'plan prefund block'):
            load_manifest(self.manifest_file())  # 16 grants of 1e11
        m = {**self.m, 'grants_per_wallet': pinned['grants_per_wallet'],
             'grant_picocredits': int(pinned['grant_picocredits'])}
        with self.assertRaisesRegex(Gate, 'plan prefund block'):
            load_manifest(self.manifest_file({**m, 'grants_per_wallet': pinned['grants_per_wallet'] + 1}))
        _, plan = load_manifest(self.manifest_file(m))
        # Batches are capped by the plan's batch cap, not the campaign's max_single.
        self.assertEqual(prefund.batch_fee_cap(plan), int(pinned['max_batch_fee_picocredits']))
        self.assertGreater(prefund.batch_fee_cap(plan),
                           int(plan['limits']['max_single_signed_fee_picocredits']))

    def test_plans_without_the_block_cap_batches_at_max_single(self):
        plan = json.loads(Path(__file__).with_name('testnet-discovery-v2.json').read_text())
        self.assertEqual(prefund.batch_fee_cap(plan),
                         int(plan['limits']['max_single_signed_fee_picocredits']))


class ReconcileTests(PrefundBase):
    def test_confirmed_grant_inputs_read_spent_past_the_spent_query_bound(self):
        """#1719 review: more images than SPENT_AUDIT_IMAGES; grants are not journal intents."""
        # Small producer outputs in an earlier block sort ahead of the large
        # ones grants select: ten audit slices of them, so the rotation alone
        # cannot reach a grant's inputs before it confirms.
        funding = self.m['funding_wallet']['address']
        for _ in range(10 * SPENT_AUDIT_IMAGES):
            self.fleet.output(0, funding, 10**6, coinbase=True)
        p = self.prefunder()
        self.offer_one(p)
        self.assertGreater(len(p.spent), SPENT_AUDIT_IMAGES)
        self.advance()
        self.tick(p)  # confirms the grant, then a routine sync and accounting
        self.assertEqual(p.records[0]['state'], 'confirmed')
        state = {s['keyImage']: s for s in p.spent}
        inputs = {o['key_image'] for o in p.inventory[p.funding]
                  if o['id'] in {s['id'] for s in p.records[0]['selected']}}
        self.assertTrue(all(state[image]['spent'] for image in inputs))
        self.offer_one(p)
        selected = [s['id'] for r in p.records for s in r['selected']]
        self.assertGreater(len(p.records), 1)
        self.assertEqual(len(selected), len(set(selected)))  # no spent input reselected

    def confirmed_image_reads_pending(self):
        """After one confirmed grant, a lagging node reports a funding image pending."""
        p = self.prefunder()
        self.offer_one(p)
        self.advance()
        self.tick(p)
        self.assertEqual(p.records[0]['state'], 'confirmed')
        selected = {s['id'] for r in p.records for s in r['selected']}
        image = next(o['key_image'] for o in p.unspent(p.funding) if o['id'] not in selected)
        self.fleet.pending_override.add(image)
        return p

    def test_transient_pending_image_waits_then_proceeds(self):
        """#1956: a mined spend that still reads pending is a wait, not a hold."""
        p = self.confirmed_image_reads_pending()
        submits = len(self.fleet.submits)
        for _ in range(3):
            self.advance()  # confirms the batch offered meanwhile; then only the image blocks
            with self.assertRaisesRegex(Quota, 'still pending'):
                self.tick(p)
        self.assertEqual(len(self.fleet.submits), submits)  # nothing new offered while waiting
        self.fleet.pending_override.clear()
        self.offer_one(p)
        self.assertGreater(len(self.fleet.submits), submits)
        self.assertIsNone(p.pending_since)

    def test_transient_pending_image_at_completion_waits(self):
        """#1956 review: the final accounting in complete() waits the same way."""
        p = self.prefunder()
        for _ in range(400):
            for _ in range(40):
                before = len(self.fleet.submits)
                self.assertFalse(self.tick(p))
                if len(self.fleet.submits) == before:
                    break
            if p.granted() == p.total:
                break
            self.advance()
        self.assertEqual(p.granted(), p.total)
        self.assertTrue(p.unconfirmed())  # the last batch is still in flight
        selected = {s['id'] for r in p.records for s in r['selected']}
        spare = next(o['key_image'] for o in p.unspent(p.funding) if o['id'] not in selected)
        self.fleet.pending_override.add(spare)
        self.advance()
        with self.assertRaisesRegex(Quota, 'still pending'):
            self.tick(p)
        self.assertFalse((Path(self.m['state_dir']) / 'opening_balances.json').exists())
        self.fleet.pending_override.clear()
        self.now += 30
        self.assertTrue(self.tick(p))
        self.assertTrue((Path(self.m['state_dir']) / 'opening_balances.json').exists())

    def test_persistent_pending_image_holds(self):
        p = self.confirmed_image_reads_pending()
        self.advance()
        with self.assertRaises(Quota):
            self.tick(p)
        self.now += prefund.PENDING_SECONDS + 1
        with self.assertRaisesRegex(Gate, 'pending for ten minutes'):
            self.tick(p)

    def test_receipt_disagreement_holds(self):
        p = self.prefunder()
        self.offer_one(p)
        self.fleet.receipt_override[HOSTS[1]] = {'fee': 1}
        self.advance()
        with self.assertRaisesRegex(Gate, 'receipt disagreement'):
            self.tick(p)

    def test_funding_accounting_mismatch_holds(self):
        p = self.prefunder()
        self.offer_one(p)
        self.advance()
        self.tick(p)
        self.assertEqual(p.records[0]['state'], 'confirmed')
        # Something other than a grant spends a producer output.
        spare = next(o for o in p.unspent(p.funding) if o['id'] not in
                     {s['id'] for r in p.records for s in r['selected']})
        self.fleet.spent[spare['key_image']] = self.fleet.height
        self.advance()
        with self.assertRaisesRegex(Gate, 'accounting mismatch'):
            self.tick(p)
        self.assertNotEqual(p.j.get('accounting')['difference'], 0)

    def test_external_transfer_into_campaign_wallet_holds(self):
        p = self.prefunder()
        self.offer_one(p)
        self.advance()
        self.tick(p)
        self.fleet.output(self.fleet.height + 1, 'addr-5', 7)
        self.advance()
        with self.assertRaisesRegex(Gate, 'campaign wallet-6'):
            self.tick(p)

    def test_quota_on_unsynced_node_waits_then_holds(self):
        p = self.prefunder()
        self.tick(p)
        self.fleet.status_override[HOSTS[0]] = {'synced': False}
        self.now += 20
        with self.assertRaises(Quota):
            self.tick(p)
        self.now += 121
        with self.assertRaisesRegex(Gate, 'two minutes'):
            self.tick(p)

    def test_no_new_grant_after_deadline(self):
        p = self.prefunder()
        self.offer_one(p)
        self.now = self.m['deadline']
        self.advance()
        with self.assertRaisesRegex(Gate, 'deadline'):
            self.tick(p)
        self.assertEqual(len(self.fleet.submits), 1)


class ReservingFleet:
    """The fake fleet behind main's real ``Rpc.reserve()`` request accounting.

    ``Fleet.call`` alone never touches the journal's ``requests`` table, so the
    prefunder's producer-budget checks would always see zero requests.
    """

    def __init__(self, fleet):
        self.fleet, self.journal = fleet, None

    async def call(self, host, method, params=None, timeout=10, write=False):
        Rpc.reserve(self, host, method, 0)
        return await self.fleet.call(host, method, params, timeout, write)


class QuotaDeferralTests(PrefundBase):
    def reserving(self):
        m, plan = load_manifest(self.manifest_file())
        p = Prefunder(m, plan, clock=lambda: self.now, rpc=ReservingFleet(self.fleet), native=self.fleet.signer)
        p.rpc.journal = p.j
        self.addCleanup(p.j.db.close)
        return p

    def limit(self, p):
        return min(p.j.limits['max_rpc_per_endpoint_per_minute'], p.j.get('quota:' + PRODUCER, 100) // 2)

    def used(self, p):
        return p.j.db.execute('SELECT COUNT(*) FROM requests WHERE host=? AND at>?',
                              (PRODUCER, self.now - 60)).fetchone()[0]

    def fill(self, p):
        """Other traffic exhausts the producer's 60 s budget right now."""
        missing = self.limit(p) - self.used(p)
        p.j.db.executemany('INSERT INTO requests(at,host,method,bytes) VALUES(?,?,?,?)',
                           [(self.now, PRODUCER, 'other', 0)] * max(0, missing))

    def step(self, p):
        try:
            return self.tick(p)
        except Quota:
            return None

    def assertSubmittedOnce(self, p):
        self.assertEqual(sorted(self.fleet.submits), sorted(r['hash'] for r in p.records if
                                                            p.marker_path(r['index']).exists()))
        self.assertEqual(len(self.fleet.submits), len(set(self.fleet.submits)))

    def test_judge_repro_budget_too_small_never_strands_a_grant(self):
        # The Judge's repro on PR #1596: the budget refuses the submit pre-check.
        p = self.prefunder()
        p.j.set('quota:' + PRODUCER, 2)
        with self.assertRaisesRegex(Quota, 'before signing'):
            self.tick(p)
        # The budget is now checked before signing: no signature, no record.
        self.assertEqual(p.records, [])
        self.assertFalse(list((Path(self.m['state_dir']) / 'artifacts').iterdir()))
        self.assertFalse(self.fleet.submits)
        p.j.set('quota:' + PRODUCER, 1000)
        for _ in range(20):
            self.advance()
            self.tick(p)  # Never a hold.
        self.assertTrue(self.fleet.submits)
        self.assertSubmittedOnce(p)

    def test_submit_deferred_before_marker_is_retried_exactly_once(self):
        p = self.reserving()
        original = self.fleet.signer

        async def signer(request):
            result = await original(request)
            if request['operation'] == 'inspect_grants' and not p.records:
                self.fill(p)  # A burst between signing and submission.
            return result
        p.signer = signer
        with self.assertRaisesRegex(Quota, 'before submit'):
            self.tick(p)
        row = p.records[0]
        self.assertEqual(row['state'], 'prepared')
        self.assertFalse(p.marker_path(0).exists())
        self.assertFalse(self.fleet.submits)
        # Keep the producer saturated past the fifteen-minute unresolved bound.
        for _ in range(17):
            self.now += 61
            self.fill(p)
            self.assertIsNone(self.step(p))
            self.assertFalse(self.fleet.submits)
            self.assertEqual(len(p.records), 1)  # Nothing new signed ahead of it.
        self.assertGreater(self.now - row['prepared_at'], prefund.UNRESOLVED_SECONDS)
        # Budget frees: the real request count admits it, once.
        for _ in range(6):
            self.advance()
            self.step(p)
        self.assertEqual(self.fleet.submits.count(row['hash']), 1)
        self.assertEqual(self.fleet.submits[0], row['hash'])
        self.assertEqual(p.records[0]['state'], 'confirmed')
        self.assertGreater(len(p.records), 1)  # Normal grants resumed.
        self.assertSubmittedOnce(p)
        # A restart sees the marker and never sends it again.
        q = self.reserving()
        self.step(q)
        self.assertEqual(self.fleet.submits.count(row['hash']), 1)

    def test_real_budget_pressure_defers_but_never_holds_or_resubmits(self):
        # Blocks every 60 s with ticks every 3 s: up to ~20 grants are pending at
        # once, so reconcile's five-host status polls press the producer's
        # default 50-request window through the real Rpc.reserve() count.
        p = self.reserving()
        deferrals = []

        async def fake_sleep(seconds, *args):
            self.now += max(seconds, 1)  # Controller.sync()'s quota wait.
        with patch('controller.asyncio.sleep', fake_sleep):
            for i in range(300):
                self.now += 3
                if i % 20 == 19:
                    self.fleet.mine(1)
                try:
                    self.tick(p)  # Any Gate fails the test.
                except Quota as error:
                    deferrals.append(str(error))
                self.assertLessEqual(self.used(p), self.limit(p))
        self.assertIn('producer request budget before signing', deferrals)
        self.assertIn('shared endpoint request budget', deferrals)
        self.assertGreaterEqual(p.granted([r for r in p.records if r['state'] == 'confirmed']), 50)
        self.assertSubmittedOnce(p)
        self.assertFalse(p.unsent())

    def test_retry_holds_if_unsent_hash_is_known(self):
        p = self.reserving()

        async def submit(row):
            raise Quota('deferred')
        real, p.submit = p.submit, submit
        self.step(p)
        p.submit = real
        self.fleet.known_override[(HOSTS[1], p.records[0]['hash'])] = 'pending'
        self.advance()
        with self.assertRaisesRegex(Gate, 'unexpectedly known'):
            self.tick(p)
        self.assertFalse(self.fleet.submits)


class StateDirTests(PrefundBase):
    def test_interrupted_record_update_has_its_own_hold(self):
        p = self.prefunder()
        self.offer_one(p)
        (Path(self.m['state_dir']) / 'grants/batch-0000.json.tmp').write_text('{')
        with self.assertRaisesRegex(Gate, 'interrupted grant record update'):
            self.prefunder()

    def test_existing_grants_dir_must_be_owner_only(self):
        state = Path(self.m['state_dir'])
        state.mkdir(mode=0o700)
        (state / 'grants').mkdir(mode=0o700)
        (state / 'grants').chmod(0o755)
        with self.assertRaisesRegex(Gate, 'grants must be an owner-only'):
            self.prefunder()

    def test_plan_is_read_once(self):
        reads = []
        real = Path.read_bytes

        def read_bytes(path):
            reads.append(str(path))
            return real(path)
        with patch.object(Path, 'read_bytes', read_bytes):
            load_manifest(self.manifest_file())
        self.assertEqual(reads.count(self.m['plan_file']), 1)


if __name__ == '__main__':
    unittest.main()
