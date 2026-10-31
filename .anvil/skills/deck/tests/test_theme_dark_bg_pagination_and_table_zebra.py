"""Static, CI-safe guard for the two theme defects fixed in issue #906.

``deck-vision`` rasterized a 13-slide deck at 96 DPI and found two systematic
theme defects in ``anvil-deck.css``:

1. The pagination glyph (``section::after``) is styled ``var(--anvil-muted)``
   (``#6b6b6b``) with no override for dark-background ``_class`` slides
   (``ask``, ``section``). Measured contrast against the ``#1f4e7a`` accent
   background is ~1.79:1 — well under WCAG's 3:1 floor for incidental text —
   so the page number is effectively invisible on the ask slide (the one
   slide most likely to be referenced by number in a follow-up email) and
   the section-divider slide.

2. Tables render with alternating-row zebra shading inherited from the
   imported Marp ``default`` theme, contradicting the stylesheet's own
   comment ("Tables — clean, no shading on alternating rows (chartjunk)").
   The ``section.ask`` block already defeated this locally for ask-slide
   tables (see ``test_ask_table_css.py``), but there was no equivalent reset
   for tables on ordinary (light-background) slides.

A true visual regression needs a Marp render plus a pixel/contrast check,
which is not assumed available in CI. Instead this test reads
``anvil-deck.css`` directly and asserts the fix rules are present and
correctly scoped — a low-fidelity guard that directly prevents silent
deletion/regression of the fix.

Runs under either ``python -m unittest discover anvil/skills/deck/tests/``
or ``pytest anvil/skills/deck/tests/``.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path


_HERE = Path(__file__).resolve().parent
_CSS = _HERE.parent / "assets" / "anvil-deck.css"


def _read_css() -> str:
    return _CSS.read_text(encoding="utf-8")


# WCAG 2.x relative-luminance contrast ratio, standalone (no external deps)
# so this test has no new import surface.
def _srgb_channel(c: float) -> float:
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def _luminance(hex_color: str) -> float:
    hex_color = hex_color.lstrip("#")
    r, g, b = (int(hex_color[i : i + 2], 16) / 255 for i in (0, 2, 4))
    r, g, b = _srgb_channel(r), _srgb_channel(g), _srgb_channel(b)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def _blend_over(fg_hex: str, alpha: float, bg_hex: str) -> str:
    """Alpha-composite an opaque hex foreground over an opaque hex background."""
    fg = fg_hex.lstrip("#")
    bg = bg_hex.lstrip("#")
    out = []
    for i in (0, 2, 4):
        f = int(fg[i : i + 2], 16)
        b = int(bg[i : i + 2], 16)
        out.append(round(alpha * f + (1 - alpha) * b))
    return "#" + "".join(f"{c:02x}" for c in out)


def _contrast(hex1: str, hex2: str) -> float:
    l1, l2 = _luminance(hex1), _luminance(hex2)
    l1, l2 = max(l1, l2), min(l1, l2)
    return (l1 + 0.05) / (l2 + 0.05)


_ASK_BG = "#1f4e7a"  # --anvil-bg-ask / --anvil-accent


class TestDarkBackgroundPaginationContrast(unittest.TestCase):
    """section::after must be overridden on every dark-background _class."""

    def test_ask_pagination_override_present(self) -> None:
        css = _read_css()
        self.assertIn("section.ask::after", css)

    def test_section_divider_pagination_override_present(self) -> None:
        css = _read_css()
        self.assertIn("section.section::after", css)

    def test_ask_pagination_color_meets_3to1_contrast(self) -> None:
        css = _read_css()
        match = re.search(
            r"section\.ask::after\s*\{[^}]*color:\s*rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\s*\)",
            css,
        )
        self.assertIsNotNone(
            match, "expected an rgba(...) color declaration in section.ask::after"
        )
        r, g, b, alpha = (
            int(match.group(1)),
            int(match.group(2)),
            int(match.group(3)),
            float(match.group(4)),
        )
        fg_hex = f"#{r:02x}{g:02x}{b:02x}"
        blended = _blend_over(fg_hex, alpha, _ASK_BG)
        ratio = _contrast(blended, _ASK_BG)
        self.assertGreaterEqual(
            ratio,
            3.0,
            f"section.ask::after color contrast {ratio:.2f}:1 against {_ASK_BG} "
            "is below the WCAG 3:1 floor for incidental text",
        )

    def test_section_divider_pagination_color_meets_3to1_contrast(self) -> None:
        css = _read_css()
        match = re.search(
            r"section\.section::after\s*\{[^}]*color:\s*rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\s*\)",
            css,
        )
        self.assertIsNotNone(
            match,
            "expected an rgba(...) color declaration in section.section::after",
        )
        r, g, b, alpha = (
            int(match.group(1)),
            int(match.group(2)),
            int(match.group(3)),
            float(match.group(4)),
        )
        fg_hex = f"#{r:02x}{g:02x}{b:02x}"
        # section.section paints var(--anvil-accent), the same #1f4e7a value
        # as --anvil-bg-ask.
        blended = _blend_over(fg_hex, alpha, _ASK_BG)
        ratio = _contrast(blended, _ASK_BG)
        self.assertGreaterEqual(
            ratio,
            3.0,
            f"section.section::after color contrast {ratio:.2f}:1 against "
            f"{_ASK_BG} is below the WCAG 3:1 floor for incidental text",
        )

    def test_default_muted_pagination_color_fails_3to1_against_ask_bg(self) -> None:
        """Sanity check: the base --anvil-muted pagination color is indeed
        the defect being fixed (regression guard on the guard)."""
        ratio = _contrast("#6b6b6b", _ASK_BG)
        self.assertLess(ratio, 3.0)


class TestTableZebraStripingReset(unittest.TestCase):
    """Tables must not carry alternating-row shading anywhere in the deck."""

    def test_general_zebra_reset_present(self) -> None:
        css = _read_css()
        self.assertIn("section table tbody tr:nth-child(even)", css)

    def test_general_zebra_reset_is_transparent(self) -> None:
        css = _read_css()
        match = re.search(
            r"section table tbody tr:nth-child\(even\)\s*\{([^}]*)\}", css
        )
        self.assertIsNotNone(match)
        block = match.group(1)
        self.assertIn("background: transparent;", block)
        self.assertIn("background-color: transparent;", block)

    def test_general_reset_scoped_above_code_block(self) -> None:
        """The reset must live in the base Tables section (applies to every
        slide), not nested under a specific _class override."""
        css = _read_css()
        tables_start = css.index("/* Tables")
        code_start = css.index("/* Code")
        zebra_start = css.index("section table tbody tr:nth-child(even)")
        self.assertTrue(tables_start < zebra_start < code_start)


if __name__ == "__main__":
    unittest.main()
