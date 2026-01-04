// Copyright (c) 2024 Botho Foundation

//! libp2p network behaviour for the gossip protocol.
//!
//! This module implements the core networking using libp2p:
//! - Gossipsub for push-based announcement propagation
//! - Kademlia DHT for peer discovery
//! - Request-response for pull-based topology sync

use crate::{
    config::GossipConfig,
    error::{GossipError, GossipResult},
    messages::{
        BlockBroadcast, NodeAnnouncement, OnionRelayMessage, TransactionBroadcast,
        ANNOUNCEMENTS_TOPIC, BLOCKS_TOPIC, ONION_RELAY_TOPIC, TOPOLOGY_SYNC_PROTOCOL,
        TRANSACTIONS_TOPIC,
    },
};
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    identify,
    kad::{self, store::MemoryStore},
    request_response::{self, Codec, ProtocolSupport},
    swarm::NetworkBehaviour,
    Multiaddr, PeerId,
};
use std::{io, time::Duration};
use tokio::sync::mpsc;

/// Events emitted by the gossip behaviour.
#[derive(Debug)]
pub enum GossipEvent {
    /// A new peer was discovered
    PeerDiscovered(PeerId),

    /// A peer disconnected
    PeerDisconnected(PeerId),

    /// Received a node announcement
    AnnouncementReceived(NodeAnnouncement),

    /// Received a transaction broadcast
    TransactionReceived(TransactionBroadcast),

    /// Received a block broadcast
    BlockReceived(BlockBroadcast),

    /// Received a topology sync request
    TopologySyncRequest { peer: PeerId, since_timestamp: u64 },

    /// Received a topology sync response
    TopologySyncResponse {
        peer: PeerId,
        announcements: Vec<NodeAnnouncement>,
    },

    /// Bootstrap completed
    Bootstrapped,

    /// Error occurred
    Error(GossipError),
}

/// Request-response codec for topology sync.
#[derive(Debug, Clone, Default)]
pub struct TopologySyncCodec;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopologySyncRequest {
    pub since_timestamp: u64,
    pub max_results: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopologySyncResponse {
    pub announcements: Vec<NodeAnnouncement>,
    pub has_more: bool,
}

impl Codec for TopologySyncCodec {
    type Protocol = &'static str;
    type Request = TopologySyncRequest;
    type Response = TopologySyncResponse;

    fn read_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = io::Result<Self::Request>> + Send + 'async_trait>,
    >
    where
        T: futures::AsyncRead + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            use futures::AsyncReadExt;
            let mut buf = Vec::new();
            io.read_to_end(&mut buf).await?;
            serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
    }

    fn read_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = io::Result<Self::Response>> + Send + 'async_trait>,
    >
    where
        T: futures::AsyncRead + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            use futures::AsyncReadExt;
            let mut buf = Vec::new();
            io.read_to_end(&mut buf).await?;
            serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
    }

    fn write_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        req: Self::Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        T: futures::AsyncWrite + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            use futures::AsyncWriteExt;
            let bytes = serde_json::to_vec(&req)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            io.write_all(&bytes).await?;
            io.close().await?;
            Ok(())
        })
    }

    fn write_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        resp: Self::Response,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        T: futures::AsyncWrite + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            use futures::AsyncWriteExt;
            let bytes = serde_json::to_vec(&resp)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            io.write_all(&bytes).await?;
            io.close().await?;
            Ok(())
        })
    }
}

/// Combined network behaviour for gossip.
#[derive(NetworkBehaviour)]
pub struct GossipBehaviour {
    /// Gossipsub for pub/sub messaging
    pub gossipsub: gossipsub::Behaviour,

    /// Kademlia DHT for peer discovery
    pub kademlia: kad::Behaviour<MemoryStore>,

    /// Identify protocol for peer identification
    pub identify: identify::Behaviour,

    /// Request-response for topology sync
    pub topology_sync: request_response::Behaviour<TopologySyncCodec>,
}

