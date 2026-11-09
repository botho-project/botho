# Research Documents

This section contains research analysis, comparisons, and technical evaluations.

## Analysis

| Document | Description |
|----------|-------------|
| [Botho vs zkVM Analysis](botho-vs-zkvm-analysis.md) | Comparison with zero-knowledge virtual machines |
| [zkVM Groth16 Analysis](zkvm-groth-analysis.md) | Technical analysis of Groth16 proving systems |
| [Useful PoW: ZK-Proof Generation](useful-pow-zk-proof-generation.md) | Why prime-finding/factoring PoUW is rejected and ZK-proof generation is the only useful-PoW direction that fits Botho (parked) |
| [Product-Decision Synthesis (2026-07)](product-decision-synthesis-2026-07.md) | Roadmap synthesis folding #441/#458 open product decisions into a sequenced plan; regulatory posture, mobile custody UX, DePIN GTM lessons. Two steps are open maintainer decisions. |
| [Settlement-Horizon Calibration](settlement-horizon-calibration.md) | Calibrating `SETTLEMENT_HORIZON_BLOCKS` (#833): wrap-out demurrage-settlement price vs Gini-erosion, across horizon × factor class. Re-run under the node's spend-time charge (#1698, §8): 5yr still the only horizon passing at 100% adoption. |
| [Bridge-Import Calibration](bridge-import-calibration.md) | Calibrating ADR 0007's epoch length `K` + import-factor floor `F` (#937): split-game cost vs innocent-entrant collateral, residual anti-hoarding vs onboarding friction, decay-by-circulation. |
| [CT-Compatible Lottery Selection](ct-compatible-lottery-selection.md) | Can a value-free lottery selection rule avoid the ZK weighted-sampling sort under confidential amounts (#902)? Scores Uniform / ClusterWeighted / FeeInverse / CirculationGated / Hybrid on redistribution, Sybil resistance, and CT-cleanliness. Historical expected-value findings: the tested value-free rules improve modeled redistribution but lose split-invariance; CT compatibility is structural. Later cap/faithful studies have distinct premises. [Evidence ledger and deferred production economics](ct-compatible-lottery-selection.md#evidence-and-claim-reconciliation). |
| [CT-Economics Gadgets](ct-economics-gadgets.md) | The two remaining #902 gadgets: demurrage and cluster-tag blending over hidden amounts. Demurrage = a committed term + one-sided Bulletproof inequality (`d ≥ k·V`), ~0.7 KB/spend, reuses ADR-0006 crypto; leakage is factor-conditional (zero for background coins). Tag blend **dissolves** — consensus enforces a mass *bound*, not the exact blend, and both the inflation bound and anti-deflation floor go value-free. Both bottom out on one core: the factor curve's input (cluster wealth), resolved by an octave-bucket proof or a value-free provenance factor (generalizing ADR 0007). Surfaces decisions D1–D3. |
| [CT Provenance-Factor Calibration](ct-provenance-factor-calibration.md) | Decision D2 of the CT reconciliation epic (#902): can a value-free provenance factor replace hidden cluster wealth as the factor curve's input? Design research + simulation, no consensus change. |
| [Demurrage Background-Reset Leak — Verdict](demurrage-background-reset-leak.md) | #834 verdict: the young-coin spend-to-background reset is a real, unpriced class transition (zero-cost demurrage escape); informs the #925 downgrade charge. |
| [Downgrade-Charge Honest-Overcharge Surface](demurrage-downgrade-overcharge.md) | #950 simulation of how much honest users overpay under the #925 downgrade charge across the settlement horizon; calibration artifact for the #925 ratification, not shipped in the node. |
| [Ring-Floor Adversarial-Dilution Sweep](ring-floor-dilution-sweep.md) | #1686 (operator: explore before ruling): the value-weighted ring-centroid floor vs amount-matched, max and quantile floors on the ring-20 × split × decoy-value dilution grid, honest over-charge on the #1757 production-route rings, privacy/decoy diversity, multi-input composition and CT compatibility. Only the max-type rules pass the 95% bar; ranked recommendation, no decision stated. |
| [Demurrage Time-Quantization: Candidate Fixes](demurrage-quantization-study.md) | #1875 (operator ruling: simulate before choosing): the six-block dead zone and per-spend truncation in `demurrage_charge`, scored for defer / exact u128 / round-up variants / consensus minimum ring age. Covers the churn attack's net escape at the real fee floor, honest extra charge and rounding error against the exact rational, split-versus-unsplit loss, young-coin spendability and privacy, and overflow at extremes. Ranked recommendation, no decision stated; the operator picks the fix for protocol 9.0.0. |
| [Economic Sims Under the Consensus Fee Model](consensus-fee-model-sim-verdicts.md) | #1695: every economic sim now prices fees with the shared `cluster_tax::consensus_fee_model` (golden-pinned to `Ledger::consensus_fee_floor`); wash/structuring/whale/M2/lottery re-run and restated as SURVIVES/FAILS. M2's Δgini PASS flips to FAIL at the real fee floor (#1727). |
| [Decoy-Flood Cost at the Real Fee Floor](decoy-flood-cost.md) | #1701: what it costs to make all 19 decoys the attacker's, at the node's real fee floor and net of lottery recapture, for the CLI, web and node pickers. A uniform flood roughly self-finances in epoch 0 and is profitable from epoch 1 (#1682). Before #1786 the node picker's key-order tie-break let a hash-grinding attacker own every ring with 19 outputs per block; with the shuffled tie-break grinding no longer helps. Proposes a pass criterion (unratified); every cell fails. |
| [M2 at Production Parameters](m2-production-verdicts.md) | #1696: M2 re-run on the node's real parameters (6.3M blocks/yr, mainnet emission and lottery-share ramp, uniform V2 lottery with payouts as outputs, coinbase-lineage clusters, optimal-decoy mining whale) with a mechanism-off counterfactual. Under #1790's removal rule the mechanism's ΔGini is between −0.028 and +0.001 (3-seed means) against the 0.05 bar, and below +0.019 at up to 1000x the transaction rate; under the node's candidate rule (ring-spent outputs stay eligible, #1808) it is −0.0002 to +0.003, and no case clears the bar at both 10 and 20 years at up to 1000x; and 100% of top-decile value prices below 1.5x: the #605 premise is broken (decision in #1727). |
| [Redistribution Redesign Study](redistribution-redesign-study.md) | #1801 (operator ruling on #1727 and #1682): five sim-only candidates (value × age tickets, inverse-factor tickets, tickets priced at their expected payout, per-block tag decay, and a combination) scored on Gini (#1790 harness, 1x/100x/1000x, 3 seeds), splitting (the historical #1761 stationary evaluator; [scope](ct-compatible-lottery-selection.md#evidence-and-claim-reconciliation)) and privacy (#1757 routes), plus node-side complexity. None passes all three axes. Also finds that the node keeps ring-spent outputs in the lottery window (#1808). Ranked recommendation, no decision stated; ratification is the operator's. |
| [Redistribution Shapes Study](redistribution-shapes-study.md) | #1987 (operator ruling on #1837, option 2): the four shapes the redesign study listed as unbuilt (a factor-dependent ticket price, a churn fee on value moved, redistribution through demurrage, a different lottery window), built as six configurations and scored on the same Gini grid (both candidate rules), #1761 splitting grid and #1757 privacy routes, plus node-side complexity and confidential-amount compatibility. None passes Gini in any cell; R3 remains the fallback. Ranked recommendation, no decision stated. |

## Regenerated tables

CI regenerates the quick, deterministic `cluster-tax-sim` reports and fails on any drift
(`scripts/research/check_research_tables.py`, #1702). The committed outputs live in
[`generated/`](generated/). Reports that paste the tables verbatim are also checked
against the doc. After an intentional sim change, run the script with `--update` and
update the affected report in the same PR. The script lists the generators it skips
because they take minutes.

## Purpose

Research documents serve to:

1. **Evaluate alternatives** - Compare different technical approaches
2. **Inform decisions** - Provide data for design choices
3. **Document rationale** - Explain why certain paths were chosen or rejected

## Related

- [Design Documents](../design/) - Proposals based on research
- [Decisions](../decisions/) - Architecture Decision Records
- [Specification](../specification/) - Formal protocol specification
