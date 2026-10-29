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
//!
//! # Versions
//!
//! - **V1** ([`UtxoSnapshot`], magic `BTHSNAP\x01`): UTXOs, key images and
//!   cluster wealth. It carries no per-output derivation contexts, so
//!   `botho.ledger.2` ledgers refuse to load it (#1286 P2). The type is kept
//!   because test fixtures still construct it for
//!   `Ledger::seed_fixture_snapshot_unvalidated`.
//! - **V2** ([`UtxoSnapshotV2`], magic `BTHSNAP\x02`, #1646): the
//!   `botho.ledger.2` snapshot. It adds the derivation-context table, the
//!   storage marker, the genesis hash, the tip block itself, the lottery pool
//!   carryover, the address index and the bridge-import cluster set, and binds
//!   all of them (plus every section root) into one SHA-256
//!   [`UtxoSnapshotV2::commitment`]. The commitment is computed over the
//!   *decompressed* section contents, so it does not depend on the zstd version
//!   that produced the file.

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

/// Snapshot format version carrying derivation contexts (`botho.ledger.2`).
pub const SNAPSHOT_VERSION_V2: u32 = 2;

/// Magic bytes of a V2 snapshot file.
pub const SNAPSHOT_MAGIC_V2: &[u8; 8] = b"BTHSNAP\x02";

/// Storage schema a V2 snapshot belongs to (the ledger's `storage_schema`).
pub const SNAPSHOT_V2_SCHEMA: &[u8] = b"botho.ledger.2";

/// Domain separator of the V2 snapshot commitment.
const SNAPSHOT_V2_DOMAIN: &[u8] = b"botho.snapshot.2";

/// Width of an outpoint key (`UtxoId::to_bytes()`).
const OUTPOINT_LEN: usize = 36;

/// Hard ceiling on the total decompressed size of a V2 snapshot's sections
/// and tip block (2 GiB). Every section's output is additionally capped by
/// its declared row count (see [`UtxoSnapshotV2::section_caps`]), so a forged
/// or corrupt file can never make decompression allocate more than this,
/// whatever its compression ratio. The ledger's LMDB map is 1 GiB
/// (`LEDGER_MAP_SIZE`), so no loadable snapshot comes close.
pub const MAX_SNAPSHOT_DECOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;

/// Decompressed-size cap of the tip block (the stored-block bound of the
/// strict ledger decoder, 16 MiB).
pub const MAX_SNAPSHOT_TIP_BLOCK: u64 = 16 * 1024 * 1024;

/// Conservative per-row maximum of a bincode `Utxo`, used as an *average*
/// bound for the UTXO section. A protocol-6 hybrid output is about 1.5 KB
/// (36 B id, amounts/keys, at most `MAX_CLUSTER_TAGS` = 16 tag entries, an
/// optional memo and a 1,088 B ML-KEM-768 ciphertext); 4 KiB leaves headroom.
const MAX_UTXO_ROW: u64 = 4096;

