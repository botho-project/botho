# Review verdict — botho-from-the-basics.2

**Verdict: ADVANCE (review side).** Total **43/44** (threshold ≥35). Review critical flags: **none**. The final READY/AUDITED determination combines this verdict with the parallel `primer-audit` sibling at revise time.

Scored on v2's own merits per the operator's instruction — no credit for being a revision. The v1 deductions were checked for cure on the page, and the six v1 "What's working" moves were checked for survival; both checks pass in full (see below).

## Critical flags: none

**Duplicates formal spec section — did not fire.** The `spec_ref` tier is ACTIVE (`../sections/*.tex` resolves to the 18 whitepaper LaTeX sections). The duplication sweep paid particular attention to the three v2 quantitative insertions, since each sits nearest the spec's own formal material:

- **The demurrage anchor (§9.4)** is one hedged number — "at the whitepaper's representative operating point, a factor-6 lineage that sat idle for a year owes on the order of 2% of the value it finally moves" — with no formula imported; the log-sigmoid/affine machinery stays deferred ("The exact curve is a log-domain sigmoid — see the whitepaper" — §9.3). It quotes WP §10's operating point ("2%/yr demurrage at factor 6"), not WP §7's fee-floor derivation.
- **The 611M arithmetic bridge (§9.1)** quotes one constant in prose ("about 6.3 million blocks a year") rather than reproducing WP §7's emission table or geometric-sum derivation; the reader can now check the sum mentally, which is a teaching device, not a normative restatement.
- **The complete capstone fee calculation (§10 step 2)** — "base rate (1 pico-BTH per byte, at the congestion floor) × ~5,000 bytes × factor 1 × a small multi-output penalty (4, for the two outputs) = 20,000 pico-BTH" — is a single worked instance of the primer's own 2-in-2-out example. It is demonstrably not a reproduction of WP §7's derivation: the spec's own worked example (`minimum_fee_dynamic_with_outputs`, 07-monetary.tex:380–398) uses 4,000 bytes and lands at 16 nano-BTH; the primer instantiates the same mechanism at its own ~5 KB capstone transaction and lands at 20 nano-BTH, and the general output-penalty rule (min(n,10)²) is never stated.

Elsewhere the body remains equation-free, the §11 table remains a navigation map, and every section still closes with a teach-then-point blockquote (targets verified against the resolved spec in the v1 review; unchanged in v2).

## Scores

1. Pedagogical scaffolding / learnability — **7/7**
2. Intuition before formalism — **6/6**
3. Worked-example / walkthrough concreteness — **5/5**
4. Technical accuracy (judgment side) — **5/5**
5. Spec cross-reference discipline — **5/5**
6. Audience calibration — **3/4**
7. Structure & navigation — **4/4**
8. Prose clarity — **4/4**
9. Rhetorical economy — **4/4**

## v1 deduction cure check (all cured)

- **Demurrage magnitude (v1 priority 1)** — cured: factor-6 ≈ 2%/yr, factor-3.5 roughly half, factor-1 exempt, accrual noted; hedged and formula-free.
- **The four jargon spots (v1 priority 2)** — all cured at point of use: §4 no longer uses "zero-knowledge" (crowd-size contrast instead; the term now debuts functionally in §5's range-proof paragraph); §7 expands "BFT (Byzantine-fault-tolerant) voting protocols, the family of agreement algorithms built to survive participants that fail or lie"; §8 glosses nonce/difficulty-target ("a throwaway counter they vary until the whole header's hash falls below a difficulty target") with a pointer back to §2; §9.5 expands "miner-extractable value (MEV)".
- **611M reproducibility (v1 priority 3)** — cured: the blocks-per-year bridge makes the sum exact (50 × 6,307,200 × 1.9375 = 611,010,000, matching WP §7's S₁).
- **Max-ring-factor referral (v1 priority 4)** — resolved by the v1 audit (both directions verified against WP §5); text correctly preserved.

## v1 "What's working" survival check (all preserved)

The problems-first spine, the recap-before-use pattern, the §9.6 design graveyard ("anything a holder can manufacture for free"), the honest caveats (§4, §6), the teach-then-point blockquotes, and the eleven-step capstone are all intact; the v2 edits are local insertions inside the existing scaffolding, exactly as the changelog claims.

## Top revision priorities

Nothing blocks; the artifact is advance-clean on the review side. If the operator elects another polish pass:

1. **Expand the two residual acronyms (dim 6, minor).** (a) §8 "RandomX is deliberately hostile to ASICs" — one parenthetical ("application-specific integrated circuits — chips custom-built for a single algorithm") gives ASIC the same treatment BFT/MEV/nonce received; (b) §11 map table "TEE approaches" — gloss ("trusted-execution-environment (secure-enclave) approaches") or drop the acronym from the row. Two one-clause fixes buy back the remaining point.
2. Nothing else. Do not add further quantitative material; the current density is at the ceiling of what teach-then-point comfortably carries.

## What's working (do not sand off in any future revision)

Everything on the v1 list, plus the three v2 insertions, which are now load-bearing teaching devices in their own right:

- The **hedged demurrage anchor** — "on the order of 2%" at a "representative operating point" is exactly the right precision; resist sharpening it into the formula.
- The **prose arithmetic bridge** for 611M — keep it in prose; do not tabulate.
- The **complete capstone fee instance** — keep it as an instance; resist generalizing "(4, for the two outputs)" into the spec's min(n,10)² rule, which belongs to WP §7.
- The **fee-leakage honesty** in the capstone summary ("reveals the transaction's size and the *lineage class* of the coins moved (and, for wealthier lineages, how long they sat idle)") — a caveat that strengthens trust.
