// Copyright (c) 2024 The Botho Foundation

//! Ethereum chain watcher for monitoring wBTH burns (burn flow).
//!
//! Scans the `WrappedBTH` contract for `BridgeBurn(from, amount,
//! bthAddress)` events and drives `BurnDetected -> BurnConfirmed`:
//!
//! - **Detection** runs up to the tip: each new burn atomically creates a
//!   `BridgeOrder::new_burn` at `BurnDetected` plus its `processed_burns`
//!   idempotency row (keyed by `"<tx_hash>#<ordinal>"`, stable across reorgs),
//!   so a rescan or reorg re-add can never create a second order.
//! - **Confirmation** requires `confirmations_required` blocks of depth AND a
//!   canonical-hash re-check of the burn's block (same pattern as
//!   `mint::ethereum::check_confirmation`): an orphaned burn is flagged and
//!   never advances toward `ReleasePending`/`Released`; if the burn is
//!   re-included, the existing record is relocated and confirms once.
//! - **Cursor reorg safety**: the persisted cursor stores the canonical hash of
//!   the last scanned block. If that hash is no longer canonical the cursor
//!   rolls back by the confirmation window (persisted BEFORE re-scanning) and
//!   the range is replayed; the idempotency layer deduplicates the replay. A
//!   reorg deeper than `confirmations_required` is outside the safety
//!   assumption the confirmation requirement itself already makes.

use alloy::{
    eips::BlockNumberOrTag,
    primitives::Address,
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::{Filter, Log},
    sol,
    sol_types::SolEvent,
};
use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, Chain, EthereumConfig, OrderStatus};
use std::{collections::HashMap, time::Duration};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use super::WatchError;
use crate::{db::Database, engine::ShutdownSignal};

sol! {
    /// Burn event of the wBTH token
    /// (`contracts/ethereum/contracts/WrappedBTH.sol`).
    #[allow(missing_docs)]
    interface IWrappedBTHEvents {
        event BridgeBurn(address indexed from, uint256 amount, string bthAddress);
    }
}

/// Delay between scan passes.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// A decoded `BridgeBurn` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnEvent {
    /// Transaction hash the burn was emitted in (0x-prefixed).
    pub tx_hash: String,
    /// Absolute log index within the block (used only to order events of
    /// the same transaction; NOT part of the idempotency key, because it
    /// shifts when other transactions move in a reorg).
    pub log_index: u64,
    /// Block number the burn was observed in.
    pub block_number: u64,
    /// Block hash the burn was observed in (0x-prefixed).
    pub block_hash: String,
    /// Burner address (0x-prefixed).
    pub from: String,
    /// Burned amount in picocredits (wBTH has 12 decimals, 1:1 with BTH).
    pub amount: u64,
    /// Destination BTH address. Per ADR 0004 the release pays a FRESH
    /// one-time stealth address resolved from this (enforced in #822).
    pub bth_address: String,
}

/// Stable idempotency key for a burn: the source tx hash plus the event's
/// ordinal among burn events of the SAME transaction. Relative intra-tx
/// event order is deterministic, so the key survives re-inclusion in a
/// different block (unlike the absolute log index).
pub fn burn_source_key(tx_hash: &str, ordinal: u32) -> String {
    format!("{}#{}", tx_hash, ordinal)
}

/// Pair each event with its per-transaction ordinal, ordered by
/// (block, log index) so processing and key assignment are deterministic.
pub fn with_tx_ordinals(mut events: Vec<BurnEvent>) -> Vec<(BurnEvent, u32)> {
    events.sort_by(|a, b| (a.block_number, a.log_index).cmp(&(b.block_number, b.log_index)));
    let mut per_tx: HashMap<String, u32> = HashMap::new();
    events
        .into_iter()
        .map(|event| {
            let counter = per_tx.entry(event.tx_hash.clone()).or_insert(0);
            let ordinal = *counter;
            *counter += 1;
            (event, ordinal)
        })
        .collect()
}

