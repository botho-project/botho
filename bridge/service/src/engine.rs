// Copyright (c) 2024 The Botho Foundation

//! Bridge engine - coordinates watchers and order processing.

use bth_bridge_core::{BridgeConfig, BridgeOrder, Chain, OrderStatus};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::{
    attestation::{AttestationProvider, StubAttestationProvider},
    db::Database,
    mint::{ethereum::EthMinter, solana::SolMinter, ConfirmationStatus, Minter},
    watchers::{BthWatcher, EthereumWatcher},
};

/// Shutdown signal type.
pub type ShutdownSignal = broadcast::Receiver<()>;

/// Delay between re-broadcast attempts of an already-signed mint tx.
const BROADCAST_RETRY_DELAY: Duration = Duration::from_secs(2);

/// The main bridge engine that coordinates all components.
pub struct BridgeEngine {
    config: BridgeConfig,
    db: Database,
    shutdown_tx: broadcast::Sender<()>,
}

impl BridgeEngine {
    /// Create a new bridge engine.
    pub fn new(config: BridgeConfig, db: Database) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            config,
            db,
            shutdown_tx,
        }
    }

    /// Build the per-chain minting backends from configuration.
    ///
    /// A chain whose minter cannot be constructed (missing Safe address,
    /// bad contract address, ...) is skipped with a warning: deposits to
    /// that chain stay in `DepositConfirmed` until the config is fixed —
    /// they are never dropped or failed.
    fn build_minters(config: &BridgeConfig) -> HashMap<Chain, Arc<dyn Minter>> {
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();

        match EthMinter::new(config.ethereum.clone()) {
            Ok(minter) => {
                minters.insert(Chain::Ethereum, Arc::new(minter));
            }
            Err(e) => warn!("Ethereum minting disabled: {}", e),
        }

        match SolMinter::new(config.solana.clone()) {
            Ok(minter) => {
                minters.insert(Chain::Solana, Arc::new(minter));
            }
            Err(e) => warn!("Solana minting disabled: {}", e),
        }

        minters
    }

    /// Run the bridge engine.
    pub async fn run(self) -> Result<(), String> {
        info!("Starting bridge engine");

        // Spawn the BTH watcher
        let bth_watcher = BthWatcher::new(
            self.config.bth.clone(),
            self.db.clone(),
            self.shutdown_tx.subscribe(),
        );

        let bth_handle = tokio::spawn(async move {
            if let Err(e) = bth_watcher.run().await {
                error!("BTH watcher error: {}", e);
            }
        });

        // Spawn the Ethereum watcher
        let eth_watcher = EthereumWatcher::new(
            self.config.ethereum.clone(),
            self.db.clone(),
            self.shutdown_tx.subscribe(),
        );

        let eth_handle = tokio::spawn(async move {
            if let Err(e) = eth_watcher.run().await {
                error!("Ethereum watcher error: {}", e);
            }
        });

        // Spawn the order processing loop
        let minters = Self::build_minters(&self.config);
        // TODO(#824): swap the stub for the validator attestation protocol.
        let attestation: Arc<dyn AttestationProvider> = Arc::new(StubAttestationProvider);
        let processor =
            OrderProcessor::new(self.config.clone(), self.db.clone(), minters, attestation);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let process_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Order processor shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {
                        if let Err(e) = processor.process_pending_orders().await {
                            error!("Order processing error: {}", e);
                        }
                    }
                }
            }
        });

        // Handle shutdown signals
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received shutdown signal");
            }
        }

        // Send shutdown signal to all components
        let _ = self.shutdown_tx.send(());

        // Wait for all components to finish
        let _ = tokio::join!(bth_handle, eth_handle, process_handle);

        info!("Bridge engine stopped");
        Ok(())
    }
}

/// Order processor handles pending orders.
struct OrderProcessor {
    config: BridgeConfig,
    db: Database,
    minters: HashMap<Chain, Arc<dyn Minter>>,
    attestation: Arc<dyn AttestationProvider>,
}

impl OrderProcessor {
    fn new(
        config: BridgeConfig,
        db: Database,
        minters: HashMap<Chain, Arc<dyn Minter>>,
        attestation: Arc<dyn AttestationProvider>,
    ) -> Self {
        Self {
            config,
            db,
            minters,
            attestation,
        }
    }

