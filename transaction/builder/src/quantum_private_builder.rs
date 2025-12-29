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

use alloc::string::String;
use alloc::vec::Vec;
use bth_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};
use bth_crypto_digestible::{DigestTranscript, Digestible, MerlinTranscript};
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_pq::{derive_onetime_sig_keypair, MlDsa65KeyPair};
use bth_transaction_core::{
    quantum_private::{QuantumPrivateTxIn, QuantumPrivateTxOut},
    tx::TxHash,
    Amount, BlockVersion, MaskedAmount,
};
use bth_util_from_random::FromRandom;
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
    /// Block version for this transaction.
    block_version: BlockVersion,
}

impl QuantumPrivateTransactionBuilder {
    /// Create a new quantum-private transaction builder.
    ///
    /// # Arguments
    /// * `sender` - The sender's quantum-safe account key
    /// * `block_version` - The block version to target
    pub fn new(sender: QuantumSafeAccountKey, block_version: BlockVersion) -> Self {
        Self {
            sender,
            inputs: Vec::new(),
            outputs: Vec::new(),
            fee: 0,
            block_version,
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

        // Get recipient's classical view public key for ECDH
        let recipient_view_key = pending.recipient.classical().view_public_key();

        // Create shared secret using ECDH: shared = tx_private_key * view_public_key
        let shared_secret = bth_transaction_core::onetime_keys::create_shared_secret(
            recipient_view_key,
            &tx_private_key,
        );

        // Create classical one-time target key using stealth address protocol
        // target = Hs(shared) * G + spend_public_key
        let target_key = bth_transaction_core::onetime_keys::create_tx_out_target_key(
            &tx_private_key,
            pending.recipient.classical(),
        );

        // Mask the amount
        let masked_amount =
            MaskedAmount::new(self.block_version, pending.amount, &shared_secret.into()).map_err(
                |e| {
                    QuantumPrivateTxBuilderError::EncapsulationError(alloc::format!(
                        "Failed to mask amount: {:?}",
                        e
                    ))
                },
            )?;

        // === Post-Quantum Layer ===

        // Encapsulate to recipient's ML-KEM public key
        let recipient_pq_kem_key = pending.recipient.pq_kem_public_key();
        let (ciphertext, pq_shared_secret) = recipient_pq_kem_key.encapsulate();

        // Derive PQ one-time signing keypair from the shared secret
        let pq_onetime_keypair = derive_onetime_sig_keypair(pq_shared_secret.as_bytes(), 0);
        let pq_target_key = pq_onetime_keypair.public_key().clone();

        Ok(QuantumPrivateTxOut::new(
            masked_amount,
            CompressedRistrettoPublic::from(&target_key),
            CompressedRistrettoPublic::from(&tx_public_key),
            ciphertext,
            pq_target_key,
        ))
    }

    /// Compute the message to be signed by all inputs.
    fn compute_signing_message(&self, outputs: &[QuantumPrivateTxOut]) -> [u8; 32] {
        let mut transcript = MerlinTranscript::new(b"quantum-private-tx");

        // Hash all outputs - use a single static label since index is included via order
        for output in outputs.iter() {
            output.append_to_transcript(b"output", &mut transcript);
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
    let pq_signing_keypair = derive_onetime_sig_keypair(pq_shared_secret.as_bytes(), output_index);

    // Derive the classical one-time private key
    // This requires the view private key and the output's public key
    let tx_public_key = RistrettoPublic::try_from(&output.public_key).map_err(|_| {
        QuantumPrivateTxBuilderError::InvalidRecipient("Invalid output public key".into())
    })?;

    // Recover one-time private key using the standard CryptoNote protocol
    let onetime_private_key = bth_transaction_core::onetime_keys::recover_onetime_private_key(
        &tx_public_key,
        account.classical().view_private_key(),
        &account.classical().subaddress_spend_private(0),
    );

    // Get the decrypted value from the masked amount
    let masked_amount = output
        .masked_amount
        .as_ref()
        .ok_or(QuantumPrivateTxBuilderError::MissingPqCredentials)?;

    // Create shared secret for decryption
    let shared_secret = bth_transaction_core::onetime_keys::create_shared_secret(
        &tx_public_key,
        account.classical().view_private_key(),
    );

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
    use bth_crypto_pq::MlKem768KeyPair;
    use bth_transaction_core::TokenId;

    /// Test successful transaction building.
    #[test]
    fn test_builder_success() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(
            account.clone(),
            BlockVersion::try_from(3).unwrap(),
        );

        // Add input with value 1000
        let mut input = create_test_input_credentials();
        input.value = 1000;
        builder.add_input(input);

        // Add output with value 900 (fee = 100)
        let recipient = account.subaddress(0);
        builder.add_output(Amount::new(900, TokenId::from(0)), recipient);
        builder.set_fee(100);

        let result = builder.build(&mut rand::thread_rng());
        assert!(result.is_ok());

        let (inputs, outputs) = result.unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(outputs.len(), 1);
    }

    /// Test transaction with multiple inputs and outputs.
    #[test]
    fn test_builder_multiple_io() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(
            account.clone(),
            BlockVersion::try_from(3).unwrap(),
        );

        // Add two inputs (500 + 600 = 1100)
        let mut input1 = create_test_input_credentials();
        input1.value = 500;
        builder.add_input(input1);

        let mut input2 = create_test_input_credentials();
        input2.value = 600;
        builder.add_input(input2);

        // Add three outputs (400 + 300 + 300 = 1000, fee = 100)
        let recipient = account.subaddress(0);
        builder.add_output(Amount::new(400, TokenId::from(0)), recipient.clone());
        builder.add_output(Amount::new(300, TokenId::from(0)), recipient.clone());
        builder.add_output(Amount::new(300, TokenId::from(0)), recipient);
        builder.set_fee(100);

        let result = builder.build(&mut rand::thread_rng());
        assert!(result.is_ok());

        let (inputs, outputs) = result.unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(outputs.len(), 3);
    }

    /// Test that each output has unique PQ data.
    #[test]
    fn test_outputs_have_unique_pq_data() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(
            account.clone(),
            BlockVersion::try_from(3).unwrap(),
        );

        let mut input = create_test_input_credentials();
        input.value = 200;
        builder.add_input(input);

        let recipient = account.subaddress(0);
        builder.add_output(Amount::new(100, TokenId::from(0)), recipient.clone());
        builder.add_output(Amount::new(100, TokenId::from(0)), recipient);

        let (_, outputs) = builder.build(&mut rand::thread_rng()).unwrap();

        // Each output should have a different PQ ciphertext (random encapsulation)
        let ct1 = outputs[0].get_pq_ciphertext().unwrap();
        let ct2 = outputs[1].get_pq_ciphertext().unwrap();
        assert_ne!(ct1.as_bytes(), ct2.as_bytes());

        // Each output should have a different public key
        assert_ne!(outputs[0].public_key, outputs[1].public_key);
    }

