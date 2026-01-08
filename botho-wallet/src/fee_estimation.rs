//! Progressive Fee Estimation
//!
//! Estimates transaction fees based on the cluster-tax progressive fee model.
//! Takes into account:
//! - Cluster factor from blended input UTXO tags
//! - Estimated transaction size
//! - Superlinear output penalty
//! - Dynamic base fee from network
//!
//! # Dynamic Fee Rate
//!
//! The base fee rate is fetched from the network via the `fee_getRate` RPC
//! method. Wallets should periodically refresh this rate using
//! [`CachedFeeRate`] to ensure accurate fee estimation based on current network
//! conditions.
//!
//! ```no_run
//! use botho_wallet::fee_estimation::{CachedFeeRate, FeeEstimator};
//!
//! // Refresh rate from network
//! let cached_rate = CachedFeeRate::default();
//! // ... fetch from rpc_pool.get_fee_rate() ...
//!
//! // Update estimator before calculating fees
//! let mut estimator = FeeEstimator::new();
//! if let Some(rate) = cached_rate.rate() {
//!     estimator.set_base_rate(rate);
//! }
//! ```

use bth_cluster_tax::{ClusterId, FeeConfig, TagVector, TagWeight, TAG_WEIGHT_SCALE};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Estimated size of a 2-input, 2-output CLSAG transaction in bytes.
///
/// CLSAG signature components (per input, ring size 11):
/// - Key image: 32 bytes
/// - c0: 32 bytes
/// - s values: 11 * 32 = 352 bytes
/// - Total per input: ~416 bytes
///
/// Transaction overhead:
/// - Version/prefix: 4 bytes
/// - Fee: 8 bytes
/// - Outputs: 2 * (amount + keys + range proof) ≈ 2 * 2500 = 5000 bytes
/// - Input references: 2 * 36 = 72 bytes
/// - Signatures: 2 * 416 = 832 bytes
///
/// Total estimate: ~6000 bytes for a typical 2-in/2-out transaction.
pub const ESTIMATED_2IN_2OUT_SIZE: usize = 6000;

/// Size overhead per additional input (CLSAG signature).
pub const SIZE_PER_ADDITIONAL_INPUT: usize = 416;

/// Size overhead per additional output.
pub const SIZE_PER_ADDITIONAL_OUTPUT: usize = 2500;

/// Default TTL for cached fee rate (60 seconds).
pub const DEFAULT_FEE_RATE_TTL: Duration = Duration::from_secs(60);

/// Minimum base rate (fallback when network is unavailable).
pub const MINIMUM_BASE_RATE: u64 = 1;

/// Cached network fee rate with TTL-based expiration.
///
/// Provides a thread-safe cache for the network fee rate with automatic
/// expiration. When the cache expires, the [`rate()`] method returns `None`,
/// signaling that a refresh is needed.
///
/// # Example
///
/// ```no_run
/// use botho_wallet::fee_estimation::CachedFeeRate;
///
/// let mut cache = CachedFeeRate::default();
///
/// // After fetching from network:
/// cache.update(5); // 5 nanoBTH/byte
///
/// // Get rate (returns None if expired)
/// if let Some(rate) = cache.rate() {
///     println!("Using cached rate: {} nanoBTH/byte", rate);
/// } else {
///     println!("Cache expired, refresh needed");
/// }
///
/// // Always get a rate with fallback
/// let rate = cache.rate_or_default();
/// ```
#[derive(Debug, Clone)]
pub struct CachedFeeRate {
    /// Current base rate in nanoBTH per byte.
    base_rate: u64,

    /// Time when this rate was last updated.
    last_updated: Option<Instant>,

    /// Time-to-live for the cached rate.
    ttl: Duration,

    /// Additional network congestion info (for display purposes).
    congestion: f64,

    /// Whether dynamic adjustment is active on the network.
    adjustment_active: bool,
}

impl Default for CachedFeeRate {
    fn default() -> Self {
        Self {
            base_rate: MINIMUM_BASE_RATE,
            last_updated: None,
            ttl: DEFAULT_FEE_RATE_TTL,
            congestion: 0.0,
            adjustment_active: false,
        }
    }
}

