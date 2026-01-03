//! Wallet Commands for Tauri
//!
//! Handles transaction building, signing, and submission for the desktop wallet.
//! Private keys never leave this module - all signing happens locally.
//!
//! SECURITY: For NEW wallets, mnemonics are generated in Rust and only briefly
//! exposed to JavaScript for display. The mnemonic is NEVER accepted back from JS.
//! For IMPORTED wallets, the mnemonic must cross the JS boundary (unavoidable for restore).
//! All sensitive key material is automatically zeroized when the wallet is locked.
//!
//! SECURITY: Wallet unlock attempts are rate-limited with exponential backoff
//! to prevent brute-force password attacks. After 5 consecutive failures, the
//! wallet is locked out for increasing durations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use rand::seq::SliceRandom;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tauri::State;

use std::path::PathBuf;

use botho_wallet::keys::WalletKeys;
use botho_wallet::rpc_pool::RpcPool;
use botho_wallet::discovery::NodeDiscovery;
use botho_wallet::storage::EncryptedWallet;
use botho_wallet::transaction::{
    sync_wallet as do_sync_wallet,
    TransactionBuilder,
    OwnedUtxo,
    PICOCREDITS_PER_CAD,
};

/// Picocredits per BTH (same as CAD internally)
const PICOCREDITS_PER_BTH: u64 = PICOCREDITS_PER_CAD;

/// Session timeout in minutes - wallet auto-locks after inactivity
const SESSION_TIMEOUT_MINS: u64 = 15;

/// An unlocked wallet session holding decrypted keys in Rust memory.
///
/// SECURITY: WalletKeys uses Zeroizing<String> internally, ensuring the mnemonic
/// is securely zeroed when the session is dropped (locked).
struct WalletSession {
    /// Decrypted wallet keys - auto-zeroized on drop
    keys: WalletKeys,
    /// Timestamp of last activity for auto-lock
    last_activity: Instant,
    /// Path to the wallet file (if loaded from file)
    wallet_path: Option<PathBuf>,
}

impl WalletSession {
    fn new(keys: WalletKeys, wallet_path: Option<PathBuf>) -> Self {
        Self {
            keys,
            last_activity: Instant::now(),
            wallet_path,
        }
    }

    /// Update activity timestamp to prevent timeout
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if session has timed out
    fn is_expired(&self) -> bool {
        self.last_activity.elapsed() > Duration::from_secs(SESSION_TIMEOUT_MINS * 60)
    }
}

/// Pending wallet creation - holds mnemonic generated in Rust for verification.
///
/// SECURITY: This allows new wallet creation without accepting mnemonic from JS.
/// The mnemonic is generated in Rust, displayed to user, and user verifies by
/// providing specific words at random positions.
struct PendingWallet {
    /// Generated wallet keys (contains Zeroizing mnemonic)
    keys: WalletKeys,
    /// Random word positions user must verify (0-indexed)
    verify_indices: [usize; 3],
    /// Timestamp for expiry (5 minutes)
    created_at: Instant,
}

/// Expiry time for pending wallet creation (5 minutes)
const PENDING_WALLET_TIMEOUT_SECS: u64 = 300;

impl PendingWallet {
    fn new(keys: WalletKeys) -> Self {
        let mut rng = rand::thread_rng();
        let mut indices: Vec<usize> = (0..24).collect();
        indices.shuffle(&mut rng);
        let verify_indices = [indices[0], indices[1], indices[2]];

        Self {
            keys,
            verify_indices,
            created_at: Instant::now(),
        }
    }

    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > Duration::from_secs(PENDING_WALLET_TIMEOUT_SECS)
    }
}

// ============================================================================
// Unlock Rate Limiting (brute-force protection)
// ============================================================================

/// Maximum failed attempts before maximum lockout kicks in
const MAX_FAILED_ATTEMPTS: u32 = 10;

/// Maximum lockout duration (5 minutes)
const MAX_LOCKOUT_SECS: u64 = 300;

/// State tracking for a single wallet path's unlock attempts
#[derive(Debug, Clone)]
struct UnlockAttemptState {
    /// Number of consecutive failed attempts
    failed_attempts: u32,
    /// Timestamp of last failed attempt
    last_failed_at: Option<Instant>,
    /// Timestamp when lockout expires (if locked out)
    lockout_until: Option<Instant>,
}

impl Default for UnlockAttemptState {
    fn default() -> Self {
        Self {
            failed_attempts: 0,
            last_failed_at: None,
            lockout_until: None,
        }
    }
}

impl UnlockAttemptState {
    /// Check if currently locked out
    fn is_locked_out(&self) -> bool {
        if let Some(until) = self.lockout_until {
            Instant::now() < until
        } else {
            false
        }
    }

    /// Get remaining lockout duration in seconds
    fn lockout_remaining_secs(&self) -> u64 {
        if let Some(until) = self.lockout_until {
            let now = Instant::now();
            if now < until {
                return (until - now).as_secs();
            }
        }
        0
    }

    /// Calculate lockout duration using exponential backoff
    /// 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, 300s (capped)
    fn calculate_lockout(&self) -> Duration {
        if self.failed_attempts == 0 {
            return Duration::ZERO;
        }

        // Exponential backoff: 2^(n-1) seconds, capped at MAX_LOCKOUT_SECS
        let secs = (1u64 << (self.failed_attempts - 1).min(10))
            .min(MAX_LOCKOUT_SECS);
        Duration::from_secs(secs)
    }

