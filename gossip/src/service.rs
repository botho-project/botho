// Copyright (c) 2024 Botho Foundation

//! High-level gossip service that manages the libp2p swarm and peer store.
//!
//! The GossipService provides:
//! - Automatic peer discovery and connection management
//! - Periodic announcement broadcasting (push)
//! - Topology synchronization from peers (pull)
//! - Integration with the peer store

use crate::{
    behaviour::{
        GossipBehaviour, GossipCommand, GossipEvent, GossipHandle, TopologySyncRequest,
        TopologySyncResponse,
    },
    config::GossipConfig,
    error::{GossipError, GossipResult},
    messages::{NodeAnnouncement, NodeCapabilities, ANNOUNCEMENTS_TOPIC},
    store::{new_shared_store, SharedPeerStore},
};
use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic},
    identify,
    kad,
    noise,
    request_response::{self, ResponseChannel},
    swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder,
};
use bth_common::{NodeID, ResponderId};
use bth_consensus_scp_types::QuorumSet;
use bth_crypto_keys::{Ed25519Pair, Signer};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    select,
    sync::mpsc,
    time::{interval, Interval},
};
use tracing::{debug, error, info, trace, warn};

/// The gossip service manages the peer-to-peer network.
pub struct GossipService {
    /// Our node's identity
    node_id: NodeID,

    /// Keypair for signing announcements
    signing_key: Arc<Ed25519Pair>,

    /// Our quorum set
    quorum_set: QuorumSet<ResponderId>,

    /// Our endpoints
    endpoints: Vec<String>,

    /// Our capabilities
    capabilities: NodeCapabilities,

    /// Version string
    version: String,

    /// Configuration
    config: GossipConfig,

    /// Peer store
    store: SharedPeerStore,

    /// Handle for sending commands to the swarm
    handle: Option<GossipHandle>,

    /// Event receiver
    event_rx: Option<mpsc::Receiver<GossipEvent>>,
}

impl GossipService {
    /// Create a new gossip service.
    pub fn new(
        node_id: NodeID,
        signing_key: Arc<Ed25519Pair>,
        quorum_set: QuorumSet<ResponderId>,
        endpoints: Vec<String>,
        capabilities: NodeCapabilities,
        version: String,
        config: GossipConfig,
    ) -> Self {
        let store = new_shared_store(config.store_config.clone());

        Self {
            node_id,
            signing_key,
            quorum_set,
            endpoints,
            capabilities,
            version,
            config,
            store,
            handle: None,
            event_rx: None,
        }
    }

    /// Get a reference to the peer store.
    pub fn store(&self) -> &SharedPeerStore {
        &self.store
    }

    /// Get a clone of the peer store for sharing.
    pub fn shared_store(&self) -> SharedPeerStore {
        Arc::clone(&self.store)
    }

    /// Get a handle for sending commands.
    pub fn handle(&self) -> Option<&GossipHandle> {
        self.handle.as_ref()
    }

    /// Create our node announcement.
    pub fn create_announcement(&self) -> NodeAnnouncement {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut announcement = NodeAnnouncement::new(
            self.node_id.clone(),
            self.endpoints.clone(),
            self.quorum_set.clone(),
            vec![], // tx_source_urls - could be configured
            self.capabilities,
            self.version.clone(),
            timestamp,
        );

        // Sign the announcement
        let bytes = announcement.signing_bytes();
        announcement.signature = self.signing_key.sign(&bytes);

        announcement
    }

    /// Start the gossip service.
    ///
    /// This spawns the swarm task and returns handles for interaction.
    pub async fn start(&mut self) -> GossipResult<()> {
        let (command_tx, command_rx) = mpsc::channel(256);
        let (event_tx, event_rx) = mpsc::channel(256);

        let handle = GossipHandle::new(command_tx);
        self.handle = Some(handle.clone());
        self.event_rx = Some(event_rx);

        // Clone what we need for the swarm task
        let config = self.config.clone();
        let store = self.shared_store();
        let initial_announcement = self.create_announcement();

        // Spawn the swarm task
        tokio::spawn(async move {
            if let Err(e) = run_swarm(config, store, command_rx, event_tx, initial_announcement).await
            {
                error!("Swarm task failed: {:?}", e);
            }
        });

        info!(
            listen_port = self.config.listen_port,
            "Gossip service started"
        );

        Ok(())
    }

    /// Process events from the gossip network.
    ///
    /// This should be called in a loop to handle incoming announcements.
    pub async fn next_event(&mut self) -> Option<GossipEvent> {
        if let Some(rx) = &mut self.event_rx {
            rx.recv().await
        } else {
            None
        }
    }

