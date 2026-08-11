---
name: memoir
description: Per-chapter-thread status orchestrator for the memoir skill. Discovers memoir threads under cwd, reports state-machine position per thread (including corpus/voice tier declaration status), and recommends the next command. Read-only. Does NOT rebuild anvil:project-book's portfolio view — use /anvil:project-book for the all-chapters table.
---

# memoir — Per-chapter-thread status orchestrator

**Role**: per-thread status orchestrator (read-only; reports state, does
not mutate).
**Reads**: all `<thread>.*/` directories under the current working
directory (ONE chapter thread's directory tree, or several when invoked
at a project root — see §Scope note).
**Writes**: nothing on disk. Returns a status report.

## Scope note — this is NOT `anvil:project-book`

This command follows the `primer.md`/`spec.md`/`report.md` precedent: a
**per-thread** status report, not a project-wide portfolio view. The
"show me all six chapters, their states, their scores" table is
`anvil:project-book`'s job (`BOOK_REPORT.md`) — composing that view a
second time here would duplicate #596 rather than reuse it. Assemble the
book with `/anvil:project-book <project-dir>`; do not look for a
portfolio command here.

## Inputs

- **CWD**: a chapter thread directory (or the project root, in which case
  every chapter thread found is reported individually — still one row per
  thread, never a cross-thread aggregate).
- **Discovery rule**: a thread is detected by the presence of any
  `<slug>.{N}/` directory (with `_progress.json`). The slug is the
  directory name up to the first `.<digit>`. A bare `<slug>/` directory
  without versioned siblings is a brief-only thread in state `EMPTY`.

## Procedure

1. Enumerate all directories matching `<slug>`, `<slug>.{N}`, or
   `<slug>.{N}.<critic>` (where `<critic>` ∈ {`review`, `audit`,
   `corpus-audit`}).
2. Group by slug. For each slug, identify:
   - The latest `N` for which `<slug>.{N}/` exists.
   - Which sibling critic dirs exist at that `N` (`.review/`, `.audit/`,
     `.corpus-audit/`).
   - The verdict (advance/block, total /44, critical flags) from
     `<slug>.{N}.review/verdict.md`, the general audit verdict from
     `<slug>.{N}.audit/verdict.md`, and — when present — the corpus-audit
     verdict from `<slug>.{N}.corpus-audit/verdict.md`.
   - The iteration count, `max_iterations`, and
     `iteration_cap_rationale` from `<slug>.{N}/_progress.json`
     (default 4, rationale `null`; project-BRIEF paired override per
     SKILL.md §"Iteration cap and override contract"). Also collect each
     version's `metadata.revision_class` (`"map_only"` /
     `"substantive"`) — the budget-composition input for the
     `## Operator notes` entry below. The key is **always absent on
     `<slug>.1/`**: v1 is the `memoir-draft` output and a draft is not a
     revision, so only `v2..v{N}` are ever classified. It is also absent
     on every pre-#869 version dir. Treat both cases as *unclassified*,
     not as a missing-data anomaly — v1 is reported as the composition
     line's standalone `v1 draft` term.
   - Whether the project BRIEF declares a top-level `corpus:` and a
     `voice:` block (with `subjects:`) — informational, surfaced so the
     operator sees at a glance which tiers are active for this project
     (the tiers are project-level, not per-chapter — SKILL.md §Dual-corpus
     provenance / §Dual voice tiers).
3. Compute the state-machine position per thread using the table in
   `SKILL.md` §State machine.
4. Recommend the next command per thread:

   | State | Recommended next command |
   |---|---|
   | `EMPTY` | `memoir-draft <thread>` |
   | `DRAFTED` (figure references present, exhibits not yet rendered) | `memoir-figures <thread>` first, then `memoir-review <thread>` + `memoir-audit <thread>` (parallel) |
   | `DRAFTED` (no figure references / exhibits current) | `memoir-review <thread>` + `memoir-audit <thread>` (parallel) |
   | `REVIEWED-PARTIAL` | `memoir-audit <thread>` (run the missing critic) |
   | `AUDITED-PARTIAL` | `memoir-review <thread>` (run the missing critic) |
   | `REVIEWED+AUDITED` (any critic blocks, `N + 1 <= max_iterations`) | `memoir-revise <thread>` |
   | `REVIEWED+AUDITED` (any critic blocks, `N + 1 > max_iterations`) | `BLOCKED — human review required` (+ the override pointer, see `## Operator notes`) |
   | `AUDITED` (all clear) | `memoir-figures <thread>` (refresh/produce PDF+exhibits if not current), then `/anvil:project-book <project-dir>` to assemble the book |

   The cap predicate is the one in `memoir-revise.md` step 3, applied to
   the latest version number `N` — **not** "is `iteration` equal to
   `max_iterations`." A thread at `Iter 4/4` that is `AUDITED` is
   terminal and healthy; the `Next` cell recommends `memoir-figures`, not
   BLOCKED. Only a thread at `N + 1 > max_iterations` **with a critic
   still blocking** is BLOCKED (issue #869 — reporting a clean
   at-the-ceiling terminus as capped is a bug).

