// Copyright (c) 2024 Botho Foundation

//! Transaction mempool for storing pending transactions.
//!
//! Handles both simple (visible sender) and private (ring signature) transactions.
//! For simple transactions, tracks spent UTXOs. For private transactions, tracks
//! spent key images to prevent double-spending.

use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

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
    /// Spent UTXOs in mempool (for simple transactions)
    spent_utxos: HashSet<UtxoId>,
    /// Spent key images in mempool (for private transactions)
    spent_key_images: HashSet<[u8; 32]>,
}

impl Mempool {
    /// Create a new empty mempool
    pub fn new() -> Self {
        Self {
            txs: HashMap::new(),
            spent_utxos: HashSet::new(),
            spent_key_images: HashSet::new(),
        }
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
            TxInputs::Simple(inputs) => {
                self.validate_simple_inputs(inputs, &tx, ledger)?
            }
            TxInputs::Ring(ring_inputs) => {
                self.validate_ring_inputs(ring_inputs, &tx, ledger)?
            }
        };

        // Validate outputs + fee <= inputs
        let output_sum: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        if output_sum + tx.fee > input_sum {
            return Err(MempoolError::InsufficientInputs {
                inputs: input_sum,
                outputs: output_sum,
                fee: tx.fee,
            });
        }

        // Mark inputs as spent
        match &tx.inputs {
            TxInputs::Simple(inputs) => {
                for input in inputs {
                    let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
                    self.spent_utxos.insert(utxo_id);
                }
            }
            TxInputs::Ring(ring_inputs) => {
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

    /// Validate simple (visible) transaction inputs
    fn validate_simple_inputs(
        &self,
        inputs: &[crate::transaction::TxInput],
        tx: &Transaction,
        ledger: &Ledger,
    ) -> Result<u64, MempoolError> {
        let signing_hash = tx.signing_hash();
        let mut input_sum = 0u64;

        // Check for double-spends within mempool
        for input in inputs {
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            if self.spent_utxos.contains(&utxo_id) {
                return Err(MempoolError::DoubleSpend);
            }
        }

        // Validate inputs exist in ledger and verify signatures
        for input in inputs {
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            match ledger.get_utxo(&utxo_id) {
                Ok(Some(utxo)) => {
                    // Verify signature against the UTXO's target_key
                    if !input.verify_signature(&signing_hash, &utxo.output.target_key) {
                        warn!(
                            "Invalid signature for input {}:{}",
                            hex::encode(&input.tx_hash[0..8]),
                            input.output_index
                        );
                        return Err(MempoolError::InvalidSignature);
                    }
                    input_sum += utxo.output.amount;
                }
                Ok(None) => {
                    return Err(MempoolError::UtxoNotFound(utxo_id));
                }
                Err(e) => {
                    return Err(MempoolError::LedgerError(e.to_string()));
                }
            }
        }

        Ok(input_sum)
    }

    /// Validate ring signature (private) transaction inputs
    fn validate_ring_inputs(
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

        // With ring signatures and trivial commitments, we can't know exact
        // input amounts without revealing which ring member is real.
        // For now, return MAX and let higher-level validation handle amounts.
        // TODO: Implement proper balance verification with commitments
        Ok(u64::MAX) // Allow through, block validation will catch issues
    }

    /// Remove a transaction from the mempool
    pub fn remove_tx(&mut self, tx_hash: &[u8; 32]) -> Option<Transaction> {
        if let Some(pending) = self.txs.remove(tx_hash) {
            // Remove spent inputs
            match &pending.tx.inputs {
                TxInputs::Simple(inputs) => {
                    for input in inputs {
                        let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
                        self.spent_utxos.remove(&utxo_id);
                    }
                }
                TxInputs::Ring(ring_inputs) => {
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
    /// For simple transactions: checks if UTXOs still exist
    /// For private transactions: checks if key images were spent
    pub fn remove_invalid(&mut self, ledger: &Ledger) {
        let mut to_remove = Vec::new();

        for (tx_hash, pending) in &self.txs {
            let is_invalid = match &pending.tx.inputs {
                TxInputs::Simple(inputs) => {
                    // Check if any input UTXO no longer exists
                    inputs.iter().any(|input| {
                        let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
                        matches!(ledger.get_utxo(&utxo_id), Ok(None) | Err(_))
                    })
                }
                TxInputs::Ring(ring_inputs) => {
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
}
