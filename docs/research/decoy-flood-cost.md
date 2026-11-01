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
2. **Uniform flood, either picker.** Both pickers ignore ownership, so
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
4. **The node picker can be beaten with 19 outputs per block, whatever the
   honest traffic.** `select_decoys_for_input` takes the nearest candidate to
   each gamma target and breaks ties by candidate order. Production candidate
   order is the `utxo_db` key order, `tx_hash ‖ output_index`. The transaction
   hash excludes signatures, so the attacker can grind it cheaply until its
   flood transaction sorts below every honest transaction in the same block.
   Every slot then takes the attacker's output first. With 19 ground outputs per
   block (2 transactions), P = 1.000 at every age tested, for any h. Mid-band
   ages need only 8. The cost is **3.46 BTH/day gross** at 5 s blocks under
   relay fees, 0.005 BTH/day for a mining attacker, and negative from epoch 1.
   Filed as #1771.

The model holds `MAX_OUTPUTS` = 16 outputs per transaction. That constant
(`bth_transaction_types::constants::MAX_OUTPUTS`) is **not enforced** by the
node's mempool (`Transaction::is_valid_structure`) or by consensus
(`validate_transfer_tx`). The real per-transaction limit is the 100 KiB gossip
message cap (#1772). So the costs above are upper bounds.

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
  golden vector pins to `Ledger::consensus_fee_floor`. The `relay` mode adds
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
- **Privacy.** Both pickers use the #1700 production-picker ports, which
  `botho/tests/privacy_sim_route_parity.rs` pins to production.
  - CLI (`select_rpc_decoy_pool`): the port does a full shuffle of the ±10%
    band, so the probability is exactly hypergeometric. A unit test checks the
    port against the formula.
  - Node (`select_decoys_for_input`): the port runs by Monte Carlo on a chain
    with `min(a+h, 20)` outputs per block. A unit test shows that this
    reduction is exact.
- **Constants.** Constants are imported where the crate graph allows.
  `MAX_OUTPUTS`, the 80% pool share, `MAX_LOTTERY_CANDIDATES`, the mainnet
  reward schedule and the dust floor are copied, and
  `botho/tests/decoy_flood_parity.rs` pins each copy to production.

**Not modelled:**

- bursty honest traffic;
- the web wallet route (chain-order picker, #1700/#1735);
- adversaries weaker than "all 19 decoys known" (see the #1700 report);
- whether lottery payouts are spendable at once (#1286/#1499).

The node picker's 720-blocks-per-day gamma assumption is used as shipped
(#1692).

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
cargo test -p botho --test decoy_flood_parity
```

The default run uses seed 1701 and 4,000 node-picker rings per (chain width,
age). It runs in a few seconds. The output below is verbatim.

## Output (`decoy-flood-sweep`, seed 1701)

Code constants: MIN_TX_FEE = 100000000 pico; CONSENSUS_FEE_BASE = 1 pico/byte; output penalty cap = 10 outputs; MAX_OUTPUTS = 16; ring size = 20 (19 decoys); ±10% age band; lottery pool = 800‰ of fees; lottery window = ages 720..=10000 (9281 blocks), dust floor 1000000 pico; MAX_LOTTERY_CANDIDATES = 10000; LOTTERY_BASE_FEE_PICO = 250000000000; reward epoch 0 = 50 BTH, halving every 6307200 blocks; lottery emission share +10%/epoch from epoch 1.

Node-picker rings per (chain width, age): 4000; seed 1701.

### 1. Fee per transaction (epoch 0, factor 1×)

| transaction | consensus floor (pico) | relay fee (pico) | relay pico/output | CT1 candidate pico/output |
|---|---:|---:|---:|---:|
| attacker 1-in/16-out, dust outputs | 267000 | 100000000 | 6.250e6 | 2.5e11 |
| honest 1-in/2-out payment | 3960 | 100000000 | 5.000e7 | 2.5e11 |

The CT1 docs' 2.5e11 pico per attacker output is 40000× the relay cost of a 16-output flood output and 14981273× its consensus floor.

### 2. P(all 19 decoys are the attacker's)

`a` = attacker outputs/block, `h` = honest outputs/block, `f = a/(a+h)`. CLI is exact (hypergeometric over the band); node is the #1700 port, Monte Carlo. `f^19` is the independent-draw reference.

| h | a | f | f^19 | real age | CLI | node (uniform keys) | node (ground keys) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0.5000 | 1.9e-6 | 1000 | 1.3e-6 | 0 | 0 |
| 1 | 1 | 0.5000 | 1.9e-6 | 22000 | 1.9e-6 | 0 | 0 |
| 1 | 1 | 0.5000 | 1.9e-6 | 60000 | 1.9e-6 | 0 | 0 |
| 1 | 9 | 0.9000 | 0.135 | 1000 | 0.135 | 0 | 0 |
| 1 | 9 | 0.9000 | 0.135 | 22000 | 0.135 | 0.055 | 0.876 |
| 1 | 9 | 0.9000 | 0.135 | 60000 | 0.135 | 0 | 0 |
| 1 | 19 | 0.9500 | 0.377 | 1000 | 0.378 | 0.050 | 1.000 |
| 1 | 19 | 0.9500 | 0.377 | 22000 | 0.377 | 0.331 | 1.000 |
| 1 | 19 | 0.9500 | 0.377 | 60000 | 0.377 | 0.050 | 1.000 |
| 1 | 27 | 0.9643 | 0.501 | 1000 | 0.502 | 0.321 | 1.000 |
| 1 | 27 | 0.9643 | 0.501 | 22000 | 0.501 | 0.472 | 1.000 |
| 1 | 27 | 0.9643 | 0.501 | 60000 | 0.501 | 0.321 | 1.000 |
| 1 | 99 | 0.9900 | 0.826 | 1000 | 0.827 | 0.810 | 1.000 |
| 1 | 99 | 0.9900 | 0.826 | 22000 | 0.826 | 0.823 | 1.000 |
| 1 | 99 | 0.9900 | 0.826 | 60000 | 0.826 | 0.810 | 1.000 |
| 4 | 4 | 0.5000 | 1.9e-6 | 1000 | 1.7e-6 | 0 | 0 |
| 4 | 4 | 0.5000 | 1.9e-6 | 22000 | 1.9e-6 | 1.8e-9 | 0.008 |
| 4 | 4 | 0.5000 | 1.9e-6 | 60000 | 1.9e-6 | 0 | 0 |
| 4 | 36 | 0.9000 | 0.135 | 1000 | 0.135 | 0.065 | 1.000 |
| 4 | 36 | 0.9000 | 0.135 | 22000 | 0.135 | 0.120 | 1.000 |
| 4 | 36 | 0.9000 | 0.135 | 60000 | 0.135 | 0.065 | 1.000 |
| 4 | 76 | 0.9500 | 0.377 | 1000 | 0.378 | 0.330 | 1.000 |
| 4 | 76 | 0.9500 | 0.377 | 22000 | 0.377 | 0.368 | 1.000 |
| 4 | 76 | 0.9500 | 0.377 | 60000 | 0.377 | 0.330 | 1.000 |
| 4 | 108 | 0.9643 | 0.501 | 1000 | 0.501 | 0.470 | 1.000 |
| 4 | 108 | 0.9643 | 0.501 | 22000 | 0.501 | 0.495 | 1.000 |
| 4 | 108 | 0.9643 | 0.501 | 60000 | 0.501 | 0.470 | 1.000 |
| 4 | 396 | 0.9900 | 0.826 | 1000 | 0.826 | 0.822 | 1.000 |
| 4 | 396 | 0.9900 | 0.826 | 22000 | 0.826 | 0.825 | 1.000 |
| 4 | 396 | 0.9900 | 0.826 | 60000 | 0.826 | 0.822 | 1.000 |
| 16 | 16 | 0.5000 | 1.9e-6 | 1000 | 1.9e-6 | 0 | 0 |
| 16 | 16 | 0.5000 | 1.9e-6 | 22000 | 1.9e-6 | 4.9e-7 | 1.000 |
| 16 | 16 | 0.5000 | 1.9e-6 | 60000 | 1.9e-6 | 0 | 0 |
| 16 | 144 | 0.9000 | 0.135 | 1000 | 0.135 | 0.119 | 1.000 |
| 16 | 144 | 0.9000 | 0.135 | 22000 | 0.135 | 0.132 | 1.000 |
| 16 | 144 | 0.9000 | 0.135 | 60000 | 0.135 | 0.119 | 1.000 |
| 16 | 304 | 0.9500 | 0.377 | 1000 | 0.377 | 0.366 | 1.000 |
| 16 | 304 | 0.9500 | 0.377 | 22000 | 0.377 | 0.375 | 1.000 |
| 16 | 304 | 0.9500 | 0.377 | 60000 | 0.377 | 0.366 | 1.000 |
| 16 | 431 | 0.9642 | 0.500 | 1000 | 0.500 | 0.493 | 1.000 |
| 16 | 431 | 0.9642 | 0.500 | 22000 | 0.500 | 0.499 | 1.000 |
| 16 | 431 | 0.9642 | 0.500 | 60000 | 0.500 | 0.493 | 1.000 |
| 16 | 1584 | 0.9900 | 0.826 | 1000 | 0.826 | 0.825 | 1.000 |
| 16 | 1584 | 0.9900 | 0.826 | 22000 | 0.826 | 0.826 | 1.000 |
| 16 | 1584 | 0.9900 | 0.826 | 60000 | 0.826 | 0.825 | 1.000 |

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
| CLI | uniform keys | 1 | 27 | 2 | 0 | relay | 3.456 | 3.999 | -0.543 [8640] | -0.136 [2160] | -0.045 [720] | FAIL |
| CLI | uniform keys | 1 | 27 | 2 | 0 | consensus | 8.19e-3 | 1.339 | -1.331 [8640] | -0.333 [2160] | -0.111 [720] | FAIL |
| CLI | uniform keys | 1 | 27 | 2 | 1 | relay | 3.456 | 41661 | -41658 [4320] | -10414 [1080] | -3471 [360] | FAIL |
| CLI | uniform keys | 1 | 27 | 2 | 1 | consensus | 8.19e-3 | 41658 | -41658 [4320] | -10415 [1080] | -3472 [360] | FAIL |
| CLI | uniform keys | 4 | 108 | 7 | 0 | relay | 12.096 | 11.997 | 0.099 [8640] | 0.025 [2160] | 8.23e-3 [720] | FAIL |
| CLI | uniform keys | 4 | 108 | 7 | 0 | consensus | 0.031 | 2.690 | -2.659 [8640] | -0.665 [2160] | -0.222 [720] | FAIL |
| CLI | uniform keys | 4 | 108 | 7 | 1 | relay | 12.096 | 41669 | -41657 [4320] | -10414 [1080] | -3471 [360] | FAIL |
| CLI | uniform keys | 4 | 108 | 7 | 1 | consensus | 0.031 | 41660 | -41660 [4320] | -10415 [1080] | -3472 [360] | FAIL |
| CLI | uniform keys | 16 | 431 | 27 | 0 | relay | 46.656 | 46.652 | 3.87e-3 [8640] | 9.66e-4 [2160] | 3.22e-4 [720] | FAIL |
| CLI | uniform keys | 16 | 431 | 27 | 0 | consensus | 0.124 | 10.759 | -10.635 [8640] | -2.659 [2160] | -0.886 [720] | FAIL |
| CLI | uniform keys | 16 | 431 | 27 | 1 | relay | 46.656 | 41700 | -41654 [4320] | -10413 [1080] | -3471 [360] | FAIL |
| CLI | uniform keys | 16 | 431 | 27 | 1 | consensus | 0.124 | 41664 | -41664 [4320] | -10416 [1080] | -3472 [360] | FAIL |
| node | uniform keys | 1 | 38 | 3 | 0 | relay | 5.184 | 5.388 | -0.204 [8640] | -0.051 [2160] | -0.017 [720] | FAIL |
| node | uniform keys | 1 | 38 | 3 | 0 | consensus | 0.010 | 1.355 | -1.345 [8640] | -0.336 [2160] | -0.112 [720] | FAIL |
| node | uniform keys | 1 | 38 | 3 | 1 | relay | 5.184 | 42098 | -42093 [4320] | -10523 [1080] | -3508 [360] | FAIL |
| node | uniform keys | 1 | 38 | 3 | 1 | consensus | 0.010 | 42094 | -42094 [4320] | -10523 [1080] | -3508 [360] | FAIL |
| node | uniform keys | 4 | 117 | 8 | 0 | relay | 13.824 | 13.367 | 0.457 [8640] | 0.114 [2160] | 0.038 [720] | FAIL |
| node | uniform keys | 4 | 117 | 8 | 0 | consensus | 0.033 | 2.699 | -2.666 [8640] | -0.666 [2160] | -0.222 [720] | FAIL |
| node | uniform keys | 4 | 117 | 8 | 1 | relay | 13.824 | 41785 | -41771 [4320] | -10443 [1080] | -3481 [360] | FAIL |
| node | uniform keys | 4 | 117 | 8 | 1 | consensus | 0.033 | 41775 | -41775 [4320] | -10444 [1080] | -3481 [360] | FAIL |
| node | uniform keys | 16 | 440 | 28 | 0 | relay | 48.384 | 48.020 | 0.364 [8640] | 0.091 [2160] | 0.030 [720] | FAIL |
| node | uniform keys | 16 | 440 | 28 | 0 | consensus | 0.126 | 10.769 | -10.642 [8640] | -2.661 [2160] | -0.887 [720] | FAIL |
| node | uniform keys | 16 | 440 | 28 | 1 | relay | 48.384 | 41732 | -41684 [4320] | -10421 [1080] | -3474 [360] | FAIL |
| node | uniform keys | 16 | 440 | 28 | 1 | consensus | 0.126 | 41695 | -41695 [4320] | -10424 [1080] | -3475 [360] | FAIL |
| node | ground keys | 1 | 19 | 2 | 0 | relay | 3.456 | 2.223 | 1.233 [8640] | 0.308 [2160] | 0.103 [720] | FAIL |
| node | ground keys | 1 | 19 | 2 | 0 | consensus | 4.79e-3 | 0.743 | -0.738 [8640] | -0.185 [2160] | -0.062 [720] | FAIL |
| node | ground keys | 1 | 19 | 2 | 1 | relay | 3.456 | 23155 | -23152 [4320] | -5788 [1080] | -1929 [360] | FAIL |
| node | ground keys | 1 | 19 | 2 | 1 | consensus | 4.79e-3 | 23154 | -23154 [4320] | -5788 [1080] | -1929 [360] | FAIL |
| node | ground keys | 4 | 19 | 2 | 0 | relay | 3.456 | 0.745 | 2.711 [8640] | 0.678 [2160] | 0.226 [720] | FAIL |
| node | ground keys | 4 | 19 | 2 | 0 | consensus | 4.79e-3 | 0.373 | -0.368 [8640] | -0.092 [2160] | -0.031 [720] | FAIL |
| node | ground keys | 4 | 19 | 2 | 1 | relay | 3.456 | 5819 | -5816 [4320] | -1454 [1080] | -485 [360] | FAIL |
| node | ground keys | 4 | 19 | 2 | 1 | consensus | 4.79e-3 | 5819 | -5819 [4320] | -1455 [1080] | -485 [360] | FAIL |
| node | ground keys | 16 | 19 | 2 | 0 | relay | 3.456 | 0.465 | 2.991 [8640] | 0.748 [2160] | 0.249 [720] | FAIL |
| node | ground keys | 16 | 19 | 2 | 0 | consensus | 4.79e-3 | 0.373 | -0.368 [8640] | -0.092 [2160] | -0.031 [720] | FAIL |
| node | ground keys | 16 | 19 | 2 | 1 | relay | 3.456 | 1455 | -1452 [4320] | -363 [1080] | -121 [360] | FAIL |
| node | ground keys | 16 | 19 | 2 | 1 | consensus | 4.79e-3 | 1455 | -1455 [4320] | -364 [1080] | -121 [360] | FAIL |

### 5. Node picker with ground keys: minimal flood by real age

Attacker outputs per block needed for P ≥ 0.5 when its outputs sort first in every block (independent of `h` once a + h ≥ 20).

| real age | a_min |
|---:|---:|
| 1000 | 19 |
| 22000 | 8 |
| 60000 | 19 |

