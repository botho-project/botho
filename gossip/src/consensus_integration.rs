// Copyright (c) 2024 Cadence Foundation

//! Integration module for using gossip with the consensus service.
//!
//! This module provides helpers for:
//! - Creating gossip announcements from consensus service configuration
//! - Starting a gossip service alongside the consensus service
//! - Syncing discovered peers with the consensus peer manager

use crate::{
    GossipConfig, GossipConfigBuilder, GossipEvent, GossipService, NodeCapabilities,
    SharedPeerStore,
};
use mc_common::{NodeID, ResponderId};
use mc_consensus_scp_types::QuorumSet;
use mc_crypto_keys::Ed25519Pair;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Configuration for gossip integration with consensus.
#[derive(Debug, Clone)]
pub struct ConsensusGossipConfig {
    /// Enable gossip-based peer discovery
    pub enabled: bool,

    /// Port for gossip (libp2p) connections
    pub gossip_port: u16,

    /// Bootstrap peers for initial discovery
    pub bootstrap_peers: Vec<String>,

    /// Announce interval in seconds
    pub announce_interval_secs: u64,

    /// Whether to automatically update peer connections based on discovered topology
    pub auto_connect_peers: bool,

    /// Maximum peers to auto-connect to
    pub max_auto_connect: usize,
}

impl Default for ConsensusGossipConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gossip_port: 7100,
            bootstrap_peers: vec![],
            announce_interval_secs: 300,
            auto_connect_peers: true,
            max_auto_connect: 10,
        }
    }
}

/// Handle for interacting with the gossip service from consensus.
pub struct ConsensusGossipHandle {
    /// Shared peer store for accessing discovered topology
    store: SharedPeerStore,

    /// Channel for receiving significant gossip events
    event_rx: mpsc::Receiver<GossipEvent>,

    /// Join handle for the gossip task
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ConsensusGossipHandle {
    /// Get the peer store for querying discovered nodes.
    pub fn store(&self) -> &SharedPeerStore {
        &self.store
    }

    /// Try to receive a gossip event without blocking.
    pub fn try_recv_event(&mut self) -> Option<GossipEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Wait for the next gossip event.
    pub async fn recv_event(&mut self) -> Option<GossipEvent> {
        self.event_rx.recv().await
    }

    /// Get newly discovered consensus-capable peers.
    ///
    /// Returns peers that have the CONSENSUS capability and are not already
    /// in the provided known set.
    pub fn get_new_consensus_peers(
        &self,
        known: &[ResponderId],
    ) -> Vec<(ResponderId, Vec<String>)> {
        let known_set: std::collections::HashSet<_> = known.iter().collect();

        self.store
            .get_consensus_nodes()
            .into_iter()
            .filter(|ann| !known_set.contains(&ann.node_id.responder_id))
            .map(|ann| (ann.node_id.responder_id, ann.endpoints))
            .collect()
    }

    /// Shutdown the gossip service.
    pub async fn shutdown(self) {
        if let Some(handle) = self.task_handle {
            handle.abort();
            let _ = handle.await;
        }
    }
}

/// Start a gossip service configured for consensus node integration.
///
/// This creates a gossip service that:
/// - Announces this node's identity and quorum set
/// - Discovers other consensus nodes
/// - Provides a handle for querying discovered topology
pub async fn start_consensus_gossip(
    node_id: NodeID,
    signing_key: Arc<Ed25519Pair>,
    quorum_set: QuorumSet<ResponderId>,
    peer_endpoints: Vec<String>,
    tx_source_urls: Vec<String>,
    gossip_config: ConsensusGossipConfig,
) -> Result<ConsensusGossipHandle, crate::GossipError> {
    let config = GossipConfigBuilder::new()
        .listen_port(gossip_config.gossip_port)
        .bootstrap_peers(
            gossip_config
                .bootstrap_peers
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect(),
        )
        .announce_interval_secs(gossip_config.announce_interval_secs)
        .sync_interval_secs(30)
        .build();

    let capabilities = NodeCapabilities::CONSENSUS | NodeCapabilities::GOSSIP | NodeCapabilities::RELAY;

    let mut service = GossipService::new(
        node_id.clone(),
        signing_key,
        quorum_set,
        peer_endpoints,
        capabilities,
        env!("CARGO_PKG_VERSION").to_string(),
        config,
    );

    let store = service.shared_store();

    // Start the gossip service
    service.start().await?;

    // Create event channel
    let (event_tx, event_rx) = mpsc::channel(256);

    // Spawn task to forward events
    let task_handle = tokio::spawn(async move {
        info!("Gossip integration started for node {}", node_id.responder_id);

        loop {
            match service.next_event().await {
                Some(event) => {
                    match &event {
                        GossipEvent::Bootstrapped => {
                            info!("Gossip network bootstrapped");
                        }
                        GossipEvent::PeerDiscovered(peer) => {
                            debug!(?peer, "Discovered gossip peer");
                        }
                        GossipEvent::AnnouncementReceived(ann) => {
                            debug!(
                                responder_id = %ann.node_id.responder_id,
                                consensus = ann.capabilities.contains(NodeCapabilities::CONSENSUS),
                                "Received node announcement"
                            );
                        }
                        GossipEvent::Error(e) => {
                            warn!(?e, "Gossip error");
                        }
                        _ => {}
                    }

                    // Forward significant events
                    if event_tx.send(event).await.is_err() {
                        break;
                    }
                }
                None => {
                    info!("Gossip service stopped");
                    break;
                }
            }
        }
    });

    Ok(ConsensusGossipHandle {
        store,
        event_rx,
        task_handle: Some(task_handle),
    })
}

/// Helper to create peer URIs from discovered endpoints.
pub fn endpoints_to_peer_uris(endpoints: &[String]) -> Vec<String> {
    endpoints
        .iter()
        .filter(|e| e.starts_with("mcp://") || e.starts_with("insecure-mcp://"))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ConsensusGossipConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.gossip_port, 7100);
        assert!(config.auto_connect_peers);
    }

    #[test]
    fn test_endpoints_to_peer_uris() {
        let endpoints = vec![
            "mcp://node1.example.com:8443".to_string(),
            "insecure-mcp://node2.example.com:8080".to_string(),
            "https://node1.example.com/ledger".to_string(), // Not a peer URI
        ];

        let uris = endpoints_to_peer_uris(&endpoints);
        assert_eq!(uris.len(), 2);
        assert!(uris[0].starts_with("mcp://"));
        assert!(uris[1].starts_with("insecure-mcp://"));
    }
}