/// bincode `Vec` length prefix.
const LEN_PREFIX: u64 = 8;

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
        cluster_wealth: Vec<(u64, u128)>, // (cluster_id, wealth) — u128 pico (#626)
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
        let utxo_bytes =
            bincode::serialize(&utxos).map_err(|e| SnapshotError::Serialization(e.to_string()))?;
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
        bincode::deserialize(&decompressed).map_err(|e| SnapshotError::Serialization(e.to_string()))
    }

    /// Decompress and deserialize the key image data.
    pub fn get_key_images(&self) -> Result<Vec<([u8; 32], u64)>, SnapshotError> {
        let decompressed = decompress(&self.key_image_data)?;
        bincode::deserialize(&decompressed).map_err(|e| SnapshotError::Serialization(e.to_string()))
    }

    /// Decompress and deserialize the cluster wealth data.
    pub fn get_cluster_wealth(&self) -> Result<Vec<(u64, u128)>, SnapshotError> {
        let decompressed = decompress(&self.cluster_wealth_data)?;
        bincode::deserialize(&decompressed).map_err(|e| SnapshotError::Serialization(e.to_string()))
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
// Snapshot format V2 (botho.ledger.2, #1646)
// ============================================================================

/// The decompressed, decoded sections of a V2 snapshot. Every list is in
/// strictly ascending key order (the LMDB iteration order of its table).
#[derive(Debug, Clone, Default)]
pub struct SnapshotV2Contents {
    /// Every unspent output, ascending by `UtxoId::to_bytes()`.
    pub utxos: Vec<Utxo>,
    /// `(outpoint key (36 B), encoded StoredContext)`, one per UTXO, same
    /// order as `utxos`.
    pub contexts: Vec<(Vec<u8>, Vec<u8>)>,
    /// `(target key, concatenated 36 B outpoint keys)`, ascending by target
    /// key, verbatim from the source ledger. Carried (not rebuilt) because
    /// the in-row order is creation order, which the canonical first-index
    /// target lookup (#1443/#1444) depends on and the sorted UTXO list does
    /// not preserve. Every UTXO appears exactly once, under its target key.
    pub address_index: Vec<([u8; 32], Vec<u8>)>,
    /// `(key image, spent height)`, ascending by key image.
    pub key_images: Vec<([u8; 32], u64)>,
    /// `(cluster id, wealth)`, ascending by little-endian cluster key.
    pub cluster_wealth: Vec<(u64, u128)>,
    /// Bridge-import cluster ids, ascending by little-endian cluster key.
    pub import_clusters: Vec<u64>,
}

/// UTXO snapshot format V2 for `botho.ledger.2` ledgers.
///
/// Adds, relative to V1: the storage marker, the genesis hash, the tip block
/// (so a restored ledger has a real tip for `get_tip` and the next block's
/// parent/timestamp checks), the lottery pool carryover, the derivation
/// contexts, the address index and the bridge-import cluster set. Integrity is
/// one SHA-256 [`commitment`](Self::commitment) over every header field and
/// every section's Merkle root; [`verify`](Self::verify) recomputes all of them
/// from the decompressed data, so tampering with any context, UTXO, key image
/// or header field is detected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoSnapshotV2 {
    /// Always [`SNAPSHOT_VERSION_V2`].
    pub version: u32,
    /// Storage schema marker of the source ledger (`botho.ledger.2`).
    pub storage_schema: Vec<u8>,
    /// Hash of block 0 of the source chain.
    pub genesis_hash: [u8; 32],
    /// Block height at which the snapshot was taken.
    pub height: u64,
    /// Hash of the tip block at `height`.
    pub block_hash: [u8; 32],
    /// Chain state metadata at `height`.
    pub chain_state: ChainState,
    /// Lottery pool carryover after the tip block.
    pub lottery_pool: u128,
    /// zstd(bincode(tip `Block`)).
    pub tip_block_data: Vec<u8>,
    pub utxo_count: u64,
    pub context_count: u64,
    pub address_index_count: u64,
    pub key_image_count: u64,
    pub cluster_count: u64,
    pub import_cluster_count: u64,
    pub utxo_merkle_root: [u8; 32],
    pub context_merkle_root: [u8; 32],
    pub address_index_merkle_root: [u8; 32],
    pub key_image_merkle_root: [u8; 32],
    pub cluster_wealth_merkle_root: [u8; 32],
    pub import_cluster_merkle_root: [u8; 32],
    /// zstd(bincode(section)) for each section of [`SnapshotV2Contents`].
    pub utxo_data: Vec<u8>,
    pub context_data: Vec<u8>,
    pub address_index_data: Vec<u8>,
    pub key_image_data: Vec<u8>,
    pub cluster_wealth_data: Vec<u8>,
    pub import_cluster_data: Vec<u8>,
    /// SHA-256 binding every field above (see [`UtxoSnapshotV2::verify`]).
    pub commitment: [u8; 32],
}

/// Section roots and the tip-block digest, recomputed from contents.
struct V2Roots {
    utxo: [u8; 32],
    context: [u8; 32],
    address_index: [u8; 32],
    key_image: [u8; 32],
    cluster_wealth: [u8; 32],
    import_cluster: [u8; 32],
}

fn ser<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, SnapshotError> {
    bincode::serialize(value).map_err(|e| SnapshotError::Serialization(e.to_string()))
}

/// Strict bincode decode (legacy fixint encoding, as `bincode::serialize`
/// writes it) that rejects trailing bytes.
fn de_strict<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, SnapshotError> {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .deserialize(bytes)
        .map_err(|e| SnapshotError::Serialization(e.to_string()))
}

