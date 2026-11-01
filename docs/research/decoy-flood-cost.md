# Decoy-Flood Cost at the Real Fee Floor, with Lottery Recapture (#1701)

**Status:** simulation evidence for #1701 (part of the design-confidence epic
#1683). Nothing here changes consensus, wallets or fees. The pass criterion in
§4 is a **proposal for human ratification**. It is not ratified.

## Verdict

The decoy-flood attack is cheap on today's rules, and in some regimes it pays
for itself. On every grid cell it fails the proposed pass criterion by orders of
magnitude. This is a real threat to ring privacy, not a modelling artefact.

1. **The CT1 docs overstate the attacker's cost by 40,000×.** They price an
   attacker output at the CT1 *candidate* base fee, 2.5e11 pico (0.25 BTH). The
   node charges per *transaction*: `max(consensus floor, MIN_TX_FEE)`. For a
   1-input, 16-output flood transaction the floor is 267,000 pico, so a relayed
   transaction pays 1e8 pico, which is 6.25e6 pico per output. An attacker that
   mines its own transactions pays only the floor, 1.67e4 pico per output.
2. **Uniform flood, any picker.** All three pickers (CLI, web, node) ignore
   ownership, so
   `P(all 19 decoys are the attacker's) ≈ f^19`. Reaching P ≥ 0.5 needs an
   attacker share f ≥ 0.964, i.e. about 27 attacker outputs per honest output.
   At h = 16 honest outputs per block and 5 s blocks, the gross cost is
   **46.7 BTH/day**. Lottery recapture returns 46.65 BTH/day of that in epoch 0,
   so the **net cost is about 0.004 BTH/day**: the flood roughly self-finances.
   An attacker that mines its own transactions **profits about 10.6 BTH/day**.
3. **From epoch 1 the flood is strongly profitable.** The lottery then also
   receives 10% of the block reward (2.5 BTH per block). The flooder wins its
   ticket share of that, a net gain of about **41,650 BTH/day** at 5 s blocks.
   This is the #1682 splitting problem: the Path C cap `ρ·base_fee` counts the
   whole window, so it is about 2,500 BTH and never binds. Any flood is paid for
   by honest holders' lottery share.