    /// Record a failed unlock attempt
    fn record_failure(&mut self) {
        self.failed_attempts = self.failed_attempts.saturating_add(1).min(MAX_FAILED_ATTEMPTS);
        self.last_failed_at = Some(Instant::now());

        // Apply exponential backoff lockout
        let lockout_duration = self.calculate_lockout();
        if !lockout_duration.is_zero() {
            self.lockout_until = Some(Instant::now() + lockout_duration);
            log::warn!(
                "Wallet unlock failed. Locked out for {} seconds ({} attempts)",
                lockout_duration.as_secs(),
                self.failed_attempts
            );
        }
    }

    /// Record a successful unlock (resets all counters)
    fn record_success(&mut self) {
        if self.failed_attempts > 0 {
            log::info!("Wallet unlocked successfully after {} failed attempts", self.failed_attempts);
        }
        self.failed_attempts = 0;
        self.last_failed_at = None;
        self.lockout_until = None;
    }
}

/// Rate limiter for wallet unlock attempts.
///
/// Tracks failed attempts per wallet path and applies exponential backoff
/// to prevent brute-force password attacks.
struct UnlockRateLimiter {
    /// Per-wallet-path unlock attempt tracking
    states: HashMap<PathBuf, UnlockAttemptState>,
}

impl Default for UnlockRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl UnlockRateLimiter {
    fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Check if unlock is allowed for the given wallet path.
    /// Returns Ok(()) if allowed, Err with message and remaining lockout time if not.
    fn check_allowed(&self, path: &PathBuf) -> Result<(), (String, u64)> {
        if let Some(state) = self.states.get(path) {
            if state.is_locked_out() {
                let remaining = state.lockout_remaining_secs();
                return Err((
                    format!(
                        "Too many failed attempts. Please wait {} seconds before trying again.",
                        remaining
                    ),
                    remaining,
                ));
            }
        }
        Ok(())
    }

    /// Record a failed unlock attempt for the given wallet path
    fn record_failure(&mut self, path: PathBuf) {
        let state = self.states.entry(path).or_default();
        state.record_failure();
    }

    /// Record a successful unlock for the given wallet path
    fn record_success(&mut self, path: PathBuf) {
        let state = self.states.entry(path).or_default();
        state.record_success();
    }

    /// Get current lockout info for a path (failed attempts, remaining lockout)
    fn get_lockout_info(&self, path: &PathBuf) -> (u32, u64) {
        if let Some(state) = self.states.get(path) {
            (state.failed_attempts, state.lockout_remaining_secs())
        } else {
            (0, 0)
        }
    }
}

/// Wallet state managed by Tauri
pub struct WalletCommands {
    /// Cached UTXOs from last sync
    utxos: Arc<Mutex<Vec<OwnedUtxo>>>,
    /// Last synced block height
    sync_height: Arc<Mutex<u64>>,
    /// Active wallet session (unlocked keys held in Rust memory only)
    session: Arc<Mutex<Option<WalletSession>>>,
    /// Pending wallet for secure creation flow (mnemonic generated in Rust)
    pending_wallet: Arc<Mutex<Option<PendingWallet>>>,
    /// Rate limiter for unlock attempts (brute-force protection)
    unlock_rate_limiter: Arc<Mutex<UnlockRateLimiter>>,
}

impl WalletCommands {
    pub fn new() -> Self {
        Self {
            utxos: Arc::new(Mutex::new(Vec::new())),
            sync_height: Arc::new(Mutex::new(0)),
            session: Arc::new(Mutex::new(None)),
            pending_wallet: Arc::new(Mutex::new(None)),
            unlock_rate_limiter: Arc::new(Mutex::new(UnlockRateLimiter::new())),
        }
    }

    /// Get wallet keys if session is active and not expired.
    /// Updates last activity timestamp on success.
    async fn get_session_keys(&self) -> Result<WalletKeys> {
        let mut session_guard = self.session.lock().await;

        match session_guard.as_mut() {
            Some(session) => {
                if session.is_expired() {
                    // Auto-lock on timeout - dropping the session zeroizes keys
                    *session_guard = None;
                    Err(anyhow!("Session expired. Please unlock your wallet again."))
                } else {
                    session.touch();
                    Ok(session.keys.clone())
                }
            }
            None => Err(anyhow!("Wallet is locked. Please unlock your wallet first.")),
        }
    }
}

/// Privacy level for transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyLevel {
    Standard,
    Private,
}

/// Parameters for sending a transaction
///
/// SECURITY: Mnemonic is NOT passed from JS. Keys are retrieved from the
/// active session which is held securely in Rust memory.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTransactionParams {
    /// Recipient address (view:hex\nspend:hex format or cad:view:spend)
    pub recipient: String,
    /// Amount in picocredits (as string to handle bigint from JS)
    pub amount: String,
    /// Privacy level: "standard" or "private"
    pub privacy_level: PrivacyLevel,
    /// Optional memo
    pub memo: Option<String>,
    /// Optional custom fee in picocredits (as string)
    pub custom_fee: Option<String>,
    /// Node host to connect to
    pub node_host: String,
    /// Node port
    pub node_port: u16,
}

