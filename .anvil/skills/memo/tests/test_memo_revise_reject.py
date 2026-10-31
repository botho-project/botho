"""Doc-coverage tests for the ``memo-revise --reject "<reason>"``
operator-rejection entry point (issue #754).

`--reject` is the lifecycle verb for an operator overruling the scoring
system: a version scored `advance:true` + 0-critical (READY by the
rubric) that the operator rejects for a reason the rubric cannot see
(comprehension, a thesis reframe, new evidence flipping a decision). It
bypasses the same step-4 verdict pre-check that `--polish` bypasses, but
grants the FULL default-path revision-plan contract rather than the
polish subset, and ranks the operator's reject reason as the
highest-priority finding.

Like the `--polish` / `--plan` / `--override-no-go` flags before it,
`--reject` is reviewer-prose-only — there is no Python detector module.
Following the precedent set by ``test_memo_revise_plan.py`` and
``test_memo_no_go_terminal.py``, this module:

1. Asserts on documented surface in ``commands/memo-revise.md``,
   ``commands/memo-review.md``, ``SKILL.md``, and
   ``templates/plan.md.template`` — substring presence and structural
   contract. The tests do NOT execute the reviser; the reviser is
   LLM-driven, so behavioural assertions belong in consumer-side
   integration tests.
2. Exercises the documented ``_progress.json`` audit-trail field shapes
   (``metadata.revision_mode = "operator_reject"`` +
   ``metadata.reject_reason``) as additive optional JSON that pre-#754
   readers tolerate via the shallow-merge contract.

Per the per-skill test filename convention (#58 — distinct filenames
across skills, ``__init__.py`` chains in every test dir), this file is
named ``test_memo_revise_reject.py``.

Runs under either ``python -m unittest discover anvil/skills/memo/tests/``
or ``pytest anvil/skills/memo/tests/``.
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from anvil.lib.testing import read_text as _read

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent
_REVISE_MD = _SKILL_ROOT / "commands" / "memo-revise.md"
_REVIEW_MD = _SKILL_ROOT / "commands" / "memo-review.md"
_SKILL_MD = _SKILL_ROOT / "SKILL.md"
_PLAN_TEMPLATE = _SKILL_ROOT / "templates" / "plan.md.template"


# ---------------------------------------------------------------------------
# 1. The flag is documented with its own CLI-flags subsection.
# ---------------------------------------------------------------------------


class TestRejectFlagDocumented(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_reject_flag_subsection_present(self) -> None:
        self.assertIn(
            '### `--reject "<reason>"`',
            self.revise,
            "memo-revise.md MUST add a `### --reject \"<reason>\"` CLI "
            "flags subsection (issue #754)",
        )

    def test_reject_names_the_advancing_precondition(self) -> None:
        # The flag targets an `advance:true` + 0-critical version — the
        # same precondition `--polish` bypasses.
        self.assertIn(
            "advance:true",
            self.revise,
            "memo-revise.md MUST name the `advance:true` precondition the "
            "reject bypass targets (issue #754)",
        )
        self.assertIn(
            "0-critical",
            self.revise,
            "memo-revise.md MUST name the 0-critical precondition the "
            "reject bypass targets (issue #754)",
        )


# ---------------------------------------------------------------------------
# 2. Bypasses the step-4 refusal, exactly like --polish.
# ---------------------------------------------------------------------------


class TestRejectBypassesVerdictPreCheck(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_step_4_carries_a_reject_bypass_note(self) -> None:
        # The procedural body (step 4) MUST carry the `--reject` bypass
        # inline, mirroring the `--polish` bypass note.
        self.assertIn(
            "`--reject` bypass",
            self.revise,
            "memo-revise.md step 4 MUST carry an inline `--reject` bypass "
            "note (issue #754)",
        )

    def test_reject_bypasses_step_4_only(self) -> None:
        lowered = self.revise.lower()
        # Iteration cap (step 3) and review-exists (step 1) still apply.
        self.assertIn(
            "iteration-cap check) still applies",
            self.revise,
            "memo-revise.md MUST state the iteration-cap check still "
            "applies under `--reject` (issue #754)",
        )
        self.assertIn(
            "review-exists check) still applies",
            self.revise,
            "memo-revise.md MUST state the review-exists check still "
            "applies under `--reject` (issue #754)",
        )
        self.assertIn("single-pass", lowered)


# ---------------------------------------------------------------------------
# 3. Full revision-plan contract, NOT the polish subset.
# ---------------------------------------------------------------------------


class TestRejectGrantsFullRevisionContract(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_full_contract_not_polish_subset(self) -> None:
        self.assertIn(
            "full revision-plan contract",
            self.revise.replace("FULL", "full"),
            "memo-revise.md MUST state `--reject` grants the FULL "
            "revision-plan contract (issue #754)",
        )

    def test_may_restructure_substantively(self) -> None:
        lowered = self.revise.lower()
        self.assertIn(
            "restructure substantively",
            lowered,
            "memo-revise.md MUST state `--reject` may restructure "
            "substantively (new skeleton, reordered argument) (issue "
            "#754)",
        )

    def test_consumes_directives_and_all_critic_siblings(self) -> None:
        # Unlike --polish, --reject consumes ALL critic siblings + the
        # _directives file + the BRIEF.
        self.assertIn(
            "_directives/v{N+1}.md",
            self.revise,
            "memo-revise.md MUST state `--reject` consumes the "
            "`_directives/v{N+1}.md` directive (issue #754)",
        )


# ---------------------------------------------------------------------------
# 4. Reason required + preserved verbatim; audit-trail metadata.
# ---------------------------------------------------------------------------


class TestRejectReasonContract(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_reason_required_rejection_shape(self) -> None:
        # Empty / whitespace-only / missing reason is rejected — same
        # shape as --polish / --override-no-go.
        self.assertIn(
            '`--reject` without a value, `--reject ""`, and '
            '`--reject "   "`',
            self.revise,
            "memo-revise.md MUST document the required-non-empty-reason "
            "rejection for `--reject` (issue #754)",
        )

    def test_metadata_fields_documented(self) -> None:
        self.assertIn(
            'metadata.revision_mode = "operator_reject"',
            self.revise,
            "memo-revise.md MUST document "
            "`metadata.revision_mode = \"operator_reject\"` (issue #754)",
        )
        self.assertIn(
            "metadata.reject_reason",
            self.revise,
            "memo-revise.md MUST document `metadata.reject_reason` "
            "(issue #754)",
        )

    def test_reason_stored_verbatim(self) -> None:
        # The verbatim-storage discipline (no trim/normalize/truncate)
        # MUST be explicit, matching the revise_force_reason precedent.
        self.assertIn(
            "stored verbatim, no trimming / normalization / truncation",
            self.revise,
            "memo-revise.md MUST state `reject_reason` is stored "
            "verbatim (issue #754)",
        )

    def test_audit_trail_only_no_state_machine_impact(self) -> None:
        lowered = self.revise.lower()
        self.assertIn("audit-trail-only", lowered)
        self.assertIn(
            "not scored",
            lowered.replace("no state-machine impact at the reviser", ""),
            "memo-revise.md MUST frame the reject fields as audit-trail "
            "only (issue #754)",
        )


# ---------------------------------------------------------------------------
# 5. Reject reason is the highest-priority finding (step 7).
# ---------------------------------------------------------------------------


class TestRejectReasonHighestPriority(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_step_7_ranks_reject_reason_above_every_critic_finding(
        self,
    ) -> None:
        self.assertIn(
            "ranked above every critic finding",
            self.revise,
            "memo-revise.md step 7 MUST rank the reject reason above "
            "every critic finding (issue #754 proposal item 3)",
        )

    def test_reject_reason_not_scope_filtered(self) -> None:
        lowered = self.revise.lower()
        self.assertIn(
            "reject reason is not subject to the `--scope` filter".lower(),
            lowered,
            "memo-revise.md MUST state the reject reason is not subject "
            "to the `--scope` filter (issue #754)",
        )


# ---------------------------------------------------------------------------
# 6. Reject-pass changelog header note (step 9).
# ---------------------------------------------------------------------------


class TestRejectChangelogHeaderNote(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_reject_pass_header_note_documented(self) -> None:
        self.assertIn(
            "Reject-pass header note",
            self.revise,
            "memo-revise.md MUST document the reject-pass changelog "
            "header note (issue #754)",
        )

    def test_header_note_names_operator_reject_revision_mode(self) -> None:
        self.assertIn(
            "revision_mode: operator_reject",
            self.revise,
            "memo-revise.md reject-pass header note MUST name "
            "`revision_mode: operator_reject` (issue #754)",
        )

    def test_reject_and_polish_header_notes_mutually_exclusive(self) -> None:
        lowered = self.revise.lower()
        self.assertIn(
            "reject-pass header note and the polish-pass header note are "
            "mutually exclusive",
            lowered,
            "memo-revise.md MUST state the reject-pass and polish-pass "
            "header notes are mutually exclusive (issue #754)",
        )


# ---------------------------------------------------------------------------
# 7. Mutual exclusion with --polish and --override-no-go; composition
#    with --scope / --plan / --apply.
# ---------------------------------------------------------------------------


class TestRejectComposition(unittest.TestCase):
    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_mutually_exclusive_with_polish_and_override_no_go(self) -> None:
        # Proposal item 5 — reject is mutually exclusive with BOTH.
        self.assertIn(
            "Mutually exclusive with BOTH",
            self.revise.replace("mutually exclusive", "Mutually exclusive"),
            "memo-revise.md MUST state `--reject` is mutually exclusive "
            "with both `--polish` and `--override-no-go` (issue #754 "
            "proposal item 5)",
        )
        # Both flag names appear in the reject subsection's composition
        # paragraph.
        self.assertIn("`--reject` + `--polish`", self.revise)
        self.assertIn("`--reject` + `--override-no-go`", self.revise)

    def test_scope_plan_apply_compose_normally(self) -> None:
        self.assertIn(
            "Composition with `--scope` / `--plan` / `--apply`",
            self.revise,
            "memo-revise.md MUST document reject composition with "
            "`--scope` / `--plan` / `--apply` (issue #754 proposal item "
            "5)",
        )

    def test_operator_reject_plan_then_apply_value(self) -> None:
        self.assertIn(
            "operator_reject_plan_then_apply",
            self.revise,
            "memo-revise.md MUST document the "
            "`operator_reject_plan_then_apply` revision_mode value for "
            "the reject + plan/apply composition (issue #754)",
        )

    def test_plan_dispatch_validates_mutual_exclusion(self) -> None:
        # Step 0a pre-flight MUST reject the contradictory combinations.
        self.assertIn(
            "Reject `--reject` + `--polish` and `--reject` + "
            "`--override-no-go` as mutually exclusive",
            self.revise,
            "memo-revise.md step 0a MUST validate reject mutual "
            "exclusion at plan-dispatch time (issue #754)",
        )

    def test_apply_reads_reject_reason_from_plan_header(self) -> None:
        # Operator does NOT re-pass --reject on --apply; the plan is the
        # audit trail.
        self.assertIn(
            "reads the reason from the plan header so the operator does "
            "NOT re-pass `--reject",
            self.revise,
            "memo-revise.md MUST state `--apply` reads the reject reason "
            "from the plan header, not re-passed on CLI (issue #754)",
        )


# ---------------------------------------------------------------------------
# 8. memo-review emits the operator-rejection findings-header note (9c).
# ---------------------------------------------------------------------------


class TestReviewOperatorRejectionNote(unittest.TestCase):
    def setUp(self) -> None:
        self.review = _read(_REVIEW_MD)

    def test_step_9c_present(self) -> None:
        self.assertIn(
            "9c.",
            self.review,
            "memo-review.md MUST add step 9c for the operator-rejection "
            "findings-header note (issue #754)",
        )

    def test_operator_rejection_subsection_documented(self) -> None:
        self.assertIn(
            "## Operator rejection",
            self.review,
            "memo-review.md step 9c MUST document the `## Operator "
            "rejection` findings.md subsection (issue #754)",
        )

    def test_reads_revision_mode_operator_reject(self) -> None:
        self.assertIn(
            "revision_mode",
            self.review,
            "memo-review.md step 9c MUST read "
            "`metadata.revision_mode` to detect an operator reject "
            "(issue #754)",
        )
        self.assertIn(
            '"operator_reject"',
            self.review,
            "memo-review.md step 9c MUST match "
            "`revision_mode == \"operator_reject\"` (issue #754)",
        )

    def test_note_has_no_score_impact(self) -> None:
        lowered = self.review.lower()
        self.assertIn(
            "observational prose with no score impact",
            lowered,
            "memo-review.md step 9c MUST frame the operator-rejection "
            "note as observational, no score impact (issue #754 — #346 "
            "precedent)",
        )

    def test_cites_346_precedent(self) -> None:
        self.assertIn(
            "#346",
            self.review[self.review.index("## Operator rejection") - 2000 :],
            "memo-review.md step 9c SHOULD cite the #346 "
            "rubric-version-transition precedent (issue #754 proposal "
            "item 4)",
        )


# ---------------------------------------------------------------------------
# 9. SKILL.md surface + plan template surface.
# ---------------------------------------------------------------------------


class TestRejectSkillAndTemplateSurface(unittest.TestCase):
    def setUp(self) -> None:
        self.skill = _read(_SKILL_MD)
        self.template = _read(_PLAN_TEMPLATE)

    def test_skill_section_present(self) -> None:
        self.assertIn(
            "Operator rejection of an advancing version",
            self.skill,
            "SKILL.md MUST add the §`Operator rejection of an advancing "
            "version` section (issue #754)",
        )

    def test_skill_command_table_lists_reject(self) -> None:
        self.assertIn(
            '--reject "<reason>"',
            self.skill,
            "SKILL.md command dispatch table MUST list the `--reject "
            "\"<reason>\"` flag on `memo-revise` (issue #754)",
        )

    def test_skill_states_mutual_exclusion(self) -> None:
        lowered = self.skill.lower()
        self.assertIn(
            "mutually exclusive",
            lowered,
            "SKILL.md MUST state `--reject` is mutually exclusive with "
            "`--polish` and `--override-no-go` (issue #754)",
        )

    def test_template_revision_mode_includes_operator_reject(self) -> None:
        self.assertIn(
            "operator_reject",
            self.template,
            "plan.md.template MUST document `operator_reject` as a "
            "`Revision mode` value (issue #754)",
        )


# ---------------------------------------------------------------------------
# 10. _progress.json audit-trail field shapes round-trip as additive JSON.
# ---------------------------------------------------------------------------


class TestRejectProgressJsonShapes(unittest.TestCase):
    def test_operator_reject_metadata_round_trips(self) -> None:
        reject_reason = (
            "Almost word-soup; the reader learns nothing interesting. "
            "Restate the thesis root as one corrected sentence and "
            "rebuild the skeleton around the Gate-4-flipping break-even "
            "re-run."
        )
        progress = {
            "version": 1,
            "thread": "sentinel",
            "phases": {
                "revise": {
                    "state": "done",
                    "started": "2026-07-28T14:00:00Z",
                    "completed": "2026-07-28T14:20:00Z",
                }
            },
            "metadata": {
                "iteration": 6,
                "max_iterations": 16,
                "revision_mode": "operator_reject",
                "reject_reason": reject_reason,
            },
        }
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "_progress.json"
            path.write_text(json.dumps(progress, indent=2))
            loaded = json.loads(path.read_text())

        self.assertEqual(
            loaded["metadata"]["revision_mode"], "operator_reject"
        )
        # Verbatim — no normalization / trimming / truncation.
        self.assertEqual(loaded["metadata"]["reject_reason"], reject_reason)

    def test_non_reject_thread_omits_reject_fields(self) -> None:
        # A non-reject version dir is byte-identical to the pre-#754
        # shape: the additive fields' absence is the default.
        progress = {
            "version": 1,
            "thread": "sentinel",
            "phases": {"revise": {"state": "done"}},
            "metadata": {"iteration": 2, "max_iterations": 4},
        }
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "_progress.json"
            path.write_text(json.dumps(progress, indent=2))
            loaded = json.loads(path.read_text())

        self.assertNotIn("revision_mode", loaded["metadata"])
        self.assertNotIn("reject_reason", loaded["metadata"])


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
