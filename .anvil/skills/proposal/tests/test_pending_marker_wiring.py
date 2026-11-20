"""Tests for the pending-marker gate wiring into ``anvil:proposal`` (issue #843).

Issue #843 is Phase 2 of the #841/#842 epic: it wires the framework-level
``anvil/lib/pending_marker.py`` primitive (shipped by #842, PR #847) into the
`proposal` skill as the reference adoption for the sibling skills (paper,
memo, report) still to come. (Paper's own adoption — issues #844/#847/#850 —
landed in parallel; this module follows the same lifecycle-test shape that
``anvil/skills/paper/tests/test_paper_pending_marker.py`` established, adapted
for proposal's two-critic — review AND audit — shape and its ``proposal.tex``
body filename.)

This module covers three classes of acceptance criteria. It deliberately
scopes its documentation-wiring assertions to the two files this PR owns —
``SKILL.md`` (the state-machine terminal-state table) and
``proposal-synthesize.md`` (the outstanding-dependencies surfacing). The
parallel doc-coverage for ``proposal-review`` / ``proposal-audit`` /
``proposal-revise`` / ``rubric.md`` landed independently via PR #851
(``tests/skills/proposal/test_proposal_pending_marker_doc.py``) and is NOT
duplicated here:

1. **Documentation wiring** (``TestDocsWiring``): the skill's SKILL.md
   READY/AUDITED terminal-state clauses require zero unresolved pending
   markers (the distinct-from-critical-flag callout mirrors paper's #850),
   and ``proposal-synthesize`` surfaces (rather than silently drops) an
   unresolved marker as an outstanding dependency.
2. **Functional wiring** (``TestBodyOverrideFunctional`` /
   ``TestGapListOutstandingDependencies``): the proposal skill's body
   filename is ``proposal.tex``, which matches neither of
   ``anvil.lib.pending_marker``'s auto-detected shapes (``<slug>.md``,
   ``main.tex``) — so the ``--body proposal.tex`` override is load-bearing,
   not optional, for this skill. And the synthesis schema's additive
   ``outstanding_dependencies`` field (added by this issue) round-trips
   correctly and stays backward-compatible (empty by default).
3. **End-to-end lifecycle** (``TestProposalPendingMarkerLifecycle``): exercises
   ``anvil/lib/pending_marker.py`` directly (no agent run) against a temp
   ``proposal.tex`` thread fixture, proving that an active marker is surfaced
   as an outstanding ``pending_dependency`` flag with no dimension penalty,
   does not force ``Verdict.BLOCK`` (the combined review+audit verdict still
   ADVANCEs), yet holds READY/AUDITED via the separate terminal gate; and
   resolving the marker lets READY become reachable.

The module filename is deliberately distinct
(``test_pending_marker_wiring``) and the package carries an ``__init__.py``
to avoid the cross-skill pytest collection collision documented in issue
#58.
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import pytest

from anvil.lib.convergence import (
    PENDING_DEPENDENCY_FLAG_TYPE,
    blocking_critical_flags,
    has_pending_dependency_flag,
)
from anvil.lib.critics import aggregate, compute_verdict
from anvil.lib.pending_marker import check_pending_markers
from anvil.lib.review_schema import Kind, Review, Score, Verdict
from anvil.lib.testing import read_text
from anvil.skills.proposal.lib.synthesis_schema import GapList


_SKILL_ROOT = Path(__file__).resolve().parent.parent

_read = lambda rel: read_text(_SKILL_ROOT / rel)


class TestDocsWiring(unittest.TestCase):
    """Every affected file documents the pending-marker contract."""

    def test_skill_md_ready_and_audited_gate_on_pending_marker(self):
        text = _read("SKILL.md")
        # Locate the READY / AUDITED table rows specifically (not just any
        # mention of "PENDING" elsewhere in the file).
        ready_row = next(
            line for line in text.splitlines() if line.startswith("| `READY`")
        )
        audited_row = next(
            line for line in text.splitlines() if line.startswith("| `AUDITED`")
        )
        self.assertIn("PENDING", ready_row)
        self.assertIn("PENDING", audited_row)

    def test_skill_md_gate_is_distinct_from_no_critical_flag(self):
        # Mirrors paper's #850 callout: the clause must explain the gate is
        # NOT implied by the critical-flag clause (pending_dependency is
        # filtered out of blocking_critical_flags).
        text = _read("SKILL.md")
        self.assertEqual(text.count("no unresolved `[PENDING <source>]` marker"), 2)
        self.assertIn("blocking_critical_flags", text)
        self.assertIn("pending_dependency", text)
        self.assertIn("never fabricate", text.lower())

    def test_synthesize_command_surfaces_outstanding_dependencies(self):
        text = _read("commands/proposal-synthesize.md")
        self.assertIn("outstanding_dependencies", text)
        self.assertIn("<thread>.{N}.pending/", text)
        # Must be explicit that pending markers are NOT treated as gaps.
        self.assertIn("not gaps", text.lower())


class TestBodyOverrideFunctional(unittest.TestCase):
    """The proposal skill's body filename requires ``--body proposal.tex``.

    ``anvil.lib.pending_marker``'s auto-detection only looks for
    ``<slug>.md`` (memo shape) or ``main.tex`` (paper shape) — the proposal
    skill's body file is ``proposal.tex`` (SKILL.md §"Body filename
    convention": slug-echo is deliberately deferred), which matches
    neither. This test asserts the override is load-bearing, not cosmetic.
    """

    def _make_version_dir(self, tmp_path: Path, body_text: str) -> Path:
        thread_dir = tmp_path / "gossamer-lan"
        version_dir = thread_dir / "gossamer-lan.1"
        version_dir.mkdir(parents=True)
        (version_dir / "proposal.tex").write_text(body_text, encoding="utf-8")
        return version_dir

    def test_auto_detection_fails_without_body_override(self):
        with tempfile.TemporaryDirectory() as td:
            version_dir = self._make_version_dir(Path(td), "No markers here.\n")
            with pytest.raises(FileNotFoundError):
                check_pending_markers(version_dir)

    def test_body_override_detects_active_marker(self):
        with tempfile.TemporaryDirectory() as td:
            version_dir = self._make_version_dir(
                Path(td),
                r"""
                \section{Bill of Materials}
                The transceiver unit price is [PENDING vendor-quote-acme]
                pending the vendor's returned quote.
                """,
            )
            result = check_pending_markers(version_dir, body=Path("proposal.tex"))
            self.assertFalse(result.passed())
            self.assertEqual(result.outstanding_sources, ["vendor-quote-acme"])
            self.assertTrue(result.body_path.endswith("proposal.tex"))

    def test_body_override_clean_body_passes(self):
        with tempfile.TemporaryDirectory() as td:
            version_dir = self._make_version_dir(
                Path(td),
                r"\section{Bill of Materials} All prices are quoted.",
            )
            result = check_pending_markers(version_dir, body=Path("proposal.tex"))
            self.assertTrue(result.passed())
            self.assertEqual(result.outstanding_sources, [])


class TestGapListOutstandingDependencies(unittest.TestCase):
    """The additive ``outstanding_dependencies`` field on ``GapList``."""

    def test_defaults_to_empty_list(self):
        gap_list = GapList(schema_version="1", for_version=1, thread="t")
        self.assertEqual(gap_list.outstanding_dependencies, [])

    def test_round_trips_through_json(self):
        gap_list = GapList(
            schema_version="1",
            for_version=2,
            thread="gossamer-lan",
            outstanding_dependencies=["vendor-quote-acme", "site-survey"],
        )
        payload = json.loads(gap_list.model_dump_json())
        self.assertEqual(
            payload["outstanding_dependencies"],
            ["vendor-quote-acme", "site-survey"],
        )
        reloaded = GapList.model_validate(payload)
        self.assertEqual(
            reloaded.outstanding_dependencies,
            ["vendor-quote-acme", "site-survey"],
        )

    def test_backward_compat_absent_field_defaults_empty(self):
        # A legacy gaps.json written before this issue omits the field
        # entirely — it must still validate cleanly.
        legacy_payload = {
            "schema_version": "1",
            "for_version": 1,
            "thread": "gossamer-lan",
            "gaps": [],
            "singletons": [],
        }
        gap_list = GapList.model_validate(legacy_payload)
        self.assertEqual(gap_list.outstanding_dependencies, [])


# ---------------------------------------------------------------------------
# End-to-end lifecycle (mirrors anvil/skills/paper/tests/test_paper_pending_marker.py,
# adapted for proposal's two-required-critic — review AND audit — shape)
# ---------------------------------------------------------------------------

# A proposal body carrying one honest, load-bearing pending measurement.
_BODY_WITH_MARKER = r"""
\section{Bill of Materials}
The 16 SFP+ LR transceivers are priced at [PENDING vendor-quote-acme] per
unit, pending the vendor's returned quote.
"""

# The same body after the pending value genuinely resolved.
_BODY_RESOLVED = r"""
\section{Bill of Materials}
The 16 SFP+ LR transceivers are priced at \$18 per unit, per
\texttt{refs/quote-acme.pdf}.
"""


def _content_review(version_dir: str, total: int = 40) -> Review:
    """A passing ``proposal-review`` content review (no critical flags, /44).

    A single aggregate dimension stands in for the full 9-dimension /44
    scoring pass — the pending-marker lifecycle does not depend on the
    per-dimension breakdown, only on the total clearing the /44 advance
    threshold and there being no ordinary critical flag. Mirrors
    ``anvil/skills/paper/tests/test_paper_pending_marker.py``'s
    ``_content_review`` helper.

    Deliberately does NOT model ``proposal-audit`` as a second scored
    ``Review`` fed through ``aggregate()``: per ``rubric.md`` §"Combined
    advance gate", the proposal skill's /44 total is review-only — audit
    contributes a separate boolean ``pass`` gate, not dimension scores. The
    lifecycle tests below model ``audit.pass`` as a plain Python bool
    alongside the aggregated review+pending verdict, matching the real
    combined-advance-gate formula rather than inventing a second scored
    dimension that ``aggregate()`` was never designed to receive from audit.
    """
    return Review(
        schema_version="1",
        kind=Kind.JUDGMENT,
        version_dir=version_dir,
        critic_id="review",
        scores=[
            Score(
                dimension="content",
                score=total,
                max=44,
                justification="Passing content review for the lifecycle fixture.",
            )
        ],
        findings=[],
        critical_flags=[],
    )


class TestProposalPendingMarkerLifecycle(unittest.TestCase):
    """draft (marker present) -> review+audit -> resolve -> READY/AUDITED."""

    def _make_thread(self, tmp: Path, body: str) -> Path:
        """Create ``<tmp>/gossamer-lan/gossamer-lan.1/proposal.tex``."""
        version_dir = tmp / "gossamer-lan" / "gossamer-lan.1"
        version_dir.mkdir(parents=True)
        (version_dir / "proposal.tex").write_text(body, encoding="utf-8")
        return version_dir

    def test_active_marker_surfaced_as_outstanding_dependency(self):
        with tempfile.TemporaryDirectory() as td:
            version_dir = self._make_thread(Path(td), _BODY_WITH_MARKER)
            result = check_pending_markers(version_dir, body=Path("proposal.tex"))

            self.assertFalse(result.passed())
            self.assertEqual(result.outstanding_sources, ["vendor-quote-acme"])
            self.assertEqual(len(result.active_markers), 1)

            flags = result.to_critical_flags()
            self.assertEqual(len(flags), 1)
            self.assertEqual(flags[0].type, PENDING_DEPENDENCY_FLAG_TYPE)

    def test_marker_incurs_no_dimension_penalty(self):
        with tempfile.TemporaryDirectory() as td:
            version_dir = self._make_thread(Path(td), _BODY_WITH_MARKER)
            result = check_pending_markers(version_dir, body=Path("proposal.tex"))
            pending_review = result.to_review(version_dir=version_dir.name)

            # The pending review's only Score is the no-dim placeholder
            # (score=None): it contributes 0 to the /44 total, so a
            # proposal scored 40/44 on review stays 40/44.
            self.assertTrue(all(s.score is None for s in pending_review.scores))

            agg = aggregate(
                [_content_review(version_dir.name, total=40), pending_review]
            )
            self.assertEqual(agg.total, 40)

    def test_pending_marker_does_not_force_block_but_holds_ready(self):
        with tempfile.TemporaryDirectory() as td:
            version_dir = self._make_thread(Path(td), _BODY_WITH_MARKER)
            result = check_pending_markers(version_dir, body=Path("proposal.tex"))
            pending_review = result.to_review(version_dir=version_dir.name)
            agg = aggregate(
                [_content_review(version_dir.name, total=40), pending_review]
            )
            audit_pass = True  # proposal-audit's independent pass/fail gate

            # The pending flag is VISIBLE in the aggregate ...
            self.assertTrue(has_pending_dependency_flag(agg.critical_flags))
            # ... but is filtered out of the BLOCK trigger, so a passing
            # proposal's review side still ADVANCEs (proposal-review step 7).
            self.assertEqual(blocking_critical_flags(agg.critical_flags), [])
            self.assertEqual(compute_verdict(agg, threshold=35), Verdict.ADVANCE)

            # The SEPARATE terminal gate holds READY/AUDITED while the
            # marker remains: review.advance == true AND audit.pass == true,
            # but has_pending_dependency_flag == true still blocks the
            # combined READY/AUDITED gate (rubric.md §"Combined advance gate").
            review_advance = compute_verdict(agg, threshold=35) == Verdict.ADVANCE
            ready = (
                review_advance
                and audit_pass
                and not has_pending_dependency_flag(agg.critical_flags)
            )
            self.assertTrue(review_advance)
            self.assertTrue(audit_pass)
            self.assertFalse(ready)

    def test_resolving_marker_allows_ready(self):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            version_dir = self._make_thread(tmp, _BODY_WITH_MARKER)

            # Resolve: the drafter replaces the marker with the real value.
            (version_dir / "proposal.tex").write_text(_BODY_RESOLVED, encoding="utf-8")

            result = check_pending_markers(version_dir, body=Path("proposal.tex"))
            self.assertTrue(result.passed())
            self.assertEqual(result.outstanding_sources, [])
            self.assertEqual(result.to_critical_flags(), [])

            pending_review = result.to_review(version_dir=version_dir.name)
            agg = aggregate(
                [_content_review(version_dir.name, total=40), pending_review]
            )
            audit_pass = True

            self.assertFalse(has_pending_dependency_flag(agg.critical_flags))
            review_advance = compute_verdict(agg, threshold=35) == Verdict.ADVANCE
            ready = (
                review_advance
                and audit_pass
                and not has_pending_dependency_flag(agg.critical_flags)
            )
            self.assertTrue(ready)


if __name__ == "__main__":
    unittest.main()
