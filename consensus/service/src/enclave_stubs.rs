// Copyright (c) 2018-2022 The MobileCoin Foundation
// Copyright (c) 2024 Cadence Foundation

//! Stub types that replace the SGX enclave types after enclave removal.
//!
//! With SGX removed, transactions no longer need to be encrypted or processed
//! in a trusted enclave. These types provide a compatible API surface while
//! operating entirely in untrusted space.

use displaydoc::Display;
use mc_blockchain_types::BlockIndex;
use mc_crypto_keys::{CompressedRistrettoPublic, Ed25519Public};
use mc_transaction_core::{ring_signature::KeyImage, tx::TxHash};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// A client session identifier (previously used for enclave attestation).
/// Now just a wrapper around a channel ID.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ClientSession(pub Vec<u8>);

impl From<Vec<u8>> for ClientSession {
    fn from(channel_id: Vec<u8>) -> Self {
        Self(channel_id)
    }
}

/// A peer session identifier (previously used for enclave attestation).
/// Now just a wrapper around a responder ID.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct PeerSession(pub String);

impl From<String> for PeerSession {
    fn from(responder_id: String) -> Self {
        Self(responder_id)
    }
}

impl AsRef<str> for PeerSession {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A message for a peer (previously encrypted by the enclave).
/// Now just contains the raw bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnclaveMessage<S> {
    /// The channel ID / session this message is for
    pub channel_id: S,
    /// The message data (previously encrypted, now plaintext)
    pub data: Vec<u8>,
    /// Additional authenticated data
    pub aad: Vec<u8>,
}

/// Context about a proposed transaction.
/// This is extracted from the transaction during proposal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TxContext {
    /// The serialized transaction bytes
    pub tx_bytes: Vec<u8>,
    /// The transaction hash
    pub tx_hash: TxHash,
    /// The highest TxOut indices referenced by this transaction's rings
    pub highest_indices: Vec<u64>,
    /// Key images from the transaction inputs
    pub key_images: Vec<KeyImage>,
    /// Output public keys from the transaction outputs
    pub output_public_keys: Vec<CompressedRistrettoPublic>,
}

/// A transaction that has been validated as well-formed.
/// Previously this was encrypted; now it's just the serialized bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WellFormedEncryptedTx(pub Vec<u8>);

/// Inputs for forming a new block.
/// Note: Membership proofs were removed with SGX.
#[derive(Clone, Debug, Default)]
pub struct FormBlockInputs<MTXC> {
    /// Well-formed encrypted transactions
    pub well_formed_encrypted_txs: Vec<WellFormedEncryptedTx>,
    /// Mint configuration transactions
    pub mint_config_txs: Vec<mc_transaction_core::mint::ValidatedMintConfigTx>,
    /// Mint transactions with their associated configurations
    pub mint_txs_with_config: Vec<(mc_transaction_core::mint::MintTx, MTXC)>,
}

/// Context about a well-formed transaction, used for sorting and validation.
#[derive(Clone, Debug, Default)]
pub struct WellFormedTxContext {
    /// Transaction fee (used for sorting - higher fees first)
    fee: u64,
    /// The transaction hash
    tx_hash: TxHash,
    /// Tombstone block for this transaction
    tombstone_block: BlockIndex,
    /// Key images from the transaction inputs
    key_images: Vec<KeyImage>,
    /// Highest indices referenced by the transaction rings
    highest_indices: Vec<u64>,
    /// Output public keys from the transaction outputs
    output_public_keys: Vec<CompressedRistrettoPublic>,
}

impl WellFormedTxContext {
    /// Create a new WellFormedTxContext.
    pub fn new(
        fee: u64,
        tx_hash: TxHash,
        tombstone_block: BlockIndex,
        key_images: Vec<KeyImage>,
        highest_indices: Vec<u64>,
        output_public_keys: Vec<CompressedRistrettoPublic>,
    ) -> Self {
        Self {
            fee,
            tx_hash,
            tombstone_block,
            key_images,
            highest_indices,
            output_public_keys,
        }
    }

