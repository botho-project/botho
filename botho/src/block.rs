use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::onetime_keys::{create_tx_out_public_key, create_tx_out_target_key};
use bth_transaction_types::{ClusterId, ClusterTagVector, Network};
use bth_util_from_random::FromRandom;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::transaction::{Transaction, TxOutput};

/// Genesis block magic bytes for mainnet (stored in prev_block_hash).
/// ASCII: "BOTHO_MAINNET_GENESIS_V1" padded to 32 bytes
pub const MAINNET_GENESIS_MAGIC: [u8; 32] = [
    0x42, 0x4F, 0x54, 0x48, 0x4F, 0x5F, 0x4D, 0x41, // BOTHO_MA
    0x49, 0x4E, 0x4E, 0x45, 0x54, 0x5F, 0x47, 0x45, // INNET_GE
    0x4E, 0x45, 0x53, 0x49, 0x53, 0x5F, 0x56, 0x31, // NESIS_V1
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
];

/// Genesis block magic bytes for testnet (stored in prev_block_hash).
/// ASCII: "BOTHO_TESTNET_GENESIS_V1" padded to 32 bytes
pub const TESTNET_GENESIS_MAGIC: [u8; 32] = [
    0x42, 0x4F, 0x54, 0x48, 0x4F, 0x5F, 0x54, 0x45, // BOTHO_TE
    0x53, 0x54, 0x4E, 0x45, 0x54, 0x5F, 0x47, 0x45, // STNET_GE
    0x4E, 0x45, 0x53, 0x49, 0x53, 0x5F, 0x56, 0x31, // NESIS_V1
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
];

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

    /// Minting difficulty target
    pub difficulty: u64,

    /// PoW nonce (the minting solution)
    pub nonce: u64,

    /// Minter's view public key
    pub minter_view_key: [u8; 32],

    /// Minter's spend public key
    pub minter_spend_key: [u8; 32],
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
        hasher.update(self.minter_view_key);
        hasher.update(self.minter_spend_key);
        hasher.finalize().into()
    }

    /// Compute the PoW hash (what minters are trying to get below target)
    pub fn pow_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.minter_view_key);
        hasher.update(self.minter_spend_key);
        hasher.finalize().into()
    }

    /// Check if PoW is valid (hash < difficulty target)
    pub fn is_valid_pow(&self) -> bool {
        let hash = self.pow_hash();
        let hash_value = u64::from_be_bytes(hash[0..8].try_into().unwrap());
        hash_value < self.difficulty
    }

    /// Create header for genesis block (defaults to testnet for backward compatibility)
    pub fn genesis() -> Self {
        Self::genesis_for_network(Network::Testnet)
    }

    /// Create header for genesis block for a specific network.
    ///
    /// Each network has a unique genesis block with different magic bytes
    /// in the prev_block_hash field, ensuring chain separation.
    pub fn genesis_for_network(network: Network) -> Self {
        let magic = match network {
            Network::Mainnet => MAINNET_GENESIS_MAGIC,
            Network::Testnet => TESTNET_GENESIS_MAGIC,
        };

        Self {
            version: 1,
            prev_block_hash: magic, // Network-specific magic bytes
            tx_root: [0u8; 32],
            timestamp: 0,
            height: 0,
            difficulty: u64::MAX, // Genesis has no PoW requirement
            nonce: 0,
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
        }
    }

    /// Check if this is a genesis block header by examining the magic bytes.
    pub fn is_genesis(&self) -> bool {
        self.height == 0
            && (self.prev_block_hash == MAINNET_GENESIS_MAGIC
                || self.prev_block_hash == TESTNET_GENESIS_MAGIC
                || self.prev_block_hash == [0u8; 32]) // Legacy genesis
    }

    /// Get the network this genesis header belongs to, if it's a genesis block.
    pub fn genesis_network(&self) -> Option<Network> {
        if self.height != 0 {
            return None;
        }
        if self.prev_block_hash == MAINNET_GENESIS_MAGIC {
            Some(Network::Mainnet)
        } else if self.prev_block_hash == TESTNET_GENESIS_MAGIC {
            Some(Network::Testnet)
        } else if self.prev_block_hash == [0u8; 32] {
            // Legacy genesis defaults to testnet
            Some(Network::Testnet)
        } else {
            None
        }
    }
}

