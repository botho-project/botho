// Copyright (c) 2024 Botho Foundation

//! Peer discovery and gossip networking using libp2p.

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity},
    identity, noise,
    request_response::{self, InboundRequestId, OutboundRequestId, ResponseChannel},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use bth_transaction_types::{MAX_BLOCK_SIZE, MAX_SCP_MESSAGE_SIZE, MAX_TRANSACTION_SIZE};

use crate::block::Block;
use crate::consensus::ScpMessage;
use crate::network::compact_block::{BlockTxn, CompactBlock, GetBlockTxn};
use crate::network::sync::{create_sync_behaviour, SyncCodec, SyncRequest, SyncResponse};
use crate::transaction::Transaction;

/// Topic for block announcements
const BLOCKS_TOPIC: &str = "botho/blocks/1.0.0";

/// Topic for transaction announcements
const TRANSACTIONS_TOPIC: &str = "botho/transactions/1.0.0";

/// Topic for SCP consensus messages
const SCP_TOPIC: &str = "botho/scp/1.0.0";

/// Topic for compact block announcements
const COMPACT_BLOCKS_TOPIC: &str = "botho/compact-blocks/1.0.0";

/// Entry in the peer table
#[derive(Debug, Clone)]
pub struct PeerTableEntry {
    pub peer_id: PeerId,
    pub address: Option<Multiaddr>,
    pub last_seen: std::time::Instant,
}

/// Events from the network layer
#[derive(Debug)]
pub enum NetworkEvent {
    /// A new block was received from a peer
    NewBlock(Block),
    /// A new transaction was received from a peer
    NewTransaction(Transaction),
    /// An SCP consensus message was received
    ScpMessage(ScpMessage),
    /// A compact block was received (for bandwidth-efficient relay)
    NewCompactBlock(CompactBlock),
    /// A request for missing transactions was received
    GetBlockTxn {
        peer: PeerId,
        request: GetBlockTxn,
    },
    /// Missing transactions were received
    BlockTxn(BlockTxn),
    /// A new peer was discovered
    PeerDiscovered(PeerId),
    /// A peer disconnected
    PeerDisconnected(PeerId),
    /// A sync request was received (need to respond)
    SyncRequest {
        peer: PeerId,
        request_id: InboundRequestId,
        request: SyncRequest,
        channel: ResponseChannel<SyncResponse>,
    },
    /// A sync response was received
    SyncResponse {
        peer: PeerId,
        request_id: OutboundRequestId,
        response: SyncResponse,
    },
}

/// Network behaviour combining gossipsub and sync request-response
#[derive(NetworkBehaviour)]
pub struct BothoBehaviour {
    /// Gossipsub for block propagation
    pub gossipsub: gossipsub::Behaviour,
    /// Request-response for chain sync
    pub sync: request_response::Behaviour<SyncCodec>,
}

/// Network discovery and gossip service
pub struct NetworkDiscovery {
    /// Local peer ID
    local_peer_id: PeerId,
    /// Gossip port
    port: u16,
    /// Bootstrap peers
    bootstrap_peers: Vec<String>,
    /// Sender for network events
    event_tx: mpsc::Sender<NetworkEvent>,
    /// Receiver for network events (taken by consumer)
    event_rx: Option<mpsc::Receiver<NetworkEvent>>,
    /// Known peers
    peers: HashMap<PeerId, PeerTableEntry>,
    /// Peers subscribed to compact blocks topic (support bandwidth optimization)
    compact_block_peers: HashSet<PeerId>,
}

