// Copyright (c) 2024 The Botho Foundation

//! BTH chain watcher for monitoring deposits.

use bth_bridge_core::BthConfig;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, info};

use crate::{db::Database, engine::ShutdownSignal};

/// BTH watcher monitors the BTH chain for deposits to the bridge address.
#[allow(dead_code)]
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
        info!("Starting BTH watcher for {}", self.config.rpc_url);

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

            self.poll_for_deposits().await;
        }
    }

    /// Poll for new deposits.
    async fn poll_for_deposits(&self) {
        // TODO: In a full implementation, we would:
        // 1. Connect to BTH WebSocket at self.config.ws_url
        // 2. Subscribe to NewBlock events
        // 3. For each block, scan outputs for bridge's stealth address
        // 4. Use view private key for stealth detection
        // 5. Match deposits to pending orders via encrypted memo
        //
        // For now, we just poll periodically as a placeholder.

        debug!("Polling for BTH deposits...");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
