// Copyright (c) 2024 Botho Foundation

//! Transaction mempool for storing pending transactions.
//!
//! All transactions are private by default using ring signatures (CLSAG, LION, or MLSAG).
//! Tracks spent key images to prevent double-spending.
//!
//! ## Fee Validation
//!
//! Uses the cluster-tax fee system to compute minimum fees based on:
//! - Transaction type (CLSAG/MLSAG = Hidden, LION = PqHidden)
//! - Transfer amount
//! - Sender's cluster wealth (currently 0, cluster tracking not yet implemented)
//! - Number of outputs with encrypted memos

use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

use bth_cluster_tax::{FeeConfig, TransactionType as FeeTransactionType};
use crate::ledger::Ledger;
use crate::transaction::{Transaction, TxInputs, UtxoId};

/// Maximum transactions in mempool
const MAX_MEMPOOL_SIZE: usize = 1000;

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
    pub fn new(tx: Transaction) -> Self {
        let tx_size = bincode::serialize(&tx).map(|b| b.len()).unwrap_or(1);
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
}

impl Mempool {
    /// Create a new empty mempool with default fee configuration
    pub fn new() -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashSet::new(),
            fee_config: FeeConfig::default(),
        }
    }

    /// Create a new empty mempool with custom fee configuration
    pub fn with_fee_config(fee_config: FeeConfig) -> Self {
        Self {
            txs: HashMap::new(),
            spent_key_images: HashSet::new(),
            fee_config,
        }
    }

    /// Get the fee configuration
    pub fn fee_config(&self) -> &FeeConfig {
        &self.fee_config
    }

    /// Estimate the minimum fee for a transaction.
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type (Plain, Hidden, or PqHidden)
    /// * `amount` - The transfer amount in picocredits
    /// * `num_memos` - Number of outputs with memos (currently unused, set to 0)
    ///
    /// # Returns
    /// The minimum fee in picocredits
    pub fn estimate_fee(&self, tx_type: FeeTransactionType, amount: u64, num_memos: usize) -> u64 {
        // cluster_wealth = 0 for now (cluster tracking not yet implemented)
        let cluster_wealth = 0u64;

        self.fee_config.minimum_fee(tx_type, amount, cluster_wealth, num_memos)
    }

    /// Estimate fee for standard-private (CLSAG) transactions.
    pub fn estimate_fee_standard(&self, amount: u64, num_memos: usize) -> u64 {
        self.estimate_fee(FeeTransactionType::Hidden, amount, num_memos)
    }

    /// Estimate fee for PQ-private (LION) transactions.
    pub fn estimate_fee_pq(&self, amount: u64, num_memos: usize) -> u64 {
        self.estimate_fee(FeeTransactionType::PqHidden, amount, num_memos)
    }

    /// Get the fee rate in basis points for a transaction type.
    ///
    /// Useful for displaying to users. 100 bps = 1%.
    pub fn fee_rate_bps(&self, tx_type: FeeTransactionType) -> u32 {
        // cluster_wealth = 0 for now
        self.fee_config.fee_rate_bps(tx_type, 0)
    }

    /// Get the fee rate in basis points for standard-private (CLSAG) transactions.
    pub fn fee_rate_bps_standard(&self) -> u32 {
        self.fee_rate_bps(FeeTransactionType::Hidden)
    }

    /// Get the fee rate in basis points for PQ-private (LION) transactions.
    pub fn fee_rate_bps_pq(&self) -> u32 {
        self.fee_rate_bps(FeeTransactionType::PqHidden)
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
            TxInputs::Mlsag(mlsag_inputs) => {
                self.validate_mlsag_inputs(mlsag_inputs, &tx, ledger)?
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

        // Validate fee meets minimum based on transaction type and amount
        let fee_tx_type = match &tx.inputs {
            TxInputs::Clsag(_) | TxInputs::Mlsag(_) => FeeTransactionType::Hidden,
            TxInputs::Lion(_) => FeeTransactionType::PqHidden, // Higher fee for ~90x larger LION signatures
        };

        // Use output_sum as the transfer amount for fee calculation
        // cluster_wealth = 0 for now (cluster tracking not yet implemented)
        let cluster_wealth = 0u64;
        // Count outputs with encrypted memos for fee calculation
        let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();
        let minimum_fee = self.fee_config.minimum_fee(
            fee_tx_type,
            output_sum,
            cluster_wealth,
            num_memos,
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
            TxInputs::Mlsag(ring_inputs) => {
                for ring_input in ring_inputs {
                    self.spent_key_images.insert(ring_input.key_image);
                }
            }
        }

        // Add to mempool
        let pending = PendingTx::new(tx);
        self.txs.insert(tx_hash, pending);

        debug!("Added transaction {} to mempool", hex::encode(&tx_hash[0..8]));
        Ok(tx_hash)
    }

    /// Validate MLSAG ring signature transaction inputs
    fn validate_mlsag_inputs(
        &self,
        ring_inputs: &[crate::transaction::RingTxInput],
        tx: &Transaction,
        ledger: &Ledger,
    ) -> Result<u64, MempoolError> {
        // Check for double-spends via key images (mempool)
        for ring_input in ring_inputs {
            if self.spent_key_images.contains(&ring_input.key_image) {
                return Err(MempoolError::DoubleSpend);
            }
        }

        // Check for double-spends via key images (ledger)
        for ring_input in ring_inputs {
            if let Ok(Some(_)) = ledger.is_key_image_spent(&ring_input.key_image) {
                return Err(MempoolError::KeyImageSpent(ring_input.key_image));
            }
        }

        // Verify ring signatures
        tx.verify_ring_signatures()
            .map_err(|_| MempoolError::InvalidSignature)?;

        // Validate potential input amounts from ring members.
        // Since we use trivial commitments (zero blinding), amounts are public.
        // For each ring, find the maximum amount among ring members to get a
        // conservative upper bound on potential input value.
        //
        // Note: This doesn't reveal which ring member is the real input, but
        // ensures the transaction COULD be valid if the right member is spent.
        let mut potential_input_sum: u64 = 0;

        for ring_input in ring_inputs {
            // Find maximum amount among ring members by looking up UTXOs
            let mut max_ring_amount: u64 = 0;
            let mut found_any = false;

            for member in &ring_input.ring {
                // Look up the UTXO by target_key to get its amount
                if let Ok(Some(utxo)) = ledger.get_utxo_by_target_key(&member.target_key) {
                    max_ring_amount = max_ring_amount.max(utxo.output.amount);
                    found_any = true;
                }
                // Ring members that can't be found might be spent or from older blocks
                // The ring signature verification ensures at least one is valid
            }

            // If we couldn't find any ring member amounts, reject the transaction.
            // All ring members should exist in the UTXO set for proper validation.
            if !found_any {
                warn!(
                    "Could not lookup ring member amounts for key image {}",
                    hex::encode(&ring_input.key_image[0..8])
                );
                return Err(MempoolError::InvalidTransaction(
                    "Cannot verify ring input amounts - no ring members found in UTXO set".to_string()
                ));
            }

            // Use checked_add to reject transactions with overflowing input sums
            // rather than silently capping at u64::MAX (which could allow invalid txs)
            potential_input_sum = potential_input_sum.checked_add(max_ring_amount)
                .ok_or_else(|| MempoolError::InvalidTransaction(
                    "Ring input sum overflow".to_string()
                ))?;
        }

        Ok(potential_input_sum)
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
                TxInputs::Mlsag(ring_inputs) => {
                    for ring_input in ring_inputs {
                        self.spent_key_images.remove(&ring_input.key_image);
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
                TxInputs::Mlsag(ring_inputs) => {
                    // Check if any key image was spent in ledger
                    ring_inputs.iter().any(|ri| {
                        matches!(ledger.is_key_image_spent(&ri.key_image), Ok(Some(_)))
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
        // Create a minimal transaction to test fee calculation
        let tx = Transaction::new_simple(vec![], vec![], 1000, 0);

        let pending = PendingTx::new(tx);
        assert!(pending.fee_per_byte > 0);
    }

    #[test]
    fn test_mempool_contains() {
        let mut mempool = Mempool::new();
        let tx_hash: [u8; 32] = [0x42; 32];

        assert!(!mempool.contains(&tx_hash));

        // Manually insert a transaction for testing
        let tx = Transaction::new_simple(vec![], vec![], 100, 0);
        let pending = PendingTx::new(tx);
        mempool.txs.insert(tx_hash, pending);

        assert!(mempool.contains(&tx_hash));
    }

    #[test]
    fn test_mempool_remove_tx() {
        let mut mempool = Mempool::new();
        let tx_hash: [u8; 32] = [0x11; 32];

        let tx = Transaction::new_simple(vec![], vec![], 500, 0);
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
        for (i, fee) in [100u64, 500, 200, 1000, 50].iter().enumerate() {
            let tx = Transaction::new_simple(vec![], vec![], *fee, i as u64);
            let tx_hash = tx.hash();
            let pending = PendingTx::new(tx);
            mempool.txs.insert(tx_hash, pending);
        }

        assert_eq!(mempool.len(), 5);

        // Get top 3 transactions - should be sorted by fee_per_byte
        let top_txs = mempool.get_transactions(3);
        assert_eq!(top_txs.len(), 3);

        // Highest fee should be first
        assert_eq!(top_txs[0].fee, 1000);
    }

    #[test]
    fn test_mempool_total_fees() {
        let mut mempool = Mempool::new();

        for (i, fee) in [100u64, 200, 300].iter().enumerate() {
            let tx = Transaction::new_simple(vec![], vec![], *fee, i as u64);
            let tx_hash = tx.hash();
            let pending = PendingTx::new(tx);
            mempool.txs.insert(tx_hash, pending);
        }

        assert_eq!(mempool.total_fees(), 600);
    }

    #[test]
    fn test_mempool_get() {
        let mut mempool = Mempool::new();

        let tx = Transaction::new_simple(vec![], vec![], 999, 0);
        let tx_hash = tx.hash();
        let pending = PendingTx::new(tx);
        mempool.txs.insert(tx_hash, pending);

        let retrieved = mempool.get(&tx_hash);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().fee, 999);

        // Non-existent transaction
        let fake_hash: [u8; 32] = [0xFF; 32];
        assert!(mempool.get(&fake_hash).is_none());
    }

    #[test]
    fn test_mempool_spent_utxos_tracking() {
        let mut mempool = Mempool::new();

        // Create a UTXO ID
        let utxo_id = UtxoId::new([0x11; 32], 0);

        // Add it to spent set
        mempool.spent_utxos.insert(utxo_id);

        assert!(mempool.spent_utxos.contains(&utxo_id));
        assert_eq!(mempool.spent_utxos.len(), 1);
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
        let tx1 = Transaction::new_simple(vec![], vec![], 100, 1);
        let tx2 = Transaction::new_simple(vec![], vec![], 200, 2);

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
            let tx = Transaction::new_simple(vec![], vec![], 100, 1);
            let tx_hash = tx.hash();
            mempool.txs.insert(tx_hash, PendingTx::new(tx));
        }

        {
            let mempool = shared.read().unwrap();
            assert_eq!(mempool.len(), 1);
        }
    }
}