fn strictly_ascending<K: Ord>(
    keys: impl Iterator<Item = K>,
    what: &str,
) -> Result<(), SnapshotError> {
    let mut prev: Option<K> = None;
    for key in keys {
        if prev.as_ref().is_some_and(|p| *p >= key) {
            return Err(SnapshotError::Invalid(format!(
                "{what} section is not in strictly ascending key order (duplicate or unsorted)"
            )));
        }
        prev = Some(key);
    }
    Ok(())
}

impl SnapshotV2Contents {
    /// Canonical-order and shape checks shared by creation and verification.
    fn check_canonical(&self) -> Result<(), SnapshotError> {
        strictly_ascending(self.utxos.iter().map(|u| u.id.to_bytes()), "UTXO")?;
        strictly_ascending(self.contexts.iter().map(|(k, _)| k.clone()), "context")?;
        strictly_ascending(self.address_index.iter().map(|(k, _)| *k), "address index")?;
        strictly_ascending(self.key_images.iter().map(|(k, _)| *k), "key image")?;
        strictly_ascending(
            self.cluster_wealth.iter().map(|(id, _)| id.to_le_bytes()),
            "cluster wealth",
        )?;
        strictly_ascending(
            self.import_clusters.iter().map(|id| id.to_le_bytes()),
            "import cluster",
        )?;
        if self.contexts.len() != self.utxos.len() {
            return Err(SnapshotError::Invalid(format!(
                "{} derivation contexts for {} UTXOs",
                self.contexts.len(),
                self.utxos.len()
            )));
        }
        for (utxo, (key, _)) in self.utxos.iter().zip(&self.contexts) {
            if key.len() != OUTPOINT_LEN || key[..] != utxo.id.to_bytes()[..] {
                return Err(SnapshotError::Invalid(
                    "derivation context outpoint does not match its UTXO".into(),
                ));
            }
        }
        // Address index: an exact partition of the UTXO set by target key.
        let mut targets: std::collections::HashMap<[u8; OUTPOINT_LEN], Option<[u8; 32]>> = self
            .utxos
            .iter()
            .map(|u| (u.id.to_bytes(), Some(u.output.target_key)))
            .collect();
        for (target, ids) in &self.address_index {
            if ids.is_empty() || ids.len() % OUTPOINT_LEN != 0 {
                return Err(SnapshotError::Invalid("address index row width".into()));
            }
            for id in ids.chunks(OUTPOINT_LEN) {
                let id: [u8; OUTPOINT_LEN] = id.try_into().expect("chunk width");
                match targets.get_mut(&id) {
                    Some(slot) if *slot == Some(*target) => *slot = None,
                    _ => {
                        return Err(SnapshotError::Invalid(
                            "address index entry is not an unindexed UTXO with that target key"
                                .into(),
                        ))
                    }
                }
            }
        }
        if targets.values().any(Option::is_some) {
            return Err(SnapshotError::Invalid(
                "UTXO missing from the address index".into(),
            ));
        }
        Ok(())
    }

    fn roots(&self) -> Result<V2Roots, SnapshotError> {
        let utxo = self
            .utxos
            .iter()
            .map(|u| Ok(hash_leaf(&ser(u)?)))
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        let context: Vec<_> = self
            .contexts
            .iter()
            .map(|(k, v)| {
                let mut data = k.clone();
                data.extend_from_slice(v);
                hash_leaf(&data)
            })
            .collect();
        let address_index: Vec<_> = self
            .address_index
            .iter()
            .map(|(target, ids)| {
                let mut data = target.to_vec();
                data.extend_from_slice(ids);
                hash_leaf(&data)
            })
            .collect();
        let key_image: Vec<_> = self
            .key_images
            .iter()
            .map(|(ki, h)| {
                let mut data = ki.to_vec();
                data.extend_from_slice(&h.to_le_bytes());
                hash_leaf(&data)
            })
            .collect();
        let cluster_wealth: Vec<_> = self
            .cluster_wealth
            .iter()
            .map(|(id, w)| {
                let mut data = id.to_le_bytes().to_vec();
                data.extend_from_slice(&w.to_le_bytes());
                hash_leaf(&data)
            })
            .collect();
        let import_cluster: Vec<_> = self
            .import_clusters
            .iter()
            .map(|id| hash_leaf(&id.to_le_bytes()))
            .collect();
        Ok(V2Roots {
            utxo: compute_merkle_root(&utxo),
            context: compute_merkle_root(&context),
            address_index: compute_merkle_root(&address_index),
            key_image: compute_merkle_root(&key_image),
            cluster_wealth: compute_merkle_root(&cluster_wealth),
            import_cluster: compute_merkle_root(&import_cluster),
        })
    }
}

