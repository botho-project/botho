"""Bounded canonical coin selection with complete real-input exclusion."""

from bisect import bisect_left, bisect_right
from itertools import combinations

ZERO_KEY = "0" * 64


def choose(controller, wallet, height, amount=0, count=None, headroom=0):
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
    created_heights = sorted(
        created for key, (created, _) in canonical.items() if key != ZERO_KEY
    )

    def in_pool(key, window):
        entry = canonical.get(key) if key != ZERO_KEY else None
        return entry is not None and window[0] <= entry[0] <= window[1]

    pools, eligible = {}, []
    for output in mature:
        age = height - output["utxo"]["created_at"]
        low, high = max(10, age - age // 10), age + age // 10
        real = bytes(output["utxo"]["target_key"]).hex()
        # low <= height - created <= high, as creation heights.
        window = (height - high, height - low)
        size = bisect_right(created_heights, window[1]) - bisect_left(
            created_heights, window[0]
        )
        size -= in_pool(real, window)
        if size >= 19 + headroom:
            pools[output["id"]] = (window, real, size)
            eligible.append(output)

    def pool_without(output, excluded):
        window, real, size = pools[output["id"]]
        return size - sum(1 for key in excluded if key != real and in_pool(key, window))
    eligible.sort(key=lambda o: (-o["utxo"]["amount"], o["id"]))
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
            if all(pool_without(o, excluded) >= 19 + headroom for o in selected):
                return list(selected), {**evidence, "combinations": attempted}
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
