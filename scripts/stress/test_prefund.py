"""Offline prefunder tests: a fake five-node fleet and a fake main signer, no network."""
import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

import prefund
from controller import SPENT_AUDIT_IMAGES
from prefund import Prefunder, SIGNER_FIELDS, latest_deadline, load_manifest
from runtime import Gate, HOSTS, Quota, Rpc
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
        self.submits, self.requests, self.submit_hook = [], [], None
        self.counter = 0
        self.coinbase_to = None

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
            for _ in range(FILLERS):
                self.output(self.height, None, 1)
            if self.coinbase_to and self.height % 25 == 0:
                self.output(self.height, self.coinbase_to, 5 * 10**9, coinbase=True)
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
            return {'lotteryRules': 2, 'protocolVersion': '7.0.0', **self.chain_info_override.get(host, {})}
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
            pending = self.pending_images()
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
        assert set(request) == {'operation'} | SIGNER_FIELDS[operation], request.keys()
        self.requests.append(('signer', operation))
        if operation in ('scan', 'restore_check'):
            address, owned = self.scan(request['wallet'], request['blocks'])
            if operation == 'restore_check':
                assert address == request['expected_address']
            return {'address': address, 'owned': owned}
        if operation == 'inspect':
            tx = json.loads(Path(request['artifact']).read_bytes())
            return {k: tx[k] for k in ('hash', 'fee', 'bytes', 'outputs', 'key_images')}
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


class PrefundBase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.now = 1_000_000.0
        plan = json.loads(Path(__file__).with_name('testnet-discovery-v2.json').read_text())
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
        # 64 large producer outputs: enough for 512 grants plus worst-case fees.
        for _ in range(64):
            self.fleet.output(1, funding['address'], 9 * 10**11, coinbase=True)
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
        self.assertEqual(found, SIGNER_FIELDS)

    def test_imports_only_tracked_harness_modules(self):
        source = Path(prefund.__file__).read_text()
        imported = set(re.findall(r'^(?:from|import) (\w+)', source, re.M))
        local = {p.stem for p in Path(prefund.__file__).parent.glob('*.py')}
        self.assertLessEqual(imported & local, {'controller', 'lottery', 'plan', 'runtime', 'selection'})
        self.assertFalse({'qualification', 'fast_timing', 'manifest_limits'} & imported)
        for module in imported - local:
            __import__(module)  # standard library only


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
        self.assertOpeningRefused('protocol is not 7.0.0')

    def test_wrong_commit(self):
        self.fleet.status_override[HOSTS[1]] = {'gitCommit': 'f' * 40}
        self.assertOpeningRefused('software identity')

    def test_minting_on_non_producer(self):
        self.fleet.status_override[HOSTS[0]] = {'mintingActive': True}
        self.assertOpeningRefused('only on the producer')

    def test_insufficient_funding_balance(self):
        self.m['grants_per_wallet'] = 32  # 1024 grants needs more than the 64 x 9e11 opening.
        self.assertOpeningRefused('cannot cover principal')


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
        # Every grant was submitted exactly once, to the producer only.
        self.assertEqual(len(self.fleet.submits), 512)
        self.assertEqual(len(set(self.fleet.submits)), 512)
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
        self.assertGreater(health['accounting']['coinbase_lottery_inflow'], 0)
        self.assertEqual([w['grant_outputs'] for w in health['inventory_readiness']['wallets']], [16] * 32)
        # The tracked controller and staging accept the output as launch opening_balances.
        self.assertDiscoveryAccepts(balances)
        self.assertTrue(self.tick(p))  # a finished run stays finished
        self.assertEqual(len(self.fleet.submits), 512)

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
        self.assertTrue((state / 'grants/grant-0000.json').exists())
        self.assertFalse((state / 'grants/grant-0000.submit').exists())
        self.assertFalse(self.fleet.submits)
        p = self.prefunder()
        self.tick(p)
        self.assertEqual(self.fleet.submits[0], p.records[0]['hash'])
        self.assertEqual(self.fleet.submits.count(p.records[0]['hash']), 1)
        self.assertTrue((state / 'grants/grant-0000.submit').exists())
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
        self.assertTrue((state / 'grants/grant-0000.submit').exists())
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
        (Path(self.m['state_dir']) / 'artifacts/grant-0001.bin').write_bytes(b'signed, never recorded')
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
            if request['operation'] == 'inspect' and not p.records:
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
        self.assertGreaterEqual(sum(r['state'] == 'confirmed' for r in p.records), 50)
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
        (Path(self.m['state_dir']) / 'grants/grant-0000.json.tmp').write_text('{')
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
