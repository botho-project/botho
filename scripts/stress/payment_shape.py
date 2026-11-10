#!/usr/bin/env python3
"""x-series payment and prefund shape under the protocol 10.0.0 fee floor (#2023).

Offline only: never connects, signs or submits. Python standard library only.

Since protocol 10.0.0 every spend owes the class-exit charge on the whole
value it moves, change included (#1685), and the stress harness pays the
node-quoted floor (#2001). A payment that spends a whole 1 BTH grant owes about
2e8 to 3.4e9 picocredits, above the x-series ``max_single`` of 1e9, so the
plan pins a prefund of many small grants (``plan["prefund"]``) and selection
spends the smallest sufficient output (``selection.cover``). This module checks
that shape against the plan before anything is staged:

``fee_check``
    Prices every planned shape with the real fee function: the golden rows in
    ``fixtures/fee-shape-golden.json``, generated and pinned by
    ``cargo test -p bth-cluster-tax --test stress_fee_shape`` (never a Python
    copy of the formula). A shape is priced at the next larger golden ``moved``
    row (the Rust test asserts the floor is monotone in value moved). At every
    fee tier the signer may use, ``tier x resign_fee`` (``stress.rs``
    ``bounded_fee``: the fee is the multiplier times the node's re-sign quote)
    times ``FEE_MARGIN`` must fit the cap, for every golden cluster wealth up
    to 1M BTH, at one wallet hop (``hop``: the outputs keep the ring's class
    less the wallet's 5% tag decay). A prefund batch is also gated at its ring
    downgrade (``ring``: a fresh coinbase's batch, claiming one hop from a 50
    BTH cluster, against a value-weighted ring centroid at the largest
    cluster wealth the prefunder can build, ``ring_bound_bth``). Rows that
    declare background against a wealthy ring (``exit``) are reported, not
    gated: the operator freezes the plan from the Part A rehearsal's p99
    quoted fee per BTH moved instead.

``coverage``
    Replays the rehearsal and the plan's whole 72 h schedule (``expand``'s
    exact senders, recipients, amounts and pinned input counts) through
    ``selection.cover`` (which reserves ``max_single`` per payment), paying
    ``FEE_MARGIN`` times the gated fee bound of each spend, every output,
    change and receipt alike, spendable only ``MATURITY_SECONDS`` after its
    payment. Every payment must find inputs, every replayed spend's gated fee
    must fit ``max_single`` (never clipped), and every replayed spend must move
    no more than a priced shape with at least as many inputs.

``prefund_check``
    The batch cap, its fee budget and the funding wallet's opening reserve,
    and the batch count against the fresh-chain decoy pace of #1937.
"""

import argparse
import json
from pathlib import Path

GOLDEN = Path(__file__).with_name("fixtures") / "fee-shape-golden.json"
# stress.rs `deserialize_fee_multiplier`: the signer's fee tiers.
TIERS = (1, 2, 4)
# Every gated fee must fit its cap twice over.
FEE_MARGIN = 2
GATED_DEPTH = "hop"
# Gated for prefund batches only, up to ring_bound_bth.
RING_DEPTH = "ring"
# Pinned probes (discovery.py, plan.expand): input counts.
PROBE_INPUTS = (1, 2, 4)
# prefund.GRANTS_PER_BATCH (stress.rs MAX_GRANT_OUTPUTS): 15 grants and change.
GRANTS_PER_BATCH = 15
# Seconds from a payment to its outputs being selectable again. At the
# sustained phase's 4 payments a minute and ~40 s blocks a chain carries about
# six decoy-eligible outputs a block, so the +/-5% random-offset window (#1692)
# holds 19 + decoy_headroom decoys at an age of about 40 blocks (1600 s);
# this doubles it, and every replayed output waits it out.
MATURITY_SECONDS = 3600
# #1937 fresh-chain pace: the first funding input is decoy-eligible near
# height 220 on a one-coinbase-per-block chain, then about one per block.
FIRST_FUNDING_BLOCK = 220
FRESH_BLOCK_SECONDS = 40
# The prefund (fresh chain to the last batch, at that pessimistic pace) must
# fit this share of the window prefund.latest_deadline leaves in the shortest
# campaign lifetime discovery_plan.validate_lifetime admits.
PREFUND_WINDOW_SHARE = 0.75
SHORTEST_LIFETIME_SECONDS = 80.5 * 3600


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load_golden(path=GOLDEN):
    golden = json.loads(Path(path).read_text())
    columns = golden["columns"]
    golden["rows"] = [dict(zip(columns, row)) for row in golden["rows"]]
    return golden


