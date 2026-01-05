// Copyright (c) 2024 Botho Foundation

//! Network module for gossip-based peer discovery and quorum management.
//!
//! This module handles:
//! - Starting the gossip service for peer discovery
//! - Displaying discovered peers in a table format
//! - Suggesting and validating quorum set configurations
//! - Chain synchronization with DDoS protections

mod compact_block;
mod connection_limiter;
mod discovery;
mod dns_seeds;
mod pex;
pub mod privacy;
mod quorum;
mod reputation;
mod sync;
pub mod transport;

pub use compact_block::{
    BlockTxn, CompactBlock, GetBlockTxn, PrefilledTx, ReconstructionResult, ShortId,
};
pub use connection_limiter::{
    ConnectionLimitExceeded, ConnectionLimiter, ConnectionLimiterMetrics,
    DEFAULT_MAX_CONNECTIONS_PER_IP,
};
pub use discovery::{
    BothoBehaviour, NetworkDiscovery, NetworkEvent, PeerTableEntry, ProtocolVersion,
    UpgradeAnnouncement, MIN_SUPPORTED_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
// Re-export rate limit configuration from bth-gossip for network setup
pub use bth_gossip::{GossipMessageType, MessageTypeLimits, PeerRateLimitConfig};
pub use dns_seeds::{DnsSeedDiscovery, DnsSeedError};
pub use pex::{
    PeerSource, PexEntry, PexFilter, PexManager, PexMessage, PexRateLimiter, PexSourceTracker,
    MAX_PEERS_PER_SUBNET, MAX_PEER_AGE_SECS, MAX_PEX_MESSAGE_SIZE, MAX_PEX_PEERS, MAX_PEX_PER_HOUR,
    PEX_INTERVAL_SECS,
};
pub use quorum::{QuorumBuilder, QuorumValidation};
pub use reputation::{PeerReputation, ReputationManager};
pub use sync::{
    create_sync_behaviour, ChainSyncManager, SyncAction, SyncCodec, SyncRateLimiter, SyncRequest,
    SyncResponse, SyncState, BLOCKS_PER_REQUEST, MAX_REQUESTS_PER_MINUTE, MAX_REQUEST_SIZE,
    MAX_RESPONSE_SIZE,
};
pub use transport::{
    negotiate_transport_initiator, negotiate_transport_responder, select_transport, NatType,
    NegotiationConfig, NegotiationError, NegotiationMessage, TransportCapabilities,
    TransportError, TransportManager, TransportManagerConfig, TransportType, UpgradeResult,
};
