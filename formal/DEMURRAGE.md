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

## Numerator saturation lowers the ring factor

`ring_centroid_implied_factor` saturates its weighted numerator, then divides
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
| `ringFactor` / `centroid` | `demurrage.rs::ring_centroid_implied_factor` | `ring_centroid_factor` |

The model uses natural numbers with explicit saturation and return clamps.
For Rust-width inputs, the first charge product is below `2^109`; elapsed
times the time scale is below `2^84`. The curve model covers only the default
parameters, preserves signed division toward zero and uses the production
seven-knot lookup table. The ring model retains input order and saturates both
accumulators exactly where Rust does.

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
