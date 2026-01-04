//! Output handlers for detected deposits.
//!
//! This module provides different ways to output detected deposits:
//! - Stdout (JSON lines)
//! - Webhook (HTTP POST)
//! - Database (placeholder)

mod stdout;
mod webhook;

pub use stdout::StdoutHandler;
pub use webhook::WebhookHandler;

use crate::deposit::DetectedDeposit;
use async_trait::async_trait;

/// Trait for deposit output handlers.
#[async_trait]
pub trait OutputHandler: Send + Sync {
    /// Handle a detected deposit.
    async fn handle(&self, deposit: &DetectedDeposit) -> anyhow::Result<()>;

    /// Handle a batch of deposits.
    async fn handle_batch(&self, deposits: &[DetectedDeposit]) -> anyhow::Result<()> {
        for deposit in deposits {
            self.handle(deposit).await?;
        }
        Ok(())
    }

    /// Flush any buffered output.
    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Create an output handler based on configuration.
pub fn create_handler(
    output_mode: &crate::config::OutputMode,
    webhook_url: Option<&str>,
    _database_url: Option<&str>,
) -> anyhow::Result<Box<dyn OutputHandler>> {
    match output_mode {
        crate::config::OutputMode::Stdout => Ok(Box::new(StdoutHandler::new())),
        crate::config::OutputMode::Webhook => {
            let url = webhook_url
                .ok_or_else(|| anyhow::anyhow!("webhook_url required for webhook output mode"))?;
            Ok(Box::new(WebhookHandler::new(url)?))
        }
        crate::config::OutputMode::Database => {
            anyhow::bail!("Database output mode not yet implemented")
        }
    }
}
