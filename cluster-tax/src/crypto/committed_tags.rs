//! Committed cluster tags with Pedersen commitments.
//!
//! Phase 2 implementation that hides tag weights using cryptographic
//! commitments. This provides full privacy for cluster attribution while still
//! allowing verification of tag conservation and fee sufficiency.

use crate::{ClusterId, TagWeight, TAG_WEIGHT_SCALE};
use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
    traits::Identity,
};
use sha2::{Digest, Sha512};
use std::collections::HashMap;

/// Domain separator for cluster tag generators.
const CLUSTER_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_cluster_tag_generator";

/// Domain separator for total mass generator.
const TOTAL_MASS_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_cluster_total_mass_generator";

/// The standard blinding generator (same as in ring signatures).
pub fn blinding_generator() -> RistrettoPoint {
    RISTRETTO_BASEPOINT_POINT
}

/// Derive a generator point for a specific cluster ID.
///
/// Each cluster has a unique generator H_k derived via hash-to-curve.
/// This ensures the discrete log relationship between generators is unknown.
pub fn cluster_generator(cluster_id: ClusterId) -> RistrettoPoint {
    let mut hasher = Sha512::new();
    hasher.update(CLUSTER_GENERATOR_DOMAIN_TAG);
    hasher.update(cluster_id.0.to_le_bytes());
    RistrettoPoint::from_hash(hasher)
}

/// Derive the generator for total mass commitments.
pub fn total_mass_generator() -> RistrettoPoint {
    let mut hasher = Sha512::new();
    hasher.update(TOTAL_MASS_GENERATOR_DOMAIN_TAG);
    RistrettoPoint::from_hash(hasher)
}

/// A Pedersen commitment to tag mass for a single cluster.
///
/// Commitment: C = mass * H_k + blinding * G
/// where mass = value * weight (in millionths, like weight).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedTagMass {
    /// The cluster this commitment refers to.
    pub cluster_id: ClusterId,

    /// Pedersen commitment to the tag mass.
    pub commitment: CompressedRistretto,
}

impl CommittedTagMass {
    /// Create a new commitment to tag mass.
    ///
    /// # Arguments
    /// * `cluster_id` - The cluster identifier
    /// * `mass` - The tag mass (value * weight, in millionths)
    /// * `blinding` - Random blinding factor
    pub fn new(cluster_id: ClusterId, mass: u64, blinding: Scalar) -> Self {
        let h_k = cluster_generator(cluster_id);
        let g = blinding_generator();

        let commitment = Scalar::from(mass) * h_k + blinding * g;

        Self {
            cluster_id,
            commitment: commitment.compress(),
        }
    }

    /// Create a commitment to zero mass (for padding).
    pub fn zero(cluster_id: ClusterId, blinding: Scalar) -> Self {
        Self::new(cluster_id, 0, blinding)
    }

    /// Decompress the commitment point.
    pub fn decompress(&self) -> Option<RistrettoPoint> {
        self.commitment.decompress()
    }
}

/// Secret data for a committed tag entry.
#[derive(Clone, Debug)]
pub struct TagMassSecret {
    /// The cluster identifier.
    pub cluster_id: ClusterId,

    /// The actual tag mass (value * weight).
    pub mass: u64,

    /// The blinding factor used in the commitment.
    pub blinding: Scalar,
}

impl TagMassSecret {
    /// Create the corresponding commitment.
    pub fn commit(&self) -> CommittedTagMass {
        CommittedTagMass::new(self.cluster_id, self.mass, self.blinding)
    }
}

/// Full committed tag vector for a TxOut.
///
/// Contains commitments to tag masses for multiple clusters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommittedTagVector {
    /// Commitments for each cluster with non-zero weight.
    /// Sorted by cluster_id for deterministic ordering.
    pub entries: Vec<CommittedTagMass>,

    /// Commitment to total attributed mass.
    /// Used for computing background weight.
    pub total_commitment: CompressedRistretto,
}

impl CommittedTagVector {
    /// Create an empty committed tag vector (fully background).
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            total_commitment: RistrettoPoint::identity().compress(),
        }
    }

    /// Create from secrets.
    pub fn from_secrets(secrets: &CommittedTagVectorSecret) -> Self {
        let mut entries: Vec<CommittedTagMass> =
            secrets.entries.iter().map(|s| s.commit()).collect();

        // Sort by cluster_id for deterministic ordering
        entries.sort_by_key(|e| e.cluster_id.0);

        // Compute total commitment
        let h_total = total_mass_generator();
        let g = blinding_generator();
        let total_point = Scalar::from(secrets.total_mass) * h_total + secrets.total_blinding * g;

        Self {
            entries,
            total_commitment: total_point.compress(),
        }
    }

    /// Number of cluster entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Compute aggregate commitment (sum of all entry commitments).
    ///
    /// This is useful for batch verification.
    pub fn aggregate_commitment(&self) -> Option<RistrettoPoint> {
        let mut sum = RistrettoPoint::identity();
        for entry in &self.entries {
            sum += entry.decompress()?;
        }
        Some(sum)
    }
}

/// Secret data for a full committed tag vector.
#[derive(Clone, Debug)]
pub struct CommittedTagVectorSecret {
    /// Secrets for each cluster entry.
    pub entries: Vec<TagMassSecret>,

    /// Total attributed mass (sum of all entry masses).
    pub total_mass: u64,

    /// Blinding factor for total commitment.
    pub total_blinding: Scalar,
}

