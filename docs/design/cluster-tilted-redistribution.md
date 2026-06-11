# Cluster-Tilted Redistribution: Validated Design

## Status

**Proposed** — Mechanism empirically validated by simulation (2026-06-11);
protocol changes not yet implemented. Supersedes the redistribution goals of
[Asymmetric UTXO Fees](asymmetric-utxo-fees.md) (whose attack-resistance
components remain valid, but whose value-weighted payout produces zero
redistribution — see `experiments/ANALYSIS.md`).

## Summary

Botho aims for *structural* Gini reduction: inequality should fall as a
consequence of protocol mechanics, not policy. Prior designs failed one of
two ways:

1. **Burn-based progressive fees**: progressive intake, but burning does not
   redistribute and transaction taxes cannot touch idle wealth (Experiment 5:
   zero Gini effect).
2. **Per-UTXO lottery payouts** (uniform, floor-based, or Hybrid α > 0):
   redistribute beautifully in honest simulations and *invert* under gaming —
   a strategic whale that splits and churns UTXOs captures the payout stream
   (5-year gamed simulation: whale grows from 5% to 24% of supply, Gini
   rises).

This design resolves the tension with one principle:

> **Every progressive term — intake and payout — must key off cluster
> provenance, the only split-invariant signal in an anonymous-value system.
> Nothing may be weighted by UTXO count, account count, or any other
> structure the holder controls for free.**

## The Mechanism

Three components, all anchored to cluster tags:

### 1. Progressive intake: cluster-factor fees (implemented)

Fees scale 1x–6x with the *global* wealth of the sender's tag clusters
(enforced against the ledger's `cluster_wealth_db` as of 2026-06-11 —
splitting funds does not reduce the rate).

### 2. Progressive payout: cluster-tilted lottery (one-line consensus change)

Lottery winner selection weight:

```
weight = value × (max_factor − cluster_factor + 1) / max_factor
```

- **Value-weighted** → splitting a position never increases total weight
  (Sybil/split-invariant by construction)
- **Tilted 6:1 toward low-factor coins** → redistributes from wealthy
  clusters to commerce/background coins
- Already implemented as `SelectionMode::ClusterWeighted` in
  `cluster-tax/src/lottery.rs`; the consensus lottery
  (`botho/src/consensus/lottery.rs`) currently defaults to
  `Hybrid { alpha: 0.3 }`, whose per-UTXO α-term gives a 1000-way split
  ~300x the lottery weight — this default must change.

### 3. Stock-level flow: tail emission routing + optional cluster demurrage

Transaction fees are a consumption tax and cannot move Gini at realistic
velocities; idle wealth must be touched.

- **Emission routing**: direct a fraction of tail emission into the lottery
  pool. Inflation is a perfectly Sybil-resistant, privacy-preserving,
  unavoidable flat wealth levy; paid out cluster-tilted, it becomes a
  progressive one.
- **Cluster demurrage (optional, larger effect)**: accrue
  `(factor − 1)/(max_factor − 1) × rate × elapsed_blocks` per UTXO, charged
  when the UTXO is spent. Tags and age are already on-chain — no balance
  surveillance. Factor-1 coins pay zero. The hold/spend pincer: hold → tail
  emission dilutes you; spend → pay accrued demurrage + progressive fee. The
  only escape is genuine commerce, which decays tags — intended behavior.

## Evidence

Agent-based simulation, 100M BTH economy, initial Gini 0.77, each scenario
run honest AND gamed (strategic whale splits 5M BTH into 1,000 UTXOs and
churns weekly to defeat eligibility decay). Five-year horizon, conservative
parameters (2.5%/yr emission, 2%/yr max demurrage, 6:1 tilt). Full matrix in
`experiments/ANALYSIS.md` § "Structural Gini Reduction Experiment"; results
in `experiments/results/gini_experiment_{1yr,5yr}.txt`. Reproduce with:

```bash
cargo build -p bth-cluster-tax --features cli --bin cluster-tax-sim --release
./target/release/cluster-tax-sim lottery-experiment --blocks 7884000
```

