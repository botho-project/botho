# M2 at Production Parameters (#1696)

**Status:** re-run record. The sims change; consensus code does not.
**Date:** 2026-10-31.
**Parent:** epic #1683, the design-confidence parity review. Follows #1695 / `consensus-fee-model-sim-verdicts.md`.
**Decision owner:** #1727. This document supplies the numbers. Whether to add a redistribution source or restate the criterion is decided there, not here.

## Verdict

**The M2 (#605) premise is broken at production parameters.** The mechanism does not reduce Gini by anything close to the 0.05 bar, and none of the top decile's value is priced above 1.5x after year 10.

| Metric (10 yr / 20 yr) | Wealth-proportional mining | Egalitarian mining | Bar |
|---|---|---|---|
| ΔGini caused by the mechanism (on − off counterfactual), mean of 3 seeds | +0.0005 to +0.0011 / −0.0000 to +0.0005 | +0.0030 to +0.0196 / −0.0087 to −0.0002 | > 0.05 |
| Margin vs the bar | short by ≈ 0.049 | short by ≥ 0.030 (best seed-mean); single seeds range −0.045 to +0.051 | |
| Legacy ΔGini (gini0 − giniF), seed 1696 | −0.006 to −0.008 (FAIL) | +0.56 to +0.66 (PASS, but the counterfactual with the mechanism off gives +0.54 to +0.61) | > 0.05 |
| Share of top-decile value priced below 1.5x | **100% / 100%** | **100% / 100%** | should be small |

Ranges span the honest and optimal-decoy whale. The legacy definition passes under egalitarian mining only because equal block rewards across 140 agents flatten a 23M-BTH genesis ladder under 690M BTH of emission (10 yr). The mechanism-off run shows the same drop, so it is not the mechanism's doing.

For comparison, the legacy harness (`m2-cumulative`, honest, seed 626626626) at its own parameters gives:

| Legacy harness | 10 yr ΔGini | 20 yr ΔGini |
|---|---|---|
| `--fee-model legacy` (flat 0.25 BTH) | +0.2172 PASS | +0.5650 PASS |
| `--fee-model consensus` | +0.0289 FAIL (#1727) | +0.0748 PASS |

The legacy harness still passes at 20 years under consensus fees. It gets there through its cluster-weighted draw, its fixed per-block pool, the absence of mining, and the demurrage it charges on a per-agent cluster that never fades. None of those is how the node works, so that pass does not carry over.

## What `m2-production` models

The legacy `m2-cumulative` / `m2-decay` harness (`cluster-tax/src/simulation/m2.rs`) is unchanged, so its numbers stay reproducible. The new variant is `cluster-tax/src/simulation/m2_production.rs`, run as `cluster-tax-sim m2-production`.

| Feature | Node source | `m2-production` | Legacy M2 |
|---|---|---|---|
| Block rate | `mainnet_policy`: 5 s, 6,307,200 blocks/yr | 6,307,200 blocks/yr; demurrage on the same clock | 4,320 blocks/day (1,576,800/yr) |
| Emission | 50 BTH, 1-yr halvings × 5, then a 2% tail recomputed from `total_mined` (`calculate_block_reward`) | same, via `cluster_tax::mainnet_monetary_policy` (drift-guarded against `botho::monetary::mainnet_policy`) | none |
| Lottery emission share | `lottery_emission_bps`: 0 in epoch 0, +10%/epoch, cap 50% | same | fixed 1.6 units/block |
| Epoch-0 demurrage holiday | `demurrage_rate_bps` = 0 below `halving_interval` | same (shared `ConsensusFeeParams`) | demurrage always on |
| Lottery candidates | outputs aged 720..=10,000 blocks, value ≥ 1e6 pico, capped at 10k (`validation.rs::lottery_candidates`) | same; a uniform 10k subset when over the cap | all UTXOs |
| Draw | uniform, 4 winners (`LotteryDrawConfig::default`) | same | `ClusterWeighted` |
| Payout | `min(carry + share + 80% fees, min(ρ·0.25 BTH, reward))`, remainder carried (`reward_cap`, `compute_pool_accounting`) | same, block by block | in-place balance add |
| Payout output | a new output copying the winner's tags, credited to W (`writer.rs`) | same | none |
| Coinbase clusters | fresh cluster per coinbase, W = payout ≤ 50 BTH (`block.rs` `to_tx_output`) | same, one cluster per block | none |
| Tags on spend | value-weighted merge, prune < 0.1%, truncate to 16 (`ClusterTagVector`); stock wallet decays 5%/hop | same | one cluster per agent |
| W | `update_cluster_wealth_for_output`: += amount × weight for every output; never decreases | same | per-agent cumulative counter |
| Fees | `consensus_fee_floor` | shared `consensus_fee_model` (#1695) | flat 0.25 BTH (legacy) or consensus |
| Adversary | — | a mining whale that picks floor-minimising decoys (#1686) and the cheaper of an in-class claim or a background claim (#1687) | split + churn |
| Counterfactual | — | the same seed with the mechanism off: no lottery share, fees burned, demurrage 0 | none |

Population: the legacy M2 ladder (80 × 50 BTH, 30 × 10k, 20 × 5k merchants at 2×/yr velocity, 9 × 2M whales) plus the strategic 5M-BTH whale, as a genesis endowment, each agent in its own cluster with W = holdings. Two hashpower scenarios: proportional to the endowment (`wealth`; the 9 whales hold 77% and the strategic whale 21%), or equal per agent (`egalitarian`).

## Results

### Main runs (seed 1696, consensus fee model)

| Mining | Whale | Yr | gini0 | giniF on | giniF off | ΔGini legacy (gini0 − on) | ΔGini mechanism (off − on) | Top decile < 1.5x |
|---|---|---|---|---|---|---|---|---|
| wealth | honest | 10 | 0.9302 | 0.9366 | 0.9369 | −0.0064 | +0.0003 | 100% |
| wealth | honest | 20 | 0.9302 | 0.9375 | 0.9386 | −0.0074 | +0.0010 | 100% |
| wealth | optimal-decoy | 10 | 0.9302 | 0.9363 | 0.9369 | −0.0061 | +0.0006 | 100% |
| wealth | optimal-decoy | 20 | 0.9302 | 0.9383 | 0.9386 | −0.0081 | +0.0003 | 100% |
| egalitarian | honest | 10 | 0.9302 | 0.2962 | 0.3234 | +0.6340 | +0.0273 | 100% |
| egalitarian | honest | 20 | 0.9302 | 0.3579 | 0.3890 | +0.5723 | +0.0311 | 100% |
| egalitarian | optimal-decoy | 10 | 0.9302 | 0.2726 | 0.3234 | +0.6576 | +0.0508 | 100% |
| egalitarian | optimal-decoy | 20 | 0.9302 | 0.3735 | 0.3890 | +0.5567 | +0.0155 | 100% |

`--fee-model consensus-relay` gives identical numbers under wealth mining. Under egalitarian mining it differs only within the seed noise below (mechanism ΔGini +0.0340 / +0.0058 honest, +0.0301 / +0.0148 optimal-decoy). Size fees (fees other than demurrage) total about 1e-3 to 1e-2 BTH over 20 years, so the relay minimum never matters.

### Seed spread (ΔGini mechanism, off − on)

| Mining | Whale | Yr | seed 1696 | seed 2 | seed 3 | mean |
|---|---|---|---|---|---|---|
| wealth | honest | 10 | +0.0003 | +0.0005 | +0.0007 | +0.0005 |
| wealth | honest | 20 | +0.0010 | −0.0022 | +0.0011 | −0.0000 |
| wealth | optimal-decoy | 10 | +0.0006 | +0.0009 | +0.0017 | +0.0011 |
| wealth | optimal-decoy | 20 | +0.0003 | −0.0002 | +0.0015 | +0.0005 |
| egalitarian | honest | 10 | +0.0273 | +0.0041 | −0.0224 | +0.0030 |
| egalitarian | honest | 20 | +0.0311 | −0.0054 | −0.0264 | −0.0002 |
| egalitarian | optimal-decoy | 10 | +0.0508 | +0.0089 | −0.0009 | +0.0196 |
| egalitarian | optimal-decoy | 20 | +0.0155 | +0.0032 | −0.0449 | −0.0087 |

The egalitarian spread is payment noise, not mechanism. Payments are lumpy (a whale pays 1/60 of its position at a time), and the on and off runs drift apart once their balances differ, so their Gini paths diverge by a few hundredths in either direction. The wealth-mining spread is small because whales dominate both runs.

### Why it fails

**1. The uniform lottery pays in proportion to output creation, which is mostly mining.** At 6.3M blocks/yr every block adds one coinbase and four payout outputs to the window, against about 13,000 transactions a year from the whole population. Payouts copy their winner's owner, so the window's ownership converges to each agent's share of coinbases plus transaction outputs, which is about its hashpower share. Seed 1696, 10 yr:

| Cohort | Hashpower (wealth) | Lottery share (wealth) | Hashpower (egalitarian) | Lottery share (egalitarian) |
|---|---|---|---|---|
| small (80) | 0.02% | 0.08% | 57.1% | 57.1% |
| middle (30) | 1.28% | 1.29% | 21.4% | 21.4% |
| merchant (20) | 0.43% | 0.55% | 14.3% | 14.4% |
| whales (9) | 76.9% | 76.7% | 6.4% | 6.4% |
| strategic (1) | 21.4% | 21.3% | 0.7% | 0.7% |

A lottery whose payout tracks the block-reward distribution cannot undo it. Over 10 years the lottery pays 1.4–1.9e8 BTH (20–27% of emission), and it goes almost exactly where the coinbases go.

**2. The cluster factor cannot see mined wealth.** Each coinbase starts a fresh cluster with W ≤ 50 BTH, a factor of about 1.0x. When a holder consolidates, every coinbase tag weighs well under 0.1% of the merged output and is pruned. Endowment tags fade the same way: the stock wallet's 5% decay per hop, plus dilution by mined coins. The whole top decile prices below 1.5x by year 6 (wealth) or year 10 (egalitarian), and by year 10 every cohort's mean priced factor is 1.000x in every scenario. Demurrage stops at that point and is never charged again:

| Run (seed 1696) | Top decile < 1.5x at yr 1 / 3 / 5 / 8 / 10 | Demurrage collected yr 0–10 | after yr 10 |
|---|---|---|---|
| wealth, honest | 0% / 0.1% / 59% / 100% / 100% | 6.0e7 BTH | 0 |
| egalitarian, honest | 5% / 27% / 54% / 93% / 100% | 1.17e8 BTH | 0 |

**3. Demurrage lands on the wrong people under egalitarian mining.** Small holders spend weekly. Each spend re-credits their coinbase clusters' W (W += amount × weight), and payments from whales carry whale tags, so small holders' positions price at about 1.7–2.8x during epoch 1. That is the same velocity-vs-holdings mispricing the legacy merchant flag reported. Over 10 years the 80 small holders pay 5.9e7 BTH of the 1.17e8 BTH collected. The 9 whales pay 8.7e6 BTH.

**4. The optimal-decoy whale does no better than the stock wallet,** because nothing is left to evade. With background claims available (#1687), the whale pays the #925 downgrade charge from the decoy-diluted floor once per spend. In this sim's thin decoy universe (#1686), that costs more than the stock wallet's gradual decay: 1.9e7 vs 1.1e7 BTH in the wealth scenario. Both end at 1.000x.

**5. Size fees are irrelevant.** Under the consensus floor the whole population pays about 0.002–0.01 BTH in size fees over 20 years. The relay minimum changes nothing. The only fee that moves value is demurrage, covered in 2 and 3.

## Validation of the step aggregation

The main runs process blocks in steps of 720 (`--step-blocks`). `--validate-days 120` runs the first 120 days of epoch 1, where the lottery share is 10% and demurrage is on, both exactly (one step per block) and at 720:

| Seed | Scenario | Gini exact / 720 | Lottery paid exact / 720 (BTH) | Largest cohort lottery-share gap |
|---|---|---|---|---|
| 1696 | wealth, honest | 0.9319 / 0.9321 | 5.365e6 / 5.377e6 | 0.16 pp |
| 1696 | egalitarian, honest | 0.3139 / 0.3172 | 6.000e6 / 6.081e6 | 0.16 pp |
| 2 | wealth, honest | 0.9319 / 0.9311 | 5.383e6 / 5.421e6 | 0.12 pp |
| 2 | egalitarian, honest | 0.3188 / 0.3133 | 5.825e6 / 6.043e6 | 0.08 pp |
| 3 | wealth, honest | 0.9341 / 0.9328 | 5.400e6 / 5.363e6 | 0.20 pp |
| 3 | egalitarian, honest | 0.3112 / 0.3196 | 6.222e6 / 5.844e6 | 0.23 pp |

Emission is identical in both modes (5.184e7 BTH). Gini agrees within 0.009, and lottery shares agree within 0.3 pp. Lottery paid differs by up to 6%, all of it the 80% of demurrage that feeds the pool. Demurrage depends on when the Poisson transactions fall, and it varies about as much between seeds in the same mode (exact-mode wealth: 2.27e5 / 2.48e5 / 2.70e5 BTH). The optimal-decoy runs show the same agreement. The aggregation error is far below the 0.05 bar and the effects above.

## Reproduce

```bash
cargo build --release -p bth-cluster-tax --features cli
B=target/release/cluster-tax-sim
$B m2-production                                  # 20 yr, snapshots 10/20, both mining models, both whales, seed 1696
$B m2-production --fee-model consensus-relay
$B m2-production --seed 2 ; $B m2-production --seed 3
$B m2-production --validate-days 120 [--seed N]   # exact vs aggregated
$B m2-production --horizon-years 10 --snapshot-years 1,2,3,4,5,6,7,8,10 --whale honest
# legacy comparison
$B m2-cumulative --horizon-years {10,20} --fee-model {legacy,consensus}
```

The runs above were made at `d5962dab` plus this change. A 20-year pair (on and off) takes about 4 minutes. `--mining` and `--whale` take `wealth|egalitarian|both` and `honest|optimal-decoy|both`. Determinism is covered by a unit test.

## Approximations, and which way they lean

The module doc in `m2_production.rs` lists them in full. Most lean in the mechanism's favour, so the failure is robust:

- **Genesis endowment in single clusters** (no pre-mine exists on the real chain). This gives whales the highest possible starting factor. *Favours the mechanism.*
- **Wallets consolidate everything on every spend.** Merged tags fall below the inputs' centroid, and the node charges that gap as a #925 downgrade. A coin-selecting wallet that never moves tainted coins pays less (the parker escape). *Favours the mechanism* (more demurrage collected).
- **Thin decoy universe.** The whale's decoys are the other agents' positions; a real chain has millions of background outputs. *Favours the mechanism.*
- **Honest decoys are negligible in value**, and decoys are assumed no older than the real input, so the age clock is the real input's.
- **Step aggregation** (validated above). Payouts are spread over the owner's eligible outputs by count.
- **Tag-mass union.** Mass below 1e-6 of a position is dropped once the union passes 4,096 clusters. *Lowers the floor slightly.*
- **No lottery farming.** No agent splits to farm tickets. #1682 (re-run in #1761) found that splitting is profitable on the implemented rules, so farming would concentrate the lottery further. *Favours the mechanism.*
- **Payments go to recipients in proportion to spending flow**, so they move no wealth in expectation but add noise.

## What this does not decide

- Whether to add a redistribution source (for example, weighting the draw against hashpower, a lottery share not proportional to output count, or a real per-output floor), or to restate the M2 criterion: **#1727**.
- Splitting to farm the lottery: #1682. Decoy dilution of the floor: #1686. Dropping tags in one transaction: #1687. The epoch-0 holiday: #1685.
- No node or consensus code changed. The only non-sim change is a drift test in `botho/src/monetary.rs` that pins the new `cluster_tax::mainnet_monetary_policy` mirror to the node's policy.
