"""Doc-coverage + fixture-shape tests for the ``memo-revise --plan`` /
``memo-revise --apply`` two-phase change-set-preview entry point (issue #243).

Phase A of issue #243 ships the plan-then-apply mode as
reviewer-prose-only (no Python detector module). Following the precedent
set by the ``--polish`` flag tests (PR #201 /
``tests/skills/memo/test_memo_revise_polish_flag.py``) and the
summary-detail consistency fixture tests (PR for issue #245), this
module:

1. Asserts on documented surface in ``commands/memo-revise.md``,
   ``SKILL.md``, and ``templates/plan.md.template`` — substring presence
   and structural ordering. The tests do NOT execute the reviser; the
   reviser is LLM-driven, so behavioural assertions belong in consumer-side
   integration tests, not here.
2. Asserts on fixture file presence + shape for the six AC10-named
   fixtures under ``tests/fixtures/memo_revise_plan/``. When a future
   Phase B issue lands an automated detector (e.g., a Python parser at
   ``anvil/skills/memo/lib/plan.py``), this module is the regression-test
   anchor.

The seven test classes map 1:1 to the seven AC10 cases:

a. ``TestPlanWritesPlanMdOnly`` — `--plan` writes plan.md and does NOT
   write memo.md (AC1).
b. ``TestApplyAgainstValidPlan`` — `--apply` against a valid plan
   produces `<thread>.{N+1}/` with `(via plan)` status annotation (AC2).
c. ``TestStalenessVerdictRejection`` — staleness rejection on
   changed-verdict mtime (AC6 case 2).
d. ``TestDeclinedItemsBecomeChangelogRows`` — declined items become
   `Resolution: declined` rows in changelog.md (AC7).
e. ``TestTargetLengthFlagFires`` — target-length flag fires when
   projected words exceed max (AC4 footer flag).
f. ``TestPolishPlanComposesWithApply`` — `--polish --plan --apply`
   produces `polish_plan_then_apply` revision_mode (AC8).
g. ``TestDefaultNoFlagPathUnchanged`` — the default no-flag path remains
   unchanged (AC3 regression guard).

Per-skill test filename convention (#58): this file is named with a
``test_memo_revise_plan`` prefix so it never collides with the
``test_revise_plan`` shape another skill might pick.

Runs under either ``python -m unittest discover anvil/skills/memo/tests/``
or ``pytest anvil/skills/memo/tests/``.
"""

from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

from anvil.lib.testing import read_text as _read

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent
_REVISE_MD = _SKILL_ROOT / "commands" / "memo-revise.md"
_SKILL_MD = _SKILL_ROOT / "SKILL.md"
_PLAN_TEMPLATE = _SKILL_ROOT / "templates" / "plan.md.template"
_FIXTURES = _HERE / "fixtures" / "memo_revise_plan"


# ---------------------------------------------------------------------------
# AC10 (a) — `--plan` writes plan.md and does NOT write memo.md
# ---------------------------------------------------------------------------


