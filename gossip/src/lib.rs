// Copyright (c) 2024 Botho Foundation

//! Gossip-based peer discovery and topology sharing for Botho.
//!
//! This crate provides a gossip protocol layer that enables:
//!
//! - **Peer Discovery**: Nodes can find each other without static configuration
//! - **Topology Sharing**: Nodes share their quorum sets so new nodes can learn
//!   the network structure and make informed trust decisions
//! - **Push-Pull Sync**: Hybrid approach using gossipsub for real-time updates
//!   and request-response for bulk synchronization
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     GossipService                           │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
//! │  │  GossipHandle│  │  PeerStore   │  │  libp2p Swarm    │  │
//! │  │  (commands)  │  │  (topology)  │  │  (networking)    │  │
//! │  └──────────────┘  └──────────────┘  └──────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use bt_gossip::{GossipService, GossipConfig, NodeCapabilities};
//!
//! // Create the service
//! let config = GossipConfig::default();
//! let mut service = GossipService::new(
//!     node_id,
//!     signing_key,
//!     quorum_set,
//!     endpoints,
//!     NodeCapabilities::CONSENSUS | NodeCapabilities::GOSSIP,
//!     "1.0.0".to_string(),
//!     config,
//! );
//!
//! // Start the service
//! service.start().await?;
//!
//! // Process events
//! while let Some(event) = service.next_event().await {
//!     match event {
//!         GossipEvent::AnnouncementReceived(ann) => {
//!             println!("Discovered peer: {}", ann.node_id.responder_id);
//!         }
//!         GossipEvent::Bootstrapped => {
//!             println!("Connected to the network!");
//!         }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! # Message Types
//!
//! The gossip protocol uses several message types:
//!
//! - [`NodeAnnouncement`]: Signed advertisement of a node's identity, endpoints,
//!   quorum set, and capabilities
//! - [`GossipMessage`]: Wrapper enum for all protocol messages
//! - [`PeerInfo`]: Lightweight peer information for peer exchange
//!
//! # Peer Store
//!
//! The [`PeerStore`] maintains the view of known peers and their configurations.
//! It provides:
//!
//! - Signature verification for announcements
//! - Deduplication and freshness checks
//! - Trust graph queries (who trusts whom)
//! - Filtering by capabilities
//!
//! # Configuration
//!
//! See [`GossipConfig`] for all configuration options including:
//!
//! - Bootstrap peers
//! - Announce/sync intervals
//! - Connection limits
//! - libp2p protocol options

#![warn(missing_docs)]
#![warn(unused_extern_crates)]

pub mod analyzer;
pub mod behaviour;
pub mod config;
pub mod consensus_integration;
pub mod error;
pub mod messages;
pub mod service;
pub mod store;

// Re-export main types for convenience
pub use analyzer::{
    QuorumSetSuggestion, QuorumSetValidation, QuorumStrategy, TopologyAnalyzer, TopologyStats,
    TrustCluster,
};
pub use behaviour::{GossipBehaviour, GossipCommand, GossipEvent, GossipHandle};
pub use consensus_integration::{
    start_consensus_gossip, ConsensusGossipConfig, ConsensusGossipHandle,
};
pub use config::{GossipConfig, GossipConfigBuilder};
pub use error::{GossipError, GossipResult};
pub use messages::{
    BlockBroadcast, GossipMessage, NodeAnnouncement, NodeCapabilities, PeerInfo,
    TransactionBroadcast, ANNOUNCEMENTS_TOPIC, BLOCKS_TOPIC, PEER_EXCHANGE_TOPIC,
    TOPOLOGY_SYNC_PROTOCOL, TRANSACTIONS_TOPIC,
};
pub use service::GossipService;
pub use store::{new_shared_store, PeerStore, PeerStoreConfig, PeerStoreStats, SharedPeerStore};
