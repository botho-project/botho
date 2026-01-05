// Copyright (c) 2018-2024 The Botho Foundation

//! Cluster-based progressive fee validation.
//!
//! This module integrates the `bth-cluster-tax` crate's fee calculation into
//! transaction validation. It provides functions to:
//!
//! - Extract effective cluster wealth from input tag vectors
//! - Compute minimum fees using size-based progressive pricing
//! - Validate that transactions pay sufficient fees
//!
//! # Fee Model
//!
//! Botho uses a size-based fee model with progressive wealth taxation:
//!
//! ```text
//! fee = fee_per_byte × tx_size × cluster_factor
//! ```
//!
//! Where `cluster_factor` (1x to 6x) is determined by the sender's cluster
//! wealth, creating progressive taxation where wealthy clusters pay more.
//!
//! # Usage
//!
//! ```ignore
//! use bth_transaction_core::validation::{
//!     compute_effective_cluster_wealth, validate_cluster_fee
//! };
//!
//! // Extract cluster wealth from input tag vectors
//! let cluster_wealth = compute_effective_cluster_wealth(input_tags, input_values);
//!
//! // Validate fee meets minimum
//! validate_cluster_fee(
//!     &tx,
//!     tx_size_bytes,
//!     cluster_wealth,
//!     &fee_config,
//!     &cluster_wealth_lookup,
//! )?;
//! ```

use crate::{tx::TxOut, ClusterId, ClusterTagVector, TAG_WEIGHT_SCALE};
use alloc::collections::BTreeMap;
use bth_cluster_tax::{FeeConfig, TransactionType};

use super::error::{TransactionValidationError, TransactionValidationResult};

/// Compute the effective cluster wealth from input tag vectors.
///
/// This function aggregates the cluster wealth contribution from each input
/// based on its tag vector weights. The effective wealth is the weighted
/// sum of each cluster's total wealth, where weights come from the tag
/// vectors.
///
/// For inputs without cluster tags, their value is treated as "background"
/// and does not contribute to cluster wealth.
///
/// # Arguments
/// * `input_tx_outs` - The real inputs to the transaction (not ring decoys)
/// * `input_values` - The decrypted values of each input
/// * `cluster_wealth` - Lookup for total wealth of each cluster
///
/// # Returns
/// The effective cluster wealth for fee calculation
pub fn compute_effective_cluster_wealth(
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    cluster_wealth: &impl ClusterWealthProvider,
) -> u64 {
    let mut total_weighted_wealth: u128 = 0;
    let mut total_value: u128 = 0;

    for (tx_out, &value) in input_tx_outs.iter().zip(input_values.iter()) {
        total_value += value as u128;

        if let Some(tags) = &tx_out.cluster_tags {
            for entry in &tags.entries {
                // Contribution = (value × weight / TAG_WEIGHT_SCALE) × cluster_wealth
                let value_fraction =
                    (value as u128 * entry.weight as u128) / TAG_WEIGHT_SCALE as u128;
                let wealth = cluster_wealth.get_cluster_wealth(entry.cluster_id);
                total_weighted_wealth += value_fraction * wealth as u128;
            }
        }
        // Background portion does not contribute to cluster wealth
    }

    if total_value == 0 {
        return 0;
    }

    // Return weighted average cluster wealth
    (total_weighted_wealth / total_value) as u64
}

/// Compute effective cluster wealth from ClusterTagVector inputs directly.
///
/// Simpler version that works with pre-extracted tag vectors and values.
///
/// # Arguments
/// * `input_tags` - Tag vectors for each input
/// * `input_values` - Value of each input
/// * `cluster_wealth` - Lookup for total wealth of each cluster
pub fn compute_effective_cluster_wealth_from_tags(
    input_tags: &[&ClusterTagVector],
    input_values: &[u64],
    cluster_wealth: &impl ClusterWealthProvider,
) -> u64 {
    let mut total_weighted_wealth: u128 = 0;
    let mut total_value: u128 = 0;

    for (tags, &value) in input_tags.iter().zip(input_values.iter()) {
        total_value += value as u128;

        for entry in &tags.entries {
            let value_fraction =
                (value as u128 * entry.weight as u128) / TAG_WEIGHT_SCALE as u128;
            let wealth = cluster_wealth.get_cluster_wealth(entry.cluster_id);
            total_weighted_wealth += value_fraction * wealth as u128;
        }
    }

    if total_value == 0 {
        return 0;
    }

    (total_weighted_wealth / total_value) as u64
}

