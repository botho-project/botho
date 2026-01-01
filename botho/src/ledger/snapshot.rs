// Copyright (c) 2024 Botho Foundation

//! UTXO snapshot support for fast initial sync.
//!
//! This module provides functionality to create and load UTXO snapshots,
//! allowing new nodes to sync from a verified snapshot instead of replaying
//! the entire blockchain history.
//!
//! # Security
//!
//! Snapshots include Merkle roots for verification:
//! - UTXO set Merkle root
//! - Key image set Merkle root
//! - Block hash at snapshot height
//!
//! After loading, nodes should sync remaining blocks from snapshot height
//! to current tip, verifying the chain connects properly.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use thiserror::Error;

use super::ChainState;
use crate::transaction::Utxo;

/// Current snapshot format version
pub const SNAPSHOT_VERSION: u32 = 1;

/// Magic bytes for snapshot file identification
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"BTHSNAP\x01";

/// Zstd compression level (3 = balanced speed/compression)
const COMPRESSION_LEVEL: i32 = 3;

/// Errors that can occur during snapshot operations
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Invalid snapshot: {0}")]
    Invalid(String),

    #[error("Version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32 },

    #[error("Merkle root verification failed: {0}")]
    MerkleVerification(String),

    #[error("Block hash verification failed")]
    BlockHashVerification,

    #[error("Compression error: {0}")]
    Compression(String),

    #[error("Decompression error: {0}")]
    Decompression(String),
}

/// UTXO snapshot for fast initial sync.
///
/// Contains the complete UTXO set and key images at a specific block height,
/// along with Merkle roots for verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoSnapshot {
    /// Snapshot format version for compatibility
    pub version: u32,

    /// Block height at which snapshot was taken
    pub height: u64,

    /// Hash of the block at snapshot height (for verification)
    pub block_hash: [u8; 32],

    /// Merkle root of the UTXO set
    pub utxo_merkle_root: [u8; 32],

    /// Merkle root of the key image set
    pub key_image_merkle_root: [u8; 32],

    /// Total number of UTXOs in the snapshot
    pub utxo_count: u64,

    /// Total number of key images in the snapshot
    pub key_image_count: u64,

    /// Chain state metadata at snapshot height
    pub chain_state: ChainState,

    /// Serialized and compressed UTXO data
    pub utxo_data: Vec<u8>,

    /// Serialized and compressed key image data
    pub key_image_data: Vec<u8>,

    /// Serialized and compressed cluster wealth data
    pub cluster_wealth_data: Vec<u8>,
}

impl UtxoSnapshot {
    /// Create a new snapshot from UTXO and key image data.
    ///
    /// This computes Merkle roots and compresses the data for storage.
    pub fn new(
        height: u64,
        block_hash: [u8; 32],
        chain_state: ChainState,
        utxos: Vec<Utxo>,
        key_images: Vec<([u8; 32], u64)>, // (key_image, spent_height)
        cluster_wealth: Vec<(u64, u64)>,  // (cluster_id, wealth)
    ) -> Result<Self, SnapshotError> {
        // Compute UTXO Merkle root
        let utxo_hashes: Vec<[u8; 32]> = utxos
            .iter()
            .map(|utxo| {
                let bytes = bincode::serialize(utxo)
                    .map_err(|e| SnapshotError::Serialization(e.to_string()))?;
                Ok(hash_leaf(&bytes))
            })
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        let utxo_merkle_root = compute_merkle_root(&utxo_hashes);

        // Compute key image Merkle root
        let key_image_hashes: Vec<[u8; 32]> = key_images
            .iter()
            .map(|(ki, height)| {
                let mut data = ki.to_vec();
                data.extend_from_slice(&height.to_le_bytes());
                hash_leaf(&data)
            })
            .collect();
        let key_image_merkle_root = compute_merkle_root(&key_image_hashes);

        // Serialize and compress UTXO data
        let utxo_bytes = bincode::serialize(&utxos)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        let utxo_data = compress(&utxo_bytes)?;

        // Serialize and compress key image data
        let key_image_bytes = bincode::serialize(&key_images)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        let key_image_data = compress(&key_image_bytes)?;

        // Serialize and compress cluster wealth data
        let cluster_wealth_bytes = bincode::serialize(&cluster_wealth)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        let cluster_wealth_data = compress(&cluster_wealth_bytes)?;

        Ok(Self {
            version: SNAPSHOT_VERSION,
            height,
            block_hash,
            utxo_merkle_root,
            key_image_merkle_root,
            utxo_count: utxos.len() as u64,
            key_image_count: key_images.len() as u64,
            chain_state,
            utxo_data,
            key_image_data,
            cluster_wealth_data,
        })
    }

