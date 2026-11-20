"""Tests for ``anvil.skills.deck.lib.figure_legibility``.

The gate flags figures whose embedded text falls below the projection
legibility floor *as displayed on the slide* (after the CSS
``max-height: 75vh`` clamp or an explicit ``h:NNNpx`` Marp keyword).
v1 is the no-deps heuristic (per the issue #563 curator's plan): read
the PNG's intrinsic dimensions from the IHDR chunk via stdlib
``struct.unpack`` and approximate displayed glyph height from a per-
diagram-type intrinsic font-size lookup.

Since issue #964, mermaid's intrinsic text-height constant (``18``,
the source-SVG font size) is multiplied by
``Geometry.mermaid_render_scale`` (default ``2.0``, matching the
pinned ``mmdc --scale 2`` invocation, issue #545) before being compared
against displayed height — the effective mermaid constant used below
is therefore ``18 * 2.0 = 36``. matplotlib's intrinsic text-height is
read from the sibling ``figures/src/<name>.py`` source (explicit
``fontsize=`` regex scan, issue #964) rather than a fixed constant;
``_make_figure``'s default matplotlib stub (``"import matplotlib\n"``)
carries no explicit fontsize, so it exercises the rcParam-default
(10 pt) fallback unless a test overrides the sibling source.

Test matrix (per the curator's plan, arithmetic updated for #964):

- ``thin_strip_default_clamp`` — 800x80 mermaid PNG referenced without
  any ``h:`` keyword. Falls back to the CSS ``max-height: 75vh`` clamp
  (~540 px). Aspect 10:1 means it's width-limited: displayed height
  = 1280 * (80/800) = 128 px. Displayed glyph height
  = 36 * (128/80) = 57.6 px → ABOVE the warning threshold. NOT flagged.

  This is a more important test than the raw goodboy.1 ratio: the
  width-limited case shows the gate correctly skips a thin strip that
  still produces legible glyphs once isotropic scaling kicks in.

- ``thin_strip_with_explicit_h_clamp`` — 800x80 mermaid PNG referenced
  with ``h:80px`` Marp keyword. Explicit clamp kicks in at 80 px.
  Displayed glyph height = 36 * (80/80) * (intrinsic_h / displayed_h)
  → at h:80px the displayed height equals the intrinsic height so the
  scale ratio is 1.0 and displayed glyph = 36 px ABOVE the threshold.

- ``thin_strip_severe_clamp`` — A genuinely illegible case: a tall
  intrinsic figure (200x1200) clamped via ``h:80px``. Displayed glyph
  = 36 * (80/1200) = 2.4 px → ERROR.

- ``tb_oriented_no_keyword`` — A reasonably-shaped 800x600 TB
  mermaid PNG, no ``h:`` keyword. Displayed height clamps to the
  CSS default 540 px → glyph 32.4 px ABOVE warning. NOT flagged.

- ``goodboy_raas_flywheel_repro`` — The canary fixture cited in the
  issue body: a 784x102 mermaid PNG (LR cycle that rendered as a thin
  strip). Width-limited displayed height = 1280 * (102/784) ≈ 166.5 px.
  Displayed glyph height = 36 * (166.5/102) ≈ 58.8 px. ABOVE warning.
  This is the BEFORE-#545 case; the figure now reads fine BECAUSE
  width-fill produces a tall enough display height. The legibility
  gate is NOT a substitute for the render-side aspect/orient fix.

- ``suppressed_via_escape_hatch`` — A genuinely illegible figure with
  ``<!-- anvil-figure-legibility-disable: <name> -->`` on the slide.
  Severity downgrades to ``info``.

- ``missing_figure_silently_skipped`` — A reference to a non-existent
  PNG. Gate skips silently (handled by step 6 reference validation,
  not this gate).

- ``matplotlib_chart_passes`` — A reasonably-shaped matplotlib chart
  (1200x800, no embedded ``pHYs`` DPI chunk) with no clamp. Treated as
  ``matplotlib`` diagram type; since no DPI is embedded, the gate
  assumes the shipped ``anvil.mplstyle`` default of 200 DPI, giving an
  intrinsic glyph height of ``10pt * (200/72) ≈ 27.8 px``. Displayed
  height clamps to 540 px; glyph ``27.8 * (540/800) ≈ 18.75 px`` →
  ABOVE the warning threshold. NOT flagged. Demonstrates the gate is
  not mermaid-specific *and* closes the issue #904 DPI-blindness bug:
  pre-#904 this exact figure (with the un-DPI-scaled 14 px constant)
  incorrectly fired an ``error``.

- ``escape_hatch_whole_slide`` — Bare ``<!-- anvil-figure-legibility-
  disable -->`` (no name) suppresses every figure on that slide.

- ``matplotlib_dpi_scaling`` (issue #904) — A 1000x1000 matplotlib
  chart with an explicit ``pHYs`` DPI chunk, exercised at 72, 100, and
  200 DPI. No clamp, so displayed height is the CSS default 540 px;
  scale = 540/1000 = 0.54. Displayed glyph = ``10pt * (dpi/72) *
  0.54``: 5.4 px @72 dpi (ERROR), 7.5 px @100 dpi (ERROR), 15.0 px
  @200 dpi (passes, just above the 14 px warning floor). Pins the
  DPI-proportional scaling directly, independent of the 200-DPI
  fallback default.

- ``matplotlib_genuinely_undersized_still_fires`` (issue #904) — A
  1200x800 matplotlib chart at a real 200 DPI, constrained via an
  explicit ``w:`` keyword to well under the CSS default. The DPI fix
  must not blind the gate to a figure that is *actually* too small
  once genuinely constrained — this is the issue's "important caveat."

- ``mermaid_render_scale_fixes_under_flag`` (issue #964) — A 700x900
  mermaid PNG (``graph TB``-shaped) referenced with ``h:400px``.
  Scale ratio = 400/900 ≈ 0.444. At the render-scale-aware default
  (``mermaid_render_scale=2.0``), displayed glyph = 36 * 0.444 = 16 px
  → ABOVE warning, NOT flagged — matching the issue's "a scale-2
  mermaid render ... that displays above the floor produces no error
  stub" acceptance criterion. Pinned against a ``mermaid_render_scale=1.0``
  override (i.e. the pre-#964 behaviour) on the *same* figure, which
  computes 18 * 0.444 = 8 px → ERROR, demonstrating the ~2x under-flag
  this issue fixes.

- ``matplotlib_source_fontsize_changes_verdict`` (issue #964) — Two
  1200x1000 matplotlib PNGs referenced with ``h:400px`` (scale ratio
  400/1000 = 0.4), differing only in their sibling
  ``figures/src/<name>.py`` source's explicit ``fontsize=`` value:
  ``fontsize=8`` → intrinsic 8pt * (200/72) ≈ 22.2 px → displayed
  ≈ 8.9 px → ERROR; ``fontsize=16`` → intrinsic 16pt * (200/72)
  ≈ 44.4 px → displayed ≈ 17.8 px → NOT flagged. Same PNG geometry
  throughout — only the source-declared fontsize changes — pinning
  that a source-side fontsize change now moves the gate's verdict
  (pre-#964 the fixed ``14.0`` px constant could never move).

Runs under either ``python -m unittest discover anvil/skills/deck/tests/``
or ``pytest anvil/skills/deck/tests/``.
"""

