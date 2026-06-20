// Copyright (c) 2024 Botho Foundation

//! Consensus value types for SCP.
//!
//! SCP reaches consensus on a set of values. In Botho, each value
//! is a transaction hash. The externalized set of transaction hashes
//! becomes the next block.

use bth_crypto_digestible::Digestible;
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

    /// Whether this is a minting transaction
    pub is_minting_tx: bool,

    /// Priority (higher = more likely to be included first)
    /// Minting transactions use their PoW hash as priority
    pub priority: u64,
}

impl ConsensusValue {
    /// Create a new consensus value for a regular (transfer) transaction.
    ///
    /// SAFETY (issue #449): `priority` MUST be a pure, deterministic function
    /// of the transaction so the same tx maps to the SAME `ConsensusValue`
    /// on every node. `ConsensusValue` derives `Eq/Ord/Hash` over ALL
    /// fields (incl. `priority`) and SCP nomination agrees on exact value
    /// identity. The old implementation used `SystemTime::now()`, so the
    /// same transfer tx became a DIFFERENT value on each node (and each
    /// tick) and nomination could never converge — the multi-node chain
    /// halted the instant a transfer entered the mempool. We use the
    /// transaction `fee` as the priority: it is identical on all nodes for
    /// the same tx and matches the combine_fn's documented "sort
    /// regular txs by priority (fee)" ordering (higher-fee txs order first
    /// within a block). This mirrors the minting path, which uses a
    /// deterministic PoW hash as its priority (issue #419).
    pub fn from_transaction(tx_hash: [u8; 32], fee: u64) -> Self {
        Self {
            tx_hash,
            is_minting_tx: false,
            priority: fee,
        }
    }

    /// Create a new consensus value for a minting transaction
    pub fn from_minting_tx(tx_hash: [u8; 32], pow_priority: u64) -> Self {
        Self {
            tx_hash,
            is_minting_tx: true,
            priority: pow_priority,
        }
    }

    /// Get the hash of this consensus value
    pub fn hash(&self) -> ConsensusValueHash {
        let mut hasher = Sha256::new();
        hasher.update(self.tx_hash);
        hasher.update([self.is_minting_tx as u8]);
        hasher.update(self.priority.to_le_bytes());
        ConsensusValueHash(hasher.finalize().into())
    }
}

impl fmt::Debug for ConsensusValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsensusValue")
            .field("tx_hash", &hex::encode(&self.tx_hash[..8]))
            .field("is_minting_tx", &self.is_minting_tx)
            .field("priority", &self.priority)
            .finish()
    }
}

impl fmt::Display for ConsensusValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.is_minting_tx { "minting" } else { "tx" };
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
        // Same fee/priority so ordering falls back to tx_hash.
        let v1 = ConsensusValue::from_transaction([1u8; 32], 0);
        let v2 = ConsensusValue::from_transaction([2u8; 32], 0);

        // Should be ordered by tx_hash
        assert!(v1 < v2);
    }

    #[test]
    fn test_minting_vs_regular() {
        let minting = ConsensusValue::from_minting_tx([1u8; 32], 1000);
        let regular = ConsensusValue::from_transaction([1u8; 32], 1000);

        assert!(minting.is_minting_tx);
        assert!(!regular.is_minting_tx);
    }

    #[test]
    fn test_hash_deterministic() {
        let v = ConsensusValue {
            tx_hash: [42u8; 32],
            is_minting_tx: true,
            priority: 12345,
        };

        let h1 = v.hash();
        let h2 = v.hash();
        assert_eq!(h1, h2);
    }

    /// Issue #449: the same transfer tx must map to the SAME `ConsensusValue`
    /// regardless of when/where it is constructed. Previously `priority` came
    /// from `SystemTime::now()`, so two constructions (or two nodes) produced
    /// non-equal values and SCP nomination could never converge.
    #[test]
    fn test_from_transaction_deterministic() {
        let tx_hash = [9u8; 32];
        let fee = 4242;

        let a = ConsensusValue::from_transaction(tx_hash, fee);
        let b = ConsensusValue::from_transaction(tx_hash, fee);

        // Identical value identity (Eq/Ord/Hash all over priority too).
        assert_eq!(a, b);
        assert_eq!(a.priority, fee);
        assert_eq!(a.hash(), b.hash());

        // A different fee yields a different value (priority participates in
        // identity), so two distinct submissions of the SAME tx_hash with
        // different fees do not silently collide.
        let c = ConsensusValue::from_transaction(tx_hash, fee + 1);
        assert_ne!(a, c);
    }
}
