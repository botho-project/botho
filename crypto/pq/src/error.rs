//! Error types for post-quantum cryptography operations

use thiserror::Error;

/// Errors that can occur during PQ cryptographic operations
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PqError {
    /// Invalid ciphertext length or format
    #[error("Invalid ciphertext: {0}")]
    InvalidCiphertext(String),

    /// Invalid public key length or format
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),

    /// Invalid secret key length or format
    #[error("Invalid secret key: {0}")]
    InvalidSecretKey(String),

    /// Invalid signature length or format
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    /// Signature verification failed
    #[error("Signature verification failed")]
    VerificationFailed,

    /// Decapsulation failed
    #[error("Decapsulation failed")]
    DecapsulationFailed,

    /// Key derivation failed
    #[error("Key derivation failed: {0}")]
    DerivationFailed(String),
}
