// Copyright (c) 2024 The Botho Foundation

//! BTH chain watcher for monitoring deposits (mint flow).
//!
//! Scans finalized BTH blocks for outputs paying the bridge's stealth
//! address, matches each deposit to its `AwaitingDeposit` order via the
//! deposit memo, and drives `AwaitingDeposit -> DepositDetected ->
//! DepositConfirmed`.
//!
//! ## Finality (SCP)
//!
//! `BthConfig::confirmations_required == 0` means SCP-final: Botho blocks
//! are final at inclusion (SCP externalization), so the watcher scans up to
//! the tip and detection implies finality. A non-zero value makes the scan
//! lag the tip by that many blocks (belt-and-suspenders for operators who
//! want depth on top of SCP), so scanned blocks still always meet the
//! requirement — either way a detected deposit advances straight through
//! `DepositDetected` to `DepositConfirmed`.
//!
//! ## Privacy boundary (ADR 0004)
//!
//! The amount carried on a [`BthDeposit`] is the REVEALED deposit amount —
//! the deliberate privacy exit of the bridge. The transport
//! ([`BthChainClient`] implementation) is responsible for view-key stealth
//! scanning and verifying the commitment opening; only the amount (never
//! the source ring) crosses this boundary.
//!
//! ## Peg eligibility (ADR 0003)
//!
//! Only factor-1 (background/commerce) coins are wrappable: a factor-1
//! coin pays exactly zero demurrage forever, so a factor-1-only reserve
//! cannot decay below the outstanding wBTH supply. Deposits whose cluster
//! factor is not 1.0 are rejected before confirmation (the order fails and
//! an audit entry is written).
//!
//! ## Implementation status
//!
//! The deterministic scan/match/gate/dedup pipeline is implemented and
//! tested against [`BthChainClient`]. The live transport (`ws_url`
//! NewBlock subscription + `view_key_file` stealth scanning + commitment
//! opening) is a fail-safe `TODO(#828)` stub: until it is wired, the
//! watcher polls, logs, and creates no state.

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, BthConfig, Chain, OrderStatus};
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use super::WatchError;
use crate::{db::Database, engine::ShutdownSignal};

/// Fixed-point scale for cluster factors, matching
/// `cluster-tax::demurrage::FACTOR_SCALE` (1000 = factor 1.0×). A deposit
/// is wrap-eligible iff its factor is exactly `FACTOR_SCALE` (ADR 0003).
pub const FACTOR_SCALE: u64 = 1000;

/// Delay between scan passes.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// A deposit to the bridge stealth address, as decoded by the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BthDeposit {
    /// BTH transaction hash (idempotency key in `processed_deposits`).
    pub tx_hash: String,
    /// REVEALED deposit amount in picocredits (ADR 0004): the transport
    /// has already verified the commitment opening.
    pub amount: u64,
    /// Deposit memo carrying the order UUID (see
    /// [`BridgeOrder::generate_memo`]); `None` if absent/undecodable.
    pub memo: Option<[u8; 64]>,
    /// Cluster factor of the received output in [`FACTOR_SCALE`] units,
    /// read from its `ClusterTagVector` (ADR 0003 eligibility gate).
    pub cluster_factor: u64,
}

/// A finalized BTH block with the bridge-relevant deposits already
/// extracted by the transport's view-key scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BthBlock {
    /// Block height.
    pub height: u64,
    /// Block id (hash) — persisted with the cursor for audit purposes.
    pub block_id: String,
    /// Deposits paying the bridge stealth address in this block.
    pub deposits: Vec<BthDeposit>,
}

/// Read access to the BTH chain, mockable for tests.
#[async_trait]
pub trait BthChainClient: Send + Sync {
    /// Current chain tip height.
    async fn tip_height(&self) -> Result<u64, WatchError>;

    /// Fetch the block at `height` with bridge deposits decoded, or
    /// `None` if the node does not have that height yet.
    async fn block_at(&self, height: u64) -> Result<Option<BthBlock>, WatchError>;
}

