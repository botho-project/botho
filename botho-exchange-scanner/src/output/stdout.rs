//! Stdout output handler.
//!
//! Outputs deposits as JSON lines to stdout, suitable for piping to other tools.

use super::OutputHandler;
use crate::deposit::DetectedDeposit;
use async_trait::async_trait;

/// Handler that prints deposits to stdout as JSON lines.
pub struct StdoutHandler {
    /// Whether to use pretty printing
    pretty: bool,
}

impl StdoutHandler {
    /// Create a new stdout handler.
    pub fn new() -> Self {
        Self { pretty: false }
    }

    /// Create a new stdout handler with pretty printing.
    pub fn pretty() -> Self {
        Self { pretty: true }
    }
}

impl Default for StdoutHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputHandler for StdoutHandler {
    async fn handle(&self, deposit: &DetectedDeposit) -> anyhow::Result<()> {
        let output = if self.pretty {
            deposit.to_json_pretty()
        } else {
            deposit.to_json()
        };

        println!("{}", output);
        Ok(())
    }

    async fn handle_batch(&self, deposits: &[DetectedDeposit]) -> anyhow::Result<()> {
        for deposit in deposits {
            self.handle(deposit).await?;
        }
        Ok(())
    }
}