impl UtxoSnapshotV2 {
    /// Build a V2 snapshot. `tip_block` is the bincode encoding of the block
    /// at `height` (its hash must be `block_hash`; the ledger checks that).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        genesis_hash: [u8; 32],
        height: u64,
        block_hash: [u8; 32],
        chain_state: ChainState,
        lottery_pool: u128,
        tip_block: &[u8],
        contents: &SnapshotV2Contents,
    ) -> Result<Self, SnapshotError> {
        contents.check_canonical()?;
        let roots = contents.roots()?;
        let mut snapshot = Self {
            version: SNAPSHOT_VERSION_V2,
            storage_schema: SNAPSHOT_V2_SCHEMA.to_vec(),
            genesis_hash,
            height,
            block_hash,
            chain_state,
            lottery_pool,
            tip_block_data: compress(tip_block)?,
            utxo_count: contents.utxos.len() as u64,
            context_count: contents.contexts.len() as u64,
            address_index_count: contents.address_index.len() as u64,
            key_image_count: contents.key_images.len() as u64,
            cluster_count: contents.cluster_wealth.len() as u64,
            import_cluster_count: contents.import_clusters.len() as u64,
            utxo_merkle_root: roots.utxo,
            context_merkle_root: roots.context,
            address_index_merkle_root: roots.address_index,
            key_image_merkle_root: roots.key_image,
            cluster_wealth_merkle_root: roots.cluster_wealth,
            import_cluster_merkle_root: roots.import_cluster,
            utxo_data: compress(&ser(&contents.utxos)?)?,
            context_data: compress(&ser(&contents.contexts)?)?,
            address_index_data: compress(&ser(&contents.address_index)?)?,
            key_image_data: compress(&ser(&contents.key_images)?)?,
            cluster_wealth_data: compress(&ser(&contents.cluster_wealth)?)?,
            import_cluster_data: compress(&ser(&contents.import_clusters)?)?,
            commitment: [0; 32],
        };
        snapshot.commitment = snapshot.compute_commitment(&sha256(tip_block))?;
        Ok(snapshot)
    }

    /// SHA-256 over the domain tag and every header field, the decompressed
    /// tip block's digest, every count and every section root. Compressed
    /// bytes are not hashed directly: [`verify`](Self::verify) recomputes the
    /// roots from the decompressed sections instead.
    fn compute_commitment(&self, tip_digest: &[u8; 32]) -> Result<[u8; 32], SnapshotError> {
        let mut h = Sha256::new();
        h.update((SNAPSHOT_V2_DOMAIN.len() as u64).to_le_bytes());
        h.update(SNAPSHOT_V2_DOMAIN);
        h.update(self.version.to_le_bytes());
        h.update((self.storage_schema.len() as u64).to_le_bytes());
        h.update(&self.storage_schema);
        h.update(self.genesis_hash);
        h.update(self.height.to_le_bytes());
        h.update(self.block_hash);
        let chain_state = ser(&self.chain_state)?;
        h.update((chain_state.len() as u64).to_le_bytes());
        h.update(&chain_state);
        h.update(self.lottery_pool.to_le_bytes());
        h.update(tip_digest);
        for count in [
            self.utxo_count,
            self.context_count,
            self.address_index_count,
            self.key_image_count,
            self.cluster_count,
            self.import_cluster_count,
        ] {
            h.update(count.to_le_bytes());
        }
        for root in [
            &self.utxo_merkle_root,
            &self.context_merkle_root,
            &self.address_index_merkle_root,
            &self.key_image_merkle_root,
            &self.cluster_wealth_merkle_root,
            &self.import_cluster_merkle_root,
        ] {
            h.update(root);
        }
        Ok(h.finalize().into())
    }

    /// Decompressed tip block bytes (bincode `Block`), capped at
    /// [`MAX_SNAPSHOT_TIP_BLOCK`].
    pub fn tip_block_bytes(&self) -> Result<Vec<u8>, SnapshotError> {
        decompress_bounded(&self.tip_block_data, MAX_SNAPSHOT_TIP_BLOCK, "tip block")
    }

    /// Decompressed-size cap of each section, derived from the declared row
    /// counts before anything is decompressed: `utxos, contexts,
    /// address_index, key_images, cluster_wealth, import_clusters`. Fails if
    /// any cap overflows or their sum plus the tip-block cap exceeds
    /// [`MAX_SNAPSHOT_DECOMPRESSED`].
    pub fn section_caps(&self) -> Result<[u64; 6], SnapshotError> {
        let too_big = || {
            SnapshotError::Invalid(format!(
                "declared row counts exceed the {MAX_SNAPSHOT_DECOMPRESSED} B decompressed \
                 snapshot ceiling"
            ))
        };
        let cap = |count: u64, row: u64| {
            count
                .checked_mul(row)
                .and_then(|n| n.checked_add(LEN_PREFIX))
                .ok_or_else(too_big)
        };
        let caps = [
            cap(self.utxo_count, MAX_UTXO_ROW)?,
            // (Vec<u8> 36 B key, Vec<u8> <= 49 B Lottery encoding)
            cap(self.context_count, LEN_PREFIX + 36 + LEN_PREFIX + 49)?,
            // Rows of ([u8; 32], Vec<u8>); their ids total 36 B per UTXO.
            cap(self.address_index_count, 32 + LEN_PREFIX)?
                .checked_add(self.utxo_count.checked_mul(36).ok_or_else(too_big)?)
                .ok_or_else(too_big)?,
            cap(self.key_image_count, 32 + 8)?,
            cap(self.cluster_count, 8 + 16)?,
            cap(self.import_cluster_count, 8)?,
        ];
        let total = caps
            .iter()
            .try_fold(MAX_SNAPSHOT_TIP_BLOCK, |sum, c| sum.checked_add(*c))
            .ok_or_else(too_big)?;
        if total > MAX_SNAPSHOT_DECOMPRESSED {
            return Err(too_big());
        }
        Ok(caps)
    }

    /// Decompress (each section bounded by [`Self::section_caps`]) and
    /// strictly decode every section. No integrity check beyond the bounds.
    pub fn contents(&self) -> Result<SnapshotV2Contents, SnapshotError> {
        let [utxo, context, address, key_image, cluster, import] = self.section_caps()?;
        Ok(SnapshotV2Contents {
            utxos: de_strict(&decompress_bounded(&self.utxo_data, utxo, "UTXO")?)?,
            contexts: de_strict(&decompress_bounded(&self.context_data, context, "context")?)?,
            address_index: de_strict(&decompress_bounded(
                &self.address_index_data,
                address,
                "address index",
            )?)?,
            key_images: de_strict(&decompress_bounded(
                &self.key_image_data,
                key_image,
                "key image",
            )?)?,
            cluster_wealth: de_strict(&decompress_bounded(
                &self.cluster_wealth_data,
                cluster,
                "cluster wealth",
            )?)?,
            import_clusters: de_strict(&decompress_bounded(
                &self.import_cluster_data,
                import,
                "import cluster",
            )?)?,
        })
    }

    /// Full integrity check. Returns the verified contents and tip block
    /// bytes, so a loader never acts on data other than what was verified.
    ///
    /// Checks, cheapest first: version and storage marker; the decompression
    /// bounds implied by the declared counts; the commitment over the header
    /// (declared counts and roots) and the bounded tip block, before any
    /// section is decompressed; then, per bounded section, canonical order,
    /// the one-context-per-UTXO and address-index bijections, and every
    /// count and Merkle root against the decompressed data.
    pub fn verify(&self) -> Result<(SnapshotV2Contents, Vec<u8>), SnapshotError> {
        if self.version != SNAPSHOT_VERSION_V2 {
            return Err(SnapshotError::VersionMismatch {
                expected: SNAPSHOT_VERSION_V2,
                got: self.version,
            });
        }
        if self.storage_schema != SNAPSHOT_V2_SCHEMA {
            return Err(SnapshotError::Invalid(format!(
                "snapshot storage schema {:?} is not {:?}",
                String::from_utf8_lossy(&self.storage_schema),
                String::from_utf8_lossy(SNAPSHOT_V2_SCHEMA),
            )));
        }
        self.section_caps()?;
        let tip = self.tip_block_bytes()?;
        if self.compute_commitment(&sha256(&tip))? != self.commitment {
            return Err(SnapshotError::MerkleVerification(
                "snapshot commitment mismatch".into(),
            ));
        }
        let contents = self.contents()?;
        contents.check_canonical()?;
        let counts = [
            (self.utxo_count, contents.utxos.len(), "UTXO"),
            (self.context_count, contents.contexts.len(), "context"),
            (
                self.address_index_count,
                contents.address_index.len(),
                "address index",
            ),
            (self.key_image_count, contents.key_images.len(), "key image"),
            (
                self.cluster_count,
                contents.cluster_wealth.len(),
                "cluster wealth",
            ),
            (
                self.import_cluster_count,
                contents.import_clusters.len(),
                "import cluster",
            ),
        ];
        for (declared, actual, what) in counts {
            if declared != actual as u64 {
                return Err(SnapshotError::Invalid(format!(
                    "{what} count {declared} does not match {actual} decoded rows"
                )));
            }
        }
        let roots = contents.roots()?;
        let checks = [
            (roots.utxo, self.utxo_merkle_root, "UTXO"),
            (
                roots.context,
                self.context_merkle_root,
                "derivation context",
            ),
            (
                roots.address_index,
                self.address_index_merkle_root,
                "address index",
            ),
            (roots.key_image, self.key_image_merkle_root, "key image"),
            (
                roots.cluster_wealth,
                self.cluster_wealth_merkle_root,
                "cluster wealth",
            ),
            (
                roots.import_cluster,
                self.import_cluster_merkle_root,
                "import cluster",
            ),
        ];
        for (computed, declared, what) in checks {
            if computed != declared {
                return Err(SnapshotError::MerkleVerification(format!(
                    "{what} Merkle root mismatch"
                )));
            }
        }
        Ok((contents, tip))
    }

    /// Write `BTHSNAP\x02` followed by the bincode-encoded snapshot.
    pub fn write_to<W: Write>(&self, mut writer: W) -> Result<(), SnapshotError> {
        writer.write_all(SNAPSHOT_MAGIC_V2)?;
        writer.write_all(&ser(self)?)?;
        Ok(())
    }

    /// Read a V2 snapshot. A V1 file (`BTHSNAP\x01`) is reported as
    /// [`SnapshotError::VersionMismatch`] `{ expected: 2, got: 1 }` without
    /// decoding its body; a truncated file or trailing bytes are errors.
    pub fn read_from<R: Read>(mut reader: R) -> Result<Self, SnapshotError> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic == SNAPSHOT_MAGIC {
            return Err(SnapshotError::VersionMismatch {
                expected: SNAPSHOT_VERSION_V2,
                got: SNAPSHOT_VERSION,
            });
        }
        if &magic != SNAPSHOT_MAGIC_V2 {
            return Err(SnapshotError::Invalid(
                "Invalid snapshot magic bytes".to_string(),
            ));
        }
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        de_strict(&data)
    }

    /// Compressed size of all sections and the tip block.
    pub fn compressed_size(&self) -> u64 {
        (self.tip_block_data.len()
            + self.utxo_data.len()
            + self.context_data.len()
            + self.address_index_data.len()
            + self.key_image_data.len()
            + self.cluster_wealth_data.len()
            + self.import_cluster_data.len()) as u64
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
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
        let mut next_level = Vec::with_capacity(current_level.len().div_ceil(2));

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
    zstd::encode_all(data, COMPRESSION_LEVEL).map_err(|e| SnapshotError::Compression(e.to_string()))
}