impl GossipBehaviour {
    /// Create a new gossip behaviour.
    pub fn new(local_peer_id: PeerId, config: &GossipConfig) -> GossipResult<Self> {
        // Configure gossipsub
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(ValidationMode::Strict)
            .message_id_fn(|msg| {
                // Use content hash as message ID for deduplication
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                std::hash::Hash::hash(&msg.data, &mut hasher);
                std::hash::Hash::hash(&msg.topic, &mut hasher);
                gossipsub::MessageId::from(std::hash::Hasher::finish(&hasher).to_string())
            })
            .build()
            .map_err(|e| GossipError::Libp2pError(e.to_string()))?;

        let gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(libp2p::identity::Keypair::generate_ed25519()),
            gossipsub_config,
        )
        .map_err(|e| GossipError::Libp2pError(e.to_string()))?;

        // Configure Kademlia
        let kademlia = kad::Behaviour::new(local_peer_id, MemoryStore::new(local_peer_id));

        // Configure identify with network-specific protocol version
        let protocol_version = config.network_id.protocol_version();
        let identify = identify::Behaviour::new(identify::Config::new(
            protocol_version,
            libp2p::identity::Keypair::generate_ed25519().public(),
        ));

        // Configure request-response
        let topology_sync = request_response::Behaviour::new(
            [(TOPOLOGY_SYNC_PROTOCOL, ProtocolSupport::Full)],
            request_response::Config::default().with_request_timeout(config.request_timeout()),
        );

        Ok(Self {
            gossipsub,
            kademlia,
            identify,
            topology_sync,
        })
    }

    /// Subscribe to the announcements topic.
    pub fn subscribe_announcements(&mut self) -> GossipResult<()> {
        let topic = IdentTopic::new(ANNOUNCEMENTS_TOPIC);
        self.gossipsub
            .subscribe(&topic)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to subscribe: {:?}", e)))?;
        Ok(())
    }

    /// Subscribe to the transactions topic.
    pub fn subscribe_transactions(&mut self) -> GossipResult<()> {
        let topic = IdentTopic::new(TRANSACTIONS_TOPIC);
        self.gossipsub
            .subscribe(&topic)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to subscribe: {:?}", e)))?;
        Ok(())
    }

    /// Subscribe to the blocks topic.
    pub fn subscribe_blocks(&mut self) -> GossipResult<()> {
        let topic = IdentTopic::new(BLOCKS_TOPIC);
        self.gossipsub
            .subscribe(&topic)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to subscribe: {:?}", e)))?;
        Ok(())
    }

    /// Publish a node announcement to the network.
    pub fn publish_announcement(&mut self, announcement: &NodeAnnouncement) -> GossipResult<()> {
        let topic = IdentTopic::new(ANNOUNCEMENTS_TOPIC);
        let data = serde_json::to_vec(announcement)
            .map_err(|e| GossipError::SerializationError(e.to_string()))?;

        self.gossipsub
            .publish(topic, data)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to publish: {:?}", e)))?;

        Ok(())
    }

    /// Publish a transaction to the network.
    pub fn publish_transaction(&mut self, tx_broadcast: &TransactionBroadcast) -> GossipResult<()> {
        let topic = IdentTopic::new(TRANSACTIONS_TOPIC);
        let data = serde_json::to_vec(tx_broadcast)
            .map_err(|e| GossipError::SerializationError(e.to_string()))?;

        self.gossipsub
            .publish(topic, data)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to publish: {:?}", e)))?;

        Ok(())
    }

    /// Publish a block to the network.
    pub fn publish_block(&mut self, block_broadcast: &BlockBroadcast) -> GossipResult<()> {
        let topic = IdentTopic::new(BLOCKS_TOPIC);
        let data = serde_json::to_vec(block_broadcast)
            .map_err(|e| GossipError::SerializationError(e.to_string()))?;

        self.gossipsub
            .publish(topic, data)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to publish: {:?}", e)))?;

        Ok(())
    }

    /// Publish an onion relay message to the network.
    ///
    /// This is used by exit nodes to broadcast transactions or by relays to
    /// forward messages to the next hop via gossipsub.
    pub fn publish_onion_relay(&mut self, msg: &OnionRelayMessage) -> GossipResult<()> {
        let topic = IdentTopic::new(ONION_RELAY_TOPIC);
        let data =
            serde_json::to_vec(msg).map_err(|e| GossipError::SerializationError(e.to_string()))?;

        self.gossipsub
            .publish(topic, data)
            .map_err(|e| GossipError::Libp2pError(format!("Failed to publish: {:?}", e)))?;

        Ok(())
    }

    /// Add a peer to Kademlia for discovery.
    pub fn add_peer(&mut self, peer_id: PeerId, addr: Multiaddr) {
        self.kademlia.add_address(&peer_id, addr);
    }

    /// Bootstrap Kademlia by finding closest peers.
    pub fn bootstrap(&mut self) -> GossipResult<()> {
        self.kademlia
            .bootstrap()
            .map_err(|e| GossipError::Libp2pError(format!("Bootstrap failed: {:?}", e)))?;
        Ok(())
    }

    /// Request topology from a peer.
    pub fn request_topology(&mut self, peer: PeerId, since_timestamp: u64, max_results: u32) {
        let request = TopologySyncRequest {
            since_timestamp,
            max_results,
        };
        self.topology_sync.send_request(&peer, request);
    }
}

