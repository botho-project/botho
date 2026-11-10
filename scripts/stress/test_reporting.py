"""Early safety holds must retain the full denominator and accounting limits."""

from test_admission_throughput import settling_quote
import asyncio
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from discovery import DiscoveryController
from reporting import heap_evidence, heap_growth
from runtime import HOSTS, Gate, Journal



def quarantine_reasons(journal, identifier):
    """``artifact_quarantined`` event reasons for ``identifier`` (#2001)."""
    return [json.loads(detail)["reason"] for (detail,) in journal.db.execute(
        "SELECT detail FROM events WHERE intent=? AND state='artifact_quarantined'",
        (identifier,))]

class StopRun(Exception):
    pass


MIB = 1024**2


def heap_rows(hours, anon, file=lambda h: 300 * MIB):
    return [{"at": h * 3600.0, "rss": anon(h) + file(h), "rss_anon": anon(h), "rss_file": file(h)}
            for h in hours]


class HeapGrowthTests(unittest.TestCase):
    def test_flat_heap_with_growing_ledger_is_zero_heap_growth(self):
        heap = heap_growth(heap_rows(range(49), lambda h: 200 * MIB, lambda h: (300 + 5 * h) * MIB))
        self.assertEqual(heap["status"], "measured")
        self.assertEqual(heap["anon_growth_bytes"], 0)
        self.assertEqual(heap["anon_slope_bytes_per_hour_last_window"], 0)
        self.assertEqual(heap["file_growth_bytes"], 240 * MIB)
        self.assertEqual(heap["samples"], 49)

    def test_slope_covers_only_the_final_window(self):
        # 24 h of 1 MiB/h growth, then 24 h flat: whole-run growth, flat slope.
        rows = heap_rows(range(49), lambda h: (200 + min(h, 24)) * MIB)
        heap = heap_growth(rows)
        self.assertEqual(heap["anon_growth_bytes"], 24 * MIB)
        self.assertAlmostEqual(heap["anon_slope_bytes_per_hour_last_window"], 0)
        growing = heap_growth(heap_rows(range(49), lambda h: (200 + 2 * h) * MIB))
        self.assertAlmostEqual(growing["anon_slope_bytes_per_hour_last_window"], 2 * MIB)

    def test_legacy_error_and_mixed_rows(self):
        legacy = [{"at": h * 3600.0, "rss": 500 * MIB} for h in range(5)]
        error = [{"at": 5 * 3600.0, "error": "OSError: gone"}]
        self.assertEqual(heap_growth(legacy)["status"], "unavailable")
        self.assertEqual(heap_growth(legacy + error)["status"], "unavailable")
        self.assertEqual(heap_growth([None, None])["status"], "unavailable")
        new = heap_rows(range(6, 9), lambda h: (100 + h) * MIB)
        heap = heap_growth(legacy + error[:1] + new[:1] + error + new[1:])
        self.assertEqual((heap["first_at"], heap["samples"]), (6 * 3600.0, 3))
        self.assertEqual(heap["anon_growth_bytes"], 2 * MIB)
        # A row with rss_anon but no rss_file still counts for heap.
        heap = heap_growth([{"at": 0, "rss_anon": 1}, {"at": 3600, "rss_anon": 2}])
        self.assertIsNone(heap["file_growth_bytes"])

    def test_offline_evidence_reads_previous_before_current_and_skips_torn_lines(self):
        with tempfile.TemporaryDirectory() as name:
            previous, current = Path(name) / "resources.previous.jsonl", Path(name) / "resources.jsonl"
            rows = heap_rows(range(4), lambda h: (100 + h) * MIB)
            previous.write_text("".join(json.dumps(r) + "\n" for r in rows[:2]))
            current.write_text("".join(json.dumps(r) + "\n" for r in rows[2:]) + '{"at": 9')
            evidence = heap_evidence({"n1": [str(previous), str(current)]})
        self.assertEqual(evidence["n1"]["samples"], 4)
        self.assertEqual(evidence["n1"]["first_rss_anon"], 100 * MIB)
        self.assertAlmostEqual(evidence["n1"]["anon_slope_bytes_per_hour_last_window"], MIB)


class TerminalReportingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.c = object.__new__(DiscoveryController)
        self.c.state = Path(self.tmp.name)
        self.c.config = {"run_id": "early-hold"}
        self.c.plan = json.loads(
            Path(__file__).with_name("testnet-discovery-v2.json").read_text()
        )
        self.c.plan["execution_mode"] = "rehearsal_only"
        self.c.events = []
        self.c.j = Journal(self.c.state / "journal.sqlite")
        self.addCleanup(self.c.j.db.close)
        self.c.j.set("status", "setup")
        # The V2 lottery preflight (#1643) already passed for this campaign.
        self.c.j.set("lottery_preflight", {"at": 999, "hosts": {}})
        self.c.j.set("rehearsal_start", 1000)
        self.opening = {
            "at": 999,
            "opening": 10000,
            "ending": 10000,
            "signed_fees": 0,
            "difference": 0,
        }
        self.c.j.set("accounting", self.opening)

    def row(self, index, state):
        name = f"rehearsal-{index:05d}"
        offered = 1000 + index * 2
        self.c.j.offer(name, "rehearsal", offered, index % 32, (index + 1) % 32, 1)
        fields = {}
        if state != "planned":
            fields.update(prepared=offered + 0.1, fee=10)
        if state == "reconciled":
            fields.update(submitted=offered + 1, finished=offered + 31)
        elif state == "expired":
            fields.update(finished=offered + 120)
        self.c.j.transition(name, state, **fields)
        if state in ("prepared", "expired"):
            self.c.j.db.execute(
                "INSERT INTO reservations VALUES(?,?,?)", (name, name, index % 32)
            )

    def early_hold(self):
        # Replay the retained run's state distribution, not live transactions.
        for index in range(224):
            state = (
                "reconciled"
                if index < 202
                else "expired"
                if index == 202
                else "prepared"
                if index == 203
                else "planned"
            )
            self.row(index, state)
        with patch("discovery.time.time", return_value=1800):
            self.c.halt("prepared intent expired; review reserved inputs")
            return self.c.report()

    def test_early_hold_reconciles_all_540_planned_offers_without_journal_writes(self):
        report = self.early_hold()
        rehearsal = report["rehearsal"]
        self.assertIsNotNone(rehearsal)
        self.assertEqual(rehearsal["expected"], 540)
        self.assertEqual(rehearsal["offered"], 224)
        self.assertEqual(rehearsal["not_yet_offered"], 316)
        self.assertEqual(report["not_yet_offered"], 316)
        self.assertEqual(rehearsal["submitted"], 202)
        self.assertEqual(rehearsal["reconciled"], 202)
        self.assertEqual(rehearsal["signed_unsubmitted"], 2)
        self.assertEqual(rehearsal["unsigned"], 20)
        self.assertEqual(
            rehearsal["state_counts"],
            {"reconciled": 202, "expired": 1, "prepared": 1, "planned": 20},
        )
        self.assertEqual(
            sum(rehearsal["state_counts"].values()) + rehearsal["not_yet_offered"], 540
        )
        self.assertEqual(report["counts"]["rehearsal:expired"], 1)
        self.assertEqual(len(self.c.j.rows()), 224)
        self.assertEqual(
            self.c.j.db.execute("SELECT COUNT(*) FROM reservations").fetchone()[0], 2
        )
        self.assertEqual(report["qualification_status"], "failed")

    def test_report_has_heap_growth_for_every_host_including_legacy_baselines(self):
        legacy = {"at": 900, "rss": 300 * MIB}
        for host in HOSTS:
            self.c.j.set("baseline:" + host, legacy)
            self.c.j.set("latest:" + host, {"at": 1700, "rss": 500 * MIB,
                                            "rss_anon": 100 * MIB, "rss_file": 400 * MIB})
        self.c.j.set("baseline:" + HOSTS[0], {**legacy, "rss_anon": 90 * MIB, "rss_file": 210 * MIB})
        self.c.j.set("rss_cycles:" + HOSTS[0], 1)
        self.c.j.set("rss_cycles_metric:" + HOSTS[0], "rss_anon")
        report = self.early_hold()
        self.assertEqual(set(report["heap_growth"]), set(HOSTS))
        heap = report["heap_growth"][HOSTS[0]]
        self.assertEqual((heap["anon_growth_bytes"], heap["file_growth_bytes"]), (10 * MIB, 190 * MIB))
        self.assertIsNone(heap["anon_slope_bytes_per_hour_last_window"])  # first vs last only
        for host in HOSTS[1:]:
            self.assertEqual(report["heap_growth"][host]["status"], "unavailable")
        self.assertEqual(report["memory_cycles"][HOSTS[0]], {"count": 1, "metric": "rss_anon"})
        self.assertEqual(report["memory_cycles"][HOSTS[1]], {"count": None, "metric": None})
        self.assertEqual(report["resources"][HOSTS[1]]["rss"], 500 * MIB)

    def test_opening_zero_is_stale_and_final_accounting_explicitly_unverified(self):
        report = self.early_hold()
        self.assertEqual(report["accounting"], self.opening)
        evidence = report["accounting_status"]
        self.assertEqual(evidence["snapshot_kind"], "opening")
        self.assertEqual(evidence["freshness"], "stale")
        self.assertEqual(evidence["final_reconciliation"], "unverified")
        self.assertEqual(evidence["unreconciled_writes"], 2)
        self.assertEqual(evidence["reserved_inputs"], 2)
        self.assertIn("retained input reservations", evidence["reasons"])
        self.assertIn("snapshot predates journal activity", evidence["reasons"])
        self.assertEqual(self.c.j.get("accounting"), self.opening)

    def test_terminal_refresh_does_not_cache_an_older_reconciliation_count(self):
        self.early_hold()
        self.c.j.transition("rehearsal-00203", "unknown", submitted=1700)
        with patch("discovery.time.time", return_value=1801):
            report = self.c.report()
        self.assertEqual(report["rehearsal"]["submitted"], 203)
        self.assertEqual(report["rehearsal"]["state_counts"]["unknown"], 1)

    def test_passed_historical_rehearsal_preserves_result_and_current_accounting(self):
        for index in range(540):
            self.row(index, "reconciled")
        self.c.j.set(
            "rehearsal_result", {"status": "passed", "submitted_per_minute": 30}
        )
        self.c.j.set("status", "rehearsal_complete")
        self.c.j.set("accounting", {**self.opening, "at": 2400, "signed_fees": 5400})
        with patch("discovery.time.time", return_value=2500):
            report = self.c.report()
        self.assertEqual(report["rehearsal"]["status"], "passed")
        self.assertEqual(report["rehearsal"]["submitted_per_minute"], 30)
        self.assertEqual(report["rehearsal"]["not_yet_offered"], 0)
        self.assertEqual(report["qualification_status"], "passed")
        self.assertEqual(
            report["accounting_status"]["final_reconciliation"], "verified"
        )

    def test_missing_or_undated_accounting_cannot_be_verified_on_legacy_report(self):
        self.c.__class__ = Controller
        self.c.plan = {}
        self.c.j.set("status", "incomplete")
        for snapshot in (None, {"difference": 0}, {**self.opening, "at": 3000}):
            self.c.j.set("accounting", snapshot)
            with patch("reporting.time.time", return_value=2500):
                report = self.c.report()
            self.assertEqual(report["accounting"], snapshot)
            self.assertEqual(
                report["accounting_status"]["final_reconciliation"], "unverified"
            )

    def test_fresh_zero_with_retained_expired_signature_is_not_final(self):
        self.row(0, "expired")
        self.c.j.set("status", "incomplete")
        self.c.j.set("accounting", {**self.opening, "at": 2400})
        with patch("discovery.time.time", return_value=2500):
            report = self.c.report()
        self.assertEqual(report["accounting_status"]["freshness"], "current")
        self.assertEqual(
            report["accounting_status"]["final_reconciliation"], "unverified"
        )
        self.assertEqual(report["accounting_status"]["unreconciled_writes"], 1)

    def test_opening_zero_before_any_write_does_not_become_final_on_hold(self):
        with patch("discovery.time.time", return_value=1800):
            self.c.halt("inventory readiness failed")
            report = self.c.report()
        self.assertEqual(report["rehearsal"]["not_yet_offered"], 540)
        self.assertEqual(
            report["accounting_status"]["final_reconciliation"], "unverified"
        )
        self.assertIn(
            "opening snapshot is not final accounting",
            report["accounting_status"]["reasons"],
        )

    def test_recent_timestamp_cannot_hide_missing_reconciled_fee_charges(self):
        self.row(0, "reconciled")
        self.c.j.set("status", "incomplete")
        self.c.j.set("accounting", {**self.opening, "at": 2400})
        with patch("discovery.time.time", return_value=2500):
            report = self.c.report()
        self.assertEqual(report["accounting_status"]["freshness"], "stale")
        self.assertEqual(
            report["accounting_status"]["final_reconciliation"], "unverified"
        )
        self.assertIn(
            "snapshot does not account for reconciled signed fees",
            report["accounting_status"]["reasons"],
        )

    def test_campaign_missing_offer_field_keeps_its_original_denominator(self):
        self.c.plan["execution_mode"] = "campaign"
        self.c.events = [None] * 7
        self.c.j.offer("campaign-one", "campaign", 1000, 0, 1, 1)
        self.c.j.set("status", "held")
        with patch("discovery.time.time", return_value=2500):
            report = self.c.report()
        self.assertEqual(report["nominal_campaign_offers"], 7)
        self.assertEqual(report["not_yet_offered"], 6)
        self.assertEqual(report["rehearsal"]["not_yet_offered"], 540)

    def sign_across_deadline(self, eligible_at, sign_seconds):
        """Real Controller.prepare/submit_prepared and Journal gates; only the
        native signer boundary (with its durable file) and RPC are faked."""
        self.row(0, "reconciled")
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.offer("rehearsal-00412", "rehearsal", 1002, 1, 0, 1)
        (self.c.state / "artifacts").mkdir()
        self.c.gate = Mock()
        self.c.sync = AsyncMock(return_value=100)
        self.c.spendable = Mock(return_value=[{"id": "owned-input"}])
        self.c.blocks, self.c.spent = [], []
        self.c.wallets = [{"key": "not-read", "address": "a"}, {"address": "b"}]
        self.c.report = Mock()
        now = [eligible_at]
        self.c.j.clock = lambda: now[0]
        payload = b"native publication before journal preparation"
        signed = {
            "hash": "ab" * 32,
            "fee": 100_000_000,
            "baseline_fee": 100_000_000,
            "fee_multiplier": 1,
            "bytes": len(payload),
            "input_count": 1,
            "outputs": [],
            "key_images": ["image"],
            "selected": [{"id": "owned-input"}],
        }

        async def native(request):
            if request["operation"] == "inspect":
                return dict(signed)
            Path(request["artifact"]).write_bytes(payload)
            now[0] += sign_seconds
            return {"artifact": request["artifact"], **signed}

        async def rpc(host, method, params=None, **_):
            if method == "fee_getRate":
                return {"baseRate": 1}
            if method == "estimateFee":
                return settling_quote(signed["fee"])
            if method == "tx_submit":
                return {"txHash": signed["hash"]}
            raise AssertionError(method)

        self.c.native = AsyncMock(side_effect=native)
        self.c.rpc = Mock(call=AsyncMock(side_effect=rpc))
        with patch("controller.time.time", side_effect=lambda: now[0]):
            result = asyncio.run(self.c.prepare("rehearsal-00411", 1, 8))
        return result, payload

    def test_sign_started_inside_slot_commits_and_submits_after_deadline(self):
        # #1635, the x1 endurance2 case: eligible at +118.3 s of a 120 s grace,
        # signed in ~4.8 s. Here eligible at deadline-1 s, signing 5 s.
        self.assertEqual(
            self.c.intent_deadline({"kind": "rehearsal", "offered": 1000}), 1120
        )
        result, _ = self.sign_across_deadline(1119, 5)
        self.assertTrue(result)
        self.assertEqual(self.c.j.get("status"), "setup")
        row = self.c.j.intent("rehearsal-00411")
        self.assertEqual(row["state"], "accepted")
        self.assertEqual(row["prepared"], 1124)
        self.assertEqual(row["submitted"], 1124)
        self.assertEqual(json.loads(row["info"])["eligible_at"], 1119)
        self.assertFalse((self.c.state / "artifacts" / "expired").exists())

    def test_runaway_signing_past_cap_skips_and_quarantines_without_hold(self):
        result, payload = self.sign_across_deadline(1001, 121)
        self.assertFalse(result)
        self.assertEqual(self.c.j.get("status"), "setup")
        row = self.c.j.intent("rehearsal-00411")
        self.assertEqual(row["state"], "skipped")
        self.assertEqual(row["reason"], "signing exceeded its allowance")
        self.assertIsNone(row["prepared"])
        self.assertIsNone(row["submitted"])
        self.assertIsNone(row["hash"])
        self.assertEqual(
            self.c.j.db.execute("SELECT COUNT(*) FROM reservations").fetchone()[0], 0
        )
        directory = self.c.state / "artifacts"
        self.assertFalse((directory / "rehearsal-00411.bin").exists())
        self.assertEqual(
            (directory / "expired/rehearsal-00411.bin").read_bytes(), payload
        )
        self.assertEqual(quarantine_reasons(self.c.j, "rehearsal-00411"), ["signing_timeout"])
        del self.c.report
        with patch("controller.time.time", return_value=1122):
            report = self.c.report()
        evidence = report["artifact_evidence"]
        self.assertEqual(evidence["status"], "checked")
        self.assertEqual(evidence["findings"], [])
        self.assertEqual(evidence["orphaned_signed"], 0)
        self.assertEqual(evidence["ambiguous_signed"], 0)
        self.assertEqual(evidence["expired_unsubmitted"], 1)
        rehearsal = report["rehearsal"]
        self.assertEqual(rehearsal["submitted"], 1)
        self.assertEqual(rehearsal["signed_unsubmitted"], 0)
        self.assertEqual(rehearsal["orphaned_signed_artifacts"], 0)
        self.assertEqual(rehearsal["ambiguous_signed_artifacts"], 0)

    def test_other_prepare_gates_still_hold_with_artifact_retained(self):
        # Only expiry is non-holding: a consumed intent is still a Gate.
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.transition("rehearsal-00411", "skipped", "test")
        with self.assertRaisesRegex(Gate, "intent already consumed"):
            self.c.j.prepare("rehearsal-00411", {}, 8, 10**12)

    def test_restart_quarantines_artifact_of_never_prepared_eligible_intent(self):
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.transition("rehearsal-00411", "eligible")
        directory = self.c.state / "artifacts"
        directory.mkdir()
        (directory / "rehearsal-00411.bin").write_bytes(b"signed")
        (directory / "rehearsal-00411.partial").write_bytes(b"partial")
        self.c.j.set("status", "setup")
        self.c.monitor = Mock()
        # Stop run() right after its startup artifact check.
        with patch("controller.asyncio.create_task", side_effect=StopRun):
            with self.assertRaises(StopRun):
                asyncio.run(Controller.run(self.c))
        self.assertEqual(self.c.j.get("status"), "setup")
        row = self.c.j.intent("rehearsal-00411")
        self.assertEqual(row["state"], "eligible")
        self.assertEqual(sorted(p.name for p in directory.iterdir()), ["expired"])
        self.assertEqual(
            sorted(p.name for p in (directory / "expired").iterdir()),
            ["rehearsal-00411.bin", "rehearsal-00411.partial"],
        )
        self.assertEqual(quarantine_reasons(self.c.j, "rehearsal-00411"), ["unprepared_restart"])
        evidence = self.c.report()["artifact_evidence"]
        self.assertEqual(evidence["orphaned_signed"], 0)
        self.assertEqual(evidence["ambiguous_signed"], 0)
        self.assertEqual(evidence["expired_unsubmitted"], 2)

    def test_restart_still_holds_on_real_orphan_beside_quarantine(self):
        directory = self.c.state / "artifacts"
        (directory / "expired").mkdir(parents=True)
        (directory / "expired/rehearsal-00001.bin").write_bytes(b"quarantined")
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.transition("rehearsal-00411", "submitting", prepared=1001)
        (directory / "rehearsal-00411.partial").write_bytes(b"partial")
        (directory / "no-journal-row.bin").write_bytes(b"unknown")
        self.c.j.set("status", "setup")
        self.c.monitor = Mock()
        with patch("controller.asyncio.create_task", side_effect=StopRun):
            with self.assertRaises(StopRun):
                asyncio.run(Controller.run(self.c))
        self.assertEqual(self.c.j.get("status"), "held")
        self.assertIn("orphaned or partial", self.c.j.get("reason"))
        self.assertTrue((directory / "rehearsal-00411.partial").exists())
        self.assertTrue((directory / "no-journal-row.bin").exists())
        evidence = self.c.report()["artifact_evidence"]
        self.assertEqual(evidence["orphaned_signed"], 1)
        self.assertEqual(evidence["ambiguous_signed"], 1)
        self.assertEqual(evidence["expired_unsubmitted"], 1)

    def test_retry_quarantines_stale_artifact_of_eligible_intent_then_signs(self):
        # Controller.prepare's own pre-sign check: an eligible, never-prepared
        # intent's leftover artifact is quarantined, not a hold.
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.transition("rehearsal-00411", "eligible")
        directory = self.c.state / "artifacts"
        directory.mkdir()
        (directory / "rehearsal-00411.bin").write_bytes(b"stale")
        self.c.j.set("status", "setup")
        self.c.gate = Mock()
        self.c.sync = AsyncMock(return_value=100)
        self.c.spendable = Mock(return_value=[{"id": "owned-input"}])
        self.c.blocks, self.c.spent = [], []
        self.c.wallets = [{"key": "not-read", "address": "a"}, {"address": "b"}]
        async def rpc(host, method, params=None, **_):
            if method == "estimateFee":
                return settling_quote(100_000_000)
            return {"baseRate": 1}

        self.c.rpc = Mock(call=AsyncMock(side_effect=rpc))
        self.c.submit_prepared = AsyncMock()
        self.c.j.clock = lambda: 1001

        async def sign(request):
            Path(request["artifact"]).write_bytes(b"fresh")
            return {"artifact": request["artifact"], "hash": "ab" * 32,
                    "fee": 100_000_000, "baseline_fee": 100_000_000,
                    "fee_multiplier": 1, "bytes": 5, "input_count": 1,
                    "selected": [{"id": "owned-input"}]}

        self.c.native = AsyncMock(side_effect=sign)
        with patch("controller.time.time", return_value=1001):
            self.assertTrue(asyncio.run(self.c.prepare("rehearsal-00411", 1, 8)))
        self.c.submit_prepared.assert_awaited_once()
        self.assertEqual(self.c.j.intent("rehearsal-00411")["state"], "prepared")
        self.assertEqual((directory / "rehearsal-00411.bin").read_bytes(), b"fresh")
        self.assertEqual(
            (directory / "expired/rehearsal-00411.bin").read_bytes(), b"stale"
        )
        self.assertEqual(quarantine_reasons(self.c.j, "rehearsal-00411"), ["unprepared_retry"])

    def test_prepare_resigns_an_underpaying_draft_at_the_node_quote(self):
        """#2001: the real Controller.prepare quotes each signed draft with
        ``estimateFee`` on the intent's ingress; an underpaying draft is
        quarantined (never submitted) and re-signed with ``min_fee``."""
        directory = self.c.state / "artifacts"
        directory.mkdir()
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.set("status", "setup")
        self.c.gate = Mock()
        self.c.sync = AsyncMock(return_value=100)
        self.c.spendable = Mock(return_value=[{"id": "owned-input"}])
        self.c.blocks, self.c.spent = [], []
        self.c.wallets = [{"key": "not-read", "address": "a"}, {"address": "b"}]
        self.c.submit_prepared = AsyncMock()
        self.c.j.clock = lambda: 1001
        floor = 2_000_000_074
        resign = floor + 2 * (floor // 64)
        requests, quoted = [], []

        async def sign(request):
            requests.append(dict(request))
            fee = max(100_000_000, request.get("min_fee", 0))
            Path(request["artifact"]).write_bytes(b"%d" % fee)
            return {"artifact": request["artifact"], "hash": "%064x" % fee,
                    "fee": fee, "baseline_fee": fee, "fee_multiplier": 1,
                    "bytes": 5, "input_count": 1, "selected": [{"id": "owned-input"}]}

        async def rpc(host, method, params=None, **_):
            if method == "estimateFee":
                fee = int(bytes.fromhex(params["tx_hex"]))
                quoted.append((host, fee))
                return {"minimumFee": floor, "consensusFeeFloor": floor,
                        "relayFeeFloor": floor, "settleFee": floor + floor // 64,
                        "resignFee": resign, "txFee": fee}
            return {"baseRate": 1}

        self.c.native = AsyncMock(side_effect=sign)
        self.c.rpc = Mock(call=AsyncMock(side_effect=rpc))
        with patch("controller.time.time", return_value=1001):
            self.assertTrue(asyncio.run(self.c.prepare("rehearsal-00411", 1, 8)))
        self.assertNotIn("min_fee", requests[0])
        self.assertEqual(requests[1]["min_fee"], resign)
        ingress = self.c.choose_ingress("rehearsal-00411")
        self.assertEqual(quoted, [(ingress, 100_000_000), (ingress, resign)])
        row = self.c.j.intent("rehearsal-00411")
        self.assertEqual(row["state"], "prepared")
        self.assertEqual(row["fee"], resign)
        info = json.loads(row["info"])
        self.assertEqual([d["fee"] for d in info["underpaid_drafts"]], [100_000_000])
        self.assertEqual(info["fee_floor_quote"]["resignFee"], resign)
        self.assertEqual((directory / "rehearsal-00411.bin").read_bytes(), b"%d" % resign)
        self.assertEqual((directory / "expired/rehearsal-00411.bin").read_bytes(), b"100000000")
        # #2001 follow-up 4: evidence tells an underpaid draft from a timeout.
        self.assertEqual(quarantine_reasons(self.c.j, "rehearsal-00411"), ["underpaid_draft"])
        self.c.submit_prepared.assert_awaited_once()

    def test_prepare_skips_when_fee_pricing_runs_past_the_signing_allowance(self):
        """#2001 follow-ups 4 and 6: a pricing loop that reaches the commit
        deadline stops before re-signing; the intent is skipped (not held),
        nothing is reserved or submitted, and the quarantine events tell the
        underpaid draft apart from the signing timeout."""
        directory = self.c.state / "artifacts"
        directory.mkdir()
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.set("status", "setup")
        self.c.gate = Mock()
        self.c.sync = AsyncMock(return_value=100)
        self.c.spendable = Mock(return_value=[{"id": "owned-input"}])
        self.c.blocks, self.c.spent = [], []
        self.c.wallets = [{"key": "not-read", "address": "a"}, {"address": "b"}]
        self.c.submit_prepared = AsyncMock()
        self.c.j.clock = lambda: 1001
        floor = 2_000_000_074
        now = [1001.0]
        signed = []

        async def sign(request):
            fee = max(100_000_000, request.get("min_fee", 0))
            signed.append(fee)
            Path(request["artifact"]).write_bytes(b"%d" % fee)
            return {"artifact": request["artifact"], "hash": "%064x" % fee,
                    "fee": fee, "baseline_fee": fee, "fee_multiplier": 1,
                    "bytes": 5, "input_count": 1, "selected": [{"id": "owned-input"}]}

        async def rpc(host, method, params=None, **_):
            if method == "estimateFee":
                now[0] += 10_000  # far past any commit deadline
                fee = int(bytes.fromhex(params["tx_hex"]))
                return {"minimumFee": floor, "consensusFeeFloor": floor,
                        "relayFeeFloor": floor, "settleFee": floor + floor // 64,
                        "resignFee": floor + 2 * (floor // 64), "txFee": fee}
            return {"baseRate": 1}

        self.c.native = AsyncMock(side_effect=sign)
        self.c.rpc = Mock(call=AsyncMock(side_effect=rpc))
        with patch("controller.time.time", side_effect=lambda: now[0]):
            self.assertFalse(asyncio.run(self.c.prepare("rehearsal-00411", 1, 8)))
        self.assertEqual(signed, [100_000_000])
        row = self.c.j.intent("rehearsal-00411")
        self.assertEqual(row["state"], "skipped")
        self.assertIn("allowance", row["reason"])
        self.assertIsNone(row["prepared"])
        self.c.submit_prepared.assert_not_awaited()
        self.assertEqual(self.c.j.get("status"), "setup")
        self.assertEqual(quarantine_reasons(self.c.j, "rehearsal-00411"), ["underpaid_draft"])
        self.assertEqual(
            sorted(p.name for p in (directory / "expired").iterdir()), ["rehearsal-00411.bin"])

    def test_unavailable_artifact_directory_preserves_historical_journal_counts(self):
        report = self.early_hold()
        self.assertEqual(report["artifact_evidence"]["status"], "unavailable")
        self.assertIsNone(report["rehearsal"]["orphaned_signed_artifacts"])
        self.assertIsNone(report["rehearsal"]["ambiguous_signed_artifacts"])
        self.assertEqual(report["rehearsal"]["unsigned_basis"], "journal_only")
        self.assertEqual(report["rehearsal"]["unsigned"], 20)

    def test_partial_unmatched_and_symlink_artifacts_are_ambiguous_without_replay(self):
        self.row(0, "planned")
        directory = self.c.state / "artifacts"
        directory.mkdir()
        (directory / "rehearsal-00000.partial").write_bytes(b"partial")
        (directory / "no-journal-row.bin").write_bytes(b"published")
        (directory / "unsafe.bin").symlink_to(self.c.state / "journal.sqlite")
        (self.c.state / "readiness-probe.bin").write_bytes(b"outside scope")
        self.c.j.set("status", "held")
        before = list(self.c.j.db.iterdump())
        report = self.c.report()
        self.assertEqual(list(self.c.j.db.iterdump()), before)
        evidence = report["artifact_evidence"]
        self.assertEqual(evidence["orphaned_signed"], 1)
        self.assertEqual(evidence["ambiguous_signed"], 2)
        self.assertEqual(len(evidence["findings"]), 3)
        unsafe = next(
            x for x in evidence["findings"] if x["path"].endswith("unsafe.bin")
        )
        self.assertIsNone(unsafe["sha256"])
        self.assertEqual(report["rehearsal"]["unsigned"], 0)
        self.assertEqual(report["rehearsal"]["ambiguous_signed_artifacts"], 1)

    def test_completed_report_keeps_journal_status_and_disclaims_fee_coverage(self):
        self.c.j.set("status", "complete")
        before = list(self.c.j.db.iterdump())
        report = self.c.report()
        self.assertEqual(list(self.c.j.db.iterdump()), before)
        self.assertEqual(report["status"], "incomplete")
        self.assertEqual(self.c.j.get("status"), "complete")

    def test_checked_empty_and_journaled_artifacts_do_not_create_orphans(self):
        directory = self.c.state / "artifacts"
        directory.mkdir()
        self.c.j.set("status", "held")
        self.assertEqual(self.c.report()["artifact_evidence"]["orphaned_signed"], 0)
        self.row(0, "prepared")
        self.c.j.transition("rehearsal-00000", "prepared", hash="ab" * 32)
        (directory / "rehearsal-00000.bin").write_bytes(b"journaled")
        report = self.c.report()
        self.assertEqual(report["artifact_evidence"]["findings"], [])
        self.assertEqual(report["rehearsal"]["signed_unsubmitted"], 1)
        self.assertEqual(report["rehearsal"]["orphaned_signed_artifacts"], 0)

    def test_unreadable_directory_and_oversized_artifact_are_not_empty_evidence(self):
        directory = self.c.state / "artifacts"
        directory.mkdir()
        self.c.j.set("status", "held")
        with patch.object(Path, "iterdir", side_effect=PermissionError("unreadable")):
            evidence = self.c.report()["artifact_evidence"]
        self.assertEqual(evidence["status"], "unavailable")
        self.assertIsNone(evidence["orphaned_signed"])
        (directory / "oversized.bin").write_bytes(
            b"x" * (self.c.j.limits["max_signed_transaction_bytes"] + 1)
        )
        evidence = self.c.report()["artifact_evidence"]
        self.assertEqual(evidence["ambiguous_signed"], 1)
        self.assertIsNone(evidence["findings"][0]["sha256"])
