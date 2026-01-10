use anyhow::{anyhow, Context, Result};
use bth_transaction_types::constants::Network;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

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
    pub minting: MintingConfig,
    /// Faucet configuration for testnet coin distribution
    #[serde(default)]
    pub faucet: FaucetConfig,
    /// Telemetry configuration for distributed tracing
    #[serde(default)]
    pub telemetry: TelemetryConfig,
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
    /// Port for gossip (libp2p) connections.
    /// If not set, uses network-specific default (7100 for mainnet, 17100 for
    /// testnet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gossip_port: Option<u16>,

    /// Port for JSON-RPC server (for thin wallet connections).
    /// If not set, uses network-specific default (7101 for mainnet, 17101 for
    /// testnet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_port: Option<u16>,

    /// Port for Prometheus metrics endpoint.
    /// If not set, uses network-specific default (9090 for mainnet, 19090 for
    /// testnet). Set to 0 to disable the metrics server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics_port: Option<u16>,

    /// Allowed CORS origins for RPC server.
    /// Default is ["http://localhost:*", "http://127.0.0.1:*"] for security.
    /// Use ["*"] to allow all origins (not recommended for production).
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,

    /// Bootstrap peers for initial discovery (multiaddr format).
    /// If not set, uses DNS discovery or network-specific seed nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bootstrap_peers: Vec<String>,

    /// DNS seed discovery configuration.
    /// When enabled, seeds are discovered via DNS TXT records.
    #[serde(default)]
    pub dns_seeds: DnsSeedConfig,

    /// Quorum configuration
    #[serde(default)]
    pub quorum: QuorumConfig,

    /// API keys for authenticated exchange endpoints.
    /// If empty, authentication is disabled for exchange endpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<ApiKeyEntry>,

    /// Maximum connections allowed per IP address for Sybil protection.
    /// Set to 0 to disable rate limiting. Default: 10.
    #[serde(default = "default_max_connections_per_ip")]
    pub max_connections_per_ip: u32,

    /// IP addresses exempt from connection rate limiting.
    /// Use for known validators or trusted infrastructure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connection_whitelist: Vec<String>,
}

/// API key entry for exchange authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyEntry {
    /// Unique identifier for this API key
    pub key_id: String,
    /// Secret key for HMAC signing
    pub key_secret: String,
    /// Permissions for this key
    #[serde(default)]
    pub permissions: ApiKeyPermissions,
    /// Rate limit (requests per minute)
    #[serde(default = "default_rate_limit")]
    pub rate_limit: u32,
    /// Optional IP whitelist (empty = allow all)
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
}

fn default_rate_limit() -> u32 {
    100
}

fn default_max_connections_per_ip() -> u32 {
    10
}

/// Permissions for an API key.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiKeyPermissions {
    /// Can access exchange-specific endpoints
    #[serde(default)]
    pub exchange_api: bool,
    /// Can register view keys for deposit notifications
    #[serde(default)]
    pub register_view_keys: bool,
    /// Can submit transactions
    #[serde(default)]
    pub submit_transactions: bool,
}

fn default_cors_origins() -> Vec<String> {
    vec![
        "http://localhost".to_string(),
        "http://127.0.0.1".to_string(),
    ]
}

/// DNS seed discovery configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsSeedConfig {
    /// Enable DNS-based seed discovery.
    /// When true, queries DNS TXT records for bootstrap peers.
    /// Default: true
    #[serde(default = "default_dns_seeds_enabled")]
    pub enabled: bool,

    /// Custom DNS seed domain (overrides network default).
    /// Default domains:
    /// - Mainnet: seeds.botho.io
    /// - Testnet: seeds.testnet.botho.io
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

fn default_dns_seeds_enabled() -> bool {
    true
}

impl Default for DnsSeedConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            domain: None,
        }
    }
}

/// Default bootstrap peers for network discovery, by network.
fn default_bootstrap_peers(network: Network) -> Vec<String> {
    match network {
        Network::Mainnet => vec![
            // Mainnet seed node
            "/dns4/seed.botho.io/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ".to_string(),
        ],
        Network::Testnet => vec![
            // Testnet seed node
            "/dns4/testnet.botho.io/tcp/17100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ".to_string(),
        ],
    }
}

impl NetworkConfig {
    /// Get the gossip port, using network default if not explicitly set
    pub fn gossip_port(&self, network: Network) -> u16 {
        self.gossip_port
            .unwrap_or_else(|| network.default_gossip_port())
    }

