# Demurrage Background-Reset Leak — Verdict

**Issue**: [#834](https://github.com/botho-project/botho/issues/834)
**Status**: **VERDICT — leak is REAL (option (a))**. An unpriced class transition.
**Date**: 2026-07-14
**Related**: ADR 0003 (demurrage-settlement on-ramp), #831 (settlement charge), #713/#581 (cluster-tag inheritance bound)
**Evidence**: `cluster-tax/src/demurrage.rs::tests::demurrage_background_reset_leak_is_real` (permanent regression test)

## The question

Can a wealthy holder cheaply reset a coin's demurrage class to background via an
**ordinary spend**, escaping the future stock-level demurrage the cluster-tilted
mechanism is designed to extract?

## Verdict

**Yes.** The escape is real and currently **unpriced**. Nothing on the ordinary
spend path charges the coin for shedding its wealthy cluster provenance. This is
in tension with the intent of the churn-invariance property, and it is the same
escape #831's settlement op deliberately *prices* for the wrapping on-ramp — so
leaving it open on the pure on-chain path would make #831 pointless. **Marked a
mainnet blocker.**

## Why the existing anti-gaming floors do NOT price it

The consensus demurrage term (`Ledger::consensus_fee_floor`,
`botho/src/ledger/store.rs:1784`) is:

```
demurrage = demurrage_charge(
    output_sum,
    max(claimed_factor, ring_centroid_implied_factor),  // the factor floor (B2)
    ring_elapsed_quantile@max,                            // the age clock (B1)
    rate_bps(height),
    blocks_per_year,
)
```

and (`cluster-tax/src/demurrage.rs`):

```
charge = value × rate × (factor − 1)/(max_factor − 1) × elapsed / blocks_per_year
```

Two floors already defend this charge, and the leak slips **between** them:

1. **The factor floor** (`ring_centroid_implied_factor`, item B2) stops a spender
   from *claiming* a background factor while spending a wealthy coin: the ring
   members carry public, inherited tags the spender cannot rewrite, so the factor
   is floored at what the ring composition implies. This works — but it only
   controls the **factor** term.

2. **The age clock** (`ring_elapsed_quantile@max`, item B1) stops a spender from
   diluting the **elapsed** term to zero with *fresh decoys* when the real input
   is *old*. It surfaces a lone old real input as the ring maximum.

Neither floor helps when **the real input is genuinely young**. The charge is
*proportional to elapsed*. A freshly-created wealthy coin has `elapsed ≈ 0`, so
even with the factor floored to its true wealthy value (5.745× in the test), the
product is `≈ 0`. The factor floor multiplies a near-zero quantity and does
nothing.

## The exact leak sequence (with observed numbers)

Numbers below are the assertions in the regression test, for a **1,000 BTH** coin
tagged into a **10M BTH** cluster, at the default factor curve and 2%/yr rate.
All values are in picocredits.

| Step | Factor applied | `elapsed` | Demurrage charged |
|------|---------------|-----------|-------------------|
| Honest baseline: hold 1 yr, then spend as wealthy | 5745 (ring floor) | 1 year | **18,980,000,000,000** (≈18.98 BTH) |
| Step 1: re-spend wealthy→wealthy to reset `created_at` | — | pays accrued only (≈0 if timed) | ≈0 |
| Step 2 (**the leak**): spend young wealthy → background | 5745 (ring floor *still applies*) | ≈0 | **0** |
| Future: spend the reset background coin, held 1 yr | 1000 (1×, background ring) | 1 year | **0** |
| **Total paid to escape the wealthy class** | | | **0** |

- **Step 1** is standard UTXO mechanics: a new output's `created_at` is the
  current block height (`store.rs:1054/1088/1142`), so re-spending resets the
  clock. The attacker times Step 2 to follow immediately, so the age they must
  pay for is ≈0.
- **Step 2** is legal deflation. `check_cluster_tag_inheritance`
  (`store.rs:116`, `CONSENSUS_TAG_DECAY_RATE = 0`) rejects only *inflation*
  (`actual > expected = input_mass`). Dropping cluster mass to background is
  permitted by design.
- **Future**: the reset coin genuinely carries background tags, so any future
  ring composed of background members implies factor **1×** — the coin is now a
  clean commerce coin and owes zero demurrage forever.

Net: a whale that periodically (a) re-spends to reset age and (b) immediately
spends young→background pays **0** to leave the demurrage regime, versus the
**~1.9%/yr** an honest wealthy holder pays. The stock-level charge is fully
escaped.

## Resolution of the churn-invariance tension

The issue framed this as: either (a) churn-invariance holds only *within* a
wealth class and the class transition is the leak, or (b) something already
prevents/prices the transition. The answer is **(a)**.

Churn-invariance (`demurrage.rs` header) is invariance of *accrued* demurrage
under re-spending at the **same factor**: `charge(T) = charge(T/2) + charge(T/2)`
(`test_churn_invariance`). It says nothing about a **change of factor**. The
demurrage charged at any spend is *accrued-to-date* (`∝ elapsed`), never
*capitalized future*. So a coin can walk down from a wealthy class to background
paying only the accrued cost of the tiny age it happens to carry at the moment of
the walk — which the attacker drives to ≈0 by resetting age first. The header has
been amended to record this scope limit next to the churn-invariance claim.

## The symmetry with #831 / ADR 0003

ADR 0003 established that the **only sanctioned way to shed cluster provenance is
to pay for it**: the demurrage-settlement op reclassifies a wealthy coin to
factor-1 and charges a settlement fee routed to the lottery pool, and #831
resolves that fee to be **churn-invariant capitalized** demurrage (full
prospective, not accrued-to-date). That op exists precisely because an *unpriced*
downgrade would be "a straight redistribution leak, not a paid transition"
(ADR 0003, Alternative 4).

This issue shows the **same unpriced downgrade already exists purely on-chain**,
via an ordinary spend, with no bridge involved. If it stays open, a wealthy
holder never needs the settlement op at all — they can shed provenance for free
on the base layer, and #831's paid on-ramp becomes decorative. The fix must
mirror #831: **price the class transition on any deflating spend.**

## Proposed fix (tracked as a mainnet blocker)

**Candidate mechanism**: when a transfer's output cluster mass drops below the
ring-implied input cluster mass (a demurrage *class downgrade*), charge the
**capitalized future demurrage** for the mass being shed, in addition to the
accrued-to-date charge — exactly the settlement-charge formula #831 uses, applied
automatically to the deflation. This makes an ordinary spend-to-background
economically identical to using the settlement op, so there is no cheaper path
out of the regime. The charge is a pure function of public ring/output data and
per-cluster wealth (the same inputs the existing floors already read), so it stays
consensus-deterministic and liveness-safe (it only ever *raises* the fee floor,
never false-rejects a valid spend).

Design tensions to weigh in the fix issue (carried from the issue's scope list):
capitalization horizon vs. churn-invariance, ring-signature privacy (the charge
must not leak which member is real), and not over-charging honest background-to-
background spends (the charge must key off an actual downgrade relative to the
ring floor, which for a genuine background spend is already 1× → no charge).

**Fix issue**: [#925](https://github.com/botho-project/botho/issues/925) —
*price the demurrage class transition on deflating spends*, filed as the fix for
this leak and marked mainnet-blocking (`priority:high` + `loom:urgent`).

## Reproduction

```
cargo test -p bth-cluster-tax --lib demurrage_background_reset_leak_is_real
```

The test asserts every number in the table above and will begin failing the
moment the class transition is priced — the intended signal that the leak is
closed.
