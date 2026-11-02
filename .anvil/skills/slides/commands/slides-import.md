---
name: slides-import
description: Import command for the slides skill. Turns an externally-authored PDF talk (Keynote/PowerPoint export, code-built PDF) into an image-backed, lintable `<thread>.1/` plus a preserved original under `refs/original/`.
---

# slides-import — Import an existing PDF deck

**Role**: importer (entry point for a deck authored outside anvil).
**Reads**: the PDF passed via `--pdf`; optionally a speaker-notes directory via `--notes`.
**Writes**: `<thread>/<thread>.1/` (`deck.md`, `figures/slide-NN.png`, `notes/NN-*.md`, `_progress.json`) and `<thread>/refs/original/` (the untouched PDF + `manifest.json`). Bare `<thread>.1/` references below are shorthand for the nested path.

## Why this command exists

`slides-draft` assumes anvil authors the Marp `deck.md`. A talk built elsewhere has no way in; the canary (#1368) hand-wrote a consumer-side bridge to get one through review/audit/vision. This command is that bridge: the **page images are the slides**, and each page's text rides along as an HTML comment so the audit's quote checks (`evidence_check`) can resolve claims against it.

## Inputs

- **Thread slug** (positional argument); the thread root `<thread>/` must exist (a `BRIEF.md` is expected but not read by the importer).
- `--pdf <path>` (required): the source PDF.
- `--notes <dir>` (optional): a directory of `NN-*.md` speaker-notes files, copied to `<thread>.1/notes/`. Without it, an empty stub is written per slide. Notes are NOT extracted from the PDF.
- `--dpi <n>` (optional, default 150): `pdftoppm` render resolution.

## Procedure

1. **Preflight**: poppler (`pdfinfo`, `pdftoppm`, `pdftotext`) must be on PATH. If absent, print the remediation (`apt-get install poppler-utils` / `brew install poppler`), write nothing, and stop.
2. **Refuse to overwrite**: if `<thread>.1/` exists, or `refs/original/` is already populated, stop with an error. Import is an entry point for a thread with no version yet.
3. **Run the helper**:

   ```bash
   python3 .anvil/skills/slides/lib/import_pdf.py <thread> --pdf <path> [--notes <dir>] [--dpi 150]
   ```

   (in the anvil source repo: `python3 anvil/skills/slides/lib/import_pdf.py …`). The helper builds everything in a staging dir and renames it into place, so a failure leaves nothing half-written.
4. **What it produces**:
   - `deck.md` — one Marp slide per PDF page: `![bg fit](figures/slide-NN.png)` followed by an HTML comment holding that page's text from `pdftotext` **without** `-layout` (reading order, so multi-column pages are not interleaved). Page text is a comment, never visible slide text. `--` runs in the text are collapsed to `—` so they cannot close the comment or form a `---` slide break.
   - `figures/slide-NN.png` — the rendered pages.
   - `notes/` — copied or stubbed as above.
   - `refs/original/<name>.pdf` + `manifest.json` (SHA-256, byte size, page count). This is **not** `<thread>.0/`: `N=0` is reserved for `<thread>.0.outline/`, and `slides.md` step 5's outline exemption is untouched.
5. **Report**: print the page count, the version dir, the original's SHA-256, and any warnings (pages with no extractable text; notes files not named `NN-<slug>.md`).

## After import

The thread is `DRAFTED` at `N=1`; run `slides-review` / `slides-audit` / `slides-rehearse` as usual. The lint reports `info: image-only slide — lint not applicable` per slide (not a pass — the lint cannot measure a picture).

To make `slides-revise` edit the real source instead of `deck.md`, add the `slides.external_source` block to the thread `BRIEF.md` — see `slides-revise.md` § "External-source threads".

## Out of scope

Native Keynote/PPTX/Google Slides parsing (export to PDF first), OCR of image-only PDFs, speaker-notes extraction from the PDF, and any consumer-specific build script (only the `build` command string is ever invoked).

## `_progress.json`

The importer writes `phases.import.state = done` with `metadata.iteration = 1`, `metadata.max_iterations = 4`, `metadata.imported_from`, and `metadata.sha256`.
