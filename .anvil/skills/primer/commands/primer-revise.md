---
name: primer-revise
description: Reviser for the primer skill. Consumes BOTH critic siblings (review + audit) for the latest version and produces a single revised version, preserving flagged-as-working pedagogical moves. REVIEWED+AUDITED → REVISED transition (loops until ≥35/44 with zero critical flags and a clean audit, or the iteration cap).
---

# primer-revise — Reviser

**Role**: reviser (one reviser consumes N critic siblings — here the review + audit pair, the `report` shape).
**Reads**: latest `<thread>.{N}/<thread>.md` + `_progress.json`, BOTH `<thread>.{N}.review/` and `<thread>.{N}.audit/` (all files), the resolved `spec_ref` sibling (when active), `<thread>/refs/` + shared `research/`, project `BRIEF.md`.
**Writes**: `<thread>.{N+1}/` with `<thread>.md`, `changelog.md`, `_progress.json` — or reports `AUDITED` without writing when the combined verdict pre-check passes.

## Procedure

1. **Discover state**: find the highest `N` with `<thread>.{N}/<thread>.md`. Require BOTH a completed `<thread>.{N}.review/` AND a completed `<thread>.{N}.audit/` (else exit pointing at the missing critic — `REVIEWED-PARTIAL`/`AUDITED-PARTIAL` are not advance-eligible per SKILL.md). Require `<thread>.{N+1}/` to not exist (immutability — never revise in place).
2. **Combined verdict pre-check**: read `<thread>.{N}.review/verdict.md` and `<thread>.{N}.audit/verdict.md`. When the review records `advance: true` (total ≥35/44, zero unresolved review critical flags) AND the audit records `audit_clean: true` (zero unresolved audit critical flags), the thread is **`AUDITED` — terminal**: report the publish-handoff summary (resolved body path, review total /44, clean audit, the handoff guarantees per SKILL.md §Publish handoff contract; note that `primer-figures` may optionally produce the PDF) and exit WITHOUT writing a new version.
3. **Iteration-cap check**: default `max_iterations: 4` (worst-case terminal version `<thread>.5/`); project-BRIEF paired override (`max_iterations` + `iteration_cap_rationale`) per the #349 memo contract — the BLOCKED notice surfaces the rationale verbatim when an elevated cap is hit. At cap → report `BLOCKED — human review required` and exit.
4. **Read all critic input**: from the review — `verdict.md` (top revision priorities first), `scoring.md` (per-dim deductions; dim 1 scaffolding gaps lead), `comments.md` (severity + `scope` tags), and the "What's working" list. From the audit — `verdict.md` (critical audit flags first), `findings.md` (per-claim factual + spec-consistency findings), `comments.md`. The two verdicts combine: a critical flag from *either* critic blocks.
5. **Handle the spec_ref contract (conditional)**: when the BRIEF declares an active `spec_ref`, re-resolve it (`anvil/lib/project_brief.py::resolve_spec_ref(<project_dir>, <slug>)`) and read the spec alongside the critic feedback so the revision stays consistent with it. When either critic carried the missing/unresolvable-`spec_ref` `major` finding, surface it in the report (the fix is operator-side BRIEF authoring or path correction, not body editing).
6. **Build the revision plan**, ordered: (1) critical flags — every flag from EITHER critic MUST be addressed:
   - **Duplicates formal spec section** (review-side) → replace the duplicated formal content with a teaching-then-pointing cross-reference ("for the formal treatment, see §X of the spec"), not by deleting the intuition around it.
   - **Contradicts cited spec** (audit-side) → correct the primer claim to agree with the spec (or, if the primer is right and the spec is wrong, that is an operator escalation, not a silent override — note it and block).
   - **Subtly-wrong intuition** (audit-side) → fix the simplification so it is lossy-but-true, not false — usually a re-worded analogy or an added caveat, never deleting the intuition wholesale.
   (2) `blocker`/`major` comments (a dim-1 scaffolding gap usually means re-ordering sections or teaching a prerequisite earlier, not local polish); (3) the lowest-scoring dims' deductions; (4) `minor`/`nit` only when they don't conflict with (1)–(3). Never touch the "What's working" list — the pedagogical moves the reviewer flagged as load-bearing.
7. **Write `<thread>.{N+1}/<thread>.md`** (slug-echo per #295) applying the plan. Re-run the drafter's step-5 self-disciplines on the result (dependency-order walk, cross-reference-not-duplicate check, technical-accuracy check) — the revision must not introduce a fresh instance of the failure mode it just fixed.
8. **Write `changelog.md`** mapping each consumed critic note to the change made (or to an explicit `declined — <reason>` entry; scoring deductions may be argued against, critical flags — from either critic — may not).
9. **Initialize `_progress.json`** for the new version: `phases.revise.state = done` (LAST write), carry forward `metadata.spec_ref_resolved` (when active), and **append the `score_history` row** for the completed review iteration per `anvil/lib/snippets/progress.md` §Convergence fields: `{ "iteration": <N>, "total": <reviewed-total>, "threshold": 35, "rubric_id": "anvil-primer-v1" }`. Stable-score termination (`STALLED`) follows `anvil/lib/snippets/rubric.md` §"Termination resolution order" over this history.
10. **Report**: e.g., `Revised botho-from-the-basics.1 → botho-from-the-basics.2 (addressed 1 audit critical flag + 3 major comments; 1 declined with reason). Next: primer-review + primer-audit botho-from-the-basics`.

## What primer-revise does NOT do

- **Never edits `<thread>.{N}/` or any critic sibling in place** — immutability is the audit trail.
- **Never advances state itself** — the next `primer-review` + `primer-audit` pass scores `<thread>.{N+1}/` on its own merits; there is no "the reviser fixed it" credit.
- **Never bypasses critical flags** — a changelog `declined` entry is legitimate for scoring deductions, never for a critical flag from either critic.
- **Never sands off the pedagogy** — rubric-point chasing that flattens flagged-as-working scaffolding is the named meta-failure mode.

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and `git.commit_per_phase` is `true`, end this phase: stage only the dirs this phase wrote, commit as `anvil(<skill>/<phase>): <thread>.{N} [<state>]`, push if `git.push` is `true`. Git failures warn and continue. Default off.

This phase's specifics:

- **Ordering**: after the `_progress.json` `done` write lands. On the no-write paths (AUDITED / BLOCKED at step 2–3) there is nothing to commit and the hook is a silent no-op.
- **Staging target**: ONLY this command's own `<thread>.{N+1}/` version dir.
- **Commit**: `anvil(primer/revise): <thread>.{N+1} [REVISED]`.