    /// Create a WellFormedTxContext from a transaction.
    pub fn from_tx(tx: &mc_transaction_core::tx::Tx, fee: u64) -> Self {
        let tx_hash = tx.tx_hash();
        let tombstone_block = tx.prefix.tombstone_block;
        let key_images = tx
            .signature
            .ring_signatures
            .iter()
            .map(|sig| sig.key_image)
            .collect();
        let highest_indices = tx
            .prefix
            .inputs
            .iter()
            .flat_map(|input| input.ring.iter())
            .filter_map(|tx_out| {
                // In the non-enclave version, we'd need to look up indices
                // This is a simplification - real implementation would query the ledger
                None::<u64>
            })
            .collect();
        let output_public_keys = tx
            .prefix
            .outputs
            .iter()
            .map(|output| output.public_key)
            .collect();

        Self {
            fee,
            tx_hash,
            tombstone_block,
            key_images,
            highest_indices,
            output_public_keys,
        }
    }

    pub fn tx_hash(&self) -> &TxHash {
        &self.tx_hash
    }

    pub fn fee(&self) -> u64 {
        self.fee
    }

    pub fn tombstone_block(&self) -> BlockIndex {
        self.tombstone_block
    }

    pub fn key_images(&self) -> &[KeyImage] {
        &self.key_images
    }

    pub fn highest_indices(&self) -> &[u64] {
        &self.highest_indices
    }

    pub fn output_public_keys(&self) -> &[CompressedRistrettoPublic] {
        &self.output_public_keys
    }
}

impl Eq for WellFormedTxContext {}

impl PartialEq for WellFormedTxContext {
    fn eq(&self, other: &Self) -> bool {
        self.tx_hash == other.tx_hash
    }
}

impl Ord for WellFormedTxContext {
    fn cmp(&self, other: &Self) -> Ordering {
        // Sort by fee (higher first), then by tx_hash for determinism
        match other.fee.cmp(&self.fee) {
            Ordering::Equal => self.tx_hash.cmp(&other.tx_hash),
            other => other,
        }
    }
}

impl PartialOrd for WellFormedTxContext {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Errors from enclave operations.
/// Most of these are no longer possible without an enclave, but kept for API compatibility.
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum Error {
    /// Transaction validation error
    TransactionValidation(mc_transaction_core::validation::TransactionValidationError),
    /// Malformed transaction: {0}
    MalformedTx(mc_transaction_core::validation::TransactionValidationError),
    /// Serialization error: {0}
    Serialization(String),
    /// Fee map digest mismatch (legacy, no longer used)
    FeeMapDigestMismatch,
    /// Signature verification failed
    Signature,
    /// Other error: {0}
    Other(String),
}

impl std::error::Error for Error {}

impl From<mc_transaction_core::validation::TransactionValidationError> for Error {
    fn from(err: mc_transaction_core::validation::TransactionValidationError) -> Self {
        Error::TransactionValidation(err)
    }
}

/// Type alias for backwards compatibility.
pub type ConsensusEnclaveError = Error;

/// Report cache error stub (was SGX attestation report caching).
/// With SGX removed, this is no longer used but kept for API compatibility.
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum ReportCacheError {
    /// Report cache is not available (stub error)
    NotAvailable,
}

/// A map from token ID to the set of governors (signers) authorized for that token.
/// This was previously defined in the enclave API but is not SGX-specific.
#[derive(Clone, Debug, Default)]
pub struct GovernorsMap {
    inner: std::collections::HashMap<mc_transaction_core::TokenId, mc_crypto_multisig::SignerSet<Ed25519Public>>,
}

impl GovernorsMap {
    /// Create a new empty GovernorsMap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a GovernorsMap from an iterator of (TokenId, SignerSet) pairs.
    pub fn try_from_iter<I>(iter: I) -> Result<Self, GovernorsMapError>
    where
        I: IntoIterator<Item = (mc_transaction_core::TokenId, mc_crypto_multisig::SignerSet<Ed25519Public>)>,
    {
        let inner: std::collections::HashMap<_, _> = iter.into_iter().collect();
        Ok(Self { inner })
    }