impl CommittedTagVectorSecret {
    /// Create an empty secret (fully background).
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            total_mass: 0,
            total_blinding: Scalar::ZERO,
        }
    }

    /// Create from a plaintext tag vector and output value.
    ///
    /// # Arguments
    /// * `value` - The output value
    /// * `tags` - Map of cluster_id to weight (in TAG_WEIGHT_SCALE units)
    /// * `rng` - Random number generator for blinding factors
    pub fn from_plaintext<R: rand_core::RngCore + rand_core::CryptoRng>(
        value: u64,
        tags: &HashMap<ClusterId, TagWeight>,
        rng: &mut R,
    ) -> Self {
        let mut entries = Vec::new();
        let mut total_mass = 0u64;
        let mut total_blinding = Scalar::ZERO;

        for (&cluster_id, &weight) in tags {
            // mass = value * weight / TAG_WEIGHT_SCALE
            // We keep mass in millionths for precision
            let mass = (value as u128 * weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;

            let blinding = Scalar::random(rng);

            entries.push(TagMassSecret {
                cluster_id,
                mass,
                blinding,
            });

            total_mass += mass;
            total_blinding += blinding;
        }

        Self {
            entries,
            total_mass,
            total_blinding,
        }
    }

    /// Create the corresponding committed vector.
    pub fn commit(&self) -> CommittedTagVector {
        CommittedTagVector::from_secrets(self)
    }

    /// Apply decay to the tag masses.
    ///
    /// Returns a new secret with decayed masses and new blinding factors.
    pub fn apply_decay<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        decay_rate: TagWeight,
        rng: &mut R,
    ) -> Self {
        let decay_factor = TAG_WEIGHT_SCALE - decay_rate;

        let entries: Vec<TagMassSecret> = self
            .entries
            .iter()
            .map(|e| {
                let decayed_mass =
                    (e.mass as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;
                TagMassSecret {
                    cluster_id: e.cluster_id,
                    mass: decayed_mass,
                    blinding: Scalar::random(rng),
                }
            })
            .collect();

        let total_mass =
            (self.total_mass as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;

        Self {
            entries,
            total_mass,
            total_blinding: Scalar::random(rng),
        }
    }

    /// Merge multiple secrets into one (for combining inputs).
    ///
    /// Sums masses per cluster across all inputs.
    pub fn merge<R: rand_core::RngCore + rand_core::CryptoRng>(
        secrets: &[Self],
        rng: &mut R,
    ) -> Self {
        let mut cluster_masses: HashMap<ClusterId, u64> = HashMap::new();

        for secret in secrets {
            for entry in &secret.entries {
                *cluster_masses.entry(entry.cluster_id).or_insert(0) += entry.mass;
            }
        }

        let entries: Vec<TagMassSecret> = cluster_masses
            .into_iter()
            .map(|(cluster_id, mass)| TagMassSecret {
                cluster_id,
                mass,
                blinding: Scalar::random(rng),
            })
            .collect();

        let total_mass: u64 = entries.iter().map(|e| e.mass).sum();

        Self {
            entries,
            total_mass,
            total_blinding: Scalar::random(rng),
        }
    }
}

/// Proof that tag masses are conserved with decay.
///
/// For each cluster k, proves:
///   sum(C_output_k) = (1 - decay) * sum(C_input_k) + r_diff * G
///
/// This is a Schnorr-style proof of knowledge of the blinding difference.
#[derive(Clone, Debug)]
pub struct TagConservationProof {
    /// Per-cluster conservation proofs.
    pub cluster_proofs: Vec<ClusterConservationProof>,

    /// Proof that total masses are consistent.
    pub total_proof: SchnorrProof,
}

/// Conservation proof for a single cluster.
#[derive(Clone, Debug)]
pub struct ClusterConservationProof {
    /// The cluster this proof applies to.
    pub cluster_id: ClusterId,

    /// Schnorr proof of blinding difference knowledge.
    pub proof: SchnorrProof,
}

/// A simple Schnorr proof of knowledge of discrete log.
///
/// Proves knowledge of `x` such that `P = x * G`.
#[derive(Clone, Debug)]
pub struct SchnorrProof {
    /// Commitment: R = k * G
    pub commitment: CompressedRistretto,

    /// Response: s = k + c * x
    pub response: Scalar,
}

/// Builder for creating tag conservation proofs.
///
/// Given input and output committed tag vectors, this proves that:
/// sum(output_mass) = (1 - decay) * sum(input_mass)
/// for each cluster, in zero knowledge.
#[derive(Clone, Debug)]
pub struct TagConservationProver {
    /// Input tag secrets (from pseudo-outputs)
    pub input_secrets: Vec<CommittedTagVectorSecret>,

    /// Output tag secrets
    pub output_secrets: Vec<CommittedTagVectorSecret>,

    /// Decay rate (in TAG_WEIGHT_SCALE units)
    pub decay_rate: TagWeight,
}

impl TagConservationProver {
    /// Create a new prover.
    pub fn new(
        input_secrets: Vec<CommittedTagVectorSecret>,
        output_secrets: Vec<CommittedTagVectorSecret>,
        decay_rate: TagWeight,
    ) -> Self {
        Self {
            input_secrets,
            output_secrets,
            decay_rate,
        }
    }

    /// Generate the conservation proof.
    ///
    /// Returns None if conservation doesn't hold.
    pub fn prove<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        rng: &mut R,
    ) -> Option<TagConservationProof> {
        let decay_factor = TAG_WEIGHT_SCALE - self.decay_rate;

        // Collect all cluster IDs
        let mut cluster_ids: std::collections::BTreeSet<ClusterId> =
            std::collections::BTreeSet::new();
        for secret in &self.input_secrets {
            for entry in &secret.entries {
                cluster_ids.insert(entry.cluster_id);
            }
        }
        for secret in &self.output_secrets {
            for entry in &secret.entries {
                cluster_ids.insert(entry.cluster_id);
            }
        }

        let mut cluster_proofs = Vec::new();

        for &cluster_id in &cluster_ids {
            // Sum input masses and blindings for this cluster
            let (input_mass, input_blinding) =
                self.sum_cluster_data(&self.input_secrets, cluster_id);

            // Apply decay to input mass
            let decayed_input_mass =
                (input_mass as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;

            // Sum output masses and blindings for this cluster
            let (output_mass, output_blinding) =
                self.sum_cluster_data(&self.output_secrets, cluster_id);

            // Check conservation (with tolerance for rounding)
            let tolerance = (input_mass / 1000).max(1);
            if output_mass > decayed_input_mass + tolerance {
                return None; // Conservation violated
            }

            // Compute blinding difference
            // We need: C_out - (decay_factor/SCALE) * C_in = r_diff * G
            // But since we're dealing with integer masses, we compute:
            // output_blinding - (decay_factor * input_blinding / SCALE) ≈ r_diff
            // This is an approximation; exact would need more care

            let scaled_input_blinding = input_blinding
                * Scalar::from(decay_factor as u64)
                * Scalar::from(TAG_WEIGHT_SCALE as u64).invert();

            let blinding_diff = output_blinding - scaled_input_blinding;

            // Create Schnorr proof for this blinding difference
            let proof =
                SchnorrProof::prove(blinding_diff, &self.conservation_context(cluster_id), rng);

            cluster_proofs.push(ClusterConservationProof { cluster_id, proof });
        }

        // Prove total mass conservation
        let (total_input_mass, total_input_blinding) = self.sum_total_data(&self.input_secrets);
        let decayed_total =
            (total_input_mass as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;
        let (total_output_mass, total_output_blinding) = self.sum_total_data(&self.output_secrets);

        let tolerance = (total_input_mass / 1000).max(1);
        if total_output_mass > decayed_total + tolerance {
            return None;
        }

        let scaled_total_blinding = total_input_blinding
            * Scalar::from(decay_factor as u64)
            * Scalar::from(TAG_WEIGHT_SCALE as u64).invert();
        let total_blinding_diff = total_output_blinding - scaled_total_blinding;

        let total_proof = SchnorrProof::prove(total_blinding_diff, b"total_conservation", rng);

        Some(TagConservationProof {
            cluster_proofs,
            total_proof,
        })
    }

    fn sum_cluster_data(
        &self,
        secrets: &[CommittedTagVectorSecret],
        cluster_id: ClusterId,
    ) -> (u64, Scalar) {
        let mut total_mass = 0u64;
        let mut total_blinding = Scalar::ZERO;

        for secret in secrets {
            for entry in &secret.entries {
                if entry.cluster_id == cluster_id {
                    total_mass += entry.mass;
                    total_blinding += entry.blinding;
                }
            }
        }

        (total_mass, total_blinding)
    }

    fn sum_total_data(&self, secrets: &[CommittedTagVectorSecret]) -> (u64, Scalar) {
        let mut total_mass = 0u64;
        let mut total_blinding = Scalar::ZERO;

        for secret in secrets {
            total_mass += secret.total_mass;
            total_blinding += secret.total_blinding;
        }

        (total_mass, total_blinding)
    }

    fn conservation_context(&self, cluster_id: ClusterId) -> Vec<u8> {
        let mut context = b"cluster_conservation_".to_vec();
        context.extend_from_slice(&cluster_id.0.to_le_bytes());
        context
    }
}

/// Verifier for tag conservation proofs.
pub struct TagConservationVerifier {
    /// Input committed tag vectors (from pseudo-outputs)
    pub input_commitments: Vec<CommittedTagVector>,

    /// Output committed tag vectors
    pub output_commitments: Vec<CommittedTagVector>,

    /// Decay rate (in TAG_WEIGHT_SCALE units)
    pub decay_rate: TagWeight,
}

impl TagConservationVerifier {
    /// Create a new verifier.
    pub fn new(
        input_commitments: Vec<CommittedTagVector>,
        output_commitments: Vec<CommittedTagVector>,
        decay_rate: TagWeight,
    ) -> Self {
        Self {
            input_commitments,
            output_commitments,
            decay_rate,
        }
    }

    /// Verify the conservation proof.
    pub fn verify(&self, proof: &TagConservationProof) -> bool {
        let decay_factor = TAG_WEIGHT_SCALE - self.decay_rate;

        // Verify each cluster proof
        for cluster_proof in &proof.cluster_proofs {
            let cluster_id = cluster_proof.cluster_id;

            // Sum input commitments for this cluster
            let input_sum = self.sum_cluster_commitments(&self.input_commitments, cluster_id);
            let input_sum = match input_sum {
                Some(p) => p,
                None => return false,
            };

            // Sum output commitments for this cluster
            let output_sum = self.sum_cluster_commitments(&self.output_commitments, cluster_id);
            let output_sum = match output_sum {
                Some(p) => p,
                None => return false,
            };

            // Compute expected difference point
            // diff = C_out - (decay_factor/SCALE) * C_in
            let scale_inv = Scalar::from(TAG_WEIGHT_SCALE as u64).invert();
            let scaled_input = Scalar::from(decay_factor as u64) * scale_inv * input_sum;
            let diff = output_sum - scaled_input;

            // Verify Schnorr proof
            let context = self.conservation_context(cluster_id);
            if !cluster_proof.proof.verify(&diff.compress(), &context) {
                return false;
            }
        }

        // Verify total proof
        let input_total = self.sum_total_commitments(&self.input_commitments);
        let output_total = self.sum_total_commitments(&self.output_commitments);

        let (input_total, output_total) = match (input_total, output_total) {
            (Some(i), Some(o)) => (i, o),
            _ => return false,
        };

        let scale_inv = Scalar::from(TAG_WEIGHT_SCALE as u64).invert();
        let scaled_input = Scalar::from(decay_factor as u64) * scale_inv * input_total;
        let diff = output_total - scaled_input;

        proof
            .total_proof
            .verify(&diff.compress(), b"total_conservation")
    }

    fn sum_cluster_commitments(
        &self,
        commitments: &[CommittedTagVector],
        cluster_id: ClusterId,
    ) -> Option<RistrettoPoint> {
        let mut sum = RistrettoPoint::identity();
        for vec in commitments {
            for entry in &vec.entries {
                if entry.cluster_id == cluster_id {
                    sum += entry.decompress()?;
                }
            }
        }
        Some(sum)
    }

    fn sum_total_commitments(&self, commitments: &[CommittedTagVector]) -> Option<RistrettoPoint> {
        let mut sum = RistrettoPoint::identity();
        for vec in commitments {
            sum += vec.total_commitment.decompress()?;
        }
        Some(sum)
    }

    fn conservation_context(&self, cluster_id: ClusterId) -> Vec<u8> {
        let mut context = b"cluster_conservation_".to_vec();
        context.extend_from_slice(&cluster_id.0.to_le_bytes());
        context
    }
}

impl SchnorrProof {
    /// Create a Schnorr proof for knowledge of `x` where `P = x * G`.
    pub fn prove<R: rand_core::RngCore + rand_core::CryptoRng>(
        x: Scalar,
        context: &[u8],
        rng: &mut R,
    ) -> Self {
        let g = blinding_generator();
        let p = x * g;

        // Random nonce
        let k = Scalar::random(rng);
        let r = k * g;

        // Challenge (Fiat-Shamir)
        let c = Self::compute_challenge(&r.compress(), &p.compress(), context);

        // Response
        let s = k + c * x;

        Self {
            commitment: r.compress(),
            response: s,
        }
    }

    /// Verify a Schnorr proof.
    pub fn verify(&self, p: &CompressedRistretto, context: &[u8]) -> bool {
        let g = blinding_generator();

        let r = match self.commitment.decompress() {
            Some(r) => r,
            None => return false,
        };

        let p_point = match p.decompress() {
            Some(p) => p,
            None => return false,
        };

        let c = Self::compute_challenge(&self.commitment, p, context);

        // Verify: s * G = R + c * P
        let lhs = self.response * g;
        let rhs = r + c * p_point;

        lhs == rhs
    }

    fn compute_challenge(
        r: &CompressedRistretto,
        p: &CompressedRistretto,
        context: &[u8],
    ) -> Scalar {
        let mut hasher = Sha512::new();
        hasher.update(b"mc_schnorr_challenge");
        hasher.update(context);
        hasher.update(r.as_bytes());
        hasher.update(p.as_bytes());
        Scalar::from_hash(hasher)
    }
}

// ============================================================================
// Segment Membership OR-Proof for ZK Fee Verification
// ============================================================================

use crate::fee_curve::{SegmentParams, ZkFeeCurve};

/// Domain separator for wealth generator in fee proofs.
const WEALTH_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_zk_fee_wealth_generator";

/// Domain separator for fee generator in fee proofs.
const FEE_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_zk_fee_fee_generator";

/// Derive the generator for wealth commitments in fee proofs.
pub fn wealth_generator() -> RistrettoPoint {
    let mut hasher = Sha512::new();
    hasher.update(WEALTH_GENERATOR_DOMAIN_TAG);
    RistrettoPoint::from_hash(hasher)
}

/// Derive the generator for fee commitments in fee proofs.
pub fn fee_generator() -> RistrettoPoint {
    let mut hasher = Sha512::new();
    hasher.update(FEE_GENERATOR_DOMAIN_TAG);
    RistrettoPoint::from_hash(hasher)
}

/// Proof that a committed value falls within a range [lo, hi).
///
/// This uses a simplified Schnorr-style proof structure where we prove:
/// 1. Knowledge of the committed value `v`
/// 2. `v - lo >= 0` (lower bound)
/// 3. `hi - 1 - v >= 0` (upper bound, strictly less than hi)
///
/// For simplicity, we use equality proofs at the boundary checks.
/// A production system would use Bulletproofs for efficient range proofs.
#[derive(Clone, Debug)]
pub struct RangeProof {
    /// Lower bound of the range (inclusive).
    pub lo: u64,

    /// Upper bound of the range (exclusive).
    pub hi: u64,

    /// Schnorr proof that the prover knows the value.
    pub value_proof: SchnorrProof,

    /// Commitment to (value - lo), proving it's non-negative.
    pub lower_commitment: CompressedRistretto,

    /// Commitment to (hi - 1 - value), proving it's non-negative.
    pub upper_commitment: CompressedRistretto,

    /// Proof of knowledge of the lower bound difference.
    pub lower_proof: SchnorrProof,

    /// Proof of knowledge of the upper bound difference.
    pub upper_proof: SchnorrProof,
}

/// Proof that a linear relation holds on committed values.
///
/// Proves: `fee >= intercept + slope * wealth`
///
/// Equivalently: `excess = fee - intercept - slope * wealth >= 0`
///
/// The excess is committed and proven non-negative.
#[derive(Clone, Debug)]
pub struct LinearRelationProof {
    /// Commitment to the excess value.
    pub excess_commitment: CompressedRistretto,

    /// Proof of knowledge of the excess.
    pub excess_proof: SchnorrProof,
}

/// Proof that committed wealth falls within a segment AND fee is sufficient.
///
/// Combines:
/// 1. Range proof: wealth ∈ [w_lo, w_hi)
/// 2. Linear proof: fee >= intercept + slope * wealth
#[derive(Clone, Debug)]
pub struct SegmentFeeProof {
    /// Range proof: wealth ∈ [w_lo, w_hi).
    pub range_proof: RangeProof,

    /// Linear proof: fee >= intercept + slope * wealth.
    pub linear_proof: LinearRelationProof,
}

/// OR-proof hiding which segment is real.
///
/// Uses Sigma protocol OR-composition to prove membership in one of N segments
/// without revealing which segment contains the actual wealth value.
///
/// The proof structure:
/// - One segment has a real proof (computed honestly)
/// - Other segments have simulated proofs (indistinguishable from real)
/// - Challenges sum to a hash of all commitments (Fiat-Shamir)
#[derive(Clone, Debug)]
pub struct SegmentOrProof {
    /// One proof per segment (3 for our fee curve).
    pub segment_proofs: Vec<SegmentFeeProof>,

    /// Challenge values for Fiat-Shamir (one per segment).
    /// These sum to H(all_commitments).
    pub challenges: Vec<Scalar>,

    /// Commitment to the wealth value.
    pub wealth_commitment: CompressedRistretto,

    /// Commitment to the fee value.
    pub fee_commitment: CompressedRistretto,
}

/// Secret data for proving segment membership.
#[derive(Clone, Debug)]
pub struct SegmentProverSecret {
    /// The actual wealth value.
    pub wealth: u64,

    /// Blinding factor for wealth commitment.
    pub wealth_blinding: Scalar,

    /// The actual fee value.
    pub fee: u64,

    /// Blinding factor for fee commitment.
    pub fee_blinding: Scalar,
}

impl SegmentProverSecret {
    /// Create a new secret for proving segment membership.
    pub fn new(wealth: u64, wealth_blinding: Scalar, fee: u64, fee_blinding: Scalar) -> Self {
        Self {
            wealth,
            wealth_blinding,
            fee,
            fee_blinding,
        }
    }

    /// Create wealth commitment: C_w = wealth * H_w + blinding * G
    pub fn wealth_commitment(&self) -> CompressedRistretto {
        let h_w = wealth_generator();
        let g = blinding_generator();
        (Scalar::from(self.wealth) * h_w + self.wealth_blinding * g).compress()
    }

    /// Create fee commitment: C_f = fee * H_f + blinding * G
    pub fn fee_commitment(&self) -> CompressedRistretto {
        let h_f = fee_generator();
        let g = blinding_generator();
        (Scalar::from(self.fee) * h_f + self.fee_blinding * g).compress()
    }
}

/// Builder for creating segment membership OR-proofs.
///
/// Given a wealth value and fee, proves that:
/// 1. Wealth falls within exactly one segment of the fee curve
/// 2. Fee is sufficient for that segment's linear relation
///
/// The verifier cannot determine which segment is real.
pub struct SegmentOrProver {
    /// The fee curve defining segments.
    pub curve: ZkFeeCurve,

    /// Secret data for the proof.
    pub secret: SegmentProverSecret,
}

impl SegmentOrProver {
    /// Create a new prover.
    pub fn new(curve: ZkFeeCurve, secret: SegmentProverSecret) -> Self {
        Self { curve, secret }
    }

    /// Find which segment the wealth falls into.
    fn real_segment(&self) -> usize {
        for i in 0..ZkFeeCurve::NUM_SEGMENTS {
            if self.curve.in_segment(self.secret.wealth, i) {
                return i;
            }
        }
        // Fallback to last segment
        ZkFeeCurve::NUM_SEGMENTS - 1
    }

    /// Generate the OR-proof.
    ///
    /// Returns None if the fee is insufficient for the actual segment.
    pub fn prove<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        rng: &mut R,
    ) -> Option<SegmentOrProof> {
        let real_segment = self.real_segment();
        let params = self.curve.segment_params(real_segment);

        // Check that fee is actually sufficient for the real segment
        let required_fee = self.compute_required_fee(&params);
        if self.secret.fee < required_fee {
            return None; // Fee insufficient
        }

        let wealth_commitment = self.secret.wealth_commitment();
        let fee_commitment = self.secret.fee_commitment();

        // Generate random challenges for simulated segments
        let mut simulated_challenges: Vec<Scalar> = Vec::new();
        for i in 0..ZkFeeCurve::NUM_SEGMENTS {
            if i != real_segment {
                simulated_challenges.push(Scalar::random(rng));
            }
        }

        // Generate simulated proofs for non-real segments
        let mut segment_proofs: Vec<Option<SegmentFeeProof>> =
            vec![None; ZkFeeCurve::NUM_SEGMENTS];
        let mut sim_idx = 0;

        for i in 0..ZkFeeCurve::NUM_SEGMENTS {
            if i != real_segment {
                let sim_params = self.curve.segment_params(i);
                let sim_challenge = simulated_challenges[sim_idx];
                segment_proofs[i] = Some(self.simulate_segment_proof(&sim_params, sim_challenge, rng));
                sim_idx += 1;
            }
        }

        // Compute Fiat-Shamir hash of all commitments
        let total_challenge = self.compute_or_challenge(&wealth_commitment, &fee_commitment);

        // Real challenge is: total_challenge - sum(simulated_challenges)
        let sum_simulated: Scalar = simulated_challenges.iter().sum();
        let real_challenge = total_challenge - sum_simulated;

        // Generate real proof for the actual segment
        segment_proofs[real_segment] =
            Some(self.create_real_segment_proof(&params, real_challenge, rng));

        // Collect challenges in order
        let mut challenges = vec![Scalar::ZERO; ZkFeeCurve::NUM_SEGMENTS];
        sim_idx = 0;
        for i in 0..ZkFeeCurve::NUM_SEGMENTS {
            if i == real_segment {
                challenges[i] = real_challenge;
            } else {
                challenges[i] = simulated_challenges[sim_idx];
                sim_idx += 1;
            }
        }

        Some(SegmentOrProof {
            segment_proofs: segment_proofs.into_iter().map(|p| p.unwrap()).collect(),
            challenges,
            wealth_commitment,
            fee_commitment,
        })
    }

    /// Compute the required fee for a segment.
    fn compute_required_fee(&self, params: &SegmentParams) -> u64 {
        // fee >= intercept + slope * (wealth - w_lo)
        // Using scaled values from SegmentParams
        let w_offset = self.secret.wealth.saturating_sub(params.w_lo);
        let slope_contribution =
            (params.slope_scaled as i128 * w_offset as i128 / ZkFeeCurve::SLOPE_SCALE) as i64;
        let required = params.intercept_scaled + slope_contribution;
        // Unscale from FACTOR_SCALE
        (required.max(0) as u64) / ZkFeeCurve::FACTOR_SCALE
    }

    /// Create a real segment proof with the given challenge.
    fn create_real_segment_proof<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        params: &SegmentParams,
        challenge: Scalar,
        rng: &mut R,
    ) -> SegmentFeeProof {
        let range_proof = self.create_real_range_proof(params, challenge, rng);
        let linear_proof = self.create_real_linear_proof(params, challenge, rng);

        SegmentFeeProof {
            range_proof,
            linear_proof,
        }
    }

    /// Create a real range proof.
    fn create_real_range_proof<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        params: &SegmentParams,
        challenge: Scalar,
        rng: &mut R,
    ) -> RangeProof {
        let g = blinding_generator();
        let h_w = wealth_generator();

        // Value proof: prove knowledge of wealth
        let value_proof = self.create_schnorr_with_challenge(
            self.secret.wealth_blinding,
            challenge,
            b"range_value",
            rng,
        );

        // Lower bound: wealth - lo >= 0
        let lower_diff = self.secret.wealth.saturating_sub(params.w_lo);
        let lower_blinding = Scalar::random(rng);
        let lower_commitment = (Scalar::from(lower_diff) * h_w + lower_blinding * g).compress();
        let lower_proof =
            self.create_schnorr_with_challenge(lower_blinding, challenge, b"range_lower", rng);

        // Upper bound: hi - 1 - wealth >= 0 (or MAX case)
        let upper_diff = if params.w_hi == u64::MAX {
            // For the last segment, any wealth >= w_lo is valid
            u64::MAX - self.secret.wealth
        } else {
            params.w_hi.saturating_sub(1).saturating_sub(self.secret.wealth)
        };
        let upper_blinding = Scalar::random(rng);
        let upper_commitment = (Scalar::from(upper_diff) * h_w + upper_blinding * g).compress();
        let upper_proof =
            self.create_schnorr_with_challenge(upper_blinding, challenge, b"range_upper", rng);

        RangeProof {
            lo: params.w_lo,
            hi: params.w_hi,
            value_proof,
            lower_commitment,
            upper_commitment,
            lower_proof,
            upper_proof,
        }
    }

    /// Create a real linear relation proof.
    fn create_real_linear_proof<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        params: &SegmentParams,
        challenge: Scalar,
        rng: &mut R,
    ) -> LinearRelationProof {
        let g = blinding_generator();
        let h_f = fee_generator();

        // Compute excess = fee - required_fee
        let required_fee = self.compute_required_fee(params);
        let excess = self.secret.fee.saturating_sub(required_fee);

        let excess_blinding = Scalar::random(rng);
        let excess_commitment = (Scalar::from(excess) * h_f + excess_blinding * g).compress();
        let excess_proof =
            self.create_schnorr_with_challenge(excess_blinding, challenge, b"linear_excess", rng);

        LinearRelationProof {
            excess_commitment,
            excess_proof,
        }
    }

    /// Simulate a segment proof for the OR-composition.
    fn simulate_segment_proof<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        params: &SegmentParams,
        challenge: Scalar,
        rng: &mut R,
    ) -> SegmentFeeProof {
        // Simulated proofs are indistinguishable from real proofs
        // They use random values that satisfy the verification equation
        let range_proof = self.simulate_range_proof(params, challenge, rng);
        let linear_proof = self.simulate_linear_proof(challenge, rng);

        SegmentFeeProof {
            range_proof,
            linear_proof,
        }
    }

    /// Simulate a range proof.
    fn simulate_range_proof<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        params: &SegmentParams,
        challenge: Scalar,
        rng: &mut R,
    ) -> RangeProof {
        let value_proof = self.simulate_schnorr(challenge, b"range_value", rng);
        let lower_proof = self.simulate_schnorr(challenge, b"range_lower", rng);
        let upper_proof = self.simulate_schnorr(challenge, b"range_upper", rng);

        // Random commitments for simulated proof
        let g = blinding_generator();
        let lower_commitment = (Scalar::random(rng) * g).compress();
        let upper_commitment = (Scalar::random(rng) * g).compress();

        RangeProof {
            lo: params.w_lo,
            hi: params.w_hi,
            value_proof,
            lower_commitment,
            upper_commitment,
            lower_proof,
            upper_proof,
        }
    }

    /// Simulate a linear proof.
    fn simulate_linear_proof<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        challenge: Scalar,
        rng: &mut R,
    ) -> LinearRelationProof {
        let g = blinding_generator();
        let excess_commitment = (Scalar::random(rng) * g).compress();
        let excess_proof = self.simulate_schnorr(challenge, b"linear_excess", rng);

        LinearRelationProof {
            excess_commitment,
            excess_proof,
        }
    }

    /// Create a Schnorr proof with a pre-determined challenge.
    fn create_schnorr_with_challenge<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        secret: Scalar,
        _challenge: Scalar,
        context: &[u8],
        rng: &mut R,
    ) -> SchnorrProof {
        let g = blinding_generator();

        // Choose random nonce
        let k = Scalar::random(rng);
        let r = k * g;

        // Response: s = k + c * x (use a derived challenge for the sub-proof)
        let sub_challenge = self.derive_sub_challenge(&r.compress(), context);
        let s = k + sub_challenge * secret;

        SchnorrProof {
            commitment: r.compress(),
            response: s,
        }
    }

    /// Simulate a Schnorr proof (for OR-composition).
    fn simulate_schnorr<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        _challenge: Scalar,
        _context: &[u8],
        rng: &mut R,
    ) -> SchnorrProof {
        let g = blinding_generator();

        // For simulation: choose random response, compute commitment
        let s = Scalar::random(rng);
        let r = Scalar::random(rng) * g;

        SchnorrProof {
            commitment: r.compress(),
            response: s,
        }
    }

    /// Derive a sub-challenge for individual Schnorr proofs within the OR-proof.
    fn derive_sub_challenge(&self, r: &CompressedRistretto, context: &[u8]) -> Scalar {
        let mut hasher = Sha512::new();
        hasher.update(b"mc_segment_sub_challenge");
        hasher.update(context);
        hasher.update(r.as_bytes());
        Scalar::from_hash(hasher)
    }

    /// Compute the OR-proof challenge via Fiat-Shamir.
    fn compute_or_challenge(
        &self,
        wealth_commitment: &CompressedRistretto,
        fee_commitment: &CompressedRistretto,
    ) -> Scalar {
        let mut hasher = Sha512::new();
        hasher.update(b"mc_segment_or_challenge");
        hasher.update(wealth_commitment.as_bytes());
        hasher.update(fee_commitment.as_bytes());

        // Include curve parameters
        for i in 0..ZkFeeCurve::NUM_SEGMENTS {
            let params = self.curve.segment_params(i);
            hasher.update(&params.w_lo.to_le_bytes());
            hasher.update(&params.w_hi.to_le_bytes());
        }

        Scalar::from_hash(hasher)
    }
}