def prefund_shape(plan):
    """The plan's pinned prefund block, validated: (grants per wallet, grant, batch cap)."""
    block = plan.get("prefund")
    require(isinstance(block, dict) and set(block) == {
        "grants_per_wallet", "grant_picocredits", "max_batch_fee_picocredits"},
        "prefund block requires grants_per_wallet, grant_picocredits, max_batch_fee_picocredits")
    grants = block["grants_per_wallet"]
    require(type(grants) is int and grants >= plan["wallets"]["min_mature_utxos_per_wallet"],
            "prefund grants_per_wallet below min_mature_utxos_per_wallet")
    values = []
    for key in ("grant_picocredits", "max_batch_fee_picocredits"):
        value = block[key]
        require(isinstance(value, str) and value.isdecimal() and int(value) > 0,
                "prefund " + key + ": positive integer string required")
        values.append(int(value))
    return grants, values[0], values[1]


def batches(plan):
    grants, _, _ = prefund_shape(plan)
    return -(-plan["wallets"]["count"] * grants // GRANTS_PER_BATCH)


def ring_bound_bth(golden):
    """Largest cumulative wealth of one prefund funding cluster, BTH (#2023 review).

    The prefunder spends mature coinbases first and never change at
    ``prefund.MAX_FUNDING_DEPTH`` (``Prefunder.funding_rank``). Cluster wealth
    only accumulates (each output adds its tagged value), and each batch hop
    carries at most (1 - decay) of the cluster's tagged value on, so one
    coinbase cluster holds at most coinbase x sum((1 - decay)^k, k = 0..depth).
    Only the producer mints on an x-series chain and the campaign wallets
    spend nothing before the prefund ends, so this bounds every ring member's
    effective wealth, and with it the value-weighted ring centroid, while
    batches are signed.
    """
    from prefund import MAX_FUNDING_DEPTH
    keep = 1 - golden["wallet_decay_rate"] / golden["tag_weight_scale"]
    coinbase_bth = golden["coinbase_picocredits"] / golden["pico_per_bth"]
    return coinbase_bth * sum(keep ** k for k in range(MAX_FUNDING_DEPTH + 1))


def largest_output(plan):
    """The largest output a campaign wallet holds, by construction.

    A grant; a receipt (at most the largest transfer, below ``need``); change
    of a padded single (below ``max(output, 2 * need)``); or the change of a
    pinned probe, below the probe's inputs, each at most a grant: the four-input
    probe's change (up to about 7.7e10) is the largest. The workload replay
    (``coverage``) checks this against every spend it makes.
    """
    _, grant, _ = prefund_shape(plan)
    wallets = plan["wallets"]
    need = (max(map(int, wallets["transfer_amounts_picocredits"]))
            + int(plan["limits"]["max_single_signed_fee_picocredits"]))
    return max(grant, 2 * need, max(PROBE_INPUTS) * max(grant, need))


def planned_shapes(plan, golden):
    """Every spend shape the plan can sign: (name, inputs, outputs, moved, tiers, cap, ring).

    ``ring`` is the cluster wealth (BTH) up to which ``ring`` rows are gated,
    or None for shapes gated at ``hop`` only.
    """
    _, grant, batch_cap = prefund_shape(plan)
    limits, wallets = plan["limits"], plan["wallets"]
    max_single = int(limits["max_single_signed_fee_picocredits"])
    need = max(map(int, wallets["transfer_amounts_picocredits"])) + max_single
    inputs = wallets["max_payment_inputs"]
    largest = largest_output(plan)
    # A one-input payment spends the smallest covering output, which can be
    # any output up to `largest` (a probe's change included). A free shape
    # spends one covering output padded to at most max(output, 2 * need), or
    # several outputs each below `need` (selection.cover); an award input
    # (~1.25e8) rides on the next golden row up.
    shapes = [("payment, one input", 1, 2, largest, TIERS, max_single, None),
              ("payment, free shape", inputs, 2, max(largest, inputs * need), TIERS,
               max_single, None)]
    # Pinned rehearsal/correctness probes: (1, 2, 4) inputs at every tier; the
    # least-value covering set of n inputs, each at most a grant while the
    # wallet holds n grants (the replay checks the value they move).
    probe_input = max(grant, need)
    for n in PROBE_INPUTS[1:]:
        shapes.append(("probe, %d inputs" % n, n, 2, n * probe_input, TIERS, max_single, None))
    # A prefund batch spends one coinbase (or more if 15 grants plus the fee
    # cap exceed one) at multiplier 1 (stress.rs prepare_grants), among decoys
    # up to the prefunder's cluster-wealth bound.
    coinbase = golden["coinbase_picocredits"]
    batch_inputs = -(-(GRANTS_PER_BATCH * grant + batch_cap) // coinbase)
    shapes.append(("prefund batch", batch_inputs, GRANTS_PER_BATCH + 1,
                   batch_inputs * coinbase, (1,), batch_cap, ring_bound_bth(golden)))
    return shapes


def price(golden, inputs, outputs, moved):
    """Golden rows for the shape, at the next larger `moved` row."""
    rows = [r for r in golden["rows"] if r["inputs"] == inputs and r["outputs"] == outputs]
    require(rows, "no golden rows for %d inputs and %d outputs: extend SHAPES in "
            "cluster-tax/tests/stress_fee_shape.rs" % (inputs, outputs))
    above = sorted({r["moved"] for r in rows if r["moved"] >= moved})
    require(above, "value moved %d exceeds the golden grid: extend MOVED in "
            "cluster-tax/tests/stress_fee_shape.rs" % moved)
    return [r for r in rows if r["moved"] == above[0]]


def ring_wealth_row(golden, bound):
    """The golden wealth row a ring bound is priced at: the next one up."""
    above = sorted({r["wealth_bth"] for r in golden["rows"] if r["depth"] == RING_DEPTH
                    and r["wealth_bth"] >= bound})
    require(above, "ring bound %.1f BTH exceeds the golden grid: extend WEALTH_BTH in "
            "cluster-tax/tests/stress_fee_shape.rs" % bound)
    return above[0]


def fee_check(plan, golden=None):
    """Gate every planned shape at every tier; returns the report rows."""
    golden = golden or load_golden()
    report, failures = [], []
    for name, inputs, outputs, moved, tiers, cap, ring in planned_shapes(plan, golden):
        rows = price(golden, inputs, outputs, moved)
        ring_row = None
        if ring is not None:
            require(any(r["depth"] == RING_DEPTH for r in rows),
                    "no golden ring rows for %d inputs and %d outputs" % (inputs, outputs))
            ring_row = ring_wealth_row(golden, ring)
        for row in rows:
            gated = (row["depth"] == GATED_DEPTH
                     or (ring_row is not None and row["depth"] == RING_DEPTH
                         and row["wealth_bth"] <= ring_row))
            if row["depth"] == RING_DEPTH and ring_row is None:
                continue
            for tier in tiers:
                fee = tier * row["resign_fee"]
                entry = {"shape": name, "inputs": inputs, "outputs": outputs, "moved": moved,
                         "priced_at_moved": row["moved"], "wealth_bth": row["wealth_bth"],
                         "depth": row["depth"], "tier": tier, "fee": fee, "cap": cap,
                         "margin": round(cap / fee, 2), "gated": gated}
                if ring_row is not None:
                    entry["ring_bound_bth"] = round(ring, 3)
                report.append(entry)
                if gated and fee * FEE_MARGIN > cap:
                    failures.append(entry)
    if failures:
        raise ValueError("planned shape over its fee cap with margin %d: %s" % (
            FEE_MARGIN, json.dumps(failures[0], sort_keys=True)))
    return report


def summarize(report):
    """Worst gated and worst reported fee per (shape, tier)."""
    worst = {}
    for entry in report:
        key = (entry["shape"], entry["tier"])
        slot = worst.setdefault(key, {"shape": entry["shape"], "tier": entry["tier"],
                                      "cap": entry["cap"], "moved": entry["moved"]})
        for depth in ("hop", "exit", "ring"):
            if depth == "ring" and not entry["gated"]:
                continue  # Ring rows past the prefunder's bound are unreachable.
            if entry["depth"] == depth and entry["fee"] > slot.get(depth + "_fee", -1):
                slot[depth + "_fee"] = entry["fee"]
                slot[depth + "_wealth_bth"] = entry["wealth_bth"]
            if entry["depth"] == depth == "ring":
                slot["ring_bound_bth"] = entry["ring_bound_bth"]
    for slot in worst.values():
        slot["hop_margin"] = round(slot["cap"] / slot["hop_fee"], 2)
        if "ring_fee" in slot:
            slot["ring_margin"] = round(slot["cap"] / slot["ring_fee"], 2)
        # Largest cluster wealth whose full exit still fits the cap.
        fits = [e["wealth_bth"] for e in report if e["shape"] == slot["shape"]
                and e["tier"] == slot["tier"] and e["depth"] == "exit" and e["fee"] <= slot["cap"]]
        slot["exit_fits_cap_to_wealth_bth"] = max(fits) if fits else 0
    return list(worst.values())


def rehearsal_events(plan):
    """discovery.py's rehearsal offers: sender i, recipient i+1, three-offer cohorts."""
    settings, count = plan["rehearsal"], plan["wallets"]["count"]
    amounts = plan["wallets"]["transfer_amounts_picocredits"]
    total = (settings["duration_seconds"] - 120) // settings["interval_seconds"]
    start = -(settings["duration_seconds"] + settings["drain_seconds"])
    return [{"offset_seconds": start + i * settings["interval_seconds"],
             "sender": "wallet-%d" % (i % count + 1),
             "recipient": "wallet-%d" % ((i + 1) % count + 1),
             "amount_picocredits": amounts[(i // 3) % len(amounts)],
             "input_count": (1, 2, 4)[i // 3] if i < 9 else None,
             "fee_multiplier": (1, 2, 4)[i % 3]}
            for i in range(total)]


def gated_fee(golden, inputs, moved, tier):
    """``FEE_MARGIN`` times the worst gated (``hop``) fee of a payment shape."""
    rows = price(golden, inputs, 2, moved)
    return FEE_MARGIN * tier * max(r["resign_fee"] for r in rows if r["depth"] == GATED_DEPTH)


def coverage(plan, events=None, golden=None):
    """Replay the rehearsal and campaign through selection.cover; returns evidence.

    Raises ValueError on the first replayed spend whose gated fee (times
    ``FEE_MARGIN`` and its tier) exceeds ``max_single``, or that moves more
    than every planned shape with at least as many inputs is priced at.
    """
    from plan import expand
    from selection import CONSOLIDATE_INPUTS, cover

    golden = golden or load_golden()
    grants, grant, _ = prefund_shape(plan)
    if events is None:
        events = expand(plan)[1]
    max_single = int(plan["limits"]["max_single_signed_fee_picocredits"])
    count = plan["wallets"]["count"]
    serial = [0]

    def output(amount, mature):
        serial[0] += 1
        return {"id": "%012d" % serial[0], "mature": mature, "utxo": {"amount": amount}}

    # The value moved each input count is priced at: the largest planned
    # shape with at least that many inputs.
    shapes = planned_shapes(plan, golden)
    priced = {k: max((s[3] for s in shapes if s[6] is None and s[1] >= k), default=0)
              for k in range(1, plan["wallets"]["max_payment_inputs"] + 1)}
    wallets = [[output(grant, float("-inf")) for _ in range(grants)] for _ in range(count)]
    failures, multi, moved, spends, fees = [], 0, 0, 0, 0
    moved_by_inputs, worst_fee = {}, 0
    for event in sorted(rehearsal_events(plan) + list(events), key=lambda e: e["offset_seconds"]):
        at = event["offset_seconds"]
        sender = int(event["sender"].split("-")[1]) - 1
        recipient = int(event["recipient"].split("-")[1]) - 1
        amount = int(event["amount_picocredits"])
        eligible = [o for o in wallets[sender] if o["mature"] <= at]
        selected, _ = cover(eligible, amount + max_single, event.get("input_count"),
                            pad=CONSOLIDATE_INPUTS)
        if not selected:
            failures.append({"at": at, "sender": event["sender"], "amount": amount,
                             "count": event.get("input_count"),
                             "balance": sum(o["utxo"]["amount"] for o in wallets[sender])})
            continue
        spends += 1
        multi += len(selected) > 1
        total = sum(o["utxo"]["amount"] for o in selected)
        moved = max(moved, total)
        moved_by_inputs[len(selected)] = max(moved_by_inputs.get(len(selected), 0), total)
        spend = {"at": at, "sender": event["sender"], "inputs": len(selected), "moved": total,
                 "count": event.get("input_count"), "tier": event["fee_multiplier"]}
        require(total <= priced.get(len(selected), 0),
                "replayed spend moves more than any priced shape: " + json.dumps(spend, sort_keys=True))
        chosen = {o["id"] for o in selected}
        wallets[sender] = [o for o in wallets[sender] if o["id"] not in chosen]
        # Selection reserves max_single; the replay pays the gated fee bound
        # twice over, so the drain is FEE_MARGIN x the planned worst case. A
        # spend over the cap fails the gate: it is never clipped (#2023 review).
        fee = gated_fee(golden, len(selected), total, event["fee_multiplier"])
        require(fee <= max_single, "replayed spend's gated fee %d exceeds max_single %d: %s"
                % (fee, max_single, json.dumps(spend, sort_keys=True)))
        worst_fee = max(worst_fee, fee)
        fees += fee
        change = total - amount - fee
        if change > 0:
            wallets[sender].append(output(change, at + MATURITY_SECONDS))
        wallets[recipient].append(output(amount, at + MATURITY_SECONDS))
    balances = [sum(o["utxo"]["amount"] for o in w) for w in wallets]
    return {"payments": spends + len(failures), "covered": spends, "uncovered": len(failures),
            "first_uncovered": failures[0] if failures else None,
            "several_inputs": multi, "largest_value_moved": moved,
            "largest_value_moved_by_inputs": {str(k): v for k, v in sorted(moved_by_inputs.items())},
            "priced_value_moved_by_inputs": {str(k): v for k, v in sorted(priced.items())},
            "worst_gated_fee": worst_fee,
            "min_closing_balance": min(balances), "maturity_seconds": MATURITY_SECONDS,
            "replayed_fees": fees}


def prefund_check(plan, golden=None):
    """Batch fee budget, opening reserve and fresh-chain pace."""
    golden = golden or load_golden()
    grants, grant, batch_cap = prefund_shape(plan)
    limits, wallets = plan["limits"], plan["wallets"]
    max_single = int(limits["max_single_signed_fee_picocredits"])
    need = max(map(int, wallets["transfer_amounts_picocredits"])) + max_single
    require(grant >= need, "prefund grant below the largest transfer plus max_single")
    require(batch_cap >= max_single, "prefund batch cap below max_single")
    from bounds import CEILINGS
    require(batch_cap <= CEILINGS["max_single_signed_fee_picocredits"],
            "prefund batch cap above the signer's program bound")
    count, n = wallets["count"], batches(plan)
    principal = count * grants * grant
    require(principal <= int(plan["funding"]["max_principal_picocredits"]),
            "prefund principal exceeds max_principal_picocredits")
    reserve = n * batch_cap
    require(reserve <= int(limits["max_signed_fee_picocredits"]),
            "worst-case prefund batch fees exceed max_signed_fee_picocredits")
    # #1937: about one decoy-eligible funding input per block after ~220.
    blocks = FIRST_FUNDING_BLOCK + n
    seconds = blocks * FRESH_BLOCK_SECONDS
    from prefund import latest_deadline
    window = latest_deadline(plan, SHORTEST_LIFETIME_SECONDS)
    require(seconds <= PREFUND_WINDOW_SHARE * window,
            "prefund batches cannot finish in %d%% of the %d s prefund window at one "
            "funding input per block" % (100 * PREFUND_WINDOW_SHARE, window))
    return {"grants_per_wallet": grants, "grant_picocredits": grant, "batches": n,
            "principal_picocredits": principal, "batch_fee_cap": batch_cap,
            "worst_case_batch_fees": reserve, "funding_needed": principal + reserve,
            "coinbases_needed": -(-(principal + reserve) // golden["coinbase_picocredits"]),
            "fresh_chain_blocks": blocks, "fresh_chain_seconds": seconds,
            "prefund_window_seconds": window}


def preflight(plan):
    """Everything above; raises ValueError on the first failure."""
    golden = load_golden()
    prefund = prefund_check(plan, golden)
    report = fee_check(plan, golden)
    replay = coverage(plan, golden=golden)
    if replay["uncovered"]:
        raise ValueError("workload replay found an uncovered payment: "
                         + json.dumps(replay["first_uncovered"], sort_keys=True))
    return {"prefund": prefund, "fees": summarize(report), "coverage": replay,
            "fee_margin": FEE_MARGIN, "gated_depth": GATED_DEPTH,
            "ring_gated_to_bth": round(ring_bound_bth(golden), 3),
            "note": "exit rows are reported, not gated: freeze the plan from the Part A "
                    "rehearsal's p99 quoted fee per BTH moved on the pinned build"}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--plan", type=Path, default=Path(__file__).with_name("x-series-plan.json"))
    args = parser.parse_args()
    try:
        result = preflight(json.loads(args.plan.read_text()))
    except (ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