    /// Get the RPC port, using network default if not explicitly set
    pub fn rpc_port(&self, network: Network) -> u16 {
        self.rpc_port.unwrap_or_else(|| network.default_rpc_port())
    }

    /// Get the metrics port, using network default if not explicitly set.
    ///
    /// Returns None if metrics are disabled (port set to 0).
    /// Default ports: 9090 for mainnet, 19090 for testnet.
    pub fn metrics_port(&self, network: Network) -> Option<u16> {
        match self.metrics_port {
            Some(0) => None, // Explicitly disabled
            Some(port) => Some(port),
            None => {
                // Network-specific defaults
                Some(match network {
                    Network::Mainnet => 9090,
                    Network::Testnet => 19090,
                })
            }
        }
    }

    /// Get bootstrap peers synchronously (uses hardcoded seeds, not DNS).
    ///
    /// For DNS-based discovery, use `bootstrap_peers_async` instead.
    pub fn bootstrap_peers(&self, network: Network) -> Vec<String> {
        if self.bootstrap_peers.is_empty() {
            default_bootstrap_peers(network)
        } else {
            self.bootstrap_peers.clone()
        }
    }

    /// Get bootstrap peers asynchronously, using DNS discovery if enabled.
    ///
    /// Priority:
    /// 1. Explicitly configured bootstrap_peers (if not empty)
    /// 2. DNS TXT record discovery (if enabled)
    /// 3. Hardcoded fallback seeds
    pub async fn bootstrap_peers_async(&self, network: Network) -> Vec<String> {
        // If explicit bootstrap peers are configured, use them
        if !self.bootstrap_peers.is_empty() {
            return self.bootstrap_peers.clone();
        }

        // If DNS discovery is enabled, try it
        if self.dns_seeds.enabled {
            use crate::network::DnsSeedDiscovery;

            let discovery = if let Some(ref domain) = self.dns_seeds.domain {
                DnsSeedDiscovery::with_domain(network, domain.clone())
            } else {
                DnsSeedDiscovery::new(network)
            };

            return discovery.discover_seeds().await;
        }

        // Fall back to hardcoded seeds
        default_bootstrap_peers(network)
    }

    /// Parse the connection whitelist strings into IpAddr values.
    /// Invalid addresses are logged and skipped.
    pub fn parsed_connection_whitelist(&self) -> Vec<std::net::IpAddr> {
        self.connection_whitelist
            .iter()
            .filter_map(|s| {
                s.parse::<std::net::IpAddr>().ok().or_else(|| {
                    tracing::warn!("Invalid IP address in connection whitelist: {}", s);
                    None
                })
            })
            .collect()
    }
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

    /// For explicit mode: number of peers required to agree (e.g., 2 in a
    /// 2-of-3) For recommended mode: this is auto-calculated as ceil(2n/3)
    #[serde(default = "default_threshold")]
    pub threshold: u32,

    /// For explicit mode: peer IDs to trust (as base58 strings)
    #[serde(default)]
    pub members: Vec<String>,

    /// For recommended mode: minimum peers before minting can start
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
pub struct MintingConfig {
    /// Whether minting is enabled
    #[serde(default)]
    pub enabled: bool,

    /// Number of minting threads (0 = auto-detect)
    #[serde(default = "default_threads")]
    pub threads: u32,
}

fn default_threads() -> u32 {
    0
}

/// Faucet configuration for testnet coin distribution.
///
/// The faucet allows users to request testnet coins for testing purposes.
/// It includes rate limiting to prevent abuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaucetConfig {
    /// Whether the faucet is enabled.
    /// Default: false (must be explicitly enabled)
    #[serde(default)]
    pub enabled: bool,

    /// Amount to dispense per request in picocredits.
    /// Default: 10 BTH (10_000_000_000_000 picocredits)
    #[serde(default = "default_faucet_amount")]
    pub amount: u64,

    /// Maximum requests per IP address per hour.
    /// Default: 5
    #[serde(default = "default_faucet_per_ip_hourly_limit")]
    pub per_ip_hourly_limit: u32,

    /// Maximum requests per destination address per 24 hours.
    /// Default: 3
    #[serde(default = "default_faucet_per_address_daily_limit")]
    pub per_address_daily_limit: u32,

    /// Maximum total BTH to dispense per day (in picocredits).
    /// Default: 10,000 BTH (10_000_000_000_000_000 picocredits)
    #[serde(default = "default_faucet_daily_limit")]
    pub daily_limit: u64,

