// Inactive durable infrastructure and local validation; all open constructors
// remain test-only. No network or ordinary Ledger rule selection.
#[allow(dead_code)]
mod experimental;
// Shared LotteryV2 rule module (#1286 P1). ACTIVE since protocol 7.0.0
// (P3, #1640): the production validator and producer use it. Some helpers
// remain experimental-store-only.
#[allow(dead_code)]
mod lottery_v2;
mod snapshot;
mod store;

pub use snapshot::{SnapshotError, SnapshotV2Contents, UtxoSnapshot, UtxoSnapshotV2};
/// Test-only `botho.ledger.2` ledger with V2 lottery payouts (#1286 P7).
#[cfg(test)]
pub(crate) use store::v2::wallet::fixture as wallet_v2_fixture;
pub use store::{
    v2::{DerivationContext, LOTTERY_RULES_VERSION},
    ClusterWealthInfo, EmissionStateUpdate, Ledger, LedgerStorageUsage, TxLocation,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("Unsupported ledger storage schema: {0}")]
    UnsupportedSchema(String),
    #[error("Experimental storage encoding: {0}")]
    StorageEncoding(String),
    #[error("Missing derivation context: {0}")]
    MissingContext(String),
    #[error("Inconsistent experimental record: {0}")]
    InconsistentRecord(String),
    #[error("Database error: {0}")]
    Database(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Block not found: height {0}")]
    BlockNotFound(u64),

    /// The ledger was restored from a V2 snapshot at height `base` (#1646):
    /// it holds genesis and the blocks from `base` upward, but not `height`
    /// (0 < height < base). Distinct from [`LedgerError::BlockNotFound`] so
    /// callers (sync serving, RPC block scans) report "not held here" rather
    /// than treating the height as past the tip.
    #[error(
        "Block {height} is not held: this ledger was restored from a snapshot at height \
         {base} and holds only genesis and blocks from {base} upward"
    )]
    BelowSnapshotBase { height: u64, base: u64 },

    #[error("Invalid block: {0}")]
    InvalidBlock(String),

    /// Not enough age-eligible decoy outputs exist yet to form a ring
    /// signature. This is a cold-start condition on a fresh chain (the decoy
    /// anonymity set has not warmed up), not a bug — it self-heals as outputs
    /// mature. Kept as a distinct typed variant so callers (e.g. the faucet
    /// RPC) can match it precisely and surface a graceful "warming up" response
    /// instead of a scary raw error string.
    #[error("Insufficient decoy candidates: need {required}, have {available}. The ledger needs more confirmed outputs for private transactions.")]
    InsufficientDecoys { required: usize, available: usize },

    #[error("Block already exists at height {0}")]
    BlockExists(u64),

    // Lottery validation errors
    #[error("Invalid lottery fee split: expected pool={expected_pool}, burn={expected_burn}, got pool={actual_pool}, burn={actual_burn}")]
    InvalidLotteryFeeSplit {
        expected_pool: u64,
        expected_burn: u64,
        actual_pool: u64,
        actual_burn: u64,
    },

    #[error("Invalid lottery drawing: verification failed")]
    InvalidLotteryDrawing,

    #[error("Lottery payout mismatch: expected {expected}, got {actual}")]
    LotteryPayoutMismatch { expected: u64, actual: u64 },

    #[error("Lottery output mismatch: expected {expected} outputs, got {actual}")]
    LotteryOutputCountMismatch { expected: usize, actual: usize },

    #[error("Lottery winner not found in candidates: {0}")]
    LotteryWinnerNotEligible(String),

    /// The sum of per-transaction fees in the block overflows `u64`. Fees are
    /// attacker-influenced, so a crafted block whose fees sum past `u64::MAX`
    /// would wrap silently under `overflow-checks=false` (release) or panic in
    /// debug. We reject such blocks deterministically instead. Mirrors the
    /// per-tx balance overflow guard from issue #340.
    #[error("Block fee total overflows u64")]
    FeeOverflow,
}

impl LedgerError {
    /// True when LMDB refused a write because the ledger's map is full
    /// (`MDB_MAP_FULL`, #1535).
    ///
    /// This is a LOCAL storage fault, not a verdict on the block: the refused
    /// write commits nothing, the same block is accepted once the node is
    /// restarted with a larger map, and peers that served the block did
    /// nothing wrong. Callers must not treat it as an invalid block or
    /// penalize the serving peer.
    pub fn is_map_full(&self) -> bool {
        matches!(self, LedgerError::Database(message) if message.contains("MDB_MAP_FULL"))
    }
}

/// Information about the current chain state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainState {
    /// Current block height
    pub height: u64,

    /// Hash of the tip block
    pub tip_hash: [u8; 32],

    /// Timestamp of the tip block
    pub tip_timestamp: u64,

    /// Total credits mined so far (gross emission), in picocredits.
    ///
    /// `u128` (not `u64`): Phase-1 emission (~1.22e21 pico ≈ 1.22B BTH)
    /// exceeds `u64::MAX` (~1.84e19 pico ≈ 18.4M BTH). With
    /// `overflow-checks=false` in release this accumulator would wrap
    /// silently and corrupt the emission schedule (it feeds
    /// `calculate_block_reward`). See issue #333.
    pub total_mined: u128,

    /// Total transaction fees burned (removed from supply), in picocredits.
    /// Net supply = total_mined - total_fees_burned.
    ///
    /// `u128` for the same reason as `total_mined`: cumulative burns track
    /// cumulative emission and can cross `u64::MAX`.
    pub total_fees_burned: u128,

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
