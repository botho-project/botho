# M2 Run Matrix — Runbook (#605 / #626 §7)

Reproducible one-liners for the M2 experiment matrix. Every run exercises the
**real production log-domain cluster-factor curve** (`ClusterFactorCurve::factor`,
`w_mid = 100,000 BTH`), not the #314 hardcoded 1.0/2.0/6.0 factors — closing the
sim-harness gap the #314 validation left open (it called `add_owner_with_factor`
and never consulted the curve). Population wealth is declared in **BTH** and
converted to **picocredits** at the curve boundary (`× PICO_PER_BTH`), killing
the sim-unit ambiguity permanently.

The harness lives in the library (`cluster-tax/src/simulation/m2.rs`) so its smoke
tests run under `cargo test -p bth-cluster-tax`. The binary subcommands below are
thin printing wrappers.

> **This PR does NOT run the full suite.** The empirical RUNS and the
> testnet reset are tracked on #605. What lands here is the reproducible harness
> plus a smoke variant of each run type (see *Smoke tests*).

## Build

```
cargo build -p bth-cluster-tax --features cli --bin cluster-tax-sim --release
BIN=target/release/cluster-tax-sim
```

## Population ladder (BTH holdings)

| cohort | n | holdings | velocity | drives |
|--------|---|----------|----------|--------|
| small | 80 | 50 BTH | 1×/yr | ~1x floor check |
| middle | 30 | 10,000 BTH | 1×/yr | mid-curve |
| **merchant** | 20 | 5,000 BTH | 2×/yr | cumulative-ratchet mispricing (NEW vs 2026-06) |
| whale | 9 | 2,000,000 BTH | 0.2×/yr | high-factor progressivity |
| strategic whale | 1 | 5,000,000 BTH | 0.2×/yr | honest vs split+churn (gamed) |

Under **cumulative** semantics each cohort's tracked cluster wealth ratchets up by
`holdings × velocity` per epoch; the factor is re-priced through the real curve at
every epoch boundary. Under **epoch-halving decay** the cumulative wealth is
additionally halved (`w >>= 1`) once every `half_life_years` epochs (a pure
function of the epoch index — never per-access, per the M3 determinism lesson).

## Run set 1 — recalibrated CUMULATIVE (validates #605 "document cumulative")

Long-horizon, honest and gamed equilibria. Δgini reported against the **>0.05**
criterion; merchant cohort flagged if it reaches ≥3x from volume alone; whales
must stay >5x.

```
# 10-year horizon
$BIN m2-cumulative --horizon-years 10            # honest
$BIN m2-cumulative --horizon-years 10 --gamed    # gamed (whale split+churn)

# 20-year horizon
$BIN m2-cumulative --horizon-years 20
$BIN m2-cumulative --horizon-years 20 --gamed
```

## Run set 2 — EPOCH-HALVING decay (validates #605 "decay" fallback)

Same harness plus deterministic epoch-halving. Half-life sweep **{2, 5, 10} yr**,
both horizons, both equilibria. In addition to Δgini, each run emits:

- **Wash-trading evasion %** — the cluster tax a strategic whale escapes by
  self-transferring to shed tracked wealth vs an honest whale. Gate: **<20%**.
  Prior art (experiments/ANALYSIS.md): 94–99% at aggressive per-hop decay. The
  log-domain curve blunts this (one halving = −0.5 sigmoid units ≈ −0.6x near
  midpoint), so shedding factor requires *exponential* wealth reductions.
- **Ring identification rate** — the adversary's probability of picking the real
  signer under decay-revealed tracked-wealth gaps, via the production privacy
  primitive `calculate_privacy_metrics`. Gate: **<50%**. Prior art: 78.7% at 20%
  per-hop decay.

```
# half-life sweep at 10-year horizon (honest)
$BIN m2-decay --horizon-years 10 --half-life-years 2
$BIN m2-decay --horizon-years 10 --half-life-years 5
$BIN m2-decay --horizon-years 10 --half-life-years 10

# gamed + 20-year horizon
$BIN m2-decay --horizon-years 20 --half-life-years 2  --gamed
$BIN m2-decay --horizon-years 20 --half-life-years 5  --gamed
$BIN m2-decay --horizon-years 20 --half-life-years 10 --gamed
```

## Decision rule (restating #605)

If **run set 1** holds Δgini > 0.05 at 10/20yr in the gamed equilibrium, the
factor distribution stays discriminating, and the merchant cohort is not
systematically mispriced (mean factor < 3x) → **option 2: document cumulative**,
lock `w_mid` at genesis. Otherwise → **option 1: epoch-halving** with the
run-set-2 half-life that maximizes Δgini subject to evasion < 20% and id-rate
< 50%.

## Determinism

Every run is deterministic for a fixed `--seed` (default `626626626`). The RNG is
a seeded `ChaCha20Rng`; the epoch-halving and factor re-pricing are pure integer /
curve operations.

## Smoke tests

A tiny-horizon (`--smoke`, 50 blocks × ≤3 epochs) variant of each run type runs as
a unit test proving the harness executes end-to-end and emits every metric:

```
cargo test -p bth-cluster-tax --lib m2
```

- `smoke_m2_cumulative_emits_metrics` — run set 1, both equilibria.
- `smoke_m2_decay_emits_all_metrics` — run set 2, half-life sweep {2,5,10},
  asserts evasion < 20% and id-rate < 50% gates are computed and emitted.
- `population_entry_factors_come_from_real_curve` — proves factors come from the
  curve (small ~1x, merchant < whale, whale > 5x).
- `m2_is_deterministic_for_fixed_seed` — bit-for-bit reproducibility.
