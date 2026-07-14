# Review verdict — botho-from-the-basics.1

**Verdict: ADVANCE (review side).** Total **41/44** (threshold ≥35). Review critical flags: **none**. The final READY/AUDITED determination combines this verdict with the parallel `primer-audit` sibling at revise time.

## Critical flags: none

**Duplicates formal spec section — did not fire.** The `spec_ref` tier is ACTIVE (`../sections/*.tex` resolves to the 18 whitepaper LaTeX sections). The duplication sweep found no formal derivation, proof, or normative table reproduced in the primer. The body carries zero equations; the one place a formula is nearest at hand, the primer explicitly declines and points — "(The exact curve is a log-domain sigmoid — see the whitepaper; the intuition is that the factor climbs across *orders of magnitude* of lineage wealth…)" (§9.3) — and constants are quoted selectively in prose, never as a table restating §7 (Monetary Constants) or the Parameter Justification appendix. The §11 table is a navigation map (question → whitepaper section), not a normative table. Every section ends with a teach-then-point blockquote whose named subsections were verified to exist verbatim in the resolved spec.

## Scores

1. Pedagogical scaffolding / learnability — **6/7**
2. Intuition before formalism — **6/6**
3. Worked-example / walkthrough concreteness — **4/5**
4. Technical accuracy (judgment side) — **5/5**
5. Spec cross-reference discipline — **5/5**
6. Audience calibration — **3/4**
7. Structure & navigation — **4/4**
8. Prose clarity — **4/4**
9. Rhetorical economy — **4/4**

## Top revision priorities

1. **Give demurrage an order of magnitude (dim 3, major).** "they owe a charge proportional to the value moved, the time held, and the cluster factor" (§9.4) is the only mechanism in the flagship economics section a reader leaves with no feel for size. One sentence — what a factor-6 lineage holding for a year actually pays, vs. the factor-1 exemption — closes the gap. Keep it qualitative-plus-one-number; do not import the formula (that would trade a dim-3 point for a dim-5 flag).
2. **Sand off the four un-taught jargon spots (dims 1 + 6).** (a) §4 "a zero-knowledge system like Zcash's shielded pool" — gloss in a clause or defer the comparison to after §5's range-proof introduction of the idea; (b) §7 expand "BFT" at point of use; (c) §8 give "nonce"/"difficulty target" a half-sentence gloss ("a throwaway counter miners vary until the block's hash falls below a target"); (d) §9.5 expand or drop "(MEV)". Each is a one-clause fix; together they buy back the dim-1 and dim-6 points.
3. **Close the 611M arithmetic gap (dim 3, minor).** §9.1 lists per-block rewards (50, 25, 12.5, …) and asserts "Summed up, this distributes roughly **611 million BTH**" — the blocks-per-year link is missing, so the number is not reproducible by the reader. One clause ("at the ~5-second reference pace, about N million blocks a year") fixes it.
4. **For the auditor (not a review deduction):** verify the §9.4 claim that charging "the **maximum factor among the ring members**" is made honest by tag-similar decoy selection — i.e., confirm against WP §5 (Decoy Selection / Cluster Tags and Progressive Fees) that the similarity constraint bounds fee inflation for a low-factor spender as well as preventing whales hiding behind low-factor decoys. See comments.md #2.

## What's working (do not sand off in revision)

- **The problems-first spine.** §1's three problems are the load-bearing frame; every later section points back ("This is the 'harvest now, decrypt later' threat from Section 1"). Preserve.
- **The recap-before-use pattern.** Each section re-states exactly the prior machinery it builds on ("Now recall the *shape* of the classical stealth address from Section 3") — this is why an 8,100-word text never loses a newcomer. Preserve.
- **§9.6, the design graveyard.** Teaching the final mechanism via the failures of the simpler designs ("never pay out on anything a holder can manufacture for free") is the strongest pedagogy in the piece. Preserve.
- **The honest caveats.** §4's "probabilistic anonymity… Botho accepts this trade deliberately" and §6's "chosen, argued, and priced" build reader trust without hedging the teaching. Preserve.
- **Teach-then-point discipline.** The per-section spec blockquotes with verbatim subsection names are exactly the companion contract. Preserve.
- **The capstone.** Eleven steps, concrete numbers, every mechanism touched, and the closing observer's-eye summary ("private to observers, honest to validators, progressive to the economy"). Preserve.