    /// Decompress and deserialize the UTXO data.
    pub fn get_utxos(&self) -> Result<Vec<Utxo>, SnapshotError> {
        let decompressed = decompress(&self.utxo_data)?;
        bincode::deserialize(&decompressed)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))
    }

    /// Decompress and deserialize the key image data.
    pub fn get_key_images(&self) -> Result<Vec<([u8; 32], u64)>, SnapshotError> {
        let decompressed = decompress(&self.key_image_data)?;
        bincode::deserialize(&decompressed)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))
    }

    /// Decompress and deserialize the cluster wealth data.
    pub fn get_cluster_wealth(&self) -> Result<Vec<(u64, u64)>, SnapshotError> {
        let decompressed = decompress(&self.cluster_wealth_data)?;
        bincode::deserialize(&decompressed)
            .map_err(|e| SnapshotError::Serialization(e.to_string()))
    }

    /// Verify the UTXO Merkle root matches the data.
    pub fn verify_utxo_merkle_root(&self) -> Result<bool, SnapshotError> {
        let utxos = self.get_utxos()?;
        let hashes: Vec<[u8; 32]> = utxos
            .iter()
            .map(|utxo| {
                let bytes = bincode::serialize(utxo)
                    .map_err(|e| SnapshotError::Serialization(e.to_string()))?;
                Ok(hash_leaf(&bytes))
            })
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        let computed_root = compute_merkle_root(&hashes);
        Ok(computed_root == self.utxo_merkle_root)
    }

    /// Verify the key image Merkle root matches the data.
    pub fn verify_key_image_merkle_root(&self) -> Result<bool, SnapshotError> {
        let key_images = self.get_key_images()?;
        let hashes: Vec<[u8; 32]> = key_images
            .iter()
            .map(|(ki, height)| {
                let mut data = ki.to_vec();
                data.extend_from_slice(&height.to_le_bytes());
                hash_leaf(&data)
            })
            .collect();
        let computed_root = compute_merkle_root(&hashes);
        Ok(computed_root == self.key_image_merkle_root)
    }

    /// Verify all Merkle roots in the snapshot.
    pub fn verify(&self) -> Result<(), SnapshotError> {
        if self.version != SNAPSHOT_VERSION {
            return Err(SnapshotError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                got: self.version,
            });
        }

        if !self.verify_utxo_merkle_root()? {
            return Err(SnapshotError::MerkleVerification(
                "UTXO Merkle root mismatch".to_string(),
            ));
        }

        if !self.verify_key_image_merkle_root()? {
            return Err(SnapshotError::MerkleVerification(
                "Key image Merkle root mismatch".to_string(),
            ));
        }

        Ok(())
    }

    /// Write snapshot to a writer with magic header.
    pub fn write_to<W: Write>(&self, mut writer: W) -> Result<(), SnapshotError> {
        // Write magic bytes
        writer.write_all(SNAPSHOT_MAGIC)?;

        // Serialize and write snapshot
        let data =
            bincode::serialize(self).map_err(|e| SnapshotError::Serialization(e.to_string()))?;
        writer.write_all(&data)?;

        Ok(())
    }

    /// Read snapshot from a reader, verifying magic header.
    pub fn read_from<R: Read>(mut reader: R) -> Result<Self, SnapshotError> {
        // Read and verify magic bytes
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != SNAPSHOT_MAGIC {
            return Err(SnapshotError::Invalid(
                "Invalid snapshot magic bytes".to_string(),
            ));
        }

        // Read remaining data
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        // Deserialize snapshot
        let snapshot: UtxoSnapshot =
            bincode::deserialize(&data).map_err(|e| SnapshotError::Serialization(e.to_string()))?;

        Ok(snapshot)
    }

    /// Get the estimated uncompressed size of the snapshot data.
    pub fn estimated_uncompressed_size(&self) -> u64 {
        // Rough estimate based on compression ratio
        (self.utxo_data.len() + self.key_image_data.len() + self.cluster_wealth_data.len()) as u64
            * 3
    }

    /// Get the compressed size of the snapshot data.
    pub fn compressed_size(&self) -> u64 {
        (self.utxo_data.len() + self.key_image_data.len() + self.cluster_wealth_data.len()) as u64
    }
}

