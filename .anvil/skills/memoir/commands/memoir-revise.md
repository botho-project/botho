---
name: memoir-revise
description: Reviser for the memoir skill. Consumes ALL critic siblings for the latest version — review, general audit, and (when the corpus tier is active) corpus-audit — and produces a single revised version, never fabricating a provenance.md source-line mapping. REVIEWED+AUDITED → REVISED transition (loops until ≥39/44 with zero critical flags across all active critics, or the iteration cap).
---

# memoir-revise — Reviser

**Role**: reviser (one reviser consumes N critic siblings — here review +
general audit, plus corpus-audit when the corpus tier is active; the
`report`/`primer`/`spec` shape extended to a third conditional sibling).
**Reads**: latest `<thread>.{N}/<thread>.tex` + `_progress.json`,
`<thread>.{N}.review/` (all files), `<thread>.{N}.audit/` (all files),
`<thread>.{N}.corpus-audit/` (all files, when present), `<thread>.{N}/provenance.md`
(when the corpus tier is active), `<thread>/refs/` + shared `research/`,
project `BRIEF.md`.
**Writes**: `<thread>.{N+1}/` with `<thread>.tex`, `provenance.md`
(carried forward + updated, when active), `changelog.md`,
`_progress.json` — or reports `AUDITED` without writing when the
combined verdict pre-check passes.

## Procedure

1. **Discover state**: find the highest `N` with `<thread>.{N}/<thread>.tex`.
   Require BOTH a completed `<thread>.{N}.review/` AND a completed
   `<thread>.{N}.audit/` (else exit pointing at the missing critic —
   `REVIEWED-PARTIAL`/`AUDITED-PARTIAL` are not advance-eligible per
   SKILL.md). Require `<thread>.{N+1}/` to not exist (immutability —
   never revise in place).
2. **Combined verdict pre-check**: re-resolve
   `anvil/lib/project_brief.py::resolve_corpus_dirs(<project_dir>)` to
   determine whether the corpus tier is currently active (the same
   check `memoir-audit` used when it ran). Read
   `<thread>.{N}.review/verdict.md` and `<thread>.{N}.audit/verdict.md`,
   and — **only when the corpus tier is active** —
   `<thread>.{N}.corpus-audit/verdict.md` (require it to exist when the
   tier is active; its absence at this step means `memoir-audit` has not
   finished the exhaustive sweep yet — treat identically to
   `AUDITED-PARTIAL`, do not proceed).

   The thread is **`AUDITED` — terminal** iff:
   - the review records `advance: true` (total >=39/44, zero unresolved
     review critical flags), AND
   - the general audit records `audit_clean: true`, AND
   - when the corpus tier is active, the corpus-audit sibling ALSO
     records `audit_clean: true` (zero unresolved fabrication-class
     critical flags).

   When all applicable conditions hold: report the publish-handoff
   summary (resolved body path, review total /44, clean audit(s), a
   pointer to `/anvil:project-book` for assembly) and exit WITHOUT
   writing a new version. Otherwise proceed to step 3.
