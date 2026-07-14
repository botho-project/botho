---
name: primer-figures
description: Figurer for the primer skill. Produces teaching diagrams (mmdc → PNG) under exhibits/ and an optional PDF render of an AUDITED primer, reusing anvil/lib/render.py (pandoc-first, LaTeX opt-in) and anvil/lib/render_gate.py. No new rendering pipeline. Runs after the thread reaches AUDITED.
---

# primer-figures — Figurer

**Role**: figurer (produces optional teaching diagrams + an optional PDF render).
**Reads**: latest AUDITED `<thread>.{N}/<thread>.md` + `_progress.json`, `<thread>/refs/` (diagram source when the author supplies mermaid `.mmd` or figure specs), project `BRIEF.md`.
**Writes**: `<thread>.{N}/exhibits/` (rendered figures) and, optionally, `<thread>.{N}/<thread>.pdf` — all inside the (previously immutable) version dir's exhibits/render slots; the markdown body is never edited.

The primer skill ends at `AUDITED`; `primer-figures` is **optional collateral, not a state advance**. It is the natural place to produce teaching diagrams and the sibling PDF because both flow from the same pandoc invocation (the `report` precedent).

## Output format (reuses the shared render pipeline — no new plumbing)

Follows `report`'s markdown-source-of-truth + optional-PDF precedent exactly (SKILL.md §Output format):

- **Diagrams via `mmdc → PNG`** — the documented working diagram path (`report`/`pub` figure primitives). Message flows, commitment/coin lifecycles, and the end-to-end walkthrough are authored as mermaid and rendered to PNG under `<thread>.{N}/exhibits/`. Inline mermaid leaks as raw code in PDF (WORK_LOG PR #72) — always render to PNG and reference the image, never embed the mermaid source in the body.
- **PDF via pandoc (primary) / LaTeX (opt-in)** — `<thread>.pdf` is produced from `<thread>.md` via `anvil/lib/render.py` using the pandoc path by default; a consumer needing precise typography can drop a `.tex` template under `.anvil/skills/primer/assets/` for the LaTeX opt-in path (mirroring `report`'s "primary: pandoc, secondary: opt-in LaTeX"). **No third rendering path is invented.**

The version dir is self-contained for archival: `<thread>.md` (source-of-truth), `exhibits/` (rendered figures), `<thread>.pdf` (optional render) side by side.

## Procedure

1. **Discover state**: find the highest `N` with `<thread>.{N}/<thread>.md`. Confirm the thread is `AUDITED` (both `<thread>.{N}.review/` advance and `<thread>.{N}.audit/` clean) — else exit with a pointer to the missing lifecycle step. The figures phase runs on a converged primer, not a draft.
2. **Render diagrams (when present)**: for each mermaid source under `<thread>/refs/` (or inline figure specs the drafter recorded), render to PNG via `mmdc` and land the output under `<thread>.{N}/exhibits/`. A missing `mmdc` binary degrades gracefully (per the `check_*_available()` family in `anvil/lib/render.py`): skip the diagram, record the gap in `_progress.json.metadata.figures.skipped`, never abort the phase.
3. **Render-gate pre-flight (deterministic)**: run `anvil/lib/render_gate.py` over the render inputs (placeholder scan, compile-success check) before the expensive PDF render — the framework-wide "deterministic pre-flight before judgment" pattern.
4. **Produce the optional PDF**: invoke `anvil/lib/render.py` to render `<thread>.md` → `<thread>.{N}/<thread>.pdf` via the pandoc path (or the LaTeX opt-in when a consumer `.tex` template is present). A missing `pandoc` binary degrades gracefully — record the gap, do not abort.
5. **Record provenance** into `<thread>.{N}/_progress.json`: `phases.figures.state = done` (LAST write), `metadata.figures.rendered` (the PNGs produced), `metadata.figures.pdf` (the PDF path or `null` when the renderer was unavailable), `metadata.figures.skipped` (any gaps).
6. **Report**: e.g., `Figured botho-from-the-basics.3 → 4 diagrams under exhibits/, botho-from-the-basics.pdf rendered (pandoc). Thread is AUDITED with collateral; publish handoff per SKILL.md.`

## What primer-figures does NOT do

- **Never edits the markdown body.** The body is the source-of-truth; figures land in `exhibits/`, the PDF is a render of the body.
- **Never invents a rendering pipeline** — reuses `anvil/lib/render.py` + `anvil/lib/render_gate.py`. The consumer-pluggable figure-adapter registry (`report.figure_adapters`) and the LaTeX/TikZ authoring path are deferred (SKILL.md §Deferred).
- **Never advances the state machine** — `AUDITED` is terminal; figures are optional collateral.
- **Never aborts on a missing renderer binary** — graceful degradation (the `check_*_available()` precedent); the gap is recorded, not fatal.

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and `git.commit_per_phase` is `true`, end this phase: stage only the dirs this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N} [<state>]`, push if `git.push` is `true`. Git failures warn and continue. Default off.

This phase's specifics:

- **Ordering**: after the `_progress.json` `done` write lands.
- **Staging target**: ONLY this command's own `<thread>.{N}/exhibits/` + `<thread>.{N}/<thread>.pdf`.
- **Commit**: `anvil(primer/figures): <thread>.{N} [AUDITED]`.
