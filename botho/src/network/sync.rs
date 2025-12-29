// Copyright (c) 2024 Botho Foundation

//! Chain synchronization protocol for downloading historical blocks from peers.
//!
//! Uses libp2p request-response pattern with DDoS protections:
//! - Message size limits prevent memory exhaustion
//! - Per-peer rate limiting prevents request flooding
//! - Request count caps prevent abuse

use futures::prelude::*;
use libp2p::{
    request_response::{self, Codec, ProtocolSupport},
    PeerId, StreamProtocol,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

use crate::block::Block;

// ============================================================================
// DDoS Protection Constants
// ============================================================================

/// Maximum size of incoming request messages (1 KB)
pub const MAX_REQUEST_SIZE: u64 = 1024;

/// Maximum size of incoming response messages (10 MB - ~100 blocks)
pub const MAX_RESPONSE_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum requests per peer per minute
pub const MAX_REQUESTS_PER_MINUTE: u32 = 60;

/// Rate limit window duration
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Number of blocks to request per batch
pub const BLOCKS_PER_REQUEST: u32 = 100;

/// Request timeout duration
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Blocks behind threshold before re-syncing
pub const SYNC_BEHIND_THRESHOLD: u64 = 10;

// ============================================================================
// Protocol Messages
// ============================================================================

/// Protocol name for chain sync
pub const SYNC_PROTOCOL: &str = "/botho/sync/1.0.0";

/// Requests for chain synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncRequest {
    /// Request the peer's current chain status
    GetStatus,
    /// Request blocks starting from a height
    GetBlocks { start_height: u64, count: u32 },
}

/// Responses to sync requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncResponse {
    /// Current chain status
    Status { height: u64, tip_hash: [u8; 32] },
    /// Requested blocks
    Blocks { blocks: Vec<Block>, has_more: bool },
    /// Error response
    Error(String),
}

// ============================================================================
// Rate Limiter
// ============================================================================

/// Per-peer rate limiter to prevent request flooding
#[derive(Debug)]
pub struct SyncRateLimiter {
    /// Request timestamps per peer (sliding window)
    peer_requests: HashMap<PeerId, Vec<Instant>>,
    /// Maximum requests per window
    max_requests: u32,
    /// Window duration
    window: Duration,
}

impl Default for SyncRateLimiter {
    fn default() -> Self {
        Self::new(MAX_REQUESTS_PER_MINUTE, RATE_LIMIT_WINDOW)
    }
}

impl SyncRateLimiter {
    /// Create a new rate limiter
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            peer_requests: HashMap::new(),
            max_requests,
            window,
        }
    }

    /// Check if a request from a peer should be allowed
    pub fn check_and_record(&mut self, peer: &PeerId) -> bool {
        let now = Instant::now();
        let window_start = now - self.window;

        let requests = self.peer_requests.entry(*peer).or_insert_with(Vec::new);

        // Remove old requests outside the window
        requests.retain(|&t| t > window_start);

        // Check if under limit
        if requests.len() >= self.max_requests as usize {
            warn!(%peer, "Rate limit exceeded");
            return false;
        }

        // Record this request
        requests.push(now);
        true
    }

    /// Get current request count for a peer
    pub fn request_count(&self, peer: &PeerId) -> usize {
        self.peer_requests.get(peer).map(|v| v.len()).unwrap_or(0)
    }

    /// Clean up old entries (call periodically)
    pub fn cleanup(&mut self) {
        let now = Instant::now();
        let window_start = now - self.window;

        self.peer_requests.retain(|_, requests| {
            requests.retain(|&t| t > window_start);
            !requests.is_empty()
        });
    }
}

// ============================================================================
// Sync Codec (with bounded reads)
// ============================================================================

/// Codec for serializing/deserializing sync messages with size limits
#[derive(Debug, Clone, Default)]
pub struct SyncCodec;

impl Codec for SyncCodec {
    type Protocol = StreamProtocol;
    type Request = SyncRequest;
    type Response = SyncResponse;

    fn read_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = io::Result<Self::Request>> + Send + 'async_trait>,
    >
    where
        T: AsyncRead + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            // Bounded read to prevent memory exhaustion
            let mut buf = vec![0u8; MAX_REQUEST_SIZE as usize];
            let mut total_read = 0;

