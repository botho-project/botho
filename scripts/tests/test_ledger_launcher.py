"""Exercise platform/resource admission and exact dispatch without running Cargo."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'test-ledger.sh'


class LedgerLauncher(unittest.TestCase):
    def invoke(self, platform='Darwin', limit='10', requested=None, binary=False, extra=()):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, body in {
                'uname': 'printf "%s\\n" "$LEDGER_TEST_OS"',
                'sysctl': '[ "$LEDGER_TEST_LIMIT" != error ] || exit 1\nprintf "%s\\n" "$LEDGER_TEST_LIMIT"',
                'cargo': 'printf "%s\\n" "$@" > "$LEDGER_TEST_LOG"',
                'lib test binary': 'printf "%s\\n" "$@" > "$LEDGER_TEST_LOG"',
            }.items():
                path = root / name
                path.write_text('#!/bin/bash\n' + body + '\n')
                path.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}',
                       LEDGER_TEST_OS=platform, LEDGER_TEST_LIMIT=limit,
                       LEDGER_TEST_LOG=str(root / 'dispatch'))
            env.pop('RUST_TEST_THREADS', None)
            if requested is not None:
                env['RUST_TEST_THREADS'] = requested
            args = ['--binary', str(root / 'lib test binary')] if binary else []
            result = subprocess.run([str(SCRIPT), *args, *extra], env=env,
                                    capture_output=True, text=True, timeout=5)
            log = root / 'dispatch'
            return result, log.read_text().splitlines() if log.exists() else None

    def test_default_mac_caps_high_request_but_keeps_lower_request(self):
        for requested, expected in [(None, 4), ('28', 4), ('2', 2)]:
            result, args = self.invoke(requested=requested)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(args, ['test', '--locked', '-p', 'botho', '--lib',
                                    'ledger::', '--', '--nocapture', f'--test-threads={expected}'])

    def test_lower_kernel_limit_and_precompiled_path_with_spaces(self):
        for limit, expected in [('4', 1), ('6', 2), ('8', 3), ('100', 4)]:
            result, args = self.invoke(limit=limit, binary=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(args, ['ledger::', '--nocapture', f'--test-threads={expected}'])

    def test_invalid_or_unavailable_limits_do_not_launch(self):
        for limit in ['error', '0', '3', '-1', '010', 'abc', '1000000000']:
            result, args = self.invoke(limit=limit)
            self.assertEqual(result.returncode, 2)
            self.assertIsNone(args)

    def test_invalid_thread_requests_and_extra_options_do_not_launch(self):
        for requested in ['0', '-1', '02', 'many']:
            result, args = self.invoke(requested=requested)
            self.assertEqual(result.returncode, 2)
            self.assertIsNone(args)
        for extra in [('--skip', 'some_test'), ('--test-threads=28',), ('--binary', 'relative')]:
            result, args = self.invoke(extra=extra)
            self.assertEqual(result.returncode, 2)
            self.assertIsNone(args)

    def test_linux_does_not_read_sysctl_or_override_thread_count(self):
        for requested in [None, '28']:
            result, args = self.invoke(platform='Linux', limit='error', requested=requested)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(args, ['test', '--locked', '-p', 'botho', '--lib',
                                    'ledger::', '--', '--nocapture'])


if __name__ == '__main__':
    unittest.main()
