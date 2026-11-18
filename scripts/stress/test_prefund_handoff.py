import copy
import json
from pathlib import Path
import ssl
import tarfile
import unittest
from unittest.mock import patch

import prefund_handoff as gate
import staging
import test_qualification
import test_staging
from runtime import Gate, Journal, digest


class HandoffTests(unittest.TestCase):
    def setUp(self):
        base = test_staging.StagingTests()
        base.setUp()
        self.addCleanup(base.doCleanups)
        self.root, self.profile = base.root, base.p
        self.plan = test_qualification.QualificationTests().plan()
        profile = self.profile
        self.plan["endpoints"] = [t["rpc_url"] for t in profile["targets"]]
        self.plan["wallets"]["count"] = 8
        self.plan["limits"]["max_inflight"] = 8
        for phase in self.plan["phases"]:
            phase["max_inflight"] = 8
        self.plan_raw = json.dumps(self.plan, indent=2).encode()
        Path(profile["plan_file"]).write_bytes(self.plan_raw)
        profile.update(
            execution_mode="rehearsal_only", isolated_discovery=True, end=8200
        )
        self.funding = self.root / "funding"
        self.funding.mkdir()
        self.state = self.root / "state"
        self.state.mkdir()
        ca_file = self.root / "ca.pem"
        der = ssl.create_default_context().get_ca_certs(binary_form=True)[0]
        ca_file.write_text(ssl.DER_cert_to_PEM_cert(der))
        profile.update(
            tls_ca_file=str(ca_file), tls_ca_sha256=gate.sha256(ca_file.read_bytes())
        )
        self.manifest = dict(
            schema_version=1,
            plan_file="/opt/prefund/plan.json",
            signer="/opt/prefund/signer",
            tls_ca_file="/opt/prefund/ca.pem",
            producer_role="faucet",
            funding_wallet={
                "address": "fixture-funder",
                "key": "/var/lib/prefund/funder",
            },
            plan_sha256=gate.sha256(self.plan_raw),
            targets=profile["targets"],
            signer_sha256=gate.sha256(Path(profile["signer"]).read_bytes()),
            tls_ca_sha256=profile["tls_ca_sha256"],
            wallets=profile["wallets"],
            deadline=1250,
            infrastructure_end=8200,
            state_dir=str(self.funding),
            grant_picocredits=20000000000,
            grants_per_wallet=40,
        )
        self.manifest_file = self.root / "prefund-manifest.json"
        self.manifest_file.write_text(json.dumps(self.manifest))
        self.manifest_file.chmod(0o600)
        profile["prefund_handoff"] = {
            "local_manifest": str(self.manifest_file),
            "remote_manifest": str(self.manifest_file),
        }
        self.balances = [40 * 20000000000] * 8
        self.health = dict(
            plan_sha256=self.manifest["plan_sha256"],
            completed_before_deadline=True,
            at=900,
            grants=320,
            principal=sum(self.balances),
            grant_picocredits=20000000000,
            opening_balances=self.balances,
            fees=12,
            accounting={
                "difference": 0,
                "opening": sum(self.balances),
                "coinbase_lottery_inflow": 22,
                "remaining": 10,
                "principal": sum(self.balances),
                "fees": 12,
            },
        )

    def prepare(self):
        bundles = staging.prepare(self.profile)
        files = bundles[0][1]
        self.assertNotIn("state/launch.json", files)
        for name, (raw, _) in files.items():
            if name in ("state/plan.json", "state/prefund-handoff.json"):
                (self.state / Path(name).name).write_bytes(raw)
        return json.loads(files["state/prefund-handoff.json"][0])

    def fund(self):
        (self.funding / "funding-health.json").write_text(json.dumps(self.health))
        (self.funding / "opening_balances.json").write_text(json.dumps(self.balances))

    def run_gate(self, pending, now):
        with patch.object(gate.time, "time", return_value=now):
            gate.handoff(self.state, pending)

    def test_stage_before_funding_and_open_once(self):
        pending = self.prepare()
        with self.assertRaisesRegex(Gate, "has not started"):
            self.run_gate(pending, 999)
        self.assertFalse((self.state / "launch.json").exists())
        self.fund()
        self.run_gate(pending, 1000)
        output = self.state / "launch.json"
        saved = json.loads(output.read_bytes())
        self.assertEqual(saved["opening_balances"], self.balances)
        self.assertEqual(
            saved["prefund_evidence"]["manifest_sha256"],
            gate.sha256(self.manifest_file.read_bytes()),
        )
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        before = output.stat().st_ino
        with self.assertRaisesRegex(Gate, "expired"):
            self.run_gate(
                pending, 5000
            )  # publication alone cannot revive a missed setup
        journal = Journal(self.state / "journal.sqlite")
        journal.set("launch_digest", digest(saved))
        journal.db.close()
        self.run_gate(pending, 5000)
        self.assertEqual(output.stat().st_ino, before)
        with self.assertRaisesRegex(Gate, "expired"):
            self.run_gate(pending, 8200)
        output.write_text("{}")
        with self.assertRaisesRegex(Gate, "existing launch differs"):
            self.run_gate(pending, 1100)
        self.assertEqual(output.read_text(), "{}")

    def test_partial_publication_then_both_files_ready(self):
        pending = self.prepare()
        (self.funding / "opening_balances.json").touch()
        with (
            patch.object(gate.time, "time", return_value=1100),
            patch.object(gate.time, "sleep", side_effect=lambda _: self.fund()) as wait,
        ):
            gate.handoff(self.state, pending)
        wait.assert_called_once()
        self.assertTrue((self.state / "launch.json").is_file())

    def test_campaign_handoff_preserves_the_full_lifetime_and_requires_timely_funding(self):
        from prefund import latest_deadline
        self.plan.update(execution_mode="campaign", setup_deadline_hours=8)
        self.plan_raw = json.dumps(self.plan, indent=2).encode()
        Path(self.profile["plan_file"]).write_bytes(self.plan_raw)
        self.profile.update(execution_mode="campaign", end=1000 + 81 * 3600)
        self.manifest.update(plan_sha256=gate.sha256(self.plan_raw), infrastructure_end=self.profile["end"],
                             deadline=1000 + 5 * 3600)
        self.manifest_file.write_text(json.dumps(self.manifest))
        self.health.update(plan_sha256=self.manifest["plan_sha256"], at=1000 + 3 * 3600)
        pending = self.prepare()
        self.assertEqual(pending["launch"]["setup_start"], 1000)
        self.assertEqual(pending["launch"]["infrastructure_end"], self.profile["end"])
        self.assertEqual(pending["launch"]["execution_mode"], "campaign")
        with self.assertRaisesRegex(Gate, "completion record"):
            self.run_gate(pending, self.manifest["deadline"])
        self.fund()
        self.run_gate(pending, self.health["at"] + 1)
        self.assertEqual(json.loads((self.state / "launch.json").read_bytes())["opening_balances"], self.balances)
        self.manifest["deadline"] = latest_deadline(self.plan, self.profile["end"]) + 1
        self.manifest_file.write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(Gate, "prefund deadlines"):
            staging.prepare(self.profile)

    def test_missing_expired_wrong_or_unreconciled_funding(self):
        pending = self.prepare()
        for label, now, change in (
            ("missing", 1250, lambda: None),
            ("stale setup", 4600, self.fund),
            ("wrong plan", 1100, lambda: self.health.update(plan_sha256="f" * 64)),
            (
                "nonzero difference",
                1100,
                lambda: self.health["accounting"].update(difference=1),
            ),
            ("wrong amount", 1100, lambda: self.health.update(grant_picocredits=1)),
            ("future completion", 1100, lambda: self.health.update(at=1200)),
            (
                "inconsistent accounting",
                1100,
                lambda: self.health["accounting"].update(remaining=0),
            ),
            ("late completion", 1300, lambda: self.health.update(at=1250)),
            (
                "different balances",
                1100,
                lambda: self.health.update(opening_balances=[1] * 8),
            ),
        ):
            with self.subTest(label=label):
                original = copy.deepcopy(self.health)
                change()
                if label != "missing":
                    self.fund()
                with self.assertRaises(Gate):
                    self.run_gate(pending, now)
                self.assertFalse((self.state / "launch.json").exists())
                self.health = original
        self.fund()
        for path in (self.funding / "STOP", self.state / "STOP"):
            path.touch()
            with self.assertRaisesRegex(Gate, "stop marker"):
                self.run_gate(pending, 1100)
            path.unlink()
        self.manifest_file.write_text("{}")
        with self.assertRaisesRegex(Gate, "staged pin"):
            self.run_gate(pending, 1100)

    def test_manifests_budgets_and_mutually_exclusive_modes(self):
        saved = copy.deepcopy(self.manifest)
        for key, value in [
            ("wallets", list(reversed(saved["wallets"]))),
            ("plan_sha256", "0" * 64),
            ("signer_sha256", "e" * 64),
            ("infrastructure_end", 9300),
            ("grants_per_wallet", 10**9),
        ]:
            with self.subTest(key=key):
                self.manifest_file.write_text(json.dumps({**saved, key: value}))
                with self.assertRaises(Gate):
                    staging.prepare(self.profile)
        self.manifest_file.write_text(json.dumps(saved))
        self.profile["opening_balances"] = self.balances
        with self.assertRaisesRegex(Gate, "choose recorded balances"):
            staging.prepare(self.profile)

    def test_six_timers_and_one_funding_condition_in_sealed_archives(self):
        profile_path = self.root / "staging.json"
        profile_path.write_text(json.dumps(self.profile))
        with patch.object(staging.subprocess, "run") as command:
            results = staging.stage(profile_path, self.root / "staged")
        command.assert_not_called()
        self.assertEqual(len(results), 6)
        for index, result in enumerate(results):
            with tarfile.open(result["archive"]) as archive:
                meta = json.load(archive.extractfile("install.json"))
                self.assertEqual(
                    result["timer"], str(Path(result["unit"]).with_suffix(".timer"))
                )
                self.assertIn("OnCalendar=1970-01-01 00:16:40 UTC", meta["timer"])
                self.assertIn("Persistent=true", meta["timer"])
                self.assertIn("OnActiveSec=1s", meta["timer"])
                self.assertIn("Unit=" + result["unit"], meta["timer"])
                self.assertIn("(( EPOCHSECONDS >= 1000 ))", meta["unit"])
                self.assertEqual("prefund_handoff.py --state" in meta["unit"], index == 0)
                self.assertEqual(
                    "package/prefund_handoff.py" in archive.getnames(), index == 0
                )
                self.assertNotIn("state/launch.json", archive.getnames())


if __name__ == "__main__":
    unittest.main()
