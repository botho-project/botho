//! Validation of tag inheritance in transactions.
//!
//! This module implements the rules for how tags must propagate from
//! transaction inputs to outputs, ensuring correct progressive fee computation.

use super::tagged_output::CompactTagVector;
use crate::{ClusterId, ClusterWealth, FeeCurve, FeeRateBps, TagWeight, TAG_WEIGHT_SCALE};
use std::collections::HashMap;

/// Decay rate per transaction hop (parts per million).
/// 50,000 = 5% decay, meaning 95% of tag weight survives each hop.
#[allow(dead_code)]
pub const DEFAULT_DECAY_RATE: TagWeight = 50_000;

/// Errors in tag inheritance validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagValidationError {
    /// Total input value doesn't match total output value plus fee.
    ValueMismatch {
        input_total: u64,
        output_total: u64,
        claimed_fee: u64,
    },

    /// Tag mass not conserved for a cluster (accounting for decay).
    TagMassNotConserved {
        cluster: ClusterId,
        input_mass: u64,
        expected_output_mass: u64,
        actual_output_mass: u64,
    },

    /// Fee paid is less than required by progressive rate.
    InsufficientFee { required_fee: u64, actual_fee: u64 },

    /// Total output tag weights exceed 100%.
    InvalidTagWeights { output_index: usize },
}

/// Input to the tag validation process.
#[derive(Clone, Debug)]
pub struct TaggedInput {
    /// Value of this input (in base units).
    pub value: u64,

    /// Tag vector of this input.
    pub tags: CompactTagVector,
}

/// Output to validate.
#[derive(Clone, Debug)]
pub struct TaggedOutput {
    /// Value of this output (in base units).
    pub value: u64,

    /// Tag vector claimed for this output.
    pub tags: CompactTagVector,
}

/// Result of fee computation.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct FeeComputation {
    /// Required minimum fee based on input tags.
    pub required_fee: u64,

    /// Effective fee rate (weighted average across clusters).
    pub effective_rate_bps: FeeRateBps,

    /// Per-cluster breakdown of fee contribution.
    pub cluster_contributions: HashMap<ClusterId, u64>,
}

/// Compute the required fee for a transaction based on input tags.
///
/// The fee rate is a weighted average of cluster rates, where weights
/// are the tag weights of the inputs.
pub fn compute_required_fee(
    inputs: &[TaggedInput],
    transfer_value: u64,
    cluster_wealth: &ClusterWealth,
    fee_curve: &FeeCurve,
) -> FeeComputation {
    // Compute total input value and tag masses
    let total_input_value: u64 = inputs.iter().map(|i| i.value).sum();

    // Aggregate tag masses across all inputs
    let mut total_tag_mass: HashMap<ClusterId, u64> = HashMap::new();
    for input in inputs {
        for i in 0..input.tags.count as usize {
            let cluster = ClusterId(input.tags.cluster_ids[i]);
            let weight = input.tags.weights[i];
            let mass = input.value as u128 * weight as u128 / TAG_WEIGHT_SCALE as u128;
            *total_tag_mass.entry(cluster).or_insert(0) += mass as u64;
        }
    }

    // Compute weighted fee rate
    let mut weighted_rate_sum: u128 = 0;
    let mut cluster_contributions = HashMap::new();

    for (&cluster, &mass) in &total_tag_mass {
        let cluster_w = cluster_wealth.get(cluster);
        let rate = fee_curve.rate_bps(cluster_w) as u128;

        // Contribution = (mass / total_value) * rate * transfer_value
        let contribution =
            mass as u128 * rate * transfer_value as u128 / (total_input_value as u128 * 10_000);

        weighted_rate_sum += mass as u128 * rate;
        cluster_contributions.insert(cluster, contribution as u64);
    }

    // Add background contribution
    let total_tag_weight: u64 = total_tag_mass.values().sum();
    let background_weight = (total_input_value as u128 * TAG_WEIGHT_SCALE as u128
        - total_tag_weight as u128 * total_input_value as u128 / total_input_value as u128)
        / total_input_value as u128;

    let bg_rate = fee_curve.background_rate_bps as u128;
    weighted_rate_sum += background_weight * bg_rate;

    // Effective rate = weighted_rate_sum / total_input_value
    let effective_rate_bps = if total_input_value > 0 {
        (weighted_rate_sum
            / (total_input_value as u128 * TAG_WEIGHT_SCALE as u128 / TAG_WEIGHT_SCALE as u128))
            as FeeRateBps
    } else {
        fee_curve.background_rate_bps
    };

    // Required fee
    let required_fee = (transfer_value as u128 * effective_rate_bps as u128 / 10_000) as u64;

    FeeComputation {
        required_fee,
        effective_rate_bps,
        cluster_contributions,
    }
}

