//! Sim <-> code constant parity guard (#1702, part of #1683).
//!
//! The `bth-cluster-tax` simulations cannot depend on `botho` or
//! `botho-wallet` (both depend on `cluster-tax`), so some production values
//! reach them as copies. Where the dependency direction allows, the sims now
//! import the shared constant instead (`MAINNET_BLOCKS_PER_YEAR`,
//! `CONSENSUS_RING_SIZE`, `LOTTERY_POOL_FRACTION_PERMILLE`,
//! `LOTTERY_BASE_FEE_PICO`, `bth_transaction_types::{RING_SIZE,
//! MAX_CLUSTER_TAGS, MAX_OUTPUTS, MAX_BLOCK_SIZE, ...}`). This file pins every
//! remaining copy to the value the node or wallet actually uses, so a
//! production change fails here until the sim and its published report follow.
//!
//! Already guarded elsewhere, not repeated here:
//! - emission model and `mainnet_policy_mirror` vs the node:
//!   `botho/src/ledger/emission_parity_tests.rs` (#1699);
//! - consensus fee split, `CONSENSUS_RING_SIZE`, wallet factor band:
//!   `botho/src/ledger/consensus_fee_parity_tests.rs` (#1698);
//! - privacy-sim decoy pickers, `AGE_SIMILARITY_SPREAD_BPS` and gamma:
//!   `botho/tests/privacy_sim_route_parity.rs` (#1700);
//! - faithful reward-cap sweep goldens and `MAX_LOTTERY_CANDIDATES`:
//!   `botho/src/ledger/lottery_v2.rs` (#1682);
//! - fee-floor inputs vs the ledger: `cluster-tax/src/fee_floor_inputs.rs`
//!   (#1742); M2 production parameters: `m2_production.rs` tests (#1696).
//!
//! # Intentional divergences
//!
//! A few historical harnesses keep pre-production parameters on purpose so
//! their recorded results stay reproducible.
//! [`intentional_divergences_are_still_the_documented_values`] pins them, so
//! aligning one (or drifting further) forces this list to be revisited:
//!
//! - `m2.rs` (legacy `m2-cumulative` / `m2-decay`) and the `lottery-experiment`
//!   CLI assume 20 s blocks (4,320/day); `m2_production` is the at-node run.
//! - The node's own decoy gamma assumes 2-minute blocks (720/day) while the
//!   chain targets 5 s blocks. The sims mirror the node, so this is a node
//!   question, tracked in #1692, not sim drift.

use bth_cluster_tax::{
    consensus_fee::{
        CONSENSUS_RING_SIZE, LOTTERY_POOL_FRACTION_PERMILLE, MAINNET_BLOCKS_PER_YEAR,
        MAINNET_HALVING_INTERVAL, RELAY_MIN_TX_FEE,
    },
    simulation::{
        bridge_import_sweep, decoy_quantile_sweep::DecoySweepParams,
        downgrade_overcharge_sweep::HonestSweepParams, emission_sweep, lottery_reward_cap_faithful,
        lottery_selection_sweep, m2, m2_production, privacy, privacy_routes,
        provenance_factor_sweep, settlement_horizon_sweep,
    },
    BLOCKS_PER_YEAR_5S, BRIDGE_IMPORT_EPOCH_BLOCKS, BRIDGE_IMPORT_FACTOR_FLOOR, FACTOR_SCALE,
    LOTTERY_BASE_FEE_PICO, SETTLEMENT_HORIZON_BLOCKS, TAG_WEIGHT_SCALE,
};

const SECS_PER_YEAR: u64 = 365 * 24 * 60 * 60;

/// Blocks per year at the node's mainnet target block time, derived the way
/// the node derives it (`ConsensusFeeParams::from_policy`, the demurrage
/// checks in `ledger/store.rs`).
fn node_blocks_per_year() -> u64 {
    SECS_PER_YEAR / botho::monetary::mainnet_policy().target_block_time_secs
}

/// The node's demurrage rate once demurrage is active (epoch 1 onward).
fn node_epoch1_demurrage_rate_bps() -> u32 {
    let policy = botho::monetary::mainnet_policy();
    let rate = policy.demurrage_rate_bps(policy.halving_interval);
    assert!(rate > 0, "demurrage must be active in epoch 1");
    rate
}

