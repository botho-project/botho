//! Pins the decoy-flood sweep's mirrored constants to production (#1701).
//!
//! `bth_cluster_tax::simulation::decoy_flood_sweep` cannot depend on `botho`
//! (`botho` depends on `cluster-tax`), so it copies the handful of constants
//! that price a flood and its lottery recapture. If any of them changes here,
//! this test fails until the sweep — and the report in
//! `docs/research/decoy-flood-cost.md` — are updated.
//!
//! The picker ports are pinned separately by `privacy_sim_route_parity.rs`, and
//! the fee model by the `consensus_fee_model_matches_ledger_floor_*` golden
//! vectors in `botho/src/ledger/store.rs`.

use botho::{consensus::lottery::LotteryFeeConfig, monetary::mainnet_policy};
use bth_cluster_tax::simulation::{decoy_flood_sweep as sweep, privacy};

fn source(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn transaction_shape_constants_match() {
    assert_eq!(
        sweep::MAX_OUTPUTS_PER_TX,
        bth_transaction_types::constants::MAX_OUTPUTS
    );
    assert_eq!(
        privacy::RING_SIZE,
        bth_transaction_types::constants::RING_SIZE
    );
    assert_eq!(privacy::RING_SIZE, bth_transaction_clsag::DEFAULT_RING_SIZE);
    assert_eq!(
        bth_cluster_tax::consensus_fee::RELAY_MIN_TX_FEE,
        bth_transaction_clsag::MIN_TX_FEE
    );
    // Flood outputs carry exactly the lottery dust floor, which must also be
    // relayable (the mempool's dust threshold).
    assert_eq!(
        sweep::lottery_draw_config().min_utxo_value,
        bth_transaction_clsag::DUST_THRESHOLD
    );
}

#[test]
fn lottery_constants_match() {
    let node = LotteryFeeConfig::default();
    assert_eq!(
        sweep::LOTTERY_POOL_PERMILLE,
        node.pool_fraction_permille as u64
    );
    let ours = sweep::lottery_draw_config();
    assert_eq!(ours.min_utxo_age, node.draw_config.min_utxo_age);
    assert_eq!(ours.circulation_window, node.draw_config.circulation_window);
    assert_eq!(ours.min_utxo_value, node.draw_config.min_utxo_value);

    // The consensus config (`ledger::lottery_v2::canonical_config`) is private;
    // check its literal pool share and that it uses the default draw config.
    let v2 = source("src/ledger/lottery_v2.rs");
    assert!(
        v2.contains("pool_fraction_permille: 800,")
            && v2.contains("draw_config: bth_cluster_tax::LotteryDrawConfig::default(),"),
        "canonical_config changed; update decoy_flood_sweep"
    );

    // MAX_LOTTERY_CANDIDATES is function-local in `lottery_candidates`.
    let validation = source("src/ledger/store/validation.rs");
    let needle = format!(
        "const MAX_LOTTERY_CANDIDATES: usize = {};",
        format_thousands(sweep::MAX_LOTTERY_CANDIDATES)
    );
    assert!(
        validation.contains(&needle),
        "expected `{needle}` in validation.rs"
    );
}

#[test]
fn mainnet_reward_schedule_matches() {
    let node = mainnet_policy();
    let ours = sweep::mainnet_monetary_policy();
    assert_eq!(ours.initial_reward, node.initial_reward);
    assert_eq!(ours.halving_interval, node.halving_interval);
    assert_eq!(ours.halving_count, node.halving_count);
    for epoch in 0..4 {
        let h = sweep::epoch_height(epoch);
        assert_eq!(ours.halving_reward(h), node.halving_reward(h));
        let r = node.halving_reward(h).unwrap();
        assert_eq!(
            ours.lottery_emission_share(h, r),
            node.lottery_emission_share(h, r)
        );
    }
}

/// `10000` -> `10_000`, the way the source writes it.
fn format_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('_');
        }
        out.push(ch);
    }
    out
}
