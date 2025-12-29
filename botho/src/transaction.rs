// Copyright (c) 2024 Botho Foundation

//! Transaction types for value transfers with CryptoNote-style stealth addresses.
//!
//! # Privacy Model
//!
//! Botho uses stealth addresses to protect recipient privacy:
//! - Each output has a unique one-time public key (unlinkable)
//! - Only the recipient can detect outputs sent to them (using view key)
//! - Only the recipient can spend outputs (using spend key)
//!
//! # Stealth Address Protocol
//!
//! For a recipient with subaddress (C, D) where C is the view public key and
//! D is the spend public key:
//!
//! **Sender creates:**
//! - Random ephemeral key `r`
//! - Target key: `P = Hs(r * C) * G + D` (one-time spend public key)
//! - Public key: `R = r * D` (ephemeral DH public key)
//!
//! **Recipient scans:**
//! - Computes `D' = P - Hs(a * R) * G` where `a` is view private key
//! - If `D' == D` (their spend public key), they own the output
//!
//! **Recipient spends:**
//! - Recovers private key: `x = Hs(a * R) + d` where `d` is spend private key

use bt_account_keys::{AccountKey, PublicAddress};
use bt_crypto_keys::{RistrettoPrivate, RistrettoPublic, RistrettoSignature};
use bt_crypto_ring_signature::onetime_keys::{
    create_tx_out_public_key, create_tx_out_target_key, recover_onetime_private_key,
    recover_public_subaddress_spend_key,
};
use bt_util_from_random::FromRandom;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Minimum transaction fee in picocredits (0.0001 credits = 100_000_000 picocredits)
pub const MIN_TX_FEE: u64 = 100_000_000;

/// Picocredits per credit (10^12)
pub const PICOCREDITS_PER_CREDIT: u64 = 1_000_000_000_000;

/// A transaction output (UTXO) with stealth addressing.
///
/// Uses CryptoNote-style one-time keys for recipient privacy:
/// - `target_key`: One-time public key that only the recipient can identify and spend
/// - `public_key`: Ephemeral DH public key for recipient to derive shared secret
///
/// The recipient's actual address is not stored in the output, making outputs
/// unlinkable even for the same recipient.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxOutput {
    /// Amount in picocredits
    pub amount: u64,

    /// One-time target key: `Hs(r * C) * G + D`
    /// This is the stealth spend public key that only the recipient can identify.
    pub target_key: [u8; 32],

    /// Ephemeral public key: `r * D`
    /// Used by recipient to derive the shared secret for detecting ownership.
    pub public_key: [u8; 32],
}

impl TxOutput {
    /// Create a new stealth output for a recipient.
    ///
    /// Generates a random ephemeral key and computes:
    /// - `target_key = Hs(r * C) * G + D` (one-time spend key)
    /// - `public_key = r * D` (ephemeral DH key)
    ///
    /// Only the recipient with view key `a` (where `C = a * D`) can detect
    /// this output belongs to them by checking if `P - Hs(a * R) * G == D`.
    pub fn new(amount: u64, recipient: &PublicAddress) -> Self {
        // Generate random ephemeral private key
        let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);

        // Create stealth output keys
        let target_key = create_tx_out_target_key(&tx_private_key, recipient);
        let public_key = create_tx_out_public_key(&tx_private_key, recipient.spend_public_key());

