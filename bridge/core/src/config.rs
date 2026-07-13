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
    /// (release construction is additionally gated on #824/#828).
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

    /// Enable testnet mode
    #[serde(default)]
    pub testnet: bool,
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
            },
            solana: SolanaConfig {
                rpc_url: "http://localhost:8899".to_string(),
                wbth_program: "11111111111111111111111111111111".to_string(),
                keypair_file: None,
                commitment: SolanaCommitment::default(),
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
                testnet: false,
            },
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
}
