# Memoir review rubric

Rubric id: **`anvil-memoir-v1`**. The reviewer scores a chapter against 9
weighted dimensions summing to **44**. The threshold to advance is **≥39/44**
(the audit-grade band — see SKILL.md §Advance threshold
rationale: a memoir reconstructs a real, named person's words and factual
history, so fabrication carries reputational/legal stakes closer to
`report`/`spec`/`datasheet`'s customer-facing band than to `primer`'s
teaching-collateral band). Any **critical flag**, from ANY of the three
critics (review, audit, corpus-audit), short-circuits the verdict — the
chapter is blocked regardless of total score until the flagged issue is
addressed.

The rubric is tuned so that **sourcing fidelity dominates**: dim 1 carries
the highest weight (7), the same "owned dominant dimension" shape as
`primer`'s pedagogy (dim 1) and `spec`'s normative correctness (dim 1).
Behind it, the two voice dimensions (narrator and subject, weight 5 each)
and narrative arc (weight 5) are heavy — a memoir succeeds or fails on
whether the reconstruction is both true and true-sounding. Dim 9
(*Rhetorical economy*) closes the standard trio (structure / prose
clarity / rhetorical economy) per the primer/spec/essay precedent.

## Dimensions

| # | Dimension | Weight | What it measures |
|---|---|---|---|
| 1 | **Sourcing fidelity** | 7 | (dominant) Every reconstructed quote and every checkable factual claim traces to the declared `corpus:` via `provenance.md`. Deductions quote the claim AND the `provenance.md` row (or its absence). Critical-flag twin: #597's five fabrication-class flags (see §Critical flags). When the corpus tier is inactive, scored on the chapter alone AND a `major` finding recommends declaring `corpus:` — the class's defining constraint is otherwise unenforceable. |
| 2 | **Narrator voice fidelity** | 5 | The narrating author's own persona prose (#461/essay precedent) scored against the resolved author-tier `voice:` docs where declared. Every deduction quotes a corpus exemplar showing what the target voice sounds like (the convergence-with-Claude adversarial check applies). Scored uncalibrated, with a `major` finding, when the author tier is undeclared. |
| 3 | **Subject voice fidelity** | 5 | Reconstructed dialogue scored against each declared subject's spoken transcript corpus (#598) — does the line sound like how THIS speaker would say it (cadence, register, characteristic openers), not merely a polished version of the substance. Every deduction quotes the transcript alongside the drifting reconstructed line. Critical-flag twin: #598's `misattribution` flag, conditional on >=2 subjects declared. Inactive (no `voice.subjects`) → not scored against any corpus; the chapter's prose-craft alone informs this dim. |
| 4 | **Narrative arc & scene craft** | 5 | Chapter-level and book-level throughline; scenes resolve rather than merely recount; the chapter earns its place in the assembled book's arc (ties to `project-book`'s `order`). |
| 5 | **Period/place texture** | 4 | Era- and place-consistent sensory and material detail; a craft dimension distinct from — but adjacent to — the #597 `anachronism` fact-check (an era-incompatible DETAIL not contradicted by any specific corpus passage is a dim-5 deduction; one the corpus chronology actively contradicts is the critical flag). |
| 6 | **Dialogue/prose flow & balance** | 4 | Narration-vs-dialogue balance reads naturally within a chapter; scene transitions, pacing, and the ratio of summary to scene serve the material. |
| 7 | **Structure & chapter navigation** | 4 | Wayfinding within the chapter (section breaks, scene markers) and the chapter's legibility as one unit of the assembled book (ties to `project-book`'s `order` and `BOOK_REPORT.md`). |
| 8 | **Prose clarity** | 5 | Standard closing-trio dim (primer/spec/essay precedent): sentence- and paragraph-level craft; the reader never re-reads a sentence to parse it. |
| 9 | **Rhetorical economy** | 5 | Standard closing-trio dim: the chapter earns its length; no padding, no wandering digression, no non-load-bearing scene. |
| | **Total** | **44** | Advance threshold: ≥39 |

(7+5+5+5+4+4+4+5+5 = 44.)

## Scoring guidance

Each dimension is scored as an **integer from 0 to its weight** (the
weight is the per-dimension maximum; no half-points). A short
justification accompanies each score (1-3 sentences citing specific
evidence: a quoted passage with a location anchor).

Calibration (stated for dim 1 at weight 7; scale proportionally for other
weights):

- **7 (full weight)** — every reconstructed quote and checkable claim
  traces to a `provenance.md` row classified VERIFIED or PARAPHRASE_OK;
  no MISMATCH/NOT_FOUND rows and no fabrication-class critical flags.
- **5-6** — mostly grounded, with one or two claims whose `provenance.md`
  mapping is thin (a NOT_FOUND row without a clear justification, a
  paraphrase that drifts further than the corpus supports).
- **3-4** — several claims have weak or missing `provenance.md` mapping;
  a careful reader would ask "how do you know that."
- **1-2** — the chapter substantially outruns its corpus; most
  reconstructed detail is unverifiable.
- **0** — no correspondence to any declared corpus; the chapter is
  invented wholesale.

**Quoted evidence.** Every justification embeds at least one **verbatim
quote from the chapter body** with a location anchor — `("the quoted
span" — §2.1)` — per `anvil/lib/snippets/rubric.md` §"Dimension scoring
guidance" rule 1, with the `no instance of <X> found` by-absence marker
allowed at full weight only. A quote that does not appear verbatim in the
body is fabricated evidence and the justification must be re-derived.

