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
- **Cluster demurrage (REQUIRED)**: accrue
  `(factor − 1)/(max_factor − 1) × rate × elapsed_blocks` per UTXO, charged
  when the UTXO is spent. Tags and age are already on-chain — no balance
  surveillance. Factor-1 coins pay zero. The hold/spend pincer: hold → tail
  emission dilutes you; spend → pay accrued demurrage + progressive fee. The
  only escape is genuine commerce, which decays tags — intended behavior.
  The emission-fraction sweep (below) shows demurrage is load-bearing: at
  any miner-viable emission fraction, the mechanism passes the Δgini
  criterion only with demurrage active.

## Evidence

Agent-based simulation, 100M BTH economy (this is the **simulation calibration
scale**, not a supply claim — mainnet Phase-1 supply is ~611M BTH; see
[minting.md](../minting.md)), initial Gini 0.77, each scenario
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

## Cluster Wealth Is Cumulative Lifetime Volume (Design Decision)

**Status: RATIFIED 2026-07-05 (option 2, accept-and-document).** Cycle-6 audit
item **M2** (#605) asked whether `cluster_wealth_db` — which is
*monotonically non-decreasing*, since `update_cluster_wealth_for_output`
(`botho/src/ledger/store.rs`) only ever adds and no decrement path exists —
should acquire a deterministic decay schedule (option 1) or be accepted and
documented as the intended semantic (option 2). The operator ratified
**option 2**.

### The semantic

Cluster wealth is **lifetime cumulative tagged volume**, not current holdings.
Every output ever attributed to a cluster adds to its tracked wealth and
nothing is ever subtracted. Decrement is not merely unimplemented — it is
essentially impossible under ring privacy: you cannot tell when a specific
tagged value is spent, so there is no privacy-preserving signal on which to
base a decrement. The progressive fee therefore prices a cluster by the total
value that has ever flowed through it, and a cluster's factor only ratchets
upward toward the `max_factor` ceiling.

### Empirical basis (2026-07-05, recalibrated log-domain curve)

The decision follows the empirical program ratified 2026-07-04. All runs use
the **real production `ClusterFactorCurve`** (log-domain, `w_mid = 100k BTH`)
merged as part of #626's fix (#627), on the `u128` accumulator (#628) — not
the #314 hardcoded 1.0/2.0/6.0 factors. Harness pinned at `main @ da24457`,
seed `626626626`, deterministic. Reproduction: `experiments/M2_RUNBOOK.md`.

**Cumulative (option 2) — the decision-rule primary** (criterion: ΔGini > 0.05
at long horizons):

| Run | ΔGini | criterion > 0.05 | whale factor |
|---|---|---|---|
| 10yr honest | +0.2171 | PASS | 5.646x PASS |
| 10yr gamed | +0.2242 | PASS | 5.646x PASS |
| 20yr honest | +0.5644 | PASS | 5.745x PASS |
| 20yr gamed | +0.5745 | PASS | 5.745x PASS |

Cumulative semantics on the recalibrated curve pass the decision rule with a
4–11× margin, at both horizons, honest and gamed. Redistribution *strengthens*
with horizon (Gini 0.93 → 0.37 at 20yr), and the gamed runs do marginally
better than honest — whale splitting/churning under cumulative tagging simply
re-tags the value, opening no evasion channel.

**Epoch-halving decay variants (option 1) — for comparison.** Deterministic
epoch halving (`w >>= 1` once per `half_life_years`, a pure function of the
epoch index — never per-access, per the M3 determinism lesson), with the
wash-trading evasion gate (<20%) and ring-identification gate (<50%):

| Half-life | 10yr ΔGini (honest/gamed) | evasion (<20%) | ring-ID (<50%) |
|---|---|---|---|
| 2yr | +0.1839 / +0.1897 | 0.0% PASS | 17.8% PASS |
| 5yr | +0.2096 / +0.2164 | 0.0% PASS | 13.2% PASS |
| 10yr | +0.2171 / +0.2242 | 1.5% PASS | 11.3% PASS |
| 5yr @ 20yr gamed | +0.4718 | 0.0% PASS | 13.2% PASS |

Epoch halving is *safe* — nothing like the prior art's 94–99% per-hop-decay
evasion or 78.7% ring identification (`experiments/ANALYSIS.md`) — but it is
**strictly weaker on ΔGini at every half-life**, converging up to the
cumulative result as the half-life grows. Decay buys nothing the criterion
values and costs both redistribution and new consensus surface.

### Accepted trade-offs

- **Velocity-vs-holdings mispricing.** Because wealth is lifetime volume, an
  active-commerce cluster prices on everything that has ever flowed through it,
  not what it currently holds. In the sim's population ladder the merchant
  cohort (5,000 BTH held, 2×/yr velocity) reaches a **mean factor of 3.53x at
  the 10-year horizon** from accumulated volume alone — above the harness's
  own ≥3x mispricing flag, which the run reports as `FLAG`. This 3.53x figure
  is the **sim cohort output**, reproducible via
  `target/release/cluster-tax-sim m2-cumulative --horizon-years 10`; it is *not*
  one of the ratified ΔGini/whale-factor result-table numbers above. Accepted
  with eyes open: the mispricing is real and flagged, but it is bounded by the
  log-domain curve (the merchant band stays well below the whale band at
  5.6–5.7x), and the ratified decision weighed the redistribution benefit
  (ΔGini +0.22 to +0.57) as dominating it. The live-testnet confirmation
  after the protocol-4.0.0 reset should re-measure merchant-cohort factors
  against this prediction.
- **Dormancy is no escape.** Parking coins does not lower a cluster's tracked
  wealth; the ratchet is permanent. This is intended — it removes hoarding as a
  factor-reduction strategy.
- **Residual and headroom.** #581's decoy-sourced tag-inflation residual and
  M3's saturation-ceiling analysis remain valid; both are held within the
  `u128` accumulator headroom introduced by #628 (no wrap at realistic
  lifetime volumes on the log-domain curve).

