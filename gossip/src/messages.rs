// Copyright (c) 2024 Botho Foundation

//! Gossip protocol message types for peer discovery and topology sharing.
//!
//! These messages allow nodes to:
//! - Announce their existence and configuration
//! - Discover other nodes on the network
//! - Learn about network topology (who trusts whom)
//! - Suggest quorum set configurations based on observed trust patterns

use bth_common::{NodeID, ResponderId};
use bth_consensus_scp_types::QuorumSet;
use bth_crypto_keys::{Ed25519Public, Ed25519Signature};
use serde::{Deserialize, Serialize};

/// Capabilities advertised by a node.
///
/// These flags indicate what services a node provides to the network.
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct NodeCapabilities: u64 {
        /// Node participates in SCP consensus (can produce blocks)
        const CONSENSUS = 0b0000_0001;
        /// Node relays transactions to other nodes
        const RELAY = 0b0000_0010;
        /// Node serves historical transaction data (archive node)
        const ARCHIVE = 0b0000_0100;
        /// Node accepts client transaction submissions
        const CLIENT_API = 0b0000_1000;
        /// Node participates in gossip protocol
        const GOSSIP = 0b0001_0000;
    }
}

impl Default for NodeCapabilities {
    fn default() -> Self {
        Self::GOSSIP
    }
}

/// A signed announcement of a node's existence and configuration.
///
/// This is the primary message type for peer discovery. Nodes periodically
/// broadcast their announcements, and other nodes collect these to build
/// a view of the network topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAnnouncement {
    /// The node's identity (responder ID + public key)
    pub node_id: NodeID,

    /// Endpoints where this node can be reached.
    /// These are ConsensusPeerUri strings, e.g.:
    /// "mcp://node.example.com:8443?consensus-msg-key=..."
    pub endpoints: Vec<String>,

    /// The node's quorum set configuration - who this node trusts.
    /// This is critical for topology analysis and quorum set suggestions.
    pub quorum_set: QuorumSet<ResponderId>,

    /// URLs where transaction data can be fetched from this node.
    /// e.g., "https://node.example.com/ledger/" or "s3://bucket/prefix/"
    pub tx_source_urls: Vec<String>,

    /// What capabilities this node provides
    pub capabilities: NodeCapabilities,

    /// Software version string
    pub version: String,

    /// Unix timestamp (seconds since epoch) when this announcement was created.
    /// Used to determine freshness and prevent replay of old announcements.
    pub timestamp: u64,

    /// Signature over all fields above, proving the announcement came from
    /// the node with `node_id.public_key`.
    #[serde(with = "signature_serde")]
    pub signature: Ed25519Signature,
}

impl NodeAnnouncement {
    /// Create a new unsigned announcement (signature will be zeroed).
    /// Call `sign()` to add a valid signature.
    pub fn new(
        node_id: NodeID,
        endpoints: Vec<String>,
        quorum_set: QuorumSet<ResponderId>,
        tx_source_urls: Vec<String>,
        capabilities: NodeCapabilities,
        version: String,
        timestamp: u64,
    ) -> Self {
        Self {
            node_id,
            endpoints,
            quorum_set,
            tx_source_urls,
            capabilities,
            version,
            timestamp,
            signature: Ed25519Signature::default(),
        }
    }

    /// Get the bytes that should be signed/verified.
    pub fn signing_bytes(&self) -> Vec<u8> {
        // Create a copy without signature for hashing
        let mut bytes = Vec::new();
        bytes.extend_from_slice(self.node_id.responder_id.to_string().as_bytes());
        bytes.extend_from_slice(self.node_id.public_key.as_ref());
        for endpoint in &self.endpoints {
            bytes.extend_from_slice(endpoint.as_bytes());
        }
        // Serialize quorum set
        if let Ok(qs_bytes) = bth_util_serial::serialize(&self.quorum_set) {
            bytes.extend_from_slice(&qs_bytes);
        }
        for url in &self.tx_source_urls {
            bytes.extend_from_slice(url.as_bytes());
        }
        bytes.extend_from_slice(&self.capabilities.bits().to_le_bytes());
        bytes.extend_from_slice(self.version.as_bytes());
        bytes.extend_from_slice(&self.timestamp.to_le_bytes());
        bytes
    }

