# anvil:memoir

Chaptered narrative nonfiction reconstructed from a private evidentiary
corpus — family memoirs, oral histories, biography-from-archive,
interview-grounded long-form profiles. Each chapter is its own thread
under this same skill, sharing one project-level `BRIEF.md`; `AUDITED` is
the terminal state (no shortcut to `READY`). Produced via the
report-shaped anvil lifecycle (`draft → review + audit (parallel) →
revise → … → AUDITED → figures`), assembled into a compiled book via
`anvil:project-book`.

**v1 scope (issue #740).** This directory ships the skeleton: the skill,
the rubric, the six lifecycle command docs, templates, `ArtifactType.MEMOIR`
registration, and a minimal synthetic worked example. See SKILL.md
§"Scope guard — v1 / deferred" for what is out of scope.

## Quick orientation

| File | What it is |
|---|---|
| `SKILL.md` | Artifact contract, dual-corpus (#597) + dual-voice (#598) adoption, the `<thread>.{N}.corpus-audit/` exhaustive audit-critic sibling, photo-placement contract (relationship to `anvil:project-photos`), the "book assembly is out of scope" contract (relationship to `anvil:project-book`), the state machine (parallel review+audit, AUDITED-terminal), and the "Relationship to `anvil:essay`/`anvil:primer`/`anvil:spec`/`anvil:report`" positioning table. Read this first. |
| `rubric.md` | 9-dimension /44 scorecard (`anvil-memoir-v1`). **≥39 advances** (the audit-grade band). Sourcing fidelity at weight 7 (dominant); the reused-verbatim #597 fabrication-class flags plus the conditional #598 misattribution flag. |
| `commands/memoir.md` | Per-chapter-thread status orchestrator (read-only) — NOT a portfolio view; use `/anvil:project-book` for that. |
| `commands/memoir-draft.md` | Drafter. Wires `corpus:` + `voice:` (author + subjects) tiers, writes `provenance.md` before prose. |
| `commands/memoir-review.md` | Reviewer. Scores the /44 rubric; runs the provenance back-check sample; scores the dual voice tiers. |
| `commands/memoir-audit.md` | Auditor. Always writes a general factual/narrative-consistency sibling; when the corpus tier is active, ALSO writes the exhaustive `<thread>.{N}.corpus-audit/` sibling (five-way classification, fabrication-class flags). |
| `commands/memoir-revise.md` | Reviser. Consumes review + audit + (when present) corpus-audit; never fabricates a `provenance.md` mapping. |
| `commands/memoir-figures.md` | Figurer. Diagrams (`mmdc → PNG`) + the memoir-local photo-placement macros (`\famphoto`/`\fullphoto`/`\marginphoto`) resolved against `project-photos`' `manifest.json` + optional PDF. |
| `templates/BRIEF.md.example` | Project-level BRIEF with a `documents:` list (multiple `artifact_type: memoir` chapters), a `corpus:` list, and a `voice:` block with `subjects:`. |
| `templates/memoir.template.tex` | LaTeX chapter skeleton with the photo-placement macro stubs. |

## What is distinctive in this skill

1. **Sourcing fidelity is the owned dominant dimension** — dim 1 carries
   weight 7, the way `primer` weights pedagogy and `spec` weights
   normative correctness.
2. **Two simultaneous, independently-activated voice tiers in one
   document** — narrator prose scored against the author-persona `voice:`
   docs, reconstructed dialogue scored against each subject's spoken
   corpus (#598), interleaved within the same chapter.
3. **The first skill to implement the exhaustive `kind: tool_evidence`
   corpus-audit critic** (`anvil/lib/snippets/provenance.md` §Section 4) —
   a dedicated `<thread>.{N}.corpus-audit/` sibling coexisting with a
   general `.audit/` sibling.
4. **Chapter-thread-native** — one thread per chapter, all owned by this
   skill, assembled by the already-shipped `anvil:project-book` (this
   skill deliberately does not rebuild that portfolio view).
5. **Photo-placement macros** — `project-photos` explicitly scopes
   placement macros out of its own surface; this skill's template
   preamble defines them and resolves stable names against
   `project-photos`' `manifest.json`.

## Deferred (see SKILL.md §Scope guard)

Cross-chapter consistency checking; a dedicated structured facts register;
a full `nitas-mama` dogfood worked example; voice-grounding wiring beyond
what #598 already generalizes; LaTeX/TikZ figure paths beyond the
photo-placement macros.
