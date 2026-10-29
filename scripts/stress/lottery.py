"""V2 lottery payouts in the stress harness (#1643, P6 of #1286).

The chain, not the signer, classifies every output row (frozen RPC contract,
https://github.com/botho-project/botho/issues/1641#issuecomment-6488144384):

- ordinary: ``lottery`` false and no ``lotteryContext``;
- legacy (V1): ``lottery`` true and no ``lotteryContext``. A legacy payout
  reuses its winner's target key, so it is never canonical, never spendable,
  and stays under the ``legacy_lottery_evidence`` hold (#1450);
- V2: ``lottery`` true with ``lotteryContext {version: 2, baseIndex, tweak,
  ordinal, height}``, ``cryptoOutputIndex == baseIndex`` and ``winnerOutpoint``
  ("hash:index"). Its one-time key P = P_base + T*G is distinct, so a V2
  payout is canonical, spendable campaign inventory, and income in accounting.

Any other combination is a malformed row and fails closed.
"""

import re

from runtime import Gate

LOTTERY_RULES = 2
PROTOCOL_VERSION = '7.0.0'
_HEX64 = re.compile(r'[0-9a-f]{64}')
_OUTPOINT = re.compile(r'([0-9a-fA-F]{64}):(0|[1-9][0-9]{0,9})')


def _natural(value):
    return type(value) is int and value >= 0


def lottery_context(row):
    """``None`` for an ordinary or legacy row; the validated V2 context otherwise.

    Returns ``{'version', 'base_index', 'tweak', 'ordinal', 'height',
    'winner'}`` with ``winner`` the lowercase "hash:index" outpoint.
    """
    context = row.get('lotteryContext')
    if context is None:
        return None
    lottery = row.get('lottery', False)
    winner = row.get('winnerOutpoint')
    match = _OUTPOINT.fullmatch(winner) if isinstance(winner, str) else None
    if (
        lottery is not True
        or not isinstance(context, dict)
        or type(context.get('version')) is not int
        or context['version'] != LOTTERY_RULES
        or not all(_natural(context.get(k)) for k in ('baseIndex', 'ordinal', 'height'))
        or not isinstance(context.get('tweak'), str)
        or not _HEX64.fullmatch(context['tweak'])
        or row.get('cryptoOutputIndex') != context['baseIndex']
        or type(row.get('cryptoOutputIndex')) is not int
        or match is None
        or int(match.group(2)) >= 1 << 32
    ):
        raise Gate('malformed V2 lottery row')
    return {
        'version': context['version'],
        'base_index': context['baseIndex'],
        'tweak': context['tweak'],
        'ordinal': context['ordinal'],
        'height': context['height'],
        'winner': match.group(1).lower() + ':' + match.group(2),
    }


def decoy_row(row):
    """Whether a canonical row may be a ring decoy.

    No lottery payout, V2 included, is a decoy: the harness's decoy pools
    (``prepare_blocks``, ``selection.choose``, ``Controller.spendable``)
    follow the native signer, which keeps payouts out at first (P5, #1642).
    """
    return not row.get('lottery', False) and row.get('lotteryContext') is None


def is_legacy_lottery(row):
    """A lottery row without a V2 context: excluded and held, as in #1450."""
    return bool(row.get('lottery', False)) and lottery_context(row) is None


def lottery_rows(blocks):
    """Every lottery row by lowercase "hash:index": (height, amount, context or None)."""
    rows = {}
    for block in blocks:
        for row in block['outputs']:
            if not row.get('lottery', False) and row.get('lotteryContext') is None:
                continue
            context = lottery_context(row)
            amount = int.from_bytes(bytes.fromhex(row['amountCommitment']), 'little')
            rows[row['txHash'].lower() + ':' + str(row['outputIndex'])] = (
                block['height'], amount, context)
    return rows


def check_lottery_rules(info, host):
    """Preflight: refuse a node that does not report V2 lottery rules on 7.0.0."""
    if not isinstance(info, dict):
        raise Gate('malformed chain info: ' + host)
    rules = info.get('lotteryRules')
    if type(rules) is not int or rules != LOTTERY_RULES:
        raise Gate('node does not report V2 lottery rules (lotteryRules=%r): %s' % (rules, host))
    if info.get('protocolVersion') != PROTOCOL_VERSION:
        raise Gate('node protocol is not %s (protocolVersion=%r): %s'
                   % (PROTOCOL_VERSION, info.get('protocolVersion'), host))
    return {'lotteryRules': rules, 'protocolVersion': info['protocolVersion']}


def v2_award_evidence(inventory, canonical_ids, rows, spent_by_image):
    """Received/spent V2 awards and every campaign-won payout not discovered.

    ``inventory`` is the per-wallet signer inventory, ``canonical_ids`` the
    per-wallet sets of ids that ``canonical_inventory`` accepted, ``rows`` is
    ``lottery_rows(blocks)``. A payout whose winner is campaign-owned must be
    in the winner's wallet, canonical, and under a key image no other owned
    output shares. Read-only; it never repairs or drops evidence.
    """
    owner = {}
    images = {}
    for wallet, owned in enumerate(inventory):
        for output in owned:
            if not isinstance(output, dict) or not isinstance(output.get('id'), str):
                continue
            owner.setdefault(output['id'], []).append((wallet, output))
            image = output.get('key_image')
            images[image] = images.get(image, 0) + 1
    received = []
    undiscovered = []
    for payout, (height, amount, context) in sorted(rows.items()):
        if context is None:
            continue
        entries = owner.get(payout, [])
        winners = {w for w, _ in owner.get(context['winner'], [])}
        wallet, output = entries[0] if len(entries) == 1 else (None, None)
        if (
            output is not None
            and images.get(output.get('key_image')) == 1
            and payout in canonical_ids[wallet]
            and (not winners or wallet in winners)
        ):
            state = spent_by_image.get(output['key_image'], {})
            received.append({'payout': payout, 'wallet': wallet, 'amount': amount,
                             'spent': state.get('spent') is True})
        elif winners or entries:
            # Campaign-won but missing, aliased, non-canonical or in another
            # wallet; or an owned payout that is not uniquely spendable.
            undiscovered.append({'payout': payout, 'winner': context['winner'],
                                 'height': height, 'amount_picocredits': amount,
                                 'owned_entries': len(entries)})
    spent = [r for r in received if r['spent']]
    return {
        'received_count': len(received),
        'received_picocredits': sum(r['amount'] for r in received),
        'spent_count': len(spent),
        'spent_picocredits': sum(r['amount'] for r in spent),
        'unspent_count': len(received) - len(spent),
        'unspent_picocredits': sum(r['amount'] for r in received if not r['spent']),
        'undiscovered': undiscovered,
    }
