// Copyright (c) 2024 Botho Foundation

//! Transaction mempool for storing pending transactions.
//!
//! All transactions are private by default using CLSAG ring signatures.
//! Tracks spent key images to prevent double-spending.
//!
//! ## Fee Validation
//!
//! Uses the cluster-tax fee system to compute minimum fees based on:
//! - Transaction type (Hidden for private, Minting for block rewards)
//! - Transfer amount
//! - Sender's cluster wealth (see note below)
//! - Number of outputs with encrypted memos
//!
//! ## Cluster Wealth Tracking
//!
//! The progressive fee system charges higher fees to wealthier clusters (1x-6x
//! multiplier). Cluster wealth is computed from transaction outputs, which
//! inherit merged+decayed tags from inputs. This means:
//!
//! - Fresh mints start with weight=100% for a new cluster ID
//! - Each transaction decays weights by 5% (DEFAULT_CLUSTER_DECAY_RATE)
//! - Mixed inputs produce merged tag vectors weighted by amount
//! - Maximum cluster wealth determines the fee multiplier
//!
//! For fee estimation (wallets), use `Wallet::compute_cluster_wealth()` on
//! UTXOs. For fee validation (mempool), cluster wealth is computed from output
//! tags.

use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

use crate::{
    ledger::Ledger,
    transaction::{Transaction, TxOutput, UtxoId},
};
use bth_cluster_tax::{
    DynamicFeeBase, DynamicFeeState, FeeConfig, FeeSuggestion,
    TransactionType as FeeTransactionType,
};
use bth_transaction_types::TAG_WEIGHT_SCALE;

