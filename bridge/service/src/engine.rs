// Copyright (c) 2024 The Botho Foundation

//! Bridge engine - coordinates watchers and order processing.

use bth_bridge_core::{BridgeConfig, BridgeOrder, Chain, OrderStatus};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::{
    api,
    attestation::{
        AttestationProvider, DisabledAttestationProvider, FederationAttestationProvider,
        StubAttestationProvider,
    },
    db::{Database, LimitCheck, LimitViolation},
    mint::{ethereum::EthMinter, solana::SolMinter, ConfirmationStatus, Minter},
    release::{bth::BthReleaser, PreparedRelease, ReleaseConfirmation, Releaser},
    reserve::Reconciler,
    watchers::{BthWatcher, EthereumWatcher, SolanaWatcher},
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

    /// Build the BTH reserve-release backend from configuration.
    ///
    /// If the releaser cannot be constructed (missing reserve address, bad
    /// federation key, unsatisfiable threshold, ...) release submission is
    /// disabled with a warning: confirmed burns stay in `BurnConfirmed`
    /// until the config is fixed — they are never dropped or failed.
    fn build_releaser(config: &BridgeConfig) -> Option<Arc<dyn Releaser>> {
        match BthReleaser::new(config.bth.clone()) {
            Ok(releaser) => Some(Arc::new(releaser)),
            Err(e) => {
                warn!("BTH release disabled: {}", e);
                None
            }
        }
    }

    /// Build the attestation provider (#824), plus an availability verdict
    /// for the health surface.
    ///
    /// - Federation configured and valid → the real
    ///   [`FederationAttestationProvider`].
    /// - Federation configured but INVALID → [`DisabledAttestationProvider`]
    ///   (fail-closed: authorizations error, orders stay retryable). Never
    ///   silently downgrades to the permissive stub.
    /// - No federation configured → the development stub.
    fn build_attestation_provider(
        config: &BridgeConfig,
    ) -> (Arc<dyn AttestationProvider>, bool, String) {
        match FederationAttestationProvider::from_config(config) {
            Ok(Some(provider)) => {
                info!("federation attestation provider active (ADR 0002 t-of-n custody)");
                (
                    Arc::new(provider),
                    true,
                    "federation provider active".to_string(),
                )
            }
            Ok(None) => {
                warn!(
                    "no attestation federation configured; using the development stub \
                     provider (threshold 0 — production authorities reject its output)"
                );
                (
                    Arc::new(StubAttestationProvider),
                    true,
                    "development stub provider (no federation configured)".to_string(),
                )
            }
            Err(e) => {
                error!(
                    "attestation federation misconfigured: {}; refusing to authorize any \
                     mint or release until the configuration is fixed",
                    e
                );
                let detail = format!("disabled: {}", e);
                (Arc::new(DisabledAttestationProvider::new(e)), false, detail)
            }
        }
    }

    /// Run the bridge engine.
    pub async fn run(self) -> Result<(), String> {
        info!("Starting bridge engine");

        // Config-level kill switch: start paused until an operator resumes
        // (the runtime state lives in the DB and otherwise survives
        // restarts on its own).
        if self.config.bridge.paused && self.db.set_paused(true, Some("paused via config"))? {
            self.db.log_audit(
                None,
                "breaker_tripped",
                "bridge.paused = true in configuration",
            )?;
            warn!("Bridge starting PAUSED (bridge.paused = true in config)");
        }

        let minters = Self::build_minters(&self.config);
        let releaser = Self::build_releaser(&self.config);
        let (attestation, attestation_ok, attestation_detail) =
            Self::build_attestation_provider(&self.config);

        // Record component availability for the health surface
        // (`/api/status`, `/metrics`). The engine already fails closed on
        // each unavailable component (orders stay retryable); this makes
        // the degradation observable instead of silent.
        self.db
            .set_component_health("attestation", attestation_ok, &attestation_detail)?;
        for chain in [Chain::Ethereum, Chain::Solana] {
            let configured = minters.contains_key(&chain);
            self.db.set_component_health(
                &format!("minter:{}", chain),
                configured,
                if configured { "configured" } else { "disabled" },
            )?;
        }
        self.db.set_component_health(
            "releaser:bth",
            releaser.is_some(),
            if releaser.is_some() {
                "configured"
            } else {
                "disabled"
            },
        )?;

        let processor = OrderProcessor::new(
            self.config.clone(),
            self.db.clone(),
            minters,
            releaser,
            attestation,
        );

        // Startup reconciliation (#843): roll stranded orders forward
        // exactly once BEFORE any watcher or processing loop runs (no
        // writer races while recovering).
        processor.recover_on_startup()?;

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

        // Spawn the Solana watcher
        let sol_watcher = SolanaWatcher::new(
            self.config.solana.clone(),
            self.db.clone(),
            self.shutdown_tx.subscribe(),
        );

        let sol_handle = tokio::spawn(async move {
            if let Err(e) = sol_watcher.run().await {
                error!("Solana watcher error: {}", e);
            }
        });

        // Spawn the order processing loop
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

        // Spawn the reserve reconciler (#825): periodic peg-invariant
        // check (DB-derived locked reserve vs on-chain wrapped supply).
        let reconciler = Reconciler::from_config(&self.config, self.db.clone());
        let reconcile_interval =
            Duration::from_secs(self.config.reserve.reconcile_interval_secs.max(1));
        let reconcile_shutdown = self.shutdown_tx.subscribe();
        let reconcile_handle = tokio::spawn(async move {
            reconciler.run(reconcile_interval, reconcile_shutdown).await;
        });

        // Spawn the bridge HTTP API (#825 proof-of-reserves + #827
        // monitoring/breaker surface); empty listen address disables it.
        let api_handle = if self.config.reserve.api_listen.is_empty() {
            None
        } else {
            let addr = self.config.reserve.api_listen.clone();
            let api_db = self.db.clone();
            let api_shutdown = self.shutdown_tx.subscribe();
            let stuck_after_secs = self.config.bridge.order_expiry_minutes.max(1) * 60;
            Some(tokio::spawn(async move {
                if let Err(e) = api::serve(addr, api_db, stuck_after_secs, api_shutdown).await {
                    error!("Bridge API error: {}", e);
                }
            }))
        };

        // Handle shutdown signals: SIGINT (ctrl-c) and, on unix, SIGTERM
        // (what systemd sends). Both trigger the same graceful drain:
        // every component gets the broadcast and is joined below.
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(|e| format!("failed to install SIGTERM handler: {}", e))?;
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Received SIGINT; shutting down gracefully");
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM; shutting down gracefully");
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            info!("Received shutdown signal");
        }

        // Send shutdown signal to all components
        let _ = self.shutdown_tx.send(());

        // Wait for all components to finish
        let _ = tokio::join!(
            bth_handle,
            eth_handle,
            sol_handle,
            process_handle,
            reconcile_handle
        );
        if let Some(handle) = api_handle {
            let _ = handle.await;
        }

        info!("Bridge engine stopped");
        Ok(())
    }
}

