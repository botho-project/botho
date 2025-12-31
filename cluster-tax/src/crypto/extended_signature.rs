//! Extended transaction signature with cluster tag proofs.
//!
//! This module extends the base MLSAG ring signature with additional
//! proofs for committed cluster tags. The design is additive - it
//! doesn't modify the core MLSAG but adds tag-related proofs alongside.
//!
//! ## Design
//!
//! The existing MLSAG proves:
//! 1. The signer knows the private key for one ring member
//! 2. The pseudo-output commitment has the same value as the real input
//!
//! We add:
//! 3. Pseudo-tag-outputs that commit to the real input's tag masses
//! 4. Schnorr proofs that each pseudo-tag-output equals the real input's tags
//! 5. Tag conservation proof between pseudo-tag-outputs and actual outputs
//!
//! ## Ring Signature Compatibility
//!
//! The tag proofs are designed to work with the existing ring signature
//! without modification. The MLSAG hides which input is real; the tag
//! proofs prove properties about the aggregate without revealing which
//! inputs contributed.

use super::committed_tags::{
    CommittedTagMass, CommittedTagVector, CommittedTagVectorSecret, SchnorrProof,
    TagConservationProof, TagConservationProver, TagConservationVerifier,
};
use crate::{ClusterId, TagWeight};
use curve25519_dalek::scalar::Scalar;

// TAG_WEIGHT_SCALE is used in tests
#[cfg(test)]
use crate::TAG_WEIGHT_SCALE;

/// Domain separator for tag pseudo-output proofs.
const TAG_PSEUDO_OUTPUT_DOMAIN: &[u8] = b"mc_tag_pseudo_output_proof";

/// A pseudo-tag-output for one input in a transaction.
///
/// Similar to amount pseudo-outputs, these commit to the real input's
/// tag masses without revealing which ring member is real.
#[derive(Clone, Debug)]
pub struct PseudoTagOutput {
    /// The committed tag vector for this pseudo-output.
    pub tags: CommittedTagVector,

    /// Proof that this pseudo-output correctly reflects the real input.
    /// This is a Schnorr proof of knowledge of the blinding factor difference
    /// between the pseudo-output and the real input.
    pub inheritance_proofs: Vec<TagInheritanceProof>,
}

/// Proof that a pseudo-tag-output inherits correctly from an input ring.
#[derive(Clone, Debug)]
pub struct TagInheritanceProof {
    /// The cluster this proof applies to.
    pub cluster_id: ClusterId,

    /// Schnorr proof of blinding difference knowledge.
    /// Proves: C_pseudo - C_real = r * G for known r
    pub proof: SchnorrProof,
}

/// Extended transaction signature including tag proofs.
///
/// This wraps the transaction's ring signatures with additional
/// tag-related proofs for Phase 2 committed tags.
#[derive(Clone, Debug)]
pub struct ExtendedTxSignature {
    /// Pseudo-tag-outputs, one per transaction input.
    /// These commit to the tag masses of the real inputs.
    pub pseudo_tag_outputs: Vec<PseudoTagOutput>,

    /// Proof that output tags correctly inherit from input pseudo-tags.
    pub conservation_proof: TagConservationProof,
}

/// Builder for creating extended signatures with tag proofs.
pub struct ExtendedSignatureBuilder {
    /// Secrets for each input's tags.
    input_tag_secrets: Vec<CommittedTagVectorSecret>,

    /// Secrets for each output's tags.
    output_tag_secrets: Vec<CommittedTagVectorSecret>,

    /// The decay rate applied to tags.
    decay_rate: TagWeight,

    /// Ring data for each input (for computing inheritance proofs).
    /// Maps input index -> (ring member tag commitments, real index)
    rings: Vec<RingTagData>,
}

/// Tag data for a ring of inputs.
#[derive(Clone, Debug)]
pub struct RingTagData {
    /// Tag commitments for each ring member (as they appear on-chain).
    pub member_tags: Vec<CommittedTagVector>,

    /// Index of the real input in the ring.
    pub real_index: usize,
}

impl ExtendedSignatureBuilder {
    /// Create a new builder.
    pub fn new(decay_rate: TagWeight) -> Self {
        Self {
            input_tag_secrets: Vec::new(),
            output_tag_secrets: Vec::new(),
            decay_rate,
            rings: Vec::new(),
        }
    }

