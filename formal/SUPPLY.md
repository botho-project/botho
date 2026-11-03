# Block supply accounting (#1839)

`Botho/Supply/Accounting.lean` proves the scalar accounting step using the
existing lottery models. It does **not** close #1839 or prove that every
accepted Rust block satisfies the step's premises. Source references below
were checked at `2c8d5cd2`.

## Fee truncation result

**Proven (model):** for every fee and every configuration,

```text
pool + burn = max(fees, pool)
```

`splitFees_total` includes the `as u64` truncation. The burn share is
recomputed as `fees.saturating_sub(pool)` after that cast. Thus truncation
cannot silently destroy any of the original fees. An invalid fraction can
instead create value: `oversized_fraction_creates_value` checks that one
unit of fees with `pool_fraction_permille = 2000` produces a two-unit pool.
This is a configuration counterexample, not an accepted-block exploit.

**Proven (model):** `splitFees_exact`, `splitFees_conserves` and
`splitFees_rounding` show that for a `u64` fee and a fraction at most 1000,
the cast is lossless, pool plus burn equals the fee, and the rounding
remainder goes to burn. The difference from the exact rational burn is
nonnegative and strictly less than one unit. The consensus constructor
`botho/src/ledger/lottery_v2.rs::canonical_config` fixes the fraction at 800.
The original DRT's truncating cases use fractions above 1000; they are
evidence about the helper's unrestricted domain, not silent consensus burns.

## Accounting identity

Let `U` be economic unspent value, `P` lottery carryover, `B` cumulative
fee burn, and `M` cumulative minted reward. The invariant is
`U + P + B = M`. In a block let `I` be spent value, `O` ordinary outputs,
`F` fees, `R` reward, `E` lottery emission and `D` actual distribution:

```text
I = O + F
U' = U - I + O + (R - E) + D
P' = P + E + fee_pool - D
B' = B + fee_burn
M' = M + R
```

`apply_conserves` proves the invariant after this step. It calls the actual
existing Lean models, including their saturating arithmetic; it does not
replace them with an idealized pool addition. `pool_inflow_fits` derives
that the additions cannot saturate from the pre-state invariant, spending
bound and total issuance budget. `counters_fit` bounds both cumulative Rust
`u128` counters. `issuance_step_fits` preserves `M ≤ height * u64::MAX` and
derives the next `u128` budget for a checked height successor and `u64`
reward. Genesis has zero reward in `Block::genesis_for_network`.

`arbitrary_pool_saturates` records the negative result outside that budget:
an arbitrary pool of `u128::MAX` loses a one-unit emission to saturation.
It is not a reachable-state counterexample. Reachability cannot be omitted
when stating conservation of `compute_pool_accounting`.

Demurrage, settlement and downgrade charges are components of the fee floor
in `store/validation.rs::consensus_fee_floor` and its settlement checks.
They are already included in `F`; subtracting them as separate burns would
double count them. `store/writer.rs::block_effects` records only the validated
lottery fee-burn share in the cumulative burn counter.

## Premises still to connect to the ledger

| Premise | Rust source / remaining obligation |
|---|---|
| `I = O + F` | `transaction/clsag/src/lib.rs::Transaction::verify_ring_signatures` checks per-transaction sums with `checked_add` and exact equality. Formalize the list aggregation and connect pseudo-output amounts to actual spent values. |
| `I ≤ U` | Trust CLSAG soundness, authenticated ring-member resolution, unique key images and fresh output identities; prove that their accepted-block composition means distinct economic coins are consumed at most once. The arithmetic theorem does not establish this. |
| `F ≤ u64::MAX` | `lottery_v2::drawing` and `store/validation.rs` reject a fee-sum overflow. Model the checked fold. |
| `E ≤ R` | `cluster-tax/src/monetary.rs::lottery_emission_bps` caps the rate at 5000/10000; the exact schedule/model bridge remains #1710. |
| `D ≤ available`, payout outputs sum to `D` | Connect `Draw`, V2 payout validation and no-winner behavior; #1849 covers the lottery conservation proofs. |
| Genesis and issuance induction | Bind the zero genesis state, checked successor and reward width to the state transition. `issuance_step_fits` supplies the arithmetic induction step. |
| The stored transition equals `apply` | Audit/DRT of the complete accepted-block writer, including transaction atomicity and identity/collision assumptions, remains open. |

**Trusted (ledger/cryptography):** these premises must be named when citing
`apply_conserves` as evidence about Rust. In particular, `U` is not the sum
of records in `utxo_db`: the database retains spent outputs as ring decoys,
and spending records key images. A raw database sum is not a supply measure.
Confidential amounts (#904) would replace the public-amount balance bridge
with a commitment-balance argument; the scalar identity alone cannot do so.

## Validation

The new theorems are listed in `Theorems.lean` and checked by the existing
sorry gate, axiom gate and independent kernel replay. Their statement
records must land before the proof PR, as required by the base-owned record
workflow. No additional axiom or trusted executable proof tactic is used.

The existing DRT legs `split_fees`, `compute_pool_accounting` and
`carryover_after` test the Rust primitives reused by this proof. Those legs
do not test the new aggregate `apply` projection or establish the open
ledger premises above. Whole-block DRT remains part of #1839.

At source `8085ad6a`, seed 1839 passed 100,000 cases per primitive plus two
saved regressions, with zero mismatches. The
[receipt](../formal-drt/results/supply-1839.json) pins the source, toolchains,
input-file hashes and branch coverage: 8,355 truncating fee casts, 16,344
pool-saturation cases, and 29,150 carryover underflows. These unrestricted
helper inputs do not establish that such states are reachable in consensus.
