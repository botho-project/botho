# Demurrage counterexamples (#1711)

`Botho/Demurrage/Counterexamples.lean` refutes three proposed properties of
the current integer kernels. It does not change consensus behavior.
The six theorems are **proven (model)**; the three DRT legs call the actual
Rust functions to test correspondence. DRT is evidence, not a proof of
Rust-to-Lean equivalence.

Tracked findings: [#1875](https://github.com/botho-project/botho/issues/1875)
(time quantization) and [#1876](https://github.com/botho-project/botho/issues/1876)
(ring saturation). Statement-record review precedes proofs in
[#1877](https://github.com/botho-project/botho/pull/1877).

## Time quantization defeats a positive-value threshold

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

These theorems are proven (model) but **not yet bound**. They are not listed
in `Theorems.lean` and have no `statements.txt` blocks. Binding them needs a
records-only pull request first (see `README.md`, "Statement binding").

The `ring_centroid_factor` DRT leg now compares the Rust function against
`ringFactorExact`. Run against the old `ringFactor`, the fixed Rust function
mismatches immediately, starting with the second pinned ring (Lean 5841, Rust
5949). The `effective_cluster_wealth` leg likewise now compares against
`Botho.Tags.effectiveWealthExact` (`Botho/Tags/EffectiveExact.lean`, theorem
`whale_claimed_wealth_exact`, also not yet bound). Seed 1876 ran 100,000
generated cases on each of `demurrage_charge`, `default_cluster_factor`,
`ring_centroid_factor` and `effective_cluster_wealth`, plus 20 pinned
fixtures. There were zero mismatches; see
`formal-drt/results/demurrage-1876.json`. The pinned
`ring_centroid_factor.jsonl` fixtures keep both original rings and add the
#1876 boundary and extreme rings.

## Charge is not always bounded by principal

`charge_exceeds_value` gives `1_020_000` charged on a `1_000_000` principal
after 51 years at 2% and factor `6000`. The kernel clamps to `u64::MAX`, not
to principal. This refutes #1711's unconditional bound. It is a long-horizon
specification issue; transaction balance validation remains a separate gate.

## Model correspondence and remaining scope

| Lean definition | Rust source | DRT leg |
| --- | --- | --- |
| `charge` | `demurrage.rs::demurrage_charge` | `demurrage_charge` |
| `defaultFactor` | `fee_curve.rs::ClusterFactorCurve::default_params().factor` | `default_cluster_factor` |
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
fixtures, with zero mismatches (300,011 comparisons).

Reproduce after building `formal-drt/lean` and `formal-drt`:

```sh
cd formal-drt
target/release/botho-formal-drt --cases 100000 --seed 1711 \
  --legs demurrage_charge,default_cluster_factor,ring_centroid_factor \
  --no-write-regressions --json /tmp/demurrage-1711.json
```

This is a partial #1711 contribution. Settlement/reset/spend composition,
ring elapsed centroid/quantile, `consensus_fee_floor`, monotonicity proofs,
fuzz-corpus replay and accepted-block witnesses remain outstanding. Do not
mark #1711 complete on these counterexamples or weaken its targets silently.
Any arithmetic repair changes consensus outputs and needs its own reviewed
policy, activation and regression work.
