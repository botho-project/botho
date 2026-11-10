"""Bounded canonical coin selection with complete real-input exclusion."""

from bisect import bisect_left, bisect_right
from itertools import combinations

from lottery import decoy_row
from signer_history import AGE_SIMILARITY_SPREAD_BPS, MIN_DECOY_AGE

ZERO_KEY = "0" * 64


MAX_PAYMENT_INPUTS = 4
# Multi-input selection searches combinations of the largest this-many
# eligible outputs: <=2516 combinations, bounded irrespective of wallet history.
CANDIDATE_CAP = 16
# Extra small inputs a free-shape single-input payment consolidates (#2023),
# leaving MAX_PAYMENT_INPUTS' last slot for an ``award_input``.
CONSOLIDATE_INPUTS = MAX_PAYMENT_INPUTS - 2


def _amount(output):
    return output["utxo"]["amount"]


def cover(eligible, need, count=None, pool_ok=None, pad=0, paddable=None):
    """Value order of a payment selection (#2023); returns ``(selected, tried)``.

    Since protocol 10.0.0 the fee floor prices the class-exit charge on the
    whole value a spend moves, change included (#1685), so a payment moves as
    little value as covers it:

    - one input (free shape, or ``count == 1``): the smallest eligible output
      of at least ``need``, ties by id. Every output is considered, not only
      the largest sixteen: only the smallest covering one is checked. A
      free-shape payment then also spends up to ``pad`` of the smallest
      ``paddable`` outputs below ``need``, smallest first, each only if the
      total stays within ``max(output, 2 * need)`` and the complete set's
      decoy pools still hold. Spending the smallest sufficient
      output leaves every receipt and change below ``need`` unspent, and
      without this a 72 h wallet ends with its value in dust no four inputs
      can combine into a payment (``payment_shape.coverage``);
    - otherwise ``n`` inputs, ``n = count`` or the fewest of 2..4 that works,
      from the largest ``CANDIDATE_CAP`` eligible outputs: the covering
      combination of least total value, ties by ids.

    A free-shape selection only reaches several inputs when no single eligible
    output covers ``need``, so each input is below ``need``; a padded single
    stays within ``max(output, 2 * need)``. No change output therefore
    outgrows ``max(largest prefunded output, 2 * need)``, and a free-shape
    selection moves at most that or ``MAX_PAYMENT_INPUTS * need``, the bound
    ``payment_shape`` prices offline (plus at most one award). ``pool_ok(selected)`` is the complete-set decoy check;
    ``tried`` counts the selections it was asked about plus the covering
    combinations enumerated. Pure: ``payment_shape``'s workload replay runs
    this same function with ``pool_ok`` always true.
    """
    pool_ok = pool_ok or (lambda selected: True)
    paddable = paddable or (lambda output: True)
    tried = 0
    if count in (None, 1):
        ordered = sorted(eligible, key=lambda o: (_amount(o), o["id"]))
        for output in ordered:
            if _amount(output) < need:
                continue
            tried += 1
            if pool_ok([output]):
                selected = [output]
                # Padding keeps the total within max(output, 2 * need), so
                # change never outgrows that and moved value stays bounded.
                limit = max(_amount(output), 2 * need)
                if count is None:
                    for small in ordered:
                        if len(selected) > pad or _amount(small) >= need:
                            break
                        total = sum(_amount(o) for o in selected) + _amount(small)
                        if total <= limit and paddable(small):
                            tried += 1
                            if pool_ok(selected + [small]):
                                selected.append(small)
                return selected, tried
            if count is None:
                break  # Single inputs never fail on decoys alone; fall through.
        if count == 1:
            return [], tried
    candidates = sorted(eligible, key=lambda o: (-_amount(o), o["id"]))[:CANDIDATE_CAP]
    for n in [count] if count else range(2, MAX_PAYMENT_INPUTS + 1):
        covering = []
        for selected in combinations(candidates, n):
            tried += 1
            total = sum(_amount(o) for o in selected)
            if total >= need:
                covering.append((total, [o["id"] for o in selected], selected))
        covering.sort(key=lambda c: (c[0], c[1]))
        for _, _, selected in covering:
            if pool_ok(list(selected)):
                return list(selected), tried
    return [], tried


