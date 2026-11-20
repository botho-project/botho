"""Fee claims require observed evidence; nominal fee tiers prove no ordering."""

from fractions import Fraction
import math

from runtime import Gate

# bth_cluster_tax::RELAY_MIN_TX_FEE, also enforced by the offline signer.
RELAY_MIN_TX_FEE = 100_000_000


def signed_fee_evidence(info, requested):
    for key in ("fee", "baseline_fee", "fee_multiplier", "bytes", "input_count"):
        if type(info.get(key)) is not int or info[key] <= 0:
            raise Gate("missing/invalid signer fee evidence: " + key)
    if requested not in (1, 2, 4) or info["fee_multiplier"] != requested:
        raise Gate("signer fee tier differs from request")
    if (
        info["baseline_fee"] < RELAY_MIN_TX_FEE
        or info["fee"] < info["baseline_fee"] * requested
    ):
        raise Gate("signed fee is below multiplied production floor")
    return {
        "paid_fee": info["fee"],
        "bytes": info["bytes"],
        "raw_wire_density": info["fee"] / info["bytes"],
        "baseline_fee": info["baseline_fee"],
        "fee_multiplier": requested,
        "floor_masked": info["baseline_fee"] == RELAY_MIN_TX_FEE,
        "dust_absorbed": info["fee"] - info["baseline_fee"] * requested,
    }


def quote_evidence(quote, status, host, at):
    for key in ("baseRate", "baseMin", "baseMax"):
        if type(quote.get(key)) is not int or quote[key] <= 0:
            raise Gate("invalid fee quote: " + key)
    if not quote["baseMin"] <= quote["baseRate"] <= quote["baseMax"]:
        raise Gate("fee quote outside advertised bounds")
    if (
        type(quote.get("adjustmentActive")) is not bool
        or type(quote.get("congestion")) not in (int, float)
        or not 0 <= quote["congestion"] <= 1
    ):
        raise Gate("missing fee activation/fullness evidence")
    return {
        **quote,
        "at": at,
        "host": host,
        "height": status["chainHeight"],
        "slot_seconds": status.get("effectiveSlotDurationSecs"),
    }


def quote_coverage(quotes):
    # Archived samples may arrive out of timestamp order. Match recovery on
    # the same host at both a strictly later time and a strictly greater height.
    timelines = {}
    active_hosts = set()
    count = 0
    for quote in quotes:
        count += 1
        active = (quote.get("adjustmentActive") is True
                  and quote.get("slot_seconds") == 3
                  and quote.get("congestion", 0) > 0.75
                  and quote.get("baseRate", 0) > quote.get("baseMin", 0))
        at_floor = (quote.get("congestion", 1) <= 0.75
                    and quote.get("baseRate") == quote.get("baseMin"))
        host = quote.get("host")
        if active:
            active_hosts.add(host)
        if active or at_floor:
            at, height = quote.get("at"), quote.get("height")
            if (host and type(at) in (int, float) and math.isfinite(at)
                    and type(height) is int and height >= 0):
                timelines.setdefault(host, []).append((at, active, height))

    recovered = False
    for host in active_hosts:
        lowest = None
        # Floor samples (False) sort before activations at the same time.
        for _, active, height in sorted(timelines.get(host, ())):
            if active:
                lowest = height if lowest is None else min(lowest, height)
            elif lowest is not None and height > lowest:
                recovered = True
                break
        if recovered:
            break
    return {
        "dynamic_activation": "observed" if active_hosts else "not_exercised",
        "quote_recovery": "observed" if recovered else "not_exercised",
        "quote_samples": count,
    }


def summarize_fees(rows, quotes):
    import json

    tiers = {}
    cohorts = {}
    floor_masked = 0
    total_fee = 0
    for row in rows:
        info = (
            json.loads(row["info"])
            if isinstance(row.get("info"), str)
            else row.get("info", {})
        )
        if row.get("state") != "reconciled" or "fee_evidence" not in info:
            continue
        evidence = signed_fee_evidence(info, info.get("fee_multiplier"))
        if evidence != info["fee_evidence"] or row["fee"] != info["fee"]:
            raise Gate("recorded fee evidence differs from paid signed fee")
        if info.get("cohort") is not None:
            key = (info["cohort"], info["input_count"], len(info.get("outputs", [])))
            cohorts.setdefault(key, set()).add(info["fee_multiplier"])
        tier = tiers.setdefault(
            str(info["fee_multiplier"]),
            {"reconciled": 0, "paid_fee": 0, "raw_wire_bytes": 0},
        )
        tier["reconciled"] += 1
        tier["paid_fee"] += row["fee"]
        tier["raw_wire_bytes"] += info["bytes"]
        floor_masked += evidence["floor_masked"]
        total_fee += row["fee"]
    matched = sum(multipliers == {1, 2, 4} for multipliers in cohorts.values())
    return {
        "tiers": tiers,
        "signed_tiers": "observed" if matched else "not_exercised",
        "matched_shape_cohorts": matched,
        **quote_coverage(quotes),
        # Different signing fees or confirmations in different blocks do not
        # establish competing admission in the same proposal opportunity.
        "priority": "not_exercised",
        "priority_reason": "No same-opportunity mempool contention/selection trace; latency alone is not priority proof.",
        "floor_masked_payments": floor_masked,
        "paid_fee_total": total_fee,
        "coverage_complete": False,
    }


