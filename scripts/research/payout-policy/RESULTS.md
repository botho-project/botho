# Inactive payout-policy observations

One local capture passed: compile 41.81 seconds, matrix 7.41 seconds; six histories,
one replay and exact archived PR1370 baseline row comparisons. Five separate
Python edge checks and the independent owner/recurrence/FIFO importer pass.
Release assertions and overflow checks are enabled. Hosted Linux is pending.

Both seeds complete all 113 ordinary payments. Per-seed totals in BTH:

| Policy | Consolidations (1306 / 902) | Gross fees | Capture | Burn | Reserve | Consolidation remaining principal |
|---|---:|---:|---:|---:|---:|---:|
| Baseline | 0 / 0 | 56.5 | 45.2 | 11.3 | 0 | 0 |
| Threshold reserve | 154 / 152 | 95.5 | 76.4 | 19.1 | 0 | 31.8 |
| Adaptive count | 288 / 285 | 129.25 | 103.4 | 25.85 | 0 | 22.65 |

Consolidation fees alone are 39 BTH under reserve and 72.75 BTH under adaptive count.
Consumed award values are70.8 and 95.4 BTH respectively: fees consume about 55.08%
and 76.26% of these values. The remaining-principal column sums outputs at their
creation; it is not additional terminal wealth, since outputs can be used later.
More successful consolidations therefore do not establish useful affordability.
Threshold 0.250001 BTH only leaves the existing minimum positive change after a
background one-input fee. No useful payment minimum or acceptable fraction was
ratified. Both fees and awards are denominated in BTH; purchasing-power movement
alone does not repair their fixed ratio. CPU measurements are not electricity.

Reserve policy has 42 draws, 7,100 accumulating blocks and maximum FIFO wait 200 blocks
per seed. Nine released fee lots had at least one original eligible candidate
expire before release. This is not nine lost entitlements: FIFO is an accounting
convention, and those cohorts never owned the pool. Adaptive policy has 113 draws,
no FIFO waiting, but different recipients/concentration. No cap stalls arose in
this grid; independent edge controls cover them. No stationary fairness,
optimality, production compatibility or universal affordability claim follows.

## Reproducible evidence

`evidence/local-manifest.json` records 24 source hashes, exact Cargo artifact/profile,
binary hash, environment, timings and all raw digests. The complete capture is
retained at `/tmp/botho-1372-local-capture`; its 18 MB raw history is deliberately
not copied into this commit. Raw SHA256:
`9cbb2e9f1efa5ed177f1951a755454f73df0906bd86f6bad5766fd8aede88365`.
Manifest SHA256:
`a706168e45c4f139c7f7c342d2f8a76f21b808bea78b7a4b89897b8ed029b18a`.
The dedicated workflow uploads full raw evidence, including failures, for 14 days.

Run the collector with a fresh output directory and approved idle target, then
`python3 scripts/research/payout-policy/check.py <capture-directory>`.
The checker verifies archived baseline identity, 24 current source hashes, all
11,232 block recurrences per history, 606 owner records, all receipt accounting and
FIFO summaries. Original source-only equivalence check passes; old test bodies
and historical evidence remain unchanged. Existing Inactive Payout Reinvestment
CI executes the extracted original three-test target and original workload.