            loop {
                match io.read(&mut buf[total_read..]).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        total_read += n;
                        if total_read >= MAX_REQUEST_SIZE as usize {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Request too large",
                            ));
                        }
                    }
                    Err(e) => return Err(e),
                }
            }

            buf.truncate(total_read);
            bincode::deserialize(&buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
    }

    fn read_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = io::Result<Self::Response>> + Send + 'async_trait>,
    >
    where
        T: AsyncRead + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            // Bounded read to prevent memory exhaustion
            let mut buf = vec![0u8; MAX_RESPONSE_SIZE as usize];
            let mut total_read = 0;

            loop {
                match io.read(&mut buf[total_read..]).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        total_read += n;
                        if total_read >= MAX_RESPONSE_SIZE as usize {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Response too large",
                            ));
                        }
                    }
                    Err(e) => return Err(e),
                }
            }

            buf.truncate(total_read);
            bincode::deserialize(&buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
    }

    fn write_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        req: Self::Request,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = io::Result<()>> + Send + 'async_trait>,
    >
    where
        T: AsyncWrite + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let bytes = bincode::serialize(&req)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            io.write_all(&bytes).await?;
            io.close().await
        })
    }

    fn write_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        resp: Self::Response,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = io::Result<()>> + Send + 'async_trait>,
    >
    where
        T: AsyncWrite + Unpin + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let bytes = bincode::serialize(&resp)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            io.write_all(&bytes).await?;
            io.close().await
        })
    }
}

// ============================================================================
// Sync State Machine
// ============================================================================

/// Current sync state
#[derive(Debug, Clone, PartialEq)]
pub enum SyncState {
    /// Discovering peer chain heights
    Discovery,
    /// Downloading blocks from best peer
    Downloading { peer: PeerId, target_height: u64 },
    /// Fully synced with network
    Synced,
    /// Sync failed, waiting to retry
    Failed { reason: String, retry_at: Instant },
}

/// Peer chain status
#[derive(Debug, Clone)]
pub struct PeerStatus {
    pub height: u64,
    pub tip_hash: [u8; 32],
    pub last_updated: Instant,
}

/// Action to take based on sync state
#[derive(Debug)]
pub enum SyncAction {
    /// Send status request to peer
    RequestStatus(PeerId),
    /// Send blocks request to peer
    RequestBlocks {
        peer: PeerId,
        start_height: u64,
        count: u32,
    },
    /// Add blocks to ledger
    AddBlocks(Vec<Block>),
    /// Transition to synced state
    Synced,
    /// Wait before retrying
    Wait(Duration),
}

/// Manager for chain synchronization
#[derive(Debug)]
pub struct ChainSyncManager {
    /// Current sync state
    state: SyncState,
    /// Known peer statuses
    peer_statuses: HashMap<PeerId, PeerStatus>,
    /// Our current chain height
    local_height: u64,
    /// Height we're currently downloading from
    download_height: u64,
    /// Rate limiter
    rate_limiter: SyncRateLimiter,
    /// Retry backoff duration
    retry_backoff: Duration,
}

impl ChainSyncManager {
    /// Create a new sync manager
    pub fn new(local_height: u64) -> Self {
        Self {
            state: SyncState::Discovery,
            peer_statuses: HashMap::new(),
            local_height,
            download_height: local_height,
            rate_limiter: SyncRateLimiter::default(),
            retry_backoff: Duration::from_secs(5),
        }
    }

    /// Get current state
    pub fn state(&self) -> &SyncState {
        &self.state
    }

    /// Check if we're synced
    pub fn is_synced(&self) -> bool {
        matches!(self.state, SyncState::Synced)
    }

    /// Update local chain height
    pub fn set_local_height(&mut self, height: u64) {
        self.local_height = height;
        self.download_height = height;
    }

    /// Get rate limiter for handling incoming requests
    pub fn rate_limiter_mut(&mut self) -> &mut SyncRateLimiter {
        &mut self.rate_limiter
    }

    /// Handle status response from a peer
    pub fn on_status(&mut self, peer: PeerId, height: u64, tip_hash: [u8; 32]) {
        debug!(%peer, height, "Received peer status");

        self.peer_statuses.insert(
            peer,
            PeerStatus {
                height,
                tip_hash,
                last_updated: Instant::now(),
            },
        );

        // If in discovery and we have at least one peer ahead, start downloading
        if matches!(self.state, SyncState::Discovery) {
            if let Some((best_peer, status)) = self.best_peer() {
                if status.height > self.local_height + SYNC_BEHIND_THRESHOLD {
                    self.state = SyncState::Downloading {
                        peer: best_peer,
                        target_height: status.height,
                    };
                    self.download_height = self.local_height;
                } else {
                    // We're close enough, consider synced
                    self.state = SyncState::Synced;
                }
            }
        }
    }

    /// Handle blocks response from a peer
    pub fn on_blocks(&mut self, blocks: Vec<Block>, has_more: bool) -> Option<SyncAction> {
        if blocks.is_empty() {
            return None;
        }

        let last_height = blocks.last().map(|b| b.height()).unwrap_or(0);
        debug!(
            count = blocks.len(),
            last_height,
            has_more,
            "Received blocks"
        );

        // Update download height
        self.download_height = last_height;

        // Return action to add blocks
        Some(SyncAction::AddBlocks(blocks))
    }

