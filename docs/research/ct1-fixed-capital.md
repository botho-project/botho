# Inactive CT1 fixed-capital controls

This is a bounded follow-up to [the first economics checkpoint](ct1-decoy-economics.md),
part of #1306 and implemented by #1318. It changes no production sampler, policy or
protocol. It does not establish stationary profitability, universal Sybil
resistance, realistic calibration, or approval of D3 or the proposed base fee.

## Paired design

The matrix contains 32 cases: initial attacker wealth 2 or 32 BTH, idle or one,
two or sixteen outputs, gross honest fees 0 or 2.5 BTH per block, and seeds 1306
or 902. Each strategy uses the same 100 mature honest tickets, creation height
1000 and block hash schedule (little-endian `seed XOR height` in the first eight
bytes, remaining bytes zero). Two seeds are paired observations, not statistical
confidence. Ticket identifiers 101 onward preserve common identifiers across
strategies; changing output count necessarily changes the candidate population.

The single creation batch pays the candidate gross fee of 0.25 BTH per output
from the fixed budget. Remaining principal is divided equally; any integer
remainder stays in old cash outside the circulation window. The live minimum
eligible principal is 1,000,000 pico. Four 2-BTH/sixteen-output cases cannot pay
creation fees plus minimum principal: they remain explicit unaffordable cases,
with zero charged fee and no state movement. Idle cash earns no ticket reward.
This models the candidate base component for already old funding, not a full
CT transaction or construction-time demurrage on an unspecified source ring.

Every history has **720 warmup blocks plus 512 measured blocks**, 1,232 total.
Attacker outputs are created at height 1000 and first become eligible at 1720.
The first 720 blocks include real creation-fee timing; no ticket is backdated.
All capture is measured after maturity. Original tickets remain unspent, and
payouts are held as cash: they do not become new eligible tickets. Honest
population and fee process are fixed rather than a full ledger equilibrium.
Real fee-paying transactions also create, spend and age honest outputs. This
model deliberately does **not** couple fee flow to that population churn: it
holds 100 honest tickets fixed while injecting external fees. That missing causal
link is a central limitation of the positive marginal observations. Repeated
churn, retirement/recreation and reinvestment are explicitly omitted.

The harness calls production `reward_cap`, `compute_pool_accounting`,
`carryover_after`, `draw_winners`, block reward and emission helpers. Gross fees
split 80% to pool and 20% burn; carryover, cap and integer payout rounding retain
production behavior. A positive funded draw with eligible candidates must return
winners and distribute the proposed payout. Assertions conserve both the pool
accounting and total attacker/honest principal, remaining fee cash, held payouts,
reserve and burn on every block. Honest fees are funded by a separate fixed
initial budget (3,080 BTH when nonzero). Emission is zero at these heights.

Final owner wealth is `principal + old cash + captured payouts`. Gross creation
fees already reduced principal; neither fees nor their burned portion are
subtracted twice. Paired deltas compare this final wealth against idle or one
output with identical initial wealth, fees and seed. Unaffordable rows retain
baseline deltas describing their unchanged holdings, not a funded strategy.

## Observed contrasts

All amounts below are BTH, with gross honest fees 2.5 BTH per block. The two-output
contrast holds at either budget. The sixteen-output contrast is funded only at
32 BTH.

| Outputs | Seed | Creation fee | Measured capture | Final wealth, 32 BTH budget | Delta versus one output |
|---|---:|---:|---:|---:|---:|
| 1 | 1306 | 0.25 | 10.5 | 42.25 | 0 |
| 2 | 1306 | 0.5 | 17.5 | 49 | 6.75 |
| 16 | 1306 | 4 | 140.5 | 168.5 | 126.25 |
| 1 | 902 | 0.25 | 7.5 | 39.25 | 0 |
| 2 | 902 | 0.5 | 21.5 | 53 | 13.75 |
| 16 | 902 | 4 | 127.5 | 155.5 | 116.25 |

With zero honest fees, all attacker capture is zero: the one-time creation fee
is distributed before attacker tickets mature, so final wealth falls by the
charged gross fee. With honest funding, these conditional comparisons show
increased capture for more eligible outputs in this finite fixed population.
They cannot establish an equilibrium profit opportunity: honest funding,
unspent tickets, no reinvestment and output creation costs are explicit modeling
conditions. This evidence does not support accepting split resistance.

The separately reported analytical uniform-ticket capture is approximately
10.138614, 20.078431 and 141.241379 BTH for one, two and sixteen eligible attacker
tickets: 1,024 BTH measured distribution times `n/(100+n)`. After additional gross creation fees, the analytical final-wealth deltas versus
one output are approximately 9.689817 BTH (two outputs) and 127.352765 BTH
(sixteen outputs). These are separately stored alongside sampled deltas. This is
not an observed result or confidence interval, and float formatting is not consensus arithmetic.

## Fixed-input payment sensitivity

The original three production selection routes are reused unchanged. Exact fee
histograms, rather than quantiles, retain every selected observation at proposed
`s=2`, rate 200 bps, two outputs and output factor 1000. Each histogram count must
equal successful selections. Payment is `floor(value * percent / 100)` for
percentages 10, 50 and 90. Affordability requires
`value >= payment + gross_fee + change_floor`, with change floors 0 and 1,000,000
pico; zero floor is an arithmetic sensitivity, not a claim that a zero-valued
change output is valid. Four fixed input values, sixteen scenarios and three
routes yield 1,152 rows. This is not coin selection or wallet dust policy.

All attempted, selected, failed, affordable and unaffordable denominators remain
in the artifact. Failure-only pools have null affordable fraction among successes;
they do not appear fully affordable. Native/CLI have 512 seeded attempts per
scenario; web has one captured deterministic RPC-order selection, not 512 random
draws. Existing sparse-age CLI and eighteen-decoy failures remain in every payment
comparison. No error is converted into a free transaction.

For the background node scenario and 1,000,000-pico change floor, all 512 draws
fund a 10% payment from 1 BTH, but none fund a 50% or 90% payment. The proposed
two-output base alone is 0.5 BTH before the additional charge. Neither 0.0001 nor
0.01 BTH funds any tested payment fraction; 1,000 BTH funds all three in this
background fixture. These thresholds are candidate-policy observations, not
ratification or a realistic transaction-size distribution.

## Reproduction and remaining acceptance

From the repository root:

```sh
CT_ECONOMICS_WRITE_REPORT=1 cargo test --locked -p botho --test ct_economics_simulation -- --nocapture
CT_ECONOMICS_WRITE_REPORT=1 cargo test --locked -p botho --test ct_economics_marginal -- --nocapture
python3 scripts/research/ct-economics/marginal_summary.py
```

`marginal.json` records the 32 histories; `marginal-summary.json` records all
sensitivity rows, raw result hashes, runtime and 21 source/fixture hashes. The
historical first-checkpoint `summary.json` remains historical; the new manifest
explicitly records the test-only histogram addition. CI executes the same checks
and uploads raw artifacts; publication does not imply a Linux pass before that
job completes.

#1306 remains open for realistic pool/distribution calibration, conditional
repeated churn with precise spent-ticket semantics, production wallet payment
workloads and a reviewed D3/base policy decision. The synthetic pool, paired
seeds and finite horizon must not be generalized beyond this checkpoint.
