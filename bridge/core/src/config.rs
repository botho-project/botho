// Copyright (c) 2024 The Botho Foundation

//! Bridge configuration types.

use serde::{Deserialize, Serialize};

/// Main bridge configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// BTH node configuration
    pub bth: BthConfig,

    /// Ethereum configuration
    pub ethereum: EthereumConfig,

    /// Solana configuration
    pub solana: SolanaConfig,

    /// Bridge-specific settings
    pub bridge: BridgeSettings,

    /// Reserve reconciliation / proof-of-reserves settings (#825).
    #[serde(default)]
    pub reserve: ReserveSettings,

    /// Federation attestation transport settings (#858): how this node
    /// exchanges signed attestation envelopes with the other validator
    /// bridge nodes.
    #[serde(default)]
    pub federation: FederationSettings,
}

/// Federation attestation envelope transport settings (#858).
///
/// The #824 attestation pipeline verifies, replay-checks, order-binds, and
/// thresholds signed envelopes, but each node only ever sees its OWN local
/// signer's envelope until they are exchanged over the network. This section
/// configures that exchange between the t-of-n validator bridge nodes (per
/// ADR 0002 the SCP validator set doubles as the federation).
///
/// Envelopes are self-authenticating — every envelope carries a signature
/// over domain-separated bytes plus a single-use replay nonce, verified
/// fail-closed by the ingest pipeline regardless of how it arrived. The
/// transport therefore needs integrity / anti-DoS but NOT confidentiality: a
/// shared-secret bearer token gates the inbound endpoint (rejecting
/// unauthenticated floods before any signature work) and authenticates
/// outbound pushes to peers. The endpoint is pure transport in front of the
/// existing verify pipeline; it never weakens verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationSettings {
    /// Base URLs of the OTHER federation members' bridge services (e.g.
    /// `https://bridge-2.example:9742`). When a node self-attests it pushes
    /// its envelope to each peer's `/api/attest` endpoint. Empty disables
    /// outbound push (single-node development, or threshold 1).
    #[serde(default)]
    pub peers: Vec<String>,

    /// Listen address for the inbound attestation endpoint
    /// (`POST /api/attest`). Empty string disables the inbound endpoint —
    /// this node then only ever counts its own local signer, so any
    /// threshold above 1 never authorizes (fail-safe). Distinct from the
    /// proof-of-reserves API so the two surfaces can bind separately.
    #[serde(default)]
    pub attest_listen: String,

    /// Shared bearer secret a peer must present (HTTP `Authorization:
    /// Bearer <token>`) to submit an envelope to THIS node's inbound
    /// endpoint. Empty disables the auth gate — acceptable only on a
    /// trusted private network, since envelopes are self-authenticating,
    /// but leaves the endpoint open to unauthenticated verification-work
    /// floods. Set it in any exposed deployment.
    #[serde(default)]
    pub inbound_auth_token: Option<String>,

    /// Bearer secret this node presents when pushing envelopes OUTBOUND to
    /// peers (their `inbound_auth_token`). `None` sends no `Authorization`
    /// header. In a symmetric federation every node shares one secret, so
    /// this typically equals `inbound_auth_token`.
    #[serde(default)]
    pub peer_auth_token: Option<String>,

    /// Per-request timeout (seconds) for an outbound push to a peer. A slow
    /// or unreachable peer must never wedge the local self-attestation path.
    #[serde(default = "default_peer_push_timeout_secs")]
    pub peer_push_timeout_secs: u64,
}

fn default_peer_push_timeout_secs() -> u64 {
    5
}

impl Default for FederationSettings {
    fn default() -> Self {
        Self {
            peers: Vec::new(),
            attest_listen: String::new(),
            inbound_auth_token: None,
            peer_auth_token: None,
            peer_push_timeout_secs: default_peer_push_timeout_secs(),
        }
    }
}