3. **Iteration-cap check**: resolve the effective cap, then apply the
   predicate.

   **Resolution order — first match wins:**

   1. The matching `documents:` entry in `<project>/BRIEF.md` (via
      `anvil/lib/project_brief.py::load_project_brief` +
      `ProjectBrief.document_for_slug(slug)`). When `doc.max_iterations`
      AND `doc.iteration_cap_rationale` are BOTH set, use
      `doc.max_iterations` as the effective cap, carry the rationale
      verbatim into the BLOCKED notice (when the cap is hit) and into
      `_progress.json.metadata.iteration_cap_rationale` (when a new
      version is written at step 9). The BRIEF parser already enforces
      the paired-override validation contract at parse time (both keys
      present, `max_iterations >= 4`, rationale non-empty) — the reviser
      does NOT re-validate.
   2. Else `metadata.max_iterations` from `<thread>.{N}/_progress.json`
      (typically the default 4 carried forward by the prior drafter /
      reviser pass).
   3. Else `anvil/lib/project_brief.py::DEFAULT_MAX_ITERATIONS` (4).

   If the BRIEF cannot be loaded at all (absent, malformed YAML), fall
   through to (2)/(3) — `load_project_brief` returns `None` on every
   absence path. If the BRIEF parses but the paired override is
   **malformed** (`max_iterations` without a rationale, a rationale
   without `max_iterations`, `max_iterations < 4`, non-integer cap), the
   parser raised `ValueError` at load time and the reviser **propagates
   that error** rather than degrading silently — the BRIEF is the
   schema-of-record (memoir follows memo's STRICT contract, not deck's
   lenient `.anvil.json` fallback). The schema violation is itself the
   actionable error: the operator fixes the BRIEF or removes the
   override.

   **`max_iterations` counts REVISIONS, not version dirs (issue #933).**
   `<thread>.1/` is `memoir-draft`'s output, not a revision — a draft
   consumes no revision budget (it still occupies a version-dir slot; it
   is simply not chargeable against the cap). Writing `<thread>.{N+1}/`
   is revision number `N` (`v2` is revision 1, `v3` is revision 2, ...,
   `v{N+1}` is revision `N`), so the cap is checked against `N`, not
   `N + 1`. This corrects the original #869 predicate
   (`N + 1 > effective_max_iterations`), which charged the free initial
   draft against the same budget as every framework-mandated repair —
   see issue #933 for the two canary threads (`05-quebec`, `03-garrard`)
   that motivated the change: a chapter now gets `effective_max_iterations`
   *revision* opportunities on top of its one free draft, so the revision
   that fixes the last defect always has a slot to be scored, not just
   written.

   **Predicate**: if `N > effective_max_iterations`, exit with the
   **BLOCKED notice** per §"BLOCKED notice" below, writing nothing.
   Otherwise proceed to step 4.

   **Pre-write budget notice (issue #933).** Before proceeding past this
   check, when the write about to happen (`<thread>.{N+1}/`) would land
   exactly ON the cap — i.e. `N == effective_max_iterations` — print to
   stdout, BEFORE writing anything:

   > Budget notice: writing `<thread>.{N+1}` consumes the FINAL revision
   > slot under the current cap (`max_iterations=<cap>`). If
   > `<thread>.{N+1}`'s critic pass does not clear all active verdicts,
   > this thread will BLOCK with no further repair slot for the version
   > about to be produced. Raise `max_iterations` in `<project>/BRIEF.md`
   > now (see SKILL.md §"Iteration cap and override contract") if this
   > chapter is trending toward needing more passes.

   This is advisory, not a refusal — the write still proceeds after it.
   The point is sequencing: the operator (or the next automated pass)
   learns the consequence **before** the version that would exhaust the
   budget exists, not only afterward in the step-10 completion report.

   **What happens at the ceiling (issue #869, corrected by #933).** The
   cap bounds *how many revisions a thread may consume*, not what a
   thread sitting at the ceiling is worth. `metadata.iteration` still
   equals the version-dir number (the framework-wide convention,
   `anvil/lib/snippets/progress.md`), but the number compared against
   `max_iterations` is `iteration - 1` (revisions consumed — the `v1`
   draft is never counted). At the default `max_iterations: 4`:

   | Latest version on disk | `memoir-revise` behavior |
   |---|---|
   | `<thread>.1/` (draft) … `<thread>.4/` (revisions 1-3) | **proceeds** — `N <= 4`; writes `<thread>.{N+1}/`. Writing `<thread>.5/` from `<thread>.4/` fires the pre-write budget notice above (`N == 4 == effective_max_iterations`). |
   | `<thread>.5/` (revision 4 — the final one), combined verdict clean | **terminates `AUDITED` at step 2** — the cap check at step 3 is never reached |
   | `<thread>.5/`, a critic still blocks | **refuses** — `5 > 4`; prints the BLOCKED notice and writes nothing |

   So: `memoir-revise` warns before the write that would consume the
   final revision slot, then still refuses once the budget is truly
   exhausted — it never warns-and-proceeds *past* the cap, only *up to*
   it. The refusal lands on the invocation *after* the one that produced
   the at-the-cap version, and only when step 2's combined verdict has
   not already terminated the thread. A chapter that lands `AUDITED`
   at exactly `iteration == max_iterations + 1` is a normal, healthy
   terminus — the cap was *reached* but was never the *terminating
   condition*, and the report MUST NOT describe it as blocked, capped, or
   a near-miss. `<thread>.{max_iterations + 1}/` (`<thread>.5/` at the
   default) is the worst-case terminal version dir; there is no
   `<thread>.6/` under a default cap.
4. **Read all critic input**: from the review — `verdict.md` (top
   revision priorities first), `scoring.md` (per-dim deductions; dim 1
   sourcing gaps lead), `comments.md`, and the "What's working" list.
   From the general audit — `verdict.md` (critical audit flags first),
   `findings.md` (factual/narrative-consistency findings),
   `comments.md`. From the corpus-audit (when present) —
   `verdict.md` (fabrication-class critical flags first), `findings.md`
   (per-provenance-row classification, including any anchor-drift rows
   per `anvil/lib/snippets/provenance.md` §Section 4a — these are
   **mechanical**, never critical flags, and are resolved at step 7, not
   here), `comments.md`. **A critical flag from ANY of the three critics
   blocks** — all must be addressed.
5. **Re-resolve the corpus + voice tiers**: re-invoke
   `resolve_corpus_dirs`, `resolve_voice_docs`, and
   `resolve_subject_voice_docs` against the project `BRIEF.md` and read
   the resolved docs alongside the critic feedback so the revision stays
   consistent with all active tiers. When any critic carried a
   missing/unresolvable-tier `major` finding, surface it in the report
   (the fix is operator-side BRIEF authoring or path correction, not
   body editing).
6. **Build the revision plan**, ordered: (1) critical flags — every flag
   from ANY critic MUST be addressed:
   - **Fabrication-class flags** (`fabricated_quote`,
     `fabricated_fact`, `misattribution_of_substance`, `anachronism`,
     `unattributed_paraphrase`, `scope_overreach` (#1032) —
     corpus-audit-side): cut or correct the offending claim so it is
     either removed, or replaced with a claim the corpus actually
     supports (with an updated `provenance.md` row). **Never invent a
     new source-line mapping to paper over the finding** — a
     MISMATCH/NOT_FOUND/FABRICATED classification is resolved by
     changing the CLAIM to match the evidence, never by changing the
     CITATION to match the claim. A `scope_overreach` finding is
     resolved by **narrowing the claim's wording** to match the row's
     own scope (e.g. "anyone has" → "the project has") — never by
     widening the cited row or fabricating a broader source.
   - **`misattribution`** (voice-identity, review-side): rewrite the
     dialogue line in the correctly-attributed speaker's own cadence, or
     re-attribute it to the speaker whose corpus actually supports it.
   - (3) `blocker`/`major` comments from any critic; (4) the
     lowest-scoring dims' deductions; (5) `minor`/`nit` only when they
     don't conflict with (1)-(4). Never touch the "What's working" list.
7. **Write `<thread>.{N+1}/<thread>.tex`** (slug-echo per #295) applying
   the plan. Re-run the drafter's step-7 self-disciplines
   (sourcing-traces-to-provenance check, narrator/subject voice
   interleaving check, scene-craft pass) — the revision must not
   introduce a fresh instance of the failure mode it just fixed.
   - **Carry forward and update `provenance.md`** (when the corpus tier
     is active): every retained claim keeps its row; every changed claim
     gets a re-derived row (a real corpus passage, or an explicit
     `NOT_FOUND` note); every cut claim's row is removed. **Fabricating a
     source-line mapping remains prohibited on revision exactly as on
     first draft.**
     - **Mechanically repoint drifted anchors (§Section 4b, #868)**:
       after copying the map forward, run `python -m
       anvil.lib.provenance_anchor repoint <thread>.{N+1}/provenance.md
       <corpus_root> [...]` (prefix `uv run --project .anvil` in an
       installed consumer repo). It rewrites ONLY the `Line range` cell
       of rows the corpus-audit sibling flagged `DRIFTED` — `Claim` /
       `Source file` / `Anchor` / `Notes`, and every non-drifted row,
       are left untouched. This is explicitly **not** the fabrication
       case above: the anchor text itself already proves the citation is
       genuine, so mechanically correcting a stale `Line range` hint to
       match where that same evidence now lives is not "inventing a new
       source-line mapping" — it never happens for `MISMATCH`/
       `NOT_FOUND`/`FABRICATED` rows, only `DRIFTED` ones.
   - **Preserve photo-placement macro references**: carry forward
     `\famphoto{...}`/`\fullphoto{...}`/`\marginphoto{...}` calls unless
     a critic specifically flagged a caption/placement problem;
     `memoir-figures` re-resolves them against the manifest.
8. **Write `changelog.md`** mapping each consumed critic note (from
   review, general audit, and corpus-audit) to the change made (or to an
   explicit `declined — <reason>` entry; scoring deductions may be
   argued against, critical flags — from ANY critic — may not).
9. **Initialize `_progress.json`** for the new version:
   `phases.revise.state = done` (LAST write), carry forward
   `metadata.corpus_dirs_resolved` / `metadata.voice_exemplars` /
   `metadata.subject_voice_exemplars` (when active), and **append the
   `score_history` row** for the completed review iteration per
   `anvil/lib/snippets/progress.md` §Convergence fields: `{ "iteration":
   <N>, "total": <reviewed-total>, "threshold": 39, "rubric_id":
   "anvil-memoir-v1" }`. Stable-score termination (`STALLED`) follows
   `anvil/lib/snippets/rubric.md` §"Termination resolution order" over
   this history.

   Also write the iteration-budget audit trail (issue #869):

   - `metadata.iteration = N+1`, `metadata.revised_from = N`.
   - `metadata.max_iterations` — the **effective** cap resolved at step 3.
   - `metadata.iteration_cap_rationale` — the verbatim operator-supplied
     rationale when the BRIEF paired override is in effect, `null`
     otherwise. Every version dir therefore carries a self-contained
     record of the cap that was in force when it was produced (readers
     tolerate an absent key on pre-#869 version dirs).
   - `metadata.revision_class` — the deterministic budget-composition
     tag defined in §"Map-only repairs still consume an iteration"
     below: `"map_only"` when every body file in `<thread>.{N+1}/`
     (`<thread>.tex` plus any `sections/*.tex`) is byte-identical to its
     `<thread>.{N}/` counterpart and the only substantive change is to
     `provenance.md`; `"substantive"` otherwise. This field is
     **audit-trail only** — it does not change the cap arithmetic, is
     not scored, and has no state-machine effect. Only *revision*
     versions carry it: `<thread>.1/` comes from `memoir-draft` and has
     no `revision_class` at all (a draft is not a revision), so the
     classified set is always `v2..v{N+1}` and the budget-composition
     line reports `v1` as a separate `v1 draft` term. Readers likewise
     tolerate an absent key on pre-#869 version dirs.
10. **Report**: e.g., `Revised 00-introduction.1 → 00-introduction.2
    (revision 1/4; addressed 1 corpus-audit critical flag [NOT_FOUND ->
    claim cut] + 2 major comments; 1 declined with reason). Next:
    memoir-review + memoir-audit 00-introduction`.

    The status line MUST name the revision budget consumed — `revision
    <k>/<max_iterations>`, where `<k>` is the version just written
    (`<thread>.{N+1}/`) minus its draft, i.e. `k = N` (`v1` the draft is
    never a `<k>`; `v2` is revision 1) — and, when
    `metadata.revision_class == "map_only"`, carry the `map-only` tag:
    the cheap operator signal that this pass spent a revision on
    provenance bookkeeping rather than prose. Examples:

    - `Revised 00-introduction.2 → 00-introduction.3 (revision 2/4;
      map-only — provenance repoint, .tex byte-identical; addressed 1
      corpus-audit critical flag [MISMATCH -> row repointed]). Next:
      memoir-review + memoir-audit 00-introduction`
    - `Revised 00-introduction.4 → 00-introduction.5 (revision 4/4 —
      final revision under the current cap; addressed 2 major comments).
      Next: memoir-review + memoir-audit 00-introduction`

    When the written version consumes the LAST revision slot (`k ==
    max_iterations`), the report MUST append the `final revision under
    the current cap` clause — but this is the confirmation, not the
    first notice: step 3's pre-write budget notice (issue #933) already
    told the operator this write would exhaust the budget **before** the
    write happened, not only here after the fact. This clause is
    advisory only — it does not refuse and does not pre-empt step 2's
    normal `AUDITED` terminus on the next critic pass.

## What memoir-revise does NOT do

- **Never edits `<thread>.{N}/` or any critic sibling in place** —
  immutability is the audit trail.
- **Never advances state itself** — the next `memoir-review` +
  `memoir-audit` pass scores `<thread>.{N+1}/` on its own merits; there
  is no "the reviser fixed it" credit.
- **Never bypasses a critical flag from any critic** — a changelog
  `declined` entry is legitimate for scoring deductions, never for a
  critical flag.
- **Never fabricates a `provenance.md` source-line mapping to resolve a
  fabrication-class finding** — the fix is always to the CLAIM, never a
  reverse-engineered CITATION. The mechanical anchor repoint (§Section
  4b, #868) is the one exception in spirit but not in substance: it only
  ever moves a `Line range` hint to match where an unchanged, already-
  verbatim-matched anchor now lives — it never touches `MISMATCH`/
  `NOT_FOUND`/`FABRICATED` rows and never changes `Claim`/`Source file`/
  `Anchor`.
- **Never proceeds to `AUDITED` when the corpus tier is active but
  `<thread>.{N}.corpus-audit/` has not yet been written** — treated
  identically to `AUDITED-PARTIAL`.
- **Never raises the iteration cap itself** — not by editing the BRIEF,
  not by writing an elevated `metadata.max_iterations` into the new
  version dir, not by treating a map-only pass as free. Raising the
  ceiling is an operator decision recorded in `<project>/BRIEF.md`; the
  reviser only *resolves*, *mirrors*, and *surfaces* it.

## Convergence

After this command produces `<thread>.{N+1}/`, the orchestrator runs
`memoir-review <thread>` **and** `memoir-audit <thread>` on the new
version (in parallel). The cycle continues until:

- the combined verdict of step 2 holds (thread reaches `AUDITED` —
  terminal), OR
- `N > effective_max_iterations` (thread is `BLOCKED` for human
  review — see §"BLOCKED notice" below), OR
- stable-score termination fires (`STALLED`) per
  `anvil/lib/snippets/rubric.md` §"Termination resolution order" over
  `score_history`.

Reaching the ceiling (the latest version is `<thread>.{max_iterations +
1}/`) is **not**, on its own, one of these terminating conditions. A
thread whose latest version has consumed every revision slot is in
exactly one of two states, and they are NOT the same report (issue
#933 — a report that just says "hit the cap" conflates a healthy
terminus with a stuck one):

- **Converged — the good outcome, whether or not the last slot was
  spent.** The combined verdict clears on the version at (or before) the
  ceiling: the thread reaches `AUDITED` — terminal, healthy, nothing to
  fix. A chapter that never needed all its revisions reaches `AUDITED`
  with slots left unspent; one that needed exactly `max_iterations`
  revisions reaches `AUDITED` on the very last one, fully spent but spent
  well. Either way this is reported as `AUDITED` (the step-2 publish-
  handoff summary), never as blocked, capped, or a near-miss.
- **Final version written and unvalidatable — the bad outcome.** The
  combined verdict does NOT clear on `<thread>.{max_iterations + 1}/`,
  and there is no revision slot left to fix it. This is the BLOCKED
  notice below, and it is the ONLY thing the BLOCKED notice ever reports
  — it never fires for the converged case above.

`memoir-revise`'s two possible ceiling-adjacent reports (the `AUDITED`
publish-handoff summary at step 2, and the BLOCKED notice below) are
structurally distinct code paths, so they cannot literally be confused
for each other — but the wording inside each MUST keep naming which one
it is (`AUDITED` / `BLOCKED`), never a bare "capped."

### BLOCKED notice

When step 3's predicate fires (`N > effective_max_iterations`), the
reviser exits **without** writing `<thread>.{N+1}/` and prints a BLOCKED
notice to stdout. Because the predicate can only fire once every revision
slot is spent, this notice is ALWAYS the **final version written and
unvalidatable** case (issue #933): a version exists (`<thread>.{N}/`, at
`N == max_iterations + 1`), a full critic pass already ran on it, it is
not clean, and there is no revision slot left to fix it. It is never
printed for the **converged, slot(s) unspent** case — a thread reaching
`AUDITED` with revision budget to spare never reaches step 3 at all (step
2's combined-verdict pre-check exits first) — see §"Convergence" above
for the full contrast. The notice surfaces the override pointer (or, when
an elevated cap is already active, the prior rationale) at **the moment
the operator needs it** — the discoverability failure recorded in issue
#349 was "I didn't know the override existed," issue #869 was its memoir
recurrence, and issue #933 is the follow-up that made the final revision
itself always reach this notice with an actual critic verdict attached
rather than being silently unwritable. This contract mirrors
`anvil/skills/memo/commands/memo-revise.md` §"BLOCKED notice" line-by-line
(modulo the #933 revision-vs-version predicate correction), substituting
memoir's three-critic verdict shape. Required lines:

1. **State line**: `BLOCKED — <thread>.{N} is a final version written and
   unvalidatable under the current cap (max_iterations=<cap>). Human
   review required.` This phrasing (issue #933) is deliberate: it names
   what happened — a version was written, a critic pass ran on it, it is
   not clean — rather than just "hit the cap," which does not distinguish
   this from the healthy "converged" case that never reaches this notice.
2. **Trajectory line** (when `score_history` / verdict data is
   available): per-iteration totals plus the latest critical-flag state
   across all active critics, e.g. `Trajectory: v1=34/44 (draft), v2=37/44,
   v3=38/44, v4=38/44, v5=38/44 (advance=false, 1 unresolved corpus-audit
   critical flag); gap to advance threshold ≥39.` This frames the
   decision: well-conditioned (monotonic improvement, a named small gap)
   → consider the override; ill-conditioned (oscillating totals, a
   persistent fabrication-class flag) → the cap is doing its job,
   escalate to a human rather than buying more passes.
3. **Budget-composition line** (issue #869; REQUIRED when any consumed
   version carries `metadata.revision_class == "map_only"`): name how
   much of the budget went to bookkeeping rather than prose, e.g.
   `Budget composition: 5/5 consumed — v1 draft, 2 substantive (v2, v5),
   2 map-only (v3 corpus-drift repair, v4 provenance repoint; .tex
   byte-identical).`

   **How `v1` participates in the tally.** The `X/Y consumed` numerator is
   the version-dir count (`metadata.iteration` of the latest version), so
   `v1` — the `memoir-draft` output — is always one of the consumed
   versions. But `v1` carries **no `metadata.revision_class`**: a draft
   is not a revision, and `memoir-draft` deliberately mirrors only
   `metadata.max_iterations` / `metadata.iteration_cap_rationale`. So `v1`
   is reported as the standalone `v1 draft` term and is **never** counted
   as substantive or map-only, and — since issue #933 — is also never
   counted against `max_iterations` itself (only `v2..v{N}` are). The
   classified terms cover `v2..v{N}` only, every classified version
   appears in exactly one group, and the line must balance:
   `1 (draft) + substantive + map-only == X`, where `X = max_iterations +
   1` whenever the BLOCKED notice fires (it can only fire on a fully-
   consumed budget). At the default cap of 4 a fully-consumed budget
   therefore has exactly **four** classified revision passes (`v2..v5`),
   never five.

   A thread that spent half its budget on framework-detected provenance
   repairs is a materially different override case from one that spent it
   all on unconverged prose, and the operator should not have to
   reconstruct that from changelogs.
4. **Override pointer** (REQUIRED when no override is currently set —
   i.e. `metadata.iteration_cap_rationale` is `null` or absent):
   `Override available — see anvil/skills/memoir/SKILL.md §"Iteration cap
   and override contract" (schema-of-record: anvil/skills/memo/SKILL.md
   §"Per-document override contract"). Required fields on the matching
   <project>/BRIEF.md documents: entry: max_iterations (int ≥ 4) AND
   iteration_cap_rationale (non-empty string explaining why this chapter
   deserves more passes). Both fields are required; setting one without
   the other is a schema violation and the BRIEF parser will refuse to
   load. The override may raise the cap but not lower it below the
   principled default of 4.`
5. **Override-already-set surfacing** (when
   `metadata.iteration_cap_rationale != null` — an elevated cap is
   already active and the thread hit *that*): print the rationale in
   full, never truncated, so the operator sees the prior authorization,
   then: `This chapter is already at its elevated cap
   (max_iterations=<cap>). Raising further requires re-evaluating the
   rationale in <project>/BRIEF.md; see anvil/skills/memoir/SKILL.md
   §"Iteration cap and override contract".`

The notice is **advisory in one direction only**: it tells the operator
the override exists and what it costs to use. The reviser never raises
the cap on its own authority, and a BLOCKED thread stays BLOCKED until
the BRIEF is edited by a human and the reviser is re-invoked.

### Map-only repairs still consume an iteration

A **map-only** revision is one where every body file in the new version
(`<thread>.tex` plus any `sections/*.tex`) is byte-identical to its
parent's counterpart and the only substantive change is to
`provenance.md` — a corpus-drift repair or a provenance repoint that
fixes bookkeeping without touching a word of prose. `memoir-revise`
records these as `metadata.revision_class = "map_only"` and surfaces the
count in the BLOCKED notice's budget-composition line, but they **do
consume a revision slot** exactly like a substantive pass — issue #933
changed whether the `v1` *draft* counts against `max_iterations` (it no
longer does), not whether a map-only *revision* does (it still does).
(Classification applies to revision passes only — `<thread>.1/` is a
draft and carries no `revision_class`; it still occupies one version-dir
slot, reported as the composition line's `v1 draft` term.) Charging
map-only revisions against the cap remains a deliberate design decision
(issue #869), not an oversight:

- **`iteration` is derived from the version-dir number, and version dirs
  are immutable.** Exempting a class of revision from the budget would
  decouple `metadata.iteration` from `<thread>.{N}/`, requiring a second
  counter that can disagree with the directory listing. Two counters that
  can disagree is precisely the bookkeeping ambiguity this issue is
  complaining about — discovery (`enumerate_versions`), the orchestrator's
  `Iter` column, and `score_history` rows all key off the directory
  number today.
- **A map-only repair is not cheap.** It still triggers a full
  `memoir-review` + `memoir-audit` pass on the new version, including the
  exhaustive `kind: tool_evidence` corpus-audit sweep — the most
  expensive critic in the skill. Bounding that cost is exactly what the
  cap is for.
- **The operator decision the exemption would enable is already
  expressible, with a better audit trail.** "This chapter spent two
  iterations on provenance repairs and deserves more passes" is a
  textbook `iteration_cap_rationale` — and the budget-composition line
  now hands the operator the evidence for it verbatim.

If the canary later shows chapters routinely exhausting their budget on
map-only repairs *despite* the override, the follow-up is a
provenance-repair path that amends the map without producing a new
version dir at all — not a second, divergent iteration counter.

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and
`git.commit_per_phase` is `true`, end this phase: stage only the dirs
this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N}
[<state>]`, push if `git.push` is `true`. Git failures warn and
continue. Default off.

This phase's specifics:

- **Ordering**: after the `_progress.json` `done` write lands. On the
  no-write paths (AUDITED / BLOCKED at step 2-3) there is nothing to
  commit and the hook is a silent no-op.
- **Staging target**: ONLY this command's own `<thread>.{N+1}/` version
  dir.
- **Commit**: `anvil(memoir/revise): <thread>.{N+1} [REVISED]`.