    /// Verify the signature on this announcement.
    pub fn verify_signature(&self) -> bool {
        use bth_crypto_keys::Verifier;
        let bytes = self.signing_bytes();
        self.node_id
            .public_key
            .verify(&bytes, &self.signature)
            .is_ok()
    }

    /// Check if this announcement is newer than another.
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.timestamp > other.timestamp
    }

    /// Check if this announcement is expired (older than max_age_secs).
    pub fn is_expired(&self, current_time: u64, max_age_secs: u64) -> bool {
        current_time.saturating_sub(self.timestamp) > max_age_secs
    }
}

/// Lightweight peer info for peer exchange.
/// Contains just enough info to initiate a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// The node's responder ID (host:port)
    pub responder_id: ResponderId,

    /// The node's public key
    #[serde(with = "pubkey_serde")]
    pub public_key: Ed25519Public,

    /// Primary endpoint URI
    pub endpoint: String,

    /// Last known timestamp of this peer's announcement
    pub last_seen: u64,
}

impl From<&NodeAnnouncement> for PeerInfo {
    fn from(ann: &NodeAnnouncement) -> Self {
        Self {
            responder_id: ann.node_id.responder_id.clone(),
            public_key: ann.node_id.public_key,
            endpoint: ann.endpoints.first().cloned().unwrap_or_default(),
            last_seen: ann.timestamp,
        }
    }
}

/// Messages exchanged in the gossip protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GossipMessage {
    /// Push: Broadcast a node announcement
    Announce(NodeAnnouncement),

    /// Pull request: Request topology from a peer
    RequestTopology {
        /// Only send announcements newer than this timestamp
        since_timestamp: u64,
        /// Maximum number of announcements to return
        max_results: u32,
    },

    /// Pull response: Batch of announcements
    TopologySnapshot {
        /// The announcements known to the sender
        announcements: Vec<NodeAnnouncement>,
        /// Whether there are more announcements available
        has_more: bool,
    },

    /// Lightweight peer exchange - just endpoint info
    PeerExchange {
        /// Known peers
        peers: Vec<PeerInfo>,
    },

    /// Request peer exchange
    RequestPeers {
        /// Maximum number of peers to return
        max_results: u32,
    },
}

/// Gossipsub topic for node announcements.
pub const ANNOUNCEMENTS_TOPIC: &str = "/botho/announcements/1.0.0";

/// Gossipsub topic for peer exchange.
pub const PEER_EXCHANGE_TOPIC: &str = "/botho/peers/1.0.0";

/// Gossipsub topic for new transactions.
pub const TRANSACTIONS_TOPIC: &str = "/botho/transactions/1.0.0";

/// Gossipsub topic for new blocks.
pub const BLOCKS_TOPIC: &str = "/botho/blocks/1.0.0";

/// Protocol ID for request-response topology sync.
pub const TOPOLOGY_SYNC_PROTOCOL: &str = "/botho/topology-sync/1.0.0";

/// A transaction broadcast message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionBroadcast {
    /// Serialized transaction data
    pub tx_data: Vec<u8>,
    /// Transaction hash (for deduplication)
    pub tx_hash: [u8; 32],
    /// Timestamp when broadcast
    pub timestamp: u64,
}

/// A block broadcast message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockBroadcast {
    /// Serialized block data
    pub block_data: Vec<u8>,
    /// Block hash
    pub block_hash: [u8; 32],
    /// Block height
    pub height: u64,
    /// Timestamp when broadcast
    pub timestamp: u64,
}

// Serde helpers for Ed25519 types

