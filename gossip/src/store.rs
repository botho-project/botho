// Copyright (c) 2024 Botho Foundation

//! In-memory store for tracking discovered peers and their announcements.
//!
//! The `PeerStore` maintains a view of the network topology by collecting
//! and managing `NodeAnnouncement` messages from gossip.

use crate::messages::{NodeAnnouncement, NodeCapabilities, PeerInfo};
use bth_common::{HashMap, HashSet, NodeID, ResponderId};
use std::sync::{Arc, RwLock};

/// Configuration for the peer store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerStoreConfig {
    /// Maximum number of peers to track
    pub max_peers: usize,

    /// Maximum age of announcements before they're considered stale (seconds)
    pub max_announcement_age_secs: u64,

    /// How often to run cleanup of stale entries (seconds)
    pub cleanup_interval_secs: u64,
}

impl Default for PeerStoreConfig {
    fn default() -> Self {
        Self {
            max_peers: 10000,
            max_announcement_age_secs: 24 * 60 * 60, // 24 hours
            cleanup_interval_secs: 5 * 60,           // 5 minutes
        }
    }
}

/// Statistics about the peer store.
#[derive(Debug, Clone, Default)]
pub struct PeerStoreStats {
    /// Total number of known peers
    pub total_peers: usize,

    /// Number of peers with CONSENSUS capability
    pub consensus_nodes: usize,

    /// Number of peers with ARCHIVE capability
    pub archive_nodes: usize,

    /// Number of peers with RELAY capability
    pub relay_nodes: usize,

    /// Timestamp of the newest announcement
    pub newest_announcement: u64,

    /// Timestamp of the oldest announcement
    pub oldest_announcement: u64,
}

/// Thread-safe store for peer announcements.
#[derive(Debug)]
pub struct PeerStore {
    /// Configuration
    config: PeerStoreConfig,

    /// Map from ResponderID to the latest announcement
    announcements: RwLock<HashMap<ResponderId, NodeAnnouncement>>,

    /// Index: NodeID public key -> ResponderID (for lookups by key)
    key_index: RwLock<HashMap<[u8; 32], ResponderId>>,
}

impl PeerStore {
    /// Create a new peer store with the given configuration.
    pub fn new(config: PeerStoreConfig) -> Self {
        Self {
            config,
            announcements: RwLock::new(HashMap::default()),
            key_index: RwLock::new(HashMap::default()),
        }
    }

