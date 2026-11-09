# Redistribution Shapes Study (#1987)

**Status:** sim-only study. No node, consensus, wallet or constant change.
**Date:** 2026-11-09.
**Commissioned by:** the operator's ruling on #1837 (2026-11-09, option 2): build and measure the four rule shapes that the [redistribution redesign study](redistribution-redesign-study.md) §7.1 lists as not built, then decide. R3 is the fallback if nothing passes.
**Inputs:** the #1801 study and its harnesses: `m2-production` (#1790, [m2-production-verdicts.md](m2-production-verdicts.md)), the #1761 splitting grid ([ct-compatible-lottery-selection.md §9.1.1](ct-compatible-lottery-selection.md)), the #1757 production decoy routes and the §5.1 claim-reveal analysis; [ct-economics-gadgets.md](ct-economics-gadgets.md) for confidential amounts (#902).
**Commit:** the runs were made at `7d1295de` plus this change. Same-commit R0, R2 and R3 were re-run alongside the shapes, so every row below compares on one binary.

> **No decision is stated here. Ratification is the operator's.** This document reports measurements and ranks the shapes against the three bars the #1801 study set. Choosing one, combining them, or taking the R3 fallback is the operator's call on #1837.

## Verdict

None of the four shapes passes all three bars. None passes the Gini bar in any cell, under either candidate rule. Six configurations were built (two shapes have two forms). Each was run on the study's full Gini grid: 6 cells × 3 seeds × 2 candidate rules × 2 horizons. The highest 3-seed mean in any shape row is +0.0725 (S4a, wealth mining, 1000x, 20 years, node rule). That is R0's own single-horizon peak (+0.0726), which the 30-day window reproduces, and the same cell is +0.0426 at 10 years. Leaving that aside, the best new effect is +0.0298 (S2, wealth mining, 1x, 20 years, node rule), against the 0.05 bar.