// ============================================================================
// Merkle Tree Implementation
// ============================================================================

/// Hash a leaf node (single item).
fn hash_leaf(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x00]); // Leaf prefix
    hasher.update(data);
    hasher.finalize().into()
}

/// Hash two child nodes to create parent.
fn hash_branch(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x01]); // Branch prefix
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Compute Merkle root from a list of leaf hashes.
///
/// Uses a binary Merkle tree. For odd numbers of leaves, the last leaf
/// is paired with itself.
pub fn compute_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        // Empty tree has zero root
        return [0u8; 32];
    }

    if leaves.len() == 1 {
        return leaves[0];
    }

    let mut current_level = leaves.to_vec();

    while current_level.len() > 1 {
        let mut next_level = Vec::with_capacity((current_level.len() + 1) / 2);

        for chunk in current_level.chunks(2) {
            let left = &chunk[0];
            let right = if chunk.len() == 2 {
                &chunk[1]
            } else {
                // Odd number: duplicate last element
                left
            };
            next_level.push(hash_branch(left, right));
        }

        current_level = next_level;
    }

    current_level[0]
}

// ============================================================================
// Compression Helpers
// ============================================================================

/// Compress data using zstd.
fn compress(data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
    zstd::encode_all(data, COMPRESSION_LEVEL)
        .map_err(|e| SnapshotError::Compression(e.to_string()))
}

