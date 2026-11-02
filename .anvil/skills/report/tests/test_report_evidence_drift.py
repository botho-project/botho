"""Doc-coverage tests for the ``anvil:report`` evidence-drift advisory (issue #857).

These are **substring-assertion** tests over the shipped command files —
the same pattern as ``anvil/skills/paper/tests/test_paper_evidence_drift.py``.
They read the command markdown as text and pin the evidence-drift wiring
the #857 curation locked:

- ``report-draft.md`` step 10b and ``report-revise.md`` step 10b invoke
  ``anvil.lib.evidence_drift record`` to snapshot ``BRIEF.md``/``refs/**``
  mtimes into ``metadata.evidence_snapshot`` at draft/revise completion.
- ``report-review.md`` documents the snapshot as an ``## Inputs`` entry,
  step 4g invokes ``anvil.lib.evidence_drift check``, and step 9 adds an
  advisory "Evidence drift" note to ``verdict.md`` — explicitly NOT
  gating ``advance``, score, or the ``AUDITED`` terminal-state transition.
- ``report/SKILL.md`` documents the mechanism.
- The bootstrap case (no recorded snapshot) is documented as "not
  drifted", never a false positive.

The module filename is deliberately distinct
(``test_report_evidence_drift``) per the #58 packaging convention so it
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

from anvil.lib.testing import read_text

_SKILL_ROOT = Path(__file__).resolve().parent.parent

_read = lambda rel: read_text(_SKILL_ROOT / rel)


class TestReportDraftRecordsSnapshot(unittest.TestCase):
    """report-draft.md step 10b records the evidence-drift baseline."""

    def setUp(self):
        self.text = _read("commands/report-draft.md")

    def test_step_invokes_evidence_drift_record(self):
        self.assertIn("anvil.lib.evidence_drift record", self.text)
        self.assertIn("evidence-drift baseline", self.text)

    def test_progress_json_snippet_documents_evidence_snapshot(self):
        self.assertIn("evidence_snapshot", self.text)


class TestReportReviseRecordsSnapshot(unittest.TestCase):
    """report-revise.md step 10b re-baselines the evidence-drift snapshot."""

    def setUp(self):
        self.text = _read("commands/report-revise.md")

    def test_step_invokes_evidence_drift_record(self):
        self.assertIn("anvil.lib.evidence_drift record", self.text)
        self.assertIn("evidence-drift baseline", self.text)

    def test_progress_json_snippet_documents_evidence_snapshot(self):
        self.assertIn("evidence_snapshot", self.text)


class TestReportReviewChecksDrift(unittest.TestCase):
    """report-review.md step 4g checks + verdict.md surfaces drift."""

    def setUp(self):
        self.text = _read("commands/report-review.md")

    def test_inputs_documents_evidence_snapshot(self):
        self.assertIn("Evidence-drift snapshot", self.text)
        self.assertIn("metadata.evidence_snapshot", self.text)

    def test_step_invokes_evidence_drift_check(self):
        self.assertIn("anvil.lib.evidence_drift check", self.text)

    def test_check_is_never_routed_through_critical_flag_machinery(self):
        idx = self.text.index("Check BRIEF/refs evidence drift")
        window = self.text[idx : idx + 1200]
        self.assertIn("NEVER routed through", window)
        self.assertIn("CriticalFlag", window)

    def test_verdict_note_is_advisory_only(self):
        idx = self.text.index("Evidence drift (conditional")
        window = self.text[idx : idx + 900]
        self.assertIn("advisory only", window)
        self.assertIn("does NOT change `advance`", window)
        self.assertIn("does NOT gate the `AUDITED` transition", window)

    def test_bootstrap_case_documented(self):
        self.assertIn("NO-SNAPSHOT", self.text)


class TestSkillDocumentsEvidenceDrift(unittest.TestCase):
    """report/SKILL.md documents the mechanism."""

    def setUp(self):
        self.skill = _read("SKILL.md")

    def test_skill_names_evidence_drift(self):
        self.assertIn("EVIDENCE-DRIFT", self.skill)
        self.assertIn("issue #857", self.skill)

    def test_skill_documents_advisory_posture(self):
        self.assertIn("purely advisory", self.skill.lower().replace("**", ""))
