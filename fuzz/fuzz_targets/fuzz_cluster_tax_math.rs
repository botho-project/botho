#![no_main]

//! Fuzz target for the cluster-tax fixed-point monetary primitives
//! (`botho/src/block.rs::calculate_block_reward`, `cluster-tax`
//! `demurrage_charge`, the integer fee-curve / sigmoid, the cluster-factor
//! LUT, and the emission/supply functions on `MonetaryPolicy`).
//!
//! These are pure deterministic integer functions on consensus state. The
//! whole-network safety property is: they must NEVER panic and never
//! overflow-trap (they are written with checked / saturating / u128-staged
//! arithmetic), and their outputs must stay within their documented bounds.
//! This is the target most likely to surface the known supply-overflow
//! (#333) if a path multiplies un-staged `u64`s.
//!
//! ## Invariants asserted (issue #337, target 4)
//!
//! For arbitrary inputs across the full u64 range:
//! 1. `ClusterFactorCurve::factor` returns a value in `[FACTOR_SCALE,
//!    factor_max * FACTOR_SCALE]` — for the default curve `[1000, 6000]` — and
//!    `sigmoid_approx` returns `<= SIGMOID_SCALE`.
//! 2. `demurrage_charge` never panics/overflows; it is a non-negative u64 fee
//!    floor and returns 0 for factor-1 coins / zero rate / zero elapsed. In the
//!    realistic parameter domain (rate_bps <= 10_000, factor <=
//!    MAX_FACTOR_SCALED, elapsed <= one year) it is bounded above by the
//!    transfer value — it is a fraction of the value being moved. Absurd rates
//!    (>100%/yr) legitimately exceed the value, so the value bound is gated on
//!    that domain while the no-panic invariant is universal.
//! 3. `calculate_block_reward`, `calculate_tail_reward`,
//!    `lottery_emission_share`, and the integer `FeeCurve` never panic, and
//!    `lottery_emission_share(h, reward) <= reward` (the miner keeps the
//!    remainder; the lottery can never take more than the whole reward).
//!
//! Every call below is the REAL consensus code path; the harness only feeds
//! adversarial inputs and checks the documented post-conditions.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use botho::block::calculate_block_reward;
use bth_cluster_tax::{
    demurrage::{FACTOR_SCALE, MAX_FACTOR_SCALED},
    demurrage_charge, ClusterFactorCurve, FeeCurve, MonetaryPolicy,
};

#[derive(Debug, Arbitrary)]
struct FuzzMath {
    // demurrage_charge inputs
    transfer_value: u64,
    cluster_factor: u64,
    elapsed_blocks: u64,
    rate_bps: u32,
    blocks_per_year: u64,

    // cluster-factor LUT / fee curve inputs
    cluster_wealth: u64,
    fee_amount: u64,

    // emission / reward inputs
    height: u64,
    total_supply: u64,
    reward: u64,
}

