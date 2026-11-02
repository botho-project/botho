"""Per-thread collection tests for `anvil:project-book` (issue #596)."""

from __future__ import annotations

import os

from _book_fixtures import make_thread
from _project_book_skill_lib import collect as CL


def test_empty_thread_no_dir(tmp_path):
    info = CL.collect_thread(tmp_path, "ghost", chapter_filename="chapter.tex")
    assert info.state == CL.STATE_EMPTY
    assert info.resolved_dir is None
    assert info.needs_placeholder is True
    assert info.next_command.endswith("draft")
    assert any("EMPTY" in w for w in info.warnings)


def test_empty_thread_dir_without_versions(tmp_path):
    make_thread(tmp_path, "appendix", version=None)
    info = CL.collect_thread(tmp_path, "appendix", chapter_filename="chapter.tex")
    assert info.state == CL.STATE_EMPTY
    assert info.needs_placeholder is True


def test_walk_to_highest_version(tmp_path):
    make_thread(tmp_path, "a", version=1, chapter=True)
    make_thread(tmp_path, "a", version=3, chapter=True)
    make_thread(tmp_path, "a", version=2, chapter=True)
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.resolved_name == "a.3"
    assert info.needs_placeholder is False


def test_pinned_symlink_wins_over_highest(tmp_path):
    make_thread(tmp_path, "a", version=1, chapter=True)
    make_thread(tmp_path, "a", version=2, chapter=True)
    # Pin .latest at v1 even though v2 exists.
    link = tmp_path / "a" / "a.latest"
    os.symlink("a.1", link)
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.resolved_dir.name == "a.1"


def test_state_from_progress_phases(tmp_path):
    make_thread(tmp_path, "a", version=1, chapter=True, phases=["draft", "review"])
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.state == CL.STATE_REVIEWED


def test_state_revised(tmp_path):
    make_thread(
        tmp_path, "a", version=1, chapter=True, phases=["draft", "review", "revise"]
    )
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.state == CL.STATE_REVISED


def test_missing_chapter_file_needs_placeholder(tmp_path):
    make_thread(tmp_path, "a", version=1, chapter=False)
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.needs_placeholder is True
    assert any("no `chapter.tex`" in w for w in info.warnings)


def test_score_from_review_sibling(tmp_path):
    make_thread(
        tmp_path,
        "a",
        version=1,
        chapter=True,
        phases=["draft", "review"],
        review_score=41,
        rubric_total=44,
        advance_threshold=39,
    )
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.score == 41
    assert info.rubric_total == 44
    assert info.advance_threshold == 39


def test_below_threshold_warning(tmp_path):
    make_thread(
        tmp_path,
        "a",
        version=1,
        chapter=True,
        phases=["draft", "review"],
        review_score=30,
        advance_threshold=39,
    )
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert any("below the advance threshold" in w for w in info.warnings)


def test_highest_review_sibling_chosen(tmp_path):
    make_thread(
        tmp_path, "a", version=1, chapter=True, review_score=20, advance_threshold=39
    )
    # Add a higher-N review sibling with a better score.
    make_thread(
        tmp_path, "a", version=2, chapter=True, review_score=42, advance_threshold=39
    )
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.score == 42


def test_clean_audit_promotes_state(tmp_path):
    make_thread(
        tmp_path,
        "a",
        version=1,
        chapter=True,
        phases=["draft", "review", "revise"],
        review_score=41,
        audit="clean",
    )
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.state == CL.STATE_AUDITED
    assert info.audit_state == "clean"
    assert info.next_command is None


def test_flagged_audit_does_not_promote(tmp_path):
    make_thread(
        tmp_path,
        "a",
        version=1,
        chapter=True,
        phases=["draft", "review"],
        review_score=41,
        audit="flagged",
    )
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.audit_state == "flagged"
    assert info.state == CL.STATE_REVIEWED


def test_custom_chapter_filename(tmp_path):
    make_thread(tmp_path, "a", version=1, chapter=True, chapter_filename="ch.tex")
    info = CL.collect_thread(tmp_path, "a", chapter_filename="ch.tex")
    assert info.needs_placeholder is False
    assert info.chapter_source.name == "ch.tex"


# --- `{slug}` chapter-filename templating (#864) -------------------------


def test_resolve_chapter_filename_substitutes_slug():
    assert CL.resolve_chapter_filename("{slug}.tex", "00-introduction") == (
        "00-introduction.tex"
    )


def test_resolve_chapter_filename_is_noop_without_token():
    """A literal filename passes through unchanged — no `.format()` blowup."""
    assert CL.resolve_chapter_filename("chapter.tex", "a") == "chapter.tex"
    assert CL.resolve_chapter_filename("ch{1}.tex", "a") == "ch{1}.tex"


def test_slug_template_resolves_slug_echo_chapter(tmp_path):
    """A memoir-shaped `<slug>.tex` body resolves under `{slug}.tex` (#864)."""
    make_thread(
        tmp_path,
        "00-introduction",
        version=1,
        chapter=True,
        chapter_filename="00-introduction.tex",
    )
    info = CL.collect_thread(
        tmp_path,
        "00-introduction",
        chapter_filename="{slug}.tex",
        artifact_type="memoir",
    )
    assert info.needs_placeholder is False
    assert info.chapter_source.name == "00-introduction.tex"
    assert not any("placeholder chapter was staged" in w for w in info.warnings)


def test_slug_template_missing_chapter_warns_with_resolved_name(tmp_path):
    """The missing-chapter warning names the slug-resolved filename."""
    make_thread(tmp_path, "01-childhood", version=1, chapter=False)
    info = CL.collect_thread(
        tmp_path, "01-childhood", chapter_filename="{slug}.tex"
    )
    assert info.needs_placeholder is True
    assert any("`01-childhood.tex`" in w for w in info.warnings)
    assert not any("{slug}" in w for w in info.warnings)


def test_literal_default_still_matches_chapter_tex(tmp_path):
    """Regression: the pre-#864 literal default is unaffected."""
    make_thread(tmp_path, "a", version=1, chapter=True)
    info = CL.collect_thread(tmp_path, "a", chapter_filename="chapter.tex")
    assert info.needs_placeholder is False
    assert info.chapter_source.name == "chapter.tex"
