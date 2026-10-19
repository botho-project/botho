"""Bounded canonical coin selection with complete real-input exclusion."""

from itertools import combinations


def choose(controller, wallet, height, amount=0, count=None, headroom=0):
    canonical = controller.canonical_history()
    raw = controller.canonical_inventory(wallet, canonical)
    unspent = controller.unspent_setup_inputs(wallet, canonical)
    mature = [o for o in unspent if height - o["utxo"]["created_at"] >= 10]
    pools, eligible = {}, []
    for output in mature:
        age = height - output["utxo"]["created_at"]
        low, high = max(10, age - age // 10), age + age // 10
        real = bytes(output["utxo"]["target_key"]).hex()
        pool = {
            key
            for key, (created, _) in canonical.items()
            if key != "0" * 64 and low <= height - created <= high
        } - {real}
        if len(pool) >= 19 + headroom:
            pools[output["id"]] = pool
            eligible.append(output)
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
            if all(len(pools[o["id"]] - excluded) >= 19 + headroom for o in selected):
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