    /// Create a new peer store with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PeerStoreConfig::default())
    }

    /// Insert or update an announcement.
    ///
    /// Returns `true` if the announcement was accepted (new or newer than existing).
    pub fn insert(&self, announcement: NodeAnnouncement) -> bool {
        // Verify signature before accepting
        if !announcement.verify_signature() {
            tracing::warn!(
                responder_id = %announcement.node_id.responder_id,
                "Rejecting announcement with invalid signature"
            );
            return false;
        }

        let responder_id = announcement.node_id.responder_id.clone();
        let pk_slice: &[u8] = announcement.node_id.public_key.as_ref();
        let public_key_bytes: [u8; 32] = pk_slice.try_into().unwrap_or([0u8; 32]);

        let mut announcements = self.announcements.write().unwrap();

        // Check if we already have a newer announcement
        if let Some(existing) = announcements.get(&responder_id) {
            if !announcement.is_newer_than(existing) {
                return false;
            }
        }

        // Check capacity
        if announcements.len() >= self.config.max_peers
            && !announcements.contains_key(&responder_id)
        {
            // Store is full and this is a new peer - try to evict oldest
            if !self.evict_oldest_locked(&mut announcements) {
                tracing::warn!("Peer store full, rejecting new announcement");
                return false;
            }
        }

        // Insert the announcement
        announcements.insert(responder_id.clone(), announcement);

        // Update key index
        let mut key_index = self.key_index.write().unwrap();
        key_index.insert(public_key_bytes, responder_id);

        true
    }

    /// Get an announcement by responder ID.
    pub fn get(&self, responder_id: &ResponderId) -> Option<NodeAnnouncement> {
        let announcements = self.announcements.read().unwrap();
        announcements.get(responder_id).cloned()
    }

    /// Get an announcement by public key.
    pub fn get_by_key(&self, public_key: &[u8; 32]) -> Option<NodeAnnouncement> {
        let key_index = self.key_index.read().unwrap();
        if let Some(responder_id) = key_index.get(public_key) {
            let announcements = self.announcements.read().unwrap();
            return announcements.get(responder_id).cloned();
        }
        None
    }

    /// Get all announcements.
    pub fn get_all(&self) -> Vec<NodeAnnouncement> {
        let announcements = self.announcements.read().unwrap();
        announcements.values().cloned().collect()
    }

    /// Get announcements newer than a given timestamp.
    pub fn get_since(&self, since_timestamp: u64) -> Vec<NodeAnnouncement> {
        let announcements = self.announcements.read().unwrap();
        announcements
            .values()
            .filter(|a| a.timestamp > since_timestamp)
            .cloned()
            .collect()
    }

    /// Get all known responder IDs.
    pub fn get_responder_ids(&self) -> Vec<ResponderId> {
        let announcements = self.announcements.read().unwrap();
        announcements.keys().cloned().collect()
    }

    /// Get all known node IDs.
    pub fn get_node_ids(&self) -> Vec<NodeID> {
        let announcements = self.announcements.read().unwrap();
        announcements.values().map(|a| a.node_id.clone()).collect()
    }

    /// Get peer info for all known peers.
    pub fn get_peer_infos(&self) -> Vec<PeerInfo> {
        let announcements = self.announcements.read().unwrap();
        announcements.values().map(PeerInfo::from).collect()
    }

    /// Get nodes with specific capabilities.
    pub fn get_with_capabilities(&self, required: NodeCapabilities) -> Vec<NodeAnnouncement> {
        let announcements = self.announcements.read().unwrap();
        announcements
            .values()
            .filter(|a| a.capabilities.contains(required))
            .cloned()
            .collect()
    }

    /// Get all consensus-capable nodes.
    pub fn get_consensus_nodes(&self) -> Vec<NodeAnnouncement> {
        self.get_with_capabilities(NodeCapabilities::CONSENSUS)
    }

    /// Remove stale announcements.
    ///
    /// Returns the number of entries removed.
    pub fn cleanup_stale(&self, current_time: u64) -> usize {
        let mut announcements = self.announcements.write().unwrap();
        let mut key_index = self.key_index.write().unwrap();

        let stale_ids: Vec<_> = announcements
            .iter()
            .filter(|(_, a)| a.is_expired(current_time, self.config.max_announcement_age_secs))
            .map(|(id, a)| {
                let pk_slice: &[u8] = a.node_id.public_key.as_ref();
                let key_bytes: [u8; 32] = pk_slice.try_into().unwrap_or([0u8; 32]);
                (id.clone(), key_bytes)
            })
            .collect();

        let count = stale_ids.len();
        for (responder_id, key_bytes) in stale_ids {
            announcements.remove(&responder_id);
            key_index.remove(&key_bytes);
        }

        if count > 0 {
            tracing::info!(removed = count, "Cleaned up stale announcements");
        }

        count
    }

    /// Get statistics about the store.
    pub fn stats(&self) -> PeerStoreStats {
        let announcements = self.announcements.read().unwrap();

        let mut stats = PeerStoreStats {
            total_peers: announcements.len(),
            ..Default::default()
        };

        for announcement in announcements.values() {
            if announcement.capabilities.contains(NodeCapabilities::CONSENSUS) {
                stats.consensus_nodes += 1;
            }
            if announcement.capabilities.contains(NodeCapabilities::ARCHIVE) {
                stats.archive_nodes += 1;
            }
            if announcement.capabilities.contains(NodeCapabilities::RELAY) {
                stats.relay_nodes += 1;
            }

            if stats.newest_announcement == 0 || announcement.timestamp > stats.newest_announcement
            {
                stats.newest_announcement = announcement.timestamp;
            }
            if stats.oldest_announcement == 0 || announcement.timestamp < stats.oldest_announcement
            {
                stats.oldest_announcement = announcement.timestamp;
            }
        }

        stats
    }

    /// Get the number of peers in the store.
    pub fn len(&self) -> usize {
        self.announcements.read().unwrap().len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if a peer is known.
    pub fn contains(&self, responder_id: &ResponderId) -> bool {
        self.announcements
            .read()
            .unwrap()
            .contains_key(responder_id)
    }

    /// Get the trust graph: who trusts whom.
    ///
    /// Returns a map from each node to the set of nodes it trusts.
    pub fn get_trust_graph(&self) -> HashMap<ResponderId, HashSet<ResponderId>> {
        let announcements = self.announcements.read().unwrap();
        let mut graph = HashMap::default();

        for (responder_id, announcement) in announcements.iter() {
            let trusted: HashSet<_> = announcement.quorum_set.nodes().into_iter().collect();
            graph.insert(responder_id.clone(), trusted);
        }

        graph
    }

    /// Get incoming trust: who trusts a given node.
    pub fn get_trusters(&self, node: &ResponderId) -> Vec<ResponderId> {
        let announcements = self.announcements.read().unwrap();

        announcements
            .iter()
            .filter(|(_, a)| a.quorum_set.nodes().contains(node))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Count how many nodes trust a given node.
    pub fn trust_count(&self, node: &ResponderId) -> usize {
        self.get_trusters(node).len()
    }

    // Internal helper to evict the oldest entry when store is full
    fn evict_oldest_locked(
        &self,
        announcements: &mut HashMap<ResponderId, NodeAnnouncement>,
    ) -> bool {
        if let Some((oldest_id, _)) = announcements
            .iter()
            .min_by_key(|(_, a)| a.timestamp)
            .map(|(id, a)| (id.clone(), a.timestamp))
        {
            let removed = announcements.remove(&oldest_id);
            if let Some(removed_ann) = removed {
                let pk_slice: &[u8] = removed_ann.node_id.public_key.as_ref();
                let key_bytes: [u8; 32] = pk_slice.try_into().unwrap_or([0u8; 32]);
                let mut key_index = self.key_index.write().unwrap();
                key_index.remove(&key_bytes);
            }
            return true;
        }
        false
    }
}

/// A shared, reference-counted peer store.
pub type SharedPeerStore = Arc<PeerStore>;

/// Create a new shared peer store.
pub fn new_shared_store(config: PeerStoreConfig) -> SharedPeerStore {
    Arc::new(PeerStore::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_consensus_scp_types::QuorumSetMember;
    use bth_crypto_keys::Ed25519Public;
    use std::str::FromStr;

    fn make_test_announcement(name: &str, timestamp: u64) -> NodeAnnouncement {
        let node_id = NodeID {
            responder_id: ResponderId::from_str(&format!("{name}:8443")).unwrap(),
            public_key: Ed25519Public::default(),
        };

        // Create a simple quorum set
        let quorum_set = QuorumSet::new(
            1,
            vec![QuorumSetMember::Node(
                ResponderId::from_str("trusted-peer:8443").unwrap(),
            )],
        );

        NodeAnnouncement::new(
            node_id,
            vec![format!("mcp://{name}:8443")],
            quorum_set,
            vec![],
            NodeCapabilities::CONSENSUS | NodeCapabilities::GOSSIP,
            "1.0.0".to_string(),
            timestamp,
        )
    }

    #[test]
    fn test_store_insert_and_get() {
        let store = PeerStore::with_defaults();
        let mut ann = make_test_announcement("node1", 1000);
        // Skip signature verification for test by directly inserting
        // In real usage, announcements would be signed

        // For testing, we need to bypass signature verification
        // This is a limitation of the test - in production, all announcements are signed
        let announcements = &store.announcements;
        {
            let mut guard = announcements.write().unwrap();
            guard.insert(ann.node_id.responder_id.clone(), ann.clone());
        }

        assert_eq!(store.len(), 1);
        assert!(store.contains(&ann.node_id.responder_id));

        let retrieved = store.get(&ann.node_id.responder_id).unwrap();
        assert_eq!(retrieved.timestamp, 1000);
    }

    #[test]
    fn test_store_stats() {
        let store = PeerStore::with_defaults();

        // Manually insert test announcements
        {
            let mut guard = store.announcements.write().unwrap();

            let mut ann1 = make_test_announcement("node1", 1000);
            ann1.capabilities = NodeCapabilities::CONSENSUS;
            guard.insert(ann1.node_id.responder_id.clone(), ann1);

            let mut ann2 = make_test_announcement("node2", 2000);
            ann2.capabilities = NodeCapabilities::ARCHIVE;
            guard.insert(ann2.node_id.responder_id.clone(), ann2);
        }

        let stats = store.stats();
        assert_eq!(stats.total_peers, 2);
        assert_eq!(stats.consensus_nodes, 1);
        assert_eq!(stats.archive_nodes, 1);
        assert_eq!(stats.oldest_announcement, 1000);
        assert_eq!(stats.newest_announcement, 2000);
    }

    #[test]
    fn test_store_cleanup() {
        let config = PeerStoreConfig {
            max_announcement_age_secs: 100,
            ..Default::default()
        };
        let store = PeerStore::new(config);

        // Insert announcements with different timestamps
        {
            let mut guard = store.announcements.write().unwrap();

            let ann1 = make_test_announcement("old-node", 100);
            guard.insert(ann1.node_id.responder_id.clone(), ann1);

            let ann2 = make_test_announcement("new-node", 900);
            guard.insert(ann2.node_id.responder_id.clone(), ann2);
        }

        assert_eq!(store.len(), 2);

        // Cleanup at time 500 - old-node should be removed (500 - 100 = 400 > 100)
        let removed = store.cleanup_stale(500);
        assert_eq!(removed, 1);
        assert_eq!(store.len(), 1);

        // new-node should still be there
        assert!(store.contains(&ResponderId::from_str("new-node:8443").unwrap()));
    }

    #[test]
    fn test_get_consensus_nodes() {
        let store = PeerStore::with_defaults();

        {
            let mut guard = store.announcements.write().unwrap();

            let mut consensus_node = make_test_announcement("consensus", 1000);
            consensus_node.capabilities = NodeCapabilities::CONSENSUS;
            guard.insert(
                consensus_node.node_id.responder_id.clone(),
                consensus_node,
            );

            let mut relay_node = make_test_announcement("relay", 1000);
            relay_node.capabilities = NodeCapabilities::RELAY;
            guard.insert(relay_node.node_id.responder_id.clone(), relay_node);
        }

        let consensus_nodes = store.get_consensus_nodes();
        assert_eq!(consensus_nodes.len(), 1);
        assert_eq!(
            consensus_nodes[0].node_id.responder_id,
            ResponderId::from_str("consensus:8443").unwrap()
        );
    }
}