    /// Process all pending orders.
    ///
    /// Submission (`DepositConfirmed -> MintPending`) and confirmation
    /// (`MintPending -> Completed`) are driven as separate retryable stages
    /// so a crash or RPC failure between them never loses (or duplicates)
    /// a mint.
    async fn process_pending_orders(&self) -> Result<(), String> {
        // Stage 1: confirmed deposits need a mint submitted.
        let deposit_orders = self.db.get_orders_by_status("deposit_confirmed")?;
        for order in deposit_orders {
            if let Err(e) = self.submit_mint(&order).await {
                warn!("Failed to submit mint for order {}: {}", order.id, e);
            }
        }

        // Stage 2: submitted mints need confirmation (or reorg unwind).
        let pending_mints = self.db.get_orders_by_status("mint_pending")?;
        for order in pending_mints {
            if let Err(e) = self.confirm_mint(&order).await {
                warn!("Failed to confirm mint for order {}: {}", order.id, e);
            }
        }

        // Process confirmed burns (need to release BTH)
        let burn_orders = self.db.get_orders_by_status("burn_confirmed")?;
        for order in burn_orders {
            if let Err(e) = self.process_burn_order(&order).await {
                warn!("Failed to process burn order {}: {}", order.id, e);
            }
        }

        // Check for expired orders
        self.expire_stale_orders()?;

        Ok(())
    }

    /// Submission stage: `DepositConfirmed -> MintPending`.
    ///
    /// Exactly-once: the `mints` table is consulted first and written
    /// BEFORE the first broadcast, so a crash at any point either finds no
    /// record (safe to prepare a fresh tx — nothing was broadcast) or finds
    /// the recorded tx and reuses it (never signs a second competing mint).
    async fn submit_mint(&self, order: &BridgeOrder) -> Result<(), String> {
        info!(
            "Processing mint order {} for {} picocredits",
            order.id, order.amount
        );

        if order.dest_chain == Chain::Bth {
            self.db.update_order_status(
                &order.id,
                &OrderStatus::Failed {
                    reason: "Cannot mint to BTH chain".to_string(),
                },
                None,
            )?;
            return Ok(());
        }

        let Some(minter) = self.minters.get(&order.dest_chain) else {
            // Not failed: minting for this chain is unconfigured/disabled.
            // The order stays DepositConfirmed and is retried when the
            // operator fixes the configuration.
            return Err(format!(
                "no minter configured for chain {}",
                order.dest_chain
            ));
        };

        // Idempotency: a previously prepared tx (crash between persistence
        // and status update, or a re-poll) is reused, never re-prepared.
        if let Some(existing) = self.db.get_mint_by_order(&order.id)? {
            info!(
                "Order {} already has mint tx {}; resuming without re-submission",
                order.id, existing.dest_tx
            );
            self.db.update_order_status(
                &order.id,
                &OrderStatus::MintPending,
                Some(&existing.dest_tx),
            )?;
            return Ok(());
        }

        // Threshold attestation from the validator federation (#824).
        let auth = self
            .attestation
            .authorize_mint(order)
            .await
            .map_err(|e| format!("attestation failed: {}", e))?;

        // Build + sign (no broadcast yet).
        let prepared = minter
            .prepare_mint(order, &auth)
            .await
            .map_err(|e| format!("prepare_mint failed: {}", e))?;

        // Persist the tx id BEFORE broadcast — the exactly-once guard.
        let record = self.db.record_mint_submitted(
            &order.id,
            &hex::encode(order.order_id_bytes()),
            order.dest_chain,
            &prepared.tx_id,
        )?;
        self.db
            .update_order_status(&order.id, &OrderStatus::MintPending, Some(&record.dest_tx))?;
        self.db.log_audit(
            Some(&order.id),
            "mint_submitted",
            &format!("chain={} tx={}", minter.chain(), record.dest_tx),
        )?;

        if record.dest_tx != prepared.tx_id {
            // Lost a race with another submission path: the recorded tx
            // wins; do not broadcast ours.
            warn!(
                "Order {} already recorded mint tx {}; discarding freshly prepared {}",
                order.id, record.dest_tx, prepared.tx_id
            );
            return Ok(());
        }

        // Broadcast the SAME signed tx with bounded retries. A persistent
        // failure leaves the order MintPending: the confirmation stage
        // detects a never-landed tx as dropped and unwinds it for
        // re-submission.
        let mut attempt = 0u32;
        loop {
            match minter.broadcast(&prepared).await {
                Ok(()) => {
                    info!(
                        "Broadcast mint tx {} for order {}",
                        prepared.tx_id, order.id
                    );
                    break;
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > self.config.bridge.max_retries {
                        warn!(
                            "Broadcast of {} failed after {} attempts: {}; leaving order {} \
                             MintPending for the confirmation stage to unwind",
                            prepared.tx_id, attempt, e, order.id
                        );
                        break;
                    }
                    warn!(
                        "Broadcast attempt {}/{} for {} failed: {}; retrying same signed tx",
                        attempt, self.config.bridge.max_retries, prepared.tx_id, e
                    );
                    tokio::time::sleep(BROADCAST_RETRY_DELAY).await;
                }
            }
        }

        Ok(())
    }

