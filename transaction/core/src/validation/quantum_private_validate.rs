// Copyright (c) 2024 Botho Foundation

//! Quantum-private transaction validation.
//!
//! This module provides validation functions for quantum-private transactions,
//! which use a hybrid classical + post-quantum cryptographic approach.
//!
//! # Security Model
//!
//! Quantum-private transactions require BOTH layers to verify:
//! - Classical layer: Schnorr signatures on Ristretto (current security)
//! - Post-quantum layer: ML-DSA-65 (Dilithium) signatures (future security)

use super::error::{TransactionValidationError, TransactionValidationResult};
use crate::quantum_private::{QuantumPrivateError, QuantumPrivateTxIn, QuantumPrivateTxOut};
use bt_crypto_pq::{
    MlDsa65PublicKey, MlKem768Ciphertext, ML_DSA_65_PUBLIC_KEY_BYTES, ML_KEM_768_CIPHERTEXT_BYTES,
};

/// Validate the structure of a quantum-private transaction output.
///
/// This checks that:
/// - The PQ ciphertext is the correct length and valid
/// - The PQ target key is the correct length and valid
/// - Required classical fields are present
///
/// # Arguments
/// * `tx_out` - The quantum-private transaction output to validate
pub fn validate_quantum_private_tx_out(
    tx_out: &QuantumPrivateTxOut,
) -> TransactionValidationResult<()> {
    // Validate PQ ciphertext length and structure
    if tx_out.pq_ciphertext.len() != ML_KEM_768_CIPHERTEXT_BYTES {
        return Err(TransactionValidationError::InvalidPqCiphertext);
    }

    // Validate ciphertext can be parsed
    MlKem768Ciphertext::from_bytes(&tx_out.pq_ciphertext)
        .map_err(|_| TransactionValidationError::InvalidPqCiphertext)?;

    // Validate PQ target key length and structure
    if tx_out.pq_target_key.len() != ML_DSA_65_PUBLIC_KEY_BYTES {
        return Err(TransactionValidationError::InvalidPqPublicKey);
    }

    // Validate public key can be parsed
    MlDsa65PublicKey::from_bytes(&tx_out.pq_target_key)
        .map_err(|_| TransactionValidationError::InvalidPqPublicKey)?;

    // Validate masked amount is present
    if tx_out.masked_amount.is_none() {
        return Err(TransactionValidationError::TxFeeError);
    }

    Ok(())
}

/// Validate the structure of a quantum-private transaction input.
///
/// This checks that:
/// - The tx_hash is the correct length
/// - The Schnorr signature is present and correct length
/// - The Dilithium signature is present and correct length
///
/// # Arguments
/// * `tx_in` - The quantum-private transaction input to validate
pub fn validate_quantum_private_tx_in_structure(
    tx_in: &QuantumPrivateTxIn,
) -> TransactionValidationResult<()> {
    // Validate tx_hash length (32 bytes)
    if tx_in.tx_hash.len() != 32 {
        return Err(TransactionValidationError::InvalidPqOutputReference);
    }

    // Validate Schnorr signature is present and correct length
    if tx_in.schnorr_signature.is_empty() {
        return Err(TransactionValidationError::MissingPqSignature);
    }
    if tx_in.schnorr_signature.len() != 64 {
        return Err(TransactionValidationError::QuantumPrivateSchnorrVerificationFailed);
    }

    // Validate Dilithium signature is present and correct length
    if tx_in.dilithium_signature.is_empty() {
        return Err(TransactionValidationError::MissingPqSignature);
    }

    // ML-DSA-65 signatures are 3309 bytes
    if tx_in.dilithium_signature.len() != 3309 {
        return Err(TransactionValidationError::QuantumPrivateDilithiumVerificationFailed);
    }

    Ok(())
}

/// Verify the signatures on a quantum-private transaction input.
///
/// This verifies BOTH the classical Schnorr signature AND the post-quantum
/// Dilithium signature. Both must verify for the input to be valid.
///
/// # Arguments
/// * `tx_in` - The quantum-private transaction input
/// * `message` - The message that was signed (typically the transaction prefix hash)
/// * `classical_public_key` - The one-time classical public key from the output being spent
/// * `pq_public_key` - The ML-DSA-65 public key from the output being spent
///
/// # Security
///
/// This hybrid verification provides:
/// 1. Immediate security against classical adversaries (Schnorr)
/// 2. Future security against quantum adversaries (Dilithium)
/// 3. Fallback security if either cryptosystem is compromised
pub fn verify_quantum_private_signatures(
    tx_in: &QuantumPrivateTxIn,
    message: &[u8],
    classical_public_key: &bt_crypto_keys::RistrettoPublic,
    pq_public_key: &MlDsa65PublicKey,
) -> TransactionValidationResult<()> {
    // Verify Schnorr signature
    verify_schnorr_signature(tx_in, message, classical_public_key)?;

    // Verify Dilithium signature
    verify_dilithium_signature(tx_in, message, pq_public_key)?;

    Ok(())
}