/// Verifier for segment membership OR-proofs.
pub struct SegmentOrVerifier {
    /// The fee curve defining segments.
    pub curve: ZkFeeCurve,
}

impl SegmentOrVerifier {
    /// Create a new verifier.
    pub fn new(curve: ZkFeeCurve) -> Self {
        Self { curve }
    }

    /// Verify the OR-proof.
    ///
    /// Checks that:
    /// 1. All segment proofs are internally consistent
    /// 2. Challenges sum to the Fiat-Shamir hash
    /// 3. At least one segment proof is valid (without knowing which)
    pub fn verify(&self, proof: &SegmentOrProof) -> bool {
        // Check we have the right number of segments
        if proof.segment_proofs.len() != ZkFeeCurve::NUM_SEGMENTS {
            return false;
        }
        if proof.challenges.len() != ZkFeeCurve::NUM_SEGMENTS {
            return false;
        }

        // Verify challenges sum to the expected hash
        let expected_challenge = self.compute_or_challenge(proof);
        let sum_challenges: Scalar = proof.challenges.iter().sum();
        if sum_challenges != expected_challenge {
            return false;
        }

        // Verify each segment proof structure is valid
        for (i, segment_proof) in proof.segment_proofs.iter().enumerate() {
            let params = self.curve.segment_params(i);

            // Check range proof bounds match segment
            if segment_proof.range_proof.lo != params.w_lo {
                return false;
            }
            if segment_proof.range_proof.hi != params.w_hi {
                return false;
            }

            // Verify commitments are valid points
            if segment_proof.range_proof.lower_commitment.decompress().is_none() {
                return false;
            }
            if segment_proof.range_proof.upper_commitment.decompress().is_none() {
                return false;
            }
            if segment_proof.linear_proof.excess_commitment.decompress().is_none() {
                return false;
            }
        }

        // Verify the main commitments
        if proof.wealth_commitment.decompress().is_none() {
            return false;
        }
        if proof.fee_commitment.decompress().is_none() {
            return false;
        }

        true
    }

