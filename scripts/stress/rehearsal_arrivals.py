"""Bound receiving-node arrival times from a closed rehearsal journal."""

import argparse
from collections import Counter
from contextlib import closing
import json
import math
from pathlib import Path
import sqlite3

from runtime import HOSTS, digest


def require(condition, message):
    if not condition:
        raise ValueError(message)


def finite(value):
    return type(value) in (int, float) and math.isfinite(value)


def classify(low, high, minimum, maximum):
    """Classify a closed interval against an inclusive plan tolerance."""
    if high < minimum or low > maximum:
        return "miss"
    if low >= minimum and high <= maximum:
        return "within"
    return "indeterminate"


def observations(db, start, interval, count):
    intents = db.execute("""
        SELECT id, offered, submitted, finished, ingress, hash, fee, state, info
        FROM intents WHERE kind='rehearsal' ORDER BY offered, id
    """).fetchall()
    require(len(intents) == count, "incomplete rehearsal offer set")
    payments = []
    previous_response = None
    for i, intent in enumerate(intents):
        identifier = intent["id"]
        info = json.loads(intent["info"])
        offered, marker, response, finished = (
            intent["offered"], intent["submitted"], info.get("response_at"), intent["finished"]
        )
        require(all(finite(t) for t in (offered, marker, response, finished)),
                f"{identifier}: missing or invalid timestamp")
        require(identifier == f"rehearsal-{i:05d}" and offered == start + i * interval,
                f"{identifier}: offer differs from schedule")
        require(offered <= marker <= response <= finished and
                (previous_response is None or previous_response < marker),
                f"{identifier}: overlapping or unordered submission clocks")
        previous_response = response
        receipts = info.get("confirmed_receipts", {})
        require(intent["state"] == "reconciled" and info["hash"] == intent["hash"]
                and intent["ingress"] in HOSTS and set(receipts) == set(HOSTS),
                f"{identifier}: incomplete reconciled payment")
        outputs = info["outputs"]
        expected = ("confirmed", intent["hash"], info["block_height"], intent["fee"],
                    len(outputs), sum(o["amount"] for o in outputs))
        for host, receipt in receipts.items():
            fields = ("status", "txHash", "blockHeight", "fee", "outputCount", "totalOutput")
            require(tuple(receipt.get(k) for k in fields) == expected,
                    f"{identifier}: inconsistent receipt on {host}")
        payments.append(dict(intent=identifier, tx_hash=intent["hash"], host=intent["ingress"],
                             bytes=info["wire_bytes"], offered=offered, marker=marker,
                             latest=response))

    # The controller admits payments serially. Every successful request must
    # match a different, nonoverlapping marker-to-acknowledgment interval.
    calls = db.execute("""
        SELECT id, at, host, bytes, status FROM requests
        WHERE method='tx_submit' AND at BETWEEN ? AND ? ORDER BY at, id
    """, (payments[0]["marker"], payments[-1]["latest"])).fetchall()
    require(len(calls) == count, "missing or extra tx_submit requests in rehearsal")
    for payment, call in zip(payments, calls):
        require(call["status"] == "ok" and call["host"] == payment["host"]
                and call["bytes"] == payment["bytes"] and finite(call["at"])
                and payment["marker"] <= call["at"] <= payment["latest"],
                f"{payment['intent']}: cannot identify a unique successful submission")
        payment.update(earliest=call["at"], request_id=call["id"])
    return payments


def summarize(payments, step, span, jitter, lag_limit):
    deadlines = Counter()
    windows = Counter()
    marker_misses = 0
    details = []
    for p in payments:
        p["deadline"] = classify(p["earliest"] - p["offered"], p["latest"] - p["offered"],
                                 0, lag_limit)
        deadlines[p["deadline"]] += 1
    for first, last in zip(payments, payments[step:]):
        low = last["earliest"] - first["latest"]
        high = last["latest"] - first["earliest"]
        status = classify(low, high, span - jitter, span + jitter)
        marker_span = last["marker"] - first["marker"]
        marker_missed = abs(marker_span - span) > jitter
        marker_misses += marker_missed
        windows[status] += 1
        details.append(dict(first=first["intent"], last=last["intent"], minimum_seconds=low,
                            maximum_seconds=high, classification=status, marker_missed=marker_missed))

    def counts(values):
        return {k: values[k] for k in ("within", "miss", "indeterminate")}

    return {
        "summary": {
            "payments": len(payments), "validator_receipts": len(HOSTS) * len(payments),
            "offer_lag_limit_seconds": lag_limit, "offer_deadlines": counts(deadlines),
            "maximum_offer_to_response_seconds": max(p["latest"] - p["offered"] for p in payments),
            "rolling_intervals": step, "rolling_target_seconds": span, "rolling_tolerance_seconds": jitter,
            "rolling_windows": counts(windows), "controller_marker_window_misses": marker_misses,
        },
        "payments": payments, "windows": details,
    }


def audit(journal, plan):
    # immutable=1 prevents SQLite from creating sidecars in the evidence
    # directory. It also ignores a WAL, so require a stopped, checkpointed copy.
    journal = journal.resolve(strict=True)
    for suffix in ("-wal", "-journal"):
        sidecar = Path(str(journal) + suffix)
        require(not sidecar.exists() or not sidecar.stat().st_size,
                "use a closed, checkpointed copy of the SQLite journal")
    uri = journal.as_uri() + "?mode=ro&immutable=1"
    with closing(sqlite3.connect(uri, uri=True)) as db:
        db.row_factory = sqlite3.Row
        meta = {row[0]: json.loads(row[1]) for row in db.execute(
            "SELECT key, value FROM meta WHERE key IN ('plan_digest','rehearsal_start','rehearsal_result')")}
        require(meta.get("plan_digest") == digest(plan), "plan differs from journal pin")
        start = meta.get("rehearsal_start")
        require(finite(start), "missing rehearsal start")
        result = meta.get("rehearsal_result") or {}
        settings = plan["rehearsal"]
        interval = settings["interval_seconds"]
        offer_seconds = settings["duration_seconds"] - 120
        require(type(interval) is int and interval > 0 and offer_seconds >= 2 * interval,
                "unsupported rehearsal schedule")
        count = offer_seconds // interval
        calibration_interval = offer_seconds / count
        step = min(count - 1, math.ceil(60 / calibration_interval))
        span = step * calibration_interval
        jitter = 1.0
        lag = min(plan["limits"]["start_slot_grace_seconds"], max(1, min(5, 2 * calibration_interval)))
        rolling = result.get("rolling_windows", {})
        require(result.get("expected") == count and result.get("offer_start") == start
                and rolling.get("intervals_per_window") == step
                and rolling.get("nominal_span_seconds") == span
                and result.get("timing_tolerance") == {
                    "span_jitter_seconds": jitter, "max_offer_lag_seconds": lag},
                "archived calibration differs from plan")
        payments = observations(db, start, interval, count)
        return summarize(payments, step, span, jitter, lag)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("journal", type=Path, help="stopped, checkpointed journal copy")
    parser.add_argument("plan", type=Path, help="installed JSON plan pinned by the journal")
    parser.add_argument("--details", action="store_true", help="include payment and window bounds")
    args = parser.parse_args()
    try:
        result = audit(args.journal, json.loads(args.plan.read_bytes()))
    except (OSError, sqlite3.Error, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"arrival audit: {error}\n")
    print(json.dumps(result if args.details else result["summary"], indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
