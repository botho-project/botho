"""#1633: controller memory and per-sync CPU must not grow ~62 KB per output.

x1 (q20261027-x1-endurance2, harness 5eddde6b) measured the controller at
860 MB RSS with 13,891 chain outputs, against ``MemoryMax=1G``, and about
45% of one core. Two terms dominated:

* each owned output was kept with the signer's ``kem_ciphertext`` as a JSON
  list of 1,089 integers (about 8.8 KB in memory per output, 5 KB on disk),
  next to the same ciphertext as hex in the output history, and
* every sync rewrote the whole ``blocks.json`` and ``inventory.json``
  through ``json.dump``'s pure-Python encoder, about 3.6 s of CPU per
  sync at 13.8k outputs and linear in the chain.

These tests drive the real ``Controller.sync`` (including the chunked native
``scan`` requests), ``accounting``, ``spendable`` and the bounded
``prepare`` history over a synthetic chain in the real RPC and signer output
formats (about 2.7 KB of serialized history per output, 2 outputs per
transaction, 64 wallets). Only the RPC and the signer process are faked.
The chain is scaled down. Budgets are asserted per output and extrapolated
to x-series h72 (about 55,800 outputs).
"""

import asyncio
import gc
import hashlib
import json
from pathlib import Path
import tempfile
import time
import tracemalloc
import unittest
from unittest.mock import patch

import controller as controller_module
from controller import Controller, compact_owned
from runtime import BlockLog, Gate, HOSTS, Journal, atomic, inventory_chunks

WALLETS = 64
KEM_BYTES = 1089  # ML-KEM-768 ciphertext + tag, as served by chain_getOutputs
H72_OUTPUTS = 55_800
MIB = 1024 * 1024


def h64(*parts):
    return hashlib.sha256(':'.join(map(str, parts)).encode()).hexdigest()


class SyntheticChain:
    """A chain in the real chain_getOutputs / signer ``owned`` formats."""

    def __init__(self, tx_per_block=6):
        self.tx_per_block = tx_per_block
        self.tamper = {}

    def kem(self, *parts):
        return (h64('kem', *parts) * 35)[:2 * KEM_BYTES]

    def block(self, k):
        if k in self.tamper:
            return self.tamper[k]
        outputs = [{
            'amountCommitment': '00203d88792d0000', 'clusterTags': [[9521182736877257924, 1000000]],
            'coinbase': True, 'cryptoOutputIndex': 0, 'kemCiphertext': self.kem(k, 'cb'),
            'ledgerOutpoint': {'outputIndex': 0, 'txHash': h64('lo', k)},
            'outputIndex': 4294967295, 'publicKey': h64('pk', k, 'cb'),
            'targetKey': h64('tk', k, 'cb'), 'txHash': h64('cb', k)}]
        for t in range(self.tx_per_block):
            for i in range(2):
                outputs.append({
                    'amountCommitment': '00203d88792d0000',
                    'clusterTags': [[16432821105838233495, 950000]],
                    'coinbase': False, 'cryptoOutputIndex': i, 'kemCiphertext': self.kem(k, t, i),
                    'ledgerOutpoint': {'outputIndex': i, 'txHash': h64('tx', k, t)},
                    'outputIndex': i, 'publicKey': h64('pk', k, t, i),
                    'targetKey': h64('tk', k, t, i), 'txHash': h64('tx', k, t)})
        return {'height': k, 'outputs': outputs}

    @staticmethod
    def owner(k, t, i):
        sender = (k * 7 + t) % WALLETS
        return (sender + 1) % WALLETS if i == 0 else sender

    def owned(self, heights, wallet):
        """The signer's ``owned`` rows for ``wallet`` over blocks at ``heights``."""
        rows = []
        for k in heights:
            if k in self.tamper:
                continue
            for t in range(self.tx_per_block):
                for i in range(2):
                    if self.owner(k, t, i) != wallet:
                        continue
                    tx = h64('tx', k, t)
                    rows.append({'id': tx + ':' + str(i), 'key_image': h64('ki', k, t, i), 'utxo': {
                        'amount': 50_000_000_000 + k, 'cluster_tags': {'tags': [[16432821105838233495, 950000]]},
                        'coinbase': False, 'created_at': k, 'crypto_output_index': i,
                        'kem_ciphertext': list(bytes.fromhex(self.kem(k, t, i))), 'lottery': False,
                        'output_index': i, 'public_key': list(bytes.fromhex(h64('pk', k, t, i))),
                        'subaddress_index': 0, 'target_key': list(bytes.fromhex(h64('tk', k, t, i))),
                        'tx_hash': list(bytes.fromhex(tx))}})
        return rows

    def outputs(self, blocks):
        return blocks * (1 + 2 * self.tx_per_block)


