//! Decoy Selection for Ring Signatures
//!
//! Wallet-side decoy-selection policy for CLSAG ring signatures. This module
//! provides the age-band primitives shared with the live send path (see
//! [`crate::ring_builder`]) plus a constraint-checking selector used for
//! validating externally-supplied decoy sets.
//!
//! # Age-similarity policy (matches the node)
//!
//! Decoys are drawn from within **±10%** of the real input's age
//! ([`AGE_SIMILARITY_SPREAD_BPS`]), floored at [`MIN_DECOY_AGE_BLOCKS`]
//! confirmations. This mirrors the node's `GammaDecoySelector`
//! (`botho/src/decoy_selection.rs`): a wider band (the old 2× ratio) let an
//! age-heuristic adversary fingerprint CLI-wallet rings by their unusually
//! broad ring-age spread. Keeping the same ±10% band collapses that distinction
//! (issue #614 item 4; #314-safe under `ring_elapsed_quantile@max`).
//!
//! # Cluster-factor ceiling
//!
//! Decoys must not have a significantly higher cluster factor than the real
//! input (`decoy_factor <= real_factor * max_factor_ratio`). This prevents a
//! malicious pool from inflating progressive fees via high-factor decoys.
//!
//! # Ring size
//!
//! The default ring size is [`DEFAULT_RING_SIZE`] = 20, matching the node's
//! consensus floor `MIN_RING_SIZE`. Rings smaller than 20 are rejected by the
//! node at signing and validation time, so the wallet must never build them.
//!
//! # Randomization
//!
//! Eligible candidates are shuffled with a CSPRNG before the ring is drawn, so
//! ring membership is never a deterministic first-N slice of a height-sorted
//! pool (issue #614 item 5).

use std::collections::HashMap;

use bth_cluster_tax::{ClusterId, TAG_WEIGHT_SCALE};
use rand::{rngs::OsRng, seq::SliceRandom};

use crate::fee_estimation::StoredTags;

/// Minimum confirmations before an output may be used as a ring member (real or
/// decoy). Mirrors the node's `MIN_DECOY_AGE_BLOCKS`.
pub const MIN_DECOY_AGE_BLOCKS: u64 = 10;

/// Half-width of the decoy age-similarity band, in basis points of the real
/// input's age. `1_000` bps = ±10%. Mirrors the node's
/// `AGE_SIMILARITY_SPREAD_BPS`.
pub const AGE_SIMILARITY_SPREAD_BPS: u64 = 1_000;

/// Default (and minimum) ring size, matching the node's consensus
/// `MIN_RING_SIZE`. A ring of 20 provides a strong anonymity set with compact
/// ~700-byte CLSAG signatures.
pub const DEFAULT_RING_SIZE: usize = 20;

/// Compute the inclusive `[min_age, max_age]` decoy-age band around a real
/// input's age under the [`AGE_SIMILARITY_SPREAD_BPS`] policy.
///
/// The lower bound is floored at [`MIN_DECOY_AGE_BLOCKS`] so decoys are always
/// confirmed. A degenerate real age (below the confirmation floor) can produce
/// `min_age > max_age`, in which case no candidate is in-band; callers must
/// guard against spending such young inputs before reaching this function.
pub fn age_similarity_band(real_input_age: u64) -> (u64, u64) {
    let delta = (real_input_age as u128 * AGE_SIMILARITY_SPREAD_BPS as u128 / 10_000) as u64;
    let min_age = real_input_age
        .saturating_sub(delta)
        .max(MIN_DECOY_AGE_BLOCKS);
    let max_age = real_input_age.saturating_add(delta);
    (min_age, max_age)
}

/// Configuration for decoy selection constraints.
#[derive(Clone, Debug)]
pub struct DecoySelectionConfig {
    /// Ring size (including the real input).
    /// Default: [`DEFAULT_RING_SIZE`] (20) — the node's consensus floor.
    pub ring_size: usize,

    /// Maximum cluster factor ratio between decoy and real input.
    /// A value of 1.5 means decoy factor must be <= 1.5x real factor.
    /// This prevents fee-inflation attacks where a malicious pool selects
    /// high-factor decoys to inflate progressive fees.
    /// Default: 1.5
    pub max_factor_ratio: f64,
}

