"""Config parsing tests for `anvil:project-book` (issue #596)."""

from __future__ import annotations

import pytest
from _book_fixtures import default_documents, write_brief
from _project_book_skill_lib import config as C


def test_zero_config_defaults(tmp_path):
    write_brief(tmp_path, project="p", documents=default_documents(["a"]))
    cfg = C.load_book_config(tmp_path)
    assert cfg.order is None
    assert cfg.master_doc is None
    assert cfg.chapters_dir == C.DEFAULT_CHAPTERS_DIR
    assert cfg.chapter_filename == C.DEFAULT_CHAPTER_FILENAME
    assert cfg.resolved_out_pdf() == "book/book.pdf"
    assert cfg.relocate_title is False


def test_relocate_title_parses_from_brief(tmp_path):
    write_brief(
        tmp_path,
        project="p",
        documents=default_documents(["a"]),
        build={"relocate_title": True},
    )
    cfg = C.load_book_config(tmp_path)
    assert cfg.relocate_title is True


def test_full_build_block(tmp_path):
    build = {
        "order": ["00-intro", "01-mid", "appendix"],
        "master_doc": "book/book.tex",
        "chapters_dir": "book/chapters",
        "chapter_filename": "chapter.tex",
        "out_pdf": "book/out.pdf",
    }
    write_brief(
        tmp_path, project="p", documents=default_documents(["00-intro"]), build=build
    )
    cfg = C.load_book_config(tmp_path)
    assert cfg.order == ["00-intro", "01-mid", "appendix"]
    assert cfg.master_doc == "book/book.tex"
    assert cfg.resolved_out_pdf() == "book/out.pdf"


def test_out_pdf_default_derives_from_chapters_dir():
    assert C.BookConfig(chapters_dir="book/chapters").resolved_out_pdf() == "book/book.pdf"
    assert C.BookConfig(chapters_dir="chapters").resolved_out_pdf() == "book.pdf"
    assert C.BookConfig(chapters_dir="deep/nest/chapters").resolved_out_pdf() == "deep/nest/book.pdf"


def test_missing_brief_raises(tmp_path):
    with pytest.raises(FileNotFoundError):
        C.load_book_config(tmp_path)


def test_unknown_build_key_rejected(tmp_path):
    write_brief(
        tmp_path,
        project="p",
        documents=default_documents(["a"]),
        build={"bogus_key": 1},
    )
    with pytest.raises(ValueError):
        C.load_book_config(tmp_path)


def test_build_block_not_a_mapping(tmp_path):
    write_brief(
        tmp_path,
        project="p",
        documents=default_documents(["a"]),
        build=["not", "a", "mapping"],
    )
    with pytest.raises(ValueError):
        C.load_book_config(tmp_path)


def test_order_must_be_list():
    with pytest.raises(Exception):
        C.BookConfig(order="not-a-list")


def test_order_rejects_duplicates_and_empties():
    with pytest.raises(Exception):
        C.BookConfig(order=["a", "a"])
    with pytest.raises(Exception):
        C.BookConfig(order=["a", ""])


def test_chapter_filename_rejects_separators():
    with pytest.raises(Exception):
        C.BookConfig(chapter_filename="sub/chapter.tex")


def test_chapter_filename_accepts_slug_token():
    """`{slug}.tex` is a valid bare filename template (#864)."""
    cfg = C.BookConfig(chapter_filename="{slug}.tex")
    assert cfg.chapter_filename == "{slug}.tex"


def test_slug_token_filename_parses_from_brief(tmp_path):
    write_brief(
        tmp_path,
        project="p",
        documents=default_documents(["a"], artifact_type="memoir"),
        build={"chapter_filename": "{slug}.tex"},
    )
    cfg = C.load_book_config(tmp_path)
    assert cfg.chapter_filename == "{slug}.tex"
    assert cfg.chapter_filename_for("memoir") == "{slug}.tex"


def test_chapter_filename_for_memoir_defaults_to_slug_echo():
    """Zero-config: a memoir document gets the slug-echo template (#864)."""
    cfg = C.BookConfig()
    assert cfg.chapter_filename_for("memoir") == C.SLUG_ECHO_CHAPTER_FILENAME
    assert cfg.chapter_filename_for("investment-memo") == C.DEFAULT_CHAPTER_FILENAME
    assert cfg.chapter_filename_for(None) == C.DEFAULT_CHAPTER_FILENAME


def test_explicit_chapter_filename_wins_over_memoir_default():
    """An explicit BRIEF value is honored verbatim, memoir included (#864)."""
    cfg = C.BookConfig(chapter_filename="ch.tex")
    assert cfg.chapter_filename_for("memoir") == "ch.tex"
    # Explicitly spelling out the framework default must NOT be re-read as
    # "unset" — `model_fields_set`, not value equality, decides.
    pinned = C.BookConfig(chapter_filename=C.DEFAULT_CHAPTER_FILENAME)
    assert pinned.chapter_filename_for("memoir") == C.DEFAULT_CHAPTER_FILENAME


def test_explicit_chapter_filename_from_brief_wins_for_memoir(tmp_path):
    write_brief(
        tmp_path,
        project="p",
        documents=default_documents(["a"], artifact_type="memoir"),
        build={"chapter_filename": "chapter.tex"},
    )
    cfg = C.load_book_config(tmp_path)
    assert cfg.chapter_filename_for("memoir") == "chapter.tex"


def test_chapters_dir_rejects_traversal_and_absolute():
    with pytest.raises(Exception):
        C.BookConfig(chapters_dir="../escape")
    with pytest.raises(Exception):
        C.BookConfig(chapters_dir="/abs/path")


def test_master_doc_and_out_pdf_reject_absolute():
    with pytest.raises(Exception):
        C.BookConfig(master_doc="/abs/book.tex")
    with pytest.raises(Exception):
        C.BookConfig(out_pdf="/abs/book.pdf")


def test_backslash_paths_normalized():
    cfg = C.BookConfig(chapters_dir="book\\chapters")
    assert cfg.chapters_dir == "book/chapters"