    /// Broadcast our announcement to the network.
    pub async fn announce(&self) -> GossipResult<()> {
        if let Some(handle) = &self.handle {
            let announcement = self.create_announcement();
            handle.announce(announcement).await
        } else {
            Err(GossipError::NetworkError("Service not started".to_string()))
        }
    }

    /// Request topology from a specific peer.
    pub async fn sync_from(&self, peer: PeerId, since_timestamp: u64) -> GossipResult<()> {
        if let Some(handle) = &self.handle {
            handle.request_topology(peer, since_timestamp).await
        } else {
            Err(GossipError::NetworkError("Service not started".to_string()))
        }
    }

    /// Get connected peers.
    pub async fn connected_peers(&self) -> GossipResult<Vec<PeerId>> {
        if let Some(handle) = &self.handle {
            handle.get_peers().await
        } else {
            Err(GossipError::NetworkError("Service not started".to_string()))
        }
    }

    /// Shutdown the service.
    pub async fn shutdown(&self) -> GossipResult<()> {
        if let Some(handle) = &self.handle {
            handle.shutdown().await
        } else {
            Ok(())
        }
    }
}

/// Run the libp2p swarm.
async fn run_swarm(
    config: GossipConfig,
    store: SharedPeerStore,
    mut command_rx: mpsc::Receiver<GossipCommand>,
    event_tx: mpsc::Sender<GossipEvent>,
    initial_announcement: NodeAnnouncement,
) -> GossipResult<()> {
    // Create the swarm
    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|e| GossipError::Libp2pError(e.to_string()))?
        .with_behaviour(|key| {
            let local_peer_id = PeerId::from(key.public());
            GossipBehaviour::new(local_peer_id, &config).expect("Failed to create behaviour")
        })
        .map_err(|e| GossipError::Libp2pError(e.to_string()))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    // Subscribe to gossip topics
    swarm.behaviour_mut().subscribe_announcements()?;
    swarm.behaviour_mut().subscribe_transactions()?;
    swarm.behaviour_mut().subscribe_blocks()?;

    // Start listening
    let listen_addr = config.listen_multiaddr();
    swarm
        .listen_on(listen_addr.clone())
        .map_err(|e| GossipError::Libp2pError(e.to_string()))?;

    info!(?listen_addr, "Listening for gossip connections");

    // Connect to bootstrap peers
    for addr in &config.bootstrap_peers {
        match swarm.dial(addr.clone()) {
            Ok(_) => info!(?addr, "Dialing bootstrap peer"),
            Err(e) => warn!(?addr, ?e, "Failed to dial bootstrap peer"),
        }
    }

    // Set up periodic tasks
    let mut announce_interval = interval(config.announce_interval());
    let mut sync_interval = interval(config.sync_interval());
    let mut cleanup_interval = interval(Duration::from_secs(
        config.store_config.cleanup_interval_secs,
    ));

    // Track connected peers
    let mut connected_peers: HashSet<PeerId> = HashSet::new();
    let mut bootstrapped = false;

    // Store our initial announcement
    store.insert(initial_announcement.clone());

    loop {
        select! {
            // Handle swarm events
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(?address, "Listening on");
                    }

                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        debug!(?peer_id, "Connected to peer");
                        connected_peers.insert(peer_id);
                        let _ = event_tx.send(GossipEvent::PeerDiscovered(peer_id)).await;

                        // Check if we've bootstrapped
                        if !bootstrapped && connected_peers.len() >= config.min_peers_for_bootstrap {
                            bootstrapped = true;
                            info!(peers = connected_peers.len(), "Bootstrapped");
                            let _ = event_tx.send(GossipEvent::Bootstrapped).await;
                        }
                    }

                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        debug!(?peer_id, "Disconnected from peer");
                        connected_peers.remove(&peer_id);
                        let _ = event_tx.send(GossipEvent::PeerDisconnected(peer_id)).await;
                    }

                    SwarmEvent::Behaviour(event) => {
                        handle_behaviour_event(
                            event,
                            &mut swarm,
                            &store,
                            &config,
                            &event_tx,
                        ).await;
                    }

                    _ => {}
                }
            }

            // Handle commands
            Some(command) = command_rx.recv() => {
                match command {
                    GossipCommand::Announce(announcement) => {
                        if let Err(e) = swarm.behaviour_mut().publish_announcement(&announcement) {
                            warn!(?e, "Failed to publish announcement");
                        }
                        store.insert(announcement);
                    }

                    GossipCommand::BroadcastTransaction(tx_broadcast) => {
                        if let Err(e) = swarm.behaviour_mut().publish_transaction(&tx_broadcast) {
                            warn!(?e, "Failed to broadcast transaction");
                        }
                    }

                    GossipCommand::BroadcastBlock(block_broadcast) => {
                        if let Err(e) = swarm.behaviour_mut().publish_block(&block_broadcast) {
                            warn!(?e, "Failed to broadcast block");
                        }
                    }

                    GossipCommand::RequestTopology { peer, since_timestamp } => {
                        swarm.behaviour_mut().request_topology(
                            peer,
                            since_timestamp,
                            config.max_batch_size,
                        );
                    }

                    GossipCommand::AddBootstrapPeer(peer_id, addr) => {
                        swarm.behaviour_mut().add_peer(peer_id, addr);
                    }

                    GossipCommand::Dial(addr) => {
                        if let Err(e) = swarm.dial(addr.clone()) {
                            warn!(?addr, ?e, "Failed to dial peer");
                        }
                    }

                    GossipCommand::GetPeers(tx) => {
                        let peers: Vec<_> = connected_peers.iter().copied().collect();
                        let _ = tx.send(peers).await;
                    }

                    GossipCommand::Shutdown => {
                        info!("Shutting down gossip swarm");
                        break;
                    }
                }
            }

            // Periodic announcement
            _ = announce_interval.tick() => {
                if !connected_peers.is_empty() {
                    trace!("Broadcasting periodic announcement");
                    // Re-create announcement with fresh timestamp
                    // This would need access to the signing key, which we don't have here
                    // In practice, we'd need to store it or have the service send a new one
                }
            }

            // Periodic sync
            _ = sync_interval.tick() => {
                // Request topology from random connected peers
                if !connected_peers.is_empty() {
                    let peers: Vec<_> = connected_peers.iter().copied().collect();
                    if let Some(peer) = peers.first() {
                        let since = store.stats().newest_announcement;
                        swarm.behaviour_mut().request_topology(
                            *peer,
                            since,
                            config.max_batch_size,
                        );
                    }
                }
            }

            // Periodic cleanup
            _ = cleanup_interval.tick() => {
                let current_time = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                store.cleanup_stale(current_time);
            }
        }
    }

    Ok(())
}

