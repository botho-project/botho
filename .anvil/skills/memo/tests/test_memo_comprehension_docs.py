"""Doc-discipline test: memo-comprehension.md declares its load-bearing contract (issue #753).

The load-bearing mechanism of the comprehension critic is **blindness**: it
must be dispatched as a fresh agent that reads ONLY the memo body — no BRIEF,
no research, no rubric, no skeleton, no prior review — because a critic that
has read the intended message cannot un-know it. This file asserts the
documented contract in ``commands/memo-comprehension.md`` (and the supporting
``SKILL.md`` / ``memo.md`` / ``rubric.md`` prose) carries the load-bearing
claims explicitly. The test matches substring / regex presence in the markdown,
NOT runtime behaviour (the critic is LLM-driven; behavioural assertions belong
in consumer-side integration tests) — structurally identical to the
``test_memo_redteam_independence_of_strongman.py`` doc-discipline suite.

Per the per-skill test filename convention (#58), this file is named
``test_memo_comprehension_docs.py``.
"""

from __future__ import annotations

import re
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent
_COMPREHENSION_MD = _SKILL_ROOT / "commands" / "memo-comprehension.md"
_SKILL_MD = _SKILL_ROOT / "SKILL.md"
_RUBRIC_MD = _SKILL_ROOT / "rubric.md"
_MEMO_MD = _SKILL_ROOT / "commands" / "memo.md"

_VERDICTS = ("CLEAR", "GARBLED", "MISSING", "HONEST-GAP")


def _read(path: Path) -> str:
    assert path.exists(), f"expected to exist: {path}"
    return path.read_text(encoding="utf-8")


def test_memo_comprehension_md_exists():
    """commands/memo-comprehension.md exists at the expected location."""
    assert _COMPREHENSION_MD.exists(), (
        f"commands/memo-comprehension.md not found at {_COMPREHENSION_MD}; "
        f"this is the load-bearing artifact for issue #753."
    )


def test_comprehension_command_declares_blindness_invocation_contract():
    """The command body MUST document blindness as a HARD requirement.

    The load-bearing claim of issue #753 is that the comprehension critic is
    dispatched as a fresh agent handed only the body — no BRIEF, no research,
    no rubric, no skeleton, no prior review. Verifying the documented contract
    carries this rule is the cheapest regression guard against a future edit
    that quietly softens blindness into a suggestion.
    """
    body = _read(_COMPREHENSION_MD)
    # The contract must be named as a HARD requirement, not a suggestion.
    assert re.search(
        r"(one hard requirement|hard requirement|MUST be dispatched)",
        body,
        re.IGNORECASE,
    ), (
        "memo-comprehension.md must document the blind-read invocation "
        "contract as a HARD requirement (not a suggestion)."
    )
    # It must name a fresh agent handed only the body.
    assert re.search(r"fresh agent", body, re.IGNORECASE), (
        "memo-comprehension.md must state the critic is dispatched as a "
        "fresh agent."
    )
    # It must explicitly exclude the intent sources during the blind read.
    for excluded in ("BRIEF", "research", "rubric", "skeleton", "prior"):
        assert re.search(rf"\bno\b[^.\n]*{excluded}", body, re.IGNORECASE) or (
            excluded in body
        ), (
            f"memo-comprehension.md must name {excluded!r} as excluded from "
            f"the blind read."
        )
    # The body-only ordering (answers before intent) must be stated.
    assert re.search(
        r"(body-only|answers first|before .* intent|ONLY .* body)",
        body,
        re.IGNORECASE,
    ), (
        "memo-comprehension.md must document the answers-before-intent "
        "ordering that enforces blindness."
    )


def test_comprehension_command_declares_four_verdict_vocabulary():
    """CLEAR / GARBLED / MISSING / HONEST-GAP all appear in the command body."""
    body = _read(_COMPREHENSION_MD)
    for verdict in _VERDICTS:
        assert verdict in body, (
            f"memo-comprehension.md must document the {verdict!r} verdict — "
            f"the four-valued vocabulary is the core charter of issue #753."
        )


def test_comprehension_command_marks_honest_gap_non_defect():
    """HONEST-GAP MUST be explicitly marked a non-defect."""
    body = _read(_COMPREHENSION_MD)
    # HONEST-GAP and "not a defect" (or "non-defect") must co-occur.
    assert re.search(
        r"HONEST-GAP[^.\n]*(non-defect|NOT a defect|not a defect)",
        body,
        re.IGNORECASE,
    ) or re.search(
        r"(non-defect|NOT a defect|not a defect)[^.\n]*HONEST-GAP",
        body,
        re.IGNORECASE,
    ) or (
        "HONEST-GAP" in body
        and re.search(r"(non-defect|NOT a defect|not a defect)", body, re.IGNORECASE)
    ), (
        "memo-comprehension.md must explicitly mark HONEST-GAP as a "
        "non-defect (an early-stage artifact that plainly states an "
        "unresolved gap is not penalized)."
    )