/// Decode an RPC log into a [`BurnEvent`]. Returns `None` for logs that do
/// not decode as `BridgeBurn`, are pending (missing block/tx metadata), or
/// carry an amount that cannot be a real BTH quantity (> u64 picocredits).
pub fn decode_burn_log(log: &Log) -> Option<BurnEvent> {
    let decoded = IWrappedBTHEvents::BridgeBurn::decode_log(&log.inner).ok()?;
    let amount: u64 = decoded.data.amount.try_into().ok()?;
    Some(BurnEvent {
        tx_hash: format!("{:#x}", log.transaction_hash?),
        log_index: log.log_index?,
        block_number: log.block_number?,
        block_hash: format!("{:#x}", log.block_hash?),
        from: format!("{:#x}", decoded.data.from),
        amount,
        bth_address: decoded.data.bthAddress.clone(),
    })
}

/// Read access to the Ethereum chain, mockable for tests.
#[async_trait]
pub trait EthChainClient: Send + Sync {
    /// Latest (tip) block number.
    async fn latest_block(&self) -> Result<u64, WatchError>;

    /// Canonical block hash at `number` (0x-prefixed), or `None` if the
    /// canonical chain does not (or no longer does) reach that height.
    async fn block_hash_at(&self, number: u64) -> Result<Option<String>, WatchError>;

    /// All `BridgeBurn` events of the wBTH contract in `[from, to]`
    /// (canonical chain only).
    async fn burn_events(&self, from: u64, to: u64) -> Result<Vec<BurnEvent>, WatchError>;
}

/// Live transport via alloy (same provider pattern as `mint::ethereum`).
pub struct AlloyEthClient {
    provider: DynProvider,
    wbth: Address,
}

impl AlloyEthClient {
    /// Build a client from configuration. Does not perform network I/O.
    pub fn new(config: &EthereumConfig) -> Result<Self, WatchError> {
        let wbth: Address = config
            .wbth_contract
            .parse()
            .map_err(|e| WatchError::Config(format!("invalid wbth_contract: {}", e)))?;
        let url = config
            .rpc_url
            .parse()
            .map_err(|e| WatchError::Config(format!("invalid ethereum rpc_url: {}", e)))?;
        let provider = ProviderBuilder::new().connect_http(url).erased();
        Ok(Self { provider, wbth })
    }
}

#[async_trait]
impl EthChainClient for AlloyEthClient {
    async fn latest_block(&self) -> Result<u64, WatchError> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| WatchError::Rpc(format!("get_block_number failed: {}", e)))
    }

    async fn block_hash_at(&self, number: u64) -> Result<Option<String>, WatchError> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| WatchError::Rpc(format!("get_block_by_number failed: {}", e)))?;
        Ok(block.map(|b| format!("{:#x}", b.header.hash)))
    }

    async fn burn_events(&self, from: u64, to: u64) -> Result<Vec<BurnEvent>, WatchError> {
        let filter = Filter::new()
            .address(self.wbth)
            .event_signature(IWrappedBTHEvents::BridgeBurn::SIGNATURE_HASH)
            .from_block(from)
            .to_block(to);

        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|e| WatchError::Rpc(format!("get_logs failed: {}", e)))?;

        let mut events = Vec::with_capacity(logs.len());
        for log in &logs {
            match decode_burn_log(log) {
                Some(event) => events.push(event),
                None => warn!(
                    "Skipping undecodable/overflowing BridgeBurn log in tx {:?}",
                    log.transaction_hash
                ),
            }
        }
        Ok(events)
    }
}

/// Ethereum watcher monitors the wBTH contract for burn events.
pub struct EthereumWatcher {
    config: EthereumConfig,
    db: Database,
    shutdown: ShutdownSignal,
}

