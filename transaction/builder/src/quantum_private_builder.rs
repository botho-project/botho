// Copyright (c) 2024 Botho Foundation

//! Quantum-private transaction builder.
//!
//! This module provides functionality to build quantum-private transactions
//! that are protected against quantum adversaries using a hybrid classical +
//! post-quantum cryptographic approach.
//!
//! # Security Model
//!
//! Quantum-private transactions require BOTH layers to sign:
//! - Classical layer: Schnorr signatures on Ristretto (current security)
//! - Post-quantum layer: ML-DSA-65 (Dilithium) signatures (future security)
//!
//! # Usage
//!
//! ```ignore
//! use bt_transaction_builder::QuantumPrivateTransactionBuilder;
//!
//! let builder = QuantumPrivateTransactionBuilder::new(sender_pq_account);
//! builder.add_input(pq_output, input_credentials)?;
//! builder.add_output(amount, recipient_pq_address)?;
//! let tx = builder.build(&mut rng)?;
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use bt_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};
use bt_crypto_digestible::{DigestTranscript, Digestible, MerlinTranscript};
use bt_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bt_crypto_pq::{derive_onetime_sig_keypair, MlDsa65KeyPair};
use bt_transaction_core::{
    encrypted_fog_hint::EncryptedFogHint,
    onetime_keys::create_tx_out_target_key,
    quantum_private::{QuantumPrivateTxIn, QuantumPrivateTxOut},
    tx::TxHash,
    Amount, MaskedAmount,
};
use bt_util_from_random::FromRandom;
use rand_core::{CryptoRng, RngCore};

/// Error type for quantum-private transaction building.
#[derive(Clone, Debug)]
pub enum QuantumPrivateTxBuilderError {
    /// No inputs have been added to the transaction.
    NoInputs,
    /// No outputs have been added to the transaction.
    NoOutputs,
    /// Input value does not equal output value plus fee.
    ValueNotConserved {
        /// Total input value
        input_value: u64,
        /// Total output value
        output_value: u64,
        /// Fee
        fee: u64,
    },
    /// Error during PQ key encapsulation.
    EncapsulationError(String),
    /// Error during signing.
    SigningError(String),
    /// Invalid recipient address.
    InvalidRecipient(String),
    /// Missing PQ credentials for input.
    MissingPqCredentials,
}

/// Credentials for spending a quantum-private output.
#[derive(Clone)]
pub struct QuantumPrivateInputCredentials {
    /// The quantum-private output being spent.
    pub output: QuantumPrivateTxOut,
    /// Hash of the transaction containing the output.
    pub tx_hash: TxHash,
    /// Index of the output within the transaction.
    pub output_index: u32,
    /// Classical one-time private key for this output.
    pub onetime_private_key: RistrettoPrivate,
    /// PQ one-time signing keypair for this output.
    pub pq_signing_keypair: MlDsa65KeyPair,
    /// The decrypted value of this output.
    pub value: u64,
}

/// A pending output to be created.
struct PendingOutput {
    /// Amount to send.
    amount: Amount,
    /// Recipient's quantum-safe public address.
    recipient: QuantumSafePublicAddress,
}

/// Builder for quantum-private transactions.
///
/// This builder creates transactions that use hybrid classical + post-quantum
/// cryptography for both inputs and outputs.
pub struct QuantumPrivateTransactionBuilder {
    /// The sender's quantum-safe account key.
    sender: QuantumSafeAccountKey,
    /// Input credentials for outputs being spent.
    inputs: Vec<QuantumPrivateInputCredentials>,
    /// Pending outputs to create.
    outputs: Vec<PendingOutput>,
    /// Fee for this transaction.
    fee: u64,
}

impl QuantumPrivateTransactionBuilder {
    /// Create a new quantum-private transaction builder.
    ///
    /// # Arguments
    /// * `sender` - The sender's quantum-safe account key
    pub fn new(sender: QuantumSafeAccountKey) -> Self {
        Self {
            sender,
            inputs: Vec::new(),
            outputs: Vec::new(),
            fee: 0,
        }
    }

    /// Set the transaction fee.
    pub fn set_fee(&mut self, fee: u64) {
        self.fee = fee;
    }

    /// Add an input to spend.
    ///
    /// # Arguments
    /// * `credentials` - Credentials for spending the output
    pub fn add_input(&mut self, credentials: QuantumPrivateInputCredentials) {
        self.inputs.push(credentials);
    }

