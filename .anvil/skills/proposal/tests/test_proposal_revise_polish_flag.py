"""Doc-coverage smoke tests for the ``proposal-revise --polish "<reason>"`` flag.

Per issue #862, ``proposal-revise`` adopts the generic operator-directed
revision contract already codified in ``anvil/lib/snippets/directed_revision.md``
and shipped by ``memo`` (the original consumer, issue #201) and ``primer``
(the report-family — review+audit — adoption, issue #691). This module pins
the **documented contract** in ``proposal-revise.md`` + ``SKILL.md``: cheap
"grep-the-doc" regression guards that the required-reason rejection prose,
the step-4-only bypass scope, and the ``metadata.revision_mode`` /
``metadata.revise_force_reason`` audit-trail field names stay documented and
don't drift back to the pre-#862 "delete a verdict or hand-edit
``_progress.json``" workaround prose.

These tests assert on substring presence and structural ordering only —
they do NOT validate prose quality and they do NOT execute the reviser.
The reviser is LLM-driven, so behavioural assertions belong in
consumer-side integration tests, not here.

Per-skill test filename convention (#58): this file is named with a
``test_proposal_`` prefix so it never collides with the
``test_revise_polish_flag`` shape another skill might pick (see
``tests/skills/memo/test_memo_revise_polish_flag.py`` for the sibling
convention this module mirrors).
"""

from __future__ import annotations

from pathlib import Path

from anvil.lib.testing import read_text as _read

SKILL_ROOT = Path(__file__).resolve().parents[1]
SKILL_MD = SKILL_ROOT / "SKILL.md"
REVISE_MD = SKILL_ROOT / "commands" / "proposal-revise.md"
DIRECTED_REVISION_SNIPPET = (
    Path(__file__).resolve().parents[4]
    / "anvil"
    / "lib"
    / "snippets"
    / "directed_revision.md"
)


# ---------------------------------------------------------------------------
# The generic snippet is referenced (adoption discoverability)
# ---------------------------------------------------------------------------


def test_proposal_revise_references_directed_revision_snippet():
    body = _read(REVISE_MD)
    assert "directed_revision.md" in body, (
        "proposal-revise.md MUST reference "
        "anvil/lib/snippets/directed_revision.md (issue #862)"
    )
    assert DIRECTED_REVISION_SNIPPET.exists()


# ---------------------------------------------------------------------------
# Default path (no flag) is unchanged — the refuse-and-notice prose no
# longer recommends verdict deletion / manual iteration bumping
# ---------------------------------------------------------------------------


def test_step4_no_longer_recommends_manual_state_surgery():
    """Step 4's parenthetical MUST stop suggesting the deleted-verdict /
    hand-edited-``_progress.json`` workaround — the issue's core complaint.
    """
    body = _read(REVISE_MD)
    lowered = body.lower()
    assert "advance == true" in body, (
        "proposal-revise.md step 4 MUST keep the combined-advance "
        "pre-check prose"
    )
    assert "deleting a verdict" not in lowered, (
        "proposal-revise.md step 4 MUST NOT recommend deleting a critic "
        "verdict sibling as the override path (issue #862)"
    )
    assert "bumping the iteration manually" not in lowered, (
        "proposal-revise.md step 4 MUST NOT recommend hand-editing "
        "_progress.json / manually bumping the iteration as the override "
        "path (issue #862)"
    )
    assert '--polish "<reason>"' in body, (
        "proposal-revise.md step 4 MUST point at --polish as the "
        "sanctioned override (issue #862)"
    )


# ---------------------------------------------------------------------------
# --polish bypasses step 4 ONLY
# ---------------------------------------------------------------------------


