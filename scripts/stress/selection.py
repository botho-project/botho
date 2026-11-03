"""Bounded canonical coin selection with complete real-input exclusion."""

from bisect import bisect_left, bisect_right
from itertools import combinations

from lottery import decoy_row
from signer_history import AGE_SIMILARITY_SPREAD_BPS, MIN_DECOY_AGE

ZERO_KEY = "0" * 64


MAX_PAYMENT_INPUTS = 4


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

    Largest-value-first never reaches a V2 payout (~1.25e8 picocredits next
    to ~1e10 principal outputs), so without ``award_input`` a campaign
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
    # Take the largest sixteen candidates: <=2516 complete selections, bounded
    # irrespective of wallet history. Missing alternatives are generator limits.
    candidates = eligible[:16]
    shapes = [count] if count else range(1, 5)
    attempted = 0
    for n in shapes:
        for selected in combinations(candidates, n):
            attempted += 1
            if sum(o["utxo"]["amount"] for o in selected) < amount + fee:
                continue
            excluded = {bytes(o["utxo"]["target_key"]).hex() for o in selected}
            if all(pool_ok(o, excluded) for o in selected):
                selected = list(selected)
                extra = {**evidence, "combinations": attempted}
                if award_input and count is None:
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
        "candidate_cap": 16,
    }