fuzz_target!(|m: FuzzMath| {
    // --- 1. Cluster-factor LUT (sigmoid) ---------------------------------
    let curve = ClusterFactorCurve::default_params();
    let sigmoid = curve.sigmoid_approx(m.cluster_wealth);
    assert!(
        sigmoid <= ClusterFactorCurve::SIGMOID_SCALE,
        "sigmoid_approx({}) = {} exceeds SIGMOID_SCALE {}",
        m.cluster_wealth,
        sigmoid,
        ClusterFactorCurve::SIGMOID_SCALE
    );

    let factor = curve.factor(m.cluster_wealth);
    // Documented bound for the default curve: 1x..=6x in FACTOR_SCALE units.
    let max_factor = curve.factor_max as u64 * ClusterFactorCurve::FACTOR_SCALE;
    let min_factor = curve.factor_min as u64 * ClusterFactorCurve::FACTOR_SCALE;
    assert!(
        factor >= min_factor && factor <= max_factor,
        "cluster factor {} out of documented bounds [{}, {}] for wealth {}",
        factor,
        min_factor,
        max_factor,
        m.cluster_wealth
    );

    // --- 2. Integer fee curve (sigmoid-based rate) ------------------------
    let fee_curve = FeeCurve::default_params();
    let rate = fee_curve.rate_bps(m.cluster_wealth);
    // Rate must stay within [r_min_bps, r_max_bps] (saturating construction).
    assert!(
        rate >= fee_curve.r_min_bps && rate <= fee_curve.r_max_bps,
        "fee rate {} out of [{}, {}]",
        rate,
        fee_curve.r_min_bps,
        fee_curve.r_max_bps
    );
    let (fee, remainder) = fee_curve.compute_fee(m.fee_amount, m.cluster_wealth);
    // Fee is taken out of the amount: fee + remainder == amount (no minting).
    assert!(
        fee <= m.fee_amount && remainder == m.fee_amount.saturating_sub(fee),
        "fee_curve.compute_fee minted value: amount={} fee={} remainder={}",
        m.fee_amount,
        fee,
        remainder
    );

    // --- 3. Demurrage charge ---------------------------------------------
    let charge = demurrage_charge(
        m.transfer_value,
        m.cluster_factor,
        m.elapsed_blocks,
        m.rate_bps,
        m.blocks_per_year,
    );
    // Factor-1 coins (or below FACTOR_SCALE, which clamps to 1x) are exempt.
    if m.cluster_factor <= FACTOR_SCALE
        || m.rate_bps == 0
        || m.elapsed_blocks == 0
        || m.blocks_per_year == 0
    {
        assert_eq!(
            charge, 0,
            "demurrage_charge must be 0 for exempt inputs (factor={}, rate={}, elapsed={}, bpy={})",
            m.cluster_factor, m.rate_bps, m.elapsed_blocks, m.blocks_per_year
        );
    }
    // The no-panic/no-overflow guarantee above (the call returned, and
    // `u64::try_from(...).unwrap_or(u64::MAX)` saturates) always holds for any
    // input and is the universal safety invariant.
    //
    // The tighter "charge <= transfer_value" property only holds in the
    // REALISTIC parameter domain. The in-horizon charge is
    //   value × rate_bps/10_000 × progressivity/5000 × elapsed/blocks_per_year
    // (demurrage.rs:78,88-93). With elapsed <= blocks_per_year the time
    // fraction is <= 1 and progressivity/5000 is <= 1 (factor clamps to
    // [FACTOR_SCALE, MAX_FACTOR_SCALED]), so the charge is bounded by
    //   value × rate_bps/10_000,
    // which is <= value iff rate_bps <= 10_000 (<=100%/yr). Real callers feed
    // MonetaryPolicy::demurrage_rate_bps (200 bps), never the absurd rates the
    // full-`u32` Arbitrary range produces, so we gate the value bound on that
    // documented domain. Outside it the charge legitimately exceeds the value
    // (e.g. a 200%/yr rate), and only the no-panic invariant applies.
    if m.blocks_per_year > 0
        && m.elapsed_blocks <= m.blocks_per_year
        && m.rate_bps <= 10_000
        && m.cluster_factor <= MAX_FACTOR_SCALED
    {
        assert!(
            charge <= m.transfer_value,
            "demurrage_charge {} exceeds transfer_value {} within one-year \
             horizon at realistic rate_bps={} factor={} (#333-class overflow?)",
            charge,
            m.transfer_value,
            m.rate_bps,
            m.cluster_factor
        );
    }

    // --- 4. Emission / reward schedule -----------------------------------
    // calculate_block_reward must never panic for any height/supply.
    let _reward = calculate_block_reward(m.height, m.total_supply);

    let policy = MonetaryPolicy::default();
    // Tail reward: pure u128-staged math, must not panic.
    let _tail = policy.calculate_tail_reward(m.total_supply);
    // Lottery emission share can never exceed the block reward it splits.
    let share = policy.lottery_emission_share(m.height, m.reward);
    assert!(
        share <= m.reward,
        "lottery_emission_share {} exceeds reward {} at height {} (#333-class)",
        share,
        m.reward,
        m.height
    );
    // The max factor constant is internally consistent with the curve bound.
    assert_eq!(MAX_FACTOR_SCALED, 6 * FACTOR_SCALE);
});