class Harness:
    def __init__(self, test, state, chain):
        self.test, self.state, self.chain = test, Path(state), chain
        self.tip = 0
        self.native_requests = []
        self.fail_native = False

    def controller(self):
        """A process start: history from disk, inventory untrusted (controller.py __init__)."""
        c = Controller.__new__(Controller)
        c.state = self.state
        c.plan = {}
        c.degrade_steps = []
        c.j = Journal(c.state / 'journal.sqlite')
        self.test.addCleanup(c.j.db.close)
        c.closed = False
        c.wallets = [{'key': 'wallet-%d' % i, 'address': 'addr-%d' % i} for i in range(WALLETS)]
        c.block_log = BlockLog(c.state)
        c.blocks = c.block_log.load()
        c.inventory = [[] for _ in c.wallets]
        c.inventory_initialized = False
        c.spent = []
        c.scan_lock = asyncio.Lock()
        c.scan_workers = 1
        c.native_payload = self.native_payload
        c.rpc = type('Rpc', (), {'call': self.rpc})()
        self.c = c
        self.set_tip(self.tip)
        return c

    def set_tip(self, tip):
        self.tip = tip
        self.c.statuses = {h: {'synced': True, 'chainHeight': tip} for h in HOSTS}

    async def native_payload(self, payload):
        # The real Controller.native chunked and encoded this request.
        request = json.loads(payload)
        if self.fail_native:
            raise Gate('signer: injected failure')
        self.native_requests.append((request['operation'], len(request['blocks'])))
        wallet = int(request['wallet'].split('-')[1])
        heights = [b['height'] for b in request['blocks']]
        del request
        # Parsed from the signer's stdout, as native_payload does.
        return json.loads(json.dumps({'address': 'addr-%d' % wallet,
                                      'owned': self.chain.owned(heights, wallet)}))

    async def rpc(self, host, method, params=None, **_):
        if method == 'chain_getOutputs':
            return [self.chain.block(k) for k in range(params['start_height'], params['end_height'] + 1)]
        if method == 'chain_areKeyImagesSpent':
            return [{'keyImage': i, 'pending': False, 'spent': int(i[:2], 16) < 64, 'spentHeight': None}
                    for i in params['keyImages']]
        raise AssertionError(method)

    def sync(self, tip=None, **kwargs):
        if tip is not None:
            self.set_tip(tip)
        return asyncio.run(self.c.sync(**kwargs))


class Scenario(unittest.TestCase):
    def harness(self, tx_per_block=6):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        return Harness(self, tmp.name, SyntheticChain(tx_per_block))

    def snapshot(self, c):
        return json.loads(json.dumps([c.blocks, c.inventory, c.spent]))


class CompactInventoryTests(Scenario):
    def test_owned_outputs_drop_only_the_ciphertext_and_history_keeps_it(self):
        h = self.harness(tx_per_block=2)
        c = h.controller()
        h.sync(30)
        raw = {o['id']: o for w in range(WALLETS) for o in h.chain.owned(range(31), w)}
        owned = [o for w in c.inventory for o in w]
        self.assertEqual(len(owned), len(raw))
        for output in owned:
            expected = raw[output['id']]
            self.assertNotIn('kem_ciphertext', output['utxo'])
            self.assertEqual(output, {**expected, 'utxo': {k: v for k, v in expected['utxo'].items()
                                                          if k != 'kem_ciphertext'}})
            # The ciphertext is still in the history, under the same output id.
            tx, index = output['id'].split(':')
            row = next(o for b in c.blocks for o in b['outputs']
                       if o['txHash'] == tx and o['outputIndex'] == int(index))
            self.assertEqual(bytes.fromhex(row['kemCiphertext']),
                             bytes(expected['utxo']['kem_ciphertext']))
        # Selection, the bounded prepare history and accounting still work.
        wallet = max(range(WALLETS), key=lambda w: len(c.inventory[w]))
        selected = c.spendable(wallet, h.tip)
        self.assertTrue(selected)
        blocks, spent = c.prepare_history(h.tip, selected[:2])
        ids = {o['txHash'] + ':' + str(o['outputIndex']) for b in blocks for o in b['outputs']}
        self.assertTrue({o['id'] for o in selected[:2]} <= ids)
        self.assertTrue(all('kemCiphertext' in o for b in blocks for o in b['outputs']))
        self.assertEqual({s['keyImage'] for s in spent}, {o['key_image'] for o in selected[:2]})

    def test_malformed_owned_entries_are_kept_verbatim(self):
        for entry in ('text', {'id': 'x'}, {'id': 'x', 'utxo': 'text'}, {'id': 'x', 'utxo': {'amount': 1}}):
            self.assertIs(compact_owned(entry), entry)