| id | shape | Gini (> 0.05, every cell) | Splitting (#1761 grid, node rules) | Privacy | Node-side complexity | Confidential amounts (#902) |
|---|---|---|---|---|---|---|
| S1 | factor-dependent ticket price (R3 with a price ∝ factor) | **FAIL** (0/6 under both rules; best +0.0037 / +0.0020) | PASS on the grid as specified (0/192), but **FAIL** (188–192/192) once ≥ 5% of organic tickets are above 1x, which is the only regime in which the subsidy exists | PASS if purchase is mandatory (reads public counts and the output's public claimed factor); a small leak if opt-in, as R3 | medium (R3 plus a factor-weighted price base) | compatible once D2 (factor over hidden cluster wealth) is resolved; the public price reveals the claimed factor bucket, which today's claimed tags already reveal |
| S2 | churn fee proportional to value moved, with R2's value × (7 − factor) weight | **FAIL** (0/6; best +0.0268 / +0.0298) | **FAIL** (100/192 at 1%; break-even fee 11–70% of value moved) | PASS on public amounts (no reveal needed); under CT the public fee reveals every spend's amount unless D1 fee quantization covers every spend | low on public amounts (a fee term and a config switch); very high under CT (the #902 Path B weighted sort) | the fee itself fits Gadget 1 (`d ≥ c·V`), but the weighted draw needs the #902 ZK sort |
| S3a | demurrage to block producers instead of the pool (R3 lottery) | **FAIL** (0/6; best +0.0027 / +0.0021) | PASS (0/192, R3) | PASS (reads what the fee floor reads today) | medium (R3 plus routing the demurrage part of the fee to the coinbase) | compatible: the demurrage part is `fee − base`, both public |
| S3b | settle on spend (background outputs, spender pays the #925 charge) + demurrage to block producers (R3 lottery) | **FAIL** (0/6; best +0.0045 / +0.0047) | PASS (0/192, R3) | PASS, and outputs carry no cluster tags (removes the #1756 output fingerprint) | medium (S3a plus an empty-tag output rule); couples to #1685 and ADR 0007 | compatible: Gadget 1 already covers the #925 charge |
| S4a | 30-day lottery window, uniform tickets | **FAIL** (0/6; best +0.0176 / +0.0725) | **FAIL** (192/192) | PASS under the node rule (reads public ages) | low (a constant), with a candidate scan over a 52× wider age band | compatible (uniform selection, as today) |
| S4b | 30-day window, value × (7 − factor) | **FAIL** (0/6; best +0.0039 / +0.0220) | **FAIL** on the node (192/192); PASS (0/192) only with a spend reveal | PASS under the node rule on public amounts; the reveal form deanonymizes 2–52% of gamma-spend rings outright (§4) | low on public amounts (constant + `ClusterWeighted`); very high under CT | needs the #902 ZK weighted sort |

"Gini" is the #1790 bar, as in the #1801 study: the mechanism's own ΔGini (off − on) above 0.05, as a 3-seed mean (seeds 1696, 2, 3), at both 10 and 20 years, in each of 6 cells (wealth or egalitarian mining × 1x, 100x or 1000x transaction density), with the honest whale. In the Gini column, the first "best" figure is under #1790's candidate rule (spent outputs leave the draw), and the second is under the node's rule (`--spent-eligible`). A shape fails if it passes no cell under either rule.

**Recommendation in one line (§6):** nothing passes, so the fallback the ruling named applies. R3 remains the best-supported option: it passes splitting and privacy, and like every measured shape it does not pass Gini. None of the four shapes adds a measurable Gini effect on top of it. S3a and S3b are the only shapes that keep R3's two passing bars. Their best cells are within 0.003 of R3's, and S3a is below R3 in the egalitarian 1x cell at 20 years.

## 1. The shapes and how each is modelled

Each shape is a set of switches on `m2_production.rs` (`ShapeSwitches` and `TicketRule::FactorPrice`). The defaults change nothing: the #1790 bit-for-bit test (`baseline_reproduces_1790_bit_for_bit`) and the #1761 reproduction (`baseline_reproduces_1761`) still pass. Everything else is the #1801 harness unchanged: population, emission, fees, cluster rules, the 720-block aggregation, the honest whale, and the counterfactual (the same seed with no lottery share, fees burned and no demurrage). Every switch is inert with the mechanism off (tested), and S3b's settle rule only starts with a positive demurrage rate. One counterfactual set therefore serves every shape.

- **S1, factor-dependent ticket price.** As R3, an output of at least `min_utxo_value` is eligible only if it buys a ticket, and the price is burned. Here the price is `base × w`, where `w` is the new output's factor in units of 1x, read from its tags at creation. Coinbases are fresh clusters, at 1x. The base is set once per step: the previous step's total lottery payout divided by the summed `w` of the tickets bought in that step. The burn still balances the payout in aggregate, but a 1x ticket pays less than its expected payout whenever higher-factor tickets are in the pool, and a high-factor ticket pays more. That makes it a budget-balanced progressive subsidy. When every ticket is at 1x it is exactly R3's price (tested). Wallets buy under R3's rule.
- **S2, churn fee.** R2's ticket weight (value × (7 − factor), the `ClusterWeighted` kernel) plus an ad-valorem fee of `c` on value moved, added to the consensus fee and pooled like any fee (80% pool, 20% burn). The node cannot tell change from payment, so it would charge the whole output sum. The m2 wallet always consolidates, so to avoid charging it on its own change the harness charges `c` on the payment only. That is what a coin-selecting wallet would pay, and it leans in the shape's favour. The splitting grid finds the `c` that closes churn (§3); it is 11–70% of the value moved. No fee an honest payer could bear reaches that, so the Gini runs use `c` = 1% (`CHURN_FEE_PPM` = 10,000), a payment-network-like rate.
- **S3a, demurrage to block producers.** The demurrage part of each fee leaves the fee stream: 80% goes to the next step's block producers, added to their coinbase outputs (so in proportion to blocks mined), and 20% is burned as before. The lottery pool keeps the emission share and 80% of the size fees. The lottery is R3, because R3 is the only studied lottery that passes splitting without a reveal. The demurrage side then carries all of the redistribution. This is "who receives".
- **S3b, settle on spend.** S3a, plus a "who pays" rule aimed at #1790's cause 3 (payment recipients inherit the payer's tags and pay its demurrage). Every spend's outputs are background, and the spender pays the #925 capitalized charge for all the tag mass it drops, from its input floor. The holder therefore pays its provenance in full at its next spend, and recipients never inherit tags. *The rule starts in epoch 1.* Under the epoch-0 holiday (#1685, open) the #925 charge is 0, so a settle rule live in epoch 0 would drop every tag for free. A first test run did exactly that, collecting no demurrage in 20 years. Starting it with demurrage leans in the shape's favour.
- **S4a and S4b, a different window.** The lottery window is 720..=518,400 blocks (30 days) instead of 720..=10,000 (about 14 hours): 52× longer. The longer window is the "age range" lever. The "eligibility basis" lever is uniform per output (S4a) against value × (7 − factor) (S4b), each under both candidate rules. Under #1790's rule a spent output leaves the draw; that needs a reveal. Under the node's rule it stays a candidate until it ages out. S4 runs on an incremental-weight path (running sums of ticket weight, inverse-CDF picks) that keeps a 52× window tractable. At the node's window that path reproduces R0 and R2 to within 0.0004 in 19 of 24 cells (seed 1696). The other five are 1x cells, at 0.004 to 0.041. Each is at or below that cell's measured seed-to-seed spread (§2.4): the two paths draw their random numbers in a different order, and the 1x cells are dominated by lumpy payment noise.

What the shapes did not try, and why:

- **Other subsidy curves for S1.** Any `w(f)` that charges 1x outputs less than their expected payout while the pool holds higher-factor tickets is a subsidy of the same kind. §3 measures the subsidy's farmability directly as a function of how many tickets are above 1x, which covers the family.
- **A burned (unpooled) churn fee.** Burning the fee does not change the break-even, which is set by the first BTH churned (§3).
- **Other windows.** 30 days is the length at which a whale's monthly consolidated output (12 spends a year) sits in the window for most of its life. A 1-year window would make value weight track holdings almost exactly, which goes further in the direction S4b measures (towards the whales).

## 2. Axis 1: Gini (m2-production, #1790 harness)

### 2.1 Summary

Same-commit R0, R2 and R3 rows are included for direct comparison. R0 reproduces every published per-seed value of the #1801 study (§3.1, §3.2) to the fourth decimal. R2 and R3 reproduce it to the fourth decimal at 1000x and within 0.0004 at 100x. At 1x they differ by up to 0.005 per seed under wealth mining and up to 0.037 under egalitarian mining. That is #1875's exact demurrage kernel (protocol 9.0.0, after the study), acting on the demurrage-heavy 1x paths. Comparisons below use the same-commit rows.

#### ΔGini summary, #1790 candidate rule: spent outputs leave the draw

| shape | mean ΔGini range, wealth | mean ΔGini range, egalitarian | best 3-seed mean | cells passing |
|---|---|---|---|---|
| R0 baseline (same commit) | -0.0006 to +0.0176 | -0.1474 to -0.0203 | +0.0176 | 0/6 |
| R2 inverse-factor (same commit) | +0.0000 to +0.0276 | -0.1639 to -0.0119 | +0.0276 | 0/6 |
| R3 ticket-price (same commit) | -0.0002 to +0.0042 | -0.0314 to -0.0051 | +0.0042 | 0/6 |
| S1 factor-dependent ticket price | -0.0002 to +0.0037 | -0.0294 to -0.0051 | +0.0037 | 0/6 |
| S2 R2 weight + 1% churn fee | -0.0025 to +0.0268 | -0.2669 to -0.0075 | +0.0268 | 0/6 |
| S3a R3 + demurrage to block producers | -0.0002 to +0.0017 | -0.0272 to +0.0027 | +0.0027 | 0/6 |
| S3b R3 + settle on spend + demurrage to block producers | -0.0002 to +0.0045 | -0.0433 to -0.0051 | +0.0045 | 0/6 |
| S4a 30-day window, uniform | +0.0002 to +0.0176 | -0.1474 to -0.0857 | +0.0176 | 0/6 |
| S4b 30-day window, value x (7 - factor) | -0.0005 to +0.0039 | -0.1639 to -0.0037 | +0.0039 | 0/6 |
#### ΔGini summary, node candidate rule: spent outputs stay eligible (--spent-eligible)

| shape | mean ΔGini range, wealth | mean ΔGini range, egalitarian | best 3-seed mean | cells passing |
|---|---|---|---|---|
| R0 baseline (same commit) | -0.0000 to +0.0726 | -0.0002 to +0.0112 | +0.0726 | 0/6 |
| R2 inverse-factor (same commit) | +0.0136 to +0.0268 | -0.0429 to -0.0013 | +0.0268 | 0/6 |
| R3 ticket-price (same commit) | -0.0003 to +0.0018 | -0.0160 to -0.0051 | +0.0018 | 0/6 |
| S1 factor-dependent ticket price | -0.0005 to +0.0020 | -0.0244 to -0.0051 | +0.0020 | 0/6 |
| S2 R2 weight + 1% churn fee | +0.0139 to +0.0298 | -0.0858 to -0.0038 | +0.0298 | 0/6 |
| S3a R3 + demurrage to block producers | -0.0003 to +0.0021 | -0.0482 to -0.0051 | +0.0021 | 0/6 |
| S3b R3 + settle on spend + demurrage to block producers | -0.0002 to +0.0047 | -0.0379 to -0.0051 | +0.0047 | 0/6 |
| S4a 30-day window, uniform | -0.0003 to +0.0725 | -0.0207 to +0.0113 | +0.0725 | 0/6 |
| S4b 30-day window, value x (7 - factor) | +0.0131 to +0.0220 | -0.0429 to -0.0034 | +0.0220 | 0/6 |

### 2.2 Reading the Gini results

- **No shape moves the bar.** Under #1790's rule the four shapes span −0.27 to +0.027. Under the node's rule they span −0.09 to +0.073, and the top of that range is S4a reproducing R0's single-horizon wealth/1000x peak, not a new effect. Every cell fails at one horizon or both.
- **S1 is R3 in practice.** The mean factor weight of the tickets bought is 1.033 to 1.034 over 10 years at 1x (§2.3) and 1.000 at density. Almost every ticket is 1x, so there is almost no subsidy to pay, and S1's Gini is R3's to within seed noise (best +0.0037 against R3's +0.0042). This is #1790's cause 2 again: the cluster factor cannot see mined wealth, so a factor-keyed subsidy has no one to subsidize.
- **S2 is R2 plus a regressive tax.** The 1% fee collects 1.3e7 to 6.1e7 BTH over 10 years (seed 1696). The 80% that reaches the pool is redistributed by R2's value weight, which pays merchants (turnover) 61–82% of the lottery at 1x. Its best cell, +0.0298 (wealth, 1x, node rule), is R2's +0.0268 plus 0.003. Under egalitarian mining at 1000x and #1790's rule it reaches −0.27, against R2's −0.16. The fee falls on value moved, so per unit of wealth a merchant (velocity 2/yr) pays 10× what a whale (0.2/yr) pays.
- **The demurrage side has little to tax (S3).** S3a moves 4.5e7 (wealth) to 9.1e7 BTH (egalitarian) of demurrage to block producers over 10 years at 1x, and next to nothing at 100x and 1000x, where tags are gone before demurrage starts (#1790's cause 1). Under egalitarian mining that money goes out 57% to small holders. But the demurrage itself is mostly paid by the same small and middle holders, who pay it as payment recipients (cause 3): 7.1e7 to 7.9e7 of the 1.0e8 to 1.1e8 BTH collected over 20 years (node rule), by seed. The transfer is about 1e8 BTH against about 7e8 BTH of emission. At 20 years (egalitarian, 1x, node rule) the cohorts' wealth shares are within 0.02 of R3's in every seed, but S3a's Gini is 0.02–0.04 higher in all three. The difference lies inside cohorts, not between them. We did not isolate the channel. Two candidates are the dividend's lumpiness (it is paid to whoever mines the following step) and R3's smaller pool (1.9e8 against 2.7–2.8e8 BTH paid over 20 years), which pays fewer and smaller lottery outputs.
- **Settling on spend makes demurrage a one-off levy (S3b).** Once every spend drops all tags and pays for them, a position is taxed once, at its first epoch-1 spend, and then sits at 1x for good. S3b collects 1.1e7 (egalitarian) to 2.9e7 BTH (wealth) of demurrage over 10 years at 1x, against 5.6e7 to 1.1e8 for R3. It does move who pays: the 9 whales' share of the demurrage paid rises from 6–7% under R3 to 31–36% (egalitarian, 1x, 20 years, node rule, by seed). But the amount is too small to register: best +0.0045 / +0.0047. In this harness the only lasting high-factor provenance is the genesis endowment. A real chain has no pre-mine, so it would collect even less.
- **A longer window makes value weight track holdings, and holdings are the whales' (S4).** S4a (uniform) is R0 at density, within 0.0002 at 1000x: the window is coinbase-dominated either way. At 1x under #1790's rule it is −0.097 / −0.121 (egalitarian). A spend removes an output's ticket, so over a 30-day window only rare spenders keep their outputs eligible for long, and the rare spenders are the whales: 53.7% of payouts against R0's 8.0% (seed 1696, 10 years). S4b (value-weighted) pays the whales 81% of the lottery under wealth mining at 1x (#1790's rule), against R2's merchant-heavy split. Its best cell, +0.0220 under the node rule, is below R2's +0.0268.

### 2.3 Where the money goes (seed 1696, 10 years)

#### Where the money goes (seed 1696, 10 years, #1790 rule)

Cohort shares of lottery payouts; totals over 10 years in BTH. `ticket factor` = mean factor weight of the tickets bought (S1).

| shape | mining | tx scale | small | middle | merchant | whales (9) | strategic | lottery paid | demurrage | ticket burn | churn fee | to miners | ticket factor |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| R0 | wealth | 1x | 0.1% | 1.3% | 0.2% | 77.6% | 20.8% | 1.395e+08 | 5.942e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | wealth | 100x | 0.5% | 1.8% | 0.0% | 82.8% | 14.9% | 9.193e+07 | 2.234e+03 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | wealth | 1000x | 0.5% | 23.8% | 0.0% | 67.2% | 8.5% | 9.192e+07 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | egalitarian | 1x | 59.6% | 25.4% | 6.1% | 8.0% | 0.9% | 1.857e+08 | 1.173e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | egalitarian | 100x | 9.2% | 30.3% | 0.0% | 54.6% | 5.9% | 9.196e+07 | 5.145e+04 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | egalitarian | 1000x | 4.4% | 45.8% | 0.0% | 44.3% | 5.5% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R2 | wealth | 1x | 0.0% | 1.5% | 80.7% | 14.7% | 3.0% | 1.498e+08 | 7.233e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | wealth | 100x | 0.0% | 2.0% | 0.0% | 79.4% | 18.5% | 9.192e+07 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | wealth | 1000x | 0.0% | 4.5% | 0.0% | 81.5% | 14.0% | 9.192e+07 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | egalitarian | 1x | 33.9% | 5.7% | 59.6% | 0.7% | 0.1% | 1.876e+08 | 1.196e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | egalitarian | 100x | 55.6% | 28.8% | 0.4% | 13.0% | 2.2% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | egalitarian | 1000x | 2.9% | 29.2% | 0.0% | 59.1% | 8.8% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R3 | wealth | 1x | 32.7% | 7.5% | 42.2% | 14.6% | 2.9% | 1.372e+08 | 5.666e+07 | 1.245e+08 | 0.000e+00 | 0.000e+00 | — |
| R3 | wealth | 100x | 8.5% | 43.9% | 0.0% | 47.1% | 0.5% | 1.553e-05 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R3 | wealth | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 1.835e-05 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R3 | egalitarian | 1x | 48.2% | 15.9% | 33.9% | 1.8% | 0.3% | 1.820e+08 | 1.125e+08 | 1.752e+08 | 0.000e+00 | 0.000e+00 | — |
| R3 | egalitarian | 100x | 8.8% | 43.7% | 0.0% | 47.0% | 0.5% | 3.462e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R3 | egalitarian | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 2.243e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| S1 | wealth | 1x | 37.0% | 7.2% | 39.8% | 13.3% | 2.6% | 1.350e+08 | 5.391e+07 | 1.473e+08 | 0.000e+00 | 0.000e+00 | 1.033 |
| S1 | wealth | 100x | 8.5% | 43.9% | 0.0% | 47.1% | 0.5% | 1.553e-05 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S1 | wealth | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 1.835e-05 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S1 | egalitarian | 1x | 48.3% | 15.8% | 33.8% | 1.8% | 0.3% | 1.795e+08 | 1.095e+08 | 1.891e+08 | 0.000e+00 | 0.000e+00 | 1.034 |
| S1 | egalitarian | 100x | 8.8% | 43.7% | 0.0% | 47.0% | 0.5% | 3.462e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S1 | egalitarian | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 2.243e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S2 | wealth | 1x | 0.0% | 1.6% | 80.0% | 15.3% | 3.1% | 1.665e+08 | 7.179e+07 | 0.000e+00 | 2.138e+07 | 0.000e+00 | 1.000 |
| S2 | wealth | 100x | 0.0% | 2.0% | 0.0% | 79.7% | 18.3% | 1.026e+08 | 1.684e+00 | 0.000e+00 | 1.329e+07 | 0.000e+00 | 1.000 |
| S2 | wealth | 1000x | 0.0% | 4.5% | 0.0% | 81.7% | 13.8% | 1.026e+08 | 2.255e-01 | 0.000e+00 | 1.329e+07 | 0.000e+00 | 1.000 |
| S2 | egalitarian | 1x | 33.1% | 5.4% | 60.7% | 0.7% | 0.1% | 2.318e+08 | 1.197e+08 | 0.000e+00 | 5.985e+07 | 0.000e+00 | 1.000 |
| S2 | egalitarian | 100x | 55.1% | 28.8% | 0.3% | 13.5% | 2.3% | 1.380e+08 | 0.000e+00 | 0.000e+00 | 5.764e+07 | 0.000e+00 | 1.000 |
| S2 | egalitarian | 1000x | 2.6% | 27.3% | 0.0% | 61.1% | 8.9% | 1.358e+08 | 0.000e+00 | 0.000e+00 | 5.478e+07 | 0.000e+00 | 1.000 |
| S3a | wealth | 1x | 0.1% | 1.3% | 0.8% | 76.6% | 21.2% | 9.192e+07 | 5.575e+07 | 9.186e+07 | 0.000e+00 | 4.460e+07 | 1.000 |
| S3a | wealth | 100x | 8.5% | 43.9% | 0.0% | 47.1% | 0.5% | 1.553e-05 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 1.346e+00 | 1.000 |
| S3a | wealth | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 1.835e-05 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 1.759e-01 | 1.000 |
| S3a | egalitarian | 1x | 58.4% | 22.4% | 11.6% | 6.7% | 0.7% | 9.192e+07 | 1.135e+08 | 9.193e+07 | 0.000e+00 | 9.077e+07 | 1.000 |
| S3a | egalitarian | 100x | 8.8% | 43.7% | 0.0% | 47.0% | 0.5% | 3.462e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S3a | egalitarian | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 2.243e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S3b | wealth | 1x | 2.0% | 4.9% | 31.1% | 49.3% | 12.8% | 9.192e+07 | 2.948e+07 | 8.643e+07 | 0.000e+00 | 2.358e+07 | 1.000 |
| S3b | wealth | 100x | 8.5% | 43.9% | 0.0% | 47.1% | 0.5% | 1.553e-05 | 4.294e-01 | 0.000e+00 | 0.000e+00 | 3.435e-01 | 1.000 |
| S3b | wealth | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 1.835e-05 | 1.294e-01 | 0.000e+00 | 0.000e+00 | 1.035e-01 | 1.000 |
| S3b | egalitarian | 1x | 53.8% | 19.5% | 21.7% | 4.5% | 0.5% | 9.192e+07 | 1.077e+07 | 9.038e+07 | 0.000e+00 | 8.617e+06 | 1.000 |
| S3b | egalitarian | 100x | 8.8% | 43.7% | 0.0% | 47.0% | 0.5% | 3.462e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S3b | egalitarian | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 2.243e-05 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | wealth | 1x | 0.0% | 0.7% | 0.0% | 86.2% | 13.2% | 1.394e+08 | 5.939e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | wealth | 100x | 0.4% | 1.8% | 0.0% | 83.0% | 14.7% | 9.192e+07 | 1.476e+03 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | wealth | 1000x | 0.5% | 23.8% | 0.0% | 67.2% | 8.5% | 9.192e+07 | 2.263e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | egalitarian | 1x | 14.0% | 29.7% | 0.2% | 53.7% | 2.4% | 1.842e+08 | 1.153e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | egalitarian | 100x | 9.1% | 30.0% | 0.0% | 54.9% | 5.9% | 9.197e+07 | 6.077e+04 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | egalitarian | 1000x | 4.4% | 45.8% | 0.0% | 44.3% | 5.5% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | wealth | 1x | 0.0% | 2.1% | 0.4% | 81.1% | 16.4% | 1.417e+08 | 6.226e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | wealth | 100x | 0.0% | 1.6% | 0.0% | 79.3% | 19.0% | 9.192e+07 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | wealth | 1000x | 0.0% | 4.5% | 0.0% | 81.5% | 14.0% | 9.192e+07 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | egalitarian | 1x | 62.8% | 20.3% | 13.2% | 3.2% | 0.4% | 1.893e+08 | 1.218e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | egalitarian | 100x | 53.3% | 28.4% | 0.3% | 15.3% | 2.6% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | egalitarian | 1000x | 2.9% | 29.2% | 0.0% | 59.1% | 8.8% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
#### Where the money goes (seed 1696, 10 years, node rule)

Cohort shares of lottery payouts; totals over 10 years in BTH. `ticket factor` = mean factor weight of the tickets bought (S1).

| shape | mining | tx scale | small | middle | merchant | whales (9) | strategic | lottery paid | demurrage | ticket burn | churn fee | to miners | ticket factor |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| R0 | wealth | 1x | 0.1% | 1.3% | 0.5% | 76.7% | 21.3% | 1.402e+08 | 6.029e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | wealth | 100x | 4.9% | 2.8% | 10.5% | 64.2% | 17.5% | 9.192e+07 | 2.282e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | wealth | 1000x | 14.6% | 5.2% | 31.7% | 38.4% | 10.2% | 9.192e+07 | 2.276e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | egalitarian | 1x | 57.1% | 21.4% | 14.4% | 6.4% | 0.7% | 1.859e+08 | 1.174e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | egalitarian | 100x | 53.2% | 19.0% | 22.2% | 5.0% | 0.6% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R0 | egalitarian | 1000x | 45.6% | 14.4% | 37.4% | 2.3% | 0.3% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| R2 | wealth | 1x | 0.0% | 1.3% | 82.6% | 13.2% | 2.8% | 1.504e+08 | 7.309e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | wealth | 100x | 0.0% | 1.7% | 50.3% | 38.4% | 9.6% | 9.192e+07 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | wealth | 1000x | 0.1% | 1.6% | 49.8% | 38.5% | 10.0% | 9.192e+07 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | egalitarian | 1x | 28.1% | 4.6% | 66.7% | 0.5% | 0.1% | 1.882e+08 | 1.204e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | egalitarian | 100x | 29.7% | 5.0% | 63.9% | 1.2% | 0.2% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R2 | egalitarian | 1000x | 29.6% | 5.0% | 64.0% | 1.2% | 0.2% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | — |
| R3 | wealth | 1x | 22.9% | 6.0% | 57.8% | 11.1% | 2.2% | 1.369e+08 | 5.620e+07 | 1.324e+08 | 0.000e+00 | 0.000e+00 | — |
| R3 | wealth | 100x | 4.4% | 2.7% | 9.8% | 65.3% | 17.9% | 9.192e+07 | 2.108e+00 | 9.198e+07 | 0.000e+00 | 0.000e+00 | — |
| R3 | wealth | 1000x | 14.0% | 5.4% | 28.8% | 41.0% | 10.7% | 9.192e+07 | 1.067e+00 | 9.194e+07 | 0.000e+00 | 0.000e+00 | — |
| R3 | egalitarian | 1x | 43.8% | 13.8% | 40.7% | 1.6% | 0.2% | 1.820e+08 | 1.126e+08 | 1.757e+08 | 0.000e+00 | 0.000e+00 | — |
| R3 | egalitarian | 100x | 53.3% | 19.1% | 22.1% | 5.0% | 0.6% | 9.192e+07 | 0.000e+00 | 9.197e+07 | 0.000e+00 | 0.000e+00 | — |
| R3 | egalitarian | 1000x | 46.2% | 14.7% | 36.4% | 2.4% | 0.3% | 9.192e+07 | 0.000e+00 | 9.194e+07 | 0.000e+00 | 0.000e+00 | — |
| S1 | wealth | 1x | 27.0% | 5.6% | 54.4% | 10.7% | 2.2% | 1.353e+08 | 5.422e+07 | 1.550e+08 | 0.000e+00 | 0.000e+00 | 1.033 |
| S1 | wealth | 100x | 4.3% | 2.6% | 9.8% | 65.3% | 17.9% | 9.192e+07 | 3.281e+00 | 9.198e+07 | 0.000e+00 | 0.000e+00 | 1.003 |
| S1 | wealth | 1000x | 14.1% | 5.4% | 28.8% | 41.0% | 10.7% | 9.192e+07 | 4.513e-01 | 9.194e+07 | 0.000e+00 | 0.000e+00 | 1.000 |
| S1 | egalitarian | 1x | 43.9% | 13.7% | 40.6% | 1.6% | 0.2% | 1.794e+08 | 1.093e+08 | 1.883e+08 | 0.000e+00 | 0.000e+00 | 1.034 |
| S1 | egalitarian | 100x | 53.3% | 19.1% | 22.1% | 5.0% | 0.6% | 9.192e+07 | 0.000e+00 | 9.197e+07 | 0.000e+00 | 0.000e+00 | 1.002 |
| S1 | egalitarian | 1000x | 46.2% | 14.7% | 36.4% | 2.4% | 0.3% | 9.192e+07 | 0.000e+00 | 9.194e+07 | 0.000e+00 | 0.000e+00 | 1.000 |
| S2 | wealth | 1x | 0.0% | 1.2% | 82.3% | 13.6% | 2.8% | 1.667e+08 | 7.180e+07 | 0.000e+00 | 2.162e+07 | 0.000e+00 | 1.000 |
| S2 | wealth | 100x | 0.0% | 1.6% | 52.7% | 36.5% | 9.1% | 1.061e+08 | 1.610e+00 | 0.000e+00 | 1.771e+07 | 0.000e+00 | 1.000 |
| S2 | wealth | 1000x | 0.1% | 1.5% | 52.0% | 36.8% | 9.5% | 1.059e+08 | 1.628e-01 | 0.000e+00 | 1.752e+07 | 0.000e+00 | 1.000 |
| S2 | egalitarian | 1x | 27.1% | 4.4% | 67.9% | 0.5% | 0.1% | 2.318e+08 | 1.202e+08 | 0.000e+00 | 6.086e+07 | 0.000e+00 | 1.000 |
| S2 | egalitarian | 100x | 28.3% | 4.8% | 65.7% | 1.1% | 0.2% | 1.416e+08 | 0.000e+00 | 0.000e+00 | 6.215e+07 | 0.000e+00 | 1.000 |
| S2 | egalitarian | 1000x | 28.2% | 4.8% | 65.7% | 1.1% | 0.2% | 1.416e+08 | 0.000e+00 | 0.000e+00 | 6.212e+07 | 0.000e+00 | 1.000 |
| S3a | wealth | 1x | 0.1% | 1.3% | 0.9% | 76.4% | 21.2% | 9.192e+07 | 5.572e+07 | 9.188e+07 | 0.000e+00 | 4.458e+07 | 1.000 |
| S3a | wealth | 100x | 4.4% | 2.7% | 9.8% | 65.3% | 17.9% | 9.192e+07 | 2.108e+00 | 9.198e+07 | 0.000e+00 | 1.687e+00 | 1.000 |
| S3a | wealth | 1000x | 13.7% | 5.4% | 28.9% | 41.2% | 10.8% | 9.192e+07 | 1.198e+00 | 9.194e+07 | 0.000e+00 | 9.585e-01 | 1.000 |
| S3a | egalitarian | 1x | 56.8% | 21.2% | 14.9% | 6.3% | 0.7% | 9.192e+07 | 1.134e+08 | 9.192e+07 | 0.000e+00 | 9.070e+07 | 1.000 |
| S3a | egalitarian | 100x | 53.3% | 19.1% | 22.1% | 5.0% | 0.6% | 9.192e+07 | 0.000e+00 | 9.197e+07 | 0.000e+00 | 0.000e+00 | 1.000 |
| S3a | egalitarian | 1000x | 46.2% | 14.7% | 36.4% | 2.4% | 0.3% | 9.192e+07 | 0.000e+00 | 9.194e+07 | 0.000e+00 | 0.000e+00 | 1.000 |
| S3b | wealth | 1x | 1.7% | 4.1% | 32.4% | 49.0% | 12.8% | 9.192e+07 | 2.948e+07 | 8.675e+07 | 0.000e+00 | 2.358e+07 | 1.000 |
| S3b | wealth | 100x | 4.5% | 2.7% | 9.8% | 65.3% | 17.9% | 9.192e+07 | 4.304e-01 | 9.198e+07 | 0.000e+00 | 3.443e-01 | 1.000 |
| S3b | wealth | 1000x | 13.9% | 5.4% | 28.9% | 41.1% | 10.8% | 9.192e+07 | 1.294e-01 | 9.194e+07 | 0.000e+00 | 1.035e-01 | 1.000 |
| S3b | egalitarian | 1x | 50.9% | 18.1% | 26.3% | 4.2% | 0.5% | 9.192e+07 | 1.077e+07 | 9.043e+07 | 0.000e+00 | 8.613e+06 | 1.000 |
| S3b | egalitarian | 100x | 53.3% | 19.1% | 22.1% | 5.0% | 0.6% | 9.192e+07 | 0.000e+00 | 9.197e+07 | 0.000e+00 | 0.000e+00 | 1.000 |
| S3b | egalitarian | 1000x | 46.2% | 14.7% | 36.4% | 2.4% | 0.3% | 9.192e+07 | 0.000e+00 | 9.194e+07 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | wealth | 1x | 0.1% | 1.3% | 0.6% | 76.8% | 21.3% | 1.402e+08 | 6.029e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | wealth | 100x | 4.9% | 2.8% | 10.5% | 64.2% | 17.6% | 9.192e+07 | 2.030e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | wealth | 1000x | 14.6% | 5.2% | 31.5% | 38.5% | 10.2% | 9.192e+07 | 1.563e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | egalitarian | 1x | 57.1% | 21.4% | 14.4% | 6.4% | 0.7% | 1.858e+08 | 1.174e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | egalitarian | 100x | 53.2% | 19.0% | 22.2% | 5.0% | 0.6% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4a | egalitarian | 1000x | 45.6% | 14.4% | 37.4% | 2.3% | 0.3% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | wealth | 1x | 0.0% | 1.0% | 68.5% | 24.3% | 6.1% | 1.483e+08 | 7.051e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | wealth | 100x | 0.1% | 1.7% | 48.3% | 39.6% | 10.3% | 9.192e+07 | 1.682e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | wealth | 1000x | 0.1% | 1.7% | 49.0% | 39.2% | 10.2% | 9.192e+07 | 2.198e-01 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | egalitarian | 1x | 27.6% | 4.5% | 67.4% | 0.5% | 0.1% | 1.885e+08 | 1.207e+08 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | egalitarian | 100x | 29.8% | 5.0% | 63.8% | 1.2% | 0.2% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |
| S4b | egalitarian | 1000x | 29.7% | 5.1% | 63.9% | 1.2% | 0.2% | 9.192e+07 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 0.000e+00 | 1.000 |

### 2.4 By cell

#### ΔGini by cell, #1790 rule

| shape | mining | tx scale | seed 1696 (10 / 20 yr) | seed 2 | seed 3 | 3-seed mean | > 0.05 |
|---|---|---:|---|---|---|---|---|
| R0 | wealth | 1x | -0.0002 / -0.0010 | +0.0004 / -0.0013 | +0.0001 / +0.0004 | +0.0001 / -0.0006 | FAIL |
| R0 | wealth | 100x | +0.0001 / +0.0006 | +0.0003 / +0.0002 | +0.0013 / +0.0024 | +0.0006 / +0.0010 | FAIL |
| R0 | wealth | 1000x | +0.0100 / +0.0185 | +0.0099 / +0.0168 | +0.0102 / +0.0175 | +0.0100 / +0.0176 | FAIL |
| R0 | egalitarian | 1x | -0.0099 / -0.0469 | -0.0382 / -0.0219 | -0.0128 / -0.0159 | -0.0203 / -0.0283 | FAIL |
| R0 | egalitarian | 100x | -0.0848 / -0.1372 | -0.0869 / -0.1365 | -0.0847 / -0.1419 | -0.0855 / -0.1385 | FAIL |
| R0 | egalitarian | 1000x | -0.0931 / -0.1470 | -0.0917 / -0.1476 | -0.0916 / -0.1477 | -0.0921 / -0.1474 | FAIL |
| R2 | wealth | 1x | +0.0271 / +0.0316 | +0.0185 / +0.0270 | +0.0247 / +0.0241 | +0.0234 / +0.0276 | FAIL |
| R2 | wealth | 100x | +0.0001 / +0.0005 | +0.0002 / +0.0002 | -0.0003 / +0.0018 | +0.0000 / +0.0008 | FAIL |
| R2 | wealth | 1000x | +0.0017 / +0.0040 | +0.0013 / +0.0040 | +0.0015 / +0.0038 | +0.0015 / +0.0039 | FAIL |
| R2 | egalitarian | 1x | +0.0184 / -0.0471 | -0.0241 / -0.0159 | -0.0301 / -0.0049 | -0.0119 / -0.0226 | FAIL |
| R2 | egalitarian | 100x | -0.0258 / -0.0416 | -0.0276 / -0.0418 | -0.0238 / -0.0428 | -0.0257 / -0.0420 | FAIL |
| R2 | egalitarian | 1000x | -0.1018 / -0.1634 | -0.1009 / -0.1641 | -0.1008 / -0.1644 | -0.1012 / -0.1639 | FAIL |
| R3 | wealth | 1x | +0.0032 / +0.0037 | +0.0043 / +0.0035 | +0.0028 / +0.0053 | +0.0035 / +0.0042 | FAIL |
| R3 | wealth | 100x | -0.0004 / -0.0004 | -0.0001 / +0.0008 | -0.0002 / +0.0020 | -0.0002 / +0.0008 | FAIL |
| R3 | wealth | 1000x | +0.0004 / +0.0011 | +0.0007 / +0.0021 | +0.0006 / +0.0020 | +0.0006 / +0.0017 | FAIL |
| R3 | egalitarian | 1x | -0.0454 / -0.0575 | -0.0242 / +0.0000 | -0.0246 / -0.0299 | -0.0314 / -0.0291 | FAIL |
| R3 | egalitarian | 100x | -0.0057 / -0.0090 | -0.0060 / -0.0113 | -0.0058 / -0.0093 | -0.0058 / -0.0098 | FAIL |
| R3 | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0050 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S1 | wealth | 1x | +0.0030 / +0.0019 | +0.0013 / +0.0010 | +0.0065 / +0.0081 | +0.0036 / +0.0037 | FAIL |
| S1 | wealth | 100x | -0.0004 / -0.0004 | -0.0001 / +0.0008 | -0.0002 / +0.0020 | -0.0002 / +0.0008 | FAIL |
| S1 | wealth | 1000x | +0.0004 / +0.0011 | +0.0007 / +0.0021 | +0.0006 / +0.0020 | +0.0006 / +0.0017 | FAIL |
| S1 | egalitarian | 1x | -0.0067 / -0.0045 | -0.0246 / -0.0582 | -0.0200 / -0.0257 | -0.0171 / -0.0294 | FAIL |
| S1 | egalitarian | 100x | -0.0057 / -0.0090 | -0.0060 / -0.0113 | -0.0058 / -0.0093 | -0.0058 / -0.0098 | FAIL |
| S1 | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0050 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S2 | wealth | 1x | +0.0255 / +0.0256 | +0.0185 / +0.0273 | +0.0256 / +0.0274 | +0.0232 / +0.0268 | FAIL |
| S2 | wealth | 100x | -0.0012 / -0.0029 | -0.0008 / -0.0028 | -0.0012 / -0.0017 | -0.0011 / -0.0025 | FAIL |
| S2 | wealth | 1000x | +0.0003 / +0.0008 | -0.0001 / +0.0003 | -0.0001 / -0.0001 | +0.0001 / +0.0003 | FAIL |
| S2 | egalitarian | 1x | +0.0336 / -0.0274 | -0.0369 / -0.0375 | -0.0192 / -0.0300 | -0.0075 / -0.0316 | FAIL |
| S2 | egalitarian | 100x | -0.0499 / -0.0834 | -0.0508 / -0.0805 | -0.0503 / -0.0870 | -0.0503 / -0.0836 | FAIL |
| S2 | egalitarian | 1000x | -0.1616 / -0.2658 | -0.1609 / -0.2672 | -0.1608 / -0.2677 | -0.1611 / -0.2669 | FAIL |
| S3a | wealth | 1x | +0.0001 / -0.0011 | +0.0004 / -0.0001 | +0.0042 / +0.0026 | +0.0016 / +0.0005 | FAIL |
| S3a | wealth | 100x | -0.0004 / -0.0004 | -0.0001 / +0.0008 | -0.0002 / +0.0020 | -0.0002 / +0.0008 | FAIL |
| S3a | wealth | 1000x | +0.0004 / +0.0011 | +0.0007 / +0.0021 | +0.0006 / +0.0020 | +0.0006 / +0.0017 | FAIL |
| S3a | egalitarian | 1x | +0.0132 / -0.0101 | -0.0142 / -0.0176 | +0.0091 / -0.0539 | +0.0027 / -0.0272 | FAIL |
| S3a | egalitarian | 100x | -0.0057 / -0.0090 | -0.0060 / -0.0113 | -0.0058 / -0.0093 | -0.0058 / -0.0098 | FAIL |
| S3a | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0050 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S3b | wealth | 1x | +0.0030 / +0.0043 | +0.0010 / +0.0007 | +0.0037 / +0.0086 | +0.0025 / +0.0045 | FAIL |
| S3b | wealth | 100x | -0.0004 / -0.0004 | -0.0001 / +0.0008 | -0.0002 / +0.0020 | -0.0002 / +0.0008 | FAIL |
| S3b | wealth | 1000x | +0.0004 / +0.0011 | +0.0007 / +0.0021 | +0.0006 / +0.0020 | +0.0006 / +0.0017 | FAIL |
| S3b | egalitarian | 1x | -0.0091 / -0.0607 | -0.0190 / -0.0361 | -0.0124 / -0.0330 | -0.0135 / -0.0433 | FAIL |
| S3b | egalitarian | 100x | -0.0057 / -0.0090 | -0.0060 / -0.0113 | -0.0058 / -0.0093 | -0.0058 / -0.0098 | FAIL |
| S3b | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0050 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S4a | wealth | 1x | -0.0004 / +0.0013 | -0.0000 / -0.0002 | +0.0028 / -0.0006 | +0.0008 / +0.0002 | FAIL |
| S4a | wealth | 100x | +0.0001 / +0.0006 | +0.0004 / +0.0005 | +0.0010 / +0.0019 | +0.0005 / +0.0010 | FAIL |
| S4a | wealth | 1000x | +0.0100 / +0.0184 | +0.0099 / +0.0169 | +0.0102 / +0.0175 | +0.0100 / +0.0176 | FAIL |
| S4a | egalitarian | 1x | -0.0779 / -0.1190 | -0.1063 / -0.0942 | -0.1073 / -0.1492 | -0.0972 / -0.1208 | FAIL |
| S4a | egalitarian | 100x | -0.0851 / -0.1374 | -0.0870 / -0.1369 | -0.0850 / -0.1421 | -0.0857 / -0.1388 | FAIL |
| S4a | egalitarian | 1000x | -0.0931 / -0.1470 | -0.0917 / -0.1476 | -0.0915 / -0.1477 | -0.0921 / -0.1474 | FAIL |
| S4b | wealth | 1x | +0.0014 / -0.0010 | +0.0027 / +0.0030 | +0.0017 / +0.0012 | +0.0019 / +0.0011 | FAIL |
| S4b | wealth | 100x | -0.0005 / -0.0007 | -0.0006 / -0.0007 | -0.0005 / +0.0015 | -0.0005 / -0.0000 | FAIL |
| S4b | wealth | 1000x | +0.0017 / +0.0040 | +0.0012 / +0.0040 | +0.0015 / +0.0037 | +0.0015 / +0.0039 | FAIL |
| S4b | egalitarian | 1x | +0.0226 / -0.0619 | -0.0164 / -0.0286 | -0.0173 / -0.0613 | -0.0037 / -0.0506 | FAIL |
| S4b | egalitarian | 100x | -0.0287 / -0.0464 | -0.0305 / -0.0484 | -0.0276 / -0.0480 | -0.0289 / -0.0476 | FAIL |
| S4b | egalitarian | 1000x | -0.1018 / -0.1633 | -0.1009 / -0.1641 | -0.1008 / -0.1644 | -0.1012 / -0.1639 | FAIL |
#### ΔGini by cell, node rule

| shape | mining | tx scale | seed 1696 (10 / 20 yr) | seed 2 | seed 3 | 3-seed mean | > 0.05 |
|---|---|---:|---|---|---|---|---|
| R0 | wealth | 1x | +0.0003 / +0.0010 | +0.0005 / -0.0022 | +0.0007 / +0.0011 | +0.0005 / -0.0000 | FAIL |
| R0 | wealth | 100x | +0.0155 / +0.0268 | +0.0143 / +0.0255 | +0.0169 / +0.0294 | +0.0156 / +0.0273 | FAIL |
| R0 | wealth | 1000x | +0.0425 / +0.0725 | +0.0426 / +0.0723 | +0.0431 / +0.0730 | +0.0427 / +0.0726 | FAIL |
| R0 | egalitarian | 1x | +0.0273 / +0.0311 | +0.0041 / -0.0054 | -0.0224 / -0.0264 | +0.0030 / -0.0002 | FAIL |
| R0 | egalitarian | 100x | +0.0068 / +0.0123 | +0.0086 / +0.0123 | +0.0056 / +0.0089 | +0.0070 / +0.0112 | FAIL |
| R0 | egalitarian | 1000x | +0.0003 / +0.0060 | +0.0005 / +0.0051 | -0.0009 / +0.0039 | -0.0000 / +0.0050 | FAIL |
| R2 | wealth | 1x | +0.0269 / +0.0327 | +0.0194 / +0.0276 | +0.0218 / +0.0200 | +0.0227 / +0.0268 | FAIL |
| R2 | wealth | 100x | +0.0149 / +0.0215 | +0.0131 / +0.0223 | +0.0149 / +0.0239 | +0.0143 / +0.0226 | FAIL |
| R2 | wealth | 1000x | +0.0138 / +0.0208 | +0.0136 / +0.0206 | +0.0133 / +0.0207 | +0.0136 / +0.0207 | FAIL |
| R2 | egalitarian | 1x | +0.0306 / -0.0271 | -0.0139 / -0.0492 | -0.0206 / -0.0146 | -0.0013 / -0.0303 | FAIL |
| R2 | egalitarian | 100x | -0.0230 / -0.0408 | -0.0202 / -0.0329 | -0.0229 / -0.0448 | -0.0220 / -0.0395 | FAIL |
| R2 | egalitarian | 1000x | -0.0264 / -0.0412 | -0.0264 / -0.0432 | -0.0279 / -0.0445 | -0.0269 / -0.0429 | FAIL |
| R3 | wealth | 1x | +0.0010 / +0.0023 | +0.0008 / -0.0010 | +0.0018 / +0.0032 | +0.0012 / +0.0015 | FAIL |
| R3 | wealth | 100x | -0.0005 / -0.0007 | -0.0001 / +0.0008 | -0.0003 / +0.0018 | -0.0003 / +0.0006 | FAIL |
| R3 | wealth | 1000x | +0.0004 / +0.0015 | +0.0006 / +0.0020 | +0.0005 / +0.0019 | +0.0005 / +0.0018 | FAIL |
| R3 | egalitarian | 1x | +0.0089 / -0.0073 | -0.0153 / -0.0005 | -0.0263 / -0.0401 | -0.0109 / -0.0160 | FAIL |
| R3 | egalitarian | 100x | -0.0058 / -0.0090 | -0.0061 / -0.0111 | -0.0058 / -0.0094 | -0.0059 / -0.0098 | FAIL |
| R3 | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0051 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S1 | wealth | 1x | +0.0000 / +0.0010 | -0.0000 / -0.0011 | -0.0015 / +0.0045 | -0.0005 / +0.0014 | FAIL |
| S1 | wealth | 100x | -0.0006 / -0.0009 | -0.0000 / +0.0007 | -0.0003 / +0.0017 | -0.0003 / +0.0005 | FAIL |
| S1 | wealth | 1000x | +0.0009 / +0.0020 | +0.0007 / +0.0021 | +0.0006 / +0.0020 | +0.0007 / +0.0020 | FAIL |
| S1 | egalitarian | 1x | +0.0150 / -0.0084 | -0.0198 / -0.0229 | -0.0177 / -0.0419 | -0.0075 / -0.0244 | FAIL |
| S1 | egalitarian | 100x | -0.0058 / -0.0090 | -0.0061 / -0.0111 | -0.0058 / -0.0094 | -0.0059 / -0.0098 | FAIL |
| S1 | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0051 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S2 | wealth | 1x | +0.0253 / +0.0296 | +0.0225 / +0.0359 | +0.0273 / +0.0238 | +0.0250 / +0.0298 | FAIL |
| S2 | wealth | 100x | +0.0151 / +0.0211 | +0.0135 / +0.0228 | +0.0142 / +0.0232 | +0.0142 / +0.0224 | FAIL |
| S2 | wealth | 1000x | +0.0142 / +0.0208 | +0.0137 / +0.0201 | +0.0137 / +0.0206 | +0.0139 / +0.0205 | FAIL |
| S2 | egalitarian | 1x | +0.0304 / -0.0154 | -0.0097 / -0.0285 | -0.0322 / -0.0231 | -0.0038 / -0.0223 | FAIL |
| S2 | egalitarian | 100x | -0.0468 / -0.0807 | -0.0434 / -0.0697 | -0.0431 / -0.0852 | -0.0444 / -0.0785 | FAIL |
| S2 | egalitarian | 1000x | -0.0502 / -0.0834 | -0.0508 / -0.0865 | -0.0519 / -0.0875 | -0.0509 / -0.0858 | FAIL |
| S3a | wealth | 1x | +0.0002 / -0.0011 | -0.0001 / +0.0009 | +0.0028 / +0.0026 | +0.0010 / +0.0008 | FAIL |
| S3a | wealth | 100x | -0.0005 / -0.0007 | -0.0001 / +0.0008 | -0.0003 / +0.0018 | -0.0003 / +0.0006 | FAIL |
| S3a | wealth | 1000x | +0.0004 / +0.0015 | +0.0006 / +0.0020 | +0.0005 / +0.0027 | +0.0005 / +0.0021 | FAIL |
| S3a | egalitarian | 1x | +0.0050 / -0.0496 | -0.0199 / -0.0330 | -0.0032 / -0.0622 | -0.0060 / -0.0482 | FAIL |
| S3a | egalitarian | 100x | -0.0058 / -0.0090 | -0.0061 / -0.0111 | -0.0058 / -0.0094 | -0.0059 / -0.0098 | FAIL |
| S3a | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0051 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S3b | wealth | 1x | +0.0033 / +0.0051 | +0.0019 / +0.0012 | +0.0033 / +0.0078 | +0.0028 / +0.0047 | FAIL |
| S3b | wealth | 100x | -0.0004 / -0.0003 | -0.0001 / +0.0009 | -0.0002 / +0.0017 | -0.0002 / +0.0008 | FAIL |
| S3b | wealth | 1000x | +0.0008 / +0.0019 | +0.0007 / +0.0018 | +0.0005 / +0.0018 | +0.0007 / +0.0018 | FAIL |
| S3b | egalitarian | 1x | -0.0092 / -0.0396 | -0.0120 / -0.0215 | -0.0087 / -0.0526 | -0.0100 / -0.0379 | FAIL |
| S3b | egalitarian | 100x | -0.0058 / -0.0090 | -0.0061 / -0.0111 | -0.0058 / -0.0094 | -0.0059 / -0.0098 | FAIL |
| S3b | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0051 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| S4a | wealth | 1x | +0.0003 / -0.0011 | +0.0004 / -0.0015 | +0.0007 / +0.0018 | +0.0005 / -0.0003 | FAIL |
| S4a | wealth | 100x | +0.0156 / +0.0266 | +0.0144 / +0.0255 | +0.0171 / +0.0291 | +0.0157 / +0.0271 | FAIL |
| S4a | wealth | 1000x | +0.0423 / +0.0723 | +0.0425 / +0.0721 | +0.0431 / +0.0729 | +0.0426 / +0.0725 | FAIL |
| S4a | egalitarian | 1x | +0.0199 / -0.0089 | -0.0022 / +0.0021 | -0.0099 / -0.0551 | +0.0026 / -0.0207 | FAIL |
| S4a | egalitarian | 100x | +0.0068 / +0.0127 | +0.0086 / +0.0123 | +0.0056 / +0.0088 | +0.0070 / +0.0113 | FAIL |
| S4a | egalitarian | 1000x | +0.0003 / +0.0060 | +0.0005 / +0.0051 | -0.0009 / +0.0039 | -0.0000 / +0.0050 | FAIL |
| S4b | wealth | 1x | +0.0186 / +0.0246 | +0.0101 / +0.0177 | +0.0105 / +0.0106 | +0.0131 / +0.0176 | FAIL |
| S4b | wealth | 100x | +0.0140 / +0.0209 | +0.0121 / +0.0214 | +0.0144 / +0.0236 | +0.0135 / +0.0220 | FAIL |
| S4b | wealth | 1000x | +0.0136 / +0.0207 | +0.0136 / +0.0204 | +0.0131 / +0.0205 | +0.0134 / +0.0206 | FAIL |
| S4b | egalitarian | 1x | +0.0057 / -0.0249 | -0.0237 / -0.0325 | +0.0079 / -0.0152 | -0.0034 / -0.0242 | FAIL |
| S4b | egalitarian | 100x | -0.0227 / -0.0407 | -0.0204 / -0.0329 | -0.0230 / -0.0447 | -0.0220 / -0.0394 | FAIL |
| S4b | egalitarian | 1000x | -0.0264 / -0.0412 | -0.0263 / -0.0432 | -0.0279 / -0.0444 | -0.0269 / -0.0429 | FAIL |

### 2.5 Fast-path validation

#### Fast-path validation (seed 1696): the S4 incremental-weight path at the node's window

| path | rule | mining | tx scale | default path ΔGini (10 / 20 yr) | fast path ΔGini (10 / 20 yr) | |Δ| max |
|---|---|---|---:|---|---|---:|
| R0 | 1790 | wealth | 1x | -0.0002 / -0.0010 | -0.0002 / -0.0010 | 0.0000 |
| R0 | 1790 | wealth | 100x | +0.0001 / +0.0006 | +0.0001 / +0.0007 | 0.0001 |
| R0 | 1790 | wealth | 1000x | +0.0100 / +0.0185 | +0.0100 / +0.0184 | 0.0001 |
| R0 | 1790 | egalitarian | 1x | -0.0099 / -0.0469 | -0.0100 / -0.0160 | 0.0310 |
| R0 | 1790 | egalitarian | 100x | -0.0848 / -0.1372 | -0.0848 / -0.1370 | 0.0002 |
| R0 | 1790 | egalitarian | 1000x | -0.0931 / -0.1470 | -0.0931 / -0.1470 | 0.0000 |
| R0 | node | wealth | 1x | +0.0003 / +0.0010 | +0.0003 / +0.0010 | 0.0000 |
| R0 | node | wealth | 100x | +0.0155 / +0.0268 | +0.0155 / +0.0267 | 0.0001 |
| R0 | node | wealth | 1000x | +0.0425 / +0.0725 | +0.0424 / +0.0725 | 0.0000 |
| R0 | node | egalitarian | 1x | +0.0273 / +0.0311 | +0.0221 / +0.0058 | 0.0254 |
| R0 | node | egalitarian | 100x | +0.0068 / +0.0123 | +0.0070 / +0.0126 | 0.0003 |
| R0 | node | egalitarian | 1000x | +0.0003 / +0.0060 | +0.0002 / +0.0060 | 0.0000 |
| R2 | 1790 | wealth | 1x | +0.0271 / +0.0316 | +0.0247 / +0.0276 | 0.0041 |
| R2 | 1790 | wealth | 100x | +0.0001 / +0.0005 | +0.0000 / +0.0003 | 0.0001 |
| R2 | 1790 | wealth | 1000x | +0.0017 / +0.0040 | +0.0017 / +0.0040 | 0.0000 |
| R2 | 1790 | egalitarian | 1x | +0.0184 / -0.0471 | +0.0342 / -0.0064 | 0.0407 |
| R2 | 1790 | egalitarian | 100x | -0.0258 / -0.0416 | -0.0258 / -0.0415 | 0.0000 |
| R2 | 1790 | egalitarian | 1000x | -0.1018 / -0.1634 | -0.1018 / -0.1633 | 0.0000 |
| R2 | node | wealth | 1x | +0.0269 / +0.0327 | +0.0268 / +0.0328 | 0.0001 |
| R2 | node | wealth | 100x | +0.0149 / +0.0215 | +0.0150 / +0.0217 | 0.0002 |
| R2 | node | wealth | 1000x | +0.0138 / +0.0208 | +0.0138 / +0.0208 | 0.0000 |
| R2 | node | egalitarian | 1x | +0.0306 / -0.0271 | +0.0296 / -0.0210 | 0.0061 |
| R2 | node | egalitarian | 100x | -0.0230 / -0.0408 | -0.0230 / -0.0407 | 0.0001 |
| R2 | node | egalitarian | 1000x | -0.0264 / -0.0412 | -0.0264 / -0.0412 | 0.0000 |

Largest fast-vs-default difference: 0.0407.

## 3. Axis 2: splitting (the #1761 grid)

The historical 192-point grid of the #1801 study's §4, under the same evaluator and premises (fee modes, capacity bounds and the inherited approximations; see the [evidence ledger](ct-compatible-lottery-selection.md#evidence-and-claim-reconciliation)). This is expected-value model evaluation, not production block execution. Shape-specific extensions:

- **S1:** every ticket (organic, whale, coinbase, payout) is bought at `base × w`, with `base = R / Σw` per block. The grid's organic outputs are 1x (`phi = 0`). The sensitivity table puts a share `phi` of organic tickets at 6x. A whale can always claim background (#1687); on 1e6-pico ticket outputs the #925 charge is negligible.
- **S2:** #1801's value-linear churn evaluator with `c × output sum` added to every transaction (the churn transaction's `x`; each organic transaction's two outputs), pooled like any fee. A unit test pins S2 at `c = 0` to the #1801 evaluator bit for bit.
- **S3a/S3b:** R3's evaluator. Routing demurrage away from the pool does not change R3's split neutrality (`price = R / T` whatever `R` contains), and settling on spend touches only the tag rule.
- **S4:** the #1761 current-code evaluator and the #1801 churn evaluator with the eligible life (and so the window count and the cap) set by the 30-day window. Unit tests pin both to the originals at the node's window.

### Splitting grid for the #1987 shapes (#1761 grid, 192 points)

Whale optimum per grid point, as in the #1801 study's §4 table. "Node" = spent outputs stay candidates; "reveal" = spent outputs leave the draw (#1790's rule). Uniform-ticket shapes have no node/reveal difference. S2 charges 10000 ppm of every transaction's output sum, pooled like any fee. S4 uses a 720..=518400-block window (517681 blocks of eligible life). Value-linear rows: the whale re-spends up to 5000000 BTH per block; organic outputs are worth 10 BTH each.

| shape | profitable (node) | profitable (reveal) | max net*/blk, node (BTH) | max net*/day, node (BTH) | reference net*/blk, node (BTH) | reference whale share, node |
|---|---:|---:|---:|---:|---:|---:|
| R0 current rules (reference) | 192/192 | 192/192 | +49.9975 | +863957 | +2.4660 | 98.7% |
| R3 ticket-price (reference) | 0/192 | 0/192 | +0.0000 | +0 | +0.0000 | 0.0% |
| S1 factor-price (whale at its factor) | 0/192 | 0/192 | +0.0000 | +0 | +0.0000 | 0.0% |
| S1 factor-price (whale claims background) | 0/192 | 0/192 | +0.0000 | +0 | +0.0000 | 0.0% |
| S2 churn-fee, R2 weight, c = 1% | 100/192 | 0/192 | +38.8675 | +671630 | +0.0000 | 0.0% |
| S3a/S3b (R3 lottery; demurrage side only) | 0/192 | 0/192 | +0.0000 | +0 | +0.0000 | 0.0% |
| S4a window-30d, uniform | 192/192 | 192/192 | +49.9975 | +863957 | +2.4660 | 98.7% |
| S4b window-30d, R2 weight | 192/192 | 0/192 | +49.9993 | +863988 | +2.4915 | 99.8% |

### S1: farming the factor subsidy

Profitable points out of 192 (node rule) when a share phi of organic tickets is at 6x and the rest at 1x. The #1761 grid's organic outputs are all 1x (phi = 0). "At its factor" prices the whale's tickets at the grid's whale factor (1x or 6x); "background" lets every whale claim 1x (#1687; the #925 charge on 1e6-pico ticket outputs is negligible).

| phi (6x share of organic tickets) | profitable, whale at its factor | max net*/blk (BTH) | profitable, whale claims background | max net*/blk (BTH) |
|---:|---:|---:|---:|---:|
| 0.00 | 0/192 | +0.0000 | 0/192 | +0.0000 |
| 0.05 | 94/192 | +2.7740 | 188/192 | +2.7740 |
| 0.20 | 96/192 | +8.5484 | 192/192 | +8.5484 |
| 0.50 | 96/192 | +15.1257 | 192/192 | +15.1257 |

### S2: churn fee needed to close the churn attack

Profitable points out of 192 under the node rule (spent outputs stay candidates), by ad-valorem fee on every transaction's output sum.

| churn fee (ppm of value moved) | churn fee (%) | R1 value-age | R2 inverse-factor |
|---:|---:|---:|---:|
| 0 | 0.00% | 192/192 | 192/192 |
| 100 | 0.01% | 168/192 | 168/192 |
| 1000 | 0.10% | 168/192 | 148/192 |
| 10000 | 1.00% | 128/192 | 100/192 |
| 30000 | 3.00% | 96/192 | 64/192 |
| 100000 | 10.00% | 64/192 | 36/192 |
| 300000 | 30.00% | 16/192 | 8/192 |
| 750000 | 75.00% | 0/192 | 0/192 |

Largest break-even fee over each epoch's 48 points (R2 weight, node rule):

| epoch | break-even churn fee (ppm) | (%) |
|---|---:|---:|
| 0 | 704225 | 70.42% |
| 1 | 574713 | 57.47% |
| 3 | 246305 | 24.63% |
| tail | 109040 | 10.90% |

**Reading it.**

- **S1's subsidy is farmable exactly when it exists.** With every organic ticket at 1x the price is R3's and nothing is profitable. Once 5% of organic tickets are at 6x, a 1x whale's manufactured tickets cost less than they win, and 94/192 points are profitable for a whale pricing at its grid factor. Since any whale can claim background, the number is 188/192. At 20% it is 192/192, worth up to +8.5 BTH per block. S1 is therefore either split-safe and inert, or redistributive and farmable. The m2 runs land it in the inert regime (mean ticket factor 1.03).
- **No churn fee an honest payer could bear closes S2.** Churning a small `x` wins `R·x / (x + O)` for a cost of `c·x`, where `R` is the payout per block and `O` the honest value weight created per block, so the break-even is about `R / O`. Over the grid that is 11% (tail) to 70% (epoch 0 with a 1,000-BTH backlog) of the value moved. At 1% churn stays profitable at 100/192 points. Burning the fee instead of pooling it does not change the break-even, which is set by the first BTH churned.
- **A longer window makes splitting worse, not better (S4).** A manufactured output keeps its share for longer, and the larger window count lifts the reward cap, so S4a is profitable at every point. S4b is split-safe only under #1790's rule, which needs the reveal.

## 4. Axis 3: privacy

The production routes (#1757 ports), chain, rings and seed of the #1801 study's §5. Per shape:

- **S1** reads output counts, the payout and each new output's claimed factor. All are public today, and none involves a ring. As for R3, a mandatory purchase publishes nothing new, while an opt-in purchase adds a public "ticketed" flag (§5.2 of the #1801 study: 9.5% ID at φ = 0.5 against 5%). One new incentive: since the price rises with the claimed factor, claiming background (#1687) becomes worth more. That is an economic effect, not a ring leak.
- **S2** needs no reveal under the node's rule: it reads output values, which are public today. Under confidential amounts the fee is public and grows with the hidden amount. A min-fee payer's fee then gives `V = (fee − base) / c` exactly, on every spend. Demurrage has the same structure, but there factor-1 coins (most transactions) have `k = 0` and leak nothing (CT-economics §3). A churn fee has `k = c > 0` on every spend. It is private under CT only if D1's mandatory fee quantization covers every spend, and then each fee still leaks its bucket. R2's weighted draw under CT also needs the #902 Path B ZK sort.
- **S3a** reads what the fee floor reads today, and only changes where the demurrage part goes.
- **S3b** removes information: outputs carry no cluster tags, so the #1756 tag fingerprint disappears from outputs. Because every output of a transaction is background, change is not distinguishable from payment. If settlement were applied to the payment output only, the tagged change output would stand out. The ring floor still reads ring members' tags, which after one hop exist only on coinbase and bridge-import outputs.
- **S4 under the node's rule** reads public ages only, as today: PASS. **S4 under #1790's rule** needs the claim-time key-image reveal of §5.1. A 30-day window changes only the real input's exposure before its spend; the per-output full-life win probability `q` does not depend on the window length. The table re-runs §5.1 with the 30-day window. The "10k window" columns reproduce §5.1 exactly (tested).

### S4: claim-reveal with a 30-day window

Same production routes, chain, rings (4000 per row) and seed (0x18019817) as the #1801 §5.1 table. The reveal is what #1790's rule needs (a winner reveals its key image; a spent output forfeits). The full-life win probability q = 1 - exp(-4/rho) does not depend on the window length; the real input's exposure before its spend does. Window 720..=518400 against the node's 720..=10000. ID = uniform guess over the surviving ring members, counting directly revealed rings (uniform = 5.0%).

| route | real age | rho | q | direct reveal, 10k window | direct reveal, 30-day | ID, 10k window | ID, 30-day | survivors (of 20), 30-day |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| node | 1000 | 5 | 55.1% | 2.5% | 0.1% | 13.4% | 11.2% | 9.5 |
| node | 1000 | 20 | 18.1% | 0.5% | 0.0% | 6.6% | 6.1% | 16.6 |
| node | 1000 | 200 | 2.0% | 0.1% | 0.0% | 5.2% | 5.1% | 19.6 |
| node | 30000 | 5 | 55.1% | 55.0% | 4.8% | 60.0% | 15.4% | 9.1 |
| node | 30000 | 20 | 18.1% | 17.9% | 1.1% | 23.0% | 7.2% | 16.4 |
| node | 30000 | 200 | 2.0% | 2.1% | 0.2% | 7.1% | 5.3% | 19.6 |
| node | 100000 | 5 | 55.1% | 55.0% | 14.1% | 60.0% | 23.7% | 8.3 |
| node | 100000 | 20 | 18.1% | 17.9% | 4.0% | 23.0% | 9.8% | 16.0 |
| node | 100000 | 200 | 2.0% | 2.1% | 0.3% | 7.1% | 5.4% | 19.6 |
| node | gamma-spend | 5 | 55.1% | 55.0% | 51.9% | 60.0% | 57.3% | 5.1 |
| node | gamma-spend | 20 | 18.1% | 17.9% | 16.9% | 23.0% | 22.0% | 13.9 |
| node | gamma-spend | 200 | 2.0% | 2.1% | 2.0% | 7.1% | 7.0% | 19.3 |
| cli | 1000 | 5 | 55.1% | 2.5% | 0.1% | 13.4% | 11.2% | 9.5 |
| cli | 1000 | 20 | 18.1% | 0.5% | 0.0% | 6.6% | 6.1% | 16.6 |
| cli | 1000 | 200 | 2.0% | 0.1% | 0.0% | 5.2% | 5.1% | 19.6 |
| cli | 30000 | 5 | 55.1% | 55.0% | 4.8% | 60.0% | 15.4% | 9.1 |
| cli | 30000 | 20 | 18.1% | 17.9% | 1.1% | 23.0% | 7.2% | 16.4 |
| cli | 30000 | 200 | 2.0% | 2.1% | 0.2% | 7.1% | 5.3% | 19.6 |
| cli | 100000 | 5 | 55.1% | 55.0% | 14.1% | 60.0% | 23.7% | 8.3 |
| cli | 100000 | 20 | 18.1% | 17.9% | 4.0% | 23.0% | 9.8% | 16.0 |
| cli | 100000 | 200 | 2.0% | 2.1% | 0.3% | 7.1% | 5.4% | 19.6 |
| cli | gamma-spend | 5 | 55.1% | 55.0% | 51.5% | 60.0% | 57.0% | 5.1 |
| cli | gamma-spend | 20 | 18.1% | 17.9% | 16.7% | 23.0% | 21.8% | 14.0 |
| cli | gamma-spend | 200 | 2.0% | 2.1% | 1.9% | 7.1% | 6.9% | 19.3 |
| web | 1000 | 5 | 55.1% | 2.5% | 0.1% | 13.4% | 11.2% | 9.5 |
| web | 1000 | 20 | 18.1% | 0.5% | 0.0% | 6.6% | 6.1% | 16.6 |
| web | 1000 | 200 | 2.0% | 0.1% | 0.0% | 5.2% | 5.1% | 19.6 |
| web | 30000 | 5 | 55.1% | 55.0% | 4.8% | 60.0% | 15.4% | 9.1 |
| web | 30000 | 20 | 18.1% | 17.9% | 1.1% | 23.0% | 7.2% | 16.4 |
| web | 30000 | 200 | 2.0% | 2.1% | 0.2% | 7.1% | 5.3% | 19.6 |
| web | 100000 | 5 | 55.1% | 55.0% | 14.1% | 60.0% | 23.7% | 8.3 |
| web | 100000 | 20 | 18.1% | 17.9% | 4.0% | 23.0% | 9.8% | 16.0 |
| web | 100000 | 200 | 2.0% | 2.1% | 0.3% | 7.1% | 5.4% | 19.6 |
| web | gamma-spend | 5 | 55.1% | 55.0% | 52.1% | 60.0% | 57.4% | 5.1 |
| web | gamma-spend | 20 | 18.1% | 17.9% | 16.7% | 23.0% | 21.8% | 14.0 |
| web | gamma-spend | 200 | 2.0% | 2.1% | 1.9% | 7.1% | 6.9% | 19.3 |

**Reading it.** A longer window sharply cuts the direct reveal of inputs spent within a few days, because they have used up only a small part of a 30-day window. At age 30,000 blocks (about 1.7 days) and ρ = 5 the rate falls from 55% to 4.8%. For the node's gamma spend-age distribution, the typical case, it barely moves: 51.5–52.1% against 55% at ρ = 5. Most sampled spend ages lie past 30 days, so the real input still had its whole window to win. Every decoy is still eliminated at rate q. The reveal form of S4b therefore fails privacy as R1/R2 do.

## 5. Node-side complexity and confidential amounts

Each estimate is the work needed for the node to enforce the rule. A wallet change and a protocol bump are implied throughout.

| id | public amounts (today) | what it needs to be split-safe | under CT (ADR 0006, #902) | overall |
|---|---|---|---|---|
| S1 | R3's machinery (a consensus fee term per eligible output, a full eligible-output counter in ledger state, mandatory purchase for every output ≥ `min_utxo_value` including coinbase and payout outputs, burn accounting), plus a factor-weighted counter for the price base. The per-output factor is the transaction's claimed factor, which the fee floor already computes. Medium. | nothing on the #1761 grid; but its subsidy is farmable whenever tickets above 1x exist (§3), and no rule is known that keeps the subsidy and closes that | the claimed factor needs D2 (octave-bucket proof or value-free provenance factor); the price is then a public function of a public bucket | **medium**, dominated by R3; a farmable subsidy on top |
| S2 | R2 is a config switch (`SelectionMode::ClusterWeighted`); the churn fee is one more term in `consensus_fee_floor` on the output sum. Low. | a fee of 11–70% of value moved, or the spent set (a reveal or a ZK non-membership proof, as R1/R2) | the fee fits Gadget 1 (`d ≥ c·V`, committed, one-sided Bulletproof), but every fee then leaks the amount unless D1 quantizes it; the weighted draw needs the #902 Path B sort | **very high** (low if splitting and the amount leak are accepted) |
| S3a | R3's machinery, plus routing the demurrage part of each fee (`consensus_fee_breakdown` already separates it) to the coinbase: coinbase validation must accept the reward plus 80% of the previous block's demurrage. Medium. | none beyond R3 | the demurrage part is `fee − base`, both public: unchanged | **medium** |
| S3b | S3a, plus a validation rule that outputs carry empty tags and pay the #925 charge from the ring floor (which exists). It retires tag propagation, so it interacts with ADR 0007's bridge-import tagging (an import is settled at its first spend) and with #1685 (the epoch-0 holiday makes settling free). Medium. | none beyond R3 | Gadget 1 already covers the #925 charge; D2 is then needed only for coinbase and import outputs' factors | **medium**, coupled to #1685 and ADR 0007 |
| S4a | `circulation_window` 10,000 → 518,400; `lottery_candidates` then walks a 52× wider age band of `utxo_db` each block before its 10,000-candidate truncation. Low (a constant), with a per-block scan cost. | not split-safe (192/192) | unchanged (uniform) | **low**, but makes splitting worse |
| S4b | S4a plus the `ClusterWeighted` switch. Low. | the spent set, as R1/R2 | the #902 Path B sort | **very high** |

## 6. Ranked recommendation (no decision stated)

This orders the options by how much of the operator's requirement each could carry. It is not a decision.

1. **R3 (tickets priced at their expected payout), the ruling's fallback.** Nothing in this study displaces it. It is still the only measured option that passes splitting and privacy at medium complexity and works unchanged under confidential amounts. It does not redistribute (same-commit best +0.0042), so adopting it means the lottery's redistributive role is retired, consistent with the #1800 restatement.
2. **S3a or S3b, layered on R3**, only if the operator wants demurrage proceeds to leave the lottery pool for a reason other than Gini. They keep R3's two passing bars and cost a medium node change, but their best cells are within 0.003 of R3's (+0.0045 / +0.0047 for S3b against +0.0042 / +0.0018). S3a is 0.03 below R3 in the egalitarian 1x cell at 20 years. S3b has one side benefit: it removes cluster tags from outputs (the #1756 fingerprint). It is coupled to #1685 (with the epoch-0 holiday in force, settling is free in epoch 0) and to ADR 0007.
3. **S1 (factor-dependent ticket price).** It is R3 in practice (mean ticket factor 1.03, Gini within noise of R3), and its subsidy becomes farmable the moment it would have an effect (≥ 5% of tickets above 1x: 188–192/192). R3 dominates it.
4. **S2 (R2 weight + churn fee).** It has the highest Gini of the four shapes (+0.0298), still below the bar and barely above R2's +0.0268. No honest-tolerable fee closes the churn attack (break-even 11–70%). It taxes merchants 10× what it taxes whales per unit of wealth, and under CT its fee would reveal every amount.
5. **S4b, then S4a (30-day window).** S4b moves value-weighted tickets to the whales and stays split-unsafe on the node. S4a reproduces R0 and makes splitting strictly worse.

Across the four shapes, the measurements extend the #1801 study's §7 pattern to the families §7.1 listed. A ticket price that is fair cannot redistribute, and one that subsidizes can be farmed. A value weight can be churned unless spent value stops counting, and a fee large enough to stop churn would stop commerce. The demurrage side has nothing durable to tax, because the cluster factor cannot see mined wealth (#1790 cause 2). A longer window moves the lottery towards holdings, which are the whales'. That is still a statement about the shapes measured, under the current fee structure, tag design and harness approximations, not a proof about every possible rule. Two directions remain unmeasured: a factor input that can see mined wealth (the D2 provenance-factor work, #902) and redistribution outside the lottery and demurrage altogether.

**No decision is stated here. Ratification is the operator's** (#1837).

## 7. Reproduce

Seeds 1696, 2 and 3 for the m2 runs; the privacy seed `0x18019817` and 4,000 rings per row (the #1801 values). Everything else is deterministic.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p bth-cluster-tax --features cli
B=target/release/cluster-tax-sim
$B redistribution-shapes --part splitting      # §3 (golden-checked in CI)
$B redistribution-shapes --part privacy        # §4 (golden-checked in CI)
# §2: one run per call, one JSON line per snapshot (10 and 20 yr)
$B redistribution-shapes --part m2 --shape {factor-price,churn-fee,demurrage-miners,settle-miners,window-30d,window-30d-value,stock} \
   --mining {wealth,egalitarian} --seed {1696,2,3} --tx-scale {1,100,1000} --run {on,off} [--spent-eligible] \
   [--churn-fee-ppm N] [--window MIN:MAX] [--whale optimal-decoy]
# The whole §2 campaign (384 runs) and its tables:
python3 scripts/research/redistribution_shapes_jobs.py /tmp/shapes-runs > /tmp/jobs.tsv
tr '\n' '\0' < /tmp/jobs.tsv | xargs -0 -n1 -P 22 scripts/research/redistribution_shapes_run_one.sh
python3 scripts/research/redistribution_shapes_tables.py /tmp/shapes-runs
# or re-derive the §2 tables from the committed run record:
python3 scripts/research/redistribution_shapes_tables.py docs/research/generated/redistribution-shapes-m2.jsonl
cargo test --release -p bth-cluster-tax --features cli --lib redistribution_shapes
cargo test --release -p bth-cluster-tax --features cli --lib baseline_reproduces_1790   # R0 still = #1790, bit for bit
```

The campaign is 384 runs: 6 shape configurations, plus same-commit R0, R2, R3 and the counterfactual, each over 2 mining models × 3 densities × 3 seeds × 2 candidate rules, and 24 fast-path validation runs at seed 1696. It took about 20.5 core-hours (median 134 s, longest 646 s per run) and 56 minutes of wall time on 22 workers of a shared 28-core Apple M-series. Every run's output is in [`generated/redistribution-shapes-m2.jsonl`](generated/redistribution-shapes-m2.jsonl).

**Scope of this grid, plainly.** The Gini grid is the #1801 study's full main grid for every shape (honest whale), not a reduced one. It does not include the following. None is likely to change a verdict, given margins of 0.02 to 0.05 below the bar, but each is a gap:

- **The optimal-decoy whale.** The #1801 study ran it at 1x only and found no material change (its §3.5). Add `--whale optimal-decoy`.
- **More than one churn fee in the Gini runs.** S2 ran at 1% only. The splitting table gives the full fee curve. Add `--churn-fee-ppm N`.
- **Other window lengths.** S4 ran at 30 days only. Add `--window 720:N` to any shape.
- **A real chain without a genesis endowment.** Inherited from #1790. It removes the only durable high-factor provenance, which leans against S1 and S3.

## 8. Approximations, and which way they lean

Everything in the #1790 record and the #1801 study's §10 still applies. The new ones:

- **S1's price base lags one step** (720 blocks), as R3's price does. The factor weight is fixed at purchase, from the output's tags and the current W. W only grows, so the weight is at most the factor at the draw. Neutral to slightly in the subsidy's favour.
- **S2 charges the payment, not the output sum.** A node would charge the output sum, including change. The always-consolidating wallet would then pay `c` on its whole position at every spend. Charging only the payment approximates a coin-selecting wallet, and *leans in the shape's favour* (less honest burden).
- **S3a pays the demurrage dividend in the step after it is collected,** spread evenly over that step's blocks. That is up to 720 blocks of lag, negligible over 20 years.
- **S3b starts at epoch 1** (see §1). *In the shape's favour*: settling in epoch 0 would drop every tag for free.
- **S4's incremental-weight path** draws a winner's output by inverse CDF over running sums instead of the default path's multinomial. It is the same distribution but a different random-number order, so runs are statistically equivalent, not bit-identical (§2.5). The node draws 4 winners from a uniform subset of at most 10,000 candidates, which the harness's owner-level multinomial approximates, as before.
- **The splitting grid's premises** (stationary economy, fixed 10-BTH organic outputs, the inherited fee modes and capacities) are the #1761 study's. The S1 sensitivity varies only the share of 6x organic tickets.