/// Compute the effective cluster wealth for a transaction using the ledger's
/// global per-cluster wealth tracker.
///
/// Outputs inherit merged+decayed tags from inputs, so the output tag weights
/// describe which clusters the sender's coins are attributed to. The fee rate
/// must be based on the GLOBAL wealth of those clusters (tracked by the
/// ledger across the whole UTXO set), not on this transaction's own value —
/// otherwise a wealthy cluster could split funds into small transactions and
/// pay the minimum rate (Sybil/split evasion).
///
/// effective_wealth = Σ_outputs Σ_tags (value × weight / SCALE × W_global) /
/// Σ_outputs value
///
/// Background (untagged) value contributes zero, matching
/// `bth_transaction_core::validation::compute_effective_cluster_wealth`.
fn effective_cluster_wealth_from_outputs(
    outputs: &[TxOutput],
    ledger: &Ledger,
) -> Result<u128, String> {
    let mut total_weighted_wealth: u128 = 0;
    let mut total_value: u128 = 0;

    for output in outputs {
        total_value = total_value.saturating_add(output.amount as u128);
        for entry in &output.cluster_tags.entries {
            let value_fraction =
                (output.amount as u128 * entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
            // Propagate DB errors instead of `unwrap_or(0)`. Defaulting a DB
            // failure to zero wealth would lower the progressive fee (fail-open,
            // audit cycle 6, M7), letting a wealthy cluster underpay on a
            // transient read error. Surfacing the error fails CLOSED: the tx is
            // rejected from the mempool with a LedgerError rather than admitted
            // at a fee computed from bogus zero wealth. The happy path is
            // unchanged.
            // `get_cluster_wealth` is u128 (16-byte accumulator, #626 PR2); the
            // fee API now takes u128 end-to-end (#626 PR3), so the u64 clamp PR2
            // added here is gone — full-width wealth flows straight into
            // `FeeConfig::cluster_factor`. `value_fraction` (≤ output.amount ≤
            // u64::MAX) × `global_wealth` (u128) and the running sum can in
            // principle exceed u128 only at astronomically-distant cumulative
            // wealth (tens of millions of BTH per member × full-supply values);
            // saturating arithmetic pins that to u128::MAX → factor 6000 (max),
            // the conservative relay direction. Deterministic; relay policy only.
            let global_wealth = ledger
                .get_cluster_wealth(entry.cluster_id.0)
                .map_err(|e| format!("get_cluster_wealth({}): {}", entry.cluster_id.0, e))?;
            total_weighted_wealth =
                total_weighted_wealth.saturating_add(value_fraction.saturating_mul(global_wealth));
        }
    }

    if total_value == 0 {
        return Ok(0);
    }

    Ok(total_weighted_wealth / total_value)
}

// ============================================================================
// Ring Tag Plausibility Validation
// ============================================================================
//
// Prevents cluster tag manipulation attacks where a malicious wallet
// deliberately selects decoys with much lower cluster tag weights than the real
// input to evade progressive fees or fingerprint transactions.
//
// The solution uses centroid-based validation: output tags must have sufficient
// similarity to the value-weighted centroid of ring member tags.

/// Block height after which ring tag plausibility is enforced.
/// Set to 0 to enforce from genesis, or higher to allow network bootstrapping.
pub const RING_TAG_VALIDATION_ACTIVATION_HEIGHT: u64 = 10_000;

/// Minimum UTXO pool size for strict enforcement.
/// Below this threshold, validation is relaxed to allow bootstrapping.
pub const SPARSE_POOL_THRESHOLD: usize = 50_000;

/// Minimum cosine similarity between output tags and ring centroid.
/// 0.7 means 70% similarity required.
pub const RING_TAG_SIMILARITY_THRESHOLD: f64 = 0.7;

/// Compute the value-weighted centroid of cluster tags from ring members.
///
/// Each ring member contributes to the centroid proportionally to its value.
/// The result is a normalized tag vector representing the "average" cluster
/// profile of the ring.
///
/// # Arguments
/// * `ring_tags` - List of (ClusterTagVector, value) pairs for each ring member
///
/// # Returns
/// A ClusterTagVector representing the weighted centroid
fn compute_ring_centroid(
    ring_tags: &[(bth_transaction_types::ClusterTagVector, u64)],
) -> bth_transaction_types::ClusterTagVector {
    use bth_transaction_types::{ClusterId, ClusterTagVector};

    let total_value: u64 = ring_tags.iter().map(|(_, v)| *v).sum();
    if total_value == 0 {
        return ClusterTagVector::empty();
    }

    // Accumulate value-weighted cluster masses
    let mut cluster_masses: HashMap<u64, u128> = HashMap::new();

    for (tags, value) in ring_tags {
        for entry in &tags.entries {
            // Mass contribution = value * weight / TAG_WEIGHT_SCALE
            let mass = (*value as u128) * (entry.weight as u128);
            *cluster_masses.entry(entry.cluster_id.0).or_default() += mass;
        }
    }

    // Convert masses back to normalized weights relative to total_value
    let pairs: Vec<(ClusterId, u32)> = cluster_masses
        .into_iter()
        .map(|(cluster_id, mass)| {
            // weight = mass / total_value (already normalized by TAG_WEIGHT_SCALE from
            // original weights)
            let weight = (mass / (total_value as u128)) as u32;
            (ClusterId(cluster_id), weight)
        })
        .collect();

    ClusterTagVector::from_pairs(&pairs)
}

/// Compute cosine similarity between two cluster tag vectors.
///
/// Returns a value between 0.0 (completely different) and 1.0 (identical).
/// Empty vectors are considered maximally similar to any vector (returns 1.0),
/// which handles the bootstrapping case of heavily diffused coins.
fn cosine_similarity(
    a: &bth_transaction_types::ClusterTagVector,
    b: &bth_transaction_types::ClusterTagVector,
) -> f64 {
    // If both are empty, they're identical
    if a.entries.is_empty() && b.entries.is_empty() {
        return 1.0;
    }

    // If one is empty (fully diffused), it's maximally similar to anything
    // This handles the case of heavily circulated coins
    if a.entries.is_empty() || b.entries.is_empty() {
        return 1.0;
    }

    // Build weight maps for efficient lookup
    let a_weights: HashMap<u64, u32> = a
        .entries
        .iter()
        .map(|e| (e.cluster_id.0, e.weight))
        .collect();
    let b_weights: HashMap<u64, u32> = b
        .entries
        .iter()
        .map(|e| (e.cluster_id.0, e.weight))
        .collect();

    // Collect all cluster IDs
    let all_clusters: std::collections::HashSet<u64> =
        a_weights.keys().chain(b_weights.keys()).copied().collect();

    // Compute dot product and magnitudes
    let mut dot_product: f64 = 0.0;
    let mut mag_a: f64 = 0.0;
    let mut mag_b: f64 = 0.0;

    for cluster in all_clusters {
        let w1 = *a_weights.get(&cluster).unwrap_or(&0) as f64;
        let w2 = *b_weights.get(&cluster).unwrap_or(&0) as f64;

        dot_product += w1 * w2;
        mag_a += w1 * w1;
        mag_b += w2 * w2;
    }

    let magnitude = (mag_a.sqrt() * mag_b.sqrt()).max(1.0);
    (dot_product / magnitude).clamp(0.0, 1.0)
}

/// Validate that a ring's composition is plausible for the claimed output tags.
///
/// The output tags must have sufficient similarity to the ring's weighted
/// centroid. This prevents selecting extreme outlier decoys to manipulate
/// apparent cluster wealth.
///
/// # Arguments
/// * `ring_tags` - List of (ClusterTagVector, value) pairs for each ring member
/// * `output_tags` - The cluster tags claimed for the transaction outputs
/// * `threshold` - Minimum similarity required (recommend 0.7)
/// * `current_height` - Current block height (for activation check)
/// * `utxo_pool_size` - Current UTXO pool size (for sparse pool bypass)
///
/// # Returns
/// * `Ok(())` if the ring composition is plausible
/// * `Err((similarity_permille, threshold_permille))` if validation fails
///   (values in parts per 1000)
pub fn validate_ring_tag_plausibility(
    ring_tags: &[(bth_transaction_types::ClusterTagVector, u64)],
    output_tags: &bth_transaction_types::ClusterTagVector,
    threshold: f64,
    current_height: u64,
    utxo_pool_size: usize,
) -> Result<(), (u32, u32)> {
    // Skip validation before activation height
    if current_height < RING_TAG_VALIDATION_ACTIVATION_HEIGHT {
        return Ok(());
    }

    // Compute value-weighted centroid of ring member tags
    let centroid = compute_ring_centroid(ring_tags);

    // Output must be plausible from the centroid (with decay tolerance)
    let similarity = cosine_similarity(&centroid, output_tags);

    if similarity < threshold {
        // Allow bypass for sparse pool (bootstrapping)
        if utxo_pool_size < SPARSE_POOL_THRESHOLD {
            warn!(
                "Relaxed ring tag consistency due to sparse UTXO pool ({} < {}): similarity={:.3}",
                utxo_pool_size, SPARSE_POOL_THRESHOLD, similarity
            );
            return Ok(());
        }
        // Convert to permille for error return
        let similarity_permille = (similarity * 1000.0) as u32;
        let threshold_permille = (threshold * 1000.0) as u32;
        return Err((similarity_permille, threshold_permille));
    }

    Ok(())
}

/// Maximum transactions in mempool.
///
/// Increased from 1000 to 10000 to support higher transaction throughput.
/// Memory impact: ~50MB at full capacity (CLSAG transactions ~5KB each).
/// See docs/memory-budget.md for detailed memory planning.
const MAX_MEMPOOL_SIZE: usize = 10_000;

/// Maximum age of a transaction in seconds before eviction
const MAX_TX_AGE_SECS: u64 = 3600; // 1 hour

/// Maximum age of a key image in the spent_key_images set before cleanup.
/// This should be longer than MAX_TX_AGE_SECS to ensure we don't clean up
/// key images while their transactions are still in consensus pending_values.
/// Set to 2x MAX_TX_AGE_SECS (2 hours).
const MAX_KEY_IMAGE_AGE_SECS: u64 = MAX_TX_AGE_SECS * 2;

/// A pending transaction with metadata
#[derive(Debug, Clone)]
pub struct PendingTx {
    pub tx: Transaction,
    pub received_at: std::time::Instant,
    pub fee_per_byte: u64,
    /// Fee density accounting for cluster factor: fee / (size ×
    /// cluster_factor). Higher values get priority. This ensures wealthy
    /// clusters (high factor) must pay more to achieve the same priority as
    /// smaller clusters. Stored as scaled integer (×1000) to avoid floating
    /// point.
    pub fee_density: u64,
    /// The cluster wealth used for fee calculation (u128 pico, #626 PR3).
    pub cluster_wealth: u128,
}

impl PendingTx {
    /// Create a new pending transaction.
    ///
    /// Uses `tx.estimate_size()` instead of serialization to avoid
    /// unnecessary heap allocation when computing fee per byte.
    /// This is critical for memory efficiency with 10K+ mempool capacity.
    pub fn new(tx: Transaction) -> Self {
        // Use estimate_size() which computes size from structure without allocation,
        // instead of bincode::serialize() which allocates the full transaction.
        let tx_size = tx.estimate_size().max(1);
        let fee_per_byte = tx.fee / tx_size as u64;
        Self {
            tx,
            received_at: std::time::Instant::now(),
            fee_per_byte,
            fee_density: fee_per_byte, // Default without cluster factor
            cluster_wealth: 0,
        }
    }

    /// Create a new pending transaction with cluster factor adjustment.
    ///
    /// Fee density = fee / (size × cluster_factor / 1000)
    /// The cluster_factor is in 1000-scale (1000 = 1x, 6000 = 6x), so we
    /// divide by cluster_factor and multiply by 1000 to normalize.
    pub fn with_cluster_factor(tx: Transaction, cluster_wealth: u128, cluster_factor: u64) -> Self {
        let tx_size = tx.estimate_size().max(1);
        let fee_per_byte = tx.fee / tx_size as u64;

        // fee_density = (fee × 1000) / (size × cluster_factor)
        // This gives priority inversely proportional to cluster factor
        let fee_density = if cluster_factor > 0 {
            (tx.fee as u128 * 1000 / (tx_size as u128 * cluster_factor as u128)) as u64
        } else {
            fee_per_byte
        };

        Self {
            tx,
            received_at: std::time::Instant::now(),
            fee_per_byte,
            fee_density,
            cluster_wealth,
        }
    }
}

/// Metrics for mempool fee enforcement.
///
/// Tracks rejection statistics for monitoring and debugging.
#[derive(Debug, Clone, Default)]
pub struct MempoolFeeMetrics {
    /// Total transactions rejected due to insufficient fee
    pub fee_rejections: u64,
    /// Total fee shortfall across all rejections (sum of minimum - provided)
    pub total_fee_shortfall: u64,
    /// Highest fee shortfall seen in a single rejection
    pub max_fee_shortfall: u64,
}

/// Transaction mempool
pub struct Mempool {
    /// Pending transactions by hash
    txs: HashMap<[u8; 32], PendingTx>,
    /// Spent key images with timestamps (for double-spend prevention).
    /// Maps key image -> time when first seen.
    /// Key images are kept even after transaction eviction to prevent race
    /// conditions with consensus pending_values. Cleaned up after
    /// MAX_KEY_IMAGE_AGE_SECS.
    spent_key_images: HashMap<[u8; 32], std::time::Instant>,
    /// Fee configuration for computing minimum fees
    fee_config: FeeConfig,
    /// Dynamic fee base for congestion control
    dynamic_fee: DynamicFeeBase,
    /// Whether we're at minimum block time (triggers dynamic fee adjustment)
    at_min_block_time: bool,
    /// Whether to enforce minimum fee requirements (can be disabled for
    /// testnet)
    enforce_minimum_fee: bool,
    /// Metrics tracking for fee enforcement
    fee_metrics: MempoolFeeMetrics,
}

impl Mempool {
    /// Create a new empty mempool with default fee configuration
    pub fn new() -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashMap::new(),
            fee_config: FeeConfig::default(),
            dynamic_fee: DynamicFeeBase::default(),
            at_min_block_time: false,
            enforce_minimum_fee: true,
            fee_metrics: MempoolFeeMetrics::default(),
        }
    }

    /// Create a new empty mempool with custom fee configuration
    pub fn with_fee_config(fee_config: FeeConfig) -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashMap::new(),
            fee_config,
            dynamic_fee: DynamicFeeBase::default(),
            at_min_block_time: false,
            enforce_minimum_fee: true,
            fee_metrics: MempoolFeeMetrics::default(),
        }
    }

    /// Create a new empty mempool with custom fee and dynamic fee configuration
    pub fn with_dynamic_fee(fee_config: FeeConfig, dynamic_fee: DynamicFeeBase) -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashMap::new(),
            fee_config,
            dynamic_fee,
            at_min_block_time: false,
            enforce_minimum_fee: true,
            fee_metrics: MempoolFeeMetrics::default(),
        }
    }

    /// Create a new mempool with fee enforcement disabled (for testnet)
    ///
    /// When fee enforcement is disabled, transactions with insufficient fees
    /// will still be accepted but logged as warnings. This is useful for
    /// testnet environments where fee requirements may be relaxed.
    pub fn with_fee_enforcement_disabled(fee_config: FeeConfig) -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashMap::new(),
            fee_config,
            dynamic_fee: DynamicFeeBase::default(),
            at_min_block_time: false,
            enforce_minimum_fee: false,
            fee_metrics: MempoolFeeMetrics::default(),
        }
    }

    /// Set whether minimum fee enforcement is enabled
    ///
    /// When disabled, transactions with insufficient fees will be accepted
    /// with a warning. Useful for testnet environments.
    pub fn set_fee_enforcement(&mut self, enforce: bool) {
        self.enforce_minimum_fee = enforce;
    }

    /// Check if minimum fee enforcement is enabled
    pub fn is_fee_enforcement_enabled(&self) -> bool {
        self.enforce_minimum_fee
    }

    /// Get fee enforcement metrics
    pub fn fee_metrics(&self) -> &MempoolFeeMetrics {
        &self.fee_metrics
    }

    /// Reset fee metrics (useful for testing or periodic resets)
    pub fn reset_fee_metrics(&mut self) {
        self.fee_metrics = MempoolFeeMetrics::default();
    }

    /// Get the fee configuration
    pub fn fee_config(&self) -> &FeeConfig {
        &self.fee_config
    }

    /// Get the dynamic fee configuration
    pub fn dynamic_fee(&self) -> &DynamicFeeBase {
        &self.dynamic_fee
    }

    /// Get mutable reference to dynamic fee for updates
    pub fn dynamic_fee_mut(&mut self) -> &mut DynamicFeeBase {
        &mut self.dynamic_fee
    }

    /// Get current dynamic fee state for diagnostics/RPC
    pub fn dynamic_fee_state(&self) -> DynamicFeeState {
        self.dynamic_fee.state(self.at_min_block_time)
    }

    /// Update dynamic fee state after a block is finalized.
    ///
    /// Call this after each block is confirmed to adjust fee base based on
    /// congestion.
    ///
    /// # Arguments
    /// * `tx_count` - Number of transactions in the finalized block
    /// * `max_tx_count` - Maximum transactions per block (from consensus
    ///   config)
    /// * `at_min_block_time` - Whether block timing is at minimum (3s blocks)
    ///
    /// # Returns
    /// The new fee base to use for the next block
    pub fn update_dynamic_fee(
        &mut self,
        tx_count: usize,
        max_tx_count: usize,
        at_min_block_time: bool,
    ) -> u64 {
        self.at_min_block_time = at_min_block_time;
        self.dynamic_fee
            .update(tx_count, max_tx_count, at_min_block_time)
    }

    /// Get the current dynamic fee base (in nanoBTH per byte)
    pub fn current_fee_base(&self) -> u64 {
        self.dynamic_fee.compute_base(self.at_min_block_time)
    }

    /// Get fee suggestions for wallets based on current network state.
    ///
    /// # Arguments
    /// * `tx_size` - Estimated transaction size in bytes
    /// * `cluster_wealth` - Sender's cluster wealth (0 if unknown)
    pub fn suggest_fees(&self, tx_size: usize, cluster_wealth: u128) -> FeeSuggestion {
        let cluster_factor = self.fee_config.cluster_factor(cluster_wealth);
        self.dynamic_fee
            .suggest_fees(tx_size, cluster_factor, self.at_min_block_time)
    }

    /// Estimate the minimum fee for a transaction using typical size.
    ///
    /// This uses the current dynamic fee base, which adjusts based on network
    /// congestion.
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type (Hidden or Minting)
    /// * `_amount` - The transfer amount (unused, fee is size-based)
    /// * `num_memos` - Number of outputs with memos
    ///
    /// # Returns
    /// The minimum fee in nanoBTH
    pub fn estimate_fee(&self, tx_type: FeeTransactionType, _amount: u64, num_memos: usize) -> u64 {
        // Use 0 for estimation - wallets should use Wallet::compute_cluster_wealth()
        // and call suggest_fees() with their actual cluster wealth for accurate
        // estimates
        let cluster_wealth = 0u128;

        // Get typical size for this tx type
        let typical_size = match tx_type {
            FeeTransactionType::Hidden => 4_000,  // ~4 KB for CLSAG
            FeeTransactionType::Minting => 1_500, // ~1.5 KB for minting
            _ => 4_000,                           // Default to CLSAG size
        };

        // Use dynamic fee calculation
        let dynamic_base = self.dynamic_fee.compute_base(self.at_min_block_time);
        self.fee_config.minimum_fee_dynamic(
            tx_type,
            typical_size,
            cluster_wealth,
            num_memos,
            dynamic_base,
        )
    }

    /// Estimate fee for private (CLSAG) transactions.
    pub fn estimate_fee_standard(&self, amount: u64, num_memos: usize) -> u64 {
        self.estimate_fee(FeeTransactionType::Hidden, amount, num_memos)
    }

    /// Estimate fee with actual cluster wealth for accurate progressive fee
    /// calculation.
    ///
    /// Wallets should use this method after calling
    /// `cluster_getWealthByTargetKeys` RPC to get their actual cluster
    /// wealth. This enables accurate progressive fee estimation where
    /// wealthy clusters pay higher fees.
    ///
    /// # Arguments
    /// * `tx_type` - Type of transaction (affects base size)
    /// * `_amount` - Transaction amount (currently unused, reserved for future)
    /// * `num_memos` - Number of output memos
    /// * `cluster_wealth` - Sender's cluster wealth from
    ///   cluster_getWealthByTargetKeys
    ///
    /// # Returns
    /// Estimated fee in nanoBTH including cluster factor multiplier
    pub fn estimate_fee_with_wealth(
        &self,
        tx_type: FeeTransactionType,
        _amount: u64,
        num_memos: usize,
        cluster_wealth: u128,
    ) -> u64 {
        // Get typical size for this tx type
        let typical_size = match tx_type {
            FeeTransactionType::Hidden => 4_000,  // ~4 KB for CLSAG
            FeeTransactionType::Minting => 1_500, // ~1.5 KB for minting
            _ => 4_000,                           // Default to CLSAG size
        };

        // Use dynamic fee calculation with actual cluster wealth
        let dynamic_base = self.dynamic_fee.compute_base(self.at_min_block_time);
        self.fee_config.minimum_fee_dynamic(
            tx_type,
            typical_size,
            cluster_wealth,
            num_memos,
            dynamic_base,
        )
    }

    /// Estimate fee using static base (ignoring current congestion).
    ///
    /// Useful for testing or when you want the base fee regardless of network
    /// state. Returns minimum fee (cluster_wealth=0). For accurate
    /// estimates with your cluster profile, use `suggest_fees()` with
    /// computed cluster wealth.
    pub fn estimate_fee_static(&self, tx_type: FeeTransactionType, num_memos: usize) -> u64 {
        let cluster_wealth = 0u128;
        self.fee_config
            .estimate_typical_fee(tx_type, cluster_wealth, num_memos)
    }

    /// Get the cluster factor for a given wealth level.
    ///
    /// Returns the multiplier as a fixed-point value (1000 = 1x, 6000 = 6x).
    /// Useful for displaying progressive fee information to users.
    pub fn cluster_factor(&self, cluster_wealth: u128) -> u64 {
        self.fee_config.cluster_factor(cluster_wealth)
    }

    /// Add a transaction to the mempool
    pub fn add_tx(&mut self, tx: Transaction, ledger: &Ledger) -> Result<[u8; 32], MempoolError> {
        let tx_hash = tx.hash();

        // Check if already in mempool
        if self.txs.contains_key(&tx_hash) {
            return Err(MempoolError::AlreadyExists);
        }

        // Check mempool size
        if self.txs.len() >= MAX_MEMPOOL_SIZE {
            self.evict_lowest_fee();
        }

        // Validate transaction structure
        tx.is_valid_structure()
            .map_err(|e| MempoolError::InvalidTransaction(e.to_string()))?;

        // Validate inputs (all transactions use CLSAG ring signatures)
        let (input_sum, ring_members, ring_tags) =
            self.validate_clsag_inputs(tx.inputs.clsag(), &tx, ledger)?;

        // Validate outputs + fee <= inputs
        // Use checked arithmetic to detect overflow from malicious transactions
        let output_sum: u64 = tx
            .outputs
            .iter()
            .try_fold(0u64, |acc, o| acc.checked_add(o.amount))
            .ok_or_else(|| MempoolError::InvalidTransaction("Output sum overflow".to_string()))?;

        let total_output = output_sum
            .checked_add(tx.fee)
            .ok_or_else(|| MempoolError::InvalidTransaction("Output + fee overflow".to_string()))?;

        if total_output > input_sum {
            return Err(MempoolError::InsufficientInputs {
                inputs: input_sum,
                outputs: output_sum,
                fee: tx.fee,
            });
        }

        // Validate fee meets minimum based on transaction size and congestion
        let fee_tx_type = FeeTransactionType::Hidden;

        // Estimate transaction size based on inputs and outputs
        let tx_size_bytes = tx.estimate_size();

        // Compute effective cluster wealth: output tags (inherited from
        // inputs) weighted against the ledger's global per-cluster wealth
        let cluster_wealth = effective_cluster_wealth_from_outputs(&tx.outputs, ledger)
            .map_err(MempoolError::LedgerError)?;
        // Count outputs with encrypted memos for fee calculation
        let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();

        // Get the current dynamic fee base (adjusts based on congestion).
        //
        // RELAY-ONLY policy: `DynamicFeeBase` carries a node-local f64 EMA (reset
        // on restart) and a wall-clock `at_min_block_time` flag, so its output is
        // NOT deterministic across nodes. It must NEVER enter consensus — see the
        // type docs on `bth_cluster_tax::DynamicFeeBase` and the consensus floor
        // `Ledger::consensus_fee_floor` (which deliberately pins the base to the
        // neutral `CONSENSUS_FEE_BASE`, omitting congestion). Because
        // `compute_base` is clamped to `>= base_min == CONSENSUS_FEE_BASE`, this
        // term can only ever RAISE the local relay threshold above the consensus
        // floor (never below it) — the relay-only-tightening invariant asserted
        // just below where `minimum_fee` is computed.
        let dynamic_base = self.dynamic_fee.compute_base(self.at_min_block_time);

        // Use dynamic fee calculation for minimum (with actual output count)
        let num_outputs = tx.outputs.len();
        let base_minimum_fee = self.fee_config.minimum_fee_dynamic_with_outputs(
            fee_tx_type,
            tx_size_bytes,
            cluster_wealth,
            num_outputs,
            num_memos,
            dynamic_base,
        );

        // Cluster demurrage: a holding charge on wealthy-cluster coins,
        // added to the minimum fee at spend time. Elapsed time is the
        // value-weighted centroid of the PUBLIC ring-member creation
        // heights (the real input dominates its own ring's centroid); the
        // factor term means factor-1 (background/commerce) spends pay zero.
        // See docs/design/cluster-tilted-redistribution.md.
        // Cluster factor for demurrage. The factor from the spender-authored
        // output tags (`cluster_wealth`) is only a CLAIM; floor it at the
        // factor implied by the ring members' own public tags so fresh
        // background decoys cannot drag demurrage to zero (audit cycle 6 H2,
        // design #574 item B2).
        let claimed_factor = self.fee_config.cluster_factor(cluster_wealth);
        let demurrage_factor =
            self.ring_centroid_floored_factor(&ring_tags, claimed_factor, ledger)?;

        // Current chain tip height. Used both for the demurrage clock below and
        // for the relay-only-tightening invariant check (M1-B5), so the mempool
        // and the consensus floor evaluate the demurrage at the identical height.
        let chain_height = ledger.get_chain_state().map(|s| s.height).unwrap_or(0);

        let demurrage = {
            let policy = crate::monetary::mainnet_policy();
            // H2/B1 (issue #578, design #574): use the max-quantile order
            // statistic of ring-member ages, NOT the value-weighted mean. The
            // mean lets a spender pad the ring with fresh high-value decoys to
            // drag the demurrage clock toward zero; the max is value-independent
            // and surfaces a lone old real input. Mempool admission and the
            // consensus fee floor (`Ledger::consensus_fee_floor`) share this
            // exact kernel so relay and consensus agree on the demurrage clock.
            let elapsed = bth_cluster_tax::ring_elapsed_quantile(
                &ring_members,
                chain_height,
                10_000, // quantile_bps = 10000 == max
            );
            let blocks_per_year = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);
            bth_cluster_tax::demurrage_charge(
                output_sum,
                demurrage_factor,
                elapsed,
                policy.demurrage_rate_bps(chain_height),
                blocks_per_year,
            )
        };
        let minimum_fee = base_minimum_fee.saturating_add(demurrage);

        // M1-B5 (issue #579, design #574 Q1/Q2): relay-only-tightening invariant.
        //
        // The mempool admission threshold `minimum_fee` must NEVER fall below the
        // deterministic consensus fee floor (`Ledger::consensus_fee_floor`, added
        // in H1-B4 / #578). Relay policy may only ever TIGHTEN above the consensus
        // floor — it can never admit a transaction that consensus would reject at
        // block validation (Bitcoin's min-relay-fee vs. consensus-validity split).
        //
        // Both sides compute the SAME base curve and the SAME demurrage kernel
        // (`ring_elapsed_quantile@max` + `ring_centroid_floored_factor`) at the
        // SAME `chain_height`; the ONLY structural difference is the congestion
        // term: the mempool multiplies in `dynamic_base = compute_base(..)` while
        // the consensus floor pins the base to the neutral `CONSENSUS_FEE_BASE`
        // (== `DynamicFeeBase::default().base_min`). Because `dynamic_base >=
        // base_min` always (`compute_base` is clamped to `[base_min, base_max]`),
        // `minimum_fee >= consensus_floor` holds structurally.
        //
        // This is a SAFETY NET, not a gate: it does not change the admission
        // decision below (that still compares `tx.fee` against `minimum_fee`).
        // We use `debug_assert!` so the invariant is enforced in tests/CI and
        // during development without adding a hot-path ledger read (an extra
        // `consensus_fee_floor` call, which resolves ring UTXOs) to release
        // builds. If it ever fires, a relay-policy change has broken the
        // never-fall-below-consensus contract and must be fixed before ship.
        debug_assert!(
            {
                match ledger.consensus_fee_floor(&tx, chain_height) {
                    Ok(consensus_floor) => minimum_fee >= consensus_floor,
                    // A DB error recomputing the floor is not an invariant
                    // violation; skip the assertion rather than panic on
                    // transient ledger pressure.
                    Err(_) => true,
                }
            },
            "mempool relay minimum_fee {} fell below the consensus fee floor \
             (relay must only tighten, never fall below consensus; height {})",
            minimum_fee,
            chain_height,
        );

        if tx.fee < minimum_fee {
            let shortfall = minimum_fee.saturating_sub(tx.fee);

            // Track metrics for all insufficient fees (even if not enforced)
            self.fee_metrics.fee_rejections += 1;
            self.fee_metrics.total_fee_shortfall = self
                .fee_metrics
                .total_fee_shortfall
                .saturating_add(shortfall);
            self.fee_metrics.max_fee_shortfall = self.fee_metrics.max_fee_shortfall.max(shortfall);

            if self.enforce_minimum_fee {
                debug!(
                    "Rejecting transaction {}: fee {} < minimum {} (cluster_wealth={}, outputs={})",
                    hex::encode(&tx_hash[0..8]),
                    tx.fee,
                    minimum_fee,
                    cluster_wealth,
                    num_outputs
                );
                return Err(MempoolError::FeeTooLow {
                    minimum: minimum_fee,
                    provided: tx.fee,
                });
            } else {
                warn!(
                    "Accepting under-fee transaction {} (enforcement disabled): fee {} < minimum {}",
                    hex::encode(&tx_hash[0..8]),
                    tx.fee,
                    minimum_fee
                );
            }
        }

        // Mark inputs as spent
        // Track spent key images to prevent double-spends in mempool
        // Use entry API to avoid overwriting timestamp if already tracked
        let now = std::time::Instant::now();
        for input in tx.inputs.clsag() {
            self.spent_key_images.entry(input.key_image).or_insert(now);
        }

        // Compute cluster factor for fee density prioritization
        let cluster_factor = self.fee_config.cluster_factor(cluster_wealth);

        // Add to mempool with cluster-adjusted fee density
        let pending = PendingTx::with_cluster_factor(tx, cluster_wealth, cluster_factor);
        self.txs.insert(tx_hash, pending);

        debug!(
            "Added transaction {} to mempool",
            hex::encode(&tx_hash[0..8])
        );
        Ok(tx_hash)
    }

    /// Validate CLSAG (standard-private) transaction inputs.
    ///
    /// Returns the potential input sum, the public (value, creation
    /// height) of every resolved ring member (which feeds the demurrage
    /// elapsed-time centroid), and the (cluster tags, value) of every resolved
    /// ring member (which feeds the cluster-factor floor — see
    /// [`Mempool::ring_centroid_floored_factor`]).
    #[allow(clippy::type_complexity)]
    fn validate_clsag_inputs(
        &self,
        clsag_inputs: &[crate::transaction::ClsagRingInput],
        tx: &Transaction,
        ledger: &Ledger,
    ) -> Result<
        (
            u64,
            Vec<(u64, u64)>,
            Vec<(bth_transaction_types::ClusterTagVector, u64)>,
        ),
        MempoolError,
    > {
        // Check for double-spends via key images (mempool)
        for input in clsag_inputs {
            if self.spent_key_images.contains_key(&input.key_image) {
                return Err(MempoolError::DoubleSpend);
            }
        }

        // Check for double-spends via key images (ledger)
        //
        // M7 fail-closed: distinguish `Ok(None)` (genuinely unspent) from a DB
        // error. Swallowing the error and admitting the tx as if unspent would
        // let an already-spent key image pass this filter under DB pressure.
        for input in clsag_inputs {
            match ledger.is_key_image_spent(&input.key_image) {
                Ok(Some(_)) => return Err(MempoolError::KeyImageSpent(input.key_image)),
                Ok(None) => {}
                Err(e) => return Err(MempoolError::LedgerError(e.to_string())),
            }
        }

        // Verify CLSAG ring signatures
        tx.verify_ring_signatures()
            .map_err(|_| MempoolError::InvalidSignature)?;

        // Validate potential input amounts from ring members
        // Also collect ring tags for plausibility validation and public
        // (value, creation height) pairs for the demurrage centroid
        let mut potential_input_sum: u64 = 0;
        let mut all_ring_tags: Vec<(bth_transaction_types::ClusterTagVector, u64)> = Vec::new();
        let mut ring_members: Vec<(u64, u64)> = Vec::new();

        for input in clsag_inputs {
            let mut max_ring_amount: u64 = 0;
            let mut found_any = false;

            for member in &input.ring {
                if let Ok(Some(utxo)) = ledger.get_utxo_by_target_key(&member.target_key) {
                    max_ring_amount = max_ring_amount.max(utxo.output.amount);
                    found_any = true;
                    // Collect ring member tags and amounts for plausibility check
                    all_ring_tags.push((utxo.output.cluster_tags.clone(), utxo.output.amount));
                    ring_members.push((utxo.output.amount, utxo.created_at));
                }
            }

            if !found_any {
                warn!(
                    "Could not lookup ring member amounts for CLSAG key image {}",
                    hex::encode(&input.key_image[0..8])
                );
                return Err(MempoolError::InvalidTransaction(
                    "Cannot verify CLSAG input amounts - no ring members found in UTXO set"
                        .to_string(),
                ));
            }

            potential_input_sum = potential_input_sum
                .checked_add(max_ring_amount)
                .ok_or_else(|| {
                    MempoolError::InvalidTransaction("CLSAG input sum overflow".to_string())
                })?;
        }

        // Validate ring tag plausibility (prevents decoy manipulation attacks)
        // Get chain state for activation height check
        let chain_state = ledger.get_chain_state().map_err(|e| {
            MempoolError::InvalidTransaction(format!("Cannot get chain state: {}", e))
        })?;

        // Estimate UTXO pool size as ~4 outputs per block (minting rewards)
        // This is a reasonable approximation for the sparse pool check
        let estimated_utxo_count = (chain_state.height as usize) * 4;

        // Compute combined output tags for validation
        let output_tags = bth_transaction_types::ClusterTagVector::merge_weighted(
            &tx.outputs
                .iter()
                .map(|o| (o.cluster_tags.clone(), o.amount))
                .collect::<Vec<_>>(),
            0, // No decay for this comparison
        );

        if let Err((similarity_permille, threshold_permille)) = validate_ring_tag_plausibility(
            &all_ring_tags,
            &output_tags,
            RING_TAG_SIMILARITY_THRESHOLD,
            chain_state.height,
            estimated_utxo_count,
        ) {
            return Err(MempoolError::RingTagMismatch {
                similarity_permille,
                threshold_permille,
            });
        }

        Ok((potential_input_sum, ring_members, all_ring_tags))
    }

    /// Floor a spender-claimed cluster factor at the factor implied by the
    /// ring centroid (audit cycle 6 H2, design #574 item B2).
    ///
    /// The `claimed_factor` is derived from the transaction's spender-authored
    /// OUTPUT tags, so on its own it is gameable: a wealthy spender can tag
    /// outputs as background and pay ~zero demurrage. This raises it to at
    /// least the factor implied by the RING MEMBERS' own (public,
    /// inherited) cluster tags, which the spender cannot rewrite. Fresh
    /// background decoys can no longer drive the factor below what the ring
    /// composition implies.
    ///
    /// Per-cluster wealth is resolved from the ledger **fail-closed** (a DB
    /// read error propagates as `LedgerError` rather than silently
    /// defaulting to zero wealth, which would lower the floor — matching
    /// the M7 fix in [`effective_cluster_wealth_from_outputs`]). The factor
    /// math is the consensus-safe, integer-only, node-local-state-free
    /// helper [`bth_cluster_tax::ring_centroid_implied_factor`] (via the
    /// shared [`Ledger::ring_centroid_floored_factor`]), which item B4 can
    /// reuse on the consensus path.
    fn ring_centroid_floored_factor(
        &self,
        ring_tags: &[(bth_transaction_types::ClusterTagVector, u64)],
        claimed_factor: u64,
        ledger: &Ledger,
    ) -> Result<u64, MempoolError> {
        let ring_members: Vec<(u64, &bth_transaction_types::ClusterTagVector)> = ring_tags
            .iter()
            .map(|(tags, value)| (*value, tags))
            .collect();

        ledger
            .ring_centroid_floored_factor(
                claimed_factor,
                &ring_members,
                &self.fee_config.cluster_curve,
            )
            .map_err(|e| MempoolError::LedgerError(e.to_string()))
    }

    /// Remove a transaction from the mempool and clear its key images.
    ///
    /// Use this when a transaction is confirmed in a block or explicitly
    /// invalidated. For eviction (space/age), use `evict_tx` instead to
    /// preserve key image tracking.
    pub fn remove_tx(&mut self, tx_hash: &[u8; 32]) -> Option<Transaction> {
        if let Some(pending) = self.txs.remove(tx_hash) {
            // Remove spent key images - safe because tx is confirmed/invalid
            for input in pending.tx.inputs.clsag() {
                self.spent_key_images.remove(&input.key_image);
            }
            Some(pending.tx)
        } else {
            None
        }
    }

    /// Evict a transaction from the mempool but KEEP its key images tracked.
    ///
    /// This is used for space/age eviction where we want to prevent the same
    /// key images from being re-submitted (which could cause double-spend
    /// issues if the evicted tx is still in consensus pending_values).
    ///
    /// Key images are only cleared when a transaction is:
    /// - Confirmed in a block (via `remove_tx`)
    /// - Invalidated because key image was spent on-chain (via `remove_tx`)
    fn evict_tx(&mut self, tx_hash: &[u8; 32]) -> Option<Transaction> {
        // Remove from txs map but DO NOT remove from spent_key_images
        self.txs.remove(tx_hash).map(|pending| pending.tx)
    }

    /// Get transactions for inclusion in a block (sorted by fee density).
    ///
    /// Fee density accounts for cluster factor, so wealthy clusters must pay
    /// higher fees to achieve the same priority as smaller clusters.
    /// Formula: `density = fee / (size × cluster_factor)`
    pub fn get_transactions(&self, max_count: usize) -> Vec<Transaction> {
        let mut txs: Vec<_> = self.txs.values().collect();

        // Sort by fee density (highest first) - accounts for cluster factor
        txs.sort_by(|a, b| b.fee_density.cmp(&a.fee_density));

        txs.into_iter()
            .take(max_count)
            .map(|p| p.tx.clone())
            .collect()
    }

    /// Get transactions with their fee data for block building.
    ///
    /// Returns (transaction, fee, cluster_wealth) tuples sorted by fee density.
    /// Used by block builder to calculate lottery pool. `cluster_wealth` is
    /// u128 pico (#626 PR3).
    pub fn get_transactions_with_fees(&self, max_count: usize) -> Vec<(Transaction, u64, u128)> {
        let mut txs: Vec<_> = self.txs.values().collect();

        // Sort by fee density (highest first)
        txs.sort_by(|a, b| b.fee_density.cmp(&a.fee_density));

        txs.into_iter()
            .take(max_count)
            .map(|p| (p.tx.clone(), p.tx.fee, p.cluster_wealth))
            .collect()
    }

    /// Remove transactions that were included in a block
    pub fn remove_confirmed(&mut self, transactions: &[Transaction]) {
        for tx in transactions {
            let tx_hash = tx.hash();
            self.remove_tx(&tx_hash);
        }
    }

    /// Remove transactions that are no longer valid
    ///
    /// Checks if any key images were spent in the ledger.
    pub fn remove_invalid(&mut self, ledger: &Ledger) {
        let mut to_remove = Vec::new();

        for (tx_hash, pending) in &self.txs {
            // Check if any key image was spent in ledger.
            //
            // M7 fail-closed: on a DB error, evict rather than retain. Retaining
            // a possibly-double-spending tx (the pre-fix behavior of
            // `matches!(..., Ok(Some(_)))`, which treats `Err(_)` as "still
            // valid") risks re-proposing a double-spend. A false eviction is
            // harmless — the sender can resubmit.
            let is_invalid = pending.tx.inputs.clsag().iter().any(|input| {
                match ledger.is_key_image_spent(&input.key_image) {
                    Ok(Some(_)) => true, // spent on-chain -> evict
                    Ok(None) => false,   // unspent -> keep
                    Err(e) => {
                        warn!(
                            "DB error checking key image {} for mempool tx {}; \
                             evicting (fail closed): {}",
                            hex::encode(&input.key_image[0..8]),
                            hex::encode(&tx_hash[0..8]),
                            e
                        );
                        true // DB error -> fail closed: evict
                    }
                }
            });

            if is_invalid {
                to_remove.push(*tx_hash);
            }
        }

        for tx_hash in to_remove {
            self.remove_tx(&tx_hash);
            debug!(
                "Removed invalid transaction {} from mempool",
                hex::encode(&tx_hash[0..8])
            );
        }
    }

    /// Evict old transactions.
    ///
    /// Note: Key images are preserved to prevent double-spend attacks where
    /// an evicted transaction is still in consensus pending_values.
    pub fn evict_old(&mut self) {
        let now = std::time::Instant::now();
        let mut to_remove = Vec::new();

        for (tx_hash, pending) in &self.txs {
            if now.duration_since(pending.received_at).as_secs() > MAX_TX_AGE_SECS {
                to_remove.push(*tx_hash);
            }
        }

        for tx_hash in to_remove {
            // Use evict_tx to preserve key image tracking
            self.evict_tx(&tx_hash);
            debug!(
                "Evicted old transaction {} from mempool (key images preserved)",
                hex::encode(&tx_hash[0..8])
            );
        }

        // Also clean up stale key images to prevent unbounded memory growth
        self.cleanup_stale_key_images();
    }

    /// Evict lowest fee density transaction.
    ///
    /// Fee density accounts for cluster factor, ensuring wealthy clusters
    /// don't unfairly occupy mempool space with lower effective priority.
    ///
    /// Note: Key images are preserved to prevent double-spend attacks where
    /// an evicted transaction is still in consensus pending_values.
    fn evict_lowest_fee(&mut self) {
        if let Some((tx_hash, _)) = self
            .txs
            .iter()
            .min_by_key(|(_, p)| p.fee_density)
            .map(|(h, p)| (*h, p.clone()))
        {
            // Use evict_tx to preserve key image tracking
            self.evict_tx(&tx_hash);
            debug!(
                "Evicted low-fee-density transaction {} from mempool (key images preserved)",
                hex::encode(&tx_hash[0..8])
            );
        }
    }

    /// Clean up stale key images that are no longer needed.
    ///
    /// Key images are preserved after transaction eviction to prevent race
    /// conditions with consensus pending_values. However, after enough time
    /// has passed (MAX_KEY_IMAGE_AGE_SECS), we can safely remove them to
    /// prevent unbounded memory growth.
    ///
    /// This should be called periodically (e.g., during evict_old).
    pub fn cleanup_stale_key_images(&mut self) {
        let now = std::time::Instant::now();
        let mut to_remove = Vec::new();

        for (key_image, added_at) in &self.spent_key_images {
            // Only remove if old AND not associated with a current transaction
            if now.duration_since(*added_at).as_secs() > MAX_KEY_IMAGE_AGE_SECS {
                // Check if any transaction in the mempool uses this key image
                let still_in_use = self.txs.values().any(|pending| {
                    pending
                        .tx
                        .inputs
                        .clsag()
                        .iter()
                        .any(|input| &input.key_image == key_image)
                });

                if !still_in_use {
                    to_remove.push(*key_image);
                }
            }
        }

        if !to_remove.is_empty() {
            debug!(
                "Cleaning up {} stale key images (older than {} seconds)",
                to_remove.len(),
                MAX_KEY_IMAGE_AGE_SECS
            );
            for key_image in to_remove {
                self.spent_key_images.remove(&key_image);
            }
        }
    }

    /// Get number of pending transactions
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    /// Check if mempool is empty
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Get total fees of all pending transactions.
    ///
    /// Saturating: admitted fees are balance-validated (bounded by real input
    /// value), but total picocredit supply exceeds `u64::MAX`, so an extreme
    /// aggregate could still overflow. This is a stats/RPC value — with
    /// `overflow-checks = true` on the release profile (#663) an unchecked
    /// `sum()` here could panic the node instead of clamping a statistic.
    pub fn total_fees(&self) -> u64 {
        self.txs
            .values()
            .fold(0u64, |acc, p| acc.saturating_add(p.tx.fee))
    }

    /// Check if a transaction is in the mempool
    pub fn contains(&self, tx_hash: &[u8; 32]) -> bool {
        self.txs.contains_key(tx_hash)
    }

    /// Check if a key image is pending (used by a transaction in the mempool
    /// or recently evicted).
    ///
    /// This is useful for UTXO selection to avoid creating transactions that
    /// would be rejected as double-spends because the key image is already
    /// tracked. Note: key images are preserved after eviction to prevent race
    /// conditions with consensus.
    pub fn is_key_image_pending(&self, key_image: &[u8; 32]) -> bool {
        self.spent_key_images.contains_key(key_image)
    }

    /// Get a transaction by hash
    pub fn get(&self, tx_hash: &[u8; 32]) -> Option<&Transaction> {
        self.txs.get(tx_hash).map(|p| &p.tx)
    }

    /// Iterate over all transactions with their hashes.
    ///
    /// Used for compact block reconstruction to build the short ID → tx
    /// mapping.
    pub fn iter_with_hashes(&self) -> impl Iterator<Item = ([u8; 32], &Transaction)> {
        self.txs.iter().map(|(hash, pending)| (*hash, &pending.tx))
    }
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe mempool wrapper
pub type SharedMempool = std::sync::Arc<std::sync::RwLock<Mempool>>;

