//! Configuration for the exchange scanner.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Output mode for detected deposits.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Print deposits to stdout as JSON lines
    #[default]
    Stdout,
    /// POST deposits to a webhook URL
    Webhook,
    /// Write deposits to a database (placeholder)
    Database,
}

/// Scanner configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannerConfig {
    /// View private key (64-character hex string, 32 bytes)
    pub view_private_key: String,

    /// Spend public key (64-character hex string, 32 bytes)
    pub spend_public_key: String,

    /// RPC endpoints to connect to (with failover support)
    #[serde(default = "default_rpc_endpoints")]
    pub rpc_endpoints: Vec<String>,

    /// Minimum subaddress index to scan (inclusive)
    #[serde(default)]
    pub subaddress_min: u64,

    /// Maximum subaddress index to scan (inclusive)
    #[serde(default = "default_subaddress_max")]
    pub subaddress_max: u64,

    /// Minimum confirmations before reporting a deposit
    #[serde(default = "default_min_confirmations")]
    pub min_confirmations: u64,

    /// State file for persisting sync progress
    #[serde(default = "default_state_file")]
    pub state_file: PathBuf,

    /// Output mode for detected deposits
    #[serde(default)]
    pub output_mode: OutputMode,

    /// Webhook URL (required if output_mode = webhook)
    pub webhook_url: Option<String>,

    /// Database connection string (required if output_mode = database)
    pub database_url: Option<String>,

    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Number of blocks to scan per batch
    #[serde(default = "default_batch_size")]
    pub batch_size: u64,
}

fn default_rpc_endpoints() -> Vec<String> {
    vec!["http://localhost:7101".to_string()]
}

fn default_subaddress_max() -> u64 {
    10_000
}

fn default_min_confirmations() -> u64 {
    10
}

fn default_state_file() -> PathBuf {
    PathBuf::from("scanner_state.json")
}

fn default_poll_interval() -> u64 {
    5
}

fn default_batch_size() -> u64 {
    100
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            view_private_key: String::new(),
            spend_public_key: String::new(),
            rpc_endpoints: default_rpc_endpoints(),
            subaddress_min: 0,
            subaddress_max: default_subaddress_max(),
            min_confirmations: default_min_confirmations(),
            state_file: default_state_file(),
            output_mode: OutputMode::default(),
            webhook_url: None,
            database_url: None,
            poll_interval_secs: default_poll_interval(),
            batch_size: default_batch_size(),
        }
    }
}

impl ScannerConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: ScannerConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> anyhow::Result<()> {
        // Check view private key format
        if self.view_private_key.len() != 64 {
            anyhow::bail!(
                "view_private_key must be 64 hex characters (32 bytes), got {}",
                self.view_private_key.len()
            );
        }
        hex::decode(&self.view_private_key)
            .map_err(|e| anyhow::anyhow!("view_private_key is not valid hex: {}", e))?;

        // Check spend public key format
        if self.spend_public_key.len() != 64 {
            anyhow::bail!(
                "spend_public_key must be 64 hex characters (32 bytes), got {}",
                self.spend_public_key.len()
            );
        }
        hex::decode(&self.spend_public_key)
            .map_err(|e| anyhow::anyhow!("spend_public_key is not valid hex: {}", e))?;

        // Check subaddress range
        if self.subaddress_max < self.subaddress_min {
            anyhow::bail!("subaddress_max must be >= subaddress_min");
        }

        // Warn about large ranges
        let range_size = self.subaddress_max - self.subaddress_min + 1;
        if range_size > 1_000_000 {
            tracing::warn!(
                "Large subaddress range ({} addresses) may use significant memory",
                range_size
            );
        }

        // Check RPC endpoints
        if self.rpc_endpoints.is_empty() {
            anyhow::bail!("At least one RPC endpoint must be specified");
        }

        // Check output mode requirements
        match self.output_mode {
            OutputMode::Webhook => {
                if self.webhook_url.is_none() {
                    anyhow::bail!("webhook_url is required when output_mode = webhook");
                }
            }
            OutputMode::Database => {
                if self.database_url.is_none() {
                    anyhow::bail!("database_url is required when output_mode = database");
                }
            }
            OutputMode::Stdout => {}
        }

        Ok(())
    }

    /// Get the subaddress range size.
    pub fn subaddress_count(&self) -> u64 {
        self.subaddress_max - self.subaddress_min + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ScannerConfig::default();
        assert_eq!(config.subaddress_min, 0);
        assert_eq!(config.subaddress_max, 10_000);
        assert_eq!(config.min_confirmations, 10);
        assert_eq!(config.output_mode, OutputMode::Stdout);
    }

    #[test]
    fn test_validate_missing_view_key() {
        let config = ScannerConfig {
            view_private_key: "abc".to_string(),
            spend_public_key: "0".repeat(64),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_range() {
        let config = ScannerConfig {
            view_private_key: "0".repeat(64),
            spend_public_key: "0".repeat(64),
            subaddress_min: 100,
            subaddress_max: 50,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }
}
