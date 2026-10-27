"""Fresh, isolated v2 execution. No historical run migration or automatic resume."""

import asyncio
import json
import math
import time
from controller import Controller
from runtime import Gate, HOSTS, Quota, atomic, digest
from selection import choose
from fee_evidence import quote_evidence, summarize_fees

# #1576 decision 1: the only rehearsal failures a plan opt-in may accept.
# Each is a cadence measurement; ledger or timestamp failures always gate.
CADENCE_FAILURES = frozenset(
    {
        "non_monotonic_submissions",
        "overall_cadence",
        "rolling_cadence",
        "submission_offer_lag",
    }
)
ACCEPTED_AS = "generator_limited (plan opt-in, #1576 decision 1)"

# #1618: one admission pass (one run-loop tick) may prepare and submit several
# due offers. After the first, an attempt starts only while the tick is younger
# than this budget, and never more than MAX_ATTEMPTS_PER_PASS (the pre-#1618
# bound) per pass. x1 attempt b measured ~2.55 s between prepares (0.69 s of
# it signing), so a 1 s budget would still admit one per tick; 4 s admits up
# to three (~40/min), and the 40/min submission quota stays the rate limit.
# admission_lock is only taken here; the monitor and reconciler run
# concurrently, and every prepare() re-checks gate() (STOP, holds, deadline).
ADMISSION_TICK_BUDGET_SECONDS = 4.0
MAX_ATTEMPTS_PER_PASS = 4


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

    # -- phase-gate slot anchoring (#1627) -----------------------------------
    # A phase boundary drains the previous phase (bounded by the slot grace),
    # then runs a full sync and accounting before the new phase's gate
    # releases. Offers of the new phase are still materialized at their
    # immutable ``offered`` times while that happens, but their start slot
    # (``start_slot_grace_seconds``) is measured from
    # ``max(offered, release of the row's phase)``: the gate's own wait never
    # uses up their grace. A row whose phase gate has not released has no slot
    # yet and never expires. ``offered`` is never rewritten, and every lag
    # metric stays measured from it (see note_deferral and the report's
    # ``discovery.phase_releases``).

    def campaign_phase(self, offered):
        """The plan phase containing a campaign offer time, or None."""
        start = self.j.get("start")
        if start is None:
            return None
        elapsed = offered - start
        return next(
            (
                p
                for p in self.plan.get("phases", ())
                if p["start_hour"] * 3600 <= elapsed < p["end_hour"] * 3600
            ),
            None,
        )

    def phase_releases(self):
        """Gate-release time of every released phase, keyed by phase id.

        Recorded durably with ``active_phase`` (campaign_tick). A phase gated
        before #1627 has no record; its slots were measured from the offer
        clock, which is exactly a release at its own boundary.
        """
        releases = dict(self.j.get("phase_release") or {})
        start = self.j.get("start")
        gates = self.j.get("phase_gates") or {}
        phases = self.plan.get("phases", ())
        ids = [p["id"] for p in phases]
        active = self.j.get("active_phase")
        # Phases up to the active one have released, whatever was recorded.
        reached = ids.index(active) + 1 if active in ids else 0
        for i, phase in enumerate(phases):
            if phase["id"] in releases:
                continue
            if i < reached or gates.get(phase["id"]) is True:
                releases[phase["id"]] = start + phase["start_hour"] * 3600
        return releases

    def slot_anchor(self, row, releases=None):
        """When a row's start slot begins; None while its phase gate is closed."""
        if row["kind"] != "campaign":
            return row["offered"]
        phase = self.campaign_phase(row["offered"])
        if phase is None:
            return row["offered"]
        releases = self.phase_releases() if releases is None else releases
        release = releases.get(phase["id"])
        if release is None:
            return None
        return max(row["offered"], release)

    def intent_deadline(self, row):
        anchor = self.slot_anchor(row)
        if anchor is None:
            # No slot before its phase gate releases: never admissible.
            return -math.inf
        deadline = anchor + self.j.limits["start_slot_grace_seconds"]
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
                # A host the active degrade step names is skipped (and the
                # skip recorded); every other host still fails closed.
                view = self.fault_view()
                live = self.live_hosts(view=view)
                for host in HOSTS:
                    if host not in live:
                        self.j.event(
                            None,
                            "fee_quote_skipped",
                            {"host": host, "step": view["step"], "at": time.time()},
                        )
                        continue
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
                tip = self.fleet_height()
                previous = self.j.get("fee_block_height", tip - 1)
                if tip - previous > 32:
                    self.j.event(
                        None, "fee_block_gap", {"from": previous + 1, "to": tip - 32}
                    )
                for height in range(max(previous + 1, tip - 31), tip + 1):
                    live = self.live_hosts()
                    block = await self.rpc.call(
                        live[height % len(live)], "getBlockByHeight", {"height": height}
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
        if result["status"] != "passed" and not self.accept_generator_limited(result):
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
                self.j.set(
                    "reason",
                    "qualification accepted as " + ACCEPTED_AS + "; campaign not requested"
                    if self.j.get("rehearsal_acceptance")
                    else "qualification passed; campaign not requested",
                )
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
            # The first phase has no boundary wait: its slots start on time.
            self.j.set("phase_release", {"correctness": start})
        atomic(
            self.state / "activated.json",
            {
                "start": start,
                "end": start + 72 * 3600,
                "plan_sha256": digest(self.plan),
                "run_id": self.config["run_id"],
                "rehearsal": result,
                **(
                    {"rehearsal_acceptance": self.j.get("rehearsal_acceptance")}
                    if self.j.get("rehearsal_acceptance")
                    else {}
                ),
                "fee_coverage": "not_exercised",
            },
        )
        self.report()

    def accept_generator_limited(self, result):
        """#1576 decision 1: a plan may accept a cadence-only miss.

        Only a ``generator_limited`` result whose failures are all cadence
        checks, under the plan's ``rehearsal.generator_limited_acceptable``
        opt-in, with a clean ledger: every offer reconciled, nothing signed
        but unsubmitted, nothing unsigned, and a checked artifact directory
        with no orphaned or ambiguous signed artifacts. The acceptance is
        journaled; the caller still runs every post-rehearsal gate.
        """
        if (
            result.get("status") != "generator_limited"
            or self.plan["rehearsal"].get("generator_limited_acceptable") is not True
            or not set(result.get("failures", ())) <= CADENCE_FAILURES
        ):
            return False
        from reporting import artifact_evidence, rehearsal_summary

        artifacts = artifact_evidence(
            self.state, self.j.rows(), self.j.limits["max_signed_transaction_bytes"]
        )
        ledger = rehearsal_summary(self.j, self.plan, artifacts)
        expected = result["expected"]
        if not (
            artifacts["status"] == "checked"
            and artifacts["orphaned_signed"] == 0
            and artifacts["ambiguous_signed"] == 0
            and ledger["expected"] == expected
            and ledger["offered"] == result["offered"] == expected
            and ledger["reconciled"] == result["reconciled"] == expected
            and ledger["orphaned_signed_artifacts"] == 0
            and ledger["ambiguous_signed_artifacts"] == 0
            and ledger["signed_unsubmitted"] == 0
            and ledger["unsigned"] == 0
        ):
            return False
        if self.j.get("rehearsal_acceptance") is None:
            acceptance = {
                "accepted_as": ACCEPTED_AS,
                "at": time.time(),
                "result": result,
                "ledger": {
                    k: ledger[k]
                    for k in (
                        "expected",
                        "offered",
                        "reconciled",
                        "orphaned_signed_artifacts",
                        "ambiguous_signed_artifacts",
                        "signed_unsubmitted",
                        "unsigned",
                    )
                },
            }
            with self.j.transaction():
                self.j.event(None, "rehearsal_generator_limited_accepted", acceptance)
                self.j.set("rehearsal_acceptance", acceptance)
        return True

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
        # Expiry is measured from the slot anchor (#1627): rows of a phase
        # whose gate has not released do not expire; the first ones after a
        # release get their full grace from the release. The candidate query
        # stays on ``offered`` (anchor >= offered), so only rows that could
        # have expired are anchored.
        releases = None
        for row in self.j.rows(
            "state IN ('planned','eligible') AND offered<? ORDER BY offered,id",
            (now - grace,),
        ):
            if (
                releases is None
                and row["kind"] == "campaign"
                and self.campaign_phase(row["offered"]) is not None
            ):
                releases = self.phase_releases()
            anchor = self.slot_anchor(row, releases)
            if anchor is None or anchor >= now - grace:
                continue
            self.j.transition(
                row["id"], "skipped", "generator_slot_expired", finished=now
            )
        # The backlog bound counts every queued row, gate-held ones included:
        # a gate wait longer than max_queued_intents offers is a generator
        # limit, reported as such, not a backlog to burst through later.
        queued = self.j.rows("state IN ('planned','eligible') ORDER BY offered,id")
        for row in queued[self.j.limits["max_queued_intents"] :]:
            self.j.transition(
                row["id"], "skipped", "generator_queue_limit", finished=now
            )

    def note_deferral(self, row, reason, now):
        """Journal once per (offer, reason) that a due offer was held back (#1615).

        ``sender_busy``: its sender still has a pending spend
        (max_inflight_per_wallet is 1). ``inflight_cap``: the global or phase
        max_inflight is full. ``admission_backlog`` (#1618): the offer was due
        and admissible but this admission pass ended (tick wall-clock budget or
        the per-pass attempt bound) before it was attempted, i.e. controller
        throughput, not a ledger limit, held it back. The set is rebuilt from
        the journal after a restart, so a deferral is never counted twice.
        """
        seen = getattr(self, "_deferrals", None)
        if seen is None:
            seen = self._deferrals = {
                (intent, json.loads(detail)["reason"])
                for intent, detail in self.j.db.execute(
                    "SELECT intent, detail FROM events WHERE state='offer_deferred'"
                )
            }
        if (row["id"], reason) in seen:
            return
        seen.add((row["id"], reason))
        detail = {"kind": row["kind"], "reason": reason, "lag_seconds": now - row["offered"]}
        # lag_seconds is always from the immutable offer clock (#1627). An
        # offer that arrived while its phase gate was closed also records how
        # much of that lag was the gate's wait.
        anchor = self.slot_anchor(row)
        if anchor is not None and anchor > row["offered"]:
            detail["gate_held_seconds"] = anchor - row["offered"]
        self.j.event(row["id"], "offer_deferred", detail)

    async def deliver(self, events, start, kind):
        """Attempt every due offer this pass can admit, oldest first (#1618).

        One pass used to stop at the first successful prepare, so the serial
        admission cadence (~2.5 s per transaction on x1) capped throughput
        near 23/min under a 30/min offer clock. A pass now keeps preparing
        (each prepare signs, then submits) until one of the existing limits
        binds: sender busy and max_inflight (re-read after every attempt),
        the per-minute submission and wire-byte budgets (begin_submit raises
        Quota, which ends the pass so the signed row keeps its slot and no
        new signature is made), MAX_ATTEMPTS_PER_PASS attempts, or the tick
        wall-clock budget. Every safety gate stays inside prepare() and
        Journal.prepare(). One read-only scan serves the whole pass: inputs
        signed earlier in the pass are reserved before the next selection.
        """
        now = time.time()
        tick_started = getattr(self, "admission_started", None)
        budget_end = (now if tick_started is None else tick_started) + (
            ADMISSION_TICK_BUDGET_SECONDS
        )
        self.materialize(events, start, kind, now)
        lookup = {event["intent"]: event for event in events}
        attempted = 0
        scanned = None

        async def scan():
            # The first attempt scans exactly where prepare() always did
            # (after its gate and sender checks); later attempts reuse it.
            nonlocal scanned
            if scanned is None:
                scanned = await self.sync()
            return scanned

        stale = True
        for row in self.j.rows(
            "kind=? AND state IN ('planned','eligible') ORDER BY offered,id", (kind,)
        ):
            if stale:
                # pending() (incl. confirmed-but-unscanned rows) guards sender
                # reuse; only inflight() rows count against the cap (#1551).
                busy = {p["sender"] for p in self.j.pending()}
                inflight = len(self.j.inflight())
                stale = False
            event = lookup[row["id"]]
            # Classify every due offer this tick cannot attempt (#1615), so a
            # report names the binding limit. Neither branch attempts: once
            # the cap is reached no row is prepared, exactly as before.
            if row["sender"] in busy:
                self.note_deferral(row, "sender_busy", now)
                continue
            if inflight >= event["max_inflight"]:
                self.note_deferral(row, "inflight_cap", now)
                continue
            if attempted and (
                attempted >= MAX_ATTEMPTS_PER_PASS or time.time() >= budget_end
            ):
                self.note_deferral(row, "admission_backlog", time.time())
                continue
            attempted += 1
            stale = True
            await self.prepare(
                row["id"],
                event["input_count"],
                event["max_inflight"],
                fee_multiplier=event["fee_multiplier"],
                cohort=event["cohort"],
                scan=scan,
            )

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
            boundary = start + phase["start_hour"] * 3600
            # Receipts a finished step deferred must be verified on every host
            # before a boundary; that is never a drain wait (#1587). A step
            # still open at the boundary (D1 starts with its phase) is not due.
            ends = {w["step"]: w["window_end"] for w in self.degrade_windows()}
            due = [
                r
                for r in self.deferred_rows()
                if ends.get(json.loads(r["info"]).get("deferred_step"), boundary)
                <= boundary
            ]
            if due:
                raise Gate("deferred step receipts unverified at phase boundary: " + due[0]["id"])
            earlier = self.j.rows("kind='campaign' AND offered<?", (boundary,))
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
                self.j.set(
                    "boundary_drain", {"phase": phase["id"], "seconds": now - boundary}
                )
                if now - boundary > self.j.limits["start_slot_grace_seconds"]:
                    raise Gate("previous phase drain exceeded bounded grace")
                return
            # Boundary scan (#1629): refetch and compare the whole output
            # history and recheck every key image, but extend wallet
            # ownership incrementally; a 64-wallet full restore_check grows
            # with the chain and would hold admission for many minutes. The
            # restore proof itself runs in setup and at the final drain.
            scan_started = time.time()
            await self.sync(full=True, rescan_wallets=False)
            scanned = time.time()
            self.accounting()
            accounted = time.time()
            if self.j.get("active_phase") == "correctness":
                shapes = {json.loads(r["info"]).get("input_count") for r in earlier}
                if not {1, 2, 4} <= shapes:
                    raise Gate("correctness shapes missing")
            if phase["id"] == "recovery":
                await self.restore_wallet(0)
                self.j.set("restore_recovery", True)
            # Release the gate (#1627): the release time starts the slot of
            # every offer this phase made while the gate was closed. It is
            # written atomically with active_phase, so a restart before this
            # point re-runs the gate and a restart after it keeps the release.
            released = time.time()
            held = sum(
                boundary <= start + e["offset_seconds"] < released for e in self.events
            )
            with self.j.transaction():
                releases = self.j.get("phase_release") or {}
                releases[phase["id"]] = released
                self.j.set("phase_release", releases)
                self.j.set("active_phase", phase["id"])
                gates = self.j.get("phase_gates", {})
                gates[phase["id"]] = True
                self.j.set("phase_gates", gates)
                self.j.event(
                    None,
                    "phase_gate_released",
                    {
                        "phase": phase["id"],
                        "boundary": boundary,
                        "released": released,
                        "gate_wait_seconds": released - boundary,
                        "gate_held_offers": held,
                        "boundary_scan_seconds": scanned - scan_started,
                        "accounting_seconds": accounted - scanned,
                    },
                )
        await self.deliver(self.events, start, "campaign")

    def phase_release_report(self):
        """Each released phase's boundary, gate release and gate wait (#1627).

        Offer lag everywhere is measured from the immutable ``offered`` time;
        an offer made inside ``[boundary, released)`` carries up to
        ``gate_wait_seconds`` of boundary-gate wait in that lag, and its start
        slot was measured from ``released``.
        """
        start = self.j.get("start")
        if start is None:
            return {}
        releases = self.phase_releases()
        recorded = self.j.get("phase_release") or {}
        out = {}
        for phase in self.plan.get("phases", ()):
            if phase["id"] not in releases:
                continue
            boundary = start + phase["start_hour"] * 3600
            released = releases[phase["id"]]
            out[phase["id"]] = {
                "boundary": boundary,
                "released": released,
                "gate_wait_seconds": released - boundary,
                "recorded": phase["id"] in recorded,
            }
        return out

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
        from reporting import offer_deferrals, rehearsal_summary

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
                "accepted_generator_limited"
                if status == "rehearsal_complete" and self.j.get("rehearsal_acceptance")
                else "passed"
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
            "phase_releases": self.phase_release_report(),
            "offer_deferrals": {
                kind: offer_deferrals(self.j, kind) for kind in ("rehearsal", "campaign")
            },
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
