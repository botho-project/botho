// Copyright (c) 2018-2022 The Botho Foundation

use crate::{InputRuleError, TxOutConversionError};
use alloc::string::String;
use displaydoc::Display;
use bth_crypto_keys::KeyError;
use serde::{Deserialize, Serialize};

/// Type alias for transaction validation results.
pub type TransactionValidationResult<T> = Result<T, TransactionValidationError>;

/// Reasons why a single transaction may fail to be valid with respect to the
/// current ledger.
#[derive(Clone, Debug, Display, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum TransactionValidationError {
    /// Each input should have one membership proof.
    InputsProofsLengthMismatch,

    /// A transaction must have at least one input.
    NoInputs,

    /**
     * A transaction must have no more than the maximum allowed number of
     * inputs.
     */
    TooManyInputs,

    /// Each input must have a signature.
    InsufficientInputSignatures,

    /// Each input must have a valid signature.
    InvalidInputSignature,

    /// Invalid RingCT signature: `{0}`
    InvalidTransactionSignature(crate::ring_ct::Error),

    /// All Range Proofs in the transaction must be valid.
    InvalidRangeProof,

    /**
     * Each input must contain a ring with no fewer than the minimum number
     * of elements.
     */
    InsufficientRingSize,

    /// Number of blocks in ledger exceeds the tombstone block number.
    TombstoneBlockExceeded,

    /// Tombstone block is too far in the future.
    TombstoneBlockTooFar,

    /// Must have at least one output.
    NoOutputs,

    /**
     * A transaction must have no more than the maximum allowed number of
     * outputs.
     */
    TooManyOutputs,

    /**
     * Each input must contain a ring with no more than the maximum number
     * of elements.
     */
    ExcessiveRingSize,

    /// All elements in all rings within the transaction must be unique.
    DuplicateRingElements,

    /// The elements of each ring must be sorted.
    UnsortedRingElements,

    /// All rings in a transaction must be of the same size.
    UnequalRingSizes,

    /**
     * Inputs must be sorted by the public key of the first ring element of
     * each input.
     */
    UnsortedInputs,

    /// Key Images must be sorted.
    UnsortedKeyImages,

    /// Contains a Key Image that has previously been spent.
    ContainsSpentKeyImage,

    /// Key Images within the transaction must be unique.
    DuplicateKeyImages,

    /// Output public keys in the transaction must be unique.
    DuplicateOutputPublicKey,

    /**
     * Contains an output public key that has previously appeared in the
     * ledger.
     */
    ContainsExistingOutputPublicKey,

    /// Each ring element must have a corresponding proof of membership.
    MissingTxOutMembershipProof,

    /// Each ring element must have a valid proof of membership.
    InvalidTxOutMembershipProof,

    /// Public keys must be valid Ristretto points.
    InvalidRistrettoPublicKey,

    /**
     * Ledger context provided by the untrusted system is insufficient to
     *  validate the transaction.
     */
    InvalidLedgerContext,

    /// Ledger error: `{0}`.
    Ledger(String),

    /// Ledger error: TxOut Index out of bounds: {0}
    LedgerTxOutIndexOutOfBounds(u64),

    /// An error occurred while validating a membership proof.
    MembershipProofValidationError,

    /// An error occurred while checking transaction fees.
    TxFeeError,

    /// Public keys must be valid Ristretto points.
    KeyError,

    /// A TxOut is missing the required memo field
    MissingMemo,

    /// A TxOut includes a memo, but this is not allowed yet
    MemosNotAllowed,

    /// Tx indicates a token id that is not yet configured
    TokenNotYetConfigured,

    /// A TxOut is missing the required masked token id field
    MissingMaskedTokenId,

    /// A TxOut includes a masked token id, but this is not allowed yet
    MaskedTokenIdNotAllowed,

    /// Outputs must be sorted by public key, ascending
    UnsortedOutputs,

    /// Input rules not yet allowed
    InputRulesNotAllowed,

    /// Input rule: {0}
    InputRule(InputRuleError),

    /// Unknown Masked Amount version
    UnknownMaskedAmountVersion,

    // =========== Cluster Tag Errors ===========
    /// Cluster tags are required but missing from TxOut
    MissingClusterTags,

    /// Cluster tags are not allowed yet for this block version
    ClusterTagsNotAllowed,

    /// Cluster tag vector is malformed (invalid weights, duplicates, etc.)
    InvalidClusterTags,

    /// Cluster tag mass inflation: cluster {0} has output mass {1} > expected {2}
    ClusterTagInflation(u64, u64, u64),

    /// Insufficient progressive fee: required {0}, actual {1}
    InsufficientProgressiveFee(u64, u64),

    // =========== Phase 2 Committed Tag Errors ===========
    /// Committed tag inheritance proof is invalid
    InvalidTagInheritanceProof,

    /// Tag conservation proof is invalid
    InvalidTagConservationProof,

    /// Extended tag signature is missing when committed tags are used
    MissingExtendedTagSignature,

    /// Wrong number of pseudo-tag-outputs in signature
    PseudoTagOutputCountMismatch,

    // =========== Quantum-Private Transaction Errors ===========
    /// Classical (Schnorr) signature verification failed for quantum-private input
    #[cfg(feature = "pq")]
    QuantumPrivateSchnorrVerificationFailed,

    /// Post-quantum (Dilithium) signature verification failed for quantum-private input
    #[cfg(feature = "pq")]
    QuantumPrivateDilithiumVerificationFailed,

    /// Invalid ML-KEM ciphertext in quantum-private output
    #[cfg(feature = "pq")]
    InvalidPqCiphertext,

    /// Invalid ML-DSA public key in quantum-private output
    #[cfg(feature = "pq")]
    InvalidPqPublicKey,

    /// Quantum-private transaction missing required PQ signature
    #[cfg(feature = "pq")]
    MissingPqSignature,

    /// Quantum-private input references invalid output
    #[cfg(feature = "pq")]
    InvalidPqOutputReference,
}

impl From<bth_crypto_keys::KeyError> for TransactionValidationError {
    fn from(_src: KeyError) -> Self {
        Self::KeyError
    }
}

impl From<InputRuleError> for TransactionValidationError {
    fn from(src: InputRuleError) -> Self {
        Self::InputRule(src)
    }
}

impl From<TxOutConversionError> for TransactionValidationError {
    fn from(src: TxOutConversionError) -> Self {
        match src {
            TxOutConversionError::UnknownMaskedAmountVersion => Self::UnknownMaskedAmountVersion,
        }
    }
}