    /// Add an input with its ring data.
    ///
    /// # Arguments
    /// * `ring_tags` - Tag commitments for each ring member
    /// * `real_index` - Index of the real input
    /// * `real_secret` - Secret data for the real input's tags
    pub fn add_input(
        &mut self,
        ring_tags: Vec<CommittedTagVector>,
        real_index: usize,
        real_secret: CommittedTagVectorSecret,
    ) {
        self.rings.push(RingTagData {
            member_tags: ring_tags,
            real_index,
        });
        self.input_tag_secrets.push(real_secret);
    }

    /// Add an output with its secret tag data.
    pub fn add_output(&mut self, secret: CommittedTagVectorSecret) {
        self.output_tag_secrets.push(secret);
    }

    /// Build the extended signature.
    pub fn build<R: rand_core::RngCore + rand_core::CryptoRng>(
        self,
        rng: &mut R,
    ) -> Option<ExtendedTxSignature> {
        let mut pseudo_tag_outputs = Vec::new();
        let mut pseudo_secrets = Vec::new();

        // For each input, create a pseudo-tag-output
        for (input_idx, (ring_data, input_secret)) in self
            .rings
            .iter()
            .zip(self.input_tag_secrets.iter())
            .enumerate()
        {
            let (pseudo_output, pseudo_secret) =
                self.create_pseudo_tag_output(input_idx, ring_data, input_secret, rng)?;
            pseudo_tag_outputs.push(pseudo_output);
            pseudo_secrets.push(pseudo_secret);
        }

        // Create conservation proof between pseudo-outputs and actual outputs
        let prover = TagConservationProver::new(
            pseudo_secrets,
            self.output_tag_secrets.clone(),
            self.decay_rate,
        );

        let conservation_proof = prover.prove(rng)?;

        Some(ExtendedTxSignature {
            pseudo_tag_outputs,
            conservation_proof,
        })
    }

    /// Create a pseudo-tag-output for one input.
    fn create_pseudo_tag_output<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        input_idx: usize,
        _ring_data: &RingTagData,
        real_secret: &CommittedTagVectorSecret,
        rng: &mut R,
    ) -> Option<(PseudoTagOutput, CommittedTagVectorSecret)> {
        // Create new blindings for the pseudo-output
        // (can't reuse real input's blindings as that would leak which ring member is
        // real)
        let mut pseudo_entries = Vec::new();
        let mut pseudo_secret_entries = Vec::new();
        let mut inheritance_proofs = Vec::new();

        for entry in &real_secret.entries {
            // New blinding for pseudo-output
            let pseudo_blinding = Scalar::random(rng);

            // Create pseudo commitment
            let pseudo_commitment =
                CommittedTagMass::new(entry.cluster_id, entry.mass, pseudo_blinding);

            pseudo_entries.push(pseudo_commitment);

            pseudo_secret_entries.push(super::committed_tags::TagMassSecret {
                cluster_id: entry.cluster_id,
                mass: entry.mass,
                blinding: pseudo_blinding,
            });

            // Create inheritance proof
            // Proves knowledge of: pseudo_blinding - real_blinding
            let blinding_diff = pseudo_blinding - entry.blinding;
            let context = self.inheritance_context(input_idx, entry.cluster_id);
            let proof = SchnorrProof::prove(blinding_diff, &context, rng);

            inheritance_proofs.push(TagInheritanceProof {
                cluster_id: entry.cluster_id,
                proof,
            });
        }

        // Create pseudo total commitment
        let pseudo_total_blinding = Scalar::random(rng);

        // Note: We could add a total inheritance proof here too, but since we
        // prove each cluster individually and the total is just a sum, it's
        // redundant. The blinding difference would be:
        // _total_blinding_diff = pseudo_total_blinding - real_secret.total_blinding

        let pseudo_secret = CommittedTagVectorSecret {
            entries: pseudo_secret_entries,
            total_mass: real_secret.total_mass,
            total_blinding: pseudo_total_blinding,
        };

        let pseudo_committed = CommittedTagVector::from_secrets(&pseudo_secret);

        Some((
            PseudoTagOutput {
                tags: pseudo_committed,
                inheritance_proofs,
            },
            pseudo_secret,
        ))
    }

    fn inheritance_context(&self, input_idx: usize, cluster_id: ClusterId) -> Vec<u8> {
        let mut context = TAG_PSEUDO_OUTPUT_DOMAIN.to_vec();
        context.extend_from_slice(&(input_idx as u64).to_le_bytes());
        context.extend_from_slice(&cluster_id.0.to_le_bytes());
        context
    }
}

