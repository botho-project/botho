//! Decoy Selection for Ring Signatures
//!
//! Implements wallet-side decoy selection constraints to prevent fee inflation
//! attacks and ensure good privacy properties for ring signatures.
//!
//! # Constraints
//!
//! 1. **Age similarity**: Decoys should be within 2x age of real input
//!    - `age > real_age / 2 && age < real_age * 2`
//!
//! 2. **Cluster factor ceiling**: Decoys should not have significantly higher
//!    factor
//!    - `decoy_factor <= real_factor * 1.5`
//!
//! # Privacy Considerations
//!
//! Overly strict constraints reduce the anonymity set. The design balances:
//! - **Looser constraints** = larger anonymity set, less fee accuracy
//! - **Stricter constraints** = smaller anonymity set, better fee accuracy
//!
//! The recommended defaults (2x age, 1.5x factor) provide reasonable balance.
//!
//! # Reference
//!
//! See `docs/design/ring-signature-tag-propagation.md` for full design
//! rationale.

use std::collections::HashMap;

use bth_cluster_tax::{ClusterId, TagVector, TAG_WEIGHT_SCALE};

use crate::fee_estimation::StoredTags;

/// Configuration for decoy selection constraints.
#[derive(Clone, Debug)]
pub struct DecoySelectionConfig {
    /// Ring size (including real input).
    /// Default: 11 (same as Monero)
    pub ring_size: usize,

    /// Maximum age ratio between decoy and real input.
    /// A value of 2.0 means decoys must be within 2x age of the real input.
    /// Default: 2.0
    pub max_age_ratio: f64,

    /// Maximum cluster factor ratio between decoy and real input.
    /// A value of 1.5 means decoy factor must be <= 1.5x real factor.
    /// This prevents fee inflation attacks where malicious nodes select
    /// high-factor decoys to inflate fees.
    /// Default: 1.5
    pub max_factor_ratio: f64,
}

impl Default for DecoySelectionConfig {
    fn default() -> Self {
        Self {
            ring_size: 11,
            max_age_ratio: 2.0,
            max_factor_ratio: 1.5,
        }
    }
}

