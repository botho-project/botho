"""Collector failure/units checks; never launches a prover or network call."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import measure_ownership as m


class CollectorTests(unittest.TestCase):
    def test_platform_rss_units_and_missing_data(self):
        self.assertEqual(m.rss_bytes('  1024 maximum resident set size\n', 'Darwin'), 1024)
        self.assertEqual(m.rss_bytes(' Maximum resident set size (kbytes): 1024\n', 'Linux'), 1048576)
        for text in ['', 'Maximum resident set size (kbytes): 0']:
            with self.assertRaises(ValueError):
                m.rss_bytes(text, 'Linux')

    def test_complete_case_required_and_identity_checked(self):
        raw = ('RESOURCE_SETUP case=zero capacity=32768 setup_ns=1\n'
               'RESOURCE_FIXTURE case=zero fixture_ns=2\n'
               'RESOURCE_SAMPLE case=zero sample=0 inputs=1 outputs=1 ring=20 significant_bits=2 '
               'multipliers=1612 constraints=3248 arithmetic_bytes=1121 clsag_field_bytes=736 '
               'signatures=1 prove_ns=4 verify_ns=5\n'
               'test result: ok. 1 passed; 0 failed; 0 ignored;\n')
        self.assertEqual(len(m.records(raw, 'zero', 1)['samples']), 1)
        for changed in [raw.replace('case=zero', 'case=ordinary-1'),
                        raw.replace('signatures=1', 'signatures=2'),
                        raw.replace('sample=0', 'sample=1'),
                        raw.replace('constraints=3248', 'constraints=262145'),
                        raw.replace('1 passed', '0 passed'),
                        raw + raw]:
            with self.assertRaises(ValueError):
                m.records(changed, 'zero', 1)

    def test_cargo_executable_identity_not_stale_directory_scan(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / 'test-executable'
            binary.touch()
            messages = Path(temp) / 'cargo.jsonl'
            record = dict(reason='compiler-artifact', target=dict(name='bth_ct_demurrage_reference'),
                          profile=dict(test=True), executable=str(binary))
            messages.write_text(json.dumps(record) + '\n')
            self.assertEqual(m.executable(messages), binary.resolve())
            record['target']['name'] = 'unrelated'
            messages.write_text(json.dumps(record) + '\n')
            with self.assertRaises(ValueError):
                m.executable(messages)

    def test_timeout_kills_process_group_and_waits(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(m.subprocess, 'Popen') as popen, patch.object(m.os, 'killpg') as kill:
            process = popen.return_value
            process.pid = 12345
            process.returncode = -9
            process.wait.side_effect = [subprocess.TimeoutExpired('synthetic', 1), -9]
            self.assertEqual(m.execute(['synthetic'], {}, Path(temp) / 'log', 1), (-9, True))
            kill.assert_called_once_with(12345, m.signal.SIGKILL)
            self.assertEqual(process.wait.call_count, 2)
            self.assertTrue(popen.call_args.kwargs['start_new_session'])


if __name__ == '__main__':
    unittest.main()