from __future__ import annotations

import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from anvil.skills.deck.lib.figure_legibility import (
    Geometry,
    lint_figures,
)


# ---------------------------------------------------------------------------
# Test helpers — minimal PNG synthesis
# ---------------------------------------------------------------------------


def _make_minimal_png(width: int, height: int) -> bytes:
    """Build a valid PNG of the requested dimensions, no Pillow required.

    The gate reads only the IHDR chunk (bytes 16-24); the image data
    is irrelevant. We synthesize a single-colour image as cheaply as
    possible: one scanline of zero filter + N white RGB triples,
    repeated for ``height`` rows, zlib-compressed.

    Mirrors the ``_make_tiny_png`` helper in test_imagegen.py
    (parameterized on dimensions).
    """
    sig = b"\x89PNG\r\n\x1a\n"

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    # IHDR: width, height, bit depth 8, colour type 2 (RGB), 0/0/0.
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)

    # One scanline = filter byte (0) + W * 3 bytes RGB. All white.
    scanline = b"\x00" + b"\xff" * (width * 3)
    raw = scanline * height
    idat = zlib.compress(raw)

    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def _make_png_with_dpi(width: int, height: int, dpi: float) -> bytes:
    """Build a valid PNG with an explicit ``pHYs`` chunk declaring ``dpi``.

    Mirrors matplotlib's ``savefig(dpi=D)`` output: ``ppux == ppuy ==
    round(D / 0.0254)``, unit byte ``1`` (meters), inserted between
    IHDR and IDAT (matplotlib's own chunk order).
    """
    sig = b"\x89PNG\r\n\x1a\n"

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)

    ppu = round(dpi / 0.0254)
    phys = struct.pack(">IIB", ppu, ppu, 1)

    scanline = b"\x00" + b"\xff" * (width * 3)
    raw = scanline * height
    idat = zlib.compress(raw)

    return (
        sig
        + chunk(b"IHDR", ihdr)
        + chunk(b"pHYs", phys)
        + chunk(b"IDAT", idat)
        + chunk(b"IEND", b"")
    )


