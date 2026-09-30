"""Fresh, isolated v2 execution. No historical run migration or automatic resume."""

import asyncio
import json
import math
import time
from controller import Controller
from runtime import Gate, HOSTS, Quota, atomic, digest
from selection import choose
from fee_evidence import quote_evidence, summarize_fees


def rehearsal_result(
    rows, expected, duration, *, start, cutoff, receipt_deadline, now, offer_grace=120
):
    """Validate the observed cadence against the original immutable offer clock.

    One second of span jitter is absolute, not a throughput percentage: a long
    trial cannot accumulate slippage. Startup lag is bounded separately. Every
    rolling ~60-second span and the whole trial must sustain the offered cadence;
    a late batch cannot use confirmation drain to masquerade as peak generation.
    """

    def timestamp(value):
        return type(value) in (int, float) and math.isfinite(value)

    interval = duration / expected
    span_jitter = 1.0
    max_lag = min(offer_grace, max(1.0, min(5.0, 2 * interval)))
    submitted = sum(timestamp(r.get("submitted")) for r in rows)
    reconciled = sum(
        r["state"] == "reconciled"
        and timestamp(r.get("submitted"))
        and timestamp(r.get("finished"))
        for r in rows
    )
    failures = set()
    if len(rows) != expected or reconciled != expected:
        failures.add("missing_reconciled_offers")
    valid = all(
        timestamp(r.get(k)) for r in rows for k in ("offered", "submitted", "finished")
    )
    actual_rate, actual_span, maximum_lag = None, None, None
    rolling = {"count": 0, "min_per_minute": None, "max_per_minute": None}
    if not valid or not rows:
        failures.add("missing_or_invalid_timestamps")
    else:
        ordered = sorted(rows, key=lambda r: r["offered"])
        times = [r["submitted"] for r in ordered]
        lags = [r["submitted"] - r["offered"] for r in ordered]
        maximum_lag = max(lags)
        for i, row in enumerate(ordered):
            if abs(row["offered"] - (start + i * interval)) > 1e-6:
                failures.add("offer_clock_changed")
            if not 0 <= lags[i] <= max_lag:
                failures.add("submission_offer_lag")
            if not row["submitted"] < min(cutoff, row["offered"] + offer_grace):
                failures.add("submission_cutoff")
            if not row["submitted"] <= row["finished"] <= min(receipt_deadline, now):
                failures.add("receipt_timestamps")
        if len(times) > 1:
            actual_span = times[-1] - times[0]
            if actual_span > 0:
                actual_rate = (len(times) - 1) * 60 / actual_span
            if any(b <= a for a, b in zip(times, times[1:])):
                failures.add("non_monotonic_submissions")
            nominal_span = (expected - 1) * interval
            if abs(actual_span - nominal_span) > span_jitter:
                failures.add("overall_cadence")
            step = min(len(times) - 1, max(1, math.ceil(60 / interval)))
            spans = [times[i + step] - times[i] for i in range(len(times) - step)]
            if any(abs(span - step * interval) > span_jitter for span in spans):
                failures.add("rolling_cadence")
            rates = [step * 60 / span for span in spans if span > 0]
            rolling = {
                "count": len(spans),
                "intervals_per_window": step,
                "nominal_span_seconds": step * interval,
                "min_per_minute": min(rates) if rates else None,
                "max_per_minute": max(rates) if rates else None,
            }
        else:
            failures.add("insufficient_submission_span")
    return {
        "status": "generator_limited" if failures else "passed",
        "failures": sorted(failures),
        "offered": len(rows),
        "expected": expected,
        "submitted": submitted,
        "reconciled": reconciled,
        "offered_per_minute": expected * 60 / duration,
        "submitted_per_minute": actual_rate,
        "submission_span_seconds": actual_span,
        "max_offer_lag_seconds": maximum_lag,
        "rolling_windows": rolling,
        "timing_tolerance": {
            "span_jitter_seconds": span_jitter,
            "max_offer_lag_seconds": max_lag,
        },
        "offer_start": start,
        "submission_cutoff": cutoff,
        "receipt_deadline": receipt_deadline,
        "coverage": "observed rehearsal delivery/cadence only; not network capacity or 72-hour sustainability",
    }


