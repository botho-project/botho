---
name: memoir-figures
description: Figurer for the memoir skill. Renders diagrams (mmdc → PNG) and resolves the memoir-local photo-placement macros (\famphoto/\fullphoto/\marginphoto) against anvil:project-photos' manifest.json, plus an optional PDF from the LaTeX source, reusing anvil/lib/render.py + anvil/lib/render_gate.py. No new rendering pipeline. Runs any time after draft/revise (no AUDITED gate).
---

# memoir-figures — Figurer

**Role**: figurer (renders drafter-planned diagrams + resolves
photo-placement macros against `project-photos`' manifest + an optional
PDF from the LaTeX source).
**Reads**: latest `<thread>.{N}/<thread>.tex` + `_progress.json`,
`<thread>/refs/` (diagram source when the author supplies mermaid `.mmd`),
`project-photos`' `manifest.json` (path resolved from the project BRIEF
or a default sibling location — see §Manifest resolution), project
`BRIEF.md`.
**Writes**: `<thread>.{N}/exhibits/` (rendered diagrams) and, optionally,
`<thread>.{N}/<thread>.pdf` — the LaTeX body is never edited.

`memoir-figures` is **collateral, not a state advance** — it does not
move the state machine. It runs **any time after draft/revise** (not
gated on `AUDITED`), mirroring `report-figures`/`primer-figures`/
`spec-figures`, so `memoir-review`/`memoir-audit` can see and score the
rendered output.

## Output format (reuses the shared render pipeline — no new plumbing)

A memoir chapter's body is LaTeX (SKILL.md §Output format):

- **Diagrams via `mmdc → PNG`** — the documented working diagram path
  (`report`/`paper`/`primer`/`spec` precedent). Rendered to PNG; the body
  already references them via `\includegraphics{exhibits/figN-slug.png}`.
- **PDF via the LaTeX pipeline** — `<thread>.pdf` is produced from
  `<thread>.tex` via `anvil/lib/render.py`'s LaTeX/xelatex path, gated by
  `anvil/lib/render_gate.py` (the LaTeX-skill analog of `marp_lint`).
  **No third rendering path is invented.**

## Photo-placement macro resolution (memoir-local, new work)

`project-photos`' own SKILL.md explicitly scopes placement macros out of
its surface ("consumer extension points via per-skill template preamble
overrides"). This skill's `templates/memoir.template.tex` preamble
defines three macros the drafter/reviser use in chapter bodies:

```latex
\famphoto{<stable-name>}{<caption>}    % standard in-text family photo
\fullphoto{<stable-name>}{<caption>}   % full-page/plate photo
\marginphoto{<stable-name>}{<caption>} % small margin inset
```

### Manifest resolution

Read `manifest.json` from the path declared in the project BRIEF's
`photos:` key when present (`photos: <path-to-manifest.json>`), else the
default sibling location `<project_dir>/manifest.json` (the location
`anvil:project-photos` writes to by default). A missing manifest when
the body contains ANY placement macro call is a **render-gate finding**
(not a crash): report `"no manifest.json resolved; N photo-placement
macro(s) in <thread>.tex cannot be checked"` and proceed with diagrams
and PDF rendering (graceful degradation).

### Resolution procedure

1. Scan `<thread>.{N}/<thread>.tex` for `\famphoto{...}`,
   `\fullphoto{...}`, `\marginphoto{...}` calls; extract each
   `<stable-name>` argument.
2. For each extracted stable name, look it up against
   `manifest.json`'s `entries[].stable` (per
   `anvil/skills/project-photos/SKILL.md` §Manifest output contract).
3. **A stable name present in the manifest** resolves cleanly — no
   finding; the macro's underlying `\includegraphics` target is the
   photo's location as declared by the consumer's own normalization step
   (`project-photos` produces the provenance map only — it never
   renames/rotates/crops the source image; see SKILL.md §Photo-placement
   contract).