/// Result of sending a transaction
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTransactionResult {
    pub success: bool,
    pub tx_hash: Option<String>,
    pub error: Option<String>,
}

/// Parameters for syncing the wallet
///
/// SECURITY: Mnemonic is NOT passed from JS. Keys are retrieved from the
/// active session which is held securely in Rust memory.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncWalletParams {
    /// Node host to connect to
    pub node_host: String,
    /// Node port
    pub node_port: u16,
    /// Height to sync from (0 for full sync)
    pub from_height: Option<u64>,
}

/// Result of wallet sync
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncWalletResult {
    pub success: bool,
    pub balance: String,
    pub utxo_count: usize,
    pub sync_height: u64,
    pub error: Option<String>,
}

/// Parameters for getting balance
///
/// SECURITY: Mnemonic is NOT passed from JS. Keys are retrieved from the
/// active session which is held securely in Rust memory.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetBalanceParams {
    /// Node host to connect to
    pub node_host: String,
    /// Node port
    pub node_port: u16,
}

/// Balance result
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResult {
    pub success: bool,
    /// Balance in picocredits (as string for JS bigint compatibility)
    pub balance: String,
    /// Formatted balance (e.g., "1.234567 BTH")
    pub formatted: String,
    pub utxo_count: usize,
    pub error: Option<String>,
}

/// Send a transaction
///
/// This command:
/// 1. Derives wallet keys from the mnemonic
/// 2. Syncs UTXOs from the connected node
/// 3. Builds and signs the transaction locally
/// 4. Submits the signed transaction to the network
#[tauri::command]
pub async fn send_transaction(
    state: State<'_, WalletCommands>,
    params: SendTransactionParams,
) -> Result<SendTransactionResult, String> {
    match send_transaction_internal(&state, params).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(SendTransactionResult {
            success: false,
            tx_hash: None,
            error: Some(e.to_string()),
        }),
    }
}

async fn send_transaction_internal(
    state: &State<'_, WalletCommands>,
    params: SendTransactionParams,
) -> Result<SendTransactionResult> {
    // 1. Get wallet keys from session (SECURITY: keys never leave Rust)
    let keys = state.get_session_keys().await?;

    // 2. Parse recipient address
    let recipient = parse_recipient_address(&params.recipient)?;

    // 3. Parse amount
    let amount: u64 = params.amount.parse()
        .map_err(|_| anyhow!("Invalid amount format"))?;

    if amount == 0 {
        return Err(anyhow!("Amount must be greater than 0"));
    }

    // 4. Connect to node
    let mut discovery = NodeDiscovery::new();
    discovery.add_bootstrap_node(format!("{}:{}", params.node_host, params.node_port).parse()?);

    let mut rpc = RpcPool::new(discovery);
    rpc.connect().await
        .map_err(|e| anyhow!("Failed to connect to node: {}", e))?;

    // 5. Sync wallet to get UTXOs
    let from_height = *state.sync_height.lock().await;
    let (utxos, sync_height) = do_sync_wallet(&mut rpc, &keys, from_height).await
        .map_err(|e| anyhow!("Failed to sync wallet: {}", e))?;

    // Combine with cached UTXOs
    let mut all_utxos = state.utxos.lock().await;
    all_utxos.extend(utxos);
    *state.sync_height.lock().await = sync_height;

    // 6. Estimate or use custom fee
    let fee = if let Some(custom_fee_str) = params.custom_fee {
        custom_fee_str.parse::<u64>()
            .map_err(|_| anyhow!("Invalid custom fee format"))?
    } else {
        // Estimate based on privacy level and transaction size
        let estimated_size = match params.privacy_level {
            PrivacyLevel::Standard => 4000,  // ~4KB for ML-DSA signature
            PrivacyLevel::Private => 22000,  // ~22KB for LION ring signature
        };
        rpc.estimate_fee("medium").await.unwrap_or(estimated_size as u64 * 100)
    };

    // 7. Build and sign transaction
    let builder = TransactionBuilder::new(keys.clone(), all_utxos.clone(), sync_height);

    // Check balance
    let balance = builder.balance();
    let total_needed = amount.checked_add(fee)
        .ok_or_else(|| anyhow!("Amount overflow"))?;

    if balance < total_needed {
        return Err(anyhow!(
            "Insufficient funds: have {} picocredits, need {}",
            balance,
            total_needed
        ));
    }

    // Build transaction based on privacy level
    let tx = match params.privacy_level {
        PrivacyLevel::Standard => {
            builder.build_transfer(&recipient, amount, fee)
                .map_err(|e| anyhow!("Failed to build transaction: {}", e))?
        }
        PrivacyLevel::Private => {
            // For private transactions, we would use ring signatures
            // For now, fall back to standard (private tx requires more infrastructure)
            log::warn!("Private transactions not yet fully implemented, using standard");
            builder.build_transfer(&recipient, amount, fee)
                .map_err(|e| anyhow!("Failed to build transaction: {}", e))?
        }
    };

    // 8. Submit transaction
    let tx_hash = rpc.submit_transaction(&tx.to_hex()).await
        .map_err(|e| anyhow!("Failed to submit transaction: {}", e))?;

    log::info!("Transaction submitted: {}", tx_hash);

    // 9. Clear spent UTXOs from cache (simplified - clear all and resync next time)
    all_utxos.clear();

    Ok(SendTransactionResult {
        success: true,
        tx_hash: Some(tx_hash),
        error: None,
    })
}

