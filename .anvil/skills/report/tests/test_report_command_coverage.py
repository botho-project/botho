"""Doc-coverage tests for the ``anvil:report`` subject voice tier (issue #613).

These are **substring-assertion** tests over the shipped command files —
the same pattern as ``anvil/skills/essay/tests/test_essay_skeleton.py``
(the PR #604 pilot). They read the command markdown as text and pin the
subject-voice-tier wiring the #613 curation locked:

- ``report-draft.md`` step 8c invokes ``resolve_subject_voice_docs`` and
  records ``metadata.subject_voice_exemplars`` (per-subject transcript map),
  composable with the existing step 8b author voice grounding.
- ``report-review.md`` step 4e resolves the tier; the per-subject pass
  extends the EXISTING dim 8 (Tone & audience calibration) sub-step; the
  ``subject_voice_grounding`` ``_summary.md`` block and the conditional
  Misattribution critical flag (``≥2 subjects``) are documented.
- The rubric stamps stay ``anvil-report-v2`` / 44 / 39 — the flag is
  **additive**, not a rubric-total change.
- ``report-revise.md`` step 6 gains a subject-tier preservation one-liner
  resolving through ``resolve_subject_voice_docs``.
- The byte-identical-when-absent contract is documented in every file.

The module filename is deliberately distinct
(``test_report_command_coverage``) per the #58 packaging convention so it
never collides with another skill's ``test_*`` module under pytest's
default import mode. The tests read files by path only — no cross-module
imports — so no ``__init__.py`` is required (matching the existing
``report/tests`` layout).

Runs under ``pytest anvil/skills/report/tests/`` or
``python -m unittest discover anvil/skills/report/tests/``.
"""

from __future__ import annotations

import unittest
from pathlib import Path

_SKILL_ROOT = Path(__file__).resolve().parent.parent

RUBRIC_ID = "anvil-report-v2"


def _read(rel: str) -> str:
    return (_SKILL_ROOT / rel).read_text(encoding="utf-8")


class TestReportDraftSubjectTier(unittest.TestCase):
    """report-draft.md step 8c: drafter contract (AC6)."""

    def setUp(self):
        self.text = _read("commands/report-draft.md")

    def test_step_8c_present(self):
        self.assertIn("8c.", self.text)

    def test_invokes_resolver(self):
        self.assertIn("resolve_subject_voice_docs", self.text)
        self.assertIn('voice_grounding.md', self.text)
        self.assertIn('"Subject voice tier"', self.text)

    def test_records_per_subject_exemplar_map(self):
        self.assertIn("subject_voice_exemplars", self.text)
        self.assertIn('{"<name>": ["<transcript path>"', self.text)

    def test_composable_with_author_tier(self):
        # Step 8b (author) and step 8c (subject) activate independently.
        self.assertIn("activates independently", self.text)
        self.assertIn("composable with it", self.text)

    def test_byte_identical_when_absent(self):
        self.assertIn("no `subjects` list", self.text)
        self.assertIn("Byte-identical to pre-#613", self.text)


class TestReportReviewSubjectTier(unittest.TestCase):
    """report-review.md steps 4e / 5 / 6 / 9 (AC7–AC10)."""

    def setUp(self):
        self.text = _read("commands/report-review.md")

    def test_step_4e_resolves_and_caches(self):
        self.assertIn("4e.", self.text)
        self.assertIn("resolve_subject_voice_docs", self.text)
        self.assertIn("subject_voice_docs_resolved", self.text)

    def test_dim_8_sub_pass_extension(self):
        # Report folds the per-subject pass into dim 8 (Tone & audience
        # calibration) — where the author voice grounding already lives.
        self.assertIn("Tone & audience calibration", self.text)
        self.assertIn(
            "subject voice tier active — <N> subject(s) scored against "
            "transcript corpora",
            self.text,
        )
        self.assertIn("MUST quote the transcript", self.text)
        self.assertIn("convergence-with-Claude", self.text)

    def test_misattribution_flag_conditional_on_two_subjects(self):
        self.assertIn("Misattribution", self.text)
        self.assertIn("≥2 subjects", self.text)
        self.assertIn("voice-identity failure", self.text)
        self.assertIn("cannot fire", self.text)

    def test_summary_block_name_and_shape(self):
        self.assertIn("subject_voice_grounding", self.text)
        self.assertIn("corpus_files_loaded", self.text)
        self.assertIn("voice_doc_loaded", self.text)
        self.assertIn("exemplars_quoted", self.text)
        self.assertIn("lines_flagged", self.text)
        self.assertIn("NOT emitted at all", self.text)
        # Both blocks emit independently when both tiers active.
        self.assertIn("emits BOTH blocks", self.text)

    def test_rubric_stamps_unchanged(self):
        self.assertIn(f'rubric_id: "{RUBRIC_ID}"', self.text)
        self.assertIn("rubric_total: 44", self.text)
        self.assertIn("advance_threshold: 39", self.text)
        self.assertIn("does NOT change the rubric total", self.text)


class TestReportReviseSubjectTier(unittest.TestCase):
    """report-revise.md step 6: subject one-liner preservation (AC11)."""

    def setUp(self):
        self.text = _read("commands/report-revise.md")

    def test_resolves_subject_voice_docs(self):
        self.assertIn("resolve_subject_voice_docs", self.text)

    def test_preservation_one_liner(self):
        self.assertIn("preserve the subject voice signatures", self.text)
        # A raised Misattribution flag cannot be declined.
        self.assertIn("Misattribution", self.text)
        self.assertIn("never `declined`", self.text)

    def test_byte_identical_when_absent(self):
        self.assertIn("byte-identical to pre-#613", self.text)


if __name__ == "__main__":
    unittest.main()
