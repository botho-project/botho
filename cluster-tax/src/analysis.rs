//! Analysis tools for parameter tuning and attack economics.
//!
//! This module provides calculations for:
//! - Tag decay over multiple hops
//! - Wash trading break-even analysis
//! - Fee curve sensitivity
//! - Structuring attack economics
//!
//! # Fee models (#1695)
//!
//! The `*_consensus` analyses price every transaction with the shared
//! [`crate::consensus_fee_model`] — the node's consensus fee floor (size ×
//! factor × min(n,10)² + demurrage), optionally with the relay `MIN_TX_FEE`.
//! The original [`analyze_wash_trading`] / [`analyze_structuring`] price a
//! linear bps [`FeeCurve`] on transfer value, which the node has never
//! enforced; they are kept (not deleted) as the **legacy** model, reachable
//! from the CLI only via `--fee-model legacy`.

use crate::{
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, ClaimedClass, ConsensusFeeInputs,
        ConsensusFeeParams,
    },
    demurrage::demurrage_charge,
    fee_curve::{ClusterFactorCurve, FeeCurve, FeeRateBps},
    tag::{TagWeight, TAG_WEIGHT_SCALE},
};

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

// ============================================================================
// Consensus-fee-model analyses (#1695)
// ============================================================================

/// Wash-trading economics under the node's consensus fee floor.
///
/// Each hop is a 1-input / 1-output self-transfer of the whole principal at
/// zero ring age (the cheapest shape for the attacker). Tag decay lowers the
/// output's claimed cluster wealth below the input's, so each hop is a class
/// **downgrade** and pays the capitalized reset charge (#925) once demurrage is
/// active — exactly as `Ledger::consensus_fee_floor` prices it. The benefit of
/// washing is (a) a lower size fee on every future transaction and (b) lower
/// accrued demurrage on the held principal.
#[derive(Debug, Clone)]
pub struct ConsensusWashTradingAnalysis {
    /// Number of wash hops.
    pub hops: u32,
    /// Cluster factor before washing (`FACTOR_SCALE`, 1000 = 1×).
    pub initial_factor: u64,
    /// Cluster factor after washing.
    pub final_factor: u64,
    /// Tag weight remaining after the hops (0.0 to 1.0).
    pub remaining_tag: f64,
    /// Σ size/output fees over the hops (pico).
    pub hop_base_fees_pico: u128,
    /// Σ demurrage (capitalized downgrade) charges over the hops (pico).
    pub hop_demurrage_pico: u128,
    /// Total wash cost under the selected mode (pico).
    pub total_wash_fees_pico: u128,
    /// Total wash cost as a fraction of the principal.
    pub total_fees_fraction: f64,
    /// Fee saved on each future 1-in/2-out transfer (pico, under the mode).
    pub fee_savings_per_tx_pico: i128,
    /// Accrued demurrage saved per year of holding the principal (pico).
    pub demurrage_savings_per_year_pico: u128,
    /// Future transfers until the per-tx fee savings repay the wash.
    pub break_even_transactions: Option<u128>,
    /// Years of holding until the demurrage savings repay the wash.
    pub break_even_years: Option<f64>,
}

fn effective_wealth(cluster_wealth_pico: u128, remaining: f64) -> u128 {
    (cluster_wealth_pico as f64 * remaining) as u128
}

