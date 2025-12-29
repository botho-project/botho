use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Main configuration for Cadence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub wallet: WalletConfig,
    pub network: NetworkConfig,
    pub mining: MiningConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletConfig {
    /// BIP39 mnemonic phrase (24 words)
    pub mnemonic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Port for gossip (libp2p) connections
    #[serde(default = "default_gossip_port")]
    pub gossip_port: u16,

    /// Port for JSON-RPC server (for thin wallet connections)
    #[serde(default = "default_rpc_port")]
    pub rpc_port: u16,

    /// Bootstrap peers for initial discovery (multiaddr format)
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,

    /// Quorum configuration
    #[serde(default)]
    pub quorum: QuorumConfig,
}

fn default_gossip_port() -> u16 {
    7100
}

fn default_rpc_port() -> u16 {
    7101
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumConfig {
    /// Number of peers required to agree (e.g., 3 in a 3-of-5)
    #[serde(default = "default_threshold")]
    pub threshold: u32,

    /// Peer public keys in the quorum (max 5 for simplicity)
    #[serde(default)]
    pub members: Vec<String>,
}

impl Default for QuorumConfig {
    fn default() -> Self {
        Self {
            threshold: 3,
            members: Vec::new(),
        }
    }
}

fn default_threshold() -> u32 {
    3
}

/// Maximum quorum set members (keeps things simple)
pub const MAX_QUORUM_MEMBERS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningConfig {
    /// Whether mining is enabled
    #[serde(default)]
    pub enabled: bool,

    /// Number of mining threads (0 = auto-detect)
    #[serde(default = "default_threads")]
    pub threads: u32,
}

fn default_threads() -> u32 {
    0
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            gossip_port: default_gossip_port(),
            rpc_port: default_rpc_port(),
            bootstrap_peers: Vec::new(),
            quorum: QuorumConfig::default(),
        }
    }
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threads: 0,
        }
    }
}

impl Config {
    /// Create a new config with the given mnemonic
    pub fn new(mnemonic: String) -> Self {
        Self {
            wallet: WalletConfig { mnemonic },
            network: NetworkConfig::default(),
            mining: MiningConfig::default(),
        }
    }

    /// Load config from a file
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;

        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))
    }

    /// Save config to a file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;

        fs::write(path, contents)
            .with_context(|| format!("Failed to write config to {}", path.display()))?;

        // Set restrictive permissions on config file (contains secrets)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, perms)
                .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
        }

        Ok(())
    }

    /// Check if config file exists
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }
}

/// Get the default config directory path
pub fn default_data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".cadence")
}

/// Get the default config file path
pub fn default_config_path() -> PathBuf {
    default_data_dir().join("config.toml")
}

/// Get the ledger database path (relative to a base directory)
pub fn ledger_db_path() -> PathBuf {
    default_data_dir().join("ledger")
}

/// Get the ledger database path from config file path
pub fn ledger_db_path_from_config(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or(config_path)
        .join("ledger")
}

/// Get the wallet database path
pub fn wallet_db_path() -> PathBuf {
    default_data_dir().join("wallet")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::new("word1 word2 word3".to_string());
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.wallet.mnemonic, "word1 word2 word3");
    }
}
