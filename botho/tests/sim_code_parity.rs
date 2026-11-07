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
//!
//! The node's decoy gamma used to assume 2-minute blocks (720/day). Since #1692
//! it derives blocks/day from the monetary policy block time, and
//! [`decoy_gamma_blocks_per_day_follows_policy`] pins node and sims to it.

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
}

/// #1692: the decoy gamma's blocks/day (and its ~2-year age cap) come from the
/// mainnet policy block time, in the node and in both privacy sims.
#[test]
fn decoy_gamma_blocks_per_day_follows_policy() {
    use botho::decoy_selection as node;
    let policy_per_day = 24 * 60 * 60 / botho::monetary::mainnet_policy().target_block_time_secs;
    assert_eq!(node::BLOCKS_PER_DAY, policy_per_day);
    assert_eq!(node::BLOCKS_PER_DAY, node_blocks_per_year() / 365);
    assert_eq!(
        privacy_routes::NODE_BLOCKS_PER_DAY,
        node::BLOCKS_PER_DAY as f64
    );
    assert_eq!(privacy::BLOCKS_PER_DAY, privacy_routes::NODE_BLOCKS_PER_DAY);
    assert_eq!(node::MAX_DECOY_AGE_BLOCKS, 2 * 365 * node::BLOCKS_PER_DAY);
}

/// Actual state transitions, independently composed through the node API.
/// These bounded cases establish conditional Rust parity, not chain
/// reachability or the randomized sim/node/Lean DRT reserved for #1969.
mod production_lottery {
    use std::collections::{BTreeMap, BTreeSet};

    use botho::consensus::lottery::{
        compute_pool_accounting, draw_lottery_winners_seeded, reward_cap, LotteryFeeConfig,
    };
    use bth_cluster_tax::{
        simulation::{
            lottery::LotterySimulation,
            lottery_production::{
                ProductionBlock, ProductionBlockResult, ProductionOutput, MAX_PRODUCTION_CANDIDATES,
            },
            lottery_reward_cap_faithful,
        },
        LotteryDrawConfig, SelectionMode,
    };

    fn canonical() -> LotteryFeeConfig {
        // Do not use Default: it accepts environment-sensitive test overrides.
        LotteryFeeConfig {
            pool_fraction_permille: 800,
            draw_config: LotteryDrawConfig::default(),
        }
    }

    fn fixture(count: u64, carryover: u128) -> (LotterySimulation, ProductionBlock) {
        let outputs: Vec<_> = (1..=count)
            .map(|id| {
                let mut identity = [0; 36];
                identity[..8].copy_from_slice(&id.to_le_bytes());
                ProductionOutput {
                    id: identity,
                    simulation_id: id,
                    owner_id: id % 2,
                    value: 1_000_000,
                    cluster_factor: 1000 + id * 100,
                    creation_block: 0,
                }
            })
            .collect();
        (
            LotterySimulation::new_production(outputs.clone(), carryover).unwrap(),
            ProductionBlock {
                height: 720,
                total_fees: 0,
                emission_share: 0,
                block_reward: u64::MAX,
                v2_seed: [73; 32],
                candidates: outputs,
            },
        )
    }

