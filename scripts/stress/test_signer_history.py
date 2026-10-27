"""Bounded signer requests at x-series size (#1631).

The pinned signer rejects a request over MAX_REQUEST (128 MiB). At about h61
the whole x-series history is larger than that. These tests use the real
request builders: the payload ``Controller.native`` encodes for ``prepare``,
and the chunk payloads for ``scan``/``restore_check``. They check that each
request stays far below the cap and below the unit memory budget. They also
check that the signer's decoy pool, reimplemented below from
ring_builder.rs, keeps the semantics of the full history.
"""

import json
import random
import unittest

from signer_history import (DECOY_SAMPLE_TARGET, RING_DECOYS, SCAN_CHUNK_BYTES,
                            SIGNER_MAX_REQUEST, age_band, history_duplicate,
                            merge_scans, prepare_blocks, prepare_spent,
                            scan_payloads)

# x-series at h72: about 99 blocks/h, about 55.8k outputs, about 2,705 B each
# (q20261026-x1-endurance blocks.json). One coinbase output per block, then
# two-output transactions.
H72_BLOCKS = 99 * 72
H72_OUTPUTS = 55_800
KEM = 'ab' * 1088  # hex ML-KEM ciphertext, about 80% of each output
UNIT_MEMORY_MAX = 1024 ** 3
SIGNER_RSS_PER_REQUEST_BYTE = 309e6 / (101.8 * 1024 ** 2)  # measured (#1631)


def output(n, lottery=False, key=None):
    return {'txHash': '%064x' % n, 'outputIndex': n % 2, 'coinbase': False, 'lottery': lottery,
            'targetKey': key or '%064x' % (n + 10 ** 9), 'publicKey': '%064x' % (n + 2 * 10 ** 9),
            'amountCommitment': '00e40b5402000000', 'clusterTags': [], 'kemCiphertext': KEM}


def chain(blocks=H72_BLOCKS, outputs=H72_OUTPUTS):
    per_block, extra = divmod(outputs, blocks)
    history, n = [], 0
    for height in range(blocks):
        rows = []
        for _ in range(per_block + (height < extra)):
            rows.append(output(n))
            n += 1
        history.append({'height': height, 'outputs': rows})
    return history


def signer_pool(blocks, height, created, excluded):
    """ring_builder.rs select_rpc_decoys_from_history before its shuffle.

    canonical_rpc_history sorts by height, skips height 0 and lottery rows,
    and keeps the first target key. The age band is then applied, then the
    excluded keys are removed.
    """
    low, high = age_band(height - created)
    seen, pool = set(), set()
    for block in sorted(blocks, key=lambda b: b['height']):
        for o in block['outputs']:
            if block['height'] == 0 or o.get('lottery'):
                continue
            key = o['targetKey'].lower()
            if key in seen:
                continue
            seen.add(key)
            if low <= height - block['height'] <= high and key not in excluded:
                pool.add(key)
    return pool


def selected_at(history, heights):
    return [(history[h]['outputs'][0], h) for h in heights]