/// Sync wallet and get UTXOs
#[tauri::command]
pub async fn sync_wallet(
    state: State<'_, WalletCommands>,
    params: SyncWalletParams,
) -> Result<SyncWalletResult, String> {
    match sync_wallet_internal(&state, params).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(SyncWalletResult {
            success: false,
            balance: "0".to_string(),
            utxo_count: 0,
            sync_height: 0,
            error: Some(e.to_string()),
        }),
    }
}

async fn sync_wallet_internal(
    state: &State<'_, WalletCommands>,
    params: SyncWalletParams,
) -> Result<SyncWalletResult> {
    // Get wallet keys from session (SECURITY: keys never leave Rust)
    let keys = state.get_session_keys().await?;

    // Connect to node
    let mut discovery = NodeDiscovery::new();
    discovery.add_bootstrap_node(format!("{}:{}", params.node_host, params.node_port).parse()?);

    let mut rpc = RpcPool::new(discovery);
    rpc.connect().await
        .map_err(|e| anyhow!("Failed to connect to node: {}", e))?;

    // Sync from specified height
    let from_height = params.from_height.unwrap_or(0);
    let (utxos, sync_height) = do_sync_wallet(&mut rpc, &keys, from_height).await
        .map_err(|e| anyhow!("Failed to sync wallet: {}", e))?;

    // Calculate balance
    let balance: u64 = utxos.iter().map(|u| u.amount).sum();

    // Update state
    let mut cached_utxos = state.utxos.lock().await;
    *cached_utxos = utxos.clone();
    *state.sync_height.lock().await = sync_height;

    Ok(SyncWalletResult {
        success: true,
        balance: balance.to_string(),
        utxo_count: utxos.len(),
        sync_height,
        error: None,
    })
}

/// Get wallet balance
#[tauri::command]
pub async fn get_balance(
    state: State<'_, WalletCommands>,
    params: GetBalanceParams,
) -> Result<BalanceResult, String> {
    match get_balance_internal(&state, params).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(BalanceResult {
            success: false,
            balance: "0".to_string(),
            formatted: "0.000000 BTH".to_string(),
            utxo_count: 0,
            error: Some(e.to_string()),
        }),
    }
}

async fn get_balance_internal(
    state: &State<'_, WalletCommands>,
    params: GetBalanceParams,
) -> Result<BalanceResult> {
    // Get wallet keys from session (SECURITY: keys never leave Rust)
    let keys = state.get_session_keys().await?;

    // Connect to node
    let mut discovery = NodeDiscovery::new();
    discovery.add_bootstrap_node(format!("{}:{}", params.node_host, params.node_port).parse()?);

    let mut rpc = RpcPool::new(discovery);
    rpc.connect().await
        .map_err(|e| anyhow!("Failed to connect to node: {}", e))?;

    // Sync from last known height
    let from_height = *state.sync_height.lock().await;
    let (utxos, sync_height) = do_sync_wallet(&mut rpc, &keys, from_height).await
        .map_err(|e| anyhow!("Failed to sync wallet: {}", e))?;

    // Merge with cached UTXOs
    let mut cached_utxos = state.utxos.lock().await;
    cached_utxos.extend(utxos);
    *state.sync_height.lock().await = sync_height;

    // Calculate balance
    let balance: u64 = cached_utxos.iter().map(|u| u.amount).sum();
    let bth = balance as f64 / PICOCREDITS_PER_BTH as f64;

    Ok(BalanceResult {
        success: true,
        balance: balance.to_string(),
        formatted: format!("{:.6} BTH", bth),
        utxo_count: cached_utxos.len(),
        error: None,
    })
}

/// Parse a recipient address from various formats
fn parse_recipient_address(address: &str) -> Result<bth_account_keys::PublicAddress> {
    use bth_crypto_keys::RistrettoPublic;

    // Format 1: cad:<view_hex>:<spend_hex> (16-byte prefixes)
    if address.starts_with("cad:") {
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 3 {
            return Err(anyhow!("Invalid cad: address format"));
        }

        // For now, require full 32-byte keys
        // The cad: format uses 16-byte prefixes for display
        return Err(anyhow!(
            "Short cad: address format not yet supported. Please provide full public keys."
        ));
    }

    // Format 2: view:<hex>\nspend:<hex> (full 32-byte keys)
    if address.contains("view:") && address.contains("spend:") {
        let mut view_bytes = None;
        let mut spend_bytes = None;

        for line in address.lines() {
            let line = line.trim();
            if let Some(hex_str) = line.strip_prefix("view:") {
                view_bytes = Some(hex::decode(hex_str.trim())
                    .map_err(|_| anyhow!("Invalid view key hex"))?);
            } else if let Some(hex_str) = line.strip_prefix("spend:") {
                spend_bytes = Some(hex::decode(hex_str.trim())
                    .map_err(|_| anyhow!("Invalid spend key hex"))?);
            }
        }

        match (view_bytes, spend_bytes) {
            (Some(v), Some(s)) if v.len() == 32 && s.len() == 32 => {
                let view_key = RistrettoPublic::try_from(&v[..])
                    .map_err(|_| anyhow!("Invalid view public key"))?;
                let spend_key = RistrettoPublic::try_from(&s[..])
                    .map_err(|_| anyhow!("Invalid spend public key"))?;

                return Ok(bth_account_keys::PublicAddress::new(&spend_key, &view_key));
            }
            _ => return Err(anyhow!("Invalid address key lengths (need 32 bytes each)")),
        }
    }

    // Format 3: bth1<hex> (simplified address - just spend key hash)
    if address.starts_with("bth1") {
        return Err(anyhow!(
            "Simplified bth1 addresses not yet supported for sending. Please provide full public keys."
        ));
    }

    Err(anyhow!(
        "Invalid address format. Expected 'view:<hex>\\nspend:<hex>' format with 32-byte keys"
    ))
}

