# Fixture: sentinel v6 skeleton→body derivation (word-soup catch)

**Studio canary date**: 2026-07-27/28
**Source thread**: `sentinel` (Studio canary, internal) — the same thread whose readability ratchet produced #745–#748 and whose rewrite produced #749–#751.
**The catch**: sentinel memo.5 scored **43/44, advance:true, 0 critical** under anvil-memo-v2 (dims 8 and 9 both 4/4, 22/22 refs back-checks verified) — yet the operator rejected it: *"almost word-soup bad … inventing jargon and mimicking the prosody of a report but not having a clear understanding of what we are trying to communicate."* Every claim was true, sourced, and hedged; the whole did not transmit the plan. After the rejection the orchestrator hand-wrote the seven-sentence skeleton and showed it to the operator, who corrected **one sentence** — the root claim, from a competitive-positioning claim ("owning the agent beats owning the gateway") to a value-migration thesis ("owning the harness beats owning the models"). That one-line correction reshaped the entire document.

This fixture preserves that failure mode as the canary anchor for the **skeleton↔section derivation leg** (`anvil/skills/memo/commands/memo-review.md` step 4e skeleton leg + `anvil/skills/memo/rubric.md` §"Summary-detail consistency" §"Skeleton↔section derivation leg (issue #752)").

## Why this fixture exists

Phase A of issue #752 ships the derivation back-check as reviewer-prose-only (no Python detector). The fixture serves three purposes:

1. **Schema anchor**: `expected_findings.json` carries the verbatim `summary_detail_consistency.skeleton_derivation` sub-block shape (extracted to the top level here for a self-contained anchor). The Phase A test (`tests/test_skeleton_derivation_fixture.py`) asserts the file parses against the schema as a shape contract.
2. **Phase B detector regression anchor**: when a future Phase B issue lands a `anvil/skills/memo/lib/skeleton_derivation.py` detector, this fixture is the regression anchor — "did the detector still catch the body delivering a thesis that contradicts its own skeleton root claim?"
3. **Worked example for the reviewer agent**: a reviewer reading `rubric.md` §"Summary-detail consistency" §"Skeleton↔section derivation leg" sees the verdict-tag rubric applied to a real memo — the root-claim `CONTRADICTED` (value-migration vs. gateway-positioning), the `§Recapture risk` `ABSENT` (a skeleton claim no body section delivers), and the `harness-native efficiency` `DIVERGENT` (an undefined coinage load-bearing a body section the skeleton never sanctioned).

## Fixture contents

- `skeleton.md` — the corrected seven-sentence claim tree: root claim (value-migration thesis) + three section claims (Commoditization / Recapture risk / Enclosure).
- `memo.md` — a minimal body that DRIFTS from the skeleton: its thesis argues gateway-vs-agent positioning (contradicting the root), it has no Recapture-risk section (ABSENT), and its §Enclosure load-bears on undefined coinages ("harness-native efficiency", "the in-boundary plane widens", "the escalation floor is invariant" used without the skeleton's definition).
- `expected_findings.json` — the expected `skeleton_derivation` sub-block. Three findings: one `CONTRADICTED` / `critical` (root claim), one `ABSENT` / `important` (§Recapture risk), one `DIVERGENT` / `important` (undefined coinage).
- `README.md` — this file.

## Worked-example walkthrough

The reviewer enumerates the root claim + three section claims and back-checks each against the body:

| Claim | Skeleton | Body | Verdict |
|---|---|---|---|
| Root | "owning the harness beats owning the models" (value migration) | Executive summary argues "owning the agent beats owning the gateway" (competitive positioning) | **CONTRADICTED** (critical) |
| §Commoditization | orchestration work already commoditized at the model layer | §Commoditization delivers it | MATCH |
| §Recapture risk | model owners cannot easily recapture orchestration-layer value | (no such section) | **ABSENT** (important) |
| §Enclosure | the harness owns the invariant escalation floor | §Enclosure argues gateway defensibility via undefined "harness-native efficiency" | **DIVERGENT** (important) |

The CONTRADICTED root-claim finding at critical severity becomes a `Skeleton derivation: CONTRADICTED` critical flag in `verdict.md` and forces `advance: false` regardless of the rubric total — see `commands/memo-review.md` step 7 + step 10 for the verdict integration.

## Backwards-compat note

When a memo has no `skeleton.md`, the leg records `ran: false` and takes no deduction (every pre-#752 thread). This fixture is the `ran: true` worked-example case; the `ran: false` path is byte-identical to the pre-#752 reviewer.

## Related

- Issue #752 — the canary report this fixture encodes.
- `anvil/skills/memo/rubric.md` §"Summary-detail consistency" §"Skeleton↔section derivation leg (issue #752)" — the rubric prose this fixture demonstrates.
- `anvil/skills/memo/commands/memo-draft.md` §"Author the skeleton before the body" — the producer-side skeleton contract.
- `anvil/skills/memo/tests/fixtures/summary_detail_consistency/northwind_gen_attribution/` — the sibling summary↔detail leg's fixture (the precedent this one mirrors).