impl DecoySelectionConfig {
    /// Create a new configuration with custom parameters.
    pub fn new(ring_size: usize, max_age_ratio: f64, max_factor_ratio: f64) -> Self {
        Self {
            ring_size,
            max_age_ratio,
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

    /// The real UTXO has invalid parameters (e.g., zero age).
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

/// Select decoys for a ring signature that satisfy age and factor constraints.
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
/// # Returns
///
/// A vector of selected decoy UTXOs, or an error if constraints cannot be
/// satisfied.
///
/// # Constraints Applied
///
/// 1. **Age similarity**: `decoy_age > real_age / max_age_ratio && decoy_age <
///    real_age * max_age_ratio`
/// 2. **Factor ceiling**: `decoy_factor <= real_factor * max_factor_ratio`
///
/// # Example
///
/// ```ignore
/// let config = DecoySelectionConfig::default();
/// let decoys = select_decoys(
///     &real_utxo,
///     &utxo_pool,
///     current_block,
///     &cluster_wealth,
///     total_supply,
///     &config,
/// )?;
/// ```
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
    if real_age == 0 {
        return Err(DecoySelectionError::InvalidRealUtxo(
            "UTXO has zero age".to_string(),
        ));
    }

    let real_factor = real_utxo.cluster_factor_global(cluster_wealth, total_supply);

    // Calculate age bounds
    let min_age = (real_age as f64 / config.max_age_ratio).ceil() as u64;
    let max_age = (real_age as f64 * config.max_age_ratio).floor() as u64;

    // Calculate maximum allowed factor for decoys
    let max_allowed_factor = real_factor * config.max_factor_ratio;

    let decoys_needed = config.decoys_needed();

    // Filter candidates that meet all constraints
    let eligible: Vec<UtxoCandidate> = utxo_pool
        .iter()
        .filter(|candidate| {
            // Skip if same as real UTXO
            if candidate.id == real_utxo.id {
                return false;
            }

            let candidate_age = candidate.age(current_block);

            // Age constraint: within max_age_ratio of real age
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

    // Select decoys (for now, take first N; could add random selection)
    Ok(eligible.into_iter().take(decoys_needed).collect())
}

/// Select decoys with fallback to relaxed constraints.
///
/// If strict constraints cannot be satisfied, progressively relaxes them
/// while ensuring some level of privacy protection.
///
/// # Fallback Strategy
///
/// 1. Try with default constraints (2x age, 1.5x factor)
/// 2. Relax age constraint to 3x
/// 3. Relax factor constraint to 2.0x
/// 4. Relax both to 4x age, 2.5x factor
/// 5. If still insufficient, return error with best available count
///
/// # Warning
///
/// Using relaxed constraints reduces privacy. The caller should consider
/// warning the user or refusing the transaction if constraints are too relaxed.
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

    // Fallback 1: Relax age constraint
    let relaxed_age = DecoySelectionConfig {
        ring_size: config.ring_size,
        max_age_ratio: 3.0,
        max_factor_ratio: config.max_factor_ratio,
    };

    if let Ok(decoys) = select_decoys(
        real_utxo,
        utxo_pool,
        current_block,
        cluster_wealth,
        total_supply,
        &relaxed_age,
    ) {
        return Ok((decoys, true));
    }

    // Fallback 2: Relax factor constraint
    let relaxed_factor = DecoySelectionConfig {
        ring_size: config.ring_size,
        max_age_ratio: config.max_age_ratio,
        max_factor_ratio: 2.0,
    };

    if let Ok(decoys) = select_decoys(
        real_utxo,
        utxo_pool,
        current_block,
        cluster_wealth,
        total_supply,
        &relaxed_factor,
    ) {
        return Ok((decoys, true));
    }

    // Fallback 3: Relax both constraints significantly
    let relaxed_both = DecoySelectionConfig {
        ring_size: config.ring_size,
        max_age_ratio: 4.0,
        max_factor_ratio: 2.5,
    };

    match select_decoys(
        real_utxo,
        utxo_pool,
        current_block,
        cluster_wealth,
        total_supply,
        &relaxed_both,
    ) {
        Ok(decoys) => Ok((decoys, true)),
        Err(e) => Err(e),
    }
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

    let min_age = (real_age as f64 / config.max_age_ratio).ceil() as u64;
    let max_age = (real_age as f64 * config.max_age_ratio).floor() as u64;
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

    #[test]
    fn test_config_defaults() {
        let config = DecoySelectionConfig::default();
        assert_eq!(config.ring_size, 11);
        assert_eq!(config.max_age_ratio, 2.0);
        assert_eq!(config.max_factor_ratio, 1.5);
        assert_eq!(config.decoys_needed(), 10);
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
        // Empty tags = 0% attribution = 1.0 factor
        assert!((factor - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cluster_factor_full_attribution() {
        let utxo = create_utxo(1, 5_000, &[(1, TAG_WEIGHT_SCALE)]);
        let factor = utxo.cluster_factor();
        // 100% attribution = 6.0 factor
        assert!((factor - 6.0).abs() < 0.001);
    }

    #[test]
    fn test_cluster_factor_partial_attribution() {
        let utxo = create_utxo(1, 5_000, &[(1, TAG_WEIGHT_SCALE / 2)]);
        let factor = utxo.cluster_factor();
        // 50% attribution = 3.5 factor
        assert!((factor - 3.5).abs() < 0.001);
    }

    #[test]
    fn test_select_decoys_basic() {
        let real_utxo = create_utxo(0, 5_000, &[(1, TAG_WEIGHT_SCALE / 2)]);
        let cluster_wealth = create_cluster_wealth();

        // Create pool of eligible decoys
        let mut pool = Vec::new();
        for i in 1..=15 {
            // Similar age (within 2x), similar factor
            pool.push(create_utxo(
                i,
                4_000 + i as u64 * 100,
                &[(1, TAG_WEIGHT_SCALE / 2)],
            ));
        }

        let config = DecoySelectionConfig {
            ring_size: 11,
            max_age_ratio: 2.0,
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

        assert!(result.is_ok());
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 10); // ring_size - 1
    }

    #[test]
    fn test_select_decoys_age_filtering() {
        let real_utxo = create_utxo(0, 9_000, &[]); // Age: 1000 blocks
        let cluster_wealth = create_cluster_wealth();

        // Pool with various ages
        let pool = vec![
            create_utxo(1, 9_800, &[]), // Age: 200 - too young (< 500)
            create_utxo(2, 9_500, &[]), // Age: 500 - at lower bound
            create_utxo(3, 8_500, &[]), // Age: 1500 - good
            create_utxo(4, 8_000, &[]), // Age: 2000 - at upper bound
            create_utxo(5, 7_000, &[]), // Age: 3000 - too old (> 2000)
            create_utxo(6, 8_700, &[]), // Age: 1300 - good
            create_utxo(7, 8_200, &[]), // Age: 1800 - good
        ];

        let config = DecoySelectionConfig {
            ring_size: 4, // Need 3 decoys
            max_age_ratio: 2.0,
            max_factor_ratio: 10.0, // Relax factor constraint for this test
        };

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok());
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 3);

        // Verify all selected decoys meet age constraint
        for decoy in &decoys {
            let age = decoy.age(CURRENT_BLOCK);
            assert!(age >= 500 && age <= 2000, "Age {} out of bounds", age);
        }
    }

    #[test]
    fn test_select_decoys_factor_filtering() {
        let real_utxo = create_utxo(0, 5_000, &[(1, TAG_WEIGHT_SCALE / 4)]); // ~25% = factor ~2.25
        let cluster_wealth = create_cluster_wealth();

        // Pool with various factors
        let pool = vec![
            create_utxo(1, 5_500, &[]),                          // 0% = 1.0 - good
            create_utxo(2, 5_500, &[(1, TAG_WEIGHT_SCALE / 4)]), // 25% = ~2.25 - good
            create_utxo(3, 5_500, &[(1, TAG_WEIGHT_SCALE / 2)]), /* 50% = 3.5 - at limit (1.5 *
                                                                  * 2.25 = 3.375) */
            create_utxo(4, 5_500, &[(1, TAG_WEIGHT_SCALE)]), // 100% = 6.0 - too high
            create_utxo(5, 5_500, &[(1, TAG_WEIGHT_SCALE / 5)]), // 20% = ~2.0 - good
        ];

        let config = DecoySelectionConfig {
            ring_size: 4,
            max_age_ratio: 10.0, // Relax age constraint for this test
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

        assert!(result.is_ok());
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 3);

        // Verify decoy 4 (100% attribution) was not selected
        for decoy in &decoys {
            assert_ne!(decoy.id, [4u8; 32], "High-factor decoy should be excluded");
        }
    }

    #[test]
    fn test_select_decoys_insufficient() {
        let real_utxo = create_utxo(0, 5_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        // Pool too small
        let pool = vec![create_utxo(1, 5_500, &[]), create_utxo(2, 5_500, &[])];

        let config = DecoySelectionConfig::default(); // Needs 10 decoys

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
                required: 10,
                available: 2
            })
        ));
    }

    #[test]
    fn test_select_decoys_empty_pool() {
        let real_utxo = create_utxo(0, 5_000, &[]);
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
        let real_utxo = create_utxo(0, 5_000, &[]);
        let cluster_wealth = create_cluster_wealth();
        let pool = vec![create_utxo(1, 5_500, &[])];

        let config = DecoySelectionConfig {
            ring_size: 1, // Invalid
            max_age_ratio: 2.0,
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
    fn test_select_decoys_zero_age() {
        let real_utxo = create_utxo(0, CURRENT_BLOCK, &[]); // Created this block = 0 age
        let cluster_wealth = create_cluster_wealth();
        let pool = vec![create_utxo(1, 5_500, &[])];

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
        let real_utxo = create_utxo(0, 5_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        // Pool includes the real UTXO
        let pool = vec![
            create_utxo(0, 5_000, &[]), // Same as real
            create_utxo(1, 5_500, &[]),
            create_utxo(2, 5_500, &[]),
            create_utxo(3, 5_500, &[]),
        ];

        let config = DecoySelectionConfig {
            ring_size: 3,
            max_age_ratio: 2.0,
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

        assert!(result.is_ok());
        let decoys = result.unwrap();

        // Verify real UTXO was not selected
        for decoy in &decoys {
            assert_ne!(decoy.id, real_utxo.id, "Real UTXO should not be in decoys");
        }
    }

    #[test]
    fn test_select_decoys_with_fallback_success_first_try() {
        let real_utxo = create_utxo(0, 5_000, &[]);
        let cluster_wealth = create_cluster_wealth();

        let mut pool = Vec::new();
        for i in 1..=15 {
            pool.push(create_utxo(i, 5_000 + i as u64 * 50, &[]));
        }

        let config = DecoySelectionConfig::default();

        let result = select_decoys_with_fallback(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok());
        let (decoys, relaxed) = result.unwrap();
        assert_eq!(decoys.len(), 10);
        assert!(!relaxed, "Should succeed without relaxation");
    }

    #[test]
    fn test_select_decoys_with_fallback_relaxed() {
        let real_utxo = create_utxo(0, 5_000, &[]); // Age: 5000
        let cluster_wealth = create_cluster_wealth();

        // Pool with UTXOs outside strict age bounds but within relaxed bounds
        let mut pool = Vec::new();
        for i in 1..=15 {
            // Ages around 1500-2000 (below min 2500 with 2x, but within 3x)
            pool.push(create_utxo(i, 8_000 + i as u64 * 50, &[]));
        }

        let config = DecoySelectionConfig {
            ring_size: 11,
            max_age_ratio: 2.0, // Strict: age must be 2500-10000
            max_factor_ratio: 1.5,
        };

        let result = select_decoys_with_fallback(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok());
        let (_decoys, relaxed) = result.unwrap();
        assert!(relaxed, "Should indicate constraints were relaxed");
    }

    #[test]
    fn test_validate_decoys_all_valid() {
        let real_utxo = create_utxo(0, 5_000, &[]); // Age: 5000
        let cluster_wealth = create_cluster_wealth();

        let decoys = vec![
            create_utxo(1, 6_000, &[]), // Age: 4000 - within bounds
            create_utxo(2, 7_000, &[]), // Age: 3000 - within bounds
        ];

        let config = DecoySelectionConfig::default();

        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(violations.is_empty(), "Expected no violations");
    }

    #[test]
    fn test_validate_decoys_age_violations() {
        let real_utxo = create_utxo(0, 9_000, &[]); // Age: 1000
        let cluster_wealth = create_cluster_wealth();

        let decoys = vec![
            create_utxo(1, 9_800, &[]), // Age: 200 - too young
            create_utxo(2, 8_500, &[]), // Age: 1500 - valid
            create_utxo(3, 5_000, &[]), // Age: 5000 - too old
        ];

        let config = DecoySelectionConfig::default();

        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert_eq!(violations.len(), 2);
        assert_eq!(violations[0].0, 0); // First decoy
        assert!(violations[0].1.contains("too young"));
        assert_eq!(violations[1].0, 2); // Third decoy
        assert!(violations[1].1.contains("too old"));
    }

    #[test]
    fn test_validate_decoys_factor_violation() {
        // Real UTXO has low factor (anonymous = ~1.0)
        let real_utxo = create_utxo(0, 5_000, &[]);

        // Create cluster wealth where cluster 1 is wealthy (50% of supply)
        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50% of supply

        // Decoy has 100% attribution to the wealthy cluster
        // This gives a high factor that exceeds the 1.5x limit
        let decoys = vec![create_utxo(1, 5_500, &[(1, TAG_WEIGHT_SCALE)])];

        let config = DecoySelectionConfig::default();

        let real_factor = real_utxo.cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
        let decoy_factor = decoys[0].cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);

        // Verify our setup produces the expected violation condition
        assert!(
            decoy_factor > real_factor * config.max_factor_ratio,
            "Decoy factor {} should exceed max allowed {} (real {} * {})",
            decoy_factor,
            real_factor * config.max_factor_ratio,
            real_factor,
            config.max_factor_ratio
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
            "Should detect 1 violation, found: {:?}",
            violations
        );
        assert!(violations[0].1.contains("factor too high"));
    }

    #[test]
    fn test_error_display() {
        let e1 = DecoySelectionError::InsufficientDecoys {
            required: 10,
            available: 5,
        };
        assert!(e1.to_string().contains("10"));
        assert!(e1.to_string().contains("5"));

        let e2 = DecoySelectionError::EmptyUtxoPool;
        assert!(e2.to_string().contains("empty"));

        let e3 = DecoySelectionError::InvalidRealUtxo("test".to_string());
        assert!(e3.to_string().contains("test"));

        let e4 = DecoySelectionError::InvalidRingSize;
        assert!(e4.to_string().contains("at least 2"));
    }
}
