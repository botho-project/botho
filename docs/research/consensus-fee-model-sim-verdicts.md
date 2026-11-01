# Economic Sims Re-Run Under the Consensus Fee Model (#1695)

**Status:** re-run record. The sims change; consensus code does not.
**Date:** 2026-10-31.
**Parent:** epic #1683, the design-confidence parity review.

## What changed

Every economic sim used to price fees its own way, and none of them matched what the node enforces:

| Sim | Legacy fee | Order of the legacy fee |
|---|---|---|
| `wash-trading`, `structuring` | linear-bps `FeeCurve` on the transfer value (30% at large wealth) | up to 0.3 × value per hop |
| `scenario-whale` (agent runner) | same linear-bps `FeeCurve` | up to 0.3 × value |
| `m2-*`, `lottery-sweep` | flat `base_fee` (0.25 or 1 BTH) × factor × outputs² | 1–24 BTH per tx |

The node's floor is `Ledger::consensus_fee_floor` (`botho/src/ledger/store/validation.rs`):

```text
floor = 1 pico/byte × size × factor × min(n_out,10)² + memo fees
      + spend_demurrage_charge(output_sum, max(claimed, ring/import floor), claimed,
                               elapsed, demurrage_rate_bps(height), blocks_per_year)
```

A default node also refuses to relay a transaction below `MIN_TX_FEE` = 1e8 pico (0.0001 BTH).

`cluster_tax::consensus_fee_model` (`cluster-tax/src/consensus_fee.rs`) is the pure half of that floor. It takes the inputs the ledger resolves from its database as parameters: the output-tag cluster wealth, the composed ring/import floor factor, and the max-quantile ring age. It has two modes, `ConsensusOnly` and `ConsensusPlusRelayMin` (`max(floor, MIN_TX_FEE)`).

A golden-vector test (`consensus_fee_model_matches_ledger_floor_golden_vectors` in `botho/src/ledger/store.rs`) recomputes seven vectors from the live `Ledger::consensus_fee_floor` and requires agreement within 1 pico: background, mid-wealth, high-wealth with accrued demurrage, memo, `n_out = 12`, young downgrade (capitalized), and bridge-import floor. Drift guards pin `RELAY_MIN_TX_FEE == MIN_TX_FEE`, the fee base, the age quantile, the mainnet demurrage schedule and the size estimate.

All five sims take `--fee-model {consensus,consensus-relay,legacy}`. The default is `consensus`. The legacy paths are kept, not deleted, and remain reachable with `--fee-model legacy`.

For a sense of scale, a background 1-in/2-out transfer (990 B) costs **3,960 pico** (4e-9 BTH) under the consensus floor, and 23,760 pico at the 6× factor. Demurrage and the relay minimum are the only fee components large enough to matter. The legacy sims overstated per-transaction cost by about 10⁴–10⁹.

## How to reproduce

Binary: `cargo build --release -p bth-cluster-tax --features cli`, then `target/release/cluster-tax-sim`. Every command below was run at commit `a0325daa` plus this change.

| Sim | Commands | Seed |
|---|---|---|
| wash-trading | `wash-trading --fee-model {legacy,consensus,consensus-relay}`; `wash-trading --height 1` (bootstrap epoch) | deterministic (closed form) |
| structuring | `structuring --fee-model {legacy,consensus,consensus-relay}`; `structuring --elapsed 6307200` | deterministic (closed form) |
| scenario-whale | `scenario-whale --fee-model {legacy,consensus,consensus-relay}` (defaults: 10M whale, 50 participants, 5000 rounds) | deterministic (agent PRNGs are fixed) |
| m2-cumulative | `m2-cumulative --horizon-years 10 [--gamed] --fee-model {…}` | 626626626 (default) |
| m2-decay | `m2-decay --horizon-years 10 --half-life-years 5 --fee-model {…}` | 626626626 (default) |
| lottery-sweep | `lottery-sweep --seed 1695 --fee-model {…}` (full 3×3 grid, 129600 blocks, 10 tx/block) | 1695 (new `--seed`; omitting it keeps the historical system-entropy behaviour) |

Units under the consensus models:
- `wash-trading` and `structuring` take `--wealth`, `--amount` and `--principal` in BTH.
- In `scenario-whale`, one sim unit is one BTH.
- `m2-*` and `lottery-sweep` use one unit = one milliBTH = 10⁹ pico.

