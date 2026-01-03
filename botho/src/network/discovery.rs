// Copyright (c) 2024 Botho Foundation

//! Peer discovery and gossip networking using libp2p.
//!
//! ## Protocol Versioning
//!
//! This module implements version negotiation for protocol upgrades:
//!
//! - **Protocol Version**: Embedded in the libp2p identify protocol's agent_version
//!   field as `botho/<version>/<block_version>`. This allows peers to discover
//!   compatibility during connection establishment.
//!
//! - **Minimum Supported Version**: Defines the oldest protocol version this node
//!   will connect to. Peers below this threshold receive a warning but are not
//!   disconnected (graceful degradation).
//!
//! - **Upgrade Announcements**: A dedicated gossipsub topic allows validators
//!   and seed nodes to broadcast upcoming network upgrades.

use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity},
    identify,
    identity, noise,
    request_response::{self, InboundRequestId, OutboundRequestId, ResponseChannel},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use bth_transaction_types::{BlockVersion, MAX_BLOCK_SIZE, MAX_SCP_MESSAGE_SIZE, MAX_TRANSACTION_SIZE};

use crate::block::Block;
use crate::consensus::ScpMessage;
use crate::network::compact_block::{BlockTxn, CompactBlock, GetBlockTxn};
use crate::network::pex::{PeerSource, PexManager, PexMessage, MAX_PEX_MESSAGE_SIZE};
use crate::network::sync::{create_sync_behaviour, SyncCodec, SyncRequest, SyncResponse};
use crate::transaction::Transaction;

/// Current protocol version string.
/// Format: major.minor.patch
/// - Major: Breaking changes requiring hard fork
/// - Minor: Soft fork compatible changes
/// - Patch: Bug fixes, no consensus impact
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Minimum supported protocol version.
/// Peers below this version receive a warning but are not disconnected
/// to allow graceful network upgrades.
pub const MIN_SUPPORTED_PROTOCOL_VERSION: &str = "1.0.0";

/// Topic for block announcements
const BLOCKS_TOPIC: &str = "botho/blocks/1.0.0";

/// Topic for transaction announcements
const TRANSACTIONS_TOPIC: &str = "botho/transactions/1.0.0";

/// Topic for SCP consensus messages
const SCP_TOPIC: &str = "botho/scp/1.0.0";

/// Topic for compact block announcements
const COMPACT_BLOCKS_TOPIC: &str = "botho/compact-blocks/1.0.0";

/// Topic for upgrade announcements.
/// Validators and seed nodes publish upcoming network upgrades here.
const UPGRADE_ANNOUNCEMENTS_TOPIC: &str = "botho/upgrades/1.0.0";

/// Topic for peer exchange (PEX) messages
const PEX_TOPIC: &str = "botho/pex/1.0.0";

/// Parsed protocol version from peer agent string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolVersion {
    /// Major version (breaking changes)
    pub major: u32,
    /// Minor version (soft fork compatible)
    pub minor: u32,
    /// Patch version (bug fixes)
    pub patch: u32,
    /// Maximum block version supported by peer
    pub block_version: Option<u32>,
}

impl ProtocolVersion {
    /// Parse a version string like "1.0.0" or agent string like "botho/1.0.0/5"
    pub fn parse(s: &str) -> Option<Self> {
        // Handle full agent string format: "botho/1.0.0/5"
        let version_str = if s.starts_with("botho/") {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() >= 2 {
                parts[1]
            } else {
                return None;
            }
        } else {
            s
        };

        let parts: Vec<&str> = version_str.split('.').collect();
        if parts.len() != 3 {
            return None;
        }

        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts[2].parse().ok()?;

        // Try to parse block version from agent string
        let block_version = if s.starts_with("botho/") {
            let agent_parts: Vec<&str> = s.split('/').collect();
            if agent_parts.len() >= 3 {
                agent_parts[2].parse().ok()
            } else {
                None
            }
        } else {
            None
        };

        Some(Self {
            major,
            minor,
            patch,
            block_version,
        })
    }

