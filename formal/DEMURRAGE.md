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
| `ringElapsedCentroid` (`Age.lean`) | `demurrage.rs::ring_elapsed_centroid` | `ring_elapsed_centroid` |
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
is below), quantile property proofs, monotonicity of other kernels, adversarial dilution,
fee-floor bounds, binary fuzz-corpus replay and reachable transaction/block
witnesses remain outstanding. Do not mark #1711 complete on these results
or weaken its targets silently.
Changes to consensus arithmetic need their own reviewed policy, activation
and regression work; the #1998 helper repair below leaves the production age
selector unchanged.


## Composition models and DRT (#1985)

Three executable definitions in `Botho/Demurrage/ExactCharge.lean` reuse
`exactCharge` without changing its definition, proofs or reviewed records:

| Model | Public Rust function in `cluster-tax/src/demurrage.rs` | Composition |
|---|---|---|
| `exactResetCharge` | `capitalized_reset_charge` | Raw output factor at least input factor returns zero; otherwise natural subtraction of two separately rounded and separately u64-capped charges |
| `exactSettlementCharge` | `demurrage_settlement_charge` | Reset to factor 1000 over exactly 31,536,000 blocks, independently of the supplied year |
| `exactSpendCharge` | `spend_demurrage_charge` | Maximum of accrued charge at supplied elapsed time and reset at the fixed 31,536,000-block horizon |