    /// Confirmation stage: `MintPending -> Completed` (or reorg unwind).
    ///
    /// `Completed` only fires once the minter reports the configured
    /// confirmation requirement met on a still-canonical block containing
    /// the mint event bound to this order id.
    async fn confirm_mint(&self, order: &BridgeOrder) -> Result<(), String> {
        let Some(minter) = self.minters.get(&order.dest_chain) else {
            return Err(format!(
                "no minter configured for chain {}",
                order.dest_chain
            ));
        };

        let dest_tx = match &order.dest_tx {
            Some(tx) => tx.clone(),
            None => match self.db.get_mint_by_order(&order.id)? {
                Some(record) => record.dest_tx,
                None => {
                    // MintPending with no recorded tx is inconsistent state
                    // (should be unreachable): unwind for re-submission.
                    warn!(
                        "Order {} is MintPending with no recorded mint tx; rolling back",
                        order.id
                    );
                    self.db.rollback_mint(&order.id)?;
                    return Ok(());
                }
            },
        };

        match minter
            .check_confirmation(order, &dest_tx)
            .await
            .map_err(|e| format!("check_confirmation failed: {}", e))?
        {
            ConfirmationStatus::Confirmed => {
                if !order.status.can_transition_to(&OrderStatus::Completed) {
                    return Err(format!(
                        "order {} cannot complete from status {}",
                        order.id, order.status
                    ));
                }
                self.db.mark_mint_confirmed(&order.id)?;
                self.db.log_audit(
                    Some(&order.id),
                    "mint_confirmed",
                    &format!("chain={} tx={}", order.dest_chain, dest_tx),
                )?;
                info!(
                    "Mint for order {} confirmed on {} (tx {})",
                    order.id, order.dest_chain, dest_tx
                );
            }
            ConfirmationStatus::Pending { confirmations } => {
                debug!(
                    "Mint tx {} for order {} at {} confirmation(s); waiting",
                    dest_tx, order.id, confirmations
                );
            }
            ConfirmationStatus::Reorged => {
                // Reorg unwind: MintPending -> DepositConfirmed. The next
                // processing tick re-runs submit_mint against the SAME
                // on-chain order id, so even if the old tx resurfaces the
                // contract-side guard (#826) keeps the mint exactly-once.
                warn!(
                    "Mint tx {} for order {} dropped/reorged before finality; \
                     rolling back to DepositConfirmed for re-submission",
                    dest_tx, order.id
                );
                self.db.rollback_mint(&order.id)?;
                self.db.log_audit(
                    Some(&order.id),
                    "mint_reorged",
                    &format!("chain={} tx={}", order.dest_chain, dest_tx),
                )?;
            }
            ConfirmationStatus::Failed { reason } => {
                warn!(
                    "Mint tx {} for order {} failed: {}",
                    dest_tx, order.id, reason
                );
                self.db
                    .update_order_status(&order.id, &OrderStatus::Failed { reason }, None)?;
            }
        }

        Ok(())
    }

