//! Sync state persistence.
//!
//! This module handles persisting the scanner's sync progress to disk,
//! enabling resumable scanning after restarts.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Persistent sync state for the scanner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct SyncState {
    /// Last fully scanned block height
    pub last_scanned_height: u64,

    /// Hash of the last scanned block (for reorg detection)
    pub last_scanned_hash: String,

    /// Timestamp of last successful sync (Unix timestamp)
    pub last_sync_timestamp: u64,

    /// Total deposits detected so far
    pub total_deposits: u64,

    /// Total amount received (picocredits)
    pub total_amount: u64,

    /// Number of outputs scanned
    pub total_outputs_scanned: u64,
}


impl SyncState {
    /// Create a new empty sync state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load sync state from a file.
    ///
    /// Returns default state if the file doesn't exist.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            tracing::info!("No sync state file found, starting from scratch");
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        let state: SyncState = serde_json::from_str(&content)?;

        tracing::info!(
            "Loaded sync state: height={}, deposits={}, amount={}",
            state.last_scanned_height,
            state.total_deposits,
            state.total_amount
        );

        Ok(state)
    }

    /// Save sync state to a file.
    ///
    /// Uses atomic write (write to temp file, then rename) to prevent corruption.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(self)?;

        // Write to temp file first
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, &content)?;

        // Atomic rename
        std::fs::rename(&temp_path, path)?;

        tracing::debug!(
            "Saved sync state: height={}",
            self.last_scanned_height
        );

        Ok(())
    }

    /// Update state after scanning a batch.
    pub fn update_after_batch(
        &mut self,
        end_height: u64,
        block_hash: &str,
        deposits_found: u64,
        amount_received: u64,
        outputs_scanned: u64,
    ) {
        self.last_scanned_height = end_height;
        self.last_scanned_hash = block_hash.to_string();
        self.last_sync_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.total_deposits += deposits_found;
        self.total_amount += amount_received;
        self.total_outputs_scanned += outputs_scanned;
    }

    /// Check if we need to rescan due to a reorg.
    ///
    /// This compares our last scanned block hash with the chain's hash
    /// at that height. If they don't match, a reorg occurred.
    pub fn check_reorg(&self, height: u64, chain_hash: &str) -> bool {
        if height != self.last_scanned_height {
            return false;
        }
        !self.last_scanned_hash.is_empty() && self.last_scanned_hash != chain_hash
    }

    /// Get the starting height for the next scan.
    pub fn next_scan_height(&self) -> u64 {
        if self.last_scanned_height == 0 {
            0
        } else {
            self.last_scanned_height + 1
        }
    }

    /// Format a human-readable summary.
    pub fn summary(&self) -> String {
        let last_sync = if self.last_sync_timestamp > 0 {
            chrono::DateTime::from_timestamp(self.last_sync_timestamp as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "never".to_string()
        };

        format!(
            "Sync State:\n  Last height: {}\n  Last sync: {}\n  Total deposits: {}\n  Total amount: {} picocredits\n  Outputs scanned: {}",
            self.last_scanned_height,
            last_sync,
            self.total_deposits,
            self.total_amount,
            self.total_outputs_scanned
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_state() {
        let state = SyncState::default();
        assert_eq!(state.last_scanned_height, 0);
        assert_eq!(state.total_deposits, 0);
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut state = SyncState::default();
        state.last_scanned_height = 12345;
        state.total_deposits = 42;
        state.last_scanned_hash = "abc123".to_string();

        state.save(&path).unwrap();

        let loaded = SyncState::load(&path).unwrap();
        assert_eq!(loaded.last_scanned_height, 12345);
        assert_eq!(loaded.total_deposits, 42);
        assert_eq!(loaded.last_scanned_hash, "abc123");
    }

    #[test]
    fn test_load_missing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");

        let state = SyncState::load(&path).unwrap();
        assert_eq!(state.last_scanned_height, 0);
    }

    #[test]
    fn test_update_after_batch() {
        let mut state = SyncState::default();

        state.update_after_batch(100, "hash1", 5, 1000000, 500);

        assert_eq!(state.last_scanned_height, 100);
        assert_eq!(state.total_deposits, 5);
        assert_eq!(state.total_amount, 1000000);
        assert_eq!(state.total_outputs_scanned, 500);

        state.update_after_batch(200, "hash2", 3, 500000, 300);

        assert_eq!(state.last_scanned_height, 200);
        assert_eq!(state.total_deposits, 8);
        assert_eq!(state.total_amount, 1500000);
    }

    #[test]
    fn test_check_reorg() {
        let mut state = SyncState::default();
        state.last_scanned_height = 100;
        state.last_scanned_hash = "abc123".to_string();

        // Same hash - no reorg
        assert!(!state.check_reorg(100, "abc123"));

        // Different hash - reorg
        assert!(state.check_reorg(100, "def456"));

        // Different height - not applicable
        assert!(!state.check_reorg(99, "def456"));
    }

    #[test]
    fn test_next_scan_height() {
        let mut state = SyncState::default();
        assert_eq!(state.next_scan_height(), 0);

        state.last_scanned_height = 100;
        assert_eq!(state.next_scan_height(), 101);
    }
}