### Validity scope and remaining verification

These results are valid **only on the #626-recalibrated log-domain curve**
(`w_mid = 100k BTH`); the pre-#626 step-function curve (`w_mid` in simulator
units) is not represented. One honest caveat for the record: the harness's
wash-trading/gaming model is its implementation of the #574-era adversary, and
the live-testnet phase exists precisely to catch model-vs-reality gaps.
**Still pending:** live-testnet confirmation of the real factor distribution
after the 4.0.0 reset, measured against the 2026-07-04 6x-pinned baseline —
tracked on #605 as the final verification. This documentation records the
ratified design decision; it does not claim the on-chain confirmation is
complete.

## Parameters and Headroom

| Lever | Validated value | Headroom | Cost of turning up |
|-------|----------------|----------|--------------------|
| Payout tilt | 6:1 linear | quadratic (36:1) | payout selection leaks ~1–2 bits of coin origin |
| Emission to lottery | 25–50% of reward (see schedule) | up to full tail emission | miner security budget |
| Demurrage | 2%/yr at factor 6 (REQUIRED) | 4–6%/yr | hoarding UX; Gesell-money politics |

### Emission Schedule (implemented 2026-06-11)

Reward-split funding: miner receives `reward × (1−f)`, lottery receives
`reward × f`; total emission and tail inflation unchanged. The fraction f is
a deterministic function of height (`MonetaryPolicy::lottery_emission_bps`):

```
epoch 0 (bootstrap):  f = 0      — mining seeds the network; an early
                                   lottery would only pay miner coinbases
per halving epoch:    f += 10pp
cap:                  f = 50%    — tail emission funds at least half the
                                   mining security budget
```

The emission-fraction sweep validates this: with 2%/yr demurrage, the
mechanism passes the Δgini > 0.05 criterion at every tested f (25/50/100%);
without demurrage only f = 100% passes, which is not miner-viable.
Per-block payouts are capped at one block reward with carryover (consensus
state), which bounds seed-grinding gain below the PoW cost of a regrind.

## Protocol Changes (Status)

1. **Consensus lottery selection mode** — DONE (2026-06-11):
   `ClusterWeighted` is the default; real cluster factors (tag weights ×
   global cluster wealth, fixed-point) wired into candidate construction;
   proposer/validator candidate sets unified (fixed a latent fork bug);
   draw arithmetic converted to integer fixed-point.
2. **Emission routing** — DONE (2026-06-11): height-scheduled reward split
   (see Emission Schedule above) with persistent carryover pool and
   per-block payout cap.
3. **Demurrage** — IMPLEMENTED (2026-06-11), spend-time accrual variant:
   `charge = value × rate × (factor−1)/(max−1) × elapsed/blocks_per_year`,
   added to the mempool minimum-fee check; proceeds flow through the
   standard fee split into the lottery pool. Pure integer arithmetic
   (`cluster-tax/src/demurrage.rs`).

   *Ring-signature anchor*: the real input is hidden, so `elapsed` is the
   value-weighted centroid of the PUBLIC creation heights of all ring
   members. Properties: a large real input dominates its own ring's
   centroid (fresh small decoys barely move it); factor-1 spenders pay
   exactly zero, so old decoys can never overcharge small users; churning
   resets the clock but pays the accrual first, so total paid is invariant
   to churn frequency. Residual gaming: a whale with access to *large,
   fresh* decoy UTXOs can partially dilute its centroid — bounded by the
   availability of such UTXOs and by the tag-plausibility check
   constraining the same ring.

   *Schedule*: 0 bps during the bootstrap epoch (same rationale as emission
   routing), 200 bps (2%/yr at factor 6) from epoch 1
   (`MonetaryPolicy::demurrage_rate_bps`).

   **Known divergence from the validating simulation**: the sweep modeled
   demurrage as a continuous daily charge on balances; the implementation
   charges accrual at spend. Total paid over any holding period is
   identical *for coins that eventually move*, but a permanently parked
   coin pays nothing until spent (it suffers only emission dilution).
   The validated Δgini figures therefore need re-confirmation with
   spend-time accrual modeled — see Open Questions.
4. **Whitepaper §10** — DONE (2026-06-11): uniform-selection claims
   corrected to cluster-tilted formula and empirical results.

## Open Questions

0. **Spend-time demurrage re-validation — RESOLVED (#314, 2026-06-11)**:
   re-ran the experiment with accrual-at-spend as implemented, plus a
   permanent-parker adversary. Δgini criterion passes at both f=25% and
   f=50% in the gamed equilibrium (K/L/M scenarios: +0.056 to +0.063 vs
   baseline; ~0.003 below the daily-charge model). The parker escape is
   bounded: nominal share drifts +0.5pp/5yr but carries an unbooked
   accrued liability ≈ its lottery gains. If an "everyone parks"
   equilibrium ever emerged, the designed countermeasure is adding
   eligibility decay to the ClusterWeighted payout weight. See
   `experiments/ANALYSIS.md` § "Spend-Time Demurrage Re-Validation".

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
- 2026-07-05: Added "Cluster Wealth Is Cumulative Lifetime Volume (Design
  Decision)" — records the operator-ratified (option 2) semantic that cluster
  wealth is cumulative lifetime tagged volume with no decay, with the
  2026-07-05 empirical run matrix on the #626-recalibrated log-domain curve
  (#605).
