//! Botho Mobile - UniFFI bindings for mobile wallet
//!
//! This crate provides FFI bindings for the Botho wallet, allowing
//! React Native (via Swift/Kotlin) to interact with the Rust core.
//!
//! # Security Model
//!
//! - Mnemonics NEVER leave Rust memory
//! - Keys are zeroized on drop
//! - Only public data crosses FFI boundary
//! - Session-based access with auto-lock timeout

use std::sync::Arc;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

mod rpc;
mod wallet_ops;

use bth_wasm_signer::core::RecipientAddress;
use rpc::NodeRpc;
use wallet_ops::{SendError, SignerKeys};

uniffi::setup_scaffolding!();

/// Testnet classical address prefix: `tbotho://1/<base58(view||spend)>`.
/// Matches `botho/src/address.rs::TESTNET_CLASSICAL_PREFIX` so faucet/send
/// recipients are parseable by the node.
const TESTNET_CLASSICAL_PREFIX: &str = "tbotho://1/";

/// Errors that can occur in mobile wallet operations
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileWalletError {
    #[error("Wallet is locked")]
    WalletLocked,

    #[error("Invalid mnemonic phrase")]
    InvalidMnemonic,

    #[error("Invalid password")]
    InvalidPassword,

    #[error("Session expired")]
    SessionExpired,

    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Invalid address format")]
    InvalidAddress,

    #[error("Insufficient funds")]
    InsufficientFunds,

    #[error("Internal error: {message}")]
    InternalError { message: String },
}

/// Result type for mobile wallet operations
pub type MobileResult<T> = Result<T, MobileWalletError>;

/// Public wallet address (safe to expose)
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletAddress {
    /// View public key (hex)
    pub view_public_key: String,
    /// Spend public key (hex)
    pub spend_public_key: String,
    /// Display format (cad:...)
    pub display: String,
}

/// Balance information
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletBalance {
    /// Balance in picocredits (smallest unit)
    pub picocredits: u64,
    /// Formatted balance (e.g., "1.234567 BTH")
    pub formatted: String,
    /// Number of UTXOs
    pub utxo_count: u32,
    /// Last sync block height
    pub sync_height: u64,
}

/// Transaction history entry
#[derive(Debug, Clone, uniffi::Record)]
pub struct TransactionEntry {
    /// Transaction hash
    pub tx_hash: String,
    /// Amount in picocredits (negative for sends)
    pub amount: i64,
    /// Block height
    pub block_height: u64,
    /// Timestamp (Unix seconds)
    pub timestamp: u64,
    /// Direction: "send" or "receive"
    pub direction: String,
    /// Counterparty address (if known)
    pub counterparty: Option<String>,
}

/// Result of a faucet request
#[derive(Debug, Clone, uniffi::Record)]
pub struct FaucetResult {
    /// Whether the faucet dispensed coins
    pub success: bool,
    /// Transaction hash of the faucet payout (empty if unsuccessful)
    pub tx_hash: String,
    /// Amount dispensed in picocredits
    pub amount: u64,
    /// Human-readable amount (e.g. "10.000000 BTH")
    pub amount_formatted: String,
    /// Optional message from the faucet (error or rate-limit info)
    pub message: String,
}

/// Health/status of a node (for the in-app node picker)
#[derive(Debug, Clone, uniffi::Record)]
pub struct NodeStatusInfo {
    /// Node software version
    pub version: String,
    /// Network name (e.g. "botho-testnet")
    pub network: String,
    /// Current chain height
    pub chain_height: u64,
    /// Sync status string (e.g. "synced")
    pub sync_status: String,
    /// Number of connected peers
    pub peer_count: u32,
}

/// Session status information
#[derive(Debug, Clone, uniffi::Record)]
pub struct SessionStatus {
    /// Whether wallet is currently unlocked
    pub is_unlocked: bool,
    /// Public address if unlocked
    pub address: Option<WalletAddress>,
    /// Seconds until session expires
    pub expires_in_seconds: Option<u64>,
}

/// Wallet session state (internal, not exposed)
struct WalletSession {
    /// Decrypted mnemonic (zeroized on drop)
    #[allow(dead_code)]
    mnemonic: Zeroizing<String>,
    /// Derived address
    address: WalletAddress,
    /// Last activity timestamp
    last_activity: std::time::Instant,
}