    /// Check if this version is compatible with another version.
    /// Returns true if major versions match and this version >= other.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        if self.major != other.major {
            return false;
        }
        if self.minor > other.minor {
            return true;
        }
        if self.minor == other.minor && self.patch >= other.patch {
            return true;
        }
        false
    }

    /// Create agent version string for libp2p identify protocol.
    pub fn to_agent_string(&self, block_version: u32) -> String {
        format!(
            "botho/{}.{}.{}/{}",
            self.major, self.minor, self.patch, block_version
        )
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(bv) = self.block_version {
            write!(f, " (block v{})", bv)?;
        }
        Ok(())
    }
}

/// Entry in the peer table
#[derive(Debug, Clone)]
pub struct PeerTableEntry {
    pub peer_id: PeerId,
    pub address: Option<Multiaddr>,
    pub last_seen: std::time::Instant,
    /// Peer's protocol version (parsed from identify agent_version)
    pub protocol_version: Option<ProtocolVersion>,
    /// Whether this peer's version is below minimum supported
    pub version_warning: bool,
}

/// Upgrade announcement broadcast via gossipsub.
///
/// Validators and seed nodes publish these to notify the network
/// of upcoming protocol upgrades.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeAnnouncement {
    /// New protocol version after upgrade
    pub target_version: String,
    /// Target block version after upgrade
    pub target_block_version: u32,
    /// Block height at which upgrade activates (0 = time-based)
    pub activation_height: Option<u64>,
    /// Unix timestamp at which upgrade activates (0 = height-based)
    pub activation_timestamp: Option<u64>,
    /// Human-readable description of the upgrade
    pub description: String,
    /// Whether this is a hard fork (breaking) or soft fork
    pub is_hard_fork: bool,
    /// Minimum version required after upgrade
    pub min_version_after: String,
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
    /// An upgrade announcement was received from the network.
    /// Node operators should take action based on the announcement.
    UpgradeAnnouncement(UpgradeAnnouncement),
    /// A peer with an outdated protocol version was detected.
    /// This is informational; the peer is not disconnected.
    PeerVersionWarning {
        peer: PeerId,
        peer_version: ProtocolVersion,
        min_version: ProtocolVersion,
    },
    /// New peer addresses received via PEX (connect to these)
    PexAddresses(Vec<Multiaddr>),
}

/// Network behaviour combining gossipsub, identify, and sync request-response
#[derive(NetworkBehaviour)]
pub struct BothoBehaviour {
    /// Gossipsub for block propagation
    pub gossipsub: gossipsub::Behaviour,
    /// Request-response for chain sync
    pub sync: request_response::Behaviour<SyncCodec>,
    /// Identify protocol for version negotiation
    pub identify: identify::Behaviour,
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
    /// PEX manager for decentralized peer discovery
    pex_manager: PexManager,
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
            pex_manager: PexManager::new(),
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

