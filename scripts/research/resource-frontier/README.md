# Inactive resource-cost and fee-affordability frontiers

Part of #1306. This is dimensional accounting over **illustrative, unmeasured
inputs**, not a fee recommendation, security estimate, price forecast or observed
Botho operating cost. It changes no production policy. There is no unique minimum
payment: the model returns a family of resource and affordability boundaries as
BTH purchasing power, accepted load, replication and retention vary.

Run from the repository root with Python 3 (standard library only):

```
python3 scripts/research/resource-frontier/model.py
python3 -m unittest discover -s scripts/research/resource-frontier -v
```

`scenarios.json` fixes all decimal inputs and the inspected source baseline.
`report.json` contains 72 complete resource/funding rows, six source-transcribed
emission examples and source/model/input hashes. `frontier.csv` summarizes cost;
`affordability.csv` contains 486 payment/fraction comparisons for the 54 positive
load rows. Eighteen zero-load rows remain explicit; a separate append-only storage
sentinel reports nonstationarity. Repeated generation is byte-identical. Updating
source hashes on another checkout is a new evidence snapshot, not an observation
of changing network cost.

## Units and costs

Currency is an abstract purchasing-power unit. The prices 0.01/1/100 currency per
BTH are sensitivity axes, not quoted market values. Each reporting period is 24
hours. Accepted transactions are `N = 3600 * H * capacity * utilization`. Capacity
10 tx/s, 10KB transactions, eight compute cores per replica, 0.25 verification
seconds and 1.2 work units per accepted transaction are assumed inputs, not
measured production throughput. The work multiplier accounts only for modeled
extra verification attempts; it is not an adversarial-load bound.

Costs include aggregate consensus/mining electricity and fixed costs; replicated
verifier base and incremental electricity; CPU/hardware amortization excluding
that electricity; fixed verifier costs; effective delivery-copy bandwidth; and
replicated retained storage. Consensus equipment and verifier replicas are
assumed disjoint for accounting. If the same hardware serves both roles, the
input allocation must remove overlap. CPU amortization excludes items charged
as fixed hardware; extra-work network ingress is not separately modeled.
No CPU timing is converted to electricity without an explicit assumed incremental
power input. All power values here are assumptions, not power measurements.

With finite retention R hours, stationary stored GB are
`(N/H) * R * bytes_per_tx * replicas / 1e9`; their period cost is
`stored_GB * H * currency_per_GB_hour`. Fixed historical stock is charged
separately. A stationary fixed-rate, fixed-retention model is a conditional
steady state, not a monetary/economic equilibrium. Infinite append-only retention
has growing stock and no finite stationary storage result in this model; a
finite-horizon append-only cost could still be calculated separately. Compute
load above configured capacity is reported overloaded, with no feasible fee
frontier. Bandwidth/storage capacity bottlenecks beyond their priced units are
not modeled.

## Resource equivalence is not operator funding

The resource-equivalent fee is `C / (N * price)` BTH per accepted transaction,
undefined at zero load. It values the resources consumed; collecting or burning
that amount does not by itself pay their providers.

At the pinned source, `LotteryFeeConfig::split_fees` sends 80% of gross fees to
the lottery pool and the rounded remainder to burn. Pool carryover and caps mean
allocation is not immediate payout. Neither destination is a direct protocol
allocation to infrastructure operators. Operators may own eligible outputs and
receive lottery payments or other indirect benefits; those revenues are unknown
and deliberately excluded, **not proved absent**.

Funding results therefore have an explicit no-indirect-revenue assumption.
They show current direct capture zero and hypothetical capture fractions 0.5/1,
which are sensitivity alternatives, not policy proposals. For hypothetical
routing, the uncaptured remainder retains the 80/20 lottery/burn split; fractions
sum to one. Current integer fee-split examples separately preserve rounding.
Burn and lottery transfers are not additional physical resource costs.

For each capture fraction, the direct funding frontier is the uncovered cost
allocated to these roles divided by `N * price * capture`. Positive deficit at
zero capture is labeled a **direct funding gap under the stated assumption**,
not socially impossible resource recovery. Zero load likewise cannot amortize
fixed cost over accepted transactions.

The issuance-supported scenario assigns an illustrative **1 BTH/hour** only to
consensus/mining costs. The available budget is separately derived from the configured height, supply and
actual block interval: tail height 31,536,000, assumed supply 611,010,000 BTH, and
40-second blocks in this grid. Assignments exceeding that derived miner issuance
are rejected; available, assigned and unassigned BTH are reported. This is a
conditional source-transcribed budget. Height and supply are held constant across
the reporting period; this does not sum evolving per-block rewards. It is not a
prediction of actual issuance, a
verified operator entitlement, or measured sustainable assignment. Unused consensus support cannot
silently subsidize verifier costs. The fee-only case sets support to zero. Source
transcriptions separately show initial/halving/tail rewards and miner-versus-pool
splits at 5- and 40-second block intervals with an explicit assumed supply. They
are not Rust-kernel execution evidence. The actual policy includes perpetual tail
issuance; fee-only is a counterfactual boundary, not its inevitable future.

An additional security expenditure budget appears separately, above enumerated
resource costs. It must not include the same electricity/hardware again. Its
incremental funding frontier does not establish attack resistance, sufficient
hash rate, incentive compatibility or the required security budget.

## Affordability and illustrative outputs

For a caller-specified payment A and fee f, the model reports `f/A` and total
spend `A+f`. Given an explicitly supplied tolerance alpha, the boundary is
`A >= f/alpha`. The grid's 0.1%/1%/10% fractions and 0.01/1/100 BTH payments are
illustrative axes only; none is a chosen acceptance limit or minimum payment.
These resource-equivalent fees also exclude any separately required economics
charge. They do not ratify D1/D3/base or imply that the CT1 proposal already uses
a cost-recovery rule.

For example, inspect the small-preset, price=1, 30-day rows in `frontier.csv`:
fixed costs persist at zero load, and cost per accepted transaction falls as load
amortizes them. Changing BTH price changes the BTH-denominated frontier inversely
without changing physical resource cost. Increasing replication raises repeated
compute/storage costs; finite retention raises storage stock. Those directional
results follow from the equations, not calibrated predictions.

## Validation and remaining work

Eight tests check source-transcribed integer accounting, component/funding
conservation, non-overlapping role support, price/load/replication/storage
sensitivity, affordability inversion, security separation, invalid exact inputs,
zero/overload/nonstationary states and byte-reproducible report denominators.
Binary floats, nonfinite values, negatives and invalid fractions are rejected.
Malformed decimal strings are normalized to validation errors. Source
transcriptions admit u64 heights/fees and u128 supply, additionally rejecting
intermediate multiplication or reward-cast overflow rather than claiming parity
outside that checked domain. The actual supplied configuration is embedded and
canonically hashed, including when callers modify it in memory.
Decimal arithmetic uses 50-digit precision for the resource calculation; these
are approximate economic quantities, not consensus picocredit rules. Fee/emission
transcription examples use exact integers.

Needed before using this to make policy: measured power/compute/network/storage
profiles on supported hardware, actual replica/role overlap and load, rejected
traffic costs, archival/pruning policy, actual financing and indirect revenue,
security requirements, and economic calibration. Neither test success nor this
illustrative frontier establishes mainnet readiness or financial viability.