    /// Get the signer set for a token ID.
    pub fn get(&self, token_id: &mc_transaction_core::TokenId) -> Option<&mc_crypto_multisig::SignerSet<Ed25519Public>> {
        self.inner.get(token_id)
    }

    /// Check if the map contains a token ID.
    pub fn contains_key(&self, token_id: &mc_transaction_core::TokenId) -> bool {
        self.inner.contains_key(token_id)
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&mc_transaction_core::TokenId, &mc_crypto_multisig::SignerSet<Ed25519Public>)> {
        self.inner.iter()
    }
}

/// Error type for GovernorsMap operations.
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum GovernorsMapError {
    /// Duplicate token ID
    DuplicateTokenId,
    /// Invalid signer set
    InvalidSignerSet,
}

/// Trait for session types.
pub trait Session: Clone + Default + Eq + std::hash::Hash + Send + Sync + 'static {}

impl Session for ClientSession {}
impl Session for PeerSession {}

/// An authentication message (stub for attestation protocol).
/// With SGX removed, this is a no-op.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthMessage {
    pub data: Vec<u8>,
}

/// The consensus enclave trait.
/// With SGX removed, this is now just transaction processing without encryption.
#[cfg_attr(test, mockall::automock)]
pub trait ConsensusEnclave: Send + Sync {
    /// Process a client's proposed transaction.
    /// Returns context extracted from the transaction.
    fn client_tx_propose(
        &self,
        msg: EnclaveMessage<ClientSession>,
    ) -> Result<TxContext, Error>;

    /// Discard a client message (used when rejecting transactions).
    fn client_discard_message(
        &self,
        msg: EnclaveMessage<ClientSession>,
    ) -> Result<(), Error>;

    /// Check if a transaction is well-formed.
    /// Note: Membership proofs were removed with SGX.
    fn tx_is_well_formed(
        &self,
        tx_bytes: Vec<u8>,
        current_block_index: u64,
    ) -> Result<(WellFormedEncryptedTx, WellFormedTxContext), Error>;

    /// Prepare transactions for sending to a peer.
    fn txs_for_peer(
        &self,
        encrypted_txs: &[WellFormedEncryptedTx],
        aad: &[u8],
        peer: &PeerSession,
    ) -> Result<EnclaveMessage<PeerSession>, Error>;

    /// Get the minting trust root public key.
    fn get_minting_trust_root(&self) -> Result<Ed25519Public, Error>;

    /// Get the block signer public key.
    fn get_signer(&self) -> Result<Ed25519Public, Error>;

    /// Accept an authentication request from a peer (stub - no-op without SGX).
    fn peer_accept(&self, req: AuthMessage) -> Result<(AuthMessage, PeerSession), Error>;

    /// Accept an authentication request from a client (stub - no-op without SGX).
    fn client_accept(&self, req: AuthMessage) -> Result<(AuthMessage, ClientSession), Error>;

    /// Form a new block from the given inputs.
    /// Note: Membership proofs were removed with SGX.
    fn form_block<MTXC: Clone>(
        &self,
        parent_block: &mc_blockchain_types::Block,
        inputs: FormBlockInputs<MTXC>,
    ) -> Result<
        (
            mc_blockchain_types::Block,
            mc_blockchain_types::BlockContents,
            mc_blockchain_types::BlockSignature,
        ),
        Error,
    >;

    /// Get DCAP evidence (stub - returns None without SGX).
    fn get_dcap_evidence(&self) -> Result<Option<()>, Error> {
        Ok(None)
    }
}

/// The AttestedApi trait (stub for attestation gRPC service).
/// With SGX removed, this is primarily a no-op service.
pub trait AttestedApi {
    fn auth(
        &mut self,
        ctx: grpcio::RpcContext,
        request: AuthMessage,
        sink: grpcio::UnarySink<AuthMessage>,
    );
}