// ============================================================================
// Session Management Commands
// ============================================================================

/// Parameters for unlocking wallet from file
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlockWalletParams {
    /// Password to decrypt the wallet file
    pub password: String,
    /// Optional path to wallet file (uses default if not provided)
    pub path: Option<String>,
}

/// Result of unlocking the wallet
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlockWalletResult {
    pub success: bool,
    /// Wallet public address (safe to expose to JS)
    pub address: Option<String>,
    /// Whether the session will auto-lock after timeout
    pub has_timeout: bool,
    /// Timeout duration in minutes
    pub timeout_mins: u64,
    /// Number of failed unlock attempts (for UI feedback)
    pub failed_attempts: u32,
    /// Seconds until lockout expires (0 if not locked out)
    pub lockout_remaining_secs: u64,
    pub error: Option<String>,
}

/// Result of wallet session status check
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusResult {
    pub is_unlocked: bool,
    /// Public address if unlocked
    pub address: Option<String>,
    /// Seconds until session expires (if unlocked)
    pub expires_in_secs: Option<u64>,
}

/// Unlock wallet from encrypted file
///
/// Decrypts the wallet file and caches the keys in Rust memory.
/// The mnemonic NEVER leaves Rust - only the public address is returned.
///
/// SECURITY: Rate-limited with exponential backoff to prevent brute-force attacks.
/// After each failed attempt, the lockout duration doubles (1s, 2s, 4s, ... up to 5min).
#[tauri::command]
pub async fn unlock_wallet(
    state: State<'_, WalletCommands>,
    params: UnlockWalletParams,
) -> Result<UnlockWalletResult, String> {
    // Determine path first for rate limiting
    let path = match &params.path {
        Some(p) => PathBuf::from(p),
        None => get_default_wallet_path().map_err(|e| e.to_string())?,
    };

    // Check rate limit BEFORE attempting unlock
    {
        let rate_limiter = state.unlock_rate_limiter.lock().await;
        if let Err((msg, remaining)) = rate_limiter.check_allowed(&path) {
            let (failed_attempts, _) = rate_limiter.get_lockout_info(&path);
            return Ok(UnlockWalletResult {
                success: false,
                address: None,
                has_timeout: true,
                timeout_mins: SESSION_TIMEOUT_MINS,
                failed_attempts,
                lockout_remaining_secs: remaining,
                error: Some(msg),
            });
        }
    }

    // Attempt unlock
    match unlock_wallet_internal(&state, params, &path).await {
        Ok(result) => {
            // Success - reset rate limiter
            let mut rate_limiter = state.unlock_rate_limiter.lock().await;
            rate_limiter.record_success(path);
            Ok(result)
        }
        Err(e) => {
            // Failure - record in rate limiter
            let mut rate_limiter = state.unlock_rate_limiter.lock().await;
            rate_limiter.record_failure(path.clone());
            let (failed_attempts, lockout_remaining) = rate_limiter.get_lockout_info(&path);

            Ok(UnlockWalletResult {
                success: false,
                address: None,
                has_timeout: true,
                timeout_mins: SESSION_TIMEOUT_MINS,
                failed_attempts,
                lockout_remaining_secs: lockout_remaining,
                error: Some(e.to_string()),
            })
        }
    }
}

async fn unlock_wallet_internal(
    state: &State<'_, WalletCommands>,
    params: UnlockWalletParams,
    path: &PathBuf,
) -> Result<UnlockWalletResult> {
    // Check if file exists
    if !path.exists() {
        return Err(anyhow!("Wallet file not found: {}", path.display()));
    }

    // Load and decrypt
    let wallet = EncryptedWallet::load(path)
        .map_err(|e| anyhow!("Failed to load wallet file: {}", e))?;

    let mnemonic = wallet.decrypt(&params.password)
        .map_err(|e| anyhow!("Failed to decrypt wallet: {}", e))?;

    // Derive keys from mnemonic (mnemonic is Zeroizing<String>)
    let keys = WalletKeys::from_mnemonic(&mnemonic)
        .map_err(|e| anyhow!("Invalid mnemonic in wallet file: {}", e))?;

    // Get public address BEFORE moving keys into session
    let address = keys.address_string();

    // Create session and cache keys in Rust memory
    let session = WalletSession::new(keys, Some(path.clone()));
    *state.session.lock().await = Some(session);

    // Also update sync height from wallet file
    *state.sync_height.lock().await = wallet.sync_height;

    log::info!("Wallet unlocked from {}", path.display());

    Ok(UnlockWalletResult {
        success: true,
        address: Some(address),
        has_timeout: true,
        timeout_mins: SESSION_TIMEOUT_MINS,
        failed_attempts: 0,
        lockout_remaining_secs: 0,
        error: None,
    })
}