def test_comprehension_command_is_findings_only_no_score_no_gate():
    """The command MUST state it owns no dimension, no score, no critical flag."""
    body = _read(_COMPREHENSION_MD)
    assert "findings-only" in body.lower(), (
        "memo-comprehension.md must describe itself as findings-only."
    )
    # All-null scores (owns no dimension).
    assert re.search(r"(all[-\s]?null|scores? .* null|null .* scores?)", body, re.IGNORECASE), (
        "memo-comprehension.md must document that all scores are null "
        "(the critic owns no rubric dimension)."
    )
    # Critical flags always empty (never gates).
    assert re.search(
        r"(critical_flags[^.\n]*(empty|\[\])|never .* gate|non-gating)",
        body,
        re.IGNORECASE,
    ), (
        "memo-comprehension.md must document that critical_flags is always "
        "empty / the critic never gates."
    )


def test_comprehension_command_declares_sidecar_atomicity():
    """The command MUST use the anvil/lib/sidecar.py atomicity primitive."""
    body = _read(_COMPREHENSION_MD)
    assert "staged_sidecar" in body, (
        "memo-comprehension.md must invoke anvil/lib/sidecar.py::staged_sidecar "
        "for atomic critic-sibling writes — the shared framework convention."
    )
    assert "cleanup_one_staging" in body, (
        "memo-comprehension.md must invoke cleanup_one_staging at entry — the "
        "per-critic parallel-safe sweep from issue #376."
    )
    assert ".comprehension.tmp" in body, (
        "memo-comprehension.md must document the leading-dot .comprehension.tmp/ "
        "staging-dir shape produced by staged_sidecar."
    )


def test_comprehension_command_declares_review_schema_compliance():
    """The command MUST emit a canonical _review.json per review_schema."""
    body = _read(_COMPREHENSION_MD)
    assert "_review.json" in body, (
        "memo-comprehension.md must emit _review.json (the canonical typed "
        "review payload consumed by aggregate)."
    )
    assert "review_schema" in body, (
        "memo-comprehension.md must reference anvil/lib/review_schema.py — the "
        "schema of record for _review.json."
    )
    assert 'critic_id: "comprehension"' in body or "critic_id: comprehension" in body or "\"comprehension\"" in body, (
        "memo-comprehension.md must name the 'comprehension' critic_id."
    )


def test_comprehension_command_declares_seven_question_questionnaire():
    """The command MUST document the fixed 7-question blind questionnaire."""
    body = _read(_COMPREHENSION_MD)
    # A few load-bearing question fragments from the issue body.
    for fragment in (
        "sell",       # Q1 — what does this company sell
        "buys it",    # Q2 — who buys it
        "win",        # Q3 — why does this team win
        "ask",        # Q4 — what is the ask
        "kills it",   # Q5 — what kills it
    ):
        assert fragment.lower() in body.lower(), (
            f"memo-comprehension.md must document the questionnaire item "
            f"referencing {fragment!r}."
        )


def test_comprehension_command_notes_zero_change_to_memo_revise():
    """The command MUST note memo-revise consumes it with zero code change."""
    body = _read(_COMPREHENSION_MD)
    assert "memo-revise" in body, (
        "memo-comprehension.md must reference memo-revise as the consumer."
    )
    assert re.search(r"zero .* change|no .* change", body, re.IGNORECASE), (
        "memo-comprehension.md must state memo-revise consumes its findings "
        "with zero code change (generic critic-sibling enumeration)."
    )


def test_skill_md_mentions_comprehension_critic_dir():
    """SKILL.md directory-layout block + prose document the comprehension sibling."""
    body = _read(_SKILL_MD)
    assert ".comprehension/" in body, (
        "SKILL.md must document the <thread>.{N}.comprehension/ sibling in its "
        "directory-layout block."
    )
    assert "#753" in body, (
        "SKILL.md must reference issue #753 in the comprehension discussion."
    )
    # The prose paragraph must name the blind-read contract and HONEST-GAP.
    assert "HONEST-GAP" in body, (
        "SKILL.md must document HONEST-GAP as a non-defect verdict in the "
        "comprehension-layer prose paragraph."
    )
    assert re.search(r"blind[- ]read", body, re.IGNORECASE), (
        "SKILL.md must describe the blind-read contract in its comprehension "
        "prose paragraph."
    )


def test_rubric_md_documents_comprehension_verdict_vocabulary():
    """rubric.md MUST document the verdict vocabulary incl. HONEST-GAP non-defect."""
    body = _read(_RUBRIC_MD)
    assert "Comprehension back-check" in body, (
        "rubric.md must add a §'Comprehension back-check' subsection "
        "documenting the verdict vocabulary."
    )
    for verdict in _VERDICTS:
        assert verdict in body, (
            f"rubric.md must document the {verdict!r} verdict in the "
            f"comprehension subsection."
        )
    assert re.search(r"(non-defect|NOT a defect|not a defect)", body, re.IGNORECASE), (
        "rubric.md must state HONEST-GAP is a non-defect."
    )
    # No new scored dimension.
    assert re.search(r"(no rubric dimension|adds NO rubric dimension|no .* dimension)", body, re.IGNORECASE), (
        "rubric.md must state the comprehension back-check adds no scored "
        "rubric dimension."
    )


def test_orchestrator_documents_optional_comprehension_critic():
    """memo.md orchestrator notes memo-comprehension as an optional parallel critic."""
    body = _read(_MEMO_MD)
    assert "memo-comprehension" in body, (
        "commands/memo.md orchestrator must document memo-comprehension as an "
        "optional parallel critic alongside memo-review."
    )
    assert re.search(r"non-gating|findings-only", body, re.IGNORECASE), (
        "commands/memo.md must note memo-comprehension is non-gating / "
        "findings-only."
    )
