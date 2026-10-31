"""Tests for ``anvil.skills.deck.lib.text_layer_completeness`` (issue #983).

Neither existing deterministic gate catches a caption pushed off a slide's
bottom edge: ``marp_lint`` (source-side, step 5b) estimates vertical cost
*before* render and can under-charge an image-heavy slide; ``auto_shrink_
detector`` (post-render, step 5c) is a peer-relative bbox rule that cannot
see clipping when the clipped page's margins happen to read "normal"
against its class (and never fires on a singleton `_class:` by design).
This module closes the gap with a ground-truth check: does the slide's
last source line actually survive into its rendered PDF page's text layer?

These tests stub ``pdftotext`` via a tiny generated executable script
(``_make_fake_pdftotext``) rather than depending on a real poppler-utils
install — mirrors the ``fake_pdftotext_*.sh`` fixture precedent in
``tests/lib/test_render_gate.py`` (issue #692), except generated at
test-time (no committed binary/script fixtures) per the ``auto_shrink``
"synthetic-only, built at test-time" convention documented in
``test_auto_shrink_detector.py``.

Test matrix (per the issue #983 test plan):

- ``TestCleanDeck`` — (a) every slide's last source line is present in its
  rendered page's text layer → zero findings, including a multi-slide,
  multi-caption deck.
- ``TestClippedCaption`` — (b) the motivating failure case: an italic
  caption pushed off the slide bottom → one deterministic ``error``
  finding naming the slide and the missing line. Also covers the
  multi-line-caption case (only the LAST physical line clips).
- ``TestEscapeHatch`` — (c) ``<!-- anvil-lint-disable: text-layer-
  completeness -->`` downgrades the same finding to ``info``.
- ``TestGracefulSkip`` — (d) missing ``pdftotext`` on ``PATH`` and a
  not-yet-rendered ``deck.pdf`` both produce a recorded (never silent)
  skip.
- ``TestLastCheckableSourceLine`` — the trailing-directive-comment edge
  case (a slide whose literal last line is an HTML comment/directive must
  not be treated as content to verify) and the image-only-line case.
- Doc-coverage tests pin ``deck-review.md``'s step 5f wiring, including
  the module-``ImportError`` "record, don't silently skip" contract (e).

Runs under either ``python -m unittest discover anvil/skills/deck/tests/``
or ``pytest anvil/skills/deck/tests/``.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from anvil.skills.deck.lib.text_layer_completeness import (
    Finding,
    RULE_ID,
    TextLayerCompletenessResult,
    _last_checkable_source_line,
    check_text_layer_completeness,
)


# ---------------------------------------------------------------------------
# Test helpers
# ---------------------------------------------------------------------------


def _make_fake_pdftotext(tmp_path: Path, pages: list[str], *, name: str = "fake_pdftotext.py") -> str:
    """Write a tiny stub ``pdftotext`` executable that prints ``pages``.

    Mirrors poppler's default per-page splitting behaviour: each page's
    text is separated by a form-feed (``\\x0c``), with a trailing form-feed
    after the last page. Ignores its argv (the module always invokes
    ``<exe> <pdf> -``).
    """
    script = tmp_path / name
    payload = "\x0c".join(pages) + "\x0c"
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        f"sys.stdout.write({payload!r})\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return str(script)


def _write_deck(tmp_path: Path, source: str, *, name: str = "deck.md") -> Path:
    p = tmp_path / name
    p.write_text(source, encoding="utf-8")
    return p


_CLEAN_DECK_SOURCE = """# Slide 1

Some content.

---

![figure](figures/fig1.png)

*Figure 1: caption text.*

---

## Slide 3