mod signature_serde {
    use bth_crypto_keys::Ed25519Signature;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(sig: &Ed25519Signature, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes: &[u8] = sig.as_ref();
        hex::encode(bytes).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Ed25519Signature, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_str = String::deserialize(deserializer)?;
        let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        Ed25519Signature::try_from(bytes.as_slice()).map_err(serde::de::Error::custom)
    }
}

mod pubkey_serde {
    use bth_crypto_keys::Ed25519Public;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(key: &Ed25519Public, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes: &[u8] = key.as_ref();
        hex::encode(bytes).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Ed25519Public, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_str = String::deserialize(deserializer)?;
        let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        Ed25519Public::try_from(bytes.as_slice()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_consensus_scp_types::QuorumSetMember;
    use std::str::FromStr;

    fn make_test_node_id(name: &str) -> NodeID {
        NodeID {
            responder_id: ResponderId::from_str(&format!("{name}:8443")).unwrap(),
            public_key: Ed25519Public::default(),
        }
    }

    #[test]
    fn test_node_capabilities() {
        let caps = NodeCapabilities::CONSENSUS | NodeCapabilities::RELAY;
        assert!(caps.contains(NodeCapabilities::CONSENSUS));
        assert!(caps.contains(NodeCapabilities::RELAY));
        assert!(!caps.contains(NodeCapabilities::ARCHIVE));
    }

    #[test]
    fn test_announcement_creation() {
        let node_id = make_test_node_id("node1.example.com");
        let quorum_set = QuorumSet::new(
            2,
            vec![
                QuorumSetMember::Node(ResponderId::from_str("peer1:8443").unwrap()),
                QuorumSetMember::Node(ResponderId::from_str("peer2:8443").unwrap()),
            ],
        );

        let announcement = NodeAnnouncement::new(
            node_id.clone(),
            vec!["mcp://node1.example.com:8443".to_string()],
            quorum_set,
            vec!["https://node1.example.com/ledger/".to_string()],
            NodeCapabilities::CONSENSUS | NodeCapabilities::RELAY,
            "1.0.0".to_string(),
            1234567890,
        );

        assert_eq!(announcement.node_id, node_id);
        assert_eq!(announcement.endpoints.len(), 1);
        assert!(announcement
            .capabilities
            .contains(NodeCapabilities::CONSENSUS));
    }

    #[test]
    fn test_announcement_expiry() {
        let node_id = make_test_node_id("node1.example.com");
        let announcement = NodeAnnouncement::new(
            node_id,
            vec![],
            QuorumSet::empty(),
            vec![],
            NodeCapabilities::default(),
            "1.0.0".to_string(),
            1000,
        );

        // Not expired if current time is close
        assert!(!announcement.is_expired(1100, 3600));

        // Expired if max_age exceeded
        assert!(announcement.is_expired(5000, 3600));
    }

    #[test]
    fn test_peer_info_from_announcement() {
        let node_id = make_test_node_id("node1.example.com");
        let announcement = NodeAnnouncement::new(
            node_id.clone(),
            vec!["mcp://node1.example.com:8443".to_string()],
            QuorumSet::empty(),
            vec![],
            NodeCapabilities::default(),
            "1.0.0".to_string(),
            1234567890,
        );

        let peer_info = PeerInfo::from(&announcement);
        assert_eq!(peer_info.responder_id, node_id.responder_id);
        assert_eq!(peer_info.endpoint, "mcp://node1.example.com:8443");
        assert_eq!(peer_info.last_seen, 1234567890);
    }

    #[test]
    fn test_gossip_message_serialization() {
        let msg = GossipMessage::RequestTopology {
            since_timestamp: 1234567890,
            max_results: 100,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: GossipMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            GossipMessage::RequestTopology {
                since_timestamp,
                max_results,
            } => {
                assert_eq!(since_timestamp, 1234567890);
                assert_eq!(max_results, 100);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
