//! Cluster demurrage: a holding charge on wealthy-cluster coins, paid at
//! spend time.
//!
//! Demurrage is the stock-level component of the cluster-tilted
//! redistribution design (docs/design/cluster-tilted-redistribution.md).
//! Transaction fees are a consumption tax and cannot touch idle wealth; the
//! emission-fraction sweep showed the mechanism passes its Δgini > 0.05
//! criterion at miner-viable emission fractions only with demurrage active.
//!
//! ## Mechanism
//!
//! ```text
//! charge = value × rate × (factor − 1)/(max_factor − 1) × elapsed / blocks_per_year
//! ```
//!
//! - `factor` is the cluster factor already used for progressive fees
//!   (1.0x..6.0x in FACTOR_SCALE units). Factor-1 (background/commerce) coins
//!   pay exactly zero — demurrage only binds wealthy clusters.
//! - `elapsed` is how long the spent coins sat idle. Under ring signatures the
//!   real input is hidden, so the elapsed value is the value-weighted centroid
//!   of the PUBLIC creation heights of all ring members: a large real input
//!   dominates its own ring's centroid, while the factor term protects small
//!   spenders from old decoys entirely.
//! - The charge is added to the transaction's minimum fee and flows through the
//!   standard fee split into the redistribution lottery pool.
//!
//! ## Why churning doesn't escape
//!
//! Spending resets a coin's creation height — but the spend pays the
//! accrued charge first. Total demurrage paid over any holding period is
//! invariant to churn frequency; churning just adds transaction fees on
//! top (which also feed the lottery pool).
//!
//! ## Determinism
//!
//! CONSENSUS-CRITICAL: pure integer arithmetic throughout.

/// Fixed-point scale for cluster factors (1000 = 1.0x, 6000 = 6.0x).
/// Matches `ClusterFactorCurve::FACTOR_SCALE` and the lottery.
pub const FACTOR_SCALE: u64 = 1000;

/// Maximum cluster factor in FACTOR_SCALE units.
pub const MAX_FACTOR_SCALED: u64 = 6 * FACTOR_SCALE;

/// Compute the demurrage charge for a spend.
///
/// # Arguments
/// * `transfer_value` - Total value moved (sum of output amounts)
/// * `cluster_factor` - Cluster factor in FACTOR_SCALE units (1000..=6000),
///   from the same computation the progressive fee uses
/// * `elapsed_blocks` - Holding duration in blocks (value-weighted ring
///   centroid of public UTXO creation heights)
/// * `rate_bps` - Annual demurrage rate at maximum factor, in basis points (200
///   = 2%/year); use `MonetaryPolicy::demurrage_rate_bps(height)`
/// * `blocks_per_year` - Blocks per year at the policy's assumed block time
///
/// # Returns
/// The charge in base units. Zero for factor-1 coins, zero rate, or zero
/// elapsed time.
pub fn demurrage_charge(
    transfer_value: u64,
    cluster_factor: u64,
    elapsed_blocks: u64,
    rate_bps: u32,
    blocks_per_year: u64,
) -> u64 {
    if rate_bps == 0 || elapsed_blocks == 0 || blocks_per_year == 0 {
        return 0;
    }

    let factor = cluster_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
    // Progressivity in FACTOR_SCALE units: 0 at factor 1.0, 5000 at factor 6.0
    let progressivity = factor - FACTOR_SCALE;
    if progressivity == 0 {
        return 0;
    }

    // charge = value × rate_bps/10_000 × progressivity/5000 ×
    // elapsed/blocks_per_year
    //
    // Multiply before dividing to preserve precision; u128 bounds:
    // value (2^64) × rate_bps (2^14) × progressivity (2^13) ≈ 2^91, then
    // × elapsed (2^64) would overflow — so fold elapsed/blocks_per_year in
    // a separate u128 stage with a precision scale.
    const TIME_SCALE: u128 = 1_000_000;
    let time_fraction = (elapsed_blocks as u128 * TIME_SCALE) / blocks_per_year as u128;

    let charge = transfer_value as u128 * rate_bps as u128 * progressivity as u128
        / 10_000
        / (MAX_FACTOR_SCALED - FACTOR_SCALE) as u128;
    // Saturating: absurd elapsed × rate combinations clamp to u64::MAX
    // rather than panic (the charge is bounded by the balance check anyway)
    let charge = charge.saturating_mul(time_fraction) / TIME_SCALE;

    u64::try_from(charge).unwrap_or(u64::MAX)
}

/// Compute the value-weighted elapsed-blocks centroid for a set of ring
/// members.
///
/// # Arguments
/// * `members` - (value, creation_height) for each ring member (public data)
/// * `current_height` - The validating block height
///
/// # Returns
/// Value-weighted average of `current_height − creation_height` across
/// members, or 0 if the set is empty or has zero total value.
pub fn ring_elapsed_centroid(members: &[(u64, u64)], current_height: u64) -> u64 {
    let mut weighted: u128 = 0;
    let mut total_value: u128 = 0;

    for &(value, creation_height) in members {
        let elapsed = current_height.saturating_sub(creation_height);
        weighted += value as u128 * elapsed as u128;
        total_value += value as u128;
    }

    if total_value == 0 {
        return 0;
    }

    (weighted / total_value) as u64
}

