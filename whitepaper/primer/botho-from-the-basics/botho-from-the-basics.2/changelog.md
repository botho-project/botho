# Changelog — botho-from-the-basics.1 → botho-from-the-basics.2

Consumes `botho-from-the-basics.1.review/` (ADVANCE, 41/44, zero critical
flags) and `botho-from-the-basics.1.audit/` (CLEAN, zero critical flags).
The combined verdict pre-check would normally report AUDITED and exit
without writing; the operator explicitly waived that early exit and
directed a polish revision (public-facing teaching document; the critics
enumerated cheap, concrete fixes). Every consumed note is mapped below.
All new numbers were re-verified against the resolved `spec_ref`
(`whitepaper/sections/*.tex`) before being written in.

## Review notes (botho-from-the-basics.1.review/)

| # | Source | Note | Disposition |
|---|---|---|---|
| R1 | verdict priority 1 / comment 1 (major, expand) | §9.4 demurrage has no order of magnitude | **Changed.** Added one qualitative-plus-numbers anchor: at the whitepaper's representative operating point (WP §10: "2%/yr demurrage at factor 6"), a factor-6 lineage idle for a year owes ~2% of the value moved; a factor-3.5 lineage roughly half that (affine in factor per the implemented mechanism); factor-1 exempt. No formula imported — the log-sigmoid/affine machinery stays deferred to the spec, per the reviewer's own guardrail. |
| R2 | verdict priority 4 / comment 2 (major, preserve) | Max-ring-factor honesty claim — referred to auditor | **No change needed (resolved by audit).** Audit finding row 61 verified both directions against WP §5 (Ring Signature Tag Propagation + Decoy Selection): the tag-similarity constraint both stops whales hiding behind low-factor decoys and bounds overcharge of low-factor spenders. Text preserved as written. |
| R3 | verdict priority 2a / comment 3 (minor) | §4 uses "zero-knowledge system" before §5 teaches it | **Changed.** Adopted the reviewer's option (b): §4 now leans on the crowd-size contrast alone ("not in the whole universe of outputs, as Zcash's shielded pool allows"); "zero-knowledge" now first appears in §5, functionally glossed at point of use. |
| R4 | verdict priority 2b / comment 4 (minor) | §7 "BFT" never expanded at point of use | **Changed.** Expanded at first flow use: "BFT (Byzantine-fault-tolerant) voting protocols, the family of agreement algorithms built to survive participants that fail or lie." Subsequent "BFT-family" uses now have a bound expansion. |
| R5 | verdict priority 2c / comment 5 (minor) | §8 "nonce"/"difficulty target" un-taught | **Changed.** Glossed at point of use: "a throwaway counter they vary until the whole header's hash falls below a difficulty target," with an explicit pointer back to §2's hash-unpredictability idea ("the only way to win is brute trial"). |
| R6 | verdict priority 2d / comment 6 (minor) | §9.5 "(MEV)" acronym assumed | **Changed.** Expanded inline: "the transaction-reordering games known as miner-extractable value (MEV)." |
| R7 | verdict priority 3 / comment 7 (minor) | §9.1 "roughly 611 million BTH" not reproducible | **Changed.** Added the blocks-per-year bridge: ~6.3 million blocks/year at the 5-second reference pace (WP §7: halving period 6,307,200 blocks ≈ 1 year), so year one ≈ 315M BTH, halving each year, summing to ~611M. The number is now checkable mental arithmetic. |
| R8 | comment 8 (nit, preserve) | §4 forward-pointer to cluster tags is exemplary | **Preserved** unchanged. |
| R9 | comment 9 (nit, preserve) | §11 map table is navigation, not duplication | **Preserved** unchanged. |
| R10 | comment 10 (nit, preserve) | §2 paint-mixing analogy correctly scoped | **Preserved** unchanged (not stretched to cover point addition). |
| R11 | verdict "What's working" list | Problems-first spine; recap-before-use; §9.6 design graveyard; honest caveats; teach-then-point blockquotes; the capstone | **Preserved** — none of the six flagged moves was restructured; all edits are local insertions/rewordings inside the existing scaffolding. |

## Audit notes (botho-from-the-basics.1.audit/)