impl CachedFeeRate {
    /// Create a new cache with custom TTL.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            ..Self::default()
        }
    }

    /// Update the cached rate with a new value from the network.
    pub fn update(&mut self, base_rate: u64) {
        self.base_rate = base_rate.max(MINIMUM_BASE_RATE);
        self.last_updated = Some(Instant::now());
    }

    /// Update with full network fee rate information.
    pub fn update_from_network(
        &mut self,
        base_rate: u64,
        congestion: f64,
        adjustment_active: bool,
    ) {
        self.base_rate = base_rate.max(MINIMUM_BASE_RATE);
        self.last_updated = Some(Instant::now());
        self.congestion = congestion;
        self.adjustment_active = adjustment_active;
    }

    /// Get the cached rate if still valid, or None if expired.
    pub fn rate(&self) -> Option<u64> {
        self.last_updated
            .filter(|t| t.elapsed() < self.ttl)
            .map(|_| self.base_rate)
    }

    /// Get the cached rate or the default minimum rate.
    ///
    /// Use this when you always need a rate value. Falls back to
    /// [`MINIMUM_BASE_RATE`] (1 nanoBTH/byte) if the cache is expired
    /// or was never updated.
    pub fn rate_or_default(&self) -> u64 {
        self.rate().unwrap_or(MINIMUM_BASE_RATE)
    }

    /// Check if the cache needs to be refreshed.
    pub fn needs_refresh(&self) -> bool {
        self.rate().is_none()
    }

    /// Get the network congestion level (0.0 to 1.0).
    pub fn congestion(&self) -> f64 {
        self.congestion
    }

    /// Check if dynamic fee adjustment is active on the network.
    pub fn is_adjustment_active(&self) -> bool {
        self.adjustment_active
    }

    /// Get the TTL for this cache.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Get time until the cache expires, or None if already expired.
    pub fn time_until_expiry(&self) -> Option<Duration> {
        self.last_updated.and_then(|t| {
            let elapsed = t.elapsed();
            if elapsed < self.ttl {
                Some(self.ttl - elapsed)
            } else {
                None
            }
        })
    }
}

/// Stored cluster tag information for a UTXO.
///
/// Tracks the cluster attribution vector for each owned UTXO, enabling
/// progressive fee calculation based on wealth distribution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredTags {
    /// Cluster ID to weight mapping (parts per million).
    /// The weights indicate attribution to specific wealth clusters.
    pub tags: Vec<(u64, TagWeight)>,
}

impl StoredTags {
    /// Create empty tag storage (fully anonymous/background).
    pub fn new() -> Self {
        Self { tags: vec![] }
    }

    /// Create from a TagVector.
    pub fn from_tag_vector(tv: &TagVector) -> Self {
        let tags: Vec<_> = tv.iter().map(|(id, w)| (id.0, w)).collect();
        Self { tags }
    }

    /// Convert to TagVector for fee calculations.
    pub fn to_tag_vector(&self) -> TagVector {
        let mut tv = TagVector::new();
        for &(id, weight) in &self.tags {
            tv.set(ClusterId::new(id), weight);
        }
        tv
    }

    /// Check if this UTXO has any cluster attribution.
    pub fn has_attribution(&self) -> bool {
        !self.tags.is_empty()
    }

    /// Total attributed weight (0 to 1_000_000).
    pub fn total_attributed(&self) -> TagWeight {
        self.tags
            .iter()
            .map(|(_, w)| w)
            .sum::<TagWeight>()
            .min(TAG_WEIGHT_SCALE)
    }
}

/// Result of fee estimation.
#[derive(Debug, Clone)]
pub struct FeeEstimate {
    /// Estimated transaction size in bytes.
    pub tx_size: usize,

    /// Blended cluster factor (1000 = 1x, up to 6000 = 6x).
    pub cluster_factor: u64,

    /// Base fee component (size * base_rate).
    pub base_fee: u64,

    /// Superlinear output penalty.
    pub output_penalty: u64,

    /// Total estimated fee.
    pub total_fee: u64,

    /// Human-readable explanation.
    pub explanation: String,
}

/// Fee estimator using the cluster-tax progressive fee model.
pub struct FeeEstimator {
    /// Full fee configuration for output penalties.
    fee_config: FeeConfig,

    /// Base fee rate in nanoBTH per byte (dynamic, from network).
    base_rate: u64,
}

impl Default for FeeEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl FeeEstimator {
    /// Create a new fee estimator with default parameters.
    pub fn new() -> Self {
        Self {
            fee_config: FeeConfig::default(),
            base_rate: 1, // 1 nanoBTH per byte (minimum)
        }
    }

    /// Create with a specific base rate (for dynamic fee adjustment).
    pub fn with_base_rate(base_rate: u64) -> Self {
        Self {
            fee_config: FeeConfig::default(),
            base_rate: base_rate.max(1),
        }
    }

