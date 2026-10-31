"""Tests for ``anvil.skills.slides.lib.import_pdf`` (issue #1368).

poppler is stubbed at the ``_run`` seam, so these need no real poppler and no
real PDF — only a placeholder file whose bytes are hashed.
"""

from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from anvil.lib.marp_lint import lint_source
from anvil.skills.slides.lib import import_pdf as ip

PAGE_TEXTS = {
    1: "Title slide\nSpeaker --- Venue\n",
    2: "Column A line 1\nColumn A line 2\nColumn B line 1\n\f",
    3: "",  # image-only page, no extractable text
}


def _fake_run(pages: dict[int, str]):
    def run(cmd: list[str]):
        out = mock.Mock()
        if cmd[0] == "pdfinfo":
            out.stdout = f"Title: x\nPages:          {len(pages)}\n"
        elif cmd[0] == "pdftoppm":
            assert "-singlefile" in cmd
            Path(cmd[-1] + ".png").write_bytes(b"\x89PNG fake")
            out.stdout = ""
        elif cmd[0] == "pdftotext":
            assert "-layout" not in cmd  # reading order, not layout mode
            out.stdout = pages[int(cmd[cmd.index("-f") + 1])]
        else:  # pragma: no cover
            raise AssertionError(cmd)
        return out

    return run


class ImportPdfTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.thread = root / "talk"
        self.thread.mkdir()
        self.pdf = root / "orig.pdf"
        self.pdf.write_bytes(b"%PDF-1.4 placeholder")
        for patcher in (
            mock.patch.object(ip, "check_poppler_available", return_value=True),
            mock.patch.object(ip, "_run", side_effect=_fake_run(PAGE_TEXTS)),
        ):
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_one_slide_per_page_with_text_comments(self) -> None:
        res = ip.import_pdf(self.thread, self.pdf)
        self.assertEqual(res.pages, 3)
        deck = (res.version_dir / "deck.md").read_text(encoding="utf-8")
        for n in (1, 2, 3):
            self.assertIn(f"![bg fit](figures/slide-0{n}.png)", deck)
            self.assertTrue((res.version_dir / "figures" / f"slide-0{n}.png").is_file())
        self.assertIn("Column A line 1\nColumn A line 2\nColumn B line 1", deck)
        self.assertIn("(no extractable text)", deck)
        # `---` in page text must not become a slide break or end the comment.
        self.assertNotIn("Speaker ---", deck)
        self.assertEqual(len(lint_source(deck).infos), 3)

    def test_lint_reports_info_not_warnings_for_imported_deck(self) -> None:
        res = ip.import_pdf(self.thread, self.pdf)
        result = lint_source((res.version_dir / "deck.md").read_text(encoding="utf-8"))
        self.assertEqual((len(result.errors), len(result.warnings)), (0, 0))
        self.assertEqual({f.severity for f in result.infos}, {"info"})
        self.assertEqual(
            {f.message for f in result.infos},
            {"image-only slide — lint not applicable"},
        )

    def test_original_preserved_with_hash_not_in_version_zero(self) -> None:
        res = ip.import_pdf(self.thread, self.pdf)
        orig = self.thread / "refs" / "original"
        self.assertEqual(res.original_dir, orig)
        self.assertEqual((orig / "orig.pdf").read_bytes(), self.pdf.read_bytes())
        manifest = json.loads((orig / "manifest.json").read_text())
        self.assertEqual(manifest["sha256"], hashlib.sha256(self.pdf.read_bytes()).hexdigest())
        self.assertEqual(manifest["pages"], 3)
        self.assertFalse((self.thread / "talk.0").exists())
        self.assertFalse((self.thread / ".talk.1.tmp").exists())

    def test_notes_stubs_and_copy(self) -> None:
        res = ip.import_pdf(self.thread, self.pdf)
        self.assertEqual(
            sorted(p.name for p in (res.version_dir / "notes").iterdir()),
            ["01-slide-01.md", "02-slide-02.md", "03-slide-03.md"],
        )

    def test_notes_dir_copied_and_gaps_stubbed(self) -> None:
        notes = Path(self._tmp.name) / "mynotes"
        notes.mkdir()
        (notes / "02-intro.md").write_text("say hi", encoding="utf-8")
        (notes / "readme.md").write_text("x", encoding="utf-8")
        res = ip.import_pdf(self.thread, self.pdf, notes_dir=notes)
        names = sorted(p.name for p in (res.version_dir / "notes").iterdir())
        self.assertIn("02-intro.md", names)
        self.assertIn("01-slide-01.md", names)
        self.assertNotIn("02-slide-02.md", names)
        self.assertEqual(
            (res.version_dir / "notes" / "02-intro.md").read_text(), "say hi"
        )
        self.assertTrue(any("readme.md" in w for w in res.warnings))

    def test_refuses_existing_version_dir(self) -> None:
        (self.thread / "talk.1").mkdir()
        with self.assertRaises(ip.ImportPdfError):
            ip.import_pdf(self.thread, self.pdf)
        self.assertFalse((self.thread / "refs").exists())

    def test_refuses_rerun(self) -> None:
        ip.import_pdf(self.thread, self.pdf)
        with self.assertRaises(ip.ImportPdfError):
            ip.import_pdf(self.thread, self.pdf)

    def test_single_page(self) -> None:
        with mock.patch.object(ip, "_run", side_effect=_fake_run({1: "only"})):
            res = ip.import_pdf(self.thread, self.pdf)
        self.assertEqual(res.pages, 1)
        self.assertEqual(
            (res.version_dir / "deck.md").read_text().count("![bg fit]"), 1
        )

    def test_failure_leaves_nothing_behind(self) -> None:
        def boom(cmd):
            if cmd[0] == "pdftotext":
                raise ip.ImportPdfError("boom")
            return _fake_run(PAGE_TEXTS)(cmd)

        with mock.patch.object(ip, "_run", side_effect=boom):
            with self.assertRaises(ip.ImportPdfError):
                ip.import_pdf(self.thread, self.pdf)
        self.assertEqual(list(self.thread.iterdir()), [])


class PreflightTests(unittest.TestCase):
    def test_missing_poppler_degrades_with_remediation_and_writes_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            thread = Path(tmp) / "talk"
            thread.mkdir()
            pdf = Path(tmp) / "a.pdf"
            pdf.write_bytes(b"x")
            with mock.patch.object(ip.shutil, "which", return_value=None):
                self.assertFalse(ip.check_poppler_available())
                with self.assertRaises(ip.ImportPdfError) as ctx:
                    ip.import_pdf(thread, pdf)
            self.assertIn("poppler", str(ctx.exception))
            self.assertEqual(list(thread.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
