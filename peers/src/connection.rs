// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Peer-to-Peer Networking - Post-SGX simplified version.

use crate::{
    consensus_msg::ConsensusMsg,
    error::{Error, PeerAttestationError, Result},
    traits::ConsensusConnection,
};
use core::fmt::{Display, Formatter, Result as FmtResult};
use bth_blockchain_types::{Block, BlockID, BlockIndex};
use bth_common::{
    logger::{o, Logger},
    trace_time, NodeID, ResponderId,
};
use bth_connection::{
    AttestedConnection, BlockInfo, BlockchainConnection, Connection, EvidenceKind,
    Error as ConnectionError, Result as ConnectionResult,
};
use bth_consensus_api::{
    consensus_common::{
        blockchain_api_client::BlockchainApiClient, BlocksRequest,
    },
    consensus_peer::{
        consensus_peer_api_client::ConsensusPeerApiClient,
        ConsensusMsg as GrpcConsensusMsg, ConsensusMsgResponse,
        GetTxsRequest as GrpcFetchTxsRequest,
    },
};
use bth_transaction_core::tx::TxHash;
use bth_util_serial::{deserialize, serialize};
use bth_util_uri::{ConnectionUri, ConsensusPeerUri as PeerUri};
use std::{
    cmp::Ordering,
    hash::{Hash, Hasher},
    ops::Range,
    result::Result as StdResult,
    sync::Arc,
};
use tokio::runtime::Runtime;
use tonic::{
    transport::{Channel, Endpoint},
    Request,
};

/// A peer connection for consensus communication.
/// Post-SGX simplified version - no attestation required.
pub struct PeerConnection {
    /// The local node ID
    local_node_id: NodeID,

    /// The remote node's responder ID
    remote_responder_id: ResponderId,

    /// The remote node's URI.
    uri: PeerUri,

    /// The logger instance.
    logger: Logger,

    /// The gRPC client for consensus peer API.
    consensus_api_client: ConsensusPeerApiClient<Channel>,

    /// The gRPC client for blockchain API.
    blockchain_api_client: BlockchainApiClient<Channel>,

    /// Tokio runtime for blocking on async calls.
    runtime: Arc<Runtime>,
}

impl PeerConnection {
    /// Construct a new PeerConnection.
    pub fn new(
        local_node_id: NodeID,
        uri: PeerUri,
        logger: Logger,
    ) -> Result<Self> {
        let remote_responder_id = uri.responder_id().unwrap_or_else(|_| {
            panic!("Could not get responder id from uri {:?}", uri.to_string())
        });
        let host_port = uri.addr();

        let logger = logger.new(o!("mc.peers.addr" => host_port.clone()));

        // Create tokio runtime for blocking
        let runtime = Arc::new(
            Runtime::new().map_err(|e| Error::Other)?
        );

        // Build the gRPC endpoint
        let endpoint_uri = format!("http://{}", host_port);

        let channel = runtime.block_on(async {
            Endpoint::from_shared(endpoint_uri)
                .map_err(|_| Error::Other)?
                .connect()
                .await
                .map_err(|_| Error::Other)
        })?;

        let consensus_api_client = ConsensusPeerApiClient::new(channel.clone());
        let blockchain_api_client = BlockchainApiClient::new(channel);

        Ok(Self {
            local_node_id,
            remote_responder_id,
            uri,
            logger,
            consensus_api_client,
            blockchain_api_client,
            runtime,
        })
    }

    /// Get the remote responder ID.
    pub fn remote_responder_id(&self) -> ResponderId {
        self.remote_responder_id.clone()
    }

    /// Get the local node ID.
    pub fn local_node_id(&self) -> &NodeID {
        &self.local_node_id
    }

    /// Block on an async gRPC call.
    fn block_on<F, T>(&self, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(future)
    }
}

impl Display for PeerConnection {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "{}", self.uri)
    }
}

impl Eq for PeerConnection {}

impl Hash for PeerConnection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uri.addr().hash(state);
    }
}

impl Ord for PeerConnection {
    fn cmp(&self, other: &Self) -> Ordering {
        self.uri.addr().cmp(&other.uri.addr())
    }
}

impl PartialEq for PeerConnection {
    fn eq(&self, other: &Self) -> bool {
        self.uri.addr() == other.uri.addr()
    }
}