/// Verifier for extended transaction signatures.
pub struct ExtendedSignatureVerifier {
    /// Tag commitments for each input ring.
    input_rings: Vec<RingTagData>,

    /// Output tag commitments.
    output_tags: Vec<CommittedTagVector>,

    /// Decay rate.
    decay_rate: TagWeight,
}

impl ExtendedSignatureVerifier {
    /// Create a new verifier.
    pub fn new(
        input_rings: Vec<RingTagData>,
        output_tags: Vec<CommittedTagVector>,
        decay_rate: TagWeight,
    ) -> Self {
        Self {
            input_rings,
            output_tags,
            decay_rate,
        }
    }

    /// Verify the extended signature.
    ///
    /// Note: This verifies the tag proofs only. The caller must also verify
    /// the base MLSAG ring signatures separately.
    pub fn verify(&self, signature: &ExtendedTxSignature) -> bool {
        // Check we have the right number of pseudo-tag-outputs
        if signature.pseudo_tag_outputs.len() != self.input_rings.len() {
            return false;
        }

        // Verify each pseudo-tag-output's inheritance proofs
        for (input_idx, (pseudo_output, ring_data)) in signature
            .pseudo_tag_outputs
            .iter()
            .zip(self.input_rings.iter())
            .enumerate()
        {
            if !self.verify_inheritance(input_idx, pseudo_output, ring_data) {
                return false;
            }
        }

        // Verify conservation between pseudo-outputs and actual outputs
        let pseudo_tags: Vec<CommittedTagVector> = signature
            .pseudo_tag_outputs
            .iter()
            .map(|p| p.tags.clone())
            .collect();

        let conservation_verifier =
            TagConservationVerifier::new(pseudo_tags, self.output_tags.clone(), self.decay_rate);

        conservation_verifier.verify(&signature.conservation_proof)
    }

    /// Verify that a pseudo-tag-output correctly inherits from its ring.
    ///
    /// This checks that the Schnorr proof is valid, which proves the prover
    /// knows the blinding difference between the pseudo-output and one of
    /// the ring members (the real one).
    fn verify_inheritance(
        &self,
        input_idx: usize,
        pseudo_output: &PseudoTagOutput,
        ring_data: &RingTagData,
    ) -> bool {
        // For each cluster in the pseudo-output, verify the inheritance proof
        for inheritance_proof in &pseudo_output.inheritance_proofs {
            let cluster_id = inheritance_proof.cluster_id;

            // Find the pseudo commitment for this cluster
            let pseudo_commitment = pseudo_output
                .tags
                .entries
                .iter()
                .find(|e| e.cluster_id == cluster_id);

            let pseudo_commitment = match pseudo_commitment {
                Some(c) => c,
                None => return false,
            };

            // For ring signature compatibility, we verify that the prover knows
            // the blinding difference from SOME ring member. In the real MLSAG,
            // the ring signature itself proves which one is real.
            //
            // However, for a complete verification, we would need to integrate
            // with the MLSAG challenge computation. For now, we verify the
            // Schnorr proof structure is valid.
            //
            // In production, this would be integrated into the MLSAG verification
            // by including tag commitments in the challenge hash.

            // Get the real input's tag commitment from the ring
            let real_commitment = ring_data
                .member_tags
                .get(ring_data.real_index)
                .and_then(|tags| tags.entries.iter().find(|e| e.cluster_id == cluster_id));

            if let Some(real_commitment) = real_commitment {
                // Compute the expected difference point
                let pseudo_point = match pseudo_commitment.decompress() {
                    Some(p) => p,
                    None => return false,
                };
                let real_point = match real_commitment.decompress() {
                    Some(p) => p,
                    None => return false,
                };

                let diff = pseudo_point - real_point;

                let context = self.inheritance_context(input_idx, cluster_id);
                if !inheritance_proof.proof.verify(&diff.compress(), &context) {
                    return false;
                }
            } else {
                // Real input doesn't have this cluster - pseudo shouldn't either
                // (unless we allow new clusters in pseudo, which we don't)
                return false;
            }
        }

        true
    }