impl Default for DecoySelectionConfig {
    fn default() -> Self {
        Self {
            ring_size: DEFAULT_RING_SIZE,
            max_factor_ratio: 1.5,
        }
    }
}

impl DecoySelectionConfig {
    /// Create a new configuration with custom parameters.
    pub fn new(ring_size: usize, max_factor_ratio: f64) -> Self {
        Self {
            ring_size,
            max_factor_ratio,
        }
    }

    /// Number of decoys needed (ring_size - 1, since real input is included).
    pub fn decoys_needed(&self) -> usize {
        self.ring_size.saturating_sub(1)
    }
}

/// Error type for decoy selection failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecoySelectionError {
    /// Not enough decoys available that meet the constraints.
    InsufficientDecoys { required: usize, available: usize },

    /// No UTXOs available in the pool.
    EmptyUtxoPool,

    /// The real UTXO has invalid parameters (e.g., too young to spend
    /// privately).
    InvalidRealUtxo(String),

    /// Ring size must be at least 2 (real input + 1 decoy).
    InvalidRingSize,
}

impl std::fmt::Display for DecoySelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientDecoys {
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient decoys: need {} but only {} meet constraints",
                    required, available
                )
            }
            Self::EmptyUtxoPool => write!(f, "UTXO pool is empty"),
            Self::InvalidRealUtxo(msg) => write!(f, "Invalid real UTXO: {}", msg),
            Self::InvalidRingSize => write!(f, "Ring size must be at least 2"),
        }
    }
}

impl std::error::Error for DecoySelectionError {}

/// Result type for decoy selection.
pub type DecoySelectionResult<T> = Result<T, DecoySelectionError>;

/// A UTXO candidate for inclusion in a ring.
///
/// This is a simplified representation used during decoy selection.
/// The wallet can convert its internal UTXO type to this format.
#[derive(Clone, Debug)]
pub struct UtxoCandidate {
    /// Unique identifier for this UTXO.
    pub id: [u8; 32],

    /// Block height where this UTXO was created.
    pub created_at: u64,

    /// Amount in picocredits.
    pub amount: u64,

    /// Cluster tags for this UTXO.
    pub tags: StoredTags,
}

impl UtxoCandidate {
    /// Calculate the age of this UTXO in blocks.
    pub fn age(&self, current_block: u64) -> u64 {
        current_block.saturating_sub(self.created_at)
    }

    /// Calculate the cluster factor for this UTXO.
    ///
    /// Uses a simplified local calculation based on tag attribution percentage.
    /// A more accurate calculation would use global cluster wealth data.
    pub fn cluster_factor(&self) -> f64 {
        let tv = self.tags.to_tag_vector();
        let total_attributed = tv.total_attributed();

        // Linear interpolation: factor = 1.0 + (attribution_pct * 5.0)
        // At 0% attribution: 1.0 (anonymous)
        // At 100% attribution: 6.0 (full wealth)
        1.0 + (total_attributed as f64 / TAG_WEIGHT_SCALE as f64 * 5.0)
    }

    /// Calculate the cluster factor using global cluster wealth data.
    ///
    /// This provides a more accurate factor based on actual wealth
    /// distribution.
    pub fn cluster_factor_global(
        &self,
        cluster_wealth: &HashMap<ClusterId, u64>,
        total_supply: u64,
    ) -> f64 {
        if total_supply == 0 {
            return 1.0;
        }

        let tv = self.tags.to_tag_vector();
        let mut weighted_factor = 0.0;
        let mut total_weight = 0u64;

        // Weighted average of cluster wealth fractions
        for (cluster, weight) in tv.iter() {
            let cluster_w = cluster_wealth.get(&cluster).copied().unwrap_or(0);
            let wealth_fraction = cluster_w as f64 / total_supply as f64;
            weighted_factor += wealth_fraction * weight as f64;
            total_weight += weight as u64;
        }

        // Background weight contributes 0 (fully diffused)
        let bg = tv.background() as u64;
        total_weight += bg;

        if total_weight == 0 {
            return 1.0;
        }

        // Normalize to factor range [1.0, 6.0]
        let raw_factor = weighted_factor / total_weight as f64 * TAG_WEIGHT_SCALE as f64;
        1.0 + (raw_factor / TAG_WEIGHT_SCALE as f64 * 5.0)
    }
}