impl PartialOrd for PeerConnection {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Connection for PeerConnection {
    type Uri = PeerUri;

    fn uri(&self) -> Self::Uri {
        self.uri.clone()
    }
}

impl AttestedConnection for PeerConnection {
    type Error = PeerAttestationError;

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

impl BlockchainConnection for PeerConnection {
    fn fetch_blocks(&mut self, range: Range<BlockIndex>) -> ConnectionResult<Vec<Block>> {
        trace_time!(self.logger, "PeerConnection::get_blocks");

        let request = BlocksRequest {
            offset: range.start,
            limit: u32::try_from(range.end - range.start)
                .or(Err(ConnectionError::RequestTooLarge))?,
        };

        let mut client = self.blockchain_api_client.clone();
        let response = self.block_on(async {
            client.get_blocks(Request::new(request)).await
        }).map_err(|e| ConnectionError::Other(e.to_string()))?;

        response
            .into_inner()
            .blocks
            .iter()
            .map(|proto_block| Block::try_from(proto_block).map_err(ConnectionError::from))
            .collect::<ConnectionResult<Vec<Block>>>()
    }

    fn fetch_block_ids(&mut self, range: Range<BlockIndex>) -> ConnectionResult<Vec<BlockID>> {
        self.fetch_blocks(range)?
            .iter()
            .map(|block| Ok(block.id.clone()))
            .collect()
    }

    fn fetch_block_height(&mut self) -> ConnectionResult<BlockIndex> {
        Ok(self.fetch_block_info()?.block_index)
    }

    fn fetch_block_info(&mut self) -> ConnectionResult<BlockInfo> {
        trace_time!(self.logger, "PeerConnection::fetch_block_info");

        let mut client = self.blockchain_api_client.clone();
        let response = self.block_on(async {
            client.get_last_block_info(Request::new(())).await
        }).map_err(|e| ConnectionError::Other(e.to_string()))?;

        Ok(BlockInfo::from(response.into_inner()))
    }
}

impl ConsensusConnection for PeerConnection {
    fn remote_responder_id(&self) -> ResponderId {
        self.remote_responder_id.clone()
    }

    fn local_node_id(&self) -> NodeID {
        self.local_node_id.clone()
    }

    fn send_consensus_msg(&mut self, msg: &ConsensusMsg) -> Result<ConsensusMsgResponse> {
        trace_time!(self.logger, "PeerConnection::send_consensus_msg");

        let serialized_msg = serialize(msg)?;

        let grpc_msg = GrpcConsensusMsg {
            from_responder_id: self.local_node_id.responder_id.to_string(),
            payload: serialized_msg,
        };

        let mut client = self.consensus_api_client.clone();
        let response = self.block_on(async {
            client.send_consensus_msg(Request::new(grpc_msg)).await
        }).map_err(Error::Grpc)?;

        Ok(response.into_inner())
    }

    fn fetch_txs(&mut self, hashes: &[TxHash]) -> Result<Vec<ConsensusMsg>> {
        trace_time!(self.logger, "PeerConnection::fetch_txs");

        // Post-SGX: channel_id is empty (no encryption needed)
        let request = GrpcFetchTxsRequest {
            channel_id: vec![],
            tx_hashes: hashes.iter().map(|h| h.to_vec()).collect(),
        };

        let mut client = self.consensus_api_client.clone();
        let response = self.block_on(async {
            client.get_txs(Request::new(request)).await
        }).map_err(Error::Grpc)?;

        let response = response.into_inner();

        // Parse the response payload - post-SGX returns data directly
        match response.payload {
            Some(bth_consensus_api::consensus_peer::get_txs_response::Payload::Success(msg)) => {
                // The data field contains serialized ConsensusMsg items
                deserialize(&msg.data).map_err(|_| Error::Serialization)
            }
            Some(bth_consensus_api::consensus_peer::get_txs_response::Payload::TxHashesNotInCache(not_found)) => {
                let missing: Vec<TxHash> = not_found.tx_hashes
                    .iter()
                    .filter_map(|h| TxHash::try_from(h.as_slice()).ok())
                    .collect();
                Err(Error::TxHashesNotInCache(missing))
            }
            None => Ok(vec![]),
        }
    }
}
