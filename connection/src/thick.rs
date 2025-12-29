// Copyright (c) 2018-2023 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Connection implementations for the thick client using tonic gRPC.
//! Post-SGX simplified version - no attestation required.

#![allow(clippy::result_large_err)]
use crate::{
    credentials::{AuthenticationError, CredentialsProvider, CredentialsProviderError},
    error::{Error, Result},
    traits::{
        AttestationError, AttestedConnection, BlockInfo, BlockchainConnection, Connection,
        UserTxConnection,
    },
};
use displaydoc::Display;
use bth_blockchain_types::{Block, BlockID, BlockIndex};
use bth_common::{
    logger::{o, Logger},
    trace_time,
};
use bth_consensus_api::{
    consensus_client::consensus_client_api_client::ConsensusClientApiClient,
    consensus_common::{
        blockchain_api_client::BlockchainApiClient, BlocksRequest, ProposeTxResult,
    },
    attest::Message as AttestMessage,
};
use bth_transaction_core::tx::Tx;
use bth_util_serial::encode;
use bth_util_uri::{ConnectionUri, ConsensusClientUri as ClientUri, UriConversionError};
use std::{
    cmp::Ordering,
    fmt::{Display, Formatter, Result as FmtResult},
    hash::{Hash, Hasher},
    ops::Range,
    result::Result as StdResult,
    sync::Arc,
};
use tokio::runtime::Runtime;
use tonic::{
    transport::{Channel, Endpoint},
    Request, Status,
};

/// Evidence kind placeholder (attestation no longer used)
#[derive(Clone, Debug)]
pub enum EvidenceKind {
    /// No attestation needed
    None,
}

/// Attestation failures a thick client can generate
#[derive(Debug, Display)]
pub enum ThickClientAttestationError {
    /// gRPC failure: {0}
    Grpc(Status),
    /// Could not create ResponderID from URI {0}: {1}
    InvalidResponderID(String, UriConversionError),
    /// Unexpected Error Converting URI {0}
    UriConversionError(UriConversionError),
    /// Credentials provider error: {0}
    CredentialsProvider(Box<dyn CredentialsProviderError + 'static>),
    /// Transport error: {0}
    Transport(String),
    /// Other: {0}
    Other(String),
}

impl From<Status> for ThickClientAttestationError {
    fn from(src: Status) -> Self {
        ThickClientAttestationError::Grpc(src)
    }
}

impl From<tonic::transport::Error> for ThickClientAttestationError {
    fn from(src: tonic::transport::Error) -> Self {
        ThickClientAttestationError::Transport(src.to_string())
    }
}

impl From<UriConversionError> for ThickClientAttestationError {
    fn from(src: UriConversionError) -> Self {
        match &src {
            UriConversionError::ResponderId(uri, _err) => {
                ThickClientAttestationError::InvalidResponderID(uri.clone(), src)
            }
            _ => ThickClientAttestationError::UriConversionError(src),
        }
    }
}

impl From<Box<dyn CredentialsProviderError + 'static>> for ThickClientAttestationError {
    fn from(src: Box<dyn CredentialsProviderError + 'static>) -> Self {
        Self::CredentialsProvider(src)
    }
}

impl AuthenticationError for ThickClientAttestationError {
    fn is_unauthenticated(&self) -> bool {
        match self {
            Self::Grpc(status) => status.code() == tonic::Code::Unauthenticated,
            _ => false,
        }
    }
}

impl AttestationError for ThickClientAttestationError {
    fn should_reattest(&self) -> bool {
        // No attestation in post-SGX version
        false
    }

    fn should_retry(&self) -> bool {
        match self {
            Self::Grpc(_) | Self::CredentialsProvider(_) | Self::Transport(_) => true,
            Self::InvalidResponderID(_, _) | Self::UriConversionError(_) => false,
            Self::Other(_) => false,
        }
    }
}

/// A connection from a client to a consensus node.
/// Post-SGX simplified version - uses direct gRPC without attestation.
pub struct ThickClient<CP: CredentialsProvider> {
    /// The chain id. This is used with the chain-id grpc header if provided.
    chain_id: String,
    /// The destination's URI
    uri: ClientUri,
    /// The logging instance
    logger: Logger,
    /// The gRPC API client for blockchain detail retrieval.
    blockchain_api_client: BlockchainApiClient<Channel>,
    /// The gRPC API client for transaction submission.
    consensus_client_api_client: ConsensusClientApiClient<Channel>,
    /// Generic interface for retrieving GRPC credentials.
    credentials_provider: CP,
    /// Tokio runtime for blocking on async calls
    runtime: Arc<Runtime>,
}