## Critical flags

Any single flag → BLOCK, regardless of total score. Each flag's
justification (in `verdict.md`, review-side; in `verdict.md`/`findings.md`,
audit-side/corpus-audit-side) quotes the offending passage and the
contract it violates. **This class reuses the existing critical-flag
vocabulary verbatim — it does NOT invent memoir-specific flag types for
v1.**

### Fabrication-class flags (#597, five-way — corpus-audit-side, conditional on an active `corpus:`)

Per `anvil/lib/snippets/provenance.md` §Section 6, raised by the exhaustive
`kind: tool_evidence` corpus-audit critic (`memoir-audit`'s
`<thread>.{N}.corpus-audit/` sibling):

1. **`fabricated_quote`** — verbatim-quoted text that does not appear in
   the corpus.
2. **`fabricated_fact`** — a named date, name, or event not traceable to
   any corpus passage.
3. **`misattribution_of_substance`** — an event or memory attributed to a
   speaker whose corpus does not contain it (the **substance-level**
   flag; the voice-identity twin is #598's `misattribution` below).
4. **`anachronism`** — an era-incompatible detail contradicted by the
   corpus chronology.
5. **`unattributed_paraphrase`** — authorial invention presented as a
   subject's memory without any corpus grounding.

**Inactive when `corpus:` is undeclared or unresolvable** — no false
critical flag, no crash; `resolve_corpus_dirs` never raises, and an
undeclared or all-missing corpus surfaces as a `major` finding instead
(see SKILL.md §Dual-corpus provenance).

### Misattribution — voice-identity failure (#598, review-side, conditional on >=2 subjects)

Raised ONLY when the subject voice tier is active with **>=2 subjects
declared**. When a line attributed to Subject A carries characteristic
markers that match Subject B's corpus and contradict Subject A's corpus,
raise this flag. This is the **voice-identity failure only** (wrong voice
in the wrong mouth); the substance-level twin is
`misattribution_of_substance` above. With fewer than 2 subjects declared,
the flag cannot fire (a single speaker has no alternate corpus to
misattribute against).

If no critical issues, the verdict says so explicitly: "Critical flags:
none."

**Never a critical flag**: a lossy-but-true period/place simplification
(dim 5 deduction), an undeclared or unresolvable `corpus:`/`voice:` tier
(a `major` finding), a `provenance.md` row classified MISMATCH or
NOT_FOUND without corroborating fabrication evidence (a `major`/`blocker`
finding per `provenance.md` §Section 3, not a flag), or length past an
implicit envelope (dim 9 deduction).

## Advance threshold

- **Review total ≥39/44** AND zero unresolved review critical flags AND a
  clean general audit (`audit_clean: true`) AND, when the corpus tier is
  active, zero unresolved fabrication-class flags on the corpus-audit
  sibling → advance; the thread is `AUDITED` (terminal — see SKILL.md
  §State machine).
- Otherwise → block; revise.
- Termination order (critical flag → threshold → iteration cap → stalled)
  per `anvil/lib/snippets/rubric.md`.

## Critic sidecar format

All three critics emit the **`human-verdict`** scorecard kind per
`anvil/lib/snippets/scorecard_kind.md`.

```
<thread>.{N}.review/
  verdict.md       Advance / block + total /44 + critical-flag paragraphs
  scoring.md       Per-dimension table: # | Dimension | Weight | Score | Justification
  comments.md      Line-level comments keyed to the chapter body
  _summary.md      Machine-readable blocks (rubric, voice_grounding,
                    subject_voice_grounding, provenance_back_check, scope_distribution)
  _meta.json       Stamps (below)
  _progress.json   Phase state for the reviewer

<thread>.{N}.audit/
  verdict.md       Audit verdict + critical audit-flag paragraphs (factual + narrative consistency)
  findings.md      Per-claim table: Claim | Kind | Verified? | Evidence
  comments.md      Line-level audit comments
  _summary.md      Machine-readable audit blocks
  _meta.json       Stamps (below)
  _progress.json   Phase state for the auditor

<thread>.{N}.corpus-audit/         (only when the corpus tier is active)
  verdict.md       Corpus-audit verdict + fabrication-class critical-flag paragraphs
  findings.md      Per-claim table: Claim | Source file | Line range | Classification | tool_calls evidence
  comments.md      Line-level corpus-audit comments
  _summary.md      Machine-readable provenance_summary block (#597 §Section 7)
  _meta.json       Stamps (below)
  _progress.json   Phase state for the corpus-audit critic
```

## `_meta.json` format

```json
{
  "critic": "review",
  "role": "memoir-review.md",
  "started": "2026-07-22T15:00:00Z",
  "finished": "2026-07-22T15:18:00Z",
  "model": "<model-id>",
  "schema_version": 1,
  "scorecard_kind": "human-verdict",
  "rubric_id": "anvil-memoir-v1",
  "rubric_total": 44,
  "advance_threshold": 39
}
```

The three rubric-stamping fields (`"rubric_id": "anvil-memoir-v1"`,
`"rubric_total": 44`, `"advance_threshold": 39`) are **mandatory** in
every critic `_meta.json` this skill writes (per-review version stamping,
issue #346), including the `<thread>.{N}.corpus-audit/` sibling. The
critic sibling dir is **read-only once written**.

Consumers add domain-specific critical-flag examples via
`.anvil/skills/memoir/rubric.overrides.md` (additive only; cannot reduce
the base rubric).