def test_polish_flag_bypasses_step4_only():
    body = _read(REVISE_MD)
    lowered = body.lower()
    assert "skipped entirely" in lowered, (
        "proposal-revise.md MUST state that step 4 is skipped entirely "
        "when --polish is passed (issue #862)"
    )
    assert "proceed to step 5" in lowered, (
        "proposal-revise.md MUST state --polish proceeds to step 5 after "
        "bypassing step 4 (issue #862)"
    )
    assert "step 1's dual-critic-required check" in body or (
        "critic-completeness check (step 1) still applies" in body
    ), (
        "proposal-revise.md MUST state the step-1 critic-completeness "
        "check still applies under --polish (issue #862)"
    )
    assert "iteration-cap check (step 3) still applies" in body, (
        "proposal-revise.md MUST state the step-3 iteration cap still "
        "applies under --polish (issue #862)"
    )


# ---------------------------------------------------------------------------
# Required, non-empty reason
# ---------------------------------------------------------------------------


def test_polish_flag_empty_reason_rejected():
    body = _read(REVISE_MD)
    lowered = body.lower()
    assert "required" in lowered, (
        "proposal-revise.md MUST state the --polish reason argument is "
        "required (issue #862)"
    )
    assert "whitespace" in lowered, (
        "proposal-revise.md MUST state whitespace-only --polish reasons "
        "are rejected (issue #862)"
    )
    assert "left untouched" in lowered, (
        "proposal-revise.md MUST state the thread is left untouched on "
        "a rejected --polish invocation (issue #862)"
    )


# ---------------------------------------------------------------------------
# Audit-trail fields land on disk
# ---------------------------------------------------------------------------


def test_polish_flag_records_revision_mode_and_reason():
    revise = _read(REVISE_MD)
    skill = _read(SKILL_MD)
    assert "revision_mode" in revise, (
        "proposal-revise.md MUST document metadata.revision_mode "
        "(issue #862)"
    )
    assert "revise_force_reason" in revise, (
        "proposal-revise.md MUST document metadata.revise_force_reason "
        "(issue #862)"
    )
    assert '"polish"' in revise, (
        "proposal-revise.md MUST show the revision_mode = \"polish\" "
        "value (issue #862)"
    )
    assert "verbatim" in revise.lower(), (
        "proposal-revise.md MUST state the operator reason is stored "
        "verbatim (issue #862 — no trimming, no normalization)"
    )
    # SKILL.md — the user-facing contract
    assert "revision_mode" in skill, (
        "SKILL.md MUST document metadata.revision_mode in the "
        "operator-initiated polish-passes section (issue #862)"
    )
    assert "revise_force_reason" in skill, (
        "SKILL.md MUST document metadata.revise_force_reason in the "
        "operator-initiated polish-passes section (issue #862)"
    )
    assert "Operator-initiated polish passes" in skill, (
        "SKILL.md MUST add an 'Operator-initiated polish passes' "
        "section (issue #862)"
    )


def test_polish_flag_no_inherited_credit_documented():
    body = _read(REVISE_MD)
    lowered = body.lower()
    assert "no inherited credit" in lowered, (
        "proposal-revise.md MUST document the no-inherited-credit rule "
        "(issue #862)"
    )
    assert "own rubric merits" in lowered or "own merits" in lowered, (
        "proposal-revise.md MUST state the next critic pair scores the "
        "polish-pass output on its own merits (issue #862)"
    )


# ---------------------------------------------------------------------------
# Changelog discipline for a polish pass
# ---------------------------------------------------------------------------


def test_polish_flag_changelog_header_documented():
    body = _read(REVISE_MD)
    assert "Polish pass" in body, (
        "proposal-revise.md MUST document the 'Polish pass' header note "
        "for --polish-produced changelog.md (issue #862)"
    )
    assert "Operator reason:" in body, (
        "proposal-revise.md MUST document the 'Operator reason: "
        "<verbatim>' shape in the changelog header note (issue #862)"
    )


# ---------------------------------------------------------------------------
# Composition with the existing --scope flag
# ---------------------------------------------------------------------------


def test_polish_composes_with_scope_flag():
    body = _read(REVISE_MD)
    assert "Composition with `--polish`" in body, (
        "proposal-revise.md MUST document the --scope / --polish "
        "composition (issue #862)"
    )
    assert "degenerate" in body.lower(), (
        "proposal-revise.md MUST document the degenerate "
        "--polish --scope critical-only case (issue #862)"
    )
