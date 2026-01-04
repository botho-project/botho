//! Webhook output handler.
//!
//! Posts deposits to a configured webhook URL.

use super::OutputHandler;
use crate::deposit::DetectedDeposit;
use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

/// Handler that POSTs deposits to a webhook URL.
pub struct WebhookHandler {
    /// HTTP client
    client: Client,
    /// Webhook URL
    url: String,
    /// Number of retries on failure
    max_retries: u32,
    /// Retry delay
    retry_delay: Duration,
}

impl WebhookHandler {
    /// Create a new webhook handler.
    pub fn new(url: &str) -> anyhow::Result<Self> {
        let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

        Ok(Self {
            client,
            url: url.to_string(),
            max_retries: 3,
            retry_delay: Duration::from_secs(1),
        })
    }

    /// Create a webhook handler with custom retry settings.
    pub fn with_retries(
        url: &str,
        max_retries: u32,
        retry_delay: Duration,
    ) -> anyhow::Result<Self> {
        let mut handler = Self::new(url)?;
        handler.max_retries = max_retries;
        handler.retry_delay = retry_delay;
        Ok(handler)
    }

    async fn post_with_retry(&self, deposit: &DetectedDeposit) -> anyhow::Result<()> {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                tracing::warn!(
                    "Webhook retry {} for deposit {}",
                    attempt,
                    deposit.deposit_id()
                );
                tokio::time::sleep(self.retry_delay * attempt).await;
            }

            match self.post_once(deposit).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown webhook error")))
    }

    async fn post_once(&self, deposit: &DetectedDeposit) -> anyhow::Result<()> {
        let response = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(deposit)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Webhook returned status {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            );
        }

        tracing::debug!("Posted deposit {} to webhook", deposit.deposit_id());

        Ok(())
    }
}

#[async_trait]
impl OutputHandler for WebhookHandler {
    async fn handle(&self, deposit: &DetectedDeposit) -> anyhow::Result<()> {
        self.post_with_retry(deposit).await
    }

    async fn handle_batch(&self, deposits: &[DetectedDeposit]) -> anyhow::Result<()> {
        // Post batch as array for efficiency
        let response = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(deposits)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Webhook batch returned status {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            );
        }

        tracing::debug!("Posted {} deposits to webhook", deposits.len());

        Ok(())
    }
}