| Configuration | Δgini vs baseline (5yr) | Gamed equilibrium |
|---------------|------------------------|--------------------|
| Uniform payout + emission | +0.177 honest | **−0.026; whale 5%→24%, +21M BTH** |
| Demurrage only | +0.009 | robust (attacker −548K BTH) |
| Cluster-tilted payout + emission | +0.054 | **+0.055 — gaming helps the pool, costs attacker 948K BTH** |
| **+ demurrage (full mechanism)** | **+0.078** | **robust; passes Δgini > 0.05 criterion** |

Key property: because churn/split fees feed the lottery pool, attacking the
mechanism *funds* it. The gamed run shows marginally more redistribution
than the honest run.

## Residual Attack Surface

Shedding cluster attribution (wash trading to lower one's factor) is the
single remaining lever, identical to the attack on progressive fees — and is
rate-bounded by the existing AND-based decay (epoch-capped) and
entropy-weighted decay (Phase 2) mechanisms. The redistribution design and
the decay design defend the same invariant: **cluster attribution must be
expensive to shed.** Hardening one hardens both.

A second-order attack — acquiring low-factor coins from others — is
self-correcting under global cluster wealth tracking: accumulating a large
share of a cluster's coins raises that cluster's wealth and hence its factor.

## Parameters and Headroom

| Lever | Validated value | Headroom | Cost of turning up |
|-------|----------------|----------|--------------------|
| Payout tilt | 6:1 linear | quadratic (36:1) | payout selection leaks ~1–2 bits of coin origin |
| Emission to lottery | 2.5%/yr of supply | up to full tail emission | miner security budget |
| Demurrage | 2%/yr at factor 6 | 4–6%/yr | hoarding UX; Gesell-money politics |

## Protocol Changes Required

1. **Consensus lottery selection mode**: `LotteryDrawConfig::default()`
   `Hybrid { alpha: 0.3 }` → `ClusterWeighted`
   (`cluster-tax/src/lottery.rs:51`). Consensus-critical: all validators
   must switch at a coordinated height. The Hybrid α-term is a known
   ~3.84x–300x splitting subsidy and must not ship to mainnet.
2. **Emission routing**: `draw_lottery_winners()` currently pools fees only;
   add a protocol-defined fraction of the block reward to `pool_amount`.
   Changes miner revenue → coordinate with security-budget analysis.
3. **Demurrage** (separate proposal recommended): per-UTXO accrual checked at
   spend, `charge = value × rate × (factor−1)/(max−1) × elapsed/blocks_per_year`,
   added to the minimum-fee check in mempool/consensus validation. Use
   fixed-point arithmetic (see the f64 consensus-fee finding in
   `audits/2026-01-03-cycle5.md`).
4. **Whitepaper §10**: the current text describes uniform random-UTXO
   selection and claims "users with more UTXOs receive more lottery income"
   as an anti-hoarding feature — that is precisely the gameable design.
   Must be corrected (see whitepaper update of 2026-06-11).

## Open Questions

1. **Tilt curve shape**: linear (max−f+1)/max vs quadratic — quantify
   privacy-bits-leaked vs Gini-per-year on the same harness.
2. **Emission fraction**: how much of tail emission can route to the lottery
   before miner participation degrades? Needs security-budget model.
3. **Demurrage accrual vs decay interaction**: tags decay while coins idle
   (age-based Phase 1) — demurrage rate should be defined on the
   *current* factor at spend time, which the decayed tags naturally provide.
   Verify the combined system in a co-simulation (redistribution + decay +
   wash trading in one model).
4. **Payout privacy**: winners are visible on-chain (lottery outputs).
   Cluster-tilted selection statistically reveals winner factor
   distribution. Quantify with the privacy harness.

## References

- `experiments/ANALYSIS.md` — sweep failure + experiment results
- [Asymmetric UTXO Fees](asymmetric-utxo-fees.md) — superseded redistribution
  design (attack analyses remain valid)
- [Lottery Redistribution](lottery-redistribution.md) — selection-mode
  trade-off analysis (Hybrid α gaming ratios)
- [Cluster Tag Decay](cluster-tag-decay.md), [Entropy-Weighted Decay](entropy-weighted-decay.md)
  — the tag-shedding bound this design depends on

## Changelog

- 2026-06-11: Initial proposal from validated experiment results
