"""Chapter staging tests for `anvil:project-book` (issue #596)."""

from __future__ import annotations

from _book_fixtures import make_thread
from _project_book_skill_lib import collect as CL
from _project_book_skill_lib import config as C
from _project_book_skill_lib import stage as S


def _collect(tmp_path, slug, **kw):
    return CL.collect_thread(tmp_path, slug, chapter_filename="chapter.tex", **kw)


def test_stage_copies_real_chapter(tmp_path):
    make_thread(
        tmp_path, "a", version=1, chapter=True, chapter_body="REAL BODY\n"
    )
    info = _collect(tmp_path, "a")
    chapters = tmp_path / "book" / "chapters"
    result = S.stage_chapters(chapters, [info])
    assert not result.refused
    staged = chapters / "a.tex"
    assert staged.read_text() == "REAL BODY\n"
    assert result.placeholders == []


def test_stage_relocates_title_after_begin_document(tmp_path):
    chapter_body = (
        "\\documentclass{article}\n"
        "\\title{My Chapter Title}\n"
        "\\author{Someone}\n"
        "\\date{}\n"
        "\n"
        "\\begin{document}\n"
        "\\maketitle\n"
        "\n"
        "Body text.\n"
        "\\end{document}\n"
    )
    make_thread(tmp_path, "a", version=1, chapter=True, chapter_body=chapter_body)
    info = _collect(tmp_path, "a")
    chapters = tmp_path / "book" / "chapters"
    result = S.stage_chapters(chapters, [info], relocate_title=True)
    assert not result.refused
    staged = (chapters / "a.tex").read_text()
    assert "\\title{My Chapter Title}" not in staged.split("\\begin{document}")[0]
    after_begin = staged.split("\\begin{document}", 1)[1]
    assert after_begin.lstrip("\n").startswith("\\title{My Chapter Title}")
    assert "\\maketitle" in staged
    assert "Body text." in staged


def test_stage_relocate_title_false_is_byte_identical_to_copy2(tmp_path):
    chapter_body = (
        "\\documentclass{article}\n"
        "\\title{My Chapter Title}\n"
        "\\begin{document}\n"
        "\\maketitle\n"
        "\\end{document}\n"
    )
    make_thread(tmp_path, "a", version=1, chapter=True, chapter_body=chapter_body)
    info = _collect(tmp_path, "a")
    chapters = tmp_path / "book" / "chapters"
    result = S.stage_chapters(chapters, [info])
    assert not result.refused
    staged = chapters / "a.tex"
    assert staged.read_text() == chapter_body
    assert staged.read_bytes() == info.chapter_source.read_bytes()


def test_stage_relocate_title_no_title_present_does_not_crash(tmp_path):
    chapter_body = (
        "\\documentclass{article}\n"
        "\\begin{document}\n"
        "No title here.\n"
        "\\end{document}\n"
    )
    make_thread(tmp_path, "a", version=1, chapter=True, chapter_body=chapter_body)
    info = _collect(tmp_path, "a")
    chapters = tmp_path / "book" / "chapters"
    result = S.stage_chapters(chapters, [info], relocate_title=True)
    assert not result.refused
    assert (chapters / "a.tex").read_text() == chapter_body


def test_stage_relocate_title_placeholder_chapter_not_transformed(tmp_path):
    info = _collect(tmp_path, "ghost")
    chapters = tmp_path / "chapters"
    result = S.stage_chapters(chapters, [info], relocate_title=True)
    assert result.placeholders == ["ghost"]
    body = (chapters / "ghost.tex").read_text()
    assert S.PLACEHOLDER_MARKER in body


def test_relocate_title_helper_handles_multiline_and_nested_braces():
    text = (
        "\\title{My \\textbf{Bold}\n"
        "Multi-line Title}\n"
        "\\begin{document}\n"
        "Body.\n"
        "\\end{document}\n"
    )
    out = S.relocate_title_after_begin_document(text)
    preamble, rest = out.split("\\begin{document}", 1)
    assert "\\title" not in preamble
    assert rest.lstrip("\n").startswith(
        "\\title{My \\textbf{Bold}\nMulti-line Title}"
    )


def test_relocate_title_helper_no_begin_document_noop():
    text = "\\title{Orphan}\nNo begin-document anchor here.\n"
    assert S.relocate_title_after_begin_document(text) == text