Bullet one
Bullet two
"""

_CLEAN_DECK_PAGES = [
    "Slide 1\n\nSome content.",
    "Figure 1: caption text.",
    "Slide 3\n\nBullet one\nBullet two",
]


# ---------------------------------------------------------------------------
# (a) Clean deck — zero findings
# ---------------------------------------------------------------------------


class TestCleanDeck:
    def test_every_slide_last_line_present_zero_findings(self, tmp_path: Path) -> None:
        deck_md = _write_deck(tmp_path, _CLEAN_DECK_SOURCE)
        deck_pdf = tmp_path / "deck.pdf"
        deck_pdf.write_bytes(b"%PDF-stub")
        fake_pdftotext = _make_fake_pdftotext(tmp_path, _CLEAN_DECK_PAGES)

        result = check_text_layer_completeness(
            deck_pdf=deck_pdf, deck_md=deck_md, pdftotext_path=fake_pdftotext
        )

        assert isinstance(result, TextLayerCompletenessResult)
        assert not result.skipped
        assert result.findings == []
        assert result.to_dict()["errors"] == 0

    def test_deck_with_zero_images_still_reports_zero_findings(
        self, tmp_path: Path
    ) -> None:
        """Decks with no images/captions must report zero findings, not skip."""
        source = "# Title\n\nJust text.\n\n---\n\n# Second\n\nMore text.\n"
        deck_md = _write_deck(tmp_path, source)
        deck_pdf = tmp_path / "deck.pdf"
        deck_pdf.write_bytes(b"%PDF-stub")
        fake_pdftotext = _make_fake_pdftotext(
            tmp_path, ["Title\n\nJust text.", "Second\n\nMore text."]
        )

        result = check_text_layer_completeness(
            deck_pdf=deck_pdf, deck_md=deck_md, pdftotext_path=fake_pdftotext
        )

        assert not result.skipped
        assert result.findings == []


# ---------------------------------------------------------------------------
# (b) Clipped caption — the motivating failure case
# ---------------------------------------------------------------------------


class TestClippedCaption:
    def test_caption_pushed_off_bottom_fires_error(self, tmp_path: Path) -> None:
        deck_md = _write_deck(tmp_path, _CLEAN_DECK_SOURCE)
        deck_pdf = tmp_path / "deck.pdf"
        deck_pdf.write_bytes(b"%PDF-stub")
        # Slide 2's rendered page is missing the caption entirely — the
        # figure rendered but the caption clipped off the bottom edge.
        clipped_pages = [
            _CLEAN_DECK_PAGES[0],
            "",  # figure image carries no extractable text; caption absent
            _CLEAN_DECK_PAGES[2],
        ]
        fake_pdftotext = _make_fake_pdftotext(tmp_path, clipped_pages)

        result = check_text_layer_completeness(
            deck_pdf=deck_pdf, deck_md=deck_md, pdftotext_path=fake_pdftotext
        )

        assert not result.skipped
        assert len(result.findings) == 1, [f.to_dict() for f in result.findings]
        f = result.findings[0]
        assert isinstance(f, Finding)
        assert f.slide == 2
        assert f.rule == RULE_ID
        assert f.severity == "error"
        assert "Figure 1: caption text." in f.message
        assert result.errors == [f]

    def test_multiline_caption_only_last_physical_line_checked(
        self, tmp_path: Path
    ) -> None:
        """A soft-wrapped multi-line caption: only the LAST source line is
        the checked "last text line" — an earlier line surviving the render
        does not save a clipped final line."""
        source = (
            "# Slide 1\n\n"
            "![figure](figures/fig1.png)\n\n"
            "*This is the first line of a long caption that wraps across*\n"
            "*two physical source lines and only the second one clips.*\n"
        )
        deck_md = _write_deck(tmp_path, source)
        deck_pdf = tmp_path / "deck.pdf"
        deck_pdf.write_bytes(b"%PDF-stub")
        # The page carries the first caption line but not the second.
        page_text = "This is the first line of a long caption that wraps across"
        fake_pdftotext = _make_fake_pdftotext(tmp_path, [page_text])

        result = check_text_layer_completeness(
            deck_pdf=deck_pdf, deck_md=deck_md, pdftotext_path=fake_pdftotext
        )

        assert not result.skipped
        assert len(result.findings) == 1
        assert result.findings[0].slide == 1
        assert "two physical source lines and only the second one clips." in (
            result.findings[0].message
        )


# ---------------------------------------------------------------------------
# (c) Escape hatch
# ---------------------------------------------------------------------------


class TestEscapeHatch:
    def test_disable_directive_downgrades_to_info(self, tmp_path: Path) -> None:
        source = (
            "# Slide 1\n\n"
            "Some content.\n\n"
            "---\n\n"
            "<!-- anvil-lint-disable: text-layer-completeness -->\n\n"
            "![figure](figures/fig1.png)\n\n"
            "*Figure 1: caption text.*\n"
        )
        deck_md = _write_deck(tmp_path, source)
        deck_pdf = tmp_path / "deck.pdf"
        deck_pdf.write_bytes(b"%PDF-stub")
        fake_pdftotext = _make_fake_pdftotext(
            tmp_path, ["Slide 1\n\nSome content.", ""]
        )

        result = check_text_layer_completeness(
            deck_pdf=deck_pdf, deck_md=deck_md, pdftotext_path=fake_pdftotext
        )

        assert not result.skipped
        assert len(result.findings) == 1
        f = result.findings[0]
        assert f.slide == 2
        assert f.severity == "info"
        assert result.errors == []
        assert result.infos == [f]


# ---------------------------------------------------------------------------
# (d) Graceful skip
# ---------------------------------------------------------------------------


class TestGracefulSkip:
    def test_skips_when_pdftotext_absent_from_path(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        deck_md = _write_deck(tmp_path, _CLEAN_DECK_SOURCE)
        deck_pdf = tmp_path / "deck.pdf"
        deck_pdf.write_bytes(b"%PDF-stub")

        monkeypatch.setattr(
            "anvil.skills.deck.lib.text_layer_completeness.shutil.which",
            lambda name: None,
        )

        result = check_text_layer_completeness(deck_pdf=deck_pdf, deck_md=deck_md)

        assert result.skipped is True
        assert result.findings == []
        assert "pdftotext" in (result.reason or "")
        d = result.to_dict()
        assert d["ran"] is False
        assert d["skipped"] is True

    def test_skips_when_deck_pdf_missing(self, tmp_path: Path) -> None:
        deck_md = _write_deck(tmp_path, _CLEAN_DECK_SOURCE)
        deck_pdf = tmp_path / "deck.pdf"  # never created

        result = check_text_layer_completeness(
            deck_pdf=deck_pdf, deck_md=deck_md, pdftotext_path="/usr/bin/pdftotext"
        )

        assert result.skipped is True
        assert result.findings == []
        assert "deck.pdf" in (result.reason or "")
        assert "deck-figures" in (result.reason or "")


# ---------------------------------------------------------------------------
# Last-checkable-source-line extraction edge cases
# ---------------------------------------------------------------------------


class TestLastCheckableSourceLine:
    def test_trailing_directive_comment_is_skipped(self) -> None:
        slide_raw = (
            "*Figure 1: a real caption.*\n"
            "<!-- anvil-lint-disable: slide-content-overflow -->\n"
        )
        assert _last_checkable_source_line(slide_raw) == "Figure 1: a real caption."

    def test_image_only_trailing_line_is_skipped(self) -> None:
        slide_raw = (
            "Some real text here.\n\n"
            "![](figures/decorative.png)\n"
        )
        assert _last_checkable_source_line(slide_raw) == "Some real text here."

    def test_all_directive_slide_returns_none(self) -> None:
        slide_raw = "<!-- _class: title -->\n<!-- anvil-lint-disable: slide-content-overflow -->\n"
        assert _last_checkable_source_line(slide_raw) is None


# ---------------------------------------------------------------------------
# Doc-coverage: deck-review.md must wire this gate in
# ---------------------------------------------------------------------------


_SKILL_ROOT = Path(__file__).resolve().parent.parent  # anvil/skills/deck/

DECK_REVIEW_MD = _SKILL_ROOT / "commands" / "deck-review.md"
SKILL_MD = _SKILL_ROOT / "SKILL.md"


def test_deck_review_md_wires_text_layer_completeness_gate() -> None:
    text = DECK_REVIEW_MD.read_text(encoding="utf-8")
    assert "text_layer_completeness" in text
    assert "check_text_layer_completeness" in text
    assert "text-layer-completeness" in text


def test_deck_review_md_documents_graceful_skip() -> None:
    text = DECK_REVIEW_MD.read_text(encoding="utf-8")
    assert "pdftotext" in text
    # Both graceful-skip cases (missing binary, missing deck.pdf) named.
    assert "deck.pdf not found" in text or "deck.pdf" in text


def test_deck_review_md_documents_escape_hatch() -> None:
    text = DECK_REVIEW_MD.read_text(encoding="utf-8")
    assert "anvil-lint-disable: text-layer-completeness" in text


def test_deck_review_md_documents_import_error_contract() -> None:
    """The module-``ImportError`` path must record an info finding, not
    silently skip (same contract as steps 5b/5c/5d) — issue #983 (e)."""
    text = DECK_REVIEW_MD.read_text(encoding="utf-8")
    idx = text.find("text_layer_completeness")
    assert idx != -1
    # Scan the region around the gate's wiring for the ImportError clause.
    region = text[idx : idx + 6000]
    assert "ImportError" in region
    assert "module not importable" in region


def test_deck_review_md_ors_into_critical_flag() -> None:
    text = DECK_REVIEW_MD.read_text(encoding="utf-8")
    assert "lint.text_layer_completeness.errors" in text


def test_deck_review_md_findings_subsection_documented() -> None:
    text = DECK_REVIEW_MD.read_text(encoding="utf-8")
    assert "Text-layer-completeness findings" in text


def test_skill_md_mentions_new_gate() -> None:
    text = SKILL_MD.read_text(encoding="utf-8")
    assert "marp_lint" in text  # sanity: the existing pattern this mirrors
    assert "text_layer_completeness" in text or "text-layer-completeness" in text
