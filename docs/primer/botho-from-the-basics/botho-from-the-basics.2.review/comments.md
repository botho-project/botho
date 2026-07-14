# Line-level comments — botho-from-the-basics.2

Keyed to `botho-from-the-basics.md` (primer §, approximate body line). Severity: blocker / major / minor / nit. Scope: preserve / expand / reduce.

## 1. [minor | expand] §8 (~line 606) — ASIC never expanded

"RandomX is deliberately hostile to ASICs: it executes randomly generated programs and leans on general-purpose CPU features (caches, branch prediction, wide memory)". The surrounding contrast (general-purpose CPU vs. "specialized-hardware oligopoly" one sentence later) carries most of the meaning, but the acronym itself never gets the point-of-use treatment v2 gave BFT, MEV, and nonce/difficulty-target. One parenthetical — "(application-specific integrated circuits — chips custom-built for a single algorithm)" — closes it.

## 2. [minor | expand] §11 map table (~line 969) — "TEE approaches" unglossed

"| Comparisons: Monero, MobileCoin, Zcash, TEE approaches | §2 Related Work |". TEE appears nowhere else in the primer and gets no expansion. The stated reader (technically curious, not a cryptographer) may not know it. Gloss ("trusted-execution-environment approaches") or drop the acronym from the row — the table remains navigation either way.

## 3. [nit | preserve] §9.4 (~line 765) — the demurrage anchor is correctly hedged

"at the whitepaper's representative operating point, a factor-6 lineage that sat idle for a year owes on the order of 2% of the value it finally moves — a mid-scale factor-3.5 lineage roughly half that". Two things to preserve: the "representative operating point" hedge (it quotes WP §10:286's own operating point without promising a parameter), and "roughly half," which is robust under both the implemented affine scaling ((3.5−1)/5 = 0.50) and a naive proportional reading (3.5/6 ≈ 0.58). Do not sharpen either phrase; the formula belongs to the spec. For the parallel auditor: the affine-vs-proportional question was already adjudicated in the v1 audit (finding 4, `cluster-tax/src/demurrage.rs`); the new sentence is consistent with that verification.

## 4. [nit | preserve] §9.1 (~line 649) — the 611M bridge belongs in prose

"At the 5-second reference pace the chain produces about 6.3 million blocks a year, so year one distributes roughly 315 million BTH". The arithmetic is now exact under the hood (50 × 6,307,200 × 1.9375 = 611,010,000 = WP §7's S₁) while reading as approximation. Keep it as prose; tabulating rewards-per-year would start restating WP §7's emission table.

## 5. [nit | preserve] §10 step 2 (~line 878) — the fee calculation is a worked instance, not the spec's derivation

"base rate (1 pico-BTH per byte, at the congestion floor) × ~5,000 bytes × factor 1 × a small multi-output penalty (4, for the two outputs) = 20,000 pico-BTH — **20 nano-BTH**". This is the fix the v1 audit asked for (F3), done the right way: a single instance whose numbers reproduce the stated figure. "(4, for the two outputs)" reads as a given constant rather than an explained rule — that is correct here; generalizing it into min(n,10)² would import WP §7's formula. Preserve as-is.

## 6. [nit | preserve] Whole body — the six v1 "What's working" moves survived the polish

Spot-verified all six: the problems-first spine ("each later section will point back at them" — §1) with back-pointers intact (§6's "harvest now, decrypt later" callback); recap-before-use ("Now recall the *shape* of the classical stealth address from Section 3" — §6); the §9.6 graveyard ("never pay out on anything a holder can manufacture for free"); the honest caveats (§4 "Botho accepts this trade deliberately", §6 "chosen, argued, and priced"); the teach-then-point blockquotes closing every section; the eleven-step capstone with its observer's-eye summary (now more honest per audit F9). The revision was genuinely local, as `changelog.md` claims. Preserve all six in any future pass.

---

Scope distribution: preserve 4, expand 2, reduce 0. No blockers, no majors; the two minors are one-clause acronym expansions (dim 6).