    fn compare(sim: &mut LotterySimulation, block: &ProductionBlock) -> ProductionBlockResult {
        let config = canonical();
        let candidates: Vec<_> = block
            .candidates
            .iter()
            .map(ProductionOutput::candidate)
            .collect();
        let cap = reward_cap(&candidates, block.height, block.block_reward, &config);
        let accounting = compute_pool_accounting(
            block.total_fees,
            block.emission_share,
            sim.production_carryover().unwrap(),
            cap,
            &config,
        );
        let node = draw_lottery_winners_seeded(
            &candidates,
            block.total_fees,
            &accounting,
            block.height,
            &block.v2_seed,
            &config,
        );
        let originals = sim.utxos.clone();
        let owners: BTreeMap<_, _> = sim
            .owners
            .iter()
            .map(|(&id, owner)| {
                (
                    id,
                    (
                        sim.owner_value(id),
                        owner.total_winnings,
                        owner.utxo_ids.clone(),
                    ),
                )
            })
            .collect();
        let before_metrics = sim.metrics.clone();
        let before_burned = sim.total_burned;
        let before_count = sim.production_outputs().unwrap().len();
        let result = sim.apply_production_block(block).unwrap();
        assert_eq!(result.block_height, node.block_height);
        assert_eq!(result.total_fees, node.total_fees);
        assert_eq!(result.fee_pool, accounting.fee_pool);
        assert_eq!(result.burn_amount, accounting.fee_burn);
        assert_eq!(result.burn_amount, node.burn_amount);
        assert_eq!(result.emission_share, accounting.emission_share);
        assert_eq!(result.available, accounting.available);
        assert_eq!(result.payout_cap, cap);
        assert_eq!(result.payout, accounting.payout);
        assert_eq!(result.pool_amount, node.pool_amount);
        assert_eq!(result.seed, node.seed);
        assert_eq!(result.paid, node.total_distributed());
        assert_eq!(
            result.carryover,
            accounting.available - node.total_distributed() as u128
        );
        assert_eq!(sim.production_carryover(), Some(result.carryover));
        assert_eq!(
            result
                .awards
                .iter()
                .map(|a| (a.winner_id, a.output.value))
                .collect::<Vec<_>>(),
            node.winners
                .iter()
                .map(|w| (w.utxo_id, w.payout))
                .collect::<Vec<_>>()
        );
        for (id, original) in &originals {
            assert_eq!(format!("{:?}", sim.utxos[id]), format!("{original:?}"));
        }
        let mut credits = BTreeMap::<u64, u64>::new();
        let mut memberships = BTreeMap::<u64, Vec<u64>>::new();
        let mut ids = BTreeSet::new();
        let mut quintiles = before_metrics.winnings_by_quintile;
        let mut wealths: Vec<_> = owners.values().map(|o| o.0).collect();
        wealths.sort_unstable();
        for award in &result.awards {
            let source = block
                .candidates
                .iter()
                .find(|o| o.id == award.winner_id)
                .unwrap();
            assert_eq!(award.output.owner_id, source.owner_id);
            assert_eq!(award.output.creation_block, block.height);
            assert!(!originals.contains_key(&award.output.simulation_id));
            assert!(ids.insert(award.output.id));
            assert_eq!(
                sim.production_outputs().unwrap()[&award.output.id],
                award.output
            );
            let stored = &sim.utxos[&award.output.simulation_id];
            assert_eq!(
                (
                    stored.value,
                    stored.owner_id,
                    stored.creation_block,
                    stored.demurrage_anchor
                ),
                (
                    award.output.value,
                    source.owner_id,
                    block.height,
                    block.height
                )
            );
            *credits.entry(source.owner_id).or_default() += stored.value;
            memberships
                .entry(source.owner_id)
                .or_default()
                .push(stored.id);
            let rank = wealths
                .iter()
                .position(|&w| w >= owners[&source.owner_id].0)
                .unwrap();
            let quintile = rank * 5 / wealths.len();
            assert_eq!(award.quintile, quintile);
            quintiles[quintile] += stored.value;
        }
        assert_eq!(
            sim.production_outputs().unwrap().len(),
            before_count + node.winners.len()
        );
        assert_eq!(sim.utxos.len(), originals.len() + node.winners.len());
        for (&id, (balance, winnings, old_ids)) in &owners {
            let credit = credits.get(&id).copied().unwrap_or(0);
            assert_eq!(sim.owner_value(id), balance + credit);
            assert_eq!(sim.owners[&id].total_winnings, winnings + credit);
            let mut expected = old_ids.clone();
            expected.extend(memberships.get(&id).into_iter().flatten().copied());
            assert_eq!(sim.owners[&id].utxo_ids, expected);
            assert_eq!(
                sim.owners[&id].total_fees_paid, 0,
                "fees were collected externally"
            );
        }
        assert_eq!(credits.values().sum::<u64>(), node.total_distributed());
        assert_eq!(
            sim.metrics.total_distributed,
            before_metrics.total_distributed + node.total_distributed()
        );
        assert_eq!(
            sim.metrics.total_fees_collected,
            before_metrics.total_fees_collected + block.total_fees
        );
        assert_eq!(sim.total_burned, before_burned + accounting.fee_burn);
        assert_eq!(
            sim.metrics.drawings_held,
            before_metrics.drawings_held + u64::from(node.has_winners())
        );
        assert_eq!(sim.metrics.winnings_by_quintile, quintiles);
        assert_eq!(sim.current_block, block.height);
        assert_eq!(sim.lottery_pool, 0);
        result
    }

    #[test]
    fn canonical_configuration_and_candidate_bound_match_node() {
        let config = canonical();
        assert_eq!(
            config.pool_fraction_permille,
            bth_cluster_tax::consensus_fee::LOTTERY_POOL_FRACTION_PERMILLE
        );
        assert_eq!(config.draw_config.selection_mode, SelectionMode::Uniform);
        assert_eq!(config.draw_config.winners_per_draw, 4);
        assert_eq!(config.draw_config.min_utxo_age, 720);
        assert_eq!(config.draw_config.min_utxo_value, 1_000_000);
        assert_eq!(config.draw_config.circulation_window, 10_000);
        // The node's existing canonical-config guard pins this mirror to its
        // private MAX_LOTTERY_CANDIDATES without exposing a consensus API.
        assert_eq!(
            MAX_PRODUCTION_CANDIDATES,
            lottery_reward_cap_faithful::MAX_LOTTERY_CANDIDATES
        );
    }