def _write_deck(
    tmp_path: Path,
    figure_refs: list[str],
    *,
    suppress_directive: str | None = None,
) -> Path:
    """Write a minimal deck.md with the given figure references.

    Each entry in ``figure_refs`` is the literal markdown after the
    standalone-image-line convention, e.g. ``![alt](figures/foo.png)``.
    """
    lines = [
        "---",
        "marp: true",
        "size: 16:9",
        "theme: anvil-deck",
        "---",
        "",
    ]
    if suppress_directive:
        lines.append(suppress_directive)
        lines.append("")
    lines.append("# Slide 1")
    lines.append("")
    for ref in figure_refs:
        lines.append(ref)
        lines.append("")
    deck = tmp_path / "deck.md"
    deck.write_text("\n".join(lines), encoding="utf-8")
    return deck


def _make_figure(
    tmp_path: Path,
    name: str,
    width: int,
    height: int,
    *,
    diagram_type: str = "mermaid",
    dpi: float | None = None,
    matplotlib_source: str | None = None,
) -> Path:
    """Create a figures/<name>.png plus a sibling src/<name>.<ext> for type.

    ``dpi``, when given, embeds a ``pHYs`` chunk declaring that physical
    density (mirrors matplotlib's ``savefig(dpi=...)``). Omitted by
    default, matching a PNG with no declared density.

    ``matplotlib_source``, when given (and ``diagram_type ==
    "matplotlib"``), overrides the sibling script's contents — used to
    exercise the issue #964 source-fontsize regex scan
    (``_matplotlib_source_fontsize_pt``). Defaults to a fontsize-free
    stub, exercising the rcParam-default fallback.
    """
    figures = tmp_path / "figures"
    figures.mkdir(exist_ok=True)
    src = figures / "src"
    src.mkdir(exist_ok=True)

    png_path = figures / f"{name}.png"
    if dpi is None:
        png_path.write_bytes(_make_minimal_png(width, height))
    else:
        png_path.write_bytes(_make_png_with_dpi(width, height, dpi))

    # Sibling source for diagram-type classification.
    if diagram_type == "mermaid":
        (src / f"{name}.mmd").write_text("flowchart LR\nA --> B\n", encoding="utf-8")
    elif diagram_type == "matplotlib":
        text = matplotlib_source if matplotlib_source is not None else "import matplotlib\n"
        (src / f"{name}.py").write_text(text, encoding="utf-8")
    # 'unknown' = no sibling source.

    return png_path


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestThinStripDefaultClamp(unittest.TestCase):
    """800x80 mermaid PNG, no h: keyword. Width-limited; passes.

    The gate must NOT fire on a thin strip that's saved by width-fill
    scaling. This case is the "width-limited" branch of the
    displayed-height computation.
    """

    def test_no_findings_when_width_limited_scaling_makes_text_legible(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "thin_strip", 800, 80, diagram_type="mermaid")
            deck = _write_deck(root, ["![alt](figures/thin_strip.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())


class TestThinStripSevereClamp(unittest.TestCase):
    """Tall figure clamped via h:80px. Drives displayed glyph well under 11 px.

    A 200x1200 mermaid PNG (height-limited because aspect is tall) with
    an explicit ``h:80px`` clamp. Scale ratio = 80/1200 = 0.067;
    displayed glyph = 18 * 0.067 = 1.2 px → ERROR.
    """

    def test_emits_error_with_correct_rule_and_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "tall_clamped", 200, 1200, diagram_type="mermaid")
            deck = _write_deck(
                root, ["![h:80px alt](figures/tall_clamped.png)"]
            )

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())
            finding = result.errors[0]
            self.assertEqual(finding.rule, "figure-legibility-floor")
            self.assertEqual(finding.severity, "error")
            self.assertEqual(finding.slide, 1)
            self.assertIn("figures/tall_clamped.png", finding.message)


