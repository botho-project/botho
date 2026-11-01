# M2 at Production Parameters (#1696)

**Status:** re-run record. The sims change; consensus code does not.
**Date:** 2026-10-31; re-run 2026-11-01 after spent outputs were removed from the lottery window (Judge review of PR #1790).
**Parent:** epic #1683, the design-confidence parity review. Follows #1695 / `consensus-fee-model-sim-verdicts.md`.
**Decision owner:** #1727. This document supplies the numbers. Whether to add a redistribution source or restate the criterion is decided there, not here.

## Verdict

**The M2 (#605) premise is broken at production parameters.** The mechanism does not reduce Gini by anything close to the 0.05 bar. Under egalitarian mining it raises Gini on average. None of the top decile's value is priced above 1.5x after year 10.

| Metric (10 yr / 20 yr) | Wealth-proportional mining | Egalitarian mining | Bar |
|---|---|---|---|
| ΔGini caused by the mechanism (off − on counterfactual), mean of 3 seeds | +0.0001 to +0.0012 / −0.0006 to +0.0003 | −0.0203 to −0.0027 / −0.0282 to −0.0091 | > 0.05 |
| Margin vs the bar | short by ≈ 0.049 | short by ≥ 0.053 (best seed-mean −0.0027); single seeds range −0.047 to +0.010 | |
| Same, at 100x / 1000x the transaction rate (seed 1696, honest whale) | +0.0001 / +0.0006 and +0.0100 / +0.0185 | −0.085 / −0.137 and −0.093 / −0.147 | > 0.05 |
| Legacy ΔGini (gini0 − giniF), seed 1696 | −0.007 to −0.010 (FAIL) | +0.49 to +0.62 (PASS, but the counterfactual with the mechanism off gives +0.54 to +0.61) | > 0.05 |
| Share of top-decile value priced below 1.5x | **100% / 100%** | **100% / 100%** | should be small |

Ranges span the honest and optimal-decoy whale. The legacy definition passes under egalitarian mining only because equal block rewards across 140 agents flatten a 23M-BTH genesis ladder under 690M BTH of emission (10 yr). The mechanism-off run shows a similar drop, so it is not the mechanism's doing.

**Correction (2026-11-01).** The first version of this record (PR #1790 as opened) left spent coins eligible for the lottery until their 10k-block window ran out. On the node, candidates come only from the UTXO set. The bug overstated the mechanism: its 3-seed means spanned −0.009 to +0.020, and seed 1696 egalitarian/honest moved from +0.0273 to −0.0099 once spent coins were removed. Every number in this document is from the corrected sim.

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
| Lottery candidates | unspent outputs aged 720..=10,000 blocks, value ≥ 1e6 pico, capped at 10k (`validation.rs::lottery_candidates`, from `utxo_db`) | same; a spend removes the spent coins from the window and from the pending buckets; a uniform 10k subset when over the cap | all UTXOs |
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
| wealth | honest | 10 | 0.9302 | 0.9371 | 0.9369 | −0.0069 | −0.0002 | 100% |
| wealth | honest | 20 | 0.9302 | 0.9396 | 0.9386 | −0.0095 | −0.0010 | 100% |
| wealth | optimal-decoy | 10 | 0.9302 | 0.9367 | 0.9369 | −0.0065 | +0.0002 | 100% |
| wealth | optimal-decoy | 20 | 0.9302 | 0.9397 | 0.9386 | −0.0096 | −0.0012 | 100% |
| egalitarian | honest | 10 | 0.9302 | 0.3334 | 0.3234 | +0.5968 | −0.0099 | 100% |
| egalitarian | honest | 20 | 0.9302 | 0.4359 | 0.3890 | +0.4943 | −0.0469 | 100% |
| egalitarian | optimal-decoy | 10 | 0.9302 | 0.3134 | 0.3234 | +0.6168 | +0.0100 | 100% |
| egalitarian | optimal-decoy | 20 | 0.9302 | 0.4038 | 0.3890 | +0.5263 | −0.0148 | 100% |

`--fee-model consensus-relay` gives identical numbers for seed 1696 under wealth mining. Elsewhere it differs only within the seed noise below. Seed 1696 egalitarian gives mechanism ΔGini +0.0024 / −0.0377 (honest) and +0.0184 / −0.0214 (optimal-decoy). The 3-seed means are −0.0008 to +0.0007 (wealth) and −0.0350 to +0.0022 (egalitarian). Size fees (fees other than demurrage) total about 0.003 to 0.01 BTH over 20 years, so the relay minimum never matters.

### Seed spread (ΔGini mechanism, off − on)

| Mining | Whale | Yr | seed 1696 | seed 2 | seed 3 | mean |
|---|---|---|---|---|---|---|
| wealth | honest | 10 | −0.0002 | +0.0004 | +0.0001 | +0.0001 |
| wealth | honest | 20 | −0.0010 | −0.0013 | +0.0004 | −0.0006 |
| wealth | optimal-decoy | 10 | +0.0002 | +0.0018 | +0.0017 | +0.0012 |
| wealth | optimal-decoy | 20 | −0.0012 | −0.0002 | +0.0024 | +0.0003 |
| egalitarian | honest | 10 | −0.0099 | −0.0382 | −0.0128 | −0.0203 |
| egalitarian | honest | 20 | −0.0469 | −0.0219 | −0.0159 | −0.0282 |
| egalitarian | optimal-decoy | 10 | +0.0100 | −0.0271 | +0.0091 | −0.0027 |
| egalitarian | optimal-decoy | 20 | −0.0148 | −0.0033 | −0.0091 | −0.0091 |

Most of the egalitarian spread is payment noise. Payments are lumpy (a whale pays 1/60 of its position at a time), and the on and off runs drift apart once their balances differ, so their Gini paths diverge by a few hundredths in either direction. Under it there is a regressive drift: all four 3-seed means are negative, because the lottery pays frequent spenders less than their hashpower (cause 1 below). The wealth-mining spread is small because whales dominate both runs.

### Transaction-density sensitivity (`--tx-scale`)

At the M2 rates the 140 agents make about 13,000 transactions a year against 6,307,200 blocks, about 0.002 per block. About 99.6% of the window's non-payout outputs are coinbases, and payouts copy their winner, so they inherit that mix. `--tx-scale N` multiplies every cohort's transaction rate by N at a fixed velocity: each transaction moves 1/N as much of the position. Honest whale, consensus fees, ΔGini mechanism (off − on) at 10 / 20 years:

| tx scale | tx / block | Coinbase share of non-payout outputs | Mining | seed 1696 | seed 2 | seed 3 | Legacy ΔGini (seed 1696) | Demurrage, 20 yr (BTH) | Lottery share, 20 yr (seed 1696) |
|---|---|---|---|---|---|---|---|---|---|
| 1x | 0.002 | 99.6% | wealth | −0.0002 / −0.0010 | +0.0004 / −0.0013 | +0.0001 / +0.0004 | −0.0069 / −0.0095 | 5.9e7 | whales 98.5%, middle 1.3% |
| 1x | 0.002 | 99.6% | egalitarian | −0.0099 / −0.0469 | −0.0382 / −0.0219 | −0.0128 / −0.0159 | +0.5968 / +0.4943 | 1.17e8 | small 59.7%, middle 25.4%, merchant 6.1%, whales 8.9% |
| 100x | 0.21 | 70.8% | wealth | +0.0001 / +0.0006 | +0.0003 / +0.0002 | +0.0013 / +0.0024 | +0.0091 / +0.0228 | 2.2e3 | whales 97.5%, middle 2.1% |
| 100x | 0.21 | 70.8% | egalitarian | −0.0848 / −0.1372 | −0.0869 / −0.1365 | −0.0847 / −0.1419 | +0.7865 / +0.7300 | 5.1e4 | small 9.1%, middle 30.4%, merchant 0.0%, whales 60.5% |
| 1000x | 2.1 | 19.5% | wealth | +0.0100 / +0.0185 | +0.0099 / +0.0168 | +0.0102 / +0.0175 | +0.0221 / +0.0426 | 0.2 | whales 73.2%, middle 26.4% |
| 1000x | 2.1 | 19.5% | egalitarian | −0.0931 / −0.1470 | −0.0917 / −0.1476 | −0.0916 / −0.1477 | +0.7914 / +0.7333 | 0 | small 4.2%, middle 44.6%, merchant 0.0%, whales 51.3% |

"Whales" is the 9 whales plus the strategic whale. Hashpower shares are the same in every row (wealth: whales 98.3%; egalitarian: small 57.1%, middle 21.4%, merchant 14.3%, whales 7.1%). The optimal-decoy whale gives the same numbers as the honest one at 100x and 1000x, digit for digit (cause 4 below explains why).

Every row fails. Density changes the result in three ways:

- **Frequent spenders lose their tickets.** A consolidating wallet that spends more often than every 720 blocks rarely holds a mature output. At 100x a merchant spends about every 170 blocks and wins almost nothing. Tickets move to the agents that spend least often, whatever their wealth. Under egalitarian mining those are the whales and middle holders, so the lottery becomes strongly regressive. Under wealth mining the whales lose some of their own tickets to middle holders, which gives the only positive drift in the table, +0.0185 at 1000x and 20 years.
- **Demurrage disappears.** The stock wallet decays tags by 5% per hop. At 100x even the whales spend 1,200 times a year, so their endowment tags are gone before the epoch-0 holiday ends, and demurrage drops from 5.9e7 to 2.2e3 BTH (wealth). At 1000x nobody pays any.
- **The off run gets flatter too.** Smaller, more frequent payments add less noise, so egalitarian giniF off falls from 0.39 to 0.05–0.06 at 20 years. The legacy ΔGini still passes under egalitarian mining, but the mechanism-off run passes by more.

### Why it fails

**1. The uniform lottery pays in proportion to unspent output creation, which is mostly mining.** At 6.3M blocks/yr every block adds one coinbase and four payout outputs to the window, against about 13,000 transactions a year from the whole population. Payouts copy their winner's owner, so the window's ownership converges to each agent's share of coinbases plus transaction outputs, about its hashpower share, less what it loses each time it spends: a consolidating spend takes all of the agent's coins out of the window. Seed 1696, honest whale, 10 yr:

| Cohort | Hashpower (wealth) | Lottery share (wealth) | Hashpower (egalitarian) | Lottery share (egalitarian) |
|---|---|---|---|---|
| small (80) | 0.02% | 0.07% | 57.1% | 59.7% |
| middle (30) | 1.28% | 1.27% | 21.4% | 25.4% |
| merchant (20) | 0.43% | 0.21% | 14.3% | 6.1% |
| whales (9) | 76.9% | 77.6% | 6.4% | 8.0% |
| strategic (1) | 21.4% | 20.8% | 0.7% | 0.9% |

A lottery whose payout tracks the block-reward distribution cannot undo it. Over 10 years the lottery pays 1.4–1.9e8 BTH (20–27% of emission), and it goes where the coinbases go, except that merchants, who spend about every 17,000 blocks against a 10,000-block window, lose more than half of their share to everyone else. Under egalitarian mining some of that goes to the whales, which is why the egalitarian means in the seed table are negative. The density table above shows the same shift, much stronger, at higher transaction rates.

**2. The cluster factor cannot see mined wealth.** Each coinbase starts a fresh cluster with W ≤ 50 BTH, a factor of about 1.0x. When a holder consolidates, every coinbase tag weighs well under 0.1% of the merged output and is pruned. Endowment tags fade the same way: the stock wallet's 5% decay per hop, plus dilution by mined coins. The whole top decile prices below 1.5x by year 6 (wealth) or year 10 (egalitarian), and by year 10 every cohort's mean priced factor is 1.000x in every scenario. Demurrage stops at that point and is never charged again:

| Run (seed 1696) | Top decile < 1.5x at yr 1 / 3 / 5 / 8 / 10 | Demurrage collected yr 0–10 | after yr 10 |
|---|---|---|---|
| wealth, honest | 0% / 0.1% / 59% / 100% / 100% | 5.9e7 BTH | 0 |
| egalitarian, honest | 5% / 34% / 44% / 93% / 100% | 1.17e8 BTH | 0 |

**3. Demurrage lands on the wrong people under egalitarian mining.** Small holders spend weekly. Each spend re-credits their coinbase clusters' W (W += amount × weight), and payments from whales carry whale tags, so small and middle holders' positions price at about 1.6–2.5x during epoch 1. That is the same velocity-vs-holdings mispricing the legacy merchant flag reported. Over 10 years the 80 small holders pay 6.0e7 BTH of the 1.17e8 BTH collected. The 9 whales pay 8.8e6 BTH.

**4. The optimal-decoy whale does no better than the stock wallet,** because nothing is left to evade. With background claims available (#1687), the whale pays the #925 downgrade charge from the decoy-diluted floor once per spend. In this sim's thin decoy universe (#1686), that costs more than the stock wallet's gradual decay: 1.9e7 vs 1.1e7 BTH in the wealth scenario. Both end at 1.000x. At 100x and 1000x the transaction rate, the whale's tags have decayed away before demurrage starts, so both strategies pay nothing and the runs are identical.

**5. Size fees are irrelevant.** Under the consensus floor the whole population pays about 0.003–0.01 BTH in size fees over 20 years (about 1–2 BTH at 1000x). The relay minimum changes nothing. The only fee that moves value is demurrage, covered in 2 and 3.

## Validation of the step aggregation

The main runs process blocks in steps of 720 (`--step-blocks`). `--validate-days 120` runs the first 120 days of epoch 1, where the lottery share is 10% and demurrage is on, both exactly (one step per block) and at 720:

| Seed | Scenario | Gini exact / 720 | Lottery paid exact / 720 (BTH) | Largest cohort lottery-share gap |
|---|---|---|---|---|
| 1696 | wealth, honest | 0.9321 / 0.9323 | 5.366e6 / 5.377e6 | 0.63 pp |
| 1696 | egalitarian, honest | 0.3197 / 0.3220 | 5.934e6 / 6.048e6 | 0.68 pp |
| 2 | wealth, honest | 0.9321 / 0.9312 | 5.383e6 / 5.421e6 | 1.13 pp |
| 2 | egalitarian, honest | 0.3221 / 0.3194 | 5.846e6 / 5.933e6 | 0.44 pp |
| 3 | wealth, honest | 0.9341 / 0.9330 | 5.400e6 / 5.363e6 | 0.45 pp |
| 3 | egalitarian, honest | 0.3136 / 0.3236 | 6.154e6 / 5.864e6 | 0.34 pp |

Emission is identical in both modes (5.184e7 BTH). Gini agrees within 0.010, and lottery shares agree within 1.2 pp. Lottery paid differs by up to 5%, all of it the 80% of demurrage that feeds the pool. Demurrage depends on when the Poisson transactions fall, and it varies about as much between seeds in the same mode (exact-mode wealth: 2.28e5 / 2.48e5 / 2.71e5 BTH). The optimal-decoy runs show the same agreement. The aggregation error is far below the 0.05 bar and the effects above.

## Reproduce

```bash
cargo build --release -p bth-cluster-tax --features cli
B=target/release/cluster-tax-sim
$B m2-production                                  # 20 yr, snapshots 10/20, both mining models, both whales, seed 1696
$B m2-production --fee-model consensus-relay
$B m2-production --seed 2 ; $B m2-production --seed 3
$B m2-production --validate-days 120 [--seed N]   # exact vs aggregated
$B m2-production --horizon-years 10 --snapshot-years 1,2,3,4,5,6,7,8,10 --whale honest
$B m2-production --tx-scale {100,1000} --mining {wealth,egalitarian} --whale honest [--seed N]
# legacy comparison
$B m2-cumulative --horizon-years {10,20} --fee-model {legacy,consensus}
```

The runs above were made at `b3404acf` plus this change. A 20-year pair (on and off) takes about 4 minutes. `--mining` and `--whale` take `wealth|egalitarian|both` and `honest|optimal-decoy|both`. Determinism is covered by a unit test.

## Approximations, and which way they lean

The module doc in `m2_production.rs` lists them in full. Most lean in the mechanism's favour. The last two (transaction density, and consolidation against the 720-block maturity) do not always, so the density table above tests them directly. The mechanism fails at every density from 1x to 1000x:

- **Genesis endowment in single clusters** (no pre-mine exists on the real chain). This gives whales the highest possible starting factor. *Favours the mechanism.*
- **Wallets consolidate everything on every spend.** Merged tags fall below the inputs' centroid, and the node charges that gap as a #925 downgrade. A coin-selecting wallet that never moves tainted coins pays less (the parker escape). *Favours the mechanism* (more demurrage collected).
- **Thin decoy universe.** The whale's decoys are the other agents' positions; a real chain has millions of background outputs. *Favours the mechanism.*
- **Honest decoys are negligible in value**, and decoys are assumed no older than the real input, so the age clock is the real input's.
- **Step aggregation** (validated above). Payouts are spread over the owner's eligible outputs by count.
- **Tag-mass union.** Mass below 1e-6 of a position is dropped once the union passes 4,096 clusters. *Lowers the floor slightly.*
- **No lottery farming.** No agent splits to farm tickets. #1682 (re-run in #1761) found that splitting is profitable on the implemented rules, so farming would concentrate the lottery further. *Favours the mechanism.*
- **Payments go to recipients in proportion to spending flow**, so they move no wealth in expectation but add noise.
- **Low transaction density.** At the M2 rates there are about 0.002 transactions per block, so about 99.6% of the window's non-payout outputs are coinbases and the lottery follows hashpower. A busier chain would draw more of the window from transaction outputs. *Direction depends on the mining model.* Under wealth mining, low density leans against the lottery leg: at 1000x the mechanism's ΔGini rises to +0.010 / +0.019 (10 / 20 yr), still FAIL. Under egalitarian mining it leans in the mechanism's favour: at 100x and 1000x the mechanism's ΔGini falls to −0.08 to −0.15. At any density above about 100x it also removes the demurrage leg, because the 5%-per-hop wallet decay strips endowment tags before demurrage starts. *That part leans against the mechanism.*
- **Full consolidation against the 720-block maturity.** Every spend consumes all of the agent's coins, so an agent that spends more often than every 720 blocks seldom holds a mature output and gets almost no tickets. At 1x only merchants come close (one spend per about 17,000 blocks, against a 10,000-block window). At 100x and above it moves the lottery away from all frequent spenders. A coin-selecting wallet that leaves received outputs alone would keep more tickets. *Against the mechanism under egalitarian mining*, because the rare spenders it favours are the whales. *Slightly in its favour under wealth mining*, where it moves tickets from whales to middle holders.

## What this does not decide

- Whether to add a redistribution source (for example, weighting the draw against hashpower, a lottery share not proportional to output count, or a real per-output floor), or to restate the M2 criterion: **#1727**.
- Splitting to farm the lottery: #1682. Decoy dilution of the floor: #1686. Dropping tags in one transaction: #1687. The epoch-0 holiday: #1685.
- No node or consensus code changed. The only non-sim change is a drift test in `botho/src/monetary.rs` that pins the new `cluster_tax::mainnet_monetary_policy` mirror to the node's policy.