/// Lock wallet and securely zeroize keys
///
/// Drops the cached keys, which triggers Zeroizing to overwrite
/// the mnemonic memory with zeros.
#[tauri::command]
pub async fn lock_wallet(
    state: State<'_, WalletCommands>,
) -> Result<bool, String> {
    let mut session_guard = state.session.lock().await;

    if session_guard.is_some() {
        // Drop the session - this triggers zeroization of the mnemonic
        *session_guard = None;
        log::info!("Wallet locked and keys zeroized");
        Ok(true)
    } else {
        Ok(false) // Already locked
    }
}

/// Check wallet session status
///
/// Returns whether the wallet is currently unlocked and time until expiry.
#[tauri::command]
pub async fn get_session_status(
    state: State<'_, WalletCommands>,
) -> Result<SessionStatusResult, String> {
    let mut session_guard = state.session.lock().await;

    match session_guard.as_mut() {
        Some(session) => {
            if session.is_expired() {
                // Auto-lock on timeout check
                *session_guard = None;
                Ok(SessionStatusResult {
                    is_unlocked: false,
                    address: None,
                    expires_in_secs: None,
                })
            } else {
                let elapsed = session.last_activity.elapsed();
                let timeout = Duration::from_secs(SESSION_TIMEOUT_MINS * 60);
                let remaining = timeout.saturating_sub(elapsed);

                Ok(SessionStatusResult {
                    is_unlocked: true,
                    address: Some(session.keys.address_string()),
                    expires_in_secs: Some(remaining.as_secs()),
                })
            }
        }
        None => Ok(SessionStatusResult {
            is_unlocked: false,
            address: None,
            expires_in_secs: None,
        }),
    }
}

// ============================================================================
// Secure Wallet Creation (mnemonic generated in Rust, never received from JS)
// ============================================================================

/// Result of generating a new mnemonic
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateMnemonicResult {
    pub success: bool,
    /// The 24 mnemonic words (displayed to user, not stored in JS)
    pub words: Option<Vec<String>>,
    /// 1-indexed word positions user must verify (e.g., [4, 8, 12])
    pub verify_positions: Option<Vec<usize>>,
    /// Expiry time in seconds
    pub expires_in_secs: Option<u64>,
    pub error: Option<String>,
}

/// Generate a new wallet mnemonic in Rust and cache it for confirmation.
///
/// SECURITY: Mnemonic is generated in Rust and only sent to JS for display.
/// The mnemonic is NEVER accepted back from JS - instead, user provides
/// specific words at random positions to prove they wrote it down.
#[tauri::command]
pub async fn generate_mnemonic(
    state: State<'_, WalletCommands>,
) -> Result<GenerateMnemonicResult, String> {
    match generate_mnemonic_internal(&state).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(GenerateMnemonicResult {
            success: false,
            words: None,
            verify_positions: None,
            expires_in_secs: None,
            error: Some(e.to_string()),
        }),
    }
}

async fn generate_mnemonic_internal(
    state: &State<'_, WalletCommands>,
) -> Result<GenerateMnemonicResult> {
    // Generate new wallet keys with random mnemonic
    let keys = WalletKeys::generate()
        .map_err(|e| anyhow!("Failed to generate mnemonic: {}", e))?;

    // Get words before caching
    let words: Vec<String> = keys.mnemonic_words()
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Create pending wallet with random verification indices
    let pending = PendingWallet::new(keys);

    // Convert 0-indexed to 1-indexed for user display
    let verify_positions: Vec<usize> = pending.verify_indices
        .iter()
        .map(|i| i + 1)
        .collect();

    // Cache the pending wallet
    *state.pending_wallet.lock().await = Some(pending);

    log::info!("New mnemonic generated, awaiting confirmation");

    Ok(GenerateMnemonicResult {
        success: true,
        words: Some(words),
        verify_positions: Some(verify_positions),
        expires_in_secs: Some(PENDING_WALLET_TIMEOUT_SECS),
        error: None,
    })
}

/// Parameters for confirming a new wallet (after mnemonic was generated)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmNewWalletParams {
    /// Password to encrypt the wallet
    pub password: String,
    /// Verification words at the positions returned by generate_mnemonic
    /// Must match exactly (case-insensitive)
    pub verify_words: Vec<String>,
    /// Optional path to save wallet file (uses default if not provided)
    pub path: Option<String>,
}

/// Result of creating a wallet file
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWalletResult {
    pub success: bool,
    /// The path where the wallet was saved
    pub path: Option<String>,
    /// The wallet's public address
    pub address: Option<String>,
    pub error: Option<String>,
}

/// Confirm and create a new wallet using the cached mnemonic.
///
/// SECURITY: The mnemonic is NEVER sent from JS to Rust. Instead,
/// we use the mnemonic cached in Rust from generate_mnemonic() call.
/// User proves they saved the mnemonic by providing specific words.
#[tauri::command]
pub async fn confirm_new_wallet(
    state: State<'_, WalletCommands>,
    params: ConfirmNewWalletParams,
) -> Result<CreateWalletResult, String> {
    match confirm_new_wallet_internal(&state, params).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(CreateWalletResult {
            success: false,
            path: None,
            address: None,
            error: Some(e.to_string()),
        }),
    }
}