/// Order processor handles pending orders.
pub(crate) struct OrderProcessor {
    config: BridgeConfig,
    db: Database,
    minters: HashMap<Chain, Arc<dyn Minter>>,
    releaser: Option<Arc<dyn Releaser>>,
    attestation: Arc<dyn AttestationProvider>,
}

impl OrderProcessor {
    pub(crate) fn new(
        config: BridgeConfig,
        db: Database,
        minters: HashMap<Chain, Arc<dyn Minter>>,
        releaser: Option<Arc<dyn Releaser>>,
        attestation: Arc<dyn AttestationProvider>,
    ) -> Self {
        Self {
            config,
            db,
            minters,
            releaser,
            attestation,
        }
    }

    /// Startup reconciliation, run BEFORE any watcher or processing loop.
    ///
    /// #843: a crash between the BTH watcher's detect step and its confirm
    /// step strands an order at `DepositDetected` forever — the watcher's
    /// replay skips it ("not awaiting a deposit") and the processor only
    /// picks up orders from `deposit_confirmed` onward. Detection implies
    /// finality in the BTH watcher (scanned blocks always meet the
    /// finality requirement), so any `DepositDetected` order with a
    /// durably recorded `source_tx` is rolled forward to
    /// `DepositConfirmed` (and its deposit marked processed — the other
    /// half of the same crash window), exactly once, with an audit entry.
    ///
    /// Orders stranded at `MintPending` / `ReleasePending` /
    /// `DepositConfirmed` / `BurnConfirmed` need no startup action: the
    /// processing stages resume them via the `mints` / `release_claims`
    /// exactly-once tables.
    pub(crate) fn recover_on_startup(&self) -> Result<(), String> {
        let detected = self.db.get_orders_by_status("deposit_detected")?;
        for order in detected {
            let Some(source_tx) = order.source_tx.clone() else {
                // DepositDetected without a recorded tx should be
                // unreachable (the detect write records both atomically);
                // surface it rather than guess.
                warn!(
                    "Order {} is DepositDetected with no recorded source_tx; leaving for triage",
                    order.id
                );
                continue;
            };

            // Crash-between-detect-and-mark window: the idempotency row
            // may be missing; write it so the watcher's replay of the
            // block short-circuits cleanly.
            self.db.mark_deposit_processed(&source_tx, &order.id)?;
            self.db
                .update_order_status(&order.id, &OrderStatus::DepositConfirmed, None)?;
            self.db.log_audit(
                Some(&order.id),
                "deposit_recovered",
                &format!(
                    "tx={} recovered DepositDetected -> DepositConfirmed on startup (#843)",
                    source_tx
                ),
            )?;
            info!(
                "Recovered stranded order {} (DepositDetected -> DepositConfirmed, tx {})",
                order.id, source_tx
            );
        }

        Ok(())
    }

    /// Process all pending orders.
    ///
    /// Submission (`DepositConfirmed -> MintPending`) and confirmation
    /// (`MintPending -> Completed`) are driven as separate retryable stages
    /// so a crash or RPC failure between them never loses (or duplicates)
    /// a mint.
    ///
    /// Circuit breaker: when the bridge is paused (kill switch or
    /// auto-trip) the SUBMIT stages are halted — no new value leaves the
    /// bridge — while the CONFIRM stages keep running so already-broadcast
    /// transactions settle to a durable terminal state.
    pub(crate) async fn process_pending_orders(&self) -> Result<(), String> {
        // Automatic breaker conditions (fail closed) are evaluated before
        // any submission.
        self.check_breaker_conditions()?;

        match self.db.is_paused()? {
            None => {
                // Stage 1: confirmed deposits need a mint submitted.
                let deposit_orders = self.db.get_orders_by_status("deposit_confirmed")?;
                for order in deposit_orders {
                    if let Err(e) = self.submit_mint(&order).await {
                        warn!("Failed to submit mint for order {}: {}", order.id, e);
                    }
                }

                // Stage 3: confirmed burns need a BTH release submitted.
                let burn_orders = self.db.get_orders_by_status("burn_confirmed")?;
                for order in burn_orders {
                    if let Err(e) = self.submit_release(&order).await {
                        warn!("Failed to submit release for order {}: {}", order.id, e);
                    }
                }
            }
            Some(reason) => {
                debug!(
                    "Bridge paused ({}); submit stages halted, confirm stages continue",
                    reason
                );
            }
        }

        // Stage 2: submitted mints need confirmation (or reorg unwind).
        let pending_mints = self.db.get_orders_by_status("mint_pending")?;
        for order in pending_mints {
            if let Err(e) = self.confirm_mint(&order).await {
                warn!("Failed to confirm mint for order {}: {}", order.id, e);
            }
        }

        // Stage 4: submitted releases need confirmation (or unwind).
        let pending_releases = self.db.get_orders_by_status("release_pending")?;
        for order in pending_releases {
            if let Err(e) = self.confirm_release(&order).await {
                warn!("Failed to confirm release for order {}: {}", order.id, e);
            }
        }

        // Check for expired orders
        self.expire_stale_orders()?;

        // Alert on orders stuck past the age threshold (funds have moved;
        // they must never auto-expire — an operator has to act).
        self.alert_stuck_orders()?;

        Ok(())
    }

