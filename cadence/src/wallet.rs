use anyhow::Result;
use bip39::{Language, Mnemonic};
use mc_account_keys::{AccountKey, PublicAddress};
use mc_core::slip10::Slip10KeyGenerator;

/// Wallet manages a single account derived from a BIP39 mnemonic
pub struct Wallet {
    account_key: AccountKey,
}

impl Wallet {
    /// Create a wallet from a mnemonic phrase
    pub fn from_mnemonic(mnemonic_phrase: &str) -> Result<Self> {
        let mnemonic = Mnemonic::from_phrase(mnemonic_phrase, Language::English)
            .map_err(|e| anyhow::anyhow!("Invalid mnemonic: {}", e))?;

        let slip10_key = mnemonic.derive_slip10_key(0);
        let account_key = AccountKey::from(slip10_key);

        Ok(Self { account_key })
    }

    /// Get the default public address for receiving funds
    pub fn default_address(&self) -> PublicAddress {
        self.account_key.default_subaddress()
    }

    /// Get the account key (needed for transaction signing)
    pub fn account_key(&self) -> &AccountKey {
        &self.account_key
    }

    /// Format the public address as a string for display
    pub fn address_string(&self) -> String {
        let addr = self.default_address();
        // Use hex encoding of the view and spend public keys
        format!(
            "view:{}\nspend:{}",
            hex::encode(addr.view_public_key().to_bytes()),
            hex::encode(addr.spend_public_key().to_bytes())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_from_mnemonic() {
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        let wallet = Wallet::from_mnemonic(mnemonic).unwrap();
        let addr = wallet.default_address();
        // Just verify we get a valid address
        assert!(!addr.view_public_key().to_bytes().is_empty());
    }
}