/// Compute the cluster factor implied by the value-weighted centroid of a
/// ring's own cluster wealth.
///
/// This is the wealth/factor analog of [`ring_elapsed_centroid`]: where that
/// function takes the value-weighted centroid of ring members' public creation
/// AGES, this takes the value-weighted centroid of their effective cluster
/// WEALTH and maps it through the progressive fee curve.
///
/// # Why this exists (audit cycle 6 H2, design #574 item B2)
///
/// The cluster factor that drives the demurrage charge (and the progressive
/// fee) is otherwise derived from the transaction's spender-authored OUTPUT
/// tags. A wealthy spender can tag every output as "background" (factor 1x) and
/// pay ~zero demurrage, even while spending coins that inherited a wealthy
/// cluster's tags — picking fresh background-tagged decoys to drag the implied
/// factor toward 1.
///
/// The ring members' tags, by contrast, are public chain state the spender
/// cannot rewrite. The real input is one of the ring members and carries its
/// inherited (wealthy) tags, so the factor the ring composition implies is a
/// floor the spender cannot claim below. There is no free or empirical
/// parameter: the floor is a pure function of public ring data and the chain's
/// per-cluster wealth.
///
/// # Arguments
/// * `members` - `(value, member_effective_cluster_wealth)` for each ring
///   member. `member_effective_cluster_wealth` is the member's value-normalized
///   cluster wealth (`Σ_tag weight × W_global / TAG_WEIGHT_SCALE`), resolved by
///   the caller from public per-cluster wealth state.
/// * `curve` - The progressive cluster factor curve.
///
/// # Returns
/// The implied cluster factor in FACTOR_SCALE units (1000 = 1x .. 6000 = 6x).
/// Background-only rings (zero centroid wealth) imply exactly the 1x floor.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic, ordered-slice input, no
/// HashMap/HashSet iteration order, no node-local state. Safe for the consensus
/// fee-floor enforcement (item B4) to reuse unchanged.
pub fn ring_centroid_implied_factor(
    members: &[(u64, u64)],
    curve: &crate::fee_curve::ClusterFactorCurve,
) -> u64 {
    let mut weighted: u128 = 0;
    let mut total_value: u128 = 0;

    for &(value, member_wealth) in members {
        weighted += value as u128 * member_wealth as u128;
        total_value += value as u128;
    }

    let centroid_wealth = if total_value == 0 {
        0
    } else {
        (weighted / total_value).min(u64::MAX as u128) as u64
    };

    curve.factor(centroid_wealth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fee_curve::ClusterFactorCurve;

    const BLOCKS_PER_YEAR: u64 = 6_307_200; // 5s blocks

    #[test]
    fn test_factor_one_pays_zero() {
        // Background/commerce coins are exempt regardless of age or value
        assert_eq!(
            demurrage_charge(
                u64::MAX,
                FACTOR_SCALE,
                BLOCKS_PER_YEAR * 10,
                200,
                BLOCKS_PER_YEAR
            ),
            0
        );
    }

    #[test]
    fn test_max_factor_full_rate() {
        // Factor 6.0, one year: exactly rate_bps of value
        // 1M × 2% = 20_000
        assert_eq!(
            demurrage_charge(
                1_000_000,
                MAX_FACTOR_SCALED,
                BLOCKS_PER_YEAR,
                200,
                BLOCKS_PER_YEAR
            ),
            20_000
        );
    }

    #[test]
    fn test_linear_in_factor_time_and_value() {
        let full = demurrage_charge(
            1_000_000,
            MAX_FACTOR_SCALED,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        );

        // Half the progressivity range: factor 3.5 → (3500-1000)/5000 = 1/2
        let half_factor = demurrage_charge(1_000_000, 3_500, BLOCKS_PER_YEAR, 200, BLOCKS_PER_YEAR);
        assert_eq!(half_factor, full / 2);

        // Half the time
        let half_time = demurrage_charge(
            1_000_000,
            MAX_FACTOR_SCALED,
            BLOCKS_PER_YEAR / 2,
            200,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(half_time, full / 2);

        // Double the value
        let double_value = demurrage_charge(
            2_000_000,
            MAX_FACTOR_SCALED,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(double_value, full * 2);
    }

    #[test]
    fn test_zero_inputs() {
        assert_eq!(
            demurrage_charge(1_000_000, 6_000, 0, 200, BLOCKS_PER_YEAR),
            0
        );
        assert_eq!(
            demurrage_charge(1_000_000, 6_000, 1000, 0, BLOCKS_PER_YEAR),
            0
        );
        assert_eq!(demurrage_charge(1_000_000, 6_000, 1000, 200, 0), 0);
        assert_eq!(demurrage_charge(0, 6_000, 1000, 200, BLOCKS_PER_YEAR), 0);
    }

    #[test]
    fn test_no_overflow_at_extremes() {
        // Must not panic; saturates at u64::MAX
        let charge = demurrage_charge(u64::MAX, u64::MAX, u64::MAX, u32::MAX, 1);
        assert!(charge > 0);
    }

    #[test]
    fn test_ring_centroid_value_weighted() {
        // A whale's own large input dominates its ring centroid: small fresh
        // decoys barely reduce the elapsed value.
        let current = 1_000_000u64;
        let members = [
            (100_000_000, 0), // real input: 100M, 1M blocks old
            (1_000, current), // fresh small decoy
            (1_000, current), // fresh small decoy
        ];
        let elapsed = ring_elapsed_centroid(&members, current);
        // 100M×1M / 100.002M ≈ 999_980
        assert!(elapsed > 999_900, "elapsed = {elapsed}");

        // Conversely, a small spender with an old whale decoy gets a large
        // centroid — but pays zero anyway via the factor term (see
        // test_factor_one_pays_zero).
    }

    #[test]
    fn test_ring_centroid_empty_and_zero_value() {
        assert_eq!(ring_elapsed_centroid(&[], 1000), 0);
        assert_eq!(ring_elapsed_centroid(&[(0, 0)], 1000), 0);
    }

    #[test]
    fn test_ring_centroid_implied_factor_wealthy_ring() {
        // Every member carries a wealthy cluster's wealth -> high implied factor.
        let curve = ClusterFactorCurve::default_params();
        // (value, member_effective_wealth)
        let members = [(1_000_000u64, 100_000_000u64), (1_000_000, 100_000_000)];
        let implied = ring_centroid_implied_factor(&members, &curve);
        // Wealthy centroid maps near the curve maximum, well above 1x.
        assert!(implied >= 5_000, "wealthy ring implied factor = {implied}");
        assert_eq!(implied, curve.factor(100_000_000));
    }

    #[test]
    fn test_ring_centroid_implied_factor_background_ring() {
        // Zero member wealth (background ring) -> exactly the 1x floor.
        let curve = ClusterFactorCurve::default_params();
        let members = [(1_000_000u64, 0u64), (2_000_000, 0)];
        let implied = ring_centroid_implied_factor(&members, &curve);
        assert_eq!(implied, curve.factor(0));
        assert_eq!(implied, FACTOR_SCALE); // 1x
    }

    #[test]
    fn test_ring_centroid_implied_factor_value_weighted() {
        // A large-value wealthy member dominates the centroid; a small fresh
        // background decoy barely moves it.
        let curve = ClusterFactorCurve::default_params();
        let members = [
            (100_000_000u64, 100_000_000u64), // big wealthy real input
            (1_000, 0),                       // tiny fresh background decoy
        ];
        let implied = ring_centroid_implied_factor(&members, &curve);
        // Centroid wealth ~ 100M × 100M / 100.001M ≈ 99.999M -> still high.
        assert!(
            implied >= 5_000,
            "value-weighted implied factor = {implied}"
        );
    }

    #[test]
    fn test_ring_centroid_implied_factor_empty() {
        let curve = ClusterFactorCurve::default_params();
        assert_eq!(ring_centroid_implied_factor(&[], &curve), curve.factor(0));
    }

    #[test]
    fn test_ring_centroid_floor_changes_demurrage_outcome() {
        // End-to-end: a background-claimed factor pays zero demurrage, but the
        // ring-floored factor produces a real charge. The spender can no longer
        // escape demurrage by understating the output tags.
        let curve = ClusterFactorCurve::default_params();
        let members = [(1_000_000u64, 100_000_000u64), (1_000_000, 100_000_000)];

        let claimed_factor = FACTOR_SCALE; // 1x background claim
        let charge_claimed = demurrage_charge(
            1_000_000,
            claimed_factor,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(charge_claimed, 0, "background claim pays no demurrage");

        let implied = ring_centroid_implied_factor(&members, &curve);
        let floored = claimed_factor.max(implied);
        let charge_floored =
            demurrage_charge(1_000_000, floored, BLOCKS_PER_YEAR, 200, BLOCKS_PER_YEAR);
        assert!(
            charge_floored > 0,
            "ring-floored factor must produce a real demurrage charge: {charge_floored}"
        );
    }

    #[test]
    fn test_churn_invariance() {
        // Paying demurrage at every churn sums to the same total as paying
        // once at the end: charge(T) = charge(T/2) + charge(T/2).
        let one_year = demurrage_charge(1_000_000, 6_000, BLOCKS_PER_YEAR, 200, BLOCKS_PER_YEAR);
        let half = demurrage_charge(1_000_000, 6_000, BLOCKS_PER_YEAR / 2, 200, BLOCKS_PER_YEAR);
        assert_eq!(one_year, half * 2);
    }
}
