---
name: memoir-review
description: Reviewer for the memoir skill. Scores the 9-dimension /44 anvil-memoir-v1 rubric (≥39 advance) with sourcing fidelity as owned dominant dim 1, spot-samples the provenance.md back-check (#597), and scores narrator + subject voice fidelity (#598) when active. Runs parallel with memoir-audit. DRAFTED/REVISED → REVIEWED transition.
---

# memoir-review — Reviewer

**Role**: reviewer (rubric/prose content critic; runs parallel with
`memoir-audit` per the `report`/`primer`/`spec` two-critic shape).
**Reads**: latest `<thread>.{N}/<thread>.tex` (+ `provenance.md` when the
corpus tier is active), project `BRIEF.md` (+ resolved `corpus:` and
`voice:` docs), `<thread>/refs/` + shared `research/`, `rubric.md`, any
consumer `.anvil/skills/memoir/rubric.overrides.md` (additive only),
prior `<thread>.{M}.review/` siblings (M < N).
**Writes**: `<thread>.{N}.review/` with `verdict.md`, `scoring.md`,
`comments.md`, `_summary.md`, `_meta.json`, `_progress.json`.

The review sibling is **read-only once written**. Revisions consume it;
they never modify it.

## Outputs

```
<thread>.{N}.review/
  verdict.md       Advance / block + total /44 + critical-flag paragraphs + top revision priorities
  scoring.md       Per-dimension table: # | Dimension | Weight | Score | Justification
  comments.md      Line-level comments (severity blocker/major/minor/nit + scope preserve/expand/reduce)
  _summary.md      Machine-readable blocks: rubric block, voice_grounding (author tier, when active),
                    subject_voice_grounding (when active), provenance_back_check (when corpus tier active), scope_distribution
  _meta.json       { critic, role, started, finished, model, schema_version, scorecard_kind: "human-verdict",
                     rubric_id: "anvil-memoir-v1", rubric_total: 44, advance_threshold: 39 }
  _progress.json   Phase state for the reviewer
```

**Atomicity** (issues #350, #376): written atomically via
`anvil/lib/sidecar.py` — files staged under `.<thread>.{N}.review.tmp/`,
atomically renamed on clean completion; stale staging from a prior
interrupt of THIS critic removed by
`cleanup_one_staging(<thread>.{N}.review)` at entry.

## Procedure

1. **Discover state, sweep, open sidecar**: find the highest `N` with
   `<thread>.{N}/<thread>.tex`; run `cleanup_one_staging(<thread>.{N}.review)`;
   if `<thread>.{N}.review/` exists, exit early (idempotent). Otherwise
   open `staged_sidecar(final_dir=<thread>.{N}.review, required_files=
   ["verdict.md", "scoring.md", "comments.md", "_summary.md",
   "_meta.json", "_progress.json"])` and write everything inside the
   staging dir. Initialize `_progress.json` and `_meta.json` with
   `scorecard_kind: "human-verdict"`, **`rubric_id: "anvil-memoir-v1"`,
   `rubric_total: 44`, `advance_threshold: 39`** (per-review version
   stamping, issue #346).

   **Non-Python-driver ordering (fail-open, manual fallback)**: a
   driver-less session uses the CLI shim (`uv run --project .anvil
   python -m anvil.lib.sidecar stage/commit/cleanup <thread>.{N}.review
   --required verdict.md,scoring.md,comments.md,_summary.md,_meta.json,
   _progress.json`) or, as a last resort, manual `mv`-based staging
   (write every required file into `.<thread>.{N}.review.tmp/`,
   `_progress.json` last, then `mv` as the last step; stamp `_meta.json`
   with `"atomicity_fallback": "manual-mv"`). Never write straight into
   the final `<thread>.{N}.review/` name.

2. **Read inputs**: the body, the matching BRIEF `documents:` entry,
   `rubric.md`, consumer rubric overrides, `<thread>.{N}/_progress.json`
   (the drafter's self-check + `metadata.corpus_dirs_resolved` /
   `metadata.voice_exemplars` / `metadata.subject_voice_exemplars`), and
   any previous review for this slug.
3. **Provenance back-check (conditional — #597)**: invoke
   `anvil/lib/project_brief.py::resolve_corpus_dirs(<project_dir>)`
   (project-level, per SKILL.md §Dual-corpus provenance) per
   `anvil/lib/snippets/provenance.md` §Section 1.
   - **When active** (>=1 resolved dir): read
     `metadata.corpus_dirs_resolved` to verify the drafter ran.
     **Spot-sample 5-10 rows** from `<thread>.{N}/provenance.md` per
     review pass, opening each cited `Source file` + `Line range` in the
     resolved corpus per §Section 3. Emit findings as `kind: judgment`
     with `evidence_span` pointing at the map row
     (`provenance.md:L<N>`), **quoting both the claim and the cited
     passage**. A row whose cited file is not resolvable → `major`
     finding. A row whose cited passage does not support the claim as
     written → `blocker` finding — UNLESS the row carries an `Anchor`
     (§Section 2) and that anchor text reads intact a few lines away:
     that is anchor drift (§Section 4a), not a content defect; flag it
     as "Line range hint looks stale, recommend a full corpus-audit
     pass" rather than `blocker`. A **missing `provenance.md`** when the
     tier is active → `major` finding directing the operator to
     re-draft with provenance tracking. This is a **sampling** check by
     design — the exhaustive five-way sweep AND anchor-drift pre-pass
     are `memoir-audit`'s `corpus-audit` sibling's job, not this
     command's.
   - **When inactive** (no `corpus:` key, `corpus: null`, or
     `corpus: []`): byte-identical to the corpus-tier-absent posture — no
     block, no findings, no `provenance_back_check` section in
     `_summary.md`. Record a **`major` finding recommending the operator
     declare `corpus:`** — a memoir whose defining constraint ("every
     reconstructed detail traces to a source") is unenforceable is a
     defect to surface, not a crash.
   - **Declared-but-missing dirs**: the tier still activates;
     `resolve_corpus_dirs` returns `missing: true` entries (never raises) —
     surface the broken declaration as a `major` finding.
4. **Load author voice grounding (conditional — #461, narrator tier)**:
   invoke `anvil/lib/project_brief.py::resolve_voice_docs(<project_dir>)`
   per `anvil/lib/snippets/voice_grounding.md`.
   - **When active**: read the resolved docs; read
     `metadata.voice_exemplars` to verify the drafter's grounding
     happened. Declared-but-missing files → a `major` finding per file
     (tier stays active). Cache the resolved list for step 6 (dim 2).
   - **When inactive**: record a `major` finding recommending the
     operator declare the author `voice:` block; dim 2 still scores,
     uncalibrated.
5. **Load subject voice grounding (conditional — #598, dialogue
   tier)**: invoke
   `anvil/lib/project_brief.py::resolve_subject_voice_docs(<project_dir>)`
   (same `<project_dir>`; activates independently of the author tier)
   per `anvil/lib/snippets/voice_grounding.md` §"Subject voice tier".
   - **When active** (>=1 declared subject): read each subject's
     resolved `corpus` + `voice_doc`; read
     `metadata.subject_voice_exemplars` to verify the drafter's
     per-speaker grounding happened. Declared-but-missing corpora/
     voice_docs → a `major` finding per subject. Cache the resolved
     subject list for step 6 (dim 3) and for the review-side
     `misattribution` critical flag (step 7).
   - **When inactive**: the subject tier does not exist for this
     project — no finding, no `subject_voice_grounding` block (subjects
     are opt-in; their absence is silence, not a defect — the customer-
     context activation convention, NOT the author-tier's
     declare-the-block recommendation).
6. **Score the 9 dimensions** per `rubric.md` into `scoring.md` (`# |
   Dimension | Weight | Score | Justification`, integer scores, 1-3
   sentence justifications quoting evidence):
   - **Quoted-evidence requirement**: each justification MUST embed at
     least one **verbatim quote from `<thread>.tex`** wrapped in inline
     double quotes with a location anchor — `("the quoted span" —
     §2.1)` — per `anvil/lib/snippets/rubric.md` §"Dimension scoring
     guidance" rule 1. A dim scored at full weight MAY substitute the
     by-absence marker `no instance of <X> found`.
   - **Dim 1 (Sourcing fidelity — owned, dominant)**: when the corpus
     tier is active, score against the `provenance.md` back-check
     (step 3) — quote the claim AND the `provenance.md` row (or its
     absence). When inactive, score on the chapter alone with the
     `major` finding.
   - **Dim 2 (Narrator voice fidelity)**: when the author tier is
     active, score against the resolved docs per `voice_grounding.md`
     §Reviewer contract — every deduction quotes a corpus exemplar
     (apply the convergence-with-Claude adversarial check). When
     inactive, score uncalibrated with the `major` finding noted.
   - **Dim 3 (Subject voice fidelity)**: when the subject tier is
     active, run a per-subject pass over each speaker's rendered
     dialogue against that speaker's resolved transcript corpus (+
     `voice_doc` when present) per `voice_grounding.md` §"Subject voice
     tier". **Every deduction MUST quote the transcript showing the
     speaker's actual cadence alongside the drifting reconstructed
     line.** Apply the generalized convergence-with-Claude check: would
     I, the AI, also write this line for this speaker? When inactive,
     the dim is scored on prose craft alone (no corpus calibration, no
     finding — subjects are opt-in).
   - Remaining dims (4-9) per their `rubric.md` rows.
7. **Identify review-side critical flags** — each with a one-paragraph
   justification in `verdict.md` quoting the offending passage and the
   violated contract:
   - **`misattribution`** (#598, conditional on the subject tier active
     with >=2 subjects declared): a line attributed to Subject A carries
     characteristic markers matching Subject B's corpus and contradicts
     Subject A's corpus. Justification MUST cite the attributed line,
     Subject A's corpus (why it doesn't fit), and — when identifiable —
     Subject B's corpus (why it fits better). This is the
     **voice-identity failure only**; the substance-level twin
     (`misattribution_of_substance`) is `memoir-audit`'s job. With
     fewer than 2 subjects declared, this flag cannot fire.
   - (The fabrication-class flags — `fabricated_quote`,
     `fabricated_fact`, `misattribution_of_substance`, `anachronism`,
     `unattributed_paraphrase` — are `memoir-audit`'s `corpus-audit`
     sibling's job; the reviewer does not raise them.)

   If none: "Critical flags: none."
8. **Verdict** into `verdict.md`: total /44, review-critical-flag count,
   `advance: true` iff **total >=39 AND zero unresolved review critical
   flags** (the general audit's and — when active — the corpus-audit's
   clean/blocked state is combined at revise time — the reviser reads
   all three siblings). Top 3 revision priorities: any critical flag
   first, then the highest-leverage dim deductions (dim 1 sourcing gaps
   lead). List "What's working" — the moves the reviser must NOT sand
   off.
9. **Write `_summary.md`** (inside the staging dir): the rubric block
   `{ "id": "anvil-memoir-v1", "total": 44, "advance_threshold": 39,
   "dimensions": 9 }`, the per-dim score map, `scope_distribution`
   `{preserve, expand, reduce}` counts over `comments.md`, and — only
   when the respective tier is active — the `voice_grounding` block
   (author tier), the `subject_voice_grounding` block (dialogue tier),
   and the `provenance_back_check` block (corpus tier). Each block is
   omitted entirely (no `{ran: false}` entry) when its tier is inactive
   — the customer-context activation convention.
10. **Finalize `_meta.json` + `_progress.json`** inside the staging dir
    (`_progress.json` LAST), then exit the `staged_sidecar` block —
    manifest verified, staging dir atomically renamed to
    `<thread>.{N}.review/`.
11. **Report**: e.g., `Reviewed 00-introduction.1 → 41/44, 0 review
    critical flags, corpus tier active (0 major/blocker back-check
    findings), author voice active, 1 subject active (0
    misattribution). Next: memoir-revise 00-introduction (after
    memoir-audit)`.

## What memoir-review does NOT do

- **Never edits the body.** Read-only against `<thread>.{N}/`.
- **Never raises the fabrication-class flags** — those are
  `memoir-audit`'s exhaustive `corpus-audit` sibling's job (a `major`/
  `blocker` finding from the back-check sample is the review-side
  surface; a critical flag requires the exhaustive sweep).
- **Never crashes on a missing/unresolvable corpus or voice contract** —
  the `major` finding is the surface (the `report`/`essay`
  customer-context posture).
- **Don't be vague** ("voice feels off" without a corpus quote is a
  defective finding — and for a subject line, "doesn't sound like her"
  without a transcript quote is equally defective, #598).

## Scorecard kind

This critic emits the `human-verdict` scorecard kind per
`anvil/lib/snippets/scorecard_kind.md`. `_meta.json` MUST include
`"scorecard_kind": "human-verdict"` plus the three rubric-stamping
fields (`"rubric_id": "anvil-memoir-v1"`, `"rubric_total": 44`,
`"advance_threshold": 39"`).

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and
`git.commit_per_phase` is `true`, end this phase: stage only the dirs
this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N}
[<state>]`, push if `git.push` is `true`. Git failures warn and
continue. Default off.

This phase's specifics:

- **Ordering**: after the staged-sidecar atomic rename (issue #350)
  lands the final-named `<thread>.{N}.review/`.
- **Staging target**: ONLY this command's own `<thread>.{N}.review/`.
- **Commit**: `anvil(memoir/review): <thread>.{N} [REVIEWED]`.
