// Copyright (c) 2024 Cadence Foundation

//! Mining transaction types for Cadence PoW consensus.
//!
//! A `MiningTx` represents a proof-of-work claim that, when valid, entitles
//! the miner to receive the block reward. Unlike regular transactions that
//! transfer value, mining transactions create new coins according to the
//! emission schedule.

use alloc::vec::Vec;
use core::fmt;
use mc_crypto_digestible::Digestible;
use mc_crypto_keys::{CompressedRistrettoPublic, RistrettoPublic};
use prost::Message;

use crate::emission::block_reward;

/// Error types for mining transaction validation
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MiningTxError {
    /// The previous block hash doesn't match
    InvalidPrevBlockHash,
    /// The height is invalid (e.g., already mined or too far ahead)
    InvalidHeight,
    /// The PoW doesn't meet the required difficulty
    InsufficientDifficulty,
    /// The claimed reward exceeds the allowed amount
    InvalidReward,
    /// The recipient address is malformed
    InvalidRecipient,
    /// The nonce has already been used (duplicate mining tx)
    DuplicateNonce,
    /// Other validation error
    Other(alloc::string::String),
}

impl fmt::Display for MiningTxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrevBlockHash => write!(f, "Invalid previous block hash"),
            Self::InvalidHeight => write!(f, "Invalid block height"),
            Self::InsufficientDifficulty => write!(f, "PoW does not meet difficulty target"),
            Self::InvalidReward => write!(f, "Invalid mining reward amount"),
            Self::InvalidRecipient => write!(f, "Invalid recipient address"),
            Self::DuplicateNonce => write!(f, "Duplicate nonce"),
            Self::Other(msg) => write!(f, "Mining error: {}", msg),
        }
    }
}

/// A mining transaction that claims block rewards via proof-of-work.
///
/// The mining transaction contains:
/// - A reference to the previous block (for chain validity)
/// - The target height being mined
/// - The miner's recipient address (as view and spend public keys)
/// - A nonce that, combined with other fields, produces a valid PoW hash
#[derive(Clone, Digestible, Eq, Hash, Message, PartialEq)]
pub struct MiningTx {
    /// Hash of the previous block in the chain.
    /// This links the mining tx to a specific chain state.
    #[prost(bytes, tag = 1)]
    pub prev_block_hash: Vec<u8>,

    /// The block height this mining tx is for.
    /// Determines the emission reward amount.
    #[prost(uint64, tag = 2)]
    pub height: u64,

    /// The recipient's view public key (32 bytes compressed).
    #[prost(message, required, tag = 3)]
    pub recipient_view_key: CompressedRistrettoPublic,

    /// The recipient's spend public key (32 bytes compressed).
    #[prost(message, required, tag = 4)]
    pub recipient_spend_key: CompressedRistrettoPublic,

    /// The nonce value found through PoW mining.
    /// Miners iterate this value until the hash meets difficulty.
    #[prost(uint64, tag = 5)]
    pub nonce: u64,

    /// Optional extra data (limited to 32 bytes).
    /// Can be used for pool identification or other metadata.
    #[prost(bytes, tag = 6)]
    pub extra: Vec<u8>,

    /// Timestamp when this mining tx was created (Unix seconds).
    #[prost(uint64, tag = 7)]
    pub timestamp: u64,
}

impl MiningTx {
    /// Create a new mining transaction.
    ///
    /// # Arguments
    /// * `prev_block_hash` - Hash of the previous block (32 bytes)
    /// * `height` - The block height being mined
    /// * `recipient` - Address to receive the mining reward
    /// * `nonce` - Initial nonce value (will be modified during mining)
    pub fn new(
        prev_block_hash: [u8; 32],
        height: u64,
        recipient: PublicAddress,
        nonce: u64,
    ) -> Self {
        Self {
            prev_block_hash: prev_block_hash.to_vec(),
            height,
            recipient,
            nonce,
            extra: Vec::new(),
            timestamp: 0, // Will be set during mining
        }
    }

    /// Get the block reward amount for this mining transaction.
    ///
    /// The reward is determined solely by the height, following
    /// the emission curve defined in the emission module.
    pub fn reward_amount(&self) -> u64 {
        block_reward(self.height)
    }

    /// Serialize the mining transaction data for hashing.
    ///
    /// This produces the input bytes that will be hashed with RandomX.
    pub fn to_pow_input(&self) -> Vec<u8> {
        // Create a deterministic serialization for PoW hashing
        let mut data = Vec::new();

        // Previous block hash (32 bytes)
        data.extend_from_slice(&self.prev_block_hash);

        // Height (8 bytes, little-endian)
        data.extend_from_slice(&self.height.to_le_bytes());

        // Recipient address (view key + spend key = 64 bytes)
        data.extend_from_slice(self.recipient.view_public_key().as_bytes());
        data.extend_from_slice(self.recipient.spend_public_key().as_bytes());

        // Nonce (8 bytes, little-endian)
        data.extend_from_slice(&self.nonce.to_le_bytes());

        // Timestamp (8 bytes, little-endian)
        data.extend_from_slice(&self.timestamp.to_le_bytes());

        // Extra data (variable, up to 32 bytes)
        data.extend_from_slice(&self.extra);

        data
    }