    /// Evaluate the automatic circuit-breaker conditions and trip the
    /// breaker (fail closed) when one fires. The reconciler additionally
    /// trips it on any peg-drift alert (see `reserve::Reconciler`).
    fn check_breaker_conditions(&self) -> Result<(), String> {
        if self.db.is_paused()?.is_some() {
            return Ok(());
        }

        let max_pending = self.config.bridge.max_pending_orders;
        if max_pending > 0 {
            let backlog = self.db.actionable_backlog()?;
            if backlog > max_pending {
                self.trip_breaker(&format!(
                    "actionable backlog {} exceeds max_pending_orders {}",
                    backlog, max_pending
                ))?;
            }
        }

        Ok(())
    }

    /// Trip the circuit breaker: pause the submit stages, audit-log the
    /// trip exactly once.
    fn trip_breaker(&self, reason: &str) -> Result<(), String> {
        if self.db.set_paused(true, Some(reason))? {
            self.db.log_audit(None, "breaker_tripped", reason)?;
            error!(
                "CIRCUIT BREAKER TRIPPED: {}; submit stages halted (confirm stages continue). \
                 Resume via POST /api/breaker after triage — see \
                 docs/operations/runbooks/bridge-order-engine-recovery.md",
                reason
            );
        }
        Ok(())
    }

    /// Rate-limit gate for a submit stage: reserve the order's volume
    /// against the per-order cap and the daily windows, exactly once per
    /// order. Returns `Ok(true)` when the order may proceed.
    ///
    /// A daily-window rejection defers the order (retryable next window);
    /// exhausting the GLOBAL window additionally trips the circuit
    /// breaker — bridge-wide volume at the cap is an anomaly signal.
    fn reserve_order_limits(&self, order: &BridgeOrder) -> Result<bool, String> {
        use bth_bridge_core::OrderType;

        // The counterparty whose daily window this order consumes: the
        // wBTH recipient for mints, the wBTH burner for burns.
        let address = match order.order_type {
            OrderType::Mint => &order.dest_address,
            OrderType::Burn => &order.source_address,
        };

        match self.db.check_and_reserve_limits(
            &order.id,
            address,
            order.amount,
            self.config.bridge.max_order_amount,
            self.config.bridge.daily_limit_per_address,
            self.config.bridge.global_daily_limit,
            chrono::Utc::now().timestamp(),
        )? {
            LimitCheck::Reserved | LimitCheck::AlreadyReserved => Ok(true),
            LimitCheck::Rejected(violation) => {
                if !self.db.has_audit_for_order(&order.id, "rate_limited")? {
                    self.db.log_audit(
                        Some(&order.id),
                        "rate_limited",
                        &format!("violation={} amount={}", violation, order.amount),
                    )?;
                }
                warn!(
                    "Order {} deferred by rate limit ({}; amount {})",
                    order.id, violation, order.amount
                );
                if violation == LimitViolation::GlobalDailyCap {
                    self.trip_breaker(&format!(
                        "global daily volume cap reached (order {} amount {})",
                        order.id, order.amount
                    ))?;
                }
                Ok(false)
            }
        }
    }

