# Research Documents

This section contains research analysis, comparisons, and technical evaluations.

## Analysis

| Document | Description |
|----------|-------------|
| [Botho vs zkVM Analysis](botho-vs-zkvm-analysis.md) | Comparison with zero-knowledge virtual machines |
| [zkVM Groth16 Analysis](zkvm-groth-analysis.md) | Technical analysis of Groth16 proving systems |
| [Useful PoW: ZK-Proof Generation](useful-pow-zk-proof-generation.md) | Why prime-finding/factoring PoUW is rejected and ZK-proof generation is the only useful-PoW direction that fits Botho (parked) |
| [Product-Decision Synthesis (2026-07)](product-decision-synthesis-2026-07.md) | Roadmap synthesis folding #441/#458 open product decisions into a sequenced plan; regulatory posture, mobile custody UX, DePIN GTM lessons. Two steps are open maintainer decisions. |
| [Settlement-Horizon Calibration](settlement-horizon-calibration.md) | Calibrating `SETTLEMENT_HORIZON_BLOCKS` (#833): wrap-out demurrage-settlement price vs Gini-erosion, across horizon × factor class. |
| [Bridge-Import Calibration](bridge-import-calibration.md) | Calibrating ADR 0007's epoch length `K` + import-factor floor `F` (#937): split-game cost vs innocent-entrant collateral, residual anti-hoarding vs onboarding friction, decay-by-circulation. |

## Purpose

Research documents serve to:

1. **Evaluate alternatives** - Compare different technical approaches
2. **Inform decisions** - Provide data for design choices
3. **Document rationale** - Explain why certain paths were chosen or rejected

## Related

- [Design Documents](../design/) - Proposals based on research
- [Decisions](../decisions/) - Architecture Decision Records
- [Specification](../specification/) - Formal protocol specification
