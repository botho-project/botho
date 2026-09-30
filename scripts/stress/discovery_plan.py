"""Opt-in dense discovery schedule; expansion is offline and never activates it."""

import hashlib
import json
import re
import random
from collections import deque
from bounds import checked_limits
from runtime import Gate, HOSTS, validate_rpc_url
import math


def execution_mode(plan):
    mode = plan.get("execution_mode", "campaign")
    if mode not in ("campaign", "rehearsal_only") or (
        mode != "campaign" and plan.get("schema_version") != 2
    ):
        raise ValueError("unsupported execution mode")
    return mode


def validate_lifetime(plan, config):
    mode = execution_mode(plan)
    if config.get("execution_mode", "campaign") != mode:
        raise Gate("deployment execution mode differs from plan")
    start, end = config["setup_start"], config["infrastructure_end"]
    low, high = (1.5, 2) if mode == "rehearsal_only" else (80.5, 82)
    if (
        any(type(v) not in (int, float) or not math.isfinite(v) for v in (start, end))
        or not 0 < start < end
        or not low * 3600 <= end - start <= high * 3600
    ):
        raise Gate(f"fixed {mode} infrastructure lifetime must be {low}–{high} hours")


def expand_discovery(plan):
    from plan import require, integer, pico

    require(
        plan.get("schema_version") == 2
        and plan.get("profile") == "isolated-discovery-v2",
        "unsupported discovery profile",
    )
    require(
        plan.get("isolated_only") is True, "discovery requires isolated-only opt-in"
    )
    require(
        plan["status"] == "design_only"
        and plan["start_utc"] is None
        and plan["run_id"] is None,
        "fresh design required",
    )
    mode = execution_mode(plan)
    require(
        plan["duration_hours"] == 72
        and plan["setup_deadline_hours"] == (1 if mode == "rehearsal_only" else 8),
        "immutable campaign/setup durations",
    )
    require(plan["network"] == "botho-testnet", "testnet required")
    require(
        re.fullmatch("[0-9a-f]{64}", plan["genesis"])
        and re.fullmatch("[0-9a-f]{40}", plan["node_commit"]),
        "invalid identity pins",
    )
    require(len(set(plan["endpoints"])) == 5, "five distinct ingresses required")
    for endpoint in plan["endpoints"]:
        validate_rpc_url(endpoint)
        require(
            endpoint not in ["https://" + h + "/rpc" for h in HOSTS],
            "public ingress forbidden",
        )
    limits = checked_limits(plan["limits"])
    wallets = plan["wallets"]
    count = integer(wallets["count"], "wallets", 8)
    require(count <= 64 and limits["max_inflight"] <= count, "wallet/inflight bound")
    require(
        wallets["max_inflight_per_wallet"] == 1
        and wallets["min_input_age_blocks"] == 10,
        "production input constraints",
    )
    require(
        16 <= wallets["min_mature_utxos_per_wallet"] <= 128, "sustained inventory bound"
    )
    require(
        wallets["max_payment_inputs"] == 4 and 4 <= wallets["decoy_headroom"] <= 32,
        "bounded selection and decoy headroom",
    )
    amounts = [pico(v, "amount") for v in wallets["transfer_amounts_picocredits"]]
    require(amounts and max(amounts) <= 10**12, "payment bound")
    require(
        plan["funding"]["mode"] == "prefunded_isolated"
        and plan["funding"]["max_grants"] == 0
        and plan["funding"]["max_bootstrap_transfers"] == 0,
        "v2 requires pre-funded inventory; no faucet or bootstrap replay",
    )
    principal = pico(plan["funding"]["max_principal_picocredits"], "principal")
    require(
        limits["max_signed_fee_picocredits"] < principal <= 10**15, "principal budget"
    )
    require(
        all(
            plan["faults"][k] is False
            for k in (
                "public_automatic_node_stops",
                "public_miner_changes",
                "public_partitions",
                "public_disk_faults",
            )
        ),
        "public faults forbidden",
    )
    rehearsal = plan["rehearsal"]
    require(
        600 <= integer(rehearsal["duration_seconds"], "rehearsal") <= 1800,
        "short rehearsal required",
    )
    require(
        60 <= integer(rehearsal["drain_seconds"], "rehearsal drain") <= 300,
        "bounded rehearsal drain",
    )
    require(
        integer(rehearsal["interval_seconds"], "rehearsal cadence") >= 1
        and 60 % rehearsal["interval_seconds"] == 0,
        "integer rehearsal cadence",
    )
    require(
        60 / rehearsal["interval_seconds"]
        <= limits["max_normal_submissions_per_minute"],
        "rehearsal rate exceeds cap",
    )
    seed = integer(plan["seed"], "seed", 0)
    require(seed < 2**32, "seed bound")
    rng = random.Random(seed)
    wallet_order = list(range(count))
    rng.shuffle(wallet_order)
    events, phases, end, idle = [], [], 0, []
    identifiers = set()
    for phase in plan["phases"]:
        start = integer(phase["start_hour"], "start", 0) * 3600
        stop = integer(phase["end_hour"], "end") * 3600
        require(start == end and start < stop <= 72 * 3600, "phase gaps/overlap")
        end = stop
        require(
            phase["id"] not in identifiers and phase["gate"],
            "duplicate phase or missing gate",
        )
        identifiers.add(phase["id"])
        require(
            1 <= phase["max_inflight"] <= limits["max_inflight"], "phase inflight bound"
        )
        if not phase["segments"]:
            idle.append(stop - start)
        before = len(events)
        offsets = []
        for segment in phase["segments"]:
            interval = integer(segment["interval_seconds"], "cadence")
            n = integer(segment["count"], "count")
            require(n <= limits["max_unique_chain_writes"], "segment bound")
            require(
                interval >= rehearsal["interval_seconds"],
                "rehearsal must exercise peak requested cadence",
            )
            for i in range(n):
                at = segment["start_hour"] * 3600 + i * interval
                require(start <= at < stop, "offer outside phase")
                offsets.append((at, segment["scenario"]))
        require(len({x[0] for x in offsets}) == len(offsets), "overlapping offers")
        for at, scenario in sorted(offsets):
            index = len(events)
            # Nine exact-shape tier probes form three matched input-count cohorts.
            shape = (1, 2, 4)[index // 3] if index < 9 else None
            sender = wallet_order[index % count]
            recipient = (sender + rng.randrange(1, count)) % count
            if index % 3 == 0:
                cohort_amount = rng.choice(amounts)
            events.append(
                {
                    "intent": f"{phase['id']}-{index:05d}",
                    "offset_seconds": at,
                    "sender": f"wallet-{sender + 1}",
                    "recipient": f"wallet-{recipient + 1}",
                    "amount_picocredits": str(cohort_amount),
                    "scenario": scenario,
                    "gate": phase["gate"],
                    "max_inflight": phase["max_inflight"],
                    "input_count": shape,
                    "fee_multiplier": (1, 2, 4)[index % 3],
                    "cohort": index // 3,
                }
            )
        phases.append(
            {
                "phase": phase["id"],
                "start_hour": start // 3600,
                "end_hour": stop // 3600,
                "payments": len(events) - before,
            }
        )
    require(
        end == 72 * 3600 and idle == [6 * 3600], "exactly one six-hour idle required"
    )
    window = deque()
    for event in events:
        while window and event["offset_seconds"] - window[0] >= 60:
            window.popleft()
        window.append(event["offset_seconds"])
        require(
            len(window) <= limits["max_normal_submissions_per_minute"],
            "combined offered rate exceeds cap",
        )
    if mode == "rehearsal_only":
        events, phases = [], []
    rehearsal_count = (rehearsal["duration_seconds"] - 120) // rehearsal[
        "interval_seconds"
    ]
    require(
        len(events) + rehearsal_count <= limits["max_unique_chain_writes"],
        "total writes including rehearsal exceed budget",
    )
    # Reserve worst-case fees for every signed attempt, including rehearsal.
    require(
        (len(events) + rehearsal_count) * limits["max_single_signed_fee_picocredits"]
        <= limits["max_signed_fee_picocredits"],
        "insufficient worst-case fee budget",
    )
    return {
        "status": "DESIGN ONLY",
        "plan_sha256": hashlib.sha256(
            json.dumps(plan, sort_keys=True).encode()
        ).hexdigest(),
        "execution_mode": mode,
        "duration_hours": 0 if mode == "rehearsal_only" else 72,
        "phases": phases,
        "nominal_campaign_payments": len(events),
        "rehearsal_payments": rehearsal_count,
        "total_planned_chain_write_ceiling": len(events) + rehearsal_count,
        "limitations": "Rate is an offer target, not network capacity. Real rehearsal required. Production dynamic fees need ~25+ tx/s, beyond this bounded generator.",
    }, events
