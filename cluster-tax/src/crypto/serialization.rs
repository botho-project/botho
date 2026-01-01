//! Binary serialization for committed tag types.
//!
//! This module provides simple binary serialization for the cryptographic
//! types used in committed cluster tags, enabling them to be embedded in
//! transaction signatures.
//!
//! Format: Fixed-size binary encoding without length prefixes where possible.

use crate::ClusterId;
use curve25519_dalek::{ristretto::CompressedRistretto, scalar::Scalar};

use super::{
    committed_tags::{
        ClusterConservationProof, CommittedTagMass, CommittedTagVector, SchnorrProof,
        TagConservationProof,
    },
    extended_signature::{ExtendedTxSignature, PseudoTagOutput, TagInheritanceProof},
};

/// Error type for deserialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeserializeError {
    /// Not enough bytes remaining.
    UnexpectedEof,
    /// Invalid scalar bytes.
    InvalidScalar,
    /// Invalid point bytes.
    InvalidPoint,
    /// Invalid length field.
    InvalidLength,
}

/// Writer for binary encoding.
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn write_point(&mut self, p: &CompressedRistretto) {
        self.write_bytes(p.as_bytes());
    }

    fn write_scalar(&mut self, s: &Scalar) {
        self.write_bytes(s.as_bytes());
    }

    fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

/// Reader for binary decoding.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DeserializeError> {
        if self.pos + n > self.data.len() {
            return Err(DeserializeError::UnexpectedEof);
        }
        let bytes = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }

    fn read_u64(&mut self) -> Result<u64, DeserializeError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> Result<u32, DeserializeError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_point(&mut self) -> Result<CompressedRistretto, DeserializeError> {
        let bytes = self.read_bytes(32)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(CompressedRistretto(arr))
    }

    fn read_scalar(&mut self) -> Result<Scalar, DeserializeError> {
        let bytes = self.read_bytes(32)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Scalar::from_canonical_bytes(arr)
            .into_option()
            .ok_or(DeserializeError::InvalidScalar)
    }
}

// ============================================================================
// Serialization implementations
// ============================================================================

impl SchnorrProof {
    /// Serialize to bytes (64 bytes: commitment + response).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_point(&self.commitment);
        w.write_scalar(&self.response);
        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);
        let commitment = r.read_point()?;
        let response = r.read_scalar()?;
        Ok(Self {
            commitment,
            response,
        })
    }
}

impl CommittedTagMass {
    /// Serialize to bytes (40 bytes: cluster_id + commitment).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u64(self.cluster_id.0);
        w.write_point(&self.commitment);
        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);
        let cluster_id = ClusterId(r.read_u64()?);
        let commitment = r.read_point()?;
        Ok(Self {
            cluster_id,
            commitment,
        })
    }
}

impl CommittedTagVector {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();

        // Entry count
        w.write_u32(self.entries.len() as u32);

        // Entries
        for entry in &self.entries {
            w.write_bytes(&entry.to_bytes());
        }

        // Total commitment
        w.write_point(&self.total_commitment);

        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);

        let entry_count = r.read_u32()? as usize;
        if entry_count > 100 {
            return Err(DeserializeError::InvalidLength);
        }

        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let entry_bytes = r.read_bytes(40)?;
            entries.push(CommittedTagMass::from_bytes(entry_bytes)?);
        }

        let total_commitment = r.read_point()?;

        Ok(Self {
            entries,
            total_commitment,
        })
    }
}

impl ClusterConservationProof {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u64(self.cluster_id.0);
        w.write_bytes(&self.proof.to_bytes());
        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);
        let cluster_id = ClusterId(r.read_u64()?);
        let proof_bytes = r.read_bytes(64)?;
        let proof = SchnorrProof::from_bytes(proof_bytes)?;
        Ok(Self { cluster_id, proof })
    }
}

impl TagConservationProof {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();

        // Cluster proof count
        w.write_u32(self.cluster_proofs.len() as u32);

        // Cluster proofs (each is 72 bytes: 8 + 64)
        for cp in &self.cluster_proofs {
            w.write_bytes(&cp.to_bytes());
        }

        // Total proof
        w.write_bytes(&self.total_proof.to_bytes());

        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);

        let cp_count = r.read_u32()? as usize;
        if cp_count > 100 {
            return Err(DeserializeError::InvalidLength);
        }

        let mut cluster_proofs = Vec::with_capacity(cp_count);
        for _ in 0..cp_count {
            let cp_bytes = r.read_bytes(72)?;
            cluster_proofs.push(ClusterConservationProof::from_bytes(cp_bytes)?);
        }

        let total_proof_bytes = r.read_bytes(64)?;
        let total_proof = SchnorrProof::from_bytes(total_proof_bytes)?;

        Ok(Self {
            cluster_proofs,
            total_proof,
        })
    }
}

impl TagInheritanceProof {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u64(self.cluster_id.0);
        w.write_bytes(&self.proof.to_bytes());
        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);
        let cluster_id = ClusterId(r.read_u64()?);
        let proof_bytes = r.read_bytes(64)?;
        let proof = SchnorrProof::from_bytes(proof_bytes)?;
        Ok(Self { cluster_id, proof })
    }
}