The default height is the first epoch-1 block (6,307,200), so the node's demurrage is active. That matches the legacy sims, which all had demurrage on. Sub-unit fees are carried across transactions (`SimConsensusFee::charge`) rather than rounded to zero.

## Verdicts

### 1. `wash-trading`: **SURVIVES** (epoch ≥ 1). Bootstrap epoch: **FAILS**, tracked as #1685

Setup: 100M-BTH cluster, 5% decay per hop, `--principal` defaulting to the whole cluster. A ledger amount is a `u64` of picocredits, so the principal saturates at `u64::MAX` pico (about 18.45M BTH, the largest single transaction the node can carry); the cost percentages below are relative to that saturated principal. (Judge note on review of #1728.) The break-even horizon is a ratio and does not depend on the principal.

| Hops | Legacy break-even | Consensus: total cost | Consensus: per-tx savings | Consensus: break-even (years of demurrage saved) |
|---|---|---|---|---|
| 5 | Never | 0.012% of principal (2,214 BTH downgrade charge) | 24 pico | 5.00 yr |
| 20 | 2298 txs | 0.056% (10,330 BTH) | 111 pico | 5.00 yr |
| 50 | 61 txs | 0.514% (94,816 BTH) | 1,018 pico | 5.00 yr |

The legacy table charged 30% of the principal per hop. Its "Never" at 5–15 hops was an artifact of the flat-rate plateau, and it reported break-even after 61 txs at 50 hops.

Under the node's floor, each hop's tag decay is a class downgrade, so it pays the #925 capitalized reset charge. That charge equals the demurrage the attacker would save over `SETTLEMENT_HORIZON_BLOCKS`, so washing breaks even after exactly the 5-year horizon, by construction. Per-tx size-fee savings are a few hundred pico and never repay the wash: about 10¹⁴ txs. With `consensus-relay`, the relay minimum swamps the factor, so per-tx savings are 0. The verdict is **SURVIVES**, as designed.

Bootstrap epoch (`--height 1`): the demurrage rate is 0, so the downgrade charge is 0. Fifty hops cost 255,242 pico (2.6e-7 BTH) and shed the factor from 5.949× to 5.692× before demurrage switches on. The verdict there is **FAILS**. This is the epoch-0 holiday already tracked in **#1685**; the numbers are posted there. No new issue.

### 2. `structuring`: **SURVIVES** (unchanged)

Setup: 1M BTH moved from a 50M-BTH cluster.

| Splits | Legacy savings | Consensus: N separate txs | Consensus: 1 tx with N+1 outputs | Consensus+relay: N txs |
|---|---|---|---|---|
| 2 | 0% | −100% | −152% | −100% |
| 10 | 0% | −900% | −5127% | −900% |
| 100 | 0% | −9900% | −32401% | −9900% |

Splitting always costs more. N transactions pay N size fees, or N relay minimums. One N-output transaction pays the `min(N,10)²` penalty. Demurrage is value-proportional, so it is the same in total whichever way the transfer is split. With `--elapsed 6307200`, the single transfer pays 19,716 BTH of accrued demurrage, and the splits pay that plus more size fees.

### 3. `scenario-whale`: **SURVIVES** (legacy said FAILS)

| Strategy | Legacy whale-fee reduction | Consensus whale fees (pico) | Consensus reduction |
|---|---|---|---|
| Passive | 0% | 60,202,774 | 0% |
| Wash Trading | **94.7%** | 7.95e15 | −1.3e10 % |
| Structuring | 4.4% | 170,248,128 | −183% |
| Aggressive | **99.4%** | 9.23e15 | −1.5e10 % |

Under the legacy curve, wash trading and the aggressive strategy cut the whale's fees by 95–99%. Under the node's floor, every wash hop is a priced downgrade, about 7,900 BTH in total, against a passive whale's 6e-5 BTH of size fees. Structuring pays more size fees. No strategy reduces fees, so the verdict is **SURVIVES**. The `consensus-relay` run gives the same ordering: Passive 2.6e11 pico, Wash 7.9e15, Structuring 7.5e11.

Caveat: the agent runner does not model coin ages or rings. Ordinary transfers are priced as in-class and young (elapsed 0, so no demurrage), and only the wash hops' downgrades carry demurrage. Final Gini is unchanged by the fee model (0.9111, 0.9054, 0.9081), because fees are negligible relative to balances in both models.

### 4. `m2-cumulative` / `m2-decay`: **FAILS** (legacy said PASS). Flipped, filed as **#1727**

| Run (10 yr, seed 626626626) | Legacy dGini | Consensus dGini | Consensus+relay dGini |
|---|---|---|---|
| `m2-cumulative` honest | +0.2172 **PASS** | +0.0289 **FAIL** | +0.0289 **FAIL** |
| `m2-cumulative --gamed` | +0.2243 **PASS** | +0.0298 **FAIL** | +0.0298 **FAIL** |
| `m2-decay --half-life-years 5` | +0.2096 **PASS** | +0.0290 **FAIL** | +0.0290 **FAIL** |

The other M2 gates are unchanged across fee models:
- The whale mean factor stays above 5× (PASS).
- The merchant mispricing flag fires in the cumulative runs (3.532×).
- Decay-run wash evasion is 0.0% (PASS).
- The ring identification rate is 13.2% (PASS).

These metrics are computed from the factor curve, not from fee flows.

The Δgini > 0.05 criterion passed only because of the flat 0.25 BTH × factor × 4 per-tx fee, which the node never charged. Demurrage (200 bps/yr at max factor, charged at spend) is the same in both runs. With node-accurate fees, Δgini drops about 7×, below the gate. Per #1695's scoping note, the remedy is not in this change. It is filed as **#1727**, and #1696's production-scale M2 re-run should use `--fee-model consensus`.

**Update (#1696):** the production-scale re-run (`m2-production`, 6.3M blocks/yr, mainnet emission ramp, uniform V2 lottery, coinbase-lineage clusters) also fails, by a wider margin: the mechanism's own ΔGini is between −0.009 and +0.020 (3-seed means), and 100% of top-decile value prices below 1.5x at 10 and 20 years. See [`m2-production-verdicts.md`](m2-production-verdicts.md).

### 5. `lottery-sweep`: **FAILS** (unchanged; legacy also FAILS)

Recommended config: penalty 1.0, decay 0.03, threshold 1000 BTH, seed 1695.

| Criterion | Legacy | Consensus | Consensus+relay |
|---|---|---|---|
| Gini reduction vs baseline > 0.05 | FAIL (−0.0000) | FAIL (−0.0000) | FAIL (−0.0000) |
| Parking ROI < 1 | PASS (−11.29) | PASS (−1.2e7) | PASS (−9.2e5) |
| Parking advantage < 2× | PASS (0.99×) | PASS (0.98×) | PASS (0.98×) |
| Splitting advantage < 2× | PASS (1.00×) | PASS (1.00×) | PASS (1.00×) |

The headline Gini criterion already failed under the legacy flat fee over this 30-day horizon. With consensus fees, the fee-funded pool is close to zero, so the result does not change. The anti-gaming criteria still pass. The parking ROI is a large negative number because the parker's extra winnings are slightly negative and the node prices a 1-in/100-out split at only 1,275,000 pico × factor (12,750 B × 10²). The penalty column has no effect under the consensus models, because the node has no split penalty beyond `min(n,10)²`. Not a flip, so no new issue. The unseeded full-grid runs gave the same verdicts.

## Summary

| Sim | Legacy verdict | Consensus verdict | Action |
|---|---|---|---|
| wash-trading (epoch ≥ 1) | mixed (Never → 61 txs) | **SURVIVES** (break-even = 5-yr horizon, by design) | none |
| wash-trading (epoch 0) | n/a | **FAILS** (free downgrade) | #1685 (existing) |
| structuring | SURVIVES | **SURVIVES** | none |
| scenario-whale | FAILS (wash 94.7%, aggressive 99.4%) | **SURVIVES** | none |
| m2-cumulative / m2-decay | PASS | **FAILS** (Δgini ≈ 0.029) | **#1727** (new) |
| lottery-sweep | FAILS (Gini) | **FAILS** (Gini); anti-gaming PASS | none (unchanged) |

## Known limits of these re-runs

- `scenario-whale` and the runner do not model rings or coin ages. Ordinary transfers carry no demurrage there. Rings and ages belong to #1683's other children (#1686, #1700).
- The lottery paths switched are the immediate-distribution paths and churn, which `m2-*` and `lottery-sweep` use. The pooled `simulate_transaction` / `simulate_transaction_uniform` paths, used by older lottery experiments, still take a caller-supplied flat fee.
- Wash trading uses the same per-hop decay approximation as the legacy analysis. Whether tags can instead be dropped outright in one transaction is #1687.
