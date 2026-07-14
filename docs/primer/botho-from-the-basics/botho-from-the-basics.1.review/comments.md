# Line-level comments — botho-from-the-basics.1

Keyed to `botho-from-the-basics.md` (primer §, approximate body line). Severity: blocker / major / minor / nit. Scope: preserve / expand / reduce.

## 1. [major | expand] §9.4 (~line 754) — demurrage has no order of magnitude

"they owe a charge proportional to the value moved, the time held, and the cluster factor — in effect a parking fee accrued by idle concentrated wealth." Every other economic mechanism in §9 gets a number (6× fee ceiling, 20/80 split, four winners, 5% decay); demurrage — the one mechanism with a century of prior art the reader may have heard of — gets only a proportionality. Add one concrete anchor (e.g., what a factor-6 lineage pays after a year idle, as a rough percentage) without importing the formula. This is the single highest-leverage teaching gap.

## 2. [major | preserve] §9.4 (~line 744) — max-ring-factor honesty claim: refer to auditor

"Botho charges the **maximum factor among the ring members** — and recall from Section 4 that decoys must have tags similar to the real input's, so this maximum is honest and whales can't hide behind low-factor decoys." The whale-can't-hide direction is clearly right; the "maximum is honest" phrasing also implicitly promises the converse — that a factor-1 spender is not overcharged by an unlucky high-factor decoy. The capstone relies on this ("The fee was computed against the highest cluster factor in each ring, so the decoys' presence gave nothing away and saved nothing" — §10 step 5). Auditor: verify both directions against WP §5 ("Decoy Selection," "Cluster Tags and Progressive Fees"). Not a review deduction; flagged per the dim-4 judgment/audit split.

## 3. [minor | expand] §4 "An honest caveat" (~line 279) — zero-knowledge used before it is taught

"not in the whole universe as a zero-knowledge system like Zcash's shielded pool would allow." The reader meets "zero-knowledge" functionally only in §5 ("a zero-knowledge proof that the committed amount lies in a sane range… without revealing it"). Either add a one-clause gloss here, or lean on the crowd-size contrast alone ("not in the whole universe of outputs, as Zcash's shielded pool allows") and name zero-knowledge in §5.

## 4. [minor | expand] §7 (~line 479) — expand BFT at point of use

"The classical alternative — BFT voting protocols — gives *deterministic* finality." The intro mentions "the Byzantine fault tolerance literature" 450 lines earlier, but the acronym is never bound to the expansion. One parenthetical at first flow use fixes it.

## 5. [minor | expand] §8 (~line 558) — nonce / difficulty target un-taught

"Miners race to find a block-header nonce meeting the difficulty target." The stated reader knows hashes, not mining internals. A half-sentence gloss — a throwaway counter varied until the block's hash falls below a target, so winning is provably expensive — keeps §8 self-contained and reinforces the §2 randomness-beacon idea.

## 6. [minor | expand] §9.5 (~line 767) — MEV acronym

"which invites fee-market manipulation and transaction-ordering games (MEV)." The parenthetical acronym adds nothing for a reader who doesn't already know it; expand ("miner-extractable value") or drop it.

## 7. [minor | expand] §9.1 (~line 645) — 611M is asserted, not reproducible

"Summed up, this distributes roughly **611 million BTH**." The reader has per-block rewards and (from §8) a 5-second reference pace, but not the blocks-per-year bridge, so the sum cannot be checked mentally. One clause supplies it.

## 8. [nit | preserve] §4 "Choosing decoys well" (~line 270) — the forward-pointer pattern is exemplary

"cluster tags (Section 9 — a Botho-specific notion of coin ancestry)" is the right way to make an unavoidable forward reference: named, sign-posted, glossed in-line. Keep this pattern; do not restructure §4/§9 to eliminate it.

## 9. [nit | preserve] §11 map table — navigation, not duplication

The by-question table maps reader intent to whitepaper sections; it restates no normative content. Preserve — this is the companion contract working as intended.

## 10. [nit | preserve] §2 (~line 127) — paint-mixing analogy correctly scoped

"A useful mental model is mixing paint: combining colors is easy, un-mixing is not." Deliberately confined to one-wayness (the drafter's self-check flags it as lossy-but-true). Resist the temptation to stretch it to cover point addition in revision — as scoped, it cannot mislead.

---

Scope distribution: preserve 3, expand 6, reduce 0. One major is a teaching gap (#1); one major is an auditor referral (#2); no blockers.
