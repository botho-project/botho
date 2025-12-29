//! Analysis tools for parameter tuning and attack economics.
//!
//! This module provides calculations for:
//! - Tag decay over multiple hops
//! - Wash trading break-even analysis
//! - Fee curve sensitivity
//! - Structuring attack economics

use crate::fee_curve::{FeeCurve, FeeRateBps};
use crate::tag::{TagWeight, TAG_WEIGHT_SCALE};

/// Calculate tag weight remaining after N hops with given decay rate.
///
/// Returns the fraction of original tag that remains (0.0 to 1.0).
pub fn tag_after_hops(decay_rate: TagWeight, hops: u32) -> f64 {
    let decay_fraction = decay_rate as f64 / TAG_WEIGHT_SCALE as f64;
    let retention = 1.0 - decay_fraction;
    retention.powi(hops as i32)
}

/// Calculate number of hops needed to reduce tag to target fraction.
///
/// Returns None if decay_rate is 0 (tags never decay).
pub fn hops_to_reach(decay_rate: TagWeight, target_fraction: f64) -> Option<u32> {
    if decay_rate == 0 {
        return None;
    }

    let decay_fraction = decay_rate as f64 / TAG_WEIGHT_SCALE as f64;
    let retention = 1.0 - decay_fraction;

    if retention <= 0.0 || retention >= 1.0 {
        return None;
    }

    // target = retention^n
    // n = log(target) / log(retention)
    let n = target_fraction.ln() / retention.ln();
    Some(n.ceil() as u32)
}

/// Wash trading break-even analysis.
///
/// Calculates whether wash trading (circular transfers to decay tags)
/// is profitable given current parameters.
#[derive(Debug, Clone)]
pub struct WashTradingAnalysis {
    /// Number of hops in the wash cycle
    pub hops: u32,

    /// Fee rate before wash trading (basis points)
    pub initial_rate_bps: FeeRateBps,

    /// Fee rate after wash trading (basis points)
    pub final_rate_bps: FeeRateBps,

    /// Total fees paid during wash cycle (as fraction of principal)
    pub total_fees_fraction: f64,

    /// Tag weight remaining after wash cycle (0.0 to 1.0)
    pub remaining_tag: f64,

    /// Future fee savings per transaction (as fraction)
    pub fee_savings_per_tx: f64,

    /// Number of future transactions needed to break even
    pub break_even_transactions: Option<u32>,
}

/// Analyze wash trading economics.
///
/// Given a starting cluster wealth and decay rate, calculate whether
/// wash trading through N hops is profitable.
pub fn analyze_wash_trading(
    cluster_wealth: u64,
    decay_rate: TagWeight,
    hops: u32,
    fee_curve: &FeeCurve,
) -> WashTradingAnalysis {
    // Initial fee rate based on cluster wealth
    let initial_rate = fee_curve.rate_bps(cluster_wealth);

    // Tag remaining after N hops
    let remaining_tag = tag_after_hops(decay_rate, hops);

    // Approximate new cluster wealth (assuming our portion is negligible to total)
    // In reality this is more complex, but for analysis purposes:
    let effective_new_wealth = (cluster_wealth as f64 * remaining_tag) as u64;
    let final_rate = fee_curve.rate_bps(effective_new_wealth);

    // Total fees paid: sum of fees at each hop
    // Simplified: assume constant rate (actual rate decreases as tags decay)
    let avg_rate = (initial_rate + final_rate) / 2;
    let total_fees_fraction = (avg_rate as f64 / 10_000.0) * hops as f64;

    // Fee savings per future transaction
    let fee_savings_per_tx = (initial_rate as f64 - final_rate as f64) / 10_000.0;

    // Break-even: how many transactions until savings exceed cost?
    let break_even_transactions = if fee_savings_per_tx > 0.0 {
        let n = total_fees_fraction / fee_savings_per_tx;
        Some(n.ceil() as u32)
    } else {
        None // Never breaks even
    };

    WashTradingAnalysis {
        hops,
        initial_rate_bps: initial_rate,
        final_rate_bps: final_rate,
        total_fees_fraction,
        remaining_tag,
        fee_savings_per_tx,
        break_even_transactions,
    }
}

/// Fee curve parameter sensitivity analysis.
#[derive(Debug, Clone)]
pub struct FeeCurveSensitivity {
    /// Wealth levels sampled
    pub wealth_levels: Vec<u64>,

    /// Corresponding fee rates (basis points)
    pub fee_rates: Vec<FeeRateBps>,

    /// Marginal rate change per unit wealth
    pub marginal_rates: Vec<f64>,
}

/// Analyze fee curve behavior across wealth levels.
pub fn analyze_fee_curve(fee_curve: &FeeCurve, num_samples: usize) -> FeeCurveSensitivity {
    // Sample from 0 to 10x the midpoint
    let max_wealth = fee_curve.w_mid * 10;
    let step = max_wealth / num_samples as u64;

    let mut wealth_levels = Vec::with_capacity(num_samples);
    let mut fee_rates = Vec::with_capacity(num_samples);
    let mut marginal_rates = Vec::with_capacity(num_samples);

    let mut prev_rate = 0u32;
    let mut prev_wealth = 0u64;

    for i in 0..num_samples {
        let wealth = step * (i as u64 + 1);
        let rate = fee_curve.rate_bps(wealth);

        wealth_levels.push(wealth);
        fee_rates.push(rate);

        if i > 0 && wealth > prev_wealth {
            let marginal = (rate as f64 - prev_rate as f64) / (wealth - prev_wealth) as f64;
            marginal_rates.push(marginal);
        } else {
            marginal_rates.push(0.0);
        }

        prev_rate = rate;
        prev_wealth = wealth;
    }

    FeeCurveSensitivity {
        wealth_levels,
        fee_rates,
        marginal_rates,
    }
}

