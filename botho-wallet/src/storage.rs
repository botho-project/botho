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
use std::fs;
use std::path::Path;
use zeroize::Zeroize;

use crate::discovery::NodeDiscovery;

/// Current wallet file format version
const WALLET_VERSION: u32 = 1;

/// Argon2 parameters (tuned for security vs. usability)
const ARGON2_MEMORY_KB: u32 = 65536; // 64 MB
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 4;

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
            sync_height: 0,
            network: "botho-mainnet".to_string(),
        })
    }

    /// Decrypt the wallet to retrieve the mnemonic
    pub fn decrypt(&self, password: &str) -> Result<String> {
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
        let nonce_bytes = hex::decode(&self.nonce)
            .map_err(|_| anyhow!("Invalid nonce format"))?;
        let ciphertext = hex::decode(&self.ciphertext)
            .map_err(|_| anyhow!("Invalid ciphertext format"))?;

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

        String::from_utf8(plaintext)
            .map_err(|_| anyhow!("Invalid mnemonic encoding"))
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
        let json = fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read wallet file: {}", e))?;

        serde_json::from_str(&json)
            .map_err(|e| anyhow!("Failed to parse wallet file: {}", e))
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

        Ok(())
    }
}

/// Derive a 32-byte encryption key from password using Argon2id
fn derive_key(password: &str, salt: &str) -> Result<[u8; 32]> {
    let salt = SaltString::from_b64(salt)
        .map_err(|_| anyhow!("Invalid salt format"))?;

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
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, PSID};
    use windows::Win32::Security::Authorization::{
        SetNamedSecurityInfoW, SE_FILE_OBJECT, EXPLICIT_ACCESS_W, SET_ACCESS,
        NO_INHERITANCE, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_W, TRUSTEE_TYPE,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        ACL, GENERIC_READ, GENERIC_WRITE,
    };
    use windows::Win32::System::Memory::{LocalFree, HLOCAL};
    use windows::core::PCWSTR;

    // Get current process token and user SID
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
    unsafe {
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut token_info_len);
    }

    let mut token_info = vec![0u8; token_info_len as usize];
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
    unsafe {
        SetEntriesInAclW(Some(&[ea]), None, &mut new_acl)?;
    }

    // Convert path to wide string
    let path_wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // Set the new DACL on the file
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
        assert_eq!(decrypted, TEST_MNEMONIC);
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
        assert_eq!(decrypted, TEST_MNEMONIC);
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
        assert_eq!(decrypted, TEST_MNEMONIC);
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
}
