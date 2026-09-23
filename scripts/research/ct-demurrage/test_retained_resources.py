import json
from pathlib import Path
import shutil
import tempfile
import unittest

from check_retained_resources import HERE, check_cases, check_linux


class RetainedEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'evidence'
        shutil.copytree(HERE / 'evidence/ownership-resources-linux', self.path)

    def rewrite_summary(self, change):
        file = self.path / 'summary.json'
        summary = json.loads(file.read_text())
        change(summary)
        file.write_text(json.dumps(summary))

    def test_complete_retained_evidence(self):
        check_linux(self.path)

    def test_missing_case_is_not_partial_success(self):
        self.rewrite_summary(lambda s: s['cases'].pop())
        with self.assertRaisesRegex(ValueError, 'case coverage'):
            check_cases(self.path)

    def test_failed_case_remains_failure(self):
        self.rewrite_summary(lambda s: s['cases'][1].update(exit_code=1))
        with self.assertRaisesRegex(ValueError, 'failed case'):
            check_cases(self.path)

    def test_linux_kib_cannot_be_labelled_bytes(self):
        self.rewrite_summary(lambda s: s['cases'][1].update(
            whole_case_peak_rss_bytes=s['cases'][1]['whole_case_peak_rss_bytes'] // 1024))
        with self.assertRaisesRegex(ValueError, 'RSS unit conversion'):
            check_cases(self.path)

    def test_summary_cannot_replace_raw_observation(self):
        self.rewrite_summary(lambda s: s['cases'][1]['samples'][0].update(prove_ns=1))
        with self.assertRaisesRegex(ValueError, 'summary/raw disagreement'):
            check_cases(self.path)

    def test_source_archive_damage_rejected(self):
        with (self.path / 'sources.tar.gz').open('ab') as file:
            file.write(b'changed')
        with self.assertRaisesRegex(ValueError, 'retained file hash'):
            check_linux(self.path)


if __name__ == '__main__':
    unittest.main()
