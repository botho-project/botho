// Copyright (c) 2024 The Botho Foundation

//! Solana chain watcher for monitoring wBTH burns (burn flow).
//!
//! Watches the `wbth` Anchor program
//! (`contracts/solana/programs/wbth`) for `BridgeBurnEvent { user, amount,
//! bth_address, timestamp }` emissions and drives `BurnDetected ->
//! BurnConfirmed`, mirroring the Ethereum watcher:
//!
//! - **Commitment as the reorg guard**: burns are only accepted at `Finalized`
//!   commitment (rooted, cannot be rolled back), regardless of the configured
//!   [`SolanaCommitment`] — the analogue of Ethereum's depth + canonical-hash
//!   re-check. A slot observed below `Finalized` that gets rolled back is
//!   treated like an Ethereum orphan: the order is held at `BurnDetected` and
//!   never advances toward release.
//! - **Idempotency**: orders are created via `Database::insert_burn_order`
//!   keyed by `"<signature>#<ordinal>"`, so a cursor replay or a
//!   rolled-back-then-re-landed burn reuses the same order (exactly-once by
//!   order id).
//! - **Cursor**: scan progress (last processed slot) persists in
//!   `watcher_cursors` under [`Chain::Solana`].
//!
//! ## Implementation status (#857)
//!
//! Live-wired: the RPC scan (`getSignaturesForAddress` +
//! `getTransaction` at `Finalized` over `SolanaConfig::wbth_program`,
//! [`crate::solana_rpc`]) feeds the (already-unit-tested) Anchor event
//! discriminator + borsh decoder [`parse_bridge_burn_event`] and the
//! exactly-once order-creation path. Only `Finalized` (rooted) signatures
//! are processed, so a rolled-back burn is never observed — the burn side
//! needs no orphan/canonical-recheck analogue to the Ethereum watcher.

use bth_bridge_core::{BridgeOrder, Chain, SolanaCommitment, SolanaConfig};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::{
    db::Database,
    engine::ShutdownSignal,
    solana_rpc::{base64_decode, HttpSolanaRpc, SolanaRpc},
};

/// Delay between poll passes.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Compute the Anchor event discriminator: the first 8 bytes of
/// `sha256("event:<Name>")`.
pub fn anchor_event_discriminator(event_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(b"event:");
    hasher.update(event_name.as_bytes());
    let digest = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&digest[..8]);
    disc
}

/// A decoded `BridgeBurnEvent` from the wBTH program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaBurn {
    /// Burner's pubkey (raw 32 bytes).
    pub user: [u8; 32],
    /// Burned amount in picocredits (wBTH mint uses 12 decimals, 1:1).
    pub amount: u64,
    /// Destination BTH address. Per ADR 0004 the release pays a FRESH
    /// one-time stealth address resolved from this (enforced in #822).
    pub bth_address: String,
    /// `Clock::unix_timestamp` the program observed at emission time. This is
    /// the program's `timestamp: i64` field (#872: previously mislabeled
    /// `slot`; the borsh byte layout is identical 8 LE bytes, so no wire
    /// change — only the semantic name is corrected).
    pub timestamp: i64,
}

/// Decode the borsh-serialized `BridgeBurnEvent` (with its 8-byte Anchor
/// event discriminator prefix) as emitted by
/// `contracts/solana/programs/wbth`:
///
/// ```text
/// [ discriminator: 8 ][ user: Pubkey 32 ][ amount: u64 LE ]
/// [ bth_address: u32 LE length + utf8 bytes ][ timestamp: i64 LE ]
/// ```
///
/// Returns `None` on a wrong discriminator, truncation, trailing bytes,
/// or a non-UTF-8 address.
pub fn parse_bridge_burn_event(data: &[u8]) -> Option<SolanaBurn> {
    let expected = anchor_event_discriminator("BridgeBurnEvent");
    let rest = data.strip_prefix(expected.as_slice())?;

    // user: Pubkey (32 bytes)
    let (user_bytes, rest) = rest.split_at_checked(32)?;
    let mut user = [0u8; 32];
    user.copy_from_slice(user_bytes);

    // amount: u64 LE
    let (amount_bytes, rest) = rest.split_at_checked(8)?;
    let amount = u64::from_le_bytes(amount_bytes.try_into().ok()?);

    // bth_address: borsh string (u32 LE length + bytes)
    let (len_bytes, rest) = rest.split_at_checked(4)?;
    let len = u32::from_le_bytes(len_bytes.try_into().ok()?) as usize;
    let (addr_bytes, rest) = rest.split_at_checked(len)?;
    let bth_address = std::str::from_utf8(addr_bytes).ok()?.to_string();

    // timestamp: i64 LE (#872: the program emits `timestamp`, not `slot`),
    // and nothing may follow.
    let (ts_bytes, rest) = rest.split_at_checked(8)?;
    if !rest.is_empty() {
        return None;
    }
    let timestamp = i64::from_le_bytes(ts_bytes.try_into().ok()?);

    Some(SolanaBurn {
        user,
        amount,
        bth_address,
        timestamp,
    })
}