impl<CP: CredentialsProvider> ThickClient<CP> {
    /// Create a new connection to the given consensus node.
    pub fn new(
        chain_id: String,
        uri: ClientUri,
        _identities: impl Into<Vec<()>>, // Unused, kept for API compat
        credentials_provider: CP,
        logger: Logger,
    ) -> Result<Self> {
        let logger = logger.new(o!("mc.cxn" => uri.to_string()));

        // Create tokio runtime for blocking
        let runtime = Arc::new(
            Runtime::new().map_err(|e| Error::Other(format!("Failed to create runtime: {}", e)))?
        );

        // Build the gRPC endpoint
        let endpoint_uri = format!("http://{}:{}", uri.host(), uri.port());

        let channel = runtime.block_on(async {
            Endpoint::from_shared(endpoint_uri)
                .map_err(|e| Error::Other(format!("Invalid endpoint: {}", e)))?
                .connect()
                .await
                .map_err(|e| Error::Other(format!("Failed to connect: {}", e)))
        })?;

        let blockchain_api_client = BlockchainApiClient::new(channel.clone());
        let consensus_client_api_client = ConsensusClientApiClient::new(channel);

        Ok(Self {
            chain_id,
            uri,
            logger,
            blockchain_api_client,
            consensus_client_api_client,
            credentials_provider,
            runtime,
        })
    }

    /// Block on an async gRPC call
    fn block_on<F, T>(&self, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(future)
    }

    fn handle_rpc_error(&mut self, err: &(impl AuthenticationError + AttestationError)) {
        if err.is_unauthenticated() {
            self.credentials_provider.clear();
        }
    }
}

impl<CP: CredentialsProvider> Connection for ThickClient<CP> {
    type Uri = ClientUri;

    fn uri(&self) -> Self::Uri {
        self.uri.clone()
    }
}

impl<CP: CredentialsProvider> AttestedConnection for ThickClient<CP> {
    type Error = ThickClientAttestationError;

    fn is_attested(&self) -> bool {
        // No attestation needed post-SGX
        true
    }

    fn attest(&mut self) -> StdResult<EvidenceKind, Self::Error> {
        // No attestation needed post-SGX
        Ok(EvidenceKind::None)
    }

    fn deattest(&mut self) {
        // No-op post-SGX
    }
}

impl<CP: CredentialsProvider> BlockchainConnection for ThickClient<CP> {
    fn fetch_blocks(&mut self, range: Range<BlockIndex>) -> Result<Vec<Block>> {
        trace_time!(self.logger, "ThickClient::get_blocks");

        let request = BlocksRequest {
            offset: range.start,
            limit: (range.end - range.start)
                .try_into()
                .or(Err(Error::RequestTooLarge))?,
        };

        let mut client = self.blockchain_api_client.clone();
        let response = self.block_on(async {
            client.get_blocks(Request::new(request)).await
        })?;

        response
            .into_inner()
            .blocks
            .iter()
            .map(|proto_block| Block::try_from(proto_block).map_err(Error::from))
            .collect::<Result<Vec<Block>>>()
    }

    fn fetch_block_ids(&mut self, range: Range<BlockIndex>) -> Result<Vec<BlockID>> {
        self.fetch_blocks(range)?
            .iter()
            .map(|block| Ok(block.id.clone()))
            .collect()
    }

    fn fetch_block_height(&mut self) -> Result<BlockIndex> {
        Ok(self.fetch_block_info()?.block_index)
    }

    fn fetch_block_info(&mut self) -> Result<BlockInfo> {
        trace_time!(self.logger, "ThickClient::fetch_block_info");

        let mut client = self.blockchain_api_client.clone();
        let response = self.block_on(async {
            client.get_last_block_info(Request::new(())).await
        })?;

        Ok(BlockInfo::from(response.into_inner()))
    }
}

impl<CP: CredentialsProvider> UserTxConnection for ThickClient<CP> {
    fn propose_tx(&mut self, tx: &Tx) -> Result<u64> {
        trace_time!(self.logger, "ThickClient::propose_tx");

        // Encode the transaction
        let tx_bytes = encode(tx);

        // Create the message (post-SGX: no encryption needed)
        let msg = AttestMessage {
            channel_id: vec![],
            data: tx_bytes,
            aad: vec![],
        };

        let mut client = self.consensus_client_api_client.clone();
        let response = self.block_on(async {
            client.client_tx_propose(Request::new(msg)).await
        })?;

        let response = response.into_inner();

        // Check the result
        let result = ProposeTxResult::try_from(response.result)
            .unwrap_or(ProposeTxResult::Ok);

        if result != ProposeTxResult::Ok {
            return Err(Error::TransactionValidation(result, response.err_msg));
        }

        Ok(response.block_count)
    }
}

impl<CP: CredentialsProvider> Display for ThickClient<CP> {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "{}", self.uri)
    }
}

impl<CP: CredentialsProvider> Eq for ThickClient<CP> {}

impl<CP: CredentialsProvider> Hash for ThickClient<CP> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.uri.addr().hash(hasher);
    }
}

impl<CP: CredentialsProvider> Ord for ThickClient<CP> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.uri.addr().cmp(&other.uri.addr())
    }
}

impl<CP: CredentialsProvider> PartialEq for ThickClient<CP> {
    fn eq(&self, other: &Self) -> bool {
        self.uri.addr() == other.uri.addr()
    }
}

impl<CP: CredentialsProvider> PartialOrd for ThickClient<CP> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
