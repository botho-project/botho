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
mod quorum;
mod reputation;
mod sync;

pub use compact_block::{
    BlockTxn, CompactBlock, GetBlockTxn, PrefilledTx, ReconstructionResult, ShortId,
};
pub use connection_limiter::{
    ConnectionLimitExceeded, ConnectionLimiter, ConnectionLimiterMetrics,
    DEFAULT_MAX_CONNECTIONS_PER_IP,
};
pub use discovery::{BothoBehaviour, NetworkDiscovery, NetworkEvent, PeerTableEntry};
pub use quorum::{QuorumBuilder, QuorumValidation};
pub use reputation::{PeerReputation, ReputationManager};
pub use sync::{
    create_sync_behaviour, ChainSyncManager, SyncAction, SyncCodec, SyncRateLimiter, SyncRequest,
    SyncResponse, SyncState, BLOCKS_PER_REQUEST, MAX_REQUEST_SIZE, MAX_REQUESTS_PER_MINUTE,
    MAX_RESPONSE_SIZE,
};