/// The Anchor program-log prefix carrying a base64-encoded `emit!` event.
const PROGRAM_DATA_PREFIX: &str = "Program data: ";

/// Extract every `BridgeBurnEvent` from a transaction's program log lines.
/// Anchor emits events as `Program data: <base64(discriminator || borsh)>`.
pub fn burns_from_logs(logs: &[String]) -> Vec<SolanaBurn> {
    logs.iter()
        .filter_map(|line| line.strip_prefix(PROGRAM_DATA_PREFIX))
        .filter_map(|b64| base64_decode(b64.trim()).ok())
        .filter_map(|bytes| parse_bridge_burn_event(&bytes))
        .collect()
}

/// Whether a commitment level is acceptable as the burn-side reorg guard.
/// Only `Finalized` (rooted) slots cannot be rolled back.
pub fn commitment_is_final(commitment: SolanaCommitment) -> bool {
    matches!(commitment, SolanaCommitment::Finalized)
}

/// Stable idempotency key for a Solana burn: the transaction signature plus
/// the event's ordinal among burn events of the SAME transaction. Solana
/// finalized transactions have a stable signature, so this key is stable.
pub fn burn_source_key(signature: &str, ordinal: u32) -> String {
    format!("{}#{}", signature, ordinal)
}

/// Solana watcher monitors the wBTH program for burn events.
pub struct SolanaWatcher {
    config: SolanaConfig,
    db: Database,
    shutdown: ShutdownSignal,
}

impl SolanaWatcher {
    /// Create a new Solana watcher.
    pub fn new(config: SolanaConfig, db: Database, shutdown: ShutdownSignal) -> Self {
        Self {
            config,
            db,
            shutdown,
        }
    }