def reference_choose(controller, wallet, height, amount=0, count=None, headroom=0):
    """selection.choose before #1633: one materialized decoy-pool set per input."""
    from itertools import combinations
    canonical = controller.canonical_history()
    raw = controller.canonical_inventory(wallet, canonical)
    unspent = controller.unspent_setup_inputs(wallet, canonical)
    mature = [o for o in unspent if height - o["utxo"]["created_at"] >= 10]
    pools, eligible = {}, []
    for output in mature:
        age = height - output["utxo"]["created_at"]
        low, high = max(10, age - age // 10), age + age // 10
        real = bytes(output["utxo"]["target_key"]).hex()
        pool = {key for key, (created, _) in canonical.items()
                if key != "0" * 64 and low <= height - created <= high} - {real}
        if len(pool) >= 19 + headroom:
            pools[output["id"]] = pool
            eligible.append(output)
    eligible.sort(key=lambda o: (-o["utxo"]["amount"], o["id"]))
    if not amount:
        return eligible
    fee = controller.j.limits["max_single_signed_fee_picocredits"]
    for n in ([count] if count else range(1, 5)):
        for selected in combinations(eligible[:16], n):
            if sum(o["utxo"]["amount"] for o in selected) < amount + fee:
                continue
            excluded = {bytes(o["utxo"]["target_key"]).hex() for o in selected}
            if all(len(pools[o["id"]] - excluded) >= 19 + headroom for o in selected):
                return list(selected)
    return []


class SelectionEquivalenceTests(unittest.TestCase):
    """choose() counts decoy pools instead of building them (#1633): same result."""

    def test_counted_pools_select_exactly_as_materialized_pools(self):
        import random
        from types import SimpleNamespace
        from selection import choose
        rng = random.Random(1633)
        for trial in range(300):
            height = rng.randrange(40, 400)
            keys = ['0' * 64] + ['%064x' % rng.randrange(1, 60 + height) for _ in range(4)]
            canonical = {}
            for k in range(1, height + 1):
                for _ in range(rng.randrange(0, 3)):
                    key = rng.choice(keys) if rng.random() < 0.05 else '%064x' % rng.getrandbits(255)
                    canonical.setdefault(key, (k, {}))
            owned = []
            for i, (key, (created, _)) in enumerate(rng.sample(sorted(canonical.items()), min(30, len(canonical)))):
                owned.append({'id': 'tx%d:0' % i, 'key_image': 'i%d' % i, 'utxo': {
                    'amount': rng.choice((1, 5, 10, 20)) * 10**9, 'created_at': created,
                    'target_key': list(bytes.fromhex(key))}})
            fake = SimpleNamespace(
                canonical_history=lambda c=canonical: c,
                canonical_inventory=lambda w, c=None, o=owned: o,
                unspent_setup_inputs=lambda w, c=None, o=owned: o,
                j=SimpleNamespace(limits={'max_single_signed_fee_picocredits': 10**8}))
            for amount, count, headroom in ((0, None, 0), (0, None, 3), (15 * 10**9, None, 0),
                                            (25 * 10**9, 4, 2), (40 * 10**9, None, 1)):
                with self.subTest(trial=trial, amount=amount, count=count, headroom=headroom):
                    got = choose(fake, 0, height, amount, count, headroom)[0]
                    self.assertEqual([o['id'] for o in got],
                                     [o['id'] for o in reference_choose(fake, 0, height, amount, count, headroom)])


class InventoryCheckpointTests(Scenario):
    def test_streamed_inventory_is_byte_identical_to_atomic_json(self):
        owned = [[], [{'id': 'a:0', 'key_image': 'k', 'utxo': {'b': [1, 2], 'a': 1.5}}], [{'id': 'b:1'}] * 3]
        for spent in ([], [{'keyImage': str(i), 'spent': bool(i % 2), 'pending': False,
                            'spentHeight': None} for i in range(9000)]):
            with tempfile.TemporaryDirectory() as tmp:
                atomic(Path(tmp) / 'a.json', {'at': 1793071224.25, 'height': 7, 'owned': owned, 'spent': spent})
                streamed = ''.join(inventory_chunks(1793071224.25, 7, owned, spent))
                self.assertEqual(streamed, (Path(tmp) / 'a.json').read_text())

    def test_checkpoint_is_rate_limited_except_on_full_sync(self):
        h = self.harness(tx_per_block=1)
        c = h.controller()
        path = h.state / 'inventory.json'
        h.sync(20)
        first = path.stat().st_ino, path.read_text()
        h.sync(21)
        self.assertEqual((path.stat().st_ino, path.read_text()), first)
        with patch.object(controller_module.time, 'monotonic',
                          return_value=time.monotonic() + controller_module.INVENTORY_CHECKPOINT_SECONDS):
            h.sync(22)
        saved = json.loads(path.read_text())
        self.assertEqual(saved['height'], 22)
        self.assertEqual(saved['owned'], c.inventory)
        self.assertEqual(saved['spent'], c.spent)
        h.sync(23, full=True, rescan_wallets=False)
        self.assertEqual(json.loads(path.read_text())['height'], 23)


class BlockLogTests(Scenario):
    def blocks(self, n):
        return [{'height': k, 'outputs': [{'txHash': h64(k), 'outputIndex': 0}]} for k in range(n)]

    def test_append_reload_and_torn_tail(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = BlockLog(tmp)
            self.assertEqual(log.load(), [])
            first = self.blocks(3)
            log.persist([], first)
            second = first + self.blocks(5)[3:]
            log.persist(first, second)
            self.assertEqual(BlockLog(tmp).load(), second)
            # A crash mid-append leaves a torn final line: never loaded, and
            # truncated by the next append.
            with open(log.path, 'ab') as f:
                f.write(b'{"height": 5, "outp')
            reloaded = BlockLog(tmp)
            self.assertEqual(reloaded.load(), second)
            third = reloaded.persisted + self.blocks(6)[5:]
            reloaded.persist(reloaded.persisted, third)
            self.assertEqual(BlockLog(tmp).load(), third)
            self.assertEqual(log.path.read_bytes().count(b'\n'), 6)

    def test_corrupt_or_gapped_whole_lines_fail_closed(self):
        for body in (b'{"height": 0, "outputs": []}\nnot json\n',
                     b'{"height": 0, "outputs": []}\n{"height": 2, "outputs": []}\n',
                     b'[]\n'):
            with tempfile.TemporaryDirectory() as tmp:
                (Path(tmp) / BlockLog.NAME).write_bytes(body)
                with self.assertRaisesRegex(Gate, 'block log corrupt'):
                    BlockLog(tmp).load()

    def test_legacy_blocks_json_loads_and_converts_on_first_persist(self):
        with tempfile.TemporaryDirectory() as tmp:
            legacy = self.blocks(4)
            (Path(tmp) / 'blocks.json').write_text(json.dumps(legacy))
            log = BlockLog(tmp)
            loaded = log.load()
            self.assertEqual(loaded, legacy)
            extended = loaded + self.blocks(5)[4:]
            log.persist(loaded, extended)  # a full rewrite: the log did not exist
            self.assertEqual(BlockLog(tmp).load(), extended)

    def test_replaced_history_list_is_rewritten_not_appended(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = BlockLog(tmp)
            log.persist([], self.blocks(4))
            # Prefunder-style reset: a fresh list, not the persisted one.
            fresh = self.blocks(2)
            log.persist([], fresh)
            self.assertEqual(BlockLog(tmp).load(), fresh)

    def test_failed_append_is_truncated_before_the_next(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = BlockLog(tmp)
            first = self.blocks(2)
            log.persist([], first)
            size = log.size
            with open(log.path, 'ab') as f:  # an append that raised half-way
                f.write(b'{"height": 2, "outputs": []}\n{"hei')
            nxt = first + self.blocks(3)[2:]
            log.persist(first, nxt)
            self.assertGreater(log.size, size)
            self.assertEqual(BlockLog(tmp).load(), nxt)


class RestartExactnessTests(Scenario):
    def reference(self, steps):
        h = self.harness(tx_per_block=2)
        h.controller()
        for tip, kwargs in steps:
            h.sync(tip, **kwargs)
        return self.snapshot(h.c)

    STEPS = [(40, {}), (41, {}), (45, {}), (46, {'full': True, 'rescan_wallets': False}), (50, {})]

    def test_restart_after_every_sync_rebuilds_the_same_state(self):
        expected = self.reference(self.STEPS)
        h = self.harness(tx_per_block=2)
        for tip, kwargs in self.STEPS:
            h.controller()
            h.sync(tip, **kwargs)
        self.assertEqual(self.snapshot(h.c), expected)
        # The persisted history is exactly the in-memory one.
        self.assertEqual(BlockLog(h.state).load(), h.c.blocks)

    def test_crash_between_history_append_and_inventory_checkpoint(self):
        expected = self.reference(self.STEPS)
        h = self.harness(tx_per_block=2)
        c = h.controller()
        h.sync(40)
        h.sync(41)
        with patch.object(Controller, 'checkpoint_inventory', side_effect=OSError('disk full')):
            with self.assertRaises(OSError):
                h.sync(45)
        # Memory did not advance; the log already holds the validated blocks.
        self.assertEqual(len(c.blocks), 42)
        self.assertEqual(len(BlockLog(h.state).load()), 46)
        # Retrying in the same process rewrites the log to match memory.
        h.sync(45)
        self.assertEqual(BlockLog(h.state).load(), c.blocks)
        h.controller()  # and a restart from that log is exact
        h.sync(46, full=True, rescan_wallets=False)
        h.sync(50)
        self.assertEqual(self.snapshot(h.c), expected)

    def test_torn_append_after_a_crash_is_dropped_and_refetched(self):
        expected = self.reference(self.STEPS)
        h = self.harness(tx_per_block=2)
        h.controller()
        h.sync(40)
        h.sync(41)
        with open(h.state / BlockLog.NAME, 'ab') as f:
            f.write(json.dumps(h.chain.block(42))[:500].encode())
        h.controller()
        self.assertEqual(len(h.c.blocks), 42)
        h.sync(45)
        h.sync(46, full=True, rescan_wallets=False)
        h.sync(50)
        self.assertEqual(self.snapshot(h.c), expected)

    def test_failed_sync_never_advances_memory_or_log(self):
        h = self.harness(tx_per_block=2)
        c = h.controller()
        h.sync(40)
        before = self.snapshot(c), (h.state / BlockLog.NAME).read_bytes()
        h.fail_native = True
        with self.assertRaises(Gate):
            h.sync(45)
        self.assertEqual((self.snapshot(c), (h.state / BlockLog.NAME).read_bytes()), before)
        h.fail_native = False
        h.sync(45)
        self.assertEqual(len(BlockLog(h.state).load()), 46)

    def test_full_refetch_still_checks_the_whole_checkpoint(self):
        for tamper, message in ((5, 'disagrees'), (None, 'disagrees')):
            with self.subTest(tamper=tamper):
                h = self.harness(tx_per_block=1)
                c = h.controller()
                h.sync(30)
                if tamper is None:
                    # A refetch shorter than the checkpoint.
                    h.set_tip(20)
                else:
                    changed = h.chain.block(tamper)
                    changed['outputs'][0]['amountCommitment'] = '00'
                    h.chain.tamper[tamper] = changed
                    h.set_tip(31)
                before = (h.state / BlockLog.NAME).read_bytes()
                with self.assertRaisesRegex(Gate, message):
                    asyncio.run(c.sync(full=True, rescan_wallets=False))
                self.assertEqual((h.state / BlockLog.NAME).read_bytes(), before)

    def test_incremental_duplicate_output_id_is_still_rejected(self):
        h = self.harness(tx_per_block=1)
        c = h.controller()
        h.sync(10)
        h.sync(11)  # the id cache now covers the checkpoint
        h.chain.tamper[12] = {'height': 12, 'outputs': [dict(h.chain.block(3)['outputs'][1])]}
        with self.assertRaisesRegex(Gate, 'duplicate output id across incremental history'):
            h.sync(12)
        self.assertEqual(len(c.blocks), 12)
        # The failed sync did not poison the cache: a clean block still appends.
        del h.chain.tamper[12]
        h.sync(12)
        self.assertEqual(len(c.blocks), 13)


class XSeriesBudgetTests(Scenario):
    """Per-output memory and per-sync CPU, extrapolated to x-series h72."""

    BLOCKS = 240  # x 13 outputs = 3,120 outputs; ~2.7 KB of history each
    # Budgets. Measured (CPython 3.15, macOS): blocks ~3.5 KB, inventory ~2.0
    # KB and spent rows ~0.2 KB per output; before #1633 inventory alone was
    # ~10 KB. 8 KB/output is ~430 MiB of retained state at h72.
    RETAINED_PER_OUTPUT = 8 * 1024
    # Before #1633 an incremental sync at this size took ~0.8 s (whole-file
    # pure-Python JSON rewrites); it now takes milliseconds.
    INCREMENTAL_CPU_SECONDS = 0.25

    def test_retained_memory_per_output_and_incremental_sync_cost(self):
        h = self.harness(tx_per_block=6)
        gc.collect()
        tracemalloc.start()
        self.addCleanup(tracemalloc.stop)
        base, _ = tracemalloc.get_traced_memory()
        c = h.controller()
        h.sync(self.BLOCKS - 1)
        outputs = h.chain.outputs(self.BLOCKS)
        self.assertEqual(sum(len(b['outputs']) for b in c.blocks), outputs)
        gc.collect()
        retained, _ = tracemalloc.get_traced_memory()
        per_output = (retained - base) / outputs
        self.assertLess(per_output, self.RETAINED_PER_OUTPUT)
        self.assertLess(per_output * H72_OUTPUTS, 1024 * MIB)

        # Steady state: one appended block per sync. The transient peak is
        # bounded by the appended blocks, not the chain.
        log = h.state / BlockLog.NAME
        inode = log.stat().st_ino
        tracemalloc.reset_peak()
        before, _ = tracemalloc.get_traced_memory()
        for _ in range(5):
            h.sync(h.tip + 1)
        after, peak = tracemalloc.get_traced_memory()
        self.assertLess(peak - before, 4 * MIB)
        self.assertEqual(log.stat().st_ino, inode)  # appended, never rewritten

        # A phase-boundary refetch does not hold a second copy of the history.
        tracemalloc.reset_peak()
        before, _ = tracemalloc.get_traced_memory()
        h.sync(h.tip + 1, full=True, rescan_wallets=False)
        _, peak = tracemalloc.get_traced_memory()
        self.assertLess(peak - before, 0.25 * (retained - base) + 4 * MIB)
        tracemalloc.stop()

        # Incremental sync CPU, untraced.
        costs = []
        for _ in range(5):
            h.set_tip(h.tip + 1)
            started = time.process_time()
            asyncio.run(c.sync())
            costs.append(time.process_time() - started)
        self.assertLess(min(costs), self.INCREMENTAL_CPU_SECONDS, costs)
        # Whole-chain scans stay serial (#1629/#1630); appended ones may not.
        self.assertEqual(c.scan_parallelism(False), 1)


class StagingBudgetTests(unittest.TestCase):
    def test_controller_unit_budget_is_explicit_and_observer_unchanged(self):
        from staging import CONTROLLER_MEMORY_MAX, unit_text
        item = {'package': '/opt/p', 'state': '/var/lib/s', 'user': 'u'}
        lines = unit_text(item).splitlines()
        self.assertIn('MemoryMax=3G', lines)
        self.assertIn('MemorySwapMax=0', lines)
        self.assertIn('CPUQuota=100%', lines)  # incremental scans stay serial (#1630)
        self.assertIn('Environment=MALLOC_ARENA_MAX=2', lines)
        # Well below the dedicated 32 GiB controller host.
        self.assertLessEqual(int(CONTROLLER_MEMORY_MAX.rstrip('G')), 32 // 8)
        # The measured h72 retained state fits the budget with room for the
        # whole-chain scan transients and the bounded signer.
        self.assertLess(XSeriesBudgetTests.RETAINED_PER_OUTPUT * H72_OUTPUTS, 1024 * MIB)
        observer = unit_text(item, True).splitlines()
        self.assertIn('MemoryMax=64M', observer)
        self.assertIn('CPUQuota=5%', observer)
        self.assertFalse(any(line.startswith('Environment=') for line in observer))


if __name__ == '__main__':
    unittest.main()