/// Reserve reconciliation / proof-of-reserves settings (#825).
///
/// Per ADR 0003 the reserve holds only factor-1 (zero-demurrage) coins, so
/// the peg invariant is exact: `Σ(wBTH outstanding) == locked BTH reserve`,
/// with no decay term. The reconciler compares the DB-derived locked
/// reserve against the on-chain wrapped supply per chain (ADR 0005) and
/// alerts on any drift beyond `tolerance_picocredits` plus the in-flight
/// allowance (orders between deposit-confirmation and mint, or between
/// burn and release).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveSettings {
    /// Absolute drift tolerance in picocredits on top of the in-flight
    /// allowance. ADR 0003 makes the invariant exact, so the default is 0;
    /// operators may raise it to absorb supply-poll timing skew.
    #[serde(default)]
    pub tolerance_picocredits: u64,

    /// Seconds between reconciliation passes.
    #[serde(default = "default_reconcile_interval_secs")]
    pub reconcile_interval_secs: u64,

    /// Listen address for the proof-of-reserves HTTP API
    /// (`GET /api/reserve/proof`, `GET /health`). Empty string disables
    /// the server.
    #[serde(default = "default_reserve_api_listen")]
    pub api_listen: String,

    /// Days of reconciliation snapshot history to retain (#846: the table
    /// grows one row per pass — ~525k rows/yr at the 60s default). Older
    /// rows are pruned each pass; the most recent snapshot always
    /// survives. 0 disables pruning.
    #[serde(default = "default_snapshot_retention_days")]
    pub snapshot_retention_days: u64,
}

fn default_reconcile_interval_secs() -> u64 {
    60
}

fn default_reserve_api_listen() -> String {
    "127.0.0.1:9741".to_string()
}

fn default_snapshot_retention_days() -> u64 {
    30
}

impl Default for ReserveSettings {
    fn default() -> Self {
        Self {
            tolerance_picocredits: 0,
            reconcile_interval_secs: default_reconcile_interval_secs(),
            api_listen: default_reserve_api_listen(),
            snapshot_retention_days: default_snapshot_retention_days(),
        }
    }
}

/// BTH node connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BthConfig {
    /// JSON-RPC URL
    pub rpc_url: String,

    /// WebSocket URL for real-time events
    pub ws_url: String,

    /// Path to encrypted view key file (for deposit detection)
    pub view_key_file: Option<String>,

    /// Path to encrypted spend key file (for withdrawals)
    pub spend_key_file: Option<String>,

    /// Number of confirmations required (0 for SCP finality)
    #[serde(default)]
    pub confirmations_required: u32,

    /// The reserve wallet's public BTH address. Release transactions spend
    /// reserve-owned outputs and return change to this address (preserving
    /// factor-1/background provenance per ADR 0003). `None` disables
    /// release submission (watch-only deployments).
    #[serde(default)]
    pub reserve_address: Option<String>,

    /// Hex-encoded 32-byte Ed25519 public keys of the release federation
    /// (the SCP validators' node keys, per ADR 0002). Every release
    /// attestation signature must come from this set. Empty disables
    /// federation-membership checking (development only).
    #[serde(default)]
    pub release_signers: Vec<String>,

    /// The threshold `t` of distinct federation signatures required to
    /// authorize a reserve release. Per ADR 0002 this must be set no lower
    /// than the SCP safety threshold in production; the default of 0 is a
    /// development value that authorizes nothing spendable on its own
    /// (release construction is additionally gated on #824/#856).
    #[serde(default)]
    pub release_threshold: u32,

    /// Confirmation depth required before a submitted release transaction
    /// is considered final and the order advances `ReleasePending ->
    /// Released`. 0 (the default) means SCP externalization finality: the
    /// transaction's block is final as soon as it appears.
    #[serde(default)]
    pub release_confirmations_required: u32,
}

/// Ethereum connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthereumConfig {
    /// RPC URL (HTTP or WebSocket)
    pub rpc_url: String,

    /// wBTH contract address
    pub wbth_contract: String,

    /// Gnosis Safe address holding `MINTER_ROLE` on the wBTH contract.
    ///
    /// Per ADR 0002, the Ethereum mint authority is a Gnosis Safe whose
    /// owners are the validators' secp256k1 keys. Mints are submitted as
    /// `Safe.execTransaction` wrapping `bridgeMint`. `None` disables mint
    /// submission (watch-only deployments).
    #[serde(default)]
    pub safe_address: Option<String>,

    /// Chain ID (1 for mainnet, 5 for goerli, etc.)
    pub chain_id: u64,

    /// Path to encrypted private key file
    pub private_key_file: Option<String>,

    /// Number of confirmations required
    #[serde(default = "default_eth_confirmations")]
    pub confirmations_required: u32,

    /// Gas price strategy
    #[serde(default)]
    pub gas_price_strategy: GasPriceStrategy,

    /// Hex-encoded 20-byte Ethereum addresses of the mint federation — the
    /// Gnosis Safe owners (the SCP validators' secp256k1 keys, per
    /// ADR 0002). Every Ethereum mint attestation must be signed by one of
    /// these. Empty disables federation attestation for Ethereum mints
    /// (development only — the engine then uses the dev stub provider).
    #[serde(default)]
    pub mint_signers: Vec<String>,

    /// The threshold `t` of distinct federation signatures required to
    /// authorize an Ethereum wBTH mint (the Safe's own on-chain threshold
    /// should equal it). Per ADR 0002 this must be no lower than the SCP
    /// safety threshold in production. Must be >= 1 whenever `mint_signers`
    /// is non-empty (a zero-threshold federation authorizes nothing).
    #[serde(default)]
    pub mint_threshold: u32,
}

