//! Detected deposit types.

use serde::{Deserialize, Serialize};

/// A detected deposit matching the exchange's view key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedDeposit {
    /// Transaction hash containing the output (hex)
    pub tx_hash: String,

    /// Output index within the transaction
    pub output_index: u32,

    /// Subaddress index that received this deposit
    pub subaddress_index: u64,

    /// Amount in picocredits
    pub amount: u64,

    /// Block height where the deposit was confirmed
    pub block_height: u64,

    /// Number of confirmations at detection time
    pub confirmations: u64,

    /// One-time target key (hex, for later spending)
    pub target_key: String,

    /// Ephemeral public key (hex, for key recovery)
    pub public_key: String,

    /// Timestamp of detection (ISO 8601)
    pub detected_at: String,
}

impl DetectedDeposit {
    /// Create a new detected deposit.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tx_hash: [u8; 32],
        output_index: u32,
        subaddress_index: u64,
        amount: u64,
        block_height: u64,
        confirmations: u64,
        target_key: [u8; 32],
        public_key: [u8; 32],
    ) -> Self {
        Self {
            tx_hash: hex::encode(tx_hash),
            output_index,
            subaddress_index,
            amount,
            block_height,
            confirmations,
            target_key: hex::encode(target_key),
            public_key: hex::encode(public_key),
            detected_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Get the unique identifier for this deposit (tx_hash:output_index).
    pub fn deposit_id(&self) -> String {
        format!("{}:{}", self.tx_hash, self.output_index)
    }

    /// Convert to JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Convert to pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Summary of scanning results for a batch of blocks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanBatchResult {
    /// Starting block height of this batch
    pub start_height: u64,

    /// Ending block height of this batch
    pub end_height: u64,

    /// Number of outputs scanned
    pub outputs_scanned: u64,

    /// Number of deposits detected
    pub deposits_found: u64,

    /// Total amount deposited (picocredits)
    pub total_amount: u64,

    /// List of detected deposits
    pub deposits: Vec<DetectedDeposit>,

    /// Scan duration in milliseconds
    pub duration_ms: u64,
}

impl ScanBatchResult {
    /// Create a new empty batch result.
    pub fn new(start_height: u64, end_height: u64) -> Self {
        Self {
            start_height,
            end_height,
            ..Default::default()
        }
    }

    /// Add a detected deposit to this batch.
    pub fn add_deposit(&mut self, deposit: DetectedDeposit) {
        self.total_amount += deposit.amount;
        self.deposits_found += 1;
        self.deposits.push(deposit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deposit_id() {
        let deposit = DetectedDeposit::new(
            [0u8; 32], 5, 100, 1_000_000, 12345, 10, [1u8; 32], [2u8; 32],
        );
        assert!(deposit.deposit_id().ends_with(":5"));
    }

    #[test]
    fn test_to_json() {
        let deposit =
            DetectedDeposit::new([0u8; 32], 0, 0, 1_000_000, 100, 10, [0u8; 32], [0u8; 32]);
        let json = deposit.to_json();
        assert!(json.contains("\"amount\":1000000"));
    }
}