/// Decompress zstd data, refusing to produce more than `cap` bytes. The
/// output buffer grows only with bytes actually produced (streaming, never
/// pre-sized from an untrusted length), and decoding stops at `cap + 1`.
fn decompress_bounded(data: &[u8], cap: u64, what: &str) -> Result<Vec<u8>, SnapshotError> {
    let decoder = zstd::stream::read::Decoder::with_buffer(data)
        .map_err(|e| SnapshotError::Decompression(e.to_string()))?;
    let mut out = Vec::new();
    decoder
        .take(cap.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| SnapshotError::Decompression(e.to_string()))?;
    if out.len() as u64 > cap {
        return Err(SnapshotError::Decompression(format!(
            "{what} section decompresses past its {cap} B bound"
        )));
    }
    Ok(out)
}

/// Decompress zstd-compressed data (unbounded: V1 snapshots only, which
/// `botho.ledger.2` ledgers refuse to load; never used on V2 input).
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
                kem_ciphertext: None,
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

    /// Payload used for the zstd compatibility fixture below.
    fn zstd_compat_payload() -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0u32..64 {
            data.extend_from_slice(b"botho snapshot zstd compat ");
            data.extend_from_slice(&i.to_le_bytes());
        }
        data
    }

    /// Frame produced by `zstd::encode_all(zstd_compat_payload(), 3)` with
    /// zstd 0.13.3 / zstd-safe 7.2.4 / zstd-sys 2.0.16+zstd.1.5.7, the stack
    /// shipped before the RUSTSEC-2026-0391/0398/0403/0410 bump (#1617).
    const ZSTD_0_13_FRAME_HEX: &[&str] = &[
        "28b52ffd0058ed05001406626f74686f20736e617073686f74207a73746420636f6d706174200000",
        "00000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20212223242526",
        "2728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f0000003f20e03703a288273251c413",
        "9928e2894c14f144268a782213453c9189423c9189229ec844114f64a288273251c4139928e2894c",
        "14f1444614f144268a782213453c9189229ec844114f64a288273251c4233251827817617202",
    ];

    fn decode_hex(parts: &[&str]) -> Vec<u8> {
        let hex: String = parts.concat();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Snapshots written by older nodes (zstd 0.13) must still decode after
    /// the zstd upgrade, since snapshot files are persisted on disk.
    #[test]
    fn test_decompress_zstd_0_13_frame() {
        let frame = decode_hex(ZSTD_0_13_FRAME_HEX);
        let decompressed = decompress(&frame).unwrap();
        assert_eq!(decompressed, zstd_compat_payload());
    }

    /// The current encoder's output for the fixture payload must round-trip.
    #[test]
    fn test_zstd_compat_payload_roundtrip() {
        let payload = zstd_compat_payload();
        let compressed = compress(&payload).unwrap();
        assert_eq!(decompress(&compressed).unwrap(), payload);
    }

    #[test]
    fn test_snapshot_creation() {
        let utxos = vec![test_utxo(1), test_utxo(2), test_utxo(3)];
        let key_images = vec![([1u8; 32], 100u64), ([2u8; 32], 200u64)];
        let cluster_wealth = vec![(1u64, 1000u128), (2u64, 2000u128)];

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

        let snapshot = UtxoSnapshot::new(
            1000,
            [1u8; 32],
            test_chain_state(),
            utxos,
            key_images,
            vec![],
        )
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

    /// #333: ChainState carries u128 supply accumulators. A snapshot whose
    /// `total_mined`/`total_fees_burned` exceed u64::MAX must round-trip
    /// through the serde/bincode snapshot file path without truncation.
    #[test]
    fn test_snapshot_roundtrip_total_mined_past_u64_max() {
        // ~1.22e21 picocredits (realistic Phase-2 gross emission), well above
        // u64::MAX (~1.84e19).
        let big_mined: u128 = 1_220_000_000_000_000_000_000;
        let big_burned: u128 = u64::MAX as u128 + 42;
        assert!(big_mined > u64::MAX as u128);
        assert!(big_burned > u64::MAX as u128);

        let mut cs = test_chain_state();
        cs.total_mined = big_mined;
        cs.total_fees_burned = big_burned;

        let snapshot =
            UtxoSnapshot::new(1000, [1u8; 32], cs, vec![test_utxo(1)], vec![], vec![]).unwrap();

        let mut buffer = Vec::new();
        snapshot.write_to(&mut buffer).unwrap();
        let recovered = UtxoSnapshot::read_from(buffer.as_slice()).unwrap();

        assert_eq!(recovered.chain_state.total_mined, big_mined);
        assert_eq!(recovered.chain_state.total_fees_burned, big_burned);
    }
}