    /// Process a burn order (burn confirmed, need to release BTH).
    async fn process_burn_order(&self, order: &BridgeOrder) -> Result<(), String> {
        info!(
            "Processing burn order {} for {} picocredits",
            order.id, order.amount
        );

        // TODO(#822): Implement actual BTH release (threshold-signed per
        // ADR 0002, reusing the operator-signed-action machinery).
        info!(
            "Would send {} BTH to {}",
            order.net_amount(),
            order.dest_address
        );

        self.db
            .update_order_status(&order.id, &OrderStatus::ReleasePending, None)?;

        Ok(())
    }

    /// Expire orders that have been waiting too long.
    fn expire_stale_orders(&self) -> Result<(), String> {
        let awaiting = self.db.get_orders_by_status("awaiting_deposit")?;

        for order in awaiting {
            if order.is_expired(self.config.bridge.order_expiry_minutes) {
                info!("Expiring stale order {}", order.id);
                self.db
                    .update_order_status(&order.id, &OrderStatus::Expired, None)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mint::{MintError, PreparedMint};
    use async_trait::async_trait;
    use bth_bridge_core::MintAuthorization;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    };

    /// Programmable in-memory minter for driving the engine in tests.
    struct MockMinter {
        chain: Chain,
        prepare_calls: AtomicU32,
        broadcast_calls: AtomicU32,
        next_tx: Mutex<String>,
        confirmation: Mutex<ConfirmationStatus>,
    }

    impl MockMinter {
        fn new(chain: Chain) -> Self {
            Self {
                chain,
                prepare_calls: AtomicU32::new(0),
                broadcast_calls: AtomicU32::new(0),
                next_tx: Mutex::new("0xmock_tx_1".to_string()),
                confirmation: Mutex::new(ConfirmationStatus::Pending { confirmations: 0 }),
            }
        }

        fn set_next_tx(&self, tx: &str) {
            *self.next_tx.lock().unwrap() = tx.to_string();
        }

        fn set_confirmation(&self, status: ConfirmationStatus) {
            *self.confirmation.lock().unwrap() = status;
        }
    }

    #[async_trait]
    impl Minter for MockMinter {
        fn chain(&self) -> Chain {
            self.chain
        }

        async fn prepare_mint(
            &self,
            _order: &BridgeOrder,
            _auth: &MintAuthorization,
        ) -> Result<PreparedMint, MintError> {
            self.prepare_calls.fetch_add(1, Ordering::SeqCst);
            Ok(PreparedMint {
                tx_id: self.next_tx.lock().unwrap().clone(),
                raw: vec![0xde, 0xad],
            })
        }

        async fn broadcast(&self, _prepared: &PreparedMint) -> Result<(), MintError> {
            self.broadcast_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn check_confirmation(
            &self,
            _order: &BridgeOrder,
            _dest_tx: &str,
        ) -> Result<ConfirmationStatus, MintError> {
            Ok(self.confirmation.lock().unwrap().clone())
        }
    }

    fn setup() -> (OrderProcessor, Arc<MockMinter>, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let minter = Arc::new(MockMinter::new(Chain::Ethereum));
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
        minters.insert(Chain::Ethereum, minter.clone());

        let processor = OrderProcessor::new(
            BridgeConfig::default(),
            db.clone(),
            minters,
            Arc::new(StubAttestationProvider),
        );
        (processor, minter, db)
    }

    fn insert_confirmed_deposit(db: &Database) -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&order).unwrap();
        order
    }

    #[tokio::test]
    async fn test_submit_then_confirmation_gating() {
        let (processor, minter, db) = setup();
        let order = insert_confirmed_deposit(&db);

        // Submission: DepositConfirmed -> MintPending with a recorded tx.
        processor.process_pending_orders().await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::MintPending);
        assert_eq!(stored.dest_tx.as_deref(), Some("0xmock_tx_1"));
        assert!(db.get_mint_by_order(&order.id).unwrap().is_some());
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 1);
        assert_eq!(minter.broadcast_calls.load(Ordering::SeqCst), 1);

