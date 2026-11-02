"""Behavioral tests for the ``_find_anvil_root`` snippet shipped in
``anvil/skills/deck/assets/figure-conventions.md`` (issue #951).

The doc ships a copy-paste JSON-read snippet that figure scripts embed
verbatim. Before #951 the snippet's walk-up checked only the **flat**,
pre-#230 layout (``.anvil`` + ``lib/figures/palette.json``) while its own
prose and docstring described the **nested**, post-#230
``.anvil/anvil/lib/figures/palette.json`` layout that
``scripts/install-anvil.sh`` actually produces — so a script copied verbatim
raised ``FileNotFoundError`` on a current consumer install.

(The flat form is spelled in segments above on purpose: the repo-wide guard
in ``tests/lib/test_no_stale_anvil_lib_asset_paths.py`` forbids shipped files
from spelling that legacy consumer path literally.)

These tests extract every ``_find_anvil_root`` definition out of the markdown
(both the §3 snippet and the §6 canonical script template), ``exec`` it, and
assert it resolves under both layouts. That makes the doc's promise
executable: the assertion is on behavior, not on the presence of a string.

Deck-distinct filename per the #58 packaging convention.

Runs under either ``python -m unittest discover anvil/skills/deck/tests/``
or ``pytest anvil/skills/deck/tests/``.
"""

from __future__ import annotations

import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_REPO_ROOT = _HERE.parents[3]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

_DOC = _HERE.parent / "assets" / "figure-conventions.md"

# Each definition runs from ``def _find_anvil_root`` through its terminating
# ``raise FileNotFoundError`` line (the snippet's documented shape).
_FUNC_RE = re.compile(
    r"^def _find_anvil_root\(.*?^    raise FileNotFoundError.*?$",
    re.MULTILINE | re.DOTALL,
)

_PALETTE = {"ANVIL_NAVY": "#1B2A4A", "ANVIL_INK": "#111111"}


def _extract_definitions(text: str) -> list[str]:
    return _FUNC_RE.findall(text)


def _compile(source: str):
    namespace: dict = {"Path": Path}
    exec(compile(source, "<figure-conventions snippet>", "exec"), namespace)
    return namespace["_find_anvil_root"]


def _make_install(root: Path, *, nested: bool) -> Path:
    """Create a simulated consumer install; return the expected ANVIL root."""
    anvil_root = root / ".anvil" / "anvil" if nested else root / ".anvil"
    figures = anvil_root / "lib" / "figures"
    figures.mkdir(parents=True)
    (figures / "palette.json").write_text(json.dumps(_PALETTE), encoding="utf-8")
    (figures / "anvil.mplstyle").write_text("figure.dpi: 200\n", encoding="utf-8")
    return anvil_root


class TestFindAnvilRootSnippet(unittest.TestCase):
    """Every shipped copy of the snippet resolves both install layouts."""

    def setUp(self) -> None:
        self.definitions = _extract_definitions(
            _DOC.read_text(encoding="utf-8")
        )

    def test_both_shipped_copies_are_extractable(self) -> None:
        """§3 snippet + §6 canonical script template — two copies, both live."""
        self.assertEqual(len(self.definitions), 2, self.definitions)

    def test_resolves_canonical_nested_install(self) -> None:
        """The post-#230 layout the installer actually writes."""
        for source in self.definitions:
            find = _compile(source)
            with tempfile.TemporaryDirectory() as td:
                repo = Path(td).resolve()
                expected = _make_install(repo, nested=True)
                # A realistic bare-python3 call site: figures/src/<name>.py
                # several levels below the repo root.
                script = repo / "deck-thread.1" / "figures" / "src" / "plot.py"
                script.parent.mkdir(parents=True)
                script.write_text("", encoding="utf-8")

                anvil = find(script.resolve())
                self.assertEqual(anvil, expected)
                # The two documented call sites compose off the returned root.
                palette = json.loads(
                    (anvil / "lib/figures/palette.json").read_text()
                )
                self.assertEqual(palette["ANVIL_NAVY"], _PALETTE["ANVIL_NAVY"])
                self.assertTrue((anvil / "lib/figures/anvil.mplstyle").is_file())

    def test_falls_back_to_pre_230_flat_install(self) -> None:
        """Legacy ``.anvil/lib/`` installs keep working (the fallback branch)."""
        for source in self.definitions:
            find = _compile(source)
            with tempfile.TemporaryDirectory() as td:
                repo = Path(td).resolve()
                expected = _make_install(repo, nested=False)
                script = repo / "deck-thread.1" / "figures" / "src" / "plot.py"
                script.parent.mkdir(parents=True)
                script.write_text("", encoding="utf-8")

                anvil = find(script.resolve())
                self.assertEqual(anvil, expected)
                self.assertTrue((anvil / "lib/figures/palette.json").is_file())

    def test_prefers_nested_when_both_layouts_are_present(self) -> None:
        """A half-upgraded install resolves to the canonical nested root."""
        for source in self.definitions:
            find = _compile(source)
            with tempfile.TemporaryDirectory() as td:
                repo = Path(td).resolve()
                _make_install(repo, nested=False)
                nested = _make_install(repo, nested=True)
                script = repo / "deck-thread.1" / "figures" / "src" / "plot.py"
                script.parent.mkdir(parents=True)
                script.write_text("", encoding="utf-8")

                self.assertEqual(find(script.resolve()), nested)

    def test_raises_when_no_anvil_ancestor_exists(self) -> None:
        for source in self.definitions:
            find = _compile(source)
            with tempfile.TemporaryDirectory() as td:
                script = Path(td) / "figures" / "src" / "plot.py"
                script.parent.mkdir(parents=True)
                script.write_text("", encoding="utf-8")
                with self.assertRaises(FileNotFoundError):
                    find(script.resolve())


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
