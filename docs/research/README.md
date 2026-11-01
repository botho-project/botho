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
| [CT-Compatible Lottery Selection](ct-compatible-lottery-selection.md) | Can a value-free lottery selection rule avoid the ZK weighted-sampling sort under confidential amounts (#902)? Scores Uniform / ClusterWeighted / FeeInverse / CirculationGated / Hybrid on redistribution, Sybil resistance, and CT-cleanliness. Finding: value-free rules recover (even beat) the redistribution and are CT-clean, but cannot recover split-invariance from public signals — surfaces a Path A/B/C decision. |
| [CT-Economics Gadgets](ct-economics-gadgets.md) | The two remaining #902 gadgets: demurrage and cluster-tag blending over hidden amounts. Demurrage = a committed term + one-sided Bulletproof inequality (`d ≥ k·V`), ~0.7 KB/spend, reuses ADR-0006 crypto; leakage is factor-conditional (zero for background coins). Tag blend **dissolves** — consensus enforces a mass *bound*, not the exact blend, and both the inflation bound and anti-deflation floor go value-free. Both bottom out on one core: the factor curve's input (cluster wealth), resolved by an octave-bucket proof or a value-free provenance factor (generalizing ADR 0007). Surfaces decisions D1–D3. |
| [CT Provenance-Factor Calibration](ct-provenance-factor-calibration.md) | Decision D2 of the CT reconciliation epic (#902): can a value-free provenance factor replace hidden cluster wealth as the factor curve's input? Design research + simulation, no consensus change. |
| [Demurrage Background-Reset Leak — Verdict](demurrage-background-reset-leak.md) | #834 verdict: the young-coin spend-to-background reset is a real, unpriced class transition (zero-cost demurrage escape); informs the #925 downgrade charge. |
| [Downgrade-Charge Honest-Overcharge Surface](demurrage-downgrade-overcharge.md) | #950 simulation of how much honest users overpay under the #925 downgrade charge across the settlement horizon; calibration artifact for the #925 ratification, not shipped in the node. |
| [Economic Sims Under the Consensus Fee Model](consensus-fee-model-sim-verdicts.md) | #1695: every economic sim now prices fees with the shared `cluster_tax::consensus_fee_model` (golden-pinned to `Ledger::consensus_fee_floor`); wash/structuring/whale/M2/lottery re-run and restated as SURVIVES/FAILS. M2's Δgini PASS flips to FAIL at the real fee floor (#1727). |
| [Decoy-Flood Cost at the Real Fee Floor](decoy-flood-cost.md) | #1701: what it costs to make all 19 decoys the attacker's, at the node's real fee floor and net of lottery recapture, for the CLI, web and node pickers. A uniform flood roughly self-finances in epoch 0 and is profitable from epoch 1 (#1682). Before #1786 the node picker's key-order tie-break let a hash-grinding attacker own every ring with 19 outputs per block; with the shuffled tie-break grinding no longer helps. Proposes a pass criterion (unratified); every cell fails. |
| [M2 at Production Parameters](m2-production-verdicts.md) | #1696: M2 re-run on the node's real parameters (6.3M blocks/yr, mainnet emission and lottery-share ramp, uniform V2 lottery with payouts as outputs, coinbase-lineage clusters, optimal-decoy mining whale) with a mechanism-off counterfactual. The mechanism's ΔGini is between −0.028 and +0.001 (3-seed means) against the 0.05 bar, and below +0.019 at up to 1000x the transaction rate, and 100% of top-decile value prices below 1.5x: the #605 premise is broken (decision in #1727). |

## Purpose

Research documents serve to:

1. **Evaluate alternatives** - Compare different technical approaches
2. **Inform decisions** - Provide data for design choices
3. **Document rationale** - Explain why certain paths were chosen or rejected

## Related

- [Design Documents](../design/) - Proposals based on research
- [Decisions](../decisions/) - Architecture Decision Records
- [Specification](../specification/) - Formal protocol specification