impl EthereumWatcher {
    /// Create a new Ethereum watcher.
    pub fn new(config: EthereumConfig, db: Database, shutdown: ShutdownSignal) -> Self {
        Self {
            config,
            db,
            shutdown,
        }
    }

    /// Run the watcher.
    pub async fn run(mut self) -> Result<(), String> {
        info!(
            "Starting Ethereum watcher for contract {}",
            self.config.wbth_contract
        );

        // Fail-safe: a misconfigured contract/RPC disables the watcher
        // (no orders are created) instead of crashing the engine.
        let client = match AlloyEthClient::new(&self.config) {
            Ok(client) => Some(client),
            Err(e) => {
                warn!("Ethereum watcher disabled: {}", e);
                None
            }
        };

        loop {
            // Check for shutdown first
            match self.shutdown.try_recv() {
                Ok(_) | Err(broadcast::error::TryRecvError::Closed) => {
                    info!("Ethereum watcher shutting down");
                    return Ok(());
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // No shutdown signal, continue
                }
            }

            if let Some(client) = &client {
                if let Err(e) = self.scan_once(client).await {
                    warn!("Ethereum scan failed (will retry): {}", e);
                }
            }

            tokio::select! {
                _ = self.shutdown.recv() => {
                    info!("Ethereum watcher shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    /// The window rolled back on a detected reorg. Reorgs deeper than the
    /// confirmation requirement are outside the bridge's safety model.
    fn reorg_window(&self) -> u64 {
        (self.config.confirmations_required as u64).max(1)
    }

    /// One scan pass: cursor integrity check, burn detection up to the
    /// tip, then depth + canonical-hash confirmation.
    pub async fn scan_once(&self, client: &dyn EthChainClient) -> Result<(), WatchError> {
        let tip = client.latest_block().await?;

        // 1. Cursor integrity: if the last scanned block was reorged out, roll back
        //    (persisting the rolled-back cursor BEFORE the re-scan) and replay;
        //    idempotency dedups already-seen burns.
        let from = match self
            .db
            .get_cursor(Chain::Ethereum)
            .map_err(WatchError::Db)?
        {
            Some(cursor) => {
                let canonical = client.block_hash_at(cursor.last_height).await?;
                let reorged = match (&cursor.last_block_hash, &canonical) {
                    (Some(stored), Some(now)) => stored != now,
                    // Canonical chain no longer reaches the cursor height.
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                if reorged {
                    let rollback_to = cursor.last_height.saturating_sub(self.reorg_window());
                    let rollback_hash = client.block_hash_at(rollback_to).await?;
                    self.db
                        .set_cursor(Chain::Ethereum, rollback_to, rollback_hash.as_deref())
                        .map_err(WatchError::Db)?;
                    self.db
                        .log_audit(
                            None,
                            "eth_cursor_reorg",
                            &format!(
                                "cursor height {} hash {:?} no longer canonical ({:?}); \
                                 rolled back to {}",
                                cursor.last_height, cursor.last_block_hash, canonical, rollback_to
                            ),
                        )
                        .map_err(WatchError::Db)?;
                    warn!(
                        "Ethereum reorg at/behind cursor height {}; re-scanning from {}",
                        cursor.last_height,
                        rollback_to + 1
                    );
                    rollback_to + 1
                } else {
                    cursor.last_height + 1
                }
            }
            // First run: cover the current unfinalized window. Operators
            // backfilling history can seed the watcher_cursors row.
            None => tip.saturating_sub(self.config.confirmations_required as u64),
        };

        // 2. Detection up to the tip: burns enter at BurnDetected.
        if from <= tip {
            let events = client.burn_events(from, tip).await?;
            for (event, ordinal) in with_tx_ordinals(events) {
                self.process_burn(&event, ordinal)?;
            }
            let tip_hash = client.block_hash_at(tip).await?;
            self.db
                .set_cursor(Chain::Ethereum, tip, tip_hash.as_deref())
                .map_err(WatchError::Db)?;
        }

        // 3. Confirmation: depth + canonical re-check.
        self.confirm_detected_burns(client, tip).await
    }

    /// Handle one detected burn event: exactly-once order creation, or —
    /// for a burn already on record (rescan / reorg re-add) — relocation
    /// of the existing record, idempotent by its order id.
    fn process_burn(&self, event: &BurnEvent, ordinal: u32) -> Result<(), WatchError> {
        let source_key = burn_source_key(&event.tx_hash, ordinal);

        if let Some(existing) = self
            .db
            .get_burn_by_source(&source_key)
            .map_err(WatchError::Db)?
        {
            let moved = existing.block_number != event.block_number
                || existing.block_hash.as_deref() != Some(event.block_hash.as_str());
            if moved || existing.orphaned {
                // Reorg re-add: same burn, new canonical location. The
                // existing order is reused — processed exactly once.
                self.db
                    .update_burn_location(&source_key, event.block_number, Some(&event.block_hash))
                    .map_err(WatchError::Db)?;
                self.db
                    .log_audit(
                        Some(&existing.order_id),
                        "burn_relocated",
                        &format!(
                            "tx={} moved to block {} ({})",
                            event.tx_hash, event.block_number, event.block_hash
                        ),
                    )
                    .map_err(WatchError::Db)?;
                info!(
                    "Burn {} re-observed at block {}; order {} relocated",
                    source_key, event.block_number, existing.order_id
                );
            } else {
                debug!("Burn {} already recorded; skipping", source_key);
            }
            return Ok(());
        }

        // The contract enforces these; defense in depth against a
        // misbehaving RPC.
        if event.amount == 0 || event.bth_address.is_empty() {
            self.db
                .log_audit(
                    None,
                    "burn_invalid",
                    &format!(
                        "tx={} amount={} bth_address_len={}",
                        event.tx_hash,
                        event.amount,
                        event.bth_address.len()
                    ),
                )
                .map_err(WatchError::Db)?;
            return Ok(());
        }

        // NOTE: the burn-side bridge fee (deducted from the released BTH)
        // is applied by the release path (#822); orders are created with
        // fee 0 here so the watcher stays fee-policy agnostic.
        let order = BridgeOrder::new_burn(
            Chain::Ethereum,
            event.amount,
            0,
            event.from.clone(),
            event.bth_address.clone(),
            event.tx_hash.clone(),
        );

        let inserted = self
            .db
            .insert_burn_order(
                &order,
                &source_key,
                event.block_number,
                Some(&event.block_hash),
            )
            .map_err(WatchError::Db)?;
        if inserted {
            self.db
                .log_audit(
                    Some(&order.id),
                    "burn_detected",
                    &format!(
                        "tx={} amount={} block={} ({})",
                        event.tx_hash, event.amount, event.block_number, event.block_hash
                    ),
                )
                .map_err(WatchError::Db)?;
            info!(
                "Detected wBTH burn {} for {} picocredits -> order {}",
                source_key, event.amount, order.id
            );
        }

        Ok(())
    }

    /// Advance `BurnDetected` orders to `BurnConfirmed` once their burn is
    /// `confirmations_required` deep in a still-canonical block. An
    /// orphaned burn is flagged (exactly once) and does NOT advance — it
    /// waits to be re-observed in a canonical block.
    async fn confirm_detected_burns(
        &self,
        client: &dyn EthChainClient,
        tip: u64,
    ) -> Result<(), WatchError> {
        let required = self.config.confirmations_required as u64;
        let detected = self
            .db
            .get_orders_by_status("burn_detected")
            .map_err(WatchError::Db)?;

        for order in detected {
            if order.source_chain != Chain::Ethereum {
                continue;
            }
            let Some(record) = self
                .db
                .get_burn_by_order(&order.id)
                .map_err(WatchError::Db)?
            else {
                continue;
            };

            let confirmations = if tip >= record.block_number {
                tip - record.block_number + 1
            } else {
                0
            };
            if confirmations < required {
                debug!(
                    "Burn {} for order {} at {} confirmation(s) (< {}); waiting",
                    record.source_key, order.id, confirmations, required
                );
                continue;
            }

            // Depth reached — the burn's block must still be canonical
            // before any BTH can ever be released for it.
            let canonical = client.block_hash_at(record.block_number).await?;
            let still_canonical = matches!(
                (&canonical, &record.block_hash),
                (Some(now), Some(seen)) if now == seen
            );

            if still_canonical {
                if !order.status.can_transition_to(&OrderStatus::BurnConfirmed) {
                    warn!(
                        "Order {} cannot transition {} -> burn_confirmed; skipping",
                        order.id, order.status
                    );
                    continue;
                }
                self.db
                    .update_order_status(&order.id, &OrderStatus::BurnConfirmed, None)
                    .map_err(WatchError::Db)?;
                self.db
                    .log_audit(
                        Some(&order.id),
                        "burn_confirmed",
                        &format!(
                            "tx={} block={} confirmations={}",
                            record.source_key, record.block_number, confirmations
                        ),
                    )
                    .map_err(WatchError::Db)?;
                info!(
                    "Burn {} confirmed at {} confirmations; order {} ready for release",
                    record.source_key, confirmations, order.id
                );
            } else if self
                .db
                .mark_burn_orphaned(&record.source_key)
                .map_err(WatchError::Db)?
            {
                self.db
                    .log_audit(
                        Some(&order.id),
                        "burn_orphaned",
                        &format!(
                            "tx={} block {} ({:?}) reorged out (canonical now {:?})",
                            record.source_key, record.block_number, record.block_hash, canonical
                        ),
                    )
                    .map_err(WatchError::Db)?;
                warn!(
                    "Burn {} reorged out before confirmation; order {} held at BurnDetected",
                    record.source_key, order.id
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, U256};
    use std::{collections::BTreeMap, sync::Mutex};

    // === Pure helpers ===

    fn addr(byte: u8) -> Address {
        Address::from([byte; 20])
    }

    fn rpc_burn_log(
        contract: Address,
        from: Address,
        amount: U256,
        bth_address: &str,
        block_number: u64,
        block_hash: u8,
        tx_hash: u8,
        log_index: u64,
    ) -> Log {
        let event = IWrappedBTHEvents::BridgeBurn {
            from,
            amount,
            bthAddress: bth_address.to_string(),
        };
        Log {
            inner: alloy::primitives::Log {
                address: contract,
                data: event.encode_log_data(),
            },
            block_number: Some(block_number),
            block_hash: Some(B256::from([block_hash; 32])),
            transaction_hash: Some(B256::from([tx_hash; 32])),
            log_index: Some(log_index),
            ..Default::default()
        }
    }

    #[test]
    fn test_decode_burn_log_roundtrip() {
        let log = rpc_burn_log(
            addr(0xEE),
            addr(0x11),
            U256::from(999_000_000_000u64),
            "bth_stealth",
            42,
            0xB0,
            0x71,
            3,
        );
        let event = decode_burn_log(&log).unwrap();
        assert_eq!(event.amount, 999_000_000_000);
        assert_eq!(event.bth_address, "bth_stealth");
        assert_eq!(event.block_number, 42);
        assert_eq!(event.log_index, 3);
        assert_eq!(event.from, format!("{:#x}", addr(0x11)));
        assert_eq!(event.tx_hash, format!("{:#x}", B256::from([0x71u8; 32])));
    }

    #[test]
    fn test_decode_burn_log_rejects_amount_overflow() {
        // An amount that cannot fit u64 picocredits cannot be a real BTH
        // quantity — the log is skipped rather than truncated.
        let log = rpc_burn_log(
            addr(0xEE),
            addr(0x11),
            U256::from(u64::MAX) + U256::from(1u8),
            "bth_stealth",
            42,
            0xB0,
            0x71,
            0,
        );
        assert!(decode_burn_log(&log).is_none());
    }

    #[test]
    fn test_tx_ordinals_stable_regardless_of_input_order() {
        let mk = |tx: &str, block: u64, log_index: u64| BurnEvent {
            tx_hash: tx.to_string(),
            log_index,
            block_number: block,
            block_hash: "0xb".to_string(),
            from: "0xf".to_string(),
            amount: 1,
            bth_address: "a".to_string(),
        };
        // Two burns in the same tx plus one in another tx, shuffled.
        let events = vec![mk("0xt1", 5, 9), mk("0xt2", 5, 4), mk("0xt1", 5, 2)];
        let keyed: Vec<(String, u32)> = with_tx_ordinals(events)
            .into_iter()
            .map(|(e, o)| (e.tx_hash, o))
            .collect();
        assert_eq!(
            keyed,
            vec![
                ("0xt1".to_string(), 0), // log_index 2 first
                ("0xt2".to_string(), 0),
                ("0xt1".to_string(), 1), // log_index 9 second
            ]
        );
        assert_eq!(burn_source_key("0xt1", 1), "0xt1#1");
    }

    // === Watcher scenarios against a mock chain ===

    struct MockChain {
        /// height -> canonical block hash
        hashes: BTreeMap<u64, String>,
        /// All events ever emitted; only those whose (height, hash) is
        /// still canonical are visible via burn_events.
        events: Vec<BurnEvent>,
    }

    struct MockEthClient {
        chain: Mutex<MockChain>,
    }

    impl MockEthClient {
        fn new() -> Self {
            Self {
                chain: Mutex::new(MockChain {
                    hashes: BTreeMap::new(),
                    events: Vec::new(),
                }),
            }
        }

        /// Extend the canonical chain to `height` (inclusive), hashing
        /// blocks with the given fork tag.
        fn extend_to(&self, height: u64, fork: &str) {
            let mut chain = self.chain.lock().unwrap();
            let start = chain.hashes.keys().next_back().map(|h| h + 1).unwrap_or(0);
            for h in start..=height {
                chain.hashes.insert(h, format!("0x{}_{}", fork, h));
            }
        }

        /// Reorg: replace canonical blocks from `from_height` up to the
        /// current tip with a new fork's hashes (orphaning events there).
        fn reorg_from(&self, from_height: u64, fork: &str, new_tip: u64) {
            let mut chain = self.chain.lock().unwrap();
            let tip = *chain.hashes.keys().next_back().unwrap();
            for h in from_height..=tip.max(new_tip) {
                if h <= new_tip {
                    chain.hashes.insert(h, format!("0x{}_{}", fork, h));
                } else {
                    chain.hashes.remove(&h);
                }
            }
        }

        fn block_hash(&self, height: u64) -> Option<String> {
            self.chain.lock().unwrap().hashes.get(&height).cloned()
        }

        /// Emit a burn event in the CURRENT canonical block at `height`.
        fn emit_burn(&self, tx: &str, amount: u64, bth_address: &str, height: u64) {
            let hash = self.block_hash(height).expect("block must exist");
            self.chain.lock().unwrap().events.push(BurnEvent {
                tx_hash: tx.to_string(),
                log_index: 0,
                block_number: height,
                block_hash: hash,
                from: "0xburner".to_string(),
                amount,
                bth_address: bth_address.to_string(),
            });
        }
    }

    #[async_trait]
    impl EthChainClient for MockEthClient {
        async fn latest_block(&self) -> Result<u64, WatchError> {
            Ok(*self
                .chain
                .lock()
                .unwrap()
                .hashes
                .keys()
                .next_back()
                .unwrap_or(&0))
        }

        async fn block_hash_at(&self, number: u64) -> Result<Option<String>, WatchError> {
            Ok(self.block_hash(number))
        }

        async fn burn_events(&self, from: u64, to: u64) -> Result<Vec<BurnEvent>, WatchError> {
            let chain = self.chain.lock().unwrap();
            Ok(chain
                .events
                .iter()
                .filter(|e| {
                    e.block_number >= from
                        && e.block_number <= to
                        // Only canonical events are visible.
                        && chain.hashes.get(&e.block_number) == Some(&e.block_hash)
                })
                .cloned()
                .collect())
        }
    }

    fn setup(confirmations_required: u32) -> (EthereumWatcher, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let config = EthereumConfig {
            rpc_url: "http://localhost:8545".to_string(),
            wbth_contract: "0x00000000000000000000000000000000000000ee".to_string(),
            safe_address: None,
            chain_id: 1,
            private_key_file: None,
            confirmations_required,
            gas_price_strategy: Default::default(),
        };
        let (_tx, rx) = broadcast::channel(1);
        (EthereumWatcher::new(config, db.clone(), rx), db)
    }

    fn burn_orders(db: &Database) -> Vec<BridgeOrder> {
        // Matches burn_detected + burn_confirmed via the LIKE prefix.
        db.get_orders_by_status("burn").unwrap()
    }

    #[tokio::test]
    async fn test_confirmation_counting_gates_burn_confirmed() {
        let (watcher, db) = setup(3);
        let client = MockEthClient::new();

        client.extend_to(5, "main");
        client.emit_burn("0xburn1", 1_000_000_000_000, "bth_dest", 5);

        // Tip 5, burn at 5 -> 1 confirmation: detected, NOT confirmed.
        watcher.scan_once(&client).await.unwrap();
        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].status, OrderStatus::BurnDetected);
        assert_eq!(orders[0].amount, 1_000_000_000_000);
        assert_eq!(orders[0].dest_address, "bth_dest");
        assert_eq!(orders[0].source_tx.as_deref(), Some("0xburn1"));

        // 2 confirmations: still below the threshold of 3.
        client.extend_to(6, "main");
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db)[0].status, OrderStatus::BurnDetected);

        // 3 confirmations: threshold met, block canonical -> confirmed.
        client.extend_to(7, "main");
        watcher.scan_once(&client).await.unwrap();
        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 1, "still exactly one order");
        assert_eq!(orders[0].status, OrderStatus::BurnConfirmed);
        assert_eq!(db.count_audit_action("burn_detected").unwrap(), 1);
        assert_eq!(db.count_audit_action("burn_confirmed").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_orphaned_burn_never_confirms() {
        let (watcher, db) = setup(2);
        let client = MockEthClient::new();

        client.extend_to(4, "main");
        client.emit_burn("0xburn1", 5_000_000_000, "bth_dest", 4);
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db)[0].status, OrderStatus::BurnDetected);

        // Reorg out the burn's block; the new fork reaches height 7
        // (deep enough that raw depth would satisfy the threshold).
        client.reorg_from(4, "fork", 7);
        watcher.scan_once(&client).await.unwrap();

        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 1);
        assert_eq!(
            orders[0].status,
            OrderStatus::BurnDetected,
            "an orphaned burn must not advance toward release"
        );
        let record = db.get_burn_by_order(&orders[0].id).unwrap().unwrap();
        assert!(record.orphaned);
        assert_eq!(db.count_audit_action("burn_orphaned").unwrap(), 1);
        assert_eq!(db.count_audit_action("eth_cursor_reorg").unwrap(), 1);

        // Further passes: still held, orphan audit not duplicated.
        client.extend_to(9, "fork");
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db)[0].status, OrderStatus::BurnDetected);
        assert_eq!(db.count_audit_action("burn_orphaned").unwrap(), 1);
        assert_eq!(db.count_audit_action("burn_confirmed").unwrap(), 0);
    }

    #[tokio::test]
    async fn test_reorg_readd_processed_exactly_once() {
        let (watcher, db) = setup(2);
        let client = MockEthClient::new();

        client.extend_to(4, "main");
        client.emit_burn("0xburn1", 5_000_000_000, "bth_dest", 4);
        watcher.scan_once(&client).await.unwrap();
        let order_id = burn_orders(&db)[0].id;

        // Orphan it...
        client.reorg_from(4, "fork", 5);
        watcher.scan_once(&client).await.unwrap();
        assert!(db.get_burn_by_order(&order_id).unwrap().unwrap().orphaned);

        // ...then the same tx is re-included at height 6 on the new fork.
        client.extend_to(6, "fork");
        client.emit_burn("0xburn1", 5_000_000_000, "bth_dest", 6);
        watcher.scan_once(&client).await.unwrap();

        // Same single order, relocated, no longer orphaned.
        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 1, "re-add must reuse the existing order");
        assert_eq!(orders[0].id, order_id);
        let record = db.get_burn_by_order(&order_id).unwrap().unwrap();
        assert!(!record.orphaned);
        assert_eq!(record.block_number, 6);
        assert_eq!(db.count_audit_action("burn_relocated").unwrap(), 1);

        // Reaches the threshold on the new fork -> confirmed exactly once.
        client.extend_to(7, "fork");
        watcher.scan_once(&client).await.unwrap();
        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
        assert_eq!(orders[0].status, OrderStatus::BurnConfirmed);
        assert_eq!(db.count_audit_action("burn_detected").unwrap(), 1);
        assert_eq!(db.count_audit_action("burn_confirmed").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_restart_resumes_from_cursor() {
        let (watcher, db) = setup(2);
        let client = MockEthClient::new();

        client.extend_to(5, "main");
        client.emit_burn("0xburn1", 1_000, "bth_dest", 5);
        watcher.scan_once(&client).await.unwrap();

        let cursor = db.get_cursor(Chain::Ethereum).unwrap().unwrap();
        assert_eq!(cursor.last_height, 5);
        assert_eq!(cursor.last_block_hash.as_deref(), Some("0xmain_5"));

        // "Restart": a fresh watcher on the SAME database.
        let (_tx2, rx2) = broadcast::channel(1);
        let watcher2 = EthereumWatcher::new(watcher.config.clone(), db.clone(), rx2);

        client.extend_to(6, "main");
        client.emit_burn("0xburn2", 2_000, "bth_dest2", 6);
        watcher2.scan_once(&client).await.unwrap();

        // Both burns exist exactly once; no duplicate of burn1.
        let orders = burn_orders(&db);
        assert_eq!(orders.len(), 2);
        assert_eq!(db.count_audit_action("burn_detected").unwrap(), 2);
        // burn1 now at 2 confirmations -> confirmed; burn2 at 1 -> detected.
        let by_tx = |tx: &str| {
            orders
                .iter()
                .find(|o| o.source_tx.as_deref() == Some(tx))
                .unwrap()
                .clone()
        };
        assert_eq!(by_tx("0xburn1").status, OrderStatus::BurnConfirmed);
        assert_eq!(by_tx("0xburn2").status, OrderStatus::BurnDetected);
    }

    #[tokio::test]
    async fn test_cursor_rewind_replay_is_noop() {
        let (watcher, db) = setup(1);
        let client = MockEthClient::new();

        client.extend_to(3, "main");
        client.emit_burn("0xburn1", 1_000, "bth_dest", 3);
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(burn_orders(&db).len(), 1);

        // Rewind the cursor behind the processed block and replay: the
        // processed_burns source key dedups the event.
        db.set_cursor(Chain::Ethereum, 0, Some("0xmain_0")).unwrap();
        watcher.scan_once(&client).await.unwrap();

        assert_eq!(
            burn_orders(&db).len(),
            1,
            "replay must not duplicate the order"
        );
        assert_eq!(db.count_audit_action("burn_detected").unwrap(), 1);
    }
}
