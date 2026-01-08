//! Encrypted Wallet Storage
//!
//! Securely stores the wallet mnemonic using:
//! - Argon2id for password-based key derivation
//! - ChaCha20-Poly1305 for authenticated encryption

use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use zeroize::{Zeroize, Zeroizing};

use crate::{discovery::NodeDiscovery, fee_estimation::PendingChangeTags};

/// Current wallet file format version
const WALLET_VERSION: u32 = 1;

/// Argon2 parameters (tuned for security vs. usability)
const ARGON2_MEMORY_KB: u32 = 65536; // 64 MB
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 4;

/// Maximum failed attempts before lockout
const MAX_FAILED_ATTEMPTS: u32 = 5;

/// Base delay in milliseconds for exponential backoff
const BASE_DELAY_MS: u64 = 1000;

/// Maximum delay in milliseconds (5 minutes)
const MAX_DELAY_MS: u64 = 300_000;

/// Rate limiter for decryption attempts.
///
/// Implements exponential backoff to prevent brute-force password attacks.
/// After MAX_FAILED_ATTEMPTS consecutive failures, a cooldown period is
/// enforced.
///
/// This struct can be persisted to disk to maintain rate limiting across CLI
/// invocations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecryptionRateLimiter {
    /// Number of consecutive failed attempts
    consecutive_failures: u32,
    /// Timestamp of last failed attempt (Unix epoch milliseconds)
    last_failure_time: Option<u64>,
}

impl Default for DecryptionRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl DecryptionRateLimiter {
    /// Create a new rate limiter.
    pub fn new() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure_time: None,
        }
    }

    /// Get the current number of consecutive failures.
    pub fn failure_count(&self) -> u32 {
        self.consecutive_failures
    }

    /// Check if we're currently in a cooldown period.
    /// Returns Ok(()) if allowed to attempt, Err with remaining wait time
    /// otherwise.
    pub fn check_rate_limit(&self) -> Result<()> {
        if self.consecutive_failures == 0 {
            return Ok(());
        }

        let Some(last_failure) = self.last_failure_time else {
            return Ok(());
        };

        let now = Self::current_time_ms();
        let required_delay = self.calculate_delay();
        let elapsed = now.saturating_sub(last_failure);

        if elapsed < required_delay {
            let remaining_secs = (required_delay - elapsed) / 1000;
            return Err(anyhow!(
                "Rate limited: please wait {} seconds before retrying ({} failed attempts)",
                remaining_secs + 1,
                self.consecutive_failures
            ));
        }

        Ok(())
    }

    /// Calculate the delay required based on consecutive failures.
    /// Uses exponential backoff: delay = base * 2^(failures - 1), capped at
    /// MAX_DELAY_MS
    fn calculate_delay(&self) -> u64 {
        if self.consecutive_failures == 0 {
            return 0;
        }

        let exponent = self.consecutive_failures.saturating_sub(1).min(10);
        let delay = BASE_DELAY_MS.saturating_mul(1u64 << exponent);
        delay.min(MAX_DELAY_MS)
    }

    /// Record a failed decryption attempt.
    pub fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_failure_time = Some(Self::current_time_ms());
    }

    /// Record a successful decryption (resets the limiter).
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure_time = None;
    }

    /// Check if the account is locked out (exceeded max attempts).
    pub fn is_locked_out(&self) -> bool {
        self.consecutive_failures >= MAX_FAILED_ATTEMPTS
    }

    /// Get the current time in milliseconds since Unix epoch.
    fn current_time_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Get human-readable remaining lockout time.
    pub fn remaining_lockout_time(&self) -> Option<String> {
        let Some(last_failure) = self.last_failure_time else {
            return None;
        };

        let now = Self::current_time_ms();
        let required_delay = self.calculate_delay();
        let elapsed = now.saturating_sub(last_failure);

        if elapsed >= required_delay {
            return None;
        }

        let remaining_ms = required_delay - elapsed;
        let remaining_secs = remaining_ms / 1000;

        if remaining_secs >= 60 {
            Some(format!("{} minutes", remaining_secs / 60 + 1))
        } else {
            Some(format!("{} seconds", remaining_secs + 1))
        }
    }

    /// Get the default path for rate limiter state file.
    ///
    /// The rate limiter state is stored alongside the wallet file.
    /// For a wallet at `~/.botho-wallet/wallet.dat`, the rate limiter
    /// state is stored at `~/.botho-wallet/rate_limiter.json`.
    pub fn default_path(wallet_path: &Path) -> std::path::PathBuf {
        wallet_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("rate_limiter.json")
    }

    /// Save the rate limiter state to a file.
    ///
    /// The state is persisted as JSON to maintain rate limiting across
    /// CLI invocations, preventing attackers from bypassing delays by
    /// restarting the CLI.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Load rate limiter state from a file.
    ///
    /// Returns a new (empty) rate limiter if the file doesn't exist
    /// or can't be parsed. This ensures graceful degradation.
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::new();
        }

        match fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|_| Self::new()),
            Err(_) => Self::new(),
        }
    }

    /// Load rate limiter state, or create new if not found.
    /// This is a convenience method that uses the default path.
    pub fn load_for_wallet(wallet_path: &Path) -> Self {
        Self::load(&Self::default_path(wallet_path))
    }

    /// Save rate limiter state using the default path.
    pub fn save_for_wallet(&self, wallet_path: &Path) -> Result<()> {
        self.save(&Self::default_path(wallet_path))
    }
}

