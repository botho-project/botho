//! Golden fee floors for the x-series stress payment shapes (#2023).
//!
//! Protocol 10.0.0 prices the class-exit charge on the whole value a spend
//! moves (#1685). The stress harness's offline plan check
//! (`scripts/stress/payment_shape.py`, run by `plan.py`) must price its
//! planned payment and prefund-batch shapes with the real fee function, not a
//! Python copy of it. This test is that function's output, pinned:
//! [`consensus_fee_breakdown`] (pinned to `Ledger::consensus_fee_floor` by the
//! `botho` golden vectors, #1695), the relay floor at the dynamic base's
//! ceiling (`FeeConfig::minimum_fee_dynamic_with_outputs`, the size term
//! `Ledger::fee_floor_at_base` prices for RPC `estimateFee`, #2001), and the
//! wallet re-sign fee [`resign_fee`] the stress signer pays.
//!
//! Each row is a shape (inputs, outputs, value moved) at a cluster wealth `W`
//! and an exit depth:
//!
//! - `hop`: one wallet hop, the ring's class at `W` and the outputs declaring
//!   `W` less the wallet's 5% tag decay (`WALLET_CLUSTER_DECAY_RATE`);
//! - `exit`: the outputs declaring background (factor 1x) against a ring at
//!   `W`, the largest downgrade charge the ring can levy;
//! - `ring` (prefund-batch shapes only): the outputs declare one wallet hop
//!   from a fresh coinbase cluster (one coinbase less the 5% decay) while the
//!   ring-floor wealth sits at `W`. The floor takes each input's maximum
//!   effective wealth among members matching its pseudo amount (all members if
//!   none match), then a pseudo-amount-weighted mean across inputs before
//!   applying the curve (`amount_matched_ring_floor_factor`). The prefunder's
//!   bound on every member also bounds this mean; `payment_shape.py` gates the
//!   downgrade at that bound (`prefund.MAX_FUNDING_DEPTH`).
//!
//! The height is in epoch 0 (in-class demurrage 0, exit pricing on), where an
//! x-series chain lives. Regenerate after a deliberate fee-function change:
//!
//! ```text
//! UPDATE_STRESS_FEE_GOLDEN=1 cargo test -p bth-cluster-tax --test stress_fee_shape
//! ```

use bth_cluster_tax::{
    consensus_fee_breakdown, consensus_tx_size_bytes, mainnet_monetary_policy, resign_fee,
    ClaimedClass, ConsensusFeeInputs, ConsensusFeeParams, DynamicFeeBase, FeeConfig, FeeModelMode,
    TransactionType, PICO_PER_BTH, TAG_WEIGHT_SCALE, WALLET_CLUSTER_DECAY_RATE,
};

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../scripts/stress/fixtures/fee-shape-golden.json"
);

/// An epoch-0 height (x-series chains stay far below the first halving).
const HEIGHT: u64 = 50_000;
/// `(inputs, outputs)`: one- to four-input payments (payment plus
/// change) and one- and two-input 15-grant prefund batches (plus change).
const SHAPES: [(usize, usize); 6] = [(1, 2), (2, 2), (3, 2), (4, 2), (1, 16), (2, 16)];
/// Value moved (sum of outputs), picocredits. The plan check rounds a shape's
/// moved value up to the next row; `floors_are_monotone_in_value_moved`
/// makes that conservative.
const MOVED: [u64; 13] = [
    5_000_000_000,
    10_000_000_000,
    20_000_000_000,
    30_000_000_000,
    50_000_000_000,
    80_000_000_000,
    100_000_000_000,
    200_000_000_000,
    500_000_000_000,
    1_000_000_000_000,
    10_000_000_000_000,
    50_000_000_000_000,
    100_000_000_000_000,
];
/// Cluster wealth, BTH. 150 BTH sits just above the prefund batch bound
/// (one coinbase cluster through `MAX_FUNDING_DEPTH` = 2 batch hops: 142.6).
const WEALTH_BTH: [u128; 7] = [50, 150, 200, 1_000, 10_000, 100_000, 1_000_000];
const DEPTHS: [&str; 3] = ["hop", "exit", "ring"];

/// Rows of `depth` exist for this shape: `ring` only for prefund batches.
fn has_depth(outputs: usize, depth: &str) -> bool {
    depth != "ring" || outputs == 16
}

struct Row {
    inputs: usize,
    outputs: usize,
    moved: u64,
    wealth_bth: u128,
    depth: &'static str,
    consensus_floor: u64,
    relay_floor_max: u64,
    resign_fee: u64,
}

fn row(inputs: usize, outputs: usize, moved: u64, wealth_bth: u128, depth: &'static str) -> Row {
    let params = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
    let config = FeeConfig::default();
    let wealth = wealth_bth * PICO_PER_BTH;
    let hop = |w: u128| {
        w * u128::from(TAG_WEIGHT_SCALE - WALLET_CLUSTER_DECAY_RATE) / u128::from(TAG_WEIGHT_SCALE)
    };
    let claimed = match depth {
        "hop" => hop(wealth),
        // A fresh coinbase cluster holds exactly one coinbase.
        "ring" => hop(u128::from(mainnet_monetary_policy().initial_reward)),
        _ => 0,
    };
    let size = consensus_tx_size_bytes(inputs, outputs, 0);
    let breakdown = consensus_fee_breakdown(
        &ConsensusFeeInputs {
            size,
            n_out: outputs,
            n_memo: 0,
            claimed: ClaimedClass::Wealth(claimed),
            floor: config.cluster_factor(wealth),
            elapsed: 0,
            height: HEIGHT,
            output_sum: moved,
        },
        &params,
    );
    // The relay floor at the congestion ceiling: the same demurrage, the size
    // term at `base_max` (RPC `estimateFee` quotes at the live relay base).
    let relay_base = config.minimum_fee_dynamic_with_outputs(
        TransactionType::Hidden,
        size,
        claimed,
        outputs,
        0,
        DynamicFeeBase::default().base_max,
    );
    let relay_floor_max = relay_base
        .saturating_add(breakdown.demurrage)
        .max(breakdown.consensus_floor);
    Row {
        inputs,
        outputs,
        moved,
        wealth_bth,
        depth,
        consensus_floor: breakdown.consensus_floor,
        relay_floor_max,
        resign_fee: resign_fee(relay_floor_max),
    }
}

fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for &(inputs, outputs) in &SHAPES {
        for &moved in &MOVED {
            for &wealth in &WEALTH_BTH {
                for depth in DEPTHS {
                    if has_depth(outputs, depth) {
                        rows.push(row(inputs, outputs, moved, wealth, depth));
                    }
                }
            }
        }
    }
    rows
}

fn render(rows: &[Row]) -> String {
    let policy = mainnet_monetary_policy();
    let mut out = String::from("{\n");
    out.push_str(
        "  \"generator\": \"cargo test -p bth-cluster-tax --test stress_fee_shape (#2023)\",\n",
    );
    out.push_str(&format!("  \"height\": {HEIGHT},\n"));
    out.push_str(&format!("  \"pico_per_bth\": {PICO_PER_BTH},\n"));
    // Epoch 0 pays no lottery emission share: a coinbase is the whole reward.
    out.push_str(&format!(
        "  \"coinbase_picocredits\": {},\n",
        policy.initial_reward
    ));
    out.push_str(&format!(
        "  \"wallet_decay_rate\": {WALLET_CLUSTER_DECAY_RATE},\n"
    ));
    out.push_str(&format!("  \"tag_weight_scale\": {TAG_WEIGHT_SCALE},\n"));
    out.push_str(
        "  \"columns\": [\"inputs\", \"outputs\", \"moved\", \"wealth_bth\", \"depth\", \
         \"consensus_floor\", \"relay_floor_max\", \"resign_fee\"],\n",
    );
    out.push_str("  \"rows\": [\n");
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&format!(
            "    [{}, {}, {}, {}, \"{}\", {}, {}, {}]{}\n",
            r.inputs,
            r.outputs,
            r.moved,
            r.wealth_bth,
            r.depth,
            r.consensus_floor,
            r.relay_floor_max,
            r.resign_fee,
            if i + 1 == rows.len() { "" } else { "," }
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

#[test]
fn stress_fee_shape_golden_matches_the_fee_function() {
    let rendered = render(&rows());
    if std::env::var_os("UPDATE_STRESS_FEE_GOLDEN").is_some() {
        std::fs::write(GOLDEN, &rendered).unwrap();
        return;
    }
    let pinned = std::fs::read_to_string(GOLDEN).unwrap_or_default();
    assert!(
        pinned == rendered,
        "{GOLDEN} is stale: the 10.0.0 fee function changed. Re-run with \
         UPDATE_STRESS_FEE_GOLDEN=1, then re-run scripts/stress/plan.py on the \
         x-series plan (#2023)"
    );
}

#[test]
fn floors_are_monotone_in_value_moved() {
    // The plan check prices a shape at the next larger MOVED row.
    for &(inputs, outputs) in &SHAPES {
        for &wealth in &WEALTH_BTH {
            for depth in DEPTHS.into_iter().filter(|d| has_depth(outputs, d)) {
                let fees: Vec<u64> = MOVED
                    .iter()
                    .map(|&moved| row(inputs, outputs, moved, wealth, depth).resign_fee)
                    .collect();
                assert!(
                    fees.windows(2).all(|w| w[0] <= w[1]),
                    "{inputs}x{outputs} at {wealth} BTH {depth}: {fees:?}"
                );
            }
        }
    }
}

#[test]
fn ring_floors_are_monotone_in_ring_wealth() {
    // The plan check prices a batch's ring downgrade at the next larger
    // WEALTH_BTH row above the prefunder's cluster-wealth bound.
    for &(inputs, outputs) in SHAPES.iter().filter(|s| has_depth(s.1, "ring")) {
        for &moved in &MOVED {
            let fees: Vec<u64> = WEALTH_BTH
                .iter()
                .map(|&wealth| row(inputs, outputs, moved, wealth, "ring").resign_fee)
                .collect();
            assert!(
                fees.windows(2).all(|w| w[0] <= w[1]),
                "{inputs}x{outputs} moving {moved} ring: {fees:?}"
            );
        }
    }
}

#[test]
fn floors_are_monotone_in_inputs() {
    // The workload replay accepts a spend that moves no more than a priced
    // shape with at least as many inputs (and the same outputs).
    for &(inputs, outputs) in &SHAPES {
        let Some(&(more, _)) = SHAPES.iter().find(|s| s.1 == outputs && s.0 == inputs + 1) else {
            continue;
        };
        for &moved in &MOVED {
            for &wealth in &WEALTH_BTH {
                for depth in DEPTHS.into_iter().filter(|d| has_depth(outputs, d)) {
                    let fewer = row(inputs, outputs, moved, wealth, depth).resign_fee;
                    let larger = row(more, outputs, moved, wealth, depth).resign_fee;
                    assert!(
                        fewer <= larger,
                        "{inputs} vs {more} inputs x{outputs} moving {moved} at {wealth} BTH \
                         {depth}: {fewer} > {larger}"
                    );
                }
            }
        }
    }
}
