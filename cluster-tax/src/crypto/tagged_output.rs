//! Tagged transaction output structure.
//!
//! Extends TxOut with cluster tag information for progressive fee computation.

use crate::{ClusterId, TagWeight, TAG_WEIGHT_SCALE};
use std::collections::HashMap;

/// Maximum number of tags stored per output.
/// Keeps output size bounded while preserving significant ancestry.
pub const MAX_STORED_TAGS: usize = 16;

/// Minimum tag weight to store (weights below this become background).
/// 0.1% = 1000 in our scale (TAG_WEIGHT_SCALE = 1_000_000)
pub const MIN_STORED_WEIGHT: TagWeight = 1000;

/// Compact on-chain representation of a tag vector.
///
/// This is the structure that would be serialized into TxOut.
/// Uses fixed-size arrays for predictable serialization.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactTagVector {
    /// Number of valid entries in the arrays below.
    pub count: u8,

    /// Cluster IDs (up to MAX_STORED_TAGS).
    /// Only first `count` entries are valid.
    pub cluster_ids: [u64; MAX_STORED_TAGS],

    /// Corresponding weights (TAG_WEIGHT_SCALE = 100%).
    /// Only first `count` entries are valid.
    pub weights: [TagWeight; MAX_STORED_TAGS],
}

impl CompactTagVector {
    /// Create an empty tag vector (fully background).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create a tag vector fully attributed to one cluster.
    pub fn single(cluster_id: ClusterId) -> Self {
        let mut result = Self::default();
        result.count = 1;
        result.cluster_ids[0] = cluster_id.0;
        result.weights[0] = TAG_WEIGHT_SCALE;
        result
    }

    /// Create from a HashMap representation.
    pub fn from_map(tags: &HashMap<ClusterId, TagWeight>) -> Self {
        let mut result = Self::default();

        // Filter and sort by weight descending
        let mut entries: Vec<_> = tags
            .iter()
            .filter(|(_, &w)| w >= MIN_STORED_WEIGHT)
            .collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));

        // Take top MAX_STORED_TAGS
        for (i, (&cluster, &weight)) in entries.iter().take(MAX_STORED_TAGS).enumerate() {
            result.cluster_ids[i] = cluster.0;
            result.weights[i] = weight;
            result.count += 1;
        }

        result
    }

    /// Convert to HashMap for computation.
    pub fn to_map(&self) -> HashMap<ClusterId, TagWeight> {
        let mut map = HashMap::new();
        for i in 0..self.count as usize {
            map.insert(ClusterId(self.cluster_ids[i]), self.weights[i]);
        }
        map
    }

    /// Get total attributed weight (remainder is background).
    pub fn total_weight(&self) -> TagWeight {
        self.weights[..self.count as usize]
            .iter()
            .sum::<TagWeight>()
            .min(TAG_WEIGHT_SCALE)
    }

    /// Get background weight.
    pub fn background_weight(&self) -> TagWeight {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_weight())
    }

    /// Get weight for a specific cluster.
    pub fn get(&self, cluster: ClusterId) -> TagWeight {
        for i in 0..self.count as usize {
            if self.cluster_ids[i] == cluster.0 {
                return self.weights[i];
            }
        }
        0
    }

    /// Serialized size in bytes.
    pub const fn serialized_size() -> usize {
        1 + // count
        MAX_STORED_TAGS * 8 + // cluster_ids (u64)
        MAX_STORED_TAGS * 4   // weights (u32)
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::serialized_size());
        bytes.push(self.count);
        for i in 0..MAX_STORED_TAGS {
            bytes.extend_from_slice(&self.cluster_ids[i].to_le_bytes());
        }
        for i in 0..MAX_STORED_TAGS {
            bytes.extend_from_slice(&self.weights[i].to_le_bytes());
        }
        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::serialized_size() {
            return None;
        }

        let count = bytes[0];
        if count as usize > MAX_STORED_TAGS {
            return None;
        }

        let mut result = Self::default();
        result.count = count;

        let mut offset = 1;
        for i in 0..MAX_STORED_TAGS {
            result.cluster_ids[i] =
                u64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
            offset += 8;
        }
        for i in 0..MAX_STORED_TAGS {
            result.weights[i] =
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?);
            offset += 4;
        }

        Some(result)
    }
}

/// A TxOut extended with cluster tag information.
///
/// In Phase 1, this wraps the existing TxOut with public tags.
/// In Phase 2, the tags would be committed/encrypted.
#[derive(Clone, Debug)]
pub struct TaggedTxOut {
    /// The underlying transaction output.
    /// Contains: masked_amount, target_key, public_key, e_fog_hint, e_memo
    ///
    /// For now, we reference it abstractly. In integration, this would be
    /// bt_transaction_core::TxOut.
    pub value_commitment: [u8; 32], // Placeholder for CompressedCommitment

    /// Public key for identifying the output.
    pub public_key: [u8; 32],

    /// Cluster tag vector for this output.
    pub tags: CompactTagVector,
}

impl TaggedTxOut {
    /// Create a new tagged output for a freshly minted coin.
    pub fn new_coinbase(
        value_commitment: [u8; 32],
        public_key: [u8; 32],
        cluster: ClusterId,
    ) -> Self {
        Self {
            value_commitment,
            public_key,
            tags: CompactTagVector::single(cluster),
        }
    }

    /// Create a new tagged output with inherited tags.
    pub fn new_transfer(
        value_commitment: [u8; 32],
        public_key: [u8; 32],
        tags: CompactTagVector,
    ) -> Self {
        Self {
            value_commitment,
            public_key,
            tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_tag_vector_single() {
        let cluster = ClusterId(42);
        let tags = CompactTagVector::single(cluster);

        assert_eq!(tags.count, 1);
        assert_eq!(tags.get(cluster), TAG_WEIGHT_SCALE);
        assert_eq!(tags.background_weight(), 0);
    }

    #[test]
    fn test_compact_tag_vector_roundtrip() {
        let mut map = HashMap::new();
        map.insert(ClusterId(1), 500_000);
        map.insert(ClusterId(2), 300_000);
        map.insert(ClusterId(3), 150_000);

        let compact = CompactTagVector::from_map(&map);
        let recovered = compact.to_map();

        assert_eq!(recovered.get(&ClusterId(1)), Some(&500_000));
        assert_eq!(recovered.get(&ClusterId(2)), Some(&300_000));
        assert_eq!(recovered.get(&ClusterId(3)), Some(&150_000));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let tags = CompactTagVector::single(ClusterId(12345));
        let bytes = tags.to_bytes();
        let recovered = CompactTagVector::from_bytes(&bytes).unwrap();

        assert_eq!(tags, recovered);
    }

    #[test]
    fn test_weight_pruning() {
        let mut map = HashMap::new();
        map.insert(ClusterId(1), 500_000);
        map.insert(ClusterId(2), 100); // Below MIN_STORED_WEIGHT

        let compact = CompactTagVector::from_map(&map);
        assert_eq!(compact.count, 1);
        assert_eq!(compact.get(ClusterId(1)), 500_000);
        assert_eq!(compact.get(ClusterId(2)), 0); // Pruned
    }
}
