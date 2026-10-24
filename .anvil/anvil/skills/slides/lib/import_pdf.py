"""PDF → image-backed ``<thread>.1/`` importer for ``anvil:slides`` (issue #1368).

A talk authored outside anvil (Keynote/PowerPoint export, code-built PDF) has
no entry point into the slides lifecycle. :func:`import_pdf` turns such a PDF
into a lintable, reviewable first version:

- ``<thread>/<thread>.1/deck.md`` — one Marp slide per PDF page: a full-slide
  ``![bg fit](figures/slide-NN.png)`` of the rendered page plus an HTML comment
  holding that page's text (reading order, i.e. ``pdftotext`` WITHOUT
  ``-layout``) so ``evidence_check`` quotes resolve. Page text is a comment,
  never visible slide text.
- ``<thread>/<thread>.1/figures/slide-NN.png`` — ``pdftoppm`` renders.
- ``<thread>/<thread>.1/notes/NN-*.md`` — copied from ``--notes <dir>`` when
  given, empty stubs otherwise. Notes are never extracted from the PDF.
- ``<thread>/refs/original/`` — the untouched PDF plus ``manifest.json``
  (SHA-256, size, page count). Deliberately NOT ``<thread>.0/``: N=0 is
  reserved for the ``<thread>.0.outline/`` sibling.

Refuses (never overwrites) when ``<thread>.1/`` or a populated
``refs/original/`` already exists. All output is built in a staging dir and
renamed into place, so a failure leaves nothing half-written.

Subprocess-only: needs poppler (``pdfinfo``, ``pdftoppm``, ``pdftotext``);
:func:`check_poppler_available` is the graceful-degradation preflight, in the
shape of the ``check_*_available()`` family in ``anvil/lib/render.py``.

CLI::

    python3 import_pdf.py <thread-dir> --pdf <path> [--notes <dir>] [--dpi 150]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

POPPLER_BINARIES = ("pdfinfo", "pdftoppm", "pdftotext")

POPPLER_REMEDIATION = (
    "poppler-utils not found on PATH (need pdfinfo, pdftoppm, pdftotext) — "
    "required by `slides-import`. Install via `apt-get install poppler-utils` "
    "or `brew install poppler`, then re-run. Nothing was written."
)

DEFAULT_DPI = 150

_NOTES_NAME_RE = re.compile(r"^(\d+)-.+\.md$")
_PAGES_RE = re.compile(r"^Pages:\s+(\d+)\s*$", re.MULTILINE)


class ImportPdfError(Exception):
    """Raised for any refusal or failure; carries a legible message."""


@dataclass
class ImportResult:
    version_dir: Path
    original_dir: Path
    pages: int
    sha256: str
    warnings: list[str] = field(default_factory=list)


def check_poppler_available() -> bool:
    """Return ``True`` if every poppler binary the importer needs is on PATH."""
    return all(shutil.which(b) is not None for b in POPPLER_BINARIES)


def _run(cmd: list[str]) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            cmd, check=True, capture_output=True, text=True, encoding="utf-8",
            errors="replace",
        )
    except subprocess.CalledProcessError as exc:
        raise ImportPdfError(
            f"`{' '.join(cmd[:1])}` failed (exit {exc.returncode}): "
            f"{(exc.stderr or '').strip()}"
        ) from exc


def _page_count(pdf: Path) -> int:
    out = _run(["pdfinfo", str(pdf)]).stdout
    m = _PAGES_RE.search(out)
    if not m or int(m.group(1)) < 1:
        raise ImportPdfError(f"could not determine page count of {pdf}")
    return int(m.group(1))


def _page_text(pdf: Path, page: int) -> str:
    # No `-layout`: reading order, so multi-column pages are not interleaved.
    return _run(
        ["pdftotext", "-f", str(page), "-l", str(page), str(pdf), "-"]
    ).stdout


def sanitize_comment_text(text: str) -> str:
    """Make ``text`` safe inside an HTML comment that sits in a Marp deck.

    ``--`` runs would terminate the comment (``-->``) or form a ``---`` slide
    break; form feeds are pdftotext page terminators. Line structure is kept.
    """
    text = text.replace("\f", "")
    text = re.sub(r"-{2,}", "—", text)
    lines = [ln.rstrip() for ln in text.splitlines()]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines)


def _slide_name(page: int, pages: int) -> str:
    return f"slide-{page:0{max(2, len(str(pages)))}d}"


def render_deck_md(page_texts: list[str]) -> str:
    """Build the image-backed ``deck.md`` for ``page_texts`` (one per page)."""
    pages = len(page_texts)
    parts = [
        "---\nmarp: true\ntheme: anvil-slides-theme\nsize: 16:9\n"
        "paginate: false\nhtml: true\n---\n"
    ]
    for i, raw in enumerate(page_texts, start=1):
        text = sanitize_comment_text(raw) or "(no extractable text)"
        parts.append(
            f"![bg fit](figures/{_slide_name(i, pages)}.png)\n\n"
            f"<!--\nPage {i} text (imported from PDF, reading order):\n"
            f"{text}\n-->\n"
        )
    return "\n---\n\n".join(parts)


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _copy_notes(
    notes_dir: Path | None, dest: Path, pages: int, warnings: list[str]
) -> None:
    dest.mkdir()
    have: set[int] = set()
    if notes_dir is not None:
        if not notes_dir.is_dir():
            raise ImportPdfError(f"--notes is not a directory: {notes_dir}")
        for f in sorted(notes_dir.glob("*.md")):
            m = _NOTES_NAME_RE.match(f.name)
            if m:
                have.add(int(m.group(1)))
            else:
                warnings.append(
                    f"notes file {f.name!r} does not match NN-<slug>.md; "
                    "copied but not matched to a slide"
                )
            shutil.copy2(f, dest / f.name)
    for i in range(1, pages + 1):
        if i not in have:
            (dest / f"{i:02d}-{_slide_name(i, pages)}.md").write_text(
                f"# Slide {i} notes\n\n", encoding="utf-8"
            )


def import_pdf(
    thread_dir: Path,
    pdf: Path,
    *,
    notes_dir: Path | None = None,
    dpi: int = DEFAULT_DPI,
) -> ImportResult:
    """Import ``pdf`` as ``<thread_dir>/<thread>.1/``. See module docstring."""
    thread_dir = Path(thread_dir)
    pdf = Path(pdf)
    thread = thread_dir.name
    version_dir = thread_dir / f"{thread}.1"
    original_dir = thread_dir / "refs" / "original"

    if not check_poppler_available():
        raise ImportPdfError(POPPLER_REMEDIATION)
    if not pdf.is_file():
        raise ImportPdfError(f"PDF not found: {pdf}")
    if version_dir.exists():
        raise ImportPdfError(
            f"{version_dir} already exists; refusing to overwrite. "
            "Import is an entry point for a thread with no version yet."
        )
    if original_dir.exists() and any(original_dir.iterdir()):
        raise ImportPdfError(
            f"{original_dir} is already populated; refusing to overwrite the "
            "preserved original."
        )

    pages = _page_count(pdf)
    warnings: list[str] = []
    stage = thread_dir / f".{thread}.1.tmp"
    stage_orig = thread_dir / f".{thread}.refs-original.tmp"
    for d in (stage, stage_orig):
        if d.exists():
            shutil.rmtree(d)
    try:
        (stage / "figures").mkdir(parents=True)
        texts: list[str] = []
        for i in range(1, pages + 1):
            _run([
                "pdftoppm", "-png", "-r", str(dpi), "-f", str(i), "-l", str(i),
                "-singlefile", str(pdf),
                str(stage / "figures" / _slide_name(i, pages)),
            ])
            texts.append(_page_text(pdf, i))
            if not texts[-1].strip():
                warnings.append(f"page {i} has no extractable text")
        (stage / "deck.md").write_text(render_deck_md(texts), encoding="utf-8")
        _copy_notes(notes_dir, stage / "notes", pages, warnings)

        stage_orig.mkdir()
        shutil.copy2(pdf, stage_orig / pdf.name)
        digest = _sha256(stage_orig / pdf.name)
        (stage_orig / "manifest.json").write_text(
            json.dumps(
                {
                    "file": pdf.name,
                    "sha256": digest,
                    "bytes": (stage_orig / pdf.name).stat().st_size,
                    "pages": pages,
                    "source_path": str(pdf),
                },
                indent=2,
            ) + "\n",
            encoding="utf-8",
        )

        now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        (stage / "_progress.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "thread": thread,
                    "phases": {
                        "import": {"state": "done", "started": now, "completed": now}
                    },
                    "metadata": {
                        "iteration": 1,
                        "max_iterations": 4,
                        "imported_from": pdf.name,
                        "sha256": digest,
                    },
                },
                indent=2,
            ) + "\n",
            encoding="utf-8",
        )

        original_dir.parent.mkdir(parents=True, exist_ok=True)
        if original_dir.exists():  # empty dir (checked above)
            original_dir.rmdir()
        stage_orig.rename(original_dir)
        stage.rename(version_dir)
    except BaseException:
        for d in (stage, stage_orig):
            shutil.rmtree(d, ignore_errors=True)
        raise
    return ImportResult(version_dir, original_dir, pages, digest, warnings)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("thread_dir", type=Path)
    ap.add_argument("--pdf", type=Path, required=True)
    ap.add_argument("--notes", type=Path, default=None)
    ap.add_argument("--dpi", type=int, default=DEFAULT_DPI)
    args = ap.parse_args(argv)
    try:
        res = import_pdf(
            args.thread_dir, args.pdf, notes_dir=args.notes, dpi=args.dpi
        )
    except ImportPdfError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"imported {res.pages} page(s) -> {res.version_dir}")
    print(f"original preserved -> {res.original_dir} (sha256 {res.sha256})")
    for w in res.warnings:
        print(f"warning: {w}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
