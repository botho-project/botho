mod store;

pub use store::{ClusterWealthInfo, Ledger, TxLocation};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Block not found: height {0}")]
    BlockNotFound(u64),

    #[error("Invalid block: {0}")]
    InvalidBlock(String),

    #[error("Block already exists at height {0}")]
    BlockExists(u64),
}

/// Information about the current chain state
#[derive(Debug, Clone)]
pub struct ChainState {
    /// Current block height
    pub height: u64,

    /// Hash of the tip block
    pub tip_hash: [u8; 32],

    /// Timestamp of the tip block
    pub tip_timestamp: u64,

    /// Total credits mined so far (gross emission)
    pub total_mined: u64,

    /// Total transaction fees burned (removed from supply)
    /// Net supply = total_mined - total_fees_burned
    pub total_fees_burned: u64,

    /// Current minting difficulty
    pub difficulty: u64,

    // --- EmissionController state ---

    /// Total transactions processed (drives halving schedule)
    pub total_tx: u64,

    /// Transactions in current difficulty adjustment epoch
    pub epoch_tx: u64,

    /// Emission in current epoch (for rate calculation)
    pub epoch_emission: u64,

    /// Burns in current epoch
    pub epoch_burns: u64,

    /// Current block reward
    pub current_reward: u64,
}

impl Default for ChainState {
    fn default() -> Self {
        use crate::block::difficulty::INITIAL_REWARD;
        Self {
            height: 0,
            tip_hash: [0u8; 32],
            tip_timestamp: 0,
            total_mined: 0,
            total_fees_burned: 0,
            difficulty: super::node::minter::INITIAL_DIFFICULTY,
            total_tx: 0,
            epoch_tx: 0,
            epoch_emission: 0,
            epoch_burns: 0,
            current_reward: INITIAL_REWARD,
        }
    }
}
