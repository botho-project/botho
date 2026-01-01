//! Transaction validation for committed cluster tags (Phase 2).
//!
//! This module provides the validation API for transactions using committed
//! (privacy-preserving) cluster tags. The validation verifies zero-knowledge
//! proofs of tag inheritance and conservation without revealing the actual
//! tag values.

use crate::{
    crypto::{CommittedTagVector, ExtendedSignatureVerifier, ExtendedTxSignature, RingTagData},
    TagWeight,
};

/// Error type for committed tag validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommittedTagValidationError {
    /// Wrong number of pseudo-tag-outputs for the number of inputs.
    PseudoOutputCountMismatch { expected: usize, actual: usize },

    /// Inheritance proof failed for an input.
    InvalidInheritanceProof { input_index: usize },

    /// Tag conservation proof is invalid.
    InvalidConservationProof,

    /// A committed tag commitment is malformed.
    InvalidCommitment,
}

/// Result type for committed tag validation.
pub type CommittedTagValidationResult<T> = Result<T, CommittedTagValidationError>;

/// Validate committed cluster tags for a transaction.
///
/// This verifies that:
/// 1. Each pseudo-tag-output correctly inherits from one ring member
/// 2. Output tags conserve mass (with decay) from input pseudo-tags
///
/// # Arguments
/// * `input_ring_tags` - Committed tag vectors for each input ring
/// * `output_tags` - Committed tag vectors for each output
/// * `signature` - The extended transaction signature with tag proofs
/// * `decay_rate` - Tag decay rate (parts per TAG_WEIGHT_SCALE)
///
/// # Returns
/// * `Ok(())` if all proofs verify
/// * `Err(CommittedTagValidationError)` if any proof fails
pub fn validate_committed_tags(
    input_ring_tags: &[RingTagData],
    output_tags: &[CommittedTagVector],
    signature: &ExtendedTxSignature,
    decay_rate: TagWeight,
) -> CommittedTagValidationResult<()> {
    // Check pseudo-output count matches input count
    if signature.pseudo_tag_outputs.len() != input_ring_tags.len() {
        return Err(CommittedTagValidationError::PseudoOutputCountMismatch {
            expected: input_ring_tags.len(),
            actual: signature.pseudo_tag_outputs.len(),
        });
    }

    // Create verifier and run verification
    let verifier =
        ExtendedSignatureVerifier::new(input_ring_tags.to_vec(), output_tags.to_vec(), decay_rate);

    if verifier.verify(signature) {
        Ok(())
    } else {
        // The verifier doesn't give us detailed failure info, so we return
        // a generic conservation proof error. In production, we might want
        // more granular error reporting.
        Err(CommittedTagValidationError::InvalidConservationProof)
    }
}

/// Validate that committed tag vectors on outputs are structurally valid.
///
/// This checks that all commitments can be decompressed to valid curve points.
/// It does NOT verify any proofs - use `validate_committed_tags` for that.
pub fn validate_committed_tag_structure(
    outputs: &[CommittedTagVector],
) -> CommittedTagValidationResult<()> {
    for output in outputs {
        // Check that the total commitment is valid
        if output.total_commitment.decompress().is_none() {
            return Err(CommittedTagValidationError::InvalidCommitment);
        }

        // Check that each entry commitment is valid
        for entry in &output.entries {
            if entry.decompress().is_none() {
                return Err(CommittedTagValidationError::InvalidCommitment);
            }
        }
    }

    Ok(())
}

/// Configuration for committed tag validation.
#[derive(Clone, Debug)]
pub struct CommittedTagConfig {
    /// Tag decay rate (parts per TAG_WEIGHT_SCALE).
    /// Default: 50,000 (5%)
    pub decay_rate: TagWeight,
}

impl Default for CommittedTagConfig {
    fn default() -> Self {
        Self {
            decay_rate: 50_000, // 5%
        }
    }
}

// ============================================================================
// Phase 2/3: Complete Committed Transaction Validation
// ============================================================================

use crate::crypto::{
    CommittedFeeProof, CommittedFeeProofVerifier, TagConservationProof, TagConservationVerifier,
};
use crate::fee_curve::ZkFeeCurve;
use crate::ClusterId;
use std::collections::HashMap;

/// Complete validation result for Phase 2 committed transactions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommittedTransactionError {
    /// Tag conservation proof failed
    ConservationProofInvalid,
    /// Fee proof failed
    FeeProofInvalid,
    /// Commitment structure invalid
    InvalidCommitment,
    /// Missing required proof
    MissingProof,
}

/// Result type for committed transaction validation.
pub type CommittedTransactionResult<T> = Result<T, CommittedTransactionError>;