    /// Estimate transaction size based on input/output count.
    pub fn estimate_tx_size(&self, num_inputs: usize, num_outputs: usize) -> usize {
        // Base size for 2-in/2-out
        let mut size = ESTIMATED_2IN_2OUT_SIZE;

        // Adjust for additional inputs
        if num_inputs > 2 {
            size += (num_inputs - 2) * SIZE_PER_ADDITIONAL_INPUT;
        } else if num_inputs < 2 {
            // Smaller transaction with 1 input
            size = size.saturating_sub((2 - num_inputs) * SIZE_PER_ADDITIONAL_INPUT);
        }

        // Adjust for additional outputs
        if num_outputs > 2 {
            size += (num_outputs - 2) * SIZE_PER_ADDITIONAL_OUTPUT;
        } else if num_outputs < 2 {
            size = size.saturating_sub((2 - num_outputs) * SIZE_PER_ADDITIONAL_OUTPUT);
        }

        size.max(1000) // Minimum transaction size
    }

    /// Calculate blended cluster factor from input UTXOs with their tag
    /// vectors.
    ///
    /// Uses value-weighted blending: larger inputs contribute more to the
    /// final cluster factor.
    pub fn calculate_blended_factor(
        &self,
        inputs: &[(u64, &StoredTags)], // (amount, tags)
    ) -> u64 {
        if inputs.is_empty() {
            return 1000; // 1x factor for empty inputs
        }

        // Blend tag vectors weighted by value using iterative mixing
        let mut blended = TagVector::new();
        let mut accumulated_value: u64 = 0;

        for (amount, tags) in inputs {
            if *amount == 0 {
                continue;
            }
            let tv = tags.to_tag_vector();
            // Mix incoming tags with current blended, weighted by value
            blended.mix(accumulated_value, &tv, *amount);
            accumulated_value = accumulated_value.saturating_add(*amount);
        }

        if accumulated_value == 0 {
            return 1000;
        }

        // Calculate cluster factor based on local tag attribution
        // Since we don't have access to global cluster wealth data, we use
        // attribution percentage as a proxy: 0% = 1x (anonymous), 100% = 6x (full
        // wealth)
        let total_attributed = blended.total_attributed();

        // Linear interpolation: factor = 1000 + (total_attributed / TAG_WEIGHT_SCALE) *
        // 5000 At 0% attribution: 1000 (1x)
        // At 100% attribution: 6000 (6x)
        let factor = 1000u64 + (total_attributed as u64 * 5000 / TAG_WEIGHT_SCALE as u64);
        factor.min(6000)
    }

    /// Calculate the superlinear output penalty for multiple outputs.
    ///
    /// Uses quadratic penalty to discourage UTXO farming:
    /// - 2 outputs: 1x (baseline)
    /// - 3 outputs: ~2.25x
    /// - 4 outputs: ~4x
    /// - etc.
    pub fn calculate_output_penalty(&self, num_outputs: usize, tx_size: usize) -> u64 {
        // Use FeeConfig's output penalty calculation
        // The penalty is a multiplier applied to the base fee
        let penalty_multiplier = self.fee_config.output_penalty(num_outputs);

        // Apply penalty to base size fee
        let base_size_fee = (tx_size as u64).saturating_mul(self.base_rate);

        // penalty_multiplier is in parts per 1000 (1000 = 1x, 2000 = 2x, etc.)
        // Return additional fee beyond the 1x baseline
        if penalty_multiplier > 1000 {
            base_size_fee.saturating_mul(penalty_multiplier - 1000) / 1000
        } else {
            0
        }
    }

    /// Estimate the total fee for a transaction.
    pub fn estimate_fee(&self, inputs: &[(u64, &StoredTags)], num_outputs: usize) -> FeeEstimate {
        let num_inputs = inputs.len();

        // Estimate transaction size
        let tx_size = self.estimate_tx_size(num_inputs, num_outputs);

        // Calculate blended cluster factor
        let cluster_factor = self.calculate_blended_factor(inputs);

        // Base fee: size * base_rate * cluster_factor / 1000
        let base_fee = (tx_size as u64)
            .saturating_mul(self.base_rate)
            .saturating_mul(cluster_factor)
            / 1000;

        // Superlinear output penalty
        let output_penalty = self.calculate_output_penalty(num_outputs, tx_size);

        // Total fee
        let total_fee = base_fee.saturating_add(output_penalty);

        // Build explanation
        let cluster_pct = (cluster_factor as f64 / 1000.0) * 100.0 - 100.0;
        let explanation = if cluster_factor > 1000 {
            format!(
                "Fee includes +{:.1}% cluster tax from input wealth attribution",
                cluster_pct
            )
        } else {
            "No cluster tax (fully anonymous inputs)".to_string()
        };

        FeeEstimate {
            tx_size,
            cluster_factor,
            base_fee,
            output_penalty,
            total_fee,
            explanation,
        }
    }

