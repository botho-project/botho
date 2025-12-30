use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::onetime_keys::{create_tx_out_public_key, create_tx_out_target_key};
use bth_util_from_random::FromRandom;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::transaction::{Transaction, TxOutput};

/// Block header containing PoW fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Block version
    pub version: u32,

    /// Hash of the previous block (32 bytes)
    pub prev_block_hash: [u8; 32],

    /// Merkle root of transactions (32 bytes)
    pub tx_root: [u8; 32],

    /// Block timestamp (unix seconds)
    pub timestamp: u64,

    /// Block height
    pub height: u64,

    /// Mining difficulty target
    pub difficulty: u64,

    /// PoW nonce (the mining solution)
    pub nonce: u64,

    /// Miner's view public key
    pub miner_view_key: [u8; 32],

    /// Miner's spend public key
    pub miner_spend_key: [u8; 32],
}

impl BlockHeader {
    /// Compute the hash of this block header
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.version.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.tx_root);
        hasher.update(self.timestamp.to_le_bytes());
        hasher.update(self.height.to_le_bytes());
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.miner_view_key);
        hasher.update(self.miner_spend_key);
        hasher.finalize().into()
    }

    /// Compute the PoW hash (what miners are trying to get below target)
    pub fn pow_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.miner_view_key);
        hasher.update(self.miner_spend_key);
        hasher.finalize().into()
    }

    /// Check if PoW is valid (hash < difficulty target)
    pub fn is_valid_pow(&self) -> bool {
        let hash = self.pow_hash();
        let hash_value = u64::from_be_bytes(hash[0..8].try_into().unwrap());
        hash_value < self.difficulty
    }

    /// Create header for genesis block
    pub fn genesis() -> Self {
        Self {
            version: 1,
            prev_block_hash: [0u8; 32],
            tx_root: [0u8; 32],
            timestamp: 0,
            height: 0,
            difficulty: u64::MAX, // Genesis has no PoW requirement
            nonce: 0,
            miner_view_key: [0u8; 32],
            miner_spend_key: [0u8; 32],
        }
    }
}

/// A mining reward transaction (coinbase) with PoW proof and stealth addressing.
///
/// Uses CryptoNote-style stealth addresses for miner privacy:
/// - `target_key`: One-time public key that only the miner can identify and spend
/// - `public_key`: Ephemeral DH public key for miner to derive shared secret
///
/// Even if the same miner wins multiple blocks, their rewards are unlinkable.
///
/// Also includes the miner's public address (view_key, spend_key) for:
/// - PoW binding: The proof of work is tied to the miner's identity
/// - Block header: Required for block construction and verification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MiningTx {
    /// Block height this reward is for
    pub block_height: u64,

    /// Reward amount in picocredits
    pub reward: u64,

    /// Miner's view public key (for PoW binding and block header)
    pub miner_view_key: [u8; 32],

    /// Miner's spend public key (for PoW binding and block header)
    pub miner_spend_key: [u8; 32],

    /// One-time target key: `Hs(r * C) * G + D`
    /// This is the stealth spend public key that only the miner can identify.
    pub target_key: [u8; 32],

    /// Ephemeral public key: `r * D`
    /// Used by miner to derive the shared secret for detecting ownership.
    pub public_key: [u8; 32],

    // PoW proof fields
    /// Previous block hash this mining tx builds on
    pub prev_block_hash: [u8; 32],

    /// Difficulty target at time of mining
    pub difficulty: u64,

    /// PoW nonce (the solution)
    pub nonce: u64,

    /// Timestamp when mined
    pub timestamp: u64,
}

impl MiningTx {
    /// Create a new mining transaction with stealth output for the given miner address.
    pub fn new(
        block_height: u64,
        reward: u64,
        miner_address: &PublicAddress,
        prev_block_hash: [u8; 32],
        difficulty: u64,
        timestamp: u64,
    ) -> Self {
        // Store miner's public address for PoW binding
        let miner_view_key = miner_address.view_public_key().to_bytes();
        let miner_spend_key = miner_address.spend_public_key().to_bytes();

        // Generate random ephemeral key for stealth output
        let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);

        // Create stealth keys for the reward output
        let target_key = create_tx_out_target_key(&tx_private_key, miner_address);
        let public_key =
            create_tx_out_public_key(&tx_private_key, miner_address.spend_public_key());

