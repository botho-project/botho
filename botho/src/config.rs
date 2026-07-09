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
    /// RPC-server configuration, including the optional operator surface
    /// (#707, P4.2 of the #695 proposal). Absent by default so existing
    /// configs and node behavior are unchanged.
    #[serde(default)]
    pub rpc: RpcConfig,
}

/// RPC-server configuration.
///
/// Today this only carries the optional `[rpc.operator]` block. When the whole
/// `[rpc]` section is absent from `config.toml`, this deserializes to its
/// default (operator surface OFF) and the node behaves exactly as before.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RpcConfig {
    /// Operator read-surface configuration (#707). Absent ⇒ the whole
    /// operator feature is OFF: `operator_*` RPCs return a clean "not enabled"
    /// error and the `botho operator mint-read-link` CLI refuses to mint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<OperatorConfig>,
}

/// Operator read-surface configuration (`[rpc.operator]`, #707).
///
/// The mere PRESENCE of this section (with a non-empty secret) turns the
/// operator read RPCs on. It is deliberately a separate opt-in from the rest
/// of the RPC surface: the operator-only reads (per-peer gate classification,
/// configured quorum contents, audit log) are a targeting map an adversary
/// must not get for free, so they are gated behind a node-verified magic-link
/// token keyed on `read_token_secret`.
///
/// This surface is READ-ONLY by construction. The write path (operator-signed
/// quorum curation) is a separate, separately-reviewed deliverable (#709,
/// governed by `docs/security/quorum-write-path.md`); no field here grants any
/// write capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorConfig {
    /// HMAC-SHA256 secret the node uses to verify operator read tokens of the
    /// form `op.<expUnixSeconds>.<hmacSha256Hex(secret, "op.<exp>")>`. Minted
    /// off-node by `botho operator mint-read-link`. An empty secret is treated
    /// as "not configured" (fail closed).
    pub read_token_secret: String,

    /// Operator-action signing public keys (hex-encoded Ed25519, 64 hex chars
    /// each), one per authorized operator key (#747, P4.4a; design
    /// `docs/security/quorum-write-path.md` §2). **Public keys only** — no
    /// private key material ever lives on a node.
    ///
    /// An empty/absent list means the node has **no write surface at all**
    /// (fail closed): downstream sub-issues gate `operator_submitAction` on a
    /// non-empty list. This issue lands ONLY the field + accessor + RpcState
    /// plumbing; no RPC consumes it yet.
    ///
    /// This list is provisioned and changed over SSH/config exclusively. There
    /// is intentionally NO RPC or signed-action code path that reads, adds, or
    /// removes entries — key management must not be self-referential (a stolen
    /// key must not enroll further keys, §2). That property is verified by
    /// absence: no accessor here or elsewhere mutates this list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_public_keys: Vec<String>,
}

impl OperatorConfig {
    /// The configured secret if it is present AND non-empty; `None` otherwise.
    /// An empty secret must not enable the feature (fail closed).
    pub fn effective_secret(&self) -> Option<&str> {
        let s = self.read_token_secret.trim();
        if s.is_empty() {
            None
        } else {
            Some(self.read_token_secret.as_str())
        }
    }

    /// The configured operator-action public keys, with empty/whitespace-only
    /// entries filtered out and trimmed. Returns an empty `Vec` when none are
    /// usably configured (fail closed — "no keys" ⇒ no write surface).
    pub fn effective_action_public_keys(&self) -> Vec<String> {
        self.action_public_keys
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect()
    }
}

impl RpcConfig {
    /// The effective operator read-token secret, or `None` when the operator
    /// surface is not configured (absent section OR empty secret). When this
    /// is `None` the operator RPCs are OFF and the node behaves as today.
    pub fn operator_read_token_secret(&self) -> Option<&str> {
        self.operator.as_ref().and_then(|o| o.effective_secret())
    }

    /// The effective operator-action signing public keys (hex Ed25519),
    /// filtered fail-closed: empty/whitespace-only entries removed, absent
    /// `[rpc.operator]` section ⇒ empty `Vec` ("no keys"). Downstream
    /// sub-issues (#748+) require this to be non-empty before exposing any
    /// operator write surface; an empty result means the write path stays OFF
    /// and the node behaves exactly as today (#747).
    pub fn operator_action_public_keys(&self) -> Vec<String> {
        self.operator
            .as_ref()
            .map(|o| o.effective_action_public_keys())
            .unwrap_or_default()
    }
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

