//! Transaction signing integration for committed cluster tags.
//!
//! This module provides the integration layer between the cluster tax
//! cryptographic primitives and the transaction signing flow.

use crate::{
    crypto::{
        CommittedTagVector, CommittedTagVectorSecret, ExtendedSignatureBuilder,
        ExtendedTxSignature, RingTagData,
    },
    TagWeight,
};

/// Input information for tag signing.
///
/// Each transaction input has an associated ring of possible inputs,
/// and the signer knows which one is real and has its secrets.
#[derive(Clone)]
pub struct TagSigningInput {
    /// Tag commitments for all ring members.
    pub ring_tags: Vec<CommittedTagVector>,

    /// Index of the real input in the ring.
    pub real_index: usize,

    /// Secret data for the real input's tags.
    pub tag_secret: CommittedTagVectorSecret,
}

/// Output information for tag signing.
#[derive(Clone)]
pub struct TagSigningOutput {
    /// The committed tag vector for this output.
    pub tag_commitment: CommittedTagVector,

    /// Secret data for this output's tags.
    pub tag_secret: CommittedTagVectorSecret,
}

/// Configuration for tag signing.
#[derive(Clone, Debug)]
pub struct TagSigningConfig {
    /// Decay rate applied to tags (parts per TAG_WEIGHT_SCALE).
    pub decay_rate: TagWeight,
}

impl Default for TagSigningConfig {
    fn default() -> Self {
        Self {
            decay_rate: 50_000, // 5%
        }
    }
}

/// Error type for tag signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagSigningError {
    /// Failed to build the extended signature.
    SignatureBuildFailed,

    /// No inputs provided.
    NoInputs,

    /// No outputs provided.
    NoOutputs,

    /// Real index out of bounds for ring.
    InvalidRealIndex {
        input: usize,
        real_index: usize,
        ring_size: usize,
    },
}

impl std::fmt::Display for TagSigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SignatureBuildFailed => write!(f, "Failed to build tag signature"),
            Self::NoInputs => write!(f, "No inputs provided"),
            Self::NoOutputs => write!(f, "No outputs provided"),
            Self::InvalidRealIndex {
                input,
                real_index,
                ring_size,
            } => {
                write!(
                    f,
                    "Input {input}: real_index {real_index} >= ring_size {ring_size}"
                )
            }
        }
    }
}

impl std::error::Error for TagSigningError {}

/// Result type for tag signing.
pub type TagSigningResult<T> = Result<T, TagSigningError>;

/// Create an extended tag signature for a transaction.
///
/// This function generates the cryptographic proofs that:
/// 1. Each pseudo-tag-output correctly commits to the real input's tags
/// 2. Output tags conserve mass (with decay) from the inputs
///
/// # Arguments
/// * `inputs` - Tag signing data for each transaction input
/// * `outputs` - Tag signing data for each transaction output
/// * `config` - Signing configuration (decay rate)
/// * `rng` - Random number generator
///
/// # Returns
/// The serialized extended tag signature bytes, suitable for inclusion
/// in SignatureRctBulletproofs.extended_tag_signature.
pub fn create_tag_signature<R: rand_core::RngCore + rand_core::CryptoRng>(
    inputs: &[TagSigningInput],
    outputs: &[TagSigningOutput],
    config: &TagSigningConfig,
    rng: &mut R,
) -> TagSigningResult<Vec<u8>> {
    // Validate inputs
    if inputs.is_empty() {
        return Err(TagSigningError::NoInputs);
    }
    if outputs.is_empty() {
        return Err(TagSigningError::NoOutputs);
    }

    for (i, input) in inputs.iter().enumerate() {
        if input.real_index >= input.ring_tags.len() {
            return Err(TagSigningError::InvalidRealIndex {
                input: i,
                real_index: input.real_index,
                ring_size: input.ring_tags.len(),
            });
        }
    }

    // Build the signature
    let mut builder = ExtendedSignatureBuilder::new(config.decay_rate);

    for input in inputs {
        builder.add_input(
            input.ring_tags.clone(),
            input.real_index,
            input.tag_secret.clone(),
        );
    }

    for output in outputs {
        builder.add_output(output.tag_secret.clone());
    }

    let signature = builder
        .build(rng)
        .ok_or(TagSigningError::SignatureBuildFailed)?;

    Ok(signature.to_bytes())
}