    /// Test that signatures are valid for each input.
    #[test]
    fn test_input_signatures_valid() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(
            account.clone(),
            BlockVersion::try_from(3).unwrap(),
        );

        // Create input with known signing keypair
        let input = create_test_input_credentials();
        let pq_public_key = input.pq_signing_keypair.public_key().clone();
        builder.add_input(input);

        let recipient = account.subaddress(0);
        builder.add_output(Amount::new(100, TokenId::from(0)), recipient);

        let (inputs, outputs) = builder.build(&mut rand::thread_rng()).unwrap();

        // Verify the PQ signature
        let message = compute_test_message(&outputs);
        let pq_sig = inputs[0].get_dilithium_signature().unwrap();
        assert!(pq_public_key.verify(&message, &pq_sig).is_ok());
    }

    /// Helper to compute signing message (mirrors builder logic).
    fn compute_test_message(outputs: &[QuantumPrivateTxOut]) -> [u8; 32] {
        let mut transcript = MerlinTranscript::new(b"quantum-private-tx");
        for output in outputs.iter() {
            output.append_to_transcript(b"output", &mut transcript);
        }
        let mut message = [0u8; 32];
        transcript.extract_digest(&mut message);
        message
    }

    /// Test total value calculation.
    #[test]
    fn test_total_value_calculation() {
        let account = create_test_account();
        let mut builder = QuantumPrivateTransactionBuilder::new(
            account.clone(),
            BlockVersion::try_from(3).unwrap(),
        );

        // Add inputs
        let mut input1 = create_test_input_credentials();
        input1.value = 500;
        builder.add_input(input1);

        let mut input2 = create_test_input_credentials();
        input2.value = 300;
        builder.add_input(input2);

        assert_eq!(builder.total_input_value(), 800);

        // Add outputs
        let recipient = account.subaddress(0);
        builder.add_output(Amount::new(400, TokenId::from(0)), recipient.clone());
        builder.add_output(Amount::new(300, TokenId::from(0)), recipient);

        assert_eq!(builder.total_output_value(), 700);
    }

    #[test]
    fn test_builder_no_inputs_error() {
        let account = create_test_account();
        let builder =
            QuantumPrivateTransactionBuilder::new(account, BlockVersion::try_from(3).unwrap());

        let result = builder.build(&mut rand::thread_rng());
        assert!(matches!(
            result,
            Err(QuantumPrivateTxBuilderError::NoInputs)
        ));
    }

    #[test]
    fn test_builder_no_outputs_error() {
        let account = create_test_account();
        let mut builder =
            QuantumPrivateTransactionBuilder::new(account, BlockVersion::try_from(3).unwrap());

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
        let mut builder = QuantumPrivateTransactionBuilder::new(
            account.clone(),
            BlockVersion::try_from(3).unwrap(),
        );

        // Add input with value 100
        let mut input = create_test_input_credentials();
        input.value = 100;
        builder.add_input(input);

        // Add output with value 200 (more than input)
        let recipient = account.subaddress(0);
        builder.add_output(Amount::new(200, TokenId::from(0)), recipient);

        let result = builder.build(&mut rand::thread_rng());
        assert!(matches!(
            result,
            Err(QuantumPrivateTxBuilderError::ValueNotConserved { .. })
        ));
    }

    fn create_test_account() -> QuantumSafeAccountKey {
        QuantumSafeAccountKey::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
    }

    fn create_test_input_credentials() -> QuantumPrivateInputCredentials {
        let rng = &mut rand::thread_rng();

        // Create a dummy output
        let shared_secret = RistrettoPublic::from_random(rng);
        let masked_amount = MaskedAmount::new(
            BlockVersion::try_from(3).unwrap(),
            Amount::new(100, TokenId::from(0)),
            &shared_secret.into(),
        )
        .unwrap();

        let kem_keypair = MlKem768KeyPair::generate();
        let (ciphertext, _) = kem_keypair.public_key().encapsulate();
        let sig_keypair = bth_crypto_pq::MlDsa65KeyPair::generate();

        let output = QuantumPrivateTxOut::new(
            masked_amount,
            CompressedRistrettoPublic::from(&RistrettoPublic::from_random(rng)),
            CompressedRistrettoPublic::from(&RistrettoPublic::from_random(rng)),
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