/// Select decoys for a ring signature that satisfy the age-similarity band and
/// cluster-factor ceiling.
///
/// # Arguments
///
/// * `real_utxo` - The real UTXO being spent
/// * `utxo_pool` - Pool of available UTXOs to select decoys from
/// * `current_block` - Current blockchain height
/// * `cluster_wealth` - Map of cluster IDs to their total wealth
/// * `total_supply` - Total coin supply for factor normalization
/// * `config` - Decoy selection configuration
///
/// # Constraints Applied
///
/// 1. **Age similarity**: decoy age within [`age_similarity_band`] of the real
///    input's age (±10%, floored at [`MIN_DECOY_AGE_BLOCKS`]).
/// 2. **Factor ceiling**: `decoy_factor <= real_factor * max_factor_ratio`.
///
/// Eligible candidates are shuffled (CSPRNG) before the ring is drawn, so the
/// result is not a deterministic first-N slice.
///
/// # Errors
///
/// Returns [`DecoySelectionError::InvalidRealUtxo`] if the real input is
/// younger than [`MIN_DECOY_AGE_BLOCKS`] (spending it would produce a
/// degenerate age band). Callers surfacing to users should translate this into
/// a "wait for confirmations" message.
pub fn select_decoys(
    real_utxo: &UtxoCandidate,
    utxo_pool: &[UtxoCandidate],
    current_block: u64,
    cluster_wealth: &HashMap<ClusterId, u64>,
    total_supply: u64,
    config: &DecoySelectionConfig,
) -> DecoySelectionResult<Vec<UtxoCandidate>> {
    // Validate inputs
    if config.ring_size < 2 {
        return Err(DecoySelectionError::InvalidRingSize);
    }

    if utxo_pool.is_empty() {
        return Err(DecoySelectionError::EmptyUtxoPool);
    }

    let real_age = real_utxo.age(current_block);

    // Young-input guard (mirrors node decoy_selection.rs; #611/#618). Below the
    // confirmation floor the ±10% band is degenerate (min_age > max_age).
    if real_age < MIN_DECOY_AGE_BLOCKS {
        return Err(DecoySelectionError::InvalidRealUtxo(format!(
            "UTXO is too new to spend privately — wait for at least {} confirmations \
             (current age: {} block(s))",
            MIN_DECOY_AGE_BLOCKS, real_age
        )));
    }

    let real_factor = real_utxo.cluster_factor_global(cluster_wealth, total_supply);

    // Age band (±10%, floored at the confirmation minimum).
    let (min_age, max_age) = age_similarity_band(real_age);

    // Calculate maximum allowed factor for decoys
    let max_allowed_factor = real_factor * config.max_factor_ratio;

    let decoys_needed = config.decoys_needed();

    // Filter candidates that meet all constraints
    let mut eligible: Vec<UtxoCandidate> = utxo_pool
        .iter()
        .filter(|candidate| {
            // Skip if same as real UTXO
            if candidate.id == real_utxo.id {
                return false;
            }

            let candidate_age = candidate.age(current_block);

            // Age constraint: within the ±10% band
            if candidate_age < min_age || candidate_age > max_age {
                return false;
            }

            // Factor constraint: not significantly higher than real
            let candidate_factor = candidate.cluster_factor_global(cluster_wealth, total_supply);
            if candidate_factor > max_allowed_factor {
                return false;
            }

            true
        })
        .cloned()
        .collect();

    // Check if we have enough eligible decoys
    if eligible.len() < decoys_needed {
        return Err(DecoySelectionError::InsufficientDecoys {
            required: decoys_needed,
            available: eligible.len(),
        });
    }

    // Shuffle before taking N so ring membership is randomized, not a
    // deterministic first-N slice of a height-sorted pool.
    let mut rng = OsRng;
    eligible.shuffle(&mut rng);
    eligible.truncate(decoys_needed);
    Ok(eligible)
}