/// Decompress zstd-compressed data.
fn decompress(data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
    zstd::decode_all(data).map_err(|e| SnapshotError::Decompression(e.to_string()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{TxOutput, UtxoId};
    use bth_transaction_types::ClusterTagVector;

    fn test_utxo(id: u8) -> Utxo {
        Utxo {
            id: UtxoId::new([id; 32], id as u32),
            output: TxOutput {
                amount: 1000 * id as u64,
                target_key: [id; 32],
                public_key: [id.wrapping_add(1); 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::default(),
            },
            created_at: id as u64,
        }
    }

    fn test_chain_state() -> ChainState {
        ChainState {
            height: 1000,
            tip_hash: [1u8; 32],
            tip_timestamp: 1234567890,
            total_mined: 1_000_000_000,
            total_fees_burned: 100_000,
            difficulty: 1000,
            total_tx: 5000,
            epoch_tx: 500,
            epoch_emission: 10000,
            epoch_burns: 1000,
            current_reward: 1000,
        }
    }

    #[test]
    fn test_merkle_root_empty() {
        let root = compute_merkle_root(&[]);
        assert_eq!(root, [0u8; 32]);
    }

    #[test]
    fn test_merkle_root_single() {
        let leaf = hash_leaf(b"test data");
        let root = compute_merkle_root(&[leaf]);
        assert_eq!(root, leaf);
    }

    #[test]
    fn test_merkle_root_two() {
        let leaf1 = hash_leaf(b"data1");
        let leaf2 = hash_leaf(b"data2");
        let root = compute_merkle_root(&[leaf1, leaf2]);
        let expected = hash_branch(&leaf1, &leaf2);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_odd() {
        let leaf1 = hash_leaf(b"data1");
        let leaf2 = hash_leaf(b"data2");
        let leaf3 = hash_leaf(b"data3");
        let root = compute_merkle_root(&[leaf1, leaf2, leaf3]);

        // Expected: hash(hash(leaf1, leaf2), hash(leaf3, leaf3))
        let left = hash_branch(&leaf1, &leaf2);
        let right = hash_branch(&leaf3, &leaf3);
        let expected = hash_branch(&left, &right);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_compression_roundtrip() {
        let data = b"Hello, World! This is test data for compression.";
        let compressed = compress(data).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_snapshot_creation() {
        let utxos = vec![test_utxo(1), test_utxo(2), test_utxo(3)];
        let key_images = vec![([1u8; 32], 100u64), ([2u8; 32], 200u64)];
        let cluster_wealth = vec![(1u64, 1000u64), (2u64, 2000u64)];

        let snapshot = UtxoSnapshot::new(
            1000,
            [1u8; 32],
            test_chain_state(),
            utxos.clone(),
            key_images.clone(),
            cluster_wealth.clone(),
        )
        .unwrap();

        assert_eq!(snapshot.version, SNAPSHOT_VERSION);
        assert_eq!(snapshot.height, 1000);
        assert_eq!(snapshot.utxo_count, 3);
        assert_eq!(snapshot.key_image_count, 2);
    }

    #[test]
    fn test_snapshot_utxo_roundtrip() {
        let utxos = vec![test_utxo(1), test_utxo(2), test_utxo(3)];

        let snapshot = UtxoSnapshot::new(
            1000,
            [1u8; 32],
            test_chain_state(),
            utxos.clone(),
            vec![],
            vec![],
        )
        .unwrap();

        let recovered = snapshot.get_utxos().unwrap();
        assert_eq!(utxos.len(), recovered.len());
        for (orig, rec) in utxos.iter().zip(recovered.iter()) {
            assert_eq!(orig.id, rec.id);
            assert_eq!(orig.output.amount, rec.output.amount);
        }
    }

    #[test]
    fn test_snapshot_merkle_verification() {
        let utxos = vec![test_utxo(1), test_utxo(2)];
        let key_images = vec![([1u8; 32], 100u64)];

        let snapshot =
            UtxoSnapshot::new(1000, [1u8; 32], test_chain_state(), utxos, key_images, vec![])
                .unwrap();

        assert!(snapshot.verify_utxo_merkle_root().unwrap());
        assert!(snapshot.verify_key_image_merkle_root().unwrap());
        snapshot.verify().unwrap();
    }

    #[test]
    fn test_snapshot_file_roundtrip() {
        let utxos = vec![test_utxo(1), test_utxo(2)];

        let snapshot =
            UtxoSnapshot::new(1000, [1u8; 32], test_chain_state(), utxos, vec![], vec![]).unwrap();

        // Write to buffer
        let mut buffer = Vec::new();
        snapshot.write_to(&mut buffer).unwrap();

        // Read back
        let recovered = UtxoSnapshot::read_from(buffer.as_slice()).unwrap();

        assert_eq!(snapshot.version, recovered.version);
        assert_eq!(snapshot.height, recovered.height);
        assert_eq!(snapshot.utxo_count, recovered.utxo_count);
        assert_eq!(snapshot.utxo_merkle_root, recovered.utxo_merkle_root);
    }

    #[test]
    fn test_invalid_magic_bytes() {
        let data = b"INVALID\x00some data here";
        let result = UtxoSnapshot::read_from(data.as_slice());
        assert!(matches!(result, Err(SnapshotError::Invalid(_))));
    }
}