/// Encrypted wallet file structure
#[derive(Serialize, Deserialize)]
pub struct EncryptedWallet {
    /// File format version
    version: u32,

    /// Argon2 salt (32 bytes, base64 encoded)
    salt: String,

    /// ChaCha20-Poly1305 nonce (12 bytes, hex encoded)
    nonce: String,

    /// Encrypted mnemonic (hex encoded)
    ciphertext: String,

    /// Optional encrypted discovery state
    discovery_state: Option<String>,

    /// Optional encrypted pending change tags for cluster tag propagation.
    /// When a transaction is built, blended input tags are stored here so
    /// they can be applied to change outputs discovered during sync.
    #[serde(default)]
    pending_change_tags: Option<String>,

    /// Last sync height
    pub sync_height: u64,

    /// Network identifier
    pub network: String,
}

impl EncryptedWallet {
    /// Create a new encrypted wallet from a mnemonic phrase
    pub fn encrypt(mnemonic: &str, password: &str) -> Result<Self> {
        // Generate random salt for Argon2
        let salt = SaltString::generate(&mut OsRng);

        // Derive encryption key from password
        let key = derive_key(password, salt.as_str())?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill(&mut nonce_bytes);

        // Encrypt the mnemonic
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow!("Failed to create cipher"))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, mnemonic.as_bytes())
            .map_err(|_| anyhow!("Encryption failed"))?;

        Ok(Self {
            version: WALLET_VERSION,
            salt: salt.to_string(),
            nonce: hex::encode(nonce_bytes),
            ciphertext: hex::encode(ciphertext),
            discovery_state: None,
            pending_change_tags: None,
            sync_height: 0,
            network: "botho-mainnet".to_string(),
        })
    }

    /// Decrypt the wallet to retrieve the mnemonic.
    ///
    /// Security: Returns `Zeroizing<String>` which automatically overwrites
    /// the memory with zeros when dropped, preventing the mnemonic from
    /// persisting in memory after use.
    pub fn decrypt(&self, password: &str) -> Result<Zeroizing<String>> {
        // Check version
        if self.version != WALLET_VERSION {
            return Err(anyhow!(
                "Unsupported wallet version: {} (expected {})",
                self.version,
                WALLET_VERSION
            ));
        }

        // Derive key from password
        let key = derive_key(password, &self.salt)?;

        // Decode nonce and ciphertext
        let nonce_bytes = hex::decode(&self.nonce).map_err(|_| anyhow!("Invalid nonce format"))?;
        let ciphertext =
            hex::decode(&self.ciphertext).map_err(|_| anyhow!("Invalid ciphertext format"))?;

        if nonce_bytes.len() != 12 {
            return Err(anyhow!("Invalid nonce length"));
        }

        // Decrypt
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow!("Failed to create cipher"))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| anyhow!("Decryption failed - wrong password?"))?;

        // Wrap in Zeroizing immediately for secure cleanup
        let mnemonic =
            String::from_utf8(plaintext).map_err(|_| anyhow!("Invalid mnemonic encoding"))?;
        Ok(Zeroizing::new(mnemonic))
    }

    /// Decrypt the wallet with rate limiting protection.
    ///
    /// This method enforces exponential backoff on failed attempts to protect
    /// against brute-force password attacks. The rate limiter state is updated
    /// based on success or failure.
    ///
    /// # Arguments
    /// * `password` - The password to decrypt the wallet
    /// * `rate_limiter` - A mutable reference to the rate limiter state
    ///
    /// # Returns
    /// * `Ok(Zeroizing<String>)` - The decrypted mnemonic on success
    ///   (auto-zeroized on drop)
    /// * `Err` - Rate limit exceeded, or decryption failed
    pub fn decrypt_with_rate_limit(
        &self,
        password: &str,
        rate_limiter: &mut DecryptionRateLimiter,
    ) -> Result<Zeroizing<String>> {
        // Check if we're rate limited
        rate_limiter.check_rate_limit()?;

        // Check for lockout
        if rate_limiter.is_locked_out() {
            if let Some(remaining) = rate_limiter.remaining_lockout_time() {
                return Err(anyhow!(
                    "Account temporarily locked due to too many failed attempts. Try again in {}",
                    remaining
                ));
            }
        }

        // Attempt decryption
        match self.decrypt(password) {
            Ok(mnemonic) => {
                rate_limiter.record_success();
                Ok(mnemonic)
            }
            Err(e) => {
                rate_limiter.record_failure();

                // Provide helpful error message based on failure count
                let failures = rate_limiter.failure_count();
                if failures >= MAX_FAILED_ATTEMPTS {
                    Err(anyhow!(
                        "Decryption failed. Account locked for {} due to {} failed attempts",
                        rate_limiter
                            .remaining_lockout_time()
                            .unwrap_or_else(|| "some time".to_string()),
                        failures
                    ))
                } else {
                    let remaining_attempts = MAX_FAILED_ATTEMPTS - failures;
                    Err(anyhow!(
                        "{}. {} attempt(s) remaining before temporary lockout",
                        e,
                        remaining_attempts
                    ))
                }
            }
        }
    }

    /// Save the wallet to a file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Serialize to JSON
        let json = serde_json::to_string_pretty(self)?;

        // Write with restricted permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            use std::io::Write;
            file.write_all(json.as_bytes())?;
        }

        #[cfg(windows)]
        {
            use std::io::Write;
            // Write file first
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            file.write_all(json.as_bytes())?;
            drop(file);

            // Set restrictive ACL (owner-only access)
            set_windows_owner_only_acl(path)?;
        }

        #[cfg(not(any(unix, windows)))]
        {
            // Fallback for other platforms - at least log a warning
            tracing::warn!("Unable to set restrictive file permissions on this platform");
            fs::write(path, json)?;
        }

        Ok(())
    }

    /// Load a wallet from a file
    pub fn load(path: &Path) -> Result<Self> {
        let json =
            fs::read_to_string(path).map_err(|e| anyhow!("Failed to read wallet file: {}", e))?;

        serde_json::from_str(&json).map_err(|e| anyhow!("Failed to parse wallet file: {}", e))
    }

    /// Check if a wallet file exists
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    /// Update sync height
    pub fn set_sync_height(&mut self, height: u64) {
        self.sync_height = height;
    }

    /// Store discovery state
    pub fn set_discovery_state(&mut self, discovery: &NodeDiscovery, password: &str) -> Result<()> {
        let state_bytes = discovery.to_bytes()?;

        // Re-derive key
        let key = derive_key(password, &self.salt)?;

        // Generate new nonce for discovery state
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill(&mut nonce_bytes);

        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow!("Failed to create cipher"))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, state_bytes.as_slice())
            .map_err(|_| anyhow!("Encryption failed"))?;

        // Store as nonce:ciphertext
        self.discovery_state = Some(format!(
            "{}:{}",
            hex::encode(nonce_bytes),
            hex::encode(ciphertext)
        ));

        Ok(())
    }

    /// Load discovery state
    pub fn get_discovery_state(&self, password: &str) -> Result<Option<NodeDiscovery>> {
        let state_str = match &self.discovery_state {
            Some(s) => s,
            None => return Ok(None),
        };

        // Parse nonce:ciphertext format
        let parts: Vec<&str> = state_str.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid discovery state format"));
        }

        let nonce_bytes = hex::decode(parts[0])?;
        let ciphertext = hex::decode(parts[1])?;

        // Derive key
        let key = derive_key(password, &self.salt)?;

        // Decrypt
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow!("Failed to create cipher"))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| anyhow!("Decryption failed"))?;

        Ok(Some(NodeDiscovery::from_bytes(&plaintext)?))
    }

    /// Change the wallet password
    pub fn change_password(&mut self, old_password: &str, new_password: &str) -> Result<()> {
        // Decrypt with old password
        let mnemonic = self.decrypt(old_password)?;

        // Re-encrypt with new password
        let new_wallet = Self::encrypt(&mnemonic, new_password)?;

        // Update fields
        self.salt = new_wallet.salt;
        self.nonce = new_wallet.nonce;
        self.ciphertext = new_wallet.ciphertext;

        // Re-encrypt discovery state if present
        if let Some(discovery) = self.get_discovery_state(old_password)? {
            self.set_discovery_state(&discovery, new_password)?;
        }

        // Re-encrypt pending change tags if present
        if let Some(tags) = self.get_pending_change_tags(old_password)? {
            self.set_pending_change_tags(&tags, new_password)?;
        }

        Ok(())
    }

    /// Store pending change tags for cluster tag propagation.
    ///
    /// Call this after building a transaction to store the blended input tags
    /// that should be applied to change outputs discovered during sync.
    pub fn set_pending_change_tags(
        &mut self,
        tags: &PendingChangeTags,
        password: &str,
    ) -> Result<()> {
        let state_bytes =
            serde_json::to_vec(tags).map_err(|e| anyhow!("Failed to serialize tags: {}", e))?;

        // Re-derive key
        let key = derive_key(password, &self.salt)?;

        // Generate new nonce
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill(&mut nonce_bytes);

        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow!("Failed to create cipher"))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, state_bytes.as_slice())
            .map_err(|_| anyhow!("Encryption failed"))?;

        // Store as nonce:ciphertext
        self.pending_change_tags = Some(format!(
            "{}:{}",
            hex::encode(nonce_bytes),
            hex::encode(ciphertext)
        ));

        Ok(())
    }

    /// Load pending change tags.
    ///
    /// Call this during sync to retrieve any pending tags that should be
    /// applied to discovered change outputs.
    pub fn get_pending_change_tags(&self, password: &str) -> Result<Option<PendingChangeTags>> {
        let state_str = match &self.pending_change_tags {
            Some(s) => s,
            None => return Ok(None),
        };

        // Parse nonce:ciphertext format
        let parts: Vec<&str> = state_str.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid pending change tags format"));
        }

        let nonce_bytes = hex::decode(parts[0])?;
        let ciphertext = hex::decode(parts[1])?;

        // Derive key
        let key = derive_key(password, &self.salt)?;

        // Decrypt
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow!("Failed to create cipher"))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| anyhow!("Decryption failed"))?;

        let tags: PendingChangeTags = serde_json::from_slice(&plaintext)
            .map_err(|e| anyhow!("Failed to deserialize pending tags: {}", e))?;

        Ok(Some(tags))
    }
}