/// Analyze wash trading with the shared consensus fee model.
///
/// `cluster_wealth_pico` is the cluster's global wealth; the attacker's
/// effective wealth after `k` hops is `cluster_wealth × (1 − decay)^k`, the
/// same approximation as the legacy [`analyze_wash_trading`].
pub fn analyze_wash_trading_consensus(
    cluster_wealth_pico: u128,
    principal_pico: u64,
    decay_rate: TagWeight,
    hops: u32,
    height: u64,
    params: &ConsensusFeeParams,
) -> ConsensusWashTradingAnalysis {
    let curve = ClusterFactorCurve::default();
    let mut hop_base: u128 = 0;
    let mut hop_dem: u128 = 0;
    let mut total: u128 = 0;
    for k in 0..hops {
        let w_in = effective_wealth(cluster_wealth_pico, tag_after_hops(decay_rate, k));
        let w_out = effective_wealth(cluster_wealth_pico, tag_after_hops(decay_rate, k + 1));
        let b = consensus_fee_breakdown(
            &ConsensusFeeInputs {
                size: consensus_tx_size_bytes(1, 1, 0),
                n_out: 1,
                n_memo: 0,
                claimed: ClaimedClass::Wealth(w_out),
                floor: curve.factor(w_in),
                elapsed: 0,
                height,
                output_sum: principal_pico,
            },
            params,
        );
        hop_base += b.base as u128;
        hop_dem += b.demurrage as u128;
        total += b.fee as u128;
    }

    let remaining_tag = tag_after_hops(decay_rate, hops);
    let w0 = cluster_wealth_pico;
    let w_n = effective_wealth(cluster_wealth_pico, remaining_tag);
    let f0 = curve.factor(w0);
    let f_n = curve.factor(w_n);

    // A typical future transfer: 1-in/2-out, in-class, young.
    let future_fee = |w: u128| {
        consensus_fee_breakdown(
            &ConsensusFeeInputs {
                size: consensus_tx_size_bytes(1, 2, 0),
                n_out: 2,
                n_memo: 0,
                claimed: ClaimedClass::Wealth(w),
                floor: curve.factor(w),
                elapsed: 0,
                height,
                output_sum: principal_pico,
            },
            params,
        )
        .fee as i128
    };
    let fee_savings_per_tx_pico = future_fee(w0) - future_fee(w_n);

    let rate = params.demurrage_rate_bps(height);
    let bpy = params.blocks_per_year;
    let demurrage_savings_per_year_pico = (demurrage_charge(principal_pico, f0, bpy, rate, bpy)
        as u128)
        .saturating_sub(demurrage_charge(principal_pico, f_n, bpy, rate, bpy) as u128);

    let break_even_transactions = if fee_savings_per_tx_pico > 0 {
        Some(total.div_ceil(fee_savings_per_tx_pico as u128))
    } else {
        None
    };
    let break_even_years = if demurrage_savings_per_year_pico > 0 {
        Some(total as f64 / demurrage_savings_per_year_pico as f64)
    } else {
        None
    };

    ConsensusWashTradingAnalysis {
        hops,
        initial_factor: f0,
        final_factor: f_n,
        remaining_tag,
        hop_base_fees_pico: hop_base,
        hop_demurrage_pico: hop_dem,
        total_wash_fees_pico: total,
        total_fees_fraction: if principal_pico > 0 {
            total as f64 / principal_pico as f64
        } else {
            0.0
        },
        fee_savings_per_tx_pico,
        demurrage_savings_per_year_pico,
        break_even_transactions,
        break_even_years,
    }
}

/// Structuring economics under the node's consensus fee floor.
#[derive(Debug, Clone)]
pub struct ConsensusStructuringAnalysis {
    /// Amount moved (pico).
    pub single_amount_pico: u64,
    /// Number of pieces.
    pub num_splits: u32,
    /// Fee for one 1-in/2-out transfer of the whole amount (pico).
    pub single_fee_pico: u64,
    /// Total fee for `num_splits` separate 1-in/2-out transfers (pico).
    pub split_txs_fee_pico: u128,
    /// Fee for one transaction with `num_splits` payment outputs + change.
    pub split_outputs_fee_pico: u64,
    /// `single − split_txs` (positive = structuring saves money).
    pub savings_separate_txs_pico: i128,
    /// `single − split_outputs` (positive = structuring saves money).
    pub savings_one_tx_pico: i128,
}