    /// Minimum seconds between requests from the same IP.
    /// Default: 60 seconds
    #[serde(default = "default_faucet_cooldown")]
    pub cooldown_secs: u64,
}

/// 10 BTH in picocredits
fn default_faucet_amount() -> u64 {
    10_000_000_000_000
}

fn default_faucet_per_ip_hourly_limit() -> u32 {
    5
}

fn default_faucet_per_address_daily_limit() -> u32 {
    3
}

/// 10,000 BTH in picocredits
fn default_faucet_daily_limit() -> u64 {
    10_000_000_000_000_000
}

fn default_faucet_cooldown() -> u64 {
    60
}

impl Default for FaucetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: default_faucet_amount(),
            per_ip_hourly_limit: default_faucet_per_ip_hourly_limit(),
            per_address_daily_limit: default_faucet_per_address_daily_limit(),
            daily_limit: default_faucet_daily_limit(),
            cooldown_secs: default_faucet_cooldown(),
        }
    }
}

/// Telemetry configuration for distributed tracing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether telemetry export is enabled
    #[serde(default)]
    pub enabled: bool,

    /// OTLP endpoint (gRPC) for trace export
    #[serde(default = "default_telemetry_endpoint")]
    pub endpoint: String,

    /// Service name for traces
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// Sampling rate (0.0 to 1.0)
    /// 0.1 = 10% of traces, 1.0 = all traces
    #[serde(default = "default_sampling_rate")]
    pub sampling_rate: f64,
}

fn default_telemetry_endpoint() -> String {
    "http://localhost:4317".to_string()
}

fn default_service_name() -> String {
    "botho-node".to_string()
}

fn default_sampling_rate() -> f64 {
    1.0
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_telemetry_endpoint(),
            service_name: default_service_name(),
            sampling_rate: default_sampling_rate(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            gossip_port: None,  // Uses network-specific default
            rpc_port: None,     // Uses network-specific default
            metrics_port: None, // Uses network-specific default (9090/19090)
            cors_origins: default_cors_origins(),
            bootstrap_peers: Vec::new(), // Uses DNS discovery or network-specific defaults
            dns_seeds: DnsSeedConfig::default(),
            quorum: QuorumConfig::default(),
            api_keys: Vec::new(),
            max_connections_per_ip: default_max_connections_per_ip(),
            connection_whitelist: Vec::new(),
        }
    }
}

impl Default for MintingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threads: 0,
        }
    }
}