/// Extract the dominant cluster from input tag vectors.
///
/// Returns the cluster that has the highest weighted contribution across
/// all inputs. This is useful for determining which cluster "owns" the
/// transaction for progressive fee purposes.
///
/// # Arguments
/// * `input_tx_outs` - The real inputs to the transaction
/// * `input_values` - The decrypted values of each input
///
/// # Returns
/// The dominant cluster ID and its total mass, or None if all background
pub fn extract_dominant_cluster(
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
) -> Option<(ClusterId, u64)> {
    let mut cluster_masses: BTreeMap<ClusterId, u64> = BTreeMap::new();

    for (tx_out, &value) in input_tx_outs.iter().zip(input_values.iter()) {
        if let Some(tags) = &tx_out.cluster_tags {
            for entry in &tags.entries {
                let mass = (value as u128 * entry.weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;
                *cluster_masses.entry(entry.cluster_id).or_insert(0) += mass;
            }
        }
    }

    cluster_masses
        .into_iter()
        .max_by_key(|(_, mass)| *mass)
        .filter(|(_, mass)| *mass > 0)
}

/// Validate that a transaction pays sufficient cluster-based fees.
///
/// This is the main integration point between transaction validation and
/// cluster-tax fee calculation. It computes the minimum required fee based
/// on transaction size and cluster wealth, then checks the declared fee.
///
/// # Arguments
/// * `declared_fee` - The fee declared in the transaction
/// * `tx_size_bytes` - Size of the serialized transaction in bytes
/// * `input_tx_outs` - The real inputs to the transaction
/// * `input_values` - The decrypted values of each input
/// * `num_memos` - Number of outputs with encrypted memos
/// * `fee_config` - The fee configuration
/// * `cluster_wealth` - Lookup for total wealth of each cluster
///
/// # Returns
/// * `Ok(())` if fee is sufficient
/// * `Err(InsufficientClusterFee)` if fee is too low
pub fn validate_cluster_fee(
    declared_fee: u64,
    tx_size_bytes: usize,
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    num_memos: usize,
    fee_config: &FeeConfig,
    cluster_wealth: &impl ClusterWealthProvider,
) -> TransactionValidationResult<()> {
    let effective_wealth =
        compute_effective_cluster_wealth(input_tx_outs, input_values, cluster_wealth);

    let minimum_fee =
        fee_config.compute_fee(TransactionType::Hidden, tx_size_bytes, effective_wealth, num_memos);

    if declared_fee < minimum_fee {
        return Err(TransactionValidationError::InsufficientClusterFee {
            required: minimum_fee,
            actual: declared_fee,
            cluster_wealth: effective_wealth,
        });
    }

    Ok(())
}

/// Validate cluster fee with dynamic base adjustment.
///
/// This version uses the dynamic fee base for congestion control,
/// adjusting the fee requirement based on network load.
///
/// # Arguments
/// * `declared_fee` - The fee declared in the transaction
/// * `tx_size_bytes` - Size of the serialized transaction in bytes
/// * `input_tx_outs` - The real inputs to the transaction
/// * `input_values` - The decrypted values of each input
/// * `num_memos` - Number of outputs with encrypted memos
/// * `fee_config` - The fee configuration
/// * `cluster_wealth` - Lookup for total wealth of each cluster
/// * `dynamic_base` - Current dynamic fee base (nanoBTH per byte)
pub fn validate_cluster_fee_dynamic(
    declared_fee: u64,
    tx_size_bytes: usize,
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    num_memos: usize,
    fee_config: &FeeConfig,
    cluster_wealth: &impl ClusterWealthProvider,
    dynamic_base: u64,
) -> TransactionValidationResult<()> {
    let effective_wealth =
        compute_effective_cluster_wealth(input_tx_outs, input_values, cluster_wealth);

    let minimum_fee = fee_config.compute_fee_with_dynamic_base(
        TransactionType::Hidden,
        tx_size_bytes,
        effective_wealth,
        num_memos,
        dynamic_base,
    );

    if declared_fee < minimum_fee {
        return Err(TransactionValidationError::InsufficientClusterFee {
            required: minimum_fee,
            actual: declared_fee,
            cluster_wealth: effective_wealth,
        });
    }

    Ok(())
}

/// Compute the cluster factor for a transaction.
///
/// Returns the fee multiplier (1x to 6x) based on the effective cluster
/// wealth of the inputs.
///
/// # Arguments
/// * `input_tx_outs` - The real inputs to the transaction
/// * `input_values` - The decrypted values of each input
/// * `fee_config` - The fee configuration
/// * `cluster_wealth` - Lookup for total wealth of each cluster
///
/// # Returns
/// The cluster factor in FACTOR_SCALE units (1000 = 1x, 6000 = 6x)
pub fn compute_cluster_factor(
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    fee_config: &FeeConfig,
    cluster_wealth: &impl ClusterWealthProvider,
) -> u64 {
    let effective_wealth =
        compute_effective_cluster_wealth(input_tx_outs, input_values, cluster_wealth);
    fee_config.cluster_factor(effective_wealth)
}

/// Trait for looking up cluster wealth.
///
/// Implementors provide access to the current total wealth attributed to
/// each cluster in the system.
pub trait ClusterWealthProvider {
    /// Get the total wealth attributed to a cluster.
    fn get_cluster_wealth(&self, cluster_id: ClusterId) -> u64;
}

/// Simple in-memory cluster wealth map.
///
/// Useful for testing and for validators that cache cluster wealth data.
pub struct ClusterWealthMap {
    wealth: BTreeMap<ClusterId, u64>,
}

impl ClusterWealthMap {
    /// Create an empty wealth map.
    pub fn new() -> Self {
        Self {
            wealth: BTreeMap::new(),
        }
    }

    /// Create from a BTreeMap.
    pub fn from_map(wealth: BTreeMap<ClusterId, u64>) -> Self {
        Self { wealth }
    }

    /// Set the wealth for a cluster.
    pub fn set(&mut self, cluster_id: ClusterId, wealth: u64) {
        self.wealth.insert(cluster_id, wealth);
    }

    /// Get the wealth for a cluster.
    pub fn get(&self, cluster_id: ClusterId) -> u64 {
        self.wealth.get(&cluster_id).copied().unwrap_or(0)
    }
}

impl Default for ClusterWealthMap {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterWealthProvider for ClusterWealthMap {
    fn get_cluster_wealth(&self, cluster_id: ClusterId) -> u64 {
        self.get(cluster_id)
    }
}

/// Adapter to use the existing ClusterWealthLookup trait.
impl<T> ClusterWealthProvider for T
where
    T: super::validate::ClusterWealthLookup,
{
    fn get_cluster_wealth(&self, cluster_id: ClusterId) -> u64 {
        super::validate::ClusterWealthLookup::get_cluster_wealth(self, cluster_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::ClusterTagEntry;

    fn make_tag_vector(entries: &[(u64, u32)]) -> ClusterTagVector {
        ClusterTagVector {
            entries: entries
                .iter()
                .map(|(id, weight)| ClusterTagEntry {
                    cluster_id: ClusterId(*id),
                    weight: *weight,
                })
                .collect(),
            decay_state: None,
        }
    }

    #[test]
    fn test_effective_wealth_single_cluster() {
        let tags = make_tag_vector(&[(1, TAG_WEIGHT_SCALE)]); // 100% cluster 1
        let input_tags = vec![&tags];
        let input_values = vec![1_000_000u64];

        let mut wealth_map = ClusterWealthMap::new();
        wealth_map.set(ClusterId(1), 10_000_000);

        let effective = compute_effective_cluster_wealth_from_tags(
            &input_tags,
            &input_values,
            &wealth_map,
        );

        // 100% of value is in cluster 1 with wealth 10M
        assert_eq!(effective, 10_000_000);
    }

    #[test]
    fn test_effective_wealth_mixed_clusters() {
        // 50% cluster 1, 30% cluster 2, 20% background
        let tags = make_tag_vector(&[
            (1, 500_000),  // 50%
            (2, 300_000),  // 30%
        ]);
        let input_tags = vec![&tags];
        let input_values = vec![1_000_000u64];

        let mut wealth_map = ClusterWealthMap::new();
        wealth_map.set(ClusterId(1), 10_000_000);
        wealth_map.set(ClusterId(2), 5_000_000);

        let effective = compute_effective_cluster_wealth_from_tags(
            &input_tags,
            &input_values,
            &wealth_map,
        );

        // 50% × 10M + 30% × 5M = 5M + 1.5M = 6.5M
        assert_eq!(effective, 6_500_000);
    }

    #[test]
    fn test_effective_wealth_background_only() {
        let tags = ClusterTagVector::empty();
        let input_tags = vec![&tags];
        let input_values = vec![1_000_000u64];

        let wealth_map = ClusterWealthMap::new();

        let effective = compute_effective_cluster_wealth_from_tags(
            &input_tags,
            &input_values,
            &wealth_map,
        );

        // All background = 0 cluster wealth
        assert_eq!(effective, 0);
    }

    #[test]
    fn test_effective_wealth_multiple_inputs() {
        // Input 1: 100% cluster 1, value 600,000
        let tags1 = make_tag_vector(&[(1, TAG_WEIGHT_SCALE)]);
        // Input 2: 100% cluster 2, value 400,000
        let tags2 = make_tag_vector(&[(2, TAG_WEIGHT_SCALE)]);

        let input_tags = vec![&tags1, &tags2];
        let input_values = vec![600_000u64, 400_000u64];

        let mut wealth_map = ClusterWealthMap::new();
        wealth_map.set(ClusterId(1), 10_000_000);
        wealth_map.set(ClusterId(2), 5_000_000);

        let effective = compute_effective_cluster_wealth_from_tags(
            &input_tags,
            &input_values,
            &wealth_map,
        );

        // Weighted average: (600K × 10M + 400K × 5M) / 1M = (6T + 2T) / 1M = 8M
        assert_eq!(effective, 8_000_000);
    }

    #[test]
    fn test_dominant_cluster_single() {
        let tags = make_tag_vector(&[(42, TAG_WEIGHT_SCALE)]);

        // Create a mock TxOut with cluster tags
        // Since we can't easily construct a full TxOut in tests, we'll test
        // the tag-based version
        let dominant = extract_dominant_cluster_from_tags(&[&tags], &[1_000_000]);

        assert_eq!(dominant, Some((ClusterId(42), 1_000_000)));
    }

    #[test]
    fn test_dominant_cluster_multiple() {
        // 60% cluster 1, 40% cluster 2
        let tags = make_tag_vector(&[
            (1, 600_000),
            (2, 400_000),
        ]);

        let dominant = extract_dominant_cluster_from_tags(&[&tags], &[1_000_000]);

        assert_eq!(dominant, Some((ClusterId(1), 600_000)));
    }

    #[test]
    fn test_dominant_cluster_background() {
        let tags = ClusterTagVector::empty();
        let dominant = extract_dominant_cluster_from_tags(&[&tags], &[1_000_000]);
        assert_eq!(dominant, None);
    }

    #[test]
    fn test_cluster_fee_sufficient() {
        let tags = make_tag_vector(&[(1, TAG_WEIGHT_SCALE)]);
        let input_tags = vec![&tags];
        let input_values = vec![1_000_000u64];

        let mut wealth_map = ClusterWealthMap::new();
        wealth_map.set(ClusterId(1), 10_000_000);

        let fee_config = FeeConfig::default();

        // Calculate minimum fee
        let effective_wealth = compute_effective_cluster_wealth_from_tags(
            &input_tags,
            &input_values,
            &wealth_map,
        );
        let min_fee = fee_config.compute_fee(TransactionType::Hidden, 4000, effective_wealth, 0);

        // Should pass with exact minimum
        let result = validate_cluster_fee_from_tags(
            min_fee,
            4000,
            &input_tags,
            &input_values,
            0,
            &fee_config,
            &wealth_map,
        );
        assert!(result.is_ok());

        // Should pass with more than minimum
        let result = validate_cluster_fee_from_tags(
            min_fee + 100,
            4000,
            &input_tags,
            &input_values,
            0,
            &fee_config,
            &wealth_map,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_cluster_fee_insufficient() {
        let tags = make_tag_vector(&[(1, TAG_WEIGHT_SCALE)]);
        let input_tags = vec![&tags];
        let input_values = vec![1_000_000u64];

        let mut wealth_map = ClusterWealthMap::new();
        wealth_map.set(ClusterId(1), 100_000_000); // Very wealthy cluster

        let fee_config = FeeConfig::default();

        // Should fail with zero fee
        let result = validate_cluster_fee_from_tags(
            0,
            4000,
            &input_tags,
            &input_values,
            0,
            &fee_config,
            &wealth_map,
        );
        assert!(matches!(
            result,
            Err(TransactionValidationError::InsufficientClusterFee { .. })
        ));
    }

    // Helper function for tag-based tests
    fn extract_dominant_cluster_from_tags(
        input_tags: &[&ClusterTagVector],
        input_values: &[u64],
    ) -> Option<(ClusterId, u64)> {
        let mut cluster_masses: BTreeMap<ClusterId, u64> = BTreeMap::new();

        for (tags, &value) in input_tags.iter().zip(input_values.iter()) {
            for entry in &tags.entries {
                let mass = (value as u128 * entry.weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;
                *cluster_masses.entry(entry.cluster_id).or_insert(0) += mass;
            }
        }

        cluster_masses
            .into_iter()
            .max_by_key(|(_, mass)| *mass)
            .filter(|(_, mass)| *mass > 0)
    }

    // Helper function for tag-based validation
    fn validate_cluster_fee_from_tags(
        declared_fee: u64,
        tx_size_bytes: usize,
        input_tags: &[&ClusterTagVector],
        input_values: &[u64],
        num_memos: usize,
        fee_config: &FeeConfig,
        cluster_wealth: &impl ClusterWealthProvider,
    ) -> TransactionValidationResult<()> {
        let effective_wealth =
            compute_effective_cluster_wealth_from_tags(input_tags, input_values, cluster_wealth);

        let minimum_fee = fee_config.compute_fee(
            TransactionType::Hidden,
            tx_size_bytes,
            effective_wealth,
            num_memos,
        );

        if declared_fee < minimum_fee {
            return Err(TransactionValidationError::InsufficientClusterFee {
                required: minimum_fee,
                actual: declared_fee,
                cluster_wealth: effective_wealth,
            });
        }

        Ok(())
    }
}