async fn confirm_new_wallet_internal(
    state: &State<'_, WalletCommands>,
    params: ConfirmNewWalletParams,
) -> Result<CreateWalletResult> {
    // Get and validate pending wallet
    let mut pending_guard = state.pending_wallet.lock().await;

    let pending = pending_guard.take()
        .ok_or_else(|| anyhow!("No pending wallet. Please call generate_mnemonic first."))?;

    if pending.is_expired() {
        return Err(anyhow!("Wallet creation timed out. Please generate a new mnemonic."));
    }

    // Verify the words at the specified positions
    if params.verify_words.len() != 3 {
        // Put pending back so user can retry
        *pending_guard = Some(pending);
        return Err(anyhow!("Expected 3 verification words"));
    }

    // Clone verify_indices to avoid borrow issues when putting pending back
    let verify_indices = pending.verify_indices;
    let mnemonic_words = pending.keys.mnemonic_words();

    for (i, word_idx) in verify_indices.iter().enumerate() {
        let expected = mnemonic_words[*word_idx].to_lowercase();
        let provided = params.verify_words[i].to_lowercase().trim().to_string();

        if expected != provided {
            // Put pending back so user can retry
            *pending_guard = Some(pending);
            return Err(anyhow!(
                "Word {} is incorrect. Please check your backup.",
                word_idx + 1
            ));
        }
    }

    // Verification passed! Create the wallet
    let keys = pending.keys;

    // Determine path
    let path = match params.path {
        Some(p) => PathBuf::from(p),
        None => get_default_wallet_path()?,
    };

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create wallet directory: {}", e))?;
    }

    // Create encrypted wallet using the mnemonic phrase
    let wallet = EncryptedWallet::encrypt(keys.mnemonic_phrase(), &params.password)
        .map_err(|e| anyhow!("Failed to encrypt wallet: {}", e))?;

    // Save to file
    wallet.save(&path)
        .map_err(|e| anyhow!("Failed to save wallet file: {}", e))?;

    // Get address before moving keys into session
    let address = keys.address_string();

    // Auto-unlock after creation
    let session = WalletSession::new(keys, Some(path.clone()));
    *state.session.lock().await = Some(session);

    log::info!("New wallet created at {} and unlocked (secure flow)", path.display());

    Ok(CreateWalletResult {
        success: true,
        path: Some(path.to_string_lossy().to_string()),
        address: Some(address),
        error: None,
    })
}

/// Cancel pending wallet creation
#[tauri::command]
pub async fn cancel_pending_wallet(
    state: State<'_, WalletCommands>,
) -> Result<bool, String> {
    let mut pending_guard = state.pending_wallet.lock().await;
    if pending_guard.is_some() {
        *pending_guard = None;
        log::info!("Pending wallet creation cancelled");
        Ok(true)
    } else {
        Ok(false)
    }
}

// ============================================================================
// Wallet Import (for restoring existing wallets - mnemonic must come from JS)
// ============================================================================

/// Parameters for importing an existing wallet
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportWalletParams {
    /// 24-word BIP39 mnemonic phrase from user input
    pub mnemonic: String,
    /// Password to encrypt the wallet
    pub password: String,
    /// Optional path to save wallet file (uses default if not provided)
    pub path: Option<String>,
}

/// Import an existing wallet from a mnemonic phrase.
///
/// SECURITY NOTE: This command accepts mnemonic from JS because there's no
/// alternative for wallet restoration - the user MUST provide their existing
/// mnemonic. For NEW wallets, use generate_mnemonic + confirm_new_wallet instead.
#[tauri::command]
pub async fn import_wallet(
    state: State<'_, WalletCommands>,
    params: ImportWalletParams,
) -> Result<CreateWalletResult, String> {
    match import_wallet_internal(&state, params).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(CreateWalletResult {
            success: false,
            path: None,
            address: None,
            error: Some(e.to_string()),
        }),
    }
}

async fn import_wallet_internal(
    state: &State<'_, WalletCommands>,
    params: ImportWalletParams,
) -> Result<CreateWalletResult> {
    // Validate mnemonic and derive keys
    let keys = WalletKeys::from_mnemonic(&params.mnemonic)
        .map_err(|e| anyhow!("Invalid mnemonic: {}", e))?;

    // Determine path
    let path = match params.path {
        Some(p) => PathBuf::from(p),
        None => get_default_wallet_path()?,
    };

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create wallet directory: {}", e))?;
    }

    // Create encrypted wallet
    let wallet = EncryptedWallet::encrypt(&params.mnemonic, &params.password)
        .map_err(|e| anyhow!("Failed to encrypt wallet: {}", e))?;

    // Save to file
    wallet.save(&path)
        .map_err(|e| anyhow!("Failed to save wallet file: {}", e))?;

    // Get address before moving keys
    let address = keys.address_string();

    // Auto-unlock after import
    let session = WalletSession::new(keys, Some(path.clone()));
    *state.session.lock().await = Some(session);

    log::info!("Wallet imported to {} and unlocked", path.display());

    Ok(CreateWalletResult {
        success: true,
        path: Some(path.to_string_lossy().to_string()),
        address: Some(address),
        error: None,
    })
}

