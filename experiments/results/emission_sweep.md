# Emission-Schedule Sweep (issue #350)

Data to inform the permanent emission-schedule decision (#321). This report presents numbers and **neutral observations only**. It does NOT recommend or select a schedule; selection is the operator's, in a separate decision-gated issue (#351).

## Method

Two tracks per schedule (see module docs for detail):

1. **Analytic monetary track** — exact, at real-world scale (`BLOCKS_PER_YEAR = 6307200`, 5s blocks). Derived total supply, time-to-tail, %-issued-by-year, steady-state inflation and early-vs-late issuance share are computed directly from the policy parameters (emission is a deterministic function of height), not simulated.
2. **Agent-based distribution track** — simulated, at sim scale. Each schedule is **horizon-scaled**: `halving_interval` is divided by a common factor so the tail is reached within 16000 simulated blocks, while the curve *shape* and the *relative ordering* of schedules are preserved. Gini trajectory, top-1%/10% share, the subsidy-vs-recycled accounting and a velocity proxy come from this track.

**Horizon-scaling assumption (explicit):** the simulated horizon (16000 blocks across 4000 rounds) is far shorter than 10 real years (~63M blocks). Distribution outcomes are therefore comparable *between schedules under the same scaling*, not as absolute predictions of real-world year-N Gini. All schedules share one fixed, deterministic agent population (120 retail, 12 merchants, 4 whales, 4 minters); the agent RNG is seeded from agent IDs, so the run is reproducible.

## Monetary metrics (analytic, real-world scale)

| Schedule | Derived supply (BTH) | Time-to-tail (yr) | Early share (first 10%) | Early share (first 25%) | % issued by Y1 | % issued by Y2 | % issued by Y5 | Steady-state gross/yr | Steady-state net/yr |
|----------|----------------------|-------------------|-------------------------|-------------------------|----------------|----------------|----------------|-----------------------|---------------------|
| S1 | 1.222e9 | 10.00 | 25.8% | 58.1% | 25.8% | 51.6% | 83.9% | 2.50% | 2.00% |
| S2 | 3.052e8 | 2.50 | 25.8% | 58.1% | 77.4% | 96.8% | 100.0% | 2.50% | 2.00% |
| S3 | 1.018e8 | 0.83 | 25.8% | 58.1% | 100.0% | 100.0% | 100.0% | 2.50% | 2.00% |
| S4 | 9.198e7 | 0.50 | 17.1% | 42.9% | 100.0% | 100.0% | 100.0% | 2.50% | 2.00% |
| S5 | 1.018e8 | 0.83 | 25.8% | 58.1% | 100.0% | 100.0% | 100.0% | 1.50% | 1.00% |

## Distribution & MoE metrics (agent-based, horizon-scaled)

| Schedule | Gini init->final | Gini @25/50/75% | Top 1% | Top 10% | Subsidy emitted | Fees recycled | Subsidy fraction | Velocity (tx/yr) | Turnover (tx/1M BTH) | Final phase |
|----------|------------------|-----------------|--------|---------|-----------------|---------------|------------------|------------------|----------------------|-------------|
| S1 | 0.906->0.916 | 0.917/0.916/0.916 | 9.0% | 100.0% | 263501756485600 | 78865627337731 | 0.770 | 9817551 | 134886.92 | Tail Emission |
| S2 | 0.906->0.919 | 0.919/0.919/0.919 | 9.5% | 100.0% | 65780428135180 | 19687468399634 | 0.770 | 9817551 | 540321.10 | Tail Emission |
| S3 | 0.906->0.923 | 0.923/0.923/0.923 | 10.9% | 100.0% | 21894654527230 | 6552813453858 | 0.770 | 9817551 | 1623338.18 | Tail Emission |
| S4 | 0.906->0.924 | 0.924/0.924/0.924 | 11.3% | 100.0% | 19775842878542 | 5918672413865 | 0.770 | 9817551 | 1797264.17 | Tail Emission |
| S5 | 0.906->0.923 | 0.923/0.923/0.923 | 10.9% | 100.0% | 21894292710390 | 6552799924336 | 0.770 | 9817551 | 1623375.04 | Tail Emission |

## Schedule definitions

- **S1** (slow / Bitcoin-ish (~1.22B BTH, ~10yr to tail)): R0=50 BTH, H=12614400 blocks (~2.00 yr), K=5, tail=200bps.
- **S2** (medium (~305M BTH, ~2.5yr to tail)): R0=50 BTH, H=3150000 blocks (~0.50 yr), K=5, tail=200bps.
- **S3** (fast / flat (~100M BTH, ~10mo to tail)): R0=50 BTH, H=1051200 blocks (~0.17 yr), K=5, tail=200bps.
- **S4** (very fast / low front (K=3, faster to tail)): R0=50 BTH, H=1051200 blocks (~0.17 yr), K=3, tail=200bps.
- **S5** (fast / flat, 1% tail (tail-rate sensitivity vs S3)): R0=50 BTH, H=1051200 blocks (~0.17 yr), K=5, tail=100bps.

## Neutral observations

- Derived total supply spans 9.198e7 to 1.222e9 BTH across the grid (~13x), driven entirely by the halving interval H.
- Time-to-tail ranges from 0.50 to 10.00 real years; faster schedules front-load issuance into fewer years.
- Early-issuance share (first 10% of blocks-to-tail) ranges across the grid; e.g. S5 mints 25.8% of Phase-1 supply in that window vs S4 at 17.1%.
- In the simulated economy, the subsidy fraction (emission / (emission + recycled fees)) averages 0.77 across schedules; the remainder of minter-facing value comes from recycled fees. This is a sim-scale accounting at the horizon-scaled emission rate, not a real-world security-budget claim.
- Final Gini and top-share figures are reported per schedule above; compare them *between* schedules under the shared scaling rather than as absolute year-N predictions.
- Tail-rate sensitivity (same shape, different tail): S3 (2% tail) steady-state net inflation 2.00%/yr vs S5 (1% tail) 1.00%/yr; their final Gini under this run is 0.923 and 0.923 respectively.

_No schedule is recommended here. The numbers above are inputs to the operator's decision in the follow-up issue._
