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

Any other combination is a malformed row and fails closed. The context is
validated as strictly as the native signer's ``LotteryContext`` (P5, #1642):
exactly the five frozen keys (the signer is ``deny_unknown_fields``, so an
extra key would fail every signer request carrying the row; a new field needs
a contract version bump), wire-type ranges (``version`` u8, ``baseIndex`` and
``ordinal`` u32, ``height`` u64), and a nonzero canonical tweak scalar.
"""

import re

from runtime import Gate

LOTTERY_RULES = 2
PROTOCOL_VERSION = '7.0.0'
_HEX64 = re.compile(r'[0-9a-f]{64}')
_OUTPOINT = re.compile(r'([0-9a-fA-F]{64}):(0|[1-9][0-9]{0,9})')


CONTEXT_KEYS = frozenset({'version', 'baseIndex', 'tweak', 'ordinal', 'height'})
# Ristretto255 / ed25519 group order: a canonical scalar is < L, little-endian.
GROUP_ORDER = 2**252 + 27742317777372353535851937790883648493
_U32, _U64 = 1 << 32, 1 << 64


def _natural(value, bound=_U64):
    return type(value) is int and 0 <= value < bound


def canonical_tweak(tweak):
    """The signer's tweak rule: 64 lowercase hex, nonzero, canonical (< L, LE)."""
    if not isinstance(tweak, str) or not _HEX64.fullmatch(tweak):
        return False
    scalar = int.from_bytes(bytes.fromhex(tweak), 'little')
    return 0 < scalar < GROUP_ORDER


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
        or set(context) != CONTEXT_KEYS
        or type(context.get('version')) is not int
        or context['version'] != LOTTERY_RULES
        or not _natural(context['baseIndex'], _U32)
        or not _natural(context['ordinal'], _U32)
        or not _natural(context['height'], _U64)
        or not canonical_tweak(context['tweak'])
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


# Award-spend policy (#1681): free-shape campaign payments also spend the
# sender's oldest mature V2 award (selection.choose ``award_input``). The
# version is journaled at campaign activation; a journal without it ran an
# older harness that never spent awards and is not judged by this rule.
AWARD_SPEND_POLICY = 1
# Received awards beyond which a campaign must have spent at least one. The
# live x1 run received ~700 by T0+11h; 16 tolerates a run that ends before
# awards mature while still catching a policy that never spends.
DEFAULT_MIN_RECEIVED_FOR_SPEND = 16
FINAL_STATUSES = ('draining', 'complete', 'incomplete')


def spend_expectation_threshold(plan):
    """The plan's pinned threshold, else the default; validated strictly."""
    block = (plan or {}).get('lottery_v2')
    if block is None:
        return DEFAULT_MIN_RECEIVED_FOR_SPEND
    if not isinstance(block, dict) or set(block) != {'min_received_for_spend'}:
        raise ValueError('lottery_v2 block must hold exactly min_received_for_spend')
    value = block['min_received_for_spend']
    if type(value) is not int or not 0 <= value <= 100_000:
        raise ValueError('lottery_v2.min_received_for_spend must be an integer 0-100000')
    return value


def award_spend_expectation(journal):
    """``received_count > N`` and ``spent_count == 0`` at campaign end fails.

    Statuses: ``not_evaluated`` (no policy marker: older harness), ``pending``
    (no accounting yet, or zero spends before the campaign ends),
    ``not_applicable`` (at most N awards received), ``pass``, ``fail``.
    """
    policy = journal.get('award_spend_policy')
    if not isinstance(policy, dict) or policy.get('version') != AWARD_SPEND_POLICY:
        return {'status': 'not_evaluated',
                'reason': 'campaign predates the #1681 award-spend policy'}
    threshold = policy['min_received']
    v2 = (journal.get('accounting') or {}).get('lottery_v2')
    if not isinstance(v2, dict):
        return {'status': 'pending', 'min_received': threshold}
    received, spent = v2.get('received_count', 0), v2.get('spent_count', 0)
    out = {'min_received': threshold, 'received_count': received, 'spent_count': spent}
    if spent > 0:
        return {**out, 'status': 'pass'}
    if received <= threshold:
        return {**out, 'status': 'not_applicable'}
    final = journal.get('status') in FINAL_STATUSES
    return {**out, 'status': 'fail' if final else 'pending'}