def is_v2_award(canonical, output):
    """Whether a canonical inventory output is a V2 lottery payout (#1643)."""
    entry = canonical.get(bytes(output["utxo"]["target_key"]).hex())
    return entry is not None and entry[1].get("lotteryContext") is not None


def thinnest_window_size(created_heights, height, age, removed=()):
    """Fewest decoys in any window the signer can draw for ``age`` (#1692).

    The signer draws ONE random-offset window ``[max(10, s), s + W]`` (ages,
    ``s = age - o``, ``o`` uniform in ``0..=W``, ``W = 2*floor(age*5%)``) and
    refuses below nineteen decoys, so the precheck is the minimum over every
    window, not the envelope. ``created_heights`` is the sorted creation
    height of every eligible decoy key; ``removed`` lists creation heights of
    keys that may not join this ring (the real input and other selected
    inputs), each subtracted from the windows that contain it. Counted with
    bisection over the shared sorted list, never materialized (#1633). The
    count only falls just after the window start passes a decoy age, so the
    minimum is at ``s = age - W`` or ``s = a + 1`` for a decoy age ``a``.
    """
    width = 2 * (age * AGE_SIMILARITY_SPREAD_BPS // 10_000)

    def count(start):
        # Ages max(10, start)..start+W, as creation heights.
        lo, hi = height - (start + width), height - max(start, MIN_DECOY_AGE)
        return (bisect_right(created_heights, hi) - bisect_left(created_heights, lo)
                - sum(1 for r in removed if lo <= r <= hi))

    best = count(age - width)
    # Decoy ages age-W..age-1 are creation heights height-age+1..height-age+W.
    first = bisect_left(created_heights, height - age + 1)
    last = bisect_right(created_heights, height - age + width)
    previous = None
    for i in range(first, last):
        created = created_heights[i]
        if created != previous:
            previous = created
            best = min(best, count(height - created + 1))
    return best


def choose(controller, wallet, height, amount=0, count=None, headroom=0,
           award_input=False):
    """Canonical selection; ``award_input`` adds the oldest mature V2 award.

    Value selection (``cover``: the smallest sufficient output, #2023) never
    reaches a V2 payout (~1.25e8 picocredits, below every payment plus the
    fee headroom), so without ``award_input`` a campaign
    discovers awards but never spends one (#1681). With it, a free-shape
    payment (``count`` None) whose value selection leaves room under
    MAX_PAYMENT_INPUTS also spends the sender's oldest eligible V2 award
    (lowest creation height, then id) whose complete-set decoy pools still
    hold. The choice is a pure function of the scanned state, so a replay
    reproduces it, and the award only adds value: the signer returns it as
    change, so exact accounting is unchanged. Pinned shapes and readiness
    probes (explicit ``count``) are never widened.
    """
    canonical = controller.canonical_history()
    raw = controller.canonical_inventory(wallet, canonical)
    unspent = controller.unspent_setup_inputs(wallet, canonical)
    mature = [o for o in unspent if height - o["utxo"]["created_at"] >= 10]
    # An input's decoy pool is every canonical key (not the zero key, not its
    # own) created inside its age band. Only pool sizes are needed, so they
    # are counted over the sorted creation heights instead of materializing
    # one set per input: that was O(inputs x chain) CPU and memory on every
    # selection, about 1.2 s at x-series h72 (#1633). The counts equal the
    # former ``len(pool)`` and ``len(pool - excluded)`` exactly.
    # Lottery payouts, V2 included, are canonical inputs but never decoys,
    # matching the native signer (#1643).
    created_heights = sorted(
        created
        for key, (created, row) in canonical.items()
        if key != ZERO_KEY and decoy_row(row)
    )

    def in_pool(key, window):
        entry = canonical.get(key) if key != ZERO_KEY else None
        return (
            entry is not None
            and decoy_row(entry[1])
            and window[0] <= entry[0] <= window[1]
        )

    def pool_heights(keys):
        # Creation heights of the given keys that are eligible decoys.
        return [canonical[key][0] for key in keys if in_pool(key, (0, height))]

    pools, eligible = {}, []
    for output in mature:
        age = height - output["utxo"]["created_at"]
        real = bytes(output["utxo"]["target_key"]).hex()
        # The thinnest random-offset window the signer can draw (#1692), not
        # the envelope: an envelope count admits inputs the signer refuses.
        size = thinnest_window_size(created_heights, height, age, pool_heights([real]))
        if size >= 19 + headroom:
            pools[output["id"]] = (age, real, size)
            eligible.append(output)

    recounts = {}

    def pool_ok(output, excluded):
        """Whether every window still holds 19 + headroom without ``excluded``."""
        age, real, size = pools[output["id"]]
        width = 2 * (age * AGE_SIMILARITY_SPREAD_BPS // 10_000)
        # Envelope ages max(10, age-W)..age+W, as creation heights.
        envelope = (height - age - width, height - max(age - width, MIN_DECOY_AGE))
        inside = frozenset(key for key in excluded
                           if key != real and in_pool(key, envelope))
        # Removing k keys lowers any window by at most k: exact when it holds.
        if size - len(inside) >= 19 + headroom:
            return True
        cache = (output["id"], inside)
        if cache not in recounts:
            recounts[cache] = thinnest_window_size(
                created_heights, height, age, pool_heights({real} | inside))
        return recounts[cache] >= 19 + headroom
    eligible.sort(key=lambda o: (-o["utxo"]["amount"], o["id"]))

    def add_award(selected, excluded):
        # Bounded: at most MAX_PAYMENT_INPUTS - 1 candidate awards checked,
        # oldest first; the first whose addition keeps every ring complete.
        if len(selected) >= MAX_PAYMENT_INPUTS or any(
            is_v2_award(canonical, o) for o in selected
        ):
            return None
        chosen = {o["id"] for o in selected}
        images = {o["key_image"] for o in selected}
        awards = sorted(
            (
                o
                for o in eligible
                if is_v2_award(canonical, o)
                and o["id"] not in chosen
                and o["key_image"] not in images
            ),
            key=lambda o: (o["utxo"]["created_at"], o["id"]),
        )
        for award in awards[: MAX_PAYMENT_INPUTS - 1]:
            widened = excluded | {bytes(award["utxo"]["target_key"]).hex()}
            if all(pool_ok(o, widened) for o in selected + [award]):
                selected.append(award)
                return award
        return None
    evidence = {
        "canonical": len(raw),
        "unreserved_unspent": len(unspent),
        "mature": len(mature),
        "eligible": len(eligible),
        "height": height,
        "wallet": wallet,
    }
    if not amount:
        return eligible, evidence
    fee = controller.j.limits["max_single_signed_fee_picocredits"]

    def complete(selected):
        excluded = {bytes(o["utxo"]["target_key"]).hex() for o in selected}
        return all(pool_ok(o, excluded) for o in selected)

    # Smallest sufficient value first (#2023), see cover().
    selected, attempted = cover(eligible, amount + fee, count, complete,
                                CONSOLIDATE_INPUTS,
                                lambda o: not is_v2_award(canonical, o))
    if selected:
        extra = {**evidence, "combinations": attempted}
        if award_input and count is None:
            excluded = {bytes(o["utxo"]["target_key"]).hex() for o in selected}
            award = add_award(selected, excluded)
            extra["award_input"] = award["id"] if award else None
        return selected, extra
    reason = (
        "reservation_or_spent"
        if raw and not unspent
        else "maturity"
        if unspent and not mature
        else "decoys"
        if mature and not eligible
        else "value_or_complete_set_decoys"
    )
    return [], {
        **evidence,
        "reason": reason,
        "combinations": attempted,
        "candidate_cap": CANDIDATE_CAP,
    }