        // Gating: while below the confirmation requirement the order must
        // NOT reach Completed.
        minter.set_confirmation(ConfirmationStatus::Pending { confirmations: 3 });
        processor.process_pending_orders().await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(
            stored.status,
            OrderStatus::MintPending,
            "MintPending must not advance before confirmations"
        );
        assert!(stored.dest_confirmed_at.is_none());

        // Confirmed at depth: now (and only now) Completed.
        minter.set_confirmation(ConfirmationStatus::Confirmed);
        processor.process_pending_orders().await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::Completed);
        assert!(stored.dest_confirmed_at.is_some());
        let mint = db.get_mint_by_order(&order.id).unwrap().unwrap();
        assert!(mint.confirmed_at.is_some());

        // No double-mint: prepare ran exactly once end to end.
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_resume_after_crash_reuses_recorded_tx() {
        let (processor, minter, db) = setup();
        let order = insert_confirmed_deposit(&db);

        // Simulate a crash AFTER the mint tx was persisted but BEFORE the
        // order status advanced: mints row exists, order still
        // DepositConfirmed.
        db.record_mint_submitted(
            &order.id,
            &hex::encode(order.order_id_bytes()),
            Chain::Ethereum,
            "0xpersisted_before_crash",
        )
        .unwrap();

        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::MintPending);
        assert_eq!(stored.dest_tx.as_deref(), Some("0xpersisted_before_crash"));
        // Exactly-once: no new tx was prepared or broadcast.
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 0);
        assert_eq!(minter.broadcast_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_reorg_unwinds_and_resubmits_same_order_id() {
        let (processor, minter, db) = setup();
        let order = insert_confirmed_deposit(&db);

        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::MintPending
        );

        // Reorg before finality: unwind to DepositConfirmed.
        minter.set_confirmation(ConfirmationStatus::Reorged);
        processor.process_pending_orders().await.unwrap();
        // (The same tick also re-submits, because submit runs before
        // confirm; inspect the final state after both stages.)
        let stored = db.get_order(&order.id).unwrap().unwrap();

        // Depending on tick interleaving the order is either back to
        // DepositConfirmed (unwound this tick, resubmitted next) or already
        // re-submitted. Drive one more tick with a fresh tx id to settle.
        minter.set_next_tx("0xmock_tx_2");
        minter.set_confirmation(ConfirmationStatus::Pending { confirmations: 0 });
        processor.process_pending_orders().await.unwrap();

        let stored2 = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored2.status, OrderStatus::MintPending);
        let mint = db.get_mint_by_order(&order.id).unwrap().unwrap();
        assert_eq!(mint.dest_tx, "0xmock_tx_2", "re-submission uses a fresh tx");
        // Same on-chain order id across both submissions (the double-mint
        // guard the contract enforces).
        assert_eq!(mint.order_id_hash, hex::encode(order.order_id_bytes()));
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 2);

        let _ = stored;
    }

    #[tokio::test]
    async fn test_failed_mint_marks_order_failed() {
        let (processor, minter, db) = setup();
        let order = insert_confirmed_deposit(&db);

        processor.process_pending_orders().await.unwrap();

        minter.set_confirmation(ConfirmationStatus::Failed {
            reason: "no BridgeMint event".to_string(),
        });
        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert!(matches!(stored.status, OrderStatus::Failed { .. }));
    }

    #[tokio::test]
    async fn test_unconfigured_chain_leaves_order_retryable() {
        let (processor, _minter, db) = setup();

        // A Solana order with no Solana minter configured: the order must
        // stay DepositConfirmed (retry later), NOT fail or complete.
        let mut order = BridgeOrder::new_mint(
            Chain::Solana,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            "So11111111111111111111111111111111111111112".to_string(),
        );
        order.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&order).unwrap();

        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositConfirmed);
    }

    #[tokio::test]
    async fn test_mint_to_bth_chain_fails_order() {
        let (processor, _minter, db) = setup();

        let mut order = BridgeOrder::new_mint(
            Chain::Bth,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            "other_bth_addr".to_string(),
        );
        order.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&order).unwrap();

        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert!(matches!(stored.status, OrderStatus::Failed { .. }));
    }
}
