"""Deterministic text-layer-completeness gate for the deck skill (issue #983).

Across a real deck thread, the most serious render defect encountered — an
italic caption below a height-uncapped figure pushed past the slide's
bottom edge and absent from the rendered PDF — was invisible to **both**
existing deterministic gates:

- ``marp_lint.py`` (source-side, ``deck-review.md`` step 5b) estimates
  vertical content cost from the markdown source *before* render. Its
  capacity model can under-charge a keyword-less image reference relative
  to the theme's actual rendered cap, so the source-side estimate can pass
  a page whose true rendered bottom edge sits past the safe area.
- ``auto_shrink_detector.py`` (post-render, ``deck-review.md`` step 5c)
  compares each slide's rendered content bbox against **peer-class
  medians** — it cannot see bottom-edge clipping at all when the clipped
  page's margins happen to look "normal" relative to its class (and it
  structurally never flags a singleton `_class:`, by design — the D4
  tautology contract, issue #965).

Neither gate asks the one ground-truth question that actually catches the
defect: **does the rendered PDF page's text layer contain the slide's
last source line?** A caption pushed off the bottom of a 720px slide
answers "no" — and unlike ``auto_shrink_detector``'s peer-relative rule,
this check needs no peer class at all, so it works uniformly on a
singleton `_class:` slide (e.g. `title`) the same as any other.

How it works
------------

1. Split ``deck.md`` into per-slide source chunks by reusing
   ``anvil.lib.marp_lint._split_slides`` — the same ``---``-based
   slide-splitting/frontmatter-handling logic the source-side lint uses,
   rather than re-deriving it (issue #983 explicitly calls this out; unlike
   ``auto_shrink_detector``'s independently-evolving ``_SLIDE_BREAK_RE``
   mirror, the slide→page correspondence here must stay byte-identical to
   what the source-side lint numbers as "slide N", so importing directly
   is the correct call).
2. For each slide, walk its source lines from the bottom up to find the
   last **checkable** line: skip blank lines, skip Marp/anvil directive
   comments (`<!-- _class: ... -->`, `<!-- anvil-lint-disable: ... -->`,
   etc. — issue #983's edge case explicitly excludes these), strip
   markdown decoration (image/link syntax, emphasis markers, leading list
   bullets) from each candidate, and use the first surviving line that
   still carries real (alphanumeric) text. A slide with no checkable line
   (image-only, directive-only) is skipped — nothing to verify.
3. Extract the rendered ``deck.pdf``'s per-page text via ``pdftotext``
   (poppler-utils), splitting on the form-feed page separator poppler
   emits by default. Mirrors the ``_which_pdftotext`` / ``_extract_pdf_text``
   precedent in ``anvil/lib/render_gate.py`` (~line 826 / 840), including
   its Unicode-whitespace-normalization contract: poppler's ``pdftotext``
   collapses Unicode ``Zs``-category separator spaces (NBSP, thin space,
   narrow NBSP, …) to a plain ASCII space during extraction, so the same
   fold is applied to the *source* side before comparing (see
   ``_normalize_whitespace``).
4. Confirm the normalized last-line text appears (case-insensitively) as a
   substring of the normalized page text. Absence emits a deterministic
   ``error``-severity finding naming the slide and the missing line — no
   LLM reviewer needed to notice.

Escape hatch
------------

``<!-- anvil-lint-disable: text-layer-completeness -->`` anywhere on the
slide downgrades that slide's finding to ``info`` (same whole-slide
suppression contract as ``slide-content-overflow``). Reuses
``anvil.lib.marp_lint._collect_disabled_rules`` — a generic, rule-name-
driven parser already shared across the ``marp_lint`` rule family, not a
check-specific heuristic, so reusing it (rather than re-deriving the
directive regex) carries no independent-evolution risk.

Graceful skip
--------------

Missing ``pdftotext`` on ``PATH``, a not-yet-rendered ``deck.pdf``, or a
missing ``deck.md`` each produce ``TextLayerCompletenessResult(skipped=True,
reason=...)`` — never a silent no-op. ``deck-review.md`` records the skip at
``info`` severity and proceeds; the rest of the review is unaffected. Same
contract as ``auto_shrink_detector.detect_auto_shrink``'s missing-deps /
missing-PDF skip paths.

Why skill-local (not ``anvil/lib/``)
-------------------------------------

This is only the **second** consumer of "extract a PDF page's text via
``pdftotext`` and compare against source content" — the first being
``anvil/lib/render_gate.py``'s whole-document glyph-verification check for
the LaTeX skills (``report`` / ``paper`` / ``spec`` / ``primer``). That
check compares *codepoint counts* across an entire document; this one
compares a *specific line's presence* on a *specific page* — different
enough in shape (per-page splitting, markdown-decoration stripping, a
different escape-hatch contract) that a shared helper would need real
generalization work, not a one-line lift. Per the "wait for the second
consumer before generalizing" convention (root ``CLAUDE.md``), this ships
skill-local first; promoting the bare ``pdftotext``-invocation primitive
(``_which_pdftotext`` / page-splitting extraction) to
``anvil/lib/render.py`` is mechanical follow-up work if a third consumer
materializes.

Wiring
------

Called from ``anvil/skills/deck/commands/deck-review.md`` step 5f, the
third deterministic pre-flight gate (alongside the step 5b source-side
``marp_lint`` estimate and the step 5c post-render ``auto_shrink_detector``
bbox check). Findings join ``_summary.md``'s ``lint`` block under a new
``text_layer_completeness`` sub-key. Any ``severity="error"`` finding ORs
into ``lint_critical_flag`` alongside the other two gates.

Public API
----------

``check_text_layer_completeness(deck_pdf, deck_md) -> TextLayerCompletenessResult``
    The public entry point. Both arguments are paths.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from anvil.lib.marp_lint import Finding, _collect_disabled_rules, _split_slides

#: Rule identifier emitted by this gate; also the escape-hatch directive
#: name: ``<!-- anvil-lint-disable: text-layer-completeness -->``.
RULE_ID: str = "text-layer-completeness"


# --- result type --------------------------------------------------------------


@dataclass
class TextLayerCompletenessResult:
    """All findings + the skipped/reason channel.

    ``skipped=True`` means the gate deliberately did NOT run for the whole
    deck — ``pdftotext`` missing from ``PATH``, ``deck.pdf`` not yet
    rendered, or ``deck.md`` missing. In that case ``findings`` is empty
    and ``reason`` carries the explanation (never a silent no-op).

    Shape mirrors ``anvil.skills.deck.lib.auto_shrink_detector.AutoShrinkResult``
    (single ``findings`` list + ``skipped`` + ``reason``); individual
    findings reuse ``anvil.lib.marp_lint.Finding`` (the same type
    ``figure_legibility.py`` reuses) since this gate — like
    ``slide-content-overflow`` — only ever emits two severities
    (``error``, or ``info`` when suppressed by the escape hatch).
    """

    findings: list[Finding] = field(default_factory=list)
    skipped: bool = False
    reason: Optional[str] = None

    @property
    def errors(self) -> list[Finding]:
        return [f for f in self.findings if f.severity == "error"]

    @property
    def warnings(self) -> list[Finding]:
        return [f for f in self.findings if f.severity == "warning"]

    @property
    def infos(self) -> list[Finding]:
        return [f for f in self.findings if f.severity == "info"]

    def to_dict(self) -> dict:
        return {
            "ran": not self.skipped,
            "skipped": self.skipped,
            "reason": self.reason,
            "errors": len(self.errors),
            "warnings": len(self.warnings),
            "infos": len(self.infos),
            "findings": [f.to_dict() for f in self.findings],
        }


# --- pdftotext extraction -----------------------------------------------------
#
# Mirrors the ``_which_pdftotext`` / ``_extract_pdf_text`` precedent in
# ``anvil/lib/render_gate.py`` (~line 826 / 840): a ``shutil.which``
# resolver plus a subprocess call to stdout, both graceful-degrading to
# ``None`` (never raising) on a missing binary / missing PDF / subprocess
# failure. This module additionally splits the extraction into per-page
# text since the completeness check is inherently per-slide.


def _which_pdftotext(override: Optional[str] = None) -> Optional[str]:
    """Resolve the ``pdftotext`` executable path, honoring the override."""
    if override is not None:
        return override
    return shutil.which("pdftotext")


def _extract_pdf_page_texts(
    pdf_path: Path, *, pdftotext_path: Optional[str] = None
) -> Optional[list[str]]:
    """Return one text string per PDF page, or ``None`` on failure.

    ``pdftotext <pdf> -`` extracts to stdout and inserts a form-feed
    (``\\x0c``) after every page by default (poppler behaviour, no
    ``-nopgbrk``) — splitting on that byte yields the per-page text
    without a second subprocess call per page. Surfaces ``None`` (not
    raise) when ``pdftotext`` is unavailable, the PDF is missing, or the
    subprocess fails — the same graceful-degrade contract
    ``render_gate._extract_pdf_text`` uses.
    """
    exe = _which_pdftotext(pdftotext_path)
    if exe is None or not pdf_path.exists():
        return None
    try:
        proc = subprocess.run(
            [exe, str(pdf_path), "-"],
            capture_output=True,
            text=True,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    pages = proc.stdout.split("\x0c")
    # A trailing form-feed after the last page produces one extra empty
    # trailing element; drop it so len(pages) == page count.
    if pages and pages[-1] == "":
        pages = pages[:-1]
    return pages


def _normalize_whitespace(text: str) -> str:
    """Fold Unicode separator-spaces to ASCII and collapse whitespace runs.

    Mirrors ``anvil/lib/render_gate.py``'s pdftotext-normalization
    contract (see its ``_sweep_nonascii_codepoints`` docstring): poppler's
    ``pdftotext`` collapses Unicode ``Zs``-category separator spaces (NBSP,
    thin space, narrow NBSP, …) to a plain ASCII space during extraction.
    The same fold is applied here to the *source* side before comparing,
    so a source caption typed with e.g. an NBSP does not false-block
    against the already-normalized PDF text. Runs of whitespace (any mix
    of the now-uniform ASCII spaces, tabs, newlines) collapse to one space
    and the result is stripped.
    """
    folded = "".join(
        " " if unicodedata.category(ch) == "Zs" else ch for ch in text
    )
    return " ".join(folded.split())


# --- last-checkable-source-line extraction -------------------------------------
#
# Marp per-slide directive comment, e.g. ``<!-- _class: ask -->`` or the
# escape hatch ``<!-- anvil-lint-disable: ... -->``. Single-line match
# (mirrors ``anvil.lib.marp_lint._DIRECTIVE_COMMENT_RE`` but applied
# per-line rather than with ``re.MULTILINE`` across a whole slide).
_DIRECTIVE_COMMENT_RE = re.compile(r"^\s*<!--\s*[^>]+?\s*-->\s*$")

# Markdown image reference: ``![alt](path)``. Stripped entirely — an
# image's alt text is not rendered as visible PDF page text by the deck
# theme, so an image-only line carries no checkable content.
_IMAGE_REF_RE = re.compile(r"!\[[^\]]*\]\([^)]*\)")

# Markdown link: ``[text](url)`` — keep the visible text, drop the target.
_LINK_RE = re.compile(r"\[([^\]]*)\]\([^)]*\)")

# Emphasis / code-span markers Marp renders away: ``*``, ``_``, backtick.
_EMPHASIS_CHARS_RE = re.compile(r"[*_`]")

# Leading list-item marker (``- ``, ``* ``, ``+ ``, ``1. ``).
_LEADING_LIST_MARKER_RE = re.compile(r"^\s*(?:[-*+]|\d+\.)\s+")


def _strip_markdown_decoration(line: str) -> str:
    """Strip markdown syntax that never reaches the rendered PDF as typed.

    Applied to a single already-stripped candidate line before the
    text-layer containment check, so a caption like ``*Figure 3: some
    text.*`` compares against the rendered ``Figure 3: some text.`` rather
    than failing on the literal asterisks Marp strips at render time.
    """
    text = _IMAGE_REF_RE.sub("", line)
    text = _LINK_RE.sub(r"\1", text)
    text = _LEADING_LIST_MARKER_RE.sub("", text)
    text = _EMPHASIS_CHARS_RE.sub("", text)
    return text.strip()


def _last_checkable_source_line(slide_raw: str) -> Optional[str]:
    """Return the slide's last non-empty, non-directive, markdown-stripped line.

    Walks the slide's source lines from the bottom up. Blank lines and
    Marp/anvil directive comments are skipped outright (issue #983's edge
    case: "a slide whose last source line is itself an HTML comment or
    directive should not be treated as content to verify"). A candidate
    line is markdown-decoration-stripped (image refs removed entirely,
    link text unwrapped, emphasis markers dropped); if nothing alphanumeric
    survives (e.g. an image-only line, or a line that is pure punctuation),
    the scan continues further up. Returns ``None`` when the slide has no
    checkable line at all (image-only / directive-only slide) — the caller
    skips such slides rather than treating the absence as a defect.
    """
    for line in reversed(slide_raw.splitlines()):
        stripped = line.strip()
        if not stripped:
            continue
        if _DIRECTIVE_COMMENT_RE.match(stripped):
            continue
        candidate = _strip_markdown_decoration(stripped)
        if any(ch.isalnum() for ch in candidate):
            return candidate
    return None


# --- public entry point ---------------------------------------------------------


def check_text_layer_completeness(
    deck_pdf: Path,
    deck_md: Path,
    *,
    pdftotext_path: Optional[str] = None,
) -> TextLayerCompletenessResult:
    """Confirm every slide's last source line survives into its rendered page.

    Parameters
    ----------
    deck_pdf:
        Path to the rendered ``deck.pdf``. Must exist; if absent the
        function returns a ``TextLayerCompletenessResult(skipped=True,
        reason=...)`` (matches the graceful-skip contract documented in
        the deck-review command).
    deck_md:
        Path to the deck markdown source.
    pdftotext_path:
        Optional override for the ``pdftotext`` executable (tests use
        this to force the "missing binary" skip path deterministically).

    Returns
    -------
    A :class:`TextLayerCompletenessResult` carrying per-slide findings.
    Slides whose last source line is a checkable line ARE compared even
    when their Marp ``_class:`` has only one instance in the deck — unlike
    ``auto_shrink_detector``, this check needs no peer class, so it has no
    singleton-carve-out gap.
    """
    deck_pdf = Path(deck_pdf)
    deck_md = Path(deck_md)

    exe = _which_pdftotext(pdftotext_path)
    if exe is None:
        return TextLayerCompletenessResult(
            skipped=True,
            reason=(
                "pdftotext not found on PATH (poppler-utils) — install "
                "poppler-utils to enable the text-layer-completeness gate."
            ),
        )
    if not deck_pdf.exists():
        return TextLayerCompletenessResult(
            skipped=True,
            reason=(
                f"deck.pdf not found at {deck_pdf} — run `deck-figures` "
                "before `deck-review` to render the PDF."
            ),
        )
    if not deck_md.exists():
        return TextLayerCompletenessResult(
            skipped=True,
            reason=f"deck.md not found at {deck_md}.",
        )

    pages = _extract_pdf_page_texts(deck_pdf, pdftotext_path=exe)
    if pages is None:
        return TextLayerCompletenessResult(
            skipped=True,
            reason=f"pdftotext failed to extract text from {deck_pdf}.",
        )

    source = deck_md.read_text(encoding="utf-8")
    slides = _split_slides(source)

    findings: list[Finding] = []
    for slide in slides:
        if slide.index > len(pages):
            # deck.pdf is stale relative to deck.md (fewer rendered pages
            # than source slides) — nothing to compare this slide against.
            # A stale-render mismatch is a different failure mode than
            # this gate targets; skip rather than raise or false-flag.
            continue

        last_line = _last_checkable_source_line(slide.raw)
        if last_line is None:
            continue

        needle = _normalize_whitespace(last_line)
        if not needle:
            continue

        page_text = _normalize_whitespace(pages[slide.index - 1])
        if needle.lower() in page_text.lower():
            continue

        disabled_rules = _collect_disabled_rules(slide.raw)
        suppressed = RULE_ID in disabled_rules

        message = (
            f"Slide {slide.index}'s last source line is missing from its "
            f'rendered PDF page\'s text layer: "{last_line}". This usually '
            "means the line (often a caption below a height-uncapped "
            "figure) was pushed past the slide's bottom edge and clipped "
            "at render time. Confirm the rendered PDF page visually, then "
            "either constrain the figure (e.g. an explicit `h:NNNpx` "
            "clamp) or trim/move the line so it fits. Suppress a "
            "deliberately off-slide line with "
            f"`<!-- anvil-lint-disable: {RULE_ID} -->`."
        )
        findings.append(
            Finding(
                slide=slide.index,
                line=slide.start_line,
                rule=RULE_ID,
                severity="info" if suppressed else "error",
                message=message,
            )
        )

    findings.sort(key=lambda f: f.slide)

    return TextLayerCompletenessResult(findings=findings, skipped=False, reason=None)


__all__ = [
    "Finding",
    "RULE_ID",
    "TextLayerCompletenessResult",
    "check_text_layer_completeness",
]
