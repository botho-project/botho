---
name: memoir
description: Draft, review, revise, and audit chaptered narrative nonfiction (family memoirs, oral histories, biography-from-archive, interview-grounded long-form profiles) reconstructed from a private evidentiary corpus. Each chapter is its own thread under the same skill; AUDITED is the terminal state — no shortcut to READY. Assembles into a book via anvil:project-book.
domain: memoir
type: skill
user-invocable: false
---

# anvil:memoir — Chaptered narrative nonfiction from a private evidentiary corpus

The `memoir` skill produces **long-form narrative nonfiction told in book
chapters, reconstructing scenes and dialogue from a private evidentiary
corpus rather than published sources**: family memoirs, oral histories,
biography-from-archive, journalistic long-form profiles built on recorded
interviews. The canary is `nitas-mama` — a family memoir of Mattie Lee Greer
Fraker narrated by her grandson from ~4,600 lines of interview transcripts
and nine collected family letters, structured as an introduction + six
chapters + appendix, each independently drafted/reviewed/revised/audited and
assembled into one `book.pdf`. Consistent with "skill identity = artifact identity"
(CLAUDE.md), `anvil:memoir` is a NEW skill directory (`anvil/skills/memoir/`)
— not a parameterization of `anvil:essay`, `anvil:primer`, or `anvil:spec`.