impl NetworkDiscovery {
    /// Create a new network discovery service
    pub fn new(port: u16, bootstrap_peers: Vec<String>) -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);

        // Generate a random keypair for this node
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        info!("Local peer ID: {}", local_peer_id);

        Self {
            local_peer_id,
            port,
            bootstrap_peers,
            event_tx,
            event_rx: Some(event_rx),
            peers: HashMap::new(),
            compact_block_peers: HashSet::new(),
        }
    }

    /// Get the local peer ID
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<NetworkEvent>> {
        self.event_rx.take()
    }

    /// Get current peer count
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get peer table entries
    pub fn peer_table(&self) -> Vec<PeerTableEntry> {
        self.peers.values().cloned().collect()
    }

    /// Check if a peer supports compact blocks (subscribed to the topic)
    pub fn peer_supports_compact_blocks(&self, peer_id: &PeerId) -> bool {
        self.compact_block_peers.contains(peer_id)
    }

    /// Count of connected peers that don't support compact blocks
    ///
    /// These "legacy" peers need full block broadcasts.
    pub fn legacy_peer_count(&self) -> usize {
        self.peers
            .keys()
            .filter(|p| !self.compact_block_peers.contains(p))
            .count()
    }

    /// Check if all connected peers support compact blocks
    pub fn all_peers_support_compact_blocks(&self) -> bool {
        self.peers
            .keys()
            .all(|p| self.compact_block_peers.contains(p))
    }

    /// Start the network service (runs in background)
    pub async fn start(&mut self) -> anyhow::Result<Swarm<BothoBehaviour>> {
        // Create swarm
        let mut swarm = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                // Configure gossipsub with message size limits
                // Use MAX_BLOCK_SIZE as the limit since blocks are the largest messages
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(1))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .max_transmit_size(MAX_BLOCK_SIZE)
                    .build()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                let gossipsub = gossipsub::Behaviour::new(
                    MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                // Create sync request-response behaviour
                let sync = create_sync_behaviour();

                Ok(BothoBehaviour { gossipsub, sync })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // Subscribe to blocks topic
        let blocks_topic = IdentTopic::new(BLOCKS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&blocks_topic)?;

        // Subscribe to transactions topic
        let transactions_topic = IdentTopic::new(TRANSACTIONS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&transactions_topic)?;

        // Subscribe to SCP consensus topic
        let scp_topic = IdentTopic::new(SCP_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&scp_topic)?;

        // Subscribe to compact blocks topic
        let compact_blocks_topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&compact_blocks_topic)?;

        // Listen on the configured port
        let listen_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", self.port).parse()?;
        swarm.listen_on(listen_addr)?;

        // Connect to bootstrap peers
        for peer_addr in &self.bootstrap_peers {
            match peer_addr.parse::<Multiaddr>() {
                Ok(addr) => {
                    info!("Dialing bootstrap peer: {}", addr);
                    if let Err(e) = swarm.dial(addr.clone()) {
                        warn!("Failed to dial {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    warn!("Invalid bootstrap peer address {}: {}", peer_addr, e);
                }
            }
        }

        self.local_peer_id = *swarm.local_peer_id();
        info!("Network started on port {}", self.port);

        Ok(swarm)
    }

    /// Broadcast a new block to the network
    pub fn broadcast_block(swarm: &mut Swarm<BothoBehaviour>, block: &Block) -> anyhow::Result<()> {
        let topic = IdentTopic::new(BLOCKS_TOPIC);
        let block_bytes = bincode::serialize(block)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, block_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish block: {:?}", e))?;

        debug!("Broadcast block {} to network", block.height());
        Ok(())
    }

    /// Broadcast a transaction to the network
    pub fn broadcast_transaction(swarm: &mut Swarm<BothoBehaviour>, tx: &Transaction) -> anyhow::Result<()> {
        let topic = IdentTopic::new(TRANSACTIONS_TOPIC);
        let tx_bytes = bincode::serialize(tx)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, tx_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish transaction: {:?}", e))?;

        debug!("Broadcast transaction {} to network", hex::encode(&tx.hash()[0..8]));
        Ok(())
    }

    /// Broadcast an SCP consensus message to the network
    pub fn broadcast_scp(swarm: &mut Swarm<BothoBehaviour>, msg: &ScpMessage) -> anyhow::Result<()> {
        let topic = IdentTopic::new(SCP_TOPIC);
        let msg_bytes = bincode::serialize(msg)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, msg_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish SCP message: {:?}", e))?;

        debug!(slot = msg.slot_index, "Broadcast SCP message");
        Ok(())
    }

    /// Broadcast a compact block to the network (bandwidth-efficient relay)
    pub fn broadcast_compact_block(
        swarm: &mut Swarm<BothoBehaviour>,
        compact_block: &CompactBlock,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        let bytes = bincode::serialize(compact_block)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish compact block: {:?}", e))?;

        debug!(
            height = compact_block.height(),
            txs = compact_block.short_ids.len(),
            "Broadcast compact block"
        );
        Ok(())
    }

    /// Request missing transactions for compact block reconstruction
    pub fn request_block_txns(
        swarm: &mut Swarm<BothoBehaviour>,
        request: &GetBlockTxn,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        let bytes = bincode::serialize(request)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish GetBlockTxn: {:?}", e))?;

        debug!(
            block = hex::encode(&request.block_hash[0..8]),
            missing = request.indices.len(),
            "Requested missing transactions"
        );
        Ok(())
    }

    /// Respond with missing transactions for compact block reconstruction
    pub fn respond_block_txns(
        swarm: &mut Swarm<BothoBehaviour>,
        response: &BlockTxn,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        let bytes = bincode::serialize(response)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish BlockTxn: {:?}", e))?;

        debug!(
            block = hex::encode(&response.block_hash[0..8]),
            txs = response.txs.len(),
            "Sent missing transactions"
        );
        Ok(())
    }

    /// Broadcast a block with bandwidth optimization.
    ///
    /// Always sends a compact block. Only sends the full block if there are
    /// legacy peers that don't support compact block relay.
    pub fn broadcast_block_smart(
        swarm: &mut Swarm<BothoBehaviour>,
        block: &Block,
        legacy_peers_exist: bool,
    ) -> anyhow::Result<()> {
        // Always send compact block (bandwidth-efficient for upgraded peers)
        let compact_block = CompactBlock::from_block(block);
        Self::broadcast_compact_block(swarm, &compact_block)?;

        // Only send full block if there are legacy peers
        if legacy_peers_exist {
            Self::broadcast_block(swarm, block)?;
            debug!(
                height = block.height(),
                "Sent full block for legacy peers"
            );
        } else {
            debug!(
                height = block.height(),
                "Skipped full block - all peers support compact blocks"
            );
        }

        Ok(())
    }

    /// Process a swarm event
    pub fn process_event(
        &mut self,
        event: SwarmEvent<BothoBehaviourEvent>,
    ) -> Option<NetworkEvent> {
        match event {
            SwarmEvent::Behaviour(BothoBehaviourEvent::Gossipsub(
                gossipsub::Event::Message { message, .. },
            )) => {
                // Determine which topic this message is from
                let topic = message.topic.as_str();

                if topic == BLOCKS_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_BLOCK_SIZE {
                        warn!(
                            "Rejected oversized block message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_BLOCK_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as a block
                    match bincode::deserialize::<Block>(&message.data) {
                        Ok(block) => {
                            info!(
                                "Received block {} from network (hash: {})",
                                block.height(),
                                hex::encode(&block.hash()[0..8])
                            );
                            return Some(NetworkEvent::NewBlock(block));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize block from gossip: {}", e);
                        }
                    }
                } else if topic == TRANSACTIONS_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_TRANSACTION_SIZE {
                        warn!(
                            "Rejected oversized transaction message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_TRANSACTION_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as a transaction
                    match bincode::deserialize::<Transaction>(&message.data) {
                        Ok(tx) => {
                            debug!(
                                "Received transaction {} from network",
                                hex::encode(&tx.hash()[0..8])
                            );
                            return Some(NetworkEvent::NewTransaction(tx));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize transaction from gossip: {}", e);
                        }
                    }
                } else if topic == SCP_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_SCP_MESSAGE_SIZE {
                        warn!(
                            "Rejected oversized SCP message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_SCP_MESSAGE_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as an SCP message
                    match bincode::deserialize::<ScpMessage>(&message.data) {
                        Ok(scp_msg) => {
                            debug!(slot = scp_msg.slot_index, "Received SCP message from network");
                            return Some(NetworkEvent::ScpMessage(scp_msg));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize SCP message from gossip: {}", e);
                        }
                    }
                } else if topic == COMPACT_BLOCKS_TOPIC {
                    // Compact block messages can be: CompactBlock, GetBlockTxn, or BlockTxn
                    // Size limit is same as full blocks
                    if message.data.len() > MAX_BLOCK_SIZE {
                        warn!(
                            "Rejected oversized compact block message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_BLOCK_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as CompactBlock first (most common)
                    if let Ok(compact_block) = bincode::deserialize::<CompactBlock>(&message.data) {
                        info!(
                            "Received compact block {} from network ({} txs, {} bytes)",
                            compact_block.height(),
                            compact_block.short_ids.len(),
                            message.data.len()
                        );
                        return Some(NetworkEvent::NewCompactBlock(compact_block));
                    }

                    // Try GetBlockTxn
                    if let Ok(request) = bincode::deserialize::<GetBlockTxn>(&message.data) {
                        debug!(
                            "Received GetBlockTxn for block {} ({} indices)",
                            hex::encode(&request.block_hash[0..8]),
                            request.indices.len()
                        );
                        let peer = message.source.unwrap_or(PeerId::random());
                        return Some(NetworkEvent::GetBlockTxn { peer, request });
                    }

                    // Try BlockTxn
                    if let Ok(response) = bincode::deserialize::<BlockTxn>(&message.data) {
                        debug!(
                            "Received BlockTxn for block {} ({} txs)",
                            hex::encode(&response.block_hash[0..8]),
                            response.txs.len()
                        );
                        return Some(NetworkEvent::BlockTxn(response));
                    }

                    warn!("Failed to deserialize compact block message");
                }

                None
            }

            // Track peers subscribing to compact blocks topic
            SwarmEvent::Behaviour(BothoBehaviourEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic },
            )) => {
                if topic.as_str() == COMPACT_BLOCKS_TOPIC {
                    self.compact_block_peers.insert(peer_id);
                    debug!(%peer_id, "Peer subscribed to compact blocks");
                }
                None
            }

            // Track peers unsubscribing from compact blocks topic
            SwarmEvent::Behaviour(BothoBehaviourEvent::Gossipsub(
                gossipsub::Event::Unsubscribed { peer_id, topic },
            )) => {
                if topic.as_str() == COMPACT_BLOCKS_TOPIC {
                    self.compact_block_peers.remove(&peer_id);
                    debug!(%peer_id, "Peer unsubscribed from compact blocks");
                }
                None
            }

            // Handle sync request-response events
            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::Message { peer, message, .. },
            )) => match message {
                request_response::Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    debug!(%peer, ?request, "Received sync request");
                    Some(NetworkEvent::SyncRequest {
                        peer,
                        request_id,
                        request,
                        channel,
                    })
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    debug!(%peer, "Received sync response");
                    Some(NetworkEvent::SyncResponse {
                        peer,
                        request_id,
                        response,
                    })
                }
            },

            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::OutboundFailure {
                    peer,
                    request_id,
                    error,
                    ..
                },
            )) => {
                warn!(%peer, ?request_id, %error, "Sync request failed");
                Some(NetworkEvent::SyncResponse {
                    peer,
                    request_id,
                    response: SyncResponse::Error(error.to_string()),
                })
            }

            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::InboundFailure {
                    peer,
                    request_id,
                    error,
                    ..
                },
            )) => {
                warn!(%peer, ?request_id, %error, "Inbound sync request failed");
                None
            }

            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::ResponseSent { .. },
            )) => None,

            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
                None
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                info!("Connected to peer: {}", peer_id);
                self.peers.insert(
                    peer_id,
                    PeerTableEntry {
                        peer_id,
                        address: None,
                        last_seen: std::time::Instant::now(),
                    },
                );
                Some(NetworkEvent::PeerDiscovered(peer_id))
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                info!("Disconnected from peer: {}", peer_id);
                self.peers.remove(&peer_id);
                self.compact_block_peers.remove(&peer_id);
                Some(NetworkEvent::PeerDisconnected(peer_id))
            }
            _ => None,
        }
    }

    /// Send a sync request to a peer
    pub fn send_sync_request(
        swarm: &mut Swarm<BothoBehaviour>,
        peer: PeerId,
        request: SyncRequest,
    ) -> OutboundRequestId {
        swarm.behaviour_mut().sync.send_request(&peer, request)
    }

    /// Send a sync response
    pub fn send_sync_response(
        swarm: &mut Swarm<BothoBehaviour>,
        channel: ResponseChannel<SyncResponse>,
        response: SyncResponse,
    ) -> Result<(), SyncResponse> {
        swarm.behaviour_mut().sync.send_response(channel, response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // PeerTableEntry tests
    // ========================================================================

    #[test]
    fn test_peer_table_entry_creation() {
        let peer_id = PeerId::random();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
        };

        assert_eq!(entry.peer_id, peer_id);
        assert!(entry.address.is_none());
    }

    #[test]
    fn test_peer_table_entry_with_address() {
        let peer_id = PeerId::random();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/9000".parse().unwrap();
        let entry = PeerTableEntry {
            peer_id,
            address: Some(addr.clone()),
            last_seen: std::time::Instant::now(),
        };

        assert_eq!(entry.address, Some(addr));
    }

    #[test]
    fn test_peer_table_entry_clone() {
        let peer_id = PeerId::random();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
        };

        let cloned = entry.clone();
        assert_eq!(cloned.peer_id, entry.peer_id);
    }

    // ========================================================================
    // NetworkDiscovery tests
    // ========================================================================

    #[test]
    fn test_network_discovery_new() {
        let discovery = NetworkDiscovery::new(9000, vec![]);

        assert_eq!(discovery.peer_count(), 0);
        assert!(discovery.peer_table().is_empty());
    }

    #[test]
    fn test_network_discovery_with_bootstrap_peers() {
        let bootstrap = vec![
            "/ip4/192.168.1.1/tcp/9000".to_string(),
            "/ip4/192.168.1.2/tcp/9000".to_string(),
        ];
        let discovery = NetworkDiscovery::new(9001, bootstrap);

        // Bootstrap peers are stored but not yet connected
        assert_eq!(discovery.peer_count(), 0);
    }

    #[test]
    fn test_network_discovery_local_peer_id() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        let peer_id = discovery.local_peer_id();

        // Should be a valid peer ID
        assert!(!peer_id.to_string().is_empty());
    }

    #[test]
    fn test_network_discovery_take_event_receiver_once() {
        let mut discovery = NetworkDiscovery::new(9000, vec![]);

        // First take should succeed
        let rx1 = discovery.take_event_receiver();
        assert!(rx1.is_some());

        // Second take should return None
        let rx2 = discovery.take_event_receiver();
        assert!(rx2.is_none());
    }

    #[test]
    fn test_network_discovery_peer_table_empty() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        let table = discovery.peer_table();

        assert!(table.is_empty());
        assert_eq!(discovery.peer_count(), 0);
    }

    // ========================================================================
    // NetworkEvent tests
    // ========================================================================

    #[test]
    fn test_network_event_peer_discovered_debug() {
        let peer_id = PeerId::random();
        let event = NetworkEvent::PeerDiscovered(peer_id);

        // Should implement Debug
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PeerDiscovered"));
    }

    #[test]
    fn test_network_event_peer_disconnected_debug() {
        let peer_id = PeerId::random();
        let event = NetworkEvent::PeerDisconnected(peer_id);

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PeerDisconnected"));
    }

    // ========================================================================
    // Topic constant tests
    // ========================================================================

    #[test]
    fn test_topic_constants_are_valid() {
        assert!(!BLOCKS_TOPIC.is_empty());
        assert!(!TRANSACTIONS_TOPIC.is_empty());
        assert!(!SCP_TOPIC.is_empty());

        // Topics should follow naming convention
        assert!(BLOCKS_TOPIC.starts_with("botho/"));
        assert!(TRANSACTIONS_TOPIC.starts_with("botho/"));
        assert!(SCP_TOPIC.starts_with("botho/"));
    }

    #[test]
    fn test_topics_are_versioned() {
        assert!(BLOCKS_TOPIC.contains("/1.0.0"));
        assert!(TRANSACTIONS_TOPIC.contains("/1.0.0"));
        assert!(SCP_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_topics_are_unique() {
        assert_ne!(BLOCKS_TOPIC, TRANSACTIONS_TOPIC);
        assert_ne!(BLOCKS_TOPIC, SCP_TOPIC);
        assert_ne!(TRANSACTIONS_TOPIC, SCP_TOPIC);
    }

    // ========================================================================
    // Compact block subscription tracking tests
    // ========================================================================

    #[test]
    fn test_compact_blocks_topic_constant() {
        assert_eq!(COMPACT_BLOCKS_TOPIC, "botho/compact-blocks/1.0.0");
        assert!(COMPACT_BLOCKS_TOPIC.starts_with("botho/"));
        assert!(COMPACT_BLOCKS_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_compact_block_peers_initially_empty() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        assert_eq!(discovery.legacy_peer_count(), 0);
        assert!(discovery.all_peers_support_compact_blocks());
    }

    #[test]
    fn test_peer_supports_compact_blocks_false_for_unknown() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        let peer_id = PeerId::random();

        assert!(!discovery.peer_supports_compact_blocks(&peer_id));
    }

    #[test]
    fn test_legacy_peer_count_with_no_peers() {
        let discovery = NetworkDiscovery::new(9000, vec![]);

        // No peers = no legacy peers
        assert_eq!(discovery.legacy_peer_count(), 0);
        assert!(discovery.all_peers_support_compact_blocks());
    }
}