/// Handle for controlling the gossip network.
#[derive(Clone)]
pub struct GossipHandle {
    /// Channel to send commands to the swarm
    command_tx: mpsc::Sender<GossipCommand>,
}

/// Commands that can be sent to the gossip swarm.
#[derive(Debug)]
pub enum GossipCommand {
    /// Publish a node announcement
    Announce(NodeAnnouncement),

    /// Broadcast a transaction
    BroadcastTransaction(TransactionBroadcast),

    /// Broadcast a block
    BroadcastBlock(BlockBroadcast),

    /// Send an onion relay message (for private transaction routing)
    SendOnionRelay(OnionRelayMessage),

    /// Request topology from a peer
    RequestTopology { peer: PeerId, since_timestamp: u64 },

    /// Add a bootstrap peer
    AddBootstrapPeer(PeerId, Multiaddr),

    /// Dial a peer
    Dial(Multiaddr),

    /// Get connected peers (response sent via channel)
    GetPeers(mpsc::Sender<Vec<PeerId>>),

    /// Shutdown the swarm
    Shutdown,
}

impl GossipHandle {
    /// Create a new handle.
    pub fn new(command_tx: mpsc::Sender<GossipCommand>) -> Self {
        Self { command_tx }
    }

    /// Publish a node announcement.
    pub async fn announce(&self, announcement: NodeAnnouncement) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::Announce(announcement))
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Broadcast a transaction to the network.
    pub async fn broadcast_transaction(&self, tx: TransactionBroadcast) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::BroadcastTransaction(tx))
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Broadcast a block to the network.
    pub async fn broadcast_block(&self, block: BlockBroadcast) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::BroadcastBlock(block))
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Send an onion relay message to the network.
    ///
    /// This is used for privacy-preserving transaction broadcast. The message
    /// is published to the onion relay gossipsub topic where relays and exit
    /// nodes can process it.
    pub async fn send_onion_relay(&self, msg: OnionRelayMessage) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::SendOnionRelay(msg))
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Request topology from a peer.
    pub async fn request_topology(&self, peer: PeerId, since_timestamp: u64) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::RequestTopology {
                peer,
                since_timestamp,
            })
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Add a bootstrap peer.
    pub async fn add_bootstrap_peer(&self, peer: PeerId, addr: Multiaddr) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::AddBootstrapPeer(peer, addr))
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Dial a peer.
    pub async fn dial(&self, addr: Multiaddr) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::Dial(addr))
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }

    /// Get connected peers.
    pub async fn get_peers(&self) -> GossipResult<Vec<PeerId>> {
        let (tx, mut rx) = mpsc::channel(1);
        self.command_tx
            .send(GossipCommand::GetPeers(tx))
            .await
            .map_err(|_| GossipError::ChannelClosed)?;

        rx.recv().await.ok_or(GossipError::ChannelClosed)
    }

    /// Shutdown the gossip network.
    pub async fn shutdown(&self) -> GossipResult<()> {
        self.command_tx
            .send(GossipCommand::Shutdown)
            .await
            .map_err(|_| GossipError::ChannelClosed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology_sync_request_serialization() {
        let request = TopologySyncRequest {
            since_timestamp: 1234567890,
            max_results: 100,
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: TopologySyncRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.since_timestamp, 1234567890);
        assert_eq!(parsed.max_results, 100);
    }

    #[test]
    fn test_topology_sync_response_serialization() {
        let response = TopologySyncResponse {
            announcements: vec![],
            has_more: false,
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: TopologySyncResponse = serde_json::from_str(&json).unwrap();

        assert!(parsed.announcements.is_empty());
        assert!(!parsed.has_more);
    }
}