impl WalletSession {
    fn is_expired(&self) -> bool {
        // 15-minute timeout
        self.last_activity.elapsed() > std::time::Duration::from_secs(15 * 60)
    }

    fn touch(&mut self) {
        self.last_activity = std::time::Instant::now();
    }
}

/// Mobile wallet interface
///
/// This is the main entry point for mobile apps. It manages wallet
/// state and provides a safe API that never exposes sensitive data.
#[derive(uniffi::Object)]
pub struct MobileWallet {
    session: Arc<Mutex<Option<WalletSession>>>,
    /// Node URL for RPC connections
    node_url: Arc<Mutex<Option<String>>>,
}

#[uniffi::export]
impl MobileWallet {
    /// Create a new mobile wallet instance
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            node_url: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the node URL for RPC connections
    pub async fn set_node_url(&self, url: String) {
        *self.node_url.lock().await = Some(url);
    }

    /// Generate a new wallet with a random mnemonic
    ///
    /// Returns the mnemonic phrase that MUST be shown to user for backup.
    /// After this call, the wallet is automatically unlocked.
    pub async fn generate_wallet(&self) -> MobileResult<String> {
        use bip39::{Language, Mnemonic};

        // Generate random mnemonic
        let mnemonic = Mnemonic::generate_in(Language::English, 24).map_err(|e| {
            MobileWalletError::InternalError {
                message: format!("Failed to generate mnemonic: {e}"),
            }
        })?;

        let phrase = mnemonic.to_string();

        // Unlock with the new mnemonic
        self.unlock_with_mnemonic(phrase.clone()).await?;

        Ok(phrase)
    }

    /// Unlock wallet with mnemonic phrase
    ///
    /// The mnemonic is stored in secure memory and zeroized on lock.
    pub async fn unlock_with_mnemonic(&self, mnemonic: String) -> MobileResult<WalletAddress> {
        // Validate mnemonic
        let _validated = bip39::Mnemonic::parse_normalized(&mnemonic)
            .map_err(|_| MobileWalletError::InvalidMnemonic)?;

        // Derive keys using botho-wallet
        let keys = botho_wallet::keys::WalletKeys::from_mnemonic(&mnemonic)
            .map_err(|_| MobileWalletError::InvalidMnemonic)?;

        // Get public address. The `display` field uses the canonical testnet
        // address format (`tbotho://1/<base58(view||spend)>`) so it is directly
        // parseable by the node for faucet requests and sends.
        let addr = keys.public_address();
        let view_bytes = addr.view_public_key().to_bytes();
        let spend_bytes = addr.spend_public_key().to_bytes();
        let address = WalletAddress {
            view_public_key: hex::encode(view_bytes),
            spend_public_key: hex::encode(spend_bytes),
            display: encode_testnet_address(&view_bytes, &spend_bytes),
        };

        // Create session
        let session = WalletSession {
            mnemonic: Zeroizing::new(mnemonic),
            address: address.clone(),
            last_activity: std::time::Instant::now(),
        };

        *self.session.lock().await = Some(session);

        Ok(address)
    }

    /// Lock the wallet and securely zeroize keys
    pub async fn lock(&self) -> bool {
        let mut session = self.session.lock().await;
        if session.is_some() {
            // Drop session - this triggers zeroization of mnemonic
            *session = None;
            true
        } else {
            false
        }
    }

    /// Get current session status
    pub async fn get_session_status(&self) -> SessionStatus {
        let mut session = self.session.lock().await;

        match session.as_mut() {
            Some(s) if s.is_expired() => {
                // Auto-lock on expiry
                *session = None;
                SessionStatus {
                    is_unlocked: false,
                    address: None,
                    expires_in_seconds: None,
                }
            }
            Some(s) => {
                let elapsed = s.last_activity.elapsed();
                let timeout = std::time::Duration::from_secs(15 * 60);
                let remaining = timeout.saturating_sub(elapsed);

                SessionStatus {
                    is_unlocked: true,
                    address: Some(s.address.clone()),
                    expires_in_seconds: Some(remaining.as_secs()),
                }
            }
            None => SessionStatus {
                is_unlocked: false,
                address: None,
                expires_in_seconds: None,
            },
        }
    }

