import json
import pathlib
import subprocess
import tempfile
import types
import unittest
from unittest.mock import patch

import host_maintenance as policy


class HostMaintenanceTests(unittest.TestCase):
    def states(self):
        return {unit: {'UnitFileState': 'masked', 'ActiveState': 'inactive'} for unit in policy.UNITS}

    def test_disabled_timer_is_insufficient_when_service_can_still_start(self):
        states = self.states()
        for unit in policy.UNITS:
            with self.subTest(unit=unit):
                changed = {k: dict(v) for k, v in states.items()}
                changed[unit]['UnitFileState'] = 'disabled'
                with self.assertRaisesRegex(RuntimeError, 'not excluded'):
                    policy.validate_units(changed)

    def test_already_running_upgrade_cannot_pass_even_when_masked(self):
        for active in ('active', 'activating', 'deactivating', 'reloading'):
            states = self.states()
            states['apt-daily-upgrade.service']['ActiveState'] = active
            with self.subTest(active=active), self.assertRaises(RuntimeError):
                policy.validate_units(states)
        policy.validate_units(self.states())

    def test_bootstrap_waits_without_killing_active_package_manager(self):
        script = policy.bootstrap('q20261005-i', 1000, 1000+82*3600)
        subprocess.run(['bash', '-n'], input=script.encode(), check=True)
        self.assertIn('systemctl mask --now apt-daily.timer apt-daily-upgrade.timer', script)
        self.assertIn('systemctl mask apt-daily.service apt-daily-upgrade.service', script)
        self.assertNotIn('mask --now apt-daily.service', script)
        self.assertNotIn('systemctl stop', script)
        self.assertIn('maintenance_wait_end', script)
        self.assertIn(policy.RESTART, script)

    def test_only_fixed_lifetime_isolated_runs_are_allowed(self):
        for run, start, end in [('production', 1000, 2000), ('q20261005-i', 1000, 1000),
                                ('q20261005-i', 1000, 1000+83*3600+1)]:
            with self.subTest(run=run, end=end), self.assertRaises(ValueError):
                policy.bootstrap(run, start, end)

    def test_x_series_run_id_renders(self):
        # Added on main for the x series (#1590); not part of the 7ff7c416 port.
        script = policy.bootstrap('q20261101-x1-endurance', 1000, 1000+82*3600)
        subprocess.run(['bash', '-n'], input=script.encode(), check=True)
        self.assertIn('"run_id": "q20261101-x1-endurance"', script)
        for run in ('Q20261101-x1-endurance', 'q20261101-X1-endurance', 'q2026110-x1-endurance'):
            with self.subTest(run=run), self.assertRaises(ValueError):
                policy.bootstrap(run, 1000, 1000+82*3600)

    def test_check_rejects_merged_policy_override_and_expired_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            files = {policy.MARKER: json.dumps({'run_id': 'q20261005-i', 'created_at': 1000, 'end': 2000}),
                     policy.APT_PATH: policy.APT, policy.RESTART_PATH: policy.RESTART}
            for name, content in files.items():
                path = root/name.lstrip('/'); path.parent.mkdir(parents=True, exist_ok=True); path.write_text(content)
            def command(args, **kwargs):
                return policy.APT if args[0] == 'apt-config' else 'UnitFileState=masked\nActiveState=inactive\n'
            info = types.SimpleNamespace(st_uid=0, st_mode=0o644)
            with patch.object(pathlib.Path, 'stat', return_value=info), patch.object(policy.subprocess, 'check_output', side_effect=command) as run:
                self.assertTrue(policy.check(root, now=1500)['verified'])
                with self.assertRaisesRegex(RuntimeError, 'lifetime expired'):
                    policy.check(root, now=2000)
                run.side_effect = lambda args, **kw: command(args, **kw).replace('Enable "0"', 'Enable "1"')
                with self.assertRaisesRegex(RuntimeError, 'Effective periodic'):
                    policy.check(root, now=1500)
                run.side_effect = command
                (root/policy.RESTART_PATH.lstrip('/')).write_text("$nrconf{restart} = 'a';\n")
                with self.assertRaisesRegex(RuntimeError, 'content changed'):
                    policy.check(root, now=1500)


if __name__ == '__main__':
    unittest.main()