class TestTbOrientedNoKeyword(unittest.TestCase):
    """A reasonable 800x600 TB mermaid PNG with no h: keyword. Passes.

    Per AC2 on the issue: a TB-aspect figure with no override emits
    zero findings.
    """

    def test_no_findings_on_well_formed_figure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "tb_diagram", 800, 600, diagram_type="mermaid")
            deck = _write_deck(root, ["![alt](figures/tb_diagram.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0)
            self.assertEqual(len(result.warnings), 0)
            self.assertEqual(len(result.infos), 0)


class TestGoodboyRaasFlywheelRepro(unittest.TestCase):
    """The canary fixture: 784x102 mermaid PNG (the cited goodboy.1 case).

    Width-limited displayed height = 1280 * (102/784) ≈ 166 px.
    Displayed glyph height = 18 * (166/102) ≈ 29.3 px → ABOVE warning.

    This is the BEFORE-#545 case; the figure now reads fine BECAUSE the
    width-fill produces a tall enough display height. The legibility
    gate is NOT a substitute for the render-side aspect/orient fix —
    that's #545. This test pins the boundary so a regression that
    makes the gate over-fire on width-limited thin strips is caught.
    """

    def test_width_limited_thin_strip_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "raas_flywheel", 784, 102, diagram_type="mermaid")
            deck = _write_deck(root, ["![alt](figures/raas_flywheel.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())


class TestExplicitHClampOnThinStripPushesUnderFloor(unittest.TestCase):
    """A thin-strip figure with an aggressive ``h:`` keyword IS flagged.

    784x102 mermaid PNG referenced with ``h:20px`` Marp keyword.
    Clamped displayed height = 20 px; scale = 20/102 ≈ 0.196;
    displayed glyph = 36 * 0.196 ≈ 7.1 px → ERROR. (Pre-#964, at the
    un-scaled 18 px constant, ``h:60px`` was the threshold-crossing
    clamp; post-#964 the doubled 36 px constant needs a tighter clamp
    to still land under the error floor — recomputed, not just
    loosened, per the issue's test-update guidance.)
    """

    def test_h_clamp_drives_under_error_threshold(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "thin_clamped", 784, 102, diagram_type="mermaid")
            deck = _write_deck(
                root, ["![h:20px alt](figures/thin_clamped.png)"]
            )

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())
            self.assertEqual(result.errors[0].rule, "figure-legibility-floor")


class TestSuppressedViaEscapeHatch(unittest.TestCase):
    """Per-figure ``anvil-figure-legibility-disable: <name>`` downgrades to info."""

    def test_named_suppression_downgrades_to_info(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "tall_clamped", 200, 1200, diagram_type="mermaid")
            deck = _write_deck(
                root,
                ["![h:80px alt](figures/tall_clamped.png)"],
                suppress_directive="<!-- anvil-figure-legibility-disable: tall_clamped -->",
            )

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0)
            self.assertEqual(len(result.warnings), 0)
            self.assertEqual(len(result.infos), 1)
            self.assertEqual(result.infos[0].severity, "info")
            self.assertEqual(result.infos[0].rule, "figure-legibility-floor")


class TestEscapeHatchWholeSlide(unittest.TestCase):
    """Bare ``anvil-figure-legibility-disable`` suppresses every figure on the slide."""

    def test_bare_directive_suppresses_all(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "f1", 200, 1200, diagram_type="mermaid")
            _make_figure(root, "f2", 200, 1200, diagram_type="mermaid")
            deck = _write_deck(
                root,
                [
                    "![h:80px alt](figures/f1.png)",
                    "![h:80px alt](figures/f2.png)",
                ],
                suppress_directive="<!-- anvil-figure-legibility-disable -->",
            )

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0)
            self.assertEqual(len(result.warnings), 0)
            self.assertEqual(len(result.infos), 2)