    /// Optional override for the persistent libp2p node identity key path
    /// (issue #439). If unset, the key lives at `<data_dir>/node_key` alongside
    /// the ledger and config. The key is generated on first run and loaded
    /// thereafter so the node's peer ID is stable across restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_key_path: Option<PathBuf>,

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
///
/// Delegates to [`crate::network::seeds::fallback_seeds`] (the single source of
/// truth) so the config defaults and the DNS-discovery fallback list never
/// drift apart. The regional (multi-seed) scaffolding is opt-in there.
fn default_bootstrap_peers(network: Network) -> Vec<String> {
    crate::network::fallback_seeds(network)
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

/// Fault model posture for `Recommended` quorum mode.
///
/// Selects how the auto-calculated quorum threshold is derived from the live
/// member count `n`:
///
/// - [`FaultModel::Crash`] (DEFAULT): crash-fault tolerance via a 2f+1 simple
///   majority — `threshold = floor(n/2) + 1`. This gives genuine fault
///   tolerance for small homogeneous clusters (e.g. 2-of-3 at n=3) so a single
///   crashed or lagging node cannot stall liveness. This is the
///   trusted-operator testnet posture and matches Stellar Core's
///   homogeneous-cluster default.
/// - [`FaultModel::Bft`]: Byzantine-fault tolerance via a 3f+1 quorum —
///   `threshold = n - floor((n-1)/3)`. Tolerates up to f Byzantine (arbitrarily
///   malicious) members but requires n >= 4 for any genuine BFT (n<=3 collapses
///   to unanimity).
///
/// Note: under either model botho currently auto-trusts every connected peer in
/// `Recommended` mode, so n<=3 has no real Byzantine robustness regardless;
/// `Crash` therefore loses nothing real at small n and gains liveness +
/// crash-fault tolerance. #420 no-fork safety is preserved because any two
/// 2f+1 majority subsets always intersect.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FaultModel {
    /// Crash-fault tolerance: 2f+1 simple majority (`floor(n/2) + 1`).
    Crash,
    /// Byzantine-fault tolerance: 3f+1 quorum (`n - floor((n-1)/3)`).
    Bft,
}

impl Default for FaultModel {
    fn default() -> Self {
        Self::Crash
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumConfig {
    /// Quorum mode: explicit (user lists peers) or recommended (auto-discover)
    #[serde(default)]
    pub mode: QuorumMode,

    /// Fault model posture for `Recommended` mode: `crash` (2f+1 simple
    /// majority, the default) or `bft` (3f+1 Byzantine quorum). Ignored in
    /// `Explicit` mode, where the operator sets an exact threshold.
    #[serde(default)]
    pub fault_model: FaultModel,

    /// For explicit mode: number of peers required to agree (e.g., 2 in a
    /// 2-of-3) For recommended mode: this is auto-calculated from `fault_model`
    #[serde(default = "default_threshold")]
    pub threshold: u32,

    /// Operator-curated quorum members (base58 PeerId strings) — the
    /// safety-bearing "curated core" of the quorum promotion gate (#651).
    ///
    /// - In `Explicit` mode these are the **only** peers (besides self)
    ///   admitted into the SCP quorum set; auto-discovered peers stay
    ///   peering/gossip-only.
    /// - In `Recommended` mode any *connected* curated member is always
    ///   admitted and does not count against [`Self::max_auto_members`].
    #[serde(default)]
    pub members: Vec<String>,

    /// For recommended mode: minimum peers before minting can start
    #[serde(default = "default_min_peers")]
    pub min_peers: u32,

    /// Quorum promotion gate (#651, epic #441 §3/P5): in `Recommended` mode,
    /// the maximum number of auto-discovered (non-curated) peers that may be
    /// promoted into the safety-critical SCP quorum set. Curated
    /// [`Self::members`] never count against this cap.
    ///
    /// Without a cap, a Sybil flood of connectable peers is auto-admitted
    /// into quorums on the next churn event — a safety (fork) risk, not just
    /// a liveness one. The cap bounds the auto-trusted set; peers beyond it
    /// remain connected for gossip/sync but hold no quorum membership.
    /// Selection above the cap is deterministic (candidates are ordered by
    /// their derived SCP `NodeID`), so the same peer set always yields the
    /// same quorum set regardless of arrival order.
    ///
    /// The default (8) is comfortably above the current live-testnet shape
    /// (self + 4 peers), so small honest clusters see zero behavior change,
    /// while keeping the quorum small enough for the exact
    /// `bth-quorum-sim` intersection check on every rebuild. Setting `0`
    /// makes `Recommended` mode curated-only (auto peers never validate).
    #[serde(default = "default_max_auto_members")]
    pub max_auto_members: u32,
}

impl Default for QuorumConfig {
    fn default() -> Self {
        Self {
            mode: QuorumMode::Recommended,
            fault_model: FaultModel::default(),
            threshold: 2,
            members: Vec::new(),
            min_peers: 1,
            max_auto_members: default_max_auto_members(),
        }
    }
}

impl QuorumConfig {
    /// Calculate the effective threshold for a given number of connected peers.
    ///
    /// In `Recommended` mode the threshold is derived from the configured
    /// [`FaultModel`] over `n = connected_count + 1` (peers plus self):
    ///
    /// - `Crash` (default): `floor(n/2) + 1` (2f+1 simple majority). n=1->1,
    ///   n=2->2, n=3->2, n=4->3, n=5->3, n=6->4.
    /// - `Bft`: `n - floor((n-1)/3)` (3f+1 quorum). n=1->1, n=2->2, n=3->3,
    ///   n=4->3, n=5->4, n=6->5.
    ///
    /// In `Explicit` mode the operator-configured `threshold` is returned
    /// as-is.
    pub fn effective_threshold(&self, connected_count: usize) -> usize {
        match self.mode {
            QuorumMode::Explicit => self.threshold as usize,
            QuorumMode::Recommended => {
                // n = total nodes including self
                let n = connected_count + 1;
                match self.fault_model {
                    // Crash-fault: 2f+1 simple majority.
                    FaultModel::Crash => n / 2 + 1,
                    // Byzantine-fault: 3f+1 quorum, threshold = n - f
                    // where f = floor((n-1)/3).
                    FaultModel::Bft => {
                        let f = n.saturating_sub(1) / 3;
                        n - f
                    }
                }
            }
        }
    }

    /// Minimum participating node count required for genuine Byzantine-fault
    /// tolerance (f >= 1) under the SCP 3f+1 bound.
    ///
    /// `3f + 1 <= n` with `f >= 1` requires `n >= 4`. Below this the quorum is
    /// degenerate (n-of-n or a bare crash-majority) and tolerates *zero*
    /// Byzantine faults regardless of the configured [`FaultModel`].
    pub const MIN_BFT_NODES: usize = 4;

    /// Whether a `Recommended`-mode cluster of `node_count` participating nodes
    /// (including self) is genuinely Byzantine-fault-tolerant.
    ///
    /// Returns `true` only when `node_count >= 4` (the hard 3f+1 >= 4 bound for
    /// f=1) and the node is running in `Recommended` mode. In `Explicit` mode
    /// the operator owns the threshold/membership decision, so this always
    /// returns `true` (no auto-derived warning).
    ///
    /// Below 4 nodes the auto-derived quorum is degenerate — n=2 -> 2-of-2,
    /// n=3 -> 3-of-3 under `Bft`, or a bare crash-majority under `Crash` — and
    /// tolerates no Byzantine (arbitrarily malicious) member. See
    /// [`Self::is_degenerate_quorum`].
    pub fn is_bft_fault_tolerant(&self, node_count: usize) -> bool {
        match self.mode {
            QuorumMode::Explicit => true,
            QuorumMode::Recommended => node_count >= Self::MIN_BFT_NODES,
        }
    }

    /// Whether a `Recommended`-mode cluster of `node_count` participating nodes
    /// (including self) has a *degenerate* quorum that tolerates zero Byzantine
    /// faults (`node_count < 4`).
    ///
    /// This is the inverse of [`Self::is_bft_fault_tolerant`]: it is `true`
    /// only in `Recommended` mode with fewer than [`Self::MIN_BFT_NODES`]
    /// nodes.
    pub fn is_degenerate_quorum(&self, node_count: usize) -> bool {
        matches!(self.mode, QuorumMode::Recommended) && node_count < Self::MIN_BFT_NODES
    }

    /// Build the loud operator warning for a degenerate `Recommended`-mode
    /// quorum, or `None` when the cluster is BFT (>= 4 nodes) or in `Explicit`
    /// mode.
    ///
    /// The wording is deliberately honest (#509 research, #510 Thread A): below
    /// 4 nodes the cluster is **not Byzantine-fault-tolerant at all** — it
    /// tolerates *zero* node failures (a degenerate n-of-n quorum), not merely
    /// "degraded". `3f + 1 >= 4` is a hard bound for f=1.
    pub fn degenerate_quorum_warning(&self, node_count: usize) -> Option<String> {
        if !self.is_degenerate_quorum(node_count) {
            return None;
        }
        Some(format!(
            "WARNING: {node_count}-node cluster in recommended mode is NOT \
             Byzantine-fault-tolerant — it tolerates ZERO node failures \
             (degenerate {node_count}-of-{node_count} quorum, crash-stop only, \
             not BFT). Add a 4th independent operator for f=1 BFT (3f+1>=4)."
        ))
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

/// Default cap on auto-promoted (non-curated) quorum members (#651). See
/// [`QuorumConfig::max_auto_members`].
fn default_max_auto_members() -> u32 {
    8
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
    /// Default: 1 BTH (1_000_000_000_000 picocredits)
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

/// 1 BTH in picocredits
fn default_faucet_amount() -> u64 {
    1_000_000_000_000
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
            node_key_path: None, // Defaults to <data_dir>/node_key

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
            rpc: RpcConfig::default(),
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
            rpc: RpcConfig::default(),
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

/// Get the persistent libp2p node-key path from the config file path (issue
/// #439).
///
/// Defaults to `<data_dir>/node_key` (the data dir is the directory containing
/// `config.toml`, the same dir that holds `ledger/`). This is the file that
/// stores the node's identity keypair so its peer ID is stable across restarts.
pub fn node_key_path_from_config(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(config_path).join("node_key")
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
        // #651 promotion gate: cap defaults to 8 — comfortably above the
        // 5-node live testnet (self + 4 peers) so small clusters see zero
        // behavior change.
        assert_eq!(quorum.max_auto_members, 8);
    }

    #[test]
    fn test_quorum_max_auto_members_serde_default() {
        // Existing config files without the #651 gate key must keep parsing
        // and get the default cap.
        let quorum: QuorumConfig = toml::from_str("").unwrap();
        assert_eq!(quorum.max_auto_members, 8);

        // And an explicit value is honored.
        let quorum: QuorumConfig = toml::from_str("max_auto_members = 3").unwrap();
        assert_eq!(quorum.max_auto_members, 3);
    }

    #[test]
    fn test_quorum_explicit_mode() {
        let quorum = QuorumConfig {
            mode: QuorumMode::Explicit,
            fault_model: FaultModel::default(),
            threshold: 2,
            members: vec!["peer1".to_string(), "peer2".to_string()],
            min_peers: 1,
            max_auto_members: 8,
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
        // Default fault model is crash (2f+1 simple majority).
        let quorum = QuorumConfig {
            mode: QuorumMode::Recommended,
            fault_model: FaultModel::Crash,
            threshold: 2,    // ignored in recommended mode
            members: vec![], // ignored in recommended mode
            min_peers: 1,
            max_auto_members: 8,
        };

        // No peers - can't mine
        let (can_mine, _, _) = quorum.can_reach_quorum(&[]);
        assert!(!can_mine);

        // One peer - can mine (2 nodes, threshold=2)
        let (can_mine, size, thresh) = quorum.can_reach_quorum(&["peer1".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 2);
        assert_eq!(thresh, 2); // 2-of-2 (can't tolerate a fault at n=2)

        // Two peers - can mine (3 nodes, threshold=2 under crash 2f+1).
        // Crash with n=3: floor(3/2)+1 = 2 -> 2-of-3 (tolerates 1 crash/lag).
        let (can_mine, size, thresh) =
            quorum.can_reach_quorum(&["peer1".to_string(), "peer2".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 3);
        assert_eq!(thresh, 2); // 2-of-3 under crash fault model
    }

    #[test]
    fn test_quorum_effective_threshold_crash() {
        // Crash fault model (DEFAULT): 2f+1 simple majority = floor(n/2) + 1.
        let quorum = QuorumConfig::default();
        assert_eq!(quorum.fault_model, FaultModel::Crash);

        // connected_count = n - 1 (peers; n includes self).
        assert_eq!(quorum.effective_threshold(0), 1); // n=1: 1-of-1
        assert_eq!(quorum.effective_threshold(1), 2); // n=2: 2-of-2
        assert_eq!(quorum.effective_threshold(2), 2); // n=3: 2-of-3
        assert_eq!(quorum.effective_threshold(3), 3); // n=4: 3-of-4
        assert_eq!(quorum.effective_threshold(4), 3); // n=5: 3-of-5
        assert_eq!(quorum.effective_threshold(5), 4); // n=6: 4-of-6
    }

    #[test]
    fn test_quorum_effective_threshold_bft() {
        // BFT fault model: 3f+1 quorum = n - floor((n-1)/3).
        let quorum = QuorumConfig {
            mode: QuorumMode::Recommended,
            fault_model: FaultModel::Bft,
            threshold: 2,
            members: vec![],
            min_peers: 1,
            max_auto_members: 8,
        };

        assert_eq!(quorum.effective_threshold(0), 1); // n=1: 1-of-1
        assert_eq!(quorum.effective_threshold(1), 2); // n=2: 2-of-2
        assert_eq!(quorum.effective_threshold(2), 3); // n=3: 3-of-3
        assert_eq!(quorum.effective_threshold(3), 3); // n=4: 3-of-4
        assert_eq!(quorum.effective_threshold(4), 4); // n=5: 4-of-5
        assert_eq!(quorum.effective_threshold(5), 5); // n=6: 5-of-6
    }

    #[test]
    fn test_quorum_degenerate_below_four_recommended() {
        // Recommended mode (default): below 4 participating nodes the quorum is
        // degenerate (zero Byzantine-fault tolerance); >= 4 is genuinely BFT.
        let quorum = QuorumConfig::default();
        assert_eq!(quorum.mode, QuorumMode::Recommended);

        for n in 1..=3 {
            assert!(
                quorum.is_degenerate_quorum(n),
                "n={n} must be flagged degenerate in recommended mode"
            );
            assert!(
                !quorum.is_bft_fault_tolerant(n),
                "n={n} must NOT be BFT in recommended mode"
            );
        }
        for n in 4..=8 {
            assert!(
                !quorum.is_degenerate_quorum(n),
                "n={n} must NOT be flagged degenerate (>= 4 is BFT)"
            );
            assert!(
                quorum.is_bft_fault_tolerant(n),
                "n={n} must be BFT in recommended mode (3f+1 >= 4)"
            );
        }
    }

    #[test]
    fn test_quorum_degenerate_warning_fires_below_four() {
        let quorum = QuorumConfig::default();

        // n < 4 -> loud, honest warning naming the n-of-n quorum and BFT bound.
        for n in 2..=3 {
            let warning = quorum
                .degenerate_quorum_warning(n)
                .unwrap_or_else(|| panic!("expected warning at n={n}"));
            assert!(warning.contains("NOT"), "warning must be loud: {warning}");
            assert!(
                warning.contains("Byzantine-fault-tolerant"),
                "warning must be honest about BFT: {warning}"
            );
            assert!(
                warning.contains("ZERO"),
                "warning must state zero fault tolerance: {warning}"
            );
            assert!(
                warning.contains(&format!("{n}-of-{n}")),
                "warning must name the degenerate n-of-n quorum: {warning}"
            );
        }

        // n >= 4 -> no warning (genuine BFT).
        for n in 4..=6 {
            assert!(
                quorum.degenerate_quorum_warning(n).is_none(),
                "no warning expected at n={n}"
            );
        }
    }

    #[test]
    fn test_quorum_degenerate_explicit_mode_never_warns() {
        // Explicit mode: operator owns the threshold/membership, so no
        // auto-derived degenerate-quorum warning regardless of node count.
        let quorum = QuorumConfig {
            mode: QuorumMode::Explicit,
            fault_model: FaultModel::default(),
            threshold: 2,
            members: vec!["peer1".to_string(), "peer2".to_string()],
            min_peers: 1,
            max_auto_members: 8,
        };

        for n in 1..=6 {
            assert!(!quorum.is_degenerate_quorum(n));
            assert!(quorum.is_bft_fault_tolerant(n));
            assert!(quorum.degenerate_quorum_warning(n).is_none());
        }
    }

    #[test]
    fn test_quorum_min_peers() {
        let quorum = QuorumConfig {
            mode: QuorumMode::Recommended,
            fault_model: FaultModel::Crash,
            threshold: 2,
            members: vec![],
            min_peers: 2, // Require at least 2 peers
            max_auto_members: 8,
        };

        // One peer - not enough
        let (can_mine, _, _) = quorum.can_reach_quorum(&["peer1".to_string()]);
        assert!(!can_mine);

        // Two peers - enough (3 nodes, threshold=2 under crash 2f+1).
        let (can_mine, size, thresh) =
            quorum.can_reach_quorum(&["peer1".to_string(), "peer2".to_string()]);
        assert!(can_mine);
        assert_eq!(size, 3);
        assert_eq!(thresh, 2); // crash: 2-of-3 for n=3
    }

    #[test]
    fn test_fault_model_parses_crash_and_bft() {
        // crash
        let q: QuorumConfig =
            toml::from_str("mode = \"recommended\"\nfault_model = \"crash\"\n").unwrap();
        assert_eq!(q.fault_model, FaultModel::Crash);
        assert_eq!(q.effective_threshold(2), 2); // n=3 -> 2-of-3

        // bft
        let q: QuorumConfig =
            toml::from_str("mode = \"recommended\"\nfault_model = \"bft\"\n").unwrap();
        assert_eq!(q.fault_model, FaultModel::Bft);
        assert_eq!(q.effective_threshold(2), 3); // n=3 -> 3-of-3
    }

    #[test]
    fn test_fault_model_defaults_to_crash_when_absent() {
        // Omitting fault_model defaults to crash.
        let q: QuorumConfig = toml::from_str("mode = \"recommended\"\n").unwrap();
        assert_eq!(q.fault_model, FaultModel::Crash);
    }

    #[test]
    fn test_fault_model_invalid_value_errors_clearly() {
        let err =
            toml::from_str::<QuorumConfig>("mode = \"recommended\"\nfault_model = \"paxos\"\n")
                .unwrap_err();
        let msg = err.to_string();
        // serde reports the unknown variant and the allowed set.
        assert!(
            msg.contains("crash") && msg.contains("bft"),
            "error should name valid fault models, got: {msg}"
        );
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

    #[test]
    fn action_public_keys_absent_operator_section_yields_no_keys() {
        // No [rpc.operator] at all ⇒ fail-closed empty list (no write surface).
        let rpc = RpcConfig::default();
        assert!(rpc.operator_action_public_keys().is_empty());
    }

    #[test]
    fn action_public_keys_parse_and_filter_whitespace() {
        let toml = "[operator]\n\
             read_token_secret = \"s\"\n\
             action_public_keys = [\"aa\", \"  \", \"\", \" bb \"]\n";
        let rpc: RpcConfig = toml::from_str(toml).unwrap();
        // Whitespace-only and empty entries filtered; survivors trimmed.
        assert_eq!(rpc.operator_action_public_keys(), vec!["aa", "bb"]);
    }

    #[test]
    fn action_public_keys_empty_list_yields_no_keys() {
        // Present [rpc.operator] but an empty/all-whitespace key list still
        // fails closed to "no keys".
        let toml = "[operator]\n\
             read_token_secret = \"s\"\n\
             action_public_keys = [\"  \", \"\"]\n";
        let rpc: RpcConfig = toml::from_str(toml).unwrap();
        assert!(rpc.operator_action_public_keys().is_empty());
    }

    #[test]
    fn action_public_keys_absent_field_defaults_empty() {
        // [rpc.operator] present for the read token, but no action_public_keys
        // field ⇒ empty (serde default) ⇒ no write surface.
        let toml = "[operator]\nread_token_secret = \"s\"\n";
        let rpc: RpcConfig = toml::from_str(toml).unwrap();
        assert!(rpc.operator_action_public_keys().is_empty());
    }
}
