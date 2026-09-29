"""Durable campaign reporting shared by legacy and discovery profiles."""

import json
import resource
import time
from plan import expand
from runtime import HOSTS, atomic, digest


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
