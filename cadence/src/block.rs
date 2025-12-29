use mc_account_keys::PublicAddress;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::transaction::Transaction;

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

/// A mining reward transaction (coinbase) with PoW proof
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MiningTx {
    /// Block height this reward is for
    pub block_height: u64,

    /// Reward amount in picocredits
    pub reward: u64,

    /// Recipient view public key
    pub recipient_view_key: [u8; 32],

    /// Recipient spend public key
    pub recipient_spend_key: [u8; 32],

    /// One-time output public key (for privacy)
    pub output_public_key: [u8; 32],

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
    /// Compute the PoW hash
    pub fn pow_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.recipient_view_key);
        hasher.update(self.recipient_spend_key);
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
        hasher.update(self.recipient_view_key);
        hasher.update(self.recipient_spend_key);
        hasher.update(self.output_public_key);
        hasher.update(self.prev_block_hash);
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.timestamp.to_le_bytes());
        hasher.finalize().into()
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
                recipient_view_key: [0u8; 32],
                recipient_spend_key: [0u8; 32],
                output_public_key: [0u8; 32],
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

    /// Create a new block template for mining with transactions
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
            .unwrap()
            .as_secs();

        let miner_view_key = miner_address.view_public_key().to_bytes();
        let miner_spend_key = miner_address.spend_public_key().to_bytes();

        // Compute transaction root from all transactions
        let tx_root = Self::compute_tx_root(&transactions);

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
            mining_tx: MiningTx {
                block_height: prev_block.height() + 1,
                reward,
                recipient_view_key: miner_view_key,
                recipient_spend_key: miner_spend_key,
                output_public_key: [0u8; 32], // TODO: generate one-time key
                prev_block_hash: prev_hash,
                difficulty,
                nonce: 0,
                timestamp,
            },
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

/// Calculate block reward based on emission schedule
pub fn calculate_block_reward(_height: u64, total_mined: u64) -> u64 {
    // Monero-style emission with tail
    const TOTAL_SUPPLY: u64 = 18_446_744_073_709_551_615; // ~18.4 quintillion picocredits
    const EMISSION_SPEED_FACTOR: u64 = 20;
    const TAIL_EMISSION: u64 = 600_000_000_000; // 0.6 credits per block in picocredits

    let remaining = TOTAL_SUPPLY.saturating_sub(total_mined);
    let main_reward = remaining >> EMISSION_SPEED_FACTOR;

    std::cmp::max(TAIL_EMISSION, main_reward)
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
    fn test_block_reward_tail_emission() {
        // At very high total mined, should get tail emission
        let reward = calculate_block_reward(1_000_000, u64::MAX - 1000);
        assert_eq!(reward, 600_000_000_000);
    }

    #[test]
    fn test_block_reward_early() {
        // Early blocks should get more than tail
        let reward = calculate_block_reward(1, 0);
        assert!(reward > 600_000_000_000);
    }
}
