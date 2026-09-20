# ADR 0009: Confidential-Amounts Economics — EpochOrigin and the CT Contract

**Status**: Proposed (unchanged; no acceptance or activation implied)
**Date**: 2026-07-16; implementation-evidence correction 2026-09-20
**Decision Makers**: Core Team
**Related**: ADR0006 (hybrid confidential-amount target), ADR0007 (bridge origins),
ADR0003/0004 (settlement and public bridge boundaries); #902, #904, #1301

## Context and authority

ADR0006 sets the target: confidential amounts using Pedersen commitments, public
fees, and universal ML-KEM-768 outputs. Production CLSAG transactions still expose
`pseudo_output_amount`; rings alone therefore do not deliver the target amount
privacy. Mainnet readiness requires implementation and verification, not merely
this ADR.

The original July research resolved important economic choices, but its initial
claim that all CT arithmetic and transaction proof obligations were complete was
too strong. The September integer audit and proof experiments refine that record;
they do not revoke the explicitly ratified EpochOrigin decision. The original
research and ratification remain linked below, including their assumptions.

The [candidate CT1 transaction contract](../design/ct-transaction-contract.md)
contains exact proposed rules, source inventory, executable boundary vectors,
remaining sign-off and implementation gates. Its proposed parameters MUST NOT be
represented as ratified merely by appearing in a specification.

## Decisions already ratified

### Path C lottery

Preserve the value-free uniform draw among circulating outputs, endogenous reward
cap `R=min(actual_fee_pool,rho*base_fee)` with rho counting **all** eligible outputs,
and carry-forward excess. #955/#980 implemented the Path C direction. The research
used a circulation window near10000 blocks and0.25BTH base fee. These historical
calibration inputs must be reconciled with final production parameters and CT1
batch/output fees; they are not permission to silently change current consensus.

The research's splitting argument assumes a cost per created eligible ticket. It
is not a universal proof for arbitrary transaction batching, decoy policy, fee
floors or payout mechanisms. CT1's changed charge/base rules need a fresh attack
and honest-wallet simulation before that economic conclusion is reused. No
hidden-value weighted sampling proof is required by uniform Path C selection.

### D2 — EpochOrigin, F=1.5x and K=17280