class DiscoveryController(Controller):
    def __init__(self, state):
        # Validate isolation before opening a writable journal.
        config = json.loads((state / "launch.json").read_text())
        plan = json.loads((state / "plan.json").read_text())
        if config.get("isolated_discovery") is not True or not config.get("targets"):
            raise Gate("explicit isolated discovery targets required")
        balances = config.get("opening_balances")
        if (
            not isinstance(balances, list)
            or len(balances) != plan["wallets"]["count"]
            or any(type(x) is not int or x <= 0 for x in balances)
        ):
            raise Gate("exact positive prefunded opening balances required")
        if sum(balances) > int(plan["funding"]["max_principal_picocredits"]):
            raise Gate("opening principal exceeds budget")
        from discovery_plan import validate_lifetime

        validate_lifetime(plan, config)
        super().__init__(state)
        self.fee_task = None

    def accounting(self):
        # The reconciler starts concurrently with setup: opening funds must be
        # independently verified before it compares them against the ledger.
        if not self.j.get("opening_verified"):
            return
        super().accounting()

    def spendable(self, wallet, height, amount=0, count=None):
        selected, evidence = choose(self, wallet, height, amount, count)
        self.j.set("selection_latest", evidence)
        return selected

    def intent_deadline(self, row):
        deadline = super().intent_deadline(row)
        if row["kind"] == "rehearsal":
            deadline = min(
                deadline,
                self.j.get("rehearsal_start")
                + self.plan["rehearsal"]["duration_seconds"],
            )
        return deadline

    def gate(self):
        super().gate()
        if (
            self.j.get("start")
            and self.j.get("end") + 1800 > self.config["infrastructure_end"]
        ):
            raise Gate("campaign exceeds immutable infrastructure cutoff")

    async def observe_fees(self):
        while not self.closed:
            try:
                for host in HOSTS:
                    quote = await self.rpc.call(host, "fee_getRate")
                    evidence = quote_evidence(
                        quote, self.statuses[host], host, time.time()
                    )
                    self.j.event(None, "fee_quote", evidence)
                    latest = self.j.get("fee_latest", {})
                    latest[host] = evidence
                    self.j.set("fee_latest", latest)
                # Preserve each finalized block's observed transaction count/hash.
                # Bounded catch-up never claims observation of skipped blocks.
                tip = min(s["chainHeight"] for s in self.statuses.values())
                previous = self.j.get("fee_block_height", tip - 1)
                if tip - previous > 32:
                    self.j.event(
                        None, "fee_block_gap", {"from": previous + 1, "to": tip - 32}
                    )
                for height in range(max(previous + 1, tip - 31), tip + 1):
                    block = await self.rpc.call(
                        HOSTS[height % 5], "getBlockByHeight", {"height": height}
                    )
                    self.j.event(
                        None,
                        "fee_block",
                        {
                            "height": height,
                            "hash": block["hash"],
                            "timestamp": block["timestamp"],
                            "tx_count": block["txCount"],
                            "transactions": block.get("transactions", []),
                        },
                    )
                    self.j.set("fee_block_height", height)
            except Quota as error:
                self.j.event(None, "fee_observation_quota", str(error))
            except Exception as error:
                self.halt("fee evidence: " + str(error))
            await asyncio.sleep(15)

    def inventory_ready(self, height):
        required = self.plan["wallets"]["min_mature_utxos_per_wallet"]
        headroom = self.plan["wallets"]["decoy_headroom"]
        largest = max(map(int, self.plan["wallets"]["transfer_amounts_picocredits"]))
        evidence = []
        for w in range(len(self.wallets)):
            ready, counts = choose(self, w, height, headroom=headroom)
            selected, _ = choose(self, w, height, largest, 4, headroom)
            evidence.append({**counts, "four_input_probe_feasible": bool(selected)})
            if len(ready) < required or not selected:
                self.j.set(
                    "inventory_readiness",
                    {"status": "generator_limited", "wallets": evidence},
                )
                return False
        self.j.set("inventory_readiness", {"status": "ready", "wallets": evidence})
        return True

    async def setup_tick(self):
        if self.j.get("status") != "setup":
            raise Gate("setup is not active")
        now = time.time()
        if now >= self.j.get("setup_start") + self.plan["setup_deadline_hours"] * 3600:
            raise Gate(
                "generator_limited: setup deadline before sustainable inventory/rehearsal"
            )
        if not self.j.get("opening_verified"):
            await self.sync(full=True)
            states = {s["keyImage"]: s for s in self.spent}
            if any(s["pending"] for s in states.values()):
                raise Gate("prefunding has pending inputs")
            balances = [
                sum(
                    o["utxo"]["amount"]
                    for o in owned
                    if not states[o["key_image"]]["spent"]
                )
                for owned in self.inventory
            ]
            if balances != self.config["opening_balances"]:
                raise Gate("prefunded opening balances differ from pinned launch")
            # Exact opening inventory becomes immutable once any rehearsal signs.
            self.j.set("opening_balance", sum(balances))
            lottery = {
                (o["txHash"], o["outputIndex"]): int.from_bytes(
                    bytes.fromhex(o["amountCommitment"]), "little"
                )
                for b in self.blocks
                for o in b["outputs"]
                if o.get("lottery")
            }
            awards = sum(
                lottery.get(
                    (bytes(o["utxo"]["tx_hash"]).hex(), o["utxo"]["output_index"]), 0
                )
                for owned in self.inventory
                for o in owned
            )
            self.j.set("opening_lottery", awards)
            self.j.set("opening_verified", True)
            self.accounting()
        start = self.j.get("rehearsal_start")
        settings = self.plan["rehearsal"]
        if start is None:
            height = await self.sync()
            if not self.inventory_ready(height):
                raise Gate(
                    "generator_limited: pre-funded inventory/decoy headroom insufficient"
                )
            start = time.time() + 15
            self.j.set("rehearsal_start", start)
        offer_seconds = settings["duration_seconds"] - 120
        total = offer_seconds // settings["interval_seconds"]
        events = []
        for i in range(total):
            events.append(
                {
                    "intent": f"rehearsal-{i:05d}",
                    "offset_seconds": i * settings["interval_seconds"],
                    "sender": f"wallet-{i % len(self.wallets) + 1}",
                    "recipient": f"wallet-{(i + 1) % len(self.wallets) + 1}",
                    "amount_picocredits": self.plan["wallets"][
                        "transfer_amounts_picocredits"
                    ][
                        (i // 3)
                        % len(self.plan["wallets"]["transfer_amounts_picocredits"])
                    ],
                    "max_inflight": self.plan["limits"]["max_inflight"],
                    "fee_multiplier": (1, 2, 4)[i % 3],
                    "cohort": f"rehearsal-{i // 3}",
                    "input_count": (1, 2, 4)[i // 3] if i < 9 else None,
                }
            )
        if now < start + settings["duration_seconds"]:
            await self.deliver(events, start, "rehearsal")
            return
        # Record every missing offer even when a slow signer crosses the boundary.
        self.materialize(events, start, "rehearsal", now)
        if (
            self.j.pending()
            and now < start + settings["duration_seconds"] + settings["drain_seconds"]
        ):
            return
        result = rehearsal_result(
            self.j.rows("kind='rehearsal'"),
            total,
            offer_seconds,
            start=start,
            cutoff=start + settings["duration_seconds"],
            receipt_deadline=start
            + settings["duration_seconds"]
            + settings["drain_seconds"],
            now=time.time(),
            offer_grace=self.j.limits["start_slot_grace_seconds"],
        )
        self.j.set("rehearsal_result", result)
        if result["status"] != "passed":
            raise Gate("generator_limited: short peak-rate rehearsal incomplete")
        height = await self.sync(full=True)
        self.accounting()
        if not self.inventory_ready(height):
            raise Gate("generator_limited: inventory depleted by rehearsal")
        # Full scans yield to the monitor; a safety hold must survive completion.
        self.gate()
        latest = self.j.get("fee_latest", {})
        if set(latest) != set(HOSTS) or any(
            not 0 <= time.time() - q["at"] <= 60 for q in latest.values()
        ):
            raise Gate("rehearsal fee observations missing or stale")
        if self.plan.get("execution_mode") == "rehearsal_only":
            with self.j.transaction():
                self.j.set("status", "rehearsal_complete")
                self.j.set("stopped_at", time.time())
                self.j.set("reason", "qualification passed; campaign not requested")
            self.report()
            return
        start = time.time() + 60
        if start + 72 * 3600 + 1800 > self.config["infrastructure_end"]:
            raise Gate("rehearsal left insufficient fixed infrastructure lifetime")
        with self.j.transaction():
            self.j.set("start", start)
            self.j.set("end", start + 72 * 3600)
            self.j.set("status", "running")
            self.j.set("active_phase", "correctness")
            self.j.set("phase_gates", {"correctness": True})
        atomic(
            self.state / "activated.json",
            {
                "start": start,
                "end": start + 72 * 3600,
                "plan_sha256": digest(self.plan),
                "run_id": self.config["run_id"],
                "rehearsal": result,
                "fee_coverage": "not_exercised",
            },
        )
        self.report()

    def materialize(self, events, start, kind, now):
        grace = self.j.limits["start_slot_grace_seconds"]
        cursor = self.j.get("offer_cursor:" + kind, 0)
        while cursor < len(events):
            event = events[cursor]
            at = start + event["offset_seconds"]
            if at > now:
                break
            sender = int(event["sender"].split("-")[-1]) - 1
            recipient = int(event["recipient"].split("-")[-1]) - 1
            with self.j.transaction():
                self.j.offer(
                    event["intent"],
                    kind,
                    at,
                    sender,
                    recipient,
                    int(event["amount_picocredits"]),
                )
                cursor += 1
                self.j.set("offer_cursor:" + kind, cursor)
        for row in self.j.rows(
            "state IN ('planned','eligible') AND offered<?", (now - grace,)
        ):
            self.j.transition(
                row["id"], "skipped", "generator_slot_expired", finished=now
            )
        queued = self.j.rows("state IN ('planned','eligible') ORDER BY offered,id")
        for row in queued[self.j.limits["max_queued_intents"] :]:
            self.j.transition(
                row["id"], "skipped", "generator_queue_limit", finished=now
            )

    async def deliver(self, events, start, kind):
        now = time.time()
        self.materialize(events, start, kind, now)
        pending = self.j.pending()
        lookup = {event["intent"]: event for event in events}
        attempted = 0
        for row in self.j.rows(
            "kind=? AND state IN ('planned','eligible') ORDER BY offered,id", (kind,)
        ):
            event = lookup[row["id"]]
            if len(pending) >= event["max_inflight"]:
                break
            if any(p["sender"] == row["sender"] for p in pending):
                continue
            attempted += 1
            sent = await self.prepare(
                row["id"],
                event["input_count"],
                event["max_inflight"],
                fee_multiplier=event["fee_multiplier"],
                cohort=event["cohort"],
            )
            if sent or attempted >= 4:
                return

    async def campaign_tick(self):
        if self.plan.get("execution_mode") == "rehearsal_only":
            raise Gate("campaign forbidden in rehearsal-only mode")
        now = time.time()
        start = self.j.get("start")
        self.materialize(self.events, start, "campaign", now)
        if now >= self.j.get("end"):
            self.j.set("status", "draining")
            self.j.set("stopped_at", self.j.get("end"))
            return
        phase = next(
            (
                p
                for p in self.plan["phases"]
                if p["start_hour"] * 3600 <= now - start < p["end_hour"] * 3600
            ),
            None,
        )
        if phase is None:
            return
        if phase["id"] != self.j.get("active_phase"):
            earlier = self.j.rows(
                "kind='campaign' AND offered<?", (start + phase["start_hour"] * 3600,)
            )
            failed = [
                r
                for r in earlier
                if r["state"]
                not in (
                    "reconciled",
                    "prepared",
                    "submitting",
                    "accepted",
                    "unknown",
                    "confirmed",
                )
            ]
            if failed:
                raise Gate("generator_limited: previous phase delivery incomplete")
            if self.j.pending() or any(r["state"] != "reconciled" for r in earlier):
                boundary = start + phase["start_hour"] * 3600
                self.j.set(
                    "boundary_drain", {"phase": phase["id"], "seconds": now - boundary}
                )
                if now - boundary > self.j.limits["start_slot_grace_seconds"]:
                    raise Gate("previous phase drain exceeded bounded grace")
                return
            await self.sync(full=True)
            self.accounting()
            if self.j.get("active_phase") == "correctness":
                shapes = {json.loads(r["info"]).get("input_count") for r in earlier}
                if not {1, 2, 4} <= shapes:
                    raise Gate("correctness shapes missing")
            if phase["id"] == "recovery":
                await self.restore_wallet(0)
                self.j.set("restore_recovery", True)
            self.j.set("active_phase", phase["id"])
            gates = self.j.get("phase_gates", {})
            gates[phase["id"]] = True
            self.j.set("phase_gates", gates)
        await self.deliver(self.events, start, "campaign")

    def report(self):
        # Dense workloads must not rewrite a growing report on each confirmation.
        status = self.j.get("status")
        cached = getattr(self, "_report", None)
        if (
            status in ("setup", "running")
            and cached
            and cached["status"] == status
            and time.time() - cached["at"] < 60
        ):
            return cached
        report = super().report()
        quotes = [
            json.loads(row[0])
            for row in self.j.db.execute(
                "SELECT detail FROM events WHERE state='fee_quote'"
            )
        ]
        report["fee_coverage"] = summarize_fees(self.j.rows(), quotes)
        from reporting import rehearsal_summary

        report["rehearsal"] = rehearsal_summary(
            self.j, self.plan, report["artifact_evidence"]
        )
        report["seed"] = self.plan["seed"]
        rows = self.j.rows("kind='campaign'")
        delivered = len(rows) == len(self.events) and all(
            r["state"] == "reconciled" for r in rows
        )
        delivery = (
            "completed"
            if delivered
            else "failed"
            if status in ("held", "incomplete")
            else "in_progress"
        )
        if self.plan.get("execution_mode") == "rehearsal_only":
            report["not_yet_offered"] = report["rehearsal"]["not_yet_offered"]
            delivery = "not_requested"
            report["qualification_status"] = (
                "passed"
                if status == "rehearsal_complete"
                else "failed"
                if status in ("held", "incomplete")
                else "in_progress"
            )
        report["execution_mode"] = self.plan.get("execution_mode", "campaign")
        report["workload_delivery"] = delivery
        report["coverage_status"] = "incomplete"
        report["discovery"] = {
            "delivery_status": delivery,
            "coverage_status": "incomplete",
            "selection": self.j.get("selection_latest"),
            "inventory": self.j.get("inventory_readiness"),
            "fee_capacity_gap": "Production activation needs 3s slots and >75% EMA fullness (~25+ tx/s); this generator does not establish that capacity.",
        }
        if status == "complete":
            report.update(
                status="incomplete",
                reason="workload completed; production fee activation/priority coverage not established",
            )
        atomic(self.state / "report.json", report)
        self._report = report
        return report

    async def run(self):
        if self.j.get("status") in ("rehearsal_complete", "complete", "incomplete"):
            self.report()
            return

        async def fees_when_ready():
            while not self.closed and not self.statuses:
                await asyncio.sleep(0.2)
            if not self.closed:
                await self.observe_fees()

        self.fee_task = asyncio.create_task(fees_when_ready())
        try:
            await super().run()
        finally:
            self.fee_task.cancel()
            await asyncio.gather(self.fee_task, return_exceptions=True)
