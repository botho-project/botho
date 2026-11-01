"""Durable campaign reporting shared by legacy and discovery profiles."""

import hashlib
import json
import math
import os
import resource
import stat
import time
from lottery import award_spend_expectation
from plan import expand
from runtime import HOSTS, atomic, digest


def legacy_lottery_evidence(inventory, lottery_amounts, spent_by_image):
    """Classify checked history/ownership receipts without changing balances.

    A different outpoint sharing the same image is not independently spendable,
    even before that image is spent. The presence of a lottery award alone does
    not classify a unique-key payout as legacy. This is evidence processing,
    not RPC authentication or a repair of the historical claims.
    """
    by_image = {}
    for owned in inventory:
        for output in owned:
            utxo = output["utxo"]
            outpoint = (bytes(utxo["tx_hash"]).hex(), utxo["output_index"])
            by_image.setdefault(output["key_image"], set()).add(outpoint)

    groups = []
    for image, outpoints in sorted(by_image.items()):
        awards = sorted(outpoints.intersection(lottery_amounts))
        if len(outpoints) < 2 or not awards:
            continue
        state = spent_by_image[image]
        groups.append({
            "key_image": image,
            "outpoints": [f"{tx}:{index}" for tx, index in sorted(outpoints)],
            "awards": [
                {"outpoint": f"{tx}:{index}", "amount_picocredits": lottery_amounts[(tx, index)]}
                for tx, index in awards
            ],
            "spent": state["spent"],
            "spent_height": state.get("spentHeight"),
        })
    return {
        "groups": groups,
        "award_count": sum(len(group["awards"]) for group in groups),
        "aliased_awards_picocredits": sum(
            award["amount_picocredits"] for group in groups for award in group["awards"]
        ),
        "spent_image_awards_picocredits": sum(
            award["amount_picocredits"]
            for group in groups if group["spent"] for award in group["awards"]
        ),
    }


LOTTERY_V2_COUNTS = (
    "received_count", "received_picocredits", "spent_count",
    "spent_picocredits", "unspent_count", "unspent_picocredits",
)


def lottery_v2_summary(snapshot):
    """V2 award counts from the latest accounting snapshot, or ``None`` (#1643)."""
    v2 = (snapshot or {}).get("lottery_v2")
    if not isinstance(v2, dict):
        return None
    return {**{k: v2.get(k) for k in LOTTERY_V2_COUNTS},
            "undiscovered_count": len(v2.get("undiscovered") or ())}


def artifact_evidence(state, rows, byte_limit):
    """Audit only controller publications; never inspect signatures or infer sends.

    Missing archives are unavailable evidence, not an empty artifact directory.
    Hash only disagreements, keeping ordinary reporting independent of total
    signed bytes. SHA256 identifies file contents, not the transaction hash.

    ``artifacts/expired/`` holds the controller's quarantine of signed
    artifacts whose intent never reached ``prepared`` (#1635): never
    submittable, never orphans, counted as ``expired_unsubmitted``.
    """
    directory = state / "artifacts"
    result = {
        "status": "unavailable",
        "directory": str(directory),
        "orphaned_signed": None,
        "ambiguous_signed": None,
        "expired_unsubmitted": None,
        "findings": [],
        "signature_validation": "not_performed",
        "submission_evidence": "journal_only",
    }
    try:
        if not stat.S_ISDIR(directory.lstat().st_mode):
            raise OSError("artifact directory is not a regular directory")
        paths = sorted(directory.iterdir())
    except OSError as error:
        result["reason"] = str(error)
        return result
    intents = {row["id"]: row for row in rows}
    expired = 0
    for path in paths:
        if path.name == "expired" and stat.S_ISDIR(path.lstat().st_mode):
            try:
                expired = sum(1 for _ in path.iterdir())
            except OSError as error:
                result["reason"] = str(error)
                return result
            continue
        row = intents.get(path.stem)
        if (
            path.suffix == ".bin"
            and row
            and row["prepared"] is not None
            and row["hash"]
        ):
            continue
        finding = {
            "path": str(path),
            "intent_id": row["id"] if row else None,
            "kind": row["kind"] if row else None,
            "classification": "orphaned_signed"
            if path.suffix == ".bin"
            else "ambiguous_signed",
            "reason": "published artifact lacks a journal preparation record"
            if path.suffix == ".bin"
            else "partial or unexpected artifact",
            "journal_transaction_hash": row["hash"] if row else None,
            "sha256": None,
            "bytes": None,
        }
        try:
            # No symlinks, special files, or unbounded reads of malformed files.
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(fd, "rb") as stream:
                before = os.fstat(stream.fileno())
                if not stat.S_ISREG(before.st_mode) or before.st_size > byte_limit:
                    raise OSError("not a bounded regular artifact")
                data = stream.read(byte_limit + 1)
                after = os.fstat(stream.fileno())
                if len(data) != before.st_size or (
                    before.st_size,
                    before.st_mtime_ns,
                ) != (after.st_size, after.st_mtime_ns):
                    raise OSError("artifact changed during inspection")
                finding.update(sha256=hashlib.sha256(data).hexdigest(), bytes=len(data))
        except OSError as error:
            finding.update(classification="ambiguous_signed", reason=str(error))
        result["findings"].append(finding)
    result.update(
        status="checked",
        expired_unsubmitted=expired,
        **{
            kind: sum(f["classification"] == kind for f in result["findings"])
            for kind in ("orphaned_signed", "ambiguous_signed")
        },
    )
    return result