impl Config {
    /// Create a new config with the given mnemonic
    pub fn new(mnemonic: String, network_type: Network) -> Self {
        Self {
            network_type,
            wallet: Some(WalletConfig { mnemonic }),
            network: NetworkConfig::default(),
            minting: MintingConfig::default(),
            faucet: FaucetConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }

    /// Create a new config without a wallet (for relay/seed nodes)
    pub fn new_relay(network_type: Network) -> Self {
        Self {
            network_type,
            wallet: None,
            network: NetworkConfig::default(),
            minting: MintingConfig::default(),
            faucet: FaucetConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }

    /// Get the network type
    pub fn network_type(&self) -> Network {
        self.network_type
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

        let contents = toml::to_string_pretty(self).context("Failed to serialize config")?;

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

/// Get the base botho directory (~/.botho)
pub fn base_data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".botho")
}

/// Get the network-specific data directory (~/.botho/testnet or
/// ~/.botho/mainnet)
pub fn data_dir(network: Network) -> PathBuf {
    base_data_dir().join(network.dir_name())
}

/// Get the config file path for a network
pub fn config_path(network: Network) -> PathBuf {
    data_dir(network).join("config.toml")
}

/// Get the ledger database path for a network
pub fn ledger_db_path(network: Network) -> PathBuf {
    data_dir(network).join("ledger")
}

/// Get the ledger database path from config file path
pub fn ledger_db_path_from_config(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(config_path).join("ledger")
}

/// Get the wallet database path for a network
pub fn wallet_db_path(network: Network) -> PathBuf {
    data_dir(network).join("wallet")
}

/// Check if mainnet is enabled
///
/// During beta, mainnet is disabled by default.
/// Set BOTHO_ENABLE_MAINNET=1 to enable.
pub fn is_mainnet_enabled() -> bool {
    std::env::var("BOTHO_ENABLE_MAINNET")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

/// Validate that the requested network can be used.
/// Returns an error if mainnet is requested but not enabled.
pub fn validate_network(network: Network) -> Result<()> {
    if network == Network::Mainnet && !is_mainnet_enabled() {
        return Err(anyhow!(
            "Mainnet is not yet enabled.\n\
             \n\
             Botho is currently in beta. Only testnet is available.\n\
             \n\
             To use testnet (recommended):\n\
             $ botho --testnet init\n\
             \n\
             To enable mainnet (for developers only):\n\
             $ BOTHO_ENABLE_MAINNET=1 botho --mainnet init"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::new("word1 word2 word3".to_string(), Network::Testnet);
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.mnemonic(), Some("word1 word2 word3"));
        assert_eq!(loaded.network_type(), Network::Testnet);
    }

    #[test]
    fn test_config_relay_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::new_relay(Network::Testnet);
        assert!(!config.has_wallet());
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert!(!loaded.has_wallet());
        assert_eq!(loaded.mnemonic(), None);
    }

    #[test]
    fn test_network_specific_paths() {
        let testnet_dir = data_dir(Network::Testnet);
        let mainnet_dir = data_dir(Network::Mainnet);

        assert!(testnet_dir.ends_with("testnet"));
        assert!(mainnet_dir.ends_with("mainnet"));
        assert_ne!(testnet_dir, mainnet_dir);
    }

    #[test]
    fn test_network_specific_ports() {
        let config = NetworkConfig::default();

        // Testnet ports should be offset by 10000
        assert_eq!(config.gossip_port(Network::Testnet), 17100);
        assert_eq!(config.gossip_port(Network::Mainnet), 7100);
        assert_eq!(config.rpc_port(Network::Testnet), 17101);
        assert_eq!(config.rpc_port(Network::Mainnet), 7101);
    }

    #[test]
    fn test_validate_network_testnet() {
        // Testnet should always be valid
        assert!(validate_network(Network::Testnet).is_ok());
    }

    #[test]
    fn test_validate_network_mainnet() {
        // Mainnet should be invalid unless env var is set
        // (We can't easily test the enabled case without affecting other tests)
        if !is_mainnet_enabled() {
            assert!(validate_network(Network::Mainnet).is_err());
        }
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
            threshold: 2,    // ignored in recommended mode
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
        let (can_mine, size, thresh) =
            quorum.can_reach_quorum(&["peer1".to_string(), "peer2".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 3);
        assert_eq!(thresh, 3); // 3-of-3 (can't tolerate any faults with 3
                               // nodes)
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
        assert_eq!(quorum.effective_threshold(1), 2); // 2 nodes: 2-of-2
        assert_eq!(quorum.effective_threshold(2), 3); // 3 nodes: 3-of-3
        assert_eq!(quorum.effective_threshold(3), 3); // 4 nodes: 3-of-4
        assert_eq!(quorum.effective_threshold(4), 4); // 5 nodes: 4-of-5
        assert_eq!(quorum.effective_threshold(5), 5); // 6 nodes: 5-of-6
        assert_eq!(quorum.effective_threshold(6), 5); // 7 nodes: 5-of-7
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
        let (can_mine, size, thresh) =
            quorum.can_reach_quorum(&["peer1".to_string(), "peer2".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 3);
        assert_eq!(thresh, 3); // BFT: 3-of-3 for n=3
    }

    #[test]
    fn test_connection_limiting_defaults() {
        let config = NetworkConfig::default();
        assert_eq!(config.max_connections_per_ip, 10);
        assert!(config.connection_whitelist.is_empty());
    }

    #[test]
    fn test_parsed_connection_whitelist() {
        let mut config = NetworkConfig::default();
        config.connection_whitelist = vec![
            "127.0.0.1".to_string(),
            "192.168.1.1".to_string(),
            "::1".to_string(),
            "invalid".to_string(), // Should be skipped
        ];

        let parsed = config.parsed_connection_whitelist();
        assert_eq!(parsed.len(), 3);
        assert!(parsed.contains(&"127.0.0.1".parse().unwrap()));
        assert!(parsed.contains(&"192.168.1.1".parse().unwrap()));
        assert!(parsed.contains(&"::1".parse().unwrap()));
    }

    #[test]
    fn test_connection_whitelist_serialization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::new("test mnemonic".to_string(), Network::Testnet);
        config.network.max_connections_per_ip = 5;
        config.network.connection_whitelist = vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()];
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.network.max_connections_per_ip, 5);
        assert_eq!(loaded.network.connection_whitelist.len(), 2);
    }
}