class XSeriesH72Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.history = chain()
        cls.height = H72_BLOCKS - 1
        cls.per_output = len(json.dumps(output(123))) + 2

    def prepare_request(self, inputs):
        """The prepare request ``Controller.prepare`` builds, at real size."""
        ids = [o['txHash'] + ':' + str(o['outputIndex']) for o, _ in inputs]
        images = {'ki-' + i for i in ids}
        spent = [{'keyImage': 'ki-%d' % n, 'spent': False, 'pending': False, 'spentHeight': None}
                 for n in range(H72_OUTPUTS)] + [
                {'keyImage': image, 'spent': False, 'pending': False, 'spentHeight': None}
                for image in images]
        blocks = prepare_blocks(self.history, self.height, ids)
        request = {'operation': 'prepare', 'wallet': '/state/wallets/w.mnemonic', 'blocks': blocks,
                   'height': self.height, 'selected': ids, 'spent': prepare_spent(spent, images),
                   'reserved': ['%064x:0' % n for n in range(64)], 'recipient': 'r' * 120,
                   'allowed_recipients': ['r' * 120] * 64, 'amount': 10 ** 10, 'base_rate': 1,
                   'artifact': '/state/artifacts/x.bin', 'fee_multiplier': 4}
        return request, len(json.dumps(request).encode())

    def test_whole_history_would_exceed_the_signer_cap(self):
        whole = self.per_output * H72_OUTPUTS
        self.assertGreater(whole, SIGNER_MAX_REQUEST)
        # And the measured RSS ratio leaves no room in a 1 GiB unit.
        self.assertGreater(whole * SIGNER_RSS_PER_REQUEST_BYTE, UNIT_MEMORY_MAX / 3)

    def test_prepare_request_is_bounded_at_h72_for_every_input_shape(self):
        # Ages chosen so the four bands are disjoint and as wide as possible,
        # which is the largest exact pool. Also a young, an overlapping
        # and a single input.
        shapes = {
            'one_old': [self.height - 7000],
            'four_disjoint_oldest': [self.height - a for a in (7000, 5700, 4650, 3800)],
            'four_overlapping': [self.height - a for a in (3000, 3100, 3200, 3300)],
            'young_and_old': [self.height - a for a in (40, 7000)],
        }
        for name, created in shapes.items():
            with self.subTest(name):
                inputs = selected_at(self.history, created)
                request, size = self.prepare_request(inputs)
                self.assertLess(size, 2 * 1024 ** 2, name)
                self.assertLess(size * SIGNER_RSS_PER_REQUEST_BYTE, 16 * 1024 ** 2)
                blocks = request['blocks']
                self.assertIsNone(history_duplicate(blocks))
                self.assertTrue(all(b['height'] <= self.height for b in blocks))
                sent = {o['txHash'] + ':' + str(o['outputIndex']) for b in blocks for o in b['outputs']}
                self.assertTrue(set(request['selected']) <= sent)
                self.assertEqual(len(request['spent']), len(inputs))
                excluded = {o['targetKey'] for o, _ in inputs}
                for o, h in inputs:
                    full = signer_pool(self.history, self.height, h, excluded)
                    bounded = signer_pool(blocks, self.height, h, excluded)
                    self.assertTrue(bounded <= full)
                    self.assertGreaterEqual(len(bounded), min(RING_DECOYS, len(full)))
                    if len(full) <= DECOY_SAMPLE_TARGET:
                        self.assertEqual(bounded, full)

    def test_restore_chunks_cover_history_exactly_under_the_cap(self):
        request = {'operation': 'restore_check', 'wallet': '/state/wallets/w.mnemonic',
                   'blocks': self.history, 'expected_address': 'a' * 120}
        heights, chunks, largest = [], 0, 0
        for payload in scan_payloads(request):
            chunks += 1
            largest = max(largest, len(payload))
            decoded = json.loads(payload)
            self.assertEqual({k: v for k, v in decoded.items() if k != 'blocks'},
                             {k: v for k, v in request.items() if k != 'blocks'})
            heights.extend(b['height'] for b in decoded['blocks'])
        self.assertEqual(heights, list(range(H72_BLOCKS)))
        self.assertLessEqual(largest, SCAN_CHUNK_BYTES)
        self.assertLess(largest * SIGNER_RSS_PER_REQUEST_BYTE, UNIT_MEMORY_MAX / 8)
        self.assertGreaterEqual(chunks, 9)


