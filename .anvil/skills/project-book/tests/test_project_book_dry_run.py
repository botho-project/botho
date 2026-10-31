"""Dry-run side-effect-free tests for `anvil:project-book` (issue #596)."""

from __future__ import annotations

from anvil.lib.testing import tree_hash as _tree_hashes
from _book_fixtures import default_documents, make_thread, write_brief
from _project_book_skill_lib import config as C
from _project_book_skill_lib import orchestrate as O


def _build_project(tmp_path):
    slugs = ["00-intro", "01-mid", "appendix"]
    write_brief(
        tmp_path,
        project="example-memoir",
        documents=default_documents(slugs),
        build={
            "order": slugs,
            "master_doc": "book/book.tex",
            "chapters_dir": "book/chapters",
        },
    )
    make_thread(tmp_path, "00-intro", version=2, chapter=True, review_score=41, audit="clean")
    make_thread(tmp_path, "01-mid", version=1, chapter=True, review_score=30)
    make_thread(tmp_path, "appendix", version=None)
    book = tmp_path / "book"
    book.mkdir()
    (book / "book.tex").write_text(
        "\\documentclass{article}\\begin{document}\\end{document}\n"
    )


def test_dry_run_writes_nothing(tmp_path):
    _build_project(tmp_path)
    before = _tree_hashes(tmp_path)
    result = O.run(tmp_path, dry_run=True)
    after = _tree_hashes(tmp_path)
    assert before == after
    assert result.report_path is None
    assert not (tmp_path / C.REPORT_FILENAME).exists()
    assert not (tmp_path / "book" / "chapters").exists()


def test_dry_run_report_has_full_plan(tmp_path):
    _build_project(tmp_path)
    result = O.run(tmp_path, dry_run=True)
    assert "## Chapters" in result.report
    assert "`00-intro`" in result.report
    assert "`appendix`" in result.report
    assert result.success is True


def test_dry_run_no_writes_even_with_existing_chapters(tmp_path):
    _build_project(tmp_path)
    # A pre-existing (marked) chapters dir must be left untouched by dry-run.
    chapters = tmp_path / "book" / "chapters"
    chapters.mkdir(parents=True)
    (chapters / C.MARKER_FILENAME).write_text("marker\n")
    (chapters / "old.tex").write_text("old\n")
    before = _tree_hashes(tmp_path)
    O.run(tmp_path, dry_run=True)
    assert _tree_hashes(tmp_path) == before