    /// Get the previous block hash as a fixed-size array.
    pub fn prev_block_hash_array(&self) -> Option<[u8; 32]> {
        if self.prev_block_hash.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&self.prev_block_hash);
            Some(arr)
        } else {
            None
        }
    }

    /// Set the extra data field (max 32 bytes).
    pub fn set_extra(&mut self, extra: &[u8]) {
        self.extra = extra.iter().take(32).copied().collect();
    }

    /// Set the timestamp to the current Unix time.
    #[cfg(feature = "std")]
    pub fn set_timestamp_now(&mut self) {
        use std::time::{SystemTime, UNIX_EPOCH};
        self.timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
    }
}

/// Represents the difficulty target as a 256-bit unsigned integer.
///
/// The difficulty target determines how small the PoW hash must be
/// for the mining transaction to be valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct DifficultyTarget {
    /// The target value - PoW hash must be less than this.
    /// Stored as big-endian bytes for comparison.
    pub target: [u8; 32],
}

impl DifficultyTarget {
    /// Create a new difficulty target from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { target: bytes }
    }

    /// Create a difficulty target from a compact difficulty value.
    ///
    /// The compact difficulty is inverted to get the target:
    /// target = MAX_TARGET / difficulty
    ///
    /// Higher difficulty = lower target = harder to find valid hash.
    pub fn from_difficulty(difficulty: u64) -> Self {
        if difficulty == 0 {
            return Self::max_target();
        }

        // Start with max target (all 0xFF)
        let mut target = [0xFFu8; 32];

        // Simple division: target = MAX / difficulty
        // For now, use a simplified approach where we set leading zeros
        let leading_zeros = 64 - difficulty.leading_zeros() as usize;
        let bytes_to_zero = leading_zeros / 8;
        let remaining_bits = leading_zeros % 8;

        for i in 0..bytes_to_zero.min(32) {
            target[i] = 0;
        }
        if bytes_to_zero < 32 && remaining_bits > 0 {
            target[bytes_to_zero] >>= remaining_bits;
        }

        Self { target }
    }

    /// The maximum (easiest) target - all bits set.
    pub fn max_target() -> Self {
        Self {
            target: [0xFFu8; 32],
        }
    }

    /// The minimum (hardest) target - only last bit set.
    pub fn min_target() -> Self {
        let mut target = [0u8; 32];
        target[31] = 1;
        Self { target }
    }

    /// Check if a hash meets this difficulty target.
    ///
    /// The hash (interpreted as a big-endian 256-bit number) must be
    /// less than the target for the PoW to be valid.
    pub fn hash_meets_target(&self, hash: &[u8; 32]) -> bool {
        // Compare as big-endian 256-bit numbers
        hash < &self.target
    }
}

impl Default for DifficultyTarget {
    fn default() -> Self {
        // Default to a moderate starting difficulty
        Self::from_difficulty(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_account_keys::AccountKey;
    use mc_util_from_random::FromRandom;
    use rand::{rngs::StdRng, SeedableRng};

    fn test_account() -> AccountKey {
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);
        AccountKey::random(&mut rng)
    }

    #[test]
    fn test_mining_tx_creation() {
        let account = test_account();
        let recipient = account.default_subaddress();
        let prev_hash = [0u8; 32];

        let mining_tx = MiningTx::new(prev_hash, 100, recipient, 0);

        assert_eq!(mining_tx.height, 100);
        assert_eq!(mining_tx.nonce, 0);
        assert_eq!(mining_tx.prev_block_hash.len(), 32);
    }

    #[test]
    fn test_reward_amount() {
        let account = test_account();
        let recipient = account.default_subaddress();
        let prev_hash = [0u8; 32];

        let mining_tx = MiningTx::new(prev_hash, 0, recipient, 0);

        // First block should have initial reward
        assert_eq!(mining_tx.reward_amount(), crate::emission::INITIAL_REWARD);
    }

    #[test]
    fn test_pow_input_deterministic() {
        let account = test_account();
        let recipient = account.default_subaddress();
        let prev_hash = [42u8; 32];

        let mut mining_tx = MiningTx::new(prev_hash, 100, recipient.clone(), 12345);
        mining_tx.timestamp = 1700000000;

        let input1 = mining_tx.to_pow_input();
        let input2 = mining_tx.to_pow_input();

        assert_eq!(input1, input2);
    }

    #[test]
    fn test_difficulty_target_comparison() {
        let easy_target = DifficultyTarget::max_target();
        let hard_target = DifficultyTarget::min_target();

        // Easy target: almost any hash should pass
        let hash = [0x80u8; 32]; // Midpoint value
        assert!(easy_target.hash_meets_target(&hash));

        // Hard target: only very small hashes pass
        assert!(!hard_target.hash_meets_target(&hash));

        // Zero hash should always pass
        let zero_hash = [0u8; 32];
        assert!(hard_target.hash_meets_target(&zero_hash));
    }

    #[test]
    fn test_extra_data_limit() {
        let account = test_account();
        let recipient = account.default_subaddress();
        let mut mining_tx = MiningTx::new([0u8; 32], 0, recipient, 0);

        // Set extra data longer than 32 bytes
        let long_extra = [0xABu8; 100];
        mining_tx.set_extra(&long_extra);

        // Should be truncated to 32 bytes
        assert_eq!(mining_tx.extra.len(), 32);
    }
}