    #[test]
    fn actual_simulator_matches_node_boundaries_and_saturation() {
        for count in [0, 1, 3, 4, 7] {
            for pool in [0, 1, 3, 11, 10_000_000_000_000u128] {
                let (mut sim, block) = fixture(count, pool);
                compare(&mut sim, &block);
            }
        }
        for height in [719, 720, 10_000, 10_001] {
            let (mut sim, mut block) = fixture(4, 20);
            block.height = height;
            block.total_fees = 7;
            block.emission_share = 13;
            compare(&mut sim, &block);
        }
        let (mut sim, mut block) = fixture(4, u128::MAX - 1);
        block.total_fees = u64::MAX;
        block.emission_share = u64::MAX;
        block.block_reward = 7;
        let result = compare(&mut sim, &block);
        assert_eq!(
            (result.available, result.carryover),
            (u128::MAX, u128::MAX - 7)
        );
        let (mut sim, mut block) = fixture(4, 10);
        block.block_reward = 0;
        block.total_fees = 7;
        compare(&mut sim, &block);
    }

    // Deterministic evidence uses decimal amounts and explicit supplied seeds;
    // execution/source identities belong in the CI receipt, not this payload.
    fn output_evidence(o: &ProductionOutput) -> serde_json::Value {
        serde_json::json!({"id": hex::encode(o.id), "simulation_id": o.simulation_id,
            "owner_id": o.owner_id, "value_pico": o.value.to_string(),
            "cluster_factor": o.cluster_factor, "creation_block": o.creation_block})
    }

    fn block_evidence(b: &ProductionBlock) -> serde_json::Value {
        serde_json::json!({"height": b.height, "total_fees_pico": b.total_fees.to_string(),
            "emission_share_pico": b.emission_share.to_string(),
            "block_reward_pico": b.block_reward.to_string(), "supplied_v2_seed": hex::encode(b.v2_seed),
            "ordered_candidates": b.candidates.iter().map(output_evidence).collect::<Vec<_>>()})
    }

    fn result_evidence(r: &ProductionBlockResult) -> serde_json::Value {
        serde_json::json!({"block_height": r.block_height, "total_fees_pico": r.total_fees.to_string(),
            "fee_pool_pico": r.fee_pool.to_string(), "burn_pico": r.burn_amount.to_string(),
            "emission_share_pico": r.emission_share.to_string(), "available_pico": r.available.to_string(),
            "payout_cap_pico": r.payout_cap.to_string(), "payout_pico": r.payout.to_string(),
            "pool_amount_pico": r.pool_amount.to_string(), "result_seed": hex::encode(r.seed),
            "paid_pico": r.paid.to_string(), "carryover_pico": r.carryover.to_string(),
            "awards": r.awards.iter().map(|a| serde_json::json!({
                "winner_id": hex::encode(a.winner_id), "output": output_evidence(&a.output),
                "quintile": a.quintile})).collect::<Vec<_>>()})
    }

