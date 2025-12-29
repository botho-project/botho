//! Key Management
//!
//! Handles BIP39 mnemonic generation and SLIP-0010 key derivation
//! for Botho wallet keys.
//!
//! When the `pq` feature is enabled, this module also provides quantum-safe
//! keys derived from the same mnemonic, using ML-KEM-768 and ML-DSA-65.

use anyhow::{anyhow, Result};
use bip39::{Language, Mnemonic};
use bth_account_keys::{AccountKey, PublicAddress};
use bth_core::slip10::Slip10KeyGenerator;
use bth_crypto_keys::RistrettoSignature;

#[cfg(feature = "pq")]
use bth_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};

/// Number of words in the mnemonic phrase
const MNEMONIC_WORDS: usize = 24;

/// Wallet keys derived from a BIP39 mnemonic
#[derive(Clone)]
pub struct WalletKeys {
    /// The mnemonic phrase (string, since Mnemonic doesn't implement Clone)
    mnemonic_phrase: String,

    /// The derived account key
    account_key: AccountKey,
}

impl WalletKeys {
    /// Generate a new wallet with a random mnemonic
    pub fn generate() -> Result<Self> {
        // Generate 256 bits of entropy for a 24-word mnemonic
        let mnemonic = Mnemonic::new(bip39::MnemonicType::Words24, Language::English);
        Self::from_mnemonic_internal(mnemonic)
    }

    /// Restore a wallet from a mnemonic phrase
    pub fn from_mnemonic(phrase: &str) -> Result<Self> {
        let mnemonic = Mnemonic::from_phrase(phrase, Language::English)
            .map_err(|e| anyhow!("Invalid mnemonic phrase: {}", e))?;

        // Validate word count
        let word_count = phrase.split_whitespace().count();
        if word_count != MNEMONIC_WORDS {
            return Err(anyhow!(
                "Expected {} word mnemonic, got {} words",
                MNEMONIC_WORDS,
                word_count
            ));
        }

        Self::from_mnemonic_internal(mnemonic)
    }

    /// Internal constructor from validated mnemonic
    fn from_mnemonic_internal(mnemonic: Mnemonic) -> Result<Self> {
        let phrase = mnemonic.phrase().to_string();

        // Derive keys using SLIP-0010 (account index 0)
        let slip10_key = mnemonic.derive_slip10_key(0);
        let account_key = AccountKey::from(slip10_key);

        Ok(Self {
            mnemonic_phrase: phrase,
            account_key,
        })
    }

    /// Get the mnemonic phrase as a string
    pub fn mnemonic_phrase(&self) -> &str {
        &self.mnemonic_phrase
    }

    /// Get the mnemonic words as a vector
    pub fn mnemonic_words(&self) -> Vec<&str> {
        self.mnemonic_phrase.split_whitespace().collect()
    }

    /// Get the public address for receiving funds
    pub fn public_address(&self) -> PublicAddress {
        self.account_key.default_subaddress()
    }

    /// Get the account key (for transaction signing)
    pub fn account_key(&self) -> &AccountKey {
        &self.account_key
    }

    /// Get the view public key bytes
    pub fn view_public_key_bytes(&self) -> [u8; 32] {
        self.public_address().view_public_key().to_bytes()
    }

    /// Get the spend public key bytes
    pub fn spend_public_key_bytes(&self) -> [u8; 32] {
        self.public_address().spend_public_key().to_bytes()
    }

    /// Format the address as a human-readable string
    pub fn address_string(&self) -> String {
        let addr = self.public_address();
        format!(
            "cad:{}:{}",
            hex::encode(&addr.view_public_key().to_bytes()[..16]),
            hex::encode(&addr.spend_public_key().to_bytes()[..16])
        )
    }

    /// Sign a message with the spend private key
    pub fn sign(&self, context: &[u8], message: &[u8]) -> Vec<u8> {
        let spend_private = self.account_key.default_subaddress_spend_private();
        let signature: RistrettoSignature = spend_private.sign_schnorrkel(context, message);
        let sig_bytes: &[u8] = signature.as_ref();
        sig_bytes.to_vec()
    }

    /// Check if a transaction output belongs to this wallet
    ///
    /// Compares the output's spend key against our spend public key.
    pub fn owns_output(&self, spend_key_bytes: &[u8; 32]) -> bool {
        &self.spend_public_key_bytes() == spend_key_bytes
    }

    // ===== Post-Quantum Key Methods (pq feature) =====

    /// Get the quantum-safe account key
    ///
    /// This derives post-quantum keys (ML-KEM-768, ML-DSA-65) from the same
    /// mnemonic. No additional backup is required - the mnemonic fully
    /// determines both classical and quantum-safe keys.
    #[cfg(feature = "pq")]
    pub fn pq_account_key(&self) -> QuantumSafeAccountKey {
        QuantumSafeAccountKey::from_mnemonic(&self.mnemonic_phrase)
    }

    /// Get the quantum-safe public address for receiving funds
    ///
    /// This address includes both classical and post-quantum public keys,
    /// allowing senders to create quantum-resistant outputs.
    #[cfg(feature = "pq")]
    pub fn pq_public_address(&self) -> QuantumSafePublicAddress {
        self.pq_account_key().default_subaddress()
    }

    /// Get the quantum-safe address as a string
    ///
    /// Format: `botho-pq://1/<base58(view||spend||pq_kem||pq_sig)>`
    ///
    /// Note: This address is ~4.3KB when base58-encoded due to the size
    /// of post-quantum public keys (ML-KEM-768: 1184 bytes, ML-DSA-65: 1952 bytes).
    #[cfg(feature = "pq")]
    pub fn pq_address_string(&self) -> String {
        self.pq_public_address().to_address_string()
    }
}