    /// Called after blocks are added to ledger
    pub fn on_blocks_added(&mut self, new_height: u64) {
        self.local_height = new_height;
        self.download_height = new_height;

        // Check if we've caught up
        if let SyncState::Downloading { target_height, .. } = self.state {
            if new_height >= target_height {
                debug!(height = new_height, "Sync complete");
                self.state = SyncState::Synced;
            }
        }
    }

    /// Handle sync failure
    pub fn on_failure(&mut self, reason: String) {
        warn!(%reason, "Sync failed");
        self.state = SyncState::Failed {
            reason,
            retry_at: Instant::now() + self.retry_backoff,
        };
    }

    /// Handle peer disconnection
    pub fn on_peer_disconnected(&mut self, peer: &PeerId) {
        self.peer_statuses.remove(peer);

        // If we were downloading from this peer, go back to discovery
        if let SyncState::Downloading {
            peer: download_peer,
            ..
        } = &self.state
        {
            if download_peer == peer {
                self.state = SyncState::Discovery;
            }
        }
    }

    /// Get the best peer to sync from
    fn best_peer(&self) -> Option<(PeerId, &PeerStatus)> {
        self.peer_statuses
            .iter()
            .max_by_key(|(_, status)| status.height)
            .map(|(peer, status)| (*peer, status))
    }

    /// Drive the state machine, returns next action to take
    pub fn tick(&mut self, connected_peers: &[PeerId]) -> Option<SyncAction> {
        // Clean up rate limiter periodically
        self.rate_limiter.cleanup();

        match &self.state {
            SyncState::Discovery => {
                // Request status from all connected peers we don't have status for
                for peer in connected_peers {
                    if !self.peer_statuses.contains_key(peer) {
                        return Some(SyncAction::RequestStatus(*peer));
                    }
                }

                // If we have statuses from all peers, check if we need to sync
                if !connected_peers.is_empty()
                    && connected_peers
                        .iter()
                        .all(|p| self.peer_statuses.contains_key(p))
                {
                    if let Some((best_peer, status)) = self.best_peer() {
                        if status.height > self.local_height + SYNC_BEHIND_THRESHOLD {
                            self.state = SyncState::Downloading {
                                peer: best_peer,
                                target_height: status.height,
                            };
                            self.download_height = self.local_height;
                        } else {
                            self.state = SyncState::Synced;
                            return Some(SyncAction::Synced);
                        }
                    } else {
                        // No peers, consider synced (we're the genesis)
                        self.state = SyncState::Synced;
                        return Some(SyncAction::Synced);
                    }
                }

                None
            }

            SyncState::Downloading { peer, target_height } => {
                if self.download_height >= *target_height {
                    self.state = SyncState::Synced;
                    return Some(SyncAction::Synced);
                }

                // Request next batch
                Some(SyncAction::RequestBlocks {
                    peer: *peer,
                    start_height: self.download_height + 1,
                    count: BLOCKS_PER_REQUEST,
                })
            }

            SyncState::Synced => {
                // Check if we've fallen behind
                if let Some((best_peer, status)) = self.best_peer() {
                    if status.height > self.local_height + SYNC_BEHIND_THRESHOLD {
                        self.state = SyncState::Downloading {
                            peer: best_peer,
                            target_height: status.height,
                        };
                        self.download_height = self.local_height;
                    }
                }
                None
            }

            SyncState::Failed { retry_at, .. } => {
                if Instant::now() >= *retry_at {
                    self.state = SyncState::Discovery;
                    self.peer_statuses.clear();
                    return self.tick(connected_peers);
                }

                Some(SyncAction::Wait(
                    retry_at.saturating_duration_since(Instant::now()),
                ))
            }
        }
    }
}

