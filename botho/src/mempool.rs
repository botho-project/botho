// Copyright (c) 2024 Botho Foundation

//! Transaction mempool for storing pending transactions.
//!
//! All transactions are private by default using ring signatures (CLSAG or LION).
//! Tracks spent key images to prevent double-spending.
//!
//! ## Fee Validation
//!
//! Uses the cluster-tax fee system to compute minimum fees based on:
//! - Transaction type (CLSAG = Hidden, LION = PqHidden)
//! - Transfer amount
//! - Sender's cluster wealth (see note below)
//! - Number of outputs with encrypted memos
//!
//! ## Cluster Wealth Tracking
//!
//! The progressive fee system charges higher fees to wealthier clusters (1x-6x multiplier).
//! Cluster wealth is computed from transaction outputs, which inherit merged+decayed tags
//! from inputs. This means:
//!
//! - Fresh mints start with weight=100% for a new cluster ID
//! - Each transaction decays weights by 5% (DEFAULT_CLUSTER_DECAY_RATE)
//! - Mixed inputs produce merged tag vectors weighted by amount
//! - Maximum cluster wealth determines the fee multiplier
//!
//! For fee estimation (wallets), use `Wallet::compute_cluster_wealth()` on UTXOs.
//! For fee validation (mempool), cluster wealth is computed from output tags.

use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

use bth_cluster_tax::{DynamicFeeBase, DynamicFeeState, FeeConfig, FeeSuggestion, TransactionType as FeeTransactionType};
use bth_transaction_types::TAG_WEIGHT_SCALE;
use crate::ledger::Ledger;
use crate::transaction::{Transaction, TxInputs, TxOutput, UtxoId};

/// Compute the maximum cluster wealth from transaction outputs.
///
/// This computes cluster wealth from outputs (which inherit from inputs via merge_weighted).
/// For each cluster, wealth contribution = sum(output_amount × tag_weight / TAG_WEIGHT_SCALE).
/// Returns the maximum wealth across all clusters, which is used for progressive fee calculation.
///
/// This is appropriate for fee validation because:
/// 1. Outputs inherit merged+decayed tags from inputs
/// 2. The sender's cluster profile is preserved through inheritance
/// 3. Higher cluster concentration → higher output tag weights → higher computed wealth
fn compute_cluster_wealth_from_outputs(outputs: &[TxOutput]) -> u64 {
    let mut cluster_wealths: HashMap<u64, u64> = HashMap::new();

    for output in outputs {
        let value = output.amount;
        for entry in &output.cluster_tags.entries {
            let contribution = ((value as u128) * (entry.weight as u128)
                / (TAG_WEIGHT_SCALE as u128)) as u64;
            *cluster_wealths.entry(entry.cluster_id.0).or_insert(0) += contribution;
        }
    }

    cluster_wealths.values().copied().max().unwrap_or(0)
}

/// Maximum transactions in mempool.
///
/// Increased from 1000 to 10000 to support higher transaction throughput.
/// Memory impact: ~50-650MB depending on transaction type (CLSAG ~5KB, LION ~65KB).
/// See docs/memory-budget.md for detailed memory planning.
const MAX_MEMPOOL_SIZE: usize = 10_000;

/// Maximum age of a transaction in seconds before eviction
const MAX_TX_AGE_SECS: u64 = 3600; // 1 hour

/// A pending transaction with metadata
#[derive(Debug, Clone)]
pub struct PendingTx {
    pub tx: Transaction,
    pub received_at: std::time::Instant,
    pub fee_per_byte: u64,
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
        }
    }
}

/// Transaction mempool
pub struct Mempool {
    /// Pending transactions by hash
    txs: HashMap<[u8; 32], PendingTx>,
    /// Spent key images in mempool (for double-spend prevention)
    spent_key_images: HashSet<[u8; 32]>,
    /// Fee configuration for computing minimum fees
    fee_config: FeeConfig,
    /// Dynamic fee base for congestion control
    dynamic_fee: DynamicFeeBase,
    /// Whether we're at minimum block time (triggers dynamic fee adjustment)
    at_min_block_time: bool,
}

