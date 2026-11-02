"""Tests for the slides editorial-overrides matcher + BRIEF parsing (issue #1373)."""

from __future__ import annotations

from pathlib import Path

import pytest

from anvil.lib.project_brief import EditorialOverride, load_editorial_overrides_for_slug
from anvil.skills.slides.lib.editorial_overrides import apply_overrides, match_override

OV = EditorialOverride(slide=3, dimension="4", reason="build photos replace sketch", date="2026-10-15")


def _f(span, dim, sev="minor"):
    return {"evidence_span": span, "dimension": dim, "severity": sev, "rationale": "r"}


def test_matching_pair():
    assert match_override(_f("slides.1/slides.md:slide=3", "4"), [OV]) is OV


def test_same_slide_different_dimension():
    assert match_override(_f("slides.1/slides.md:slide=3", "5"), [OV]) is None


def test_same_dimension_different_slide():
    assert match_override(_f("slides.1/slides.md:slide=13", "4"), [OV]) is None
    assert match_override(_f("slides.1/slides.md:slide=2", "4"), [OV]) is None


def test_no_span_or_dimension():
    assert match_override(_f(None, "4"), [OV]) is None
    assert match_override(_f("slides.1/slides.md:slide=3", None), [OV]) is None


def test_apply_downgrades_and_lists_unmatched():
    stale = EditorialOverride(slide=9, dimension="2", reason="x", date="2026-10-15")
    out, applied, unmatched = apply_overrides(
        [_f("a:slide=3", "4", "major")], [OV, stale]
    )
    assert out[0]["severity"] == "nit"
    assert out[0]["rationale"].startswith("overridden: build photos replace sketch (2026-10-15)")
    assert applied == [OV] and unmatched == [stale]


def test_blocker_never_overridden():
    out, applied, _ = apply_overrides([_f("a:slide=3", "4", "blocker")], [OV])
    assert out[0]["severity"] == "blocker"
    assert "override ignored: critical" in out[0]["rationale"]
    assert applied == []


def _write_brief(tmp_path: Path, block: str) -> None:
    (tmp_path / "BRIEF.md").write_text(
        "---\nproject: demo\ndocuments:\n  - slug: talk\n    artifact_type: slides\n" + block + "---\n\nbody\n"
    )


def test_brief_parses_and_loads(tmp_path):
    _write_brief(
        tmp_path,
        "    editorial_overrides:\n"
        "      - {slide: 3, dimension: 4, reason: kept on purpose, date: 2026-10-15}\n",
    )
    got = load_editorial_overrides_for_slug(tmp_path, "talk")
    assert got == [EditorialOverride(slide=3, dimension="4", reason="kept on purpose", date="2026-10-15")]


def test_absent_key_and_slug_yield_empty(tmp_path):
    assert load_editorial_overrides_for_slug(tmp_path, "talk") == []
    _write_brief(tmp_path, "")
    assert load_editorial_overrides_for_slug(tmp_path, "talk") == []
    assert load_editorial_overrides_for_slug(tmp_path, "other") == []


@pytest.mark.parametrize(
    "entry",
    [
        "{slide: 3, dimension: 4, reason: '', date: 2026-10-15}",
        "{slide: 3, dimension: 4, date: 2026-10-15}",
        "{slide: 3, dimension: 4, reason: why}",
        "{slide: three, dimension: 4, reason: why, date: 2026-10-15}",
    ],
)
def test_invalid_entries_rejected(tmp_path, entry):
    from anvil.lib.project_brief import load_project_brief

    _write_brief(tmp_path, f"    editorial_overrides:\n      - {entry}\n")
    with pytest.raises(ValueError, match="editorial_overrides"):
        load_project_brief(tmp_path)