        Self {
            amount,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
        }
    }

    /// Create a stealth output with a specific ephemeral key (for testing).
    pub fn new_with_key(
        amount: u64,
        recipient: &PublicAddress,
        tx_private_key: &RistrettoPrivate,
    ) -> Self {
        let target_key = create_tx_out_target_key(tx_private_key, recipient);
        let public_key = create_tx_out_public_key(tx_private_key, recipient.spend_public_key());

        Self {
            amount,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
        }
    }

    /// Compute a unique identifier for this output.
    pub fn id(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.amount.to_le_bytes());
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        hasher.finalize().into()
    }

    /// Check if this stealth output belongs to an account.
    ///
    /// Uses the view private key to compute the expected spend public key
    /// and compares with the account's known subaddresses.
    ///
    /// Returns `Some(subaddress_index)` if the output belongs to this account,
    /// or `None` if it doesn't.
    pub fn belongs_to(&self, account: &AccountKey) -> Option<u64> {
        let view_private = account.view_private_key();
        let public_key = match RistrettoPublic::try_from(&self.public_key[..]) {
            Ok(pk) => pk,
            Err(_) => return None,
        };
        let target_key = match RistrettoPublic::try_from(&self.target_key[..]) {
            Ok(tk) => tk,
            Err(_) => return None,
        };

        // Recover what the spend public key would be if this output belongs to us
        let recovered_spend_key =
            recover_public_subaddress_spend_key(view_private, &target_key, &public_key);

        // Check against default subaddress (index 0)
        let default_subaddr = account.default_subaddress();
        let default_spend = default_subaddr.spend_public_key();
        if recovered_spend_key.to_bytes() == default_spend.to_bytes() {
            return Some(0);
        }

        // Check against change subaddress (index 1)
        let change_subaddr = account.change_subaddress();
        let change_spend = change_subaddr.spend_public_key();
        if recovered_spend_key.to_bytes() == change_spend.to_bytes() {
            return Some(1);
        }

        // Could extend to check more subaddresses if needed
        None
    }

    /// Recover the one-time private key needed to spend this output.
    ///
    /// This should only be called after verifying `belongs_to` returns Some.
    ///
    /// # Arguments
    /// * `account` - The account that owns this output
    /// * `subaddress_index` - The subaddress index (from `belongs_to`)
    pub fn recover_spend_key(
        &self,
        account: &AccountKey,
        subaddress_index: u64,
    ) -> Option<RistrettoPrivate> {
        let public_key = RistrettoPublic::try_from(&self.public_key[..]).ok()?;
        let view_private = account.view_private_key();
        let subaddress_spend_private = account.subaddress_spend_private(subaddress_index);

        Some(recover_onetime_private_key(
            &public_key,
            view_private,
            &subaddress_spend_private,
        ))
    }
}

/// A reference to a previous output being spent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxInput {
    /// Hash of the transaction containing the output
    pub tx_hash: [u8; 32],

    /// Index of the output in that transaction
    pub output_index: u32,

    /// Signature proving ownership (64 bytes for Schnorrkel)
    pub signature: Vec<u8>,
}

impl TxInput {
    /// Verify the signature for this input.
    ///
    /// # Arguments
    /// * `signing_hash` - The transaction's signing hash (from `Transaction::signing_hash()`)
    /// * `target_key` - The one-time public key from the UTXO being spent
    ///
    /// # Returns
    /// `true` if the signature is valid, `false` otherwise.
    pub fn verify_signature(&self, signing_hash: &[u8; 32], target_key: &[u8; 32]) -> bool {
        // Signature must be exactly 64 bytes
        if self.signature.len() != 64 {
            return false;
        }

        // Parse the target key as a RistrettoPublic
        let public_key = match RistrettoPublic::try_from(&target_key[..]) {
            Ok(pk) => pk,
            Err(_) => return false,
        };

        // Parse the signature
        let signature = match RistrettoSignature::try_from(self.signature.as_slice()) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        // Verify using Schnorrkel with the same domain separator used for signing
        public_key
            .verify_schnorrkel(b"botho-tx-v1", signing_hash, &signature)
            .is_ok()
    }
}

/// A complete transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction version
    pub version: u32,

    /// Inputs (outputs being spent)
    pub inputs: Vec<TxInput>,

    /// Outputs (new UTXOs being created)
    pub outputs: Vec<TxOutput>,

    /// Transaction fee in picocredits
    pub fee: u64,

    /// Block height when this tx was created (for replay protection)
    pub created_at_height: u64,
}

impl Transaction {
    /// Create a new transaction
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

