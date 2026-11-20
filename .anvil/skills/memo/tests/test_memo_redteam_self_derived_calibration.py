"""Doc-discipline test: self-derived calibration mode (issue #913).

``memo-redteam``'s ``calibration.md`` crosscheck was, pre-#913, emitted
ONLY when a hand-authored ``refs/strongman-against.md`` (or portfolio-level
equivalent) resolved in the refs-dir list. In the studio canary that file
is absent on most threads — the reviser cannot honestly author a strongman
against its own memo, since a strongman is meant to supply objections
independently of the author — so the calibration signal, in practice the
single most valuable output of a red-team pass, was gated behind substrate
that usually does not exist.

This file asserts that the documented contract in
``commands/memo-redteam.md`` (and its ``rubric.md`` / ``SKILL.md``
companions) now defines a second, clearly-labelled ``self-derived`` mode:
emitted when no strongman resolves, deriving its anticipated-objection set
from the memo's own risk register / concessions / kill list, and stating
its mode + author-generous bias explicitly at the top of the file. It also
asserts ``calibration.md`` is no longer conditional in the staged-sidecar
manifest, and that ``_meta.json`` records the resolved mode.

Per the framework's doc-discipline convention (e.g.
``test_memo_redteam_independence_of_strongman.py``), this test matches
substring/pattern presence in the markdown, NOT runtime behaviour — the
critic is LLM-driven; behavioural assertions belong in consumer-side
integration tests.

Per the per-skill test filename convention (#58), this file is named
``test_memo_redteam_self_derived_calibration.py``.
"""

from __future__ import annotations

import re
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent
_REDTEAM_MD = _SKILL_ROOT / "commands" / "memo-redteam.md"
_SKILL_MD = _SKILL_ROOT / "SKILL.md"
_RUBRIC_MD = _SKILL_ROOT / "rubric.md"


def _read(path: Path) -> str:
    assert path.exists(), f"expected to exist: {path}"
    return path.read_text(encoding="utf-8")


def test_redteam_command_defines_self_derived_mode():
    """memo-redteam.md names the self-derived mode and when it fires."""
    body = _read(_REDTEAM_MD)
    assert "self-derived" in body, (
        "memo-redteam.md must name the 'self-derived' calibration mode "
        "(issue #913)."
    )
    # The mode must be tied explicitly to the absence of strongman-against.md.
    matchers = [
        r"no.{0,40}strongman-against\.md.{0,80}resolves.{0,120}self-derived",
        r"self-derived.{0,200}no.{0,40}strongman-against\.md",
        r"absent.{0,80}strongman-against\.md.{0,200}self-derived",
    ]
    found = any(re.search(p, body, re.IGNORECASE | re.DOTALL) for p in matchers)
    assert found, (
        "memo-redteam.md must tie 'self-derived' mode explicitly to the "
        "absence of refs/strongman-against.md — none of the expected "
        f"patterns matched: {matchers}"
    )


def test_redteam_command_self_derived_source_is_memo_self_critique():
    """Self-derived mode's anticipated set comes from the memo's own risk register/concessions/kill list."""
    body = _read(_REDTEAM_MD)
    assert "risk register" in body.lower(), (
        "memo-redteam.md must name the memo's own risk register as "
        "self-derived-mode substrate."
    )
    assert "concession" in body.lower(), (
        "memo-redteam.md must name the memo's own concessions/caveats as "
        "self-derived-mode substrate."
    )
    assert "kill" in body.lower(), (
        "memo-redteam.md must name the memo's own kill-criteria/kill list "
        "as self-derived-mode substrate."
    )


def test_redteam_command_calibration_md_no_longer_conditional():
    """calibration.md is unconditional in the staged-sidecar manifest (issue #913)."""
    body = _read(_REDTEAM_MD)
    # The old conditional framing must be gone from the manifest description.
    assert "ALWAYS" in body or "always" in body, (
        "memo-redteam.md must state calibration.md is now always emitted."
    )
    assert "unconditional" in body.lower(), (
        "memo-redteam.md must state calibration.md's manifest membership is "
        "unconditional (issue #913 — previously gated on strongman-against.md "
        "presence)."
    )
    # The required-files manifest list must include calibration.md alongside
    # the other four base files (all five, comma/quote-separated in the doc).
    assert re.search(
        r'"_review\.json".{0,10}"objections\.md".{0,10}"calibration\.md".{0,10}"_meta\.json".{0,10}"_progress\.json"',
        body,
    ), (
        "memo-redteam.md's staged_sidecar required_files manifest literal "
        "must list calibration.md unconditionally alongside the other four "
        "required files."
    )


def test_redteam_command_calibration_md_states_mode_and_bias_at_top():
    """calibration.md's self-derived template opens with mode + bias banner."""
    body = _read(_REDTEAM_MD)
    assert "Mode: self-derived" in body, (
        "memo-redteam.md's self-derived calibration.md template must open "
        "with an explicit 'Mode: self-derived.' banner line."
    )
    assert "**Bias.**" in body, (
        "memo-redteam.md's self-derived calibration.md template must carry "
        "an explicit '**Bias.**' statement — a self-derived anticipated set "
        "is generous to the author by construction, since it can only "
        "contain objections the author already wrote down."
    )
    assert "generous to the author" in body.lower() or "generous-to-the-author" in body.lower(), (
        "memo-redteam.md must explicitly characterize the self-derived "
        "anticipated set as generous to the author by construction."
    )


def test_redteam_command_meta_json_records_calibration_mode():
    """_meta.json schema carries calibration_mode alongside strongman_crosscheck_present."""
    body = _read(_REDTEAM_MD)
    assert "calibration_mode" in body, (
        "memo-redteam.md's _meta.json shape must add a calibration_mode "
        "field (issue #913 AC)."
    )
    assert "strongman-crosscheck" in body and "self-derived" in body, (
        "memo-redteam.md must document both calibration_mode enum values: "
        "'strongman-crosscheck' and 'self-derived'."
    )
    assert "strongman_crosscheck_present" in body, (
        "memo-redteam.md must retain the existing strongman_crosscheck_present "
        "boolean field alongside the new calibration_mode field."
    )


def test_rubric_md_documents_two_calibration_modes():
    """rubric.md's Red-team back-check section documents both modes."""
    body = _read(_RUBRIC_MD)
    assert "self-derived" in body, (
        "rubric.md must document the self-derived calibration mode "
        "(issue #913) in its Red-team back-check subsection."
    )
    assert "calibration_mode" in body, (
        "rubric.md must reference the calibration_mode _meta.json field."
    )


def test_skill_md_documents_calibration_always_emitted():
    """SKILL.md's directory-layout block reflects the unconditional emission."""
    body = _read(_SKILL_MD)
    assert "self-derived" in body, (
        "SKILL.md must mention the self-derived calibration mode in its "
        "redteam sibling discussion (issue #913)."
    )
    # The stale "(conditional)" parenthetical must be gone from the
    # calibration.md directory-layout line.
    assert "(conditional) Author-strongman crosscheck" not in body, (
        "SKILL.md's directory-layout block still carries the pre-#913 "
        "'(conditional)' framing for calibration.md — it is now always "
        "emitted."
    )


def test_redteam_command_notes_calibration_never_optional():
    """The 'Notes for the red-team agent' section warns against skipping calibration."""
    body = _read(_REDTEAM_MD)
    assert "never optional" in body.lower() or "not optional" in body.lower(), (
        "memo-redteam.md must explicitly warn the agent that calibration.md "
        "is never optional, even in the common no-strongman case (issue #913)."
    )
