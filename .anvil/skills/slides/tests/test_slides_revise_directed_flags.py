"""Doc-contract tests for ``slides-revise --polish`` / ``--reject`` (issue #1369).

Cheap "grep-the-doc" guards, modeled on
``anvil/skills/proposal/tests/test_proposal_revise_polish_flag.py``: they
assert substring presence only and do NOT execute the (LLM-driven) reviser.
"""

from __future__ import annotations

from pathlib import Path

from anvil.lib.testing import read_text as _read

SKILL_ROOT = Path(__file__).resolve().parents[1]
SKILL_MD = SKILL_ROOT / "SKILL.md"
REVISE_MD = SKILL_ROOT / "commands" / "slides-revise.md"
REVIEW_MD = SKILL_ROOT / "commands" / "slides-review.md"
SNIPPET = Path(__file__).resolve().parents[4] / "anvil" / "lib" / "snippets" / "directed_revision.md"


def test_flag_subsections_present():
    body = _read(REVISE_MD)
    assert "## CLI flags" in body
    assert '### `--polish "<reason>"`' in body
    assert '### `--reject "<reason>"`' in body
    assert "directed_revision.md" in body


def test_step4_no_longer_recommends_manual_state_surgery():
    body = _read(REVISE_MD)
    lowered = body.lower()
    assert "advance == true" in body
    assert "bumping the iteration manually" not in lowered
    assert "deleting the verdict or bumping" not in lowered
    assert "no critical flags from any sibling" in lowered or (
        "no critical flags from any sibling (review, audit, rehearse)" in lowered
    )


def test_bypass_is_step4_only_with_guards_still_applying():
    body = _read(REVISE_MD)
    lowered = body.lower()
    assert "skipped entirely" in lowered
    assert "proceed to step 5" in lowered
    assert "bypass only this step" in lowered or "bypass only" in lowered
    assert "step 1's review-exists check" in body
    assert "iteration-cap check (step 3) still applies" in body


def test_reason_required_and_thread_untouched():
    lowered = _read(REVISE_MD).lower()
    assert "required" in lowered
    assert "whitespace" in lowered
    assert "left untouched" in lowered


def test_mutual_exclusion_documented():
    body = _read(REVISE_MD)
    assert "Mutually exclusive with `--polish`" in body
    assert "mutually exclusive" in _read(SKILL_MD).lower()


def test_reject_reason_ranked_above_critic_findings():
    body = _read(REVISE_MD).lower()
    assert "highest-priority finding" in body
    assert "above every critic finding" in body


def test_audit_trail_field_names():
    body = _read(REVISE_MD)
    for token in (
        "revision_mode",
        '"polish"',
        '"operator_reject"',
        "revise_force_reason",
        "reject_reason",
        "verbatim",
        "Polish pass",
        "Operator rejection",
    ):
        assert token in body, token


def test_no_inherited_credit_documented():
    assert "no inherited credit" in _read(REVISE_MD).lower()


def test_review_emits_operator_rejection_note():
    body = _read(REVIEW_MD)
    assert "## Operator rejection" in body
    assert "operator_reject" in body
    assert "no score impact" in body


def test_skill_md_cross_reference_and_command_row():
    body = _read(SKILL_MD)
    assert "Operator-initiated polish / rejection" in body
    assert 'slides-revise <thread> [--polish "<reason>"] [--reject "<reason>"]' in body


def test_snippet_lists_slides_as_adopted_not_pending():
    body = _read(SNIPPET)
    adopted, _, pending = body.partition("**Pending adoption**")
    assert "**`slides`**" in adopted
    pending_list = pending.split("## Adoption step")[0]
    assert "`slides`" not in pending_list
