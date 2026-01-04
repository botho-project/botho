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

        // Get public address
        let addr = keys.public_address();
        let address = WalletAddress {
            view_public_key: hex::encode(addr.view_public_key().to_bytes()),
            spend_public_key: hex::encode(addr.spend_public_key().to_bytes()),
            display: keys.address_string(),
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
    /// Requires wallet to be unlocked. Syncs with the network.
    pub async fn get_balance(&self) -> MobileResult<WalletBalance> {
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

        // TODO: Implement actual network sync
        // For now, return placeholder
        Ok(WalletBalance {
            picocredits: 0,
            formatted: "0.000000 BTH".to_string(),
            utxo_count: 0,
            sync_height: 0,
        })
    }

    /// Get transaction history
    ///
    /// Returns recent transactions, paginated.
    pub async fn get_transaction_history(
        &self,
        limit: u32,
        offset: u32,
    ) -> MobileResult<Vec<TransactionEntry>> {
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

        // TODO: Implement actual transaction history fetch
        let _ = (limit, offset);
        Ok(vec![])
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

impl Default for MobileWallet {
    fn default() -> Self {
        Self::new()
    }
}

// Required for bip39 usage
mod bip39 {
    pub use ::bip39::*;
}