/// Validate a mnemonic phrase without creating keys
pub fn validate_mnemonic(phrase: &str) -> Result<()> {
    let word_count = phrase.split_whitespace().count();
    if word_count != MNEMONIC_WORDS {
        return Err(anyhow!(
            "Expected {} words, got {}",
            MNEMONIC_WORDS,
            word_count
        ));
    }

    Mnemonic::from_phrase(phrase, Language::English)
        .map_err(|e| anyhow!("Invalid mnemonic: {}", e))?;

    Ok(())
}

/// Check if a word is a valid BIP39 word
pub fn is_valid_word(word: &str) -> bool {
    // A word is valid if it's in the English wordlist
    // We can check by trying to parse a mnemonic containing just that word repeated
    // This is a simple approximation - for full validation, use validate_mnemonic
    !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase())
}

/// Suggest completions for a partial word (simplified - returns empty for now)
pub fn suggest_completions(_partial: &str) -> Vec<&'static str> {
    // For simplicity, we don't implement autocomplete
    // A full implementation would need access to the BIP39 wordlist
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard BIP39 test vector (24 words)
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn test_generate_wallet() {
        let keys = WalletKeys::generate().unwrap();

        // Should generate 24 words
        assert_eq!(keys.mnemonic_words().len(), 24);

        // Should produce valid address
        let addr = keys.public_address();
        assert!(!addr.view_public_key().to_bytes().iter().all(|b| *b == 0));
    }

    #[test]
    fn test_restore_wallet() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        assert_eq!(keys.mnemonic_phrase(), TEST_MNEMONIC);
        assert_eq!(keys.mnemonic_words().len(), 24);
    }

    #[test]
    fn test_deterministic_derivation() {
        // Same mnemonic should produce same keys
        let keys1 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let keys2 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        assert_eq!(
            keys1.view_public_key_bytes(),
            keys2.view_public_key_bytes()
        );
        assert_eq!(
            keys1.spend_public_key_bytes(),
            keys2.spend_public_key_bytes()
        );
    }

    #[test]
    fn test_invalid_mnemonic() {
        // Wrong word count
        assert!(WalletKeys::from_mnemonic("abandon abandon abandon").is_err());

        // Invalid word
        assert!(WalletKeys::from_mnemonic("invalid word mnemonic here").is_err());
    }

    #[test]
    fn test_validate_mnemonic() {
        assert!(validate_mnemonic(TEST_MNEMONIC).is_ok());
        assert!(validate_mnemonic("abandon").is_err());
        assert!(validate_mnemonic("invalid words here").is_err());
    }

    #[test]
    fn test_is_valid_word() {
        // Our simplified check just validates it's lowercase ascii
        assert!(is_valid_word("abandon"));
        assert!(is_valid_word("zoo"));
        assert!(is_valid_word("test")); // any lowercase word passes
        assert!(!is_valid_word("")); // empty fails
        assert!(!is_valid_word("ABC")); // uppercase fails
    }

    #[test]
    fn test_suggest_completions() {
        // Our simplified implementation returns empty
        let suggestions = suggest_completions("ab");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_sign() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let signature = keys.sign(b"test-context", b"test-message");

        // Schnorrkel signatures are 64 bytes
        assert_eq!(signature.len(), 64);
    }

    #[cfg(feature = "pq")]
    mod pq_tests {
        use super::*;

        #[test]
        fn test_pq_account_key() {
            let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

            // Should be able to derive PQ keys
            let pq_account = keys.pq_account_key();

            // Verify we can access the keypairs
            assert_eq!(
                pq_account.pq_kem_keypair().public_key().as_bytes().len(),
                1184 // ML-KEM-768 public key size
            );
            assert_eq!(
                pq_account.pq_sig_keypair().public_key().as_bytes().len(),
                1952 // ML-DSA-65 public key size
            );
        }

        #[test]
        fn test_pq_keys_deterministic() {
            // Same mnemonic should produce identical PQ keys
            let keys1 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
            let keys2 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

            let pq1 = keys1.pq_account_key();
            let pq2 = keys2.pq_account_key();

            // KEM keys should be identical
            assert_eq!(
                pq1.pq_kem_keypair().public_key().as_bytes(),
                pq2.pq_kem_keypair().public_key().as_bytes(),
                "ML-KEM public keys should be deterministic from mnemonic"
            );

            // Signature keys should be identical
            assert_eq!(
                pq1.pq_sig_keypair().public_key().as_bytes(),
                pq2.pq_sig_keypair().public_key().as_bytes(),
                "ML-DSA public keys should be deterministic from mnemonic"
            );
        }

        #[test]
        fn test_pq_address_string_format() {
            let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
            let pq_addr = keys.pq_address_string();

            // Should have the correct prefix
            assert!(pq_addr.starts_with("botho-pq://1/"));

            // Should be a valid base58-encoded address
            assert!(pq_addr.len() > 100); // PQ addresses are large
        }

        #[test]
        fn test_pq_address_roundtrip() {
            use bth_account_keys::QuantumSafePublicAddress;

            let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
            let pq_addr = keys.pq_address_string();

            // Should be able to parse the address back
            let parsed =
                QuantumSafePublicAddress::from_address_string(&pq_addr).expect("should parse");
            let reparsed = parsed.to_address_string();
            assert_eq!(pq_addr, reparsed);
        }
    }
}