/// Select decoys with fallback to a relaxed cluster-factor ceiling.
///
/// The age-similarity band is a **privacy invariant** and is never relaxed — a
/// wider band would fingerprint the transaction. Only the factor ceiling is
/// progressively relaxed if the strict ceiling yields too few decoys.
///
/// # Warning
///
/// A relaxed factor ceiling admits higher-factor decoys, which can slightly
/// raise the estimated fee. The caller should consider warning the user.
pub fn select_decoys_with_fallback(
    real_utxo: &UtxoCandidate,
    utxo_pool: &[UtxoCandidate],
    current_block: u64,
    cluster_wealth: &HashMap<ClusterId, u64>,
    total_supply: u64,
    config: &DecoySelectionConfig,
) -> DecoySelectionResult<(Vec<UtxoCandidate>, bool)> {
    // Try with original config
    match select_decoys(
        real_utxo,
        utxo_pool,
        current_block,
        cluster_wealth,
        total_supply,
        config,
    ) {
        Ok(decoys) => return Ok((decoys, false)),
        Err(DecoySelectionError::InsufficientDecoys { .. }) => {
            // Continue to fallback
        }
        Err(e) => return Err(e),
    }

    // Progressively relax ONLY the factor ceiling (never the age band).
    for relaxed_factor in [2.0_f64, 3.0, 5.0] {
        let relaxed = DecoySelectionConfig {
            ring_size: config.ring_size,
            max_factor_ratio: relaxed_factor,
        };

        match select_decoys(
            real_utxo,
            utxo_pool,
            current_block,
            cluster_wealth,
            total_supply,
            &relaxed,
        ) {
            Ok(decoys) => return Ok((decoys, true)),
            Err(DecoySelectionError::InsufficientDecoys { .. }) => continue,
            Err(e) => return Err(e),
        }
    }

    // Still insufficient after maximum relaxation — report the strict shortfall.
    select_decoys(
        real_utxo,
        utxo_pool,
        current_block,
        cluster_wealth,
        total_supply,
        config,
    )
    .map(|decoys| (decoys, true))
}