/// Complete Phase 2 transaction validation with committed tags.
///
/// This function validates both tag conservation and fee sufficiency
/// using zero-knowledge proofs, without revealing the actual tag values
/// or wealth levels.
///
/// # Arguments
/// * `input_ring_tags` - Committed tag vectors for each input ring
/// * `output_tags` - Committed tag vectors for each output
/// * `tx_signature` - Extended transaction signature with tag proofs
/// * `tag_conservation_proof` - Proof of tag mass conservation with decay
/// * `fee_proof` - Proof of fee sufficiency for committed wealth
/// * `cluster_wealth` - Public cluster wealth values
/// * `fee_curve` - ZK-compatible fee curve
/// * `fee_paid` - Public fee amount paid
/// * `base_fee` - Public base fee (size-based)
/// * `decay_rate` - Tag decay rate
///
/// # Returns
/// * `Ok(())` if all proofs verify
/// * `Err(CommittedTransactionError)` if any validation fails
pub fn validate_committed_transaction(
    input_commitments: &[CommittedTagVector],
    output_commitments: &[CommittedTagVector],
    tag_conservation_proof: &TagConservationProof,
    fee_proof: &CommittedFeeProof,
    cluster_wealth: &HashMap<ClusterId, u64>,
    fee_curve: &ZkFeeCurve,
    fee_paid: u64,
    base_fee: u64,
    decay_rate: TagWeight,
) -> CommittedTransactionResult<()> {
    // 1. Validate commitment structure
    validate_commitment_structure(input_commitments)?;
    validate_commitment_structure(output_commitments)?;

    // 2. Verify tag conservation proof
    let conservation_verifier = TagConservationVerifier::new(
        input_commitments.to_vec(),
        output_commitments.to_vec(),
        decay_rate,
    );

    if !conservation_verifier.verify(tag_conservation_proof) {
        return Err(CommittedTransactionError::ConservationProofInvalid);
    }

    // 3. Verify fee proof
    let fee_verifier = CommittedFeeProofVerifier::new(
        input_commitments.to_vec(),
        cluster_wealth.clone(),
        fee_curve.clone(),
        fee_paid,
        base_fee,
    );

    if !fee_verifier.verify(fee_proof) {
        return Err(CommittedTransactionError::FeeProofInvalid);
    }

    Ok(())
}