impl PseudoTagOutput {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();

        // Tags (variable length)
        let tags_bytes = self.tags.to_bytes();
        w.write_u32(tags_bytes.len() as u32);
        w.write_bytes(&tags_bytes);

        // Inheritance proofs count
        w.write_u32(self.inheritance_proofs.len() as u32);

        // Inheritance proofs (each is 72 bytes)
        for ip in &self.inheritance_proofs {
            w.write_bytes(&ip.to_bytes());
        }

        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);

        // Tags
        let tags_len = r.read_u32()? as usize;
        if tags_len > 10000 {
            return Err(DeserializeError::InvalidLength);
        }
        let tags_bytes = r.read_bytes(tags_len)?;
        let tags = CommittedTagVector::from_bytes(tags_bytes)?;

        // Inheritance proofs
        let ip_count = r.read_u32()? as usize;
        if ip_count > 100 {
            return Err(DeserializeError::InvalidLength);
        }

        let mut inheritance_proofs = Vec::with_capacity(ip_count);
        for _ in 0..ip_count {
            let ip_bytes = r.read_bytes(72)?;
            inheritance_proofs.push(TagInheritanceProof::from_bytes(ip_bytes)?);
        }

        Ok(Self {
            tags,
            inheritance_proofs,
        })
    }
}

impl ExtendedTxSignature {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();

        // Pseudo-tag-outputs count
        w.write_u32(self.pseudo_tag_outputs.len() as u32);

        // Pseudo-tag-outputs (variable length each)
        for pto in &self.pseudo_tag_outputs {
            let pto_bytes = pto.to_bytes();
            w.write_u32(pto_bytes.len() as u32);
            w.write_bytes(&pto_bytes);
        }

        // Conservation proof
        w.write_bytes(&self.conservation_proof.to_bytes());

        w.into_vec()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let mut r = Reader::new(bytes);

        // Pseudo-tag-outputs
        let pto_count = r.read_u32()? as usize;
        if pto_count > 100 {
            return Err(DeserializeError::InvalidLength);
        }

        let mut pseudo_tag_outputs = Vec::with_capacity(pto_count);
        for _ in 0..pto_count {
            let pto_len = r.read_u32()? as usize;
            if pto_len > 100000 {
                return Err(DeserializeError::InvalidLength);
            }
            let pto_bytes = r.read_bytes(pto_len)?;
            pseudo_tag_outputs.push(PseudoTagOutput::from_bytes(pto_bytes)?);
        }

        // Conservation proof - remaining bytes
        let remaining = &r.data[r.pos..];
        let conservation_proof = TagConservationProof::from_bytes(remaining)?;

        Ok(Self {
            pseudo_tag_outputs,
            conservation_proof,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{CommittedTagVectorSecret, ExtendedSignatureBuilder},
        TAG_WEIGHT_SCALE,
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
    fn test_schnorr_proof_roundtrip() {
        let x = Scalar::from(12345u64);
        let proof = SchnorrProof::prove(x, b"test", &mut OsRng);

        let bytes = proof.to_bytes();
        assert_eq!(bytes.len(), 64);

        let restored = SchnorrProof::from_bytes(&bytes).expect("Should deserialize");
        assert_eq!(proof.commitment, restored.commitment);
        assert_eq!(proof.response, restored.response);
    }

    #[test]
    fn test_committed_tag_vector_roundtrip() {
        let secret = create_test_secret(1_000_000, &[(1, 500_000), (2, 300_000)]);
        let original = secret.commit();

        let bytes = original.to_bytes();
        let restored = CommittedTagVector::from_bytes(&bytes).expect("Should deserialize");

        assert_eq!(original.entries.len(), restored.entries.len());
        assert_eq!(original.total_commitment, restored.total_commitment);

        for (orig, rest) in original.entries.iter().zip(restored.entries.iter()) {
            assert_eq!(orig.cluster_id, rest.cluster_id);
            assert_eq!(orig.commitment, rest.commitment);
        }
    }

    #[test]
    fn test_extended_signature_roundtrip() {
        let decay_rate = 50_000;

        // Create input
        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);
        let input_commitment = input_secret.commit();
        let ring_tags = vec![input_commitment.clone()];

        // Create output
        let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

        // Build signature
        let mut builder = ExtendedSignatureBuilder::new(decay_rate);
        builder.add_input(ring_tags, 0, input_secret);
        builder.add_output(output_secret);

        let original = builder.build(&mut OsRng).expect("Should build signature");

        // Serialize and deserialize
        let bytes = original.to_bytes();
        let restored = ExtendedTxSignature::from_bytes(&bytes).expect("Should deserialize");

        // Verify structure matches
        assert_eq!(
            original.pseudo_tag_outputs.len(),
            restored.pseudo_tag_outputs.len()
        );
        assert_eq!(
            original.conservation_proof.cluster_proofs.len(),
            restored.conservation_proof.cluster_proofs.len()
        );
    }
}
