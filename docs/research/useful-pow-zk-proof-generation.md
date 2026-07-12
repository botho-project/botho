# Useful Proof-of-Work: ZK-Proof-Generation as the Only Direction That Fits Botho

## Research Summary

This is a **parked research record**, not scheduled work. It preserves the durable
version of the "useful proof-of-work" instinct so the *good* form of the idea is not
lost and the *bad* forms are not re-litigated.

Bottom line: the naive useful-PoW paths (prime-finding, factoring, generic
Proof-of-Useful-Work) are **rejected** for Botho on both first-principles and
Botho-specific grounds. The one direction worth keeping alive is **ZK-proof
generation**, because it has the right verification asymmetry (cheap to verify,
expensive to produce) *and* real demand — including Botho's own Phase-2 ZK roadmap.
This is long-horizon, research-grade work: **RandomX remains the ratified, live PoW**
([#441](https://github.com/botho-project/botho/issues/441)), and this idea is
revisited only under the conditions stated below.

---

## 1. Context

Origin: this idea was filed alongside the RandomX ratification
([#441](https://github.com/botho-project/botho/issues/441)) on 2026-07-07 — not as
scheduled feature work, but as a durable record.

The maintainer's instinct: mining "work" is hard to make externally valuable. A useful
PoW (find Mersenne primes, factor semiprimes, etc.) would be *nice*, but ultimately PoW
value is **game-theoretic** — the competition itself creates the narrative and the
security, not the byproduct. A 2026 literature review confirmed this and ruled out the
naive useful-PoW paths for Botho specifically.

The record below captures that review: why prime-finding / factoring / generic PoUW is
rejected, the single direction (ZK-proof generation) that survives scrutiny, and the
concrete conditions under which it becomes worth revisiting.

---

## 2. Why Prime-Finding / Factoring / Generic PoUW is REJECTED for Botho

### 2.1 The fundamental PoUW dilemma

Usefulness and security are in **direct tension**. This is not an engineering detail to
be optimized away — it is structural:

- **Useful problems admit shortcuts.** Real-world-valuable problems tend to have
  specialized hardware, algorithmic shortcuts, and non-uniform difficulty. A PoW needs
  the opposite: uniform, unpredictable difficulty with no shortcuts.
- **Verification is either too expensive or too cheap.** If verifying the useful result
  is expensive, it kills mining/validation efficiency (every node re-does heavy work).
  If it is cheap, the "work" was not actually hard — the security guarantee collapses.
- **The "easy-instances" attack.** When difficulty is non-uniform, an adversary steers
  toward cheap sub-instances and collects disproportionate reward for below-average
  work, breaking the reward-for-work invariant.

Empirically, deployed useful-PoW systems exhibit a large **"usefulness gap"** between
intended and actual utility (see the empirical measurement in the References).

### 2.2 Prior art produced ~no real-world value

- **Primecoin** deliberately **avoided Mersenne primes**. It uses Cunningham chains and
  bi-twin chains — chosen for *PoW verifiability*, not for scientific value. Its output
  has essentially no research value: a narrative fig leaf.
- **FACT0RN** (factoring random semiprimes) factors numbers that are useful to no one —
  random semiprimes are not a problem anyone needs solved.
- **Ofelimos** (IOG's provably-secure combinatorial-optimization PoUW) is a serious
  academic construction, but was never productionized.

The pattern is consistent: either the "useful" output is not actually useful, or the
construction never ships.

### 2.3 The Botho-specific killer: CPU-egalitarianism

Number-theoretic PoW (prime search, factoring) is **more ASIC/GPU-friendly than
RandomX**, not less. Adopting it would directly sabotage the CPU-egalitarian goal that
Botho's managed-node product depends on — the whole "a modest village node mines for
you" story requires that commodity CPUs stay competitive.

RandomX was ratified precisely for CPU-egalitarianism (see
[#441](https://github.com/botho-project/botho/issues/441) — the RandomX-ratification
mining-economics analysis). RandomX is not aspirational: it is **wired into the node**
today at `botho/src/pow.rs`, with the prover vendored under `vendor/randomx-rs/`.
Swapping in a number-theoretic PoW would be the *worst* trade for Botho's economics.

---

## 3. The One Direction Worth Keeping Alive: ZK-Proof Generation

The single useful-compute form that survives the dilemma in §2 is **ZK proof
generation**. It is also the direction endorsed by the broader retrospective on
"useful hard problems" for crypto (cf. Vitalik Buterin's writing on the topic).

### 3.1 Why it survives the dilemma

- **Right verification asymmetry.** ZK proofs are *cheap to verify, expensive to
  produce* — exactly the asymmetry a PoW needs. Verification cost does not blow up node
  budgets, yet producing the proof is genuinely hard.
- **Real, non-pretend demand.** The demand for proof generation is the crypto
  ecosystem's *own* recurring need (rollups, privacy systems, bridges), not a
  manufactured "scientific" market that nobody actually pays for. The work product has a
  real consumer.

### 3.2 Why it fits Botho specifically

Botho is a privacy chain (CLSAG ring signatures, Bulletproofs range proofs) with a
**Phase-2 ZK roadmap already noted** for cluster-tag inheritance proofs. This surfaced
during the [#581](https://github.com/botho-project/botho/issues/581) tightening: the
residual decoy-sourced inflation gap in cluster-tag inheritance closes only with a
consensus-visible ZK binding to the real input.

If mining ever generated ZK proofs that **the network itself consumes** — e.g. batched
cluster-tag-inheritance proofs or range proofs — the mining work product would be
*genuinely* useful, because Botho actually needs those proofs. That closes the loop the
naive useful-PoW paths never could: the useful output is something the protocol already
has to pay for.

---

## 4. Status and Revisit Conditions

- **Long-horizon / research-grade.** This is a multi-year idea, not a near-term PoW
  swap. **RandomX stays the PoW** ([#441](https://github.com/botho-project/botho/issues/441)),
  and it is live in code (`botho/src/pow.rs`). Nothing here is scheduled work.
- **Revisit only if BOTH hold:**
  - **(a)** Botho's ZK roadmap (Phase-2 cluster tags,
    [#581](https://github.com/botho-project/botho/issues/581)) matures to where proof
    generation is a **real recurring network cost**, AND
  - **(b)** a proof system with a **mining-compatible producer/verifier asymmetry**
    becomes available.
  Only when both are true does "useful PoW = generate the proofs we already need"
  become concrete.
- **Until then:** the narrative value of RandomX + lottery redistribution + the
  "village node mines for you" story is sufficient — and cleaner than a useful-work
  veneer that would not survive scrutiny.

---

## 5. Cross-References

- [#441](https://github.com/botho-project/botho/issues/441) — RandomX ratification /
  Botho product-architecture epic. This research was filed alongside it; the
  RandomX-as-CPU-egalitarian-PoW decision (live at `botho/src/pow.rs`) is the anchor
  that makes number-theoretic PoW a non-starter for Botho.
- [#581](https://github.com/botho-project/botho/issues/581) *(closed)* — "Tighten
  cluster-tag inheritance bound: ring-member-sum input set permits decoy-sourced
  inflation." The Phase-2 ZK tag-inheritance context that would be the concrete future
  consumer of network-generated ZK proofs.

---

## References

1. **The PoUW dilemma (SoK).** "SoK: Proof-of-Useful-Work." IACR ePrint 2025/1814.
   https://eprint.iacr.org/2025/1814
2. **PoUW security/usefulness tension.** "Proof-of-Useful-Work" analysis, arXiv:2209.03865.
   https://arxiv.org/abs/2209.03865
3. **Empirical "usefulness gap."** Measurement of the gap between intended and actual
   utility in deployed useful-PoW, arXiv:2606.04819.
   https://arxiv.org/abs/2606.04819