/// A minting transaction (coinbase) that creates new coins via PoW.
///
/// Uses CryptoNote-style stealth addresses for minter privacy:
/// - `target_key`: One-time public key that only the minter can identify and spend
/// - `public_key`: Ephemeral DH public key for minter to derive shared secret
///
/// Even if the same minter wins multiple blocks, their rewards are unlinkable.
///
/// Also includes the minter's public address (view_key, spend_key) for:
/// - PoW binding: The proof of work is tied to the minter's identity
/// - Block header: Required for block construction and verification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MintingTx {
    /// Block height this reward is for
    pub block_height: u64,

    /// Reward amount in picocredits
    pub reward: u64,

    /// Minter's view public key (for PoW binding and block header)
    pub minter_view_key: [u8; 32],

    /// Minter's spend public key (for PoW binding and block header)
    pub minter_spend_key: [u8; 32],

    /// One-time target key: `Hs(r * C) * G + D`
    /// This is the stealth spend public key that only the minter can identify.
    pub target_key: [u8; 32],

    /// Ephemeral public key: `r * D`
    /// Used by minter to derive the shared secret for detecting ownership.
    pub public_key: [u8; 32],

    // PoW proof fields
    /// Previous block hash this minting tx builds on
    pub prev_block_hash: [u8; 32],

    /// Difficulty target at time of minting
    pub difficulty: u64,

    /// PoW nonce (the solution)
    pub nonce: u64,

    /// Timestamp when minted
    pub timestamp: u64,
}

impl MintingTx {
    /// Create a new minting transaction with stealth output for the given minter address.
    pub fn new(
        block_height: u64,
        reward: u64,
        minter_address: &PublicAddress,
        prev_block_hash: [u8; 32],
        difficulty: u64,
        timestamp: u64,
    ) -> Self {
        // Store minter's public address for PoW binding
        let minter_view_key = minter_address.view_public_key().to_bytes();
        let minter_spend_key = minter_address.spend_public_key().to_bytes();

        // Generate random ephemeral key for stealth output
        let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);

        // Create stealth keys for the reward output
        let target_key = create_tx_out_target_key(&tx_private_key, minter_address);
        let public_key =
            create_tx_out_public_key(&tx_private_key, minter_address.spend_public_key());

        Self {
            block_height,
            reward,
            minter_view_key,
            minter_spend_key,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
            prev_block_hash,
            difficulty,
            nonce: 0,
            timestamp,
        }
    }

    /// Compute the PoW hash.
    /// Uses minter keys to match BlockHeader::pow_hash for block validation.
    pub fn pow_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.minter_view_key);
        hasher.update(self.minter_spend_key);
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

    /// Compute the hash of this minting transaction (for consensus)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.block_height.to_le_bytes());
        hasher.update(self.reward.to_le_bytes());
        hasher.update(self.minter_view_key);
        hasher.update(self.minter_spend_key);
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        hasher.update(self.prev_block_hash);
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.timestamp.to_le_bytes());
        hasher.finalize().into()
    }

    /// Convert this minting transaction's output into a TxOutput for ledger storage.
    ///
    /// This allows the ledger to store minting rewards using the same UTXO format
    /// as regular transaction outputs.
    ///
    /// Minting creates a **new cluster origin** - the output is tagged with 100%
    /// attribution to a new cluster derived from the minting tx hash. This is how
    /// coin lineage tracking begins.
    pub fn to_tx_output(&self) -> TxOutput {
        // Create a new cluster ID from the first 8 bytes of the minting tx hash
        let tx_hash = self.hash();
        let cluster_id = ClusterId(u64::from_le_bytes(tx_hash[0..8].try_into().unwrap()));

        TxOutput {
            amount: self.reward,
            target_key: self.target_key,
            public_key: self.public_key,
            e_memo: None, // Minting rewards don't have memos
            cluster_tags: ClusterTagVector::single(cluster_id),
        }
    }
}

/// A complete block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub minting_tx: MintingTx,
    /// Regular transactions included in this block
    pub transactions: Vec<Transaction>,
}

impl Block {
    /// Create the genesis block (defaults to testnet for backward compatibility)
    pub fn genesis() -> Self {
        Self::genesis_for_network(Network::Testnet)
    }