fn default_eth_confirmations() -> u32 {
    12
}

/// Solana connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConfig {
    /// RPC URL
    pub rpc_url: String,

    /// wBTH program ID
    pub wbth_program: String,

    /// Path to encrypted keypair file
    pub keypair_file: Option<String>,

    /// Commitment level
    #[serde(default)]
    pub commitment: SolanaCommitment,

    /// Hex-encoded 32-byte Ed25519 public keys of the mint federation (the
    /// SCP validators' node keys, per ADR 0002). Every Solana mint
    /// attestation must be signed by one of these. Empty disables
    /// federation attestation for Solana mints (development only).
    #[serde(default)]
    pub mint_signers: Vec<String>,

    /// The threshold `t` of distinct federation signatures required to
    /// authorize a Solana wBTH mint. Must be >= 1 whenever `mint_signers`
    /// is non-empty.
    #[serde(default)]
    pub mint_threshold: u32,
}

impl SolanaConfig {
    /// Whether this Solana config is in a "production" custody posture: a
    /// federation is configured (`mint_signers` non-empty AND a positive
    /// `mint_threshold`), meaning ADR-0002 t-of-n custody is expected to be
    /// enforced by an on-chain multisig `mint_authority`.
    ///
    /// The startup custody guard uses this as its strict-mode signal: in a
    /// production posture a single-key `mint_authority` (equal to the local
    /// `keypair_file` pubkey) is a HARD startup error; otherwise it is a
    /// warning, so devnet/testnet single-key iteration is not bricked.
    pub fn requires_multisig_authority(&self) -> bool {
        !self.mint_signers.is_empty() && self.mint_threshold >= 1
    }
}

/// Bridge-specific settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSettings {
    /// Path to mnemonic file (encrypted)
    pub mnemonic_file: String,

    /// Path to SQLite database
    pub db_path: String,

    /// Bridge fee in basis points (100 = 1%)
    #[serde(default = "default_fee_bps")]
    pub fee_bps: u32,

    /// Minimum bridge fee in picocredits
    #[serde(default = "default_min_fee")]
    pub min_fee: u64,

    /// Maximum order amount in picocredits
    #[serde(default = "default_max_order")]
    pub max_order_amount: u64,

    /// Daily limit per address in picocredits
    #[serde(default = "default_daily_limit")]
    pub daily_limit_per_address: u64,

    /// Global daily limit in picocredits
    #[serde(default = "default_global_daily_limit")]
    pub global_daily_limit: u64,

    /// Order expiry time in minutes
    #[serde(default = "default_order_expiry")]
    pub order_expiry_minutes: i64,

    /// Number of retry attempts for failed operations
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Start with the circuit breaker tripped: the engine pauses the
    /// submit stages (mints and releases) at startup until an operator
    /// resumes via the API. Confirmation stages keep running so in-flight
    /// orders settle. The runtime pause state lives in the database
    /// (`bridge_state`) and survives restarts independently of this flag.
    #[serde(default)]
    pub paused: bool,

    /// Actionable-order backlog above which the circuit breaker trips
    /// automatically (a flood of orders is an anomaly signal — fail
    /// closed and let an operator inspect). 0 disables the auto-trip.
    #[serde(default = "default_max_pending_orders")]
    pub max_pending_orders: u64,

    /// Enable testnet mode
    #[serde(default)]
    pub testnet: bool,

    /// Path to this bridge node's Ed25519 attestation signing key (hex,
    /// 32-byte seed) — its federation identity for BTH releases and Solana
    /// mints (per ADR 0002, the validator's node key). `None` disables
    /// local attestation signing.
    #[serde(default)]
    pub attestation_ed25519_key_file: Option<String>,

    /// Path to this bridge node's secp256k1 attestation signing key (hex,
    /// 32 bytes) — its Gnosis Safe owner identity for Ethereum mints.
    /// `None` disables local Ethereum mint attestation signing.
    #[serde(default)]
    pub attestation_secp256k1_key_file: Option<String>,

    /// Path of the persisted attestation nonce store (replay protection
    /// across restarts). Defaults to `<db_path>.attestation-nonces.json`.
    #[serde(default)]
    pub attestation_nonce_file: Option<String>,
}