/// Create a new shared mempool
pub fn new_shared_mempool() -> SharedMempool {
    std::sync::Arc::new(std::sync::RwLock::new(Mempool::new()))
}

/// Mempool errors
#[derive(Debug, Clone)]
pub enum MempoolError {
    AlreadyExists,
    DoubleSpend,
    UtxoNotFound(UtxoId),
    InvalidTransaction(String),
    InvalidSignature,
    InsufficientInputs {
        inputs: u64,
        outputs: u64,
        fee: u64,
    },
    /// Fee is below the minimum required
    FeeTooLow {
        minimum: u64,
        provided: u64,
    },
    LedgerError(String),
    Full,
    /// Key image was already spent (ring signature double-spend)
    KeyImageSpent([u8; 32]),
    /// Ring tag plausibility check failed (potential decoy manipulation attack)
    /// Values are in permille (parts per 1000): 700 = 70%
    RingTagMismatch {
        similarity_permille: u32,
        threshold_permille: u32,
    },
}

impl std::fmt::Display for MempoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyExists => write!(f, "Transaction already in mempool"),
            Self::DoubleSpend => write!(f, "Double-spend detected"),
            Self::UtxoNotFound(id) => write!(f, "UTXO not found: {:?}", id),
            Self::InvalidTransaction(msg) => write!(f, "Invalid transaction: {}", msg),
            Self::InvalidSignature => write!(f, "Invalid transaction signature"),
            Self::InsufficientInputs {
                inputs,
                outputs,
                fee,
            } => {
                write!(f, "Insufficient inputs: {} < {} + {}", inputs, outputs, fee)
            }
            Self::FeeTooLow { minimum, provided } => {
                write!(
                    f,
                    "Fee too low: {} provided, {} required",
                    provided, minimum
                )
            }
            Self::LedgerError(msg) => write!(f, "Ledger error: {}", msg),
            Self::Full => write!(f, "Mempool is full"),
            Self::KeyImageSpent(ki) => {
                write!(f, "Key image already spent: {}", hex::encode(&ki[0..8]))
            }
            Self::RingTagMismatch {
                similarity_permille,
                threshold_permille,
            } => {
                write!(
                    f,
                    "Ring tag mismatch: similarity {}‰ < threshold {}‰",
                    similarity_permille, threshold_permille
                )
            }
        }
    }
}