        Self {
            block_height,
            reward,
            miner_view_key,
            miner_spend_key,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
            prev_block_hash,
            difficulty,
            nonce: 0,
            timestamp,
        }
    }

    /// Compute the PoW hash.
    /// Uses stealth keys (target_key, public_key) to bind PoW to the specific output.
    pub fn pow_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        hasher.finalize().into()
    }

    /// Verify the PoW is valid
    pub fn verify_pow(&self) -> bool {
        let hash = self.pow_hash();
        let hash_value = u64::from_be_bytes(hash[0..8].try_into().unwrap());
        hash_value < self.difficulty
    }

    /// Get the PoW hash value as u64 (lower = better, used for priority in consensus)
    pub fn pow_priority(&self) -> u64 {
        let hash = self.pow_hash();
        // Invert so that lower hash = higher priority
        u64::MAX - u64::from_be_bytes(hash[0..8].try_into().unwrap())
    }

    /// Compute the hash of this mining transaction (for consensus)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.block_height.to_le_bytes());
        hasher.update(self.reward.to_le_bytes());
        hasher.update(self.miner_view_key);
        hasher.update(self.miner_spend_key);
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        hasher.update(self.prev_block_hash);
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.timestamp.to_le_bytes());
        hasher.finalize().into()
    }

    /// Convert this mining transaction's output into a TxOutput for ledger storage.
    ///
    /// This allows the ledger to store mining rewards using the same UTXO format
    /// as regular transaction outputs.
    pub fn to_tx_output(&self) -> TxOutput {
        TxOutput {
            amount: self.reward,
            target_key: self.target_key,
            public_key: self.public_key,
            e_memo: None, // Mining rewards don't have memos
        }
    }
}

/// A complete block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub mining_tx: MiningTx,
    /// Regular transactions included in this block
    pub transactions: Vec<Transaction>,
}

impl Block {
    /// Create the genesis block
    pub fn genesis() -> Self {
        Self {
            header: BlockHeader::genesis(),
            mining_tx: MiningTx {
                block_height: 0,
                reward: 0,
                // Genesis has no real miner - use zero keys
                miner_view_key: [0u8; 32],
                miner_spend_key: [0u8; 32],
                // Genesis has no stealth output - use zero keys
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                prev_block_hash: [0u8; 32],
                difficulty: u64::MAX,
                nonce: 0,
                timestamp: 0,
            },
            transactions: Vec::new(),
        }
    }

    /// Get the block hash
    pub fn hash(&self) -> [u8; 32] {
        self.header.hash()
    }

    /// Get block height
    pub fn height(&self) -> u64 {
        self.header.height
    }

    /// Create a new block template for mining (without transactions)
    pub fn new_template(
        prev_block: &Block,
        miner_address: &PublicAddress,
        difficulty: u64,
        reward: u64,
    ) -> Self {
        Self::new_template_with_txs(prev_block, miner_address, difficulty, reward, Vec::new())
    }

    /// Create a new block template for mining with transactions.
    ///
    /// The mining reward output uses stealth addressing for miner privacy.
    pub fn new_template_with_txs(
        prev_block: &Block,
        miner_address: &PublicAddress,
        difficulty: u64,
        reward: u64,
        transactions: Vec<Transaction>,
    ) -> Self {
        let prev_hash = prev_block.hash();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let miner_view_key = miner_address.view_public_key().to_bytes();
        let miner_spend_key = miner_address.spend_public_key().to_bytes();

        // Compute transaction root from all transactions
        let tx_root = Self::compute_tx_root(&transactions);

        // Create stealth output for mining reward
        let mining_tx = MiningTx::new(
            prev_block.height() + 1,
            reward,
            miner_address,
            prev_hash,
            difficulty,
            timestamp,
        );

        Self {
            header: BlockHeader {
                version: 1,
                prev_block_hash: prev_hash,
                tx_root,
                timestamp,
                height: prev_block.height() + 1,
                difficulty,
                nonce: 0,
                miner_view_key,
                miner_spend_key,
            },
            mining_tx,
            transactions,
        }
    }

    /// Compute merkle root of transactions
    fn compute_tx_root(transactions: &[Transaction]) -> [u8; 32] {
        if transactions.is_empty() {
            return [0u8; 32];
        }

        let mut hasher = Sha256::new();
        for tx in transactions {
            hasher.update(tx.hash());
        }
        hasher.finalize().into()
    }

    /// Get total fees from all transactions
    pub fn total_fees(&self) -> u64 {
        self.transactions.iter().map(|tx| tx.fee).sum()
    }
}

/// Calculate block reward using the Two-Phase Monetary Model.
///
/// This is a convenience function for code that doesn't have access to a
/// `MonetarySystem` instance. For stateful monetary policy (with difficulty
/// adjustment and fee burn tracking), use `MonetarySystem` directly.
///
/// # Arguments
/// * `height` - Current block height
/// * `total_supply` - Current total supply (for tail emission calculation)
///
/// # Returns
/// The block reward for the given height.
pub fn calculate_block_reward_v2(height: u64, total_supply: u64) -> u64 {
    use bth_cluster_tax::MonetaryPolicy;

    let policy = crate::monetary::mainnet_policy();

    // Check which phase we're in
    if policy.is_halving_phase(height) {
        // Phase 1: Halving schedule
        policy.halving_reward(height).unwrap_or(1)
    } else {
        // Phase 2: Calculate tail reward based on supply
        policy.calculate_tail_reward(total_supply)
    }
}