class PrepareHistorySemanticsTests(unittest.TestCase):
    def test_small_pools_are_sent_whole_and_identical(self):
        history = chain(blocks=300, outputs=900)
        inputs = selected_at(history, [200])
        blocks = prepare_blocks(history, 299, [o['txHash'] + ':1' if o['outputIndex'] else
                                               o['txHash'] + ':0' for o, _ in inputs])
        excluded = {inputs[0][0]['targetKey']}
        self.assertEqual(signer_pool(blocks, 299, 200, excluded),
                         signer_pool(history, 299, 200, excluded))

    def test_earlier_alias_is_sent_so_signer_still_rejects_non_canonical(self):
        history = chain(blocks=300, outputs=900)
        selected = history[250]['outputs'][1]
        alias = output(10 ** 6)
        alias['targetKey'] = selected['targetKey'].upper()
        history[5]['outputs'].append(alias)
        blocks = prepare_blocks(history, 299, [selected['txHash'] + ':' + str(selected['outputIndex'])])
        sent = [o for b in blocks for o in b['outputs']]
        self.assertIn(alias, sent)
        self.assertIn(selected, sent)

    def test_lottery_rows_are_never_canonical_decoys(self):
        history = chain(blocks=300, outputs=900)
        for block in history[150:250]:
            block['outputs'].append(output(10 ** 7 + block['height'], lottery=True))
        target = history[200]['outputs'][0]
        blocks = prepare_blocks(history, 299, [target['txHash'] + ':0'])
        self.assertFalse(any(o['lottery'] for b in blocks for o in b['outputs']))

    def test_overlapping_bands_share_one_inclusion_probability(self):
        # Two overlapping bands and large pools: every output in the union is
        # included with the same probability, whichever band it is in.
        history = chain(blocks=4000, outputs=40_000)
        height = 3999
        created = [height - 2000, height - 2200]
        ids = [history[h]['outputs'][0]['txHash'] + ':0' for h in created]
        low = height - age_band(2200)[1]
        high = height - age_band(2000)[0]
        counts = {}
        trials = 200
        for seed in range(trials):
            blocks = prepare_blocks(history, height, ids, rng=random.Random(seed))
            for b in blocks:
                if low <= b['height'] <= high:
                    for o in b['outputs']:
                        counts[b['height']] = counts.get(b['height'], 0) + 1
        overlap = [h for h in counts if height - age_band(2200)[1] <= h <= height - age_band(2200)[0]
                   and height - age_band(2000)[1] <= h <= height - age_band(2000)[0]]
        only = [h for h in counts if h not in overlap and h not in created]
        mean = lambda hs: sum(counts[h] for h in hs) / len(hs)
        self.assertAlmostEqual(mean(overlap) / mean(only), 1.0, delta=0.15)

    def test_spent_rows_trimmed_to_selected_images_keeping_duplicates(self):
        rows = [{'keyImage': 'a'}, {'keyImage': 'b'}, {'keyImage': 'a'}]
        self.assertEqual(prepare_spent(rows, {'a'}), [rows[0], rows[2]])


class ScanChunkTests(unittest.TestCase):
    def test_empty_history_still_sends_one_request(self):
        payloads = list(scan_payloads({'operation': 'scan', 'wallet': 'w', 'blocks': []}))
        self.assertEqual([json.loads(p) for p in payloads],
                         [{'operation': 'scan', 'wallet': 'w', 'blocks': []}])

    def test_merge_concatenates_in_order_and_rejects_address_mismatch(self):
        self.assertEqual(merge_scans([{'address': 'a', 'owned': [1]}, {'address': 'a', 'owned': [2, 3]}]),
                         {'address': 'a', 'owned': [1, 2, 3]})
        with self.assertRaises(ValueError):
            merge_scans([{'address': 'a', 'owned': []}, {'address': 'b', 'owned': []}])

    def test_cross_chunk_duplicates_are_detected_before_splitting(self):
        history = chain(blocks=10, outputs=20)
        self.assertIsNone(history_duplicate(history))
        history[9]['outputs'].append(dict(history[0]['outputs'][0]))
        self.assertEqual(history_duplicate(history), 'duplicate output id')
        history[9]['outputs'].pop()
        history.append({'height': 3, 'outputs': []})
        self.assertEqual(history_duplicate(history), 'duplicate block height')


class ControllerNativeChunkingTests(unittest.IsolatedAsyncioTestCase):
    async def test_native_scan_is_chunked_and_merged(self):
        from unittest.mock import patch
        from controller import Controller
        from runtime import Gate
        c = Controller.__new__(Controller)
        calls = []

        async def payload(data):
            request = json.loads(data)
            calls.append([b['height'] for b in request['blocks']])
            return {'address': 'a', 'owned': [b['height'] for b in request['blocks']]}

        c.native_payload = payload
        history = chain(blocks=40, outputs=80)
        budget = len(json.dumps({'operation': 'scan', 'wallet': 'w', 'blocks': history[:10]})) + 4
        with patch('controller.scan_payloads', lambda r: scan_payloads(r, budget)):
            result = await c.native({'operation': 'scan', 'wallet': 'w', 'blocks': history})
        self.assertGreater(len(calls), 1)
        self.assertEqual(result, {'address': 'a', 'owned': list(range(40))})
        history.append({'height': 0, 'outputs': []})
        with self.assertRaises(Gate):
            await c.native({'operation': 'scan', 'wallet': 'w', 'blocks': history})


if __name__ == '__main__':
    unittest.main()