/// Verify an extended tag signature.
///
/// This function verifies the cryptographic proofs in a tag signature.
///
/// # Arguments
/// * `signature_bytes` - Serialized extended tag signature
/// * `input_rings` - Tag data for each input ring (only commitments, not
///   secrets)
/// * `output_tags` - Committed tag vectors for each output
/// * `decay_rate` - Expected decay rate
///
/// # Returns
/// `Ok(())` if the signature is valid, `Err` otherwise.
pub fn verify_tag_signature(
    signature_bytes: &[u8],
    input_rings: &[RingTagData],
    output_tags: &[CommittedTagVector],
    decay_rate: TagWeight,
) -> TagSigningResult<()> {
    use crate::crypto::ExtendedSignatureVerifier;

    // Deserialize
    let signature = ExtendedTxSignature::from_bytes(signature_bytes)
        .map_err(|_| TagSigningError::SignatureBuildFailed)?;

    // Verify
    let verifier =
        ExtendedSignatureVerifier::new(input_rings.to_vec(), output_tags.to_vec(), decay_rate);

    if verifier.verify(&signature) {
        Ok(())
    } else {
        Err(TagSigningError::SignatureBuildFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClusterId, TAG_WEIGHT_SCALE};
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
    fn test_create_and_verify_tag_signature() {
        let config = TagSigningConfig::default();

        // Create input
        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input_commitment = input_secret.commit();

        // Create ring with real and fake
        let fake = create_test_secret(500_000, &[(99, TAG_WEIGHT_SCALE)]).commit();
        let ring_tags = vec![input_commitment.clone(), fake.clone()];

        let input = TagSigningInput {
            ring_tags: ring_tags.clone(),
            real_index: 0,
            tag_secret: input_secret.clone(),
        };

        // Create output (properly decayed from input)
        let output_secret = input_secret.apply_decay(config.decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        let output = TagSigningOutput {
            tag_commitment: output_commitment.clone(),
            tag_secret: output_secret,
        };

        // Create signature
        let sig_bytes = create_tag_signature(&[input], &[output], &config, &mut OsRng)
            .expect("Should create signature");

        // Verify signature
        let ring_data = RingTagData {
            member_tags: ring_tags,
            real_index: 0,
        };

        verify_tag_signature(
            &sig_bytes,
            &[ring_data],
            &[output_commitment],
            config.decay_rate,
        )
        .expect("Should verify");
    }

    #[test]
    fn test_invalid_real_index() {
        let config = TagSigningConfig::default();

        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input_commitment = input_secret.commit();

        let input = TagSigningInput {
            ring_tags: vec![input_commitment],
            real_index: 5, // Out of bounds!
            tag_secret: input_secret.clone(),
        };

        let output_secret = input_secret.apply_decay(config.decay_rate, &mut OsRng);
        let output = TagSigningOutput {
            tag_commitment: output_secret.commit(),
            tag_secret: output_secret,
        };

        let result = create_tag_signature(&[input], &[output], &config, &mut OsRng);
        assert!(matches!(
            result,
            Err(TagSigningError::InvalidRealIndex { .. })
        ));
    }

    #[test]
    fn test_empty_inputs() {
        let config = TagSigningConfig::default();

        let secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let output = TagSigningOutput {
            tag_commitment: secret.commit(),
            tag_secret: secret,
        };

        let result = create_tag_signature(&[], &[output], &config, &mut OsRng);
        assert!(matches!(result, Err(TagSigningError::NoInputs)));
    }
}