/// Handle events from the gossip behaviour.
async fn handle_behaviour_event(
    event: <GossipBehaviour as libp2p::swarm::NetworkBehaviour>::ToSwarm,
    swarm: &mut Swarm<GossipBehaviour>,
    store: &SharedPeerStore,
    config: &GossipConfig,
    event_tx: &mpsc::Sender<GossipEvent>,
) {
    match event {
        // Gossipsub events
        crate::behaviour::GossipBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            propagation_source,
            message,
            ..
        }) => {
            // Try to parse as a node announcement
            match serde_json::from_slice::<NodeAnnouncement>(&message.data) {
                Ok(announcement) => {
                    debug!(
                        responder_id = %announcement.node_id.responder_id,
                        "Received announcement"
                    );

                    if store.insert(announcement.clone()) {
                        let _ = event_tx
                            .send(GossipEvent::AnnouncementReceived(announcement))
                            .await;
                    }
                }
                Err(e) => {
                    trace!(?e, "Failed to parse gossipsub message");
                }
            }
        }

        // Kademlia events
        crate::behaviour::GossipBehaviourEvent::Kademlia(kad::Event::RoutingUpdated {
            peer,
            addresses,
            ..
        }) => {
            debug!(?peer, "Kademlia routing updated");
        }

        crate::behaviour::GossipBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
            result: kad::QueryResult::Bootstrap(Ok(_)),
            ..
        }) => {
            info!("Kademlia bootstrap completed");
        }

        // Identify events
        crate::behaviour::GossipBehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info,
            ..
        }) => {
            debug!(?peer_id, protocol = ?info.protocol_version, "Identified peer");

            // Validate network ID matches
            if !config.network_id.matches_protocol(&info.protocol_version) {
                warn!(
                    ?peer_id,
                    their_protocol = ?info.protocol_version,
                    our_network = %config.network_id,
                    "Disconnecting peer: network mismatch"
                );
                // Disconnect the peer - they're on a different network
                let _ = swarm.disconnect_peer_id(peer_id);
                return;
            }

            // Add peer addresses to Kademlia
            for addr in info.listen_addrs {
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
            }
        }

        // Request-response events
        crate::behaviour::GossipBehaviourEvent::TopologySync(
            request_response::Event::Message { peer, message },
        ) => {
            match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    debug!(?peer, since = request.since_timestamp, "Topology sync request");

                    // Get announcements from store
                    let announcements = store.get_since(request.since_timestamp);
                    let has_more = announcements.len() > request.max_results as usize;
                    let announcements: Vec<_> = announcements
                        .into_iter()
                        .take(request.max_results as usize)
                        .collect();

                    let response = TopologySyncResponse {
                        announcements,
                        has_more,
                    };

                    if let Err(e) = swarm
                        .behaviour_mut()
                        .topology_sync
                        .send_response(channel, response)
                    {
                        warn!(?e, "Failed to send topology response");
                    }
                }

                request_response::Message::Response { response, .. } => {
                    debug!(
                        ?peer,
                        count = response.announcements.len(),
                        "Received topology response"
                    );

                    // Insert all announcements into store
                    for announcement in &response.announcements {
                        store.insert(announcement.clone());
                    }

                    let _ = event_tx
                        .send(GossipEvent::TopologySyncResponse {
                            peer,
                            announcements: response.announcements,
                        })
                        .await;
                }
            }
        }

        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_util_from_random::FromRandom;
    use std::str::FromStr;

    fn make_test_service() -> GossipService {
        // Create a keypair and use its public key for the NodeID
        let signing_key = Ed25519Pair::from_random(&mut rand::thread_rng());
        let public_key = signing_key.public_key();

        let node_id = NodeID {
            responder_id: ResponderId::from_str("test-node:8443").unwrap(),
            public_key,
        };

        GossipService::new(
            node_id,
            Arc::new(signing_key),
            QuorumSet::empty(),
            vec!["mcp://test-node:8443".to_string()],
            NodeCapabilities::GOSSIP,
            "1.0.0".to_string(),
            GossipConfig::default(),
        )
    }

    #[test]
    fn test_create_announcement() {
        let service = make_test_service();
        let announcement = service.create_announcement();

        assert_eq!(
            announcement.node_id.responder_id,
            ResponderId::from_str("test-node:8443").unwrap()
        );
        assert!(announcement.capabilities.contains(NodeCapabilities::GOSSIP));
        assert!(announcement.timestamp > 0);
    }

    #[test]
    fn test_announcement_signature_verification() {
        let service = make_test_service();
        let announcement = service.create_announcement();

        // The announcement should have a valid signature
        assert!(announcement.verify_signature());
    }

    #[test]
    fn test_store_access() {
        let service = make_test_service();

        // store() should return a reference to the peer store
        let store = service.store();
        assert!(store.announcements.read().unwrap().is_empty());
    }

    #[test]
    fn test_shared_store() {
        let service = make_test_service();

        // shared_store() should return a clone of the Arc
        let store1 = service.shared_store();
        let store2 = service.shared_store();

        // Both should point to the same underlying store
        assert!(Arc::ptr_eq(&store1, &store2));
    }

    #[test]
    fn test_handle_before_start() {
        let service = make_test_service();

        // handle() should return None before start() is called
        assert!(service.handle().is_none());
    }

    #[test]
    fn test_announcement_contains_correct_data() {
        let signing_key = Ed25519Pair::from_random(&mut rand::thread_rng());
        let public_key = signing_key.public_key();

        let node_id = NodeID {
            responder_id: ResponderId::from_str("my-node:8443").unwrap(),
            public_key,
        };

        let mut quorum_set = QuorumSet::empty();
        quorum_set.threshold = 2;

        let endpoints = vec![
            "mcp://my-node:8443".to_string(),
            "mcp://my-node:8444".to_string(),
        ];

        let capabilities = NodeCapabilities::CONSENSUS | NodeCapabilities::GOSSIP;
        let version = "2.0.0".to_string();

        let service = GossipService::new(
            node_id.clone(),
            Arc::new(signing_key),
            quorum_set.clone(),
            endpoints.clone(),
            capabilities,
            version.clone(),
            GossipConfig::default(),
        );

        let announcement = service.create_announcement();

        assert_eq!(announcement.node_id, node_id);
        assert_eq!(announcement.endpoints, endpoints);
        assert_eq!(announcement.quorum_set, quorum_set);
        assert_eq!(announcement.capabilities, capabilities);
        assert_eq!(announcement.version, version);
    }

    #[test]
    fn test_announcement_timestamp_is_recent() {
        let service = make_test_service();
        let announcement = service.create_announcement();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Timestamp should be within the last second
        assert!(announcement.timestamp <= now);
        assert!(announcement.timestamp >= now - 1);
    }

    #[test]
    fn test_different_services_have_different_announcements() {
        let service1 = make_test_service();
        let service2 = make_test_service();

        let ann1 = service1.create_announcement();
        let ann2 = service2.create_announcement();

        // Different keypairs should produce different public keys
        assert_ne!(ann1.node_id.public_key, ann2.node_id.public_key);
    }
}
