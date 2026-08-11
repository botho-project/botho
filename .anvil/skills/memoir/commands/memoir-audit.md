---
name: memoir-audit
description: Auditor for the memoir skill. Always writes a general factual/narrative-consistency sibling. When the corpus tier (#597) is active, ALSO writes the exhaustive kind:tool_evidence corpus-audit critic per anvil/lib/snippets/provenance.md §Section 4 as a <thread>.{N}.corpus-audit/ sibling — running the anchor-drift pre-pass (§Section 4a, #868) before inventorying every claim, classifying every provenance.md row VERIFIED/PARAPHRASE_OK/MISMATCH/NOT_FOUND/FABRICATED, and raising the five fabrication-class critical flags. Runs parallel with memoir-review. DRAFTED/REVISED → AUDITED transition.
---

# memoir-audit — Auditor

**Role**: auditor. Writes **two** critic siblings on a single invocation:
a **general** factual/narrative-consistency audit (always) and an
**exhaustive** corpus-provenance audit (conditional on the corpus tier
being active). Runs parallel with `memoir-review` per the
`report`/`primer`/`spec` two-critic shape — this command adds a THIRD
parallel sibling when the corpus tier is active, per SKILL.md's "N
parallel critics, one reviser" primitive (CLAUDE.md).
**Reads**: latest `<thread>.{N}/<thread>.tex` (+ `provenance.md` when the
corpus tier is active), `<thread>.{N}/_progress.json`, project `BRIEF.md`
(+ the resolved `corpus:` roots), `<thread>/refs/` + shared `research/`,
`rubric.md`.
**Writes**: `<thread>.{N}.audit/` (always) with `verdict.md`,
`findings.md`, `comments.md`, `_summary.md`, `_meta.json`,
`_progress.json`; `<thread>.{N}.corpus-audit/` (only when the corpus
tier is active) with the same file shape.

Both audit siblings are **read-only once written**. Revisions consume
them; they never modify them.

## Outputs

```
<thread>.{N}.audit/                         (general — ALWAYS written)
  verdict.md       Audit verdict + critical audit-flag paragraphs (factual + narrative consistency)
  findings.md      Per-claim table: Claim | Kind (factual/narrative) | Verified? | Evidence
  comments.md      Line-level audit comments keyed to the chapter body
  _summary.md      Machine-readable audit blocks
  _meta.json       { critic: "audit", ..., scorecard_kind: "human-verdict",
                     rubric_id: "anvil-memoir-v1", rubric_total: 44, advance_threshold: 39 }
  _progress.json   Phase state for the general auditor

<thread>.{N}.corpus-audit/                  (exhaustive — ONLY when corpus tier active)
  verdict.md       Corpus-audit verdict + fabrication-class critical-flag paragraphs
  findings.md      Per-provenance.md-row table: Claim | Source file | Line range | Classification | tool_calls evidence
                    (+ distinct anchor-drift rows per §Section 4a, #868 — never phrased as
                    MISMATCH/NOT_FOUND)
  comments.md      Line-level corpus-audit comments
  _summary.md      provenance_summary block (#597 §Section 7): total_claims / verified /
                    paraphrase_ok / mismatch / not_found / fabricated / anchor_drift_count (#868)
  _meta.json       { critic: "corpus-audit", ..., scorecard_kind: "human-verdict",
                     rubric_id: "anvil-memoir-v1", rubric_total: 44, advance_threshold: 39 }
  _progress.json   Phase state for the corpus-audit critic
```

**Atomicity** (issues #350, #376): each sibling is written atomically via
`anvil/lib/sidecar.py` — files staged under its own `.tmp/` dir,
atomically renamed on clean completion; stale staging from a prior
interrupt of EACH critic removed by `cleanup_one_staging(...)` at entry
(two independent sweeps, one per sibling).

## Procedure

1. **Discover state, sweep, open the general-audit sidecar**: find the
   highest `N` with `<thread>.{N}/<thread>.tex`; run
   `cleanup_one_staging(<thread>.{N}.audit)`; if `<thread>.{N}.audit/`
   AND (when the corpus tier check below determines it is required)
   `<thread>.{N}.corpus-audit/` already exist, exit early (idempotent).
   Otherwise open `staged_sidecar(final_dir=<thread>.{N}.audit,
   required_files=["verdict.md", "findings.md", "comments.md",
   "_summary.md", "_meta.json", "_progress.json"])`. Initialize
   `_progress.json` and `_meta.json` with `scorecard_kind:
   "human-verdict"`, **`rubric_id: "anvil-memoir-v1"`, `rubric_total:
   44`, `advance_threshold: 39`** (per-review version stamping, issue
   #346).

   **Non-Python-driver ordering (fail-open, manual fallback)**: a
   driver-less session uses the CLI shim (`uv run --project .anvil
   python -m anvil.lib.sidecar stage/commit/cleanup <thread>.{N}.audit
   --required verdict.md,findings.md,comments.md,_summary.md,_meta.json,
   _progress.json`, repeated identically for `<thread>.{N}.corpus-audit`
   when applicable) or, as a last resort, manual `mv`-based staging
   (write every required file into the `.tmp/` dir, `_progress.json`
   last, then `mv` as the last step; stamp `_meta.json` with
   `"atomicity_fallback": "manual-mv"`). Never write straight into either
   final sibling name.

2. **Read inputs**: the body, the matching BRIEF `documents:` entry,
   `<thread>.{N}/_progress.json` (the drafter's self-check +
   `metadata.corpus_dirs_resolved`), `<thread>/refs/` + shared
   `research/`.
3. **General factual / narrative-consistency audit (ALWAYS runs)**: walk
   every load-bearing claim, scene, and chronological anchor in the
   chapter. For each, record a `findings.md` row (`Claim | Kind: factual
   | Verified? | Evidence`). This pass covers narrative accuracy BEYOND
   sourcing (chapter-internal consistency — does the chapter contradict
   its own earlier-stated timeline or detail; does a scene's physical
   staging make sense) — it is NOT a substitute for the corpus-provenance
   sweep at step 4, which is a distinct, exhaustive, evidence-cited pass.
   A chapter-internal contradiction (e.g. a character's age stated two
   incompatible ways within the same chapter) is a `major`/`blocker`
   `findings.md` row here, escalated to a critical flag only if it rises
   to a factual showstopper the reviser must fix before advance.
4. **Resolve the corpus tier (conditional — the exhaustive
   corpus-provenance sweep)**: invoke
   `anvil/lib/project_brief.py::resolve_corpus_dirs(<project_dir>)`
   (project-level, per SKILL.md §Dual-corpus provenance) per
   `anvil/lib/snippets/provenance.md` §Section 1.
   - **When active** (>=1 resolved dir): open a SECOND
     `staged_sidecar(final_dir=<thread>.{N}.corpus-audit,
     required_files=["verdict.md", "findings.md", "comments.md",
     "_summary.md", "_meta.json", "_progress.json"])` (its own
     `cleanup_one_staging` sweep at entry). Run the **exhaustive**
     `kind: tool_evidence` corpus-audit critic per §Section 4:
     1. **Inventory** every attributed quote and factual claim in the
        chapter, and every row in `<thread>.{N}/provenance.md`. A claim
        in the chapter with **no `provenance.md` row is a finding in
        itself** (unmapped claim).
     2. **Anchor-resolution pre-pass (§Section 4a, issue #868)**: for
        every row carrying an `Anchor` value, run `python -m
        anvil.lib.provenance_anchor check <thread>.{N}/provenance.md
        <corpus_root> [...]` (one invocation covers the whole table;
        prefix `uv run --project .anvil` in an installed consumer repo)
        BEFORE classification — it searches the WHOLE cited file for the
        anchor text, not just the hinted `Line range`, and reports
        `NO_ANCHOR` / `FILE_NOT_FOUND` / `NOT_FOUND` / `RESOLVED` /
        `DRIFTED` per row. A `DRIFTED` row emits a distinct
        `findings.md` row worded as **anchor drift** ("quoted text still
        present in `<file>` but now at line `<X>` — Line range hint is
        stale") — never phrased as `MISMATCH`/`NOT_FOUND`, and NOT a
        `critical_flags` entry (drift alone is not fabrication). A
        `NO_ANCHOR` row (legacy, pre-#868) is not a finding — proceed to
        classify it against the cited `Line range` exactly as before.
     3. For **each** map row, open the resolved passage — the anchor's
        ACTUAL location from step 2 when it drifted, otherwise the cited
        `Line range` — in the resolved corpus and **classify** it with
        the five-way vocabulary (§Section 5): `VERIFIED`, `PARAPHRASE_OK`,
        `MISMATCH`, `NOT_FOUND`, `FABRICATED`. A `DRIFTED` row's content
        classification is independent of its drift finding — a relocated
        passage that still supports the claim is `VERIFIED`/
        `PARAPHRASE_OK` same as any other row.
     4. Every `MISMATCH` / `NOT_FOUND` / `FABRICATED` row emits a
        `findings.md` row with a non-empty **`tool_calls`** array
        recording the file-read operation that produced the evidence
        (the passage read, the lines inspected) — this is a `kind:
        tool_evidence` critic; `anvil/lib/review_schema.py` already
        enforces `tool_calls` on every `tool_evidence` finding.
     5. Fabrication-class entries additionally emit the corresponding
        **critical flag** (§Section 6 — see step 6 below), which routes
        through the existing verdict machinery
        (`anvil/lib/critics.py::_compute_verdict_impl` already
        short-circuits any `critical_flags` → `Verdict.BLOCK`).
     `kind: tool_evidence` with `findings == []` is valid — a chapter
     whose every claim VERIFIED/PARAPHRASE_OK is a clean corpus audit
     (zero drift findings included).
     Write `_progress.json.metadata.provenance_summary` (§Section 7): the
     six counts (`total_claims`, `verified`, `paraphrase_ok`,
     `mismatch`, `not_found`, `fabricated`) summing to `total_claims`,
     plus an `anchor_drift_count` counting `DRIFTED` rows separately (a
     bookkeeping count, not one of the six that sum to `total_claims` —
     drift is orthogonal to content classification).
     **Finalize the corpus-audit `_meta.json` + `_progress.json`**
     (`_progress.json` LAST) and exit that `staged_sidecar` block —
     manifest verified, staging dir atomically renamed to
     `<thread>.{N}.corpus-audit/`.
   - **When inactive** (no `corpus:` key, `corpus: null`, or
     `corpus: []`): do NOT write a `<thread>.{N}.corpus-audit/` sibling
     at all — byte-identical to the corpus-tier-absent posture. Record a
     **`major` finding in the general `<thread>.{N}.audit/`** recommending
     the operator declare `corpus:` (the class's defining constraint —
     "every reconstructed detail traces to a source" — is otherwise
     unenforceable).
   - **Declared-but-missing dirs**: the tier ACTIVATES; `resolve_corpus_dirs`
     returns `missing: true` entries (**never raises**). Surface the
     broken declaration as a **`major` finding** in the general audit;
     the exhaustive sweep does not run against a missing directory (no
     `<thread>.{N}.corpus-audit/` sibling is written) — graceful
     degradation, **no false critical flag**, no crash.
5. **Identify general audit-side critical flags** (in `<thread>.{N}.audit/verdict.md`):
   a chapter-internal factual contradiction severe enough to make the
   narrative untrustworthy on its own terms (rare; most findings are
   `major`/`blocker`, not critical). If none: "Critical flags: none."
6. **Identify fabrication-class critical flags** (in
   `<thread>.{N}.corpus-audit/verdict.md`, conditional on the active
   sweep at step 4) — each with a one-paragraph justification quoting
   the offending chapter text and the corpus evidence (or its absence).
   These are the **five #597 flags, reused verbatim — no memoir-specific
   flag types for v1**:
   - **`fabricated_quote`** — verbatim-quoted text absent from the
     corpus.
   - **`fabricated_fact`** — a named date/name/event not traceable to
     any corpus passage.
   - **`misattribution_of_substance`** — an event or memory attributed
     to a speaker whose corpus does not contain it (the **substance-level**
     flag; the voice-identity twin — `misattribution` — is
     `memoir-review`'s job).
   - **`anachronism`** — an era-incompatible detail contradicted by the
     corpus chronology.
   - **`unattributed_paraphrase`** — authorial invention presented as a
     subject's memory without any corpus grounding.

   If none (and the corpus tier is active): "Critical flags: none." If
   the corpus tier is inactive, the corpus-audit sibling does not exist
   and these flags simply cannot fire — CriticalFlag.type is not raised
   from an inactive tier.
7. **Verdict**: `<thread>.{N}.audit/verdict.md` records `audit_clean:
   true` iff zero unresolved general critical flags.
   `<thread>.{N}.corpus-audit/verdict.md` (when written) records
   `audit_clean: true` iff zero unresolved fabrication-class critical
   flags. (Neither auditor sibling scores the /44 rubric — that is
   `memoir-review`; the reviser combines all three siblings' verdicts.)
8. **Report**: e.g., `Audited 00-introduction.1 → general audit clean (1
   minor chapter-internal note); corpus tier active, corpus-audit clean
   (22 claims: 18 VERIFIED, 3 PARAPHRASE_OK, 1 NOT_FOUND — 0
   fabrication-class flags). Next: memoir-revise 00-introduction (after
   memoir-review)`.

## What memoir-audit does NOT do

- **Never edits the body.** Read-only against `<thread>.{N}/`.
- **Never scores the /44 rubric** — that is `memoir-review`.
- **Never writes a `corpus-audit` sibling when the corpus tier is
  inactive** — no `{ran: false}` placeholder dir; the tier's absence is
  surfaced as a `major` finding in the general audit sibling only.
- **Never crashes on a missing/unresolvable `corpus:`** —
  `resolve_corpus_dirs` never raises; the broken declaration is a
  `major` finding and the exhaustive sweep is skipped.
- **Never invents a memoir-specific critical-flag vocabulary** — reuses
  the five #597 fabrication-class flags verbatim.
- **Never populates or edits `provenance.md`** — that is the
  drafter's/reviser's job; the auditor reads and classifies it.

## Scorecard kind

Both critics emit the `human-verdict` scorecard kind per
`anvil/lib/snippets/scorecard_kind.md`. Each sibling's `_meta.json` MUST
include `"scorecard_kind": "human-verdict"` plus the three
rubric-stamping fields (`"rubric_id": "anvil-memoir-v1"`, `"rubric_total":
44`, `"advance_threshold": 39"`).

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and
`git.commit_per_phase` is `true`, end this phase: stage only the dirs
this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N}
[<state>]`, push if `git.push` is `true`. Git failures warn and
continue. Default off.

This phase's specifics:

- **Ordering**: after each staged-sidecar atomic rename (issue #350)
  lands its final-named sibling.
- **Staging target**: ONLY this command's own `<thread>.{N}.audit/` and
  (when written) `<thread>.{N}.corpus-audit/`.
- **Commit**: `anvil(memoir/audit): <thread>.{N} [AUDITED]` — a single
  commit covering both siblings when both were written this pass.