What makes the class distinct is that it composes **four already-shipped
primitives** in one artifact for the first time: dual-corpus claim
provenance (#597), dual voice tiers active at once (#598), chapter-thread
assembly via `anvil:project-book` (#596), and photo embedding via
`anvil:project-photos` (#599). No existing skill is this shape — see
§Relationship to other skills below.

## Artifact contract

A **memoir project** carries one or more **chapter threads**, each owned by
this same skill, sharing one project-level `BRIEF.md`:

```
<project>/                     Project root
  BRIEF.md                     Project-level brief: documents: list (one
                               entry per chapter, artifact_type: memoir),
                               top-level corpus: list (#597), top-level
                               voice: block with subjects: (#598), optional
                               build: block consumed by anvil:project-book
  transcripts/, letters/, …    The declared corpus: roots (read-only
                               ground truth — see §Dual-corpus provenance)
  research/                    Optional shared evidence pool
  00-introduction/             Chapter thread (named for the chapter slug)
    refs/                      Optional reference material for this chapter
    00-introduction.1/         First drafted version (immutable once written)
      00-introduction.tex      Chapter body (LaTeX; filename echoes the
                               slug per #295 — never chapter.tex, never
                               memoir.tex)
      provenance.md            Claim -> source map (#597 §Section 2; only
                               when the corpus tier is active)
      _progress.json           Phase state for this version
      changelog.md              (revisions only)
    00-introduction.1.review/   Reviewer sibling (rubric /44)
    00-introduction.1.audit/    Auditor sibling (factual + narrative
                               consistency; ALWAYS written)
    00-introduction.1.corpus-audit/  Exhaustive provenance audit sibling
                               (#597 §Section 4; ONLY when the corpus tier
                               is active)
    00-introduction.2/         Revised version (consumes v1 + ALL siblings)
    ...
  01-childhood/ ...
  appendix/ ...
```

Every chapter is a normal versioned-dir thread under the framework's
standard grammar (`anvil/lib/snippets/version_layout.md`): immutable once
`_progress.json` records the phase `done`; revisions are a new version dir,
never an in-place edit. **The same `anvil:memoir` skill owns every chapter**
— there is no per-chapter skill switch and no separate "chapter skill."

## Dual-corpus provenance (#597 — adopted, not reinvented)

The project's factual ground truth is declared ONCE, at the **project**
`BRIEF.md` level, via the already-general top-level `corpus:` key
(`anvil/lib/snippets/provenance.md` §Section 1):

```yaml
corpus:
  - transcripts/
  - letters/
```

`anvil/lib/project_brief.py::resolve_corpus_dirs` already resolves N
declared directories in declared order — **no `anvil/lib/` change is
needed for the "dual" in dual-corpus**; the contract was already a list.
Every chapter thread under `documents:` inherits the same resolved corpus.
What `memoir` contributes is **adoption wiring** in its own commands (the
three-touch-point pattern `essay` used under #611):

- **`memoir-draft`** writes `<thread>.{N}/provenance.md` before prose and
  records `metadata.corpus_dirs_resolved`.
- **`memoir-review`** spot-samples 5-10 `provenance.md` rows per pass
  (§Section 3 back-check).
- **`memoir-audit`** runs the **exhaustive** `kind: tool_evidence`
  corpus-audit critic (§Section 4) — inventories every claim, classifies
  every `provenance.md` row VERIFIED / PARAPHRASE_OK / MISMATCH / NOT_FOUND
  / FABRICATED, and writes a `<thread>.{N}.corpus-audit/` sibling.
- **`memoir-revise`** reads and preserves the map, never fabricates a
  source-line mapping.

**Byte-identical when absent.** No `corpus:` key, `corpus: null`, or
`corpus: []` → no `provenance.md`, no findings, no corpus-audit sibling —
the tier is silent-off exactly as documented in `provenance.md`. A
**declared-but-missing** corpus directory ACTIVATES the tier and surfaces
as a `major` finding, never a crash.

## Dual voice tiers active at once (#598 — adopted, not reinvented)

The project declares its voice tiers ONCE, at the project `BRIEF.md` level,
via the already-general `voice:` block (`anvil/lib/snippets/voice_grounding.md`):

```yaml
voice:
  style_guide: STYLE_GUIDE.md    # optional author-persona narration tier
  values: VALUES.md
  subjects:                      # subject dialogue tier (#598)
    - name: grani
      corpus: transcripts/grani/**/*.md
      voice_doc: planning/grani-voice.md
    - name: aunt-jo
      corpus: transcripts/aunt-jo/**/*.md
```

Both tiers are **independently activated, coexisting** contracts
(`VoiceDocs.has_subjects` / `resolve_subject_voice_docs`) — no
`anvil/lib/` change is needed. A memoir typically declares BOTH: the
author-persona tier grounds narrator framing prose (rubric dim 2), the
subject tier grounds each speaker's reconstructed dialogue against their
own spoken corpus (rubric dim 3), interleaved within the SAME chapter.
`memoir-draft`/`memoir-review`/`memoir-revise` wire both tiers using the
same three-touch-point pattern essay uses (`essay-draft.md` steps 3/3b,
`essay-review.md` steps 4/4b, `essay-revise.md`'s preservation contract):
byte-identical when the corresponding key is absent.

## Scope boundary: sourcing (#597) vs. voice (#598)

For a reconstructed line "She said the factory burned down in 1924":
**#597** asks *does the transcript corpus contain any passage supporting
that event?* (substance); **#598** asks *does the line sound like how she
would say it?* (cadence). Both matter; neither contains the other.
Misattribution splits cleanly at this boundary: **substance-level**
misattribution (an event belongs to a different speaker's testimony) is
#597's `misattribution_of_substance` flag (audit-side); **voice-identity**
misattribution (right substance, wrong voice) is #598's `misattribution`
critical flag (review-side, conditional on >=2 subjects declared).

## State machine

Per-thread state, derived from on-disk evidence (not flags) — mirrors
`primer`/`spec` file-for-file (the report-shaped, parallel review+audit,
AUDITED-terminal precedent):

```
EMPTY → DRAFTED → REVIEWED+AUDITED → REVISED → … → AUDITED
```

| State | Evidence |
|---|---|
| `EMPTY` | No `<thread>.{N}/` directories exist for this chapter |
| `DRAFTED` | Latest `<thread>.{N}/` exists with `<thread>.tex` (slug-echo per #295) and `_progress.json.phases.draft.state == done`; no sibling review/audit at the same `N` |
| `REVIEWED-PARTIAL` / `AUDITED-PARTIAL` | Only one of `<thread>.{N}.review/` / `<thread>.{N}.audit/` exists (the two critics run in parallel; neither alone is advance-eligible) |
| `REVIEWED+AUDITED` | BOTH `<thread>.{N}.review/verdict.md` AND `<thread>.{N}.audit/verdict.md` exist for the latest `N` (the `<thread>.{N}.corpus-audit/` sibling, when the corpus tier is active, is also required — see §Combined verdict) |
| `REVISED` | A `<thread>.{N+1}/` exists after a prior REVIEWED+AUDITED pass |
| `AUDITED` (terminal) | The review records `advance: true` (≥39/44, zero unresolved review critical flags) AND the audit records `audit_clean: true` AND, when the corpus tier is active, the corpus-audit sibling records zero unresolved fabrication-class critical flags |

**`AUDITED` is the terminal state — no shortcut to `READY`.** There is no
`READY` state in this class; a memoir reconstructs a real, named person's
words and factual history, so the audit gate is mandatory (mirrors
`primer`/`spec` exactly, per the issue's own point 4).

Threshold: **≥39/44 advances** (the audit-grade band — see §Advance
threshold rationale below). Any critical flag from ANY of the three
critics (review, audit, corpus-audit) short-circuits regardless of total.
Iteration cap: default `max_iterations: 4`; project-BRIEF paired override
(`max_iterations` + `iteration_cap_rationale`, the #349 memo contract) —
see §"Iteration cap and override contract" below for the full semantics.

## Iteration cap and override contract

The cap bounds **how many version dirs a chapter thread may accumulate**,
and `metadata.iteration` always equals the version-dir number. The
governing predicate lives in `commands/memoir-revise.md` step 3:
`memoir-revise` refuses to write `<thread>.{N+1}/` when
`N + 1 > effective_max_iterations`. At the default cap of 4 the
worst-case terminal version dir is `<thread>.4/` — there is no
`<thread>.5/` under a default cap.

**At `iteration == max_iterations` (issue #869)** the thread is not
"capped," "blocked," or "at risk." The combined-verdict pre-check runs
*before* the cap check, so a chapter that satisfies §Combined verdict at
exactly the cap reports `AUDITED` and terminates normally — the cap was
reached but was never the terminating condition. The refusal fires only
on the **next** invocation, and only if a critic still blocks. Reports
(`memoir-revise`'s status line, `memoir`'s `Iter` column and `Operator
notes`) MUST distinguish these two cases; describing a clean `4/4`
terminus as capped is a reporting bug.

**Raising the ceiling is an explicit, recorded operator decision.** It is
never a silent edit and never something a command does on its own
authority:

1. **Authoring surface** — the paired override on the matching
   `documents:` entry of `<project>/BRIEF.md`: `max_iterations` (int ≥ 4)
   **and** `iteration_cap_rationale` (non-empty prose saying *why this
   chapter deserves more passes*). Both keys are required; one without
   the other is a schema violation and `load_project_brief` raises
   `ValueError` at parse time rather than degrading silently. The
   override may raise the cap, never lower it below the principled
   default of 4. Semantics are **sticky raise**, not single-use — the
   required rationale, not single-use semantics, is what prevents abuse.
   Git history of `BRIEF.md` is the durable record.
   `anvil/skills/memo/SKILL.md` §"Per-document override contract" is the
   schema-of-record (the fields are shared, in
   `anvil/lib/project_brief.py`); memoir reuses it verbatim rather than
   defining a memoir-local variant.

   ```yaml
   documents:
     - slug: 00-introduction
       artifact_type: memoir
       max_iterations: 6
       iteration_cap_rationale: |
         Raised to 6 on 2026-08-02. v3 and v4 were both map-only
         provenance repairs (corpus drift, then a repoint) — the chapter
         consumed half its budget on framework-detected bookkeeping while
         scoring 43-44/44 throughout. The prose budget was effectively 2
         passes, not 4.
   ```

2. **Per-version mirror** — `memoir-draft` and `memoir-revise` write the
   resolved `metadata.max_iterations` + `metadata.iteration_cap_rationale`
   into every `<thread>.{N}/_progress.json`, so each immutable version dir
   carries the cap that was in force when it was produced.

3. **BLOCKED-notice surfacing** — when the cap is hit, `memoir-revise`
   prints the override pointer (no override set) or echoes the existing
   rationale verbatim (elevated cap already active), plus a
   budget-composition line naming how many consumed iterations were
   map-only provenance repairs versus substantive prose revisions. `v1`
   is always reported as a separate `v1 draft` term — it is produced by
   `memoir-draft` and carries no `revision_class` (a draft is not a
   revision), so only `v2..v{iteration}` are classified and the line
   balances as `1 (draft) + substantive + map-only == consumed`. See
   `commands/memoir-revise.md` §"BLOCKED notice".

**Map-only repairs still consume an iteration** — deliberately. A
revision whose body files are byte-identical to the parent's and which
only rewrites `provenance.md` is tagged
`metadata.revision_class = "map_only"` and counted in the
budget-composition line, but it is not exempt from the cap: `iteration`
is derived from the immutable version-dir number, a second counter that
could disagree with the directory listing would reintroduce exactly the
bookkeeping ambiguity this contract removes, and a map-only pass still
triggers the expensive exhaustive corpus-audit sweep. The operator
affordance for "this chapter spent its budget on bookkeeping" is the
paired override, now fed by first-class evidence. Full rationale:
`commands/memoir-revise.md` §"Map-only repairs still consume an
iteration".

## Advance threshold rationale (≥39, not ≥35)

Unlike `primer` (educational collateral, ≥35), a memoir reconstructs a
real, named person's words and factual history — fabrication carries
reputational/legal stakes closer to `report`/`spec`/`datasheet`'s
customer-facing band than to primer's teaching-collateral band. Combined
with "audit-mandatory, no shortcut to READY," ≥39/44 is the more
defensible default (see `rubric.md`).

## Output format (LaTeX source-of-truth with photo embedding)

A chapter's body is **LaTeX** (`<thread>.{N}/<thread>.tex`, optionally
`\input`-ing `sections/*.tex`) — the `paper`/`spec`/`datasheet` precedent,
chosen because `anvil:project-book` assembles chapters into one master
LaTeX document and photo placement needs real typesetting macros. The
version dir is self-contained for archival: `<thread>.tex` as
source-of-truth, `provenance.md` (when the corpus tier is active),
`exhibits/` (rendered diagrams, when any), `<thread>.pdf` (optional
standalone render) side by side. **The primary figure/render path is
`mmdc → PNG` + pandoc/XeLaTeX** (the `report`/`spec` "primary path
pandoc/XeLaTeX, secondary opt-in TikZ" precedent) — no new rendering
pipeline is invented; see §Photo-placement contract below for the
memoir-local addition.

## Photo-placement contract (relationship to `anvil:project-photos`)

`anvil:project-photos` (#599) emits a deterministic `manifest.json`
mapping each scanned photo's original capture to a **stable name** —
strictly read-only over the source images. Its own SKILL.md explicitly
scopes placement macros **out of its surface** ("consumer extension
points via per-skill template preamble overrides" — §Out of scope). This
skill's `templates/memoir.template.tex` preamble therefore defines three
placement macros the drafter/reviser use in chapter bodies:

- `\famphoto{<stable-name>}{<caption>}` — a standard in-text family photo.
- `\fullphoto{<stable-name>}{<caption>}` — a full-page/plate photo.
- `\marginphoto{<stable-name>}{<caption>}` — a small margin inset.

Each macro resolves `<stable-name>` against `manifest.json`'s
`entries[].stable` at compile time (`memoir-figures` reads the manifest,
not the raw photos directory — `project-photos` remains strictly
read-only). A chapter referencing a stable name **not present** in
`manifest.json` surfaces as a `memoir-figures` render-gate finding — never
a silent placeholder, never a crash (see `commands/memoir-figures.md`).

## Relationship to `anvil:project-book`

**Book assembly is out of memoir's surface — use
`/anvil:project-book <project-dir>`.** `anvil:project-book` (#596) already
stages the `.latest`-resolved version of every chapter thread into a
consumer-owned master LaTeX document, two-pass compiles it, and writes a
per-thread `BOOK_REPORT.md` (state/score/audit + next command). The
`memoir.md` orchestrator in THIS skill is a **per-chapter-thread status
report only** — the same shape as `primer.md`/`spec.md`/`report.md`
(`/anvil:memoir <thread-dir>` reports one thread's draft/review/audit/
revise/figures state). It does NOT rebuild `project-book`'s portfolio
view — do not look for a "show me all six chapters" command here; that is
`project-book`'s `BOOK_REPORT.md`.

## Relationship to `anvil:project-photos`

See §Photo-placement contract above. `anvil:project-photos` produces the
provenance `manifest.json`; `anvil:memoir` never touches the source photos
and never regenerates the manifest — it only resolves stable names against
it at render time.

## Relationship to `anvil:essay` / `anvil:primer` / `anvil:spec` / `anvil:report`

| Skill | Shape | Why it does not fit |
|---|---|---|
| `anvil:essay` | Markdown, single-thread, 500-1500 words, **READY-terminal, no audit** | Deliberately short-form with no audit phase — a memoir's top-line requirement (hallucination prevention) is exactly what essay's v1 scope left out. Essay pilots the #597/#598 primitives at short-form single-thread scale; memoir composes them at book-chapter scale with the exhaustive audit essay explicitly deferred ("the exhaustive five-way audit pass is out of essay's surface... that is paper's follow-on" — `essay-review.md`). |
| `anvil:primer` | Markdown, single-document, pedagogy-dominant, audit-mandatory | Teaches a subject from intuition against an optional `spec_ref`; not chapter-threaded, no dual-corpus/dual-voice grounding. |
| `anvil:spec` | LaTeX, single-document, normative-correctness-dominant, audit-mandatory | Maintains truthfulness against an implementation (`code_ref`); not narrative, not chapter-threaded, no voice grounding at all. |
| `anvil:report` | Markdown → PDF, two-stage promotion gate, engagement-shaped | Provenance model is "your own evidence notes," not a fixed dual-corpus of transcripts + letters; not chapter-threaded, no subject-voice tier. |
| `anvil:memoir` | LaTeX, **chapter-threaded**, sourcing-fidelity-dominant, audit-mandatory, dual-corpus + dual-voice, photo embedding | The only class combining two simultaneous voice tiers (narrator + reconstructed dialogue) in one document, treating "one chapter of an assembled book" as its native thread shape, with `AUDITED` as a hallucination-prevention gate. |

None of the four is a memoir subtype; `memoir` selects no overlay from any
of them.

## Command dispatch

| Command | Role | Reads | Writes |
|---|---|---|---|
| `memoir` | per-chapter-thread status orchestrator (read-only) | one `<thread>.*` dir tree (NOT the whole project — see §Relationship to `anvil:project-book`) | (none; reports state + recommends next command) |
| `memoir-draft <thread>` | drafter | project `BRIEF.md` (+ `corpus:`, `voice:` docs), `<thread>/refs/`, shared `research/`; for revisions, also the prior version + all critic siblings | `<thread>.{N}/<thread>.tex` (+ `provenance.md` when corpus tier active) + `_progress.json` |
| `memoir-review <thread>` | reviewer (rubric /44 + provenance back-check + voice scoring) | latest `<thread>.{N}/`, resolved corpus/voice docs, `rubric.md` | `<thread>.{N}.review/` |
| `memoir-audit <thread>` | auditor (factual/narrative consistency, ALWAYS; exhaustive corpus-provenance sweep, conditional) | latest `<thread>.{N}/`, resolved corpus, `rubric.md` | `<thread>.{N}.audit/` (always) + `<thread>.{N}.corpus-audit/` (when corpus tier active) |
| `memoir-revise <thread>` | reviser (consumes review + audit + corpus-audit when present) | latest `<thread>.{N}/` + all critic siblings | `<thread>.{N+1}/` with `changelog.md`, or reports `AUDITED` |
| `memoir-figures <thread>` | figurer (diagrams + photo placement + optional PDF) | latest `<thread>.{N}/<thread>.tex`, `project-photos`' `manifest.json` | `<thread>.{N}/exhibits/` + optional `<thread>.pdf` |

## Rubric

See `rubric.md` for the 9-dimension **/44** schema (`anvil-memoir-v1`), the
**≥39** advance threshold, sourcing-fidelity-dominant weighting (dim 1 at
weight 7, the same "owned dominant dimension" shape as primer's pedagogy
and spec's normative correctness), and the critical-flag set: #597's five
fabrication-class flags (`fabricated_quote`, `fabricated_fact`,
`misattribution_of_substance`, `anachronism`, `unattributed_paraphrase` —
reused **verbatim**, not reinvented) plus #598's `misattribution`
voice-identity flag (conditional on >=2 subjects declared).

Every critic-writing command stamps `_meta.json` with
`scorecard_kind: "human-verdict"`, `rubric_id: "anvil-memoir-v1"`,
`rubric_total: 44`, `advance_threshold: 39` (per-review version stamping,
issue #346) and writes its sidecar atomically via `anvil/lib/sidecar.py`
+ `staged_sidecar` + the per-critic `cleanup_one_staging` sweep (issues
#350/#376).

## Project BRIEF artifact type

`memoir` is registered as a **skill-identity** `artifact_type` value in the
shared project-BRIEF registry (`anvil/lib/project_brief.py::REGISTERED_ARTIFACT_TYPES`
/ `SKILL_IDENTITY_ARTIFACT_TYPES`; per the
#386/#408/#432/#440/#460/#486/#686/#697 precedent). In a shared project
BRIEF, a `documents:` entry with `artifact_type: memoir` declares that
this skill owns the chapter thread. It is NOT a memo subtype: it selects
no memo rubric overlay, and memo commands fail loudly when pointed at a
thread declaring it. No memoir-specific BRIEF grammar is needed for
`corpus:`/`voice.subjects:` — both are already-general project-level keys
(§Dual-corpus provenance / §Dual voice tiers).

## Scope guard — v1 / deferred

**In scope for v1 (this issue, #740):** the skill skeleton (`SKILL.md`,
`rubric.md`, six commands, templates), `corpus:`/`voice.subjects:`
adoption wiring in memoir's own commands, the exhaustive `kind:
tool_evidence` corpus-audit critic as a `<thread>.{N}.corpus-audit/`
sibling, `ArtifactType.MEMOIR` registration, skeleton tests, and a
minimal synthetic worked example under `examples/`.

**Deferred (explicitly out of scope for v1):**

- **Cross-chapter consistency checking** (a fact stated in chapter 2
  contradicted in chapter 5) — could piggyback on `spec`'s
  constant-consistency-gate precedent (#708) in a follow-up.
- **A dedicated structured facts register** (names/dates/relationships) as
  a companion input, mirroring primer's `spec_ref`/spec's `code_ref`
  pattern — worth its own follow-up issue.
- **A full `nitas-mama` dogfood / vendored worked example** (the `spec`
  Phase 4 / #709 precedent) — worth its own follow-up issue once the
  skeleton lands, the same sequencing `primer` and `spec` both used.
- **Any voice-grounding wiring beyond what #598 already generalizes**
  (e.g. a memoir-specific vocabulary-reminder tool).
- **LaTeX/TikZ figure paths beyond the photo-placement macros** — the
  primary path stays `mmdc` + pandoc/XeLaTeX only.

## Git sync hook (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md` (`.anvil/anvil/lib/snippets/git_sync.md`
in an installed consumer repo): a repo-level `.anvil/config.json` with
`git.commit_per_phase: true` has each write-bearing memoir command end its
phase by staging only the dirs it wrote and committing as
`anvil(memoir/<phase>): <thread>.{N} [<state>]`. Default off; the read-only
`memoir` status orchestrator is exempt by definition.