    #[test]
    fn actual_production_paired_two_block_evidence() {
        let config = canonical();
        assert_eq!(config.pool_fraction_permille, 800);
        assert_eq!(config.draw_config.selection_mode, SelectionMode::Uniform);
        assert_eq!(config.draw_config.winners_per_draw, 4);
        assert_eq!(config.draw_config.min_utxo_age, 720);
        assert_eq!(config.draw_config.min_utxo_value, 1_000_000);
        assert_eq!(config.draw_config.circulation_window, 10_000);
        let mut traces = vec![];
        for height in [1439, 1440] {
            let (mut sim, mut first_block) = fixture(4, 10_000_000);
            first_block.total_fees = 1_250_000;
            first_block.emission_share = 2_000_000;
            first_block.block_reward = 4_000_000;
            let initial = sim.production_outputs().unwrap().clone();
            let first = compare(&mut sim, &first_block);
            // Independent numeric oracle, in addition to canonical node parity.
            assert_eq!(
                (first.fee_pool, first.burn_amount, first.available),
                (1_000_000, 250_000, 13_000_000)
            );
            assert_eq!(
                (first.payout_cap, first.payout, first.paid, first.carryover),
                (4_000_000, 4_000_000, 4_000_000, 9_000_000)
            );
            assert_eq!(first.awards.len(), 4);
            assert_eq!(first.seed, [73; 32]);
            assert!(first
                .awards
                .iter()
                .all(|a| a.output.value == 1_000_000 && a.output.creation_block == 720));
            let after_first = sim.production_outputs().unwrap().clone();
            assert_eq!(after_first.len(), 8);
            let second_block = ProductionBlock {
                height,
                total_fees: 2_500_000,
                emission_share: 3_000_000,
                block_reward: 4_000_000,
                v2_seed: [74; 32],
                candidates: first.awards.iter().map(|a| a.output.clone()).collect(),
            };
            let second = compare(&mut sim, &second_block);
            assert_eq!(
                (second.fee_pool, second.burn_amount, second.available),
                (2_000_000, 500_000, 14_000_000)
            );
            let mature = height == 1440;
            let expected_paid = if mature { 4_000_000 } else { 0 };
            assert_eq!(
                (second.payout_cap, second.payout, second.paid),
                (expected_paid, expected_paid, expected_paid)
            );
            assert_eq!(
                second.carryover,
                if mature { 10_000_000 } else { 14_000_000 }
            );
            assert_eq!(second.awards.len(), if mature { 4 } else { 0 });
            assert_eq!(second.seed, if mature { [74; 32] } else { [0; 32] });
            assert_eq!(second.pool_amount, expected_paid);
            for award in &second.awards {
                assert_eq!(award.output.creation_block, 1440);
                assert_eq!(award.output.value, 1_000_000);
                assert!(!after_first.contains_key(&award.output.id));
            }
            for (id, output) in initial.iter().chain(after_first.iter()) {
                assert_eq!(&sim.production_outputs().unwrap()[id], output);
            }
            assert_eq!(
                sim.production_outputs().unwrap().len(),
                if mature { 12 } else { 8 }
            );
            assert_eq!(sim.metrics.total_fees_collected, 3_750_000);
            assert_eq!(sim.total_burned, 750_000);
            assert_eq!(
                sim.metrics.total_distributed,
                if mature { 8_000_000 } else { 4_000_000 }
            );
            assert_eq!(second.available, second.paid as u128 + second.carryover);
            let owners: Vec<_> = [0, 1].into_iter().map(|id| {
                assert_eq!(sim.owner_value(id), if mature { 6_000_000 } else { 4_000_000 });
                assert_eq!(sim.owners[&id].total_winnings, if mature { 4_000_000 } else { 2_000_000 });
                assert_eq!(sim.owners[&id].utxo_ids.len(), if mature { 6 } else { 4 });
                serde_json::json!({"owner_id": id, "balance_pico": sim.owner_value(id).to_string(),
                    "winnings_pico": sim.owners[&id].total_winnings.to_string(),
                    "utxo_ids": sim.owners[&id].utxo_ids})
            }).collect();
            traces.push(serde_json::json!({"second_height": height,
                "initial_carryover_pico": "10000000", "initial_outputs": initial.values().map(output_evidence).collect::<Vec<_>>(),
                "blocks": [block_evidence(&first_block), block_evidence(&second_block)],
                "results": [result_evidence(&first), result_evidence(&second)],
                "final_outputs": sim.production_outputs().unwrap().values().map(output_evidence).collect::<Vec<_>>(),
                "owners": owners, "fees_collected_pico": sim.metrics.total_fees_collected.to_string(),
                "burned_pico": sim.total_burned.to_string(), "paid_pico": sim.metrics.total_distributed.to_string(),
                "current_block": sim.current_block, "drawings_held": sim.metrics.drawings_held,
                "winnings_by_quintile": sim.metrics.winnings_by_quintile}));
        }
        assert_eq!(traces[0]["results"][0], traces[1]["results"][0]);
        let payload = serde_json::json!({"schema": 1, "claim": "synthetic actual-entry behavior and node parity only",
            "assumptions": "external ordered synthetic candidates/owners, collected fees, emission/reward scheduling and already-derived V2 seeds; not profitability, equilibrium, candidate discovery or seed derivation",
            "rng": "node/simulator seeded draw; supplied seed is separate from empty-result zero sentinel",
            "config": {"pool_fraction_permille": 800, "selection_mode": "Uniform", "winners_per_draw": 4,
                "min_utxo_age": 720, "min_utxo_value_pico": "1000000", "circulation_window": 10000},
            "traces": traces});
        if let Some(path) = std::env::var_os("BOTHO_LOTTERY_PARITY_PAYLOAD") {
            std::fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
        }
    }

    #[test]
    fn actual_simulator_reuses_carryover_and_fresh_outputs() {
        let (mut sim, mut block) = fixture(4, 10_000_000);
        block.block_reward = 4_000_000;
        let first = compare(&mut sim, &block);
        assert_eq!(first.carryover, 6_000_000);
        block.height += 1;
        block.candidates = sim
            .production_outputs()
            .unwrap()
            .values()
            .cloned()
            .collect();
        assert_eq!(compare(&mut sim, &block).carryover, 2_000_000);
        block.candidates = first.awards.iter().map(|a| a.output.clone()).collect();
        block.height = 1439;
        assert!(compare(&mut sim, &block).awards.is_empty());
        block.height = 1440;
        assert_eq!(compare(&mut sim, &block).paid, 2_000_000);
    }
}
