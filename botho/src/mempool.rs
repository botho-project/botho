// Copyright (c) 2024 Botho Foundation

//! Transaction mempool for storing pending transactions.

use bt_account_keys::PublicAddress;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

use crate::ledger::Ledger;
use crate::transaction::{Transaction, TxInput, UtxoId};

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
    /// Set of spent UTXOs (to detect double-spends)
    spent_utxos: HashSet<UtxoId>,
}

impl Mempool {
    /// Create a new empty mempool
    pub fn new() -> Self {
        Self {
            txs: HashMap::new(),
            spent_utxos: HashSet::new(),
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
            // Evict lowest fee transaction
            self.evict_lowest_fee();
        }

        // Validate transaction structure
        tx.is_valid_structure()
            .map_err(|e| MempoolError::InvalidTransaction(e.to_string()))?;

        // Check for double-spends within mempool
        for input in &tx.inputs {
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            if self.spent_utxos.contains(&utxo_id) {
                return Err(MempoolError::DoubleSpend);
            }
        }

        // Validate inputs exist in ledger and verify signatures
        let signing_hash = tx.signing_hash();
        let mut input_sum = 0u64;
        for input in &tx.inputs {
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            match ledger.get_utxo(&utxo_id) {
                Ok(Some(utxo)) => {
                    // Verify signature against the UTXO's target_key (one-time public key)
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

        // Validate outputs + fee <= inputs
        let output_sum: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        if output_sum + tx.fee > input_sum {
            return Err(MempoolError::InsufficientInputs {
                inputs: input_sum,
                outputs: output_sum,
                fee: tx.fee,
            });
        }

        // Mark UTXOs as spent
        for input in &tx.inputs {
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            self.spent_utxos.insert(utxo_id);
        }

        // Add to mempool
        let pending = PendingTx::new(tx);
        self.txs.insert(tx_hash, pending);

        debug!("Added transaction {} to mempool", hex::encode(&tx_hash[0..8]));
        Ok(tx_hash)
    }

    /// Remove a transaction from the mempool
    pub fn remove_tx(&mut self, tx_hash: &[u8; 32]) -> Option<Transaction> {
        if let Some(pending) = self.txs.remove(tx_hash) {
            // Remove spent UTXOs
            for input in &pending.tx.inputs {
                let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
                self.spent_utxos.remove(&utxo_id);
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

    /// Remove transactions that spend UTXOs that no longer exist
    pub fn remove_invalid(&mut self, ledger: &Ledger) {
        let mut to_remove = Vec::new();

        for (tx_hash, pending) in &self.txs {
            for input in &pending.tx.inputs {
                let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
                match ledger.get_utxo(&utxo_id) {
                    Ok(None) => {
                        // UTXO no longer exists
                        to_remove.push(*tx_hash);
                        break;
                    }
                    Err(_) => {
                        to_remove.push(*tx_hash);
                        break;
                    }
                    Ok(Some(_)) => {}
                }
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
pub type SharedMempool = Arc<RwLock<Mempool>>;

/// Create a new shared mempool
pub fn new_shared_mempool() -> SharedMempool {
    Arc::new(RwLock::new(Mempool::new()))
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
