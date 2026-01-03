// Copyright (c) 2024 The Botho Foundation

//! Bridge engine - coordinates watchers and order processing.

use bth_bridge_core::{BridgeConfig, BridgeOrder, Chain, OrderStatus};
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::db::Database;
use crate::watchers::{BthWatcher, EthereumWatcher};

/// Shutdown signal type.
pub type ShutdownSignal = broadcast::Receiver<()>;

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
        let processor = OrderProcessor::new(self.config.clone(), self.db.clone());
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
}

impl OrderProcessor {
    fn new(config: BridgeConfig, db: Database) -> Self {
        Self { config, db }
    }

    /// Process all pending orders.
    async fn process_pending_orders(&self) -> Result<(), String> {
        // Process confirmed deposits (need to mint wBTH)
        let deposit_orders = self.db.get_orders_by_status("deposit_confirmed")?;
        for order in deposit_orders {
            if let Err(e) = self.process_mint_order(&order).await {
                warn!("Failed to process mint order {}: {}", order.id, e);
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

    /// Process a mint order (deposit confirmed, need to mint wBTH).
    async fn process_mint_order(&self, order: &BridgeOrder) -> Result<(), String> {
        info!("Processing mint order {} for {} picocredits", order.id, order.amount);

        match order.dest_chain {
            Chain::Ethereum => {
                // TODO: Implement actual ETH minting via alloy
                // For now, just log and update status
                info!("Would mint {} wBTH on Ethereum to {}", order.net_amount(), order.dest_address);

                // Simulate mint pending
                self.db.update_order_status(&order.id, &OrderStatus::MintPending, None)?;

                // In real implementation, we'd wait for confirmation then:
                // self.db.update_order_status(&order.id, &OrderStatus::Completed, Some(&tx_hash))?;
            }
            Chain::Solana => {
                // TODO: Implement actual Solana minting
                info!("Would mint {} wBTH on Solana to {}", order.net_amount(), order.dest_address);
                self.db.update_order_status(&order.id, &OrderStatus::MintPending, None)?;
            }
            Chain::Bth => {
                // Invalid - can't mint to BTH
                self.db.update_order_status(
                    &order.id,
                    &OrderStatus::Failed {
                        reason: "Cannot mint to BTH chain".to_string(),
                    },
                    None,
                )?;
            }
        }

        Ok(())
    }

    /// Process a burn order (burn confirmed, need to release BTH).
    async fn process_burn_order(&self, order: &BridgeOrder) -> Result<(), String> {
        info!("Processing burn order {} for {} picocredits", order.id, order.amount);

        // TODO: Implement actual BTH sending
        info!("Would send {} BTH to {}", order.net_amount(), order.dest_address);

        self.db.update_order_status(&order.id, &OrderStatus::ReleasePending, None)?;

        // In real implementation:
        // 1. Build BTH transaction
        // 2. Sign with hot wallet
        // 3. Submit via RPC
        // 4. Wait for confirmation
        // 5. Update status to Released

        Ok(())
    }

    /// Expire orders that have been waiting too long.
    fn expire_stale_orders(&self) -> Result<(), String> {
        let awaiting = self.db.get_orders_by_status("awaiting_deposit")?;

        for order in awaiting {
            if order.is_expired(self.config.bridge.order_expiry_minutes) {
                info!("Expiring stale order {}", order.id);
                self.db.update_order_status(&order.id, &OrderStatus::Expired, None)?;
            }
        }

        Ok(())
    }
}