def selected_value(info):
    selected = info.get("selected")
    if (not isinstance(selected, list) or not selected
            or type(info.get("input_count")) is not int or len(selected) != info["input_count"]):
        return None
    if any(not isinstance(o, dict) or type(o.get("amount")) is not int
           or o["amount"] <= 0 for o in selected):
        return None
    return sum(o["amount"] for o in selected)


def complete_percentile(samples, signed):
    result = {"signed": signed, "samples": len(samples), "p99": None}
    if samples and len(samples) == signed:
        result["p99"] = sorted(samples, key=lambda pair: pair[0])[(99 * signed + 99) // 100 - 1][1]
    return result


def resign_fee(relay_floor):
    """The wallet envelope, pinned against the Rust fee-shape golden rows."""
    return max(RELAY_MIN_TX_FEE, min(2**64 - 1, relay_floor + 2 * (relay_floor // 64)))


def floor_quote_summary(records, *, per_input_value):
    """Nearest-rank p99 of settling node quotes, once per signed transaction.

    Payment observations are ranked by the exact ratio of re-sign quote to
    selected input value (including change). Prefund uses the absolute quote.
    Partial archives retain their counts and cannot yield a qualifying p99.
    """
    samples = []
    signed = 0
    for info in records:
        signed += 1
        if not isinstance(info, dict):
            continue
        quote = info.get("fee_floor_quote")
        fee = quote.get("resignFee") if isinstance(quote, dict) else None
        if type(fee) is not int or fee <= 0:
            continue
        moved = selected_value(info) if per_input_value else 1
        if moved is None:
            continue
        observation = {"resign_fee_picocredits": fee}
        if per_input_value:
            observation["selected_input_picocredits"] = moved
        samples.append(((Fraction(fee, moved), fee, moved), observation))
    return complete_percentile(samples, signed)


def payment_consensus_summary(records):
    """Rank the fixed-base consensus floor per picocredit of outputs, including change."""
    samples = []
    signed = 0
    max_selected = 0
    for info in records:
        signed += 1
        if not isinstance(info, dict):
            continue
        moved = selected_value(info)
        fee = info.get("fee")
        quote = info.get("fee_floor_quote")
        if not isinstance(quote, dict) or moved is None or type(fee) is not int or not 0 < fee < moved < 2**64:
            continue
        floor = quote.get("relayFeeFloor")
        consensus = quote.get("consensusFeeFloor")
        if type(floor) is not int or type(consensus) is not int or not 0 < consensus <= floor < 2**64:
            continue
        minimum = max(RELAY_MIN_TX_FEE, floor)
        settle = max(RELAY_MIN_TX_FEE, min(2**64 - 1, floor + floor // 64))
        if (any(type(quote.get(k)) is not int for k in ("minimumFee", "settleFee", "resignFee"))
                or quote.get("minimumFee") != minimum or quote.get("settleFee") != settle
                or quote.get("resignFee") != resign_fee(floor) or fee < settle):
            continue
        if info["input_count"] > 4:
            continue
        output_value = moved - fee
        if "outputs" in info:
            outputs = info["outputs"]
            if (not isinstance(outputs, list) or not 1 <= len(outputs) <= 2
                    or any(not isinstance(o, dict) or type(o.get("amount")) is not int or o["amount"] <= 0
                           for o in outputs) or sum(o["amount"] for o in outputs) != output_value):
                continue
        max_selected = max(max_selected, moved)
        # A capitalized downgrade subtracts two individually floored charges.
        # Its unrounded value is less than the observed charge plus one.
        samples.append(((Fraction(consensus + 1, output_value), consensus, output_value),
                        {"consensus_floor_picocredits": consensus, "output_value_picocredits": output_value}))
    return {**complete_percentile(samples, signed), "max_selected_input_picocredits": max_selected or None}