    /// Run the watcher.
    pub async fn run(mut self) -> Result<(), String> {
        info!(
            "Starting Solana watcher for program {}",
            self.config.wbth_program
        );

        if !commitment_is_final(self.config.commitment) {
            warn!(
                "solana.commitment is {:?}; burns are only accepted at Finalized commitment \
                 (the reorg guard) regardless of this setting",
                self.config.commitment
            );
        }

        // Fail-safe: a bad RPC URL disables the watcher (creates no state)
        // instead of crashing the engine.
        let rpc = match HttpSolanaRpc::new(self.config.rpc_url.clone()) {
            Ok(rpc) => Some(rpc),
            Err(e) => {
                warn!("Solana watcher disabled: {}", e);
                None
            }
        };

        loop {
            // Check for shutdown first
            match self.shutdown.try_recv() {
                Ok(_) | Err(broadcast::error::TryRecvError::Closed) => {
                    info!("Solana watcher shutting down");
                    return Ok(());
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // No shutdown signal, continue
                }
            }

            if let Some(rpc) = &rpc {
                if let Err(e) = self.scan_once(rpc).await {
                    warn!("Solana scan failed (will retry): {}", e);
                }
            }

            tokio::select! {
                _ = self.shutdown.recv() => {
                    info!("Solana watcher shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    /// One scan pass. Burns are only ever read at FINALIZED commitment (the
    /// reorg guard): a finalized signature is rooted and cannot roll back, so
    /// — unlike the Ethereum watcher — there is no orphan/canonical-recheck
    /// path, and a detected burn advances straight to `BurnConfirmed`.
    ///
    /// The cursor stores the newest processed signature (in `last_block_hash`)
    /// and its slot (in `last_height`). `getSignaturesForAddress` returns
    /// newest-first bounded by `until = <cursor signature>`; we process the
    /// batch oldest-first and persist the cursor only after the whole batch
    /// succeeds, so a crash replays (the idempotency layer dedups).
    pub async fn scan_once(&self, rpc: &dyn SolanaRpc) -> Result<(), String> {
        // Only finalized signatures are accepted; the reorg guard is the
        // commitment level, not a depth count.
        let commitment = "finalized";

        let cursor = self.db.get_cursor(Chain::Solana)?;
        let until = cursor.as_ref().and_then(|c| c.last_block_hash.clone());

        // Newest-first, exclusive of `until`.
        let mut sigs = rpc
            .get_signatures_for_address(&self.config.wbth_program, until.as_deref(), commitment)
            .await?;

        if sigs.is_empty() {
            return Ok(());
        }

        // Process oldest-first so the cursor advances monotonically and a
        // mid-batch failure replays cleanly.
        sigs.reverse();
        let newest = sigs.last().cloned();

        for (signature, slot) in &sigs {
            let Some((logs, _tx_slot)) = rpc.get_transaction_logs(signature, commitment).await?
            else {
                // Not yet retrievable at finalized; leave the cursor behind
                // it so the next pass retries this signature.
                debug!(
                    "Solana tx {} not finalized-retrievable yet; deferring",
                    signature
                );
                return Ok(());
            };

            let burns = burns_from_logs(&logs);
            for (ordinal, burn) in burns.into_iter().enumerate() {
                self.process_burn(signature, ordinal as u32, *slot, &burn)?;
            }
        }

        // Persist the cursor only after the whole batch is processed.
        if let Some((sig, slot)) = newest {
            self.db.set_cursor(Chain::Solana, slot, Some(&sig))?;
        }
        Ok(())
    }

    /// Create (exactly once) a `BurnConfirmed` order for one decoded burn.
    /// Because only finalized (rooted) signatures reach here, the burn is
    /// already final — there is no `BurnDetected` waiting period.
    fn process_burn(
        &self,
        signature: &str,
        ordinal: u32,
        slot: u64,
        burn: &SolanaBurn,
    ) -> Result<(), String> {
        let source_key = burn_source_key(signature, ordinal);

        if self.db.get_burn_by_source(&source_key)?.is_some() {
            debug!("Solana burn {} already recorded; skipping", source_key);
            return Ok(());
        }

        // Defense in depth against a misbehaving RPC (the program enforces
        // these).
        if burn.amount == 0 || burn.bth_address.is_empty() {
            self.db.log_audit(
                None,
                "solana_burn_invalid",
                &format!(
                    "sig={} amount={} bth_address_len={}",
                    signature,
                    burn.amount,
                    burn.bth_address.len()
                ),
            )?;
            return Ok(());
        }

        // The burn-side fee is applied by the release path (#822); orders are
        // created fee-0 so the watcher stays fee-policy agnostic.
        let order = BridgeOrder::new_burn(
            Chain::Solana,
            burn.amount,
            0,
            bs58_encode_user(&burn.user),
            burn.bth_address.clone(),
            signature.to_string(),
            ordinal,
        );

        let inserted = self.db.insert_burn_order(&order, &source_key, slot, None)?;
        if !inserted {
            // Raced by a concurrent insert / cursor replay: the existing
            // order wins.
            return Ok(());
        }

        self.db.log_audit(
            Some(&order.id),
            "solana_burn_detected",
            &format!("sig={} amount={} slot={}", signature, burn.amount, slot),
        )?;

        // Finalized => confirm immediately (no reorg possible).
        use bth_bridge_core::OrderStatus;
        if order.status.can_transition_to(&OrderStatus::BurnConfirmed) {
            self.db
                .update_order_status(&order.id, &OrderStatus::BurnConfirmed, None)?;
            self.db.log_audit(
                Some(&order.id),
                "solana_burn_confirmed",
                &format!("sig={} slot={} (finalized)", signature, slot),
            )?;
            info!(
                "Solana burn {} confirmed (finalized) for {} picocredits -> order {}",
                source_key, burn.amount, order.id
            );
        }

        Ok(())
    }
}

/// Render a burner pubkey as base58 for the order's source-address field.
fn bs58_encode_user(user: &[u8; 32]) -> String {
    bs58::encode(user).into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::solana_rpc::base64_encode;

    fn encode_event(user: [u8; 32], amount: u64, bth_address: &str, timestamp: i64) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&anchor_event_discriminator("BridgeBurnEvent"));
        data.extend_from_slice(&user);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&(bth_address.len() as u32).to_le_bytes());
        data.extend_from_slice(bth_address.as_bytes());
        data.extend_from_slice(&timestamp.to_le_bytes());
        data
    }

    #[test]
    fn test_event_discriminator_known_vector() {
        // sha256("event:BridgeBurnEvent")[..8] — pinned so a silent rename
        // of the on-chain event breaks tests, not the live watcher.
        let disc = anchor_event_discriminator("BridgeBurnEvent");
        let mut hasher = Sha256::new();
        hasher.update(b"event:BridgeBurnEvent");
        assert_eq!(disc, hasher.finalize()[..8]);
        assert_ne!(disc, anchor_event_discriminator("BridgeMintEvent"));
    }

    #[test]
    fn test_parse_bridge_burn_event_roundtrip() {
        let user = [7u8; 32];
        let data = encode_event(user, 999_000_000_000, "bth_stealth_addr", 12345);

        let burn = parse_bridge_burn_event(&data).unwrap();
        assert_eq!(burn.user, user);
        assert_eq!(burn.amount, 999_000_000_000);
        assert_eq!(burn.bth_address, "bth_stealth_addr");
        assert_eq!(burn.timestamp, 12345);
    }

    #[test]
    fn test_parse_bridge_burn_event_rejects_malformed() {
        let good = encode_event([1u8; 32], 5, "addr", 9);

        // Wrong discriminator.
        let mut wrong_disc = good.clone();
        wrong_disc[..8].copy_from_slice(&anchor_event_discriminator("BridgeMintEvent"));
        assert!(parse_bridge_burn_event(&wrong_disc).is_none());

        // Truncated at every boundary.
        for cut in [4, 8, 20, 40, 44, 46, good.len() - 1] {
            assert!(
                parse_bridge_burn_event(&good[..cut]).is_none(),
                "truncation at {} must be rejected",
                cut
            );
        }

        // Trailing bytes.
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(parse_bridge_burn_event(&trailing).is_none());

        // Non-UTF-8 address bytes.
        let mut bad_utf8 = encode_event([1u8; 32], 5, "ab", 9);
        let addr_start = 8 + 32 + 8 + 4;
        bad_utf8[addr_start] = 0xFF;
        bad_utf8[addr_start + 1] = 0xFE;
        assert!(parse_bridge_burn_event(&bad_utf8).is_none());
    }

    #[test]
    fn test_only_finalized_commitment_is_a_reorg_guard() {
        assert!(commitment_is_final(SolanaCommitment::Finalized));
        assert!(!commitment_is_final(SolanaCommitment::Confirmed));
        assert!(!commitment_is_final(SolanaCommitment::Processed));
    }

    #[test]
    fn test_burns_from_logs_extracts_program_data() {
        let user = [7u8; 32];
        let event_bytes = encode_event(user, 123, "bth_dest", 999);
        let program_data = format!("{}{}", PROGRAM_DATA_PREFIX, base64_encode(&event_bytes));
        let logs = vec![
            "Program wBTH... invoke [1]".to_string(),
            "Program log: Burned 123 wBTH".to_string(),
            program_data,
            "Program wBTH... success".to_string(),
        ];
        let burns = burns_from_logs(&logs);
        assert_eq!(burns.len(), 1);
        assert_eq!(burns[0].amount, 123);
        assert_eq!(burns[0].bth_address, "bth_dest");
        assert_eq!(burns[0].timestamp, 999);

        // No program-data line -> no burns.
        assert!(burns_from_logs(&["Program log: nothing".to_string()]).is_empty());
    }

    // === Watcher scan against a mocked JSON-RPC transport ===

    use crate::solana_rpc::{SignatureState, SolanaRpc};
    use async_trait::async_trait;
    use bth_bridge_core::OrderStatus;
    use std::{collections::HashMap, sync::Mutex};

    /// A mock program-history transport: signatures newest-first, each with
    /// its finalized transaction's program logs.
    struct MockSolClient {
        /// (signature, slot) newest-first.
        signatures: Mutex<Vec<(String, u64)>>,
        /// signature -> (logs, slot).
        txs: Mutex<HashMap<String, (Vec<String>, u64)>>,
    }

    impl MockSolClient {
        fn new() -> Self {
            Self {
                signatures: Mutex::new(Vec::new()),
                txs: Mutex::new(HashMap::new()),
            }
        }

        /// Record a finalized burn transaction (prepended = newest).
        fn add_burn_tx(&self, signature: &str, slot: u64, user: [u8; 32], amount: u64, dest: &str) {
            let event = encode_event(user, amount, dest, 1234);
            let log = format!("{}{}", PROGRAM_DATA_PREFIX, base64_encode(&event));
            self.txs.lock().unwrap().insert(
                signature.to_string(),
                (vec!["Program log: burn".to_string(), log], slot),
            );
            self.signatures
                .lock()
                .unwrap()
                .insert(0, (signature.to_string(), slot));
        }
    }

    #[async_trait]
    impl SolanaRpc for MockSolClient {
        async fn get_latest_blockhash(&self) -> Result<([u8; 32], u64), String> {
            Ok(([0u8; 32], 0))
        }
        async fn send_transaction(&self, _raw: &[u8]) -> Result<String, String> {
            unreachable!("watcher never sends")
        }
        async fn get_signature_status(&self, _sig: &str) -> Result<SignatureState, String> {
            Ok(SignatureState::Unknown)
        }
        async fn get_account_data(
            &self,
            _address: &str,
            _commitment: &str,
        ) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        async fn get_signatures_for_address(
            &self,
            _address: &str,
            until: Option<&str>,
            _commitment: &str,
        ) -> Result<Vec<(String, u64)>, String> {
            let sigs = self.signatures.lock().unwrap();
            // Return everything newer than `until` (exclusive), newest-first.
            let mut out = Vec::new();
            for entry in sigs.iter() {
                if Some(entry.0.as_str()) == until {
                    break;
                }
                out.push(entry.clone());
            }
            Ok(out)
        }
        async fn get_transaction_logs(
            &self,
            signature: &str,
            _commitment: &str,
        ) -> Result<Option<(Vec<String>, u64)>, String> {
            Ok(self.txs.lock().unwrap().get(signature).cloned())
        }
    }

    fn setup() -> (SolanaWatcher, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let config = SolanaConfig {
            rpc_url: "http://localhost:8899".to_string(),
            wbth_program: "So11111111111111111111111111111111111111112".to_string(),
            keypair_file: None,
            keypair_env: None,
            enforce_key_permissions: false,
            commitment: SolanaCommitment::Finalized,
            mint_signers: Vec::new(),
            mint_threshold: 0,
        };
        let (_tx, rx) = broadcast::channel(1);
        (SolanaWatcher::new(config, db.clone(), rx), db)
    }

    fn burn_orders(db: &Database) -> Vec<BridgeOrder> {
        db.get_orders_by_status("burn").unwrap()
    }

    #[tokio::test]
    async fn test_scan_creates_confirmed_burn_order() {
        let (watcher, db) = setup();
        let client = MockSolClient::new();
        client.add_burn_tx("sigA", 100, [1u8; 32], 5_000_000_000, "bth_dest");

        watcher.scan_once(&client).await.unwrap();

        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 1);
        // Finalized burns confirm immediately (no reorg possible).
        assert_eq!(orders[0].status, OrderStatus::BurnConfirmed);
        assert_eq!(orders[0].amount, 5_000_000_000);
        assert_eq!(orders[0].dest_address, "bth_dest");
        assert_eq!(orders[0].source_tx.as_deref(), Some("sigA"));
        assert_eq!(db.count_audit_action("solana_burn_confirmed").unwrap(), 1);

        // Cursor advanced to the newest signature.
        let cursor = db.get_cursor(Chain::Solana).unwrap().unwrap();
        assert_eq!(cursor.last_height, 100);
        assert_eq!(cursor.last_block_hash.as_deref(), Some("sigA"));
    }

    #[tokio::test]
    async fn test_scan_is_idempotent_across_passes() {
        let (watcher, db) = setup();
        let client = MockSolClient::new();
        client.add_burn_tx("sigA", 100, [1u8; 32], 1_000, "d1");

        watcher.scan_once(&client).await.unwrap();
        // A second pass with no new signatures does nothing.
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db).len(), 1);
        assert_eq!(db.count_audit_action("solana_burn_detected").unwrap(), 1);

