use crate::enclave_stubs::Error as EnclaveError;
use displaydoc::Display;
use mc_transaction_core::tx::TxHash;

/// Errors experienced when handling PeerAPI requests.
#[derive(Debug, Display)]
pub enum PeerServiceError {
    /// Unknown peer `{0}`.
    UnknownPeer(String),

    /// The ConsensusMsg's signature is invalid.
    ConsensusMsgInvalidSignature,

    /// Unknown transactions `{0:?}`.
    UnknownTransactions(Vec<TxHash>),

    /// Enclave-related error `{0}`.
    Enclave(EnclaveError),

    /// Something went wrong...
    InternalError,
}
