//! Drift guards: the `cluster-tax` emission model against the node (#1699).
//!
//! The simulators (`bth_cluster_tax::simulation::emission_sweep`, the
//! `DifficultyController` used by the agent-based runner) model issuance,
//! fee burn and block timing with copies of the node's rules, because
//! `cluster-tax` cannot depend on `botho`. These tests fail CI when the node
//! and the copies diverge. They read node code only; nothing here changes
//! consensus behaviour.

// `emission_sweep::cumulative_supply_at` is a thin wrapper over
// `MonetaryPolicy::cumulative_issuance_at` (the `simulation` module is behind a
// cluster-tax feature botho does not enable), so the policy methods are what
// these tests pin.
use bth_cluster_tax::monetary::{mainnet_policy_mirror, node_constants};

use crate::{
    block::{calculate_block_reward, difficulty, dynamic_timing},
    consensus::ConsensusConfig,
    monetary::mainnet_policy,
};

/// Policy-years the issuance parity test covers (5 halving + 15 tail).
const PARITY_POLICY_YEARS: u64 = 20;

/// The sim's copy of the mainnet policy is the node's policy, field for field.
#[test]
fn mainnet_policy_mirror_matches_node() {
    assert_eq!(
        mainnet_policy_mirror(),
        mainnet_policy(),
        "bth_cluster_tax::monetary::mainnet_policy_mirror drifted from \
         botho::monetary::mainnet_policy — update the mirror (do not change \
         the ratified policy to match a sim)"
    );
}

/// #1699 acceptance: `emission_sweep::cumulative_supply_at` (=
/// `MonetaryPolicy::cumulative_issuance_at`) equals the
/// node's `ChainState::total_mined` (the running sum of
/// `calculate_block_reward(height, total_mined)`) over 20 policy-years.
///
/// The node loop is the validator's own rule
/// (`ledger::store::validation`: `calculate_block_reward(block.height(),
/// state.total_mined)`, then `total_mined += reward`), block by block from
/// height 1 — about 126M calls. Checkpoints cover every halving boundary, the
/// tail onset, and the end of every policy-year.
#[test]
fn sim_cumulative_supply_matches_node_total_mined_over_20_policy_years() {
    let node_policy = mainnet_policy();
    let sim_policy = mainnet_policy_mirror();
    let bpy = node_policy.halving_interval;
    let tail_start = node_policy.tail_emission_start_height();
    let end = PARITY_POLICY_YEARS * bpy;

    let mut checkpoints: Vec<u64> = vec![1, 2, tail_start - 1, tail_start, tail_start + 1];
    for y in 1..=PARITY_POLICY_YEARS {
        checkpoints.push(y * bpy - 1);
        checkpoints.push(y * bpy);
    }
    checkpoints.sort_unstable();
    checkpoints.dedup();

    let sim = sim_policy.issuance_at_checkpoints(&checkpoints);

    let mut total_mined: u128 = 0;
    let mut next = 0usize;
    for height in 1..=end {
        total_mined += calculate_block_reward(height, total_mined) as u128;
        if next < checkpoints.len() && checkpoints[next] == height {
            assert_eq!(
                sim[next], total_mined,
                "sim issuance diverged from node total_mined at height {height}"
            );
            next += 1;
        }
    }
    assert_eq!(next, checkpoints.len());

    // The single-height entry point agrees.
    assert_eq!(sim_policy.cumulative_issuance_at(end), total_mined);

    // And the tail really compounds: the old fixed-at-onset model is short.
    let onset = sim[checkpoints
        .iter()
        .position(|&h| h == tail_start - 1)
        .unwrap()];
    let fixed_tail = node_policy.calculate_tail_reward_u128(onset) as u128;
    let fixed_model = onset + (end - tail_start + 1) as u128 * fixed_tail;
    assert!(total_mined > fixed_model);
}

/// Per-block reward rule: the sim's `block_reward_at` is the node's
/// `calculate_block_reward` at both phases, including supplies past
/// `u64::MAX`.
#[test]
fn block_reward_at_matches_calculate_block_reward() {
    let p = mainnet_policy_mirror();
    let h = p.halving_interval;
    let supplies: [u128; 4] = [
        0,
        1,
        611_010_000 * 1_000_000_000_000,
        u64::MAX as u128 * 1_000,
    ];
    for height in [
        1,
        2,
        h - 1,
        h,
        2 * h + 7,
        5 * h - 1,
        5 * h,
        5 * h + 1,
        40 * h,
    ] {
        for &s in &supplies {
            assert_eq!(
                p.block_reward_at(height, s),
                calculate_block_reward(height, s),
                "height {height} supply {s}"
            );
        }
    }
}

/// The node's fee burn share is what the sims assume (20%; 80% to the
/// lottery pool via the canonical, environment-independent config).
#[test]
fn fee_burn_share_matches_canonical_lottery_config() {
    let canonical = super::lottery_v2::canonical_config();
    assert_eq!(
        node_constants::FEE_BURN_SHARE_PERMILLE,
        1_000 - canonical.pool_fraction_permille
    );
    // And split_fees really burns that share.
    let (pool, burn) = canonical.split_fees(1_000_000);
    assert_eq!(pool, 800_000);
    assert_eq!(
        burn,
        1_000_000 * node_constants::FEE_BURN_SHARE_PERMILLE as u64 / 1_000
    );
}

/// Block timing: the difficulty target and the load-paced tiers.
#[test]
fn block_timing_constants_match_node() {
    assert_eq!(
        node_constants::TARGET_BLOCK_TIME_SECS,
        difficulty::TARGET_BLOCK_TIME_SECS
    );
    assert_eq!(
        node_constants::TARGET_BLOCK_TIME_SECS,
        mainnet_policy().target_block_time_secs
    );
    assert_eq!(
        node_constants::MIN_BLOCK_TIME_SECS,
        ConsensusConfig::MIN_BLOCK_TIME_SECS
    );
    assert_eq!(
        node_constants::MAX_BLOCK_TIME_SECS,
        dynamic_timing::MAX_BLOCK_TIME
    );
    assert_eq!(
        node_constants::BLOCK_TIME_LEVELS,
        dynamic_timing::BLOCK_TIME_LEVELS
    );
}
