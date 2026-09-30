"""Durable campaign reporting shared by legacy and discovery profiles."""

import json
import math
import resource
import time
from plan import expand
from runtime import HOSTS, atomic, digest


def accounting_status(journal, rows, now):
    """Describe retained evidence without inferring balances or releasing inputs."""
    snapshot = journal.get("accounting")
    writes = [
        r for r in rows if r["prepared"] is not None or r["submitted"] is not None
    ]
    unresolved = sum(r["state"] != "reconciled" for r in writes)
    reserved = journal.db.execute("SELECT COUNT(*) FROM reservations").fetchone()[0]
    activity = [
        r[k]
        for r in rows
        for k in ("prepared", "submitted", "finished")
        if r[k] is not None
    ]
    last_activity = max(activity, default=None)
    first_write = min(
        (r[k] for r in writes for k in ("prepared", "submitted") if r[k] is not None),
        default=None,
    )
    reasons = []
    at = snapshot.get("at") if snapshot else None
    valid_time = type(at) in (int, float) and math.isfinite(at) and at <= now
    freshness = "missing" if snapshot is None else "unknown"
    kind = "missing" if snapshot is None else "unknown"
    if snapshot is None:
        reasons.append("no accounting snapshot")
    elif not valid_time:
        reasons.append("snapshot timestamp missing, invalid, or in the future")
    else:
        freshness = "current"
        kind = (
            "opening" if first_write is None or at < first_write else "reconciliation"
        )
        if last_activity is not None and at < last_activity:
            freshness = "stale"
            reasons.append("snapshot predates journal activity")
        if journal.get("status") in ("held", "incomplete") and at < journal.get(
            "stopped_at", 0
        ):
            freshness = "stale"
            reasons.append("snapshot predates admission stop")
    expected_fees = sum(
        r["fee"] for r in rows if r["state"] == "reconciled" and r["kind"] != "funding"
    )
    if snapshot:
        if snapshot.get("signed_fees") != expected_fees:
            freshness = "stale" if valid_time else freshness
            reasons.append("snapshot does not account for reconciled signed fees")
        if snapshot.get("difference") != 0:
            reasons.append("accounting difference is nonzero or missing")
    if unresolved:
        reasons.append("signed or submitted work remains unreconciled")
    if reserved:
        reasons.append("retained input reservations")
    terminal = journal.get("status") in (
        "held",
        "incomplete",
        "complete",
        "rehearsal_complete",
    )
    if terminal and kind == "opening":
        reasons.append("opening snapshot is not final accounting")
    return {
        "snapshot_kind": kind,
        "snapshot_at": at,
        "freshness": freshness,
        "last_journal_activity": last_activity,
        "unreconciled_writes": unresolved,
        "reserved_inputs": reserved,
        "reconciled_signed_fees": expected_fees,
        "final_reconciliation": "not_final"
        if not terminal
        else "unverified"
        if reasons
        else "verified",
        "reasons": reasons if terminal else [*reasons, "run is still active"],
    }


def rehearsal_summary(journal, plan):
    settings = plan["rehearsal"]
    expected = (settings["duration_seconds"] - 120) // settings["interval_seconds"]
    rows = journal.rows("kind='rehearsal'")
    counts = {}
    for row in rows:
        counts[row["state"]] = counts.get(row["state"], 0) + 1
    result = journal.get("rehearsal_result") or {
        "status": "failed"
        if journal.get("status") in ("held", "incomplete")
        else "in_progress"
    }
    return {
        **result,
        "expected": expected,
        "offered": len(rows),
        "not_yet_offered": max(0, expected - len(rows)),
        "submitted": sum(r["submitted"] is not None for r in rows),
        "signed_unsubmitted": sum(
            r["prepared"] is not None and r["submitted"] is None for r in rows
        ),
        "unsigned": sum(r["prepared"] is None and r["submitted"] is None for r in rows),
        "reconciled": counts.get("reconciled", 0),
        "state_counts": counts,
    }


def controller_report(self):
    rows = self.j.rows()
    nominal = (
        len(self.events)
        if hasattr(self, "events")
        else len(expand(self.plan)[1])
        if self.plan
        else 0
    )
    counts = {}
    for row in rows:
        label = row["kind"] + ":" + row["state"]
        counts[label] = counts.get(label, 0) + 1
    complete = [r for r in rows if r["state"] == "reconciled" and r["submitted"]]

    def latency(field):
        values = sorted(r["finished"] - r[field] for r in complete)
        if not values:
            return {"count": 0}
        return {
            "count": len(values),
            "median": values[len(values) // 2],
            "p95": values[min(len(values) - 1, int(len(values) * 0.95))],
            "max": values[-1],
        }

    report = {
        "at": time.time(),
        "run_id": self.config["run_id"],
        "status": self.j.get("status"),
        "reason": self.j.get("reason"),
        "setup_start": self.j.get("setup_start"),
        "start": self.j.get("start"),
        "end": self.j.get("end"),
        "plan_sha256": digest(self.plan),
        "counts": counts,
        "submitted_to_reconciled_seconds": latency("submitted"),
        "nominal_campaign_offers": nominal,
        "targets": list(self.targets.values()) if hasattr(self, "targets") else None,
        "infrastructure_end": self.config.get("infrastructure_end"),
        "not_yet_offered": nominal - sum(r["kind"] == "campaign" for r in rows),
        "offered_to_reconciled_seconds": latency("offered"),
        "signed_fees": sum(r["fee"] for r in rows if r["prepared"]),
        "faucet_fees": sum(
            json.loads(r["info"]).get("faucet_fee", 0)
            for r in rows
            if r["kind"] == "funding"
        ),
        "accounting": self.j.get("accounting"),
        "accounting_status": accounting_status(self.j, rows, time.time()),
        "fleet": self.j.get("fleet_latest"),
        "resources": {h: self.j.get("latest:" + h) for h in HOSTS},
        "controller": {
            "max_rss": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
            "user_cpu": resource.getrusage(resource.RUSAGE_SELF).ru_utime,
        },
        "coverage": {
            "web": False,
            "snap": False,
            "confidential_amounts": False,
            "public_node_faults": False,
        },
    }
    atomic(self.state / "report.json", report)
    self.j.set("report_at", time.time())
    return report


def workload_complete(journal, events, phases):
    """A delivered schedule cannot erase a safety hold or a missing phase gate."""
    rows = journal.rows("kind='campaign'")
    accounting = journal.get("accounting", {})
    gates = journal.get("phase_gates", {})
    return (
        journal.get("status") == "draining"
        and len(rows) == len(events)
        and all(row["state"] == "reconciled" for row in rows)
        and all(gates.get(phase["id"]) is True for phase in phases)
        and accounting.get("difference") == 0
        and not journal.pending()
    )
