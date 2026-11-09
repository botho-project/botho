# Demurrage counterexamples (#1711)

`Botho/Demurrage/Counterexamples.lean` refutes three proposed properties of
the pre-9.0.0 integer kernels. It does not change consensus behavior.
The six theorems are **proven (model)**; the three DRT legs call the actual
Rust functions to test correspondence. DRT is evidence, not a proof of
Rust-to-Lean equivalence.

Tracked findings: [#1875](https://github.com/botho-project/botho/issues/1875)
(time quantization) and [#1876](https://github.com/botho-project/botho/issues/1876)
(ring saturation). Statement-record review precedes proofs in
[#1877](https://github.com/botho-project/botho/pull/1877).

## Status since protocol 9.0.0 (#1875 fixed)

The time-quantization finding below is **fixed** in protocol 9.0.0: the
operator chose exact arithmetic after the simulation study (PR #1908,
`docs/research/demurrage-quantization-study.md`). `demurrage_charge` now takes
one floor of the full rational
`value × rate × (factor − 1000) × elapsed / (10_000 × 5_000 × blocks_per_year)`,
clamped to `u64::MAX`, with no intermediate time fraction.

- `charge` (`Counterexamples.lean`) is the **legacy** pre-9.0.0 kernel. Its
  theorems (`six_block_dead_zone`, `churn_counterexample`,
  `charge_exceeds_value`, `saturation_lowers_charge`) and their statement
  records are unchanged: they are the machine-checked record of the finding.
- `exactCharge` (`ExactCharge.lean`) is the 9.0.0 kernel, written as the
  specification itself on `Nat` (it does not mirror the Rust u128
  decomposition, so the DRT checks that the decomposition is exact). The
  `demurrage_charge` DRT leg now runs `exactCharge`. Its six claims below
  are listed in `Theorems.lean` and bound in `statements.txt` (#1916, #1963).
- With the DRT dispatch temporarily pointed back at the legacy `charge`, the
  committed regression lines produce mismatches against the 9.0.0 Rust
  kernel, so the leg distinguishes the two kernels.
- `charge_exceeds_value` still holds for the exact kernel (51 years at 2%
  and 6x is 102% of principal): the clamp is to `u64::MAX`, not principal.
- The #1908 study's u128 candidate was exact only for
  `blocks_per_year <= 368_934_881_474` and saturated upward beyond. The
  production kernel splits `elapsed` at 2^32, so it is exact for every input;
  `cluster-tax` tests compare it with a 256-bit oracle on both sides of that
  limit, and the regression file pins two inputs beyond it.

### Exact-charge threshold and splitting (#1916)

Write `p = clamp(factor, 1000, 6000) - 1000`, `K = rate * p * elapsed`,
`D = 10000 * 5000 * year`, `M = u64::MAX`, and
`C(v) = min(floor(v*K/D), M)`. The following are **proven (model)** in
`Botho/Demurrage/ExactCharge.lean`; the existing DRT leg provides separate
evidence of correspondence with Rust.

- `exactCharge_positive_iff`: if year, rate and elapsed are positive and
  factor is greater than 1000, then `C(v) > 0` exactly when
  `v >= (D + K - 1) / K`. This is the sharp ceiling threshold, including
  zero below it. If factor is at most 1000, or year, rate or elapsed is zero,
  the charge is zero; checked examples cover all four guards. A threshold
  above `u64::MAX` cannot be met by a Rust-width input.
- At factor 6000, rate 200, year 6,307,200 and elapsed 1, the threshold is
  **315,360,000 pico**. Kernel-checked examples give zero at 315,359,999 and
  one at 315,360,000. The exact kernel therefore has a value threshold even
  at one block, unlike the legacy six-block dead zone.
- `exactCharge_split_loss_bound`: for any nonempty list `xs` of natural
  values with common parameters and positive year, write `whole = C(sum xs)`,
  `parts = sum(map C xs)` and `n = length xs`. Then `whole < parts + n`,
  including when charges are capped. Zero-valued pieces are allowed. The
  empty list is excluded because its strict inequality would be false.
- `exactCharge_split_rounding`: with the additional aggregate no-cap
  condition `floor((sum xs)*K/D) <= M`, also `parts <= whole`. Together the
  bounds limit pure rounding loss to at most `n - 1` pico. The checked split
  `[315359999, 1]` at the threshold parameters charges zero in parts and one
  as a whole, without clipping.
- `exactCharge_split_clipping_counterexample`: with values
  `[9223372036854775807, 9223372036854775808]`, factor 6000, rate 10000,
  elapsed 2 and year 1, the total value and whole charge are
  `M = 18446744073709551615`, while the parts charge **36893488147419103229**
  (`2*M - 1`). Both values and the parameters fit their Rust widths. This
  synthetic helper-domain witness explains why `parts <= whole` requires
  the no-cap condition; it does not construct a reachable ledger history.

These statements preserve the existing `exactCharge` definition. They prove
arithmetic properties of the model, not Rust-to-Lean equivalence or general
fee-floor, churn, or ledger-reachability claims.

### Exact-charge monotonicity (#1963)

Two additional statements are **proven (model)** for the existing total `Nat`
formula, with all other inputs held fixed:

- `exactCharge_mono_elapsed`: `e1 ≤ e2` implies
  `exactCharge value factor e1 rate year ≤ exactCharge value factor e2 rate year`.
- `exactCharge_mono_factor`: `f1 ≤ f2` implies
  `exactCharge value f1 elapsed rate year ≤ exactCharge value f2 elapsed rate year`.

Neither requires positive year, Rust-width inputs or an uncapped result. The
proofs include zero year (natural division by zero is zero), zero inputs,
both factor clamp regions and the final `u64::MAX` plateau. They are
non-strict inequalities: increasing an input may leave the charge unchanged.
Unnamed kernel-checked examples cover year zero, factors 999/1000/1001 and
5999/6000/6001, and elapsed 1/2 on the cap plateau. The reviewed statement
records landed separately in #1962/PR #1964; their bytes remain unchanged.

Separately **tested** correspondence is reused from historical CI
[run 42310903557, job 128211803318](https://github.com/botho-project/botho/actions/runs/42310903557/job/128211803318):
10,000 generated `demurrage_charge` cases plus 8 committed regressions,
zero mismatches, seed 1707. Its checkout
`1006ffb1ace1059220a225bbb01b23b75d61e6fa` has the complete tree of the
records base `25474cba31fd89dcca9e65efc64bdfc01c4f42c7`.
Artifact `13548218953` has ZIP SHA-256
`9bfcf962e2bda4c19b9d0e8051fc98e28bef06fa9a47b5d2ee57d76615f3a254`.
The report does not supply a binary digest. This is not a fresh proof-head
or local DRT run. The reuse audit preserves the executable `exactCharge`
expression and its arithmetic definitions; changes in the model file are
proofs, examples and its claim-count comment. Production, Cargo inputs,
generator, runner, codec, dispatch and fixtures match the historical receipt.
The ordinary final-head CI gates, including DRT, still apply.

These model proofs and finite correspondence tests do not establish Rust
semantics equivalence, Rust panic-freedom, reset-horizon monotonicity,
charge bounded by principal, or accepted-chain reachability. The separate
Aeneas `capitalized_reset_charge_no_downgrade` result and #1713 retain their
own scope and trust assumptions.

## Time quantization defeats a positive-value threshold (pre-9.0.0)

`cluster-tax/src/demurrage.rs::demurrage_charge` computes
`time_fraction = elapsed * 1_000_000 / blocks_per_year` before multiplying by
the annual charge. With the production five-second reference year
(`6_307_200` blocks), every elapsed time from zero through six gives zero.
`six_block_dead_zone` proves this for all values, factors and rates. Increasing
the transfer value cannot repair the zero, so #1711's positive-charge target
needs an elapsed-time precondition as well as a value threshold.

At a fixed principal of `10^18` picocredits (1,000,000 BTH), factor `6000`
and rate `200` basis points, the charge for six blocks is zero; the charge for
twelve blocks is `20_000_000_000` picocredits (0.02 BTH). Thus two six-block
kernel calls do not equal one twelve-block call. This refutes the documented
same-class churn identity. The theorem does not construct transactions:
ring-age selection, transaction fees, principal changes and accepted-state
reachability still matter. A class downgrade has a separate capitalized
charge, which is outside this counterexample.

## Numerator saturation lowers the ring factor (pre-9.0.0; fixed by #1876)

This section describes the 8.x code. Protocol 9.0.0 fixes it; see
[the next section](#fixed-in-protocol-900-exact-centroid-1876). The theorems
below still hold: they are statements about the pre-fix model `centroid` /
`ringFactor`, which is kept unchanged as the historical witness.

Before 9.0.0, `ring_centroid_implied_factor` saturated its weighted numerator, then divided
by the total value. Its comment says overflow pins the centroid to `u128::MAX`
and the factor to `6000`. Division after saturation makes that claim false.

Take twenty members, each with the same effective wealth of `10^20`
picocredits (100,000,000 BTH):

| Value of each member | Calculated centroid | Default factor |
| --- | --- | --- |
| `10^17` picocredits | `100000000000000000000` | `5949` |
| `10^18` picocredits | `17014118346046923173` | `5841` |

Multiplying every value by ten changes a homogeneous weighted mean and lowers
the fee factor. Each individual product in the second ring fits `u128`; the
sum overflows. The denominator stays exact. All inputs fit Rust's widths.
`homogeneous_ring_scale_counterexample` pins both factors;
`homogeneous_centroid_counterexample` pins the saturated quotient;
`saturation_lowers_charge` proves the lower factor lowers a one-year accrued
charge on the same principal.

All members have equal wealth, so this is independent of #1686's ordinary
background-decoy dilution. The scalar ring satisfies the twenty-member shape,
but we have not constructed a ledger history producing these values and
effective wealths. A larger claimed/import factor could mask the lower ring
floor in the final fee composition.

## Fixed in protocol 9.0.0: exact centroid (#1876)

The operator ruled to fix all three fee-floor weighted means with exact wide
arithmetic: `ring_centroid_implied_factor`, and `ring_centroid_floored_factor`
and `effective_cluster_wealth_from_outputs` in
`botho/src/ledger/store/validation.rs`. They now compute the exact floor of
`Σ value·wealth / Σ value` over a 256-bit numerator
(`cluster-tax/src/wide.rs`). The result is clamped at `u128::MAX` only at the
end. For the ring centroid that clamp cannot be reached, because a weighted
mean never exceeds its largest member wealth.

`Botho/Demurrage/Exact.lean` models the fixed kernel as `exactCentroid` /
`ringFactorExact`:

- `exactCentroid_homogeneous`: for every member count `n > 0`, value `v > 0`
  and wealth `w ≤ u128::MAX`, a homogeneous ring's centroid is `w`. This is
  scale invariance.
- `homogeneous_ring_scale_exact`: both counterexample rings now give 5949.
- `homogeneous_centroid_exact`: the 10^18 ring's centroid is `10^20`.
- `exact_versus_saturating_boundary`: the pre-fix and fixed models agree on
  the largest homogeneous 20-member ring at `W = 10^20` whose numerator fits
  u128, and they diverge at the next value.

These theorems and the supporting `foldl_replicate_exact` theorem are
**proven (model)**, listed in `Theorems.lean` and bound in `statements.txt`
(#1916), with their original types and definition closures unchanged.

The `ring_centroid_factor` DRT leg now compares the Rust function against
`ringFactorExact`. Run against the old `ringFactor`, the fixed Rust function
mismatches immediately, starting with the second pinned ring (Lean 5841, Rust
5949). The `effective_cluster_wealth` leg likewise now compares against
`Botho.Tags.effectiveWealthExact` (`Botho/Tags/EffectiveExact.lean`, theorem
`whale_claimed_wealth_exact`, likewise listed and bound by #1916). Seed 1876 ran 100,000
generated cases on each of `demurrage_charge`, `default_cluster_factor`,
`ring_centroid_factor` and `effective_cluster_wealth`, plus 20 pinned
fixtures. There were zero mismatches; see
`formal-drt/results/demurrage-1876.json`. The pinned
`ring_centroid_factor.jsonl` fixtures keep both original rings and add the
#1876 boundary and extreme rings.

For #1916, seed 1916 ran 10,000 generated cases on each of
`demurrage_charge`, `ring_centroid_factor` and `effective_cluster_wealth`,
plus 8, 8 and 3 committed replays respectively: **30,019 Lean calls, zero
mismatches**. The Rust functions, fixtures and generators were unchanged;
the harness retained release overflow checks. After building
`formal-drt/lean` with Lean 4.35.0, reproduce this bounded run from the repo root:

```sh
CARGO_BUILD_JOBS=2 cargo +nightly-2026-06-09 run --release \
  --manifest-path formal-drt/Cargo.toml -- --cases 10000 --seed 1916 \
  --legs demurrage_charge,ring_centroid_factor,effective_cluster_wealth \
  --model formal-drt/lean/.lake/build/bin/botho-model \
  --regressions formal-drt/regressions --no-write-regressions
```

## Charge is not always bounded by principal

`charge_exceeds_value` gives `1_020_000` charged on a `1_000_000` principal
after 51 years at 2% and factor `6000`. The kernel clamps to `u64::MAX`, not
to principal. This refutes #1711's unconditional bound. It is a long-horizon
specification issue; transaction balance validation remains a separate gate.

## Model correspondence and remaining scope

| Lean definition | Rust source | DRT leg |
| --- | --- | --- |
| `exactCharge` | `demurrage.rs::demurrage_charge` (protocol 9.0.0) | `demurrage_charge` |
| `charge` (legacy) | `demurrage.rs::demurrage_charge` before 9.0.0 (kept in its tests as `legacy_quantized_charge`) | none since #1875 (receipt `demurrage-1711.json`) |
| `defaultFactor` | `fee_curve.rs::ClusterFactorCurve::default_params().factor` | `default_cluster_factor` |
| `ringElapsedQuantile` (`Age.lean`) | `demurrage.rs::ring_elapsed_quantile` | `ring_elapsed_quantile` |
| `ringFactorExact` / `exactCentroid` | `demurrage.rs::ring_centroid_implied_factor` (protocol 9.0.0) | `ring_centroid_factor` |
| `ringFactor` / `centroid` | `demurrage.rs::ring_centroid_implied_factor` before 9.0.0 (historical) | none since #1876 |

The model uses natural numbers with explicit saturation and return clamps.
For Rust-width inputs, the first charge product is below `2^109`; elapsed
times the time scale is below `2^84`. The curve model covers only the default
parameters, preserves signed division toward zero and uses the production
seven-knot lookup table. The pre-fix ring model retains input order and
saturates both accumulators exactly where the 8.x Rust did. The exact model
keeps both accumulators unbounded and clamps the result once.

Pinned DRT fixtures include both rings, the six/twelve-block calls, the
51-year call and curve boundaries. Generated cases cover the Rust integer
widths, near-zero elapsed times, saturation and homogeneous rings.
See `formal-drt/results/demurrage-1711.json` for the run receipt.
The seed-1711 run compared 100,000 generated cases per leg and all 11 pinned
fixtures, with zero mismatches (300,011 comparisons), against the legacy
kernel. The #1875 receipt, `formal-drt/results/demurrage-1875.json`, is the
same three legs with seed 1875 against the exact kernel (`exactCharge`),
including the five new `demurrage_charge` regression lines.

Reproduce after building `formal-drt/lean` and `formal-drt`:

```sh
cd formal-drt
target/release/botho-formal-drt --cases 100000 --seed 1711 \
  --legs demurrage_charge,default_cluster_factor,ring_centroid_factor \
  --no-write-regressions --json /tmp/demurrage-1711.json
# 9.0.0 exact kernel (#1875): the same command with --seed 1875
```

This is a partial #1711 contribution. The unconditional principal bound
remains refuted. Reset/settlement/spend property proofs (the later #1985 model/DRT increment
is below), ring elapsed centroid, quantile property proofs, monotonicity of other kernels, adversarial dilution,
fee-floor bounds, binary fuzz-corpus replay and reachable transaction/block
witnesses remain outstanding. Do not mark #1711 complete on these results
or weaken its targets silently.
Any arithmetic repair changes consensus outputs and needs its own reviewed
policy, activation and regression work.


## Composition models and DRT (#1985)

Three executable definitions in `Botho/Demurrage/ExactCharge.lean` reuse
`exactCharge` without changing its definition, proofs or reviewed records:

| Model | Public Rust function in `cluster-tax/src/demurrage.rs` | Composition |
|---|---|---|
| `exactResetCharge` | `capitalized_reset_charge` | Raw output factor at least input factor returns zero; otherwise natural subtraction of two separately rounded and separately u64-capped charges |
| `exactSettlementCharge` | `demurrage_settlement_charge` | Reset to factor 1000 over exactly 31,536,000 blocks, independently of the supplied year |
| `exactSpendCharge` | `spend_demurrage_charge` | Maximum of accrued charge at supplied elapsed time and reset at the fixed 31,536,000-block horizon |

These are **tested models**, not new theorems. The existing extracted
`capitalized_reset_charge_no_downgrade` proves only its named branch;
finite correspondence here does not prove Rust equivalence or panic freedom.
The parent still owns other composition properties, fee-floor composition,
adversarial dilution, binary fuzz-corpus replay and ledger reachability.

One stateless `demurrage_composition` leg calls all three real Rust functions
and the canonical Lean definitions on every case. Value, both factors,
generic reset horizon, spend elapsed and year are independent u64 inputs;
rate is u32. Half the generator mixture uses full-width/edge-biased inputs,
including zero year and years above 368,934,881,474. Half uses ordinary
positive values/rates, factors 1000..6000 and independent times, preventing
zero/capped results from dominating all coverage. No typed input is excluded.
The report records each output's zero/positive/cap class, the number of
capped generic-reset terms, accrued/reset dominance and year region.

The 32 committed vectors include independently varying zero horizon and
elapsed, raw factor clamp boundaries, one and two capped terms, zero guards,
large years and these discriminators:

- Separate floors: value 3000, input/output factors 1002/1001, horizon 1,
  rate 10000 and year 1 give reset **1**; charging the factor difference
  would give zero.
- Fixed horizon: value 1000000, input factor 6000, rate 200 and year 1 give
  settlement **630720000000**; incorrectly using `5 * year` gives 100000.
- Distinct fields: value 1000000, factors 6000/3500, generic horizon 6307200,
  elapsed 63072000, rate 200 and year 6307200 give reset **10000**,
  settlement **100000**, spend **200000**.
- Separate caps: value `u64::MAX`, factors 6000/3500, rate 200 and year
  6307200 give reset **9223372036854775808** at a 50-year generic horizon
  and **0** at 100 years. These are helper-domain tests, not production
  five-year or accepted-chain witnesses. They refute an unconditional
  reset-horizon monotonicity claim; they do not change the fixed horizon.

`composition_live_rust_and_lean_vectors` asserts the live outputs of both
Rust and the built Lean codec against every vector's explicit `expected`
values, checks decimal-string input encoding and serialization, and pins the
three-distinct-field case. Ordinary runner replay reads only `input`, so
stored output metadata alone would not provide this check. Build the Lean
DRT executable before running this test, as the existing CI workflow does.

The normal PR tier retains all original 25 legs and registers this as the
26th: 10,000 generated and compared composition cases check **30,000 function
results**, plus 32 replays. The full campaign still uses 10,000 cases per
leg and the five-minute run limit. Nightly discovers the new leg with the
unchanged 1M-case, four-worker, 3600-second leg and 9600-second collection
settings; registration and helper checks do not assert a million-case run.

Validation receipts on the implementing PR distinguish the normal campaign
from temporary actual-wrapper mutations: factor-difference reset, variable
settlement horizon and max-to-add spend. They retain the rebuilt binary hashes,
expected numerical mismatches, real generator/shrink/saved-fixture replay,
and exact restoration. No mutant or control-only workflow is part of the
normal source. Historical receipts above remain historical; final PR CI
provides the new composition evidence.


## Ring age quantile model and DRT (#1993)

`Botho.Demurrage.ringElapsedQuantile` in `Botho/Demurrage/Age.lean` is an
executable model of `cluster-tax/src/demurrage.rs::ring_elapsed_quantile`.
Empty input returns zero. Member values are ignored, including zero values;
ages are saturated current-minus-creation heights, sorted ascending. The
quantile is clamped to 10,000 bps, the rank is `ceil(q * n / 10000)`, and
`rank - 1` is clamped to the available indices. Thus q=0 selects the minimum
and q=10,000 selects the maximum. Source inspection of ledger validation
(`botho/src/ledger/store/validation.rs`, constant in `store.rs`) and mempool
relay (`botho/src/mempool.rs`) finds both callers using 10,000 bps. The DRT
adapter executes the public helper, not either complete caller.

The scalar `ring_elapsed_quantile` leg is the 27th registered leg. Its
explicit campaign budget is **0..=64 members**, with full-width u64 values,
creation/current heights and u32 q support. Relative past heights supply
positive mixed ages; repeated/equal heights and ordered/unsorted layouts are
also generated. Interior q and nearest-rank boundaries are deliberately
sampled alongside 0, 10,000 and larger q, including u32 max. Existing report
classes record actual list lengths, age/order, q/rank/output, wide/zero
current height, zero/equal/future creation heights and zero/maximal values.
The length budget is not a production-validity bound or arbitrary-cardinality
coverage. Stored u64 fields and all wire integers are exact decimal strings.

`quantile_live_rust_and_lean_vectors` asserts explicit numerical outputs for
27 fixtures on both the actual Rust helper and the built Lean codec, with
serialization/decimal checks. Ordinary replay reads only fixture `input`.
The live test separately runs 114 transformations: reverse, rotation and
endpoint swap, plus all-zero, all-maximal and mixed replacement values,
keeping the same current height, q and age multiset. These are finite checks.
At current 100, unsorted ages [30,10,20] change result from 10 to 20 between
q=3333/3334 and from 20 to 30 between q=6666/6667. One age 500000 among ten
zero ages yields 0 at q=9090 and 500000 at q=9091 or 10,000; this does not
identify a real spender or establish a decoy-economic guarantee.

The implementing PR retains remote numerical controls for ceiling-to-floor
rank rounding and omitted sorting in the actual production helper, with an
unchanged Lean model, rebuilt runners, generated shrinking, saved failing
replay and restored passing replay. Normal-source evidence reports 10,000
generated and compared cases per leg separately from replay and these live
transformations. The five-minute PR campaign and nightly 1M/leg, four-worker,
3600-second leg and 9600-second collection budgets are unchanged; discovery
and nightly-helper checks are not a million-case execution claim.

No theorem, extraction equivalence, universal permutation/value-independence,
generic overflow/allocation/panic freedom or accepted-chain reachability
result is added. #1711 retains those proof/reachability obligations, the
separate elapsed-centroid model and its unchecked-u128 accumulator overflow
lead, dilution, fee-floor composition and binary fuzz-corpus replay. Historical
receipts above remain historical; final-head receipts are on the child PR.
