# Redistribution Redesign Study (#1801)

**Status:** sim-only study. No node, consensus, wallet or constant change.
**Date:** 2026-11-01.
**Commissioned by:** the operator ruling of 2026-11-01 on #1727 and #1682 (option 1): restate the claim now (#1800) and run a sim-only redesign study before any constant or consensus rule moves.
**Inputs:** #1790 (`m2-production`, [m2-production-verdicts.md](m2-production-verdicts.md)), #1761 (the historical stationary expected-value/candidate-window splitting study, [ct-compatible-lottery-selection.md §9.1.1](ct-compatible-lottery-selection.md)), #1757 (the production decoy routes, `cluster-tax/src/simulation/PRIVACY_ANALYSIS.md`).

> **No decision is stated here. Ratification is the operator's.** This document reports measurements and ranks the candidates against the three axes the ruling set. Choosing one, combining them, or restating the redistribution criterion instead is the operator's call.

## Verdict

None of the five candidates passes all three axes, and none passes the Gini axis in any form measured. Each row is one rule, and every cell in a row measures that rule. The splitting column evaluates a model of node eligibility/cost rules, not actual production block transitions; “node” in those preserved table labels has that meaning. See the [source and conclusion ledger](ct-compatible-lottery-selection.md#evidence-and-claim-reconciliation). R4 and R5 have two forms that differ on privacy, so each form has its own row:

| id | candidate | Gini (> 0.05, every cell) | Splitting (#1761 grid, node rules) | Privacy | Node-side complexity |
|---|---|---|---|---|---|
| R0 | current rules (baseline) | **FAIL** (0/6 cells; best mean +0.0176) | **FAIL** (192/192 profitable) | PASS (reads nothing new) | — |
| R1 | tickets ∝ value × age | **FAIL** (0/6 cells; best mean +0.0317) | **FAIL** on the node (192/192, by re-spending capital); PASS (0/192) only with a spend reveal | **FAIL** as specified: needs the hidden spent set (a claim-time reveal, §5.1, or a ZK non-membership proof); under CT, also a ZK sort | low on public amounts; very high to make it split-safe or CT-compatible |
| R2 | tickets ∝ value × (7 − factor) | **FAIL** (0/6 cells; best mean +0.0265) | **FAIL** on the node (192/192); PASS only with a spend reveal | **FAIL**, as R1 | lowest (the `ClusterWeighted` kernel already exists); very high for the same reasons as R1 |
| R3 | uniform tickets priced at their expected payout | **FAIL** (0/6 cells; best mean +0.0033) | **PASS** (0/192) | PASS if purchase is mandatory; a small leak if opt-in | medium |
| R4 | tag decay per block, allowance keyed to the real input's age | **FAIL** (1/6 cells; best mean +0.0780) | **FAIL** (192/192, lottery unchanged) | **FAIL** (93–100% of rings identified at real ages ≥ 30,000 blocks, §5.3) | medium |
| R4p | tag decay per block, allowance keyed to the ring's youngest member (tags effectively never decay) | **FAIL** (0/6 cells; best mean +0.0163) | **FAIL** (192/192, lottery unchanged) | PASS (the allowance is a function of public ring ages) | medium |
| R5 | R2 + R4 | **FAIL** (0/6 cells; best mean +0.0290) | **FAIL** on the node; PASS only with a spend reveal | **FAIL**, as R2 and R4 | as R2 plus R4 |
| R5p | R2 + R4p | **FAIL** (0/6 cells; best mean +0.0405) | **FAIL** on the node; PASS only with a spend reveal | **FAIL**, as R2 | as R2 plus R4 |

"Gini" is the #1790 bar: the mechanism's own effect (ΔGini, off − on) above 0.05, as a 3-seed mean (seeds 1696, 2, 3), at both 10 and 20 years, in each of the 6 cells (wealth or egalitarian mining × 1x, 100x or 1000x transaction density), with the honest whale, under #1790's rule (spent outputs leave the draw). A "cell" passes only if both horizons pass. §3.2 repeats R0–R3 under the node's rule. R4 and R5 are §3.1; R4p and R5p are §3.1.1.

The best Gini result any candidate reaches is +0.0780 (R4, wealth mining, 1000x, 20-year 3-seed mean). It clears 0.05 in that one cell, but R4 fails the other five, is not private, and leaves the splitting hole open. Its private form, R4p, passes no cell.

No configuration in the study reached the bar, including those that gave up privacy or split-neutrality. Giving up an axis bought nothing measurable on Gini. Value-linear weight with the spend reveal (not private) peaks at +0.0317. The node's own rule (spent outputs stay eligible, not split-neutral) peaks at +0.0726 at one horizon in one cell. §7 sets out the pattern across the candidate families studied. That is an argument over those families, under the current fee structure and tag design, not a proof about every possible lottery rule; §7.1 lists shapes it does not cover.

## 1. Why these candidates

The ruling names four measured failure mechanisms. Each candidate targets one or more of them:

| mechanism (#1790 / #1761) | candidates that target it | how |
|---|---|---|
| tickets track hashpower and output count | R1, R2 | weight by value, so a 50-BTH coinbase or a dust split counts for its value, not as one ticket |
| frequent spenders lose their tickets | R1, R2 | an output's weight is its value, so whoever's large outputs sit in the window wins, whatever the output count |
| tag decay erases demurrage at high density | R4, R5 | decay is keyed to elapsed blocks, so 1,000 hops a year decay no faster than 12 |
| splitting is profitable | R1, R2, R3 | value-linear weight (a split adds no weight) or a ticket price equal to the expected payout (a manufactured ticket earns nothing) |

R5 combines the progressive tilt (R2) with decay-proof tags (R4), because R2's tilt only works if factors survive (#1790 cause 2: every factor is 1.000x by year 10).

**Candidates considered and not built:**

- **A per-cluster weight cap** (the issue's "capped per cluster"). Every coinbase starts a fresh cluster, and a holder can claim background tags at any time (#1687). A per-cluster cap therefore binds only on holders who keep honest tags, so it taxes honesty. Enforcing it also requires the node to sum weight across outputs by their plaintext tag, which is the #1756 linkage the privacy work is trying to remove. R2 keeps the cluster signal per output instead.
- **A per-output value cap.** It is evaded by splitting into outputs at the cap. Splitting then restores value-proportional weight, so the cap changes nothing for a strategic holder.
- **Inverse-fee or circulation weights.** #902 (§4.3 of the CT lottery memo) already showed that these are Sybil-broken. A per-coin inverse signal is a term the holder multiplies for free.
- **Raising the demurrage rate, the lottery share or the per-transaction floor.** The ruling rejected these levers.

## 2. A finding that changes the question: the node cannot drop spent outputs

`m2-production` (#1790) removes a spent output from the lottery window. Its record says "On the node, candidates come only from the UTXO set." That sentence is true, but the node's UTXO set never loses a ring-spent output:

- `Ledger::write_block` (`botho/src/ledger/store/writer.rs`) records each CLSAG input's **key image** and adds the new outputs. It deletes nothing from `utxo_db`. It cannot: the node does not know which of the 20 ring members is the real input.
- `lottery_candidates` (`botho/src/ledger/store/validation.rs`) walks `utxo_db` and filters only by age (720..=10,000) and value (≥ 1e6 pico). It has no spent check.

So **on the node, a spent output stays a lottery candidate until it ages out of the window, and a win still pays its owner** (the payout is a new output to the winner's stealth target). #1790's first version modelled exactly this. Its "correction" removed it, which moved seed 1696 egalitarian/honest at 10 years from +0.0273 to −0.0099. The study's node-rule runs below reproduce +0.0273.

That finding matters on every axis:

- **Gini.** The node-rule baseline differs from #1790's published baseline. The study reports both (§3.2). This study's runs use the #1790 rule unless marked (`redistribution-redesign --spent-eligible` selects the node rule), so R0 reproduces #1790 exactly as the ruling requires.
- **Splitting.** Under a value-linear weight, the modeled node eligibility rule lets a holder re-spend the same capital every block and mint fresh weight each time, because the spent copies keep counting. The historical stationary splitting grid finds this "churn" attack profitable at every point (§4); this is model evaluation, not production transition execution.
- **Privacy.** "Spent outputs leave the draw" can only be enforced if the winner proves its output is unspent. That means either revealing the key image or a zero-knowledge non-membership proof against the key-image set. §5.1 measures the reveal.

#1808 corrected #1790's record and made `m2-production` default to the node's rule (`--candidate-rule 1790` reproduces #1790; the `redistribution-redesign` subcommand is unchanged).

## 3. Axis 1: Gini (m2-production, #1790 harness)

Each candidate is a pair of switches on `m2_production.rs` (`TicketRule` × `TagDecay`, plus `spent_eligible`). Everything else is #1790 unchanged: the population, emission, fees, cluster rules, the 720-block aggregation, the honest whale, and the counterfactual (the same seed with no lottery share, fees burned and no demurrage). R0 is the default, and a unit test pins it bit for bit to the #1790 harness (`baseline_reproduces_1790_bit_for_bit`). R0's 36 published density cells (§"Transaction-density sensitivity" of the #1790 record) are reproduced to the fourth decimal by this study's runs.

How each candidate is modelled:

- **R1 value-age.** An eligible output's weight is its value × its age in blocks. Within an owner, wins go to entries in proportion to weight.
- **R2 inverse-factor.** Weight = value × (7 − factor), the `SelectionMode::ClusterWeighted` kernel (a unit test checks it against `LotteryCandidate::weight`). The factor is taken when the output enters the window, from its tags and the current W. W only grows, so this slightly overweights high-W outputs.
- **R3 ticket-price.** Uniform draw. An output of at least `min_utxo_value` is eligible only if it buys its ticket at creation, and the price is burned. The price is set once per step: the previous step's total lottery payout divided by the number of tickets bought in that step (`p = R / T`). A ticket stays eligible for 9,281 blocks, about 12.9 steps, so in steady state that price equals a ticket's expected payout. Wallets buy rationally: not for an output worth no more than the price, and, under #1790's rule (where a spent ticket stops counting), not for an output whose owner's median holding time is shorter than the 10,000-block window. Coinbase and payout outputs pay too.
- **R4 time-decay, keyed to the real input's age (`--decay-key real`, the default).** The honest wallet decays tags by `0.95^(elapsed / 525,600)`, where elapsed is the number of blocks since the position's last spend, instead of 5% per hop. The reference interval is the 1x whale's mean time between spends, so the 1x whale decays about as it does now. Pricing semantics are unchanged: the gap from the undecayed inputs is still charged as a #925 downgrade. **This is the non-private form of R4.** For the node to check it, the allowance must be keyed to the real input's age, and §5.3 shows that the resulting tag weight identifies 93–100% of rings at real ages of 30,000 blocks and more. Every row labelled "R4" or "R5" in §3.1–§3.5 measures this form.
- **R4p time-decay, keyed to the ring's youngest member (`--decay-key youngest`).** The private form of R4 (§5.3): the allowance is a function of the youngest ring member's age, which is public. The harness models it at its floor, a youngest member at the 10-block decoy minimum (`MIN_DECOY_AGE_BLOCKS`), so each spend may decay tags by at most `1 − 0.95^(10 / 525,600)`, about 1e-6. In practice R4p means tags essentially never decay. A ring whose youngest member is older allows more decay, up to R4 itself when the real input is the youngest member, so R4p and R4 bracket the private rule's decay allowance. R5p is R2's weight with R4p's decay. §3.1.1 reports both.
- **R5** = R2's weight + R4's decay.

### 3.1 Main matrix (#1790 candidate rule: spent outputs leave the draw)

ΔGini caused by the mechanism (off − on), honest whale, consensus fees, 20-year runs with snapshots at 10 and 20 years. The off run for R4 and R5 uses R4's decay; the others use the R0 off run. (Without demurrage the decay schedule only moves size fees, which total under 2 BTH over 20 years.)

| variant | mean ΔGini range, wealth | mean ΔGini range, egalitarian | best 3-seed mean | cells passing |
|---|---|---|---|---|
| R0 baseline | -0.0006 to +0.0176 | -0.1474 to -0.0203 | +0.0176 | 0/6 |
| R1 value-age | -0.0006 to +0.0317 | -0.1665 to -0.0008 | +0.0317 | 0/6 |
| R2 inverse-factor | -0.0000 to +0.0265 | -0.1639 to -0.0108 | +0.0265 | 0/6 |
| R3 ticket-price | -0.0002 to +0.0033 | -0.0338 to -0.0051 | +0.0033 | 0/6 |
| R4 time-decay | -0.0107 to +0.0780 | -0.6038 to +0.0059 | +0.0780 | 1/6 |
| R5 inverse-factor+time-decay | -0.0161 to +0.0290 | -0.6386 to -0.0034 | +0.0290 | 0/6 |

| variant | mining | tx scale | seed 1696 (10 / 20 yr) | seed 2 | seed 3 | 3-seed mean | > 0.05 |
|---|---|---:|---|---|---|---|---|
| R0 baseline | wealth | 1x | -0.0002 / -0.0010 | +0.0004 / -0.0013 | +0.0001 / +0.0004 | +0.0001 / -0.0006 | FAIL |
| R0 baseline | wealth | 100x | +0.0001 / +0.0006 | +0.0003 / +0.0002 | +0.0013 / +0.0024 | +0.0006 / +0.0010 | FAIL |
| R0 baseline | wealth | 1000x | +0.0100 / +0.0185 | +0.0099 / +0.0168 | +0.0102 / +0.0175 | +0.0100 / +0.0176 | FAIL |
| R0 baseline | egalitarian | 1x | -0.0099 / -0.0469 | -0.0382 / -0.0219 | -0.0128 / -0.0159 | -0.0203 / -0.0283 | FAIL |
| R0 baseline | egalitarian | 100x | -0.0848 / -0.1372 | -0.0869 / -0.1365 | -0.0847 / -0.1419 | -0.0855 / -0.1385 | FAIL |
| R0 baseline | egalitarian | 1000x | -0.0931 / -0.1470 | -0.0917 / -0.1476 | -0.0916 / -0.1477 | -0.0921 / -0.1474 | FAIL |
| R1 value-age | wealth | 1x | +0.0268 / +0.0333 | +0.0190 / +0.0265 | +0.0307 / +0.0352 | +0.0255 / +0.0317 | FAIL |
| R1 value-age | wealth | 100x | -0.0007 / -0.0004 | -0.0006 / -0.0020 | -0.0005 / +0.0007 | -0.0006 / -0.0005 | FAIL |
| R1 value-age | wealth | 1000x | +0.0016 / +0.0042 | +0.0014 / +0.0039 | +0.0014 / +0.0037 | +0.0015 / +0.0040 | FAIL |
| R1 value-age | egalitarian | 1x | +0.0244 / -0.0136 | -0.0176 / +0.0032 | -0.0091 / +0.0076 | -0.0008 / -0.0009 | FAIL |
| R1 value-age | egalitarian | 100x | -0.0440 / -0.0732 | -0.0444 / -0.0716 | -0.0448 / -0.0757 | -0.0444 / -0.0735 | FAIL |
| R1 value-age | egalitarian | 1000x | -0.1035 / -0.1660 | -0.1025 / -0.1665 | -0.1024 / -0.1669 | -0.1028 / -0.1665 | FAIL |
| R2 inverse-factor | wealth | 1x | +0.0273 / +0.0293 | +0.0185 / +0.0265 | +0.0246 / +0.0236 | +0.0234 / +0.0265 | FAIL |
| R2 inverse-factor | wealth | 100x | +0.0000 / +0.0001 | +0.0001 / +0.0001 | -0.0002 / +0.0019 | -0.0000 / +0.0007 | FAIL |
| R2 inverse-factor | wealth | 1000x | +0.0017 / +0.0040 | +0.0013 / +0.0040 | +0.0015 / +0.0038 | +0.0015 / +0.0039 | FAIL |
| R2 inverse-factor | egalitarian | 1x | +0.0210 / -0.0485 | -0.0264 / -0.0529 | -0.0270 / -0.0362 | -0.0108 / -0.0459 | FAIL |
| R2 inverse-factor | egalitarian | 100x | -0.0258 / -0.0416 | -0.0276 / -0.0418 | -0.0238 / -0.0428 | -0.0257 / -0.0420 | FAIL |
| R2 inverse-factor | egalitarian | 1000x | -0.1018 / -0.1634 | -0.1009 / -0.1641 | -0.1008 / -0.1644 | -0.1012 / -0.1639 | FAIL |
| R3 ticket-price | wealth | 1x | +0.0032 / +0.0034 | +0.0036 / +0.0013 | +0.0028 / +0.0053 | +0.0032 / +0.0033 | FAIL |
| R3 ticket-price | wealth | 100x | -0.0004 / -0.0004 | -0.0001 / +0.0008 | -0.0002 / +0.0020 | -0.0002 / +0.0008 | FAIL |
| R3 ticket-price | wealth | 1000x | +0.0004 / +0.0011 | +0.0007 / +0.0021 | +0.0006 / +0.0020 | +0.0006 / +0.0017 | FAIL |
| R3 ticket-price | egalitarian | 1x | -0.0366 / -0.0425 | -0.0273 / -0.0290 | -0.0246 / -0.0299 | -0.0295 / -0.0338 | FAIL |
| R3 ticket-price | egalitarian | 100x | -0.0057 / -0.0090 | -0.0060 / -0.0113 | -0.0058 / -0.0093 | -0.0058 / -0.0098 | FAIL |
| R3 ticket-price | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0050 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |
| R4 time-decay | wealth | 1x | +0.0004 / -0.0003 | -0.0008 / -0.0025 | +0.0006 / -0.0005 | +0.0001 / -0.0011 | FAIL |
| R4 time-decay | wealth | 100x | -0.0102 / -0.0134 | -0.0109 / -0.0117 | -0.0084 / -0.0071 | -0.0099 / -0.0107 | FAIL |
| R4 time-decay | wealth | 1000x | +0.0726 / +0.0780 | +0.0725 / +0.0781 | +0.0711 / +0.0779 | +0.0721 / +0.0780 | PASS |
| R4 time-decay | egalitarian | 1x | +0.0051 / -0.0109 | +0.0167 / -0.0096 | -0.0042 / +0.0161 | +0.0059 / -0.0015 | FAIL |
| R4 time-decay | egalitarian | 100x | -0.5925 / -0.4892 | -0.6102 / -0.4981 | -0.6088 / -0.5014 | -0.6038 / -0.4962 | FAIL |
| R4 time-decay | egalitarian | 1000x | -0.5531 / -0.4550 | -0.5507 / -0.4565 | -0.5518 / -0.4569 | -0.5519 / -0.4561 | FAIL |
| R5 inverse-factor+time-decay | wealth | 1x | +0.0240 / +0.0273 | +0.0186 / +0.0323 | +0.0295 / +0.0274 | +0.0240 / +0.0290 | FAIL |
| R5 inverse-factor+time-decay | wealth | 100x | -0.0123 / -0.0186 | -0.0128 / -0.0166 | -0.0101 / -0.0132 | -0.0117 / -0.0161 | FAIL |
| R5 inverse-factor+time-decay | wealth | 1000x | +0.0086 / +0.0144 | +0.0118 / +0.0164 | +0.0158 / +0.0194 | +0.0121 / +0.0167 | FAIL |
| R5 inverse-factor+time-decay | egalitarian | 1x | +0.0203 / +0.0052 | -0.0256 / -0.0521 | -0.0049 / +0.0080 | -0.0034 / -0.0130 | FAIL |
| R5 inverse-factor+time-decay | egalitarian | 100x | -0.6205 / -0.4395 | -0.6336 / -0.4516 | -0.6336 / -0.4553 | -0.6292 / -0.4488 | FAIL |
| R5 inverse-factor+time-decay | egalitarian | 1000x | -0.6385 / -0.5389 | -0.6379 / -0.5408 | -0.6393 / -0.5410 | -0.6386 / -0.5402 | FAIL |

### 3.1.1 The private form of R4 and R5 (decay keyed to the ring's youngest member)

The §3.1 rows for R4 and R5 measure decay keyed to the real input's age, which §5.3 shows is not private. These rows measure R4p and R5p, the private forms (§3, modelled at the 10-block floor, so tags essentially never decay). Same harness, seeds and cells; #1790's rule; the off run uses the same decay.

| variant | mining | tx scale | seed 1696 (10 / 20 yr) | seed 2 | seed 3 | 3-seed mean | > 0.05 |
|---|---|---:|---|---|---|---|---|
| R4p time-decay (youngest-keyed) | wealth | 1x | +0.0000 / +0.0011 | -0.0001 / -0.0015 | -0.0011 / +0.0001 | -0.0004 / -0.0001 | FAIL |
| R4p time-decay (youngest-keyed) | wealth | 100x | -0.0061 / -0.0652 | -0.0079 / -0.0595 | -0.0052 / -0.0537 | -0.0064 / -0.0594 | FAIL |
| R4p time-decay (youngest-keyed) | wealth | 1000x | +0.0166 / -0.0654 | +0.0160 / -0.0774 | +0.0165 / -0.0767 | +0.0163 / -0.0731 | FAIL |
| R4p time-decay (youngest-keyed) | egalitarian | 1x | -0.1546 / -0.0868 | -0.1504 / -0.0790 | -0.1680 / -0.0952 | -0.1577 / -0.0870 | FAIL |
| R4p time-decay (youngest-keyed) | egalitarian | 100x | -0.1936 / -0.2630 | -0.2413 / -0.3040 | -0.2404 / -0.3032 | -0.2251 / -0.2900 | FAIL |
| R4p time-decay (youngest-keyed) | egalitarian | 1000x | -0.4840 / -0.4600 | -0.4911 / -0.4623 | -0.4675 / -0.4571 | -0.4809 / -0.4598 | FAIL |
| R5p inverse-factor+time-decay (youngest-keyed) | wealth | 1x | +0.0275 / +0.0436 | +0.0235 / +0.0368 | +0.0315 / +0.0412 | +0.0275 / +0.0405 | FAIL |
| R5p inverse-factor+time-decay (youngest-keyed) | wealth | 100x | -0.0043 / -0.0773 | -0.0035 / -0.0727 | -0.0023 / -0.0715 | -0.0034 / -0.0738 | FAIL |
| R5p inverse-factor+time-decay (youngest-keyed) | wealth | 1000x | +0.0014 / -0.0771 | +0.0011 / -0.0753 | +0.0011 / -0.0748 | +0.0012 / -0.0757 | FAIL |
| R5p inverse-factor+time-decay (youngest-keyed) | egalitarian | 1x | -0.1992 / -0.1622 | -0.1844 / -0.1406 | -0.2200 / -0.1533 | -0.2012 / -0.1520 | FAIL |
| R5p inverse-factor+time-decay (youngest-keyed) | egalitarian | 100x | -0.0987 / -0.1184 | -0.1233 / -0.1343 | -0.1338 / -0.1504 | -0.1186 / -0.1344 | FAIL |
| R5p inverse-factor+time-decay (youngest-keyed) | egalitarian | 1000x | -0.5182 / -0.5119 | -0.5156 / -0.5143 | -0.4892 / -0.4964 | -0.5077 / -0.5075 | FAIL |

| variant | mean ΔGini range, wealth | mean ΔGini range, egalitarian | best 3-seed mean | cells passing |
|---|---|---|---|---|
| R4p time-decay (youngest-keyed) | -0.0731 to +0.0163 | -0.4809 to -0.0870 | +0.0163 | 0/6 |
| R5p inverse-factor+time-decay (youngest-keyed) | -0.0757 to +0.0405 | -0.5077 to -0.1186 | +0.0405 | 0/6 |

Reading it: the private form of R4 does worse than the non-private one. With tags that do not decay, demurrage never stops. For R4p, seed 1696 collects 2.4e8 to 1.0e9 BTH of demurrage over 20 years in every cell, against 5.0e7 to 8.2e8 BTH for R4 (§3.4), and it keeps growing between year 10 and year 20. It falls on payment recipients as in R4, and the lottery returns most of it to the whales (R4p, seed 1696, 10 years: 52.8–83.0% of payouts in every cell except egalitarian 1x). R4's single passing cell (wealth, 1000x) becomes +0.0163 / −0.0731. R5p does slightly better than R5 under wealth mining at 1x (+0.0405 at 20 years) and worse everywhere else. No measured form of R4 passes Gini in more than one cell, and the private forms pass in none.

### 3.2 Node candidate rule (spent outputs stay eligible)

The same runs with `spent_eligible = true`, for the rules that make sense without a reveal. R4 and R5 are omitted because R4 changes only demurrage and R5 is covered by R2. R1 and R2 are included to show their Gini effect when spent value keeps counting. Their splitting result under this rule is §4's churn attack.

| variant | mean ΔGini range, wealth | mean ΔGini range, egalitarian | best 3-seed mean | cells passing |
|---|---|---|---|---|
| R0 baseline | -0.0000 to +0.0726 | -0.0002 to +0.0112 | +0.0726 | 0/6 |
| R1 value-age | +0.0136 to +0.0290 | -0.0429 to -0.0098 | +0.0290 | 0/6 |
| R2 inverse-factor | +0.0136 to +0.0294 | -0.0429 to -0.0019 | +0.0294 | 0/6 |
| R3 ticket-price | -0.0003 to +0.0018 | -0.0160 to -0.0051 | +0.0018 | 0/6 |

| variant | mining | tx scale | seed 1696 (10 / 20 yr) | seed 2 | seed 3 | 3-seed mean | > 0.05 |
|---|---|---:|---|---|---|---|---|
| R0 baseline | wealth | 1x | +0.0003 / +0.0010 | +0.0005 / -0.0022 | +0.0007 / +0.0011 | +0.0005 / -0.0000 | FAIL |
| R0 baseline | wealth | 100x | +0.0155 / +0.0268 | +0.0143 / +0.0255 | +0.0169 / +0.0294 | +0.0156 / +0.0273 | FAIL |
| R0 baseline | wealth | 1000x | +0.0425 / +0.0725 | +0.0426 / +0.0723 | +0.0431 / +0.0730 | +0.0427 / +0.0726 | FAIL |
| R0 baseline | egalitarian | 1x | +0.0273 / +0.0311 | +0.0041 / -0.0054 | -0.0224 / -0.0264 | +0.0030 / -0.0002 | FAIL |
| R0 baseline | egalitarian | 100x | +0.0068 / +0.0123 | +0.0086 / +0.0123 | +0.0056 / +0.0089 | +0.0070 / +0.0112 | FAIL |
| R0 baseline | egalitarian | 1000x | +0.0003 / +0.0060 | +0.0005 / +0.0051 | -0.0009 / +0.0039 | -0.0000 / +0.0050 | FAIL |
| R1 value-age | wealth | 1x | +0.0255 / +0.0309 | +0.0188 / +0.0324 | +0.0239 / +0.0236 | +0.0227 / +0.0290 | FAIL |
| R1 value-age | wealth | 100x | +0.0149 / +0.0212 | +0.0130 / +0.0221 | +0.0150 / +0.0241 | +0.0143 / +0.0225 | FAIL |
| R1 value-age | wealth | 1000x | +0.0138 / +0.0209 | +0.0136 / +0.0206 | +0.0133 / +0.0206 | +0.0136 / +0.0207 | FAIL |
| R1 value-age | egalitarian | 1x | +0.0130 / -0.0196 | -0.0355 / -0.0054 | -0.0179 / -0.0043 | -0.0135 / -0.0098 | FAIL |
| R1 value-age | egalitarian | 100x | -0.0230 / -0.0407 | -0.0202 / -0.0330 | -0.0229 / -0.0448 | -0.0220 / -0.0395 | FAIL |
| R1 value-age | egalitarian | 1000x | -0.0264 / -0.0412 | -0.0263 / -0.0431 | -0.0279 / -0.0445 | -0.0269 / -0.0429 | FAIL |
| R2 inverse-factor | wealth | 1x | +0.0281 / +0.0348 | +0.0216 / +0.0324 | +0.0225 / +0.0210 | +0.0241 / +0.0294 | FAIL |
| R2 inverse-factor | wealth | 100x | +0.0150 / +0.0218 | +0.0131 / +0.0223 | +0.0150 / +0.0239 | +0.0144 / +0.0227 | FAIL |
| R2 inverse-factor | wealth | 1000x | +0.0138 / +0.0208 | +0.0136 / +0.0206 | +0.0133 / +0.0207 | +0.0136 / +0.0207 | FAIL |
| R2 inverse-factor | egalitarian | 1x | +0.0271 / -0.0308 | -0.0199 / -0.0389 | -0.0130 / -0.0326 | -0.0019 / -0.0341 | FAIL |
| R2 inverse-factor | egalitarian | 100x | -0.0230 / -0.0408 | -0.0202 / -0.0329 | -0.0229 / -0.0448 | -0.0220 / -0.0395 | FAIL |
| R2 inverse-factor | egalitarian | 1000x | -0.0264 / -0.0412 | -0.0264 / -0.0432 | -0.0279 / -0.0445 | -0.0269 / -0.0429 | FAIL |
| R3 ticket-price | wealth | 1x | +0.0010 / +0.0023 | +0.0008 / -0.0010 | +0.0018 / +0.0032 | +0.0012 / +0.0015 | FAIL |
| R3 ticket-price | wealth | 100x | -0.0005 / -0.0007 | -0.0001 / +0.0008 | -0.0003 / +0.0018 | -0.0003 / +0.0006 | FAIL |
| R3 ticket-price | wealth | 1000x | +0.0004 / +0.0015 | +0.0006 / +0.0020 | +0.0005 / +0.0019 | +0.0005 / +0.0018 | FAIL |
| R3 ticket-price | egalitarian | 1x | +0.0089 / -0.0073 | -0.0153 / -0.0005 | -0.0263 / -0.0401 | -0.0109 / -0.0160 | FAIL |
| R3 ticket-price | egalitarian | 100x | -0.0058 / -0.0090 | -0.0061 / -0.0111 | -0.0058 / -0.0094 | -0.0059 / -0.0098 | FAIL |
| R3 ticket-price | egalitarian | 1000x | -0.0051 / -0.0083 | -0.0051 / -0.0083 | -0.0052 / -0.0084 | -0.0051 / -0.0083 | FAIL |

### 3.3 Where the lottery goes (seed 1696, 10 years, #1790 candidate rule)

Cohort shares of all lottery payouts. Hashpower shares: wealth mining gives the whales 98.3%; egalitarian mining gives small 57.1%, middle 21.4%, merchant 14.3% and whales 7.1%.

| variant | mining | tx scale | small | middle | merchant | whales (9) | strategic | lottery paid (BTH) | demurrage (BTH) | ticket burn (BTH) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| R0 | wealth | 1x | 0.1% | 1.3% | 0.2% | 77.6% | 20.8% | 1.395e+08 | 5.942e+07 | 0.000e+00 |
| R0 | wealth | 100x | 0.5% | 1.8% | 0.0% | 82.8% | 14.9% | 9.193e+07 | 2.234e+03 | 0.000e+00 |
| R0 | wealth | 1000x | 0.5% | 23.8% | 0.0% | 67.2% | 8.5% | 9.192e+07 | 2.198e-01 | 0.000e+00 |
| R0 | egalitarian | 1x | 59.6% | 25.4% | 6.1% | 8.0% | 0.9% | 1.857e+08 | 1.173e+08 | 0.000e+00 |
| R0 | egalitarian | 100x | 9.2% | 30.3% | 0.0% | 54.6% | 5.9% | 9.196e+07 | 5.145e+04 | 0.000e+00 |
| R0 | egalitarian | 1000x | 4.4% | 45.8% | 0.0% | 44.3% | 5.5% | 9.192e+07 | 0.000e+00 | 0.000e+00 |
| R1 | wealth | 1x | 0.0% | 2.3% | 79.1% | 15.5% | 3.2% | 1.486e+08 | 7.085e+07 | 0.000e+00 |
| R1 | wealth | 100x | 0.0% | 1.6% | 0.0% | 80.7% | 17.7% | 9.192e+07 | 1.682e+00 | 0.000e+00 |
| R1 | wealth | 1000x | 0.0% | 4.4% | 0.0% | 81.8% | 13.7% | 9.192e+07 | 2.198e-01 | 0.000e+00 |
| R1 | egalitarian | 1x | 36.0% | 6.2% | 56.0% | 1.5% | 0.3% | 1.903e+08 | 1.230e+08 | 0.000e+00 |
| R1 | egalitarian | 100x | 38.5% | 35.1% | 0.1% | 22.5% | 3.8% | 9.192e+07 | 0.000e+00 | 0.000e+00 |
| R1 | egalitarian | 1000x | 2.5% | 26.4% | 0.0% | 62.1% | 9.0% | 9.192e+07 | 0.000e+00 | 0.000e+00 |
| R2 | wealth | 1x | 0.0% | 1.5% | 80.7% | 14.7% | 3.0% | 1.496e+08 | 7.214e+07 | 0.000e+00 |
| R2 | wealth | 100x | 0.0% | 2.0% | 0.0% | 79.4% | 18.5% | 9.192e+07 | 1.682e+00 | 0.000e+00 |
| R2 | wealth | 1000x | 0.0% | 4.5% | 0.0% | 81.5% | 14.0% | 9.192e+07 | 2.198e-01 | 0.000e+00 |
| R2 | egalitarian | 1x | 33.9% | 5.7% | 59.6% | 0.7% | 0.1% | 1.877e+08 | 1.197e+08 | 0.000e+00 |
| R2 | egalitarian | 100x | 55.6% | 28.8% | 0.4% | 13.0% | 2.2% | 9.192e+07 | 0.000e+00 | 0.000e+00 |
| R2 | egalitarian | 1000x | 2.9% | 29.2% | 0.0% | 59.1% | 8.8% | 9.192e+07 | 0.000e+00 | 0.000e+00 |
| R3 | wealth | 1x | 32.7% | 7.5% | 42.3% | 14.6% | 2.9% | 1.372e+08 | 5.666e+07 | 1.245e+08 |
| R3 | wealth | 100x | 8.5% | 43.9% | 0.0% | 47.1% | 0.5% | 1.553e-05 | 1.682e+00 | 0.000e+00 |
| R3 | wealth | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 1.835e-05 | 2.198e-01 | 0.000e+00 |
| R3 | egalitarian | 1x | 48.2% | 15.9% | 33.9% | 1.8% | 0.3% | 1.819e+08 | 1.125e+08 | 1.751e+08 |
| R3 | egalitarian | 100x | 8.8% | 43.7% | 0.0% | 47.0% | 0.5% | 3.462e-05 | 0.000e+00 | 0.000e+00 |
| R3 | egalitarian | 1000x | 0.0% | 0.0% | 0.0% | 100.0% | 0.0% | 2.243e-05 | 0.000e+00 | 0.000e+00 |
| R4 | wealth | 1x | 0.1% | 1.3% | 0.2% | 77.5% | 20.9% | 1.322e+08 | 5.028e+07 | 0.000e+00 |
| R4 | wealth | 100x | 0.5% | 1.4% | 0.0% | 83.3% | 14.8% | 1.772e+08 | 1.998e+08 | 0.000e+00 |
| R4 | wealth | 1000x | 0.6% | 30.0% | 0.0% | 59.8% | 9.5% | 1.796e+08 | 5.695e+08 | 0.000e+00 |
| R4 | egalitarian | 1x | 59.6% | 25.4% | 6.0% | 8.0% | 0.9% | 2.286e+08 | 1.709e+08 | 0.000e+00 |
| R4 | egalitarian | 100x | 7.7% | 25.9% | 0.0% | 59.5% | 6.9% | 3.770e+08 | 8.181e+08 | 0.000e+00 |
| R4 | egalitarian | 1000x | 1.8% | 33.9% | 0.0% | 57.2% | 7.1% | 2.693e+08 | 8.145e+08 | 0.000e+00 |
| R5 | wealth | 1x | 0.0% | 1.9% | 79.2% | 15.8% | 3.1% | 1.402e+08 | 6.037e+07 | 0.000e+00 |
| R5 | wealth | 100x | 0.0% | 1.5% | 0.0% | 80.2% | 18.3% | 1.771e+08 | 1.912e+08 | 0.000e+00 |
| R5 | wealth | 1000x | 0.0% | 4.2% | 0.0% | 77.1% | 18.6% | 1.721e+08 | 5.513e+08 | 0.000e+00 |
| R5 | egalitarian | 1x | 33.4% | 5.7% | 60.2% | 0.6% | 0.1% | 2.408e+08 | 1.862e+08 | 0.000e+00 |
| R5 | egalitarian | 100x | 34.8% | 21.3% | 0.4% | 36.7% | 6.9% | 3.616e+08 | 8.566e+08 | 0.000e+00 |
| R5 | egalitarian | 1000x | 0.9% | 13.5% | 0.0% | 73.0% | 12.6% | 2.800e+08 | 8.269e+08 | 0.000e+00 |

### 3.4 Demurrage at density: R0 vs R4 (seed 1696)

| variant | mining | tx scale | demurrage 10 yr (BTH) | demurrage 20 yr (BTH) | of which #925 downgrade charge (20 yr) | ΔGini 20 yr (seed 1696) |
|---|---|---:|---:|---:|---:|---:|
| R0 | wealth | 1x | 5.942e+07 | 5.942e+07 | 59% | -0.0010 |
| R0 | wealth | 100x | 2.234e+03 | 2.234e+03 | 100% | +0.0006 |
| R0 | wealth | 1000x | 2.198e-01 | 2.198e-01 | 100% | +0.0185 |
| R0 | egalitarian | 1x | 1.173e+08 | 1.173e+08 | 92% | -0.0469 |
| R0 | egalitarian | 100x | 5.145e+04 | 5.145e+04 | 100% | -0.1372 |
| R0 | egalitarian | 1000x | 0.000e+00 | 0.000e+00 | 0% | -0.1470 |
| R4 | wealth | 1x | 5.028e+07 | 5.028e+07 | 56% | -0.0003 |
| R4 | wealth | 100x | 1.998e+08 | 2.000e+08 | 83% | -0.0134 |
| R4 | wealth | 1000x | 5.695e+08 | 5.695e+08 | 94% | +0.0780 |
| R4 | egalitarian | 1x | 1.709e+08 | 1.709e+08 | 93% | -0.0109 |
| R4 | egalitarian | 100x | 8.181e+08 | 8.192e+08 | 97% | -0.4892 |
| R4 | egalitarian | 1000x | 8.145e+08 | 8.145e+08 | 96% | -0.4550 |

### 3.5 Optimal-decoy whale (seed 1696, 1x)

| variant | mining | ΔGini 10 yr | ΔGini 20 yr |
|---|---|---:|---:|
| R0 baseline | wealth | +0.0002 | -0.0012 |
| R0 baseline | egalitarian | +0.0100 | -0.0148 |
| R1 value-age | wealth | +0.0257 | +0.0300 |
| R1 value-age | egalitarian | +0.0305 | -0.0303 |
| R2 inverse-factor | wealth | +0.0233 | +0.0237 |
| R2 inverse-factor | egalitarian | +0.0151 | -0.0366 |
| R3 ticket-price | wealth | +0.0031 | +0.0041 |
| R3 ticket-price | egalitarian | -0.0131 | -0.0650 |
| R4 time-decay | wealth | +0.0008 | -0.0013 |
| R4 time-decay | egalitarian | +0.0264 | -0.0406 |
| R5 inverse-factor+time-decay | wealth | +0.0240 | +0.0265 |
| R5 inverse-factor+time-decay | egalitarian | +0.0331 | -0.0340 |

### 3.6 Reading the Gini results

- **Only one cell passes, and the variant behind it fails everywhere else.** R4, in its non-private form (decay keyed to the real input's age), clears 0.05 under wealth mining at 1000x (+0.0721 / +0.0780 as 3-seed means). Two caveats apply. 94% of the demurrage behind that result is the #925 downgrade charge on tag mass that the always-consolidating wallet loses to pruning and the 16-tag cap, which #1790 already lists as an upper bound. And the same rule under egalitarian mining gives −0.46 to −0.60 at 100x and 1000x.
- **R4 does fix the measured density mechanism, but the demurrage lands on the wrong people.** With per-block decay, demurrage no longer vanishes at density: 2.0e8 BTH (wealth) and 8.2e8 BTH (egalitarian) at 100x over 20 years, against 2.2e3 and 5.1e4 for R0. The tags survive, though, and payments carry them to everyone. By year 10 (seed 1696, egalitarian, 100x), the 80 small holders have paid 3.4e8 BTH and the strategic whale 4.9e6 BTH. That is #1790's cause 3 (demurrage priced on velocity, not holdings), amplified. The lottery then returns most of the pool to the whales (59.5%), who spend least often.
- **Value-linear weight (R1, R2, R5) is the best lottery rule here, and it still falls well short.** Under wealth mining at 1x it moves the lottery off coinbase counts: the whales' share drops from 98.4% to 18.7% (R1) and 17.7% (R2). The 3-seed means reach +0.0234 to +0.0317. For R1 and R2 at 100x and 1000x the effect disappears (−0.0006 to +0.0040). Under egalitarian mining it is −0.0008 at best and −0.17 at worst; R5 goes as low as −0.64, for R4's reason.
- **A 10,000-block window turns value weight into turnover weight.** The window is about 14 hours. A whale's consolidated output, created 12 times a year, sits in it for 1.8% of the time. A merchant's (365 times a year) sits in it for 54%. So R1 and R2 pay merchants 79–81% of the lottery under wealth mining and 56–60% under egalitarian mining. Merchants are neither poor nor rich in this population, which is why the tilt (R2 vs R1) changes little: every factor is close to 1x by year 10 (#1790 cause 2).
- **R3 redistributes nothing, as designed.** The best 3-seed mean is +0.0033. At 1x every cohort buys tickets and wins back about what it pays (ticket burn 1.2e8–1.8e8 BTH against 1.4e8–1.8e8 BTH paid out over 10 years, seed 1696). At 100x and above, every position is spent within the 10,000-block window, so under #1790's rule no wallet buys a ticket and the lottery stops paying. The emission share then sits in the carry. Under egalitarian mining that costs −0.005 to −0.034 against the counterfactual, in which miners keep the share.
- **The node rule (§3.2) is kinder to the baseline at density.** Spent outputs keep their tickets, so frequent spenders are no longer cut out of the draw. R0 reaches +0.0427 / +0.0726 under wealth mining at 1000x, and −0.0002 to +0.0112 under egalitarian mining (against −0.02 to −0.15 under #1790's rule). No cell passes at both horizons. R1 and R2 under the node rule stay in the same range as under #1790's rule.
- **The optimal-decoy whale changes nothing material** at 1x (§3.5). It pays the downgrade charge once and ends at 1x, as in #1790.
- **Most of the demurrage the harness collects is the downgrade charge, even for R0.** The share is 59% (wealth) and 92% (egalitarian) at 1x. Every m2-production demurrage figure therefore depends heavily on the always-consolidate wallet approximation.

## 4. Axis 2: splitting (the #1761 grid)

The historical stationary grid is #1761's 192 points: 4 epochs × 2 backlogs × 3 organic output rates × 2 fee modes × 2 whale factors × 2 capacity bounds. Each point uses the real fee floor, the code's reward cap and the 80% pool. R0 and R4 use the #1761 evaluator unchanged (`lottery_reward_cap_faithful::optimum`). A unit test (`baseline_reproduces_1761`) checks that the R0 row reproduces #1761's `Rules::CurrentCode` approximation: 192/192 profitable, a maximum of +49.9975 BTH/block, a reference net of +2.4660 BTH/block, and the same optimum at every point.

- **R3** prices every ticket, including the coinbase and the 4 payout outputs, at `p = R / T`: the payout per block over the tickets created per block. The whale's winnings `s·R` then equal its ticket cost `(k + 4s)·p` exactly, so its net is minus its fee floor at every point.
- **R1, R2 and R5** (value-linear) cannot be split for profit within the no-re-spend model: a split adds no weight. The modeled node rule instead lets the whale re-spend capital every block while spent copies keep their weight (§2). The grid evaluates that churn attack in expected values, not through stateful production execution. The whale re-spends `x` BTH per block (up to 5M BTH, the m2 strategic whale) in one 1-input, 2-output transaction, priced by the real floor at its factor with a 10-block demurrage clock (the ring's youngest decoys are at least 10 blocks old). Organic outputs are worth 10 BTH each. The coinbase carries the miner payout. The age term cancels because every output spends the same 9,281 blocks in the window. "Reveal" is #1790's rule, under which the whale's weight is the value it actually holds, as for any holder.

Generated by `cluster-tax-sim redistribution-redesign --part splitting` (golden in `docs/research/generated/`). Fee modes and candidate/remedy assumptions inherit the historical evaluator; current quantitative production profitability needs production-mode re-run. Existing table labels, numbers and rankings are retained:

### Splitting grid (#1761 grid, 192 points)

Whale optimum per grid point (4 epochs x 2 backlogs x 3 organic rates x 2 fee modes x 2 factors x 2 capacities). "Node" = spent outputs stay candidates (what the node does). "Reveal" = spent outputs leave the draw (#1790's rule; needs the winner to reveal its key image). Value-linear rules: the whale re-spends up to 5000000 BTH per block; organic outputs are worth 10 BTH each. Reference point = epoch 1, backlog 0, rho_o 20, floor+MIN_TX_FEE, 6x, proposer slot.

| variant | profitable (node) | profitable (reveal) | max net*/blk, node (BTH) | max net*/day, node (BTH) | reference net*/blk, node (BTH) | reference whale share, node |
|---|---:|---:|---:|---:|---:|---:|
| R0 baseline | 192/192 | 192/192 | +49.9975 | +863957 | +2.4660 | 98.7% |
| R1 value-age | 192/192 | 0/192 | +49.9993 | +863988 | +2.4970 | 99.9% |
| R2 inverse-factor | 192/192 | 0/192 | +49.9993 | +863988 | +2.4915 | 99.8% |
| R3 ticket-price | 0/192 | 0/192 | +0.0000 | +0 | +0.0000 | 0.0% |
| R4 time-decay | 192/192 | 192/192 | +49.9975 | +863957 | +2.4660 | 98.7% |
| R5 inverse-factor+time-decay | 192/192 | 0/192 | +49.9993 | +863988 | +2.4915 | 99.8% |

### Value-linear churn: sensitivity (node candidate rule)

Profitable points out of 192 when the whale re-spends at most its capital per block.

| whale capital (BTH) | organic output value (BTH) | R1 value-age | R2 inverse-factor |
|---:|---:|---:|---:|
| 1000 | 10 | 192/192 | 192/192 |
| 1000 | 1000 | 180/192 | 180/192 |
| 100000 | 10 | 192/192 | 192/192 |
| 100000 | 1000 | 192/192 | 192/192 |
| 5000000 | 10 | 192/192 | 192/192 |
| 5000000 | 1000 | 192/192 | 192/192 |

### R1 churn by epoch (node candidate rule, reference fee mode and capacity)

| epoch | rho_o | factor | re-spent/blk (BTH) | whale share | net*/blk (BTH) | net*/day (BTH) |
|---|---:|---:|---:|---:|---:|---:|
| 0 | 20 | 1x | 5000000 | 100.0% | +0.0008 | +13 |
| 0 | 20 | 6x | 5000000 | 100.0% | +0.0008 | +13 |
| 0 | 200 | 1x | 5000000 | 100.0% | +0.0080 | +138 |
| 0 | 200 | 6x | 5000000 | 100.0% | +0.0080 | +138 |
| 1 | 20 | 1x | 5000000 | 100.0% | +2.5007 | +43212 |
| 1 | 20 | 6x | 267276 | 99.9% | +2.4970 | +43148 |
| 1 | 200 | 1x | 5000000 | 100.0% | +2.5070 | +43320 |
| 1 | 200 | 6x | 815663 | 99.8% | +2.4966 | +43141 |
| 3 | 20 | 1x | 5000000 | 100.0% | +1.8757 | +32412 |
| 3 | 20 | 6x | 267276 | 99.9% | +1.8727 | +32360 |
| 3 | 200 | 1x | 5000000 | 100.0% | +1.8822 | +32525 |
| 3 | 200 | 6x | 815663 | 99.8% | +1.8732 | +32368 |
| tail | 20 | 1x | 5000000 | 100.0% | +1.2117 | +20938 |
| tail | 20 | 6x | 213821 | 99.9% | +1.2092 | +20896 |
| tail | 200 | 1x | 5000000 | 100.0% | +1.2184 | +21054 |
| tail | 200 | 6x | 652530 | 99.7% | +1.2110 | +20926 |

**Reading the modeled outcomes.** Value-linear weight closes the splitting hole here only if spent value stops counting, and the modeled node rule retains it. Churn beats splitting by a wide margin, because one transaction re-mints the whole position's weight. 1,000 BTH of capital is enough at 180–192 of the 192 points. R3 closes the hole by construction, and it is the only candidate here that does so without a reveal.

## 5. Axis 3: privacy

A rule fails this axis if it needs information a ring hides (which member is real, or whether an output is spent), or if it publishes something that narrows the ring. The measurements use the #1757 production pickers (`privacy_routes.rs`, pinned to the node, CLI and web code by `botho/tests/privacy_sim_route_parity.rs`) through a new sampling hook (`sample_route_ring_ages`).

### 5.1 Value-linear weights (R1, R2, R5) and the spent set

- **What they read.** Output value and age (R1), plus the factor from plaintext tags and W (R2). The live chain has public amounts, so value is readable today. Under ADR 0006's confidential amounts it is not, and a value-weighted draw then needs the ZK weighted-sampling sort (#902, Path B). A winner's proof would also show that its amount covers the drawn offset, which leaks a lower bound on a hidden amount for 4 outputs per block.
- **What they need to be split-safe.** On the node they are churn-farmable (§4) unless spent value stops counting. Without breaking ring privacy there are two ways to do that:
  1. **Claim-reveal:** a winner must publish its output's key image to claim, and a spent output forfeits. A rational owner never reveals the key image of an output it has already spent, because the win is forfeit anyway. So the ring in which an output is spent is deanonymized directly only if that output won, and was claimed, **before** the spend. Every claimed output is also marked as a known decoy in every other ring that uses it.
  2. **A ZK non-membership proof** that the output's key image is not in the spent set. This leaks nothing but needs an accumulator over all key images and a new proof per claim. It is very high complexity, and nothing in the codebase supports it today.

The table measures claim-reveal. If ρ eligible outputs are created per block and 4 win, an output that sits in the window for all of its 9,281 blocks wins at least once with probability `q = 1 − e^(−4/ρ)`: 55% at ρ = 5 (the m2 1x window, nearly all coinbase and payout outputs), 18% at ρ = 20, and 2% at ρ = 200. Wins arrive at a constant per-block rate, so an output spent at age `a` is exposed only for ages 720..min(a, 10,000) and wins before its spend with probability `1 − (1 − q)^f`, where `f` is that exposure as a share of the 9,281-block window. The real input of each sampled ring is revealed directly at that per-ring rate (column "direct reveal"). Each decoy is eliminated at the full rate q. That is exact for decoys that were never spent inside their window and an upper bound for those that were.

Production routes (#1757 ports), chain of 1 output/block to height 1200000, 4000 rings per row, seed 0x18019817. Every winner reveals its output's key image to claim, and a spent output forfeits. q = share of outputs that win at least once over a full 9281-block window = 1 - exp(-4/rho), rho = eligible outputs created per block. Each decoy is eliminated with probability q (an upper bound for decoys spent inside their own window). The real input is exposed only if it wins before it is spent, during ages 720..min(real age, 10000): "direct reveal" is that rate, per ring, from the ring's own real age. ID = how often the adversary's single guess is the real input (uniform = 5.0%), counting directly revealed rings as identified.

| route | real age | Band-Center ID, no reveal | rho | q | direct reveal | survivors (of 20) | ID uniform-over-survivors | ID Band-Center-over-survivors |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| node | 1000 | 5.1% | 5 | 55.1% | 2.5% | 9.3 | 13.4% | 13.0% |
| node | 1000 | 5.1% | 20 | 18.1% | 0.5% | 16.5 | 6.6% | 6.5% |
| node | 1000 | 5.1% | 200 | 2.0% | 0.1% | 19.6 | 5.2% | 5.3% |
| node | 30000 | 4.8% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 59.6% |
| node | 30000 | 4.8% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.1% |
| node | 30000 | 4.8% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.0% |
| node | 100000 | 5.3% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 60.3% |
| node | 100000 | 5.3% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.2% |
| node | 100000 | 5.3% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.4% |
| node | gamma-spend | 5.5% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 59.5% |
| node | gamma-spend | 5.5% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.3% |
| node | gamma-spend | 5.5% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.6% |
| cli | 1000 | 5.0% | 5 | 55.1% | 2.5% | 9.3 | 13.4% | 14.0% |
| cli | 1000 | 5.0% | 20 | 18.1% | 0.5% | 16.5 | 6.6% | 6.6% |
| cli | 1000 | 5.0% | 200 | 2.0% | 0.1% | 19.6 | 5.2% | 5.3% |
| cli | 30000 | 4.7% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 60.7% |
| cli | 30000 | 4.7% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.0% |
| cli | 30000 | 4.7% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 6.9% |
| cli | 100000 | 5.3% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 60.6% |
| cli | 100000 | 5.3% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.7% |
| cli | 100000 | 5.3% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.5% |
| cli | gamma-spend | 5.2% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 60.4% |
| cli | gamma-spend | 5.2% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.2% |
| cli | gamma-spend | 5.2% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.3% |
| web | 1000 | 5.5% | 5 | 55.1% | 2.5% | 9.3 | 13.4% | 13.8% |
| web | 1000 | 5.5% | 20 | 18.1% | 0.5% | 16.5 | 6.6% | 7.0% |
| web | 1000 | 5.5% | 200 | 2.0% | 0.1% | 19.6 | 5.2% | 5.6% |
| web | 30000 | 4.9% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 59.7% |
| web | 30000 | 4.9% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 22.6% |
| web | 30000 | 4.9% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.0% |
| web | 100000 | 4.3% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 60.1% |
| web | 100000 | 4.3% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 22.5% |
| web | 100000 | 4.3% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 6.5% |
| web | gamma-spend | 5.6% | 5 | 55.1% | 55.0% | 4.8 | 60.0% | 60.7% |
| web | gamma-spend | 5.6% | 20 | 18.1% | 17.9% | 13.8 | 23.0% | 23.5% |
| web | gamma-spend | 5.6% | 200 | 2.0% | 2.1% | 19.2 | 7.1% | 7.6% |

Reading it, per real-input age:

- **Real age 1,000** (spent 280 blocks into its window): the real input is revealed directly in 2.5% of rings at ρ = 5, 0.5% at ρ = 20 and 0.1% at ρ = 200. The first version of this table used the full-window rate, 55%, 18% and 2%, for these rows; that overstated direct deanonymization of young inputs by a factor of about 20 to 35. The decoys are still eliminated at rate q, so the real input is now usually among *fewer* survivors: a uniform guess over survivors identifies it in 13.4%, 6.6% and 5.2% of rings, against 5% today. Since the random-offset age window (#1692) the real input is not the band center on any route (Band-Center with no reveal: 4.3–5.6%, against 50–52% on the CLI and web routes and up to 84% on the node route before), so the Band-Center guess over survivors only matches the uniform one: 13.0–14.0% at ρ = 5, 6.5–7.0% at ρ = 20 and 5.3–5.6% at ρ = 200.
- **Real ages 30,000 and 100,000** (spent after the window closed): unchanged from the first version. The real input had its whole window to win, so it is revealed directly in 55%, 18% and 2% of rings, and a uniform guess over survivors identifies it in 60%, 23% and 7%.
- **Gamma-spend ages** (the node's spend-age distribution): almost unchanged. Direct reveal is 55.0%, 17.9% and 2.1%, because nearly all sampled spend ages are past the window. The first version's overstatement here is below the table's resolution.

So claim-reveal costs a ring chain its anonymity mainly for outputs that are held past the window, which is the typical case under the gamma spend-age distribution, and much less for inputs spent young. #1790's candidate rule, which every Gini number in §3.1 assumes, is a privacy cost unless it comes with a zero-knowledge proof. The size of that cost depends strongly on how long outputs are held before they are spent.

### 5.2 Ticket price (R3)

R3 reads the output count, the public payout and the window count. All three are public, so none of them involves a ring. If buying a ticket is **mandatory** for every output of at least `min_utxo_value`, nothing new is published. If it is **opt-in**, each output carries a public "ticketed" flag. A spender whose wallet policy is known (for example, "always ticket change") is then found among the ticketed members of its ring. With a fraction φ of outputs ticketed, that guess succeeds `1/(1 + 19φ)` of the time: 9.5% at φ = 0.5 and 5.5% at φ = 0.9, against 5% today. The flag does not depend on age, so the route makes no difference and no route measurement is needed.

### 5.3 Time decay (R4, R5)

R4 needs a time allowance for tag decay that the node can check. If the allowance is keyed to **the real input's age**, the output's plaintext tag weight reveals that age to within one ppm quantum: 11 blocks for a full-weight tag, about 1,000 blocks for a 1% tag. The adversary then picks the ring member of that age. If the allowance is keyed to **the ring's youngest member** (the safe direction for the node: the least decay any member could justify), it is a function of public ring ages and reveals nothing new.

The published tag weight reveals the real input's age to within 11 blocks for a full-weight tag and 1025 blocks for a 1% tag (one ppm quantum at the R4 decay rate). ID = uniform guess among members whose age matches. Keyed to the ring's youngest member, the allowance reveals only that member's age, which is public; the last column is how often the real input is that member.

| route | real age | rings | ID, full-weight tag | ID, 1% tag | real input is youngest |
|---|---|---:|---:|---:|---:|
| node | 1000 | 4000 | 23.7% | 5.0% | 5.2% |
| node | 30000 | 4000 | 93.4% | 9.2% | 4.8% |
| node | 100000 | 4000 | 97.8% | 25.9% | 5.5% |
| node | gamma-spend | 4000 | 99.6% | 70.7% | 4.9% |
| cli | 1000 | 4000 | 23.7% | 5.0% | 5.1% |
| cli | 30000 | 4000 | 93.1% | 9.2% | 5.1% |
| cli | 100000 | 4000 | 97.8% | 25.8% | 4.8% |
| cli | gamma-spend | 4000 | 99.7% | 70.6% | 4.9% |
| web | 1000 | 4000 | 23.4% | 5.0% | 4.3% |
| web | 30000 | 4000 | 93.1% | 9.3% | 5.1% |
| web | 100000 | 4000 | 97.9% | 25.4% | 4.7% |
| web | gamma-spend | 4000 | 99.5% | 69.6% | 5.3% |

Reading it: keyed to the real input's age, R4 identifies 93–100% of rings on every route at real ages of 30,000 blocks and more. That holds even after cluster fingerprinting is fixed (#1756), because the leak is the decay, not the cluster. Only young inputs (23–24% at age 1,000, where the ±5% age window is only 100 blocks wide) and weak tags blur it. The 1% tag column rises to 70–71% at gamma-spend ages because #1692 also moved the gamma spend model from 720 to 17,280 blocks per day, so sampled spend ages are 24 times larger in blocks and their age windows are far wider than the 1,025-block tag quantum: fewer decoys share the real input's quantum. Keyed to the youngest member, the allowance adds nothing to what the ring ages already show: since the random-offset window (#1692) the real input is the youngest member in 4.3–5.5% of rings on every route, the uniform rate. (Before #1692 the node picker made it the youngest in 100% of rings at age 1,000.) The Gini of this private form (R4p, with R5p as its R2 combination) is measured separately in §3.1.1: it passes no cell.

## 6. Node-side implementation complexity

Each estimate is the work needed for the node to enforce the rule. A wallet change and a protocol bump (testnet reset) are implied throughout.

| id | public amounts (today) | what makes it split-safe | under CT (ADR 0006) | overall |
|---|---|---|---|---|
| R1 | new `SelectionMode` arm (`value × age`; `LotteryCandidate` already has `value` and `creation_block`, but `weight()` needs the height), plus `canonical_config` (`ledger/lottery_v2.rs`). Small. | a claim protocol (key-image reveal, claim window, payout escrow and expiry, reorg handling) or a ZK non-membership proof over the key-image set. Large to very large. | the ZK weighted-sampling sort (#902 Path B). Very large. | **very high** |
| R2 | `SelectionMode::ClusterWeighted` exists, and `lottery_candidates` already computes each candidate's factor: a config switch. Trivial. | as R1 | as R1, plus a hidden-W factor gadget (CT-economics D2) | **very high** (trivial if splitting is accepted) |
| R3 | a new consensus fee term `price × eligible outputs`; a full (untruncated) eligible-output counter in ledger state for the price; a per-output eligibility record (opt-in) or a rule that every output ≥ `min_utxo_value` pays (mandatory); coinbase and payout outputs pay or become ineligible; burn accounting. Medium (several hundred lines across ledger, mempool and three wallets). | none needed | price and count are public; works unchanged | **medium** |
| R4p (R4's private form; R4 itself would also need the real input's age published) | an anti-deflation floor in tag-inheritance validation (today only inflation is checked, `verify_cluster_tag_inheritance`): outputs must keep `input tags × allowance(youngest ring member age)`, and any extra drop pays the #925 charge without the epoch-0 holiday (#1685); the decay change in `botho/src/wallet.rs`, `botho-wallet` and the web signer (which publishes empty tags today). Medium. | — | the CT-economics memo's value-free mass floor (D1–D3) | **medium**, coupled to #925, #1685, #1687 and #1756 |
| R5 | R2 + R4 | as R1 | as R1 + R4 | **very high** |

## 7. Why no studied candidate passes all three

This section describes the pattern across the candidate families the study built (§1). It is an argument over those families, not a proof about every possible lottery rule, and it is conditional on the current fee structure and tag design. §7.1 lists shapes it does not cover.

- **Among the studied families, splitting neutrality came from value-linear weight or from a priced ticket.** Under #1761's stationary-economy premises, a per-output ticket priced below its expected payout is a subsidy the modeled splitter can farm; this is not a measured current production profit rate. A value-linear weight cannot be farmed by splitting.
- **Value-linear weight needs the spent set.** On a ring chain, a re-spent coin's old output keeps its weight unless the node learns that it was spent. That costs a reveal (§5.1) or a non-membership proof (§6). The node's current UTXO set cannot provide it (§2).
- **A ticket priced at its expected payout redistributes nothing.** If every ticket costs its expected payout, every holder gets back on average what it paid. The lottery becomes a fair game that burns the emission share. In §3, R3 does no better than the baseline in any cell (best 3-seed mean +0.0033).
- **Value-linear weight does not reach the bar even with the spent set.** Inside a 10,000-block (about 14-hour) circulation window, value × age rewards outputs that were recently created and are large. That is turnover, not need: §3.3 shows merchants taking most of the lottery. Value-linear weight also cannot pay the poor more than their hashpower under egalitarian mining, which is the scenario in which a lottery would have to redistribute.
- **Giving up privacy or split-neutrality did not reach the bar either.** The data does not show a trade-off in which two of the three properties are reachable. Every configuration measured falls short on Gini, including the ones that give up an axis:
  - value-linear weight with the spend reveal (R1, R2, R5 under #1790's rule; not private) peaks at +0.0317;
  - the node's own rule, spent outputs staying eligible (not split-neutral), peaks at +0.0726 (R0, wealth mining, 1000x), at one horizon in one cell;
  - R4 keyed to the real input's age (not private, and not split-neutral) passes 1 of 6 cells and gives −0.46 to −0.60 under egalitarian mining at 100x and 1000x.

Within the studied families, then: the rules that read only public, unlinkable data either subsidized splitting (R0, R4p) or paid out nothing net (R3), and the rules that read value needed the spent set (R1, R2, R5) and stayed below the bar even with it. No studied rule reached the bar on any terms.

### 7.1 Shapes the study did not cover

The argument above says nothing about rule shapes outside the families built here. Examples:

- **A factor-dependent ticket price**, cheaper for low-factor outputs. This is R3 with a progressive subsidy. Whether the subsidy can be farmed by splitting low-factor outputs was not measured.
- **A churn fee proportional to the value moved.** This would price the §4 churn attack under the node's rule directly, instead of requiring the spent set, and could make value-linear weight split-safe without a reveal. Its effect on honest payments was not measured.
- **Redistribution through the demurrage side** (who pays, #1790 cause 3) rather than through ticket weighting.
- **A different lottery window**, which changes how much value-linear weight tracks turnover (§3.6).

All four have since been built and measured on this study's harnesses and bars in the [redistribution shapes study](redistribution-shapes-study.md) (#1987, from the #1837 ruling). None passes all three bars, and none passes Gini in any cell.

## 8. Ranked recommendation (no decision stated)

This ranking orders the candidates by how much of the operator's requirement each one could carry. It is not a decision.

1. **R3 (tickets priced at their expected payout).** This is the only candidate that passes two axes: splitting (0/192 profitable, by construction) and privacy (if purchase is mandatory, it reads only public counts). It works the same with or without confidential amounts, and its complexity is medium. It fails Gini by design. A fairly priced ticket returns on average what it costs, so the lottery stops being a redistribution channel (best 3-seed mean +0.0033). Under the study's pricing/eligibility premises, adopting it would close the modeled splitting hole (#1682) and would amount to retiring the lottery's redistributive role, which is consistent with the restatement in #1800. If the operator wants the lottery to redistribute, R3 is not the answer.
2. **R2 (value × (7 − factor)), with R5 as its variant.** It has the best lottery-side Gini (up to +0.0265 for R2 and +0.0290 for R5 as 3-seed means, under wealth mining at 1x; at most +0.0294 under the node rule), and on public amounts it is a configuration switch, because the kernel and the per-candidate factor already exist. But it is split-safe only if spent value stops counting. That requires a claim-time key-image reveal (privacy FAIL; §5.1: for inputs held past the window, which is typical under the gamma spend-age distribution, 2–55% of rings are deanonymized outright and a uniform guess identifies the real input in 7–60% of rings instead of 5%, depending on output density; for an input spent at age 1,000, 0.1–2.5% outright and 5.2–13.4% by a uniform guess) or a ZK non-membership proof (very high complexity, no existing support). Under CT it also needs the #902 ZK sort. Even with all of that, it does not reach 0.05.
3. **R1 (value × age).** Same blockers as R2, with no tilt and slightly more code. Its Gini numbers are similar (best +0.0317, under wealth mining at 1x). The age term adds nothing in steady state and penalizes frequent spenders.
4. **R4 (per-block tag decay).** Its two forms differ. Keyed to the real input's age (R4), it addresses the stated density mechanism, since demurrage survives churn, and it is the only rule with a passing cell (wealth, 1000x). But it identifies 93–100% of rings at real ages of 30,000 blocks and more (§5.3), and its net effect is dominated by who pays: strongly regressive under egalitarian mining at density (−0.46 to −0.60). Keyed to the ring's youngest member (R4p), it is private, but tags then effectively never decay, and it passes no cell (best +0.0163; −0.09 to −0.48 under egalitarian mining, §3.1.1). Both leave the lottery unchanged (192/192). It would be worth revisiting only together with a fix for how demurrage falls on payment recipients (the velocity mispricing, #1790 cause 3) and with #1756.
5. **R0 (current-rules model).** It fails Gini in the separate M2 study and splitting in the inherited stationary grid. Its only advantage is that it exists.

Across all five, the study supports this reading for the candidate families it built: no measured configuration reached the 0.05 bar, including the ones that gave up privacy or split-neutrality (§7). That is a statement about these families under the current fee structure and tag design, not about every possible rule (§7.1).

One reading of the operator's options, offered as a map rather than a choice set:

- keep the lottery as a non-redistributive mechanism and close the splitting hole (R3);
- retire the lottery, or its redistributive role;
- pay for the spent-set proof that value-linear weighting needs, knowing that the measured gain stays below 0.05;
- drop or restate the redistribution claim (#1800);
- look for redistribution beyond the lottery, for example on the demurrage side, or in the shapes §7.1 lists and the study did not build;
- commission further measurement before choosing.

**No decision is stated here. Ratification is the operator's.** In particular, nothing here says whether to adopt any candidate, change a constant, restate the M2 criterion (#1800), or retire the lottery's redistribution role.

## 9. Reproduce

```bash
cargo build --release -p bth-cluster-tax --features cli
B=target/release/cluster-tax-sim
$B redistribution-redesign --part splitting          # §4 (golden-checked)
$B redistribution-redesign --part privacy            # §5 (golden-checked)
# §3: one run per call, prints a JSON line per snapshot (10 and 20 yr)
$B redistribution-redesign --part m2 --variant {baseline,value-age,inverse-factor,ticket-price,time-decay,inverse-factor+time-decay} \
   --mining {wealth,egalitarian} --seed {1696,2,3} --tx-scale {1,100,1000} --run {on,off} [--spent-eligible] [--whale optimal-decoy] \
   [--decay-key youngest]   # R4p / R5p (§3.1.1): time-decay variants only
cargo test --release -p bth-cluster-tax --features cli --lib redistribution_redesign
cargo test --release -p bth-cluster-tax --features cli --lib baseline_reproduces_1790   # R0 = #1790, bit for bit
```

The 304 m2 runs (on/off pairs: 6 variants × 2 mining models × 3 seeds × 3 densities with the #1790 rule; R0–R3 again with spent outputs eligible; the optimal-decoy whale at 1x; and R4p and R5p, 72 runs, with `--decay-key youngest`) take 20 s to 5 min each on an Apple M-series laptop. The uniform-ticket rules are the slow ones. The first 232 were run in parallel at `5bb7969e` plus this change. The 72 R4p/R5p runs were added in review, on the rebased branch (the harness change only adds the `--decay-key youngest` arm, so the other runs are unaffected); they took 8 minutes on 24 cores.

## 10. Approximations, and which way they lean

Everything listed in the #1790 record still applies. The new ones:

- **Ticket weights are fixed at window entry for R2.** W only grows, so a factor taken at entry is at most the factor at the draw. This slightly overweights high-W outputs, which leans in the mechanism's favour (more tilt).
- **R3's price lags by one step** (720 blocks). It is the previous step's total payout divided by the tickets bought in that step, which equals a ticket's expected payout only in steady state (each ticket is eligible for about 12.9 steps). The node would use the parent block's state. In steady state the lag cancels. A sudden drop in organic activity lets anyone buy underpriced tickets for up to one window, and a flood raises the price for the flooder.
- **R4p models the youngest ring member at the 10-block floor**, the least decay any ring allows. A real wallet's youngest ring member is usually older, so the private rule allows somewhat more decay than R4p; R4 (keyed to the real input's age) is the most it could allow.
- **R4 decays the whole position by the time since its last spend.** Coins received since then are over-decayed, which leans against the mechanism.
- **The splitting grid's churn model** prices one churn transaction per block with a 10-block demurrage clock and gives organic outputs a fixed 10-BTH value. The sensitivity table varies capital (1,000 BTH to 5M BTH) and organic value (10 to 1,000 BTH): 180–192 points stay profitable.
- **Claim-reveal** draws wins independently per output, at a constant per-block rate over the 720..10,000 window. The real input is exposed only for the part of its window before its spend, using each ring's own real age, because a spent output forfeits and its owner has no reason to reveal its key image. Each decoy is eliminated at the full-window rate q. That is exact for decoys never spent inside their window and an upper bound for decoys that were (they too could only be revealed before their own spend), so the survivor-based ID columns lean slightly against the rule. The adversary is also assumed not to exploit the asymmetry itself (a young real input is less likely to have been revealed than its decoys), which leans in the rule's favour.