/// Analyze structuring with the shared consensus fee model: does splitting a
/// transfer into `num_splits` pieces — as separate transactions, or as one
/// many-output transaction — lower the fee? `elapsed` is the coins' ring age
/// (demurrage is value-proportional, so it is the same in total either way).
pub fn analyze_structuring_consensus(
    amount_pico: u64,
    cluster_wealth_pico: u128,
    num_splits: u32,
    elapsed: u64,
    height: u64,
    params: &ConsensusFeeParams,
) -> ConsensusStructuringAnalysis {
    let curve = ClusterFactorCurve::default();
    let factor = curve.factor(cluster_wealth_pico);
    let fee = |n_out: usize, value: u64| {
        consensus_fee_breakdown(
            &ConsensusFeeInputs {
                size: consensus_tx_size_bytes(1, n_out, 0),
                n_out,
                n_memo: 0,
                claimed: ClaimedClass::Wealth(cluster_wealth_pico),
                floor: factor,
                elapsed,
                height,
                output_sum: value,
            },
            params,
        )
        .fee
    };
    let splits = num_splits.max(1);
    let single_fee_pico = fee(2, amount_pico);
    let piece = amount_pico / splits as u64;
    let remainder = amount_pico - piece * splits as u64;
    let mut split_txs_fee_pico: u128 = 0;
    for i in 0..splits {
        let v = if i == 0 { piece + remainder } else { piece };
        split_txs_fee_pico += fee(2, v) as u128;
    }
    let split_outputs_fee_pico = fee(splits as usize + 1, amount_pico);
    ConsensusStructuringAnalysis {
        single_amount_pico: amount_pico,
        num_splits: splits,
        single_fee_pico,
        split_txs_fee_pico,
        split_outputs_fee_pico,
        savings_separate_txs_pico: single_fee_pico as i128 - split_txs_fee_pico as i128,
        savings_one_tx_pico: single_fee_pico as i128 - split_outputs_fee_pico as i128,
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
        assert!((13..=15).contains(&hops), "Expected ~14 hops, got {hops}");
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
            analysis
                .break_even_transactions
                .map(|n| n > 10)
                .unwrap_or(true),
            "Wash trading should not be easily profitable"
        );
    }
    #[test]
    fn consensus_structuring_never_saves() {
        use crate::{consensus_fee::FeeModelMode, fee_curve::PICO_PER_BTH};
        for mode in [
            FeeModelMode::ConsensusOnly,
            FeeModelMode::ConsensusPlusRelayMin,
        ] {
            let p = ConsensusFeeParams::mainnet(mode);
            for splits in [1u32, 2, 10, 50] {
                let a = analyze_structuring_consensus(
                    1_000_000 * PICO_PER_BTH as u64,
                    10_000_000 * PICO_PER_BTH,
                    splits,
                    1_000_000,
                    p.first_demurrage_height(),
                    &p,
                );
                assert!(a.savings_separate_txs_pico <= 1, "{mode:?} {splits}: {a:?}");
                assert!(a.savings_one_tx_pico <= 0, "{mode:?} {splits}: {a:?}");
            }
        }
    }

    #[test]
    fn consensus_wash_prices_downgrade_when_demurrage_active() {
        use crate::{consensus_fee::FeeModelMode, fee_curve::PICO_PER_BTH};
        let p = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let w = 10_000_000 * PICO_PER_BTH;
        let principal = 1_000_000 * PICO_PER_BTH as u64;
        let active = analyze_wash_trading_consensus(
            w,
            principal,
            50_000,
            20,
            p.first_demurrage_height(),
            &p,
        );
        assert!(active.final_factor < active.initial_factor);
        assert!(active.hop_demurrage_pico > 0, "{active:?}");
        // The capitalized charge prices ~SETTLEMENT_HORIZON years of saved
        // demurrage, so washing cannot repay in under a year.
        assert!(active.break_even_years.unwrap() > 1.0, "{active:?}");
        let epoch0 = analyze_wash_trading_consensus(w, principal, 50_000, 20, 1, &p);
        assert_eq!(epoch0.hop_demurrage_pico, 0);
    }
}
