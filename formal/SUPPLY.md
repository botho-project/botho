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

## Actual accepted-block oracle (#1889)

`botho::ledger::formal_drt_supply` is available only with `test-fixtures`.
The isolated `formal-drt` crate exercises it with:

```sh
cd formal-drt
cargo test --release supply_oracle -- --nocapture
```

This oracle observes real `Ledger::seal_block` / `Ledger::add_block`
commits, including the production signature and checked-balance callback,
key-image collision checks, V2 context writes and LMDB transaction commit.
A separate read-only invocation of the production pre-writer V2 checks
identifies the rejection stage; it does not replace validation in
`add_block`. Database, encoding and other local errors fail the fixture;
only `LedgerError::is_block_fault()` outcomes count as consensus rejection.

The bounded fixture starts at fresh genesis and changes the stored
difficulty to `u64::MAX`, matching the existing activation-test profile.
Every header still passes actual PoW verification. Lottery maturity remains
720 blocks, and candidate, fee and transaction validation rules are
unchanged. This is a **genesis-origin chain under an altered test profile**,
not canonical production-schedule reachability. It uses neither seeded
snapshots nor the drawing double. Arbitrary storage fixtures and
premise-satisfying scalar examples remain different evidence domains.

Each observed case records the full signed candidate block, pre/post chain
state and pool, every persisted output and derivation context, and watched
key-image state. Newly persisted amounts are partitioned into coinbase,
ordinary and lottery outputs. The independent ghost map contains the real
source outpoints known to the fixture signer. It consumes each accepted
source once and adds actual persisted outputs; the raw row sum is never
called economic unspent value. The checked comparison is:

```text
new_output_value + pool_after + burn_delta
  = known_real_spent_value + reward + pool_before
```

This cross-addition form permits a decreasing pool. Demurrage and settlement
charges remain components of fees and are not subtracted again. The
accounting comparison does not prove source ownership, general output
spendability, CLSAG soundness or all accepted-block supply invariants.

The profile covers zero fees with no candidates and carryover, three fresh
signed transfers, real V2 payouts at maturity, a subsequently signed payout
spend, replay, two independently signed transactions spending one source,
and a signed-output mutation. The latter two are sealed with valid V2 body
roots, pass the pre-writer check and fail in the real transaction/writer
path. A payout-binding mutation is an earlier-stage control. Rejections
compare complete observation fingerprints before/after and after closing
and reopening LMDB. Accepted observations are also checked after reopen.
An executable negative control changes observed `pool_after` by one,
requires the comparison to fail, restores it, and requires success.

The [oracle receipt](../formal-drt/results/accepted-block-oracle-1839.json)
records measured runs. `SUPPLY_ORACLE_CASE` logs separate signing, sealing,
precheck, actual add-block and observation costs. The receipt separates
wallet initialization and cold chain construction from those steady-case
costs. `SUPPLY_ORACLE_DIR` selects the retained evidence directory: it holds
the entire signed chain and per-case observations/known inputs, with hashes
in `receipt.json`. These are replay inputs, not cached acceptance counts.
CI uploads them with the unit-test log for 14 days.

The existing release unit-test step runs the new tests without a filter,
with a 420-second command limit and eight-minute step limit. The profile
also checks a 360-second execution bound. The separate five-minute random
DRT tier is unchanged. There is no new random leg or nightly campaign in
this stage: #1890 must use the measured real-verification costs to choose
its fixture reuse and account for the existing million-case nightly budget.
No Lean theorem or statement record is added here; the parent #1839 and
its model-composition child #1890 remain separate work.

The first optimized Linux CI measurement at PR head `87bc3798` passed all
three unfiltered unit tests (two oracle tests) in 18.99 seconds. Its oracle
profile committed 743 blocks, with seven observed accepted scenarios, four
intentional consensus rejections and zero unexpected infrastructure errors.
Cold chain construction took 17.90 seconds; the three fresh ordinary
signed commits took 6.06–6.24 ms each, with 6.57–7.03 ms signing costs.
The mature payout spend took 7.97 ms in `add_block`, plus 7.90 ms signing,
3.28 ms precheck and 4.85 ms observation. Reopening, artifact writing and
setup are additional costs, not hidden in the commit timing.

A million cases in 60 minutes on four ideally scaling workers allows only
14.4 ms per case. Even the measured ordinary signing/sealing/precheck/
commit/observation totals exceed that budget. This is arithmetic on a small
measurement, not a parallel-throughput benchmark. Stage #1890 must measure
its fixture and observation strategy; it cannot assume the nightly budget
fits, omit real verification or count cached acceptance as fresh cases.
