// Copyright (c) 2024 Cadence Foundation

//! Peer discovery and gossip networking using libp2p.

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity},
    identity, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::block::Block;
use crate::consensus::ScpMessage;

/// Topic for block announcements
const BLOCKS_TOPIC: &str = "cadence/blocks/1.0.0";

/// Topic for SCP consensus messages
const SCP_TOPIC: &str = "cadence/scp/1.0.0";

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
    /// An SCP consensus message was received
    ScpMessage(ScpMessage),
    /// A new peer was discovered
    PeerDiscovered(PeerId),
    /// A peer disconnected
    PeerDisconnected(PeerId),
}

/// Network behaviour combining gossipsub
#[derive(NetworkBehaviour)]
pub struct CadenceBehaviour {
    /// Gossipsub for block propagation
    pub gossipsub: gossipsub::Behaviour,
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

    /// Start the network service (runs in background)
    pub async fn start(&mut self) -> anyhow::Result<Swarm<CadenceBehaviour>> {
        // Create swarm
        let mut swarm = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                // Configure gossipsub
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(1))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .build()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                let gossipsub = gossipsub::Behaviour::new(
                    MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                Ok(CadenceBehaviour { gossipsub })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // Subscribe to blocks topic
        let blocks_topic = IdentTopic::new(BLOCKS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&blocks_topic)?;

        // Subscribe to SCP consensus topic
        let scp_topic = IdentTopic::new(SCP_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&scp_topic)?;

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
    pub fn broadcast_block(swarm: &mut Swarm<CadenceBehaviour>, block: &Block) -> anyhow::Result<()> {
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

    /// Broadcast an SCP consensus message to the network
    pub fn broadcast_scp(swarm: &mut Swarm<CadenceBehaviour>, msg: &ScpMessage) -> anyhow::Result<()> {
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

    /// Process a swarm event
    pub fn process_event(
        &mut self,
        event: SwarmEvent<CadenceBehaviourEvent>,
    ) -> Option<NetworkEvent> {
        match event {
            SwarmEvent::Behaviour(CadenceBehaviourEvent::Gossipsub(
                gossipsub::Event::Message { message, .. },
            )) => {
                // Determine which topic this message is from
                let topic = message.topic.as_str();

                if topic == BLOCKS_TOPIC {
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
                } else if topic == SCP_TOPIC {
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
                }

                None
            }
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
                Some(NetworkEvent::PeerDisconnected(peer_id))
            }
            _ => None,
        }
    }
}