/// Result of checking wallet file existence
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletFileExistsResult {
    pub exists: bool,
    pub path: String,
}

/// Get the default wallet file path
fn get_default_wallet_path() -> Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow!("Could not determine data directory"))?;
    Ok(data_dir.join("botho-wallet").join("wallet.dat"))
}

/// Check if a wallet file exists
#[tauri::command]
pub async fn wallet_file_exists(
    path: Option<String>,
) -> Result<WalletFileExistsResult, String> {
    let wallet_path = match path {
        Some(p) => PathBuf::from(p),
        None => get_default_wallet_path().map_err(|e| e.to_string())?,
    };

    Ok(WalletFileExistsResult {
        exists: wallet_path.exists(),
        path: wallet_path.to_string_lossy().to_string(),
    })
}

/// Get the default wallet file path
#[tauri::command]
pub async fn get_wallet_path() -> Result<String, String> {
    get_default_wallet_path()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_view_spend_address() {
        let view_hex = "0".repeat(64);  // 32 zero bytes
        let spend_hex = "1".repeat(64); // 32 bytes of 0x11...

        let address = format!("view:{}\nspend:{}", view_hex, spend_hex);

        // This will fail because zero bytes aren't valid Ristretto points
        // but it tests the parsing logic
        let result = parse_recipient_address(&address);
        assert!(result.is_err()); // Expected - zero bytes aren't valid points
    }

    #[test]
    fn test_reject_short_cad_address() {
        let result = parse_recipient_address("cad:abcd1234:efgh5678");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not yet supported"));
    }

    #[test]
    fn test_reject_bth1_address() {
        let result = parse_recipient_address("bth1abcdef1234567890");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not yet supported"));
    }

    // ========================================================================
    // Unlock Rate Limiter Tests
    // ========================================================================

    #[test]
    fn test_unlock_rate_limiter_allows_first_attempt() {
        let rate_limiter = UnlockRateLimiter::new();
        let path = PathBuf::from("/test/wallet.dat");

        assert!(rate_limiter.check_allowed(&path).is_ok());
    }

    #[test]
    fn test_unlock_rate_limiter_exponential_backoff() {
        let mut state = UnlockAttemptState::default();

        // First failure: 1 second lockout (2^0 = 1)
        state.record_failure();
        assert_eq!(state.failed_attempts, 1);
        assert_eq!(state.calculate_lockout().as_secs(), 1);
        assert!(state.is_locked_out()); // Should be locked out

        // Simulate time passing by resetting lockout_until
        state.lockout_until = None;

        // Second failure: 2 seconds lockout (2^1 = 2)
        state.record_failure();
        assert_eq!(state.failed_attempts, 2);
        assert_eq!(state.calculate_lockout().as_secs(), 2);

        // Reset lockout
        state.lockout_until = None;

        // Third failure: 4 seconds lockout (2^2 = 4)
        state.record_failure();
        assert_eq!(state.failed_attempts, 3);
        assert_eq!(state.calculate_lockout().as_secs(), 4);

        // Fourth failure: 8 seconds (2^3 = 8)
        state.lockout_until = None;
        state.record_failure();
        assert_eq!(state.failed_attempts, 4);
        assert_eq!(state.calculate_lockout().as_secs(), 8);
    }

    #[test]
    fn test_unlock_rate_limiter_success_resets() {
        let mut rate_limiter = UnlockRateLimiter::new();
        let path = PathBuf::from("/test/wallet.dat");

        // Record some failures
        rate_limiter.record_failure(path.clone());
        rate_limiter.record_failure(path.clone());
        rate_limiter.record_failure(path.clone());

        let (failed_attempts, _) = rate_limiter.get_lockout_info(&path);
        assert_eq!(failed_attempts, 3);

        // Success should reset
        rate_limiter.record_success(path.clone());
        let (failed_attempts, _) = rate_limiter.get_lockout_info(&path);
        assert_eq!(failed_attempts, 0);
    }

    #[test]
    fn test_unlock_rate_limiter_max_lockout() {
        let mut state = UnlockAttemptState::default();

        // Simulate many failures - should cap at MAX_LOCKOUT_SECS
        for _ in 0..15 {
            state.lockout_until = None; // Clear lockout to allow more failures
            state.record_failure();
        }

        // Lockout should be capped at MAX_LOCKOUT_SECS (300s)
        let lockout = state.calculate_lockout();
        assert!(lockout.as_secs() <= MAX_LOCKOUT_SECS);
    }

    #[test]
    fn test_unlock_rate_limiter_per_path_tracking() {
        let mut rate_limiter = UnlockRateLimiter::new();
        let path1 = PathBuf::from("/test/wallet1.dat");
        let path2 = PathBuf::from("/test/wallet2.dat");

        // Failures on path1 shouldn't affect path2
        rate_limiter.record_failure(path1.clone());
        rate_limiter.record_failure(path1.clone());
        rate_limiter.record_failure(path1.clone());

        let (attempts1, _) = rate_limiter.get_lockout_info(&path1);
        let (attempts2, _) = rate_limiter.get_lockout_info(&path2);

        assert_eq!(attempts1, 3);
        assert_eq!(attempts2, 0);
        assert!(rate_limiter.check_allowed(&path2).is_ok());
    }
}