    /// Create the genesis block for a specific network.
    ///
    /// Each network has a unique genesis block with different magic bytes,
    /// ensuring that mainnet and testnet chains are completely separate.
    pub fn genesis_for_network(network: Network) -> Self {
        let genesis_magic = match network {
            Network::Mainnet => MAINNET_GENESIS_MAGIC,
            Network::Testnet => TESTNET_GENESIS_MAGIC,
        };

        Self {
            header: BlockHeader::genesis_for_network(network),
            minting_tx: MintingTx {
                block_height: 0,
                reward: 0,
                // Genesis has no real minter - use zero keys
                minter_view_key: [0u8; 32],
                minter_spend_key: [0u8; 32],
                // Genesis has no stealth output - use zero keys
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                prev_block_hash: genesis_magic, // Network-specific magic
                difficulty: u64::MAX,
                nonce: 0,
                timestamp: 0,
            },
            transactions: Vec::new(),
        }
    }

    /// Check if this is a genesis block.
    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }

    /// Get the network this genesis block belongs to, if it's a genesis block.
    pub fn genesis_network(&self) -> Option<Network> {
        self.header.genesis_network()
    }

    /// Get the block hash
    pub fn hash(&self) -> [u8; 32] {
        self.header.hash()
    }

    /// Get block height
    pub fn height(&self) -> u64 {
        self.header.height
    }

    /// Create a new block template for minting (without transactions)
    pub fn new_template(
        prev_block: &Block,
        minter_address: &PublicAddress,
        difficulty: u64,
        reward: u64,
    ) -> Self {
        Self::new_template_with_txs(prev_block, minter_address, difficulty, reward, Vec::new())
    }

    /// Create a new block template for minting with transactions.
    ///
    /// The minting reward output uses stealth addressing for minter privacy.
    pub fn new_template_with_txs(
        prev_block: &Block,
        minter_address: &PublicAddress,
        difficulty: u64,
        reward: u64,
        transactions: Vec<Transaction>,
    ) -> Self {
        let prev_hash = prev_block.hash();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let minter_view_key = minter_address.view_public_key().to_bytes();
        let minter_spend_key = minter_address.spend_public_key().to_bytes();

        // Compute transaction root from all transactions
        let tx_root = Self::compute_tx_root(&transactions);

        // Create stealth output for minting reward
        let minting_tx = MintingTx::new(
            prev_block.height() + 1,
            reward,
            minter_address,
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
                minter_view_key: minter_view_key,
                minter_spend_key: minter_spend_key,
            },
            minting_tx,
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

/// Dynamic block timing based on network load.
///
/// Adjusts block time to balance:
/// - Overhead efficiency (slower when idle)
/// - Finality latency (faster under load)
///
/// Uses discrete levels for stability and predictability.
pub mod dynamic_timing {
    use super::Block;

    /// Minimum block time (consensus floor - SCP needs time to complete)
    pub const MIN_BLOCK_TIME: u64 = 5;

    /// Maximum block time (efficiency ceiling when idle)
    pub const MAX_BLOCK_TIME: u64 = 40;

    /// Number of recent blocks to analyze for load estimation
    pub const SMOOTHING_WINDOW: usize = 10;

    /// Block metadata overhead in bytes (header + minting_tx)
    pub const BLOCK_METADATA_SIZE: u64 = 476;

    /// Average transaction size estimate (CLSAG 1-in-2-out)
    pub const AVG_TX_SIZE: u64 = 2800;

    /// Discrete block time levels: (tx_rate_threshold, block_time_secs)
    /// Higher load → faster blocks, lower load → slower blocks
    pub const BLOCK_TIME_LEVELS: [(f64, u64); 5] = [
        (20.0, 3),  // Very high load: 20+ tx/s → 3s blocks
        (5.0, 5),   // High load: 5+ tx/s → 5s blocks
        (1.0, 10),  // Medium load: 1+ tx/s → 10s blocks
        (0.2, 20),  // Low load: 0.2+ tx/s → 20s blocks
        (0.0, 40),  // Idle: <0.2 tx/s → 40s blocks
    ];

    /// Compute the target block time based on recent transaction load.
    ///
    /// This is deterministic from chain state, so all validators compute
    /// the same value for a given chain tip.
    ///
    /// # Arguments
    /// * `recent_blocks` - The last SMOOTHING_WINDOW blocks (newest last)
    ///
    /// # Returns
    /// Target block time in seconds
    pub fn compute_block_time(recent_blocks: &[Block]) -> u64 {
        if recent_blocks.len() < 2 {
            // Not enough data, use default
            return 20;
        }

        // Compute total transaction count in the window
        // (We use tx count rather than bytes since we'd need to serialize for exact bytes)

        // Compute time span of the window
        let first_time = recent_blocks.first().map(|b| b.header.timestamp).unwrap_or(0);
        let last_time = recent_blocks.last().map(|b| b.header.timestamp).unwrap_or(0);
        let window_time = last_time.saturating_sub(first_time);

        if window_time == 0 {
            return 20; // Avoid division by zero
        }

        // Compute transaction rate (tx/sec)
        let total_txs: usize = recent_blocks.iter().map(|b| b.transactions.len()).sum();
        let tx_rate = total_txs as f64 / window_time as f64;

        // Find appropriate level
        for (threshold, block_time) in BLOCK_TIME_LEVELS {
            if tx_rate >= threshold {
                return block_time;
            }
        }

        MAX_BLOCK_TIME
    }

    /// Compute the overhead percentage at a given block time and tx rate.
    ///
    /// Returns the percentage of ledger space consumed by block metadata
    /// vs actual transaction data.
    pub fn compute_overhead_percent(block_time: u64, tx_rate: f64) -> f64 {
        let tx_bytes_per_block = tx_rate * block_time as f64 * AVG_TX_SIZE as f64;
        let total_bytes = BLOCK_METADATA_SIZE as f64 + tx_bytes_per_block;

        if total_bytes == 0.0 {
            return 100.0;
        }

        (BLOCK_METADATA_SIZE as f64 / total_bytes) * 100.0
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_block_time_levels() {
            // Verify levels are sorted descending by threshold
            let mut prev_threshold = f64::MAX;
            for (threshold, _) in BLOCK_TIME_LEVELS {
                assert!(threshold < prev_threshold, "Levels must be sorted descending");
                prev_threshold = threshold;
            }
        }

        #[test]
        fn test_overhead_calculation() {
            // At 1 tx/s with 20s blocks: 20 txs per block
            // 20 * 2800 = 56000 bytes of tx data
            // 476 / (476 + 56000) = 0.84% overhead
            let overhead = compute_overhead_percent(20, 1.0);
            assert!(overhead < 1.0, "1 tx/s at 20s blocks should have <1% overhead");

            // At 0.1 tx/s with 20s blocks: 2 txs per block
            // 2 * 2800 = 5600 bytes of tx data
            // 476 / (476 + 5600) = 7.8% overhead
            let overhead = compute_overhead_percent(20, 0.1);
            assert!(overhead > 5.0 && overhead < 10.0, "0.1 tx/s at 20s should be ~8% overhead");
        }
    }
}

/// Difficulty as a monetary policy feedback controller.
///
/// Difficulty is the control variable that adjusts minting rate to hit targets:
///
/// **Phase 1 (Halving, ~10 years)**: High initial rewards to drive adoption
///   - Halving schedule based on cumulative transaction count
///   - Difficulty adjusts to hit target emission per tx-epoch
///
/// **Phase 2 (Tail emission)**: Sustainable 2% net inflation
///   - Net inflation = gross emission - fee burns
///   - Difficulty adjusts to maintain 2% target
///
/// The feedback loop:
/// ```text
///                        error
/// target_emission ──────────┐
///         rate              ▼
///                     ┌───────────┐
/// actual_emission ───>│ PI control│──> difficulty
///         rate        └───────────┘
/// ```
pub mod difficulty {
    use crate::node::minter::INITIAL_DIFFICULTY;

    // --- Legacy constants for backward compatibility ---

    /// Legacy: blocks between adjustments (for old block-based code)
    pub const ADJUSTMENT_WINDOW: u64 = 180;

    /// Legacy: target block time for old adjustment logic
    pub const TARGET_BLOCK_TIME: u64 = 20;

    // --- Core constants ---

    /// Minimum difficulty (floor to prevent stuck chain)
    pub const MIN_DIFFICULTY: u64 = 1;

    /// Maximum difficulty (ceiling)
    pub const MAX_DIFFICULTY: u64 = INITIAL_DIFFICULTY;

    /// Maximum adjustment factor per epoch (damping)
    pub const MAX_ADJUSTMENT_FACTOR: f64 = 2.0;

    /// Transactions per difficulty adjustment epoch.
    /// Adjustment frequency scales with network usage.
    pub const ADJUSTMENT_TX_COUNT: u64 = 1000;

    /// Halving interval in cumulative transactions.
    /// Ties monetary schedule to network adoption, not wall-clock time.
    /// ~10M tx per halving → 5 halvings = ~50M tx for full adoption phase.
    pub const HALVING_TX_INTERVAL: u64 = 10_000_000;

    /// Number of halvings before tail emission
    pub const HALVING_COUNT: u32 = 5;

    /// Target tail inflation (basis points). 200 = 2%
    pub const TAIL_INFLATION_BPS: u64 = 200;

    /// Initial block reward (50 BTH in picocredits)
    pub const INITIAL_REWARD: u64 = 50_000_000_000_000;

    /// Expected transactions per block (for emission rate calc)
    pub const EXPECTED_TX_PER_BLOCK: u64 = 20;

    /// Monetary policy phase
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Phase {
        /// Halving phase with epoch number (0-indexed)
        Halving { epoch: u32 },
        /// Tail emission phase
        Tail,
    }

    /// Emission controller: difficulty as monetary policy.
    ///
    /// Tracks network state and adjusts difficulty to hit emission targets.
    #[derive(Debug, Clone)]
    pub struct EmissionController {
        // --- State ---
        /// Current PoW difficulty
        pub difficulty: u64,
        /// Cumulative transactions (drives halving schedule)
        pub total_tx: u64,
        /// Cumulative gross emission (picocredits minted)
        pub total_emitted: u64,
        /// Cumulative fees burned
        pub total_burned: u64,

        // --- Current epoch accumulators ---
        /// Tx in current adjustment epoch
        pub epoch_tx: u64,
        /// Emission in current epoch
        pub epoch_emission: u64,
        /// Burns in current epoch
        pub epoch_burns: u64,

        // --- Derived ---
        /// Current block reward
        pub current_reward: u64,
    }

    impl Default for EmissionController {
        fn default() -> Self {
            Self::new(INITIAL_DIFFICULTY)
        }
    }

    impl EmissionController {
        pub fn new(initial_difficulty: u64) -> Self {
            Self {
                difficulty: initial_difficulty,
                total_tx: 0,
                total_emitted: 0,
                total_burned: 0,
                epoch_tx: 0,
                epoch_emission: 0,
                epoch_burns: 0,
                current_reward: INITIAL_REWARD,
            }
        }

        /// Restore from persisted chain state
        pub fn from_chain_state(
            difficulty: u64,
            total_mined: u64,
            total_fees_burned: u64,
            total_tx: u64,
            epoch_tx: u64,
            epoch_emission: u64,
            epoch_burns: u64,
            current_reward: u64,
        ) -> Self {
            Self {
                difficulty,
                total_tx,
                total_emitted: total_mined,
                total_burned: total_fees_burned,
                epoch_tx,
                epoch_emission,
                epoch_burns,
                current_reward,
            }
        }

        /// Current monetary phase
        pub fn phase(&self) -> Phase {
            let epoch = (self.total_tx / HALVING_TX_INTERVAL) as u32;
            if epoch < HALVING_COUNT {
                Phase::Halving { epoch }
            } else {
                Phase::Tail
            }
        }

        /// Current block reward
        pub fn block_reward(&self) -> u64 {
            self.current_reward
        }

        /// Net circulating supply
        pub fn net_supply(&self) -> u64 {
            self.total_emitted.saturating_sub(self.total_burned)
        }

        /// Target emission rate (picocredits per tx) for feedback control
        fn target_emission_per_tx(&self) -> u64 {
            match self.phase() {
                Phase::Halving { epoch } => {
                    // Block reward / expected tx per block
                    let halved_reward = INITIAL_REWARD >> epoch;
                    halved_reward / EXPECTED_TX_PER_BLOCK
                }
                Phase::Tail => {
                    // Target: 2% net inflation annually
                    // Assuming ~10M tx/year at maturity
                    // Use u128 to avoid overflow with large supplies
                    let supply = self.net_supply() as u128;
                    let target_annual_net = (supply * TAIL_INFLATION_BPS as u128 / 10_000) as u64;

                    // Gross = net + expected burns
                    // Estimate burn rate from history
                    let burn_per_tx = if self.total_tx > 0 {
                        self.total_burned / self.total_tx
                    } else {
                        0
                    };

                    // Per-tx target = annual / (10M tx/year) + burn_per_tx
                    (target_annual_net / 10_000_000) + burn_per_tx
                }
            }
        }

        /// Record a finalized block and update controller.
        ///
        /// Returns (new_difficulty, new_block_reward)
        pub fn record_block(
            &mut self,
            tx_count: u64,
            reward_paid: u64,
            fees_burned: u64,
        ) -> (u64, u64) {
            // Update totals
            self.total_tx += tx_count;
            self.total_emitted += reward_paid;
            self.total_burned += fees_burned;

            // Update epoch accumulators
            self.epoch_tx += tx_count;
            self.epoch_emission += reward_paid;
            self.epoch_burns += fees_burned;

            // Check halving
            self.update_reward();

            // Adjust difficulty at epoch boundary
            if self.epoch_tx >= ADJUSTMENT_TX_COUNT {
                self.adjust_difficulty();
            }

            (self.difficulty, self.current_reward)
        }

        /// Update block reward based on phase
        fn update_reward(&mut self) {
            match self.phase() {
                Phase::Halving { epoch } => {
                    self.current_reward = INITIAL_REWARD >> epoch;
                }
                Phase::Tail => {
                    // Tail: reward = target annual inflation / expected blocks per year
                    // ~500k blocks/year at 60s blocks (conservative estimate)
                    // Use u128 to avoid overflow with large supplies
                    let supply = self.net_supply() as u128;
                    let annual_target = supply * TAIL_INFLATION_BPS as u128 / 10_000;
                    self.current_reward = ((annual_target / 500_000) as u64).max(1);
                }
            }
        }

        /// Adjust difficulty based on emission rate error
        fn adjust_difficulty(&mut self) {
            if self.epoch_tx == 0 {
                return;
            }

            let target = self.target_emission_per_tx();
            if target == 0 {
                self.reset_epoch();
                return;
            }

            // Actual emission per tx this epoch
            let actual = self.epoch_emission / self.epoch_tx;

            // Error ratio: actual / target
            // > 1: emitting too fast → harder difficulty (lower value)
            // < 1: emitting too slow → easier difficulty (higher value)
            let ratio = actual as f64 / target as f64;

            // Invert for control direction and clamp
            let adjustment = (1.0 / ratio).clamp(
                1.0 / MAX_ADJUSTMENT_FACTOR,
                MAX_ADJUSTMENT_FACTOR,
            );

            let new_diff = (self.difficulty as f64 * adjustment) as u64;
            self.difficulty = new_diff.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY);

            self.reset_epoch();
        }

        fn reset_epoch(&mut self) {
            self.epoch_tx = 0;
            self.epoch_emission = 0;
            self.epoch_burns = 0;
        }

        /// Transactions until next halving (0 if in tail phase)
        pub fn tx_until_halving(&self) -> u64 {
            match self.phase() {
                Phase::Halving { epoch } => {
                    let next = (epoch as u64 + 1) * HALVING_TX_INTERVAL;
                    next.saturating_sub(self.total_tx)
                }
                Phase::Tail => 0,
            }
        }

        /// Estimated current inflation rate (bps)
        pub fn current_inflation_bps(&self) -> u64 {
            let supply = self.net_supply();
            if supply == 0 || self.total_tx == 0 {
                return 0;
            }
            // Net emission per tx, annualized assuming 10M tx/year
            let net_per_tx = self.total_emitted.saturating_sub(self.total_burned)
                / self.total_tx;
            let annual = net_per_tx * 10_000_000;
            (annual * 10_000 / supply) as u64
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_initial_state() {
            let ctrl = EmissionController::new(1000);
            assert_eq!(ctrl.phase(), Phase::Halving { epoch: 0 });
            assert_eq!(ctrl.block_reward(), INITIAL_REWARD);
        }

        #[test]
        fn test_halving_transition() {
            let mut ctrl = EmissionController::new(1000);
            ctrl.total_tx = HALVING_TX_INTERVAL;
            ctrl.update_reward();

            assert_eq!(ctrl.phase(), Phase::Halving { epoch: 1 });
            assert_eq!(ctrl.block_reward(), INITIAL_REWARD / 2);
        }

        #[test]
        fn test_tail_phase() {
            let mut ctrl = EmissionController::new(1000);
            ctrl.total_tx = HALVING_TX_INTERVAL * HALVING_COUNT as u64;
            ctrl.total_emitted = 100_000_000_000_000_000; // 100M BTH
            ctrl.update_reward();

            assert_eq!(ctrl.phase(), Phase::Tail);
            assert!(ctrl.block_reward() > 0);
            assert!(ctrl.block_reward() < INITIAL_REWARD);
        }

        #[test]
        fn test_difficulty_decreases_when_over_emitting() {
            let mut ctrl = EmissionController::new(1000);

            // Emit 2x target per tx
            let target = ctrl.target_emission_per_tx();
            for _ in 0..10 {
                ctrl.record_block(100, target * 200, 0); // 2x emission
            }

            assert!(ctrl.difficulty < 1000, "Should get harder when over-emitting");
        }

        #[test]
        fn test_fee_burn_tracking() {
            let mut ctrl = EmissionController::new(1000);
            ctrl.record_block(10, 1000, 100);

            assert_eq!(ctrl.total_burned, 100);
            assert_eq!(ctrl.net_supply(), 900);
        }
    }

    // --- Legacy functions for backward compatibility ---

    /// Legacy: Calculate difficulty adjustment based on block window.
    ///
    /// This is the old block-time-based adjustment. Prefer `EmissionController`
    /// for new code, which uses tx-count-based monetary policy.
    pub fn calculate_new_difficulty(
        current_difficulty: u64,
        window_start_time: u64,
        window_end_time: u64,
        blocks_in_window: u64,
    ) -> u64 {
        if blocks_in_window == 0 || window_end_time <= window_start_time {
            return current_difficulty;
        }

        let actual_time = window_end_time - window_start_time;
        let expected_time = blocks_in_window * TARGET_BLOCK_TIME;

        let ratio = actual_time as f64 / expected_time as f64;
        let clamped = ratio.clamp(1.0 / MAX_ADJUSTMENT_FACTOR, MAX_ADJUSTMENT_FACTOR);

        let new_difficulty = (current_difficulty as f64 * clamped) as u64;
        new_difficulty.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block() {
        // Default genesis is testnet
        let genesis = Block::genesis();
        assert_eq!(genesis.height(), 0);
        assert_eq!(genesis.header.prev_block_hash, TESTNET_GENESIS_MAGIC);
        assert!(genesis.is_genesis());
        assert_eq!(genesis.genesis_network(), Some(Network::Testnet));
    }

    #[test]
    fn test_genesis_blocks_per_network() {
        let testnet_genesis = Block::genesis_for_network(Network::Testnet);
        let mainnet_genesis = Block::genesis_for_network(Network::Mainnet);

        // Both are genesis blocks
        assert!(testnet_genesis.is_genesis());
        assert!(mainnet_genesis.is_genesis());

        // They have different magic bytes
        assert_eq!(testnet_genesis.header.prev_block_hash, TESTNET_GENESIS_MAGIC);
        assert_eq!(mainnet_genesis.header.prev_block_hash, MAINNET_GENESIS_MAGIC);
        assert_ne!(
            testnet_genesis.header.prev_block_hash,
            mainnet_genesis.header.prev_block_hash
        );

        // They produce different hashes
        assert_ne!(testnet_genesis.hash(), mainnet_genesis.hash());

        // Network detection works
        assert_eq!(testnet_genesis.genesis_network(), Some(Network::Testnet));
        assert_eq!(mainnet_genesis.genesis_network(), Some(Network::Mainnet));
    }

    #[test]
    fn test_genesis_magic_bytes_readable() {
        // Verify the magic bytes are human-readable
        let mainnet_str = std::str::from_utf8(&MAINNET_GENESIS_MAGIC[..24]).unwrap();
        let testnet_str = std::str::from_utf8(&TESTNET_GENESIS_MAGIC[..24]).unwrap();

        assert_eq!(mainnet_str, "BOTHO_MAINNET_GENESIS_V1");
        assert_eq!(testnet_str, "BOTHO_TESTNET_GENESIS_V1");
    }

    #[test]
    fn test_block_hash_deterministic() {
        let genesis = Block::genesis();
        let hash1 = genesis.hash();
        let hash2 = genesis.hash();
        assert_eq!(hash1, hash2);
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
