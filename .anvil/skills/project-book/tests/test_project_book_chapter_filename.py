"""End-to-end `chapter_filename` templating tests for `anvil:project-book`.

Covers issue #864: `build.chapter_filename` is a per-thread template whose
`{slug}` token resolves to each thread's own slug, plus the zero-config
`artifact_type: memoir` -> `{slug}.tex` default. Driven through
`orchestrate.run(..., dry_run=True)` so no renderer is required.
"""

from __future__ import annotations

from _book_fixtures import default_documents, make_thread, write_brief
from _project_book_skill_lib import orchestrate as O


def _thread(result, slug):
    for info in result.threads:
        if info.slug == slug:
            return info
    raise AssertionError(f"no thread {slug!r} in result")


def _memoir_project(tmp_path, build=None):
    """One memoir thread carrying a slug-echo `<slug>.tex` body."""
    write_brief(
        tmp_path,
        project="walters-family-tree",
        documents=default_documents(["00-introduction"], artifact_type="memoir"),
        build=build,
    )
    make_thread(
        tmp_path,
        "00-introduction",
        version=3,
        chapter=True,
        chapter_filename="00-introduction.tex",
    )


def test_memoir_zero_config_resolves_slug_echo_chapter(tmp_path):
    """No `build:` block at all: memoir defaults to `{slug}.tex` (#864)."""
    _memoir_project(tmp_path)
    result = O.run(tmp_path, dry_run=True)
    info = _thread(result, "00-introduction")
    assert info.needs_placeholder is False
    assert info.chapter_source.name == "00-introduction.tex"
    assert not any("placeholder chapter was staged" in w for w in info.warnings)
    assert "has no `chapter.tex`" not in result.report


def test_memoir_explicit_slug_template_resolves(tmp_path):
    """An explicit `{slug}.tex` template resolves per-thread."""
    _memoir_project(tmp_path, build={"chapter_filename": "{slug}.tex"})
    info = _thread(O.run(tmp_path, dry_run=True), "00-introduction")
    assert info.needs_placeholder is False
    assert info.chapter_source.name == "00-introduction.tex"


def test_explicit_chapter_filename_not_overridden_for_memoir(tmp_path):
    """An explicit BRIEF value wins over the memoir default (#864)."""
    _memoir_project(tmp_path, build={"chapter_filename": "chapter.tex"})
    info = _thread(O.run(tmp_path, dry_run=True), "00-introduction")
    # The thread only carries `00-introduction.tex`, so pinning `chapter.tex`
    # must still produce a placeholder — the default is not silently applied.
    assert info.needs_placeholder is True
    assert any("`chapter.tex`" in w for w in info.warnings)


def test_non_memoir_zero_config_unchanged(tmp_path):
    """Regression: a non-memoir thread keeps the literal `chapter.tex`."""
    write_brief(
        tmp_path,
        project="p",
        documents=default_documents(["00-intro"], artifact_type="investment-memo"),
    )
    make_thread(tmp_path, "00-intro", version=1, chapter=True)
    info = _thread(O.run(tmp_path, dry_run=True), "00-intro")
    assert info.needs_placeholder is False
    assert info.chapter_source.name == "chapter.tex"


def test_mixed_project_resolves_each_type_independently(tmp_path):
    """One project, two artifact types, zero config — both resolve."""
    write_brief(
        tmp_path,
        project="mixed",
        documents=[
            {"slug": "00-introduction", "artifact_type": "memoir"},
            {"slug": "appendix", "artifact_type": "investment-memo"},
        ],
    )
    make_thread(
        tmp_path,
        "00-introduction",
        version=1,
        chapter=True,
        chapter_filename="00-introduction.tex",
    )
    make_thread(tmp_path, "appendix", version=1, chapter=True)
    result = O.run(tmp_path, dry_run=True)
    memoir_info = _thread(result, "00-introduction")
    memo_info = _thread(result, "appendix")
    assert memoir_info.needs_placeholder is False
    assert memoir_info.chapter_source.name == "00-introduction.tex"
    assert memo_info.needs_placeholder is False
    assert memo_info.chapter_source.name == "chapter.tex"


def test_memoir_stages_slug_echo_chapter_in_apply_mode(tmp_path):
    """Apply mode stages the real body, not a placeholder (#864)."""
    _memoir_project(tmp_path)
    result = O.run(tmp_path)
    staged = tmp_path / "book" / "chapters" / "00-introduction.tex"
    assert staged.is_file()
    assert "Real content for 00-introduction." in staged.read_text(encoding="utf-8")
    assert result.stage_result is not None
    assert result.stage_result.refused is False
