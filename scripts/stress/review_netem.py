#!/usr/bin/env python3
"""Reconcile the D4 runner log with the frozen campaign plan and controller report."""

import argparse
import hashlib
import json
import math
from pathlib import Path

from faults import DEFAULT_DEV, PRECHECK_LEAD, Refusal, commands
from plan import expand
from runtime import Gate, digest


def need(condition, message):
    if not condition:
        raise ValueError(message)


def finite(value):
    return type(value) in (int, float) and math.isfinite(value)


def runner_evidence(rows, step, start, dev):
    (role,) = step["targets"]
    fault = int(start) + step["fault_start_offset"]
    restore = int(start) + step["restore_offset"]
    # Only the four traffic-control argument lists are compared. They depend on
    # the plan, interface and gossip port, independently of the service unit.
    rendered = commands(step, {role: {}}, "botho.service", dev=dev)
    hashes = [digest(e["argv"]) for e in rendered["apply"] if e["action"] == "netem"]
    step_rows = [r for r in rows if r["step"] == step["id"]]
    samples = [r for r in step_rows if r["action"] == "qdisc_counters"]
    rows = [
        r for r in step_rows if r["phase"] != "warn" and r["action"] != "qdisc_counters"
    ]
    need(rows, "no runner records for the planned step")
    need(all(r["role"] == role for r in rows), "runner target differs from plan")
    need(all(r["ok"] is True for r in rows), "runner recorded an unsuccessful action")
    times = [r["at"] for r in rows]
    need(
        all(finite(t) for t in times) and times == sorted(times), "invalid runner clock"
    )
    prechecks = [r for r in rows if r["phase"] == "precheck"]
    need(
        prechecks and rows[: len(prechecks)] == prechecks,
        "missing or misplaced precheck",
    )
    need(
        all(
            r["action"] == "minting_check"
            and r["minting_active"] is False
            and r["planned_at"] == fault
            for r in prechecks
        ),
        "invalid minting precheck",
    )
    need(
        prechecks[-1]["at"] >= fault - PRECHECK_LEAD, "missing precheck at fault start"
    )
    actions = rows[len(prechecks) :]
    expected = [
        ("apply", "deadman"),
        *[("apply", "netem") for _ in hashes],
        ("restore", "restore"),
        ("verify", "verify"),
        ("disarm", "disarm"),
    ]
    need(
        [(r["phase"], r["action"]) for r in actions] == expected,
        "missing, extra or out-of-order runner actions",
    )
    for row in actions:
        planned = fault if row["phase"] == "apply" else restore
        need(
            row["planned_at"] == planned and row["at"] >= planned,
            "runner action outside its planned schedule",
        )
    setup = actions[1 : 1 + len(hashes)]
    last_setup = setup[-1]["at"]
    need(last_setup < restore, "setup finished after scheduled restoration")
    need(
        actions[-2]["at"] <= start + step["window_end_offset"],
        "restoration was verified after the recovery window",
    )
    if any("command_sha256" in r for r in setup):
        need(
            [r.get("command_sha256") for r in setup] == hashes,
            "setup command hashes differ from the frozen plan or interface",
        )
        command_evidence = "matched_argument_hashes"
    else:
        command_evidence = "legacy_action_names_only"
    counters = None
    observation_error = None
    if samples:
        sample = samples[0]
        value = sample.get("counters")
        if len(samples) != 1 or not (
            sample.get("role") == role
            and sample.get("planned_at") == (fault + restore) / 2
            and finite(sample.get("at"))
            and max(last_setup, (fault + restore) / 2) <= sample["at"] < restore
        ):
            observation_error = "counter sample does not match this target and window"
        elif (
            sample.get("phase") == "observe"
            and sample.get("ok") is True
            and isinstance(value, dict)
            and all(
                type(value.get(k)) is int and value[k] >= 0
                for k in ("packets", "drops")
            )
        ):
            counters = {k: value[k] for k in ("packets", "drops")}
            counters["observed_at"] = sample["at"]
        else:
            observation_error = "counter read unsuccessful or incomplete"
    return {
        "target_role": role,
        "queue_counters": counters,
        "queue_observation_error": observation_error,
        "planned_loss_percent": step["loss_percent"],
        "planned_delay_ms": step["delay_ms"],
        "command_evidence": command_evidence,
        "last_setup_ack": last_setup,
        "restore_ack": actions[-3]["at"],
        "restore_verified_at": actions[-2]["at"],
        "disarmed_at": actions[-1]["at"],
        "setup_to_restore_ack_seconds": actions[-3]["at"] - last_setup,
    }


def review(plan, activation, report, rows, dev=DEFAULT_DEV):
    need(
        all(isinstance(value, dict) for value in (plan, activation, report)),
        "plan, activation and report must be objects",
    )
    need(all(isinstance(row, dict) for row in rows), "invalid runner record")
    summary, _ = expand(plan)
    steps = [s for s in summary.get("degrade_steps", []) if s["action"] == "netem"]
    need(len(steps) == 1, "expected one scheduled netem step")
    step = steps[0]
    run, start = activation["run_id"], activation["start"]
    need(
        run and report["run_id"] == run and finite(start) and report["start"] == start,
        "report and activation refer to different attempts or start times",
    )
    need(
        report["plan_sha256"] == activation["plan_sha256"] == summary["plan_sha256"],
        "report or activation differs from frozen plan",
    )
    matches = [r for r in report["degrade"] if r["step"] == step["id"]]
    need(len(matches) == 1, "expected one controller verdict for the step")
    result = matches[0]
    need(
        result["action"] == step["action"]
        and result["targets"] == step["target_hosts"]
        and result["expect"] == step["expect"],
        "controller step differs from plan",
    )
    verdict = result["verdict"]
    if verdict == "pass":
        need(
            not result["failures"]
            and not result["awaiting"]
            and finite(report["at"])
            and report["at"] >= start + step["window_end_offset"],
            "controller pass conflicts with its outstanding checks or report time",
        )
        need(
            result["fault_observed"] is True or result["fault_observed"] == "unknown",
            "controller reports fault absent",
        )
    output = {
        "run_id": run,
        "plan_sha256": summary["plan_sha256"],
        "step": step["id"],
        "controller_verdict": verdict,
        "campaign_status": report["status"],
    }
    try:
        output["runner"] = runner_evidence(rows, step, start, dev)
    except (ValueError, KeyError, TypeError, Refusal) as error:
        output["runner_error"] = str(error)
    output["corroborated"] = verdict == "pass" and "runner" in output
    return output


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fleet", type=Path, help="archived campaign fleet directory")
    parser.add_argument(
        "--dev", default=DEFAULT_DEV, help="interface selected by the operator"
    )
    args = parser.parse_args(argv)
    try:
        names = (
            "campaign-plan.json",
            "campaign-start.json",
            "result.json",
            "faults.jsonl",
        )
        files = {name: (args.fleet / name).read_bytes() for name in names}
        plan, activation, report = [json.loads(files[n]) for n in names[:3]]
        rows = [json.loads(line) for line in files["faults.jsonl"].splitlines()]
        result = review(plan, activation, report, rows, dev=args.dev)
        result["input_sha256"] = {
            k: hashlib.sha256(v).hexdigest() for k, v in files.items()
        }
    except (OSError, ValueError, KeyError, TypeError, Gate, Refusal) as error:
        result = {"corroborated": False, "error": str(error)}
    print(json.dumps(result, indent=2, allow_nan=False))
    return 0 if result["corroborated"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
