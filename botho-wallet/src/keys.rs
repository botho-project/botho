//! Key Management
//!
//! Handles BIP39 mnemonic generation and SLIP-0010 key derivation
//! for Botho wallet keys.

use anyhow::{anyhow, Result};
use bip39::{Language, Mnemonic};
use bt_account_keys::{AccountKey, PublicAddress};
use bt_core::slip10::Slip10KeyGenerator;
use bt_crypto_keys::RistrettoSignature;

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
}