/// Create request-response behaviour for sync protocol
pub fn create_sync_behaviour() -> request_response::Behaviour<SyncCodec> {
    let protocols = [(StreamProtocol::new(SYNC_PROTOCOL), ProtocolSupport::Full)];

    let config = request_response::Config::default()
        .with_request_timeout(REQUEST_TIMEOUT);

    request_response::Behaviour::new(protocols, config)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peer_id() -> PeerId {
        PeerId::random()
    }

    // Rate limiter tests

    #[test]
    fn test_rate_limiter_allows_initial_requests() {
        let mut limiter = SyncRateLimiter::new(5, Duration::from_secs(60));
        let peer = make_peer_id();

        for _ in 0..5 {
            assert!(limiter.check_and_record(&peer));
        }
    }

    #[test]
    fn test_rate_limiter_blocks_excess_requests() {
        let mut limiter = SyncRateLimiter::new(3, Duration::from_secs(60));
        let peer = make_peer_id();

        assert!(limiter.check_and_record(&peer));
        assert!(limiter.check_and_record(&peer));
        assert!(limiter.check_and_record(&peer));
        assert!(!limiter.check_and_record(&peer)); // Should be blocked
    }

    #[test]
    fn test_rate_limiter_tracks_peers_independently() {
        let mut limiter = SyncRateLimiter::new(2, Duration::from_secs(60));
        let peer1 = make_peer_id();
        let peer2 = make_peer_id();

        assert!(limiter.check_and_record(&peer1));
        assert!(limiter.check_and_record(&peer1));
        assert!(!limiter.check_and_record(&peer1)); // peer1 blocked

        assert!(limiter.check_and_record(&peer2)); // peer2 still allowed
        assert!(limiter.check_and_record(&peer2));
        assert!(!limiter.check_and_record(&peer2)); // peer2 now blocked
    }

    #[test]
    fn test_rate_limiter_request_count() {
        let mut limiter = SyncRateLimiter::new(10, Duration::from_secs(60));
        let peer = make_peer_id();

        assert_eq!(limiter.request_count(&peer), 0);
        limiter.check_and_record(&peer);
        assert_eq!(limiter.request_count(&peer), 1);
        limiter.check_and_record(&peer);
        assert_eq!(limiter.request_count(&peer), 2);
    }

    // Sync state machine tests

    #[test]
    fn test_sync_manager_starts_in_discovery() {
        let manager = ChainSyncManager::new(0);
        assert!(matches!(manager.state(), SyncState::Discovery));
    }

    #[test]
    fn test_sync_manager_transitions_to_downloading() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        // Report peer with much higher chain
        manager.on_status(peer, 100, [1u8; 32]);

        assert!(matches!(
            manager.state(),
            SyncState::Downloading { target_height: 100, .. }
        ));
    }

    #[test]
    fn test_sync_manager_stays_synced_if_close() {
        let mut manager = ChainSyncManager::new(95);
        let peer = make_peer_id();

        // Peer is only 5 blocks ahead (< SYNC_BEHIND_THRESHOLD)
        manager.on_status(peer, 100, [1u8; 32]);

        assert!(matches!(manager.state(), SyncState::Synced));
    }

    #[test]
    fn test_sync_manager_completes_on_caught_up() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]);
        assert!(matches!(manager.state(), SyncState::Downloading { .. }));

        // Simulate adding all blocks
        manager.on_blocks_added(100);

        assert!(manager.is_synced());
    }

    #[test]
    fn test_sync_manager_handles_peer_disconnect() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]);
        assert!(matches!(manager.state(), SyncState::Downloading { .. }));

        manager.on_peer_disconnected(&peer);

        // Should go back to discovery
        assert!(matches!(manager.state(), SyncState::Discovery));
    }

    #[test]
    fn test_sync_manager_tick_requests_status() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        let action = manager.tick(&[peer]);

        assert!(matches!(action, Some(SyncAction::RequestStatus(_))));
    }

    #[test]
    fn test_sync_manager_tick_requests_blocks() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]);

        let action = manager.tick(&[peer]);

        assert!(matches!(
            action,
            Some(SyncAction::RequestBlocks {
                start_height: 1,
                count: 100,
                ..
            })
        ));
    }

    // Message serialization tests

    #[test]
    fn test_sync_request_serialization() {
        let request = SyncRequest::GetBlocks {
            start_height: 100,
            count: 50,
        };
        let bytes = bincode::serialize(&request).unwrap();
        let decoded: SyncRequest = bincode::deserialize(&bytes).unwrap();

        match decoded {
            SyncRequest::GetBlocks { start_height, count } => {
                assert_eq!(start_height, 100);
                assert_eq!(count, 50);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_sync_response_serialization() {
        let response = SyncResponse::Status {
            height: 1000,
            tip_hash: [42u8; 32],
        };
        let bytes = bincode::serialize(&response).unwrap();
        let decoded: SyncResponse = bincode::deserialize(&bytes).unwrap();

        match decoded {
            SyncResponse::Status { height, tip_hash } => {
                assert_eq!(height, 1000);
                assert_eq!(tip_hash, [42u8; 32]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_constants_are_reasonable() {
        assert!(MAX_REQUEST_SIZE > 0);
        assert!(MAX_RESPONSE_SIZE > MAX_REQUEST_SIZE);
        assert!(MAX_REQUESTS_PER_MINUTE > 0);
        assert!(BLOCKS_PER_REQUEST > 0);
        assert!(SYNC_BEHIND_THRESHOLD > 0);
    }
}