    /// Compute the transaction hash (includes signatures)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.version.to_le_bytes());

        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.target_key);
            hasher.update(output.public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Compute the signing hash (excludes signatures for deterministic signing)
    ///
    /// This hash is used as the message for signing/verifying transaction inputs.
    /// It includes all transaction data except the signatures themselves.
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Domain separator for transaction signing
        hasher.update(b"botho-tx-v1");

        hasher.update(self.version.to_le_bytes());

        // Include input references but NOT signatures
        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        // Include all outputs (stealth keys, not recipient identity)
        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.target_key);
            hasher.update(output.public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Get total output amount (excluding fee)
    pub fn total_output(&self) -> u64 {
        self.outputs.iter().map(|o| o.amount).sum()
    }

    /// Check basic transaction validity (structure only, not signatures or UTXO existence)
    pub fn is_valid_structure(&self) -> Result<(), &'static str> {
        if self.inputs.is_empty() {
            return Err("Transaction has no inputs");
        }
        if self.outputs.is_empty() {
            return Err("Transaction has no outputs");
        }
        if self.outputs.iter().any(|o| o.amount == 0) {
            return Err("Transaction has zero-amount output");
        }
        if self.fee < MIN_TX_FEE {
            return Err("Transaction fee below minimum");
        }
        Ok(())
    }
}

/// Identifier for a UTXO (transaction hash + output index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UtxoId {
    pub tx_hash: [u8; 32],
    pub output_index: u32,
}

impl UtxoId {
    pub fn new(tx_hash: [u8; 32], output_index: u32) -> Self {
        Self {
            tx_hash,
            output_index,
        }
    }

    /// Convert to bytes for storage
    pub fn to_bytes(&self) -> [u8; 36] {
        let mut bytes = [0u8; 36];
        bytes[0..32].copy_from_slice(&self.tx_hash);
        bytes[32..36].copy_from_slice(&self.output_index.to_le_bytes());
        bytes
    }

    /// Parse from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 36 {
            return None;
        }
        let tx_hash: [u8; 32] = bytes[0..32].try_into().ok()?;
        let output_index = u32::from_le_bytes(bytes[32..36].try_into().ok()?);
        Some(Self {
            tx_hash,
            output_index,
        })
    }
}

/// A stored UTXO with its output data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utxo {
    pub id: UtxoId,
    pub output: TxOutput,
    /// Block height where this UTXO was created
    pub created_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test output with raw bytes (for structure tests)
    fn test_output(amount: u64, target: [u8; 32], public: [u8; 32]) -> TxOutput {
        TxOutput {
            amount,
            target_key: target,
            public_key: public,
        }
    }

    #[test]
    fn test_utxo_id_roundtrip() {
        let id = UtxoId::new([1u8; 32], 42);
        let bytes = id.to_bytes();
        let recovered = UtxoId::from_bytes(&bytes).unwrap();
        assert_eq!(id, recovered);
    }

    #[test]
    fn test_transaction_hash_deterministic() {
        let tx = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        let hash1 = tx.hash();
        let hash2 = tx.hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_signing_hash_excludes_signatures() {
        // Create two transactions with different signatures but same content
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64], // zeros
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0xff; 64], // ones
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );

        // signing_hash should be the same (excludes signatures)
        assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_signing_hash_changes_with_content() {
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![test_output(2000, [2u8; 32], [3u8; 32])], // Different amount
            100,
            1,
        );

        // signing_hash should be different when content changes
        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_transaction_is_valid_structure_no_inputs() {
        let tx = Transaction::new(
            vec![],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        assert!(tx.is_valid_structure().is_err());
    }

    #[test]
    fn test_transaction_is_valid_structure_no_outputs() {
        let tx = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![],
            100,
            1,
        );
        assert!(tx.is_valid_structure().is_err());
    }

    #[test]
    fn test_transaction_is_valid_structure_valid() {
        let tx = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_ok());
    }

    // Stealth address tests require actual crypto keys - see wallet tests
}
