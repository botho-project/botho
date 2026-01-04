//! Background deposit scanner for registered view keys.
//!
//! This module provides a background task that scans new blocks for outputs
//! matching registered exchange view keys, pushing deposit events via
//! WebSocket.

use std::sync::{Arc, RwLock};

use super::{view_keys::ViewKeyRegistry, websocket::WsBroadcaster};
use crate::ledger::Ledger;

/// Background deposit scanner.
///
/// Scans new blocks for outputs matching registered view keys and
/// broadcasts deposit events via WebSocket.
pub struct DepositScanner {
    /// Registry of view keys to scan for
    view_key_registry: Arc<ViewKeyRegistry>,
    /// WebSocket broadcaster for deposit events
    ws_broadcaster: Arc<WsBroadcaster>,
    /// Ledger for accessing blockchain data
    ledger: Arc<RwLock<Ledger>>,
    /// Last scanned block height
    last_scanned_height: u64,
}

impl DepositScanner {
    /// Create a new deposit scanner.
    pub fn new(
        view_key_registry: Arc<ViewKeyRegistry>,
        ws_broadcaster: Arc<WsBroadcaster>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            view_key_registry,
            ws_broadcaster,
            ledger,
            last_scanned_height: 0,
        }
    }

    /// Set the starting scan height.
    pub fn set_start_height(&mut self, height: u64) {
        self.last_scanned_height = height;
    }

    /// Get the last scanned height.
    pub fn last_scanned_height(&self) -> u64 {
        self.last_scanned_height
    }

    /// Scan a specific block for deposits matching registered view keys.
    ///
    /// This is called when a new block is added to the chain.
    pub fn scan_block(&mut self, block_height: u64) -> ScanResult {
        // Skip if no view keys registered
        if self.view_key_registry.count() == 0 {
            self.last_scanned_height = block_height;
            return ScanResult::default();
        }

        let ledger = match self.ledger.read() {
            Ok(l) => l,
            Err(_) => {
                tracing::error!("Failed to acquire ledger lock for deposit scanning");
                return ScanResult::default();
            }
        };

        let block = match ledger.get_block(block_height) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(
                    "Failed to get block {} for deposit scanning: {}",
                    block_height,
                    e
                );
                return ScanResult::default();
            }
        };

        let chain_height = ledger
            .get_chain_state()
            .map(|s| s.height)
            .unwrap_or(block_height);

        drop(ledger); // Release lock before scanning

        let mut result = ScanResult::default();
        result.block_height = block_height;

        for tx in &block.transactions {
            let tx_hash = tx.hash();

            for (output_index, output) in tx.outputs.iter().enumerate() {
                result.outputs_scanned += 1;

                // Scan against all registered view keys
                let matches = self
                    .view_key_registry
                    .scan_output(&output.target_key, &output.public_key);

                for (view_key_id, subaddress_index) in matches {
                    let confirmations = chain_height.saturating_sub(block_height) + 1;

                    // Broadcast deposit event
                    self.ws_broadcaster.deposit_detected(
                        &view_key_id,
                        subaddress_index,
                        &tx_hash,
                        output_index as u32,
                        output.amount,
                        confirmations,
                        block_height,
                    );

                    result.deposits_found += 1;
                    result.total_amount += output.amount;

                    tracing::info!(
                        "Deposit detected: view_key={}, subaddress={}, tx={}, amount={}",
                        view_key_id,
                        subaddress_index,
                        hex::encode(&tx_hash[..8]),
                        output.amount
                    );
                }
            }
        }

        self.last_scanned_height = block_height;

        if result.deposits_found > 0 {
            tracing::info!(
                "Block {} scan complete: {} deposits found, {} picocredits",
                block_height,
                result.deposits_found,
                result.total_amount
            );
        }

        result
    }

    /// Scan multiple blocks (catch-up scan).
    ///
    /// Used when the node starts up to scan any blocks that were added
    /// while the node was offline.
    pub fn scan_range(&mut self, start_height: u64, end_height: u64) -> ScanResult {
        let mut total_result = ScanResult::default();

        for height in start_height..=end_height {
            let block_result = self.scan_block(height);
            total_result.outputs_scanned += block_result.outputs_scanned;
            total_result.deposits_found += block_result.deposits_found;
            total_result.total_amount += block_result.total_amount;
        }

        total_result.block_height = end_height;
        total_result
    }

    /// Send confirmation updates for previously detected deposits.
    ///
    /// Called periodically to update clients about confirmation progress.
    pub fn update_confirmations(&self, current_height: u64) {
        // This would require tracking detected deposits, which we skip for now.
        // The client-side scanner handles confirmations more efficiently.
        let _ = current_height;
    }
}

/// Result of scanning a block or range.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    /// Block height scanned (last block if range)
    pub block_height: u64,
    /// Number of outputs scanned
    pub outputs_scanned: u64,
    /// Number of deposits found
    pub deposits_found: u64,
    /// Total amount deposited (picocredits)
    pub total_amount: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full tests require setting up a mock ledger and registry.
    // These are integration-level tests that would be in a separate test module.

    #[test]
    fn test_scan_result_default() {
        let result = ScanResult::default();
        assert_eq!(result.deposits_found, 0);
        assert_eq!(result.outputs_scanned, 0);
    }
}