fn default_fee_bps() -> u32 {
    10 // 0.1%
}

fn default_min_fee() -> u64 {
    100_000_000 // 0.0001 BTH
}

fn default_max_order() -> u64 {
    1_000_000_000_000_000 // 1M BTH
}

fn default_daily_limit() -> u64 {
    100_000_000_000_000 // 100k BTH per address
}

fn default_global_daily_limit() -> u64 {
    10_000_000_000_000_000 // 10M BTH global
}

fn default_order_expiry() -> i64 {
    60 // 1 hour
}

fn default_max_retries() -> u32 {
    3
}

fn default_max_pending_orders() -> u64 {
    1_000
}

/// Gas price strategy for Ethereum transactions.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GasPriceStrategy {
    /// Use low gas price (slower, cheaper)
    Low,
    /// Use medium gas price (balanced)
    #[default]
    Medium,
    /// Use high gas price (faster, more expensive)
    High,
    /// Use a fixed gas price in gwei
    Fixed(u64),
}

/// Solana commitment level.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SolanaCommitment {
    /// Processed (fastest, but may be rolled back)
    Processed,
    /// Confirmed (1/3 of validators)
    Confirmed,
    /// Finalized (2/3 of validators, most secure)
    #[default]
    Finalized,
}

impl BridgeConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {}", e))?;
        toml::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))
    }

    /// Calculate the bridge fee for an amount.
    pub fn calculate_fee(&self, amount: u64) -> u64 {
        let percentage_fee = (amount as u128 * self.bridge.fee_bps as u128 / 10_000) as u64;
        percentage_fee.max(self.bridge.min_fee)
    }
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            bth: BthConfig {
                rpc_url: "http://localhost:7101".to_string(),
                ws_url: "ws://localhost:7101/ws".to_string(),
                view_key_file: None,
                spend_key_file: None,
                confirmations_required: 0,
                reserve_address: None,
                release_signers: Vec::new(),
                release_threshold: 0,
                release_confirmations_required: 0,
            },
            ethereum: EthereumConfig {
                rpc_url: "http://localhost:8545".to_string(),
                wbth_contract: "0x0000000000000000000000000000000000000000".to_string(),
                safe_address: None,
                chain_id: 1,
                private_key_file: None,
                confirmations_required: 12,
                gas_price_strategy: GasPriceStrategy::default(),
                mint_signers: Vec::new(),
                mint_threshold: 0,
            },
            solana: SolanaConfig {
                rpc_url: "http://localhost:8899".to_string(),
                wbth_program: "11111111111111111111111111111111".to_string(),
                keypair_file: None,
                commitment: SolanaCommitment::default(),
                mint_signers: Vec::new(),
                mint_threshold: 0,
            },
            bridge: BridgeSettings {
                mnemonic_file: "bridge_mnemonic.enc".to_string(),
                db_path: "bridge.db".to_string(),
                fee_bps: default_fee_bps(),
                min_fee: default_min_fee(),
                max_order_amount: default_max_order(),
                daily_limit_per_address: default_daily_limit(),
                global_daily_limit: default_global_daily_limit(),
                order_expiry_minutes: default_order_expiry(),
                max_retries: default_max_retries(),
                paused: false,
                max_pending_orders: default_max_pending_orders(),
                testnet: false,
                attestation_ed25519_key_file: None,
                attestation_secp256k1_key_file: None,
                attestation_nonce_file: None,
            },
            reserve: ReserveSettings::default(),
            federation: FederationSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_calculation() {
        let config = BridgeConfig::default();

        // 0.1% of 1 BTH = 0.001 BTH = 1_000_000_000 picocredits
        let fee = config.calculate_fee(1_000_000_000_000);
        assert_eq!(fee, 1_000_000_000);

        // Small amount should use minimum fee
        let small_fee = config.calculate_fee(1_000_000);
        assert_eq!(small_fee, default_min_fee());
    }

    #[test]
    fn test_default_config() {
        let config = BridgeConfig::default();
        assert_eq!(config.bridge.fee_bps, 10);
        assert!(!config.bridge.testnet);
        assert!(!config.bridge.paused);
        assert_eq!(config.bridge.max_pending_orders, 1_000);
    }

    #[test]
    fn test_breaker_knobs_parse() {
        // A pre-existing config without the breaker knobs still parses
        // with safe defaults, and the knobs round-trip from TOML.
        let legacy: BridgeSettings = toml::from_str(
            r#"
            mnemonic_file = "m.enc"
            db_path = "bridge.db"
            "#,
        )
        .unwrap();
        assert!(!legacy.paused);
        assert_eq!(legacy.max_pending_orders, 1_000);

        let configured: BridgeSettings = toml::from_str(
            r#"
            mnemonic_file = "m.enc"
            db_path = "bridge.db"
            paused = true
            max_pending_orders = 50
            "#,
        )
        .unwrap();
        assert!(configured.paused);
        assert_eq!(configured.max_pending_orders, 50);
    }

    #[test]
    fn test_bth_release_knobs_default_and_parse() {
        // Defaults: release submission disabled, SCP finality.
        let config = BridgeConfig::default();
        assert!(config.bth.reserve_address.is_none());
        assert!(config.bth.release_signers.is_empty());
        assert_eq!(config.bth.release_threshold, 0);
        assert_eq!(config.bth.release_confirmations_required, 0);

        // A pre-existing config without the release knobs still parses.
        let legacy: BthConfig = toml::from_str(
            r#"
            rpc_url = "http://localhost:7101"
            ws_url = "ws://localhost:7101/ws"
            "#,
        )
        .unwrap();
        assert!(legacy.reserve_address.is_none());
        assert_eq!(legacy.release_confirmations_required, 0);

        // The release knobs round-trip from TOML.
        let configured: BthConfig = toml::from_str(
            r#"
            rpc_url = "http://localhost:7101"
            ws_url = "ws://localhost:7101/ws"
            reserve_address = "bth_reserve_addr"
            release_signers = ["aa", "bb"]
            release_threshold = 3
            release_confirmations_required = 2
            "#,
        )
        .unwrap();
        assert_eq!(
            configured.reserve_address.as_deref(),
            Some("bth_reserve_addr")
        );
        assert_eq!(configured.release_signers.len(), 2);
        assert_eq!(configured.release_threshold, 3);
        assert_eq!(configured.release_confirmations_required, 2);
    }

    #[test]
    fn test_federation_settings_default_and_parse() {
        // Defaults (#858): no peers, inbound endpoint disabled, no auth,
        // 5s push timeout — a single-node / threshold-1 deployment.
        let config = BridgeConfig::default();
        assert!(config.federation.peers.is_empty());
        assert!(config.federation.attest_listen.is_empty());
        assert!(config.federation.inbound_auth_token.is_none());
        assert!(config.federation.peer_auth_token.is_none());
        assert_eq!(config.federation.peer_push_timeout_secs, 5);

        // A pre-existing config without a [federation] section still parses.
        let legacy: FederationSettings = toml::from_str("").unwrap();
        assert!(legacy.peers.is_empty());
        assert_eq!(legacy.peer_push_timeout_secs, 5);

        // The knobs round-trip from TOML.
        let configured: FederationSettings = toml::from_str(
            r#"
            peers = ["http://bridge-2:9742", "http://bridge-3:9742"]
            attest_listen = "0.0.0.0:9742"
            inbound_auth_token = "in-secret"
            peer_auth_token = "out-secret"
            peer_push_timeout_secs = 3
            "#,
        )
        .unwrap();
        assert_eq!(configured.peers.len(), 2);
        assert_eq!(configured.attest_listen, "0.0.0.0:9742");
        assert_eq!(configured.inbound_auth_token.as_deref(), Some("in-secret"));
        assert_eq!(configured.peer_auth_token.as_deref(), Some("out-secret"));
        assert_eq!(configured.peer_push_timeout_secs, 3);
    }

    #[test]
    fn test_reserve_settings_default_and_parse() {
        // Defaults: exact peg (ADR 0003), 60s cadence, localhost API.
        let config = BridgeConfig::default();
        assert_eq!(config.reserve.tolerance_picocredits, 0);
        assert_eq!(config.reserve.reconcile_interval_secs, 60);
        assert_eq!(config.reserve.api_listen, "127.0.0.1:9741");

        // A pre-existing config without a [reserve] section still parses.
        let legacy: ReserveSettings = toml::from_str("").unwrap();
        assert_eq!(legacy.tolerance_picocredits, 0);
        assert_eq!(legacy.reconcile_interval_secs, 60);
        assert_eq!(legacy.snapshot_retention_days, 30);

        // The knobs round-trip from TOML; empty api_listen disables.
        let configured: ReserveSettings = toml::from_str(
            r#"
            tolerance_picocredits = 5000
            reconcile_interval_secs = 15
            api_listen = ""
            "#,
        )
        .unwrap();
        assert_eq!(configured.tolerance_picocredits, 5000);
        assert_eq!(configured.reconcile_interval_secs, 15);
        assert!(configured.api_listen.is_empty());
    }
}