    /// Get the current base rate.
    pub fn base_rate(&self) -> u64 {
        self.base_rate
    }

    /// Update the base rate (for dynamic fee adjustment).
    pub fn set_base_rate(&mut self, rate: u64) {
        self.base_rate = rate.max(1);
    }
}

/// Format a fee estimate for display.
pub fn format_fee_estimate(estimate: &FeeEstimate, picocredits_per_cad: u64) -> String {
    let fee_cad = estimate.total_fee as f64 / picocredits_per_cad as f64;
    let cluster_multiplier = estimate.cluster_factor as f64 / 1000.0;

    let mut lines = vec![
        format!("Estimated Fee: {:.6} CAD", fee_cad),
        format!("  Transaction size: {} bytes", estimate.tx_size),
        format!("  Cluster factor: {:.2}x", cluster_multiplier),
    ];

    if estimate.output_penalty > 0 {
        let penalty_cad = estimate.output_penalty as f64 / picocredits_per_cad as f64;
        lines.push(format!("  Output penalty: {:.6} CAD", penalty_cad));
    }

    lines.push(format!("  {}", estimate.explanation));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stored_tags_empty() {
        let tags = StoredTags::new();
        assert!(!tags.has_attribution());
        assert_eq!(tags.total_attributed(), 0);
    }

    #[test]
    fn test_stored_tags_roundtrip() {
        let mut tv = TagVector::new();
        tv.set(ClusterId::new(1), 500_000);
        tv.set(ClusterId::new(2), 300_000);

        let stored = StoredTags::from_tag_vector(&tv);
        assert!(stored.has_attribution());

        let recovered = stored.to_tag_vector();
        assert_eq!(recovered.get(ClusterId::new(1)), 500_000);
        assert_eq!(recovered.get(ClusterId::new(2)), 300_000);
    }

    #[test]
    fn test_estimate_tx_size() {
        let estimator = FeeEstimator::new();

        // Standard 2-in/2-out
        assert_eq!(estimator.estimate_tx_size(2, 2), ESTIMATED_2IN_2OUT_SIZE);

        // More inputs
        assert!(estimator.estimate_tx_size(3, 2) > ESTIMATED_2IN_2OUT_SIZE);

        // More outputs
        assert!(estimator.estimate_tx_size(2, 3) > ESTIMATED_2IN_2OUT_SIZE);
    }

    #[test]
    fn test_blended_factor_anonymous() {
        let estimator = FeeEstimator::new();

        // Empty inputs
        let factor = estimator.calculate_blended_factor(&[]);
        assert_eq!(factor, 1000); // 1x

        // Anonymous inputs (no tags)
        let empty_tags = StoredTags::new();
        let inputs = vec![
            (1_000_000_000_000u64, &empty_tags),
            (500_000_000_000u64, &empty_tags),
        ];
        let factor = estimator.calculate_blended_factor(&inputs);
        assert_eq!(factor, 1000); // 1x for anonymous
    }

    #[test]
    fn test_blended_factor_attributed() {
        let estimator = FeeEstimator::new();

        // Fully attributed input (100% to one cluster)
        let mut tv = TagVector::new();
        tv.set(ClusterId::new(1), TAG_WEIGHT_SCALE);
        let full_tags = StoredTags::from_tag_vector(&tv);

        let inputs = vec![(1_000_000_000_000u64, &full_tags)];
        let factor = estimator.calculate_blended_factor(&inputs);

        // Should be maximum (6x = 6000)
        assert_eq!(factor, 6000);
    }

    #[test]
    fn test_blended_factor_mixed() {
        let estimator = FeeEstimator::new();

        // Mix of attributed and anonymous
        let mut tv = TagVector::new();
        tv.set(ClusterId::new(1), TAG_WEIGHT_SCALE);
        let full_tags = StoredTags::from_tag_vector(&tv);
        let empty_tags = StoredTags::new();

        // 50% attributed (by value)
        let inputs = vec![
            (1_000_000_000_000u64, &full_tags),
            (1_000_000_000_000u64, &empty_tags),
        ];
        let factor = estimator.calculate_blended_factor(&inputs);

        // Should be between 1x and 6x
        assert!(factor > 1000);
        assert!(factor < 6000);
    }

    #[test]
    fn test_output_penalty() {
        let estimator = FeeEstimator::new();
        let tx_size = 6000;

        // 2 outputs should have no/minimal penalty
        let penalty_2 = estimator.calculate_output_penalty(2, tx_size);

        // More outputs should have higher penalty
        let penalty_4 = estimator.calculate_output_penalty(4, tx_size);
        assert!(penalty_4 > penalty_2);
    }

    #[test]
    fn test_full_estimate() {
        let estimator = FeeEstimator::new();
        let empty_tags = StoredTags::new();

        let inputs = vec![(1_000_000_000_000u64, &empty_tags)];

        let estimate = estimator.estimate_fee(&inputs, 2);

        assert!(estimate.tx_size > 0);
        assert_eq!(estimate.cluster_factor, 1000); // Anonymous
        assert!(estimate.total_fee > 0);
    }

    #[test]
    fn test_format_estimate() {
        let estimate = FeeEstimate {
            tx_size: 6000,
            cluster_factor: 1500,
            base_fee: 9000,
            output_penalty: 0,
            total_fee: 9000,
            explanation: "Fee includes +50.0% cluster tax".to_string(),
        };

        let formatted = format_fee_estimate(&estimate, 1_000_000_000_000);
        assert!(formatted.contains("Estimated Fee"));
        assert!(formatted.contains("6000 bytes"));
        assert!(formatted.contains("1.50x"));
    }

    #[test]
    fn test_cached_fee_rate_default() {
        let cache = CachedFeeRate::default();

        // Should need refresh initially (never updated)
        assert!(cache.needs_refresh());
        assert_eq!(cache.rate(), None);
        assert_eq!(cache.rate_or_default(), MINIMUM_BASE_RATE);
    }

    #[test]
    fn test_cached_fee_rate_update() {
        let mut cache = CachedFeeRate::default();

        cache.update(5);

        assert!(!cache.needs_refresh());
        assert_eq!(cache.rate(), Some(5));
        assert_eq!(cache.rate_or_default(), 5);
    }

    #[test]
    fn test_cached_fee_rate_update_from_network() {
        let mut cache = CachedFeeRate::default();

        cache.update_from_network(10, 0.5, true);

        assert_eq!(cache.rate(), Some(10));
        assert!((cache.congestion() - 0.5).abs() < 0.001);
        assert!(cache.is_adjustment_active());
    }

    #[test]
    fn test_cached_fee_rate_minimum_enforced() {
        let mut cache = CachedFeeRate::default();

        // Update with 0 should result in minimum
        cache.update(0);

        assert_eq!(cache.rate(), Some(MINIMUM_BASE_RATE));
    }

    #[test]
    fn test_cached_fee_rate_expiry() {
        let mut cache = CachedFeeRate::with_ttl(Duration::from_millis(10));

        cache.update(5);
        assert!(!cache.needs_refresh());
        assert!(cache.time_until_expiry().is_some());

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(15));

        assert!(cache.needs_refresh());
        assert_eq!(cache.rate(), None);
        assert_eq!(cache.rate_or_default(), MINIMUM_BASE_RATE);
        assert!(cache.time_until_expiry().is_none());
    }

    #[test]
    fn test_fee_estimator_with_dynamic_rate() {
        let mut estimator = FeeEstimator::new();

        // Default rate
        assert_eq!(estimator.base_rate(), 1);

        // Update with higher rate (simulating network congestion)
        estimator.set_base_rate(5);
        assert_eq!(estimator.base_rate(), 5);

        // Estimate fee should reflect new rate
        let empty_tags = StoredTags::new();
        let inputs = vec![(1_000_000_000_000u64, &empty_tags)];

        let estimate = estimator.estimate_fee(&inputs, 2);

        // Fee should be 5x higher than with rate of 1
        // (base_fee = tx_size * rate * cluster_factor / 1000)
        // With anonymous inputs, cluster_factor = 1000, so:
        // base_fee = 6000 * 5 * 1000 / 1000 = 30000
        assert!(estimate.base_fee >= 25000); // Allow some tolerance
    }

    #[test]
    fn test_cached_fee_rate_integration_with_estimator() {
        let mut cache = CachedFeeRate::default();
        let mut estimator = FeeEstimator::new();

        // Simulate fetching from network
        cache.update_from_network(3, 0.2, false);

        // Apply cached rate to estimator
        if let Some(rate) = cache.rate() {
            estimator.set_base_rate(rate);
        }

        assert_eq!(estimator.base_rate(), 3);
    }
}
