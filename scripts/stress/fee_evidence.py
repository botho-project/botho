"""Fee claims require observed evidence; nominal fee tiers prove no ordering."""

from runtime import Gate


def signed_fee_evidence(info, requested):
    for key in ("fee", "baseline_fee", "fee_multiplier", "bytes", "input_count"):
        if type(info.get(key)) is not int or info[key] <= 0:
            raise Gate("missing/invalid signer fee evidence: " + key)
    if requested not in (1, 2, 4) or info["fee_multiplier"] != requested:
        raise Gate("signer fee tier differs from request")
    if (
        info["baseline_fee"] < 100_000_000
        or info["fee"] < info["baseline_fee"] * requested
    ):
        raise Gate("signed fee is below multiplied production floor")
    return {
        "paid_fee": info["fee"],
        "bytes": info["bytes"],
        "raw_wire_density": info["fee"] / info["bytes"],
        "baseline_fee": info["baseline_fee"],
        "fee_multiplier": requested,
        "floor_masked": info["baseline_fee"] == 100_000_000,
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


def summarize_fees(rows, quotes):
    import json

    confirmed = []
    for row in rows:
        info = (
            json.loads(row["info"])
            if isinstance(row.get("info"), str)
            else row.get("info", {})
        )
        if row.get("state") == "reconciled" and "fee_evidence" in info:
            evidence = signed_fee_evidence(info, info.get("fee_multiplier"))
            if evidence != info["fee_evidence"] or row["fee"] != info["fee"]:
                raise Gate("recorded fee evidence differs from paid signed fee")
            confirmed.append((row, info))
    cohorts = {}
    for row, info in confirmed:
        key = (info.get("cohort"), info["input_count"], len(info.get("outputs", [])))
        if info.get("cohort") is not None:
            cohorts.setdefault(key, set()).add(info["fee_multiplier"])
    matched = sum(tiers == {1, 2, 4} for tiers in cohorts.values())
    activation = [
        q
        for q in quotes
        if q.get("adjustmentActive") is True
        and q.get("slot_seconds") == 3
        and q.get("congestion", 0) > 0.75
        and q.get("baseRate", 0) > q.get("baseMin", 0)
    ]
    recovery = any(
        q.get("host") == a.get("host")
        and q["at"] > a["at"]
        and q.get("height", 0) > a.get("height", 0)
        and q.get("congestion", 1) <= 0.75
        and q.get("baseRate") == q.get("baseMin")
        for a in activation
        for q in quotes
    )
    tiers = {}
    for row, info in confirmed:
        tier = tiers.setdefault(
            str(info["fee_multiplier"]),
            {"reconciled": 0, "paid_fee": 0, "raw_wire_bytes": 0},
        )
        tier["reconciled"] += 1
        tier["paid_fee"] += row["fee"]
        tier["raw_wire_bytes"] += info["bytes"]
    return {
        "tiers": tiers,
        "signed_tiers": "observed" if matched else "not_exercised",
        "matched_shape_cohorts": matched,
        "dynamic_activation": "observed" if activation else "not_exercised",
        "quote_recovery": "observed" if recovery else "not_exercised",
        # Different signing fees or confirmations in different blocks do not
        # establish competing admission in the same proposal opportunity.
        "priority": "not_exercised",
        "priority_reason": "No same-opportunity mempool contention/selection trace; latency alone is not priority proof.",
        "quote_samples": len(quotes),
        "floor_masked_payments": sum(
            i["fee_evidence"]["floor_masked"] for _, i in confirmed
        ),
        "paid_fee_total": sum(r["fee"] for r, _ in confirmed),
        "coverage_complete": False,
    }
