use anyhow::{anyhow, Context, Result};
use bth_transaction_types::Network;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Main configuration for Botho
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Network type (mainnet or testnet)
    #[serde(default)]
    pub network_type: Network,
    /// Wallet configuration (optional for relay/seed nodes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet: Option<WalletConfig>,
    pub network: NetworkConfig,
    pub mining: MiningConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletConfig {
    /// BIP39 mnemonic phrase (24 words)
    pub mnemonic: String,
}

impl Config {
    /// Check if this config has a wallet configured
    pub fn has_wallet(&self) -> bool {
        self.wallet.is_some()
    }

    /// Get the mnemonic if wallet is configured
    pub fn mnemonic(&self) -> Option<&str> {
        self.wallet.as_ref().map(|w| w.mnemonic.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Port for gossip (libp2p) connections
    #[serde(default = "default_gossip_port")]
    pub gossip_port: u16,

    /// Port for JSON-RPC server (for thin wallet connections)
    #[serde(default = "default_rpc_port")]
    pub rpc_port: u16,

    /// Allowed CORS origins for RPC server.
    /// Default is ["http://localhost:*", "http://127.0.0.1:*"] for security.
    /// Use ["*"] to allow all origins (not recommended for production).
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,

    /// Bootstrap peers for initial discovery (multiaddr format)
    /// Defaults to official seed nodes if not specified.
    #[serde(default = "default_bootstrap_peers")]
    pub bootstrap_peers: Vec<String>,

    /// Quorum configuration
    #[serde(default)]
    pub quorum: QuorumConfig,
}

fn default_cors_origins() -> Vec<String> {
    vec![
        "http://localhost".to_string(),
        "http://127.0.0.1".to_string(),
    ]
}

fn default_gossip_port() -> u16 {
    7100
}

fn default_rpc_port() -> u16 {
    7101
}

/// Default bootstrap peer for network discovery.
/// Format: /dns4/<hostname>/tcp/7100/p2p/<peer_id>
fn default_bootstrap_peers() -> Vec<String> {
    vec![
        // seed.botho.io (98.95.2.200)
        "/dns4/seed.botho.io/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ".to_string(),
    ]
}

/// Quorum configuration mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuorumMode {
    /// User explicitly lists trusted peer IDs
    Explicit,
    /// Automatically trust discovered peers (uses min_peers threshold)
    Recommended,
}

impl Default for QuorumMode {
    fn default() -> Self {
        Self::Recommended
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumConfig {
    /// Quorum mode: explicit (user lists peers) or recommended (auto-discover)
    #[serde(default)]
    pub mode: QuorumMode,

    /// For explicit mode: number of peers required to agree (e.g., 2 in a 2-of-3)
    /// For recommended mode: this is auto-calculated as ceil(2n/3)
    #[serde(default = "default_threshold")]
    pub threshold: u32,

    /// For explicit mode: peer IDs to trust (as base58 strings)
    #[serde(default)]
    pub members: Vec<String>,

    /// For recommended mode: minimum peers before mining can start
    #[serde(default = "default_min_peers")]
    pub min_peers: u32,
}

impl Default for QuorumConfig {
    fn default() -> Self {
        Self {
            mode: QuorumMode::Recommended,
            threshold: 2,
            members: Vec::new(),
            min_peers: 1,
        }
    }
}

impl QuorumConfig {
    /// Calculate the effective threshold for a given number of connected peers
    /// Uses BFT formula: threshold = n - floor((n-1)/3) ≈ ceil(2n/3)
    pub fn effective_threshold(&self, connected_count: usize) -> usize {
        match self.mode {
            QuorumMode::Explicit => self.threshold as usize,
            QuorumMode::Recommended => {
                // n = total nodes including self
                let n = connected_count + 1;
                // BFT threshold: n - f where f = floor((n-1)/3)
                let f = n.saturating_sub(1) / 3;
                n - f
            }
        }
    }

    /// Check if we can reach quorum with the given connected peers
    /// Returns (can_mine, quorum_size, threshold)
    pub fn can_reach_quorum(&self, connected_peer_ids: &[String]) -> (bool, usize, usize) {
        match self.mode {
            QuorumMode::Explicit => {
                // Count how many of our trusted members are connected
                let trusted_connected: usize = connected_peer_ids
                    .iter()
                    .filter(|p| self.members.contains(p))
                    .count();

                // Quorum includes self + trusted connected peers
                let quorum_size = trusted_connected + 1;
                let threshold = self.threshold as usize;

                (quorum_size >= threshold, quorum_size, threshold)
            }
            QuorumMode::Recommended => {
                // In recommended mode, we trust all connected peers
                let connected = connected_peer_ids.len();

                // Must have at least min_peers connected
                if connected < self.min_peers as usize {
                    return (false, connected + 1, self.min_peers as usize + 1);
                }

                // Quorum includes self + all connected peers
                let quorum_size = connected + 1;
                let threshold = self.effective_threshold(connected);

                (quorum_size >= threshold, quorum_size, threshold)
            }
        }
    }
}

fn default_threshold() -> u32 {
    2
}

fn default_min_peers() -> u32 {
    1
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
            cors_origins: default_cors_origins(),
            bootstrap_peers: default_bootstrap_peers(),
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
            wallet: Some(WalletConfig { mnemonic }),
            network: NetworkConfig::default(),
            mining: MiningConfig::default(),
        }
    }

