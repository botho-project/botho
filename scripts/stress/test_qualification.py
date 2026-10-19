"""Qualification is terminal and cannot arm a campaign, including after restart."""

import asyncio
import json
from pathlib import Path
import unittest
from unittest.mock import AsyncMock, Mock, patch

from plan import expand
from runtime import Gate, HOSTS
import test_discovery
import test_rehearsal
import test_staging


class QualificationTests(unittest.TestCase):
    def plan(self):
        plan = test_discovery.DiscoveryTests().profile()
        plan.update(execution_mode="rehearsal_only", setup_deadline_hours=1)
        return plan

    def test_no_campaign_events_or_budget_in_qualification(self):
        summary, events = expand(self.plan())
        self.assertEqual(events, [])
        self.assertEqual(summary["nominal_campaign_payments"], 0)
        self.assertEqual(summary["total_planned_chain_write_ceiling"], 540)

    def test_invalid_mode_or_duration_rejected(self):
        for mode, hours in [
            (True, 1),
            (None, 1),
            ("typo", 1),
            ("campaign", 1),
            ("rehearsal_only", 8),
        ]:
            plan = self.plan()
            plan.update(execution_mode=mode, setup_deadline_hours=hours)
            with self.assertRaises(ValueError):
                expand(plan)
        legacy = json.loads(
            Path(__file__).with_name("testnet-72h-plan.json").read_text()
        )
        legacy["execution_mode"] = "rehearsal_only"
        with self.assertRaises(ValueError):
            expand(legacy)

    def controller(self, cadence=2):
        fixture = test_discovery.DiscoveryExecutorTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        c = fixture.c
        c.plan = self.plan()
        del c.report
        c.config.update(execution_mode="rehearsal_only", infrastructure_end=6400)
        c.j.set("start", None)
        c.j.set("end", None)
        c.j.set("status", "setup")
        c.j.set("setup_start", 1000)
        c.j.set("opening_verified", True)
        c.j.set("rehearsal_start", 1000)
        c.j.set("fee_latest", {host: {"at": 2500} for host in HOSTS})
        for i, row in enumerate(test_rehearsal.RehearsalCadenceTests().rows(cadence)):
            identifier = f"rehearsal-{i:05d}"
            c.j.offer(identifier, "rehearsal", row["offered"], 0, 1, 1)
            c.j.transition(
                identifier,
                row["state"],
                submitted=row["submitted"],
                finished=row["finished"],
            )
        c.sync = AsyncMock(return_value=100)
        c.gate = Mock()
        c.accounting = Mock()
        c.inventory_ready = Mock(return_value=True)
        return c

    def test_pass_is_terminal_without_t0_on_tick_or_restart(self):
        c = self.controller()
        with patch("discovery.time.time", return_value=2500):
            asyncio.run(c.setup_tick())
            self.assertEqual(c.j.get("status"), "rehearsal_complete")
            c.accounting.assert_called_once()
            c.inventory_ready.assert_called_once()
            self.assertEqual(c.report()["workload_delivery"], "not_requested")
            self.assertEqual(c.report()["qualification_status"], "passed")
            c.monitor = AsyncMock()
            c.deliver = AsyncMock()
            asyncio.run(c.run())
            with self.assertRaises(Gate):
                asyncio.run(c.setup_tick())
            with self.assertRaises(Gate):
                asyncio.run(c.campaign_tick())
            c.monitor.assert_not_called()
            c.deliver.assert_not_called()
        self.assertIsNone(c.j.get("start"))
        self.assertIsNone(c.j.get("end"))
        self.assertFalse((c.state / "activated.json").exists())
        self.assertFalse(c.j.rows("kind='campaign'"))

    def test_run_loop_keeps_qualification_terminal_status(self):
        from controller import Controller

        c = self.controller()
        (c.state / "artifacts").mkdir()
        c.fresh = 2500
        c.closed = False
        c.admission_lock = asyncio.Lock()
        c.monitor = AsyncMock()
        c.reconcile = AsyncMock()
        with patch("discovery.time.time", return_value=2500):
            asyncio.run(Controller.run(c))
        self.assertEqual(c.j.get("status"), "rehearsal_complete")
        self.assertTrue(c.closed)
        self.assertIsNone(c.j.get("start"))
        self.assertFalse(c.j.rows("kind='campaign'"))

    def test_terminal_failure_never_resumes_on_restart(self):
        c = self.controller()
        c.j.set("status", "incomplete")
        c.j.set("reason", "missed offers")
        c.monitor = AsyncMock()
        asyncio.run(c.run())
        self.assertEqual(c.j.get("status"), "incomplete")
        self.assertEqual(c.report()["qualification_status"], "failed")
        c.monitor.assert_not_called()

    def test_slow_rehearsal_and_postchecks_cannot_succeed(self):
        for failure in (
            "cadence",
            "inventory",
            "accounting",
            "fees",
            "concurrent_hold",
        ):
            c = self.controller(2.2 if failure == "cadence" else 2)
            if failure == "inventory":
                c.inventory_ready.return_value = False
            if failure == "accounting":
                c.accounting.side_effect = Gate("accounting mismatch")
            if failure == "fees":
                c.j.set("fee_latest", {})
            if failure == "concurrent_hold":
                c.gate.side_effect = Gate("concurrent safety hold")
            with patch("discovery.time.time", return_value=2500):
                with self.assertRaises(Gate):
                    asyncio.run(c.setup_tick())
            self.assertNotEqual(c.j.get("status"), "rehearsal_complete")
            self.assertIsNone(c.j.get("start"))
            self.assertFalse((c.state / "activated.json").exists())

    def test_staging_bounds_short_lifetime_and_pins_mode(self):
        fixture = test_staging.StagingTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        p = fixture.p
        plan = self.plan()
        plan["wallets"]["count"] = 8
        plan["limits"]["max_inflight"] = 8
        for phase in plan["phases"]:
            phase["max_inflight"] = 8
        plan["endpoints"] = [r["rpc_url"] for r in p["targets"]]
        Path(p["plan_file"]).write_text(json.dumps(plan))
        p.update(
            execution_mode="rehearsal_only",
            isolated_discovery=True,
            opening_balances=[10**12] * 8,
            end=1000 + 5400,
        )
        import staging

        bundles = staging.prepare(p)
        launch = json.loads(bundles[0][1]["state/launch.json"][0])
        for item, _, observer in bundles:
            self.assertIn("RuntimeMaxSec=5400", staging.unit_text(item, observer))
        self.assertEqual(launch["execution_mode"], "rehearsal_only")
        for end in (1000 + 5399, 1000 + 7201, 1000 + 81 * 3600, float("inf")):
            p["end"] = end
            with self.assertRaises(Gate):
                staging.prepare(p)
        p["end"] = 6400
        p["execution_mode"] = "campaign"
        with self.assertRaises(Gate):
            staging.prepare(p)

    def test_real_constructor_restart_preserves_mode_and_rejects_manifest_change(self):
        from discovery import DiscoveryController
        from test_targets import targets
        import hashlib
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            plan = self.plan()
            rows = targets()
            plan["endpoints"] = [r["rpc_url"] for r in rows]
            signer = state / "signer"
            signer.write_bytes(b"unused signer")
            known = state / "known_hosts"
            known.write_bytes(b"pinned host keys")
            config = dict(
                execution_mode="rehearsal_only",
                isolated_discovery=True,
                opening_balances=[10**12] * 32,
                setup_start=1000,
                infrastructure_end=6400,
                targets=rows,
                run_id="qualification-test",
                signer=str(signer),
                signer_sha256=hashlib.sha256(signer.read_bytes()).hexdigest(),
                known_hosts=str(known),
                known_hosts_sha256=hashlib.sha256(known.read_bytes()).hexdigest(),
                controller_files={},
                wallets=[dict(address=str(i), key="unused") for i in range(32)],
            )
            (state / "plan.json").write_text(json.dumps(plan))
            (state / "launch.json").write_text(json.dumps(config))
            original = DiscoveryController(state)
            original.j.set("status", "rehearsal_complete")
            original.j.db.close()
            restarted = DiscoveryController(state)
            try:
                restarted.monitor = AsyncMock()
                asyncio.run(restarted.run())
                restarted.monitor.assert_not_called()
                self.assertEqual(restarted.j.get("status"), "rehearsal_complete")
                self.assertIsNone(restarted.j.get("start"))
            finally:
                restarted.j.db.close()
            # Both documents made internally valid cannot change the stored mode.
            plan.update(execution_mode="campaign", setup_deadline_hours=8)
            config.update(
                execution_mode="campaign", infrastructure_end=1000 + 81 * 3600
            )
            (state / "plan.json").write_text(json.dumps(plan))
            (state / "launch.json").write_text(json.dumps(config))
            from runtime import Journal

            opened = []

            def capture_journal(*args, **kwargs):
                journal = Journal(*args, **kwargs)
                opened.append(journal)
                return journal

            try:
                with patch("controller.Journal", side_effect=capture_journal):
                    with self.assertRaisesRegex(Gate, "manifest changed"):
                        DiscoveryController(state)
            finally:
                for journal in opened:
                    journal.db.close()