impl Mempool {
    /// Create a new empty mempool with default fee configuration
    pub fn new() -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashSet::new(),
            fee_config: FeeConfig::default(),
            dynamic_fee: DynamicFeeBase::default(),
            at_min_block_time: false,
        }
    }

    /// Create a new empty mempool with custom fee configuration
    pub fn with_fee_config(fee_config: FeeConfig) -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashSet::new(),
            fee_config,
            dynamic_fee: DynamicFeeBase::default(),
            at_min_block_time: false,
        }
    }

    /// Create a new empty mempool with custom fee and dynamic fee configuration
    pub fn with_dynamic_fee(fee_config: FeeConfig, dynamic_fee: DynamicFeeBase) -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashSet::new(),
            fee_config,
            dynamic_fee,
            at_min_block_time: false,
        }
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
    /// Call this after each block is confirmed to adjust fee base based on congestion.
    ///
    /// # Arguments
    /// * `tx_count` - Number of transactions in the finalized block
    /// * `max_tx_count` - Maximum transactions per block (from consensus config)
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
        self.dynamic_fee.update(tx_count, max_tx_count, at_min_block_time)
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
    pub fn suggest_fees(&self, tx_size: usize, cluster_wealth: u64) -> FeeSuggestion {
        let cluster_factor = self.fee_config.cluster_factor(cluster_wealth);
        self.dynamic_fee.suggest_fees(tx_size, cluster_factor, self.at_min_block_time)
    }

    /// Estimate the minimum fee for a transaction using typical size.
    ///
    /// This uses the current dynamic fee base, which adjusts based on network congestion.
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type (Hidden or PqHidden)
    /// * `_amount` - The transfer amount (unused, fee is size-based)
    /// * `num_memos` - Number of outputs with memos
    ///
    /// # Returns
    /// The minimum fee in nanoBTH
    pub fn estimate_fee(&self, tx_type: FeeTransactionType, _amount: u64, num_memos: usize) -> u64 {
        // Use 0 for estimation - wallets should use Wallet::compute_cluster_wealth()
        // and call suggest_fees() with their actual cluster wealth for accurate estimates
        let cluster_wealth = 0u64;

        // Get typical size for this tx type
        let typical_size = match tx_type {
            FeeTransactionType::Hidden => 4_000,      // ~4 KB for CLSAG
            FeeTransactionType::PqHidden => 65_000,   // ~65 KB for LION
            FeeTransactionType::Minting => 1_500,     // ~1.5 KB for minting
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

    /// Estimate fee for standard-private (CLSAG) transactions.
    pub fn estimate_fee_standard(&self, amount: u64, num_memos: usize) -> u64 {
        self.estimate_fee(FeeTransactionType::Hidden, amount, num_memos)
    }

    /// Estimate fee for PQ-private (LION) transactions.
    pub fn estimate_fee_pq(&self, amount: u64, num_memos: usize) -> u64 {
        self.estimate_fee(FeeTransactionType::PqHidden, amount, num_memos)
    }

    /// Estimate fee with actual cluster wealth for accurate progressive fee calculation.
    ///
    /// Wallets should use this method after calling `cluster_getWealthByTargetKeys` RPC
    /// to get their actual cluster wealth. This enables accurate progressive fee
    /// estimation where wealthy clusters pay higher fees.
    ///
    /// # Arguments
    /// * `tx_type` - Type of transaction (affects base size)
    /// * `_amount` - Transaction amount (currently unused, reserved for future)
    /// * `num_memos` - Number of output memos
    /// * `cluster_wealth` - Sender's cluster wealth from cluster_getWealthByTargetKeys
    ///
    /// # Returns
    /// Estimated fee in nanoBTH including cluster factor multiplier
    pub fn estimate_fee_with_wealth(
        &self,
        tx_type: FeeTransactionType,
        _amount: u64,
        num_memos: usize,
        cluster_wealth: u64,
    ) -> u64 {
        // Get typical size for this tx type
        let typical_size = match tx_type {
            FeeTransactionType::Hidden => 4_000,      // ~4 KB for CLSAG
            FeeTransactionType::PqHidden => 65_000,   // ~65 KB for LION
            FeeTransactionType::Minting => 1_500,     // ~1.5 KB for minting
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
    /// Useful for testing or when you want the base fee regardless of network state.
    /// Returns minimum fee (cluster_wealth=0). For accurate estimates with your
    /// cluster profile, use `suggest_fees()` with computed cluster wealth.
    pub fn estimate_fee_static(&self, tx_type: FeeTransactionType, num_memos: usize) -> u64 {
        let cluster_wealth = 0u64;
        self.fee_config.estimate_typical_fee(tx_type, cluster_wealth, num_memos)
    }

    /// Get the cluster factor for a given wealth level.
    ///
    /// Returns the multiplier as a fixed-point value (1000 = 1x, 6000 = 6x).
    /// Useful for displaying progressive fee information to users.
    pub fn cluster_factor(&self, cluster_wealth: u64) -> u64 {
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

        // Validate based on input type
        let input_sum = match &tx.inputs {
            TxInputs::Clsag(clsag_inputs) => {
                self.validate_clsag_inputs(clsag_inputs, &tx, ledger)?
            }
            TxInputs::Lion(lion_inputs) => {
                self.validate_lion_inputs(lion_inputs, &tx, ledger)?
            }
        };

        // Validate outputs + fee <= inputs
        // Use checked arithmetic to detect overflow from malicious transactions
        let output_sum: u64 = tx.outputs.iter()
            .try_fold(0u64, |acc, o| acc.checked_add(o.amount))
            .ok_or_else(|| MempoolError::InvalidTransaction("Output sum overflow".to_string()))?;

        let total_output = output_sum.checked_add(tx.fee)
            .ok_or_else(|| MempoolError::InvalidTransaction("Output + fee overflow".to_string()))?;

        if total_output > input_sum {
            return Err(MempoolError::InsufficientInputs {
                inputs: input_sum,
                outputs: output_sum,
                fee: tx.fee,
            });
        }

        // Validate fee meets minimum based on transaction type, size, and congestion
        let fee_tx_type = match &tx.inputs {
            TxInputs::Clsag(_) => FeeTransactionType::Hidden,
            TxInputs::Lion(_) => FeeTransactionType::PqHidden, // Higher fee for ~16x larger LION signatures
        };

        // Estimate transaction size based on inputs and outputs
        let tx_size_bytes = tx.estimate_size();

        // Compute cluster wealth from transaction outputs (which inherit from inputs)
        let cluster_wealth = compute_cluster_wealth_from_outputs(&tx.outputs);
        // Count outputs with encrypted memos for fee calculation
        let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();

        // Get the current dynamic fee base (adjusts based on congestion)
        let dynamic_base = self.dynamic_fee.compute_base(self.at_min_block_time);

        // Use dynamic fee calculation for minimum
        let minimum_fee = self.fee_config.minimum_fee_dynamic(
            fee_tx_type,
            tx_size_bytes,
            cluster_wealth,
            num_memos,
            dynamic_base,
        );

        if tx.fee < minimum_fee {
            return Err(MempoolError::FeeTooLow {
                minimum: minimum_fee,
                provided: tx.fee,
            });
        }

        // Mark inputs as spent
        match &tx.inputs {
            TxInputs::Clsag(clsag_inputs) => {
                for input in clsag_inputs {
                    self.spent_key_images.insert(input.key_image);
                }
            }
            TxInputs::Lion(lion_inputs) => {
                for input in lion_inputs {
                    // LION key images are larger - hash to 32 bytes for tracking
                    use sha2::{Sha256, Digest};
                    let mut hasher = Sha256::new();
                    hasher.update(&input.key_image);
                    let key_image_hash: [u8; 32] = hasher.finalize().into();
                    self.spent_key_images.insert(key_image_hash);
                }
            }
        }

        // Add to mempool
        let pending = PendingTx::new(tx);
        self.txs.insert(tx_hash, pending);

        debug!("Added transaction {} to mempool", hex::encode(&tx_hash[0..8]));
        Ok(tx_hash)
    }

    /// Validate CLSAG (standard-private) transaction inputs
    fn validate_clsag_inputs(
        &self,
        clsag_inputs: &[crate::transaction::ClsagRingInput],
        tx: &Transaction,
        ledger: &Ledger,
    ) -> Result<u64, MempoolError> {
        // Check for double-spends via key images (mempool)
        for input in clsag_inputs {
            if self.spent_key_images.contains(&input.key_image) {
                return Err(MempoolError::DoubleSpend);
            }
        }

        // Check for double-spends via key images (ledger)
        for input in clsag_inputs {
            if let Ok(Some(_)) = ledger.is_key_image_spent(&input.key_image) {
                return Err(MempoolError::KeyImageSpent(input.key_image));
            }
        }

        // Verify CLSAG ring signatures
        tx.verify_ring_signatures()
            .map_err(|_| MempoolError::InvalidSignature)?;

        // Validate potential input amounts from ring members
        let mut potential_input_sum: u64 = 0;

        for input in clsag_inputs {
            let mut max_ring_amount: u64 = 0;
            let mut found_any = false;

            for member in &input.ring {
                if let Ok(Some(utxo)) = ledger.get_utxo_by_target_key(&member.target_key) {
                    max_ring_amount = max_ring_amount.max(utxo.output.amount);
                    found_any = true;
                }
            }

            if !found_any {
                warn!(
                    "Could not lookup ring member amounts for CLSAG key image {}",
                    hex::encode(&input.key_image[0..8])
                );
                return Err(MempoolError::InvalidTransaction(
                    "Cannot verify CLSAG input amounts - no ring members found in UTXO set".to_string()
                ));
            }

            potential_input_sum = potential_input_sum.checked_add(max_ring_amount)
                .ok_or_else(|| MempoolError::InvalidTransaction(
                    "CLSAG input sum overflow".to_string()
                ))?;
        }

        Ok(potential_input_sum)
    }

    /// Validate LION (PQ-private) transaction inputs
    fn validate_lion_inputs(
        &self,
        lion_inputs: &[crate::transaction::LionRingInput],
        tx: &Transaction,
        ledger: &Ledger,
    ) -> Result<u64, MempoolError> {
        use sha2::{Sha256, Digest};

        // Check for double-spends via key images (mempool)
        for input in lion_inputs {
            let mut hasher = Sha256::new();
            hasher.update(&input.key_image);
            let key_image_hash: [u8; 32] = hasher.finalize().into();
            if self.spent_key_images.contains(&key_image_hash) {
                return Err(MempoolError::DoubleSpend);
            }
        }

        // Check for double-spends via key images (ledger)
        for input in lion_inputs {
            let mut hasher = Sha256::new();
            hasher.update(&input.key_image);
            let key_image_hash: [u8; 32] = hasher.finalize().into();
            if let Ok(Some(_)) = ledger.is_key_image_spent(&key_image_hash) {
                return Err(MempoolError::KeyImageSpent(key_image_hash));
            }
        }

        // Verify LION ring signatures
        tx.verify_ring_signatures()
            .map_err(|_| MempoolError::InvalidSignature)?;

        // Validate potential input amounts from ring members
        let mut potential_input_sum: u64 = 0;

        for input in lion_inputs {
            let mut max_ring_amount: u64 = 0;
            let mut found_any = false;

            for member in &input.ring {
                if let Ok(Some(utxo)) = ledger.get_utxo_by_target_key(&member.target_key) {
                    max_ring_amount = max_ring_amount.max(utxo.output.amount);
                    found_any = true;
                }
            }

            if !found_any {
                let key_image_display = if input.key_image.len() >= 8 {
                    hex::encode(&input.key_image[0..8])
                } else {
                    hex::encode(&input.key_image)
                };
                warn!(
                    "Could not lookup ring member amounts for LION key image {}",
                    key_image_display
                );
                return Err(MempoolError::InvalidTransaction(
                    "Cannot verify LION input amounts - no ring members found in UTXO set".to_string()
                ));
            }

            potential_input_sum = potential_input_sum.checked_add(max_ring_amount)
                .ok_or_else(|| MempoolError::InvalidTransaction(
                    "LION input sum overflow".to_string()
                ))?;
        }

        Ok(potential_input_sum)
    }

    /// Remove a transaction from the mempool
    pub fn remove_tx(&mut self, tx_hash: &[u8; 32]) -> Option<Transaction> {
        if let Some(pending) = self.txs.remove(tx_hash) {
            // Remove spent inputs
            match &pending.tx.inputs {
                TxInputs::Clsag(clsag_inputs) => {
                    for input in clsag_inputs {
                        self.spent_key_images.remove(&input.key_image);
                    }
                }
                TxInputs::Lion(lion_inputs) => {
                    for input in lion_inputs {
                        use sha2::{Sha256, Digest};
                        let mut hasher = Sha256::new();
                        hasher.update(&input.key_image);
                        let key_image_hash: [u8; 32] = hasher.finalize().into();
                        self.spent_key_images.remove(&key_image_hash);
                    }
                }
            }
            Some(pending.tx)
        } else {
            None
        }
    }

    /// Get transactions for inclusion in a block (sorted by fee)
    pub fn get_transactions(&self, max_count: usize) -> Vec<Transaction> {
        let mut txs: Vec<_> = self.txs.values().collect();

        // Sort by fee per byte (highest first)
        txs.sort_by(|a, b| b.fee_per_byte.cmp(&a.fee_per_byte));

        txs.into_iter()
            .take(max_count)
            .map(|p| p.tx.clone())
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
            let is_invalid = match &pending.tx.inputs {
                TxInputs::Clsag(clsag_inputs) => {
                    // Check if any key image was spent in ledger
                    clsag_inputs.iter().any(|input| {
                        matches!(ledger.is_key_image_spent(&input.key_image), Ok(Some(_)))
                    })
                }
                TxInputs::Lion(lion_inputs) => {
                    // Check if any key image was spent in ledger
                    lion_inputs.iter().any(|input| {
                        use sha2::{Sha256, Digest};
                        let mut hasher = Sha256::new();
                        hasher.update(&input.key_image);
                        let key_image_hash: [u8; 32] = hasher.finalize().into();
                        matches!(ledger.is_key_image_spent(&key_image_hash), Ok(Some(_)))
                    })
                }
            };

            if is_invalid {
                to_remove.push(*tx_hash);
            }
        }

        for tx_hash in to_remove {
            self.remove_tx(&tx_hash);
            debug!("Removed invalid transaction {} from mempool", hex::encode(&tx_hash[0..8]));
        }
    }

    /// Evict old transactions
    pub fn evict_old(&mut self) {
        let now = std::time::Instant::now();
        let mut to_remove = Vec::new();

        for (tx_hash, pending) in &self.txs {
            if now.duration_since(pending.received_at).as_secs() > MAX_TX_AGE_SECS {
                to_remove.push(*tx_hash);
            }
        }

        for tx_hash in to_remove {
            self.remove_tx(&tx_hash);
            debug!("Evicted old transaction {} from mempool", hex::encode(&tx_hash[0..8]));
        }
    }

    /// Evict lowest fee transaction
    fn evict_lowest_fee(&mut self) {
        if let Some((tx_hash, _)) = self.txs.iter()
            .min_by_key(|(_, p)| p.fee_per_byte)
            .map(|(h, p)| (*h, p.clone()))
        {
            self.remove_tx(&tx_hash);
            debug!("Evicted low-fee transaction {} from mempool", hex::encode(&tx_hash[0..8]));
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

    /// Get total fees of all pending transactions
    pub fn total_fees(&self) -> u64 {
        self.txs.values().map(|p| p.tx.fee).sum()
    }

    /// Check if a transaction is in the mempool
    pub fn contains(&self, tx_hash: &[u8; 32]) -> bool {
        self.txs.contains_key(tx_hash)
    }

    /// Get a transaction by hash
    pub fn get(&self, tx_hash: &[u8; 32]) -> Option<&Transaction> {
        self.txs.get(tx_hash).map(|p| &p.tx)
    }

    /// Iterate over all transactions with their hashes.
    ///
    /// Used for compact block reconstruction to build the short ID → tx mapping.
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
}

impl std::fmt::Display for MempoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyExists => write!(f, "Transaction already in mempool"),
            Self::DoubleSpend => write!(f, "Double-spend detected"),
            Self::UtxoNotFound(id) => write!(f, "UTXO not found: {:?}", id),
            Self::InvalidTransaction(msg) => write!(f, "Invalid transaction: {}", msg),
            Self::InvalidSignature => write!(f, "Invalid transaction signature"),
            Self::InsufficientInputs { inputs, outputs, fee } => {
                write!(f, "Insufficient inputs: {} < {} + {}", inputs, outputs, fee)
            }
            Self::FeeTooLow { minimum, provided } => {
                write!(f, "Fee too low: {} provided, {} required", provided, minimum)
            }
            Self::LedgerError(msg) => write!(f, "Ledger error: {}", msg),
            Self::Full => write!(f, "Mempool is full"),
            Self::KeyImageSpent(ki) => write!(f, "Key image already spent: {}", hex::encode(&ki[0..8])),
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

        // Add transactions with different fees (use created_at_height to make each unique)
        for (i, fee) in [MIN_TX_FEE, MIN_TX_FEE * 5, MIN_TX_FEE * 2, MIN_TX_FEE * 10, MIN_TX_FEE].iter().enumerate() {
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

        for (i, fee) in [MIN_TX_FEE, MIN_TX_FEE * 2, MIN_TX_FEE * 3].iter().enumerate() {
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

        mempool.spent_key_images.insert(key_image);

        assert!(mempool.spent_key_images.contains(&key_image));
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
}