    /// Get the current protocol version
    pub fn protocol_version() -> &'static str {
        PROTOCOL_VERSION
    }

    /// Get peers with version warnings (below minimum supported)
    pub fn peers_with_version_warnings(&self) -> Vec<&PeerTableEntry> {
        self.peers.values().filter(|p| p.version_warning).collect()
    }

    /// Get count of peers with outdated versions
    pub fn outdated_peer_count(&self) -> usize {
        self.peers.values().filter(|p| p.version_warning).count()
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
                    .map_err(std::io::Error::other)?;

                let gossipsub = gossipsub::Behaviour::new(
                    MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .map_err(std::io::Error::other)?;

                // Create sync request-response behaviour
                let sync = create_sync_behaviour();

                // Configure identify protocol with version information
                // Agent version format: "botho/<protocol_version>/<block_version>"
                let agent_version = format!(
                    "botho/{}/{}",
                    PROTOCOL_VERSION,
                    *BlockVersion::MAX
                );
                let identify_config = identify::Config::new(
                    "/botho/id/1.0.0".to_string(),
                    key.public(),
                )
                .with_agent_version(agent_version);
                let identify = identify::Behaviour::new(identify_config);

                Ok(BothoBehaviour { gossipsub, sync, identify })
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

        // Subscribe to upgrade announcements topic
        let upgrade_topic = IdentTopic::new(UPGRADE_ANNOUNCEMENTS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&upgrade_topic)?;

        // Subscribe to PEX topic
        let pex_topic = IdentTopic::new(PEX_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&pex_topic)?;

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

    /// Broadcast a PEX message with known peers
    pub fn broadcast_pex(
        swarm: &mut Swarm<BothoBehaviour>,
        message: &PexMessage,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(PEX_TOPIC);
        let bytes = bincode::serialize(message)?;

        // Size check
        if bytes.len() > MAX_PEX_MESSAGE_SIZE {
            return Err(anyhow::anyhow!(
                "PEX message too large: {} bytes (max: {})",
                bytes.len(),
                MAX_PEX_MESSAGE_SIZE
            ));
        }

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish PEX message: {:?}", e))?;

        debug!(peers = message.entries.len(), "Broadcast PEX message");
        Ok(())
    }

    /// Get the PEX manager for external use
    pub fn pex_manager(&self) -> &PexManager {
        &self.pex_manager
    }

    /// Get mutable PEX manager
    pub fn pex_manager_mut(&mut self) -> &mut PexManager {
        &mut self.pex_manager
    }

    /// Check if we should broadcast PEX and do it if ready
    ///
    /// Call this periodically (e.g., every minute) to share known peers.
    pub fn maybe_broadcast_pex(&mut self, swarm: &mut Swarm<BothoBehaviour>) {
        if !self.pex_manager.should_broadcast() {
            return;
        }

        // Collect shareable peers
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let peers: Vec<_> = self
            .peers
            .values()
            .filter_map(|entry| {
                entry.address.as_ref().map(|addr| {
                    let last_seen = current_time
                        - entry
                            .last_seen
                            .elapsed()
                            .as_secs()
                            .min(current_time);
                    (entry.peer_id, addr.clone(), last_seen)
                })
            })
            .collect();

        if let Some(message) = self.pex_manager.prepare_broadcast(peers) {
            if let Err(e) = Self::broadcast_pex(swarm, &message) {
                warn!("Failed to broadcast PEX: {}", e);
            } else {
                self.pex_manager.record_broadcast();
            }
        }
    }

    /// Record a peer with its discovery source for eclipse attack prevention
    pub fn record_peer_source(&mut self, peer_id: PeerId, addr: &Multiaddr, source: PeerSource) {
        self.pex_manager
            .source_tracker
            .record_peer(peer_id, addr, source);
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

    /// Broadcast an upgrade announcement to the network.
    ///
    /// This should only be called by validators or seed nodes to notify
    /// the network of upcoming protocol upgrades.
    pub fn broadcast_upgrade_announcement(
        swarm: &mut Swarm<BothoBehaviour>,
        announcement: &UpgradeAnnouncement,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(UPGRADE_ANNOUNCEMENTS_TOPIC);
        let bytes = bincode::serialize(announcement)?;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish upgrade announcement: {:?}", e))?;

        info!(
            target_version = %announcement.target_version,
            target_block_version = announcement.target_block_version,
            is_hard_fork = announcement.is_hard_fork,
            "Broadcast upgrade announcement"
        );
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
                } else if topic == UPGRADE_ANNOUNCEMENTS_TOPIC {
                    // Upgrade announcement messages are relatively small
                    const MAX_UPGRADE_MESSAGE_SIZE: usize = 4096;
                    if message.data.len() > MAX_UPGRADE_MESSAGE_SIZE {
                        warn!(
                            "Rejected oversized upgrade announcement: {} bytes (max: {})",
                            message.data.len(),
                            MAX_UPGRADE_MESSAGE_SIZE
                        );
                        return None;
                    }

                    match bincode::deserialize::<UpgradeAnnouncement>(&message.data) {
                        Ok(announcement) => {
                            info!(
                                target_version = %announcement.target_version,
                                target_block_version = announcement.target_block_version,
                                is_hard_fork = announcement.is_hard_fork,
                                description = %announcement.description,
                                "Received upgrade announcement from network"
                            );
                            return Some(NetworkEvent::UpgradeAnnouncement(announcement));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize upgrade announcement: {}", e);
                        }
                    }
                } else if topic == PEX_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_PEX_MESSAGE_SIZE {
                        warn!(
                            "Rejected oversized PEX message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_PEX_MESSAGE_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as PEX message
                    match bincode::deserialize::<PexMessage>(&message.data) {
                        Ok(pex_msg) => {
                            let peer = message.source.unwrap_or(PeerId::random());
                            debug!(
                                %peer,
                                entries = pex_msg.entries.len(),
                                "Received PEX message"
                            );

                            // Process through PEX manager
                            let valid_addrs = self.pex_manager.process_incoming(&peer, &pex_msg);

                            if !valid_addrs.is_empty() {
                                return Some(NetworkEvent::PexAddresses(valid_addrs));
                            }
                        }
                        Err(e) => {
                            warn!("Failed to deserialize PEX message from gossip: {}", e);
                        }
                    }
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

            // Handle identify protocol events for version tracking
            SwarmEvent::Behaviour(BothoBehaviourEvent::Identify(
                identify::Event::Received { peer_id, info, .. },
            )) => {
                // Parse the agent_version to extract protocol version
                let peer_version = ProtocolVersion::parse(&info.agent_version);
                let min_version = ProtocolVersion::parse(MIN_SUPPORTED_PROTOCOL_VERSION);

                let version_warning = match (&peer_version, &min_version) {
                    (Some(pv), Some(mv)) => !pv.is_compatible_with(mv),
                    _ => false,
                };

                // Update peer entry with version information
                if let Some(entry) = self.peers.get_mut(&peer_id) {
                    entry.protocol_version = peer_version.clone();
                    entry.version_warning = version_warning;
                    entry.last_seen = std::time::Instant::now();
                }

                if version_warning {
                    if let (Some(pv), Some(mv)) = (peer_version.clone(), min_version) {
                        warn!(
                            %peer_id,
                            peer_version = %pv,
                            min_version = %mv,
                            "Peer has outdated protocol version"
                        );
                        return Some(NetworkEvent::PeerVersionWarning {
                            peer: peer_id,
                            peer_version: pv,
                            min_version: mv,
                        });
                    }
                }

                if let Some(pv) = peer_version {
                    debug!(
                        %peer_id,
                        protocol_version = %pv,
                        agent_version = %info.agent_version,
                        "Identified peer version"
                    );
                }

                None
            }

            SwarmEvent::Behaviour(BothoBehaviourEvent::Identify(
                identify::Event::Sent { .. } | identify::Event::Pushed { .. } | identify::Event::Error { .. },
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
                        protocol_version: None, // Will be set when identify completes
                        version_warning: false,
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
            protocol_version: None,
            version_warning: false,
        };

        assert_eq!(entry.peer_id, peer_id);
        assert!(entry.address.is_none());
        assert!(entry.protocol_version.is_none());
        assert!(!entry.version_warning);
    }

    #[test]
    fn test_peer_table_entry_with_address() {
        let peer_id = PeerId::random();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/9000".parse().unwrap();
        let entry = PeerTableEntry {
            peer_id,
            address: Some(addr.clone()),
            last_seen: std::time::Instant::now(),
            protocol_version: None,
            version_warning: false,
        };

        assert_eq!(entry.address, Some(addr));
    }

    #[test]
    fn test_peer_table_entry_with_version() {
        let peer_id = PeerId::random();
        let version = ProtocolVersion::parse("botho/1.0.0/5").unwrap();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
            protocol_version: Some(version.clone()),
            version_warning: false,
        };

        assert_eq!(entry.protocol_version, Some(version));
    }

    #[test]
    fn test_peer_table_entry_clone() {
        let peer_id = PeerId::random();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
            protocol_version: None,
            version_warning: false,
        };

        let cloned = entry.clone();
        assert_eq!(cloned.peer_id, entry.peer_id);
    }

    // ========================================================================
    // ProtocolVersion tests
    // ========================================================================

    #[test]
    fn test_protocol_version_parse_simple() {
        let version = ProtocolVersion::parse("1.0.0").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 0);
        assert!(version.block_version.is_none());
    }

    #[test]
    fn test_protocol_version_parse_agent_string() {
        let version = ProtocolVersion::parse("botho/1.2.3/5").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 2);
        assert_eq!(version.patch, 3);
        assert_eq!(version.block_version, Some(5));
    }

    #[test]
    fn test_protocol_version_parse_agent_without_block_version() {
        let version = ProtocolVersion::parse("botho/1.0.0").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 0);
        assert!(version.block_version.is_none());
    }

    #[test]
    fn test_protocol_version_parse_invalid() {
        assert!(ProtocolVersion::parse("invalid").is_none());
        assert!(ProtocolVersion::parse("1.0").is_none());
        assert!(ProtocolVersion::parse("").is_none());
    }

    #[test]
    fn test_protocol_version_is_compatible() {
        let v1_0_0 = ProtocolVersion::parse("1.0.0").unwrap();
        let v1_0_1 = ProtocolVersion::parse("1.0.1").unwrap();
        let v1_1_0 = ProtocolVersion::parse("1.1.0").unwrap();
        let v2_0_0 = ProtocolVersion::parse("2.0.0").unwrap();

        // Same version is compatible
        assert!(v1_0_0.is_compatible_with(&v1_0_0));

        // Higher patch is compatible with lower
        assert!(v1_0_1.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v1_0_1));

        // Higher minor is compatible with lower
        assert!(v1_1_0.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v1_1_0));

        // Different major is not compatible
        assert!(!v2_0_0.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v2_0_0));
    }

    #[test]
    fn test_protocol_version_to_agent_string() {
        let version = ProtocolVersion::parse("1.0.0").unwrap();
        let agent = version.to_agent_string(5);
        assert_eq!(agent, "botho/1.0.0/5");
    }

    #[test]
    fn test_protocol_version_display() {
        let version = ProtocolVersion::parse("botho/1.2.3/5").unwrap();
        let display = format!("{}", version);
        assert_eq!(display, "1.2.3 (block v5)");

        let version_no_block = ProtocolVersion::parse("1.2.3").unwrap();
        let display_no_block = format!("{}", version_no_block);
        assert_eq!(display_no_block, "1.2.3");
    }

    // ========================================================================
    // UpgradeAnnouncement tests
    // ========================================================================

    #[test]
    fn test_upgrade_announcement_serialization() {
        let announcement = UpgradeAnnouncement {
            target_version: "1.1.0".to_string(),
            target_block_version: 6,
            activation_height: Some(100000),
            activation_timestamp: None,
            description: "Test upgrade".to_string(),
            is_hard_fork: false,
            min_version_after: "1.1.0".to_string(),
        };

        let serialized = bincode::serialize(&announcement).unwrap();
        let deserialized: UpgradeAnnouncement = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.target_version, "1.1.0");
        assert_eq!(deserialized.target_block_version, 6);
        assert_eq!(deserialized.activation_height, Some(100000));
        assert!(deserialized.activation_timestamp.is_none());
        assert!(!deserialized.is_hard_fork);
    }

    #[test]
    fn test_upgrade_announcement_hard_fork() {
        let announcement = UpgradeAnnouncement {
            target_version: "2.0.0".to_string(),
            target_block_version: 7,
            activation_height: None,
            activation_timestamp: Some(1700000000),
            description: "Major protocol upgrade".to_string(),
            is_hard_fork: true,
            min_version_after: "2.0.0".to_string(),
        };

        assert!(announcement.is_hard_fork);
        assert_eq!(announcement.activation_timestamp, Some(1700000000));
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
        assert!(!UPGRADE_ANNOUNCEMENTS_TOPIC.is_empty());

        // Topics should follow naming convention
        assert!(BLOCKS_TOPIC.starts_with("botho/"));
        assert!(TRANSACTIONS_TOPIC.starts_with("botho/"));
        assert!(SCP_TOPIC.starts_with("botho/"));
        assert!(UPGRADE_ANNOUNCEMENTS_TOPIC.starts_with("botho/"));
    }

    #[test]
    fn test_topics_are_versioned() {
        assert!(BLOCKS_TOPIC.contains("/1.0.0"));
        assert!(TRANSACTIONS_TOPIC.contains("/1.0.0"));
        assert!(SCP_TOPIC.contains("/1.0.0"));
        assert!(UPGRADE_ANNOUNCEMENTS_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_topics_are_unique() {
        assert_ne!(BLOCKS_TOPIC, TRANSACTIONS_TOPIC);
        assert_ne!(BLOCKS_TOPIC, SCP_TOPIC);
        assert_ne!(TRANSACTIONS_TOPIC, SCP_TOPIC);
        assert_ne!(UPGRADE_ANNOUNCEMENTS_TOPIC, BLOCKS_TOPIC);
        assert_ne!(UPGRADE_ANNOUNCEMENTS_TOPIC, TRANSACTIONS_TOPIC);
        assert_ne!(UPGRADE_ANNOUNCEMENTS_TOPIC, SCP_TOPIC);
    }

    #[test]
    fn test_upgrade_announcements_topic() {
        assert_eq!(UPGRADE_ANNOUNCEMENTS_TOPIC, "botho/upgrades/1.0.0");
    }

    // ========================================================================
    // Protocol version constant tests
    // ========================================================================

    #[test]
    fn test_protocol_version_constant() {
        assert_eq!(PROTOCOL_VERSION, "1.0.0");
        let parsed = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        assert_eq!(parsed.major, 1);
        assert_eq!(parsed.minor, 0);
        assert_eq!(parsed.patch, 0);
    }

    #[test]
    fn test_min_supported_protocol_version_constant() {
        assert_eq!(MIN_SUPPORTED_PROTOCOL_VERSION, "1.0.0");
        let parsed = ProtocolVersion::parse(MIN_SUPPORTED_PROTOCOL_VERSION).unwrap();
        assert_eq!(parsed.major, 1);
    }

    #[test]
    fn test_current_version_compatible_with_min() {
        let current = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        let min = ProtocolVersion::parse(MIN_SUPPORTED_PROTOCOL_VERSION).unwrap();
        assert!(current.is_compatible_with(&min));
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

    // ========================================================================
    // PEX integration tests
    // ========================================================================

    #[test]
    fn test_pex_topic_constant() {
        assert_eq!(PEX_TOPIC, "botho/pex/1.0.0");
        assert!(PEX_TOPIC.starts_with("botho/"));
        assert!(PEX_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_network_discovery_has_pex_manager() {
        let discovery = NetworkDiscovery::new(9000, vec![]);

        // PEX manager should be initialized
        assert!(discovery.pex_manager().should_broadcast());
    }

    #[test]
    fn test_pex_manager_access() {
        let mut discovery = NetworkDiscovery::new(9000, vec![]);

        // Should be able to access PEX manager mutably
        discovery.pex_manager_mut().record_broadcast();
        assert!(!discovery.pex_manager().should_broadcast());
    }

    #[test]
    fn test_record_peer_source() {
        let mut discovery = NetworkDiscovery::new(9000, vec![]);
        let peer = PeerId::random();
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();

        discovery.record_peer_source(peer, &addr, PeerSource::Bootstrap);

        assert_eq!(
            discovery.pex_manager().source_tracker.get_source(&peer),
            Some(PeerSource::Bootstrap)
        );
    }

    #[test]
    fn test_network_event_pex_addresses() {
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();
        let event = NetworkEvent::PexAddresses(vec![addr.clone()]);

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PexAddresses"));
    }
}
