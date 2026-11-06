"""Corruption controls for the new exact-relation evidence, not historical data."""

import json
from pathlib import Path
import shutil
import tempfile
import unittest

from check_exact_resources import (
    check_artifact,
    check_supplement,
    parse_arithmetic,
    validate,
)

HERE = Path(__file__).resolve().parent


class ExactEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def copy_capture(self, name):
        return Path(
            shutil.copytree(HERE / "evidence" / ("exact-9-" + name), self.root / name)
        )

    def test_complete_retained_evidence(self):
        validate(HERE)

    def test_missing_production_export_rejected(self):
        directory = self.copy_capture("macos")
        path = directory / "supplemental-sources.json"
        data = json.loads(path.read_text())
        del data["source_sha256"]["cluster-tax/src/lib.rs"]
        path.write_text(json.dumps(data))
        review = json.loads((HERE / "exact-relation-review-sources.json").read_text())
        with self.assertRaisesRegex(ValueError, "required supplemental source"):
            check_supplement(directory, review)

    def test_artifact_zip_corruption_rejected(self):
        directory = self.copy_capture("linux")
        path = directory / "workflow-artifact.zip"
        path.write_bytes(path.read_bytes() + b"corruption")
        with self.assertRaisesRegex(ValueError, "artifact digest"):
            check_artifact(directory)

    def test_zero_test_discriminator_rejected(self):
        directory = self.copy_capture("arithmetic")
        path = directory / "short-hold.stdout"
        path.write_text(path.read_text().replace("1 passed;", "0 passed;"))
        with self.assertRaisesRegex(ValueError, "measurement test did not pass"):
            parse_arithmetic(directory)


if __name__ == "__main__":
    unittest.main()
