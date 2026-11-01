//! Sync state persistence.
//!
//! This module handles persisting the scanner's sync progress to disk,
//! enabling resumable scanning after restarts.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Persistent sync state for the scanner.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    /// Last fully scanned block height. Only blocks that have reached
    /// `min_confirmations` are ever scanned (#1779), so every deposit at or
    /// below this height has already been credited.
    pub last_scanned_height: u64,

    /// Whether any block (including genesis) has been scanned. Disambiguates
    /// "scanned through block 0" from "nothing scanned yet", both of which
    /// have `last_scanned_height == 0`; without it block 0 would be re-scanned
    /// (and its deposits re-credited) on every pass. Absent in legacy state
    /// files, where it defaults to `false` (the old behavior).
    #[serde(default)]
    pub scanned_any: bool,

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
    /// Uses atomic write (write to temp file, then rename) to prevent
    /// corruption.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(self)?;

        // Write to temp file first
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, &content)?;

        // Atomic rename
        std::fs::rename(&temp_path, path)?;

        tracing::debug!("Saved sync state: height={}", self.last_scanned_height);

        Ok(())
    }

    /// Update state after scanning a batch.
    pub fn update_after_batch(
        &mut self,
        end_height: u64,
        deposits_found: u64,
        amount_received: u64,
        outputs_scanned: u64,
    ) {
        self.last_scanned_height = end_height;
        self.scanned_any = true;
        self.last_sync_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.total_deposits += deposits_found;
        self.total_amount += amount_received;
        self.total_outputs_scanned += outputs_scanned;
    }

    /// Get the starting height for the next scan.
    pub fn next_scan_height(&self) -> u64 {
        if self.last_scanned_height == 0 && !self.scanned_any {
            0
        } else {
            self.last_scanned_height + 1
        }
    }

    /// Position the checkpoint so the next scan starts at `height`
    /// (the `--from-height` override).
    pub fn set_start_height(&mut self, height: u64) {
        if height == 0 {
            self.last_scanned_height = 0;
            self.scanned_any = false;
        } else {
            self.last_scanned_height = height - 1;
            self.scanned_any = true;
        }
    }

    /// Plan the next batch: the inclusive block range `[start, end]` to scan,
    /// or `None` if there is nothing confirmed to scan yet.
    ///
    /// The range never extends past [`confirmed_tip`] (#1779): every deposit
    /// in a scanned block already has at least `min_confirmations`, so the
    /// checkpoint can advance to `end` without stranding a not-yet-confirmed
    /// deposit behind it. Blocks above the confirmed tip are left for a later
    /// pass, which will scan them exactly once when they mature. The range is
    /// also capped at `batch_size` blocks (#1656).
    pub fn plan_batch(
        &self,
        chain_height: u64,
        min_confirmations: u64,
        batch_size: u64,
    ) -> Option<(u64, u64)> {
        let start = self.next_scan_height();
        let tip = confirmed_tip(chain_height, min_confirmations)?;
        if start > tip {
            return None;
        }
        let end = std::cmp::min(start.saturating_add(batch_size.max(1) - 1), tip);
        Some((start, end))
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

/// Highest block whose outputs have at least `min_confirmations`
/// confirmations at `chain_height`, or `None` if no block does yet.
///
/// A block at height `h` has `chain_height - h + 1` confirmations (the tip
/// block itself has one). `min_confirmations` of `0` is treated as `1`: an
/// output cannot be credited before it is in a block.
pub fn confirmed_tip(chain_height: u64, min_confirmations: u64) -> Option<u64> {
    let needed = min_confirmations.max(1);
    chain_height.saturating_add(1).checked_sub(needed)
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

        state.save(&path).unwrap();

        let loaded = SyncState::load(&path).unwrap();
        assert_eq!(loaded.last_scanned_height, 12345);
        assert_eq!(loaded.total_deposits, 42);
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

        state.update_after_batch(100, 5, 1000000, 500);

        assert_eq!(state.last_scanned_height, 100);
        assert_eq!(state.total_deposits, 5);
        assert_eq!(state.total_amount, 1000000);
        assert_eq!(state.total_outputs_scanned, 500);

        state.update_after_batch(200, 3, 500000, 300);

        assert_eq!(state.last_scanned_height, 200);
        assert_eq!(state.total_deposits, 8);
        assert_eq!(state.total_amount, 1500000);
    }

    #[test]
    fn test_next_scan_height() {
        let mut state = SyncState::default();
        assert_eq!(state.next_scan_height(), 0);

        state.last_scanned_height = 100;
        assert_eq!(state.next_scan_height(), 101);
    }

    /// Scanning block 0 must not leave the checkpoint where block 0 is
    /// scanned again (and its deposits re-credited) on the next pass.
    #[test]
    fn genesis_is_not_rescanned_after_being_scanned() {
        let mut state = SyncState::default();
        assert_eq!(state.next_scan_height(), 0);
        state.update_after_batch(0, 0, 0, 0);
        assert_eq!(state.next_scan_height(), 1);
    }

    #[test]
    fn legacy_state_without_scanned_any_loads() {
        let state: SyncState = serde_json::from_str(
            r#"{"last_scanned_height":7,"last_scanned_hash":"","last_sync_timestamp":0,
                "total_deposits":0,"total_amount":0,"total_outputs_scanned":0}"#,
        )
        .unwrap();
        assert!(!state.scanned_any);
        assert_eq!(state.next_scan_height(), 8);
    }

    #[test]
    fn set_start_height_positions_next_scan() {
        let mut state = SyncState::default();
        state.update_after_batch(50, 0, 0, 0);
        state.set_start_height(0);
        assert_eq!(state.next_scan_height(), 0);
        state.set_start_height(1);
        assert_eq!(state.next_scan_height(), 1);
        state.set_start_height(42);
        assert_eq!(state.next_scan_height(), 42);
    }

    #[test]
    fn confirmed_tip_counts_the_tip_block_as_one_confirmation() {
        // Chain at 100, need 10: blocks 0..=91 have >= 10 confirmations.
        assert_eq!(confirmed_tip(100, 10), Some(91));
        assert_eq!(confirmed_tip(100, 1), Some(100));
        assert_eq!(confirmed_tip(100, 0), Some(100));
        assert_eq!(confirmed_tip(9, 10), Some(0));
        assert_eq!(confirmed_tip(8, 10), None);
        assert_eq!(confirmed_tip(0, 1), Some(0));
    }

    #[test]
    fn confirmed_tip_does_not_overflow_at_u64_max() {
        // Saturates rather than panicking/wrapping; an unreachable height
        // is clamped, so the result is conservative (never above the tip).
        assert_eq!(confirmed_tip(u64::MAX, 1), Some(u64::MAX - 1));
        assert_eq!(confirmed_tip(u64::MAX, 0), Some(u64::MAX - 1));
        assert_eq!(confirmed_tip(u64::MAX, 10), Some(u64::MAX - 10));
    }

    /// The stored-hash field was dropped (#1792); state files that still
    /// carry `last_scanned_hash` must keep loading and round-trip cleanly.
    #[test]
    fn legacy_state_with_last_scanned_hash_still_loads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"last_scanned_height":7,"scanned_any":true,"last_scanned_hash":"deadbeef",
                "last_sync_timestamp":0,"total_deposits":1,"total_amount":2,
                "total_outputs_scanned":3}"#,
        )
        .unwrap();
        let state = SyncState::load(&path).unwrap();
        assert_eq!(state.last_scanned_height, 7);
        state.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("last_scanned_hash"));
    }

    #[test]
    fn plan_batch_stops_at_the_confirmed_tip() {
        let mut state = SyncState::default();
        // Nothing has 10 confirmations yet.
        assert_eq!(state.plan_batch(5, 10, 100), None);
        // Chain at 50: only blocks 0..=41 are confirmed.
        assert_eq!(state.plan_batch(50, 10, 100), Some((0, 41)));
        state.update_after_batch(41, 0, 0, 0);
        // No new confirmed blocks until the chain grows.
        assert_eq!(state.plan_batch(50, 10, 100), None);
        assert_eq!(state.plan_batch(53, 10, 100), Some((42, 44)));
    }

    #[test]
    fn plan_batch_respects_the_batch_cap() {
        let mut state = SyncState::default();
        state.update_after_batch(99, 0, 0, 0);
        assert_eq!(state.plan_batch(10_000, 10, 1000), Some((100, 1099)));
        // A zero batch size still makes progress one block at a time.
        assert_eq!(state.plan_batch(10_000, 10, 0), Some((100, 100)));
    }
}