/// Difficulty adjustment constants
pub mod difficulty {
    use crate::node::miner::INITIAL_DIFFICULTY;

    /// Target block time in seconds
    pub const TARGET_BLOCK_TIME: u64 = 60;

    /// Number of blocks in adjustment window
    pub const ADJUSTMENT_WINDOW: u64 = 10;

    /// Maximum adjustment factor (prevent huge jumps)
    pub const MAX_ADJUSTMENT_FACTOR: f64 = 4.0;

    /// Minimum difficulty (to prevent getting stuck)
    pub const MIN_DIFFICULTY: u64 = 1;

    /// Maximum difficulty
    pub const MAX_DIFFICULTY: u64 = INITIAL_DIFFICULTY;

    /// Calculate new difficulty based on recent block times
    ///
    /// Arguments:
    /// - `current_difficulty`: Current difficulty target
    /// - `window_start_time`: Timestamp of first block in window
    /// - `window_end_time`: Timestamp of last block in window
    /// - `blocks_in_window`: Number of blocks in the window
    ///
    /// Returns the new difficulty target
    pub fn calculate_new_difficulty(
        current_difficulty: u64,
        window_start_time: u64,
        window_end_time: u64,
        blocks_in_window: u64,
    ) -> u64 {
        if blocks_in_window == 0 || window_end_time <= window_start_time {
            return current_difficulty;
        }

        // Calculate actual time taken
        let actual_time = window_end_time - window_start_time;

        // Calculate expected time
        let expected_time = blocks_in_window * TARGET_BLOCK_TIME;

        // Calculate adjustment ratio
        // If blocks are too fast (actual_time < expected_time), decrease difficulty (make it harder)
        // If blocks are too slow (actual_time > expected_time), increase difficulty (make it easier)
        // Note: Lower difficulty value = harder to find a hash below it
        let ratio = actual_time as f64 / expected_time as f64;

        // Clamp adjustment factor
        let clamped_ratio = ratio.clamp(1.0 / MAX_ADJUSTMENT_FACTOR, MAX_ADJUSTMENT_FACTOR);

        // Calculate new difficulty
        // Higher ratio (slower blocks) = multiply difficulty by ratio (make easier)
        // Lower ratio (faster blocks) = multiply difficulty by ratio (make harder)
        let new_difficulty = (current_difficulty as f64 * clamped_ratio) as u64;

        // Clamp to valid range
        new_difficulty.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block() {
        let genesis = Block::genesis();
        assert_eq!(genesis.height(), 0);
        assert_eq!(genesis.header.prev_block_hash, [0u8; 32]);
    }

    #[test]
    fn test_block_hash_deterministic() {
        let genesis = Block::genesis();
        let hash1 = genesis.hash();
        let hash2 = genesis.hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    #[allow(deprecated)]
    fn test_block_reward_tail_emission_legacy() {
        // Legacy Monero-style: At very high total mined, should get tail emission
        let reward = calculate_block_reward(1_000_000, u64::MAX - 1000);
        assert_eq!(reward, 600_000_000_000);
    }

    #[test]
    #[allow(deprecated)]
    fn test_block_reward_early_legacy() {
        // Legacy Monero-style: Early blocks should get more than tail
        let reward = calculate_block_reward(1, 0);
        assert!(reward > 600_000_000_000);
    }

    #[test]
    fn test_block_reward_v2_halving() {
        // Two-Phase model: First halving period
        let policy = crate::monetary::mainnet_policy();
        let reward = calculate_block_reward_v2(0, 0);
        assert_eq!(reward, policy.initial_reward);

        // After first halving
        let reward_after = calculate_block_reward_v2(policy.halving_interval, 0);
        assert_eq!(reward_after, policy.initial_reward / 2);
    }

    #[test]
    fn test_block_reward_v2_tail() {
        // Two-Phase model: Tail emission phase
        let policy = crate::monetary::mainnet_policy();
        let tail_start = policy.tail_emission_start_height();
        let supply = 100_000_000_000_000_000u64; // 100M BTH in picocredits

        let reward = calculate_block_reward_v2(tail_start + 100, supply);

        // Should be based on supply and inflation target
        assert!(reward > 0);
        // Tail reward should be much smaller than initial
        assert!(reward < policy.initial_reward);
    }
}