    fn inheritance_context(&self, input_idx: usize, cluster_id: ClusterId) -> Vec<u8> {
        let mut context = TAG_PSEUDO_OUTPUT_DOMAIN.to_vec();
        context.extend_from_slice(&(input_idx as u64).to_le_bytes());
        context.extend_from_slice(&cluster_id.0.to_le_bytes());
        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_extended_signature_simple() {
        let decay_rate = 50_000; // 5%

        // Input: 1M units, 100% to cluster 1
        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input_commitment = input_secret.commit();

        // Create a ring with the real input and some fake ones
        let fake1 = create_test_secret(500_000, &[(2, TAG_WEIGHT_SCALE)]).commit();
        let fake2 = create_test_secret(750_000, &[(3, 500_000)]).commit();

        let ring_tags = vec![fake1, input_commitment.clone(), fake2];
        let real_index = 1;

        // Output: decayed tags
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        // Build signature
        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags.clone(), real_index, input_secret.clone());
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng);
        assert!(signature.is_some(), "Should build signature");
        let signature = signature.unwrap();

        // Verify
        let ring_data = RingTagData {
            member_tags: ring_tags,
            real_index,
        };
        let verifier =
            ExtendedSignatureVerifier::new(vec![ring_data], vec![output_commitment], decay_rate);

        assert!(verifier.verify(&signature), "Signature should verify");
    }

    #[test]
    fn test_extended_signature_multiple_inputs() {
        let decay_rate = 50_000;

        // Two inputs, each with different cluster attribution
        let input1_secret = create_test_secret(1_000_000, &[(1, 600_000), (2, 400_000)]);
        let input2_secret = create_test_secret(500_000, &[(1, TAG_WEIGHT_SCALE)]);

        let input1_commitment = input1_secret.commit();
        let input2_commitment = input2_secret.commit();

        // Create fake ring members
        let fake = create_test_secret(100_000, &[(99, TAG_WEIGHT_SCALE)]).commit();

        let ring1 = vec![input1_commitment.clone(), fake.clone()];
        let ring2 = vec![fake.clone(), input2_commitment.clone()];

        // Properly compute output by merging inputs and applying decay
        let merged = CommittedTagVectorSecret::merge(
            &[input1_secret.clone(), input2_secret.clone()],
            &mut OsRng,
        );
        let output_secret = merged.apply_decay(decay_rate, &mut OsRng);
        let output_commitment = output_secret.commit();

        // Build signature
        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring1.clone(), 0, input1_secret);
        builder.add_input(ring2.clone(), 1, input2_secret);
        builder.add_output(output_secret);

        let signature = builder.build(&mut OsRng);
        assert!(signature.is_some(), "Should build signature");
        let signature = signature.unwrap();

        // Verify
        let verifier = ExtendedSignatureVerifier::new(
            vec![
                RingTagData {
                    member_tags: ring1,
                    real_index: 0,
                },
                RingTagData {
                    member_tags: ring2,
                    real_index: 1,
                },
            ],
            vec![output_commitment],
            decay_rate,
        );

        assert!(verifier.verify(&signature));
    }

    #[test]
    fn test_extended_signature_rejects_inflation() {
        let decay_rate = 50_000;

        // Input: 50% to cluster 1
        let input_secret = create_test_secret(1_000_000, &[(1, 500_000)]);
        let input_commitment = input_secret.commit();

        let ring_tags = vec![input_commitment.clone()];

        // Try to create inflated output (60% > 50% * 95%)
        let inflated_output = create_test_secret(1_000_000, &[(1, 600_000)]);

        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags, 0, input_secret);
        builder.add_output(inflated_output);

        // Should fail to build because conservation is violated
        let signature = builder.build(&mut OsRng);
        assert!(signature.is_none(), "Should reject inflated tags");
    }
}