4. **A stable name NOT present in `manifest.json`** — this is the
   documented edge case (issue #740's own test plan): surfaces as a
   `major` **render-gate finding** in `_progress.json.metadata.figures.unresolved_photos`
   (a list of `{macro, stable_name, location}` entries) — **never a silent placeholder, never a crash**.
   `memoir-review`'s dim-7 (Structure & chapter navigation) scoring reads this list.
5. **`project-photos` is never invoked to regenerate or mutate the
   manifest** — strictly read-only consumption, matching
   `project-photos`' own strictly-read-only-over-source-images contract.

## Procedure

1. **Discover state**: find the highest `N` with `<thread>.{N}/<thread>.tex`.
   **No terminal-state gate** — runs any time after the draft exists.
2. **Resume check**: if every referenced diagram/photo resolves AND
   `<thread>.pdf` is newer than the body AND `phases.figures.state ==
   done`, exit early (idempotent). **Zero-figure, zero-photo thread**
   (no `\includegraphics`/`\famphoto`/`\fullphoto`/`\marginphoto`
   references in the body): this step is a silent no-op for diagrams and
   photos; proceed to the optional PDF (step 5) directly.
3. **Render diagrams**: for each `\includegraphics{exhibits/<filename>}`
   reference, render its mermaid source (a `.mmd` under `<thread>/refs/`
   or a recorded inline spec) to PNG via `mmdc`, landing the output at
   exactly the referenced path. A missing `mmdc` binary (or an
   unlaunchable pinned Chromium — `check_mmdc_launchable()`) degrades
   gracefully: skip the diagram, record the gap in
   `_progress.json.metadata.figures.skipped`, never abort the phase.
4. **Resolve photo-placement macros**: per §Photo-placement macro
   resolution above.
5. **Render-gate pre-flight (deterministic)**: run
   `anvil/lib/render_gate.py` over the LaTeX render inputs (placeholder
   scan, compile-success check) before the expensive PDF render.
6. **Produce the optional PDF**: invoke `anvil/lib/render.py` to render
   `<thread>.tex` → `<thread>.{N}/<thread>.pdf` via the xelatex/LaTeX
   path. A missing `xelatex` degrades gracefully.
7. **Validate by file existence**: after rendering, assert that every
   `\includegraphics{exhibits/<filename>}` reference resolves to a file
   that now exists under `<thread>.{N}/exhibits/`. Any reference whose
   target is still missing is recorded in
   `metadata.figures.unresolved` — a deterministic record the review's
   figure-existence check reads (not a fatal error here; graceful
   degradation).
8. **Record provenance** into `<thread>.{N}/_progress.json`:
   `phases.figures.state = done` (LAST write),
   `metadata.figures.rendered`, `metadata.figures.pdf`,
   `metadata.figures.skipped`, `metadata.figures.unresolved`,
   `metadata.figures.unresolved_photos` (per §Photo-placement macro
   resolution step 4).
9. **Report**: e.g., `Figured 00-introduction.2 → 2 diagrams rendered
   (all resolved), 5 photo macros resolved / 1 unresolved (stable name
   "045.jpg" not in manifest.json), 00-introduction.pdf rendered
   (xelatex). Next: memoir-review + memoir-audit can now score the
   rendered output.`

## What memoir-figures does NOT do

- **Never invents new body content or new figure/photo references.**
  The body already contains the macro calls the drafter/reviser placed;
  the figurer only fills in / resolves what those calls point at.
- **Never invents a rendering pipeline** — reuses `anvil/lib/render.py`
  (LaTeX/xelatex) + `anvil/lib/render_gate.py`.
- **Never mutates `manifest.json` or any source photo** — strictly
  read-only consumption of `project-photos`' output.
- **Never advances the state machine** — figures are collateral.
- **Never aborts on a missing renderer binary or a missing manifest** —
  graceful degradation; the gap is recorded and surfaced at review.

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and
`git.commit_per_phase` is `true`, end this phase: stage only the dirs
this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N}
[<state>]`, push if `git.push` is `true`. Git failures warn and
continue. Default off.

This phase's specifics:

- **Ordering**: after the `_progress.json` `done` write lands.
- **Staging target**: ONLY this command's own `<thread>.{N}/exhibits/` +
  `<thread>.{N}/<thread>.pdf`.
- **Commit**: `anvil(memoir/figures): <thread>.{N} [<state>]` (the
  bracket carries the thread's current derived state per SKILL.md
  §State machine — figures does not advance the state machine and may
  run before the thread is `AUDITED`).
