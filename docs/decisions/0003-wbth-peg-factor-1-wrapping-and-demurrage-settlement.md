# ADR 0003: wBTH Peg — Factor-1 Wrapping + Demurrage-Settlement On-ramp

**Status**: Accepted
**Date**: 2026-07-13
**Decision Makers**: Core Team
**Related**: [Epic #816](https://github.com/botho-project/botho/issues/816), issues #818, #825, #831; ADR 0002, 0004, 0005

## Context

The bridge locks native BTH in a reserve and mints wBTH 1:1 on Ethereum/Solana. For the peg to hold, the locked reserve must remain equal to the outstanding wrapped supply **over time**, not just at the instant of locking.

Botho applies **demurrage**: a holding charge on wealthy-cluster coins, paid at spend time, that funds the redistribution lottery pool (`cluster-tax/src/demurrage.rs`). The charge is:

```
charge = value × rate × (factor − 1)/(max_factor − 1) × elapsed / blocks_per_year
```

where `factor` is the coin's cluster factor (1.0×–6.0× in `FACTOR_SCALE` units), derived from cluster wealth (`ClusterFactorCurve::factor(cluster_wealth)`). A generic bridge would hit a peg-breaking problem: BTH locked in the reserve would **demur while wrapped**, so the reserve shrinks below the wrapped supply and the bridge silently becomes fractionally reserved.

## Problem Statement

How does the bridge keep `Σ(wBTH outstanding) == locked BTH reserve` given that reserve BTH is subject to demurrage while locked?

## Decision

**Only factor-1 (background/commerce) coins are wrappable, and a new demurrage-settlement operation lets holders pay to reclassify a wealthy coin down to factor-1.** This keeps the peg *by construction* rather than by exemption or rebasing.

1. **Factor-1-only wrapping.** Because of the `(factor − 1)` term, a factor-1 coin pays **exactly zero demurrage, permanently** — independent of `elapsed`. Restricting the reserve to factor-1 coins means the reserve never decays, so the peg invariant holds over time with no special "reserve class" and no consensus exemption. Wrap eligibility is checkable at deposit: the output the bridge receives carries a `ClusterTagVector`, so the bridge reads its factor directly and mints only for factor-1 deposits.

2. **Demurrage-settlement op (the on-ramp).** A holder of a wealthy-cluster coin cannot wrap it directly. A new operation lets them **pay a charge to reclassify the coin to factor-1/background**, after which it is wrappable. The settlement **fee is routed to the redistribution lottery pool** — the same sink demurrage charges already feed. Net effect: the only sanctioned way to shed cluster provenance is to pay for it, and that payment funds redistribution. This is the **one consensus-level piece** of the bridge (new transaction semantics + a validation-rule carve-out in the cluster-tag inheritance bound); it is specified and tracked in **#831**.

3. **Unwrap yields factor-1 provenance.** Releases produce background/factor-1 outputs, matching what went in, so the bridge cannot be used to launder a high-cluster coin into a fresh low-factor one.

4. **Post-wrap demurrage escape is accepted.** Once wrapped, coins sit in transparent wBTH and pay no further demurrage. This is treated as "they settled on the way in," keeping wBTH a clean 1:1 DeFi asset.

## Consequences

### Positive

1. **Peg-stable by construction.** The reserve holds only zero-demurrage coins, so `Σ(wBTH) == reserve` cannot drift from decay — no exemption logic, no rebasing, no operator subsidy.
2. **No hard fork of a live chain.** Botho is pre-mainnet; the settlement op is added before mainnet, consistent with the no-hard-forks-before-mainnet posture.
3. **Aligned with the redistribution design.** The settlement charge funds the lottery pool, so wrapping wealthy value becomes a redistribution event rather than an escape hatch — the wealthy pay their demurrage dues to gain DeFi access.
4. **Cheap eligibility check.** Factor is already carried on outputs; the bridge and the proof-of-reserves reconciler (#825) read it directly.

### Negative

1. **Introduces a consensus-level mechanism.** The settlement op touches the audited cluster-tag inheritance bound (#713/#581) — the one place cluster mass may legitimately drop. It needs consensus-grade determinism and adversarial review (#831), or it becomes a laundering/inflation vector.
2. **Wealthy holders face friction.** They must settle (pay) before wrapping — intended, but a UX cost that must be surfaced clearly.
3. **Permanent demurrage escape post-wrap.** A holder can settle once, wrap, and stay in wBTH demurrage-free forever. Accepted as a deliberate economic stance, but it does let sufficiently motivated wealthy value exit the demurrage regime via the bridge.

### Neutral

1. The peg invariant generalizes across chains: `Σ(wBTH on ETH) + Σ(wBTH on SOL) == locked reserve` (ADR 0005).
2. The settlement charge formula (full prospective demurrage vs. a multiple vs. accrued-to-date) is a follow-on design question resolved in #831; it must be churn-invariant like the existing demurrage.

## Alternatives Considered

### 1. Exempt the reserve from demurrage

- Pro: simple peg; no wrap-eligibility restriction.
- Con: requires the demurrage mechanism to recognize a special reserve class — a consensus change of similar weight without the redistribution benefit, and a standing carve-out rather than a paid, auditable transition.

### 2. Rebasing wBTH

- Pro: exact 1:1 backing maintained as the reserve demurs.
- Con: rebasing tokens are composability-hostile in DeFi — undermines the entire reason to wrap.

### 3. Bridge absorbs the decay (operator top-up)

- Pro: users see a clean 1:1 asset.
- Con: an unbounded operator subsidy and a solvency risk if underfunded.

### 4. wBTH intentionally demurrage-free with no eligibility gate

- Pro: simplest technically.
- Con: lets any wealthy coin escape demurrage by wrapping with no settlement — a straight redistribution leak, not a paid transition.

## Implementation

See #831 (the consensus settlement op: charge formula, tag-rewrite rule, inheritance-bound carve-out, lottery routing, ring-signature interaction) and #825 (reserve accounting / proof-of-reserves consuming the factor-1 invariant). The bridge deposit path (#824/#822) enforces factor-1 eligibility before minting.

## References

- Epic #816; issues #818, #825, #831
- `cluster-tax/src/demurrage.rs`, `cluster-tax/src/fee_curve.rs` (`cluster_factor`), `cluster-tax/src/lottery.rs`
- `botho/src/ledger/store.rs` `check_cluster_tag_inheritance` (#713/#581 bound the settlement op must coordinate with)
- ADR 0002 (custody), ADR 0004 (privacy), ADR 0005 (chain scope)
