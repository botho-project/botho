#!/usr/bin/env python3
"""Validate/expand a design manifest offline. Never connects, signs or submits."""
import argparse
from collections import deque
import hashlib
import json
from pathlib import Path
import re

DEFAULT_PLAN = Path(__file__).with_name("testnet-72h-plan.json")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, name, minimum=1):
    require(type(value) is int and value >= minimum, f"{name}: invalid integer")
    return value


def pico(value, name):
    require(isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*", value),
            f"{name}: use a positive integer string in picocredits")
    return int(value)


def expand(plan):
    require(plan["schema_version"] == 1, "unsupported schema")
    require(plan["status"] == "design_only", "this tool supports design manifests only")
    require(plan["start_utc"] is None and plan["run_id"] is None,
            "activation belongs to the future executor, not this offline planner")
    require(plan["network"] == "botho-testnet", "testnet identity required")
    require(re.fullmatch(r"[0-9a-f]{64}", plan["genesis"]), "invalid genesis")
    require(re.fullmatch(r"[0-9a-f]{40}", plan["node_commit"]), "invalid source pin")
    endpoints = plan["endpoints"]
    require(len(endpoints) == 5 and len(set(endpoints)) == 5
            and all(re.fullmatch(r"https://[a-z0-9.-]+/rpc", e) for e in endpoints),
            "five distinct TLS ingresses required")
    duration = integer(plan["duration_hours"], "duration") * 3600
    require(duration == 72 * 3600, "this design is a 72-hour campaign")
    wallets = plan["wallets"]
    wallet_count = integer(wallets["count"], "wallet count", 2)
    integer(wallets["min_mature_utxos_per_wallet"], "mature inventory")
    require(wallets["min_input_age_blocks"] >= 10, "cannot relax production age floor")
    require(wallets["max_inflight_per_wallet"] == 1, "one inflight spend per wallet")
    amounts = [pico(v, "transfer amount") for v in wallets["transfer_amounts_picocredits"]]
    require(amounts, "transfer amounts required")
    limits, funding = plan["limits"], plan["funding"]
    global_inflight = integer(limits["max_inflight"], "inflight")
    require(global_inflight <= wallet_count, "inflight exceeds independent wallets")
    require(limits["max_automatic_rebroadcasts"] == 0, "automatic rebroadcast disabled")
    fee = pico(limits["max_single_signed_fee_picocredits"], "single fee")
    total_fee = pico(limits["max_signed_fee_picocredits"], "total fee")
    principal = pico(funding["max_principal_picocredits"], "principal")
    require(fee <= total_fee < principal, "fee/principal budgets inconsistent")
    require(max(amounts) + fee <= principal // wallet_count,
            "nominal individual funding cannot cover largest single payment")
    grants = integer(funding["max_grants"], "grants")
    grant = pico(funding["grant_picocredits"], "grant amount")
    require(grants * grant <= principal, "funding exceeds principal cap")
    require((grants + wallet_count - 1) // wallet_count
            <= funding["max_grants_per_wallet_24h"] <= 3,
            "funding exceeds deployed recipient limit")
    require(funding["grant_interval_seconds"] >= 900, "funding must respect planned pacing")
    require((grants - 1) * funding["grant_interval_seconds"]
            < integer(plan["setup_deadline_hours"], "setup deadline") * 3600,
            "funding schedule cannot finish within setup deadline")
    faults = plan["faults"]
    require(all(faults[k] is False for k in ["public_automatic_node_stops",
            "public_miner_changes", "public_partitions", "public_disk_faults"]),
            "automatic public fault injection is outside this manifest")
    require(faults["invalid_transaction_destination"] == "isolated_cluster_only",
            "negative transactions must stay isolated")
    events, phase_summaries, identifiers = [], [], set()
    previous_end = 0
    for phase in plan["phases"]:
        phase_id = phase["id"]
        require(phase_id not in identifiers, "duplicate phase id")
        identifiers.add(phase_id)
        start = integer(phase["start_hour"], "phase start", 0) * 3600
        end = integer(phase["end_hour"], "phase end") * 3600
        require(start == previous_end and start < end <= duration, "phase gaps/overlap")
        previous_end = end
        phase_inflight = integer(phase["max_inflight"], "phase inflight")
        require(phase_inflight <= global_inflight, "phase exceeds global inflight cap")
        require(phase["gate"], "each phase requires a gate")
        phase_events = []
        for segment in phase["segments"]:
            count = integer(segment["count"], "segment count")
            require(count <= limits["max_unique_chain_writes"], "oversized segment")
            interval = integer(segment["interval_seconds"], "segment interval")
            segment_start = integer(segment["start_hour"], "segment start", 0) * 3600
            if segment["scenario"] == "burst":
                require(count <= limits["max_burst_size"] and count <= phase_inflight,
                        "burst exceeds concurrency cap")
            elif count > 1:
                require(interval * limits["max_normal_submissions_per_minute"] >= 60,
                        "normal offered rate exceeds cap")
            for i in range(count):
                offset = segment_start + i * interval
                require(start <= offset < end, "event outside phase")
                phase_events.append((offset, segment["scenario"]))
        phase_events.sort()
        require(len({e[0] for e in phase_events}) == len(phase_events), "overlapping segment events")
        for offset, scenario in phase_events:
            index = len(events)
            sender = index % wallet_count
            events.append({"intent": f"{phase_id}-{index:04d}", "offset_seconds": offset,
                           "scenario": scenario, "sender": f"wallet-{sender + 1}",
                           "recipient": f"wallet-{(sender + 1) % wallet_count + 1}",
                           "amount_picocredits": str(amounts[index % len(amounts)]),
                           "gate": phase["gate"], "max_inflight": phase_inflight})
        phase_summaries.append({"phase": phase_id, "start_hour": start // 3600,
                                "end_hour": end // 3600, "payments": len(phase_events)})
    require(previous_end == duration, "phases must cover the full campaign")
    window = deque()
    for event in events:
        if event["scenario"] == "burst":
            continue
        while window and event["offset_seconds"] - window[0] >= 60:
            window.popleft()
        window.append(event["offset_seconds"])
        require(len(window) <= limits["max_normal_submissions_per_minute"],
                "combined segments exceed the normal per-minute rate")
    total = len(events) + grants + integer(funding["max_bootstrap_transfers"], "bootstrap")
    require(total <= limits["max_unique_chain_writes"], "campaign exceeds chain-write budget")
    summary = {"status": "DESIGN ONLY — nothing scheduled or submitted",
               "plan_sha256": hashlib.sha256(json.dumps(plan, sort_keys=True).encode()).hexdigest(),
               "duration_hours": plan["duration_hours"], "phases": phase_summaries,
               "nominal_campaign_payments": len(events), "max_funding_grants": grants,
               "max_bootstrap_transfers": funding["max_bootstrap_transfers"],
               "total_planned_chain_write_ceiling": total,
               "absolute_chain_write_cap": limits["max_unique_chain_writes"],
               "min_initial_mature_outputs": wallet_count * wallets["min_mature_utxos_per_wallet"],
               "max_principal_picocredits": str(principal),
               "max_signed_fees_picocredits": str(total_fee),
               "limitations": "Static budget/window validation only; runtime gates, signing, accounting, rate limiting and enforcement remain to be implemented."}
    return summary, events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path, default=DEFAULT_PLAN)
    parser.add_argument("--output", type=Path, help="optional local directory for expanded JSON")
    args = parser.parse_args()
    try:
        summary, events = expand(json.loads(args.plan.read_text()))
    except (ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        (args.output / "intents.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
