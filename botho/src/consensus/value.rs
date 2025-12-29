// Copyright (c) 2024 Botho Foundation

//! Consensus value types for SCP.
//!
//! SCP reaches consensus on a set of values. In Botho, each value
//! is a transaction hash. The externalized set of transaction hashes
//! becomes the next block.

use bt_crypto_digestible::Digestible;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// A transaction hash that can be included in consensus.
///
/// This is the unit of consensus - SCP agrees on which transaction
/// hashes should be included in the next block.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Digestible)]
pub struct ConsensusValue {
    /// The transaction hash (32 bytes)
    pub tx_hash: [u8; 32],

    /// Whether this is a mining transaction
    pub is_mining_tx: bool,

    /// Priority (higher = more likely to be included first)
    /// Mining transactions use their PoW hash as priority
    pub priority: u64,
}

impl ConsensusValue {
    /// Create a new consensus value for a regular transaction
    pub fn from_transaction(tx_hash: [u8; 32]) -> Self {
        // Regular transactions have timestamp-based priority
        let priority = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            tx_hash,
            is_mining_tx: false,
            priority,
        }
    }

    /// Create a new consensus value for a mining transaction
    pub fn from_mining_tx(tx_hash: [u8; 32], pow_priority: u64) -> Self {
        Self {
            tx_hash,
            is_mining_tx: true,
            priority: pow_priority,
        }
    }

    /// Get the hash of this consensus value
    pub fn hash(&self) -> ConsensusValueHash {
        let mut hasher = Sha256::new();
        hasher.update(self.tx_hash);
        hasher.update([self.is_mining_tx as u8]);
        hasher.update(self.priority.to_le_bytes());
        ConsensusValueHash(hasher.finalize().into())
    }
}

impl fmt::Debug for ConsensusValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsensusValue")
            .field("tx_hash", &hex::encode(&self.tx_hash[..8]))
            .field("is_mining_tx", &self.is_mining_tx)
            .field("priority", &self.priority)
            .finish()
    }
}

impl fmt::Display for ConsensusValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.is_mining_tx { "mining" } else { "tx" };
        write!(f, "{}:{}", prefix, hex::encode(&self.tx_hash[..8]))
    }
}

/// Hash of a ConsensusValue for quick comparisons
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConsensusValueHash(pub [u8; 32]);

impl fmt::Debug for ConsensusValueHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CVH({})", hex::encode(&self.0[..8]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consensus_value_ordering() {
        let v1 = ConsensusValue::from_transaction([1u8; 32]);
        let v2 = ConsensusValue::from_transaction([2u8; 32]);

        // Should be ordered by tx_hash
        assert!(v1 < v2);
    }

    #[test]
    fn test_mining_vs_regular() {
        let mining = ConsensusValue::from_mining_tx([1u8; 32], 1000);
        let regular = ConsensusValue::from_transaction([1u8; 32]);

        assert!(mining.is_mining_tx);
        assert!(!regular.is_mining_tx);
    }

    #[test]
    fn test_hash_deterministic() {
        let v = ConsensusValue {
            tx_hash: [42u8; 32],
            is_mining_tx: true,
            priority: 12345,
        };

        let h1 = v.hash();
        let h2 = v.hash();
        assert_eq!(h1, h2);
    }
}