5. Detect anomalies and surface them:
   - A `<slug>.{N}/_progress.json` with any phase `in_progress` AND the
     version dir older than 10 minutes — likely a crashed phase; the next
     invocation's `cleanup_one_staging` sweep handles stale critic
     staging.
   - A critic sibling dir without a matching `<slug>.{N}/` — orphan;
     report.
   - A gap in version numbers — report.
   - A project declaring `corpus:` with no `<slug>.{N}.corpus-audit/`
     sibling at the latest reviewed/audited `N` — the exhaustive
     provenance sweep has not run; recommend `memoir-audit <thread>`.
   - An `AUDITED` thread whose critic siblings carry a stale rubric stamp
     (`_meta.json.rubric_id` != `anvil-memoir-v1`) — informational;
     recommend `anvil:rubric-rebackport`.

## Output format

Print a markdown table to stdout:

```
| Thread          | Latest | State            | Review | Audit | Corpus-audit | Iter | Next                              |
|-----------------|--------|------------------|--------|-------|--------------|------|------------------------------------|
| 00-introduction | .4     | AUDITED          | 44/44  | clean | clean        | 4/4  | memoir-figures 00-introduction     |
| 01-childhood    | .1     | REVIEWED+AUDITED | 35/44  | flag  | clean        | 1/4  | memoir-revise 01-childhood         |
| 02-the-farm     | .4     | REVIEWED+AUDITED | 38/44  | clean | flag         | 4/4  | BLOCKED — human review required    |
| appendix        | -      | EMPTY            | -      | -     | -            | 0/4  | memoir-draft appendix              |
```

The first and third rows are the two distinct at-the-ceiling cases
(issue #869): `00-introduction` reached `Iter 4/4` and terminated
`AUDITED` on score + clean flags — the cap was reached but was never the
terminating condition, so it gets the normal terminal recommendation.
`02-the-farm` is at the same `4/4` with a corpus-audit flag still open,
so the next revise pass would exceed the cap and it is genuinely BLOCKED.
The `Iter` column alone never distinguishes them — the `State` +
`Next` cells must.

Follow the table with an `## Anomalies` section if any were detected, and
an `## Operator notes` section for threads requiring human review
(iteration cap exceeded, an unresolved fabrication-class critical flag
across multiple revisions, an undeclared `corpus:`/`voice:` tier
surfaced repeatedly, etc.).

For a BLOCKED-on-cap thread, the `## Operator notes` entry MUST carry the
same three surfacings `memoir-revise`'s BLOCKED notice does, so the
portfolio view is not a strictly worse place to learn about the override
than the per-thread command:

1. the **budget composition** — how many of the consumed iterations were
   `revision_class: "map_only"` provenance repairs versus substantive
   prose revisions, with `v1` named separately as the (unclassified)
   draft so the entry balances the same way `memoir-revise`'s notice
   does: `1 (draft) + substantive + map-only == consumed`;
2. the **override pointer** when `iteration_cap_rationale` is `null`
   (required BRIEF fields: `max_iterations` int ≥ 4 **and**
   `iteration_cap_rationale`, both or neither — see SKILL.md §"Iteration
   cap and override contract");
3. the **existing rationale verbatim** when an elevated cap is already
   active, plus the "re-evaluate the rationale before raising again"
   prompt.

This command never edits the BRIEF and never raises a cap — it reports.

## Notes

- This command does **not** write to disk. Safe to run repeatedly. As a
  read-only command it is exempt from the per-phase git-sync hook by
  definition (SKILL.md §"Git sync hook").
- The orchestrator is the recommended per-thread entry point; the
  lifecycle commands (`memoir-draft`, `memoir-review`, `memoir-audit`,
  `memoir-revise`, `memoir-figures`) can be invoked directly in sequence.
- For the cross-chapter assembled-book view, use `/anvil:project-book
  <project-dir>` — never reimplement it here (§Scope note above).
