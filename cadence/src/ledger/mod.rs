mod store;

pub use store::Ledger;

use crate::block::Block;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("Database error: {0}")]
    Database(#[from] lmdb::Error),

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

    /// Total credits mined so far
    pub total_mined: u64,

    /// Current mining difficulty
    pub difficulty: u64,
}

impl Default for ChainState {
    fn default() -> Self {
        Self {
            height: 0,
            tip_hash: [0u8; 32],
            total_mined: 0,
            difficulty: super::node::miner::INITIAL_DIFFICULTY,
        }
    }
}