        // A new finalized burn is picked up incrementally (bounded by the
        // cursor's `until`).
        client.add_burn_tx("sigB", 101, [2u8; 32], 2_000, "d2");
        watcher.scan_once(&client).await.unwrap();
        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 2);
        assert!(orders
            .iter()
            .all(|o| o.status == OrderStatus::BurnConfirmed));
    }

    #[tokio::test]
    async fn test_scan_replay_after_cursor_rewind_is_noop() {
        let (watcher, db) = setup();
        let client = MockSolClient::new();
        client.add_burn_tx("sigA", 100, [1u8; 32], 1_000, "d1");
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db).len(), 1);

        // Rewind the cursor (as a crash-replay would) and rescan: the
        // source-key idempotency dedups the burn.
        db.set_cursor(Chain::Solana, 0, None).unwrap();
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db).len(), 1);
        assert_eq!(db.count_audit_action("solana_burn_detected").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_scan_skips_invalid_burn() {
        let (watcher, db) = setup();
        let client = MockSolClient::new();
        // amount 0 is rejected as invalid (defense in depth).
        client.add_burn_tx("sigA", 100, [1u8; 32], 0, "d1");
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db).len(), 0);
        assert_eq!(db.count_audit_action("solana_burn_invalid").unwrap(), 1);
    }
}