class TestMissingFigureSilentlySkipped(unittest.TestCase):
    """A referenced-but-missing PNG is silently skipped by this gate.

    Missing-file handling is owned by step 6 of ``deck-figures``
    (reference validation). The legibility gate must skip cleanly so
    it doesn't double-fire on missing files.
    """

    def test_missing_file_yields_no_findings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # Note: no figure created at this path
            deck = _write_deck(root, ["![alt](figures/does_not_exist.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0)
            self.assertEqual(len(result.warnings), 0)
            self.assertEqual(len(result.infos), 0)


class TestMatplotlibChartNotMermaidSpecial(unittest.TestCase):
    """The gate is diagram-type-aware but not mermaid-only.

    A 1200x800 matplotlib chart with no embedded ``pHYs`` DPI chunk,
    referenced with no clamp, displays at 540 px (the CSS default
    cap). Since no DPI is declared, the gate assumes the shipped
    ``anvil.mplstyle`` default of 200 DPI: intrinsic glyph height
    = 10pt * (200/72) ≈ 27.8 px. Scale = 540/800 = 0.675;
    displayed glyph ≈ 27.8 * 0.675 ≈ 18.75 px → ABOVE the warning
    floor. NOT flagged.

    This is the exact issue #904 regression case: pre-#904, the
    un-DPI-scaled 14 px constant computed 14 * 0.675 ≈ 9.45 px and
    fired an ``error`` on a figure that was demonstrably legible.
    """

    def test_matplotlib_chart_at_default_dpi_not_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "chart", 1200, 800, diagram_type="matplotlib")
            deck = _write_deck(root, ["![alt](figures/chart.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())


class TestMatplotlibDpiScaling(unittest.TestCase):
    """Issue #904: the gate reads the PNG's embedded DPI (``pHYs``).

    A 1000x1000 matplotlib chart, no clamp, so displayed height is
    the CSS default 540 px; scale = 540/1000 = 0.54. Displayed glyph
    = ``10pt * (dpi/72) * 0.54``:

    - 72 dpi:  10 * 1.000 * 0.54 =  5.4 px → ERROR
    - 100 dpi: 10 * 1.389 * 0.54 =  7.5 px → ERROR
    - 200 dpi: 10 * 2.778 * 0.54 = 15.0 px → passes (just above the
      14 px warning floor)

    Same figure geometry throughout — only the declared DPI changes —
    so this pins the DPI-proportional scaling directly rather than
    relying on the 200 DPI no-``pHYs`` fallback.
    """

    def test_72_dpi_emits_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "chart", 1000, 1000, diagram_type="matplotlib", dpi=72)
            deck = _write_deck(root, ["![alt](figures/chart.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())
            self.assertIn("matplotlib", result.errors[0].message)

    def test_100_dpi_emits_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "chart", 1000, 1000, diagram_type="matplotlib", dpi=100)
            deck = _write_deck(root, ["![alt](figures/chart.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())

    def test_200_dpi_not_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "chart", 1000, 1000, diagram_type="matplotlib", dpi=200)
            deck = _write_deck(root, ["![alt](figures/chart.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())


class TestMatplotlibGenuinelyUndersizedStillFires(unittest.TestCase):
    """Issue #904's "important caveat": a truly-undersized figure still fires.

    A 1200x800 matplotlib chart rendered at a real 200 DPI (so the DPI
    fix's scaling is in play), but referenced with an explicit ``w:``
    keyword clamping it to 400 px — well under a third of the 1280 px
    slide width. Width clamp dominates: displayed height =
    400 * (800/1200) ≈ 266.7 px; scale ≈ 0.333; displayed glyph
    ≈ 10 * (200/72) * 0.333 ≈ 9.3 px → ERROR. The DPI fix must not
    blind the gate to a figure that is genuinely constrained too small.
    """

    def test_narrow_w_clamp_still_emits_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "chart", 1200, 800, diagram_type="matplotlib", dpi=200)
            deck = _write_deck(root, ["![w:400px alt](figures/chart.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())
            self.assertEqual(result.errors[0].rule, "figure-legibility-floor")


class TestMermaidRenderScaleFixesUnderFlag(unittest.TestCase):
    """Issue #964: the mermaid render-scale fix stops the ~2x under-flag.

    A 700x900 mermaid PNG (a ``graph TB``-shaped figure) referenced
    with ``h:400px``. Scale ratio = 400/900 ≈ 0.444.

    At the shipped default (``mermaid_render_scale=2.0``, matching the
    load-bearing ``mmdc --scale 2`` invocation), displayed glyph =
    36 * 0.444 = 16 px → ABOVE warning, NOT flagged: this is the
    acceptance criterion "a scale-2 mermaid render ... that displays
    above the floor produces no error stub."

    Pinned against the pre-#964 behaviour on the *identical* figure via
    a ``mermaid_render_scale=1.0`` override: 18 * 0.444 = 8 px → ERROR
    — reproducing the exact ~2x under-flag this issue fixes (a figure
    that is in fact legible, incorrectly flagged).
    """

    def test_default_scale_not_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "tb_shape", 700, 900, diagram_type="mermaid")
            deck = _write_deck(root, ["![h:400px alt](figures/tb_shape.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())

    def test_pre_964_unscaled_geometry_reproduces_the_under_flag(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "tb_shape", 700, 900, diagram_type="mermaid")
            deck = _write_deck(root, ["![h:400px alt](figures/tb_shape.png)"])

            geo = Geometry(mermaid_render_scale=1.0)
            result = lint_figures(deck, geometry=geo)

            self.assertEqual(len(result.errors), 1, result.to_summary())


class TestMatplotlibSourceFontsizeChangesVerdict(unittest.TestCase):
    """Issue #964: a source-side fontsize change moves the gate's verdict.

    Two 1200x1000 matplotlib PNGs referenced with ``h:400px`` (scale
    ratio 400/1000 = 0.4), differing only in their sibling
    ``figures/src/<name>.py`` source's explicit ``fontsize=`` value:

    - ``fontsize=8``: intrinsic 8pt * (200/72) ≈ 22.2 px (no embedded
      DPI, so the 200 DPI fallback applies) → displayed ≈ 8.9 px →
      ERROR.
    - ``fontsize=16``: intrinsic 16pt * (200/72) ≈ 44.4 px → displayed
      ≈ 17.8 px → NOT flagged.

    Pre-#964 the fixed ``14.0`` px constant could never move — a
    source-side fontsize bump (e.g. 10pt → 12pt, applied specifically
    to satisfy the gate) produced an identical estimate on both
    versions. This pins that the fix actually reads the source.
    """

    def test_small_fontsize_emits_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(
                root,
                "annotated",
                1200,
                1000,
                diagram_type="matplotlib",
                matplotlib_source="ax.annotate('n', xy=(0, 0), fontsize=8)\n",
            )
            deck = _write_deck(root, ["![h:400px alt](figures/annotated.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())
            self.assertIn("matplotlib", result.errors[0].message)

    def test_larger_fontsize_on_identical_geometry_is_not_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(
                root,
                "annotated",
                1200,
                1000,
                diagram_type="matplotlib",
                matplotlib_source="ax.annotate('n', xy=(0, 0), fontsize=16)\n",
            )
            deck = _write_deck(root, ["![h:400px alt](figures/annotated.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0, result.to_summary())
            self.assertEqual(len(result.warnings), 0, result.to_summary())

    def test_global_rcparams_font_size_override_is_read(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(
                root,
                "annotated",
                1200,
                1000,
                diagram_type="matplotlib",
                matplotlib_source="plt.rcParams['font.size'] = 8\n",
            )
            deck = _write_deck(root, ["![h:400px alt](figures/annotated.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 1, result.to_summary())


class TestWorstCaseAcrossSlides(unittest.TestCase):
    """A figure referenced from N slides with different clamps reports the worst.

    Per the curator's edge-case note: 'Figure referenced from multiple
    slides with different `h:` overrides → check against the smallest
    display height (worst-case).'
    """

    def test_worst_case_clamp_picked(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _make_figure(root, "f", 200, 1200, diagram_type="mermaid")

            # Two slides referencing the same figure with different h: clamps.
            deck = root / "deck.md"
            deck.write_text(
                "\n".join(
                    [
                        "---",
                        "marp: true",
                        "theme: anvil-deck",
                        "---",
                        "",
                        "# Slide 1",
                        "",
                        "![h:400px alt](figures/f.png)",
                        "",
                        "---",
                        "",
                        "# Slide 2",
                        "",
                        "![h:80px alt](figures/f.png)",
                        "",
                    ]
                ),
                encoding="utf-8",
            )

            result = lint_figures(deck)

            # The h:80px on slide 2 is the worst case → that's what we report.
            self.assertEqual(len(result.errors), 1, result.to_summary())
            self.assertEqual(result.errors[0].slide, 2)


class TestNonPngSilentlySkipped(unittest.TestCase):
    """References to non-PNG files (e.g. SVG) are silently skipped."""

    def test_svg_reference_yields_no_findings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            figures = root / "figures"
            figures.mkdir()
            # Write a file with a `.png` extension but non-PNG content
            # to exercise the IHDR-signature reject path.
            (figures / "fake.png").write_bytes(b"<svg>not a real png</svg>")
            deck = _write_deck(root, ["![alt](figures/fake.png)"])

            result = lint_figures(deck)

            self.assertEqual(len(result.errors), 0)
            self.assertEqual(len(result.warnings), 0)


class TestGeometryOverride(unittest.TestCase):
    """A consumer with a custom CSS cap can pass a Geometry override.

    Issue #964: the intrinsic figure height here is 2400 (not the
    pre-#964 1200) — doubled to offset the now-doubled effective
    mermaid constant (``18 * mermaid_render_scale`` = 36, default
    scale 2.0) so the worked arithmetic below reproduces the original
    test's intent (default cap → error; widened cap → passes) rather
    than just loosening the assertions.
    """

    def test_tighter_max_height_drives_under_floor(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # 200x2400 figure, no clamp.
            _make_figure(root, "tall", 200, 2400, diagram_type="mermaid")
            deck = _write_deck(root, ["![alt](figures/tall.png)"])

            # Default cap is 75vh = 540 px; with that cap, displayed glyph
            # = 36 * (540/2400) = 8.1 px → already an error.
            # Verify by overriding to a *generous* cap so the
            # default-default case becomes legible.
            result = lint_figures(deck)
            self.assertEqual(len(result.errors), 1)

            # Widen the slide height geometry to 1440 px so the 75vh cap
            # becomes 1080 px; the figure's intrinsic 2400 px height means
            # it's still height-limited, to 1080. Glyph = 36 *
            # (1080/2400) = 16.2 px → ABOVE warning.
            geo = Geometry(slide_height_px=1440)
            result_wide = lint_figures(deck, geometry=geo)
            self.assertEqual(len(result_wide.errors), 0)
            self.assertEqual(len(result_wide.warnings), 0)


if __name__ == "__main__":
    unittest.main()
