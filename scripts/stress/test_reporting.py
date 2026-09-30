"""Early safety holds must retain the full denominator and accounting limits."""

import asyncio
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from discovery import DiscoveryController
from runtime import Gate, Journal


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

    def test_published_native_artifact_survives_expired_prepare_and_report_is_read_only(
        self,
    ):
        self.row(0, "reconciled")
        self.c.j.offer("rehearsal-00411", "rehearsal", 1000, 0, 1, 1)
        self.c.j.offer("rehearsal-00412", "rehearsal", 1002, 1, 0, 1)
        (self.c.state / "artifacts").mkdir()
        self.c.gate = Mock()
        self.c.sync = AsyncMock(return_value=100)
        self.c.spendable = Mock(return_value=[{"id": "owned-input"}])
        self.c.blocks, self.c.spent = [], []
        self.c.wallets = [{"key": "not-read", "address": "a"}, {"address": "b"}]
        self.c.rpc = Mock(call=AsyncMock(return_value={"baseRate": 1}))
        self.c.submit_prepared = AsyncMock()
        now = [1001]
        self.c.j.clock = lambda: now[0]
        payload = b"native publication before journal preparation"

        async def publish_before_deadline(request):
            # Emulate only the native subprocess boundary, including its durable
            # file side effect. Real Controller.prepare and Journal.prepare run.
            Path(request["artifact"]).write_bytes(payload)
            now[0] = 1120
            return {
                "artifact": request["artifact"],
                "hash": "ab" * 32,
                "fee": 100_000_000,
                "baseline_fee": 100_000_000,
                "fee_multiplier": 1,
                "bytes": len(payload),
                "input_count": 1,
                "selected": [{"id": "owned-input"}],
            }

        self.c.native = AsyncMock(side_effect=publish_before_deadline)
        with patch("controller.time.time", side_effect=lambda: now[0]):
            with self.assertRaisesRegex(Gate, "preparation expired"):
                asyncio.run(self.c.prepare("rehearsal-00411", 1, 8))
            self.c.halt("preparation expired or intent already consumed")
            before = list(self.c.j.db.iterdump())
            report = self.c.report()
        self.c.submit_prepared.assert_not_awaited()
        self.assertEqual(list(self.c.j.db.iterdump()), before)
        row = self.c.j.intent("rehearsal-00411")
        self.assertEqual(row["state"], "eligible")
        self.assertIsNone(row["prepared"])
        self.assertIsNone(row["submitted"])
        self.assertIsNone(row["hash"])
        rehearsal = report["rehearsal"]
        self.assertEqual(rehearsal["submitted"], 1)
        self.assertEqual(rehearsal["signed_unsubmitted"], 0)
        self.assertEqual(rehearsal["orphaned_signed_artifacts"], 1)
        self.assertEqual(rehearsal["ambiguous_signed_artifacts"], 0)
        self.assertEqual(rehearsal["unsigned"], 1)
        self.assertEqual(report["qualification_status"], "failed")
        artifact = report["artifact_evidence"]["findings"][0]
        self.assertEqual(
            artifact["path"], str(self.c.state / "artifacts/rehearsal-00411.bin")
        )
        self.assertEqual(artifact["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertEqual(artifact["bytes"], len(payload))
        self.assertIsNone(artifact["journal_transaction_hash"])
        self.assertIn(
            "artifact/journal disagreement", report["accounting_status"]["reasons"]
        )

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
