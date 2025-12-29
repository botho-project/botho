use anyhow::Result;
use bip39::{Language, Mnemonic};
use mc_account_keys::{AccountKey, PublicAddress};
use mc_core::slip10::Slip10KeyGenerator;
use mc_crypto_keys::RistrettoSignature;

use crate::ledger::Ledger;
use crate::transaction::{Transaction, UtxoId};

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

    /// Sign all inputs of a transaction
    ///
    /// This method looks up each UTXO being spent, verifies the wallet owns it,
    /// and signs the transaction's signing_hash with the wallet's spend key.
    ///
    /// Returns an error if:
    /// - A referenced UTXO doesn't exist
    /// - The wallet doesn't own the UTXO (spend key mismatch)
    pub fn sign_transaction(&self, tx: &mut Transaction, ledger: &Ledger) -> Result<()> {
        let signing_hash = tx.signing_hash();
        let our_address = self.default_address();
        let our_spend_key = our_address.spend_public_key().to_bytes();

        // Get the private spend key for signing
        let spend_private = self.account_key.default_subaddress_spend_private();

        for input in &mut tx.inputs {
            // Look up the UTXO being spent
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            let utxo = ledger
                .get_utxo(&utxo_id)
                .map_err(|e| anyhow::anyhow!("Failed to get UTXO: {}", e))?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "UTXO not found: {}:{}",
                        hex::encode(&input.tx_hash[0..8]),
                        input.output_index
                    )
                })?;

            // Verify we own this UTXO
            if utxo.output.recipient_spend_key != our_spend_key {
                return Err(anyhow::anyhow!(
                    "UTXO {}:{} does not belong to this wallet",
                    hex::encode(&input.tx_hash[0..8]),
                    input.output_index
                ));
            }

            // Sign the transaction with our spend private key
            let signature: RistrettoSignature =
                spend_private.sign_schnorrkel(b"cadence-tx-v1", &signing_hash);

            // Store the 64-byte signature
            let sig_bytes: &[u8] = signature.as_ref();
            input.signature = sig_bytes.to_vec();
        }

        Ok(())
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