    /// Get wallet balance
    ///
    /// Requires wallet to be unlocked. Syncs the wallet against the configured
    /// node: scans the chain for owned outputs (node-identical ownership check)
    /// and excludes any already spent on-chain or pending (spent-filter model),
    /// then returns the spendable balance.
    pub async fn get_balance(&self) -> MobileResult<WalletBalance> {
        let (keys, _addr) = self.signer_keys().await?;
        let rpc = self.node_rpc().await?;

        let synced = wallet_ops::sync(&rpc, &keys)
            .await
            .map_err(|message| MobileWalletError::NetworkError { message })?;

        let picocredits = synced.balance();
        Ok(WalletBalance {
            picocredits,
            formatted: format_bth(picocredits),
            utxo_count: synced.spendable.len() as u32,
            sync_height: synced.height,
        })
    }

    /// Get transaction history
    ///
    /// Returns the wallet's owned, unspent outputs as "receive" entries from
    /// the synced chain view, ordered most-recent-first and paginated. (A
    /// full send-history view requires local persistence of broadcast txs,
    /// which the thin bridge does not yet keep; received value is recovered
    /// from the chain scan.)
    pub async fn get_transaction_history(
        &self,
        limit: u32,
        offset: u32,
    ) -> MobileResult<Vec<TransactionEntry>> {
        let (keys, _addr) = self.signer_keys().await?;
        let rpc = self.node_rpc().await?;

        let synced = wallet_ops::sync(&rpc, &keys)
            .await
            .map_err(|message| MobileWalletError::NetworkError { message })?;

        let mut entries: Vec<TransactionEntry> = synced
            .spendable
            .iter()
            .map(|o| TransactionEntry {
                // The owned output's one-time target key is the closest stable
                // per-output identifier the thin client has.
                tx_hash: o.target_key.clone(),
                amount: o.amount as i64,
                block_height: synced.height,
                timestamp: 0,
                direction: "receive".to_string(),
                counterparty: None,
            })
            .collect();

        // Largest first as a deterministic ordering, then paginate.
        entries.sort_by(|a, b| b.amount.cmp(&a.amount));
        let start = offset as usize;
        let end = start.saturating_add(limit as usize).min(entries.len());
        if start >= entries.len() {
            return Ok(vec![]);
        }
        Ok(entries[start..end].to_vec())
    }

    /// Send a transfer of `amount_picocredits` to `to_address`.
    ///
    /// `to_address` must be a testnet address (`tbotho://1/<base58>`) or the
    /// legacy `view:<hex>,spend:<hex>` form. Syncs, builds a node-valid CLSAG
    /// transaction from the wallet's spendable outputs, submits it, and returns
    /// the transaction hash.
    pub async fn send_transaction(
        &self,
        to_address: String,
        amount_picocredits: u64,
    ) -> MobileResult<String> {
        let (keys, _addr) = self.signer_keys().await?;
        let recipient = parse_recipient(&to_address)?;
        let rpc = self.node_rpc().await?;

        let synced = wallet_ops::sync(&rpc, &keys)
            .await
            .map_err(|message| MobileWalletError::NetworkError { message })?;

        match wallet_ops::send(&rpc, &keys, recipient, amount_picocredits, &synced).await {
            Ok(tx_hash) => Ok(tx_hash),
            Err(SendError::Insufficient) => Err(MobileWalletError::InsufficientFunds),
            Err(SendError::Other(message)) => {
                // Distinguish a few actionable failure classes for the app.
                if message.contains("decoy") {
                    Err(MobileWalletError::NetworkError { message })
                } else {
                    Err(MobileWalletError::InternalError { message })
                }
            }
        }
    }

