"""Bounded history for native signer requests (#1631).

The pinned ``botho-stress-wallet`` reads one JSON request of at most
``MAX_REQUEST`` (128 MiB). At x-series size, the whole output history crosses
that limit at about T0+61h. The signer also holds about three times the
request size in RSS, and the campaign unit has ``MemoryMax=1G``. This module
keeps every request bounded *without changing the signer*.

``prepare`` (botho-wallet/src/stress.rs, ring_builder.rs) reads ``blocks``
only for these purposes. The bounded history keeps each one:

* Ownership is scanned only for the caller-selected output ids, so the
  selected outputs must be present. Their block height is ``created_at``,
  which sets maturity and age.
* An input is canonical only if no earlier eligible output (height > 0, not
  lottery) has the same target key (``canonical_rpc_history``). The first
  occurrence of each selected target key is sent too. A
  non-canonical selection is therefore still rejected as "input is not
  canonical".
* Each ring draws ``MIN_RING_SIZE - 1 = 19`` decoys uniformly, through an
  OsRng shuffle, from the canonical outputs whose age lies in the input's
  +/-10% band (ten-block floor). The selected target keys are excluded. Only
  outputs inside some band can become decoys.
* No check needs contiguous heights, a start at height 0, global output
  indices, or unselected outputs. Structural validation and the
  duplicate-id and duplicate-height rules apply to whatever is sent.
  Every block was already validated by the signer's own ``scan`` during
  ``sync``.

Decoy privacy. When an input's band pool is small, it is sent whole and the
signer's pool is identical. Otherwise the outputs of overlapping bands are
Bernoulli-sampled with one probability per connected group of bands. Given
how many outputs survive in a band, every subset of that size is equally
likely. The signer then picks 19 uniformly from them, so each ring is still
a uniform 19-subset of its band, exactly as with the full history. Separate
per-band samples would not have this property: outputs where two bands
overlap would get two chances to appear.

The sampling randomness comes from ``random.SystemRandom``. A seed taken
from public data must never be used. Once a sample is as small as a ring,
the seed alone fixes the ring. An observer could then replay the sampling
for each ring member and find the real input.

Two documented departures from exact uniformity can occur:

* A band can draw fewer than 19 eligible outputs. The expected count is
  ``DECOY_SAMPLE_TARGET`` = 64, so the probability is below 1e-10. In that
  case the band is topped up uniformly from its unsent outputs, and the
  signer's "Not enough decoys" error is never introduced.
* A group can exceed ``DECOY_GROUP_CEILING`` expected outputs. That needs
  extreme density differences between overlapping bands. The sampling
  probability is then capped to keep the request bounded.

``scan`` and ``restore_check`` judge each output's ownership independently.
The signer's response is the owned outputs in request order. Sending the
history in consecutive block chunks and concatenating the ``owned`` lists
gives the same response as one request. The single-request duplicate
height and output-id rules are checked across the whole history before the
history is split.
"""

from bisect import bisect_left, bisect_right
import json
import random

# botho-wallet/src/stress.rs MAX_REQUEST (pinned signer, 386b5ede).
SIGNER_MAX_REQUEST = 128 * 1024 * 1024
# The signer used about 3x the request size in RSS (309 MB for a 101.8 MiB
# request). 16 MiB chunks keep one signer under about 64 MB, even with one
# signer per controller CPU (#1629/#1630).
SCAN_CHUNK_BYTES = 16 * 1024 * 1024
# MIN_RING_SIZE - 1 (transaction/clsag/src/lib.rs).
RING_DECOYS = 19
# MIN_DECOY_AGE_BLOCKS (botho-wallet/src/decoy_selection.rs).
MIN_DECOY_AGE = 10
# Expected sampled outputs in a group's smallest band.
DECOY_SAMPLE_TARGET = 64
# Upper bound on expected sampled outputs per group of overlapping bands.
# There are at most 4 groups (4 inputs), so about 8.2k outputs (about 22 MB)
# whatever the chain length. The usual case is a few hundred.
DECOY_GROUP_CEILING = 2048
SCAN_OPERATIONS = ('scan', 'restore_check')


def age_band(age):
    """stress.rs/decoy_selection.rs ``age_similarity_band``: +/-10%, ten-block floor."""
    delta = age * 1000 // 10_000
    return max(age - delta, MIN_DECOY_AGE), age + delta


def output_id(output):
    """The signer's selected-id form: lowercase tx hash and RPC output index."""
    return output['txHash'].lower() + ':' + str(output['outputIndex'])


def prepare_spent(spent, key_images):
    """Only the spent-state rows the signer reads, which are the selected key images.

    Every matching row is kept. This preserves the signer's rule that each
    image has exactly one row.
    """
    return [s for s in spent if s.get('keyImage') in key_images]