    /// Add an output to the transaction.
    ///
    /// # Arguments
    /// * `amount` - Amount to send
    /// * `recipient` - Recipient's quantum-safe public address
    pub fn add_output(&mut self, amount: Amount, recipient: QuantumSafePublicAddress) {
        self.outputs.push(PendingOutput { amount, recipient });
    }

    /// Get the total input value.
    pub fn total_input_value(&self) -> u64 {
        self.inputs.iter().map(|i| i.value).sum()
    }

    /// Get the total output value.
    pub fn total_output_value(&self) -> u64 {
        self.outputs.iter().map(|o| o.amount.value).sum()
    }

    /// Build the quantum-private transaction.
    ///
    /// This creates all outputs with PQ encryption and signs all inputs
    /// with both classical and post-quantum signatures.
    ///
    /// # Arguments
    /// * `rng` - Cryptographically secure random number generator
    ///
    /// # Returns
    /// A tuple of (inputs, outputs) for the quantum-private transaction.
    pub fn build<RNG: CryptoRng + RngCore>(
        self,
        rng: &mut RNG,
    ) -> Result<(Vec<QuantumPrivateTxIn>, Vec<QuantumPrivateTxOut>), QuantumPrivateTxBuilderError>
    {
        // Validate inputs
        if self.inputs.is_empty() {
            return Err(QuantumPrivateTxBuilderError::NoInputs);
        }
        if self.outputs.is_empty() {
            return Err(QuantumPrivateTxBuilderError::NoOutputs);
        }

        // Check value conservation
        let input_value = self.total_input_value();
        let output_value = self.total_output_value();
        if input_value != output_value + self.fee {
            return Err(QuantumPrivateTxBuilderError::ValueNotConserved {
                input_value,
                output_value,
                fee: self.fee,
            });
        }

        // Build outputs
        let mut tx_outputs = Vec::with_capacity(self.outputs.len());
        for pending in &self.outputs {
            let tx_out = self.build_output(pending, rng)?;
            tx_outputs.push(tx_out);
        }

        // Compute message to sign (hash of outputs)
        let message = self.compute_signing_message(&tx_outputs);

        // Build and sign inputs
        let mut tx_inputs = Vec::with_capacity(self.inputs.len());
        for input_creds in &self.inputs {
            let tx_in = self.build_and_sign_input(input_creds, &message, rng)?;
            tx_inputs.push(tx_in);
        }

        Ok((tx_inputs, tx_outputs))
    }

    /// Build a single quantum-private output.
    fn build_output<RNG: CryptoRng + RngCore>(
        &self,
        pending: &PendingOutput,
        rng: &mut RNG,
    ) -> Result<QuantumPrivateTxOut, QuantumPrivateTxBuilderError> {
        // Generate ephemeral key for this output
        let tx_private_key = RistrettoPrivate::from_random(rng);
        let tx_public_key = RistrettoPublic::from(&tx_private_key);

        // Get recipient's classical public keys
        let recipient_spend_key = pending.recipient.classical().spend_public_key();

        // Create classical one-time target key using stealth address protocol
        let target_key = create_tx_out_target_key(&tx_private_key, recipient_spend_key);

        // Create shared secret for amount masking using ECDH
        let recipient_view_key = pending.recipient.classical().view_public_key();
        let shared_secret =
            bt_transaction_core::onetime_keys::create_shared_secret(recipient_view_key, &tx_private_key);

        // Mask the amount
        let masked_amount = MaskedAmount::new(pending.amount, &shared_secret.into())
            .map_err(|e| {
                QuantumPrivateTxBuilderError::EncapsulationError(alloc::format!(
                    "Failed to mask amount: {:?}",
                    e
                ))
            })?;

        // Create fake fog hint (Botho doesn't use fog)
        let e_fog_hint = EncryptedFogHint::fake_onetime_hint(rng);

        // === Post-Quantum Layer ===

        // Encapsulate to recipient's ML-KEM public key
        let recipient_pq_kem_key = pending.recipient.pq_kem_public_key();
        let (ciphertext, pq_shared_secret) = recipient_pq_kem_key.encapsulate(rng).map_err(|e| {
            QuantumPrivateTxBuilderError::EncapsulationError(alloc::format!(
                "ML-KEM encapsulation failed: {:?}",
                e
            ))
        })?;

        // Derive PQ one-time signing keypair from the shared secret
        let pq_onetime_keypair =
            derive_onetime_sig_keypair(pq_shared_secret.as_bytes(), 0);
        let pq_target_key = pq_onetime_keypair.public_key().clone();

        Ok(QuantumPrivateTxOut::new(
            masked_amount,
            CompressedRistrettoPublic::from(&target_key),
            CompressedRistrettoPublic::from(&tx_public_key),
            e_fog_hint,
            ciphertext,
            pq_target_key,
        ))
    }