    /// Create a new config without a wallet (for relay/seed nodes)
    pub fn new_relay() -> Self {
        Self {
            wallet: None,
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
        .join(".botho")
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
        assert_eq!(loaded.mnemonic(), Some("word1 word2 word3"));
    }

    #[test]
    fn test_config_relay_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::new_relay();
        assert!(!config.has_wallet());
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert!(!loaded.has_wallet());
        assert_eq!(loaded.mnemonic(), None);
    }

    #[test]
    fn test_quorum_config_default() {
        let quorum = QuorumConfig::default();
        assert_eq!(quorum.mode, QuorumMode::Recommended);
        assert_eq!(quorum.threshold, 2);
        assert_eq!(quorum.min_peers, 1);
        assert!(quorum.members.is_empty());
    }

    #[test]
    fn test_quorum_explicit_mode() {
        let quorum = QuorumConfig {
            mode: QuorumMode::Explicit,
            threshold: 2,
            members: vec!["peer1".to_string(), "peer2".to_string()],
            min_peers: 1,
        };

        // No peers connected - can't reach quorum
        let (can_mine, size, thresh) = quorum.can_reach_quorum(&[]);
        assert!(!can_mine);
        assert_eq!(size, 1); // just self
        assert_eq!(thresh, 2);

        // One trusted peer connected - can reach 2-of-2
        let (can_mine, size, thresh) = quorum.can_reach_quorum(&["peer1".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 2); // self + peer1
        assert_eq!(thresh, 2);

        // Untrusted peer connected - doesn't count
        let (can_mine, _, _) = quorum.can_reach_quorum(&["untrusted".to_string()]);
        assert!(!can_mine);
    }

    #[test]
    fn test_quorum_recommended_mode() {
        let quorum = QuorumConfig {
            mode: QuorumMode::Recommended,
            threshold: 2, // ignored in recommended mode
            members: vec![], // ignored in recommended mode
            min_peers: 1,
        };

        // No peers - can't mine
        let (can_mine, _, _) = quorum.can_reach_quorum(&[]);
        assert!(!can_mine);

        // One peer - can mine (2 nodes, threshold=2)
        let (can_mine, size, thresh) = quorum.can_reach_quorum(&["peer1".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 2);
        assert_eq!(thresh, 2); // 2-of-2

        // Two peers - can mine (3 nodes, threshold=3)
        // BFT with n=3: f=(3-1)/3=0, threshold=3-0=3
        let (can_mine, size, thresh) = quorum.can_reach_quorum(&[
            "peer1".to_string(),
            "peer2".to_string(),
        ]);
        assert!(can_mine);
        assert_eq!(size, 3);
        assert_eq!(thresh, 3); // 3-of-3 (can't tolerate any faults with 3 nodes)
    }

    #[test]
    fn test_quorum_effective_threshold() {
        let quorum = QuorumConfig::default();

        // BFT thresholds: threshold = n - floor((n-1)/3)
        // n=2: f=0, threshold=2
        // n=3: f=0, threshold=3
        // n=4: f=1, threshold=3
        // n=5: f=1, threshold=4
        // n=6: f=1, threshold=5
        // n=7: f=2, threshold=5
        assert_eq!(quorum.effective_threshold(1), 2);  // 2 nodes: 2-of-2
        assert_eq!(quorum.effective_threshold(2), 3);  // 3 nodes: 3-of-3
        assert_eq!(quorum.effective_threshold(3), 3);  // 4 nodes: 3-of-4
        assert_eq!(quorum.effective_threshold(4), 4);  // 5 nodes: 4-of-5
        assert_eq!(quorum.effective_threshold(5), 5);  // 6 nodes: 5-of-6
        assert_eq!(quorum.effective_threshold(6), 5);  // 7 nodes: 5-of-7
    }

    #[test]
    fn test_quorum_min_peers() {
        let quorum = QuorumConfig {
            mode: QuorumMode::Recommended,
            threshold: 2,
            members: vec![],
            min_peers: 2, // Require at least 2 peers
        };

        // One peer - not enough
        let (can_mine, _, _) = quorum.can_reach_quorum(&["peer1".to_string()]);
        assert!(!can_mine);

        // Two peers - enough (3 nodes, threshold=3 with BFT)
        let (can_mine, size, thresh) = quorum.can_reach_quorum(&[
            "peer1".to_string(),
            "peer2".to_string(),
        ]);
        assert!(can_mine);
        assert_eq!(size, 3);
        assert_eq!(thresh, 3);  // BFT: 3-of-3 for n=3
    }
}