4. **Grinding keys no longer helps against the node picker (changed by
   #1786).** As first run, `select_decoys_for_input` broke nearest-target ties
   by candidate order, which is the `utxo_db` key order
   (`tx_hash ‖ output_index`). An attacker that ground its transaction hash
   below every honest one in the block got its outputs taken first: 19 ground
   outputs per block gave P = 1.000 at every age, for any h, at 3.46 BTH/day
   gross (#1771). #1786 shuffles the eligible candidates once per ring before
   selection, so ties are now uniform. Re-run on the port at full chain width
   (§5 of the output, re-run again after #1692), the ground-key P matches the
   uniform-key P within Monte Carlo noise: at h = 1 and a = 19 it is
   0.387 / 0.382 / 0.376 at ages 1,000 / 22,000 / 60,000 (exact uniform-key
   value: 0.378 / 0.377 / 0.377). The minimal flood for P ≥ 0.5 is
   28 / 27 / 28 outputs per block with ground keys and 27 / 27 / 27 with
   uniform keys, against 19 / 8 / 19 before #1786. The node picker is bounded
   by finding 2, like the others. Ground keys still cluster the attacker in
   the lottery's key-order candidate scan, which only lowers its ticket share,
   so the criterion uses uniform keys.
5. **#1692 made the node picker as floodable as the others.** Before #1692
   the node picker clamped gamma targets to the edge of a band centred on the
   real age and took the nearest candidates, so at ages 1,000 and 60,000 a
   flood needed 38 outputs per block (h = 1) for P ≥ 0.5. The random-offset
   window samples its members uniformly, so the node's P is now the same
   hypergeometric value as the CLI and web pickers' (27 outputs per block).
   The edge clamp was not a flood defence worth keeping: it identified the
   real input outright (#1700).

The model holds `MAX_OUTPUTS` = 16 outputs per transaction. Since #1780 the
mempool enforces that (`Transaction::is_valid_structure`, called by
`Mempool::add_tx`), so the `relay` rows are exact. Block validation still
accepts more outputs (#1772 Part B, pending #1684), so an attacker that mines
its own transactions could pack more outputs per transaction than modelled. The
`consensus` rows are therefore upper bounds on that attacker's cost.

## Model

The full method is in the module docs of
[`cluster-tax/src/simulation/decoy_flood_sweep.rs`](../../cluster-tax/src/simulation/decoy_flood_sweep.rs).
In short:

- **Traffic.** Steady state. Each block has `h` honest outputs, in 2-output
  payments that pay the relay minimum, and `a` attacker outputs, in 16-output
  transactions of dust (`min_utxo_value` = 1e6 pico) on background (1×) coins.
  The attacker share is `f = a/(a+h)` in every age band and in the lottery
  window.
- **Fees.** Fees come from `consensus_fee_breakdown`, the #1695 model that a
  golden vector pins to `Ledger::consensus_fee_floor`. The claimed class and
  input-class floor are resolved by the shared ledger mirror
  `cluster_tax::resolve_fee_floor` (#1697) over the flood's untagged outputs
  and ring; both come out at 1×. The `relay` mode adds
  `MIN_TX_FEE`. The `consensus` mode is the bare floor, which is what an
  attacker that mines its own transactions pays. `MIN_TX_FEE` is a mempool
  check only, not a block-validity rule.
- **Lottery.** Each block pays `min(available, ρ_window·LOTTERY_BASE_FEE_PICO,
  block_reward)`. `available` grows by `0.8·fees + emission_share`, where the
  emission share is 0 in epoch 0 and 10% of 25 BTH in epoch 1. In steady state
  the payout equals the inflow unless the inflow exceeds the block reward. The
  Path C cap never binds (#1682), and the tables show the binding term for each
  row. Recapture is ticket share × payout. With uniform keys the ticket share is
  `f`. With ground keys it is lower: the 10,000-candidate scan starts at a
  random keyspace offset, and the attacker's outputs form one clump in that
  keyspace (closed form in `ground_ticket_share`, checked by Monte Carlo).
- **Privacy.** All three pickers use the #1700 production-picker ports in
  `cluster-tax/src/simulation/privacy_routes.rs`, which
  `botho/tests/privacy_sim_route_parity.rs` pins to production.
  - Since #1692 every picker draws one random-offset age window per ring
    (width 20% of the real age, real input uniform inside it) and takes a
    uniform sample of it. Given the window, the probability that all 19
    decoys are the attacker's is exactly hypergeometric over the window's
    `(W + 1)·(a + h) − 1` other outputs, and it does not depend on the
    offset when every block has the same `a` and `h`.
  - CLI (`select_rpc_decoy_pool`): the port shuffles the window, so the
    formula applies directly. A unit test checks the port against it.
  - Web (`selectDecoysForInput`): a partial Fisher–Yates over the window,
    also a uniform sample. A unit test checks the web port against it.
  - Node (`select_decoys_for_input`, with the #1786 shuffle): the first 19
    in-window members of a uniformly shuffled eligible set, again a uniform
    sample. A unit test checks the port against the formula, so the tables
    use the exact value. The ground-key re-run (§5) is Monte Carlo on the
    port at full chain width `a + h`, and a second test runs the literal port
    on the grind case.
- **Constants.** Constants are imported where the crate graph allows. The
  mainnet reward schedule comes from the shared, drift-guarded
  `cluster_tax::monetary::mainnet_policy_mirror` (#1751), and the 80% pool
  share from its `node_constants::FEE_BURN_SHARE_PERMILLE`. `MAX_OUTPUTS`,
  `MAX_LOTTERY_CANDIDATES` and the dust floor are copied, and
  `botho/tests/decoy_flood_parity.rs` pins each copy to production.

**Not modelled:**

- bursty honest traffic;
- the pickers' thin-window fallback (fewer than 19 candidates in the window;
  never reached at these rates);
- adversaries weaker than "all 19 decoys known" (see the #1700 report);
- whether lottery payouts are spendable at once (#1286/#1499).

Since #1692 the node picker no longer uses the gamma spend model to place
decoys, and its blocks-per-day constant comes from the monetary policy block
time (17,280/day at 5 s blocks).

## Pass criterion (PROPOSED, for ratification)

> Sustaining P(all decoys are the attacker's) ≥ 0.5 against any production
> picker, at every real-input age tested (1,000 / 22,000 / 60,000 blocks), must
> cost a **positive** net amount per day of at least **1% of that day's
> block-reward emission**. At 5 s blocks that is 8,640 BTH/day in epoch 0 and
> 4,320 BTH/day in epoch 1.

The yardstick ties the privacy budget to the chain's own security spend. The
1% level is a placeholder for the maintainers to set. Every row fails today;
see §4 of the output below. Most rows also fail the weaker "net cost > 0"
floor.

## Reproduce

From the repository root:

```sh
cargo run --release -p bth-cluster-tax --features cli --bin cluster-tax-sim -- decoy-flood-sweep
# quick grid: ... -- decoy-flood-sweep --smoke
cargo test --release -p bth-cluster-tax --lib decoy_flood
cargo test -p botho --test decoy_flood_parity                  # 3 passed
```

The default run uses seed 1701 and 4,000 node-picker rings per (chain width,
age) for the §5 Monte Carlo. It runs in about a second. The output below is
verbatim.

### Re-run after #1692 (2026-11-01)

#1692 replaced every picker's centred ±10% band with a random-offset window.
§2 and §4 now give the node picker the exact hypergeometric P of the CLI and
web pickers, so its minimal flood fell from 38 / 117 / 440 to
27 / 108 / 431 outputs per block (h = 1 / 4 / 16), and its §4 rows equal the
CLI / web rows. §5 was re-run on the new port. The fee and lottery economics
did not change.

### Re-run after #1786, #1780, #1757 and #1751 (2026-11-01)

The sweep was re-run after these merged to main:

- #1786 shuffled the node picker's tie-break. This changed finding 4: key
  grinding no longer helps (§5 of the output). The node uniform-key P moved
  only by Monte Carlo noise (e.g. h = 1, a = 9, age 22,000: 0.055 → 0.054),
  and every economics and criterion row is unchanged.
- #1780 made `MAX_OUTPUTS` = 16 a mempool rule, so the `relay` rows are exact
  rather than upper bounds. The fee rows are unchanged.
- #1757 added the web picker port (#1735). It has the CLI picker's `P`.
- #1751 (emission model) and #1697/#1742 (fee-floor inputs) are now reused
  instead of copied. Neither changes any number here: epochs 0 and 1 are in
  the halving phase, and flood traffic resolves to factor 1×.

## Output (`decoy-flood-sweep`, seed 1701)

Code constants: MIN_TX_FEE = 100000000 pico; CONSENSUS_FEE_BASE = 1 pico/byte; output penalty cap = 10 outputs; MAX_OUTPUTS = 16 (mempool-enforced since #1780, not by block validation); ring size = 20 (19 decoys); ±10% age band; lottery pool = 800‰ of fees; lottery window = ages 720..=10000 (9281 blocks), dust floor 1000000 pico; MAX_LOTTERY_CANDIDATES = 10000; LOTTERY_BASE_FEE_PICO = 250000000000; reward epoch 0 = 50 BTH, halving every 6307200 blocks; lottery emission share +10%/epoch from epoch 1.

Node-picker rings per (chain width, age) for the §5 ground-key Monte Carlo: 4000; seed 1701.

### 1. Fee per transaction (epoch 0, factor 1×)

| transaction | consensus floor (pico) | relay fee (pico) | relay pico/output | CT1 candidate pico/output |
|---|---:|---:|---:|---:|
| attacker 1-in/16-out, dust outputs | 267000 | 100000000 | 6.250e6 | 2.5e11 |
| honest 1-in/2-out payment | 3960 | 100000000 | 5.000e7 | 2.5e11 |

The CT1 docs' 2.5e11 pico per attacker output is 40000× the relay cost of a 16-output flood output and 14981273× its consensus floor.

### 2. P(all 19 decoys are the attacker's)

`a` = attacker outputs/block, `h` = honest outputs/block, `f = a/(a+h)`. Exact (hypergeometric over the window). Since #1692 all three pickers take a uniform sample of a random-offset age window, so the CLI / web and node columns are the same value (each port is checked against the formula by a unit test). Attacker keys do not matter to any picker (§5). `f^19` is the independent-draw reference.

| h | a | f | f^19 | real age | CLI / web | node |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0.5000 | 1.9e-6 | 1000 | 1.3e-6 | 1.3e-6 |
| 1 | 1 | 0.5000 | 1.9e-6 | 22000 | 1.9e-6 | 1.9e-6 |
| 1 | 1 | 0.5000 | 1.9e-6 | 60000 | 1.9e-6 | 1.9e-6 |
| 1 | 9 | 0.9000 | 0.135 | 1000 | 0.135 | 0.135 |
| 1 | 9 | 0.9000 | 0.135 | 22000 | 0.135 | 0.135 |
| 1 | 9 | 0.9000 | 0.135 | 60000 | 0.135 | 0.135 |
| 1 | 19 | 0.9500 | 0.377 | 1000 | 0.378 | 0.378 |
| 1 | 19 | 0.9500 | 0.377 | 22000 | 0.377 | 0.377 |
| 1 | 19 | 0.9500 | 0.377 | 60000 | 0.377 | 0.377 |
| 1 | 27 | 0.9643 | 0.501 | 1000 | 0.502 | 0.502 |
| 1 | 27 | 0.9643 | 0.501 | 22000 | 0.501 | 0.501 |
| 1 | 27 | 0.9643 | 0.501 | 60000 | 0.501 | 0.501 |
| 1 | 99 | 0.9900 | 0.826 | 1000 | 0.827 | 0.827 |
| 1 | 99 | 0.9900 | 0.826 | 22000 | 0.826 | 0.826 |
| 1 | 99 | 0.9900 | 0.826 | 60000 | 0.826 | 0.826 |
| 4 | 4 | 0.5000 | 1.9e-6 | 1000 | 1.7e-6 | 1.7e-6 |
| 4 | 4 | 0.5000 | 1.9e-6 | 22000 | 1.9e-6 | 1.9e-6 |
| 4 | 4 | 0.5000 | 1.9e-6 | 60000 | 1.9e-6 | 1.9e-6 |
| 4 | 36 | 0.9000 | 0.135 | 1000 | 0.135 | 0.135 |
| 4 | 36 | 0.9000 | 0.135 | 22000 | 0.135 | 0.135 |
| 4 | 36 | 0.9000 | 0.135 | 60000 | 0.135 | 0.135 |
| 4 | 76 | 0.9500 | 0.377 | 1000 | 0.378 | 0.378 |
| 4 | 76 | 0.9500 | 0.377 | 22000 | 0.377 | 0.377 |
| 4 | 76 | 0.9500 | 0.377 | 60000 | 0.377 | 0.377 |
| 4 | 108 | 0.9643 | 0.501 | 1000 | 0.501 | 0.501 |
| 4 | 108 | 0.9643 | 0.501 | 22000 | 0.501 | 0.501 |
| 4 | 108 | 0.9643 | 0.501 | 60000 | 0.501 | 0.501 |
| 4 | 396 | 0.9900 | 0.826 | 1000 | 0.826 | 0.826 |
| 4 | 396 | 0.9900 | 0.826 | 22000 | 0.826 | 0.826 |
| 4 | 396 | 0.9900 | 0.826 | 60000 | 0.826 | 0.826 |
| 16 | 16 | 0.5000 | 1.9e-6 | 1000 | 1.9e-6 | 1.9e-6 |
| 16 | 16 | 0.5000 | 1.9e-6 | 22000 | 1.9e-6 | 1.9e-6 |
| 16 | 16 | 0.5000 | 1.9e-6 | 60000 | 1.9e-6 | 1.9e-6 |
| 16 | 144 | 0.9000 | 0.135 | 1000 | 0.135 | 0.135 |
| 16 | 144 | 0.9000 | 0.135 | 22000 | 0.135 | 0.135 |
| 16 | 144 | 0.9000 | 0.135 | 60000 | 0.135 | 0.135 |
| 16 | 304 | 0.9500 | 0.377 | 1000 | 0.377 | 0.377 |
| 16 | 304 | 0.9500 | 0.377 | 22000 | 0.377 | 0.377 |
| 16 | 304 | 0.9500 | 0.377 | 60000 | 0.377 | 0.377 |
| 16 | 431 | 0.9642 | 0.500 | 1000 | 0.500 | 0.500 |
| 16 | 431 | 0.9642 | 0.500 | 22000 | 0.500 | 0.500 |
| 16 | 431 | 0.9642 | 0.500 | 60000 | 0.500 | 0.500 |
| 16 | 1584 | 0.9900 | 0.826 | 1000 | 0.826 | 0.826 |
| 16 | 1584 | 0.9900 | 0.826 | 22000 | 0.826 | 0.826 |
| 16 | 1584 | 0.9900 | 0.826 | 60000 | 0.826 | 0.826 |

### 3. Per-block economics (uniform keys; BTH per block)

Recapture = ticket share × steady-state payout; payout = min(inflow, ρ·base, block reward). Net < 0 means the flood pays for itself and then some.

| h | a | f | epoch | attacker fee | gross | honest fees | payout | binds | ρ·base cap | recapture | net | recapture/gross |
|---:|---:|---:|---:|---|---:|---:|---:|---|---:|---:|---:|---:|
| 1 | 1 | 0.5000 | 0 | relay | 1.00e-4 | 1.00e-4 | 1.60e-4 | inflow | 2500 | 8.00e-5 | 2.00e-5 | 0.80 |
| 1 | 1 | 0.5000 | 0 | consensus | 8.70e-10 | 1.00e-4 | 8.00e-5 | inflow | 2500 | 4.00e-5 | -4.00e-5 | 4.6e4 |
| 1 | 1 | 0.5000 | 1 | relay | 1.00e-4 | 1.00e-4 | 2.500 | inflow | 2500 | 1.250 | -1.250 | 1.3e4 |
| 1 | 1 | 0.5000 | 1 | consensus | 8.70e-10 | 1.00e-4 | 2.500 | inflow | 2500 | 1.250 | -1.250 | 1.4e9 |
| 1 | 9 | 0.9000 | 0 | relay | 1.00e-4 | 1.00e-4 | 1.60e-4 | inflow | 2500 | 1.44e-4 | -4.40e-5 | 1.44 |
| 1 | 9 | 0.9000 | 0 | consensus | 1.48e-7 | 1.00e-4 | 8.01e-5 | inflow | 2500 | 7.21e-5 | -7.20e-5 | 486.45 |
| 1 | 9 | 0.9000 | 1 | relay | 1.00e-4 | 1.00e-4 | 2.500 | inflow | 2500 | 2.250 | -2.250 | 2.3e4 |
| 1 | 9 | 0.9000 | 1 | consensus | 1.48e-7 | 1.00e-4 | 2.500 | inflow | 2500 | 2.250 | -2.250 | 1.5e7 |
| 1 | 19 | 0.9500 | 0 | relay | 2.00e-4 | 1.00e-4 | 2.40e-4 | inflow | 2500 | 2.28e-4 | -2.80e-5 | 1.14 |
| 1 | 19 | 0.9500 | 0 | consensus | 2.77e-7 | 1.00e-4 | 8.02e-5 | inflow | 2500 | 7.62e-5 | -7.59e-5 | 275.14 |
| 1 | 19 | 0.9500 | 1 | relay | 2.00e-4 | 1.00e-4 | 2.500 | inflow | 2500 | 2.375 | -2.375 | 1.2e4 |
| 1 | 19 | 0.9500 | 1 | consensus | 2.77e-7 | 1.00e-4 | 2.500 | inflow | 2500 | 2.375 | -2.375 | 8.6e6 |
| 1 | 27 | 0.9643 | 0 | relay | 2.00e-4 | 1.00e-4 | 2.40e-4 | inflow | 2500 | 2.31e-4 | -3.14e-5 | 1.16 |
| 1 | 27 | 0.9643 | 0 | consensus | 4.74e-7 | 1.00e-4 | 8.04e-5 | inflow | 2500 | 7.75e-5 | -7.70e-5 | 163.52 |
| 1 | 27 | 0.9643 | 1 | relay | 2.00e-4 | 1.00e-4 | 2.500 | inflow | 2500 | 2.411 | -2.411 | 1.2e4 |
| 1 | 27 | 0.9643 | 1 | consensus | 4.74e-7 | 1.00e-4 | 2.500 | inflow | 2500 | 2.411 | -2.411 | 5.1e6 |
| 1 | 99 | 0.9900 | 0 | relay | 7.00e-4 | 1.00e-4 | 6.40e-4 | inflow | 2500 | 6.34e-4 | 6.64e-5 | 0.91 |
| 1 | 99 | 0.9900 | 0 | consensus | 1.61e-6 | 1.00e-4 | 8.13e-5 | inflow | 2500 | 8.05e-5 | -7.89e-5 | 49.92 |
| 1 | 99 | 0.9900 | 1 | relay | 7.00e-4 | 1.00e-4 | 2.501 | inflow | 2500 | 2.476 | -2.475 | 3.5e3 |
| 1 | 99 | 0.9900 | 1 | consensus | 1.61e-6 | 1.00e-4 | 2.500 | inflow | 2500 | 2.475 | -2.475 | 1.5e6 |
| 4 | 4 | 0.5000 | 0 | relay | 1.00e-4 | 2.00e-4 | 2.40e-4 | inflow | 2500 | 1.20e-4 | -2.00e-5 | 1.20 |
| 4 | 4 | 0.5000 | 0 | consensus | 1.97e-8 | 2.00e-4 | 1.60e-4 | inflow | 2500 | 8.00e-5 | -8.00e-5 | 4.1e3 |
| 4 | 4 | 0.5000 | 1 | relay | 1.00e-4 | 2.00e-4 | 2.500 | inflow | 2500 | 1.250 | -1.250 | 1.3e4 |
| 4 | 4 | 0.5000 | 1 | consensus | 1.97e-8 | 2.00e-4 | 2.500 | inflow | 2500 | 1.250 | -1.250 | 6.4e7 |
| 4 | 36 | 0.9000 | 0 | relay | 3.00e-4 | 2.00e-4 | 4.00e-4 | inflow | 2500 | 3.60e-4 | -6.00e-5 | 1.20 |
| 4 | 36 | 0.9000 | 0 | consensus | 5.54e-7 | 2.00e-4 | 1.60e-4 | inflow | 2500 | 1.44e-4 | -1.44e-4 | 260.80 |
| 4 | 36 | 0.9000 | 1 | relay | 3.00e-4 | 2.00e-4 | 2.500 | inflow | 2500 | 2.250 | -2.250 | 7.5e3 |
| 4 | 36 | 0.9000 | 1 | consensus | 5.54e-7 | 2.00e-4 | 2.500 | inflow | 2500 | 2.250 | -2.250 | 4.1e6 |
| 4 | 76 | 0.9500 | 0 | relay | 5.00e-4 | 2.00e-4 | 5.60e-4 | inflow | 2500 | 5.32e-4 | -3.20e-5 | 1.06 |
| 4 | 76 | 0.9500 | 0 | consensus | 1.29e-6 | 2.00e-4 | 1.61e-4 | inflow | 2500 | 1.53e-4 | -1.52e-4 | 118.86 |
| 4 | 76 | 0.9500 | 1 | relay | 5.00e-4 | 2.00e-4 | 2.501 | inflow | 2500 | 2.376 | -2.375 | 4.8e3 |
| 4 | 76 | 0.9500 | 1 | consensus | 1.29e-6 | 2.00e-4 | 2.500 | inflow | 2500 | 2.375 | -2.375 | 1.8e6 |
| 4 | 108 | 0.9643 | 0 | relay | 7.00e-4 | 2.00e-4 | 7.20e-4 | inflow | 2500 | 6.94e-4 | 5.71e-6 | 0.99 |
| 4 | 108 | 0.9643 | 0 | consensus | 1.82e-6 | 2.00e-4 | 1.61e-4 | inflow | 2500 | 1.56e-4 | -1.54e-4 | 85.50 |
| 4 | 108 | 0.9643 | 1 | relay | 7.00e-4 | 2.00e-4 | 2.501 | inflow | 2500 | 2.411 | -2.411 | 3.4e3 |
| 4 | 108 | 0.9643 | 1 | consensus | 1.82e-6 | 2.00e-4 | 2.500 | inflow | 2500 | 2.411 | -2.411 | 1.3e6 |
| 4 | 396 | 0.9900 | 0 | relay | 2.50e-3 | 2.00e-4 | 2.16e-3 | inflow | 2500 | 2.14e-3 | 3.62e-4 | 0.86 |
| 4 | 396 | 0.9900 | 0 | consensus | 6.63e-6 | 2.00e-4 | 1.65e-4 | inflow | 2500 | 1.64e-4 | -1.57e-4 | 24.69 |
| 4 | 396 | 0.9900 | 1 | relay | 2.50e-3 | 2.00e-4 | 2.502 | inflow | 2500 | 2.477 | -2.475 | 990.86 |
| 4 | 396 | 0.9900 | 1 | consensus | 6.63e-6 | 2.00e-4 | 2.500 | inflow | 2500 | 2.475 | -2.475 | 3.7e5 |
| 16 | 16 | 0.5000 | 0 | relay | 1.00e-4 | 8.00e-4 | 7.20e-4 | inflow | 2500 | 3.60e-4 | -2.60e-4 | 3.60 |
| 16 | 16 | 0.5000 | 0 | consensus | 2.67e-7 | 8.00e-4 | 6.40e-4 | inflow | 2500 | 3.20e-4 | -3.20e-4 | 1.2e3 |
| 16 | 16 | 0.5000 | 1 | relay | 1.00e-4 | 8.00e-4 | 2.501 | inflow | 2500 | 1.250 | -1.250 | 1.3e4 |
| 16 | 16 | 0.5000 | 1 | consensus | 2.67e-7 | 8.00e-4 | 2.501 | inflow | 2500 | 1.250 | -1.250 | 4.7e6 |
| 16 | 144 | 0.9000 | 0 | relay | 9.00e-4 | 8.00e-4 | 1.36e-3 | inflow | 2500 | 1.22e-3 | -3.24e-4 | 1.36 |
| 16 | 144 | 0.9000 | 0 | consensus | 2.40e-6 | 8.00e-4 | 6.42e-4 | inflow | 2500 | 5.78e-4 | -5.75e-4 | 240.42 |
| 16 | 144 | 0.9000 | 1 | relay | 9.00e-4 | 8.00e-4 | 2.501 | inflow | 2500 | 2.251 | -2.250 | 2.5e3 |
| 16 | 144 | 0.9000 | 1 | consensus | 2.40e-6 | 8.00e-4 | 2.501 | inflow | 2500 | 2.251 | -2.251 | 9.4e5 |
| 16 | 304 | 0.9500 | 0 | relay | 1.90e-3 | 8.00e-4 | 2.16e-3 | inflow | 2500 | 2.05e-3 | -1.52e-4 | 1.08 |
| 16 | 304 | 0.9500 | 0 | consensus | 5.07e-6 | 8.00e-4 | 6.44e-4 | inflow | 2500 | 6.12e-4 | -6.07e-4 | 120.61 |
| 16 | 304 | 0.9500 | 1 | relay | 1.90e-3 | 8.00e-4 | 2.502 | inflow | 2500 | 2.377 | -2.375 | 1.3e3 |
| 16 | 304 | 0.9500 | 1 | consensus | 5.07e-6 | 8.00e-4 | 2.501 | inflow | 2500 | 2.376 | -2.376 | 4.7e5 |
| 16 | 431 | 0.9642 | 0 | relay | 2.70e-3 | 8.00e-4 | 2.80e-3 | inflow | 2500 | 2.70e-3 | 2.24e-7 | 1.00 |
| 16 | 431 | 0.9642 | 0 | consensus | 7.20e-6 | 8.00e-4 | 6.46e-4 | inflow | 2500 | 6.23e-4 | -6.15e-4 | 86.51 |
| 16 | 431 | 0.9642 | 1 | relay | 2.70e-3 | 8.00e-4 | 2.503 | inflow | 2500 | 2.413 | -2.411 | 893.78 |
| 16 | 431 | 0.9642 | 1 | consensus | 7.20e-6 | 8.00e-4 | 2.501 | inflow | 2500 | 2.411 | -2.411 | 3.4e5 |
| 16 | 1584 | 0.9900 | 0 | relay | 9.90e-3 | 8.00e-4 | 8.56e-3 | inflow | 2500 | 8.47e-3 | 1.43e-3 | 0.86 |
| 16 | 1584 | 0.9900 | 0 | consensus | 2.64e-5 | 8.00e-4 | 6.61e-4 | inflow | 2500 | 6.55e-4 | -6.28e-4 | 24.76 |
| 16 | 1584 | 0.9900 | 1 | relay | 9.90e-3 | 8.00e-4 | 2.509 | inflow | 2500 | 2.483 | -2.474 | 250.86 |
| 16 | 1584 | 0.9900 | 1 | consensus | 2.64e-5 | 8.00e-4 | 2.501 | inflow | 2500 | 2.476 | -2.476 | 9.4e4 |

### 4. Pass criterion (PROPOSED, not ratified)

Proposed: sustaining P ≥ 0.5 at every reported age ([1000, 22000, 60000]) must cost a **positive** net amount per day of at least 1% of that day's block-reward emission. Minimal flood `a_min` found by search; costs per day in BTH; threshold in brackets.

| picker | keys | h | a_min | attacker tx/block | epoch | attacker fee | gross/day @5s | recapture/day @5s | net/day @5s [thr] | net/day @20s [thr] | net/day @60s [thr] | verdict |
|---|---|---:|---:|---:|---:|---|---:|---:|---:|---:|---:|---|
| CLI / web | uniform keys | 1 | 27 | 2 | 0 | relay | 3.456 | 3.999 | -0.543 [8640] | -0.136 [2160] | -0.045 [720] | FAIL |
| CLI / web | uniform keys | 1 | 27 | 2 | 0 | consensus | 8.19e-3 | 1.339 | -1.331 [8640] | -0.333 [2160] | -0.111 [720] | FAIL |
| CLI / web | uniform keys | 1 | 27 | 2 | 1 | relay | 3.456 | 41661 | -41658 [4320] | -10414 [1080] | -3471 [360] | FAIL |
| CLI / web | uniform keys | 1 | 27 | 2 | 1 | consensus | 8.19e-3 | 41658 | -41658 [4320] | -10415 [1080] | -3472 [360] | FAIL |
| CLI / web | uniform keys | 4 | 108 | 7 | 0 | relay | 12.096 | 11.997 | 0.099 [8640] | 0.025 [2160] | 8.23e-3 [720] | FAIL |
| CLI / web | uniform keys | 4 | 108 | 7 | 0 | consensus | 0.031 | 2.690 | -2.659 [8640] | -0.665 [2160] | -0.222 [720] | FAIL |
| CLI / web | uniform keys | 4 | 108 | 7 | 1 | relay | 12.096 | 41669 | -41657 [4320] | -10414 [1080] | -3471 [360] | FAIL |
| CLI / web | uniform keys | 4 | 108 | 7 | 1 | consensus | 0.031 | 41660 | -41660 [4320] | -10415 [1080] | -3472 [360] | FAIL |
| CLI / web | uniform keys | 16 | 431 | 27 | 0 | relay | 46.656 | 46.652 | 3.87e-3 [8640] | 9.66e-4 [2160] | 3.22e-4 [720] | FAIL |
| CLI / web | uniform keys | 16 | 431 | 27 | 0 | consensus | 0.124 | 10.759 | -10.635 [8640] | -2.659 [2160] | -0.886 [720] | FAIL |
| CLI / web | uniform keys | 16 | 431 | 27 | 1 | relay | 46.656 | 41700 | -41654 [4320] | -10413 [1080] | -3471 [360] | FAIL |
| CLI / web | uniform keys | 16 | 431 | 27 | 1 | consensus | 0.124 | 41664 | -41664 [4320] | -10416 [1080] | -3472 [360] | FAIL |
| node | uniform keys | 1 | 27 | 2 | 0 | relay | 3.456 | 3.999 | -0.543 [8640] | -0.136 [2160] | -0.045 [720] | FAIL |
| node | uniform keys | 1 | 27 | 2 | 0 | consensus | 8.19e-3 | 1.339 | -1.331 [8640] | -0.333 [2160] | -0.111 [720] | FAIL |
| node | uniform keys | 1 | 27 | 2 | 1 | relay | 3.456 | 41661 | -41658 [4320] | -10414 [1080] | -3471 [360] | FAIL |
| node | uniform keys | 1 | 27 | 2 | 1 | consensus | 8.19e-3 | 41658 | -41658 [4320] | -10415 [1080] | -3472 [360] | FAIL |
| node | uniform keys | 4 | 108 | 7 | 0 | relay | 12.096 | 11.997 | 0.099 [8640] | 0.025 [2160] | 8.23e-3 [720] | FAIL |
| node | uniform keys | 4 | 108 | 7 | 0 | consensus | 0.031 | 2.690 | -2.659 [8640] | -0.665 [2160] | -0.222 [720] | FAIL |
| node | uniform keys | 4 | 108 | 7 | 1 | relay | 12.096 | 41669 | -41657 [4320] | -10414 [1080] | -3471 [360] | FAIL |
| node | uniform keys | 4 | 108 | 7 | 1 | consensus | 0.031 | 41660 | -41660 [4320] | -10415 [1080] | -3472 [360] | FAIL |
| node | uniform keys | 16 | 431 | 27 | 0 | relay | 46.656 | 46.652 | 3.87e-3 [8640] | 9.66e-4 [2160] | 3.22e-4 [720] | FAIL |
| node | uniform keys | 16 | 431 | 27 | 0 | consensus | 0.124 | 10.759 | -10.635 [8640] | -2.659 [2160] | -0.886 [720] | FAIL |
| node | uniform keys | 16 | 431 | 27 | 1 | relay | 46.656 | 41700 | -41654 [4320] | -10413 [1080] | -3471 [360] | FAIL |
| node | uniform keys | 16 | 431 | 27 | 1 | consensus | 0.124 | 41664 | -41664 [4320] | -10416 [1080] | -3472 [360] | FAIL |

### 5. Node picker with ground keys: the #1771 grind case after #1786

The node port at full chain width (a + h outputs per block), with the attacker's outputs sorted first in every block, against the uniform-key value at the same rates. Before #1786 the port took each block's lowest key first and the ground column was 1.000 for a ≥ 19 at every age. Monte Carlo, 4000 rings per cell.

| h | a | real age | node, ground keys | node, uniform keys |
|---:|---:|---:|---:|---:|
| 1 | 8 | 1000 | 0.104 | 0.107 |
| 1 | 19 | 1000 | 0.387 | 0.378 |
| 1 | 27 | 1000 | 0.499 | 0.502 |
| 1 | 38 | 1000 | 0.616 | 0.612 |
| 1 | 8 | 22000 | 0.105 | 0.107 |
| 1 | 19 | 22000 | 0.382 | 0.377 |
| 1 | 27 | 22000 | 0.503 | 0.501 |
| 1 | 38 | 22000 | 0.609 | 0.611 |
| 1 | 8 | 60000 | 0.108 | 0.107 |
| 1 | 19 | 60000 | 0.376 | 0.377 |
| 1 | 27 | 60000 | 0.493 | 0.501 |
| 1 | 38 | 60000 | 0.603 | 0.610 |

Minimal attacker outputs per block for P ≥ 0.5 (ground keys searched up to a + h = 256; pre-#1786 ground-key minima were 19 / 8 / 19 at ages 1000 / 22000 / 60000).

| h | real age | a_min, ground keys | a_min, uniform keys |
|---:|---:|---:|---:|
| 1 | 1000 | 28 | 27 |
| 1 | 22000 | 27 | 27 |
| 1 | 60000 | 28 | 27 |