/// Live transport against a BTH node.
///
/// TODO(#828): implement against `BthConfig::ws_url` — subscribe to
/// NewBlock, and for each block run the view-key stealth scan
/// (`view_key_file`) over outputs, verify the Pedersen commitment opening
/// to reveal the amount (ADR 0004), read the output's `ClusterTagVector`
/// factor (ADR 0003), and decode the 64-byte deposit memo. Until then this
/// client is a fail-safe stub: it reports `NotImplemented` and the watcher
/// creates no state.
pub struct NodeBthClient {
    #[allow(dead_code)]
    config: BthConfig,
}

impl NodeBthClient {
    /// Build a client from configuration. Does not perform network I/O.
    pub fn new(config: BthConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl BthChainClient for NodeBthClient {
    async fn tip_height(&self) -> Result<u64, WatchError> {
        Err(WatchError::NotImplemented(
            "BTH node websocket transport (ws_url NewBlock subscription) pending #828".to_string(),
        ))
    }

    async fn block_at(&self, _height: u64) -> Result<Option<BthBlock>, WatchError> {
        Err(WatchError::NotImplemented(
            "BTH block fetch + view-key stealth scan pending #828".to_string(),
        ))
    }
}

/// BTH watcher monitors the BTH chain for deposits to the bridge address.
pub struct BthWatcher {
    config: BthConfig,
    db: Database,
    shutdown: ShutdownSignal,
}

impl BthWatcher {
    /// Create a new BTH watcher.
    pub fn new(config: BthConfig, db: Database, shutdown: ShutdownSignal) -> Self {
        Self {
            config,
            db,
            shutdown,
        }
    }

    /// Run the watcher.
    pub async fn run(mut self) -> Result<(), String> {
        info!("Starting BTH watcher for {}", self.config.ws_url);

        let client = NodeBthClient::new(self.config.clone());

        loop {
            // Check for shutdown first
            match self.shutdown.try_recv() {
                Ok(_) | Err(broadcast::error::TryRecvError::Closed) => {
                    info!("BTH watcher shutting down");
                    return Ok(());
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // No shutdown signal, continue
                }
            }

            match self.scan_once(&client).await {
                Ok(blocks) if blocks > 0 => {
                    debug!("BTH watcher processed {} block(s)", blocks);
                }
                Ok(_) => {}
                Err(WatchError::NotImplemented(msg)) => {
                    // Fail-safe transport stub: no state is created.
                    debug!("BTH watcher idle: {}", msg);
                }
                Err(e) => warn!("BTH scan failed (will retry): {}", e),
            }

            tokio::select! {
                _ = self.shutdown.recv() => {
                    info!("BTH watcher shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    /// One scan pass: process every finalized block past the persisted
    /// cursor. Returns the number of blocks processed.
    ///
    /// The cursor is persisted only AFTER a block is fully processed, so a
    /// restart resumes at the right block; `processed_deposits` plus the
    /// `record_deposit_detected` status guard deduplicate any replay.
    pub async fn scan_once(&self, client: &dyn BthChainClient) -> Result<u64, WatchError> {
        let tip = client.tip_height().await?;

        // Finality target: with confirmations_required == 0 (SCP-final)
        // every included block is final, so scan to the tip; otherwise lag
        // the tip so scanned blocks always meet the depth requirement.
        let target = tip.saturating_sub(self.config.confirmations_required as u64);

        let mut next = match self.db.get_cursor(Chain::Bth).map_err(WatchError::Db)? {
            Some(cursor) => cursor.last_height + 1,
            // First run: start from genesis. Operators bootstrapping
            // against a long-lived chain can seed the watcher_cursors row
            // to skip history.
            None => 0,
        };

        let mut processed = 0u64;
        while next <= target {
            let Some(block) = client.block_at(next).await? else {
                break;
            };

            for deposit in &block.deposits {
                self.process_deposit(block.height, deposit)?;
            }

            // Persist only after the block is fully processed.
            self.db
                .set_cursor(Chain::Bth, block.height, Some(&block.block_id))
                .map_err(WatchError::Db)?;

            processed += 1;
            next += 1;
        }

        Ok(processed)
    }

    /// Handle one deposit from a finalized block: dedup, memo→order match,
    /// factor-1 gate (ADR 0003), then detect + confirm.
    fn process_deposit(&self, height: u64, deposit: &BthDeposit) -> Result<(), WatchError> {
        let db = &self.db;

        // Idempotency layer independent of the cursor: a cursor rewind or
        // crash replay of an already-processed tx is a no-op.
        if db
            .is_deposit_processed(&deposit.tx_hash)
            .map_err(WatchError::Db)?
        {
            debug!("Deposit {} already processed; skipping", deposit.tx_hash);
            return Ok(());
        }

        // Match to a pending order via the memo-embedded order UUID.
        let Some(order_id) = deposit
            .memo
            .as_ref()
            .and_then(BridgeOrder::order_id_from_memo)
        else {
            warn!(
                "Deposit {} at height {} has no decodable order memo; leaving unmatched",
                deposit.tx_hash, height
            );
            db.log_audit(
                None,
                "deposit_unmatched",
                &format!(
                    "tx={} height={} amount={}",
                    deposit.tx_hash, height, deposit.amount
                ),
            )
            .map_err(WatchError::Db)?;
            return Ok(());
        };

        let Some(order) = db.get_order(&order_id).map_err(WatchError::Db)? else {
            warn!(
                "Deposit {} references unknown order {}; leaving unmatched",
                deposit.tx_hash, order_id
            );
            db.log_audit(
                None,
                "deposit_unknown_order",
                &format!("tx={} order={}", deposit.tx_hash, order_id),
            )
            .map_err(WatchError::Db)?;
            return Ok(());
        };

        if order.status != OrderStatus::AwaitingDeposit {
            debug!(
                "Deposit {} for order {} in status {}; not awaiting a deposit — skipping",
                deposit.tx_hash, order.id, order.status
            );
            return Ok(());
        }

        // Factor-1 eligibility gate (ADR 0003): only zero-demurrage
        // (background/commerce) coins may enter the reserve, otherwise the
        // locked reserve decays below the outstanding wBTH supply.
        if deposit.cluster_factor != FACTOR_SCALE {
            let reason = format!(
                "deposit {} is not factor-1 (factor {}/{}); only factor-1 coins are \
                 wrappable per ADR 0003 — settle demurrage (#831) and retry",
                deposit.tx_hash, deposit.cluster_factor, FACTOR_SCALE
            );
            warn!("Rejecting order {}: {}", order.id, reason);
            db.mark_deposit_processed(&deposit.tx_hash, &order.id)
                .map_err(WatchError::Db)?;
            db.update_order_status(
                &order.id,
                &OrderStatus::Failed {
                    reason: reason.clone(),
                },
                None,
            )
            .map_err(WatchError::Db)?;
            db.log_audit(Some(&order.id), "deposit_rejected_non_factor1", &reason)
                .map_err(WatchError::Db)?;
            return Ok(());
        }

        if deposit.amount == 0 {
            let reason = format!("deposit {} has zero revealed amount", deposit.tx_hash);
            warn!("Rejecting order {}: {}", order.id, reason);
            db.mark_deposit_processed(&deposit.tx_hash, &order.id)
                .map_err(WatchError::Db)?;
            db.update_order_status(
                &order.id,
                &OrderStatus::Failed {
                    reason: reason.clone(),
                },
                None,
            )
            .map_err(WatchError::Db)?;
            db.log_audit(Some(&order.id), "deposit_rejected_zero_amount", &reason)
                .map_err(WatchError::Db)?;
            return Ok(());
        }

        // Detect: record the REVEALED amount (authoritative over the
        // amount quoted at order creation, ADR 0004) and the deposit tx as
        // source_tx. The status guard doubles as a replay no-op.
        let detected = db
            .record_deposit_detected(&order.id, &deposit.tx_hash, deposit.amount)
            .map_err(WatchError::Db)?;
        if !detected {
            debug!(
                "Order {} no longer awaiting deposit; skipping {}",
                order.id, deposit.tx_hash
            );
            return Ok(());
        }
        db.mark_deposit_processed(&deposit.tx_hash, &order.id)
            .map_err(WatchError::Db)?;

        if deposit.amount != order.amount {
            db.log_audit(
                Some(&order.id),
                "deposit_amount_mismatch",
                &format!("expected={} revealed={}", order.amount, deposit.amount),
            )
            .map_err(WatchError::Db)?;
        }
        db.log_audit(
            Some(&order.id),
            "deposit_detected",
            &format!(
                "tx={} height={} amount={}",
                deposit.tx_hash, height, deposit.amount
            ),
        )
        .map_err(WatchError::Db)?;

        // Confirm: scanned blocks already meet the finality requirement
        // (SCP-final at inclusion when confirmations_required == 0, else
        // the scan lags the tip by the requirement), so detection implies
        // finality and the order advances straight to DepositConfirmed.
        db.update_order_status(&order.id, &OrderStatus::DepositConfirmed, None)
            .map_err(WatchError::Db)?;
        db.log_audit(
            Some(&order.id),
            "deposit_confirmed",
            &format!(
                "tx={} height={} scp_final={}",
                deposit.tx_hash,
                height,
                self.config.confirmations_required == 0
            ),
        )
        .map_err(WatchError::Db)?;

        info!(
            "Deposit {} confirmed for order {} ({} picocredits, height {})",
            deposit.tx_hash, order.id, deposit.amount, height
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::Mutex};

    /// In-memory BTH chain for driving the watcher deterministically.
    struct MockBthClient {
        blocks: Mutex<BTreeMap<u64, BthBlock>>,
    }

    impl MockBthClient {
        fn new() -> Self {
            Self {
                blocks: Mutex::new(BTreeMap::new()),
            }
        }

        /// Append the next block (height = current tip + 1, or 0).
        fn push_block(&self, deposits: Vec<BthDeposit>) -> u64 {
            let mut blocks = self.blocks.lock().unwrap();
            let height = blocks.keys().next_back().map(|h| h + 1).unwrap_or(0);
            blocks.insert(
                height,
                BthBlock {
                    height,
                    block_id: format!("bth_block_{}", height),
                    deposits,
                },
            );
            height
        }
    }

    #[async_trait]
    impl BthChainClient for MockBthClient {
        async fn tip_height(&self) -> Result<u64, WatchError> {
            Ok(*self.blocks.lock().unwrap().keys().next_back().unwrap_or(&0))
        }

        async fn block_at(&self, height: u64) -> Result<Option<BthBlock>, WatchError> {
            Ok(self.blocks.lock().unwrap().get(&height).cloned())
        }
    }

    fn setup(confirmations_required: u32) -> (BthWatcher, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let config = BthConfig {
            rpc_url: "http://localhost:7101".to_string(),
            ws_url: "ws://localhost:7101/ws".to_string(),
            view_key_file: None,
            spend_key_file: None,
            confirmations_required,
            reserve_address: None,
            release_signers: Vec::new(),
            release_threshold: 0,
            release_confirmations_required: 0,
        };
        let (_tx, rx) = broadcast::channel(1);
        // _tx dropped: try_recv returns Closed, but tests drive scan_once
        // directly, never run().
        (BthWatcher::new(config, db.clone(), rx), db)
    }

    fn awaiting_order(db: &Database, amount: u64) -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            amount,
            1_000_000_000,
            "bridge_stealth_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.generate_memo();
        db.insert_order(&order).unwrap();
        order
    }

    fn deposit_for(order: &BridgeOrder, tx: &str, amount: u64, factor: u64) -> BthDeposit {
        BthDeposit {
            tx_hash: tx.to_string(),
            amount,
            memo: order.memo,
            cluster_factor: factor,
        }
    }

    #[tokio::test]
    async fn test_factor1_deposit_detects_and_confirms_at_scp_finality() {
        // confirmations_required == 0: SCP-final at inclusion.
        let (watcher, db) = setup(0);
        let client = MockBthClient::new();
        let order = awaiting_order(&db, 1_000_000_000_000);

        // Revealed amount differs from the quoted amount: the revealed
        // amount is authoritative (ADR 0004).
        client.push_block(vec![deposit_for(
            &order,
            "0xdep1",
            750_000_000_000,
            FACTOR_SCALE,
        )]);

        watcher.scan_once(&client).await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositConfirmed);
        assert_eq!(stored.source_tx.as_deref(), Some("0xdep1"));
        assert_eq!(
            stored.amount, 750_000_000_000,
            "revealed amount is recorded"
        );
        assert!(db.is_deposit_processed("0xdep1").unwrap());
        assert_eq!(db.count_audit_action("deposit_detected").unwrap(), 1);
        assert_eq!(db.count_audit_action("deposit_confirmed").unwrap(), 1);
        assert_eq!(db.count_audit_action("deposit_amount_mismatch").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_non_factor1_deposit_rejected_with_audit() {
        let (watcher, db) = setup(0);
        let client = MockBthClient::new();

        // A factor-2.5 deposit must be rejected (ADR 0003)...
        let rejected = awaiting_order(&db, 1_000_000_000_000);
        client.push_block(vec![deposit_for(
            &rejected,
            "0xwealthy",
            1_000_000_000_000,
            2_500,
        )]);
        watcher.scan_once(&client).await.unwrap();

        let stored = db.get_order(&rejected.id).unwrap().unwrap();
        assert!(
            matches!(stored.status, OrderStatus::Failed { ref reason } if reason.contains("factor-1")),
            "order must fail with a factor-1 reason, got {}",
            stored.status
        );
        assert_eq!(
            db.count_audit_action("deposit_rejected_non_factor1")
                .unwrap(),
            1
        );
        // The rejected tx is still marked processed (no reprocessing loop).
        assert!(db.is_deposit_processed("0xwealthy").unwrap());

        // ...while an identical factor-1 deposit confirms.
        let accepted = awaiting_order(&db, 1_000_000_000_000);
        client.push_block(vec![deposit_for(
            &accepted,
            "0xsettled",
            1_000_000_000_000,
            FACTOR_SCALE,
        )]);
        watcher.scan_once(&client).await.unwrap();

        let stored = db.get_order(&accepted.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositConfirmed);
    }

    #[tokio::test]
    async fn test_confirmation_lag_when_depth_required() {
        // confirmations_required = 2: the scan lags the tip, so a deposit
        // at the tip is not touched until it is 2 deep.
        let (watcher, db) = setup(2);
        let client = MockBthClient::new();
        let order = awaiting_order(&db, 1_000_000_000_000);

        client.push_block(vec![]); // height 0
        client.push_block(vec![]); // height 1
        client.push_block(vec![deposit_for(
            &order,
            "0xdep",
            1_000_000_000_000,
            FACTOR_SCALE,
        )]); // height 2 (tip)

        watcher.scan_once(&client).await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(
            stored.status,
            OrderStatus::AwaitingDeposit,
            "deposit at the tip must not be processed before the depth requirement"
        );

        client.push_block(vec![]); // height 3
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::AwaitingDeposit,
            "one confirmation is still below the requirement"
        );

        client.push_block(vec![]); // height 4: deposit now 2 deep
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::DepositConfirmed
        );
    }

    #[tokio::test]
    async fn test_restart_resumes_from_cursor_without_missing_or_duplicating() {
        let (watcher, db) = setup(0);
        let client = MockBthClient::new();
        let first = awaiting_order(&db, 1_000_000_000_000);

        client.push_block(vec![]);
        client.push_block(vec![deposit_for(
            &first,
            "0xdep1",
            1_000_000_000_000,
            FACTOR_SCALE,
        )]);
        watcher.scan_once(&client).await.unwrap();

        let cursor = db.get_cursor(Chain::Bth).unwrap().unwrap();
        assert_eq!(cursor.last_height, 1);
        assert_eq!(cursor.last_block_hash.as_deref(), Some("bth_block_1"));

        // "Restart": a fresh watcher on the same db must resume at height 2
        // and pick up only the new block — no re-processing of old blocks.
        let (_tx, rx) = broadcast::channel(1);
        let watcher2 = BthWatcher::new(
            BthConfig {
                rpc_url: String::new(),
                ws_url: String::new(),
                view_key_file: None,
                spend_key_file: None,
                confirmations_required: 0,
                reserve_address: None,
                release_signers: Vec::new(),
                release_threshold: 0,
                release_confirmations_required: 0,
            },
            db.clone(),
            rx,
        );

        let second = awaiting_order(&db, 2_000_000_000_000);
        client.push_block(vec![deposit_for(
            &second,
            "0xdep2",
            2_000_000_000_000,
            FACTOR_SCALE,
        )]);

        let processed = watcher2.scan_once(&client).await.unwrap();
        assert_eq!(processed, 1, "only the new block is scanned after restart");

        assert_eq!(
            db.get_order(&first.id).unwrap().unwrap().status,
            OrderStatus::DepositConfirmed
        );
        assert_eq!(
            db.get_order(&second.id).unwrap().unwrap().status,
            OrderStatus::DepositConfirmed
        );
        // Exactly one detect/confirm per deposit across the restart.
        assert_eq!(db.count_audit_action("deposit_detected").unwrap(), 2);
        assert_eq!(db.count_audit_action("deposit_confirmed").unwrap(), 2);
    }

    #[tokio::test]
    async fn test_cursor_rewind_replay_is_deduplicated() {
        let (watcher, db) = setup(0);
        let client = MockBthClient::new();
        let order = awaiting_order(&db, 1_000_000_000_000);

        client.push_block(vec![]); // height 0
        client.push_block(vec![deposit_for(
            &order,
            "0xdep",
            1_000_000_000_000,
            FACTOR_SCALE,
        )]); // height 1
        watcher.scan_once(&client).await.unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::DepositConfirmed
        );
        assert_eq!(db.get_cursor(Chain::Bth).unwrap().unwrap().last_height, 1);

        // Simulate a non-atomically-advanced cursor (rewound behind
        // already-processed blocks): replaying block 1 must be a complete
        // no-op thanks to processed_deposits.
        db.set_cursor(Chain::Bth, 0, None).unwrap();
        let replayed = watcher.scan_once(&client).await.unwrap();
        assert_eq!(replayed, 1, "block 1 is scanned again after the rewind");

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositConfirmed);
        assert_eq!(
            db.count_audit_action("deposit_detected").unwrap(),
            1,
            "replay after cursor rewind must not re-detect"
        );
        assert_eq!(db.count_audit_action("deposit_confirmed").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_unmatched_and_unknown_deposits_are_audited_not_fatal() {
        let (watcher, db) = setup(0);
        let client = MockBthClient::new();

        // No memo at all.
        let no_memo = BthDeposit {
            tx_hash: "0xnomemo".to_string(),
            amount: 5,
            memo: None,
            cluster_factor: FACTOR_SCALE,
        };
        // Memo referencing an order that does not exist.
        let mut ghost = BridgeOrder::new_mint(
            Chain::Ethereum,
            1,
            0,
            "a".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        ghost.generate_memo();
        let unknown = deposit_for(&ghost, "0xghost", 5, FACTOR_SCALE);

        client.push_block(vec![no_memo, unknown]);
        watcher.scan_once(&client).await.unwrap();

        assert_eq!(db.count_audit_action("deposit_unmatched").unwrap(), 1);
        assert_eq!(db.count_audit_action("deposit_unknown_order").unwrap(), 1);
        // Cursor still advanced past the block.
        assert_eq!(db.get_cursor(Chain::Bth).unwrap().unwrap().last_height, 0);
    }
}
