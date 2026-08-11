---
name: memoir-draft
description: Drafter for the memoir skill. Produces the first version of a chapter (LaTeX body) from the project BRIEF, the resolved dual-corpus (#597) and dual-voice (#598) tiers, and any thread refs, writing a provenance.md claim-to-source map before prose. EMPTY → DRAFTED transition.
---

# memoir-draft — Drafter

**Role**: drafter.
**Reads**: project `BRIEF.md` (matching `documents:` entry with
`artifact_type: memoir` + top-level `corpus:` + top-level `voice:`
block), the resolved corpus + voice docs, `<thread>/refs/`, shared
`research/` (when present); for re-drafts after a crashed pass, the
partial `_progress.json`.
**Writes**: `<thread>.{N}/<thread>.tex` (+ `provenance.md` when the
corpus tier is active) + `_progress.json` (new version dir; immutable
once `done`).

This is the `EMPTY → DRAFTED` transition for ONE chapter thread.
(Revisions go through `memoir-revise`, which produces `<thread>.{N+1}/`
from critic feedback — this command drafts v1, or a fresh v1 after an
abandoned thread.)

## Procedure

1. **Discover state**: confirm no `<thread>.{N}/` exists yet (else exit
   with a pointer to `memoir-review`/`memoir-audit`/`memoir-revise` per
   the state table in SKILL.md). Create `<thread>.1/` and initialize
   `_progress.json` (`phases.draft.state = in_progress`, per
   `anvil/lib/snippets/progress.md`).
2. **Read the project BRIEF**: locate the matching `documents:` entry
   (slug = thread dir name; `artifact_type: memoir`). Read
   `target_length` when declared; a memoir chapter is long-form by
   nature — there is no short envelope. Record the resolved target in
   `_progress.json.metadata.target_length_resolved` when declared.

   **Resolve and mirror the iteration cap** (issue #869) into
   `_progress.json.metadata`, so v1 carries the same audit trail every
   later version does — see SKILL.md §"Iteration cap and override
   contract":
   - When the entry declares BOTH `max_iterations` AND
     `iteration_cap_rationale`, write both values into
     `metadata.max_iterations` / `metadata.iteration_cap_rationale`, and
     confirm the elevated cap in the step-10 status line, e.g.
     `... max_iterations=6 (BRIEF override active)`. The BRIEF parser
     enforces the paired-override validation at parse time (both keys,
     `max_iterations >= 4`, non-empty rationale) — the drafter does NOT
     re-validate, and propagates a `ValueError` rather than degrading
     silently.
   - Otherwise write `metadata.max_iterations =
     anvil/lib/project_brief.py::DEFAULT_MAX_ITERATIONS` (4) and
     `metadata.iteration_cap_rationale = null`. No warning — both keys
     absent is the default-cap case.
   - **Never write `metadata.revision_class`.** That key classifies a
     *revision* pass (`"map_only"` / `"substantive"`, written by
     `memoir-revise` step 9); `<thread>.1/` is a draft, so it is
     unclassified by construction. It still consumes one iteration of the
     budget, and the BLOCKED notice reports it as the standalone
     `v1 draft` term of its budget-composition line (see
     `memoir-revise.md` §"BLOCKED notice" item 3).
3. **Load corpus grounding (conditional — #597)**: invoke
   `anvil/lib/project_brief.py::resolve_corpus_dirs(<project_dir>)` per
   `anvil/lib/snippets/provenance.md` §Section 1. The `<project_dir>` is
   the PROJECT root (`corpus:` is declared once at the project level per
   SKILL.md §Dual-corpus provenance, never per-chapter).
   - **When active** (>=1 resolved dir): write `<thread>.1/provenance.md`
     **before prose**, per §Section 2 — one markdown table row per
     attributed quote (verbatim, in quotes) and per checkable factual
     claim (named dates, names, events, places), each mapping to its
     supporting corpus passage (`Source file` relative to a declared
     corpus dir + `Line range` + an `Anchor` — a short verbatim quoted
     snippet copied exactly from the cited passage, per §Section 2
     "Anchor: the stable identity"). The `Line range` is a hint; the
     `Anchor` is the row's real identity and is what an audit's
     anchor-drift pre-pass (§Section 4a) searches for. **Fabricating a source-line mapping is prohibited** — if no corpus passage supports
     a claim, cut the claim or record it with a `NOT_FOUND` source note
     (leave `Anchor` blank on a `NOT_FOUND` row — there is nothing to
     anchor); do NOT invent a citation. **Record the resolved corpus dir
     paths in `_progress.json.metadata.corpus_dirs_resolved`.**
   - **When inactive** (no `corpus:` key, `corpus: null`, or
     `corpus: []`): omit `metadata.corpus_dirs_resolved` entirely and
     draft without a provenance map. Do NOT invent a provenance
     contract. Byte-identical to the corpus-tier-absent posture.
   - **Declared-but-missing dirs**: proceed with whatever resolved
     (`resolve_corpus_dirs` returns `missing: true` entries, never raises);
     the critics surface the broken declaration as a `major` finding.
4. **Load author voice grounding (conditional — #461, narrator tier)**:
   invoke `anvil/lib/project_brief.py::resolve_voice_docs(<project_dir>)`
   per `anvil/lib/snippets/voice_grounding.md`. When active, load values →
   style_guide → vocabulary → corpus exemplars and choose 3-5
   voice-matched exemplars for the NARRATOR framing prose (never the
   reconstructed dialogue — that is step 5). Record
   `_progress.json.metadata.voice_exemplars`. When inactive, omit the
   field and draft without narrator calibration.
5. **Load subject voice grounding (conditional — #598, dialogue tier)**:
   invoke
   `anvil/lib/project_brief.py::resolve_subject_voice_docs(<project_dir>)`
   (same `<project_dir>`; the subject tier activates independently of
   the author tier) per `anvil/lib/snippets/voice_grounding.md`
   §"Subject voice tier". For each subject whose dialogue this chapter
   renders, load its resolved `corpus` (spoken transcripts) and
   `voice_doc` when present. Ground every reconstructed line in that
   speaker's recorded register — clipped declaratives stay clipped; do
   not smooth speech into balanced multi-clause prose. **Record the
   consulted transcript paths in
   `_progress.json.metadata.subject_voice_exemplars`** (a per-subject
   map). When inactive (no `subjects` list declared), omit the field and
   draft with no subject dialogue calibration.
6. **Ingest evidence**: read `<thread>/refs/` text-readable materials and
   the shared `research/` pool (when present) as authoritative substrate
   beyond the declared `corpus:` roots (letters/notes not part of the
   formal ground-truth corpus, background research, ADR-style planning
   notes).
7. **Draft the body** to `<thread>.1/<thread>.tex` (the filename **echoes
   the slug** per #295 — never `chapter.tex`, never `memoir.tex`), using
   the `templates/memoir.template.tex` skeleton:
   - **Sourcing discipline (dim 1 — the dominant dim)**: every
     reconstructed quote and every checkable factual claim traces to a
     `provenance.md` row when the corpus tier is active. When the corpus
     tier is inactive, do not invent unverifiable specificity — prefer
     honest vagueness over false precision.
   - **Narrator vs. subject voice (dims 2/3)**: narrator framing prose in
     the author's own persona; reconstructed dialogue in the speaker's
     own recorded cadence — the two tiers are interleaved WITHIN the same
     chapter, never conflated.
   - **Scene craft (dim 4)**: scenes resolve, don't just recount; the
     chapter earns its place in the book's throughline.
   - **Photo placement (when a manifest exists)**: place
     `\famphoto{<stable-name>}{<caption>}` / `\fullphoto{...}` /
     `\marginphoto{...}` macro calls at the point in the body where a
     photo belongs (SKILL.md §Photo-placement contract). A referenced
     stable name that does not yet resolve is expected and correct pre-
     `memoir-figures` (the same "broken reference is tolerated before the
     figurer runs" contract as `primer`/`spec`'s figure-plan references).
8. **Self-check** into `_progress.json.metadata.self_check`: word/section
   count vs target, corpus exemplars consulted (per §3, when active),
   author voice exemplars consulted (per §4, when active), subject voice
   exemplars consulted per speaker (per §5, when active), and a
   sourcing-discipline note (every reconstructed quote traced or
   explicitly marked NOT_FOUND).
9. **Finalize**: set `phases.draft.state = done` (the `_progress.json`
   write is LAST so crash recovery per `anvil/lib/snippets/progress.md`
   sees an incomplete phase, not a half-blessed one).
10. **Report**: e.g., `Drafted 00-introduction.1 (2,400 words; corpus tier
    active — 2 roots, 18 provenance rows; author voice active; 1 subject
    (grani) active). Next: memoir-review + memoir-audit 00-introduction
    (parallel)`.

## What memoir-draft does NOT do

- **No image rendering.** The drafter places macro references
  (`\famphoto{...}` etc.) and diagram references that don't yet resolve;
  rendering stays exclusively `memoir-figures`'s job.
- **No review-side or audit-side gates.** The rubric scoring and the
  factual/narrative-consistency + exhaustive corpus-provenance audits run
  in `memoir-review`/`memoir-audit`; the drafter's step-7 disciplines
  exist to pass them, not to replace them.
- **Never writes the corpus or the voice docs.** Both are
  operator-declared project-level artifacts; an absent contract is
  surfaced by the critics, not silently filled in by the drafter.
- **Never fabricates a `provenance.md` source-line mapping.** A claim
  with no supporting passage is cut or recorded `NOT_FOUND` — never
  invented.

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md` (`.anvil/anvil/lib/snippets/git_sync.md`
in an installed consumer repo): if `.anvil/config.json` exists and
`git.commit_per_phase` is `true`, end this phase: stage only the dirs
this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N}
[<state>]`, push if `git.push` is `true`. Git failures warn and continue
— never fail the phase. Default off.

This phase's specifics:

- **Ordering**: after the `_progress.json` `done` write lands.
- **Staging target**: ONLY this command's own `<thread>.{N}/` version
  dir.
- **Commit**: `anvil(memoir/draft): <thread>.{N} [DRAFTED]`.
