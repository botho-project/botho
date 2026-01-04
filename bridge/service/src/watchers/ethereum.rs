// Copyright (c) 2024 The Botho Foundation

//! Ethereum chain watcher for monitoring wBTH burns.

use bth_bridge_core::EthereumConfig;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, info};

use crate::{db::Database, engine::ShutdownSignal};

/// Ethereum watcher monitors the wBTH contract for burn events.
#[allow(dead_code)]
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

            self.poll_for_burns().await;
        }
    }

    /// Poll for burn events.
    async fn poll_for_burns(&self) {
        // TODO: In a full implementation, we would:
        // 1. Use alloy to subscribe to BridgeBurn events
        // 2. Parse the burn amount and BTH address from the event
        // 3. Create a burn order in the database
        //
        // For now, we just poll periodically as a placeholder.

        debug!("Polling for Ethereum burn events...");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