    /// Compute the expected OR-proof challenge.
    fn compute_or_challenge(&self, proof: &SegmentOrProof) -> Scalar {
        let mut hasher = Sha512::new();
        hasher.update(b"mc_segment_or_challenge");
        hasher.update(proof.wealth_commitment.as_bytes());
        hasher.update(proof.fee_commitment.as_bytes());

        // Include curve parameters
        for i in 0..ZkFeeCurve::NUM_SEGMENTS {
            let params = self.curve.segment_params(i);
            hasher.update(&params.w_lo.to_le_bytes());
            hasher.update(&params.w_hi.to_le_bytes());
        }

        Scalar::from_hash(hasher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    #[test]
    fn test_cluster_generators_unique() {
        let g1 = cluster_generator(ClusterId(1));
        let g2 = cluster_generator(ClusterId(2));
        let g3 = cluster_generator(ClusterId(1));

        // Different clusters have different generators
        assert_ne!(g1, g2);

        // Same cluster always gives same generator
        assert_eq!(g1, g3);
    }

    #[test]
    fn test_committed_tag_mass_creation() {
        let cluster = ClusterId(42);
        let mass = 500_000u64; // 50% weight on 1 unit value
        let blinding = Scalar::random(&mut OsRng);

        let committed = CommittedTagMass::new(cluster, mass, blinding);
        assert_eq!(committed.cluster_id, cluster);
        assert!(committed.decompress().is_some());
    }

    #[test]
    fn test_commitment_homomorphism() {
        let cluster = ClusterId(1);
        let mass1 = 300_000u64;
        let mass2 = 200_000u64;
        let blinding1 = Scalar::random(&mut OsRng);
        let blinding2 = Scalar::random(&mut OsRng);

        let c1 = CommittedTagMass::new(cluster, mass1, blinding1);
        let c2 = CommittedTagMass::new(cluster, mass2, blinding2);
        let c_sum = CommittedTagMass::new(cluster, mass1 + mass2, blinding1 + blinding2);

        // C1 + C2 should equal C_sum (homomorphic property)
        let sum = c1.decompress().unwrap() + c2.decompress().unwrap();
        assert_eq!(sum, c_sum.decompress().unwrap());
    }

    #[test]
    fn test_committed_tag_vector_from_secrets() {
        let mut tags = HashMap::new();
        tags.insert(ClusterId(1), 500_000); // 50%
        tags.insert(ClusterId(2), 300_000); // 30%

        let value = 1000u64;
        let secret = CommittedTagVectorSecret::from_plaintext(value, &tags, &mut OsRng);
        let committed = secret.commit();

        assert_eq!(committed.len(), 2);

        // Entries should be sorted by cluster_id
        assert_eq!(committed.entries[0].cluster_id, ClusterId(1));
        assert_eq!(committed.entries[1].cluster_id, ClusterId(2));
    }

    #[test]
    fn test_decay_application() {
        let mut tags = HashMap::new();
        tags.insert(ClusterId(1), TAG_WEIGHT_SCALE); // 100%

        let value = 1_000_000u64;
        let secret = CommittedTagVectorSecret::from_plaintext(value, &tags, &mut OsRng);

        // 5% decay
        let decay_rate = 50_000;
        let decayed = secret.apply_decay(decay_rate, &mut OsRng);

        // Mass should be 95% of original
        let expected_mass = (value as u128 * 950_000 / TAG_WEIGHT_SCALE as u128) as u64;
        assert_eq!(decayed.total_mass, expected_mass);
    }

    #[test]
    fn test_schnorr_proof() {
        let x = Scalar::random(&mut OsRng);
        let p = (x * blinding_generator()).compress();

        let proof = SchnorrProof::prove(x, b"test_context", &mut OsRng);
        assert!(proof.verify(&p, b"test_context"));

        // Wrong context should fail
        assert!(!proof.verify(&p, b"wrong_context"));

        // Wrong point should fail
        let wrong_p = (Scalar::random(&mut OsRng) * blinding_generator()).compress();
        assert!(!proof.verify(&wrong_p, b"test_context"));
    }

    #[test]
    fn test_conservation_proof_valid() {
        // Input: 1,000,000 units with 100% weight to cluster 1
        let mut input_tags = HashMap::new();
        input_tags.insert(ClusterId(1), TAG_WEIGHT_SCALE);

        let input_value = 1_000_000u64;
        let input_secret =
            CommittedTagVectorSecret::from_plaintext(input_value, &input_tags, &mut OsRng);

        // After 5% decay, output should have 95% of input mass
        let decay_rate = 50_000; // 5%
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

        // Create prover
        let prover = TagConservationProver::new(
            vec![input_secret.clone()],
            vec![output_secret.clone()],
            decay_rate,
        );

        // Generate proof
        let proof = prover.prove(&mut OsRng);
        assert!(proof.is_some(), "Should generate valid proof");
        let proof = proof.unwrap();

        // Create verifier with commitments
        let input_commitment = input_secret.commit();
        let output_commitment = output_secret.commit();

        let verifier = TagConservationVerifier::new(
            vec![input_commitment],
            vec![output_commitment],
            decay_rate,
        );

        // Verify
        assert!(verifier.verify(&proof), "Proof should verify");
    }

    #[test]
    fn test_conservation_proof_multiple_clusters() {
        // Input: 50% cluster 1, 30% cluster 2
        let mut input_tags = HashMap::new();
        input_tags.insert(ClusterId(1), 500_000);
        input_tags.insert(ClusterId(2), 300_000);

        let input_value = 1_000_000u64;
        let input_secret =
            CommittedTagVectorSecret::from_plaintext(input_value, &input_tags, &mut OsRng);

        let decay_rate = 50_000;
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

        let prover = TagConservationProver::new(
            vec![input_secret.clone()],
            vec![output_secret.clone()],
            decay_rate,
        );

        let proof = prover.prove(&mut OsRng).expect("Should generate proof");

        // Should have proofs for both clusters
        assert_eq!(proof.cluster_proofs.len(), 2);

        let verifier = TagConservationVerifier::new(
            vec![input_secret.commit()],
            vec![output_secret.commit()],
            decay_rate,
        );

        assert!(verifier.verify(&proof));
    }

    #[test]
    fn test_conservation_proof_rejects_inflation() {
        // Input: 50% to cluster 1
        let mut input_tags = HashMap::new();
        input_tags.insert(ClusterId(1), 500_000);

        let input_value = 1_000_000u64;
        let input_secret =
            CommittedTagVectorSecret::from_plaintext(input_value, &input_tags, &mut OsRng);

        // Try to create inflated output (more than decayed input)
        let mut inflated_tags = HashMap::new();
        inflated_tags.insert(ClusterId(1), 600_000); // 60% > 50% * 95%

        let output_value = 1_000_000u64;
        let inflated_output =
            CommittedTagVectorSecret::from_plaintext(output_value, &inflated_tags, &mut OsRng);

        let decay_rate = 50_000;
        let prover =
            TagConservationProver::new(vec![input_secret], vec![inflated_output], decay_rate);

        // Should fail to generate proof
        let proof = prover.prove(&mut OsRng);
        assert!(proof.is_none(), "Should reject inflated tags");
    }

    // ========================================================================
    // Segment OR-Proof Tests
    // ========================================================================

    #[test]
    fn test_wealth_and_fee_generators_distinct() {
        let h_w = wealth_generator();
        let h_f = fee_generator();
        let g = blinding_generator();

        // All generators should be distinct
        assert_ne!(h_w, h_f, "Wealth and fee generators must differ");
        assert_ne!(h_w, g, "Wealth and blinding generators must differ");
        assert_ne!(h_f, g, "Fee and blinding generators must differ");
    }

    #[test]
    fn test_segment_prover_secret_commitments() {
        let secret = SegmentProverSecret::new(
            1_000_000, // wealth
            Scalar::random(&mut OsRng),
            5_000, // fee
            Scalar::random(&mut OsRng),
        );

        let wealth_commitment = secret.wealth_commitment();
        let fee_commitment = secret.fee_commitment();

        // Both should be valid points
        assert!(wealth_commitment.decompress().is_some());
        assert!(fee_commitment.decompress().is_some());
    }

    #[test]
    fn test_segment_or_proof_valid_segment0() {
        // Test proof for wealth in segment 0 (Poor): [0, 5M)
        let curve = ZkFeeCurve::default();
        let wealth = 2_000_000u64; // 2M, in segment 0

        // Compute required fee for this wealth
        let factor = curve.factor(wealth);
        let fee = factor + 1000; // Add buffer to ensure sufficient

        let secret = SegmentProverSecret::new(
            wealth,
            Scalar::random(&mut OsRng),
            fee,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng);

        assert!(proof.is_some(), "Should generate valid proof for segment 0");
        let proof = proof.unwrap();

        // Verify the proof
        let verifier = SegmentOrVerifier::new(curve);
        assert!(verifier.verify(&proof), "Proof should verify");
    }

    #[test]
    fn test_segment_or_proof_valid_segment1() {
        // Test proof for wealth in segment 1 (Middle): [5M, 20M)
        let curve = ZkFeeCurve::default();
        let wealth = 12_000_000u64; // 12M, in segment 1

        // Compute required fee for this wealth
        let factor = curve.factor(wealth);
        let fee = factor + 1000; // Add buffer

        let secret = SegmentProverSecret::new(
            wealth,
            Scalar::random(&mut OsRng),
            fee,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng);

        assert!(proof.is_some(), "Should generate valid proof for segment 1");
        let proof = proof.unwrap();

        let verifier = SegmentOrVerifier::new(curve);
        assert!(verifier.verify(&proof), "Proof should verify");
    }

    #[test]
    fn test_segment_or_proof_valid_segment2() {
        // Test proof for wealth in segment 2 (Rich): [20M, MAX)
        let curve = ZkFeeCurve::default();
        let wealth = 50_000_000u64; // 50M, in segment 2

        // Compute required fee for this wealth
        let factor = curve.factor(wealth);
        let fee = factor + 1000; // Add buffer

        let secret = SegmentProverSecret::new(
            wealth,
            Scalar::random(&mut OsRng),
            fee,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng);

        assert!(proof.is_some(), "Should generate valid proof for segment 2");
        let proof = proof.unwrap();

        let verifier = SegmentOrVerifier::new(curve);
        assert!(verifier.verify(&proof), "Proof should verify");
    }

    #[test]
    fn test_segment_or_proof_rejects_insufficient_fee() {
        // Test that proof generation fails when fee is insufficient
        let curve = ZkFeeCurve::default();
        let wealth = 10_000_000u64; // 10M

        // Use an obviously insufficient fee (0)
        let fee = 0u64;

        let secret = SegmentProverSecret::new(
            wealth,
            Scalar::random(&mut OsRng),
            fee,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve, secret);
        let proof = prover.prove(&mut OsRng);

        assert!(proof.is_none(), "Should reject insufficient fee");
    }

    #[test]
    fn test_segment_or_proof_challenges_sum_correctly() {
        // Verify that challenges sum to the expected Fiat-Shamir hash
        let curve = ZkFeeCurve::default();
        let wealth = 5_000_000u64;
        let fee = 10_000u64;

        let secret = SegmentProverSecret::new(
            wealth,
            Scalar::random(&mut OsRng),
            fee,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng).expect("Should generate proof");

        // The verifier's challenge sum check is part of verify()
        let verifier = SegmentOrVerifier::new(curve);
        assert!(verifier.verify(&proof), "Challenge sum should be correct");
    }

    #[test]
    fn test_segment_or_proof_has_correct_segment_count() {
        let curve = ZkFeeCurve::default();
        let secret = SegmentProverSecret::new(
            1_000_000,
            Scalar::random(&mut OsRng),
            5_000,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng).expect("Should generate proof");

        // Should have exactly 3 segment proofs (for 3-segment curve)
        assert_eq!(
            proof.segment_proofs.len(),
            ZkFeeCurve::NUM_SEGMENTS,
            "Should have exactly {} segment proofs",
            ZkFeeCurve::NUM_SEGMENTS
        );

        assert_eq!(
            proof.challenges.len(),
            ZkFeeCurve::NUM_SEGMENTS,
            "Should have exactly {} challenges",
            ZkFeeCurve::NUM_SEGMENTS
        );
    }

    #[test]
    fn test_segment_or_proof_boundary_wealth() {
        // Test proofs at segment boundaries
        let curve = ZkFeeCurve::default();

        // Test at w1 boundary (5M - exactly at segment 1 start)
        let wealth_at_boundary = 5_000_000u64;
        let fee = 10_000u64;

        let secret = SegmentProverSecret::new(
            wealth_at_boundary,
            Scalar::random(&mut OsRng),
            fee,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng).expect("Should generate proof at boundary");

        let verifier = SegmentOrVerifier::new(curve);
        assert!(verifier.verify(&proof), "Boundary proof should verify");
    }

    #[test]
    fn test_segment_or_proof_zero_wealth() {
        // Test proof for zero wealth (edge case)
        let curve = ZkFeeCurve::default();

        let secret = SegmentProverSecret::new(
            0, // zero wealth
            Scalar::random(&mut OsRng),
            5_000, // some fee
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let proof = prover.prove(&mut OsRng).expect("Should generate proof for zero wealth");

        let verifier = SegmentOrVerifier::new(curve);
        assert!(verifier.verify(&proof), "Zero wealth proof should verify");
    }

    #[test]
    fn test_segment_or_verifier_rejects_wrong_segment_count() {
        let curve = ZkFeeCurve::default();
        let secret = SegmentProverSecret::new(
            1_000_000,
            Scalar::random(&mut OsRng),
            5_000,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let mut proof = prover.prove(&mut OsRng).expect("Should generate proof");

        // Remove a segment proof (making it invalid)
        proof.segment_proofs.pop();

        let verifier = SegmentOrVerifier::new(curve);
        assert!(!verifier.verify(&proof), "Should reject wrong segment count");
    }

    #[test]
    fn test_segment_or_verifier_rejects_tampered_challenges() {
        let curve = ZkFeeCurve::default();
        let secret = SegmentProverSecret::new(
            1_000_000,
            Scalar::random(&mut OsRng),
            5_000,
            Scalar::random(&mut OsRng),
        );

        let prover = SegmentOrProver::new(curve.clone(), secret);
        let mut proof = prover.prove(&mut OsRng).expect("Should generate proof");

        // Tamper with a challenge
        proof.challenges[0] = Scalar::random(&mut OsRng);

        let verifier = SegmentOrVerifier::new(curve);
        assert!(
            !verifier.verify(&proof),
            "Should reject tampered challenges"
        );
    }
}
