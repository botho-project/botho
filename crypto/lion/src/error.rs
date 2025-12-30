//! Error types for the Lion ring signature scheme.

use core::fmt;

/// Errors that can occur in Lion ring signature operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LionError {
    /// Ring size is invalid (must be exactly 7).
    InvalidRingSize { expected: usize, got: usize },

    /// Index is out of bounds for the ring.
    IndexOutOfBounds { index: usize, ring_size: usize },

    /// Invalid public key encoding.
    InvalidPublicKey,

    /// Invalid secret key encoding.
    InvalidSecretKey,

    /// Invalid key image encoding.
    InvalidKeyImage,

    /// Invalid signature encoding.
    InvalidSignature,

    /// Signature verification failed.
    VerificationFailed,

    /// Invalid polynomial coefficient (out of range).
    InvalidCoefficient,

    /// NTT domain mismatch.
    NttDomainMismatch,

    /// Rejection sampling exceeded maximum iterations.
    RejectionSamplingFailed,

    /// Invalid challenge value.
    InvalidChallenge,

    /// Serialization error.
    SerializationError,

    /// Deserialization error with details.
    DeserializationError(&'static str),
}

impl fmt::Display for LionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRingSize { expected, got } => {
                write!(f, "invalid ring size: expected {expected}, got {got}")
            }
            Self::IndexOutOfBounds { index, ring_size } => {
                write!(f, "index {index} out of bounds for ring of size {ring_size}")
            }
            Self::InvalidPublicKey => write!(f, "invalid public key encoding"),
            Self::InvalidSecretKey => write!(f, "invalid secret key encoding"),
            Self::InvalidKeyImage => write!(f, "invalid key image encoding"),
            Self::InvalidSignature => write!(f, "invalid signature encoding"),
            Self::VerificationFailed => write!(f, "signature verification failed"),
            Self::InvalidCoefficient => write!(f, "polynomial coefficient out of range"),
            Self::NttDomainMismatch => write!(f, "NTT domain mismatch"),
            Self::RejectionSamplingFailed => {
                write!(f, "rejection sampling exceeded maximum iterations")
            }
            Self::InvalidChallenge => write!(f, "invalid challenge value"),
            Self::SerializationError => write!(f, "serialization error"),
            Self::DeserializationError(msg) => write!(f, "deserialization error: {msg}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LionError {}

/// Result type for Lion operations.
pub type Result<T> = core::result::Result<T, LionError>;
