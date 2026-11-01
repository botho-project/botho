# ADR 0007: Bridge-Import Cluster Tagging via Block-Epoch Keys

**Status**: Accepted (ratified 2026-07-14 with `K` = 1 day = 17,280 blocks and `F` = 1.5×, per the #937/#940 calibration). **Re-calibrated on production semantics 2026-10-31 (#1697). The values are unchanged, but several guarantees below do not hold against the node as built. See §Production-semantics re-calibration.**
**Date**: 2026-07-14
**Decision Makers**: Core Team
**Related**: ADR 0002 (federation custody), ADR 0003 (factor-1 wrap + demurrage-settlement), ADR 0004 (bridge privacy / amount revelation), ADR 0006 (PQ + confidential-amounts target); issues #831, #925, #902, #904, epic #816

## Context

Botho's anti-hoarding mechanism prices **coin lineage**: the cluster-factor curve maps the wealth traceable to a coin's cluster origin(s) onto a demurrage / progressive-fee / lottery-tilt multiplier (1×–6×). It is Sybil-resistant domestically because splitting a coin does not change the wealth traceable to its origin (`check_cluster_tag_inheritance` rejects inflation; value-weighted blending carries provenance through spends).

The BTH↔wBTH bridge (ADR 0003) introduces a vector the lineage mechanism cannot see. wBTH is fungible on external chains; on unwrap the federation mints BTH. Under ADR 0003 as written, **unwrapped BTH returns at factor-1 (background)** — the cheapest, least-taxed class. Two consequences follow:

1. **The entry leak.** External wealth of arbitrary size buys wBTH on a DEX and unwraps into Botho at factor-1, having paid no lineage premium. The mechanism cannot inspect the real-world wealth behind an incoming transfer, and pricing it away would break the cross-asset liquidity the bridge exists to provide. Pre-existing global wealth concentration flows in clean.

2. **The bridge as a lineage-reset laundromat.** A *domestic* holder who accumulated a high-factor lineage (e.g. via mining) can round-trip — BTH → wBTH → unwrap → factor-1 BTH — and reset to background for the price of a wrap. This is a third "reset door" alongside the deliberate wrap-out (#831) and the spend-to-background leak (#925). If any reset door is cheap, the entire cluster-factor mechanism is trivially bypassable: accrue high, reset cheap, repeat — only the unsophisticated pay.

We accept (2026-07-14 design discussion) that (1) is **unavoidable in the general case** — you cannot tax external wealth entry without breaking liquidity — but that (2) is a real defect, and that the entry leak can be *materially narrowed* without touching liquidity by refusing to grant imported wealth the privileged factor-1 status that only domestically-circulated money should earn. Principle: **only money that has circulated within the Botho network is cheap to spend.**

## Decision

**Unwrapping mints BTH into a bridge-import cluster keyed to the block-height epoch of the unwrap, at an elevated factor derived from that epoch cluster's aggregate unwrap wealth, subject to a floor. Imported wealth normalizes toward background only by circulating (blending with domestically-tagged coins through ordinary spends).**

Concretely:

1. **Epoch-keyed import cluster.** Every unwrap in block height range `[mK, (m+1)K)` joins a single shared cluster origin `c_import(m) = H("bridge-import" ‖ m)`, where `m = ⌊height / K⌋` and **`K = 17,280` blocks (1 day at the 5s reference)**. The unwrapped output carries a 100%-weight tag to `c_import(m)`, exactly as a minting output carries 100% attribution to its new mint cluster (a bridge-import cluster is simply a *third way to create a cluster origin*, alongside minting — no new machinery beyond the tag).

2. **Factor from shared epoch wealth.** The import cluster's wealth (the curve input) is the **sum of all unwrap amounts in the epoch**, and the production `ClusterFactorCurve` (log-sigmoid, `W_MID = 100k`, saturating 6×) maps it to the factor — the identical curve domestic clusters use. A quiet epoch with small total inflow yields a low factor; a high-volume / flood epoch yields a high factor.

3. **Import-factor floor `F`.** The effective factor of any bridge-import cluster is clamped to `≥ F`, with **`F = 1.5×`**. This bounds the payoff of split-gaming (below — a split-gamer cannot erode below the floor) and encodes the minimum "toll" for entering via the bridge rather than earning domestically. `F = 1.5` clears the ~1.27× a genuine ~1000-BTH retail import already prices at (so the floor binds), while the transient onboarding toll on a small entrant is ~0.20%/yr and self-heals in ≈9 domestic-mixing spends (#940).

4. **Decay only by circulation.** An imported coin's factor falls solely through the existing value-weighted tag-blending on spends: as it mixes with background-tagged inputs it shifts weight off `c_import(m)` toward background and its factor drops. There is no time-based decay — sitting idle does not normalize imported wealth; *using* it in the domestic economy does.

### Why the epoch key is the load-bearing choice (Sybil-resistance)

The naive alternatives fail:
- **Flat per-unwrap factor** (every unwrap gets a fixed factor) is Sybil-proof but blunt — it over-taxes a genuinely small entrant identically to a whale.
- **Size-based per-unwrap factor** (factor from the individual unwrap amount) recovers size-sensitivity but is **Sybil-able**: a whale drip-splits into N dust unwraps, each a separate small origin at factor ~1, then reassembles domestically.

The epoch key defeats the split because all unwraps in a window **share one accumulating cluster** — intra-epoch splitting piles into the same pool and still hits the high factor. Diluting requires spreading across *epochs*, which costs wall-clock time (`K` blocks each). **Time-as-cost replaces provenance-as-cost** — the bridge-boundary analog of the domestic "you cannot split your way out of a lineage."

### The intrinsic constraint: shared fate is the price of identity-free Sybil-resistance

You cannot have all three of {Sybil-resistant, no-shared-fate, no-external-identity}. The only thing that makes splitting costly without an identity to bind to is forcing split pieces to aggregate into a shared pool — which necessarily means an unwrapper's factor depends partly on strangers who unwrapped in the same epoch. `K` is the dial: short epochs → little co-location collateral but cheaper to split-game over time; long epochs → costlier to game but a small entrant is likelier to be caught in a co-occurring whale's flood.

We judge the shared-fate coupling a **feature, not a bug**: bridge-inflow-*rate* becomes a concentration signal — a sudden capital flood (the inflow that would most dilute the domestic distribution) is treated as maximally concentrated, while an organic trickle enters benignly. The floor `F` plus a modest `K` bound the collateral to innocent small co-entrants, whose small coins additionally blend down quickly through normal spending.

## Consequences

- **Collapses the reset-vector map.** The bridge round-trip now *degrades* lineage (out at factor-1 per ADR 0003's wrap eligibility, back at import-factor ≥ F) instead of resetting it. The bridge is removed from the reset-door list. The only remaining domestic reset door is the spend-to-background leak — **#925 remains the one must-fix**; the #831 settlement charge is no longer partly guarding a round-trip-launder and can be re-scoped (see below).
- **Confidential-amounts-clean by construction.** The epoch cluster's wealth = the sum of unwrap amounts, which are **public at the bridge boundary by necessity** (ADR 0004: lock/unlock reveals the amount). The import factor is therefore computable from already-public data with **no ZK gadget** — it sidesteps, for imported coins, the part of the #902 confidential-amounts problem that would otherwise apply. (Domestic cluster-wealth-under-CT remains #902's hard problem, unchanged.)
- **Deterministic and trust-free.** `m = ⌊height/K⌋`, `c_import(m) = H("bridge-import" ‖ m)` — no operator input, no external identity, trivially consensus-verifiable and auditable. It composes with the existing blend / demurrage / lottery machinery unchanged.
- **Two grades of BTH, accepted.** Unwrapped BTH is more expensive to spend than domestically-circulated background BTH until it circulates. This is continuous with the cluster-tag design (BTH is already non-fungible by lineage); wBTH↔BTH market pricing of the usage-cost differential is left to the market (Core Team, 2026-07-14) — the protocol does not attempt to equalize it.
- **Re-scopes #831 / #925 / the settlement horizon.** With the round-trip door closed, the demurrage-settlement charge (#831) and the #925 fix price only the two remaining doors. The shared-horizon calibration (#833) recommended 5yr against a mass-exodus worst case now judged unrealistic; in light of this ADR the Core Team leans toward a **1-year** lineage-reset horizon (break-even = "you repay ~a year of dodged demurrage, no more"), shared across #831 and #925, with #925 structured as `charge = max(actual_accrued_demurrage, capitalized_reset_charge)` so an old coin spent normally pays its real accrued demurrage and the horizon bites only the young-coin exploit. Those decisions are finalized in the #831/#925 issues, not here; this ADR only records that the reset-vector collapse is what makes a low horizon safe. *(Update 2026-10-31, #1698: the shipped value is `SETTLEMENT_HORIZON_BLOCKS = 5 × BLOCKS_PER_YEAR_5S` (5 years). That matches the code, the whitepaper, and the #833 recommendation. The 1-year lean was never adopted. A re-run of the #833 erosion sweep under the node's actual spend-time demurrage charge (`docs/research/settlement-horizon-calibration.md` §8) still finds that 1yr fails the 0.05 Δgini floor at 50% and 100% top-decile exit, while 5yr passes at both. Moving to 1yr is an open maintainer decision, tracked on #1698.)*

## Calibration (resolved — #937 / #940)

Both constants were calibrated by `cluster-tax/src/simulation/bridge_import_sweep.rs` (deterministic, reproducible; results in `docs/research/bridge-import-calibration.md`) and ratified 2026-07-14:

1. **Epoch length `K` = 17,280 blocks (1 day).** The split game is dead at *every* candidate `K` — a 2M-BTH whale needs 541 separate epochs to drip-dilute an import down to the floor, and "epochs-to-floor" is `K`-independent, so `K` only converts that into wall-clock time (541 days at 1-day epochs). `K` therefore trades only collateral-*probability* (the chance a small entrant lands in a co-occurring whale-flood epoch) against operational legibility; shorter is marginally better on collateral (14 h is the collateral-optimal endpoint) but 1 day was chosen for legibility, since the collateral over-charge is small and self-heals in ≈9 spends. **A future maintainer preferring minimal collateral over legibility may set `K` = 10,080 blocks (~14 h); the sim regenerates the numbers.**
2. **Import-factor floor `F` = 1.5×.** `F = 1.0` is rejected (the whole premium is gameable toward 1× with patience); `F = 1.5` is the residual a split-gamer cannot erode, clears the ~1.27× a retail ~1000-BTH import already prices, and carries a ~0.20%/yr transient onboarding toll.

Decay-by-circulation confirmed against the real `TagVector::mix`: a worst-case 6× flood import blends to the 1.5× floor in ≈9 domestic-mixing spends (most of the drop in the first 4). The ADR §Decision invariant holds: a pure-external holder who never receives domestic value stays at 6× indefinitely (mixing with zero incoming value is a no-op on the tag).

## Production-semantics re-calibration (#1697, 2026-10-31)

The #940 sweep modelled this ADR's text. #1697 rebuilt it on what the node runs: cumulative per-output cluster wealth (`writer.rs:469-500`), the per-spend `max(curve(output-tag wealth), ring-centroid factor, F-blend on output tags)` (`validation.rs:224-246`), production `ClusterTagVector` with the wallet's 5% decay, and `consensus_fee_model` fees. Full numbers are in `docs/research/bridge-import-calibration.md`. Corrections to this ADR:

- **§Decision 2, "wealth = sum of unwrap amounts in the epoch".** On the node, `W[c_import(m)]` is cumulative: every downstream output carrying the tag adds to it, and nothing subtracts. Collateral on a small co-entrant therefore keeps growing after the epoch closes. A 1,000-BTH entrant in a 5,000-BTH flood epoch goes from 1.58× to 3.20× after the flood is spent 20 times.
- **§Decision 3, "~0.20 %/yr onboarding toll".** The floor applies to the demurrage factor, but the claimed (output) class is the plain curve. An honest wallet's first spend of a lone 1,000-BTH import therefore pays #925's capitalized downgrade charge on the 1.475× − 1.254× gap: **0.44% of value**, once.
- **§Decision 4, "decay only by circulation" and §Calibration, "a pure-external holder stays at 6× indefinitely".** Neither holds. The stock wallet decays tags 5% per transfer, so 135 self-spends with no domestic money prune the import tag to background (0.11% fees for a flood coin). A spender may declare any decay, because consensus rejects only inflation. Shedding to weight ÷ 20 per hop takes a 5.44× flood coin to 1.00× in **3 spends for 0.15%**. Dropping all tags in one spend costs 0.06% for a small entrant, because the F-blend is read from output tags. Tracked as #1687 and #1686.
- **§Calibration 1, "541 epochs / 541 days".** Re-run: 584 epochs. `K` is in blocks, so that is 584 days at 5 s, 2,336 days at 20 s and 4,672 days at 40 s. The split-game wall is real but no longer the cheapest escape (above).
- **§Consequences, "confidential-amounts-clean by construction".** This holds only for the epoch's initial unwrap total. The cumulative `W` also adds the amounts of downstream outputs that carry the tag, and those are not public under confidential amounts (#902).
- **Epoch steering.** The relayer chooses `created_at_height` (`bth_scan.rs:379, 420`), and so which epoch id a release carries. An id for an epoch with no recorded unwrap is not in the import set, so no floor applies. Filed separately.

`K` and `F` are **not** changed by this re-calibration. No value of either closes the escapes above; they depend on where the floor is enforced (#1687). Whether `K` should be restated in wall-clock terms, given 20–40 s blocks, is open for a human decision.

## Alternatives considered

1. **Status quo (ADR 0003 as written): unwrap → factor-1.** Rejected — the entry leak and the round-trip laundromat (Context) make the cluster-factor mechanism bypassable via the bridge.
2. **Flat import factor (fixed, size-independent).** Sybil-proof but blunt; over-taxes small legitimate entrants. Subsumed by this ADR's floor `F` as the *minimum*, with epoch-wealth providing the size-sensitivity above it.
3. **Size-based per-unwrap factor.** Sybil-able by drip-split (each dust unwrap a separate low-wealth origin). Rejected — see §Why the epoch key.
4. **Bind import factor to external / recipient identity.** Rejected — no trustworthy external identity exists behind fungible wBTH, and per-Botho-address keying is Sybil-able by address generation.
5. **Time-based decay of the import tag.** Rejected — a patient whale simply waits out the decay to reach factor-1. Decay must require *circulation* (blending), which forces the imported wealth to actually enter the domestic economy to normalize.

## References

- ADR 0003 (factor-1 wrap eligibility + demurrage-settlement), ADR 0004 (bridge amount revelation), ADR 0006 (confidential-amounts target)
- #925 (spend-to-background leak — the remaining domestic reset door), #831 (settlement op), #833 (horizon calibration), #902 (CT↔economics spec epic)
- 2026-07-14 design discussion (this ADR's derivation): the entry leak, the reset-vector map, shared-fate as intrinsic to identity-free Sybil-resistance
- [ADR 0009](0009-confidential-amounts-economics.md) — generalizes this epoch-keyed, circulation-decayed, `F`-floored provenance factor inward to *domestic* mint-origin wealth (the `EpochOrigin` rule), inheriting `F` = 1.5× and `K` = 17,280; the same construction now keys both the bridge factor (public unwrap amounts) and the domestic factor (public coinbase amounts) under confidential amounts