Since protocol 10.0.0 (#1685) the consensus fee floor calls
`spend_demurrage_charge_with_exit_rate`, which takes the accrued rate
(`demurrage_rate_bps`, 0 in epoch 0) and the reset rate
(`exit_pricing_rate_bps`, 200 bps from block 1) separately. It is the same
`max` of the same two DRT-covered functions, and a unit test pins it to
`spend_demurrage_charge` whenever the two rates are equal, so
`exactSpendCharge` covers every epoch-1+ spend. The epoch-0 shape (accrual 0,
reset 200) has no Lean model or DRT leg yet.

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

The #1993 increment added no property theorem; its campaigns remain finite
**tested** correspondence evidence. The later #2008 increment proves the four
model properties below. Universal Rust/extraction equivalence, generic
overflow/allocation/panic freedom and accepted-chain reachability remain open,
as do the other #1711 kernel/composition properties, dilution, full fee-floor
behavior and binary fuzz-corpus replay. Historical receipts above retain their
original scope; each child PR records its own final-head checks.

On 2026-11-09, [formal gates](https://github.com/botho-project/botho/actions/runs/42711257939)
and the [control/normal campaign](https://github.com/botho-project/botho/actions/runs/42712236451)
passed at actual normal checkout `7eb426d8c7bcda232a5d8469e4dfd37d5b56a752`
(tree `d1695f8756607a2a71af4568c0f8c2997e59ce83`). The temporary workflow came
from control commit `43820316bf8cb7f3febe8952c96a57f21b3598f6`. After exact
restoration, seed 1707 completed **270,000 generated = compared**, 225 replays,
zero mismatches, and 70.212 summed leg seconds; quantile contributed 10,000
comparisons, 27 replays and 2.447 seconds. Its observed classes included all
65 lengths, 4,642 mixed ages, 7,867 interior q, 6,615 interior ranks and 4,348
positive outputs (37 at u64 max). The live 27 vectors/114 transformations are
separate from those campaign counts.

At seed 1993, floor rounding gave Rust 10 versus Lean 20 at q=3334; omitted
sorting gave Rust 30 versus Lean 10 at q=0. A clean scratch floor campaign
generated 64 inputs, compared 23, made five shrink attempts and saved five
numerical failures. The same five inputs failed replay and passed after exact
restoration/rebuild; both targeted vectors also passed restored replay.
Original archives, API digests, patches, commands, source/model/runner hashes
and root/resolved lockfiles are retained in the PR evidence receipt. The
root/standalone lock hashes differ (1,157/688 packages); unchanged tracked
source does not establish identical dependency graphs or binaries across
runs. These receipts precede the documentation-only follow-up; final PR CI
validates its final checkout separately. No million-case run was performed.

### Proven elapsed-quantile model properties (#2008)

Four claims about the unchanged canonical `ringElapsedQuantile` are
**proven (model)** and listed in `Theorems.lean`:

| Theorem | Full-Nat contract |
| --- | --- |
| `ringElapsedQuantile_value_independent` | Equal creation-height sequences give equal results, with arbitrary independent changes to each value. |
| `ringElapsedQuantile_perm` | Any permutation of complete `(value, creationHeight)` pairs preserves the result and multiplicities. |
| `ringElapsedQuantile_mono_quantile` | Increasing quantile from `q1` to `q2` cannot lower the result, assuming solely `q1 ≤ q2`. |
| `ringElapsedQuantile_le_height` | The result is at most current height, unconditionally. |

The domain is all natural values, heights and quantiles, and arbitrary finite
lists. No nonempty, positive-value, u64-width, nonoverflow or 64-member premise
is added. Natural subtraction still saturates future creation heights to zero;
the quantile still clamps at 10,000 bps. Zero-valued members count, so an
all-zero-value ring can have a positive quantile, unlike the zero-total centroid.
Equal old values may receive different replacement values. The unnamed
kernel-checked examples include those cases, values beyond u64, empty input,
zero current height, future births, reordered duplicate pairs, q=0/10,000/20,000,
and the 3333/3334 and 6666/6667 thresholds for three unsorted mixed ages.

The four complete records were approved first in [PR #2009](https://github.com/botho-project/botho/pull/2009),
merged at `21417a368d4cea3c9db66b5f9c2ad0052c514a2e`; all 253 records remain
unchanged here. The genuine proposal source is
`bb70f0c0ad932ecf497ba35bfc1e0589f14c5a8f` (tree
`fd5b7ab7aa293a89f30fd44f7453665f97cf9085`). Its original
[run 42770787453](https://github.com/botho-project/botho/actions/runs/42770787453)
and independent [run 42771140251](https://github.com/botho-project/botho/actions/runs/42771140251)
compiled the proofs and generated matching complete blocks. Both proposal
workflows failed the expected missing-record self-test and skipped isolated
verification; neither is a green full workflow. The implementing PR retains
its own final-head build, kernel replay and isolated binding of all 253 claims
to the records-bearing base, with original artifact/log archives and API digests.

The finite Rust/Lean campaigns above remain **tested**, separate from these
universal model proofs. The canonical quantile, centroid and existing centroid
proofs are unchanged, as are Rust/DRT sources, fixtures and campaign budgets.
Source identity alone does not establish equal resolved dependencies or binaries
across runs. These claims do not prove universal Rust/extraction equivalence,
width/overflow/panic/allocation/cardinality safety, accepted/genesis reachability,
fee behavior, a principal bound or privacy economics. Other #1711 kernel,
composition and factor properties, the false unconditional principal-bound
disposition, dilution, full fee-floor/no-saturation and binary fuzz replay remain
separate obligations.

## Exact elapsed-age centroid (#1998)

`ring_elapsed_centroid` now computes `floor(Σ value × saturated_age / Σ value)`
with `wide::WeightedMean`, returning zero for an empty or zero-weight sum.
The canonical `Botho.Demurrage.ringElapsedCentroid` uses `Nat` subtraction,
products, sums and one final division (including `Nat` division by zero = 0).
Its scalar DRT leg invokes the public crate helper and the built Lean codec;
all u64 members/heights and results use lossless decimal strings.

For a slice on a target with at most 64-bit `usize`, `n < 2^64`, so the
numerator is below `2^192` and denominator below `2^128`. The existing U256
numerator/u128 denominator cannot saturate. A positive-weight mean cannot
exceed its largest age, at most `u64::MAX`, so the accumulator's final clamp
and u64 conversion are exact here. Release targets are x86_64/aarch64. This
source/arithmetic argument preserves all nonoverflowing old results; it is
not a machine-checked Rust-equivalence or panic-freedom theorem.

Twenty independent numerical fixtures cover high products beyond u128,
both denominator-division branches, empty/singleton/zero weights, future
ages, full-width heights, unequal weights and flooring once. The live test
asserts each expected value separately against fresh Rust and Lean calls,
then checks 18 distinct pair permutations, 20 zero-weight insertions and
seven common doublings with every scaled weight fitting u64. These **45
transformations** are separate from vectors, generated cases and replays.
The unequal case `[(1,0),(3,8)]` at height 10 gives 4, while `[(1,9),(1,9)]`
gives 1, distinguishing the weighted mean and single final floor.

Generated lengths are bounded to 0..=64 solely for campaign work. The
mixture deliberately samples ordinary positive ages/weights, full-width
relative and independent heights, near-maximal multi-member products,
zero/future/equal ages and maximal ages. Diagnostics classify lengths,
numerator overflow (checked sticky u128 detection), denominator width,
zero/positive/max output, mixed ages and unequal weights. No helper input
restriction follows from this finite domain. The unchanged normal budget
is 10,000 generated cases per discovered leg (28 legs), within five minutes;
nightly retains 1M/leg, four workers, 3600 seconds/leg and 9600 seconds total.
No nightly-million execution is claimed.

The [old-source reproduction](https://github.com/botho-project/botho/actions/runs/42731267633)
checks out `9f62edc7` and calls the actual public helper with two `(M,0)`
members at height `M = u64::MAX`. Checked release panics at the numerator
addition; an explicitly unchecked diagnostic returns `9223372036854775806`
instead of `18446744073709551615`. Normal root and DRT release profiles
retain overflow checks. The probe's manifest, resolved lock, flags,
executable/source hashes and original artifact archive are retained.

Runtime/control and final-head campaign receipts are linked in the child
PR, including original ZIP hashes verified against GitHub API digests,
run/job/artifact identities, actual source checkout versus control-workflow
commit, commands, toolchains and executable identities, and the copied root
versus resolved standalone locks. A source-equal run does not imply an equal
resolved dependency graph or executable; normal CI and diagnostic runs are
identified separately. Historical fixtures, receipts, controls, theorem
lists and statement records remain unchanged.

The #1998 increment adds arithmetic repair, an executable model and finite
correspondence and mutation evidence; it adds no listed model theorem. The
four subsequent #2004 model proofs below leave Rust/extraction equivalence,
machine-width/panic/allocation/cardinality safety and accepted/genesis-chain
reachability unproved. Production validation and relay still select `ring_elapsed_quantile`
at 10,000 bps; the centroid is a public helper and simulation comparison
kernel (the ledger occurrence is a test). No accepted-chain exploit is
established. #1711 remains open for its separate proof and policy questions.

### Proven elapsed-centroid model properties (#2004)

Four claims about the unchanged canonical `ringElapsedCentroid` are now
**proven (model)** and listed in `Theorems.lean`:

| Theorem | Full-Nat contract |
| --- | --- |
| `ringElapsedCentroid_perm` | Any permutation of complete `(value, creationHeight)` pairs preserves the result. |
| `ringElapsedCentroid_insert_zero` | A zero-value pair with any creation height can be inserted between arbitrary prefix and suffix lists. |
| `ringElapsedCentroid_scale_values` | Multiplying every value by the same `k` preserves the result, assuming solely `0 < k`. |
| `ringElapsedCentroid_le_height` | The result is at most current height, unconditionally. |

These quantify over arbitrary finite lists and all natural values/heights,
without positive-total, u64-width, nonoverflow or 64-member premises. Empty
and all-zero rings return zero; future creation heights have saturated age
zero. Unnamed kernel-checked examples cover those boundaries, zero current
height, middle insertion, pair permutation, the unequal-weight fractional
mean `27 / 5`, and positive scaling whose values exceed u64.

The reviewed statements were merged first in [PR #2005](https://github.com/botho-project/botho/pull/2005)
at `90a7fb8b2f8817293e699ae797f78a9359ebf955`; this increment preserves all
249 complete records. The proof source was retained separately at
`c3da7077422cc72bd71273c56f3b98cded305e5d` (tree
`fb54ebf01aa180741ffe650360b6c794d2eb044d`). Its independent proposal
[run 42755505776](https://github.com/botho-project/botho/actions/runs/42755505776)
compiled the proofs and generated matching records, but the workflow failed
at the expected missing-record self-test and skipped isolated verification.
It is historical proposal evidence; the implementing PR records its own
final-head build and isolated verification against the records-bearing base.

The #1998 finite Rust/Lean correspondence remains **tested**, separate from
these universal Nat model proofs. Its final normal [DRT run 42736471238](https://github.com/botho-project/botho/actions/runs/42736471238)
reported 280,000 comparisons, 245 replays and zero mismatches at reviewed head
`56cb51f37bc0e6964401b339d82453d1f20f5ed8`, using identical-tree checkout
`786bc9f994c783408d1dd46b0a12c837ee525a18`. Original
[report artifact 13727407395](https://github.com/botho-project/botho/actions/runs/42736471238/artifacts/13727407395)
has ZIP SHA-256 `f31f5e5595fc6618b46b9f2e255fa119224867b6446e4b82141217f2ca0b0ccc`;
this is a historical run, not execution of the #2004 proof head. The final diff preserves the executable
Age declarations and Rust/DRT sources, fixtures and budgets; source identity
does not imply equal resolved dependencies or binaries across runs. The
historical runs above retain their original scopes and provenance. No new
Rust/extraction equivalence or machine-width/panic/allocation/cardinality/
reachability proof follows from these properties. The height bound concerns
the elapsed centroid only; neither it nor positive scaling proves accepted-chain
fee behavior, a principal bound or general factor dilution resistance.
The four quantile properties above and the remaining #1711 obligations stay separate.