/// Verify the classical Schnorr signature on a quantum-private input.
///
/// Uses a standard Schnorr signature scheme: sig = (R, s) where
/// s = k + c*x, c = H(R || P || m), and verification checks s*G = R + c*P.
fn verify_schnorr_signature(
    tx_in: &QuantumPrivateTxIn,
    message: &[u8],
    public_key: &bt_crypto_keys::RistrettoPublic,
) -> TransactionValidationResult<()> {
    use bt_crypto_digestible::{DigestTranscript, Digestible, MerlinTranscript};
    use curve25519_dalek::{
        constants::RISTRETTO_BASEPOINT_POINT,
        ristretto::CompressedRistretto,
        scalar::Scalar,
    };

    // Get the signature bytes
    let signature_bytes: [u8; 64] = tx_in
        .schnorr_signature
        .as_slice()
        .try_into()
        .map_err(|_| TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;

    // Parse signature as (R, s) - 32 bytes each
    let r_bytes: [u8; 32] = signature_bytes[..32]
        .try_into()
        .map_err(|_| TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;
    let s_bytes: [u8; 32] = signature_bytes[32..]
        .try_into()
        .map_err(|_| TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;

    // Parse R as a compressed Ristretto point
    let r_compressed = CompressedRistretto::from_slice(&r_bytes)
        .map_err(|_| TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;
    let r_point = r_compressed
        .decompress()
        .ok_or(TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;

    // Parse s as a scalar
    let s = Scalar::from_canonical_bytes(s_bytes)
        .into_option()
        .ok_or(TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;

    // Get public key point from compressed bytes
    let pk_bytes: [u8; 32] = public_key.to_bytes();
    let pk_compressed = CompressedRistretto::from_slice(&pk_bytes)
        .map_err(|_| TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;
    let pk_point = pk_compressed
        .decompress()
        .ok_or(TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)?;

    // Compute challenge: c = H(R || P || message)
    let mut transcript = MerlinTranscript::new(b"quantum-private-schnorr");
    r_bytes.append_to_transcript(b"R", &mut transcript);
    pk_bytes.as_slice().append_to_transcript(b"P", &mut transcript);
    message.append_to_transcript(b"msg", &mut transcript);

    let mut challenge_bytes = [0u8; 32];
    transcript.extract_digest(&mut challenge_bytes);
    let c = Scalar::from_bytes_mod_order(challenge_bytes);

    // Verify: s*G == R + c*P
    let lhs = s * RISTRETTO_BASEPOINT_POINT;
    let rhs = r_point + c * pk_point;

    if lhs != rhs {
        return Err(TransactionValidationError::QuantumPrivateSchnorrVerificationFailed);
    }

    Ok(())
}

/// Verify the post-quantum Dilithium signature on a quantum-private input.
fn verify_dilithium_signature(
    tx_in: &QuantumPrivateTxIn,
    message: &[u8],
    public_key: &MlDsa65PublicKey,
) -> TransactionValidationResult<()> {
    use bt_crypto_pq::MlDsa65Signature;

    // Parse the signature
    let signature = MlDsa65Signature::from_bytes(&tx_in.dilithium_signature)
        .map_err(|_| TransactionValidationError::QuantumPrivateDilithiumVerificationFailed)?;

    // Verify the signature
    public_key
        .verify(message, &signature)
        .map_err(|_| TransactionValidationError::QuantumPrivateDilithiumVerificationFailed)?;

    Ok(())
}

/// Validate a list of quantum-private transaction outputs.
pub fn validate_quantum_private_outputs(
    outputs: &[QuantumPrivateTxOut],
) -> TransactionValidationResult<()> {
    if outputs.is_empty() {
        return Err(TransactionValidationError::NoOutputs);
    }

    for output in outputs {
        validate_quantum_private_tx_out(output)?;
    }

    // Check for duplicate PQ public keys
    let mut seen_keys = alloc::collections::BTreeSet::new();
    for output in outputs {
        if !seen_keys.insert(&output.pq_target_key) {
            return Err(TransactionValidationError::DuplicateOutputPublicKey);
        }
    }

    Ok(())
}

/// Validate a list of quantum-private transaction inputs.
pub fn validate_quantum_private_inputs(
    inputs: &[QuantumPrivateTxIn],
) -> TransactionValidationResult<()> {
    if inputs.is_empty() {
        return Err(TransactionValidationError::NoInputs);
    }

    for input in inputs {
        validate_quantum_private_tx_in_structure(input)?;
    }

    Ok(())
}

/// Context needed to verify quantum-private transaction inputs.
///
/// This trait allows the validation layer to look up the outputs being spent
/// without coupling to a specific database implementation.
pub trait QuantumPrivateOutputLookup {
    /// Look up a quantum-private output by its transaction hash and index.
    ///
    /// Returns the output if found, or an error if not found or invalid.
    fn get_quantum_private_output(
        &self,
        tx_hash: &[u8],
        output_index: u32,
    ) -> Result<QuantumPrivateTxOut, QuantumPrivateError>;
}

/// Verify all signatures on quantum-private transaction inputs.
///
/// This looks up each referenced output and verifies both the classical
/// and post-quantum signatures against the public keys in that output.
///
/// # Arguments
/// * `inputs` - The quantum-private transaction inputs
/// * `message` - The message that was signed (transaction prefix hash)
/// * `output_lookup` - Trait object for looking up referenced outputs
pub fn verify_all_quantum_private_signatures(
    inputs: &[QuantumPrivateTxIn],
    message: &[u8],
    output_lookup: &impl QuantumPrivateOutputLookup,
) -> TransactionValidationResult<()> {
    for input in inputs {
        // Look up the output being spent
        let output = output_lookup
            .get_quantum_private_output(&input.tx_hash, input.output_index)
            .map_err(|_| TransactionValidationError::InvalidPqOutputReference)?;

        // Get the classical public key
        let classical_public_key = bt_crypto_keys::RistrettoPublic::try_from(&output.target_key)
            .map_err(|_| TransactionValidationError::InvalidRistrettoPublicKey)?;

        // Get the PQ public key
        let pq_public_key = output
            .get_pq_target_key()
            .map_err(|_| TransactionValidationError::InvalidPqPublicKey)?;

        // Verify both signatures
        verify_quantum_private_signatures(input, message, &classical_public_key, &pq_public_key)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn test_validate_tx_out_empty_ciphertext() {
        let tx_out = QuantumPrivateTxOut {
            masked_amount: None,
            target_key: Default::default(),
            public_key: Default::default(),
            e_fog_hint: Default::default(),
            e_memo: None,
            cluster_tags: None,
            pq_ciphertext: Vec::new(),
            pq_target_key: Vec::new(),
        };

        let result = validate_quantum_private_tx_out(&tx_out);
        assert!(matches!(
            result,
            Err(TransactionValidationError::InvalidPqCiphertext)
        ));
    }

    #[test]
    fn test_validate_tx_in_empty_signatures() {
        let tx_in = QuantumPrivateTxIn {
            tx_hash: vec![0u8; 32],
            output_index: 0,
            schnorr_signature: Vec::new(),
            dilithium_signature: Vec::new(),
        };

        let result = validate_quantum_private_tx_in_structure(&tx_in);
        assert!(matches!(
            result,
            Err(TransactionValidationError::MissingPqSignature)
        ));
    }

    #[test]
    fn test_validate_tx_in_wrong_tx_hash_length() {
        let tx_in = QuantumPrivateTxIn {
            tx_hash: vec![0u8; 16], // Wrong length
            output_index: 0,
            schnorr_signature: vec![0u8; 64],
            dilithium_signature: vec![0u8; 3309],
        };

        let result = validate_quantum_private_tx_in_structure(&tx_in);
        assert!(matches!(
            result,
            Err(TransactionValidationError::InvalidPqOutputReference)
        ));
    }

    #[test]
    fn test_validate_tx_in_wrong_schnorr_length() {
        let tx_in = QuantumPrivateTxIn {
            tx_hash: vec![0u8; 32],
            output_index: 0,
            schnorr_signature: vec![0u8; 32], // Wrong length
            dilithium_signature: vec![0u8; 3309],
        };

        let result = validate_quantum_private_tx_in_structure(&tx_in);
        assert!(matches!(
            result,
            Err(TransactionValidationError::QuantumPrivateSchnorrVerificationFailed)
        ));
    }

    #[test]
    fn test_validate_tx_in_wrong_dilithium_length() {
        let tx_in = QuantumPrivateTxIn {
            tx_hash: vec![0u8; 32],
            output_index: 0,
            schnorr_signature: vec![0u8; 64],
            dilithium_signature: vec![0u8; 1000], // Wrong length
        };

        let result = validate_quantum_private_tx_in_structure(&tx_in);
        assert!(matches!(
            result,
            Err(TransactionValidationError::QuantumPrivateDilithiumVerificationFailed)
        ));
    }

    #[test]
    fn test_validate_tx_in_valid_structure() {
        let tx_in = QuantumPrivateTxIn {
            tx_hash: vec![0u8; 32],
            output_index: 0,
            schnorr_signature: vec![0u8; 64],
            dilithium_signature: vec![0u8; 3309],
        };

        let result = validate_quantum_private_tx_in_structure(&tx_in);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_empty_inputs() {
        let inputs: Vec<QuantumPrivateTxIn> = Vec::new();
        let result = validate_quantum_private_inputs(&inputs);
        assert!(matches!(result, Err(TransactionValidationError::NoInputs)));
    }

    #[test]
    fn test_validate_empty_outputs() {
        let outputs: Vec<QuantumPrivateTxOut> = Vec::new();
        let result = validate_quantum_private_outputs(&outputs);
        assert!(matches!(result, Err(TransactionValidationError::NoOutputs)));
    }
}