[D2 was explicitly ratified in #902](https://github.com/botho-project/botho/issues/902#issuecomment-4986600535).
The factor keys on public mint/coinbase-epoch pool wealth, with origin base floor
F=1500 in units where1000=1x, and epoch length K=17280. This choice generalizes
ADR0007 to domestic origins. It is **not reopened** by the remaining work.

The calibration's `epoch_origin_factor` blends an origin base toward background
using a public origin weight. Its simulated **weight update** uses value-weighted
`TagVector::mix`, which preserves self-spends. The observed Gini recovery and
churn-invariance are evidence for those simulated rules, not for any arbitrary
public tag propagation rule. The accepted drip-mint-across-epochs residual is
unchanged; public lineage remains part of the stated privacy model.

### Public fees and universal hybrid outputs

Preserve public transaction fees and universal ML-KEM-768 encapsulation from
ADR0006. Ordinary transfer values must become commitments linked to the actual
selected ring member. Public bridge boundaries remain explicitly typed exceptions
under ADR0004; they do not permit publication of unrelated private input amounts.

## Remaining precise choices and proof obligations

### D3 — tag inheritance and circulation enforcement

A max-weight upper bound is value-free, but permits zero tags on every output.
It does **not** prove that lower tags came from real background-value acquisition.
A claim that wallet mixing alone enforces circulation-only decay is insufficient.
`ClusterTagInflation` is the correct existing error name; exact value-weighted
mixing is not what that upper-bound check establishes.

CT1 proposes maximum public ring factor plus paid public deflation, charging each
input against the minimum output factor. It gives a concrete dilution attack,
integer fee examples and honest-decoy overcharge consequences. This is a substantive
implementation-policy choice requiring sign-off and new simulations; it does not
inherit the calibration's free-circulation guarantee. The alternative is a fully
linked hidden-value blend proof. Neither choice is silently accepted here.

### Exact charge arithmetic

The production kernel has staged annual and time floors, u128 multiplication
saturation, u64 charge saturation, independently saturated reset terms before
subtraction, and a maximum of accrued and reset charge. It is not exactly one
real-valued coefficient times value. #1266 records counterexamples and a76-bit
slack incompatible with simply calling the existing u64 range-proof API.

#1284 gives an inactive single-input integer construction differentially tested
against the actual Rust kernel. #1288 adds actual R1CS proofs, public statement
binding and adversarial negative controls. The measured experiment has1412
multipliers,2844 linear constraints and1121 serialized proof bytes per case.
These are experimental single-input observations, not whole-transaction sizes or
production verification budgets. Bulletproof R1CS is a distinct experimental path
requiring internal cryptographic review; the former '~0.7KB, two inequalities,
no additional proof review' description is not an implementation contract.

The full transaction still needs original-input/pseudo-output linkage, shared
amount witnesses across all charge branches, exact aggregate public fee/bucket
relation, output ranges, conservation, field no-wrap bounds, transcript/wire
binding and authenticated ledger/client integration. Passing an inactive circuit
experiment does not discharge these obligations.

### D1 — mandatory deterministic quantization

The direction remains mandatory fee quantization; the concrete granularity is
unratified. CT1 compares2/3/4 significant bits and recommends2, with exact integer
preimages and overpayment bounds. It separates the public base and rounds the
aggregate charge once. The public fee must correspond to that exact bucket.

An upper-bound inequality or optional overpayment only proves the fee covers the
charge. It does not prove a two-sided interval or a guaranteed privacy floor.
Small buckets can be singletons, and a coarse charge interval need not give a
nontrivial amount interval once other public information is considered. Zero
charge reveals the zero-charge predicate; it does not establish universal
zero leakage about value. The whitepaper's metadata claims need to reflect these
limits when the final implementation is accepted.

## Current implementation status

- **Age is already value-free:** `Ledger::consensus_fee_floor` calls
  `ring_elapsed_quantile`; #577 is closed. The previous statement that centroid
  age remained live is stale.
- **Factor is not yet value-free:** the live path still calls
  `ring_centroid_floored_factor`, which reads values. Do not conflate factor and age.
- **CT1 remains proposed/inactive:** exact public origin/tag rules, D1, combined
  transaction proof, hybrid encrypted openings and all consumers are not supplied
  by the isolated experiments.
- **No activation is selected:** historical references to a later pre-mainnet
  reset batch were planning context, not standing authorization to reset a chain
  or discard claims. Versioned storage, snapshots/RPC/clients, bridge integration
  and #1286 legacy lottery claim disposition require a concrete reviewed plan.

## Consequences and gates

EpochOrigin and uniform Path C avoid a hidden-wealth factor database and a
value-weighted lottery selection proof. They do not make amount-dependent charging
or full transaction soundness trivial. CT1 supplies a reviewable proposed contract;
implementation, internal security review, adversarial/honest economic simulations,
cross-platform resource measurements and the remaining policy sign-off precede
activation. #902/#904 remain open implementation/specification gates. External
audit engagement is a separate operator action, not authorized by this document.

## Historical evidence and references

- [CT1 proposed contract and rollout decomposition](../design/ct-transaction-contract.md)
- [Path C research](../research/ct-compatible-lottery-selection.md): original
  realized-capture and reward-cap experiments (#952).
- [CT economics gadgets](../research/ct-economics-gadgets.md): original July
  simplifications (#975), qualified by the exact-integer evidence above.
- [EpochOrigin calibration](../research/ct-provenance-factor-calibration.md):
  D2 research (#985), including value-weighted simulation assumptions.
- [#902 ratification](https://github.com/botho-project/botho/issues/902),
  [#904 implementation](https://github.com/botho-project/botho/issues/904).
- [#1266 integer audit](https://github.com/botho-project/botho/pull/1266),
  [#1284 integer construction](https://github.com/botho-project/botho/pull/1284),
  [#1288 proof experiment](https://github.com/botho-project/botho/pull/1288).
