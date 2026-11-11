# Ring-Floor Adversarial-Dilution Sweep (#1686)

**Status:** ruled and implemented. This was the exploration record for the operator's ruling on #1686. The operator ruled for (b) `match-max`, composed per input (2026-11-10, ADR 0015), and protocol 10.0.0 implements it. The sweep has been re-run against the production kernel, which is the `node 10.0.0` column of every table. See [Re-run against the 10.0.0 node rule](#re-run-against-the-1000-node-rule).
**Date:** 2026-11-09 (exploration), 2026-11-10 (re-run against the node rule).
**Parent:** epic #1683, the design-confidence parity review.
**Operator direction:** "explore before ruling" (#1686, 2026-11-09).

## Question

The #925 downgrade charge floors a spend's declared output factor at a factor implied by the ring. Through protocol 9.x the node computed that floor with `ring_centroid_implied_factor`, which is the value-weighted mean of the ring members' cluster wealth (`botho/src/ledger/store/validation.rs`, then `ring_centroid_floored_factor`). The spender picks the decoys, so high-value background decoys dilute the charge. Protocol 10.0.0 replaces it (ADR 0015).

This sweep compares four candidate floor rules on four axes:
- dilution;
- honest over-charge on rings drawn by the production pickers;
- privacy and decoy diversity;
- compatibility with confidential amounts (#902).

| id | rule |
|---|---|
| (a) `mean` | the pre-10.0.0 node rule: value-weighted mean of member wealth (the old kernel, pinned bit for bit by a test) |
| (b) `match-mean` | value-weighted mean over members whose amount equals the input's `pseudo_output_amount` |
| (b) `match-max` | maximum over those amount-matched members |
| (c) `max` | maximum member factor over the whole ring (the rule `ring-signature-tag-propagation.md` chose; `ring_cluster_factor` computes it in f64 and has no caller) |
| (d) `p50` / `p90` / `p95` | unweighted nearest-rank quantile of member factors (same convention as `ring_elapsed_quantile`). At ring 20, `p95` is the second-highest member |
| (d) `vw-median` | value-weighted median of member wealth |
| `node 10.0.0` | the 10.0.0 node rule as the node runs it: the production kernel `bth_cluster_tax::amount_matched_ring_floor_wealth` (`cluster-tax/src/ring_floor.rs`), which is (b) `match-max` composed per input by `pseudo_output_amount` |

Every rule produces a floor wealth, which goes through the production `ClusterFactorCurve` once. The charge is the node's own `spend_demurrage_charge`, composed as `max(claimed, floor)`.

## How to reproduce

```text
cargo build --release -p bth-cluster-tax --features cli --bin cluster-tax-sim
target/release/cluster-tax-sim honest-overcharge-sweep --adversarial-dilution
```

- The exploration ran at commit `7d1295de` plus PR #1995. The re-run against the node rule (the `node 10.0.0` column) ran on the #1686 implementation branch. Each takes about 10 s on an Apple M-series release build.
- The seed is pinned (`SEED = 0xB07A_1686` in `cluster-tax/src/simulation/downgrade_overcharge_sweep/dilution.rs`). The production-route rings use `sample_route_ring_ages` with the same seed.
- The output is byte-identical across runs. The golden is `docs/research/generated/ring-floor-dilution-sweep.txt`, and CI regenerates it (`scripts/research/check_research_tables.py`). Every table below is pasted verbatim from it.
- `--ring-size N` changes the ring size; the default is the node's 20.

## Model

**Dilution grid** (the acceptance grid from #1686):
- The attacker holds 1,000 BTH in a 10M-BTH cluster and splits it into k ∈ {1, 10, 100, 1000} coins.
- Each coin is spent young (elapsed 0, so only the capitalized term binds) to a background output.
- The ring has 20 members; the 19 decoys are background coins worth D ∈ {10, 2000, 100000} BTH.
- An extra column, `D = coin`, gives every decoy the coin's own amount. This is the best response against the amount-matched rules. The attacker can create such decoys from any background funds; they cost only fees.
- The metric is the charge collected divided by the undiluted charge (the floor at the real coin's own factor).

**Honest population:**
- Rings come from the three production pickers (#1757 routes `node`, `cli` and `web`), with gamma-spend real ages on a 1.2M-block chain.
- The pickers read only ages. Each decoy's amount and cluster wealth are therefore a deterministic function of its output id:
  - a share `c` of outputs carry a wealthy tag, drawn uniformly from the settlement sweep's six classes (10k to 10M BTH) plus a 50M-BTH class;
  - wealthy outputs are worth 5k to 100k BTH;
  - background outputs are worth 10 to 2,000 BTH. These are the parent sweep's ranges.
- Real inputs are the parent sweep's agent archetypes: 85% background (retail, merchant, mixer) and 15% 10M-class whales.
- Declared outputs carry the production wallet's 5% per-hop tag decay (`DEFAULT_CLUSTER_DECAY_RATE = 50_000`).
- The clock is the oldest ring member's age, as in the node.

**Oracle.** The reference is an oracle floor at the real input's own wealth: what a rule would charge if it knew the real input. A false positive is a charge above the oracle's. "Collection" is the total charge levied divided by the oracle's total.

## Results

### 1. Dilution: charge collected / undiluted charge

Attacker: 1000 BTH in a 10000000-BTH cluster (undiluted floor 5.745×, charge 9.49% of value), split into k coins, each spent young to a background output in a ring of 20 whose 19 decoys are background coins worth D BTH. `D = coin` sets every decoy to the coin's own amount. Pass bar (#1686): ≥ 95% in every cell.

| k / coin / D | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| 1 / 1000 BTH / 10 | 99.3% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 100.0% | 100.0% |
| 1 / 1000 BTH / 2000 | 66.6% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 1 / 1000 BTH / 100000 | 11.9% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 1 / 1000 BTH / = coin | 76.2% | 76.2% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 10 / 100 BTH / 10 | 95.8% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 10 / 100 BTH / 2000 | 33.3% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 10 / 100 BTH / 100000 | 3.2% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 10 / 100 BTH / = coin | 76.2% | 76.2% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 100 / 10 BTH / 10 | 76.2% | 76.2% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 100 / 10 BTH / 2000 | 9.3% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 100 / 10 BTH / 100000 | 0.7% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 100 / 10 BTH / = coin | 76.2% | 76.2% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 1000 / 1 BTH / 10 | 43.3% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 1000 / 1 BTH / 2000 | 1.7% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 1000 / 1 BTH / 100000 | 0.0% | 100.0% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| 1000 / 1 BTH / = coin | 76.2% | 76.2% | 100.0% | 100.0% | 0.0% | 0.0% | 0.0% | 0.0% | 100.0% |
| **worst cell** | **0.0%** | **76.2%** | **100.0%** | **100.0%** | **0.0%** | **0.0%** | **0.0%** | **0.0%** | **100.0%** |
| **passes ≥ 95%** | no | no | yes | yes | no | no | no | no | yes |

Floor factor under (a) `mean` per cell (reproduces #1686's table):

| k / coin | D = 10 | D = 2000 | D = 100000 | D = coin |
|---|---|---|---|---|
| 1 / 1000 BTH | 5712 | 4162 | 1567 | 4617 |
| 10 / 100 BTH | 5544 | 2580 | 1150 | 4617 |
| 100 / 10 BTH | 4617 | 1441 | 1032 | 4617 |
| 1000 / 1 BTH | 3055 | 1079 | 1000 | 4617 |

### 2. Honest spends on production-route rings

Rings from the #1757 production pickers (`sample_route_ring_ages`, gamma-spend real ages, 1200000-block chain, one output per block), 4000 rings per row. Real inputs from the agent archetypes (85% background, 15% 10M-class whales); declared outputs carry the wallet's per-hop decay. The oracle floors at the real input's own wealth. c = share of wealthy-tagged outputs.

#### 2a. False-positive rate (charged above the oracle)

| route / population | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| node / c=0%, decay=5%, round=0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% |
| node / c=1%, decay=5%, round=0% | 15.0% | 0.0% | 0.0% | 15.0% | 0.0% | 0.1% | 1.6% | 12.8% | 0.0% |
| node / c=5%, decay=5%, round=0% | 54.3% | 0.0% | 0.0% | 54.5% | 0.0% | 5.9% | 21.1% | 47.7% | 0.0% |
| node / c=10%, decay=5%, round=0% | 76.2% | 0.0% | 0.0% | 76.7% | 0.0% | 25.6% | 50.0% | 69.7% | 0.0% |

#### 2b. Mean over-charge on false-positive spends (% of coin value)

| route / population | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| node / c=0%, decay=5%, round=0% | 0.00% | 0.00% | 0.00% | 0.00% | 0.00% | 0.00% | 0.00% | 0.00% | 0.00% |
| node / c=1%, decay=5%, round=0% | 6.09% | 0.00% | 0.00% | 6.49% | 0.00% | 4.56% | 4.86% | 6.38% | 0.00% |
| node / c=5%, decay=5%, round=0% | 6.62% | 0.00% | 0.00% | 7.06% | 0.00% | 4.74% | 5.74% | 6.32% | 0.00% |
| node / c=10%, decay=5%, round=0% | 7.18% | 0.00% | 0.00% | 7.64% | 0.00% | 5.01% | 6.18% | 6.33% | 0.00% |

#### 2c. Aggregate over-charge (% of all honest value moved)

| route / population | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| node / c=0%, decay=5%, round=0% | 0.000% | 0.000% | 0.000% | 0.000% | 0.000% | 0.000% | 0.000% | 0.000% | 0.000% |
| node / c=1%, decay=5%, round=0% | 0.370% | 0.000% | 0.000% | 0.402% | 0.000% | 0.000% | 0.038% | 0.317% | 0.000% |
| node / c=5%, decay=5%, round=0% | 1.529% | 0.000% | 0.000% | 1.655% | 0.000% | 0.109% | 0.528% | 1.226% | 0.000% |
| node / c=10%, decay=5%, round=0% | 2.359% | 0.000% | 0.000% | 2.558% | 0.000% | 0.525% | 1.303% | 1.826% | 0.000% |

#### 2d. Honest collection (total charge / oracle charge; < 100% = honest decay downgrades under-priced)

| route / population | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| node / c=0%, decay=5%, round=0% | 99.9% | 100.0% | 100.0% | 100.0% | 99.9% | 99.9% | 99.9% | 100.0% | 100.0% |
| node / c=1%, decay=5%, round=0% | 448.3% | 100.0% | 100.0% | 478.0% | 99.9% | 100.0% | 135.2% | 398.1% | 100.0% |
| node / c=5%, decay=5%, round=0% | 1539.1% | 100.0% | 100.0% | 1657.2% | 99.9% | 202.2% | 596.7% | 1253.4% | 100.0% |
| node / c=10%, decay=5%, round=0% | 2320.1% | 100.0% | 100.0% | 2507.5% | 99.9% | 594.1% | 1325.8% | 1818.8% | 100.0% |

#### 2e. Route check: false-positive rate on every production route

| route / population | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| node / c=5%, decay=5%, round=0% | 54.3% | 0.0% | 0.0% | 54.5% | 0.0% | 5.9% | 21.1% | 47.7% | 0.0% |
| cli / c=5%, decay=5%, round=0% | 55.5% | 0.0% | 0.0% | 55.8% | 0.0% | 6.7% | 20.8% | 49.4% | 0.0% |
| web / c=5%, decay=5%, round=0% | 55.0% | 0.0% | 0.0% | 55.1% | 0.0% | 6.2% | 21.3% | 48.5% | 0.0% |

#### 2f. Sensitivity (node route): false-positive rate / collection

| route / population | (a) mean | (b) match-mean | (b) match-max | (c) max | (d) p50 | (d) p90 | (d) p95 | (d) vw-median | node 10.0.0 |
|---|---|---|---|---|---|---|---|---|---|
| node / c=5%, decay=0%, round=0% | 54.3% / 1538.3% | 0.0% / 100.0% | 0.0% / 100.0% | 54.5% / 1656.1% | 0.0% / 100.0% | 5.9% / 202.4% | 21.1% / 596.7% | 47.7% / 1253.2% | 0.0% / 100.0% |
| node / c=5%, decay=5%, round=25% | 54.3% / 1333.2% | 1.1% / 182.5% | 1.1% / 192.3% | 54.5% / 1435.4% | 0.0% / 99.9% | 5.9% / 187.3% | 21.1% / 525.6% | 47.4% / 1084.3% | 1.1% / 192.3% |

### 3. Privacy and decoy diversity on the same rings

| route / population | spender | rings | mean amount matches | real is the only match | ring has a wealthier decoy | max-factor adversary ID | max-safe filter: draws rejected | max-factor adversary ID (filtered) |
|---|---|---|---|---|---|---|---|---|
| node / c=5%, decay=5%, round=0% | background | 3388 | 1.01 | 99.2% | 61.7% | 1.9% | 4.98% | 5.0% |
| node / c=5%, decay=5%, round=0% | wealthy | 612 | 1.00 | 100.0% | 14.4% | 79.8% | 0.78% | 92.7% |
| cli / c=5%, decay=5%, round=0% | background | 3415 | 1.01 | 99.0% | 63.1% | 1.8% | 5.10% | 5.0% |
| cli / c=5%, decay=5%, round=0% | wealthy | 585 | 1.00 | 100.0% | 13.0% | 81.3% | 0.75% | 93.4% |
| web / c=5%, decay=5%, round=0% | background | 3449 | 1.01 | 99.3% | 62.0% | 1.9% | 5.04% | 5.0% |
| web / c=5%, decay=5%, round=0% | wealthy | 551 | 1.00 | 100.0% | 11.8% | 81.6% | 0.64% | 92.9% |
| node / c=5%, decay=5%, round=25% | background | 3388 | 1.27 | 86.6% | 61.7% | 1.9% | 4.98% | 5.0% |
| node / c=5%, decay=5%, round=25% | wealthy | 612 | 1.04 | 96.1% | 14.4% | 79.8% | 0.78% | 92.7% |

### 4. Multi-input composition (honest two-input blend)

2000 honest spends on node-route rings (c=5%, decay=5%, round=0%): one 10M-class whale coin plus one background coin of the same amount, declared at the decayed 50/50 blend. Pooled = all 40 members in one statistic (the node before 10.0.0); per-input = one floor per ring, combined by the public `pseudo_output_amount`s. `node 10.0.0` runs the production kernel, which composes per input itself, so both of its columns are the same.

| rule | pooled FP rate | pooled over-charge (% of value) | pooled collection | per-input FP rate | per-input over-charge (% of value) | per-input collection |
|---|---|---|---|---|---|---|
| (a) mean | 28.4% | 0.045% | 124.0% | 25.6% | 0.035% | 118.7% |
| (b) match-mean | 0.0% | 0.000% | 100.0% | 0.0% | 0.000% | 100.0% |
| (b) match-max | 100.0% | 0.081% | 143.4% | 0.0% | 0.000% | 100.0% |
| (c) max | 100.0% | 0.171% | 192.0% | 56.5% | 0.110% | 159.0% |
| (d) p50 | 0.0% | 0.000% | 99.8% | 0.0% | 0.000% | 99.8% |
| (d) p90 | 0.2% | 0.000% | 100.0% | 0.1% | 0.000% | 99.9% |
| (d) p95 | 10.9% | 0.009% | 104.9% | 4.9% | 0.006% | 103.0% |
| (d) vw-median | 14.6% | 0.016% | 108.4% | 16.6% | 0.025% | 113.6% |
| node 10.0.0 | 0.0% | 0.000% | 100.0% | 0.0% | 0.000% | 100.0% |

### 5. Confidential-amounts compatibility (#902)

| rule | reads member amounts | reads `pseudo_output_amount` | computable from public data under CT |
|---|---|---|---|
| (a) mean | yes | no | no |
| (b) match-mean | yes | yes | no |
| (b) match-max | yes | yes | no |
| (c) max | no | no | yes |
| (d) p50 | no | no | yes |
| (d) p90 | no | no | yes |
| (d) p95 | no | no | yes |
| (d) vw-median | yes | no | no |
| node 10.0.0 | yes | yes | no |

## Re-run against the 10.0.0 node rule

The `node 10.0.0` column runs the production kernel the node calls (`amount_matched_ring_floor_wealth`, through `ValidationReads::ring_floored_factor_for_tx`). A unit test (`node_rule_is_per_input_match_max`) pins it bit for bit to (b) `match-max` composed per input, on single rings, unmatched pseudo amounts and two-input spends. In every table it matches the recommended rule:

- **Dilution:** 100.0% of the undiluted charge in every cell, including `D = coin`. Passes the ≥ 95% bar.
- **Honest over-charge:** 0.0% false positives at every contamination level and on every production route; 1.1% with 25% round amounts (2f). Collection is 100.0% (2d).
- **Multi-input:** 0.0% false positives and exact collection on honest two-input class blends. The kernel composes per input, so it shows the same numbers in the pooled column.
- **CT:** reads member amounts and the pseudo amount, so it is not computable under confidential amounts. ADR 0015 records the successor.

The store test `consensus_fee_floor_factor_floor_resists_value_dilution` (`botho/src/ledger/store.rs`) checks the same thing through `consensus_fee_floor` itself: #1686's three table rows (floors 4162, 1441 and 1000 under the old rule) all pay the undiluted 5.745× charge.

## Reading the results

This section and the ranking below are the exploration's analysis (2026-11-09), kept as the record of the ruling. In them, "today" and "the node" mean the pre-10.0.0 rule (a).

### Dilution

- Only the two max-type rules pass #1686's bar of 95% in every cell: (b) `match-max` and (c) `max`. Both collect 100% everywhere.
- (a) `mean` reproduces the issue's table exactly (floors 4162 / 1441 / 1000). Its worst cell collects 0%: a 1 BTH coin among 100k-BTH decoys.
- (b) `match-mean` is defeated by the `D = coin` column. Twenty equal-amount members put the real coin at 1/20 of the matched value, and the attacker keeps 23.8% of the charge in every row of that column.
- Every (d) quantile collects 0% in every cell. The attacker controls 19 of the 20 members, so any order statistic below the maximum reads a background decoy. The value-weighted median survives only the single cell where the real coin holds more than half the ring's value.
- **No quantile rule gives any protection against a spender who chooses the decoys.**

### Honest over-charge (production pickers)

**The baseline is already bad.** Today's rule (a) charges 15.0% / 54.3% / 76.2% of honest spends above the oracle at c = 1% / 5% / 10% wealthy outputs. The mean over-charge on those spends is about 6–7% of the coin's value. This is the same channel the parent sweep found (§4.5 of `demurrage-downgrade-overcharge.md`): one high-value wealthy decoy lifts a background spender's value-weighted floor.

**Rule (c) `max`** is essentially the same as today:
- false-positive rate 54.5% against 54.3% at c = 5%;
- mean over-charge 7.06% against 6.62%;
- aggregate over-charge 1.66% against 1.53% of honest value.

It fires on every ring that holds a wealthier decoy. "No worse than today" therefore holds only narrowly: (c) is slightly worse on every contaminated row of 2a–2c.

**Rules (b) `match-max` and `match-mean`** have zero false positives when amounts are continuous. A decoy almost never shares the real input's exact amount.
- If 25% of amounts snap to round values, the false-positive rate is 1.1% (2f).
- Collection is exact (100%): the oracle's decay downgrade is priced exactly.

**Rules (d) `p50` / `p90`** have few false positives, but section 1 shows they collect nothing from an attacker.

**Route independence.** The three production routes agree to within 2 points (2e). This is expected: no production picker reads tags or amounts, so ring composition is independent of the route.

**How to read 2d.** Collection above 100% means honest over-charge. The oracle's total is small, because only whales' 5%-decay downgrades and accrued demurrage are priced. Any false-positive mass therefore multiplies the ratio.

### Privacy and decoy diversity

**(b) costs no privacy today.** With public amounts, the real input is the only member with its amount in 99.0–99.3% of background rings and 100% of wealthy rings. Even with 25% round amounts, it is the only match in 86.6% (background) and 96.1% (wealthy) of rings. Rule (b) reads exactly the information that already identifies the real input (the #902 / ADR 0009 amount-matching leak), so it adds no leak. It does, however, write a dependency on public amounts into consensus.

**The floor itself leaks nothing.** Every rule is a function of public ring data plus, for (b), the public pseudo amount. The fee reveals nothing about which member is real beyond what is already public.

**(c) `max` creates pressure on decoy diversity.**
- 61.7% of background rings and 14.4% of wealthy rings hold a wealthier decoy, and the max rule over-charges each one.
- A wallet avoids that with a one-sided filter that redraws any decoy wealthier than the real input. Under the reference population, this rejects about 5% of draws for background spenders and about 0.8% for whales.
- For background spenders, the max-factor adversary's identification rate rises from 1.9% to the uniform 5.0%.
- For whales, it rises from 79.8% to 92.7%. A whale's ring max is already usually the whale.
- Native outputs publish the real input's tags in plaintext (#1756), so the cluster-fingerprint adversary already identifies about 99.7% of rings (#1757). The filter's marginal cost on today's chain is small; under any future tag-hiding design it would matter.

### Multi-input composition

The node pools every input's ring into one statistic. For an honest two-input spend that blends a 10M-class coin with an equal background coin:
- **Pooled `match-max` over-charges 100% of these spends**, and pooled `max` does too. The maximum reads the wealthy input and applies it to the whole output sum.
- Composing per input, weighted by the public `pseudo_output_amount`s, brings `match-max` to 0% false positives and exact collection.
- It cuts `max` only from 100% to 56.5%, because the decoy channel remains.
- **Any max-type rule has to be per-input.** Per-input composition by pseudo amount is itself CT-incompatible (the weights become commitments), so the CT-era cross-input composition is an open design question.

### CT compatibility (#902)

- (a), (b) and `vw-median` read member amounts, and (b) also reads `pseudo_output_amount`. None of them can be computed under CT.
- (c) and the unweighted (d) quantiles read only member factors, so they are value-free. This matches the plan in `ct-economics-gadgets.md` §5.3: replace the mean with a value-free order statistic.
- They still need each member's cluster wealth, which reduces to the same §4 core (octave-bucket proof or provenance factor).
- Of the value-free rules, only (c) `max` resists dilution.

## Ranked recommendation

Ruling is the operator's. This is a ranking, not a decision.

1. **Now (public amounts): (b) `match-max`, composed per input by `pseudo_output_amount`.**
   - The only rule that passes the dilution bar (100% in every cell) **and** has no honest over-charge: 0%, or 1.1% with 25% round amounts. Today's rule has 54% at c = 5%.
   - Prices the honest decay downgrade exactly, and costs no privacy beyond the existing public-amount leak.
   - Use the max over matched members, not the mean. `match-mean` loses 23.8% to equal-amount background decoys.
   - Compose per input. Pooled `match-max` over-charges every honest class-blend spend.
   - Its weakness is that it dies with CT, so it needs a written successor.
2. **CT-era successor: (c) `max`, per input, paired with a production one-sided decoy filter.** The filter redraws decoys wealthier than the real input; `select_decoys_cluster_aware` exists but has no production caller.
   - Dilution-proof and value-free.
   - Without the filter, its honest over-charge is about today's (54.5% vs 54.3% at c = 5%, slightly higher magnitude).
   - With the filter, the cost is about 5% of a background spender's decoy pool. The max-factor adversary's identification rate on whales rises from 80% to 93%.
   - Two prerequisites before it can ship: a value-free cross-input composition, and the §4 cluster-wealth core.
3. **(a) `mean` (status quo).** Fails the bar: the worst cell collects 0%, and an attacker keeps more than a third of the charge in 9 of the 16 cells. It also has the same honest false-positive rate as (c), so it is worse than (c) on dilution and no better on honest over-charge.
4. **(d) quantiles and `vw-median`.** Collect 0% in effectively every cell, because a ring of 20 with 19 attacker-chosen members defeats any order statistic below the maximum. The low false-positive rate of `p50`/`p90` comes from never charging anything.

This agrees with the 2026-11-01 decision comment (option 2 now, with max-over-ring as the CT-era plan). It adds four refinements: use the **max** over matched members, compose **per input**, give `max` a **wallet filter**, and treat cross-input composition under CT as an open item for the ADR.

## Limits

- The UTXO population is a synthetic mix (c, value ranges, round-amount share), not chain data. Absolute false-positive rates scale with c. The ranking does not change across c ∈ {1%, 5%, 10%}.
- The multi-input row models only an equal-value two-input blend.
- An attacker who merges the wealthy coin with **their own** large background input legitimately dilutes the blended tags. Every per-input rule prices that blend as designed. It is a property of tag inheritance, not of the ring floor, and is not measured here.
- The exploration (PR #1995) carried no consensus change. The rule was ruled on 2026-11-10 and ships in the protocol 10.0.0 batch with the store test `consensus_fee_floor_factor_floor_resists_value_dilution`.
- Pricing the honest decay downgrade exactly has a second-order effect the oracle does not show: every hop that lowers a wealthy coin's class, including the wallet's 5% tag decay, now pays its capitalized share. The bridge-import sweep shows the cumulative cost (`bridge-import-calibration.md` §0).
