//! Legacy `botho-tx-v1` transaction types (QUARANTINED — do not use).
//!
//! # Why this module exists
//!
//! Before issue #614, the CLI thin wallet built its own flat, non-private
//! transaction format ("botho-tx-v1"): plaintext recipient view/spend keys, a
//! per-input Schnorrkel signature, no ring signature, no key images, and a
//! **cryptographically broken** stealth output.
//!
//! Specifically, [`TxOutput::new`] below derived `output_public_key` as
//! `SHA256(view_key || spend_key || amount || random_bytes)`. That is NOT the
//! Ristretto Diffie-Hellman stealth protocol the node uses
//! (`create_tx_out_target_key` / `create_tx_out_public_key`). Outputs built
//! this way could **never** be detected by a recipient's
//! `WalletScanner::check_ownership`, and the flat transaction could never
//! bincode-deserialize as `botho::transaction::Transaction` — the node rejected
//! every CLI-built tx with `io error: unexpected end of file`.
//!
//! These types are preserved here (per CLAUDE.md code-preservation) rather than
//! deleted outright, so the historical wire format and its signing-hash domain
//! separator remain recoverable. **Nothing in the wallet builds or submits
//! these anymore** — the live send path uses the real CLSAG format in
//! [`crate::transaction`]. Do not reintroduce these into any submission path.

#![allow(dead_code)]

use bth_account_keys::PublicAddress;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Legacy v1 transaction output (BROKEN stealth derivation — see module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxOutput {
    pub amount: u64,
    pub recipient_view_key: [u8; 32],
    pub recipient_spend_key: [u8; 32],
    pub output_public_key: [u8; 32],
}

impl TxOutput {
    /// Create a new output for a recipient (legacy, cryptographically broken).
    ///
    /// The `output_public_key` is a SHA256 hash, not a Ristretto DH key, so the
    /// recipient can never detect it. Retained only for format archaeology.
    pub fn new(amount: u64, recipient: &PublicAddress) -> Self {
        let view_key = recipient.view_public_key().to_bytes();
        let spend_key = recipient.spend_public_key().to_bytes();

        let mut random_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut random_bytes);

        let mut hasher = Sha256::new();
        hasher.update(view_key);
        hasher.update(spend_key);
        hasher.update(amount.to_le_bytes());
        hasher.update(random_bytes);
        let output_key: [u8; 32] = hasher.finalize().into();

        Self {
            amount,
            recipient_view_key: view_key,
            recipient_spend_key: spend_key,
            output_public_key: output_key,
        }
    }
}

/// Legacy v1 transaction input (flat UTXO reference + Schnorrkel signature).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxInput {
    pub tx_hash: [u8; 32],
    pub output_index: u32,
    pub signature: Vec<u8>,
}

/// Legacy v1 transaction (flat, non-private — see module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
    pub fee: u64,
    pub created_at_height: u64,
}

impl Transaction {
    /// Create a new unsigned legacy transaction.
    pub fn new(
        inputs: Vec<TxInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            version: 1,
            inputs,
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Compute the legacy signing hash (message to be signed).
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"botho-tx-v1");
        hasher.update(self.version.to_le_bytes());

        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.recipient_view_key);
            hasher.update(output.recipient_spend_key);
            hasher.update(output.output_public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_signing_hash_ignores_signature() {
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0xff; 64], // Different signature
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        // Signing hash is independent of signature content.
        assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    }
}