// Helper to validate commitment structure (for transaction validation)
fn validate_commitment_structure(
    commitments: &[CommittedTagVector],
) -> CommittedTransactionResult<()> {
    for commitment in commitments {
        if commitment.total_commitment.decompress().is_none() {
            return Err(CommittedTransactionError::InvalidCommitment);
        }
        for entry in &commitment.entries {
            if entry.decompress().is_none() {
                return Err(CommittedTransactionError::InvalidCommitment);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{CommittedTagVectorSecret, ExtendedSignatureBuilder},
        ClusterId, TAG_WEIGHT_SCALE,
    };
    use rand_core::OsRng;
    use std::collections::HashMap;

    fn create_test_secret(value: u64, clusters: &[(u64, u32)]) -> CommittedTagVectorSecret {
        let mut tags = HashMap::new();
        for &(cluster_id, weight) in clusters {
            tags.insert(ClusterId(cluster_id), weight);
        }
        CommittedTagVectorSecret::from_plaintext(value, &tags, &mut OsRng)
    }

    #[test]
    fn test_validate_committed_tags_simple() {
        let decay_rate = 50_000;

        // Create input with 100% to cluster 1
        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input_commitment = input_secret.commit();

        // Create ring with real input and fake
        let fake = create_test_secret(500_000, &[(2, TAG_WEIGHT_SCALE)]).commit();
        let ring_tags = vec![input_commitment.clone(), fake];
        let real_index = 0;

        // Create decayed output
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        // Build signature
        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags.clone(), real_index, input_secret);
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng).expect("Should build signature");

        // Validate
        let ring_data = RingTagData {
            member_tags: ring_tags,
            real_index,
        };

        let result =
            validate_committed_tags(&[ring_data], &[output_commitment], &signature, decay_rate);

        assert!(result.is_ok(), "Should validate: {:?}", result);
    }

    #[test]
    fn test_validate_wrong_pseudo_output_count() {
        let decay_rate = 50_000;

        // Create input
        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input_commitment = input_secret.commit();
        let ring_tags = vec![input_commitment.clone()];

        // Create output
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        // Build signature
        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags.clone(), 0, input_secret);
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng).expect("Should build signature");

        // Try to validate with wrong number of ring inputs
        let ring_data1 = RingTagData {
            member_tags: ring_tags.clone(),
            real_index: 0,
        };
        let ring_data2 = RingTagData {
            member_tags: ring_tags,
            real_index: 0,
        };

        let result = validate_committed_tags(
            &[ring_data1, ring_data2], // 2 rings but signature has 1 pseudo-output
            &[output_commitment],
            &signature,
            decay_rate,
        );

        assert!(matches!(
            result,
            Err(CommittedTagValidationError::PseudoOutputCountMismatch { .. })
        ));
    }

    #[test]
    fn test_validate_structure_valid() {
        let secret = create_test_secret(1_000_000, &[(1, 500_000), (2, 300_000)]);
        let commitment = secret.commit();

        let result = validate_committed_tag_structure(&[commitment]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_structure_empty_outputs() {
        let result = validate_committed_tag_structure(&[]);
        assert!(result.is_ok(), "Empty outputs should be valid");
    }

    #[test]
    fn test_validate_structure_multiple_outputs() {
        let secret1 = create_test_secret(500_000, &[(1, TAG_WEIGHT_SCALE)]);
        let secret2 = create_test_secret(500_000, &[(2, TAG_WEIGHT_SCALE)]);

        let result = validate_committed_tag_structure(&[secret1.commit(), secret2.commit()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_committed_tag_config_default() {
        let config = CommittedTagConfig::default();
        assert_eq!(config.decay_rate, 50_000); // 5%
    }

    #[test]
    fn test_validate_multiple_inputs() {
        let decay_rate = 50_000;

        // Create two inputs from different clusters
        let input1_secret = create_test_secret(500_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input2_secret = create_test_secret(500_000, &[(2, TAG_WEIGHT_SCALE)]);

        let ring1 = vec![input1_secret.commit()];
        let ring2 = vec![input2_secret.commit()];

        // Create decayed output combining both
        let output1_secret = input1_secret.apply_decay(decay_rate, &mut OsRng);
        let output2_secret = input2_secret.apply_decay(decay_rate, &mut OsRng);

        // Get commitments before moving secrets
        let output1_commit = output1_secret.commit();
        let output2_commit = output2_secret.commit();

        // Build signature with two inputs
        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring1.clone(), 0, input1_secret);
        builder.add_input(ring2.clone(), 0, input2_secret);
        builder.add_output(output1_secret);
        builder.add_output(output2_secret);

        let signature = builder.build(&mut OsRng).expect("Should build signature");

        // Validate
        let ring_data1 = RingTagData {
            member_tags: ring1,
            real_index: 0,
        };
        let ring_data2 = RingTagData {
            member_tags: ring2,
            real_index: 0,
        };

        let result = validate_committed_tags(
            &[ring_data1, ring_data2],
            &[output1_commit, output2_commit],
            &signature,
            decay_rate,
        );

        assert!(
            result.is_ok(),
            "Multiple inputs should validate: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_zero_decay_rate() {
        let decay_rate = 0; // No decay

        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let ring_tags = vec![input_secret.commit()];

        // With 0 decay, output should equal input
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags.clone(), 0, input_secret);
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng).expect("Should build signature");

        let ring_data = RingTagData {
            member_tags: ring_tags,
            real_index: 0,
        };

        let result =
            validate_committed_tags(&[ring_data], &[output_commitment], &signature, decay_rate);

        assert!(result.is_ok(), "Zero decay should validate: {:?}", result);
    }

    #[test]
    fn test_validate_high_decay_rate() {
        let decay_rate = 900_000; // 90% decay

        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let ring_tags = vec![input_secret.commit()];

        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags.clone(), 0, input_secret);
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng).expect("Should build signature");

        let ring_data = RingTagData {
            member_tags: ring_tags,
            real_index: 0,
        };

        let result =
            validate_committed_tags(&[ring_data], &[output_commitment], &signature, decay_rate);

        assert!(result.is_ok(), "High decay should validate: {:?}", result);
    }

    #[test]
    fn test_validate_error_display() {
        // Test that error types are distinguishable
        let e1 = CommittedTagValidationError::PseudoOutputCountMismatch {
            expected: 2,
            actual: 1,
        };
        let e2 = CommittedTagValidationError::InvalidInheritanceProof { input_index: 0 };
        let e3 = CommittedTagValidationError::InvalidConservationProof;
        let e4 = CommittedTagValidationError::InvalidCommitment;

        assert_ne!(e1, e2);
        assert_ne!(e2, e3);
        assert_ne!(e3, e4);

        // Test debug output works
        let _ = format!("{:?}", e1);
        let _ = format!("{:?}", e2);
        let _ = format!("{:?}", e3);
        let _ = format!("{:?}", e4);
    }

    #[test]
    fn test_validate_with_mixed_clusters() {
        let decay_rate = 50_000;

        // Create input with mixed clusters
        let input_secret = create_test_secret(
            1_000_000,
            &[
                (1, 500_000), // 50%
                (2, 300_000), // 30%
                (3, 200_000), // 20%
            ],
        );
        let ring_tags = vec![input_secret.commit()];

        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags.clone(), 0, input_secret);
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng).expect("Should build signature");

        let ring_data = RingTagData {
            member_tags: ring_tags,
            real_index: 0,
        };

        let result =
            validate_committed_tags(&[ring_data], &[output_commitment], &signature, decay_rate);

        assert!(
            result.is_ok(),
            "Mixed clusters should validate: {:?}",
            result
        );
    }
}