class TestPlanWritesPlanMdOnly(unittest.TestCase):
    """AC1 — `memo-revise <thread> --plan` writes a plan-only artifact at
    `<thread>.{N+1}.plan/plan.md` and exits WITHOUT producing
    `<thread>.{N+1}/memo.md`. The plan sibling is critic-sibling-shaped,
    NOT a version dir.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_plan_writes_to_dot_plan_sibling_not_version_dir(self) -> None:
        # The plan path MUST land at the critic-sibling location, not the
        # version dir. The exact path string must appear in the spec so a
        # future edit that confuses the two locations trips the test.
        self.assertIn(
            "<thread>.{N+1}.plan/plan.md",
            self.revise,
            "memo-revise.md MUST document the plan artifact location as "
            "`<thread>.{N+1}.plan/plan.md` (AC1 — critic-sibling shape, "
            "NOT a version dir)",
        )

    def test_plan_does_not_write_memo_md(self) -> None:
        # The "does NOT contain memo.md" discipline must be explicit so
        # the writer-side spec can't be misread as producing both.
        lowered = self.revise.lower()
        self.assertTrue(
            "without producing" in lowered
            or "must not contain" in lowered
            or "does not contain" in lowered
            or "no `<thread>.{n+1}/memo.md` is written" in lowered,
            "memo-revise.md MUST state that `--plan` does NOT produce "
            "`<thread>.{N+1}/memo.md` (AC1 — plan-only artifact)",
        )

    def test_plan_sibling_is_critic_sibling_shape(self) -> None:
        # The plan sibling MUST follow the critic-sibling shape with a
        # `_meta.json` declaring `scorecard_kind: planner` so the
        # existing `enumerate_siblings` discovery machinery picks it up
        # without modification.
        self.assertIn(
            "scorecard_kind",
            self.revise,
            "memo-revise.md MUST document the plan sibling's "
            "`_meta.json` `scorecard_kind` (AC5)",
        )
        self.assertIn(
            "planner",
            self.revise,
            "memo-revise.md MUST declare the plan sibling's "
            "`scorecard_kind: planner` (AC5)",
        )


# ---------------------------------------------------------------------------
# AC10 (b) — `--apply` against a valid plan produces `<thread>.{N+1}/`
# ---------------------------------------------------------------------------


class TestApplyAgainstValidPlan(unittest.TestCase):
    """AC2 — `memo-revise <thread> --apply` reads an existing plan,
    validates it, and produces `<thread>.{N+1}/memo.md` + `changelog.md`
    per the existing reviser contract. The status line is annotated
    `(via plan)` so downstream tooling sees the two-phase path was
    taken.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.fixture = _FIXTURES / "clean_plan_apply"

    def test_apply_status_line_annotation_documented(self) -> None:
        self.assertIn(
            "(via plan)",
            self.revise,
            "memo-revise.md MUST document the `(via plan)` status-line "
            "annotation for `--apply` (AC8)",
        )

    def test_apply_emits_same_status_line_shape_as_default(self) -> None:
        # The "MUST emit the same status line as the unflagged path"
        # discipline must be explicit so downstream tooling doesn't have
        # to special-case via-plan invocations.
        lowered = self.revise.lower()
        self.assertTrue(
            "revised" in lowered and "(via plan)" in lowered,
            "memo-revise.md MUST document that `--apply` emits the same "
            "`Revised <thread>.{N} → <thread>.{N+1}/` status line as the "
            "default path, with `(via plan)` annotation appended (AC2)",
        )

    def test_clean_plan_apply_fixture_present(self) -> None:
        self.assertTrue(
            (self.fixture / "plan.md").is_file(),
            "clean_plan_apply fixture MUST contain a `plan.md`",
        )
        self.assertTrue(
            (self.fixture / "expected_apply_result.json").is_file(),
            "clean_plan_apply fixture MUST contain "
            "`expected_apply_result.json`",
        )

    def test_clean_plan_apply_fixture_expected_metadata(self) -> None:
        expected = json.loads(
            (self.fixture / "expected_apply_result.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(
            expected["progress_metadata"]["revision_mode"],
            "plan_then_apply",
            "clean_plan_apply fixture MUST expect "
            "`revision_mode = plan_then_apply` (AC8)",
        )
        self.assertIsNone(
            expected["progress_metadata"]["revise_force_reason"],
            "clean_plan_apply fixture MUST expect `revise_force_reason = "
            "null` for a non-polish plan (AC8)",
        )
        self.assertEqual(
            expected["status_line_annotation"],
            "(via plan)",
            "clean_plan_apply fixture MUST expect `(via plan)` status "
            "annotation (AC8)",
        )


# ---------------------------------------------------------------------------
# AC10 (c) — staleness rejection on changed-verdict mtime
# ---------------------------------------------------------------------------


class TestStalenessVerdictRejection(unittest.TestCase):
    """AC6 — `--apply` MUST refuse a plan when the source review
    `<thread>.{N}.review/verdict.md` has changed (mtime later than
    `plan.md`'s). The rejection produces a clear error pointing at the
    remediation (re-run `--plan` to refresh).
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.fixture = _FIXTURES / "stale_verdict_rejected"

    def test_staleness_check_verdict_mtime_documented(self) -> None:
        # The verdict-mtime staleness check (plan-validity case 2) must
        # be explicit in the spec.
        lowered = self.revise.lower()
        self.assertIn(
            "mtime",
            lowered,
            "memo-revise.md MUST document the verdict-mtime staleness "
            "check (AC6 case 2)",
        )
        self.assertIn(
            "stale",
            lowered,
            "memo-revise.md MUST use the word 'stale' to describe the "
            "rejection (AC6)",
        )

    def test_all_five_rejection_cases_documented(self) -> None:
        # AC6 enumerates five rejection cases. Each must appear in the
        # spec so the apply-side parser knows what to enforce.
        lowered = self.revise.lower()
        self.assertIn(
            "no matching plan",
            lowered,
            "memo-revise.md MUST document plan-validity case 1 (no "
            "matching plan exists) (AC6)",
        )
        self.assertIn(
            "stale review",
            lowered,
            "memo-revise.md MUST document plan-validity case 2 (stale "
            "review) (AC6)",
        )
        self.assertIn(
            "new critic sibling",
            lowered,
            "memo-revise.md MUST document plan-validity case 3 (new "
            "critic sibling added) (AC6)",
        )
        self.assertIn(
            "plan_max_age_days",
            self.revise,
            "memo-revise.md MUST document plan-validity case 4 "
            "(`plan_max_age_days` config knob) (AC6)",
        )
        self.assertIn(
            "already exists",
            lowered,
            "memo-revise.md MUST document plan-validity case 5 (target "
            "version already exists) (AC6)",
        )

    def test_remediation_hint_re_run_plan(self) -> None:
        # Each rejection MUST point at remediation (re-run --plan).
        lowered = self.revise.lower()
        self.assertIn(
            "re-run",
            lowered,
            "memo-revise.md MUST document remediation (re-run `--plan`) "
            "for staleness rejections (AC6)",
        )

    def test_stale_verdict_fixture_present(self) -> None:
        self.assertTrue(
            (self.fixture / "plan.md").is_file(),
            "stale_verdict_rejected fixture MUST contain a `plan.md`",
        )
        self.assertTrue(
            (self.fixture / "expected_apply_result.json").is_file(),
            "stale_verdict_rejected fixture MUST contain "
            "`expected_apply_result.json`",
        )
        expected = json.loads(
            (self.fixture / "expected_apply_result.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertFalse(
            expected["apply_should_succeed"],
            "stale_verdict_rejected fixture MUST expect `--apply` to "
            "fail (AC6 case 2)",
        )
        self.assertEqual(expected["rejection_reason"], "stale_review")
        self.assertTrue(
            expected["thread_left_untouched"],
            "stale_verdict_rejected fixture MUST expect the thread to be "
            "left untouched on rejection (AC6 — no partial output)",
        )


# ---------------------------------------------------------------------------
# AC10 (d) — declined items become `Resolution: declined` rows
# ---------------------------------------------------------------------------


class TestDeclinedItemsBecomeChangelogRows(unittest.TestCase):
    """AC7 — `--apply` honors operator edits to `plan.md`: declined
    items become `Resolution: declined — <reason>` rows in
    `changelog.md`. Three accepted rejection shapes: same-line
    `<!-- declined: <reason> -->`, row deletion, or `declined` priority
    cell + bracketed `[declined: <reason>]` in summary.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.fixture = _FIXTURES / "declined_items_decay"

    def test_all_three_rejection_shapes_documented(self) -> None:
        # AC7 enumerates three accepted rejection shapes. Each must
        # appear in the spec so the apply-side parser knows what to
        # honor.
        self.assertIn(
            "<!-- declined:",
            self.revise,
            "memo-revise.md MUST document the same-line "
            "`<!-- declined: <reason> -->` rejection shape (AC7)",
        )
        lowered = self.revise.lower()
        self.assertIn(
            "row deletion",
            lowered,
            "memo-revise.md MUST document the row-deletion rejection "
            "shape (AC7)",
        )
        self.assertIn(
            "[declined:",
            self.revise,
            "memo-revise.md MUST document the `[declined: <reason>]` "
            "priority-cell-replacement rejection shape (AC7)",
        )

    def test_declined_items_resolution_format_in_changelog(self) -> None:
        # The changelog row format MUST be documented: `Resolution:
        # declined — <reason>`.
        self.assertIn(
            "Resolution: declined",
            self.revise,
            "memo-revise.md MUST document the `Resolution: declined` "
            "changelog row format (AC7)",
        )

    def test_reason_flows_verbatim(self) -> None:
        # The "MUST NOT paraphrase or shorten" discipline must be
        # explicit so a future edit doesn't normalize reasons.
        lowered = self.revise.lower()
        self.assertTrue(
            "verbatim" in lowered,
            "memo-revise.md MUST state declined reasons flow verbatim "
            "(AC7 — no paraphrase, no normalize)",
        )

    def test_declined_items_decay_fixture_exercises_all_three_shapes(self) -> None:
        plan_md = (self.fixture / "plan.md").read_text(encoding="utf-8")
        # Shape 1: same-line declined comment
        self.assertIn(
            "<!-- declined:",
            plan_md,
            "declined_items_decay fixture MUST exercise the same-line "
            "comment rejection shape",
        )
        # Shape 2: row deletion — verified by the README + the count
        # in expected_apply_result.json
        expected = json.loads(
            (self.fixture / "expected_apply_result.json").read_text(
                encoding="utf-8"
            )
        )
        shapes_exercised = {
            item["shape"] for item in expected["declined_items"]
        }
        self.assertEqual(
            shapes_exercised,
            {
                "same_line_comment",
                "row_deletion",
                "priority_cell_replacement",
            },
            "declined_items_decay fixture MUST exercise all three "
            "rejection shapes (AC7)",
        )
        self.assertEqual(
            expected["declined_count"],
            3,
            "declined_items_decay fixture MUST expect 3 declined items "
            "(AC7)",
        )

    def test_declined_items_decay_preserves_canary_intent(self) -> None:
        # The canary intent: operator wanted "clean and forceful
        # presentation" but the reviser would have pulled toward
        # comprehensive/defensible. The declined-item reasons MUST
        # preserve this canary signal so the fixture stays anchored to
        # the issue #243 evidence.
        plan_md = (self.fixture / "plan.md").read_text(encoding="utf-8")
        self.assertIn(
            "clean and forceful presentation",
            plan_md,
            "declined_items_decay fixture MUST preserve the canary "
            "operator-intent phrase from issue #243 (Studio Northwind "
            "memo.3 → memo.4 evidence)",
        )


# ---------------------------------------------------------------------------
# AC10 (e) — target-length flag fires when projected words exceed max
# ---------------------------------------------------------------------------


class TestTargetLengthFlagFires(unittest.TestCase):
    """AC4 — the plan aggregate footer carries a `Target-length flag`
    that is one of `within_target` / `exceeds_max` / `under_min` /
    `no_target`. The flag fires (`exceeds_max`) when the projected new
    word count exceeds `max_words`.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.template = _read(_PLAN_TEMPLATE)
        self.fixture = _FIXTURES / "target_length_exceeded"

    def test_all_four_flag_values_documented_in_template(self) -> None:
        for flag in (
            "within_target",
            "exceeds_max",
            "under_min",
            "no_target",
        ):
            self.assertIn(
                flag,
                self.template,
                f"plan.md.template MUST document the `{flag}` "
                f"target-length flag value (AC4)",
            )

    def test_target_length_flag_documented_in_revise_md(self) -> None:
        self.assertIn(
            "Target-length flag",
            self.revise,
            "memo-revise.md MUST document the `Target-length flag` "
            "field in the plan aggregate footer (AC4)",
        )
        # The four values must also appear in the spec.
        for flag in (
            "within_target",
            "exceeds_max",
            "under_min",
        ):
            self.assertIn(
                flag,
                self.revise,
                f"memo-revise.md MUST document the `{flag}` flag value "
                f"(AC4)",
            )

    def test_target_length_exceeded_fixture_present(self) -> None:
        self.assertTrue(
            (self.fixture / "plan.md").is_file(),
            "target_length_exceeded fixture MUST contain a `plan.md`",
        )
        plan_md = (self.fixture / "plan.md").read_text(encoding="utf-8")
        self.assertIn(
            "`exceeds_max`",
            plan_md,
            "target_length_exceeded fixture's `plan.md` MUST flag the "
            "projected word count overshoot with `exceeds_max` (AC4)",
        )

    def test_target_length_exceeded_projected_exceeds_max(self) -> None:
        expected = json.loads(
            (self.fixture / "expected_apply_result.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertGreater(
            expected["projected_words"],
            expected["max_words"],
            "target_length_exceeded fixture MUST have "
            "`projected_words > max_words` (AC4 flag trigger)",
        )
        self.assertEqual(
            expected["target_length_flag"],
            "exceeds_max",
            "target_length_exceeded fixture MUST expect "
            "`target_length_flag = exceeds_max` (AC4)",
        )


# ---------------------------------------------------------------------------
# AC10 (f) — `--polish --plan --apply` produces `polish_plan_then_apply`
# ---------------------------------------------------------------------------


class TestPolishPlanComposesWithApply(unittest.TestCase):
    """AC1 + AC8 — `--plan` composes with `--polish`. The full
    `--polish "<reason>" --plan` → operator edits → `--apply` flow
    produces `metadata.revision_mode = "polish_plan_then_apply"` on the
    target version dir. The operator reason flows from the plan header,
    NOT re-passed on the `--apply` CLI.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.skill = _read(_SKILL_MD)
        self.fixture = _FIXTURES / "polish_plan_compose"

    def test_polish_plan_then_apply_value_documented(self) -> None:
        self.assertIn(
            "polish_plan_then_apply",
            self.revise,
            "memo-revise.md MUST document the "
            "`polish_plan_then_apply` revision_mode value (AC8)",
        )
        self.assertIn(
            "plan_then_apply",
            self.revise,
            "memo-revise.md MUST document the `plan_then_apply` "
            "revision_mode value (AC8)",
        )

    def test_polish_reason_flows_from_plan_not_cli(self) -> None:
        # The "operator does NOT re-pass --polish on --apply; the plan IS
        # the audit trail" discipline must be explicit.
        lowered = self.revise.lower()
        self.assertTrue(
            "does not re-pass" in lowered
            or "does not re-pass" in lowered
            or "not re-pass" in lowered
            or "carried through" in lowered
            or "from plan-time" in lowered,
            "memo-revise.md MUST state the operator does NOT re-pass "
            "`--polish` on `--apply` (the plan carries the reason "
            "through) (AC1 composition contract)",
        )

    def test_mutual_exclusion_plan_and_apply_documented(self) -> None:
        lowered = self.revise.lower()
        self.assertIn(
            "mutually exclusive",
            lowered,
            "memo-revise.md MUST document `--plan` and `--apply` as "
            "mutually exclusive (AC1)",
        )

    def test_polish_plan_compose_fixture_present(self) -> None:
        self.assertTrue(
            (self.fixture / "plan.md").is_file(),
            "polish_plan_compose fixture MUST contain a `plan.md`",
        )
        self.assertTrue(
            (self.fixture / "expected_apply_result.json").is_file(),
            "polish_plan_compose fixture MUST contain "
            "`expected_apply_result.json`",
        )

    def test_polish_plan_compose_fixture_declares_polish_mode(self) -> None:
        plan_md = (self.fixture / "plan.md").read_text(encoding="utf-8")
        self.assertIn(
            "Revision mode | `polish`",
            plan_md,
            "polish_plan_compose fixture's `plan.md` MUST declare "
            "`Revision mode: polish` in the header (AC8)",
        )
        self.assertIn(
            "Operator reason",
            plan_md,
            "polish_plan_compose fixture's `plan.md` MUST include an "
            "`Operator reason` header field (AC8)",
        )

    def test_polish_plan_compose_expected_revision_mode(self) -> None:
        expected = json.loads(
            (self.fixture / "expected_apply_result.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(
            expected["progress_metadata"]["revision_mode"],
            "polish_plan_then_apply",
            "polish_plan_compose fixture MUST expect "
            "`revision_mode = polish_plan_then_apply` (AC8)",
        )
        # The operator reason MUST flow verbatim from the plan header.
        self.assertEqual(
            expected["progress_metadata"]["revise_force_reason"],
            "Sharpen the conditional terms in Recommendation; reviewer "
            "noted dim 4 at 5/6 with specific suggestion.",
            "polish_plan_compose fixture MUST expect the operator "
            "reason verbatim from plan header (AC8 — reason flows "
            "through plan, NOT re-passed on CLI)",
        )
        # The changelog header note from the polish-pass header MUST be
        # preserved on the via-plan path (per AC8 composition contract).
        self.assertTrue(
            expected["changelog_header_note_present"],
            "polish_plan_compose fixture MUST expect the polish-pass "
            "changelog header note (AC8 — composes cleanly)",
        )


# ---------------------------------------------------------------------------
# AC10 (g) — default no-flag path remains unchanged (regression)
# ---------------------------------------------------------------------------


class TestDefaultNoFlagPathUnchanged(unittest.TestCase):
    """AC3 — the default no-flag path remains unchanged. Every existing
    consumer (the canary today, the 8 shipped skills' integration tests,
    the install-script regression tests) MUST NOT break. The
    `no_flag_regression/` fixture's README pins this contract.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.fixture = _FIXTURES / "no_flag_regression"

    def test_no_flag_regression_fixture_readme_present(self) -> None:
        # The fixture is intentionally empty of plan artifacts; its
        # README documents the regression contract.
        self.assertTrue(
            (self.fixture / "README.md").is_file(),
            "no_flag_regression fixture MUST contain a `README.md` "
            "documenting the unchanged-default-path contract (AC3)",
        )

    def test_no_flag_regression_fixture_has_no_plan_md(self) -> None:
        # The absence of plan.md IS the fixture: this fixture
        # represents the path the operator takes WITHOUT --plan or
        # --apply. If a future edit accidentally adds a plan.md, the
        # fixture loses its meaning.
        self.assertFalse(
            (self.fixture / "plan.md").is_file(),
            "no_flag_regression fixture MUST NOT contain a `plan.md` "
            "(the absence IS the fixture — see README.md) (AC3)",
        )

    def test_default_path_unchanged_documented(self) -> None:
        # The spec MUST document that the default no-flag path is
        # unchanged. The phrase must appear verbatim (or close to it)
        # so a future edit that quietly modifies the default path trips
        # the test.
        lowered = self.revise.lower()
        self.assertTrue(
            "default no-flag path" in lowered
            or "default path is unchanged" in lowered
            or "default path remains" in lowered
            or "path is unchanged by issue #243" in lowered
            or "must not break" in lowered,
            "memo-revise.md MUST state the default no-flag path is "
            "unchanged (AC3 — load-bearing regression contract)",
        )

    def test_default_path_legacy_11_step_procedure_documented(self) -> None:
        # The "legacy 11-step procedure" framing MUST appear so the
        # dispatch contract is explicit (steps 0a / 0b dispatch fire
        # FIRST, then fall through to the unchanged 11-step procedure).
        lowered = self.revise.lower()
        self.assertTrue(
            "11-step" in lowered or "11 step" in lowered,
            "memo-revise.md MUST reference the legacy 11-step procedure "
            "to make the dispatch contract explicit (AC3)",
        )

    def test_dispatch_steps_0a_0b_at_top_of_procedure(self) -> None:
        # The two dispatch steps (0a for --plan, 0b for --apply) MUST
        # appear in the Procedure block at the TOP, before the
        # unchanged 11-step default-path procedure.
        self.assertIn(
            "Step 0a",
            self.revise,
            "memo-revise.md MUST add `Step 0a` (--plan dispatch) at top "
            "of the Procedure block (AC1)",
        )
        self.assertIn(
            "Step 0b",
            self.revise,
            "memo-revise.md MUST add `Step 0b` (--apply dispatch) at "
            "top of the Procedure block (AC2)",
        )

    def test_existing_step_labels_survive(self) -> None:
        # The original 11-step procedure MUST survive verbatim — no
        # silent renumbering or step deletion. Spot-check the step
        # labels at the boundaries and a few in the middle.
        for label in (
            "1. **Discover state**",
            "3. **Iteration cap check**",
            "4. **Verdict pre-check**",
            "5. **Initialize `_progress.json`**",
            "9. **Write `changelog.md`**",
            "10. **Update `_progress.json`**",
            "11. **Report**",
        ):
            self.assertIn(
                label,
                self.revise,
                f"memo-revise.md MUST preserve step label `{label}` "
                f"(AC3 — no silent renumbering)",
            )


# ---------------------------------------------------------------------------
# Cross-cutting: template + SKILL.md surface coverage
# ---------------------------------------------------------------------------


class TestPlanTemplatePresent(unittest.TestCase):
    """AC9 — `templates/plan.md.template` exists and documents the
    canonical plan artifact shape (header + planned-edits table +
    aggregate footer + convictions section).
    """

    def test_template_file_exists(self) -> None:
        self.assertTrue(
            _PLAN_TEMPLATE.is_file(),
            "anvil/skills/memo/templates/plan.md.template MUST exist "
            "(AC9)",
        )

    def test_template_documents_planned_edits_table(self) -> None:
        template = _read(_PLAN_TEMPLATE)
        # The table columns MUST be documented per AC4. `Mode` was added
        # by issue #748 (resolution-mode vocabulary).
        for col in (
            "ID",
            "Source",
            "Priority",
            "Insertion site",
            "Summary",
            "Words Δ",
            "Dim Δ",
            "Mode",
        ):
            self.assertIn(
                col,
                template,
                f"plan.md.template MUST document the `{col}` table "
                f"column (AC4)",
            )

    def test_template_documents_aggregate_footer(self) -> None:
        template = _read(_PLAN_TEMPLATE)
        for field in (
            "Items planned",
            "Items by priority",
            "Total expected words Δ",
            "Projected new word count",
            "Target length window",
            "Target-length flag",
        ):
            self.assertIn(
                field,
                template,
                f"plan.md.template MUST document the `{field}` "
                f"aggregate-footer field (AC4)",
            )

    def test_template_documents_three_rejection_shapes(self) -> None:
        # The operator-use comment block MUST document all three
        # accepted rejection shapes (AC7).
        template = _read(_PLAN_TEMPLATE)
        self.assertIn(
            "<!-- declined:",
            template,
            "plan.md.template MUST document the same-line declined "
            "comment shape (AC7)",
        )
        lowered = template.lower()
        self.assertIn(
            "delete the row",
            lowered,
            "plan.md.template MUST document the row-deletion shape "
            "(AC7)",
        )
        self.assertIn(
            "[declined:",
            template,
            "plan.md.template MUST document the priority-cell + "
            "bracketed-reason shape (AC7)",
        )


class TestSkillMdSurfaceCoverage(unittest.TestCase):
    """AC9 — `SKILL.md` documents the user-facing change-set-preview
    surface (sibling to the polish-pass section), includes the plan
    sibling in the artifact-contract directory layout, and notes that
    plan siblings do NOT advance the thread to REVISED.
    """

    def setUp(self) -> None:
        self.skill = _read(_SKILL_MD)

    def test_change_set_preview_section_present(self) -> None:
        self.assertIn(
            "Operator-confirmable change-set preview",
            self.skill,
            "SKILL.md MUST add the §`Operator-confirmable change-set "
            "preview` section (AC9)",
        )

    def test_plan_sibling_in_artifact_contract(self) -> None:
        self.assertIn(
            "<thread>.2.plan/",
            self.skill,
            "SKILL.md MUST include `<thread>.2.plan/` in the "
            "artifact-contract directory layout (AC9 / AC5)",
        )

    def test_plan_sibling_does_not_advance_state(self) -> None:
        # The state-machine non-gating discipline MUST appear in
        # SKILL.md so the reader knows plan siblings are advisory.
        lowered = self.skill.lower()
        self.assertTrue(
            "plan siblings do not advance" in lowered
            or "does not advance the thread to `revised`" in lowered
            or "does not advance the thread's state to `revised`" in lowered,
            "SKILL.md MUST state plan siblings do NOT advance the "
            "thread to REVISED (AC5 — load-bearing state-machine "
            "non-gating)",
        )

    def test_plan_apply_in_command_dispatch_table(self) -> None:
        # The Command dispatch table MUST list `--plan|--apply` flags.
        self.assertIn(
            "[--plan|--apply]",
            self.skill,
            "SKILL.md Command dispatch table MUST document the "
            "`--plan|--apply` flags on `memo-revise` (AC9)",
        )

    def test_audit_trail_only_framing_preserved(self) -> None:
        # The audit-trail-only discipline MUST extend to plan_then_apply
        # values (same constraints as --polish).
        lowered = self.skill.lower()
        self.assertTrue(
            "audit-trail" in lowered or "audit trail" in lowered,
            "SKILL.md MUST preserve the audit-trail-only framing "
            "(AC5 — same constraints as polish-pass)",
        )


# ---------------------------------------------------------------------------
# Fixture inventory sanity check (AC10 — 6 fixtures named in the curation)
# ---------------------------------------------------------------------------


class TestFixtureInventoryComplete(unittest.TestCase):
    """AC10 — the curation comment named six fixtures. Each MUST exist
    under `tests/fixtures/memo_revise_plan/` so the regression-test
    surface is complete.
    """

    EXPECTED_FIXTURES = (
        "clean_plan_apply",
        "stale_verdict_rejected",
        "declined_items_decay",
        "target_length_exceeded",
        "polish_plan_compose",
        "no_flag_regression",
    )

    def test_all_six_fixture_dirs_present(self) -> None:
        for name in self.EXPECTED_FIXTURES:
            self.assertTrue(
                (_FIXTURES / name).is_dir(),
                f"AC10 fixture `{name}/` MUST exist under "
                f"`tests/fixtures/memo_revise_plan/`",
            )

    def test_no_unexpected_extra_fixture_dirs(self) -> None:
        # A loose extra fixture directory would dilute the AC10
        # contract. Pin the inventory tight so adding a new fixture is
        # a deliberate test edit.
        present = {
            p.name
            for p in _FIXTURES.iterdir()
            if p.is_dir() and not p.name.startswith("_")
        }
        self.assertEqual(
            present,
            set(self.EXPECTED_FIXTURES),
            "tests/fixtures/memo_revise_plan/ MUST contain exactly the "
            "six AC10-named fixture directories",
        )


# ---------------------------------------------------------------------------
# Issue #748 — resolution modes (added/restructured/removed/moved-to-exhibit)
# + net-delta reporting: the additive-only reviser contract.
# ---------------------------------------------------------------------------


class TestResolutionModeVocabulary(unittest.TestCase):
    """Issue #748 — every addressed `Resolution:` row in `changelog.md`
    declares HOW the finding was resolved via a mode tag, extending the
    existing `declined` / `addressed (judgment-held)` vocabulary rather
    than replacing it. Mirrors the precedent set by
    ``test_declined_items_resolution_format_in_changelog`` above: assert
    on the documented row-format substring in ``commands/memo-revise.md``.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_four_resolution_modes_documented(self) -> None:
        for mode in ("added", "restructured", "removed", "moved-to-exhibit"):
            self.assertIn(
                f"`{mode}`",
                self.revise,
                f"memo-revise.md MUST document the `{mode}` resolution "
                f"mode (issue #748)",
            )

    def test_mode_tagged_resolution_row_format_documented(self) -> None:
        # The mode-tagged row shape MUST be documented: `Resolution:
        # <mode> — <change>`.
        self.assertIn(
            "Resolution: <mode> — <change>",
            self.revise,
            "memo-revise.md MUST document the `Resolution: <mode> — "
            "<change>` changelog row format (issue #748)",
        )

    def test_declined_and_judgment_held_dispositions_survive(self) -> None:
        # The pre-#748 vocabulary (`declined`, `addressed
        # (judgment-held)`) MUST NOT be removed — issue #748 extends it.
        self.assertIn("Resolution: declined", self.revise)
        self.assertIn("addressed (judgment-held)", self.revise)

    def test_removed_before_added_prompt_instruction_documented(self) -> None:
        # Proposal item 3 — the revision-plan prompt (step 7) MUST
        # instruct the reviser to consider `removed` before `added`, and
        # to re-examine an all-`added` plan.
        self.assertIn(
            "consider `removed` before `added`",
            self.revise,
            "memo-revise.md step 7 MUST inject the 'consider `removed` "
            "before `added`' instruction into the revision-plan prompt "
            "(issue #748 proposal item 3)",
        )
        lowered = self.revise.lower()
        self.assertIn(
            "should be re-examined before applying",
            lowered,
            "memo-revise.md MUST document that an all-`added` plan "
            "should be re-examined before applying (issue #748)",
        )

    def test_worked_removed_example_in_changelog(self) -> None:
        # Proposal item 6 / acceptance sketch: one worked example of a
        # `removed` resolution in the command doc's changelog example.
        self.assertIn(
            "removed — cut",
            self.revise,
            "memo-revise.md MUST include a worked `removed` resolution "
            "example in the changelog.md example table (issue #748)",
        )

    def test_no_new_gating_stated(self) -> None:
        # Proposal item 4 — audit-trail only, no score/gate/state-machine
        # impact. Same framing discipline as `metadata.scope` /
        # `metadata.revision_mode`.
        lowered = self.revise.lower()
        self.assertIn(
            "no new gating",
            lowered,
            "memo-revise.md MUST explicitly state resolution modes "
            "introduce no new gating (issue #748 proposal item 4)",
        )


class TestNetDeltaHeader(unittest.TestCase):
    """Issue #748 proposal item 2 — the changelog header reports the
    revision's net word count and bold-span count delta.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)

    def test_net_delta_header_documented(self) -> None:
        self.assertIn(
            "Net-delta header",
            self.revise,
            "memo-revise.md MUST document the changelog net-delta "
            "header (issue #748)",
        )

    def test_words_and_bold_spans_lines_documented(self) -> None:
        self.assertIn(
            "words: 3417 → 3597 (+180)",
            self.revise,
            "memo-revise.md MUST document the `words: <before> → "
            "<after> (<Δ>)` net-delta header line (issue #748)",
        )
        self.assertIn(
            "bold spans: 122 → 136 (+14)",
            self.revise,
            "memo-revise.md MUST document the `bold spans: <before> → "
            "<after> (<Δ>)` net-delta header line (issue #748)",
        )

    def test_net_delta_header_is_audit_trail_only(self) -> None:
        # Same audit-trail-only framing as `metadata.scope` /
        # `metadata.revision_mode` — no score or gating impact.
        lowered = self.revise.lower()
        self.assertIn(
            "audit-trail only",
            lowered,
            "memo-revise.md MUST frame the net-delta header as "
            "audit-trail only (issue #748)",
        )


class TestPlanModeColumnEditableBeforeApply(unittest.TestCase):
    """Issue #748 proposal item 5 — under `--plan`, planned rows carry a
    proposed resolution mode that is editable before `--apply`.
    """

    def setUp(self) -> None:
        self.revise = _read(_REVISE_MD)
        self.template = _read(_PLAN_TEMPLATE)
        self.skill = _read(_SKILL_MD)

    def test_plan_row_mode_cell_documented_in_revise_md(self) -> None:
        self.assertIn(
            "`Mode` (one of `added` / `restructured` / `removed` / "
            "`moved-to-exhibit`",
            self.revise,
            "memo-revise.md MUST document the plan row `Mode` cell "
            "(issue #748)",
        )

    def test_mode_cell_editable_independent_of_decline(self) -> None:
        lowered = self.revise.lower()
        self.assertIn(
            "may be edited by the operator",
            lowered,
            "memo-revise.md MUST state the `Mode` cell is editable by "
            "the operator before `--apply` (issue #748)",
        )

    def test_apply_reads_mode_cell_into_changelog(self) -> None:
        self.assertIn(
            "read the `Mode` cell",
            self.revise,
            "memo-revise.md `--apply` dispatch (step 0b) MUST document "
            "reading each row's `Mode` cell into the changelog "
            "resolution-mode tag (issue #748)",
        )

    def test_template_documents_mode_column_operator_use(self) -> None:
        self.assertIn(
            "RESOLUTION MODE (issue #748)",
            self.template,
            "plan.md.template's operator-use comment block MUST "
            "document the `Mode` cell contract (issue #748)",
        )

    def test_skill_md_documents_proposed_mode(self) -> None:
        self.assertIn(
            "proposed resolution mode",
            self.skill,
            "SKILL.md MUST document that planned edits carry a "
            "proposed resolution mode (issue #748)",
        )


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