impl std::error::Error for MempoolError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{ClsagRingInput, RingMember, TxOutput, MIN_RING_SIZE, MIN_TX_FEE};
    use bth_transaction_types::ClusterTagVector;

    /// Helper to create a test output with raw bytes
    fn test_output(amount: u64, id: u8) -> TxOutput {
        TxOutput {
            amount,
            target_key: [id; 32],
            public_key: [id.wrapping_add(1); 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
        }
    }

    /// Helper to create a minimal test ring member
    fn test_ring_member(id: u8) -> RingMember {
        RingMember {
            target_key: [id; 32],
            public_key: [id.wrapping_add(1); 32],
            commitment: [id.wrapping_add(2); 32],
        }
    }

    /// Helper to create a test CLSAG input with MIN_RING_SIZE members
    fn test_clsag_input(ring_id: u8) -> ClsagRingInput {
        let ring: Vec<RingMember> = (0..MIN_RING_SIZE)
            .map(|i| test_ring_member(ring_id.wrapping_add(i as u8)))
            .collect();
        ClsagRingInput {
            ring,
            key_image: [ring_id; 32],
            commitment_key_image: [ring_id.wrapping_add(100); 32],
            clsag_signature: vec![0u8; 32 + 32 * MIN_RING_SIZE], // Fake signature
            pseudo_output_amount: 0,
        }
    }

    /// Create a test transaction with given fee and height
    fn test_tx(fee: u64, height: u64) -> Transaction {
        Transaction::new_clsag(
            vec![test_clsag_input(height as u8)],
            vec![test_output(1000, height as u8)],
            fee.max(MIN_TX_FEE), // Ensure minimum fee
            height,
        )
    }

    #[test]
    fn test_effective_cluster_wealth_uses_global_ledger_wealth() {
        use crate::{
            ledger::{Ledger, UtxoSnapshot},
            transaction::{Utxo, UtxoId},
        };
        use bth_transaction_types::ClusterId;

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Seed the ledger with a whale cluster: global wealth 100M
        let whale_amount = 100_000_000u64;
        let whale_utxo = Utxo {
            id: UtxoId::new([0x11; 32], 0),
            output: TxOutput {
                amount: whale_amount,
                target_key: [0x42; 32],
                public_key: [0x33; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::single(ClusterId(1)),
            },
            created_at: 1,
        };
        let snapshot = UtxoSnapshot::new(
            1,
            [0u8; 32],
            crate::ledger::ChainState::default(),
            vec![whale_utxo],
            vec![],
            vec![(1, whale_amount as u128)],
        )
        .unwrap();
        ledger.load_from_snapshot(&snapshot, None).unwrap();

        // A tiny transaction whose outputs are 100% tagged to the whale
        // cluster: the fee wealth must be the GLOBAL cluster wealth, not the
        // transaction's own value — splitting cannot reduce the fee rate.
        let small_output = TxOutput {
            amount: 1_000,
            target_key: [0x55; 32],
            public_key: [0x56; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::single(ClusterId(1)),
        };
        let wealth = effective_cluster_wealth_from_outputs(&[small_output], &ledger).unwrap();
        assert_eq!(wealth, whale_amount as u128);

        // Half-tagged output: 50% of the global wealth
        let mut half_tags = ClusterTagVector::single(ClusterId(1));
        half_tags.entries[0].weight = TAG_WEIGHT_SCALE / 2;
        let half_output = TxOutput {
            amount: 1_000,
            target_key: [0x57; 32],
            public_key: [0x58; 32],
            e_memo: None,
            cluster_tags: half_tags,
        };
        let wealth = effective_cluster_wealth_from_outputs(&[half_output], &ledger).unwrap();
        assert_eq!(wealth, whale_amount as u128 / 2);

        // Untagged (background) outputs contribute zero cluster wealth
        let bg_output = test_output(1_000, 9);
        let wealth = effective_cluster_wealth_from_outputs(&[bg_output], &ledger).unwrap();
        assert_eq!(wealth, 0);
    }

    /// M7 fail-closed: when the per-cluster wealth lookup hits a DB error, the
    /// fee-input wealth computation must PROPAGATE the error rather than
    /// `unwrap_or(0)`. Defaulting to zero wealth would lower the progressive
    /// fee (fail-open), letting a wealthy cluster underpay on a transient
    /// DB error.
    ///
    /// The DB error is injected by exhausting the LMDB reader table: the ledger
    /// is opened with one reader slot, held open, so `get_cluster_wealth`'s
    /// read txn fails with `MdbError::ReadersFull`.
    #[test]
    fn test_effective_cluster_wealth_fails_closed_on_db_error() {
        use crate::ledger::Ledger;
        use bth_transaction_types::ClusterId;

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_single_reader(dir.path()).unwrap();

        // Hold the only reader slot open so the next read_txn() fails.
        let _held = ledger.read_txn_for_test().unwrap();

        // A cluster-tagged output forces a get_cluster_wealth() lookup.
        let tagged_output = TxOutput {
            amount: 1_000,
            target_key: [0x55; 32],
            public_key: [0x56; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::single(ClusterId(1)),
        };
        let result = effective_cluster_wealth_from_outputs(&[tagged_output], &ledger);
        assert!(
            result.is_err(),
            "wealth computation must fail closed (propagate DB error), got {:?}",
            result
        );
    }

    /// M7 fail-closed (admission): when the ledger key-image lookup hits a DB
    /// error, `validate_clsag_inputs` must REJECT the transaction rather than
    /// admit it as if the key image were unspent. The pre-fix
    /// `if let Ok(Some(_))` swallowed the error and fell through to admission.
    ///
    /// The DB error is injected by exhausting the LMDB reader table: the ledger
    /// is opened with one reader slot, held open, so `is_key_image_spent`'s
    /// read txn fails with `MdbError::ReadersFull`.
    #[test]
    fn test_validate_clsag_inputs_fails_closed_on_db_error() {
        use crate::ledger::Ledger;

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_single_reader(dir.path()).unwrap();

        // Hold the only reader slot open so the next read_txn() fails.
        let _held = ledger.read_txn_for_test().unwrap();

        let mempool = Mempool::new();
        let tx = test_tx(MIN_TX_FEE, 1);
        let result = mempool.validate_clsag_inputs(tx.inputs.clsag(), &tx, &ledger);

        assert!(
            matches!(result, Err(MempoolError::LedgerError(_))),
            "admission must fail closed (propagate DB error), got {:?}",
            result
        );
    }

    /// M7 fail-closed (eviction): when the ledger key-image lookup hits a DB
    /// error, `remove_invalid` must EVICT the transaction rather than retain a
    /// possible double-spend. The pre-fix `matches!(..., Ok(Some(_)))` treated
    /// `Err(_)` as "still valid" and kept the tx. A false eviction is harmless
    /// (the sender can resubmit), so evicting on uncertainty fails closed.
    #[test]
    fn test_remove_invalid_evicts_on_db_error() {
        use crate::ledger::Ledger;

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_single_reader(dir.path()).unwrap();

        // Hold the only reader slot open so the next read_txn() fails.
        let _held = ledger.read_txn_for_test().unwrap();

        let mut mempool = Mempool::new();
        let tx = test_tx(MIN_TX_FEE, 1);
        let tx_hash = tx.hash();
        mempool.txs.insert(tx_hash, PendingTx::new(tx));
        assert_eq!(mempool.len(), 1);

        mempool.remove_invalid(&ledger);

        assert!(
            mempool.is_empty(),
            "eviction must fail closed (evict on DB error), tx still present"
        );
    }

    #[test]
    fn test_mempool_new() {
        let mempool = Mempool::new();
        assert!(mempool.is_empty());
        assert_eq!(mempool.len(), 0);
    }

    #[test]
    fn test_mempool_default() {
        let mempool = Mempool::default();
        assert!(mempool.is_empty());
        assert_eq!(mempool.total_fees(), 0);
    }

    #[test]
    fn test_pending_tx_fee_per_byte() {
        let tx = test_tx(MIN_TX_FEE, 0);
        let pending = PendingTx::new(tx);
        assert!(pending.fee_per_byte > 0);
    }

    #[test]
    fn test_mempool_contains() {
        let mut mempool = Mempool::new();
        let tx_hash: [u8; 32] = [0x42; 32];

        assert!(!mempool.contains(&tx_hash));

        let tx = test_tx(MIN_TX_FEE, 0);
        let pending = PendingTx::new(tx);
        mempool.txs.insert(tx_hash, pending);

        assert!(mempool.contains(&tx_hash));
    }

    #[test]
    fn test_mempool_remove_tx() {
        let mut mempool = Mempool::new();
        let tx_hash: [u8; 32] = [0x11; 32];

        let tx = test_tx(MIN_TX_FEE, 0);
        let pending = PendingTx::new(tx.clone());
        mempool.txs.insert(tx_hash, pending);

        assert_eq!(mempool.len(), 1);

        let removed = mempool.remove_tx(&tx_hash);
        assert!(removed.is_some());
        assert_eq!(mempool.len(), 0);

        // Removing again should return None
        let removed_again = mempool.remove_tx(&tx_hash);
        assert!(removed_again.is_none());
    }

    #[test]
    fn test_mempool_get_transactions_sorted_by_fee() {
        let mut mempool = Mempool::new();

        // Add transactions with different fees (use created_at_height to make each
        // unique)
        for (i, fee) in [
            MIN_TX_FEE,
            MIN_TX_FEE * 5,
            MIN_TX_FEE * 2,
            MIN_TX_FEE * 10,
            MIN_TX_FEE,
        ]
        .iter()
        .enumerate()
        {
            let tx = test_tx(*fee, i as u64);
            let tx_hash = tx.hash();
            let pending = PendingTx::new(tx);
            mempool.txs.insert(tx_hash, pending);
        }

        assert_eq!(mempool.len(), 5);

        // Get top 3 transactions - should be sorted by fee_per_byte
        let top_txs = mempool.get_transactions(3);
        assert_eq!(top_txs.len(), 3);

        // Highest fee should be first
        assert_eq!(top_txs[0].fee, MIN_TX_FEE * 10);
    }

    #[test]
    fn test_mempool_total_fees() {
        let mut mempool = Mempool::new();

        for (i, fee) in [MIN_TX_FEE, MIN_TX_FEE * 2, MIN_TX_FEE * 3]
            .iter()
            .enumerate()
        {
            let tx = test_tx(*fee, i as u64);
            let tx_hash = tx.hash();
            let pending = PendingTx::new(tx);
            mempool.txs.insert(tx_hash, pending);
        }

        assert_eq!(mempool.total_fees(), MIN_TX_FEE * 6);
    }

    #[test]
    fn test_mempool_get() {
        let mut mempool = Mempool::new();

        let tx = test_tx(MIN_TX_FEE * 9, 0);
        let tx_hash = tx.hash();
        let expected_fee = tx.fee;
        let pending = PendingTx::new(tx);
        mempool.txs.insert(tx_hash, pending);

        let retrieved = mempool.get(&tx_hash);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().fee, expected_fee);

        // Non-existent transaction
        let fake_hash: [u8; 32] = [0xFF; 32];
        assert!(mempool.get(&fake_hash).is_none());
    }

    #[test]
    fn test_mempool_spent_key_images_tracking() {
        let mut mempool = Mempool::new();

        let key_image: [u8; 32] = [0xDE; 32];

        mempool
            .spent_key_images
            .insert(key_image, std::time::Instant::now());

        assert!(mempool.spent_key_images.contains_key(&key_image));
        assert_eq!(mempool.spent_key_images.len(), 1);
    }

    #[test]
    fn test_remove_confirmed_clears_transactions() {
        let mut mempool = Mempool::new();

        // Add some transactions (use different created_at_height to make them unique)
        let tx1 = test_tx(MIN_TX_FEE, 1);
        let tx2 = test_tx(MIN_TX_FEE * 2, 2);

        let tx1_hash = tx1.hash();
        let tx2_hash = tx2.hash();

        mempool.txs.insert(tx1_hash, PendingTx::new(tx1.clone()));
        mempool.txs.insert(tx2_hash, PendingTx::new(tx2.clone()));

        assert_eq!(mempool.len(), 2);

        // Remove confirmed transaction
        mempool.remove_confirmed(&[tx1]);

        assert_eq!(mempool.len(), 1);
        assert!(!mempool.contains(&tx1_hash));
        assert!(mempool.contains(&tx2_hash));
    }

    #[test]
    fn test_mempool_error_display() {
        let err = MempoolError::AlreadyExists;
        assert_eq!(format!("{}", err), "Transaction already in mempool");

        let err = MempoolError::DoubleSpend;
        assert_eq!(format!("{}", err), "Double-spend detected");

        let err = MempoolError::InvalidSignature;
        assert_eq!(format!("{}", err), "Invalid transaction signature");

        let err = MempoolError::Full;
        assert_eq!(format!("{}", err), "Mempool is full");

        let err = MempoolError::InsufficientInputs {
            inputs: 100,
            outputs: 80,
            fee: 30,
        };
        assert!(format!("{}", err).contains("Insufficient inputs"));

        let key_image: [u8; 32] = [0xAB; 32];
        let err = MempoolError::KeyImageSpent(key_image);
        assert!(format!("{}", err).contains("Key image already spent"));
    }

    #[test]
    fn test_shared_mempool() {
        let shared = new_shared_mempool();

        {
            let mempool = shared.read().unwrap();
            assert!(mempool.is_empty());
        }

        {
            let mut mempool = shared.write().unwrap();
            let tx = test_tx(MIN_TX_FEE, 1);
            let tx_hash = tx.hash();
            mempool.txs.insert(tx_hash, PendingTx::new(tx));
        }

        {
            let mempool = shared.read().unwrap();
            assert_eq!(mempool.len(), 1);
        }
    }

    // =========== Ring Tag Plausibility Tests ===========

    use bth_transaction_types::ClusterId;

    /// Helper to create a tag vector with a single cluster at full weight
    fn single_cluster_tags(cluster_id: u64) -> ClusterTagVector {
        ClusterTagVector::single(ClusterId(cluster_id))
    }

    /// Helper to create a tag vector with specific (cluster, weight) pairs
    fn multi_cluster_tags(pairs: &[(u64, u32)]) -> ClusterTagVector {
        let pairs: Vec<(ClusterId, u32)> =
            pairs.iter().map(|(id, w)| (ClusterId(*id), *w)).collect();
        ClusterTagVector::from_pairs(&pairs)
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let tags1 = single_cluster_tags(1);
        let tags2 = single_cluster_tags(1);
        let sim = cosine_similarity(&tags1, &tags2);
        assert!(
            (sim - 1.0).abs() < 0.001,
            "Identical tags should have similarity 1.0"
        );
    }

    #[test]
    fn test_cosine_similarity_different() {
        let tags1 = single_cluster_tags(1);
        let tags2 = single_cluster_tags(2);
        let sim = cosine_similarity(&tags1, &tags2);
        assert!(
            sim < 0.1,
            "Completely different tags should have low similarity"
        );
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let empty1 = ClusterTagVector::empty();
        let empty2 = ClusterTagVector::empty();
        let tags = single_cluster_tags(1);

        // Empty to empty is 1.0
        assert!((cosine_similarity(&empty1, &empty2) - 1.0).abs() < 0.001);

        // Empty to non-empty is 1.0 (maximally compatible)
        assert!((cosine_similarity(&empty1, &tags) - 1.0).abs() < 0.001);
        assert!((cosine_similarity(&tags, &empty1) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_partial_overlap() {
        // 50% cluster 1, 50% cluster 2
        let tags1 = multi_cluster_tags(&[(1, 500_000), (2, 500_000)]);
        // 100% cluster 1
        let tags2 = single_cluster_tags(1);

        let sim = cosine_similarity(&tags1, &tags2);
        // Should be around 0.707 (cos 45°)
        assert!(
            sim > 0.6 && sim < 0.8,
            "Partial overlap should give moderate similarity: {}",
            sim
        );
    }

    #[test]
    fn test_compute_ring_centroid_single_member() {
        let tags = single_cluster_tags(42);
        let ring_tags = vec![(tags.clone(), 1000u64)];
        let centroid = compute_ring_centroid(&ring_tags);

        // Centroid of single member should equal that member's tags
        let sim = cosine_similarity(&centroid, &tags);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_ring_centroid_equal_weights() {
        let tags1 = single_cluster_tags(1);
        let tags2 = single_cluster_tags(2);
        let ring_tags = vec![(tags1, 1000u64), (tags2, 1000u64)];
        let centroid = compute_ring_centroid(&ring_tags);

        // Centroid should have ~50% weight in each cluster
        assert_eq!(centroid.entries.len(), 2);
        let total_weight: u32 = centroid.entries.iter().map(|e| e.weight).sum();
        assert!(total_weight > 900_000 && total_weight <= 1_000_000);
    }

    #[test]
    fn test_compute_ring_centroid_weighted() {
        let tags1 = single_cluster_tags(1);
        let tags2 = single_cluster_tags(2);
        // 3x more value in tags1
        let ring_tags = vec![(tags1, 3000u64), (tags2, 1000u64)];
        let centroid = compute_ring_centroid(&ring_tags);

        // Cluster 1 should have higher weight (~75%)
        let cluster1_weight = centroid
            .entries
            .iter()
            .find(|e| e.cluster_id.0 == 1)
            .map(|e| e.weight)
            .unwrap_or(0);
        let cluster2_weight = centroid
            .entries
            .iter()
            .find(|e| e.cluster_id.0 == 2)
            .map(|e| e.weight)
            .unwrap_or(0);

        assert!(
            cluster1_weight > cluster2_weight * 2,
            "3x value should give >2x weight: c1={}, c2={}",
            cluster1_weight,
            cluster2_weight
        );
    }

    #[test]
    fn test_validate_ring_tag_plausibility_valid_homogeneous() {
        // All ring members have same cluster
        let tags = single_cluster_tags(1);
        let ring_tags = vec![
            (tags.clone(), 1000),
            (tags.clone(), 1000),
            (tags.clone(), 1000),
        ];

        // Output also has same cluster (valid)
        let result = validate_ring_tag_plausibility(
            &ring_tags,
            &tags,
            RING_TAG_SIMILARITY_THRESHOLD,
            20_000,  // Above activation height
            100_000, // Above sparse threshold
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_ring_tag_plausibility_valid_heterogeneous() {
        // Mixed ring members
        let tags1 = single_cluster_tags(1);
        let tags2 = single_cluster_tags(2);
        let ring_tags = vec![(tags1.clone(), 1000), (tags2.clone(), 1000)];

        // Output is a blend (centroid-compatible)
        let output_tags = multi_cluster_tags(&[(1, 500_000), (2, 500_000)]);

        let result = validate_ring_tag_plausibility(
            &ring_tags,
            &output_tags,
            RING_TAG_SIMILARITY_THRESHOLD,
            20_000,
            100_000,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_ring_tag_plausibility_invalid_outlier() {
        // Ring has only cluster 2 and 3
        let tags2 = single_cluster_tags(2);
        let tags3 = single_cluster_tags(3);
        let ring_tags = vec![(tags2.clone(), 1000), (tags3.clone(), 1000)];

        // But output claims to be mostly cluster 1 (impossible!)
        let output_tags = single_cluster_tags(1);

        let result = validate_ring_tag_plausibility(
            &ring_tags,
            &output_tags,
            RING_TAG_SIMILARITY_THRESHOLD,
            20_000,
            100_000,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_ring_tag_plausibility_skipped_before_activation() {
        // Invalid ring (would normally fail)
        let ring_tags = vec![(single_cluster_tags(2), 1000)];
        let output_tags = single_cluster_tags(1);

        // But before activation height, validation is skipped
        let result = validate_ring_tag_plausibility(
            &ring_tags,
            &output_tags,
            RING_TAG_SIMILARITY_THRESHOLD,
            1_000, // Below RING_TAG_VALIDATION_ACTIVATION_HEIGHT
            100_000,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_ring_tag_plausibility_sparse_pool_bypass() {
        // Invalid ring (would normally fail)
        let ring_tags = vec![(single_cluster_tags(2), 1000)];
        let output_tags = single_cluster_tags(1);

        // But with sparse pool, validation is relaxed
        let result = validate_ring_tag_plausibility(
            &ring_tags,
            &output_tags,
            RING_TAG_SIMILARITY_THRESHOLD,
            20_000, // Above activation
            10_000, // Below SPARSE_POOL_THRESHOLD
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_ring_tag_mismatch_error_display() {
        let err = MempoolError::RingTagMismatch {
            similarity_permille: 350,
            threshold_permille: 700,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("350"));
        assert!(msg.contains("700"));
    }

    // =========== Fee Density Prioritization Tests ===========

    #[test]
    fn test_pending_tx_fee_density_without_cluster_factor() {
        let tx = test_tx(10_000, 1);
        let pending = PendingTx::new(tx);

        // Without cluster factor, fee_density equals fee_per_byte
        assert_eq!(pending.fee_density, pending.fee_per_byte);
        assert_eq!(pending.cluster_wealth, 0);
    }

    #[test]
    fn test_pending_tx_fee_density_with_cluster_factor() {
        let tx = test_tx(10_000, 1);

        // Compare two pending txs with different cluster factors
        let pending_1x = PendingTx::with_cluster_factor(tx.clone(), 100_000, 1000); // 1x factor
        let pending_2x = PendingTx::with_cluster_factor(tx.clone(), 1_000_000, 2000); // 2x factor

        // With 2x cluster factor, fee density should be approximately half
        // (Same fee, same size, but 2x divisor)
        let ratio = pending_1x.fee_density as f64 / pending_2x.fee_density as f64;
        assert!(
            ratio > 1.9 && ratio < 2.1,
            "2x cluster factor should halve fee density (ratio: {})",
            ratio
        );
        assert_eq!(pending_2x.cluster_wealth, 1_000_000);
    }

    #[test]
    fn test_fee_density_prioritization_wealthy_pays_more() {
        // Same transaction, different cluster factors
        let tx = test_tx(10_000, 1);

        // tx1: small cluster (factor 1000 = 1x)
        let pending1 = PendingTx::with_cluster_factor(tx.clone(), 100_000, 1000);

        // tx2: wealthy cluster (factor 3000 = 3x)
        let pending2 = PendingTx::with_cluster_factor(tx.clone(), 10_000_000, 3000);

        // Small cluster should have higher priority (higher fee density)
        assert!(
            pending1.fee_density > pending2.fee_density,
            "Small cluster (density {}) should have higher priority than wealthy cluster (density {})",
            pending1.fee_density,
            pending2.fee_density
        );

        // The ratio should be approximately 3x
        let ratio = pending1.fee_density as f64 / pending2.fee_density as f64;
        assert!(
            ratio > 2.9 && ratio < 3.1,
            "3x cluster factor should give 3x priority difference (ratio: {})",
            ratio
        );
    }

    #[test]
    fn test_fee_density_wealthy_can_pay_for_priority() {
        // Use high enough fees to avoid MIN_TX_FEE clamping
        let base_fee = MIN_TX_FEE * 10;
        let tx1 = test_tx(base_fee, 1);
        let tx2 = test_tx(base_fee * 3, 1); // 3x fee, same structure

        // tx1: small cluster (factor 1000 = 1x)
        let pending1 = PendingTx::with_cluster_factor(tx1, 100_000, 1000);

        // tx2: wealthy cluster (factor 3000 = 3x) but pays 3x fee
        let pending2 = PendingTx::with_cluster_factor(tx2, 10_000_000, 3000);

        // With 3x fee and 3x factor, densities should be approximately equal
        let ratio = pending1.fee_density as f64 / pending2.fee_density as f64;
        assert!(
            ratio > 0.9 && ratio < 1.1,
            "3x fee should compensate for 3x cluster factor (ratio: {})",
            ratio
        );
    }

    // =========== Fee Enforcement Tests ===========

    #[test]
    fn test_mempool_fee_enforcement_enabled_by_default() {
        let mempool = Mempool::new();
        assert!(
            mempool.is_fee_enforcement_enabled(),
            "Fee enforcement should be enabled by default"
        );
    }

    #[test]
    fn test_mempool_fee_enforcement_can_be_disabled() {
        let mempool = Mempool::with_fee_enforcement_disabled(FeeConfig::default());
        assert!(
            !mempool.is_fee_enforcement_enabled(),
            "Fee enforcement should be disabled"
        );
    }

    #[test]
    fn test_mempool_fee_enforcement_toggle() {
        let mut mempool = Mempool::new();
        assert!(mempool.is_fee_enforcement_enabled());

        mempool.set_fee_enforcement(false);
        assert!(!mempool.is_fee_enforcement_enabled());

        mempool.set_fee_enforcement(true);
        assert!(mempool.is_fee_enforcement_enabled());
    }

    #[test]
    fn test_mempool_fee_metrics_initial_state() {
        let mempool = Mempool::new();
        let metrics = mempool.fee_metrics();

        assert_eq!(metrics.fee_rejections, 0);
        assert_eq!(metrics.total_fee_shortfall, 0);
        assert_eq!(metrics.max_fee_shortfall, 0);
    }

    #[test]
    fn test_mempool_fee_metrics_reset() {
        let mut mempool = Mempool::new();

        // Manually modify metrics to test reset
        mempool.fee_metrics.fee_rejections = 10;
        mempool.fee_metrics.total_fee_shortfall = 1000;
        mempool.fee_metrics.max_fee_shortfall = 500;

        mempool.reset_fee_metrics();

        let metrics = mempool.fee_metrics();
        assert_eq!(metrics.fee_rejections, 0);
        assert_eq!(metrics.total_fee_shortfall, 0);
        assert_eq!(metrics.max_fee_shortfall, 0);
    }

    #[test]
    fn test_mempool_error_fee_too_low_display() {
        let err = MempoolError::FeeTooLow {
            minimum: 10_000,
            provided: 5_000,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("10000") || msg.contains("10_000") || msg.contains("10,000"));
        assert!(msg.contains("5000") || msg.contains("5_000") || msg.contains("5,000"));
        assert!(msg.contains("required") || msg.contains("minimum"));
    }

    #[test]
    fn test_mempool_fee_metrics_default() {
        let metrics = MempoolFeeMetrics::default();
        assert_eq!(metrics.fee_rejections, 0);
        assert_eq!(metrics.total_fee_shortfall, 0);
        assert_eq!(metrics.max_fee_shortfall, 0);
    }

    #[test]
    fn test_mempool_fee_metrics_clone() {
        let metrics = MempoolFeeMetrics {
            fee_rejections: 5,
            total_fee_shortfall: 1000,
            max_fee_shortfall: 300,
        };
        let cloned = metrics.clone();
        assert_eq!(cloned.fee_rejections, 5);
        assert_eq!(cloned.total_fee_shortfall, 1000);
        assert_eq!(cloned.max_fee_shortfall, 300);
    }

    #[test]
    fn test_mempool_with_fee_config_has_enforcement_enabled() {
        let config = FeeConfig::default();
        let mempool = Mempool::with_fee_config(config);
        assert!(mempool.is_fee_enforcement_enabled());
    }

    #[test]
    fn test_mempool_with_dynamic_fee_has_enforcement_enabled() {
        let config = FeeConfig::default();
        let dynamic_fee = DynamicFeeBase::default();
        let mempool = Mempool::with_dynamic_fee(config, dynamic_fee);
        assert!(mempool.is_fee_enforcement_enabled());
    }

    // ---------------------------------------------------------------------
    // M1-B5 (issue #579, design #574): relay-only-tightening invariant.
    //
    // The mempool admission threshold must always be >= the deterministic
    // consensus fee floor: relay policy can only ever TIGHTEN above consensus,
    // never fall below it. Congestion (the node-local `DynamicFeeBase`) stays a
    // relay-only multiplier; block validity uses the congestion-free
    // `Ledger::consensus_fee_floor`. A node with a HOT congestion EMA and a node
    // with a COLD EMA therefore both admit/reject against a threshold that is
    // always >= the (shared) consensus floor, and both accept the identical set
    // of BLOCKS.
    // ---------------------------------------------------------------------

    /// Seed the ledger, via a snapshot, with a set of ring UTXOs and cluster
    /// wealth, returning the ring members that reference them. Mirrors the
    /// store.rs `seed_ring_utxos` helper but through the public snapshot path
    /// (the low-level UTXO db is private to the ledger module).
    #[cfg(test)]
    fn seed_ring_via_snapshot(
        ledger: &Ledger,
        // (amount, created_at, cluster_id (0 == background), seed byte)
        specs: &[(u64, u64, u64, u8)],
        tip_height: u64,
        cluster_wealth: &[(u64, u128)],
    ) -> Vec<RingMember> {
        use crate::{
            ledger::{ChainState, UtxoSnapshot},
            transaction::{Utxo, UtxoId},
        };
        use bth_transaction_types::{ClusterId, TAG_WEIGHT_SCALE};

        let mut utxos = Vec::new();
        let mut ring = Vec::new();
        for &(amount, created_at, cluster, seed) in specs {
            let tags = if cluster == 0 {
                ClusterTagVector::empty()
            } else {
                ClusterTagVector::from_pairs(&[(ClusterId(cluster), TAG_WEIGHT_SCALE)])
            };
            let output = TxOutput {
                amount,
                target_key: [seed; 32],
                public_key: [seed.wrapping_add(1); 32],
                e_memo: None,
                cluster_tags: tags,
            };
            utxos.push(Utxo {
                id: UtxoId::new([seed; 32], 0),
                output: output.clone(),
                created_at,
            });
            ring.push(RingMember::from_output(&output));
        }

        let mut chain_state = ChainState::default();
        chain_state.height = tip_height;
        let snapshot = UtxoSnapshot::new(
            tip_height,
            [0u8; 32],
            chain_state,
            utxos,
            vec![],
            cluster_wealth.to_vec(),
        )
        .unwrap();
        ledger.load_from_snapshot(&snapshot, None).unwrap();
        ring
    }

    /// Compute the mempool relay-admission `minimum_fee` for `tx` under a given
    /// `DynamicFeeBase` congestion state. This reproduces the exact formula
    /// used in `Mempool::add_tx` (base curve × cluster factor × congestion
    /// base, plus the `ring_elapsed_quantile@max` + factor-floored
    /// demurrage) so the test asserts against the true relay threshold,
    /// differing from the consensus floor ONLY by the congestion
    /// multiplier.
    #[cfg(test)]
    fn relay_minimum_fee(
        tx: &Transaction,
        ledger: &Ledger,
        fee_config: &FeeConfig,
        dynamic_fee: &DynamicFeeBase,
        at_min_block_time: bool,
    ) -> u64 {
        let tx_size_bytes = tx.estimate_size();
        let num_outputs = tx.outputs.len();
        let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();
        let cluster_wealth = effective_cluster_wealth_from_outputs(&tx.outputs, ledger).unwrap();
        let dynamic_base = dynamic_fee.compute_base(at_min_block_time);

        let base_minimum_fee = fee_config.minimum_fee_dynamic_with_outputs(
            FeeTransactionType::Hidden,
            tx_size_bytes,
            cluster_wealth,
            num_outputs,
            num_memos,
            dynamic_base,
        );

        let output_sum: u64 = tx
            .outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.amount));

        // Ring members: (value, created_at) for the age quantile, plus tags for
        // the factor floor — resolved from the committed UTXO set, exactly as
        // the mempool does inside validate_clsag_inputs.
        let mut ring_members: Vec<(u64, u64)> = Vec::new();
        let mut ring_tags: Vec<(ClusterTagVector, u64)> = Vec::new();
        for input in tx.inputs.clsag() {
            for member in &input.ring {
                if let Ok(Some(utxo)) = ledger.get_utxo_by_target_key(&member.target_key) {
                    ring_members.push((utxo.output.amount, utxo.created_at));
                    ring_tags.push((utxo.output.cluster_tags.clone(), utxo.output.amount));
                }
            }
        }

        let claimed_factor = fee_config.cluster_factor(cluster_wealth);
        let ring_refs: Vec<(u64, &ClusterTagVector)> = ring_tags
            .iter()
            .map(|(tags, value)| (*value, tags))
            .collect();
        let demurrage_factor = ledger
            .ring_centroid_floored_factor(claimed_factor, &ring_refs, &fee_config.cluster_curve)
            .unwrap();

        let policy = crate::monetary::mainnet_policy();
        let chain_height = ledger.get_chain_state().map(|s| s.height).unwrap_or(0);
        let elapsed = bth_cluster_tax::ring_elapsed_quantile(&ring_members, chain_height, 10_000);
        let blocks_per_year = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);
        let demurrage = bth_cluster_tax::demurrage_charge(
            output_sum,
            demurrage_factor,
            elapsed,
            policy.demurrage_rate_bps(chain_height),
            blocks_per_year,
        );

        base_minimum_fee.saturating_add(demurrage)
    }

    /// A HOT node (congested, elevated `DynamicFeeBase`) and a COLD node
    /// (uncongested) both compute a relay `minimum_fee` that is >= the
    /// consensus fee floor. The hot node is strictly stricter (relay only
    /// tightens); the consensus floor is identical for both (block validity is
    /// congestion-independent), so both nodes accept the same BLOCKS.
    #[test]
    fn relay_minimum_never_below_consensus_floor_hot_and_cold() {
        use crate::transaction::{ClsagRingInput, TxInputs};
        use bth_transaction_types::{ClusterId, TAG_WEIGHT_SCALE};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Wealthy, old cluster so demurrage is non-trivial. Height must be past
        // the halving interval for demurrage to be active (see store.rs tests).
        let tip_height = 8_000_000u64;
        let wealthy_cluster = 9u64;
        let ring = seed_ring_via_snapshot(
            &ledger,
            &[
                (100_000_000, 0, wealthy_cluster, 80),
                (100_000_000, 10, wealthy_cluster, 81),
            ],
            tip_height,
            // Cluster global wealth in picocredits: 10M BTH -> high factor so
            // demurrage is non-trivial (#626 log-domain curve).
            &[(wealthy_cluster, 10_000_000_000_000_000_000)],
        );

        let outputs = vec![TxOutput {
            amount: 90_000_000,
            target_key: [130; 32],
            public_key: [131; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::from_pairs(&[(
                ClusterId(wealthy_cluster),
                TAG_WEIGHT_SCALE,
            )]),
        }];
        let tx = Transaction {
            inputs: TxInputs::new(vec![ClsagRingInput {
                ring,
                key_image: [0u8; 32],
                commitment_key_image: [0u8; 32],
                clsag_signature: Vec::new(),
                pseudo_output_amount: 0,
            }]),
            outputs,
            fee: 0,
            created_at_height: 0,
        };

        let fee_config = FeeConfig::default();

        // Consensus floor: congestion-free, evaluated at the same height the
        // mempool uses for its demurrage clock.
        let consensus_floor = ledger.consensus_fee_floor(&tx, tip_height).unwrap();
        assert!(
            consensus_floor > 0,
            "expected a positive floor for a wealthy old ring"
        );

        // COLD node: fresh EMA, not at min block time -> dynamic base pinned to
        // base_min == CONSENSUS_FEE_BASE.
        let cold = DynamicFeeBase::default();
        let relay_cold = relay_minimum_fee(&tx, &ledger, &fee_config, &cold, false);

        // HOT node: sustained 100% fullness at min block time -> elevated base.
        let mut hot = DynamicFeeBase::default();
        for _ in 0..50 {
            hot.update(100, 100, true);
        }
        assert!(
            hot.compute_base(true) > cold.compute_base(false),
            "hot node's congestion base must exceed the cold node's"
        );
        let relay_hot = relay_minimum_fee(&tx, &ledger, &fee_config, &hot, true);

        // Relay only tightens: both nodes are >= the consensus floor, and the
        // hot node is strictly stricter than the cold node.
        assert!(
            relay_cold >= consensus_floor,
            "cold relay minimum {relay_cold} fell below consensus floor {consensus_floor}"
        );
        assert!(
            relay_hot >= consensus_floor,
            "hot relay minimum {relay_hot} fell below consensus floor {consensus_floor}"
        );
        assert!(
            relay_hot > relay_cold,
            "hot node must charge more than cold node (hot={relay_hot}, cold={relay_cold})"
        );

        // The cold, uncongested relay minimum equals the consensus floor exactly:
        // the only difference between them is the congestion multiplier, which is
        // 1x when uncongested.
        assert_eq!(
            relay_cold, consensus_floor,
            "uncongested relay minimum must equal the consensus floor (same kernel, 1x congestion)"
        );

        // Block validity is congestion-independent: `verify_consensus_fee_floor`
        // never reads any node-local congestion state, so the block-acceptance
        // decision is identical for the hot and cold nodes above. A tx paying
        // exactly `consensus_floor` is block-valid; one paying `consensus_floor -
        // 1` is not — on every node, regardless of EMA.
        let at_floor_tx = Transaction {
            fee: consensus_floor,
            ..tx.clone()
        };
        assert!(
            ledger
                .verify_consensus_fee_floor_for_test(&at_floor_tx, tip_height)
                .is_ok(),
            "a tx at exactly the consensus floor must pass block validation"
        );
        let under_tx = Transaction {
            fee: consensus_floor - 1,
            ..tx.clone()
        };
        assert!(
            ledger
                .verify_consensus_fee_floor_for_test(&under_tx, tip_height)
                .is_err(),
            "a tx below the consensus floor must be rejected at block validation"
        );
    }
}