    /// Request testnet coins from the faucet for this wallet's address.
    ///
    /// Calls the configured node's `faucet_request` RPC with this wallet's
    /// address. (Point `set_node_url` at the faucet node, e.g.
    /// `https://faucet.botho.io`.)
    pub async fn request_faucet(&self) -> MobileResult<FaucetResult> {
        let (_keys, address) = self.signer_keys().await?;
        let rpc = self.node_rpc().await?;

        let result = rpc
            .call("faucet_request", serde_json::json!({ "address": address }))
            .await
            .map_err(|message| MobileWalletError::NetworkError { message })?;

        let success = result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let tx_hash = result
            .get("txHash")
            .or_else(|| result.get("tx_hash"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Amount may be returned as a number or a string.
        let amount = result.get("amount").map(parse_u64_value).unwrap_or(0);
        let amount_formatted = result
            .get("amountFormatted")
            .or_else(|| result.get("amount_formatted"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format_bth(amount));
        let message = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(FaucetResult {
            success,
            tx_hash,
            amount,
            amount_formatted,
            message,
        })
    }

    /// Get the configured node's status (height/sync/peers) for a node picker.
    ///
    /// Does not require an unlocked wallet.
    pub async fn get_node_status(&self) -> MobileResult<NodeStatusInfo> {
        let rpc = self.node_rpc().await?;
        let result = rpc
            .call("node_getStatus", serde_json::json!({}))
            .await
            .map_err(|message| MobileWalletError::NetworkError { message })?;

        Ok(NodeStatusInfo {
            version: result
                .get("nodeVersion")
                .or_else(|| result.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            network: result
                .get("network")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            chain_height: result
                .get("chainHeight")
                .or_else(|| result.get("chain_height"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            sync_status: result
                .get("syncStatus")
                .or_else(|| result.get("sync_status"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            peer_count: result
                .get("peerCount")
                .or_else(|| result.get("peer_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
        })
    }

    /// Get wallet public address
    pub async fn get_address(&self) -> MobileResult<WalletAddress> {
        let mut session_guard = self.session.lock().await;
        let session = session_guard
            .as_mut()
            .ok_or(MobileWalletError::WalletLocked)?;

        if session.is_expired() {
            drop(session_guard);
            self.lock().await;
            return Err(MobileWalletError::SessionExpired);
        }

        session.touch();
        Ok(session.address.clone())
    }
}

// Internal helpers, kept out of the `#[uniffi::export]` impl block so they are
// not part of the FFI surface (they return non-FFI types).
impl MobileWallet {
    /// Build a JSON-RPC client for the currently-configured node URL.
    async fn node_rpc(&self) -> MobileResult<NodeRpc> {
        let url =
            self.node_url
                .lock()
                .await
                .clone()
                .ok_or_else(|| MobileWalletError::NetworkError {
                    message: "No node URL configured. Call set_node_url first.".to_string(),
                })?;
        Ok(NodeRpc::new(&url))
    }

    /// Re-derive the wallet's signer keys (hex spend/view private keys) and the
    /// parseable address from the unlocked session's mnemonic. Validates the
    /// session is unlocked and not expired, and refreshes the activity timer.
    ///
    /// Private keys are derived on demand and never stored beyond this call.
    async fn signer_keys(&self) -> MobileResult<(SignerKeys, String)> {
        let mut session_guard = self.session.lock().await;
        let session = session_guard
            .as_mut()
            .ok_or(MobileWalletError::WalletLocked)?;

        if session.is_expired() {
            *session_guard = None;
            return Err(MobileWalletError::SessionExpired);
        }

        session.touch();
        let mnemonic = session.mnemonic.clone();
        let address = session.address.display.clone();
        drop(session_guard);

        let keys = botho_wallet::keys::WalletKeys::from_mnemonic(&mnemonic)
            .map_err(|_| MobileWalletError::InvalidMnemonic)?;
        let account = keys.account_key();

        let signer = SignerKeys {
            spend_private_key: hex::encode(account.spend_private_key().to_bytes()),
            view_private_key: hex::encode(account.view_private_key().to_bytes()),
        };
        Ok((signer, address))
    }
}

impl Default for MobileWallet {
    fn default() -> Self {
        Self::new()
    }
}

/// Picocredits per BTH (1 BTH = 1e12 picocredits).
const PICOCREDITS_PER_BTH: u64 = 1_000_000_000_000;

/// Format a picocredit amount as a human-readable BTH string.
fn format_bth(picocredits: u64) -> String {
    let whole = picocredits / PICOCREDITS_PER_BTH;
    let frac = picocredits % PICOCREDITS_PER_BTH;
    // Show 6 decimal places (microBTH resolution) like the rest of the wallet.
    let micro = frac / 1_000_000;
    format!("{whole}.{micro:06} BTH")
}

/// Parse a u64 from a JSON value that may be a number or a numeric string.
fn parse_u64_value(v: &serde_json::Value) -> u64 {
    if let Some(n) = v.as_u64() {
        n
    } else if let Some(s) = v.as_str() {
        s.parse().unwrap_or(0)
    } else {
        0
    }
}

/// Encode a testnet classical address as `tbotho://1/<base58(view||spend)>`.
fn encode_testnet_address(view_bytes: &[u8; 32], spend_bytes: &[u8; 32]) -> String {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(view_bytes);
    bytes.extend_from_slice(spend_bytes);
    let encoded = bs58::encode(&bytes).into_string();
    format!("{TESTNET_CLASSICAL_PREFIX}{encoded}")
}

/// Parse a recipient address string into the signer-core recipient form (hex
/// view/spend keys).
///
/// Accepts the testnet classical form (`tbotho://1/<base58(view||spend)>`) and
/// the legacy `view:<hex>,spend:<hex>` form.
fn parse_recipient(address: &str) -> MobileResult<RecipientAddress> {
    let address = address.trim();

    if let Some(encoded) = address.strip_prefix(TESTNET_CLASSICAL_PREFIX) {
        let bytes = bs58::decode(encoded)
            .into_vec()
            .map_err(|_| MobileWalletError::InvalidAddress)?;
        if bytes.len() != 64 {
            return Err(MobileWalletError::InvalidAddress);
        }
        return Ok(RecipientAddress {
            view_public_key: hex::encode(&bytes[0..32]),
            spend_public_key: hex::encode(&bytes[32..64]),
        });
    }

    // Legacy "view:<hex>,spend:<hex>"
    if address.starts_with("view:") {
        let parts: Vec<&str> = address.split(',').collect();
        if parts.len() == 2 {
            let view_hex = parts[0].trim().strip_prefix("view:");
            let spend_hex = parts[1].trim().strip_prefix("spend:");
            if let (Some(v), Some(s)) = (view_hex, spend_hex) {
                let view = hex::decode(v.trim()).map_err(|_| MobileWalletError::InvalidAddress)?;
                let spend = hex::decode(s.trim()).map_err(|_| MobileWalletError::InvalidAddress)?;
                if view.len() == 32 && spend.len() == 32 {
                    return Ok(RecipientAddress {
                        view_public_key: hex::encode(view),
                        spend_public_key: hex::encode(spend),
                    });
                }
            }
        }
    }

    Err(MobileWalletError::InvalidAddress)
}

// Required for bip39 usage
mod bip39 {
    pub use ::bip39::*;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bth_examples() {
        assert_eq!(format_bth(0), "0.000000 BTH");
        assert_eq!(format_bth(1_000_000_000_000), "1.000000 BTH");
        assert_eq!(format_bth(10_000_000_000_000), "10.000000 BTH");
        assert_eq!(format_bth(1_500_000), "0.000001 BTH");
    }

    #[test]
    fn testnet_address_roundtrip() {
        let view = [7u8; 32];
        let spend = [9u8; 32];
        let addr = encode_testnet_address(&view, &spend);
        assert!(addr.starts_with("tbotho://1/"));

        let parsed = parse_recipient(&addr).expect("parse tbotho");
        assert_eq!(parsed.view_public_key, hex::encode(view));
        assert_eq!(parsed.spend_public_key, hex::encode(spend));
    }

    #[test]
    fn parse_legacy_address() {
        let view = hex::encode([1u8; 32]);
        let spend = hex::encode([2u8; 32]);
        let legacy = format!("view:{view},spend:{spend}");
        let parsed = parse_recipient(&legacy).expect("parse legacy");
        assert_eq!(parsed.view_public_key, view);
        assert_eq!(parsed.spend_public_key, spend);
    }

    #[test]
    fn parse_invalid_address_errors() {
        assert!(matches!(
            parse_recipient("not-an-address"),
            Err(MobileWalletError::InvalidAddress)
        ));
        assert!(matches!(
            parse_recipient("tbotho://1/xx"),
            Err(MobileWalletError::InvalidAddress)
        ));
    }

    #[test]
    fn parse_u64_value_number_or_string() {
        assert_eq!(parse_u64_value(&serde_json::json!(42)), 42);
        assert_eq!(parse_u64_value(&serde_json::json!("123")), 123);
        assert_eq!(parse_u64_value(&serde_json::json!(null)), 0);
    }
}
