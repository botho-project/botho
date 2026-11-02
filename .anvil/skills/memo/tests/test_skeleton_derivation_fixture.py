"""Tests for the skeleton→section derivation back-check fixture (issue #752).

Phase A of issue #752 ships the skeleton↔section derivation leg (a third leg of
the summary-detail consistency back-check) as reviewer-prose-only — no Python
detector module. The fixture under
``tests/fixtures/skeleton_derivation/sentinel_word_soup/`` preserves the Studio
canary worked example (the sentinel memo.5 that scored 43/44 yet failed to
transmit its plan; the body argued a gateway-positioning thesis that
contradicts the skeleton's value-migration root claim) so that:

1. The expected ``summary_detail_consistency.skeleton_derivation`` sub-block
   shape is locked as a schema contract (extracted to the top level in the
   fixture for a self-contained anchor).
2. When a future Phase B issue lands an automated detector at
   ``anvil/skills/memo/lib/skeleton_derivation.py``, this fixture is the
   regression-test anchor (did the detector still catch the body delivering a
   thesis that contradicts its own skeleton root claim?).
3. A reviewer agent reading ``rubric.md`` §"Summary-detail consistency"
   §"Skeleton↔section derivation leg" has a worked example to ground the
   verdict-tag rubric against.

Because Phase A has no Python detector to invoke, the tests here are
**shape-only**: they assert that ``skeleton.md`` / ``memo.md`` are well-formed
(the value-migration root claim and the contradicting gateway framing are
present verbatim) and that ``expected_findings.json`` parses against the schema.
Phase B's detector test will extend this module with a behavioral assertion
(``detector(skeleton.md, memo.md) == expected_findings.json``) when it lands.

Runs under either ``python -m unittest discover anvil/skills/memo/tests/`` or
``pytest anvil/skills/memo/tests/`` per the issue #58 cross-skill packaging
convention.
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path


_HERE = Path(__file__).resolve().parent
_FIXTURE_DIR = (
    _HERE
    / "fixtures"
    / "skeleton_derivation"
    / "sentinel_word_soup"
)

# The verdict tags and severity vocabularies are shared verbatim with the
# summary↔detail leg — see rubric.md §"Summary-detail consistency"
# §"Skeleton↔section derivation leg (issue #752)".
_VALID_VERDICTS = {"ABSENT", "CONTRADICTED", "DIVERGENT"}
_VALID_SEVERITIES = {"critical", "important", "suggestion"}


class TestFixtureFilesPresent(unittest.TestCase):
    """The fixture directory contains the four expected files."""

    def test_skeleton_md_exists(self) -> None:
        self.assertTrue(
            (_FIXTURE_DIR / "skeleton.md").is_file(),
            "fixture must contain skeleton.md",
        )

    def test_memo_md_exists(self) -> None:
        self.assertTrue(
            (_FIXTURE_DIR / "memo.md").is_file(),
            "fixture must contain memo.md",
        )

    def test_expected_findings_json_exists(self) -> None:
        self.assertTrue(
            (_FIXTURE_DIR / "expected_findings.json").is_file(),
            "fixture must contain expected_findings.json",
        )

    def test_readme_md_exists(self) -> None:
        self.assertTrue(
            (_FIXTURE_DIR / "README.md").is_file(),
            "fixture must contain README.md",
        )


class TestSkeletonAndMemoWellFormed(unittest.TestCase):
    """The skeleton states the value-migration root claim; the body drifts to
    a contradicting gateway-positioning frame and undefined coinages.

    These assertions pin the verbatim text the canary fixture exists to
    preserve. If a future change rewords the fixture, they force a deliberate
    update rather than silent drift away from the canary anchor.
    """

    def setUp(self) -> None:
        self.skeleton = (_FIXTURE_DIR / "skeleton.md").read_text(encoding="utf-8")
        self.memo = (_FIXTURE_DIR / "memo.md").read_text(encoding="utf-8")

    def test_skeleton_root_is_value_migration(self) -> None:
        self.assertIn(
            "owning the harness beats owning the models",
            self.skeleton,
            "skeleton root claim must be the value-migration thesis verbatim",
        )

    def test_body_thesis_contradicts_root(self) -> None:
        # The body argues the opposite (competitive-positioning) frame — this
        # is the CONTRADICTED failure mode the fixture encodes.
        self.assertIn(
            "owning the agent beats owning the gateway",
            self.memo,
            "body must carry the contradicting gateway-positioning thesis",
        )

    def test_body_carries_undefined_coinage(self) -> None:
        # The DIVERGENT anchor: an undefined coinage load-bearing §Enclosure.
        self.assertIn("harness-native efficiency", self.memo)

    def test_body_has_no_recapture_section(self) -> None:
        # The ABSENT anchor: the §Recapture risk skeleton claim has no body
        # section. The skeleton names it; the body does not deliver it.
        self.assertIn("§Recapture risk", self.skeleton)
        self.assertNotIn("Recapture", self.memo)


class TestExpectedFindingsParses(unittest.TestCase):
    """``expected_findings.json`` parses and matches the documented
    ``skeleton_derivation`` sub-block shape.
    """

    def setUp(self) -> None:
        with (_FIXTURE_DIR / "expected_findings.json").open(
            "r", encoding="utf-8"
        ) as fh:
            self.payload = json.load(fh)

    def test_top_level_key_present(self) -> None:
        self.assertIn("skeleton_derivation", self.payload)

    def test_block_shape_minimum_keys(self) -> None:
        block = self.payload["skeleton_derivation"]
        for key in (
            "ran",
            "claims_enumerated",
            "findings_count",
            "findings_by_severity",
            "findings",
            "critical_flag_candidate",
        ):
            self.assertIn(
                key,
                block,
                f"skeleton_derivation block must contain key '{key}'",
            )

    def test_ran_is_true(self) -> None:
        self.assertIs(self.payload["skeleton_derivation"]["ran"], True)

    def test_findings_by_severity_uses_allowed_keys(self) -> None:
        sev = self.payload["skeleton_derivation"]["findings_by_severity"]
        for key in ("critical", "important", "suggestion"):
            self.assertIn(key, sev)
            self.assertIsInstance(sev[key], int)

    def test_findings_have_required_fields(self) -> None:
        required = {
            "claim_id",
            "claim_excerpt",
            "skeleton_location",
            "body_location",
            "verdict",
            "severity",
            "message",
            "suggested_fix",
        }
        for finding in self.payload["skeleton_derivation"]["findings"]:
            missing = required - set(finding.keys())
            self.assertFalse(
                missing,
                f"finding missing required fields: {missing}",
            )

    def test_findings_use_allowed_verdict_tags(self) -> None:
        for finding in self.payload["skeleton_derivation"]["findings"]:
            self.assertIn(finding["verdict"], _VALID_VERDICTS)

    def test_findings_use_allowed_severity_tags(self) -> None:
        for finding in self.payload["skeleton_derivation"]["findings"]:
            self.assertIn(finding["severity"], _VALID_SEVERITIES)

    def test_critical_findings_have_load_bearing_justification(self) -> None:
        for finding in self.payload["skeleton_derivation"]["findings"]:
            if finding["severity"] == "critical":
                self.assertIn(
                    "load_bearing_justification",
                    finding,
                    "critical findings must carry load_bearing_justification",
                )
                self.assertTrue(
                    finding["load_bearing_justification"].strip(),
                    "load_bearing_justification must be non-empty",
                )

    def test_critical_flag_candidate_matches_findings(self) -> None:
        # Per the rubric: critical_flag_candidate MUST equal
        # any(f.severity == "critical" for f in findings) — both the
        # always-critical CONTRADICTED and a root-claim ABSENT qualify.
        block = self.payload["skeleton_derivation"]
        expected = any(f["severity"] == "critical" for f in block["findings"])
        self.assertEqual(
            block["critical_flag_candidate"],
            expected,
            "critical_flag_candidate must match the derived predicate",
        )

    def test_findings_count_matches_findings_list(self) -> None:
        block = self.payload["skeleton_derivation"]
        self.assertEqual(block["findings_count"], len(block["findings"]))

    def test_findings_by_severity_sums_to_findings_count(self) -> None:
        block = self.payload["skeleton_derivation"]
        total = sum(block["findings_by_severity"].values())
        self.assertEqual(total, block["findings_count"])


class TestSentinelCanaryFindings(unittest.TestCase):
    """The fixture encodes the Studio canary failure mode verbatim.

    The sentinel memo.5 catch is the worked-example anchor for the derivation
    leg. These tests pin the specific shape — a CONTRADICTED / critical finding
    on the root claim — so a future change to ``expected_findings.json`` cannot
    silently drift away from the canary.
    """

    def setUp(self) -> None:
        with (_FIXTURE_DIR / "expected_findings.json").open(
            "r", encoding="utf-8"
        ) as fh:
            self.block = json.load(fh)["skeleton_derivation"]

    def test_root_claim_is_contradicted_critical(self) -> None:
        root = [
            f
            for f in self.block["findings"]
            if f["skeleton_location"] == "root claim"
        ]
        self.assertEqual(
            len(root), 1, "fixture must carry exactly one root-claim finding"
        )
        self.assertEqual(root[0]["verdict"], "CONTRADICTED")
        self.assertEqual(root[0]["severity"], "critical")

    def test_critical_flag_candidate_is_true(self) -> None:
        # The whole point of the canary: the body delivers a thesis that
        # contradicts its own skeleton root claim, so the flag MUST be set.
        self.assertIs(self.block["critical_flag_candidate"], True)

    def test_encodes_absent_and_divergent_legs(self) -> None:
        verdicts = {f["verdict"] for f in self.block["findings"]}
        self.assertIn("ABSENT", verdicts, "fixture must encode the ABSENT leg")
        self.assertIn(
            "DIVERGENT", verdicts, "fixture must encode the DIVERGENT leg"
        )


if __name__ == "__main__":
    unittest.main()