/// Calculate effective fee for a transfer given cluster composition.
///
/// Takes a list of (cluster_wealth, tag_weight) pairs representing
/// the sender's cluster attribution.
pub fn effective_fee_rate(
    cluster_weights: &[(u64, TagWeight)],
    background_weight: TagWeight,
    fee_curve: &FeeCurve,
) -> FeeRateBps {
    let mut weighted_rate: u64 = 0;
    let mut total_weight: u64 = 0;

    for &(cluster_wealth, weight) in cluster_weights {
        let rate = fee_curve.rate_bps(cluster_wealth) as u64;
        weighted_rate += rate * weight as u64;
        total_weight += weight as u64;
    }

    // Add background contribution
    weighted_rate += fee_curve.background_rate_bps as u64 * background_weight as u64;
    total_weight += background_weight as u64;

    if total_weight == 0 {
        return fee_curve.background_rate_bps;
    }

    (weighted_rate / total_weight) as FeeRateBps
}

/// Structuring attack analysis.
///
/// Compares fee for single large transfer vs. multiple smaller transfers.
#[derive(Debug, Clone)]
pub struct StructuringAnalysis {
    /// Single transfer amount
    pub single_amount: u64,

    /// Fee for single transfer
    pub single_fee: u64,

    /// Number of split transfers
    pub num_splits: u32,

    /// Amount per split transfer
    pub split_amount: u64,

    /// Total fee for all split transfers
    pub total_split_fees: u64,

    /// Savings from structuring (negative means structuring costs more)
    pub savings: i64,
}

/// Analyze whether structuring (splitting transfers) reduces fees.
///
/// In a properly designed system, this should show minimal or negative savings.
pub fn analyze_structuring(
    amount: u64,
    cluster_wealth: u64,
    num_splits: u32,
    fee_curve: &FeeCurve,
) -> StructuringAnalysis {
    // Single transfer fee
    let single_rate = fee_curve.rate_bps(cluster_wealth);
    let single_fee = (amount as u128 * single_rate as u128 / 10_000) as u64;

    // Split transfer fees
    // Key insight: the fee rate is based on cluster wealth, not transfer size
    // So splitting doesn't help if cluster wealth stays the same
    let split_amount = amount / num_splits as u64;
    let split_rate = fee_curve.rate_bps(cluster_wealth); // Same rate!
    let per_split_fee = (split_amount as u128 * split_rate as u128 / 10_000) as u64;
    let total_split_fees = per_split_fee * num_splits as u64;

    // Account for remainder
    let remainder = amount - (split_amount * num_splits as u64);
    let remainder_fee = (remainder as u128 * split_rate as u128 / 10_000) as u64;
    let total_split_fees = total_split_fees + remainder_fee;

    let savings = single_fee as i64 - total_split_fees as i64;

    StructuringAnalysis {
        single_amount: amount,
        single_fee,
        num_splits,
        split_amount,
        total_split_fees,
        savings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_decay_calculation() {
        // 5% decay per hop
        let decay_rate = 50_000; // 5% in our scale

        // After 1 hop: 95% remaining
        let remaining = tag_after_hops(decay_rate, 1);
        assert!((remaining - 0.95).abs() < 0.001);

        // After 14 hops: should be roughly half (0.95^14 ≈ 0.488)
        let remaining = tag_after_hops(decay_rate, 14);
        assert!((remaining - 0.488).abs() < 0.01);
    }

    #[test]
    fn test_hops_to_halve() {
        // 5% decay per hop
        let decay_rate = 50_000;

        // Should take about 14 hops to halve
        let hops = hops_to_reach(decay_rate, 0.5).unwrap();
        assert!(hops >= 13 && hops <= 15, "Expected ~14 hops, got {hops}");
    }

    #[test]
    fn test_structuring_no_benefit() {
        let fee_curve = FeeCurve::default_params();
        let cluster_wealth = 50_000_000; // Above midpoint

        let analysis = analyze_structuring(1_000_000, cluster_wealth, 10, &fee_curve);

        // Splitting should not save money (might lose a bit due to rounding)
        assert!(
            analysis.savings <= 0,
            "Structuring should not save money: savings = {}",
            analysis.savings
        );
    }

    #[test]
    fn test_wash_trading_analysis() {
        let fee_curve = FeeCurve::default_params();
        let cluster_wealth = 100_000_000; // Large cluster

        let analysis = analyze_wash_trading(cluster_wealth, 50_000, 20, &fee_curve);

        // Should require many transactions to break even (if ever)
        assert!(
            analysis.break_even_transactions.map(|n| n > 10).unwrap_or(true),
            "Wash trading should not be easily profitable"
        );
    }
}