/// `BLOCKS_PER_YEAR` (and its 5 s variants) everywhere a sim counts years.
#[test]
fn blocks_per_year_matches_node_block_time() {
    let node = node_blocks_per_year();
    assert_eq!(node, 6_307_200, "mainnet target is 5 s blocks");
    assert_eq!(MAINNET_BLOCKS_PER_YEAR, node);
    assert_eq!(BLOCKS_PER_YEAR_5S, node);
    assert_eq!(MAINNET_HALVING_INTERVAL, node);
    assert_eq!(botho::monetary::mainnet_policy().halving_interval, node);

    assert_eq!(settlement_horizon_sweep::BLOCKS_PER_YEAR, node);
    assert_eq!(emission_sweep::BLOCKS_PER_YEAR, node);
    assert_eq!(lottery_selection_sweep::BLOCKS_PER_YEAR, node);
    // provenance_factor_sweep.rs is SHA-pinned by the CT contract inventory,
    // so its copy is asserted rather than rewritten as an import.
    assert_eq!(provenance_factor_sweep::BLOCKS_PER_YEAR, node);
    assert_eq!(m2_production::PROD_BLOCKS_PER_YEAR, node);
    assert_eq!(DecoySweepParams::default().blocks_per_year, node);
    assert_eq!(
        lottery_reward_cap_faithful::BLOCKS_PER_DAY,
        (node / 365) as f64
    );
}

/// `demurrage_rate_bps`: the sims' `RATE_BPS` copies equal the node's rate,
/// and the node keeps demurrage off in epoch 0 (which the sims model via
/// `ConsensusFeeParams::demurrage_rate_bps`, shared code).
#[test]
fn demurrage_rate_matches_node_policy() {
    let rate = node_epoch1_demurrage_rate_bps();
    assert_eq!(settlement_horizon_sweep::RATE_BPS, rate);
    assert_eq!(lottery_selection_sweep::RATE_BPS, rate);
    assert_eq!(provenance_factor_sweep::RATE_BPS, rate);
    assert_eq!(DecoySweepParams::default().rate_bps, rate);

    let policy = botho::monetary::mainnet_policy();
    assert_eq!(policy.demurrage_rate_bps(0), 0);
    assert_eq!(policy.demurrage_rate_bps(policy.halving_interval - 1), 0);
}

/// `SETTLEMENT_HORIZON_BLOCKS` is shared code (node and sims both import it);
/// pin its ratified meaning — five years at the node's block time (#833).
#[test]
fn settlement_horizon_is_five_node_years() {
    assert_eq!(SETTLEMENT_HORIZON_BLOCKS, 5 * node_blocks_per_year());
}

/// `BRIDGE_IMPORT_*` are shared code; pin the ratified ADR-0007 meaning
/// (K = one day of node blocks, F = 1.5x) and the calibration sweep's derived
/// reference values.
#[test]
fn bridge_import_constants_match_ratified_values() {
    let blocks_per_day = node_blocks_per_year() / 365;
    assert_eq!(BRIDGE_IMPORT_EPOCH_BLOCKS, blocks_per_day);
    assert_eq!(BRIDGE_IMPORT_FACTOR_FLOOR, FACTOR_SCALE * 3 / 2);

    let target = botho::monetary::mainnet_policy().target_block_time_secs;
    assert_eq!(bridge_import_sweep::reference_block_secs(), target);
    assert_eq!(bridge_import_sweep::BLOCK_TIMES_SECS[0], target);
    assert_eq!(bridge_import_sweep::hold_blocks_one_day(), blocks_per_day);
}

/// `MIN_TX_FEE`: the sims' relay minimum equals the node's mempool minimum.
#[test]
fn relay_min_fee_matches_node() {
    assert_eq!(RELAY_MIN_TX_FEE, botho::transaction::MIN_TX_FEE);
    assert_eq!(RELAY_MIN_TX_FEE, bth_transaction_clsag::MIN_TX_FEE);
}

/// `MIN_RING_SIZE`: every sim that models production rings uses the
/// consensus minimum, as does the thin wallet.
#[test]
fn ring_size_matches_consensus_minimum() {
    let min = botho::transaction::MIN_RING_SIZE;
    assert_eq!(min, bth_transaction_clsag::MIN_RING_SIZE);
    assert_eq!(CONSENSUS_RING_SIZE, min);
    assert_eq!(bth_transaction_types::RING_SIZE, min);
    assert_eq!(privacy::RING_SIZE, min);
    // The #577 gate and #925 honest over-charge sweeps were re-run at ring 20
    // (#1796); their defaults must track consensus, not a historical 11.
    assert_eq!(DecoySweepParams::default().ring_size, min);
    assert_eq!(HonestSweepParams::default().ring_size, min);
    assert_eq!(privacy::DECOY_COUNT, min - 1);
    assert_eq!(m2_production::RING_SIZE, min);
    assert_eq!(botho_wallet::decoy_selection::DEFAULT_RING_SIZE, min);
}