def accounting_status(journal, rows, now, artifacts=None):
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
        if snapshot.get("legacy_lottery", {}).get("award_count", 0):
            reasons.append("unsupported legacy lottery payouts")
        if (snapshot.get("lottery_v2") or {}).get("undiscovered"):
            reasons.append("undiscovered lottery award")
    if unresolved:
        reasons.append("signed or submitted work remains unreconciled")
    if reserved:
        reasons.append("retained input reservations")
    if artifacts and artifacts["findings"]:
        reasons.append("artifact/journal disagreement")
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


DEFERRAL_REASONS = ("sender_busy", "inflight_cap", "admission_backlog")


def offer_deferrals(journal, kind):
    """Offers of ``kind`` held back while due, by reason (#1615).

    Counts distinct offers, not ticks. ``binding_limit`` names the reason that
    held back the most offers (None when nothing was deferred);
    ``skipped_after_deferral`` counts deferred offers that later missed their
    start slot, the outcome that stops a run.
    """
    by_reason = {reason: set() for reason in DEFERRAL_REASONS}
    for intent, detail in journal.db.execute(
        "SELECT intent, detail FROM events WHERE state='offer_deferred'"
    ):
        info = json.loads(detail)
        if info.get("kind") == kind and info.get("reason") in by_reason:
            by_reason[info["reason"]].add(intent)
    deferred = set().union(*by_reason.values())
    skipped = {
        r["id"] for r in journal.rows("kind=? AND state='skipped'", (kind,))
    }
    counts = {reason: len(ids) for reason, ids in by_reason.items()}
    binding = max(DEFERRAL_REASONS, key=lambda r: counts[r]) if deferred else None
    return {
        **counts,
        "offers_deferred": len(deferred),
        "skipped_after_deferral": len(deferred & skipped),
        "binding_limit": binding,
    }


def rehearsal_summary(journal, plan, artifacts=None):
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
    findings = [
        f for f in (artifacts or {}).get("findings", []) if f["kind"] == "rehearsal"
    ]
    uncertain = {f["intent_id"] for f in findings}
    checked = artifacts is not None and artifacts["status"] == "checked"
    # Present only under the plan opt-in (#1613): never readable as a pass.
    acceptance = journal.get("rehearsal_acceptance")
    accepted = {"accepted_as": acceptance["accepted_as"]} if acceptance else {}
    return {
        **result,
        **accepted,
        "expected": expected,
        "offered": len(rows),
        "not_yet_offered": max(0, expected - len(rows)),
        "submitted": sum(r["submitted"] is not None for r in rows),
        "signed_unsubmitted": sum(
            r["prepared"] is not None and r["submitted"] is None for r in rows
        ),
        "orphaned_signed_artifacts": sum(
            f["classification"] == "orphaned_signed" for f in findings
        )
        if checked
        else None,
        "ambiguous_signed_artifacts": sum(
            f["classification"] == "ambiguous_signed" for f in findings
        )
        if checked
        else None,
        "unsigned": sum(
            r["prepared"] is None
            and r["submitted"] is None
            and r["id"] not in uncertain
            for r in rows
        ),
        "unsigned_basis": "journal_and_artifact_presence"
        if checked
        else "journal_only",
        "reconciled": counts.get("reconciled", 0),
        "state_counts": counts,
        "offer_deferrals": offer_deferrals(journal, "rehearsal"),
    }


DEGRADE_MILESTONES = ("first_reachable_at", "synced_at_tip_at", "first_ballot_at", "first_coinbase_at")