/// Derive a 32-byte encryption key from password using Argon2id
fn derive_key(password: &str, salt: &str) -> Result<[u8; 32]> {
    let salt = SaltString::from_b64(salt).map_err(|_| anyhow!("Invalid salt format"))?;

    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(
            ARGON2_MEMORY_KB,
            ARGON2_ITERATIONS,
            ARGON2_PARALLELISM,
            Some(32),
        )
        .map_err(|_| anyhow!("Invalid Argon2 parameters"))?,
    );

    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| anyhow!("Key derivation failed"))?;

    let hash_output = hash.hash.ok_or_else(|| anyhow!("No hash output"))?;
    let hash_bytes = hash_output.as_bytes();

    let mut key = [0u8; 32];
    key.copy_from_slice(&hash_bytes[..32]);

    Ok(key)
}

/// Securely zero out sensitive data.
///
/// Uses the `zeroize` crate which provides guaranteed memory clearing
/// that won't be optimized away by the compiler.
pub fn secure_zero(data: &mut [u8]) {
    data.zeroize();
}

/// Set Windows ACL to owner-only access (equivalent to Unix 0600)
#[cfg(windows)]
fn set_windows_owner_only_acl(path: &Path) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE, PSID},
            Security::{
                Authorization::{
                    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_INHERITANCE,
                    SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_TYPE, TRUSTEE_W,
                },
                GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION, GENERIC_READ,
                GENERIC_WRITE, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
            },
            System::Memory::{LocalFree, HLOCAL},
        },
    };

    // Get current process token and user SID
    // SAFETY: GetCurrentProcess returns a pseudo-handle that doesn't require
    // closing. OpenProcessToken is called with valid parameters: the process
    // handle, TOKEN_QUERY permission, and a valid mutable pointer to receive
    // the token handle. The token handle is later closed with CloseHandle after
    // use.
    let token = unsafe {
        let mut token = HANDLE::default();
        let current_process = windows::Win32::System::Threading::GetCurrentProcess();
        windows::Win32::System::Threading::OpenProcessToken(
            current_process,
            TOKEN_QUERY,
            &mut token,
        )?;
        token
    };

    // Get token user info
    let mut token_info_len = 0u32;
    // SAFETY: First call to GetTokenInformation with null buffer to query required
    // size. The function is called with a valid token handle and writes the
    // required buffer size to token_info_len. The return value is ignored as
    // this call is expected to fail with ERROR_INSUFFICIENT_BUFFER.
    unsafe {
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut token_info_len);
    }

    let mut token_info = vec![0u8; token_info_len as usize];
    // SAFETY: Second call to GetTokenInformation with properly sized buffer.
    // - token is a valid handle obtained from OpenProcessToken above
    // - token_info buffer is allocated with the exact size returned by the first
    //   call
    // - token_info_len correctly reflects the buffer size
    // - CloseHandle is called on the token handle which was successfully opened
    // The token_info buffer will contain a valid TOKEN_USER structure after
    // success.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(token_info.as_mut_ptr() as *mut _),
            token_info_len,
            &mut token_info_len,
        )?;
        CloseHandle(token)?;
    }

    // SAFETY: The token_info buffer was filled by GetTokenInformation with
    // TokenUser class. The Windows API guarantees the buffer contains a valid
    // TOKEN_USER structure when the call succeeds. The buffer is properly
    // aligned (heap-allocated) and the reference is valid for the lifetime of
    // token_info.
    let token_user = unsafe { &*(token_info.as_ptr() as *const TOKEN_USER) };
    let user_sid = token_user.User.Sid;

    // Create EXPLICIT_ACCESS for owner with full control
    let mut ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: (GENERIC_READ | GENERIC_WRITE).0,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_TYPE(0), // TRUSTEE_IS_USER
            ptstrName: windows::core::PWSTR(user_sid.0 as *mut u16),
            ..Default::default()
        },
    };

    // Create ACL with the explicit access entry
    let mut new_acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: SetEntriesInAclW is called with:
    // - A valid slice containing one EXPLICIT_ACCESS_W entry
    // - None for the existing ACL (creating a new one)
    // - A valid mutable pointer to receive the new ACL
    // The resulting ACL pointer must be freed with LocalFree when done.
    unsafe {
        SetEntriesInAclW(Some(&[ea]), None, &mut new_acl)?;
    }

    // Convert path to wide string
    let path_wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // Set the new DACL on the file
    // SAFETY: SetNamedSecurityInfoW is called with:
    // - A valid null-terminated wide string path (path_wide)
    // - SE_FILE_OBJECT indicating this is a file path
    // - Security info flags to set DACL with protection
    // - Default (null) owner and group SIDs (not being modified)
    // - The ACL created by SetEntriesInAclW above
    // The path_wide Vec lives through this call, so the pointer remains valid.
    let result = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR::from_raw(path_wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            PSID::default(),
            PSID::default(),
            Some(new_acl),
            None,
        )
    };

    // Free the ACL
    // SAFETY: new_acl was allocated by SetEntriesInAclW and must be freed with
    // LocalFree. We check for null before freeing. LocalFree takes ownership of
    // the memory and invalidates the pointer - we don't use new_acl after this
    // point.
    if !new_acl.is_null() {
        unsafe {
            let _ = LocalFree(HLOCAL(new_acl as *mut _));
        }
    }

    if result.is_err() {
        tracing::warn!("Failed to set file ACL: {:?}", result);
        // Don't fail - file is still written, just with default permissions
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
    const TEST_PASSWORD: &str = "test-password-123";

    #[test]
    fn test_encrypt_decrypt() {
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        let decrypted = wallet.decrypt(TEST_PASSWORD).unwrap();
        assert_eq!(&*decrypted, TEST_MNEMONIC);
    }

    #[test]
    fn test_wrong_password() {
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        let result = wallet.decrypt("wrong-password");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        wallet.save(&wallet_path).unwrap();

        let loaded = EncryptedWallet::load(&wallet_path).unwrap();
        let decrypted = loaded.decrypt(TEST_PASSWORD).unwrap();
        assert_eq!(&*decrypted, TEST_MNEMONIC);
    }

    #[test]
    fn test_change_password() {
        let mut wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();

        let new_password = "new-password-456";
        wallet.change_password(TEST_PASSWORD, new_password).unwrap();

        // Old password should fail
        assert!(wallet.decrypt(TEST_PASSWORD).is_err());

        // New password should work
        let decrypted = wallet.decrypt(new_password).unwrap();
        assert_eq!(&*decrypted, TEST_MNEMONIC);
    }

    #[test]
    fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        assert!(!EncryptedWallet::exists(&wallet_path));

        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        wallet.save(&wallet_path).unwrap();

        assert!(EncryptedWallet::exists(&wallet_path));
    }

    #[test]
    fn test_sync_height() {
        let mut wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        assert_eq!(wallet.sync_height, 0);

        wallet.set_sync_height(12345);
        assert_eq!(wallet.sync_height, 12345);
    }

    #[test]
    fn test_rate_limiter_initial_state() {
        let limiter = DecryptionRateLimiter::new();
        assert_eq!(limiter.failure_count(), 0);
        assert!(!limiter.is_locked_out());
        assert!(limiter.check_rate_limit().is_ok());
    }

    #[test]
    fn test_rate_limiter_records_failures() {
        let mut limiter = DecryptionRateLimiter::new();

        limiter.record_failure();
        assert_eq!(limiter.failure_count(), 1);

        limiter.record_failure();
        assert_eq!(limiter.failure_count(), 2);
    }

    #[test]
    fn test_rate_limiter_resets_on_success() {
        let mut limiter = DecryptionRateLimiter::new();

        limiter.record_failure();
        limiter.record_failure();
        assert_eq!(limiter.failure_count(), 2);

        limiter.record_success();
        assert_eq!(limiter.failure_count(), 0);
        assert!(!limiter.is_locked_out());
    }

    #[test]
    fn test_rate_limiter_lockout_after_max_attempts() {
        let mut limiter = DecryptionRateLimiter::new();

        for _ in 0..MAX_FAILED_ATTEMPTS {
            limiter.record_failure();
        }

        assert!(limiter.is_locked_out());
        assert_eq!(limiter.failure_count(), MAX_FAILED_ATTEMPTS);
    }

    #[test]
    fn test_decrypt_with_rate_limit_success() {
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        let mut limiter = DecryptionRateLimiter::new();

        let result = wallet.decrypt_with_rate_limit(TEST_PASSWORD, &mut limiter);
        assert!(result.is_ok());
        assert_eq!(&*result.unwrap(), TEST_MNEMONIC);
        assert_eq!(limiter.failure_count(), 0);
    }

    #[test]
    fn test_decrypt_with_rate_limit_failure_increments() {
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        let mut limiter = DecryptionRateLimiter::new();

        let result = wallet.decrypt_with_rate_limit("wrong-password", &mut limiter);
        assert!(result.is_err());
        assert_eq!(limiter.failure_count(), 1);

        // Wait for rate limit delay (1 second for first failure)
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let result = wallet.decrypt_with_rate_limit("wrong-password", &mut limiter);
        assert!(result.is_err());
        assert_eq!(limiter.failure_count(), 2);
    }

    #[test]
    fn test_decrypt_with_rate_limit_success_after_failure() {
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        let mut limiter = DecryptionRateLimiter::new();

        // Fail once
        let _ = wallet.decrypt_with_rate_limit("wrong-password", &mut limiter);
        assert_eq!(limiter.failure_count(), 1);

        // Wait for rate limit (in test we can skip this by checking immediately
        // since the delay is 1 second for first failure)
        std::thread::sleep(std::time::Duration::from_millis(1100));

        // Succeed
        let result = wallet.decrypt_with_rate_limit(TEST_PASSWORD, &mut limiter);
        assert!(result.is_ok());
        assert_eq!(limiter.failure_count(), 0);
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        let mut limiter = DecryptionRateLimiter::new();

        // No delay with 0 failures
        assert_eq!(limiter.calculate_delay(), 0);

        // 1 second with 1 failure
        limiter.record_failure();
        assert_eq!(limiter.calculate_delay(), 1000);

        // 2 seconds with 2 failures
        limiter.record_failure();
        assert_eq!(limiter.calculate_delay(), 2000);

        // 4 seconds with 3 failures
        limiter.record_failure();
        assert_eq!(limiter.calculate_delay(), 4000);

        // 8 seconds with 4 failures
        limiter.record_failure();
        assert_eq!(limiter.calculate_delay(), 8000);

        // 16 seconds with 5 failures (lockout threshold)
        limiter.record_failure();
        assert_eq!(limiter.calculate_delay(), 16000);
    }

    #[test]
    fn test_rate_limiter_serialization() {
        let mut limiter = DecryptionRateLimiter::new();
        limiter.record_failure();
        limiter.record_failure();

        // Serialize to JSON
        let json = serde_json::to_string(&limiter).unwrap();
        assert!(json.contains("consecutive_failures"));
        assert!(json.contains("2"));

        // Deserialize back
        let restored: DecryptionRateLimiter = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.failure_count(), 2);
    }

    #[test]
    fn test_rate_limiter_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let limiter_path = temp_dir.path().join("rate_limiter.json");

        // Create limiter with some failures
        let mut limiter = DecryptionRateLimiter::new();
        limiter.record_failure();
        limiter.record_failure();
        limiter.record_failure();

        // Save
        limiter.save(&limiter_path).unwrap();
        assert!(limiter_path.exists());

        // Load
        let loaded = DecryptionRateLimiter::load(&limiter_path);
        assert_eq!(loaded.failure_count(), 3);
        assert!(loaded.last_failure_time.is_some());
    }

    #[test]
    fn test_rate_limiter_load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let limiter_path = temp_dir.path().join("nonexistent.json");

        // Loading nonexistent file returns fresh limiter
        let limiter = DecryptionRateLimiter::load(&limiter_path);
        assert_eq!(limiter.failure_count(), 0);
        assert!(!limiter.is_locked_out());
    }

    #[test]
    fn test_rate_limiter_load_invalid_json() {
        let temp_dir = TempDir::new().unwrap();
        let limiter_path = temp_dir.path().join("invalid.json");

        // Write invalid JSON
        fs::write(&limiter_path, "not valid json").unwrap();

        // Loading invalid JSON returns fresh limiter
        let limiter = DecryptionRateLimiter::load(&limiter_path);
        assert_eq!(limiter.failure_count(), 0);
    }

    #[test]
    fn test_rate_limiter_default_path() {
        let wallet_path = Path::new("/home/user/.botho-wallet/wallet.dat");
        let limiter_path = DecryptionRateLimiter::default_path(wallet_path);
        assert_eq!(
            limiter_path,
            Path::new("/home/user/.botho-wallet/rate_limiter.json")
        );
    }

    #[test]
    fn test_rate_limiter_for_wallet() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // Create limiter with failures
        let mut limiter = DecryptionRateLimiter::new();
        limiter.record_failure();
        limiter.record_failure();

        // Save using wallet path
        limiter.save_for_wallet(&wallet_path).unwrap();

        // Load using wallet path
        let loaded = DecryptionRateLimiter::load_for_wallet(&wallet_path);
        assert_eq!(loaded.failure_count(), 2);
    }

    #[test]
    fn test_rate_limiter_persistence_across_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // Session 1: Two failures
        {
            let mut limiter = DecryptionRateLimiter::load_for_wallet(&wallet_path);
            limiter.record_failure();
            limiter.record_failure();
            limiter.save_for_wallet(&wallet_path).unwrap();
        }

        // Session 2: One more failure (should have 3 total)
        {
            let mut limiter = DecryptionRateLimiter::load_for_wallet(&wallet_path);
            assert_eq!(limiter.failure_count(), 2); // Persisted from session 1
            limiter.record_failure();
            assert_eq!(limiter.failure_count(), 3);
            limiter.save_for_wallet(&wallet_path).unwrap();
        }

        // Session 3: Verify count
        {
            let limiter = DecryptionRateLimiter::load_for_wallet(&wallet_path);
            assert_eq!(limiter.failure_count(), 3);
        }
    }

    #[test]
    fn test_rate_limiter_success_resets_persisted_state() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // Session 1: Add failures
        {
            let mut limiter = DecryptionRateLimiter::load_for_wallet(&wallet_path);
            limiter.record_failure();
            limiter.record_failure();
            limiter.record_failure();
            limiter.save_for_wallet(&wallet_path).unwrap();
        }

        // Session 2: Success resets
        {
            let mut limiter = DecryptionRateLimiter::load_for_wallet(&wallet_path);
            assert_eq!(limiter.failure_count(), 3);
            limiter.record_success();
            limiter.save_for_wallet(&wallet_path).unwrap();
        }

        // Session 3: Verify reset
        {
            let limiter = DecryptionRateLimiter::load_for_wallet(&wallet_path);
            assert_eq!(limiter.failure_count(), 0);
            assert!(!limiter.is_locked_out());
        }
    }
}