    /// Audit-log (exactly once per order) any order stuck past the age
    /// threshold in a post-deposit stage.
    fn alert_stuck_orders(&self) -> Result<(), String> {
        let threshold_secs = self.config.bridge.order_expiry_minutes.max(1) * 60;
        for order in self.db.stuck_orders(threshold_secs)? {
            if self
                .db
                .has_audit_for_order(&order.id, "stuck_order_alert")?
            {
                continue;
            }
            let age_secs = (chrono::Utc::now() - order.updated_at).num_seconds();
            self.db.log_audit(
                Some(&order.id),
                "stuck_order_alert",
                &format!(
                    "status={} age_secs={} threshold_secs={}",
                    order.status, age_secs, threshold_secs
                ),
            )?;
            warn!(
                "Order {} stuck in {} for {}s (threshold {}s) — operator attention needed",
                order.id, order.status, age_secs, threshold_secs
            );
        }
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

        // Rate limits (#827): mirror the contract-side caps service-side.
        // A deferred order stays DepositConfirmed and retries next window
        // (the deposit already happened — never dropped).
        if !self.reserve_order_limits(order)? {
            return Ok(());
        }

        // Reserve accounting (#825, ADR 0003): the confirmed deposit's
        // backing (net amount — the fee is bridge revenue, not peg
        // backing) enters the locked ledger exactly once, BEFORE the mint
        // is submitted, so the peg invariant is maintainable from the
        // moment supply can appear. Idempotent by output id across ticks.
        if self.db.record_locked_output(
            &format!("dep:{}", order.id),
            order.dest_chain,
            order.net_amount(),
            &order.id,
        )? {
            self.db.log_audit(
                Some(&order.id),
                "reserve_locked",
                &format!(
                    "chain={} amount={} tx={:?}",
                    order.dest_chain,
                    order.net_amount(),
                    order.source_tx
                ),
            )?;
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
        self.db.log_audit(
            Some(&order.id),
            "attestation_authorized",
            &format!(
                "action=bridge.mint_wbth chain={} threshold={} signers={}",
                order.dest_chain,
                auth.threshold,
                auth.signatures.len()
            ),
        )?;

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
                // Reserve accounting (#825): a failed mint's deposit no
                // longer backs wrapped supply — it is owed back to the
                // depositor. Unlock its VALUE (#846: the specific dep:
                // output may already have been consumed by a release's
                // FIFO spend) so the peg ledger stays exact. A shortfall
                // is audited and surfaces as drift — never blocks the
                // failure transition (already durably recorded above).
                match self.db.unlock_backing_for_order(
                    &order.id,
                    order.dest_chain,
                    order.net_amount(),
                ) {
                    Ok(true) => {
                        self.db.log_audit(
                            Some(&order.id),
                            "reserve_unlocked",
                            &format!(
                                "chain={} amount={} mint failed; deposit no longer backs supply",
                                order.dest_chain,
                                order.net_amount()
                            ),
                        )?;
                    }
                    Ok(false) => {} // replay: already unlocked
                    Err(e) => {
                        error!(
                            "Reserve unlock for failed mint order {} failed: {}",
                            order.id, e
                        );
                        self.db
                            .log_audit(Some(&order.id), "reserve_unlock_failed", &e)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Release submission stage: `BurnConfirmed -> ReleasePending`.
    ///
    /// Exactly-once: a durable claim in the `release_claims` table is taken
    /// BEFORE any signing, and the signed transaction (hash + raw bytes) is
    /// recorded BEFORE the first broadcast. A crash or concurrent tick at
    /// any point either finds no recorded tx (nothing was ever broadcast —
    /// safe to sign fresh) or finds the recorded tx and reuses it — the
    /// engine NEVER signs a second release with different reserve inputs,
    /// which could double-spend the reserve (BTH has no on-chain order-id
    /// guard; the claims table is the release-side exactly-once guard).
    async fn submit_release(&self, order: &BridgeOrder) -> Result<(), String> {
        info!(
            "Processing burn order {}: release {} picocredits to {}",
            order.id,
            order.net_amount(),
            order.dest_address
        );

        if order.dest_chain != Chain::Bth {
            self.db.update_order_status(
                &order.id,
                &OrderStatus::Failed {
                    reason: format!(
                        "burn orders release on the BTH chain, not {}",
                        order.dest_chain
                    ),
                },
                None,
            )?;
            return Ok(());
        }

        let Some(releaser) = self.releaser.as_ref() else {
            // Not failed: releasing is unconfigured/disabled. The order
            // stays BurnConfirmed and is retried when the operator fixes
            // the configuration.
            return Err("BTH release not configured".to_string());
        };

        // Rate limits (#827): a deferred burn stays BurnConfirmed and
        // retries next window — a confirmed burn is owed BTH and is never
        // failed by a limit, only delayed and surfaced as stuck.
        if !self.reserve_order_limits(order)? {
            return Ok(());
        }

        // Durable exactly-once claim, taken BEFORE any signing/submission.
        let claim = self
            .db
            .try_claim_release(&order.id, &hex::encode(order.order_id_bytes()))?;

        // Idempotency: a previously signed tx (crash between persistence
        // and status update, or a re-poll) is reused, never re-signed.
        if let Some(existing_tx) = claim.release_tx_hash {
            info!(
                "Order {} already has release tx {}; resuming without re-signing",
                order.id, existing_tx
            );
            self.db.update_order_status(
                &order.id,
                &OrderStatus::ReleasePending,
                Some(&existing_tx),
            )?;
            // Re-broadcast the exact recorded bytes (idempotent; "already
            // known" is success). A failure here is non-fatal: the
            // confirmation stage keeps polling and this path re-runs.
            if let Some(raw) = claim.release_tx_raw {
                let prepared = PreparedRelease {
                    tx_hash: existing_tx.clone(),
                    raw,
                };
                if let Err(e) = releaser.broadcast(&prepared).await {
                    warn!(
                        "Re-broadcast of recorded release tx {} for order {} failed: {}",
                        existing_tx, order.id, e
                    );
                }
            }
            return Ok(());
        }

        // Threshold attestation from the validator federation (#824),
        // bound to this exact order id, amount, and recipient. The
        // releaser re-verifies every signature before touching reserve key
        // material.
        let auth = self
            .attestation
            .authorize_release(order)
            .await
            .map_err(|e| format!("attestation failed: {}", e))?;
        self.db.log_audit(
            Some(&order.id),
            "attestation_authorized",
            &format!(
                "action=bridge.release_bth threshold={} signers={}",
                auth.threshold,
                auth.signatures.len()
            ),
        )?;

        // Build + threshold-sign (no broadcast yet). Nothing was recorded
        // in the claim, so a failure here (including the #856
        // NotImplemented stub) leaves the order BurnConfirmed for a clean
        // retry — no reserve funds have moved.
        let prepared = releaser
            .prepare_release(order, &auth)
            .await
            .map_err(|e| format!("prepare_release failed: {}", e))?;

        // Persist the signed tx BEFORE broadcast — the exactly-once guard.
        let record = self
            .db
            .record_release_tx(&order.id, &prepared.tx_hash, &prepared.raw)?;
        let recorded_tx = record
            .release_tx_hash
            .ok_or_else(|| "release tx missing after record".to_string())?;
        self.db
            .update_order_status(&order.id, &OrderStatus::ReleasePending, Some(&recorded_tx))?;
        self.db.log_audit(
            Some(&order.id),
            "release_submitted",
            &format!(
                "chain=bth tx={} amount={} recipient={}",
                recorded_tx,
                order.net_amount(),
                order.dest_address
            ),
        )?;

        if recorded_tx != prepared.tx_hash {
            // Lost a race with another submission path: the recorded tx
            // wins; do not broadcast ours.
            warn!(
                "Order {} already recorded release tx {}; discarding freshly signed {}",
                order.id, recorded_tx, prepared.tx_hash
            );
            return Ok(());
        }

        // Broadcast the SAME signed tx with bounded retries. A persistent
        // failure leaves the order ReleasePending: the resume path
        // re-broadcasts the recorded bytes and the confirmation stage
        // detects a provably-dead tx and unwinds it for re-submission.
        let mut attempt = 0u32;
        loop {
            match releaser.broadcast(&prepared).await {
                Ok(()) => {
                    info!(
                        "Broadcast release tx {} for order {}",
                        prepared.tx_hash, order.id
                    );
                    break;
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > self.config.bridge.max_retries {
                        warn!(
                            "Broadcast of {} failed after {} attempts: {}; leaving order {} \
                             ReleasePending for resume/confirmation to handle",
                            prepared.tx_hash, attempt, e, order.id
                        );
                        break;
                    }
                    warn!(
                        "Broadcast attempt {}/{} for {} failed: {}; retrying same signed tx",
                        attempt, self.config.bridge.max_retries, prepared.tx_hash, e
                    );
                    tokio::time::sleep(BROADCAST_RETRY_DELAY).await;
                }
            }
        }

        Ok(())
    }

    /// Release confirmation stage: `ReleasePending -> Released` (or unwind).
    ///
    /// `Released` only fires once the releaser reports the configured
    /// confirmation requirement met (`release_confirmations_required`;
    /// 0 = SCP externalization finality).
    async fn confirm_release(&self, order: &BridgeOrder) -> Result<(), String> {
        let Some(releaser) = self.releaser.as_ref() else {
            return Err("BTH release not configured".to_string());
        };

        let dest_tx = match &order.dest_tx {
            Some(tx) => tx.clone(),
            None => match self
                .db
                .get_release_by_order(&order.id)?
                .and_then(|c| c.release_tx_hash)
            {
                Some(tx) => tx,
                None => {
                    // ReleasePending with no recorded tx is inconsistent
                    // state (should be unreachable): unwind for
                    // re-submission — nothing was ever signed/broadcast.
                    warn!(
                        "Order {} is ReleasePending with no recorded release tx; rolling back",
                        order.id
                    );
                    self.db.rollback_release(&order.id)?;
                    return Ok(());
                }
            },
        };

        match releaser
            .check_confirmation(order, &dest_tx)
            .await
            .map_err(|e| format!("check_confirmation failed: {}", e))?
        {
            ReleaseConfirmation::Confirmed => {
                if !order.status.can_transition_to(&OrderStatus::Released) {
                    return Err(format!(
                        "order {} cannot be released from status {}",
                        order.id, order.status
                    ));
                }
                // Reserve accounting (#825): spend locked outputs for the
                // GROSS burn amount (the on-chain supply dropped by the
                // full burn; the fee stays in custody as revenue). Applied
                // BEFORE the order leaves `release_pending` so a crash
                // between the two replays the idempotent spend next tick
                // instead of losing it.
                match self
                    .db
                    .apply_release_spend(&order.id, order.source_chain, order.amount)
                {
                    Ok(true) => {
                        self.db.log_audit(
                            Some(&order.id),
                            "reserve_spent",
                            &format!(
                                "chain={} amount={} tx={}",
                                order.source_chain, order.amount, dest_tx
                            ),
                        )?;
                    }
                    Ok(false) => {} // replay: already applied
                    Err(e) => {
                        // Do not block the release confirmation: the funds
                        // already moved on-chain. The ledger mismatch will
                        // surface as drift on the next reconciliation.
                        error!("Reserve spend for release order {} failed: {}", order.id, e);
                        self.db
                            .log_audit(Some(&order.id), "reserve_spend_failed", &e)?;
                    }
                }
                self.db.mark_release_confirmed(&order.id)?;
                self.db.log_audit(
                    Some(&order.id),
                    "release_confirmed",
                    &format!("chain=bth tx={}", dest_tx),
                )?;
                info!(
                    "Release for order {} confirmed on BTH (tx {})",
                    order.id, dest_tx
                );
            }
            ReleaseConfirmation::Pending { confirmations } => {
                debug!(
                    "Release tx {} for order {} at {} confirmation(s); waiting",
                    dest_tx, order.id, confirmations
                );
            }
            ReleaseConfirmation::Dropped => {
                // Unwind: ReleasePending -> BurnConfirmed. Only reported
                // when the recorded tx PROVABLY cannot land (its key images
                // were spent by a different tx, or it is permanently
                // invalid), so re-signing a fresh tx on the next tick
                // cannot double-release.
                warn!(
                    "Release tx {} for order {} provably dropped; \
                     rolling back to BurnConfirmed for re-submission",
                    dest_tx, order.id
                );
                self.db.rollback_release(&order.id)?;
                self.db.log_audit(
                    Some(&order.id),
                    "release_dropped",
                    &format!("chain=bth tx={}", dest_tx),
                )?;
            }
            ReleaseConfirmation::Failed { reason } => {
                warn!(
                    "Release tx {} for order {} failed: {}",
                    dest_tx, order.id, reason
                );
                // The claim is left intact deliberately: a Failed release
                // needs operator attention, and any retry must reuse the
                // recorded tx rather than sign a competing reserve spend.
                self.db
                    .update_order_status(&order.id, &OrderStatus::Failed { reason }, None)?;
            }
        }

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
    use crate::{
        mint::{MintError, PreparedMint},
        release::ReleaseError,
    };
    use async_trait::async_trait;
    use bth_bridge_core::{MintAuthorization, ReleaseAuthorization};
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

    /// Programmable in-memory releaser for driving the engine in tests.
    struct MockReleaser {
        prepare_calls: AtomicU32,
        broadcast_calls: AtomicU32,
        next_tx: Mutex<String>,
        confirmation: Mutex<ReleaseConfirmation>,
        /// Attestations seen by prepare_release (order-binding assertions).
        last_auth: Mutex<Option<ReleaseAuthorization>>,
    }

    impl MockReleaser {
        fn new() -> Self {
            Self {
                prepare_calls: AtomicU32::new(0),
                broadcast_calls: AtomicU32::new(0),
                next_tx: Mutex::new("bth_release_tx_1".to_string()),
                confirmation: Mutex::new(ReleaseConfirmation::Pending { confirmations: 0 }),
                last_auth: Mutex::new(None),
            }
        }

        fn set_next_tx(&self, tx: &str) {
            *self.next_tx.lock().unwrap() = tx.to_string();
        }

        fn set_confirmation(&self, status: ReleaseConfirmation) {
            *self.confirmation.lock().unwrap() = status;
        }
    }

    #[async_trait]
    impl Releaser for MockReleaser {
        async fn prepare_release(
            &self,
            _order: &BridgeOrder,
            auth: &ReleaseAuthorization,
        ) -> Result<PreparedRelease, ReleaseError> {
            self.prepare_calls.fetch_add(1, Ordering::SeqCst);
            *self.last_auth.lock().unwrap() = Some(auth.clone());
            Ok(PreparedRelease {
                tx_hash: self.next_tx.lock().unwrap().clone(),
                raw: vec![0xca, 0xfe],
            })
        }

        async fn broadcast(&self, _prepared: &PreparedRelease) -> Result<(), ReleaseError> {
            self.broadcast_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn check_confirmation(
            &self,
            _order: &BridgeOrder,
            _dest_tx: &str,
        ) -> Result<ReleaseConfirmation, ReleaseError> {
            Ok(self.confirmation.lock().unwrap().clone())
        }
    }

    fn setup() -> (OrderProcessor, Arc<MockMinter>, Database) {
        let (processor, minter, _releaser, db) = setup_full();
        (processor, minter, db)
    }

    fn setup_full() -> (OrderProcessor, Arc<MockMinter>, Arc<MockReleaser>, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let minter = Arc::new(MockMinter::new(Chain::Ethereum));
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
        minters.insert(Chain::Ethereum, minter.clone());

        let releaser = Arc::new(MockReleaser::new());

        let processor = OrderProcessor::new(
            BridgeConfig::default(),
            db.clone(),
            minters,
            Some(releaser.clone()),
            Arc::new(StubAttestationProvider),
        );
        (processor, minter, releaser, db)
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

    // === Release (burn) path ===

    fn insert_confirmed_burn(db: &Database) -> BridgeOrder {
        let mut order = BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_user_stealth_addr".to_string(),
            "0xburntx".to_string(),
        );
        order.set_status(OrderStatus::BurnConfirmed);
        db.insert_order(&order).unwrap();
        order
    }

    #[tokio::test]
    async fn test_release_submit_then_finality_gating() {
        let (processor, _minter, releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        // Submission: BurnConfirmed -> ReleasePending with a recorded tx
        // (claim taken and tx recorded BEFORE broadcast).
        processor.process_pending_orders().await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::ReleasePending);
        assert_eq!(stored.dest_tx.as_deref(), Some("bth_release_tx_1"));
        let claim = db.get_release_by_order(&order.id).unwrap().unwrap();
        assert_eq!(claim.release_tx_hash.as_deref(), Some("bth_release_tx_1"));
        assert_eq!(claim.release_tx_raw.as_deref(), Some(&[0xca, 0xfe][..]));
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 1);
        assert_eq!(releaser.broadcast_calls.load(Ordering::SeqCst), 1);

        // The attestation handed to the releaser was bound to THIS order:
        // id, net amount, and recipient.
        let auth = releaser.last_auth.lock().unwrap().clone().unwrap();
        assert_eq!(auth.order_id, order.order_id_bytes());
        assert_eq!(auth.amount, order.net_amount());
        assert_eq!(auth.recipient, order.dest_address);

        // Gating: while below the confirmation requirement the order must
        // NOT reach Released.
        releaser.set_confirmation(ReleaseConfirmation::Pending { confirmations: 0 });
        processor.process_pending_orders().await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(
            stored.status,
            OrderStatus::ReleasePending,
            "ReleasePending must not advance before finality"
        );
        assert!(stored.dest_confirmed_at.is_none());

        // Finality reached (SCP externalization by default): Released.
        releaser.set_confirmation(ReleaseConfirmation::Confirmed);
        processor.process_pending_orders().await.unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::Released);
        assert!(stored.dest_confirmed_at.is_some());
        let claim = db.get_release_by_order(&order.id).unwrap().unwrap();
        assert!(claim.confirmed_at.is_some());

        // No double-release: the tx was signed exactly once end to end.
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_release_resume_after_crash_reuses_recorded_tx() {
        let (processor, _minter, releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        // Simulate a crash AFTER the release tx was signed and persisted
        // but BEFORE the order status advanced: claim row with a recorded
        // tx exists, order still BurnConfirmed.
        db.try_claim_release(&order.id, &hex::encode(order.order_id_bytes()))
            .unwrap();
        db.record_release_tx(&order.id, "bth_persisted_before_crash", &[0xaa])
            .unwrap();

        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::ReleasePending);
        assert_eq!(
            stored.dest_tx.as_deref(),
            Some("bth_persisted_before_crash")
        );
        // Exactly-once: NO new tx was signed — the recorded one was reused
        // (and re-broadcast, which is idempotent).
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 0);
        assert_eq!(releaser.broadcast_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_release_crash_after_claim_before_sign_is_safe() {
        let (processor, _minter, releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        // Crash AFTER the claim was taken but BEFORE anything was signed:
        // claim exists with no recorded tx. Nothing was ever broadcast, so
        // signing fresh on resume is safe — and must happen exactly once.
        db.try_claim_release(&order.id, &hex::encode(order.order_id_bytes()))
            .unwrap();

        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::ReleasePending);
        assert_eq!(stored.dest_tx.as_deref(), Some("bth_release_tx_1"));
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_release_replayed_burn_single_release() {
        let (processor, _minter, releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        // Multiple ticks over the same confirmed burn (a replayed burn
        // event re-marking the order, or overlapping ticks) must produce
        // exactly one signed release.
        processor.process_pending_orders().await.unwrap();
        processor.process_pending_orders().await.unwrap();
        processor.process_pending_orders().await.unwrap();

        assert_eq!(
            releaser.prepare_calls.load(Ordering::SeqCst),
            1,
            "a burn order must be signed exactly once"
        );
        let claim = db.get_release_by_order(&order.id).unwrap().unwrap();
        assert_eq!(claim.release_tx_hash.as_deref(), Some("bth_release_tx_1"));

        // Even if the order is forced back to BurnConfirmed (replayed burn
        // confirmation), the recorded tx is reused — never re-signed.
        db.update_order_status(&order.id, &OrderStatus::BurnConfirmed, None)
            .unwrap();
        processor.process_pending_orders().await.unwrap();
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 1);
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::ReleasePending);
        assert_eq!(stored.dest_tx.as_deref(), Some("bth_release_tx_1"));
    }

    #[tokio::test]
    async fn test_release_dropped_unwinds_and_resubmits() {
        let (processor, _minter, releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::ReleasePending
        );

        // Provably-dead tx: unwind to BurnConfirmed; the next tick signs a
        // fresh tx (safe only because Dropped guarantees the old one can
        // never land).
        releaser.set_confirmation(ReleaseConfirmation::Dropped);
        processor.process_pending_orders().await.unwrap();

        releaser.set_next_tx("bth_release_tx_2");
        releaser.set_confirmation(ReleaseConfirmation::Pending { confirmations: 0 });
        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::ReleasePending);
        let claim = db.get_release_by_order(&order.id).unwrap().unwrap();
        assert_eq!(
            claim.release_tx_hash.as_deref(),
            Some("bth_release_tx_2"),
            "re-submission after a provably-dead tx signs a fresh tx"
        );
        assert_eq!(claim.order_id_hash, hex::encode(order.order_id_bytes()));
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_release_failed_marks_order_failed_keeps_claim() {
        let (processor, _minter, releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        processor.process_pending_orders().await.unwrap();

        releaser.set_confirmation(ReleaseConfirmation::Failed {
            reason: "wrong recipient output".to_string(),
        });
        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert!(matches!(stored.status, OrderStatus::Failed { .. }));
        // The claim survives so any operator-driven retry reuses the
        // recorded tx instead of signing a competing reserve spend.
        assert!(db.get_release_by_order(&order.id).unwrap().is_some());
    }

    #[tokio::test]
    async fn test_release_unconfigured_leaves_order_retryable() {
        // No releaser configured: the burn order must stay BurnConfirmed
        // (retry when the operator fixes the config), NOT fail or advance.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let processor = OrderProcessor::new(
            BridgeConfig::default(),
            db.clone(),
            HashMap::new(),
            None,
            Arc::new(StubAttestationProvider),
        );
        let order = insert_confirmed_burn(&db);

        processor.process_pending_orders().await.unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::BurnConfirmed);
        assert!(db.get_release_by_order(&order.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_release_pending_without_tx_rolls_back() {
        let (processor, _minter, _releaser, db) = setup_full();
        let order = insert_confirmed_burn(&db);

        // Force the inconsistent state: ReleasePending with no claim.
        db.update_order_status(&order.id, &OrderStatus::ReleasePending, None)
            .unwrap();

        processor.process_pending_orders().await.unwrap();

        // The confirm stage unwinds it; the SAME tick's submit stage has
        // already run, so it lands back at BurnConfirmed and is
        // re-submitted next tick.
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::BurnConfirmed);
    }

    // === Reserve accounting wiring (#825) ===

    #[tokio::test]
    async fn test_mint_lifecycle_locks_net_backing_exactly_once() {
        let (processor, minter, db) = setup();
        let order = insert_confirmed_deposit(&db);

        // Submission locks the NET amount (fee = revenue, not backing).
        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.locked_reserve_total().unwrap(), order.net_amount());
        assert_eq!(
            db.locked_reserve_by_chain(Chain::Ethereum).unwrap(),
            order.net_amount()
        );
        assert_eq!(db.count_audit_action("reserve_locked").unwrap(), 1);

        // Repeated ticks and confirmation never double-lock.
        minter.set_confirmation(ConfirmationStatus::Confirmed);
        processor.process_pending_orders().await.unwrap();
        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.locked_reserve_total().unwrap(), order.net_amount());
        assert_eq!(db.count_audit_action("reserve_locked").unwrap(), 1);
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::Completed
        );
    }

    #[tokio::test]
    async fn test_failed_mint_unlocks_backing() {
        let (processor, minter, db) = setup();
        let order = insert_confirmed_deposit(&db);

        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.locked_reserve_total().unwrap(), order.net_amount());

        minter.set_confirmation(ConfirmationStatus::Failed {
            reason: "no BridgeMint event".to_string(),
        });
        processor.process_pending_orders().await.unwrap();

        // The failed mint's deposit no longer backs supply.
        assert_eq!(db.locked_reserve_total().unwrap(), 0);
        assert_eq!(db.count_audit_action("reserve_unlocked").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_release_lifecycle_spends_gross_burn_from_reserve() {
        let (processor, _minter, releaser, db) = setup_full();

        // Seed the reserve as if prior mints locked 1.5 BTH of backing.
        let prior_mint = uuid::Uuid::new_v4();
        db.record_locked_output("dep:seed", Chain::Ethereum, 1_500_000_000_000, &prior_mint)
            .unwrap();

        // Burn of 1 BTH (gross); the release pays net to the user.
        let order = insert_confirmed_burn(&db);
        processor.process_pending_orders().await.unwrap();

        // Not spent while ReleasePending.
        assert_eq!(db.locked_reserve_total().unwrap(), 1_500_000_000_000);

        releaser.set_confirmation(ReleaseConfirmation::Confirmed);
        processor.process_pending_orders().await.unwrap();

        // Released: the GROSS burn left the ledger, change stayed locked.
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::Released
        );
        assert_eq!(
            db.locked_reserve_total().unwrap(),
            1_500_000_000_000 - order.amount
        );
        assert_eq!(db.count_audit_action("reserve_spent").unwrap(), 1);

        // Replayed ticks never double-spend.
        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.locked_reserve_total().unwrap(),
            1_500_000_000_000 - order.amount
        );
        assert_eq!(db.count_audit_action("reserve_spent").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_release_spend_shortfall_alerts_but_never_blocks_release() {
        let (processor, _minter, releaser, db) = setup_full();

        // No locked backing at all (a drift condition): the release must
        // still confirm — the funds already moved on-chain — while the
        // ledger mismatch is audited for the reconciler to surface.
        let order = insert_confirmed_burn(&db);
        processor.process_pending_orders().await.unwrap();
        releaser.set_confirmation(ReleaseConfirmation::Confirmed);
        processor.process_pending_orders().await.unwrap();

        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::Released
        );
        assert_eq!(db.count_audit_action("reserve_spend_failed").unwrap(), 1);
        assert_eq!(db.count_audit_action("reserve_spent").unwrap(), 0);
    }

    // === Circuit breaker + rate limits + monitoring (#827) ===

    #[tokio::test]
    async fn test_kill_switch_halts_submits_but_confirms_settle() {
        let (processor, minter, releaser, db) = setup_full();

        // An in-flight mint (already broadcast) and a not-yet-submitted
        // deposit.
        let inflight = insert_confirmed_deposit(&db);
        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.get_order(&inflight.id).unwrap().unwrap().status,
            OrderStatus::MintPending
        );
        let waiting = insert_confirmed_deposit(&db);
        let burn = insert_confirmed_burn(&db);

        // Trip the kill switch.
        assert!(db.set_paused(true, Some("manual")).unwrap());

        // Confirm stage still settles the in-flight order...
        minter.set_confirmation(ConfirmationStatus::Confirmed);
        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.get_order(&inflight.id).unwrap().unwrap().status,
            OrderStatus::Completed,
            "confirm stages must keep running while paused"
        );

        // ...but no NEW value leaves the bridge: the waiting deposit and
        // the confirmed burn are untouched.
        assert_eq!(
            db.get_order(&waiting.id).unwrap().unwrap().status,
            OrderStatus::DepositConfirmed,
            "submit stages must halt while paused"
        );
        assert_eq!(
            db.get_order(&burn.id).unwrap().unwrap().status,
            OrderStatus::BurnConfirmed
        );
        assert_eq!(releaser.prepare_calls.load(Ordering::SeqCst), 0);

        // Resume: everything proceeds.
        assert!(db.set_paused(false, None).unwrap());
        minter.set_confirmation(ConfirmationStatus::Pending { confirmations: 0 });
        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.get_order(&waiting.id).unwrap().unwrap().status,
            OrderStatus::MintPending
        );
        assert_eq!(
            db.get_order(&burn.id).unwrap().unwrap().status,
            OrderStatus::ReleasePending
        );
    }

    #[tokio::test]
    async fn test_breaker_auto_trips_on_backlog() {
        let (mut config, db) = (BridgeConfig::default(), {
            let db = Database::open_in_memory().unwrap();
            db.migrate().unwrap();
            db
        });
        config.bridge.max_pending_orders = 2;

        let minter = Arc::new(MockMinter::new(Chain::Ethereum));
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
        minters.insert(Chain::Ethereum, minter.clone());
        let processor = OrderProcessor::new(
            config,
            db.clone(),
            minters,
            None,
            Arc::new(StubAttestationProvider),
        );

        // Backlog of 3 actionable orders > max_pending_orders of 2.
        for _ in 0..3 {
            insert_confirmed_deposit(&db);
        }

        processor.process_pending_orders().await.unwrap();

        let reason = db.is_paused().unwrap().expect("breaker must trip");
        assert!(reason.contains("backlog"), "{}", reason);
        assert_eq!(db.count_audit_action("breaker_tripped").unwrap(), 1);
        // Fail closed: nothing was submitted this tick.
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 0);

        // The trip is audited exactly once across further ticks.
        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.count_audit_action("breaker_tripped").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_per_order_cap_defers_mint() {
        let (mut config, db) = (BridgeConfig::default(), {
            let db = Database::open_in_memory().unwrap();
            db.migrate().unwrap();
            db
        });
        // Cap below the order amount.
        config.bridge.max_order_amount = 1_000;

        let minter = Arc::new(MockMinter::new(Chain::Ethereum));
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
        minters.insert(Chain::Ethereum, minter.clone());
        let processor = OrderProcessor::new(
            config,
            db.clone(),
            minters,
            None,
            Arc::new(StubAttestationProvider),
        );

        let order = insert_confirmed_deposit(&db); // amount 1 BTH >> 1_000
        processor.process_pending_orders().await.unwrap();

        // Deferred, not failed, and never submitted.
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::DepositConfirmed
        );
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 0);
        assert_eq!(db.count_audit_action("rate_limited").unwrap(), 1);
        // The rejection is audited exactly once per order.
        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.count_audit_action("rate_limited").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_global_cap_trips_breaker() {
        let (mut config, db) = (BridgeConfig::default(), {
            let db = Database::open_in_memory().unwrap();
            db.migrate().unwrap();
            db
        });
        config.bridge.global_daily_limit = 1_000; // below one order

        let minter = Arc::new(MockMinter::new(Chain::Ethereum));
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
        minters.insert(Chain::Ethereum, minter.clone());
        let processor = OrderProcessor::new(
            config,
            db.clone(),
            minters,
            None,
            Arc::new(StubAttestationProvider),
        );

        insert_confirmed_deposit(&db);
        processor.process_pending_orders().await.unwrap();

        // Anomalous volume: the global window exhausting trips the
        // breaker (fail closed).
        let reason = db.is_paused().unwrap().expect("breaker must trip");
        assert!(reason.contains("global daily volume"), "{}", reason);
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_stuck_order_alert_fires_once() {
        let (processor, _minter, db) = setup();

        // A MintPending order that has not advanced for 3 hours (default
        // expiry threshold is 60 minutes). Funds moved: it must alert,
        // never auto-expire.
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.set_status(OrderStatus::MintPending);
        // Submitted tx that the (mock) chain keeps reporting as Pending —
        // without it confirm_mint would unwind the order instead.
        order.dest_tx = Some("0xstuck_mint_tx".to_string());
        order.updated_at = chrono::Utc::now() - chrono::Duration::hours(3);
        db.insert_order(&order).unwrap();

        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.count_audit_action("stuck_order_alert").unwrap(), 1);
        assert!(!db
            .get_order(&order.id)
            .unwrap()
            .unwrap()
            .status
            .is_terminal());

        // Exactly once per order, not per tick.
        processor.process_pending_orders().await.unwrap();
        assert_eq!(db.count_audit_action("stuck_order_alert").unwrap(), 1);
    }

    // === Startup recovery (#843) ===

    #[tokio::test]
    async fn test_recover_stranded_deposit_detected_order() {
        let (processor, minter, db) = setup();

        // Simulate the #843 crash window: the watcher recorded the detect
        // (status + amount + source_tx durably written) and died before
        // marking the deposit processed or confirming.
        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bridge_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        db.insert_order(&order).unwrap();
        assert!(db
            .record_deposit_detected(&order.id, "0xdeposit", 999_000_000_000)
            .unwrap());

        // Startup recovery rolls it forward and closes both crash halves.
        processor.recover_on_startup().unwrap();
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositConfirmed);
        assert!(db.is_deposit_processed("0xdeposit").unwrap());
        assert_eq!(db.count_audit_action("deposit_recovered").unwrap(), 1);

        // Recovery is idempotent and the order then completes exactly once.
        processor.recover_on_startup().unwrap();
        assert_eq!(db.count_audit_action("deposit_recovered").unwrap(), 1);

        processor.process_pending_orders().await.unwrap();
        minter.set_confirmation(ConfirmationStatus::Confirmed);
        processor.process_pending_orders().await.unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::Completed
        );
        assert_eq!(minter.prepare_calls.load(Ordering::SeqCst), 1);
    }
}