    /// Compute the message to be signed by all inputs.
    fn compute_signing_message(&self, outputs: &[QuantumPrivateTxOut]) -> [u8; 32] {
        let mut transcript = MerlinTranscript::new(b"quantum-private-tx");

        // Hash all outputs
        for (i, output) in outputs.iter().enumerate() {
            let label = alloc::format!("output_{}", i);
            output.append_to_transcript(label.as_bytes(), &mut transcript);
        }

        // Extract 32-byte digest
        let mut message = [0u8; 32];
        transcript.extract_digest(&mut message);
        message
    }

    /// Build and sign a single quantum-private input.
    fn build_and_sign_input<RNG: CryptoRng + RngCore>(
        &self,
        creds: &QuantumPrivateInputCredentials,
        message: &[u8; 32],
        rng: &mut RNG,
    ) -> Result<QuantumPrivateTxIn, QuantumPrivateTxBuilderError> {
        // Sign with classical Schnorr signature
        let schnorr_signature = self.sign_schnorr(&creds.onetime_private_key, message, rng)?;

        // Sign with post-quantum Dilithium signature
        // MlDsa65KeyPair::sign returns MlDsa65Signature directly (infallible)
        let dilithium_signature = creds.pq_signing_keypair.sign(message);

        Ok(QuantumPrivateTxIn::new(
            creds.tx_hash,
            creds.output_index,
            schnorr_signature,
            dilithium_signature,
        ))
    }

    /// Create a Schnorr signature.
    ///
    /// Signature scheme: sig = (R, s) where R = k*G, s = k + H(R||P||m)*x
    fn sign_schnorr<RNG: CryptoRng + RngCore>(
        &self,
        private_key: &RistrettoPrivate,
        message: &[u8; 32],
        rng: &mut RNG,
    ) -> Result<[u8; 64], QuantumPrivateTxBuilderError> {
        use curve25519_dalek::{constants::RISTRETTO_BASEPOINT_POINT, scalar::Scalar};

        // Generate random nonce k
        let mut k_bytes = [0u8; 64];
        rng.fill_bytes(&mut k_bytes);
        let k = Scalar::from_bytes_mod_order_wide(&k_bytes);

        // Compute R = k*G
        let r_point = k * RISTRETTO_BASEPOINT_POINT;
        let r_bytes = r_point.compress().to_bytes();

        // Get public key
        let public_key = RistrettoPublic::from(private_key);
        let pk_bytes = public_key.to_bytes();

        // Compute challenge c = H(R || P || m)
        let mut transcript = MerlinTranscript::new(b"quantum-private-schnorr");
        r_bytes.append_to_transcript(b"R", &mut transcript);
        pk_bytes.as_slice().append_to_transcript(b"P", &mut transcript);
        message.as_slice().append_to_transcript(b"msg", &mut transcript);

        let mut c_bytes = [0u8; 32];
        transcript.extract_digest(&mut c_bytes);
        let c = Scalar::from_bytes_mod_order(c_bytes);

        // Compute s = k + c*x
        let x = Scalar::from_bytes_mod_order(private_key.to_bytes());
        let s = k + c * x;

        // Return signature (R, s)
        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(&r_bytes);
        signature[32..].copy_from_slice(&s.to_bytes());

        Ok(signature)
    }
}