/// `AGE_SIMILARITY_SPREAD_BPS` and the decoy age bounds: node, thin wallet
/// and both privacy sims agree.
#[test]
fn decoy_selection_constants_match_node() {
    use botho::decoy_selection as node;
    use botho_wallet::decoy_selection as wallet;

    assert_eq!(
        wallet::AGE_SIMILARITY_SPREAD_BPS,
        node::AGE_SIMILARITY_SPREAD_BPS
    );
    assert_eq!(
        privacy_routes::AGE_SIMILARITY_SPREAD_BPS,
        node::AGE_SIMILARITY_SPREAD_BPS
    );
    assert_eq!(wallet::MIN_DECOY_AGE_BLOCKS, node::MIN_DECOY_AGE_BLOCKS);
    assert_eq!(
        privacy_routes::MIN_DECOY_AGE_BLOCKS,
        node::MIN_DECOY_AGE_BLOCKS
    );
    assert_eq!(privacy::MIN_AGE_BLOCKS, node::MIN_DECOY_AGE_BLOCKS);
    assert_eq!(privacy::MAX_AGE_BLOCKS, node::MAX_DECOY_AGE_BLOCKS);
    assert_eq!(
        privacy::MIN_CLUSTER_SIMILARITY,
        node::MIN_CLUSTER_SIMILARITY
    );

    // The legacy privacy sim's gamma reuses the route-parity mirrors, which
    // `privacy_sim_route_parity.rs` pins to `GammaDecoySelector::new()`.
    let selector = node::GammaDecoySelector::new();
    assert_eq!(selector.distribution().shape(), privacy::GAMMA_SHAPE);
    assert_eq!(
        selector.distribution().scale_blocks(),
        privacy::GAMMA_SCALE_DAYS * privacy::BLOCKS_PER_DAY
    );
}

/// Per-hop cluster-tag decay: the sims' 5% equals the node and wallet default.
#[test]
fn wallet_tag_decay_matches_sims() {
    let node = botho::wallet::DEFAULT_CLUSTER_DECAY_RATE;
    assert_eq!(botho_wallet::transaction::DEFAULT_CLUSTER_DECAY_RATE, node);
    let scale = TAG_WEIGHT_SCALE as f64;
    assert_eq!(
        (m2_production::STOCK_WALLET_DECAY * scale).round() as u32,
        node
    );
    assert_eq!((privacy::DEFAULT_DECAY_RATE * scale).round() as u32, node);
    assert_eq!(
        m2_production::MIN_TAG_WEIGHT,
        bth_transaction_types::MIN_STORED_WEIGHT as f64
            / bth_transaction_types::TAG_WEIGHT_SCALE as f64
    );
    assert_eq!(
        m2_production::MAX_CLUSTER_TAGS,
        bth_transaction_types::MAX_CLUSTER_TAGS
    );
}

/// Lottery pool split, base fee and block limits the lottery sims price with.
#[test]
fn lottery_constants_match_node() {
    let node_pool = botho::consensus::LotteryFeeConfig::default().pool_fraction_permille;
    assert_eq!(LOTTERY_POOL_FRACTION_PERMILLE, node_pool);
    assert_eq!(
        lottery_reward_cap_faithful::POOL_FRACTION_PERMILLE,
        node_pool as u64
    );
    assert_eq!(m2_production::POOL_FRACTION_PERMILLE, node_pool as u128);
    assert_eq!(
        m2_production::MAX_LOTTERY_CANDIDATES,
        lottery_reward_cap_faithful::MAX_LOTTERY_CANDIDATES as u64
    );

    // The node's Sybil cap prices tickets at the shared LOTTERY_BASE_FEE_PICO.
    assert_eq!(
        lottery_selection_sweep::BASE_FEE_PICO,
        LOTTERY_BASE_FEE_PICO as u128
    );
    assert_eq!(
        provenance_factor_sweep::BASE_FEE_PICO,
        LOTTERY_BASE_FEE_PICO as u128
    );

    assert_eq!(
        lottery_reward_cap_faithful::MAX_OUTPUTS_PER_TX as u64,
        bth_transaction_types::MAX_OUTPUTS
    );
    assert_eq!(
        lottery_reward_cap_faithful::MAX_BLOCK_SIZE_BYTES,
        bth_transaction_types::MAX_BLOCK_SIZE
    );
    assert_eq!(
        lottery_reward_cap_faithful::PROPOSER_MAX_TXS_PER_SLOT,
        botho::consensus::ConsensusConfig::default().max_txs_per_slot
    );
}

/// The documented intentional divergences (module docs). If one of these
/// fails, a harness was realigned or drifted: update the list above, and the
/// harness's published report, in the same change.
#[test]
fn intentional_divergences_are_still_the_documented_values() {
    assert_eq!(m2::M2_BLOCKS_PER_DAY, 4_320);
    assert_ne!(m2::M2_BLOCKS_PER_DAY, node_blocks_per_year() / 365);

    // Node gamma assumes 2-minute blocks (#1692); the sims mirror the node.
    assert_eq!(privacy_routes::NODE_BLOCKS_PER_DAY, 720.0);
    assert_eq!(privacy::BLOCKS_PER_DAY, privacy_routes::NODE_BLOCKS_PER_DAY);
}