| # | Source | Note | Disposition |
|---|---|---|---|
| F2 | findings F2 (major, operator-facing) | Whitepaper §4 ↔ code divergence on ML-DSA-65's role (minting vs per-input) | **Declined for this artifact — operator escalation, per the audit's own disposition.** The audit states "no primer edit required"; the primer correctly follows the declared oracle (the whitepaper), and the whitepaper↔implementation reconciliation is being filed separately as a GitHub issue by the operator. The primer's ML-DSA-65 treatment is untouched. |
| F3 | findings F3 (minor) | Capstone step 2 fee formula omits the ×4 output penalty that produces the stated figure | **Changed.** The complete calculation is now printed: 1 pico-BTH/byte (congestion floor) × ~5,000 bytes × factor 1 × multi-output penalty 4 = 20,000 pico-BTH = **20 nano-BTH**, replacing the vague "a few tens of nano-BTH." Matches WP §7's own derivation shape (`minimum_fee_dynamic_with_outputs`, min(2,10)² = 4) and the Parameter appendix's "<5 KB for 2-in-2-out." |
| F4 | findings F4 (minor) | "two-of-forty possible inputs" is a union-size upper bound | **Changed.** Rephrased to the real guarantee: "spending two inputs, each hidden among twenty candidate coins" (rings may overlap, so forty was an upper bound). |
| F5 | findings F5 (minor) | "roughly fair odds per watt" extends past the spec's claims | **Changed.** Now: "odds proportional to the computation it contributes, with no structural edge for specialized hardware" — exactly the spec's linear-reward-scaling + ASIC-resistance claims, no per-watt assertion. |
| F6 | findings F6 (minor) | §9.6(3) "redistribution marginally *improves*" — full-system row is +0.078 honest vs +0.076 gamed | **Changed.** Now: "redistribution holds essentially unchanged — it does not degrade," matching WP §10's full-system numbers and its own claim. |
| F7 | findings F7 (minor) | "the overwhelming majority of supply" is horizon-dependent under the 2% tail | **Changed.** Horizon-qualified: "the large majority of supply for the chain's first decades … twenty years in, Phase-1 coins still make up about 69% of it" (WP §7 projection: 611M / 884.9M at year 20). |
| F8 | findings F8 (observation) | "saturate near 6×" at exactly 1M BTH is ≈5.2× | **Changed** (auditor's suggested phrasing adopted): "million-BTH lineages climb past 5× toward the 6× ceiling" (WP §5 table: ≈5.2–6.0× at ≥1M). |
| F9 | findings F9 (observation) | "fee … reveals only the lineage class" — also leaks size and, for factor >1, the demurrage clock | **Changed.** Capstone summary now reads: "a fee that reveals the transaction's size and the *lineage class* of the coins moved (and, for wealthier lineages, how long they sat idle), but never who and never how much." |
| F10 | findings F10 (observation, spec-internal) | WP-internal inconsistencies (§10 "Min block time 5s" vs §7's 3 s floor; §2's CLSAG ring-16 = 704 B vs §5's ring-20) | **No primer change** — both are whitepaper editorial items; the primer already sides with the governing spec statements in both cases, as the audit confirms. Left to the operator alongside F2. |

## Self-disciplines re-run on the result

- **Dependency-order walk**: every newly introduced term is glossed at point
  of use (BFT, nonce/difficulty target, MEV, multi-output penalty) or moved
  behind its teaching point (zero-knowledge now debuts in §5). The §9.1
  bridge uses only the 5-second reference pace taught in §8; the capstone
  fee arithmetic uses only picocredit units taught in §9.1.
- **Cross-reference-not-duplicate**: the demurrage anchor is one number plus
  a hedge ("representative operating point"), no formula; the fee
  calculation is a single worked instance of the primer's own example (the
  fix the auditor requested), not a restatement of WP §7's equation
  environment; the blocks-per-year constant is quoted in prose, not a table.
- **Technical accuracy**: all new figures re-verified against the spec —
  2%/yr at factor 6 (WP §10 operating point; affine-to-zero per the
  implemented mechanism), 6,307,200 blocks/year (WP §7 constants +
  Parameter appendix), 50 × 6.3M ≈ 315M and Σ ≈ 611M (WP §7:
  S₁ = 611,010,000), 69% at year 20 (WP §7 projection: 884.9M), 20 nano-BTH
  (WP §7 derivation shape at the appendix's ~5 KB size), ≈5.2–6.0× at ≥1M
  BTH (WP §5 table), +0.078/+0.076 (WP §10 table).

Word count: 8,108 → 8,338 (+230).
