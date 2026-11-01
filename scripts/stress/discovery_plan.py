"""Opt-in dense discovery schedule; expansion is offline and never activates it."""

import hashlib
import json
import re
import random
from collections import deque
from bounds import checked_limits
from lottery import spend_expectation_threshold
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


# Scripted validator-degrade schedule (x-series, #1585). Fault roles n1..n5 map
# in fixed order onto runtime.HOSTS, the keys the controller uses everywhere.
DEGRADE_ROLES = tuple(f"n{i + 1}" for i in range(len(HOSTS)))
DEGRADE_ACTIONS = {
    # action -> exact target count
    "stop_restart": 1,  # clean `systemctl stop` (SIGTERM), then start
    "kill_restart": 1,  # SIGKILL, then start
    "isolate": 1,  # firewall-isolate, no restart
    "netem": 1,  # tc netem loss/latency
    "stop_restart_pair": 2,  # below the 4-of-5 quorum: must halt safely
}
DEGRADE_STEP_KEYS = {
    "id",
    "action",
    "targets",
    "start_seconds",
    "fault_seconds",
    "recovery_seconds",
    "offers",
    "expect",
}
NETEM_KEYS = {"loss_percent", "delay_ms"}
# Offers must stop this long before an `offers: none` fault so that in-flight
# payments confirm while quorum still holds.
DEGRADE_QUIET_LEAD_SECONDS = 300


def degrade_opt_in(plan):
    """The scoped opt-in; an absent key means False."""
    from plan import require

    value = plan["faults"].get("validator_degrade_phase", False)
    require(type(value) is bool, "validator_degrade_phase must be a boolean")
    return value


def expand_degrade(plan, phase_bounds, events):
    """Validate the `degrade` block; return steps with campaign-relative offsets.

    Offsets are integer seconds from campaign `start`:
    fault_start_offset (fault applied), restore_offset (executor restores)
    and window_end_offset (node rejoined; step verdict taken).
    """
    from plan import require, integer

    block = plan["degrade"]
    require(
        isinstance(block, dict) and set(block) == {"phase", "producer_role", "steps"},
        "degrade block requires exactly phase, producer_role and steps",
    )
    require(block["phase"] in phase_bounds, "degrade phase names no plan phase")
    require(block["producer_role"] in DEGRADE_ROLES, "unknown degrade producer role")
    phase_start, phase_stop = phase_bounds[block["phase"]]
    steps = block["steps"]
    require(isinstance(steps, list) and steps, "degrade steps required")
    offers = sorted(e["offset_seconds"] for e in events)
    expanded, identifiers, previous_end = [], set(), phase_start
    for step in steps:
        require(isinstance(step, dict), "degrade step must be an object")
        action = step.get("action")
        require(action in DEGRADE_ACTIONS, "unknown degrade action")
        keys = DEGRADE_STEP_KEYS | (NETEM_KEYS if action == "netem" else set())
        require(set(step) == keys, f"degrade step fields must be exactly {sorted(keys)}")
        step_id = step["id"]
        require(
            isinstance(step_id, str)
            and re.fullmatch("[A-Za-z0-9_-]{1,16}", step_id)
            and step_id not in identifiers,
            "degrade step ids must be distinct short tokens",
        )
        identifiers.add(step_id)
        targets = step["targets"]
        require(
            isinstance(targets, list)
            and all(t in DEGRADE_ROLES for t in targets)
            and len(set(targets)) == len(targets),
            "degrade targets must be distinct n1..n5 roles",
        )
        require(
            block["producer_role"] not in targets, "degrade must not target the producer"
        )
        require(
            len(targets) == DEGRADE_ACTIONS[action],
            f"{action} requires exactly {DEGRADE_ACTIONS[action]} target(s)",
        )
        require(step["offers"] in ("continue", "none"), "offers: continue or none")
        require(step["expect"] in ("progress", "safe_halt"), "expect: progress or safe_halt")
        if action == "stop_restart_pair":
            require(
                step["expect"] == "safe_halt" and step["offers"] == "none",
                "below-quorum pair requires expect safe_halt and offers none",
            )
        fault = integer(step["fault_seconds"], "fault_seconds", 60)
        recovery = integer(step["recovery_seconds"], "recovery_seconds", 300)
        require(fault <= 3600 and recovery <= 3600, "degrade duration out of bounds")
        start = phase_start + integer(step["start_seconds"], "start_seconds", 0)
        restore, window_end = start + fault, start + fault + recovery
        require(
            previous_end <= start,
            "degrade steps must be ordered and not overlap, including recovery",
        )
        require(window_end <= phase_stop, "degrade step outside its phase")
        previous_end = window_end
        if step["offers"] == "none":
            quiet = start - DEGRADE_QUIET_LEAD_SECONDS
            require(
                not any(quiet <= at < window_end for at in offers),
                "offer inside an offers:none step window or its quiet lead",
            )
        entry = {
            "id": step_id,
            "action": action,
            "targets": list(targets),
            "target_hosts": [HOSTS[DEGRADE_ROLES.index(t)] for t in targets],
            "offers": step["offers"],
            "expect": step["expect"],
            "fault_start_offset": start,
            "restore_offset": restore,
            "window_end_offset": window_end,
        }
        if action == "netem":
            loss, delay = step["loss_percent"], step["delay_ms"]
            require(
                type(loss) is int
                and 0 <= loss <= 50
                and type(delay) is int
                and 0 <= delay <= 2000,
                "netem requires integer loss_percent 0-50 and delay_ms 0-2000",
            )
            entry.update(loss_percent=loss, delay_ms=delay)
        expanded.append(entry)
    return expanded


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
    # Optional pin for the #1681 award-spend expectation; default otherwise.
    spend_expectation_threshold(plan)
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
    degrade = degrade_opt_in(plan)
    require(
        degrade == ("degrade" in plan),
        "validator_degrade_phase opt-in and degrade block require each other",
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
    # #1576 decision 1 (#1613): a v2 plan may accept a clean generator_limited
    # rehearsal. Absent means False; anything but a boolean is rejected.
    require(
        type(rehearsal.get("generator_limited_acceptable", False)) is bool,
        "generator_limited_acceptable must be a boolean",
    )
    seed = integer(plan["seed"], "seed", 0)
    require(seed < 2**32, "seed bound")
    rng = random.Random(seed)
    wallet_order = list(range(count))
    rng.shuffle(wallet_order)
    events, phases, end, idle = [], [], 0, []
    identifiers, phase_bounds = set(), {}
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
        phase_bounds[phase["id"]] = (start, stop)
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
    # Validate against the full campaign schedule before rehearsal clears it.
    degrade_steps = expand_degrade(plan, phase_bounds, events) if degrade else None
    if mode == "rehearsal_only":
        events, phases = [], []
        if degrade:
            degrade_steps = []  # no campaign schedule, so no fault is scheduled
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
    summary = {
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
    }
    if degrade:
        # Only with the opt-in, so existing summaries stay byte-identical.
        summary["degrade_steps"] = degrade_steps
    return summary, events