/// Validate that output tags correctly inherit from input tags.
///
/// Rules:
/// 1. Value conservation: sum(inputs) = sum(outputs) + fee
/// 2. Tag mass conservation with decay: for each cluster k, sum(output_mass_k)
///    = (1 - decay) * sum(input_mass_k)
/// 3. Fee sufficiency: fee >= compute_required_fee(inputs)
///
/// Note: In Phase 1 (public tags), we can validate this directly.
/// In Phase 2 (committed tags), this becomes a ZK proof.
pub fn validate_tag_inheritance(
    inputs: &[TaggedInput],
    outputs: &[TaggedOutput],
    claimed_fee: u64,
    cluster_wealth: &ClusterWealth,
    fee_curve: &FeeCurve,
    decay_rate: TagWeight,
) -> Result<(), TagValidationError> {
    // 1. Value conservation
    let input_total: u64 = inputs.iter().map(|i| i.value).sum();
    let output_total: u64 = outputs.iter().map(|o| o.value).sum();

    if input_total != output_total + claimed_fee {
        return Err(TagValidationError::ValueMismatch {
            input_total,
            output_total,
            claimed_fee,
        });
    }

    // 2. Validate output tag weights don't exceed 100%
    for (idx, output) in outputs.iter().enumerate() {
        if output.tags.total_weight() > TAG_WEIGHT_SCALE {
            return Err(TagValidationError::InvalidTagWeights { output_index: idx });
        }
    }

    // 3. Tag mass conservation with decay
    let decay_factor = TAG_WEIGHT_SCALE - decay_rate;

    // Compute input tag masses
    let mut input_masses: HashMap<ClusterId, u64> = HashMap::new();
    for input in inputs {
        for i in 0..input.tags.count as usize {
            let cluster = ClusterId(input.tags.cluster_ids[i]);
            let weight = input.tags.weights[i];
            let mass = input.value as u128 * weight as u128 / TAG_WEIGHT_SCALE as u128;
            *input_masses.entry(cluster).or_insert(0) += mass as u64;
        }
    }

    // Compute output tag masses
    let mut output_masses: HashMap<ClusterId, u64> = HashMap::new();
    for output in outputs {
        for i in 0..output.tags.count as usize {
            let cluster = ClusterId(output.tags.cluster_ids[i]);
            let weight = output.tags.weights[i];
            let mass = output.value as u128 * weight as u128 / TAG_WEIGHT_SCALE as u128;
            *output_masses.entry(cluster).or_insert(0) += mass as u64;
        }
    }

    // Check conservation for each cluster
    for (&cluster, &input_mass) in &input_masses {
        let expected_output_mass =
            (input_mass as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;
        let actual_output_mass = output_masses.get(&cluster).copied().unwrap_or(0);

        // Allow small tolerance for rounding
        let tolerance = (input_mass / 1000).max(1);

        if actual_output_mass > expected_output_mass + tolerance {
            return Err(TagValidationError::TagMassNotConserved {
                cluster,
                input_mass,
                expected_output_mass,
                actual_output_mass,
            });
        }
    }

    // 4. Fee sufficiency
    let fee_computation = compute_required_fee(inputs, output_total, cluster_wealth, fee_curve);

    if claimed_fee < fee_computation.required_fee {
        return Err(TagValidationError::InsufficientFee {
            required_fee: fee_computation.required_fee,
            actual_fee: claimed_fee,
        });
    }

    Ok(())
}

/// Compute output tag vectors given inputs and their values.
///
/// This is the "correct" way to compute output tags that will pass validation.
/// Used by wallet software to construct valid transactions.
#[allow(dead_code)]
pub fn compute_output_tags(
    inputs: &[TaggedInput],
    output_values: &[u64],
    decay_rate: TagWeight,
) -> Vec<CompactTagVector> {
    // Total input value
    let total_input: u64 = inputs.iter().map(|i| i.value).sum();
    if total_input == 0 {
        return output_values
            .iter()
            .map(|_| CompactTagVector::empty())
            .collect();
    }

    // Aggregate input tag masses
    let mut input_masses: HashMap<ClusterId, u64> = HashMap::new();
    for input in inputs {
        for i in 0..input.tags.count as usize {
            let cluster = ClusterId(input.tags.cluster_ids[i]);
            let weight = input.tags.weights[i];
            let mass = input.value as u128 * weight as u128 / TAG_WEIGHT_SCALE as u128;
            *input_masses.entry(cluster).or_insert(0) += mass as u64;
        }
    }

    // Apply decay
    let decay_factor = TAG_WEIGHT_SCALE - decay_rate;
    let decayed_masses: HashMap<ClusterId, u64> = input_masses
        .into_iter()
        .map(|(c, m)| {
            let decayed = (m as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;
            (c, decayed)
        })
        .collect();

    // Total output value
    let total_output: u64 = output_values.iter().sum();

    // Distribute tag masses proportionally to output values
    output_values
        .iter()
        .map(|&out_value| {
            if out_value == 0 || total_output == 0 {
                return CompactTagVector::empty();
            }

            // This output gets proportional share of each tag mass
            let mut tags: HashMap<ClusterId, TagWeight> = HashMap::new();

            for (&cluster, &mass) in &decayed_masses {
                // Output's share of this cluster's mass
                let output_mass = mass as u128 * out_value as u128 / total_output as u128;

                // Convert mass to weight: weight = mass / value
                if out_value > 0 {
                    let weight =
                        (output_mass * TAG_WEIGHT_SCALE as u128 / out_value as u128) as TagWeight;
                    if weight > 0 {
                        tags.insert(cluster, weight);
                    }
                }
            }

            CompactTagVector::from_map(&tags)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cluster_wealth() -> ClusterWealth {
        let mut cw = ClusterWealth::new();
        cw.set(ClusterId(1), 50_000_000); // Above midpoint
        cw.set(ClusterId(2), 1_000_000); // Below midpoint
        cw
    }

    #[test]
    fn test_compute_output_tags_single_input() {
        let inputs = vec![TaggedInput {
            value: 1000,
            tags: CompactTagVector::single(ClusterId(1)),
        }];

        let output_values = vec![500, 500];
        let outputs = compute_output_tags(&inputs, &output_values, DEFAULT_DECAY_RATE);

        // Both outputs should have ~95% weight to cluster 1 (after 5% decay)
        assert_eq!(outputs.len(), 2);

        for output in &outputs {
            let weight = output.get(ClusterId(1));
            // 95% of 1_000_000 = 950_000
            assert!(
                weight >= 940_000 && weight <= 960_000,
                "Expected ~950000, got {weight}"
            );
        }
    }

    #[test]
    fn test_validation_passes_correct_tx() {
        let cluster_wealth = test_cluster_wealth();
        let fee_curve = FeeCurve::default_params();

        let inputs = vec![TaggedInput {
            value: 10000,
            tags: CompactTagVector::single(ClusterId(1)),
        }];

        // First compute required fee to know how much goes to outputs
        let fee_comp = compute_required_fee(&inputs, 10000, &cluster_wealth, &fee_curve);
        let fee = fee_comp.required_fee + 10; // A bit more than required

        let output_value = 10000 - fee;
        let output_tags = compute_output_tags(&inputs, &[output_value], DEFAULT_DECAY_RATE);

        let outputs = vec![TaggedOutput {
            value: output_value,
            tags: output_tags[0].clone(),
        }];

        let result = validate_tag_inheritance(
            &inputs,
            &outputs,
            fee,
            &cluster_wealth,
            &fee_curve,
            DEFAULT_DECAY_RATE,
        );

        assert!(result.is_ok(), "Validation should pass: {:?}", result);
    }

    #[test]
    fn test_validation_fails_insufficient_fee() {
        let mut cluster_wealth = ClusterWealth::new();
        cluster_wealth.set(ClusterId(1), 100_000_000); // Very large cluster = high fee

        let fee_curve = FeeCurve::default_params();

        let inputs = vec![TaggedInput {
            value: 10_000,
            tags: CompactTagVector::single(ClusterId(1)),
        }];

        let output_tags = compute_output_tags(&inputs, &[9_999], DEFAULT_DECAY_RATE);

        let outputs = vec![TaggedOutput {
            value: 9_999,
            tags: output_tags[0].clone(),
        }];

        // Try to pay only 1 unit fee (should be way too low)
        let result = validate_tag_inheritance(
            &inputs,
            &outputs,
            1,
            &cluster_wealth,
            &fee_curve,
            DEFAULT_DECAY_RATE,
        );

        assert!(matches!(
            result,
            Err(TagValidationError::InsufficientFee { .. })
        ));
    }

    #[test]
    fn test_validation_fails_tag_inflation() {
        let cluster_wealth = test_cluster_wealth();
        let fee_curve = FeeCurve::default_params();

        // Input has 50% attribution to cluster 1
        let mut input_map = HashMap::new();
        input_map.insert(ClusterId(1), 500_000); // 50%

        let inputs = vec![TaggedInput {
            value: 10000,
            tags: CompactTagVector::from_map(&input_map),
        }];

        // Input mass for cluster 1 = 10000 * 0.5 = 5000
        // After 5% decay: expected = 5000 * 0.95 = 4750

        // Try to claim 60% weight in output, which would be:
        // Output mass = 9000 * 0.6 = 5400 > 4750 (inflation!)
        let mut inflated_map = HashMap::new();
        inflated_map.insert(ClusterId(1), 600_000); // 60%

        let outputs = vec![TaggedOutput {
            value: 9000,
            tags: CompactTagVector::from_map(&inflated_map),
        }];

        let result = validate_tag_inheritance(
            &inputs,
            &outputs,
            1000,
            &cluster_wealth,
            &fee_curve,
            DEFAULT_DECAY_RATE,
        );

        assert!(
            matches!(result, Err(TagValidationError::TagMassNotConserved { .. })),
            "Expected TagMassNotConserved, got {:?}",
            result
        );
    }
}