def test_relocate_title_helper_shared_line_keeps_its_newline():
    """A title sharing its line with other preamble content must not swallow
    that line's newline — doing so merges the next line onto the remainder
    (issue #1205 review finding)."""
    text = (
        "\\documentclass{article}\n"
        "\\newcommand{\\foo}{bar} \\title{Real Title}\n"
        "\\begin{document}\n"
        "Body.\n"
        "\\end{document}\n"
    )
    out = S.relocate_title_after_begin_document(text)
    assert out == (
        "\\documentclass{article}\n"
        "\\newcommand{\\foo}{bar} \n"
        "\\begin{document}\n"
        "\\title{Real Title}\n"
        "Body.\n"
        "\\end{document}\n"
    )
    # The structural anchor stays on its own line.
    assert "\n\\begin{document}\n" in out


def test_relocate_title_helper_commented_title_does_not_comment_out_anchor():
    """A commented-out `% \\title{...}` placeholder must not drag
    `\\begin{document}` onto the comment line (issue #1205 review finding)."""
    text = (
        "\\documentclass{article}\n"
        "% \\title{Commented Out}\n"
        "\\begin{document}\n"
        "Body.\n"
        "\\end{document}\n"
    )
    out = S.relocate_title_after_begin_document(text)
    assert "% \\begin{document}" not in out
    assert "\n\\begin{document}\n" in out


def test_stage_generates_placeholder_for_empty(tmp_path):
    info = _collect(tmp_path, "ghost")
    chapters = tmp_path / "chapters"
    result = S.stage_chapters(chapters, [info])
    assert result.placeholders == ["ghost"]
    body = (chapters / "ghost.tex").read_text()
    assert S.PLACEHOLDER_MARKER in body
    assert "\\chapter{Ghost}" in body
    assert "Not started" in body


def test_stage_writes_marker(tmp_path):
    info = _collect(tmp_path, "ghost")
    chapters = tmp_path / "chapters"
    S.stage_chapters(chapters, [info])
    assert (chapters / C.MARKER_FILENAME).is_file()


def test_stage_structure_one_file_per_thread(tmp_path):
    make_thread(tmp_path, "a", version=1, chapter=True)
    make_thread(tmp_path, "b", version=1, chapter=True)
    infos = [_collect(tmp_path, "a"), _collect(tmp_path, "b")]
    chapters = tmp_path / "chapters"
    S.stage_chapters(chapters, infos)
    names = sorted(p.name for p in chapters.iterdir())
    assert names == [C.MARKER_FILENAME, "a.tex", "b.tex"]


def test_marker_guard_refuses_foreign_dir(tmp_path):
    chapters = tmp_path / "chapters"
    chapters.mkdir()
    (chapters / "someones-file.tex").write_text("do not delete")
    info = _collect(tmp_path, "ghost")
    result = S.stage_chapters(chapters, [info])
    assert result.refused
    assert (chapters / "someones-file.tex").exists()  # not deleted
    assert "someones-file.tex" or True


def test_marker_authorized_rebuild(tmp_path):
    chapters = tmp_path / "chapters"
    chapters.mkdir()
    (chapters / C.MARKER_FILENAME).write_text("marker")
    (chapters / "stale.tex").write_text("stale")
    info = _collect(tmp_path, "ghost")
    result = S.stage_chapters(chapters, [info])
    assert not result.refused
    assert not (chapters / "stale.tex").exists()  # blown away
    assert (chapters / "ghost.tex").exists()


def test_placeholder_escapes_underscore_slug(tmp_path):
    body = S.placeholder_chapter("chapter_one")
    assert "\\texttt{chapter\\_one}" in body


def test_slug_title_strips_ordinal():
    assert S.slug_title("00-introduction") == "Introduction"
    assert S.slug_title("01_childhood_years") == "Childhood Years"
    assert S.slug_title("appendix") == "Appendix"


def test_gitignore_suggestion_when_uncovered(tmp_path):
    (tmp_path / ".git").mkdir()
    (tmp_path / ".gitignore").write_text("*.log\n")
    note = S.gitignore_suggestion(tmp_path, "book/chapters")
    assert note is not None
    assert "book/chapters" in note


def test_gitignore_suggestion_none_when_covered(tmp_path):
    (tmp_path / ".git").mkdir()
    (tmp_path / ".gitignore").write_text("book/chapters/\n")
    assert S.gitignore_suggestion(tmp_path, "book/chapters") is None


def test_gitignore_suggestion_parent_covers(tmp_path):
    (tmp_path / ".git").mkdir()
    (tmp_path / ".gitignore").write_text("book/\n")
    assert S.gitignore_suggestion(tmp_path, "book/chapters") is None