def degrade_report(journal, steps):
    """One row per planned degrade step (#1591); times are seconds from restore_at.

    ``verdict`` is ``pass``/``fail`` once taken, ``open`` when the window_end
    checks passed but after-window evidence (payments, deferred receipts,
    accounting) is still owed, ``in_progress`` inside the window, and
    ``not_started`` before it. Over-1 s loop counts are recorded only.
    """
    rows = []
    for step in steps:
        record = journal.get("degrade:" + step["id"])
        row = {
            "step": step["id"],
            "action": step["action"],
            "expect": step["expect"],
            "targets": list(step["target_hosts"]),
            "verdict": "not_started",
            "failures": [],
            "awaiting": [],
            "fault_observed": None,
            "milestones": {},
            "outage_blocks": None,
            "catchup_blocks_per_second": {},
            "loop": {},
        }
        if record is not None:
            verdict = record.get("verdict")
            restore = record.get("restore_at")
            row["verdict"] = (
                verdict["status"] if verdict
                else "open" if record.get("closed_at") is not None
                else "in_progress"
            )
            row["failures"] = list(verdict.get("failures") or []) if verdict else []
            row["awaiting"] = list(record.get("awaiting") or [])
            row["fault_observed"] = record.get("fault_observed")
            for host, track in (record.get("hosts") or {}).items():
                row["milestones"][host] = {
                    name: None if track.get(name) is None else track[name] - restore
                    for name in DEGRADE_MILESTONES
                }
                reachable, synced = track.get("first_reachable_at"), track.get("synced_at_tip_at")
                gained = (
                    track["synced_height"] - track["reachable_height"]
                    if synced is not None and track.get("reachable_height") is not None
                    else None
                )
                row["catchup_blocks_per_second"][host] = (
                    gained / (synced - reachable)
                    if gained is not None and synced > reachable
                    else None
                )
            if record.get("tip_fault_start") is not None and record.get("tip_restore") is not None:
                row["outage_blocks"] = record["tip_restore"] - record["tip_fault_start"]
            row["loop"] = {
                host: None if delta is None
                else {"over_1s": delta["over_1s"], "delayed": delta["delayed"]}
                for host, delta in (record.get("loops") or {}).items()
            }
        rows.append(row)
    return rows


def controller_report(self):
    rows = self.j.rows()
    artifacts = artifact_evidence(
        self.state, rows, self.j.limits["max_signed_transaction_bytes"]
    )
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

    # Empty without the opt-in and for a rehearsal-only plan: never vacuous.
    degrade = degrade_report(self.j, getattr(self, "degrade_steps", None) or [])
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
        # V2 awards received and spent, counted apart from transfers (#1643).
        "lottery_v2": lottery_v2_summary(self.j.get("accounting")),
        # D0 "discover AND spend" (#1681): awards received must be spent.
        "lottery_v2_spend_expectation": award_spend_expectation(self.j),
        "artifact_evidence": artifacts,
        "accounting_status": accounting_status(self.j, rows, time.time(), artifacts),
        "fleet": self.j.get("fleet_latest"),
        "resources": {h: self.j.get("latest:" + h) for h in HOSTS},
        "controller": {
            "max_rss": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
            "user_cpu": resource.getrusage(resource.RUSAGE_SELF).ru_utime,
        },
        "degrade": degrade,
        "coverage": {
            "validator_degrade": bool(degrade)
            and all(row["verdict"] == "pass" for row in degrade),
            "web": False,
            "snap": False,
            "confidential_amounts": False,
            "public_node_faults": False,
            "lottery_v2_spend": award_spend_expectation(self.j)["status"] == "pass",
        },
    }
    atomic(self.state / "report.json", report)
    return report


def workload_complete(journal, events, phases, degrade_steps=()):
    """A delivered schedule cannot erase a safety hold or a missing phase gate.

    Every planned degrade step also needs a stored ``pass`` verdict (#1591),
    so a controller restart that missed a verdict cannot report complete.
    A campaign that received more V2 awards than its pinned threshold but
    spent none fails the #1681 expectation and is not complete either.
    """
    rows = journal.rows("kind='campaign'")
    accounting = journal.get("accounting", {})
    gates = journal.get("phase_gates", {})
    return (
        journal.get("status") == "draining"
        and len(rows) == len(events)
        and all(row["state"] == "reconciled" for row in rows)
        and all(gates.get(phase["id"]) is True for phase in phases)
        and accounting.get("difference") == 0
        and award_spend_expectation(journal)["status"] != "fail"
        and not journal.pending()
        and all(
            ((journal.get("degrade:" + step["id"]) or {}).get("verdict") or {}).get("status")
            == "pass"
            for step in degrade_steps
        )
    )
