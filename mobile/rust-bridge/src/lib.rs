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

        // Get public address. The `display` field uses the canonical testnet v2
        // address format (`tbotho://2/<base58(view||spend||kem||dsa)>`) so it is
        // directly parseable by the node for faucet requests and sends.
        let addr = keys.public_address();
        let view_bytes = addr.view_public_key().to_bytes();
        let spend_bytes = addr.spend_public_key().to_bytes();
        let address = WalletAddress {
            view_public_key: hex::encode(view_bytes),
            spend_public_key: hex::encode(spend_bytes),
            display: encode_testnet_address(&addr)?,
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
    /// `to_address` must be a testnet v2 (post-quantum) address
    /// (`tbotho://2/<base58>`); classical-only forms are rejected because a
    /// send output on 6.0.0 requires the recipient's published ML-KEM key.
    /// Syncs, builds a node-valid CLSAG transaction from the wallet's
    /// spendable outputs, submits it, and returns the transaction hash.
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

        // Derive the wallet's OWN ML-KEM-768 public key from its BIP39 seed
        // using the node-identical derivation (`derive_pq_keys_from_seed`, via
        // the shared wasm-signer core). The change output is a self-send
        // encapsulated against this key so the sender can later recover its
        // change under the 6.0.0 hybrid scheme (issue #978).
        //
        // Derived here from the seed (rather than via `WalletKeys`) so it is
        // present even in the mobile crate's isolated build, where
        // `botho-wallet`'s `pq` feature is off and `public_address()` carries no
        // PQ keys. The seed-derived key is byte-identical to the one the node
        // and browser derive for the same mnemonic.
        let seed = bip39::Mnemonic::parse_normalized(&mnemonic)
            .map_err(|_| MobileWalletError::InvalidMnemonic)?
            .to_seed("");
        let sender_pq = bth_wasm_signer::core::derive_pq_public_keys_from_seed(&hex::encode(seed))
            .map_err(|message| MobileWalletError::InternalError { message })?;

        let signer = SignerKeys {
            spend_private_key: hex::encode(account.spend_private_key().to_bytes()),
            view_private_key: hex::encode(account.view_private_key().to_bytes()),
            sender_kem_public_key: sender_pq.kem_public_key,
            // The BIP39 seed (hex) so the RECEIVE scan can derive the wallet's
            // ML-KEM secret and detect hybrid incoming payments + its own change
            // (issue #988). Same feature-independent seed the send path uses.
            seed: hex::encode(seed),
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

/// Encode a testnet v2 address as
/// `tbotho://2/<base58(view||spend||kem||dsa)>` via the shared codec.
///
/// The address must carry both post-quantum keys (the wallet's seed-derived
/// addresses always do).
fn encode_testnet_address(addr: &bth_account_keys::PublicAddress) -> MobileResult<String> {
    bth_address_codec::encode_address(addr, bth_address_codec::Network::Testnet)
        .map_err(|_| MobileWalletError::InvalidAddress)
}

/// Parse a recipient address string into the signer-core recipient form (hex
/// view/spend keys plus the recipient's raw ML-KEM-768 public key).
///
/// Only the v2 post-quantum form
/// (`(t)botho://2/<base58(view||spend||kem||dsa)>`, decoded via the shared
/// [`bth_address_codec`]) is accepted. Under protocol 6.0.0 every send output
/// is a hybrid post-quantum output whose 1,088-byte ML-KEM ciphertext is
/// encapsulated against the recipient's published ML-KEM-768 key (issue #978),
/// so that key is required.
///
/// The retired v1 form and the legacy `view:<hex>,spend:<hex>` form publish no
/// ML-KEM key, so a send to them cannot produce a valid output — a KEM-less
/// output is rejected by consensus enforcement (`validate_transfer_tx`, #974).
/// Both are hard-errored here rather than silently building an unacceptable
/// transaction.
fn parse_recipient(address: &str) -> MobileResult<RecipientAddress> {
    let address = address.trim();

    if address.starts_with(bth_address_codec::MAINNET_PREFIX)
        || address.starts_with(bth_address_codec::TESTNET_PREFIX)
    {
        let (addr, _network) = bth_address_codec::decode_address(address)
            .map_err(|_| MobileWalletError::InvalidAddress)?;
        // A v2 address must publish well-formed post-quantum keys; a
        // classical-only decode cannot yield the ML-KEM key we must encapsulate
        // against.
        if !addr.has_pq_keys() {
            return Err(MobileWalletError::InvalidAddress);
        }
        return Ok(RecipientAddress {
            view_public_key: hex::encode(addr.view_public_key().to_bytes()),
            spend_public_key: hex::encode(addr.spend_public_key().to_bytes()),
            kem_public_key: hex::encode(addr.kem_public_key()),
        });
    }

    // Any non-v2 form (retired v1, legacy `view:…,spend:…`) publishes no ML-KEM
    // key and cannot be sent to on 6.0.0.
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

    /// Build a v2 address with dummy-but-correctly-sized PQ payloads.
    fn sample_v2_address() -> bth_account_keys::PublicAddress {
        use bth_account_keys::{AccountKey, ML_DSA_65_PUBLIC_KEY_LEN, ML_KEM_768_PUBLIC_KEY_LEN};
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::from_seed([5u8; 32]);
        let kem = vec![7u8; ML_KEM_768_PUBLIC_KEY_LEN];
        let dsa = vec![9u8; ML_DSA_65_PUBLIC_KEY_LEN];
        AccountKey::random(&mut rng)
            .default_subaddress()
            .with_pq_keys(kem, dsa)
    }

    #[test]
    fn testnet_address_roundtrip() {
        let public = sample_v2_address();
        let addr = encode_testnet_address(&public).expect("encode v2");
        assert!(addr.starts_with("tbotho://2/"));

        let parsed = parse_recipient(&addr).expect("parse tbotho");
        assert_eq!(
            parsed.view_public_key,
            hex::encode(public.view_public_key().to_bytes())
        );
        assert_eq!(
            parsed.spend_public_key,
            hex::encode(public.spend_public_key().to_bytes())
        );
        // The parsed recipient must carry the address's published ML-KEM key so
        // the send path can encapsulate the output's ciphertext (#978).
        assert_eq!(parsed.kem_public_key, hex::encode(public.kem_public_key()));
    }

    // Cross-encoder byte-identical vector (ADR 0008 D5): the SAME address must
    // encode to the SAME `tbotho://2/…` string via the node codec, the mobile
    // bridge, and the wasm-signer — and each must decode it back identically.
    #[test]
    fn cross_encoder_byte_identical_node_mobile_wasm() {
        let addr = sample_v2_address();

        // NODE path == the shared codec directly (node routes through it).
        let node = bth_address_codec::encode_address(&addr, bth_address_codec::Network::Testnet)
            .expect("node encode");
        // MOBILE bridge path.
        let mobile = encode_testnet_address(&addr).expect("mobile encode");
        // WASM-signer path.
        let wasm = bth_wasm_signer::core::encode_address_string(&addr, true).expect("wasm encode");

        assert_eq!(
            node, mobile,
            "node and mobile encoders must be byte-identical"
        );
        assert_eq!(node, wasm, "node and wasm encoders must be byte-identical");

        // All three decode the string back to the same view/spend/kem/dsa.
        let (node_dec, _) = bth_address_codec::decode_address(&node).expect("node decode");
        let mobile_dec = parse_recipient(&mobile).expect("mobile decode");
        let wasm_dec = bth_wasm_signer::core::decode_address_string(&wasm).expect("wasm decode");

        assert_eq!(
            hex::encode(node_dec.view_public_key().to_bytes()),
            mobile_dec.view_public_key
        );
        assert_eq!(
            hex::encode(node_dec.spend_public_key().to_bytes()),
            mobile_dec.spend_public_key
        );
        // The mobile parse path also recovers the published ML-KEM key.
        assert_eq!(
            hex::encode(node_dec.kem_public_key()),
            mobile_dec.kem_public_key
        );
        assert_eq!(node_dec.kem_public_key(), wasm_dec.kem_public_key());
        assert_eq!(node_dec.dsa_public_key(), wasm_dec.dsa_public_key());
        assert_eq!(
            node_dec.view_public_key().to_bytes(),
            wasm_dec.view_public_key().to_bytes()
        );
    }

    #[test]
    fn old_v1_address_rejected() {
        // A 64-byte v1 body under the retired prefix must not parse.
        let body = bs58::encode([0u8; 64]).into_string();
        assert!(matches!(
            parse_recipient(&format!("tbotho://1/{body}")),
            Err(MobileWalletError::InvalidAddress)
        ));
    }

    /// #978: the legacy `view:…,spend:…` form publishes no ML-KEM key, so on
    /// protocol 6.0.0 it is a hard error — a send to it would produce a
    /// KEM-less output that consensus enforcement rejects. It must no
    /// longer parse.
    #[test]
    fn parse_legacy_address_is_rejected() {
        let view = hex::encode([1u8; 32]);
        let spend = hex::encode([2u8; 32]);
        let legacy = format!("view:{view},spend:{spend}");
        assert!(matches!(
            parse_recipient(&legacy),
            Err(MobileWalletError::InvalidAddress)
        ));
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

/// Send-path acceptance: a transaction built exactly the way the mobile bridge
/// builds it must produce hybrid post-quantum outputs the node accepts and the
/// wallets can receive (issue #978). This exercises the two pieces the bridge
/// wires together — the recipient's ML-KEM key recovered by [`parse_recipient`]
/// from a real v2 address, and the sender's own ML-KEM key threaded through
/// [`SignerKeys`] — and proves the resulting outputs carry valid ciphertexts
/// and are detected by the node-identical hybrid scanner.
#[cfg(test)]
mod send_acceptance_tests {
    use super::{encode_testnet_address, parse_recipient};
    use crate::wallet_ops::SignerKeys;
    use bth_account_keys::AccountKey;
    use bth_crypto_pq::{derive_pq_keys_from_seed, PqKeyMaterial, BIP39_SEED_SIZE};
    use bth_transaction_clsag::{TxOutput, DEFAULT_RING_SIZE, MIN_TX_FEE};
    use bth_wasm_signer::core::{build_and_sign_with_rng, DecoyOutput, SignRequest, SpendInput};
    use rand::{rngs::StdRng, SeedableRng};

    /// Deterministic ML-KEM-768 / ML-DSA-65 keypair from a seed byte, using the
    /// node-identical derivation.
    fn pq_from(seed_byte: u8) -> PqKeyMaterial {
        derive_pq_keys_from_seed(&[seed_byte; BIP39_SEED_SIZE])
    }

    /// `count` random decoy outputs paid to throwaway recipients.
    fn make_decoys(count: usize, amount: u64, rng: &mut StdRng) -> Vec<DecoyOutput> {
        (0..count)
            .map(|_| {
                let decoy_account = AccountKey::random(rng);
                let out = TxOutput::new(amount, &decoy_account.default_subaddress());
                DecoyOutput {
                    target_key: hex::encode(out.target_key),
                    public_key: hex::encode(out.public_key),
                    amount,
                }
            })
            .collect()
    }

    /// #978 (mobile): a send built via the bridge's real components — recipient
    /// parsed from a `tbotho://2/` address (carrying its ML-KEM key) and the
    /// sender's own KEM key threaded through `SignerKeys` — yields a
    /// transaction whose recipient (index 0) and change (index 1) outputs
    /// BOTH carry a valid 1,088-byte ML-KEM ciphertext and are detected by
    /// the respective node-identical hybrid scanner. Before this fix the
    /// mobile bridge omitted both KEM keys, so the outputs were KEM-less
    /// and rejected by 6.0.0 consensus enforcement (`validate_transfer_tx`,
    /// #974).
    #[test]
    fn mobile_built_send_outputs_carry_kem_and_are_receivable() {
        let mut rng = StdRng::from_seed([57u8; 32]);

        // Sender: classical account + its own ML-KEM keypair.
        let sender = AccountKey::random(&mut rng);
        let sender_pq = pq_from(0x31);

        // Recipient: classical account + its own ML-KEM keypair, published in a
        // real v2 address string the mobile bridge parses.
        let recipient = AccountKey::random(&mut rng);
        let recipient_pq = pq_from(0x42);
        let recipient_address = recipient.default_subaddress().with_pq_keys(
            recipient_pq.kem_keypair.public_key().as_bytes().to_vec(),
            recipient_pq.sig_keypair.public_key().as_bytes().to_vec(),
        );
        let recipient_addr_str = encode_testnet_address(&recipient_address).expect("encode v2");

        // MOBILE parse path recovers the recipient's published ML-KEM key.
        let parsed_recipient = parse_recipient(&recipient_addr_str).expect("parse v2 recipient");
        assert!(
            !parsed_recipient.kem_public_key.is_empty(),
            "parsed recipient must carry the published ML-KEM key"
        );

        // MOBILE signer keys: spend/view + the sender's own derived KEM key
        // (same shape `signer_keys()` builds).
        let signer = SignerKeys {
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            sender_kem_public_key: hex::encode(sender_pq.kem_keypair.public_key().as_bytes()),
            // Send-only test: the seed is only consulted by the RECEIVE scan.
            seed: String::new(),
        };

        // Owned input well above amount + fee so a change output exists.
        let owned_amount = 10_000_000_000u64;
        let send_amount = 4_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        // Build the SignRequest exactly as `wallet_ops::send` does.
        let request = SignRequest {
            spend_private_key: signer.spend_private_key.clone(),
            view_private_key: signer.view_private_key.clone(),
            // Classical (KEM-less) input in this send-only test: empty seed +
            // no ciphertext takes the classical recovery path unchanged (#988).
            seed: signer.seed.clone(),
            inputs: vec![SpendInput {
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                output_index: 0,
                kem_ciphertext: None,
                decoys,
            }],
            recipient: parsed_recipient,
            sender_kem_public_key: signer.sender_kem_public_key.clone(),
            amount: send_amount,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };

        let tx = build_and_sign_with_rng(&request, &mut rng).expect("mobile build+sign");

        // Recipient (index 0) + change (index 1), each with a 1,088-byte
        // ML-KEM ciphertext.
        assert_eq!(tx.outputs.len(), 2, "expected recipient + change outputs");
        for (i, out) in tx.outputs.iter().enumerate() {
            let ct = out
                .kem_ciphertext
                .as_ref()
                .unwrap_or_else(|| panic!("output {i} is missing its ML-KEM ciphertext"));
            assert_eq!(
                ct.len(),
                bth_crypto_pq::ML_KEM_768_CIPHERTEXT_BYTES,
                "output {i} ciphertext must be exactly 1088 bytes"
            );
        }

        // The recipient's node-identical hybrid scan detects its output; the
        // sender's detects its own change. If either KEM key were wrong the
        // one-time key would not match and the funds would be unspendable.
        assert_eq!(
            tx.outputs[0].belongs_to_hybrid(&recipient, &recipient_pq.kem_keypair, 0),
            Some(0),
            "recipient must detect the mobile-built output at subaddress 0"
        );
        assert_eq!(
            tx.outputs[1].belongs_to_hybrid(&sender, &sender_pq.kem_keypair, 1),
            Some(0),
            "sender must detect its own change at default subaddress"
        );

        // Cross-check: neither wallet detects the other's output (ciphertexts
        // are bound to distinct ML-KEM keys).
        assert_eq!(
            tx.outputs[1].belongs_to_hybrid(&recipient, &recipient_pq.kem_keypair, 1),
            None,
            "recipient must not detect the sender's change"
        );
    }
}