/// Validate that a set of decoys meets the selection constraints.
///
/// This can be used to verify decoys selected by an external party
/// (e.g., from a remote node) meet the expected constraints.
pub fn validate_decoys(
    real_utxo: &UtxoCandidate,
    decoys: &[UtxoCandidate],
    current_block: u64,
    cluster_wealth: &HashMap<ClusterId, u64>,
    total_supply: u64,
    config: &DecoySelectionConfig,
) -> Vec<(usize, String)> {
    let mut violations = Vec::new();

    let real_age = real_utxo.age(current_block);
    let real_factor = real_utxo.cluster_factor_global(cluster_wealth, total_supply);

    let (min_age, max_age) = age_similarity_band(real_age);
    let max_allowed_factor = real_factor * config.max_factor_ratio;

    for (i, decoy) in decoys.iter().enumerate() {
        let decoy_age = decoy.age(current_block);
        let decoy_factor = decoy.cluster_factor_global(cluster_wealth, total_supply);

        if decoy_age < min_age {
            violations.push((
                i,
                format!("Decoy too young: age {} < min {}", decoy_age, min_age),
            ));
        }

        if decoy_age > max_age {
            violations.push((
                i,
                format!("Decoy too old: age {} > max {}", decoy_age, max_age),
            ));
        }

        if decoy_factor > max_allowed_factor {
            violations.push((
                i,
                format!(
                    "Decoy factor too high: {:.2} > max {:.2}",
                    decoy_factor, max_allowed_factor
                ),
            ));
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_utxo(id: u8, created_at: u64, tags: &[(u64, u32)]) -> UtxoCandidate {
        let mut stored_tags = StoredTags::new();
        stored_tags.tags = tags.iter().map(|&(c, w)| (c, w)).collect();

        UtxoCandidate {
            id: [id; 32],
            created_at,
            amount: 1_000_000_000_000,
            tags: stored_tags,
        }
    }

    fn create_cluster_wealth() -> HashMap<ClusterId, u64> {
        let mut wealth = HashMap::new();
        wealth.insert(ClusterId::new(1), 1_000_000_000_000); // 10%
        wealth.insert(ClusterId::new(2), 2_000_000_000_000); // 20%
        wealth.insert(ClusterId::new(3), 500_000_000_000); // 5%
        wealth
    }

    const TOTAL_SUPPLY: u64 = 10_000_000_000_000;
    const CURRENT_BLOCK: u64 = 10_000;

    /// Build a pool of `n` decoy candidates whose ages fall inside the ±10%
    /// band of a real input created at `real_created_at`, all with the given
    /// tags. Ages are spread across the in-band window.
    fn in_band_pool(n: usize, real_created_at: u64, tags: &[(u64, u32)]) -> Vec<UtxoCandidate> {
        let real_age = CURRENT_BLOCK - real_created_at;
        let (min_age, max_age) = age_similarity_band(real_age);
        let mut pool = Vec::new();
        for i in 0..n {
            // Distribute ages evenly within [min_age, max_age].
            let span = max_age - min_age;
            let age = min_age + (i as u64 % (span + 1));
            let created_at = CURRENT_BLOCK - age;
            pool.push(create_utxo((i + 1) as u8, created_at, tags));
        }
        pool
    }

    #[test]
    fn test_config_defaults() {
        let config = DecoySelectionConfig::default();
        assert_eq!(config.ring_size, 20);
        assert_eq!(config.max_factor_ratio, 1.5);
        assert_eq!(config.decoys_needed(), 19);
        // Ring size must match the node's consensus floor.
        assert_eq!(config.ring_size, DEFAULT_RING_SIZE);
    }

    #[test]
    fn test_age_similarity_band() {
        // ±10% around a healthy age.
        assert_eq!(age_similarity_band(1000), (900, 1100));
        assert_eq!(age_similarity_band(100), (90, 110));
        // ±10% of 50 is ±5 -> [45, 55]; above the confirmation floor.
        let (min_age, max_age) = age_similarity_band(50);
        assert_eq!(min_age, 45);
        assert_eq!(max_age, 55);
        // A lower age hits the confirmation floor: ±10% of 80 is ±8 -> min 72.
        let (min_floor, _) = age_similarity_band(80);
        assert_eq!(min_floor, 72);
    }

    #[test]
    fn test_age_similarity_band_floor() {
        // A tiny age produces a degenerate band (min > max) after flooring.
        let (min_age, max_age) = age_similarity_band(5);
        assert_eq!(min_age, MIN_DECOY_AGE_BLOCKS);
        assert_eq!(max_age, 5);
        assert!(min_age > max_age, "degenerate band expected for young age");
    }

    #[test]
    fn test_utxo_age_calculation() {
        let utxo = create_utxo(1, 5_000, &[]);
        assert_eq!(utxo.age(10_000), 5_000);
        assert_eq!(utxo.age(5_000), 0);
        assert_eq!(utxo.age(3_000), 0); // Saturating sub
    }

    #[test]
    fn test_cluster_factor_anonymous() {
        let utxo = create_utxo(1, 5_000, &[]);
        let factor = utxo.cluster_factor();
        assert!((factor - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cluster_factor_full_attribution() {
        let utxo = create_utxo(1, 5_000, &[(1, TAG_WEIGHT_SCALE)]);
        let factor = utxo.cluster_factor();
        assert!((factor - 6.0).abs() < 0.001);
    }

    #[test]
    fn test_select_decoys_basic() {
        // Real input created at 9_000 -> age 1000 -> band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, &[(1, TAG_WEIGHT_SCALE / 2)]);
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(25, 9_000, &[(1, TAG_WEIGHT_SCALE / 2)]);

        let config = DecoySelectionConfig::default(); // ring 20, needs 19
        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok(), "{:?}", result.err());
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 19);
        // All within the ±10% band.
        for d in &decoys {
            let age = d.age(CURRENT_BLOCK);
            assert!((900..=1100).contains(&age), "age {} out of band", age);
        }
    }

    #[test]
    fn test_select_decoys_age_filtering() {
        // Real age 1000 -> band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        let mut pool = in_band_pool(19, 9_000, &[]);
        // Add clearly out-of-band candidates that must never be selected.
        pool.push(create_utxo(200, 9_800, &[])); // age 200 - too young
        pool.push(create_utxo(201, 8_000, &[])); // age 2000 - too old

        let config = DecoySelectionConfig::default();
        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        )
        .expect("selection should succeed");

        for decoy in &decoys {
            let age = decoy.age(CURRENT_BLOCK);
            assert!((900..=1100).contains(&age), "Age {} out of band", age);
            assert_ne!(decoy.id, [200u8; 32]);
            assert_ne!(decoy.id, [201u8; 32]);
        }
    }

    #[test]
    fn test_select_decoys_factor_filtering() {
        // Real ~25% attribution to a wealthy cluster (50% of supply).
        let real_utxo = create_utxo(0, 9_000, &[(1, TAG_WEIGHT_SCALE / 4)]);
        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50% of supply

        // 19 acceptable low-factor (anonymous) decoys plus one 100%-attribution
        // decoy whose factor exceeds real_factor * 1.5 and must be excluded.
        let mut pool = in_band_pool(19, 9_000, &[]);
        pool.push(create_utxo(250, 9_000, &[(1, TAG_WEIGHT_SCALE)]));

        let config = DecoySelectionConfig::default();
        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        )
        .expect("selection should succeed");

        for decoy in &decoys {
            assert_ne!(
                decoy.id, [250u8; 32],
                "High-factor decoy should be excluded"
            );
        }
    }

    #[test]
    fn test_select_decoys_insufficient() {
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(2, 9_000, &[]);

        let config = DecoySelectionConfig::default(); // needs 19
        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(matches!(
            result,
            Err(DecoySelectionError::InsufficientDecoys {
                required: 19,
                available: 2
            })
        ));
    }

    #[test]
    fn test_select_decoys_empty_pool() {
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        let result = select_decoys(
            &real_utxo,
            &[],
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        );

        assert!(matches!(result, Err(DecoySelectionError::EmptyUtxoPool)));
    }

    #[test]
    fn test_select_decoys_invalid_ring_size() {
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(5, 9_000, &[]);

        let config = DecoySelectionConfig {
            ring_size: 1, // Invalid
            max_factor_ratio: 1.5,
        };

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(matches!(result, Err(DecoySelectionError::InvalidRingSize)));
    }

    #[test]
    fn test_select_decoys_young_input_rejected() {
        // Real input younger than the confirmation floor must be rejected
        // cleanly (no panic, no degenerate band).
        let real_utxo = create_utxo(0, CURRENT_BLOCK - 5, &[]); // age 5 < 10
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(25, 9_000, &[]);

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        );

        match result {
            Err(DecoySelectionError::InvalidRealUtxo(msg)) => {
                assert!(msg.contains("too new"), "unexpected message: {}", msg);
            }
            other => panic!("expected InvalidRealUtxo, got {:?}", other),
        }
    }

    #[test]
    fn test_select_decoys_zero_age_rejected() {
        // Age 0 is a special case of the young-input guard.
        let real_utxo = create_utxo(0, CURRENT_BLOCK, &[]);
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(25, 9_000, &[]);

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        );

        assert!(matches!(
            result,
            Err(DecoySelectionError::InvalidRealUtxo(_))
        ));
    }

    #[test]
    fn test_select_decoys_excludes_self() {
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        // Pool includes the real UTXO (same id) plus enough in-band decoys.
        let mut pool = in_band_pool(25, 9_000, &[]);
        pool.push(create_utxo(0, 9_000, &[])); // same id as real

        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        )
        .expect("selection should succeed");

        for decoy in &decoys {
            assert_ne!(decoy.id, real_utxo.id, "Real UTXO should not be in decoys");
        }
    }

    #[test]
    fn test_select_decoys_is_randomized() {
        // With a pool much larger than needed, the selected set should not be
        // the deterministic first-19 of the input order (astronomically
        // unlikely under a fair shuffle).
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(60, 9_000, &[]);

        let first_n_ids: Vec<[u8; 32]> = pool.iter().take(19).map(|c| c.id).collect();

        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        )
        .expect("selection should succeed");
        let selected_ids: Vec<[u8; 32]> = decoys.iter().map(|c| c.id).collect();

        assert_eq!(selected_ids.len(), 19);
        assert_ne!(
            selected_ids, first_n_ids,
            "selection must be shuffled, not a deterministic first-N slice"
        );
    }

    #[test]
    fn test_select_decoys_with_fallback_success_first_try() {
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(25, 9_000, &[]);

        let (decoys, relaxed) = select_decoys_with_fallback(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        )
        .expect("selection should succeed");

        assert_eq!(decoys.len(), 19);
        assert!(!relaxed, "Should succeed without relaxation");
    }

    #[test]
    fn test_select_decoys_with_fallback_relaxes_factor() {
        // Real input is anonymous (factor 1.0); strict ceiling 1.5x = 1.5.
        let real_utxo = create_utxo(0, 9_000, &[]);
        // Cluster 1 is wealthy so attributed decoys have a high factor.
        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50%

        // All in-band decoys are attributed to the wealthy cluster (high
        // factor) — the strict ceiling excludes them, forcing relaxation.
        let pool = in_band_pool(25, 9_000, &[(1, TAG_WEIGHT_SCALE)]);

        let (decoys, relaxed) = select_decoys_with_fallback(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        )
        .expect("selection should succeed after relaxation");

        assert_eq!(decoys.len(), 19);
        assert!(relaxed, "Should indicate the factor ceiling was relaxed");
    }

    #[test]
    fn test_validate_decoys_all_valid() {
        // Real age 1000 -> band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        let decoys = vec![
            create_utxo(1, 9_050, &[]), // age 950 - in band
            create_utxo(2, 8_950, &[]), // age 1050 - in band
        ];

        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        );

        assert!(
            violations.is_empty(),
            "Expected no violations: {:?}",
            violations
        );
    }

    #[test]
    fn test_validate_decoys_age_violations() {
        // Real age 1000 -> band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        let decoys = vec![
            create_utxo(1, 9_800, &[]), // age 200 - too young
            create_utxo(2, 9_000, &[]), // age 1000 - valid
            create_utxo(3, 5_000, &[]), // age 5000 - too old
        ];

        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        );

        assert_eq!(violations.len(), 2);
        assert_eq!(violations[0].0, 0); // First decoy
        assert!(violations[0].1.contains("too young"));
        assert_eq!(violations[1].0, 2); // Third decoy
        assert!(violations[1].1.contains("too old"));
    }

    #[test]
    fn test_validate_decoys_factor_violation() {
        let real_utxo = create_utxo(0, 9_000, &[]);

        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50% of supply

        // Decoy age in band (age 1000), but 100% attribution -> high factor.
        let decoys = vec![create_utxo(1, 9_000, &[(1, TAG_WEIGHT_SCALE)])];

        let config = DecoySelectionConfig::default();

        let real_factor = real_utxo.cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
        let decoy_factor = decoys[0].cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
        assert!(
            decoy_factor > real_factor * config.max_factor_ratio,
            "Decoy factor {} should exceed max allowed {}",
            decoy_factor,
            real_factor * config.max_factor_ratio
        );

        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert_eq!(
            violations.len(),
            1,
            "Should detect 1 violation: {:?}",
            violations
        );
        assert!(violations[0].1.contains("factor too high"));
    }

    #[test]
    fn test_error_display() {
        let e1 = DecoySelectionError::InsufficientDecoys {
            required: 19,
            available: 5,
        };
        assert!(e1.to_string().contains("19"));
        assert!(e1.to_string().contains("5"));

        let e2 = DecoySelectionError::EmptyUtxoPool;
        assert!(e2.to_string().contains("empty"));

        let e3 = DecoySelectionError::InvalidRealUtxo("test".to_string());
        assert!(e3.to_string().contains("test"));

        let e4 = DecoySelectionError::InvalidRingSize;
        assert!(e4.to_string().contains("at least 2"));
    }
}
