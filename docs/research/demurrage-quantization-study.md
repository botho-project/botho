# Demurrage Time-Quantization: Candidate Fixes Compared (#1875)

**Issue:** [#1875](https://github.com/botho-project/botho/issues/1875) (operator ruling 2026-11-04: "simulate before choosing"; the chosen fix ships in protocol 9.0.0 with #1876).
**Status:** simulation evidence only. Nothing here changes consensus, `demurrage_charge`, wallets or fees. The ranking in §6 is a recommendation. **The operator decides.**
**Reproduce:**
```
cargo run --release -p bth-cluster-tax --features cli --bin cluster-tax-sim -- demurrage-quantization-sweep
```
Sim source: `cluster-tax/src/simulation/demurrage_quantization_sweep.rs` (deterministic; seed `0xB07A_1875`; about 4 s in release). Every table in §5 is that command's output pasted verbatim. CI regenerates it and fails on drift (`scripts/research/check_research_tables.py`, golden `generated/demurrage-quantization-sweep.txt`).

## 1. The finding

`demurrage_charge` floors the time fraction before it applies the value:

```text
time_fraction = elapsed × 1_000_000 / blocks_per_year            // floored
charge        = (value × rate × p / 10_000 / 5_000) × time_fraction / 1_000_000
```

At 6,307,200 blocks per year, one `TIME_SCALE` unit is about 6.31 blocks. So:

- every hold of 1 to 6 blocks is charged **zero**, for any value (the Lean `six_block_dead_zone`, reproduced on the production gate by PR #1899);
- every longer hold is charged a whole number of units, so `frac(elapsed × 1e6 / bpy)` units leak on **every** spend. A 12-block hold pays 52.6% of its true charge.

Consensus feeds `elapsed` from the ring's oldest member (`ring_elapsed_quantile@max`) and enforces no minimum ring-member age. A custom wallet whose ring members are all young can therefore churn.

## 2. Candidates

| option | rule | consensus surface |
|--------|------|-------------------|
| 4 defer | today's `demurrage_charge` | none |
| 1 exact | `floor(value × rate × p × elapsed / (10_000 × 5_000 × bpy))`, floored once at the end | the kernel |
| 2a ≥1 pico | today's charge, but at least 1 pico when the true charge is nonzero | the kernel |
| 2b ceil time | today's formula with the time fraction rounded **up** | the kernel |
| 2c exact ceil | option 1 rounded up once at the end (a variant, not in the ruling's list) | the kernel |
| 3a all≥M | today's kernel; reject a ring with **any** member younger than M (the real input must be M blocks old) | ring validation |
| 3b max≥M | today's kernel; reject a ring whose **oldest** member is younger than M. Under `@max` this is the same as charging `max(elapsed, M)` | ring validation (or a one-line clamp) |

Option 1's production-shaped kernel (`exact_charge` in the sim) uses only `u128`:
`num = value × rate × p` (< 2^109), `den = 10_000 × 5_000 × bpy` (< 2^90),
`charge = (num / den) × elapsed + floor((num % den) × elapsed / den)`, with
`checked_mul` saturation to `u64::MAX`. It is exact over the whole `u64` input domain whenever
`bpy ≤ 368,934,881,474` (any block time above about 86 µs). Beyond that it saturates upward. The
sim checks it against a 256-bit oracle.

## 3. Method

- **(a) Attack.** An optimal churner holds its principal in hops of `h` blocks for a year
  (`floor(bpy/h)·h` blocks). It is compared with a buy-and-hold holder **under the same rule**.
  Fees are the node's consensus floor for a 1-input, 1-output CLSAG spend
  (`consensus_fee_breakdown`: 4,998 pico at 5.745×). Two cost bounds bracket reality:
  - *Cheapest*: consensus floor only, one coin, and young decoys supplied by a busy chain. This
    is what a miner pays, or anyone who bribes one to include its transactions.
  - *Dearest*: the relay minimum `max(floor, 1e8 pico)`. The principal is split into 20 coins
    that churn round-robin, so they are each other's young decoys and no chain throughput is
    assumed.

  Lottery recapture of fees is ignored, which overstates the attacker's cost. Principals are
  1k / 100k / 1M / 10M BTH. Factors come from 100k / 1M / 10M / 1B BTH clusters on the
  production curve (3.5× to 6.0×), with principal ≤ cluster. Holds 1..=50 are profiled, and the
  optimum is searched over 1..=17,280 (one day).
- **(b) Honest users.** Honest rings come from the production node-picker port
  (`privacy_routes`), and the consensus statistic is the ring's oldest member, as in the node.
  Two populations: the gamma spend model on a 1.2M-block chain, and short-hold commerce (real
  ages 10 blocks to 1 day on a one-output-per-block chain). Values are log-uniform 1 to 1M BTH.
  Each kernel is compared with today's charge and with the exact rational charge, computed in
  256-bit arithmetic. Split-versus-unsplit loss is measured over 2,000 random time and value
  splits.
- **(c) Option 3.** `M` is tuned against today's kernel. Young real inputs are run through the
  node, web and CLI picker ports with the `privacy_routes` Age-Rank and Band-Center adversaries.
  A raised floor (M > 10) is simulated by restricting the node picker's decoys to ages ≥ M.
- **(d) Extremes.** Every kernel is evaluated at `u64::MAX` values, elapsed and rates, and at
  absurd `blocks_per_year`, against the oracle. Release builds keep `overflow-checks = true`, and
  the unit tests run every kernel on these inputs.

## 4. Headline numbers

Headline cell: a 1M BTH coin in a 10M BTH cluster (5.745×, the PR #1899 repro). A one-year hold
owes 18,980 BTH. Annual figures are BTH per year.

| option | escaped at the attack optimum, cheapest / dearest | best hold | honest extra vs today, gamma / short holds | per-tx error vs exact (pico) | split loss bound | break-even hold | young coins unspendable |
|---|---|---|---|---|---|---|---|
| 4 defer | 18,980 / 16,878 (100% of the charge) | 6 | 0 / 0 | down to −1.7e10 (−0.017 BTH) | up to 1 unit (A/1e6) per piece, 1.0e11 pico seen | none: net > 0 at every hold up to 1 day; ≥ 1% of the charge up to h = 618 | no |
| 1 exact | never profitable | – | +0.0005% / +0.13% | (−1, 0] | [0, n) pico | never profitable | no |
| 2a ≥1 pico | 18,980 / 16,878 | 6 | 0 / 0 | as defer | as defer | as defer | no |
| 2b ceil time | never profitable | – | +0.0011% / +0.25% | up to +1.7e10 | up to −1 unit per piece (splitting costs more) | never profitable | no |
| 2c exact ceil | never profitable | – | +0.0005% / +0.13% | [0, +1) | (−n, 0] pico | never profitable | no |
| 3a all≥7 / ≥10 | 9,004 / 9,004 (47%) | 12 | 0 / 0 | as defer | as defer | as defer | up to 6 / 9 blocks (30 / 45 s) |
| 3b max≥7 / ≥10 | 9,004 / 9,004 (47%) | 12 | 0 / 0 | as defer | as defer (worse for parts < M) | as defer | no |
| 3a all≥631 | 187 (0.99%) | 637 | 0 / 0 | as defer | as defer | as defer | up to 630 blocks (52.5 min) |

Across the grid, today's kernel leaks the whole charge on every principal at the cheapest cost:
10 BTH/yr on 1k BTH at 3.5×, up to 199,999 BTH/yr on 10M BTH at 6×. At the dearest cost the
relay minimum matters only for small coins (1k BTH: 0.05 to 0.2 BTH/yr). From 1M BTH up, the
leak stays above 85% of the charge.

### Break-even

Under today's truncation there is no practical break-even hold. One hop leaks up to one
`TIME_SCALE` unit, which is `A/1e6` of the coin's annual charge `A` (0.019 BTH on the headline
coin). That is far above the size fee. Once a hop's own demurrage exceeds the relay minimum
(about 630 to 1,300 blocks for a 50 BTH coin at 6× to 3.5×), the relay minimum stops binding
too. So churning stays
net-profitable at every hold searched, for every principal (§5 A4). The gain only falls as
`1/h`. For principals ≥ 100k BTH it drops below 1% of the charge after about 618 blocks and below
0.1% after about 6,181 blocks.
Options 1, 2b and 2c are never profitable at any hold, because the most a churner can save is
under 1 pico per hop and every hop costs at least 4,998 pico.

### Why option 3 alone does not close it

The dead zone is the visible part. The rest of the leak is truncation. The worst escape at holds
≥ M is about `1/(M × 1e6/bpy)`. That is 47% at M = 7 or 10 (at h = 12), 8.6% at M = 64, 0.99%
at M = 631 and 0.10% at M = 6,308 (§5 C1). Bounding the leak to ε needs M ≈ 6.3/ε blocks. Even
there, the escape stays net-profitable for every principal in the grid.

### Honest users: what the minimum would touch

No honest wallet reaches the dead zone today. Every production picker draws decoys only at
ages ≥ 10 (`MIN_DECOY_AGE_BLOCKS`), so an honest ring's oldest member is always ≥ 10. In both
populations the smallest elapsed statistic is 29 (short holds) or 218,531 (gamma). So:

- 3a and 3b at M ≤ 10 change **no** honest charge and reject no honest gamma or short-hold
  spend (§5 B1, B2).
- 3a at M ≤ 10 affects only coins younger than M. The CLI already refuses those, and the node
  and web pickers spend them with a nearest-age fallback. In that case the real input is the
  **youngest ring member 100% of the time** (§5 C2), so such spends are already fully
  identifiable. Making them wait up to M − 1 blocks costs 30 to 45 s of liveness and removes a
  privacy leak, but it is a #1882 question, not a demurrage fix.
- A minimum above 10 moves the young-coin privacy cliff up to M. At M = 20 or 64, real inputs
  aged from M up to between 1.05·M and 1.25·M are identified 100% of the time by the Age-Rank
  adversary, compared with 50% to 55% at the same ages under today's floor of 10 (§5 C3,
  1 output/block). At
  M = 631, only a real input exactly M blocks old is exposed. The gamma model has no spends or
  decoys younger than 17,280 blocks (§5 C4), so the population-level decoy-distribution effect
  is nil. The gamma model is a weak proxy for short holds, though (it was fit to Monero, which
  has a 10-block lock).

**#1882 interaction.** For real ages below 20 the ±5% window has width `2·floor(age/20) = 0`, so
it holds only the real input's own block. Young coins therefore always fall through to the
nearest-age fallback, which identifies the real input (C2: Age-Rank 25% to 100% at ages 10 to
30). A 3a minimum does not change the window. Any #1882 widening rule would also have to stop
at M, so near M the widening becomes one-sided and re-centres the ring. Options 1 and 2 have no
interaction with #1882.

## 5. Results (generated)

Seed 0xb07a1875. 6307200 blocks/year, 200 bps, ring size 20. Kernels: see `simulation::demurrage_quantization_sweep`.

### (a) Churn attack

Headline cell: 1000000 BTH coin in a 10M BTH cluster (factor 5.745x), rate 200 bps. A one-year hold owes 18980.0000 BTH (exact). Hop fee (1-in-1-out, consensus floor): 4998 pico; relay minimum: 100000000 pico.

#### A1. Gross escape by hold length (headline cell, % of the buy-and-hold charge under the same rule; `rej` = consensus rejects the spend)

| hold | 4 defer | 1 exact | 2a >=1 pico | 2b ceil time | 2c exact ceil | 3a all>=7 | 3a all>=10 | 3b max>=7 | 3b max>=10 |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 100.00% | 0.00% | 100.00% | -530.72% | -0.00% | rej | rej | -530.72% | -530.72% |
| 2 | 100.00% | 0.00% | 100.00% | -215.36% | -0.00% | rej | rej | -215.36% | -215.36% |
| 3 | 100.00% | 0.00% | 100.00% | -110.24% | -0.00% | rej | rej | -110.24% | -110.24% |
| 4 | 100.00% | 0.00% | 100.00% | -57.68% | -0.00% | rej | rej | -57.68% | -57.68% |
| 5 | 100.00% | 0.00% | 100.00% | -26.14% | -0.00% | rej | rej | -26.14% | -26.14% |
| 6 | 100.00% | 0.00% | 100.00% | -5.12% | -0.00% | rej | rej | -5.12% | -5.12% |
| 7 | 9.90% | 0.00% | 9.90% | -80.21% | -0.00% | 9.90% | rej | 9.90% | 9.90% |
| 8 | 21.16% | 0.00% | 21.16% | -57.68% | -0.00% | 21.16% | rej | 21.16% | 21.16% |
| 9 | 29.92% | 0.00% | 29.92% | -40.16% | -0.00% | 29.92% | rej | 29.92% | 29.92% |
| 10 | 36.93% | 0.00% | 36.93% | -26.14% | -0.00% | 36.93% | 36.93% | 36.93% | 36.93% |
| 11 | 42.66% | 0.00% | 42.66% | -14.68% | -0.00% | 42.66% | 42.66% | 42.66% | 42.66% |
| 12 | 47.44% | 0.00% | 47.44% | -5.12% | -0.00% | 47.44% | 47.44% | 47.44% | 47.44% |
| 13 | 2.97% | 0.00% | 2.97% | -45.55% | -0.00% | 2.97% | 2.97% | 2.97% | 2.97% |
| 14 | 9.90% | 0.00% | 9.90% | -35.15% | -0.00% | 9.90% | 9.90% | 9.90% | 9.90% |
| 15 | 15.90% | 0.00% | 15.90% | -26.14% | -0.00% | 15.90% | 15.90% | 15.90% | 15.90% |
| 16 | 21.16% | 0.00% | 21.16% | -18.26% | -0.00% | 21.16% | 21.16% | 21.16% | 21.16% |
| 17 | 25.80% | 0.00% | 25.80% | -11.30% | -0.00% | 25.80% | 25.80% | 25.80% | 25.80% |
| 18 | 29.92% | 0.00% | 29.92% | -5.12% | -0.00% | 29.92% | 29.92% | 29.92% | 29.92% |
| 19 | 0.41% | 0.00% | 0.41% | -32.78% | -0.00% | 0.41% | 0.41% | 0.41% | 0.41% |
| 20 | 5.39% | 0.00% | 5.39% | -26.14% | -0.00% | 5.39% | 5.39% | 5.39% | 5.39% |
| 21 | 9.90% | 0.00% | 9.90% | -20.14% | -0.00% | 9.90% | 9.90% | 9.90% | 9.90% |
| 22 | 13.99% | 0.00% | 13.99% | -14.68% | -0.00% | 13.99% | 13.99% | 13.99% | 13.99% |
| 23 | 17.73% | 0.00% | 17.73% | -9.69% | -0.00% | 17.73% | 17.73% | 17.73% | 17.73% |
| 24 | 21.16% | 0.00% | 21.16% | -5.12% | -0.00% | 21.16% | 21.16% | 21.16% | 21.16% |
| 25 | 24.31% | 0.00% | 24.31% | -0.92% | -0.00% | 24.31% | 24.31% | 24.31% | 24.31% |
| 26 | 2.97% | 0.00% | 2.97% | -21.29% | -0.00% | 2.97% | 2.97% | 2.97% | 2.97% |
| 27 | 6.56% | 0.00% | 6.56% | -16.80% | 0.00% | 6.56% | 6.56% | 6.56% | 6.56% |
| 28 | 9.90% | 0.00% | 9.90% | -12.63% | -0.00% | 9.90% | 9.90% | 9.90% | 9.90% |
| 29 | 13.00% | 0.00% | 13.00% | -8.74% | -0.00% | 13.00% | 13.00% | 13.00% | 13.00% |
| 30 | 15.90% | 0.00% | 15.90% | -5.12% | -0.00% | 15.90% | 15.90% | 15.90% | 15.90% |
| 31 | 18.62% | 0.00% | 18.62% | -1.73% | -0.00% | 18.62% | 18.62% | 18.62% | 18.62% |
| 32 | 1.45% | 0.00% | 1.45% | -18.26% | -0.00% | 1.45% | 1.45% | 1.45% | 1.45% |
| 33 | 4.44% | 0.00% | 4.44% | -14.68% | -0.00% | 4.44% | 4.44% | 4.44% | 4.44% |
| 34 | 7.25% | 0.00% | 7.25% | -11.30% | -0.00% | 7.25% | 7.25% | 7.25% | 7.25% |
| 35 | 9.90% | 0.00% | 9.90% | -8.12% | -0.00% | 9.90% | 9.90% | 9.90% | 9.90% |
| 36 | 12.40% | 0.00% | 12.40% | -5.12% | -0.00% | 12.40% | 12.40% | 12.40% | 12.40% |
| 37 | 14.77% | 0.00% | 14.77% | -2.28% | -0.00% | 14.77% | 14.77% | 14.77% | 14.77% |
| 38 | 0.41% | 0.00% | 0.41% | -16.19% | -0.00% | 0.41% | 0.41% | 0.41% | 0.41% |
| 39 | 2.97% | 0.00% | 2.97% | -13.21% | -0.00% | 2.97% | 2.97% | 2.97% | 2.97% |
| 40 | 5.39% | 0.00% | 5.39% | -10.38% | -0.00% | 5.39% | 5.39% | 5.39% | 5.39% |
| 41 | 7.70% | 0.00% | 7.70% | -7.68% | -0.00% | 7.70% | 7.70% | 7.70% | 7.70% |
| 42 | 9.90% | 0.00% | 9.90% | -5.12% | -0.00% | 9.90% | 9.90% | 9.90% | 9.90% |
| 43 | 11.99% | 0.00% | 11.99% | -2.68% | -0.00% | 11.99% | 11.99% | 11.99% | 11.99% |
| 44 | 13.99% | 0.00% | 13.99% | -0.34% | -0.00% | 13.99% | 13.99% | 13.99% | 13.99% |
| 45 | 1.89% | 0.00% | 1.89% | -12.13% | -0.00% | 1.89% | 1.89% | 1.89% | 1.89% |
| 46 | 4.02% | 0.00% | 4.02% | -9.69% | -0.00% | 4.02% | 4.02% | 4.02% | 4.02% |
| 47 | 6.06% | 0.00% | 6.06% | -7.36% | -0.00% | 6.06% | 6.06% | 6.06% | 6.06% |
| 48 | 8.02% | 0.00% | 8.02% | -5.12% | -0.00% | 8.02% | 8.02% | 8.02% | 8.02% |
| 49 | 9.90% | 0.00% | 9.90% | -2.97% | -0.00% | 9.90% | 9.90% | 9.90% | 9.90% |
| 50 | 11.70% | 0.00% | 11.70% | -0.92% | -0.00% | 11.70% | 11.70% | 11.70% | 11.70% |

#### A2. Net escape at the attack optimum, cheapest cost (consensus floor, 1 coin): BTH/yr (best hold); holds searched 1..=17280

| principal | cluster | factor | 4 defer | 1 exact | 2a >=1 pico | 2b ceil time | 2c exact ceil | 3a all>=7 | 3a all>=10 | 3b max>=7 | 3b max>=10 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1000 | 100k | 3.500x | 9.9968 (h=6) | none | 9.9968 (h=6) | none | none | 4.7424 (h=12) | 4.7424 (h=12) | 4.7424 (h=12) | 4.7424 (h=12) |
| 1000 | 1M | 5.093x | 16.3673 (h=6) | none | 16.3673 (h=6) | none | none | 7.7645 (h=12) | 7.7645 (h=12) | 7.7645 (h=12) | 7.7645 (h=12) |
| 1000 | 10M | 5.745x | 18.9747 (h=6) | none | 18.9747 (h=6) | none | none | 9.0015 (h=12) | 9.0015 (h=12) | 9.0015 (h=12) | 9.0015 (h=12) |
| 1000 | 1B | 6.000x | 19.9945 (h=6) | none | 19.9945 (h=6) | none | none | 9.4853 (h=12) | 9.4853 (h=12) | 9.4853 (h=12) | 9.4853 (h=12) |
| 100000 | 100k | 3.500x | 999.9968 (h=6) | none | 999.9968 (h=6) | none | none | 474.3984 (h=12) | 474.3984 (h=12) | 474.3984 (h=12) | 474.3984 (h=12) |
| 100000 | 1M | 5.093x | 1637.1953 (h=6) | none | 1637.1953 (h=6) | none | none | 776.6854 (h=12) | 776.6854 (h=12) | 776.6854 (h=12) | 776.6854 (h=12) |
| 100000 | 10M | 5.745x | 1897.9947 (h=6) | none | 1897.9947 (h=6) | none | none | 900.4086 (h=12) | 900.4086 (h=12) | 900.4086 (h=12) | 900.4086 (h=12) |
| 100000 | 1B | 6.000x | 1999.9945 (h=6) | none | 1999.9945 (h=6) | none | none | 948.7973 (h=12) | 948.7973 (h=12) | 948.7973 (h=12) | 948.7973 (h=12) |
| 1000000 | 1M | 5.093x | 16371.9953 (h=6) | none | 16371.9953 (h=6) | none | none | 7766.8745 (h=12) | 7766.8745 (h=12) | 7766.8745 (h=12) | 7766.8745 (h=12) |
| 1000000 | 10M | 5.745x | 18979.9947 (h=6) | none | 18979.9947 (h=6) | none | none | 9004.1094 (h=12) | 9004.1094 (h=12) | 9004.1094 (h=12) | 9004.1094 (h=12) |
| 1000000 | 1B | 6.000x | 19999.9945 (h=6) | none | 19999.9945 (h=6) | none | none | 9487.9973 (h=12) | 9487.9973 (h=12) | 9487.9973 (h=12) | 9487.9973 (h=12) |
| 10000000 | 10M | 5.745x | 189799.9947 (h=6) | none | 189799.9947 (h=6) | none | none | 90041.1174 (h=12) | 90041.1174 (h=12) | 90041.1174 (h=12) | 90041.1174 (h=12) |
| 10000000 | 1B | 6.000x | 199999.9945 (h=6) | none | 199999.9945 (h=6) | none | none | 94879.9973 (h=12) | 94879.9973 (h=12) | 94879.9973 (h=12) | 94879.9973 (h=12) |

#### A3. Net escape at the attack optimum, dearest cost (relay minimum, 20 self-decoy coins): BTH/yr (best hold); holds searched 1..=17280

| principal | cluster | factor | 4 defer | 1 exact | 2a >=1 pico | 2b ceil time | 2c exact ceil | 3a all>=7 | 3a all>=10 | 3b max>=7 | 3b max>=10 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1000 | 100k | 3.500x | 0.0488 (h=1274) | none | 0.0488 (h=1274) | none | none | 0.0488 (h=1274) | 0.0488 (h=1274) | 0.0488 (h=1274) | 0.0488 (h=1274) |
| 1000 | 1M | 5.093x | 0.1294 (h=782) | none | 0.1294 (h=782) | none | none | 0.1294 (h=782) | 0.1294 (h=782) | 0.1294 (h=782) | 0.1294 (h=782) |
| 1000 | 10M | 5.745x | 0.1699 (h=681) | none | 0.1699 (h=681) | none | none | 0.1699 (h=681) | 0.1699 (h=681) | 0.1699 (h=681) | 0.1699 (h=681) |
| 1000 | 1B | 6.000x | 0.1961 (h=637) | none | 0.1961 (h=637) | none | none | 0.1961 (h=637) | 0.1961 (h=637) | 0.1961 (h=637) | 0.1961 (h=637) |
| 100000 | 100k | 3.500x | 299.1787 (h=18) | none | 299.1787 (h=18) | none | none | 299.1787 (h=18) | 299.1787 (h=18) | 299.1787 (h=18) | 299.1787 (h=18) |
| 100000 | 1M | 5.093x | 586.0000 (h=12) | none | 586.0000 (h=12) | none | none | 586.0000 (h=12) | 586.0000 (h=12) | 586.0000 (h=12) | 586.0000 (h=12) |
| 100000 | 10M | 5.745x | 846.8000 (h=12) | none | 846.8000 (h=12) | none | none | 846.8000 (h=12) | 846.8000 (h=12) | 846.8000 (h=12) | 846.8000 (h=12) |
| 100000 | 1B | 6.000x | 948.7451 (h=12) | none | 948.7451 (h=12) | none | none | 948.7451 (h=12) | 948.7451 (h=12) | 948.7451 (h=12) | 948.7451 (h=12) |
| 1000000 | 1M | 5.093x | 14269.6001 (h=6) | none | 14269.6001 (h=6) | none | none | 7766.8302 (h=12) | 7766.8302 (h=12) | 7766.8302 (h=12) | 7766.8302 (h=12) |
| 1000000 | 10M | 5.745x | 16877.6001 (h=6) | none | 16877.6001 (h=6) | none | none | 9004.0595 (h=12) | 9004.0595 (h=12) | 9004.0595 (h=12) | 9004.0595 (h=12) |
| 1000000 | 1B | 6.000x | 17897.6001 (h=6) | none | 17897.6001 (h=6) | none | none | 9487.9451 (h=12) | 9487.9451 (h=12) | 9487.9451 (h=12) | 9487.9451 (h=12) |
| 10000000 | 10M | 5.745x | 187697.6001 (h=6) | none | 187697.6001 (h=6) | none | none | 90041.0675 (h=12) | 90041.0675 (h=12) | 90041.0675 (h=12) | 90041.0675 (h=12) |
| 10000000 | 1B | 6.000x | 197897.6001 (h=6) | none | 197897.6001 (h=6) | none | none | 94879.9451 (h=12) | 94879.9451 (h=12) | 94879.9451 (h=12) | 94879.9451 (h=12) |

#### A4. Break-even and materiality holds, dearest cost: largest hold in 1..=17280 with net escape > 0 / >= 1% / >= 0.1% of the buy-and-hold charge (`none` = never; `17280` = still true at the search cap)

| principal | cluster | 4 defer | 1 exact | 2a >=1 pico | 2b ceil time | 2c exact ceil | 3a all>=7 | 3a all>=10 | 3b max>=7 | 3b max>=10 |
|---|---|---|---|---|---|---|---|---|---|---|
| 1000 | 100k | 17280 / none / 6181 | none / none / none | 17280 / none / 6181 | none / none / none | none / none / none | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 |
| 1000 | 1M | 17280 / none / 6181 | none / none / none | 17280 / none / 6181 | none / none / none | none / none / none | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 |
| 1000 | 10M | 17280 / none / 6181 | none / none / none | 17280 / none / 6181 | none / none / none | none / none / none | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 |
| 1000 | 1B | 17280 / none / 6181 | none / none / none | 17280 / none / 6181 | none / none / none | none / none / none | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 | 17280 / none / 6181 |
| 100000 | 100k | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 100000 | 1M | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 100000 | 10M | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 100000 | 1B | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 1000000 | 1M | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 1000000 | 10M | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 1000000 | 1B | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 10000000 | 10M | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |
| 10000000 | 1B | 17280 / 618 / 6181 | none / none / none | 17280 / 618 / 6181 | none / none / none | none / none / none | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 | 17280 / 618 / 6181 |

### (b) Honest users

#### B1. gamma spend model (node picker, default chain) (1000 spends; elapsed statistic min 218531, median 551680 blocks; values log-uniform 1..1M BTH)

| kernel | extra vs today | extra BTH / 1k spends | bias vs exact rational | err min (pico) | err p50 | err p99 | err max | max rel err | spends changed | rejected |
|---|---|---|---|---|---|---|---|---|---|---|
| 4 defer | +0.000000% | 0.000000 | -0.000544% | -17055071192.258 | -7977623.012 | -3033.471 | -385.346 | 2.490e-5 | 0.0% | 0.0% |
| 1 exact | +0.000544% | 0.573829 | -0.000000% | -0.998 | -0.497 | -0.010 | -0.000 | 8.432e-10 | 100.0% | 0.0% |
| 2a >=1 pico | +0.000000% | 0.000000 | -0.000544% | -17055071192.258 | -7977623.012 | -3033.471 | -385.346 | 2.490e-5 | 0.0% | 0.0% |
| 2b ceil time | +0.001114% | 1.175737 | +0.000570% | 13.908 | 7124568.989 | 9809127568.258 | 17494168956.089 | 2.205e-5 | 100.0% | 0.0% |
| 2c exact ceil | +0.000544% | 0.573829 | +0.000000% | 0.002 | 0.503 | 0.990 | 1.000 | 1.140e-9 | 100.0% | 0.0% |
| 3a all>=7 | +0.000000% | 0.000000 | -0.000544% | -17055071192.258 | -7977623.012 | -3033.471 | -385.346 | 2.490e-5 | 0.0% | 0.0% |
| 3a all>=10 | +0.000000% | 0.000000 | -0.000544% | -17055071192.258 | -7977623.012 | -3033.471 | -385.346 | 2.490e-5 | 0.0% | 0.0% |
| 3b max>=7 | +0.000000% | 0.000000 | -0.000544% | -17055071192.258 | -7977623.012 | -3033.471 | -385.346 | 2.490e-5 | 0.0% | 0.0% |
| 3b max>=10 | +0.000000% | 0.000000 | -0.000544% | -17055071192.258 | -7977623.012 | -3033.471 | -385.346 | 2.490e-5 | 0.0% | 0.0% |

#### B2. short holds 10 blocks..1 day (node picker, 1 output/block) (1200 spends; elapsed statistic min 29, median 127 blocks; values log-uniform 1..1M BTH)

| kernel | extra vs today | extra BTH / 1k spends | bias vs exact rational | err min (pico) | err p50 | err p99 | err max | max rel err | spends changed | rejected |
|---|---|---|---|---|---|---|---|---|---|---|
| 4 defer | +0.000000% | 0.000000 | -0.133012% | -14021196045.095 | -6896984.085 | -5431.657 | -571.057 | 1.590e-1 | 0.0% | 0.0% |
| 1 exact | +0.133189% | 0.597432 | -0.000000% | -0.999 | -0.498 | -0.013 | -0.001 | 1.634e-5 | 100.0% | 0.0% |
| 2a >=1 pico | +0.000000% | 0.000000 | -0.133012% | -14021196045.095 | -6896984.085 | -5431.657 | -571.057 | 1.590e-1 | 0.0% | 0.0% |
| 2b ceil time | +0.245830% | 1.102692 | +0.112491% | 280.214 | 5169896.786 | 7405538924.836 | 16552860429.009 | 8.745e-2 | 100.0% | 0.0% |
| 2c exact ceil | +0.133189% | 0.597432 | +0.000000% | 0.001 | 0.502 | 0.987 | 0.999 | 1.482e-5 | 100.0% | 0.0% |
| 3a all>=7 | +0.000000% | 0.000000 | -0.133012% | -14021196045.095 | -6896984.085 | -5431.657 | -571.057 | 1.590e-1 | 0.0% | 0.0% |
| 3a all>=10 | +0.000000% | 0.000000 | -0.133012% | -14021196045.095 | -6896984.085 | -5431.657 | -571.057 | 1.590e-1 | 0.0% | 0.0% |
| 3b max>=7 | +0.000000% | 0.000000 | -0.133012% | -14021196045.095 | -6896984.085 | -5431.657 | -571.057 | 1.590e-1 | 0.0% | 0.0% |
| 3b max>=10 | +0.000000% | 0.000000 | -0.133012% | -14021196045.095 | -6896984.085 | -5431.657 | -571.057 | 1.590e-1 | 0.0% | 0.0% |

#### B3. Split-versus-unsplit loss, pico: `charge(whole) − Σ charge(parts)` over 2000 random splits (positive = splitting pays less; holds log-uniform 1 block..~1 week)

| kernel | parts | time split max | time split min | value split max | value split min |
|---|---|---|---|---|---|
| 4 defer | 2 | 19070362789 | 0 | 1 | 0 |
| 4 defer | 10 | 104531854711 | 0 | 8 | 1 |
| 1 exact | 2 | 1 | 0 | 1 | 0 |
| 1 exact | 10 | 9 | 0 | 7 | 2 |
| 2a >=1 pico | 2 | 19070362789 | -1 | 1 | -1 |
| 2a >=1 pico | 10 | 104531854711 | -3 | 8 | 1 |
| 2b ceil time | 2 | 1 | -18627295925 | 1 | 0 |
| 2b ceil time | 10 | -33464 | -108678444402 | 7 | 2 |
| 2c exact ceil | 2 | 0 | -1 | 0 | -1 |
| 2c exact ceil | 10 | 0 | -9 | -2 | -7 |
| 3a all>=7 | 2 | 19070912166 | 0 | 1 | 0 |
| 3a all>=7 | 10 | 104531854711 | 35328 | 8 | 1 |
| 3a all>=10 | 2 | 19070912166 | 0 | 1 | 0 |
| 3a all>=10 | 10 | 104531854711 | 35328 | 8 | 1 |
| 3b max>=7 | 2 | 19070362789 | -16004132427 | 1 | 0 |
| 3b max>=7 | 10 | 104531854711 | -126791518469 | 8 | 1 |
| 3b max>=10 | 2 | 19070362789 | -16004132427 | 1 | 0 |
| 3b max>=10 | 10 | 104531854711 | -126791518469 | 8 | 1 |

### (c) Option 3: minimum ring-member age

#### C1. Tuning M with today's floored kernel (headline cell)

| M (blocks) | M (time) | worst gross escape, h>=M | at h | 3a net BTH/yr cheapest | 3a net BTH/yr dearest | 3b net BTH/yr dearest | 3a max wait |
|---|---|---|---|---|---|---|---|
| 7 | 0.6 min | 47.44% | 12 | 9004.1094 | 9004.0595 | 9004.0595 | 6 blocks (30 s) |
| 10 | 0.8 min | 47.44% | 12 | 9004.1094 | 9004.0595 | 9004.0595 | 9 blocks (45 s) |
| 13 | 1.1 min | 29.92% | 18 | 5678.8142 | 5678.7810 | 5678.7810 | 12 blocks (60 s) |
| 20 | 1.7 min | 24.31% | 25 | 4614.7200 | 4614.6961 | 4614.6961 | 19 blocks (95 s) |
| 64 | 5.3 min | 8.59% | 69 | 1630.6093 | 1630.6006 | 1630.6006 | 63 blocks (5.2 min) |
| 128 | 10.7 min | 4.44% | 132 | 841.9905 | 841.9860 | 841.9860 | 127 blocks (10.6 min) |
| 631 | 52.6 min | 0.99% | 637 | 187.1048 | 187.1039 | 187.1039 | 630 blocks (52.5 min) |
| 1262 | 105.2 min | 0.49% | 1274 | 93.1348 | 93.1344 | 93.1344 | 1261 blocks (105.1 min) |
| 6308 | 525.7 min | 0.10% | 6345 | 18.7333 | 18.7332 | 18.7332 | 6307 blocks (525.6 min) |

#### C2. Young real inputs under today's wallet floor (production picker ports, 300 rings per row; youngest = real input has the youngest age rank, ties split; Age-Rank / Band-Center = `privacy_routes` adversaries' identification rates; ideal 5%)

| real age | outputs/block | node youngest | node Age-Rank | node Band-Center | web Age-Rank | cli refuses | node median @max overcharge | 3a M=7 wait | 3a M=10 wait |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 1 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 28.00 | 6 | 9 |
| 3 | 1 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 9.33 | 4 | 7 |
| 6 | 1 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 4.67 | 1 | 4 |
| 7 | 1 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 4.00 | 0 | 3 |
| 9 | 1 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 3.11 | 0 | 1 |
| 10 | 1 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 2.90 | 0 | 0 |
| 12 | 1 | 0.0% | 100.0% | 0.0% | 100.0% | 100% | 2.42 | 0 | 0 |
| 15 | 1 | 0.0% | 100.0% | 0.0% | 100.0% | 100% | 1.93 | 0 | 0 |
| 19 | 1 | 0.0% | 100.0% | 50.0% | 100.0% | 100% | 1.53 | 0 | 0 |
| 20 | 1 | 0.0% | 43.7% | 50.0% | 48.7% | 100% | 1.45 | 0 | 0 |
| 30 | 1 | 0.0% | 47.0% | 50.0% | 58.0% | 100% | 1.33 | 0 | 0 |
| 60 | 1 | 0.0% | 50.7% | 50.0% | 44.7% | 100% | 1.15 | 0 | 0 |
| 120 | 1 | 0.0% | 35.7% | 34.8% | 35.7% | 100% | 1.07 | 0 | 0 |
| 1 | 4 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 14.00 | 6 | 9 |
| 3 | 4 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 4.67 | 4 | 7 |
| 6 | 4 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 2.33 | 1 | 4 |
| 7 | 4 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 2.00 | 0 | 3 |
| 9 | 4 | 100.0% | 100.0% | 0.0% | 100.0% | 100% | 1.56 | 0 | 1 |
| 10 | 4 | 25.0% | 25.0% | 0.0% | 25.0% | 100% | 1.40 | 0 | 0 |
| 12 | 4 | 0.0% | 25.0% | 25.0% | 25.0% | 100% | 1.17 | 0 | 0 |
| 15 | 4 | 0.0% | 25.0% | 25.0% | 25.0% | 100% | 1.13 | 0 | 0 |
| 19 | 4 | 0.0% | 25.0% | 25.0% | 25.0% | 100% | 1.11 | 0 | 0 |
| 20 | 4 | 0.0% | 25.0% | 25.0% | 25.0% | 100% | 1.10 | 0 | 0 |
| 30 | 4 | 0.0% | 25.0% | 25.0% | 25.0% | 100% | 1.07 | 0 | 0 |
| 60 | 4 | 4.8% | 5.3% | 5.4% | 6.1% | 0% | 1.05 | 0 | 0 |
| 120 | 4 | 4.0% | 5.3% | 5.9% | 3.4% | 0% | 1.05 | 0 | 0 |

#### C3. A minimum above the wallet floor (3a, M > 10): real inputs just above M, node picker with decoys restricted to ages >= M (1 output/block; Age-Rank here is in-sample)

| M | real age | youngest, floor M | Age-Rank, floor M | youngest, floor 10 | Age-Rank, floor 10 | median @max overcharge, floor M |
|---|---|---|---|---|---|---|
| 20 | 20 | 100.0% | 100.0% | 0.0% | 52.0% | 1.950 |
| 20 | 21 | 0.0% | 100.0% | 0.0% | 51.7% | 1.857 |
| 20 | 25 | 0.0% | 100.0% | 0.0% | 54.7% | 1.560 |
| 20 | 40 | 0.0% | 52.0% | 0.0% | 55.0% | 1.250 |
| 64 | 64 | 100.0% | 100.0% | 0.0% | 55.3% | 1.297 |
| 64 | 67 | 0.0% | 100.0% | 0.0% | 50.7% | 1.239 |
| 64 | 80 | 0.0% | 51.0% | 0.0% | 50.3% | 1.113 |
| 64 | 128 | 0.0% | 37.7% | 0.0% | 35.7% | 1.070 |
| 631 | 631 | 100.0% | 100.0% | 3.7% | 9.3% | 1.048 |
| 631 | 662 | 4.3% | 9.0% | 4.7% | 7.3% | 1.048 |
| 631 | 788 | 5.7% | 7.0% | 6.7% | 7.3% | 1.053 |
| 631 | 1262 | 3.3% | 7.0% | 7.3% | 7.7% | 1.039 |

#### C4. Share of honest gamma-model rings a minimum excludes (1000 rings)

| M | real inputs younger than M | decoys younger than M |
|---|---|---|
| 7 | 0.00% | 0.00% |
| 10 | 0.00% | 0.00% |
| 64 | 0.00% | 0.00% |
| 631 | 0.00% | 0.00% |
| 6308 | 0.00% | 0.00% |
| 17280 | 0.00% | 0.00% |

### (d) Determinism and overflow at extreme inputs

`exact` is exact up to `blocks_per_year` = 368934881474; `oracle` is the 256-bit exact floor (clamped to u64::MAX). `=` marks a kernel equal to the oracle floor, `^` equal to its ceiling.

| input | oracle floor | 4 defer | 1 exact | 2a >=1 pico | 2b ceil time | 2c exact ceil |
|---|---|---|---|---|---|---|
| u64::MAX value, 6 blocks | 350965450413 | 0 | 350965450413 = | 1 | 368934881474 | 350965450414 ^ |
| u64::MAX value, 7 blocks | 409459692148 | 368934881474 | 409459692148 = | 368934881474 | 737869762948 | 409459692149 ^ |
| u64::MAX value, 1 year | 368934881474191032 | 368934881474191032 = | 368934881474191032 = | 368934881474191032 = | 368934881474191032 = | 368934881474191033 ^ |
| u64::MAX value, u64::MAX elapsed | 18446744073709551615 | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = |
| 1 pico, 1 block, 1.001x | 0 | 0 = | 0 = | 1 ^ | 0 = | 1 ^ |
| 1M BTH, u32::MAX bps, 1 block | 68096259750761035 | 0 | 68096259750761035 = | 1 | 429496729500000000 | 68096259750761036 ^ |
| u64::MAX all, bpy = 1 | 18446744073709551615 | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = |
| u64::MAX all, bpy = limit | 18446744073709551615 | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = |
| 1M BTH, 1 block, bpy = limit | 54210 | 0 | 54210 = | 1 | 20000000000 | 54211 ^ |
| u64::MAX value, 1 block, bpy = u64::MAX | 429496 | 0 | 429496 = | 1 | 7922816249581759351 | 429497 ^ |
| u64::MAX value+elapsed, bpy = u64::MAX | 18446744073709551615 | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = | 18446744073709551615 = |

## 6. Ranked recommendation (for the operator to decide)

This ranking weighs four things in order: closing the escape, the honest-user cost, the
churn-invariance bound #1711 asks for, and the size of the consensus change. Other weightings
are legitimate. The tables above support any of them.

1. **Option 1, exact (floor once).**
   - It closes the escape completely. A churner saves under 1 pico per hop and pays at least
     4,998 pico, so it is never profitable at any hold, principal or cost bound.
   - Honest users pay only what today's kernel under-charges: +0.0005% on gamma-model spends and
     +0.13% on short holds, about 0.6 BTH per 1,000 spends of 1 to 1M BTH. Its bias against the
     exact rational is 0, and every spend's error is in (−1, 0] pico.
   - Split-versus-unsplit loss is in [0, n) pico for n pieces. That is the justified bound #1711
     asks for, and it is in the safe direction: splitting never saves more than n pico.
   - It has no privacy, wallet or liveness impact, and it is `u128`-only and saturating.
   - Cost: a kernel change in protocol 9.0.0. The Lean `Demurrage` model and its DRT fixtures
     must be regenerated, and the #1899 tests flip as intended.
   - #1711's positive-charge property becomes satisfiable with a value threshold and no
     elapsed restriction. A one-block hold at 6× and 200 bps is charged once
     `value ≥ 0.000315 BTH` (3.15e8 pico); at the lowest factor above 1×, once `value ≥ 1.58 BTH`.
2. **Option 2c, exact ceiling (a close second).** It behaves like option 1, but every error is in
   [0, +1) pico and splitting costs up to n pico extra. Choose it over option 1 if the operator
   wants "the charge is positive whenever the true charge is" as an unconditional theorem.
   Choose option 1 if "never over-charge an honest spend" (the `spend_demurrage_charge` doc
   contract) takes precedence. The engineering cost is the same, so this is a choice of rounding
   direction, not of mechanism.
3. **Option 2b, ceiling on the time fraction.** It closes the escape, because churning costs
   more than holding. But it over-charges short holds by up to a whole unit: a one-block hold is
   charged 6.3× its true value, short-hold spends pay +0.25% in aggregate, and single spends pay
   up to +0.017 BTH. It also breaks churn-invariance in the other direction by up to one unit per
   piece (−1.1e11 pico seen at 10 pieces). It is a smaller code change than option 1, but it
   keeps the quantization #1711 objects to.
4. **Option 3, minimum ring age, alone.** It does not close the escape. M = 7 or 10 leaves 47%
   of the charge escapable (about 9,000 BTH/yr on the headline coin). Reaching 1% needs
   M ≈ 631 blocks (53 min), and even then 187 BTH/yr remains profitable. 3a makes young coins
   wait up to M − 1 blocks and, for M > 10, moves the young-coin privacy cliff up to M. 3b at
   M ≤ 10 touches no honest spend, but it leaks exactly as much. Option 3 is only worth
   considering next to option 1, as defense in depth against unknown low-elapsed paths. With
   option 1 in place there is nothing left in the dead zone for it to defend. The separate
   argument that coins under 10 blocks are 100% identifiable when spent belongs to #1882.
5. **Option 2a, at least 1 pico.** Not a fix. It charges 1 pico per hop, so the escape is
   unchanged to 6 significant figures.
6. **Option 4, defer.** About 18,980 BTH/yr escapable per 1M BTH in a 10M BTH cluster, and about
   188,000 to 200,000 BTH/yr on a 10M BTH coin. It is reachable from height 6,307,200 (epoch 1,
   rate 200 bps) by any wallet able to keep its rings young. In epoch 0 the rate is 0 and
   nothing leaks.

## 7. Other effects of changing the kernel

- **#925 / #831 capitalized charges.** `capitalized_reset_charge` and
  `demurrage_settlement_charge` call `demurrage_charge` over `SETTLEMENT_HORIZON_BLOCKS` =
  31,536,000. That is exactly 5,000,000 `TIME_SCALE` units, so the time floor has no effect
  there. Under option 1, only the value-side floor goes away, and these charges change by at most
  4 pico (0 for round values such as the #1876 test's 983,800 BTH). Any pico change is still
  consensus, so it rides the same activation. Option 2b leaves them unchanged.
- **#1876.** This study did not touch `ring_centroid_implied_factor` or
  `store/validation.rs`. The two fixes compose: #1876 changes the factor, and this change
  affects how the factor is applied over time.
- **Ratified constants.** K/F, `SETTLEMENT_HORIZON_BLOCKS`, the 200 bps rate and emission are
  unchanged by every option.

## 8. Limits of the model

- The principal is held constant over the year. The demurrage it pays, 2%/yr at most, is not
  deducted.
- Lottery recapture is ignored. The attacker would win back part of its fees, and the escaped
  demurrage would otherwise have funded the lottery pool for other holders.
- Holds shorter than 10 blocks need decoys younger than any honest wallet picks. That makes the
  attacker's rings fingerprintable, but it is a privacy cost to the attacker that consensus
  cannot see, so it does not limit the attack.
- The honest populations are the gamma spend model and a fixed set of short-hold ages. Real
  short-hold frequencies on Botho are unknown.
- Option 3's privacy numbers come from the `privacy_routes` ports on synthetic chains of one and
  four outputs per block. A raised floor is modelled by filtering decoy candidates, not by
  porting a changed picker.
