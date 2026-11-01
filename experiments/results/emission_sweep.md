# Emission-Schedule Sweep (issue #350)

Data to inform the permanent emission-schedule decision (#321). This report presents numbers and **neutral observations only**. It does NOT recommend or select a schedule; selection is the operator's, in a separate decision-gated issue (#351).

## Method

Two tracks per schedule (see module docs for detail):

1. **Analytic monetary track** — exact, at real-world scale (`BLOCKS_PER_YEAR = 6307200`, 5s blocks). Derived total supply, time-to-tail, %-issued-by-year, steady-state inflation and early-vs-late issuance share are computed directly from the policy parameters (emission is a deterministic function of height), not simulated.
2. **Agent-based distribution track** — simulated, at sim scale. Each schedule is **horizon-scaled**: `halving_interval` is divided by a common factor so the tail is reached within 16000 simulated blocks, while the curve *shape* and the *relative ordering* of schedules are preserved. Gini trajectory, top-1%/10% share, the subsidy-vs-recycled accounting and a velocity proxy come from this track.

**Horizon-scaling assumption (explicit):** the simulated horizon (16000 blocks across 4000 rounds) is far shorter than 10 real years (~63M blocks). Distribution outcomes are therefore comparable *between schedules under the same scaling*, not as absolute predictions of real-world year-N Gini. All schedules share one fixed, deterministic agent population (120 retail, 12 merchants, 4 whales, 4 minters); the agent RNG is seeded from agent IDs, so the run is reproducible.

## Monetary metrics (analytic, real-world scale)

| Schedule | Derived supply (BTH) | Time-to-tail (policy-yr) | Early share (first 10%) | Early share (first 25%) | % issued by Y1 | % issued by Y2 | % issued by Y5 | Tail gross/policy-yr (calibrated) | Tail gross/policy-yr (compounded) | Policy-assumed net/yr |
|----------|----------------------|--------------------------|-------------------------|-------------------------|----------------|----------------|----------------|-----------------------------------|-----------------------------------|-----------------------|
| S1 | 1.222e9 | 10.00 | 25.8% | 58.1% | 25.8% | 51.6% | 83.9% | 2.50% | 2.532% | 2.00% |
| S2 | 3.052e8 | 2.50 | 25.8% | 58.1% | 77.4% | 96.8% | 100.0% | 2.50% | 2.532% | 2.00% |
| S3 | 1.018e8 | 0.83 | 25.8% | 58.1% | 100.0% | 100.0% | 100.0% | 2.50% | 2.532% | 2.00% |
| S4 | 9.198e7 | 0.50 | 17.1% | 42.9% | 100.0% | 100.0% | 100.0% | 2.50% | 2.532% | 2.00% |
| S5 | 1.018e8 | 0.83 | 25.8% | 58.1% | 100.0% | 100.0% | 100.0% | 1.50% | 1.511% | 1.00% |
| M | 6.110e8 | 5.00 | 25.8% | 58.1% | 51.6% | 77.4% | 100.0% | 2.50% | 2.532% | 2.00% |

*Policy-assumed net* subtracts the policy's assumed 0.5%/yr fee burn (`expected_fee_burn_rate_bps`). At realistic fee volumes the burn is far smaller, so realised net inflation is close to the gross column; see the node-parity section. Years are policy-years (6,307,200 blocks), calendar years only at 5 s blocks.

## Distribution & MoE metrics (agent-based, horizon-scaled)

| Schedule | Gini init->final | Gini @25/50/75% | Top 1% | Top 10% | Subsidy emitted | Fees recycled | Subsidy fraction | Velocity (tx/yr) | Turnover (tx/1M BTH) | Final phase |
|----------|------------------|-----------------|--------|---------|-----------------|---------------|------------------|------------------|----------------------|-------------|
| S1 | 0.906->0.916 | 0.917/0.916/0.916 | 9.0% | 100.0% | 263452507239576934 | 79009403389254688 | 0.769 | 9817551 | 135028.09 | Tail Emission |
| S2 | 0.906->0.919 | 0.919/0.919/0.919 | 9.5% | 100.0% | 65731409299853010 | 19712827224158719 | 0.769 | 9817551 | 541194.42 | Tail Emission |
| S3 | 0.906->0.923 | 0.923/0.923/0.923 | 11.0% | 100.0% | 21845037607724814 | 6551310281077897 | 0.769 | 9817551 | 1628445.41 | Tail Emission |
| S4 | 0.906->0.924 | 0.924/0.924/0.924 | 11.3% | 100.0% | 19726198056789547 | 5915872541517087 | 0.769 | 9817551 | 1803360.82 | Tail Emission |
| S5 | 0.906->0.923 | 0.923/0.923/0.923 | 11.0% | 100.0% | 21844522555524809 | 6551155353770549 | 0.769 | 9817551 | 1628483.75 | Tail Emission |
| M | 0.906->0.917 | 0.917/0.917/0.917 | 9.0% | 100.0% | 131704803216139964 | 39498268208478595 | 0.769 | 9817551 | 270100.16 | Tail Emission |

## Schedule definitions

- **S1** (slow / Bitcoin-ish (~1.22B BTH, ~10yr to tail)): R0=50 BTH, H=12614400 blocks (~2.00 yr), K=5, tail=200bps.
- **S2** (medium (~305M BTH, ~2.5yr to tail)): R0=50 BTH, H=3150000 blocks (~0.50 yr), K=5, tail=200bps.
- **S3** (fast / flat (~100M BTH, ~10mo to tail)): R0=50 BTH, H=1051200 blocks (~0.17 yr), K=5, tail=200bps.
- **S4** (very fast / low front (K=3, faster to tail)): R0=50 BTH, H=1051200 blocks (~0.17 yr), K=3, tail=200bps.
- **S5** (fast / flat, 1% tail (tail-rate sensitivity vs S3)): R0=50 BTH, H=1051200 blocks (~0.17 yr), K=5, tail=100bps.
- **M** (canonical mainnet (ratified #351; reference, not a candidate)): R0=50 BTH, H=6307200 blocks (~1.00 yr), K=5, tail=200bps.

## Neutral observations

- Derived total supply spans 9.198e7 to 1.222e9 BTH across the grid (~13x), driven entirely by the halving interval H.
- Time-to-tail ranges from 0.50 to 10.00 real years; faster schedules front-load issuance into fewer years.
- Early-issuance share (first 10% of blocks-to-tail) ranges across the grid; e.g. M mints 25.8% of Phase-1 supply in that window vs S4 at 17.1%.
- In the simulated economy, the subsidy fraction (emission / (emission + recycled fees)) averages 0.77 across schedules; the remainder of minter-facing value comes from recycled fees. This is a sim-scale accounting at the horizon-scaled emission rate, not a real-world security-budget claim.
- The agent-based track books every fee as removed from supply (`fees_recycled`). The node burns only 20% of fees and pays 80% back out through the lottery, so the track overstates supply removal by fees; its block rewards do follow the node's per-block tail recompute.
- Final Gini and top-share figures are reported per schedule above; compare them *between* schedules under the shared scaling rather than as absolute year-N predictions.
- Tail-rate sensitivity (same shape, different tail): S3 (2% tail) steady-state net inflation 2.00%/yr vs S5 (1% tail) 1.00%/yr; their final Gini under this run is 0.923 and 0.923 respectively.

_No schedule is recommended here. The numbers above are inputs to the operator's decision in the follow-up issue._

## Node parity: the live policy as the node executes it (#1699)

Issuance below is the node's `ChainState::total_mined`, computed block by block with the node's reward rule (`calculate_block_reward`: halving reward in Phase 1, tail reward **recomputed every block** from cumulative gross issuance in Phase 2). A drift guard in `botho` checks it against the node over 20 policy-years. A *policy-year* is 6,307,200 blocks — one calendar year only at 5 s blocks.

| Policy-year | Gross issued (BTH) | Growth vs prior year | Old fixed-tail model (BTH) | Old model understates by |
|------------:|-------------------:|---------------------:|---------------------------:|-------------------------:|
| 1 | 315.36M | — | 315.36M | 0.00% |
| 2 | 473.04M | 50.000% | 473.04M | 0.00% |
| 3 | 551.88M | 16.667% | 551.88M | 0.00% |
| 4 | 591.30M | 7.143% | 591.30M | 0.00% |
| 5 | 611.01M | 3.333% | 611.01M | 0.00% |
| 6 | 626.48M | 2.532% | 626.29M | 0.03% |
| 7 | 642.34M | 2.532% | 641.56M | 0.12% |
| 10 | 692.36M | 2.532% | 687.39M | 0.72% |
| 15 | 784.55M | 2.532% | 763.76M | 2.72% |
| 20 | 889.01M | 2.532% | 840.14M | 5.82% |

- Tail reward: 2.4219 BTH/block at onset, 3.5238 BTH/block at the end of policy-year 20. It grows with gross issuance; tail growth is 2.532%/policy-year with per-block compounding (calibrated rate 200+50 bps).
- Fees (shared consensus fee model, #1728): a typical 1-input/2-output transaction (990 bytes) has a consensus floor of 3960 pico; with the default relay minimum it pays 100000000 pico (0.0001 BTH). The node burns 200‰ of fees (the rest funds the lottery).
- The tail reward is calibrated as 200 bps net + 50 bps assumed fee burn. For base fees to burn 50 bps of tail-onset supply per year, the network would need **~4844 tx/s sustained** at the relay minimum.
- Demurrage is collected as part of the fee, so 20% of it is burned too. Upper bound: 0.40%/yr of supply, only if every coin sat at the maximum cluster factor and settled yearly. Factor-1 coins pay none.

| Sustained load (tx/s) | Block time | Calendar yr per policy-yr | Calendar yr to tail | Gross tail %/calendar yr | Base-fee burn %/calendar yr | Net (excl. demurrage) |
|----------------------:|-----------:|--------------------------:|--------------------:|-------------------------:|----------------------------:|----------------------:|
| 0.1 | 40 s | 8.0 | 40.0 | 0.312% | 0.00001% | 0.312% |
| 0.5 | 20 s | 4.0 | 20.0 | 0.625% | 0.00005% | 0.625% |
| 2 | 10 s | 2.0 | 10.0 | 1.250% | 0.00021% | 1.250% |
| 5 | 5 s | 1.0 | 5.0 | 2.500% | 0.00052% | 2.499% |
| 20 | 3 s | 0.6 | 3.0 | 4.167% | 0.00206% | 4.165% |
| 100 | 3 s | 0.6 | 3.0 | 4.167% | 0.01032% | 4.156% |

Neutral observations:

- Per policy-year, realised net tail inflation is the ~2.5% gross rate minus a base-fee burn that is negligible at any load the timing tiers anticipate, minus at most the demurrage-burn bound above: between ~2.1% and ~2.53% compounded, near the top in practice. The "2% net" figure holds only if burns reach the assumed 0.5%/yr. Whether to keep and restate the figure or recalibrate the tail is an open operator decision (#1788).
- In calendar time, emission and the run-up to tail scale with block time: blocks are load-paced (3–40 s), so "5 years to tail" is 3 years at sustained ≥20 tx/s and 40 years when idle.
- The calendar columns assume blocks run at the load-paced tier. Whether the 5 s difficulty target eases to `MAX_DIFFICULTY` at low load on the live network is a testnet measurement, not modelled here.
- These are inputs to a human decision. The ratified parameters (#351) are unchanged.