/// Helper to derive quantum-private input credentials from a received output.
///
/// This is used when the recipient wants to spend a quantum-private output
/// they received.
pub fn derive_input_credentials(
    account: &QuantumSafeAccountKey,
    output: &QuantumPrivateTxOut,
    tx_hash: TxHash,
    output_index: u32,
) -> Result<QuantumPrivateInputCredentials, QuantumPrivateTxBuilderError> {
    // Get the ML-KEM ciphertext from the output
    let ciphertext = output.get_pq_ciphertext().map_err(|e| {
        QuantumPrivateTxBuilderError::EncapsulationError(alloc::format!(
            "Invalid PQ ciphertext: {:?}",
            e
        ))
    })?;

    // Decapsulate to get the shared secret
    let pq_shared_secret = account.pq_kem_keypair().decapsulate(&ciphertext).map_err(|e| {
        QuantumPrivateTxBuilderError::EncapsulationError(alloc::format!(
            "ML-KEM decapsulation failed: {:?}",
            e
        ))
    })?;

    // Derive the PQ one-time signing keypair
    let pq_signing_keypair =
        derive_onetime_sig_keypair(pq_shared_secret.as_bytes(), output_index);

    // Derive the classical one-time private key
    // This requires the view private key and the output's public key
    let tx_public_key = RistrettoPublic::try_from(&output.public_key).map_err(|_| {
        QuantumPrivateTxBuilderError::InvalidRecipient("Invalid output public key".into())
    })?;

    // Create shared secret using ECDH: shared = view_key * tx_public_key
    let shared_secret = bt_transaction_core::onetime_keys::create_shared_secret(
        &tx_public_key,
        account.classical().view_private_key(),
    );

    // Recover one-time private key: x = H(shared_secret) + spend_private_key
    let onetime_private_key = bt_transaction_core::onetime_keys::recover_onetime_private_key(
        &shared_secret.into(),
        account.classical().subaddress_spend_private(0),
    );

    // Get the decrypted value from the masked amount
    let masked_amount = output
        .masked_amount
        .as_ref()
        .ok_or(QuantumPrivateTxBuilderError::MissingPqCredentials)?;

    let (value, _token_id) = masked_amount
        .get_value(&shared_secret.into())
        .map_err(|_| QuantumPrivateTxBuilderError::MissingPqCredentials)?;

    Ok(QuantumPrivateInputCredentials {
        output: output.clone(),
        tx_hash,
        output_index,
        onetime_private_key,
        pq_signing_keypair,
        value: value.value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_crypto_pq::MlKem768KeyPair;

    #[test]
    fn test_builder_no_inputs_error() {
        let account = create_test_account();
        let builder = QuantumPrivateTransactionBuilder::new(account);

        let result = builder.build(&mut rand::thread_rng());
        assert!(matches!(
            result,
            Err(QuantumPrivateTxBuilderError::NoInputs)
        ));
    }

    #[test]
    fn test_builder_no_outputs_error() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(account);

        // Add a dummy input
        let input = create_test_input_credentials();
        builder.add_input(input);

        let result = builder.build(&mut rand::thread_rng());
        assert!(matches!(
            result,
            Err(QuantumPrivateTxBuilderError::NoOutputs)
        ));
    }

    #[test]
    fn test_value_not_conserved_error() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(account.clone());

        // Add input with value 100
        let mut input = create_test_input_credentials();
        input.value = 100;
        builder.add_input(input);

        // Add output with value 200 (more than input)
        let recipient = account.public_address();
        builder.add_output(Amount::new(200, bt_transaction_core::TokenId::MOB), recipient);

        let result = builder.build(&mut rand::thread_rng());
        assert!(matches!(
            result,
            Err(QuantumPrivateTxBuilderError::ValueNotConserved { .. })
        ));
    }

    fn create_test_account() -> QuantumSafeAccountKey {
        QuantumSafeAccountKey::random(&mut rand::thread_rng())
    }

    fn create_test_input_credentials() -> QuantumPrivateInputCredentials {
        let rng = &mut rand::thread_rng();

        // Create a dummy output
        let masked_amount = MaskedAmount::new(
            Amount::new(100, bt_transaction_core::TokenId::MOB),
            &RistrettoPublic::from_random(rng).into(),
        )
        .unwrap();

        let kem_keypair = MlKem768KeyPair::generate(rng);
        let (ciphertext, _) = kem_keypair.public_key().encapsulate(rng).unwrap();
        let sig_keypair = bt_crypto_pq::MlDsa65KeyPair::generate(rng);

        let output = QuantumPrivateTxOut::new(
            masked_amount,
            CompressedRistrettoPublic::from(&RistrettoPublic::from_random(rng)),
            CompressedRistrettoPublic::from(&RistrettoPublic::from_random(rng)),
            EncryptedFogHint::fake_onetime_hint(rng),
            ciphertext,
            sig_keypair.public_key().clone(),
        );

        QuantumPrivateInputCredentials {
            output,
            tx_hash: TxHash::from([0u8; 32]),
            output_index: 0,
            onetime_private_key: RistrettoPrivate::from_random(rng),
            pq_signing_keypair: sig_keypair,
            value: 100,
        }
    }
}