def prepare_blocks(blocks, height, selected_ids, rng=None,
                   target=DECOY_SAMPLE_TARGET, ceiling=DECOY_GROUP_CEILING):
    """Bounded ``prepare`` history for ``selected_ids`` at pinned ``height``.

    Returns partial blocks in height order. Each one keeps the original
    order of its retained outputs.
    """
    rng = rng or random.SystemRandom()
    wanted = set(selected_ids)
    canonical = []  # (height, position, key): first eligible target occurrences
    first = {}
    located = {}
    for block in sorted(blocks, key=lambda b: b['height']):
        h = block['height']
        for position, output in enumerate(block['outputs']):
            if output_id(output) in wanted:
                located.setdefault(output_id(output), (h, position, output))
            if h > 0 and not output.get('lottery', False):
                key = output['targetKey'].lower()
                if key not in first:
                    first[key] = (h, position)
                    canonical.append((h, position, key))
    chosen = set()
    excluded = set()
    ages = []
    for h, position, output in located.values():
        chosen.add((h, position))
        key = output['targetKey'].lower()
        excluded.add(key)
        if key in first:
            chosen.add(first[key])
        if height - h >= MIN_DECOY_AGE:
            ages.append(height - h)
    # A selected id missing from history is simply not sent. The signer then
    # fails closed ("selected input is not owned"), as it does today.

    heights = [h for h, _, _ in canonical]

    def pool(low, high):
        """Canonical, non-excluded outputs with block height in [low, high]."""
        return [(h, p) for h, p, key in canonical[bisect_left(heights, low):bisect_right(heights, high)]
                if key not in excluded and h <= height]

    # Height intervals of each band, merged into connected groups.
    intervals = sorted((height - high, height - low) for low, high in map(age_band, ages))
    groups = []
    for low, high in intervals:
        if groups and low <= groups[-1][-1][1]:
            groups[-1].append((low, high))
        else:
            groups.append([(low, high)])
    for group in groups:
        bands = [pool(low, high) for low, high in group]
        union = pool(min(b[0] for b in group), max(b[1] for b in group))
        smallest = min(len(b) for b in bands)
        p = 1.0 if smallest <= target else target / smallest
        if p * len(union) > ceiling:
            p = ceiling / len(union)
        sample = set(union) if p >= 1 else {x for x in union if rng.random() < p}
        for band in bands:
            have = sum(1 for x in band if x in sample)
            need = min(RING_DECOYS, len(band))
            if have < need:
                sample.update(rng.sample([x for x in band if x not in sample], need - have))
        chosen |= sample

    by_height = {}
    for h, position in chosen:
        by_height.setdefault(h, set()).add(position)
    source = {block['height']: block for block in blocks}
    return [{'height': h, 'outputs': [o for i, o in enumerate(source[h]['outputs']) if i in by_height[h]]}
            for h in sorted(by_height)]


def history_duplicate(blocks):
    """The signer's whole-request ``validate_history`` duplicate rules, or None."""
    heights, ids = set(), set()
    for block in blocks:
        if block['height'] in heights:
            return 'duplicate block height'
        heights.add(block['height'])
        for output in block['outputs']:
            identifier = (output['txHash'], output['outputIndex'])
            if identifier in ids:
                return 'duplicate output id'
            ids.add(identifier)
    return None


def scan_payloads(request, budget=SCAN_CHUNK_BYTES):
    """Yield encoded ``scan``/``restore_check`` requests over consecutive chunks.

    Each block is serialized once and lazily, so the controller never holds
    the whole-history JSON either. A payload exceeds ``budget`` only when a
    single block does. At least one payload is always produced, so an empty
    history still checks the wallet address.
    """
    envelope = json.dumps({k: v for k, v in request.items() if k != 'blocks'})
    head = envelope[:-1] + ', "blocks": ['
    tail = ']}'
    parts, size = [], len(head) + len(tail)
    for block in request['blocks']:
        part = json.dumps(block)
        if parts and size + len(part) + 2 > budget:
            yield (head + ', '.join(parts) + tail).encode()
            parts, size = [], len(head) + len(tail)
        parts.append(part)
        size += len(part) + 2
    if parts or not request['blocks']:
        yield (head + ', '.join(parts) + tail).encode()


def merge_scans(results):
    """Concatenate chunk responses in order. Returns the single-request shape."""
    addresses = {r['address'] for r in results}
    if len(addresses) != 1:
        raise ValueError('signer address differs across scan chunks')
    return {'address': results[0]['address'], 'owned': [o for r in results for o in r['owned']]}
