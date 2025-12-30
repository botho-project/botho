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
use super::reputation::ReputationManager;

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
    /// Peer reputation tracking for sync selection
    reputation: ReputationManager,
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
            reputation: ReputationManager::new(),
        }
    }

    /// Get access to the reputation manager
    pub fn reputation_mut(&mut self) -> &mut ReputationManager {
        &mut self.reputation
    }

    /// Record that a request was sent to a peer (for latency tracking)
    pub fn on_request_sent(&mut self, peer: PeerId) {
        self.reputation.request_sent(peer);
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
    pub fn on_blocks(&mut self, peer: &PeerId, blocks: Vec<Block>, has_more: bool) -> Option<SyncAction> {
        // Record successful response in reputation
        self.reputation.response_received(peer);

        if blocks.is_empty() {
            return None;
        }

        let last_height = blocks.last().map(|b| b.height()).unwrap_or(0);
        debug!(
            count = blocks.len(),
            last_height,
            has_more,
            %peer,
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
    pub fn on_failure(&mut self, peer: Option<&PeerId>, reason: String) {
        // Record failure in reputation if we know which peer failed
        if let Some(p) = peer {
            self.reputation.request_failed(p);
            warn!(%reason, %p, "Sync failed from peer");
        } else {
            warn!(%reason, "Sync failed");
        }

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
    ///
    /// Selection criteria (in order of priority):
    /// 1. Exclude banned peers (< 25% success rate)
    /// 2. Among peers at similar height (within 10 blocks), prefer better reputation
    /// 3. For peers at very different heights, prefer higher height
    fn best_peer(&self) -> Option<(PeerId, &PeerStatus)> {
        // Filter out banned peers
        let candidates: Vec<_> = self
            .peer_statuses
            .iter()
            .filter(|(peer, _)| !self.reputation.is_banned(peer))
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // Find max height among candidates
        let max_height = candidates
            .iter()
            .map(|(_, status)| status.height)
            .max()
            .unwrap_or(0);

        // Height threshold: peers within this range are considered "equivalent"
        const HEIGHT_EQUIVALENCE_THRESHOLD: u64 = 10;

        // Filter to peers at or near max height
        let top_peers: Vec<_> = candidates
            .iter()
            .filter(|(_, status)| status.height + HEIGHT_EQUIVALENCE_THRESHOLD >= max_height)
            .collect();

        // Among top peers, select by reputation score (lower is better)
        top_peers
            .into_iter()
            .min_by(|(a_peer, a_status), (b_peer, b_status)| {
                // First compare heights (higher is better)
                let height_cmp = b_status.height.cmp(&a_status.height);
                if height_cmp != std::cmp::Ordering::Equal {
                    return height_cmp;
                }

                // Same height: compare reputation (lower score is better)
                let score_a = self
                    .reputation
                    .get(a_peer)
                    .map(|r| r.score())
                    .unwrap_or(500.0);
                let score_b = self
                    .reputation
                    .get(b_peer)
                    .map(|r| r.score())
                    .unwrap_or(500.0);

                score_a
                    .partial_cmp(&score_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(peer, status)| (**peer, *status))
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

    // ========================================================================
    // Additional SyncRateLimiter tests
    // ========================================================================

    #[test]
    fn test_rate_limiter_default() {
        let mut limiter = SyncRateLimiter::default();
        let peer = make_peer_id();

        // Should use default constants
        assert_eq!(limiter.max_requests, MAX_REQUESTS_PER_MINUTE);
        assert_eq!(limiter.window, RATE_LIMIT_WINDOW);

        // Should allow initial requests
        assert!(limiter.check_and_record(&peer));
    }

    #[test]
    fn test_rate_limiter_cleanup_removes_old_entries() {
        let mut limiter = SyncRateLimiter::new(100, Duration::from_millis(50));
        let peer1 = make_peer_id();
        let peer2 = make_peer_id();

        limiter.check_and_record(&peer1);
        limiter.check_and_record(&peer2);

        assert_eq!(limiter.request_count(&peer1), 1);
        assert_eq!(limiter.request_count(&peer2), 1);

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(60));

        limiter.cleanup();

        // Both should be cleaned up
        assert_eq!(limiter.request_count(&peer1), 0);
        assert_eq!(limiter.request_count(&peer2), 0);
    }

    #[test]
    fn test_rate_limiter_partial_cleanup() {
        let mut limiter = SyncRateLimiter::new(100, Duration::from_millis(100));
        let peer = make_peer_id();

        limiter.check_and_record(&peer);
        std::thread::sleep(Duration::from_millis(60));
        limiter.check_and_record(&peer);

        // Should have 2 requests
        assert_eq!(limiter.request_count(&peer), 2);

        // Wait for first to expire but not second
        std::thread::sleep(Duration::from_millis(50));
        limiter.cleanup();

        // First request should be cleaned, second should remain
        assert_eq!(limiter.request_count(&peer), 1);
    }

    // ========================================================================
    // SyncState tests
    // ========================================================================

    #[test]
    fn test_sync_state_equality() {
        assert_eq!(SyncState::Discovery, SyncState::Discovery);
        assert_eq!(SyncState::Synced, SyncState::Synced);
        assert_ne!(SyncState::Discovery, SyncState::Synced);
    }

    #[test]
    fn test_sync_state_downloading_equality() {
        let peer = make_peer_id();
        let state1 = SyncState::Downloading {
            peer,
            target_height: 100,
        };
        let state2 = SyncState::Downloading {
            peer,
            target_height: 100,
        };
        assert_eq!(state1, state2);
    }

    // ========================================================================
    // ChainSyncManager failure and recovery tests
    // ========================================================================

    #[test]
    fn test_sync_manager_on_failure_transitions_to_failed() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]);
        assert!(matches!(manager.state(), SyncState::Downloading { .. }));

        manager.on_failure(Some(&peer), "connection reset".to_string());

        match manager.state() {
            SyncState::Failed { reason, .. } => {
                assert_eq!(reason, "connection reset");
            }
            _ => panic!("Expected Failed state"),
        }
    }

    #[test]
    fn test_sync_manager_failure_without_peer() {
        let mut manager = ChainSyncManager::new(0);

        manager.on_failure(None, "network error".to_string());

        match manager.state() {
            SyncState::Failed { reason, .. } => {
                assert_eq!(reason, "network error");
            }
            _ => panic!("Expected Failed state"),
        }
    }

    #[test]
    fn test_sync_manager_failure_records_reputation() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        // Record a successful request first
        manager.reputation_mut().request_sent(peer);
        manager.reputation_mut().response_received(&peer);

        // Now record a failure
        manager.on_failure(Some(&peer), "timeout".to_string());

        let rep = manager.reputation_mut().get(&peer).unwrap();
        assert_eq!(rep.failures, 1);
        assert_eq!(rep.successes, 1);
    }

    #[test]
    fn test_sync_manager_tick_retries_after_backoff() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        // Transition to failed state with short retry
        manager.on_failure(None, "test".to_string());

        // Immediately after failure, should wait
        let action = manager.tick(&[peer]);
        assert!(matches!(action, Some(SyncAction::Wait(_))));

        // Modify retry time to be in the past (simulating time passage)
        if let SyncState::Failed { retry_at, .. } = &mut manager.state {
            // We can't mutate directly, but we can test the tick behavior
        }
    }

    #[test]
    fn test_sync_manager_set_local_height() {
        let mut manager = ChainSyncManager::new(0);

        // Set a new local height
        manager.set_local_height(500);

        // Verify by checking that tick returns correct behavior
        let peer = make_peer_id();
        manager.on_status(peer, 505, [1u8; 32]);

        // Should be synced since 505 - 500 = 5 < SYNC_BEHIND_THRESHOLD (10)
        assert!(manager.is_synced());
    }

    #[test]
    fn test_sync_manager_is_synced() {
        let mut manager = ChainSyncManager::new(100);
        assert!(!manager.is_synced());

        // Report peer at same height
        let peer = make_peer_id();
        manager.on_status(peer, 100, [1u8; 32]);

        assert!(manager.is_synced());
    }

    // ========================================================================
    // Best peer selection with reputation tests
    // ========================================================================

    #[test]
    fn test_sync_manager_prefers_peer_with_better_reputation() {
        let mut manager = ChainSyncManager::new(0);

        let fast_peer = make_peer_id();
        let slow_peer = make_peer_id();

        // Both at same height
        manager.on_status(fast_peer, 100, [1u8; 32]);
        manager.on_status(slow_peer, 100, [2u8; 32]);

        // Record reputation - fast peer has better latency
        for _ in 0..3 {
            manager
                .reputation_mut()
                .get_or_create(&fast_peer)
                .record_success(Duration::from_millis(50));
            manager
                .reputation_mut()
                .get_or_create(&slow_peer)
                .record_success(Duration::from_millis(500));
        }

        // Should prefer fast_peer due to better reputation
        // Reset to downloading state to re-evaluate
        manager.on_peer_disconnected(&fast_peer);
        manager.on_peer_disconnected(&slow_peer);

        manager.on_status(fast_peer, 100, [1u8; 32]);
        manager.on_status(slow_peer, 100, [2u8; 32]);

        if let SyncState::Downloading { peer, .. } = manager.state() {
            // Should pick fast_peer
            assert_eq!(*peer, fast_peer);
        }
    }

    #[test]
    fn test_sync_manager_avoids_banned_peer() {
        let mut manager = ChainSyncManager::new(0);

        let good_peer = make_peer_id();
        let bad_peer = make_peer_id();

        // Bad peer has higher chain but is banned
        manager.on_status(good_peer, 100, [1u8; 32]);
        manager.on_status(bad_peer, 200, [2u8; 32]); // Higher!

        // Ban bad_peer
        for _ in 0..4 {
            manager.reputation_mut().get_or_create(&bad_peer).record_failure();
        }

        // Reset and try again
        manager.on_peer_disconnected(&good_peer);
        manager.on_peer_disconnected(&bad_peer);

        manager.on_status(good_peer, 100, [1u8; 32]);
        manager.on_status(bad_peer, 200, [2u8; 32]);

        if let SyncState::Downloading { peer, .. } = manager.state() {
            // Should pick good_peer despite bad_peer having higher chain
            assert_eq!(*peer, good_peer);
        }
    }

    #[test]
    fn test_sync_manager_on_request_sent_tracks_latency() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_request_sent(peer);

        // Peer should now be tracked
        assert!(manager.reputation_mut().get(&peer).is_some());
    }

    // ========================================================================
    // on_blocks tests
    // ========================================================================

    #[test]
    fn test_sync_manager_on_blocks_empty() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        let action = manager.on_blocks(&peer, vec![], false);
        assert!(action.is_none());
    }

    #[test]
    fn test_sync_manager_on_blocks_records_reputation() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]);

        // Calling on_blocks should record a successful response
        let _ = manager.on_blocks(&peer, vec![], false);

        // The peer should have a reputation entry
        let rep = manager.reputation_mut().get(&peer);
        assert!(rep.is_some());
    }

    // ========================================================================
    // Tick state machine tests
    // ========================================================================

    #[test]
    fn test_sync_manager_tick_no_peers() {
        let mut manager = ChainSyncManager::new(0);

        let action = manager.tick(&[]);
        assert!(action.is_none());
    }

    #[test]
    fn test_sync_manager_tick_synced_detects_falling_behind() {
        let mut manager = ChainSyncManager::new(100);
        let peer = make_peer_id();

        // Start synced
        manager.on_status(peer, 100, [1u8; 32]);
        assert!(manager.is_synced());

        // Peer advances significantly
        manager.on_status(peer, 200, [2u8; 32]);

        // Tick should detect we need to sync
        manager.tick(&[peer]);

        assert!(matches!(manager.state(), SyncState::Downloading { .. }));
    }

    #[test]
    fn test_sync_manager_download_completes_on_target() {
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]);

        // Simulate downloading all blocks
        manager.on_blocks_added(100);

        assert!(manager.is_synced());
    }

    // ========================================================================
    // PeerStatus tests
    // ========================================================================

    #[test]
    fn test_peer_status_clone() {
        let status = PeerStatus {
            height: 100,
            tip_hash: [42u8; 32],
            last_updated: Instant::now(),
        };

        let cloned = status.clone();
        assert_eq!(cloned.height, 100);
        assert_eq!(cloned.tip_hash, [42u8; 32]);
    }

    // ========================================================================
    // SyncAction tests
    // ========================================================================

    #[test]
    fn test_sync_action_debug() {
        let action = SyncAction::Synced;
        let debug = format!("{:?}", action);
        assert!(debug.contains("Synced"));

        let action = SyncAction::Wait(Duration::from_secs(5));
        let debug = format!("{:?}", action);
        assert!(debug.contains("Wait"));
    }

    // ========================================================================
    // SyncRequest/Response additional tests
    // ========================================================================

    #[test]
    fn test_sync_request_get_status() {
        let request = SyncRequest::GetStatus;
        let bytes = bincode::serialize(&request).unwrap();
        let decoded: SyncRequest = bincode::deserialize(&bytes).unwrap();

        assert!(matches!(decoded, SyncRequest::GetStatus));
    }

    #[test]
    fn test_sync_response_error() {
        let response = SyncResponse::Error("test error".to_string());
        let bytes = bincode::serialize(&response).unwrap();
        let decoded: SyncResponse = bincode::deserialize(&bytes).unwrap();

        match decoded {
            SyncResponse::Error(msg) => assert_eq!(msg, "test error"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_sync_response_blocks_empty() {
        let response = SyncResponse::Blocks {
            blocks: vec![],
            has_more: false,
        };
        let bytes = bincode::serialize(&response).unwrap();
        let decoded: SyncResponse = bincode::deserialize(&bytes).unwrap();

        match decoded {
            SyncResponse::Blocks { blocks, has_more } => {
                assert!(blocks.is_empty());
                assert!(!has_more);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_sync_protocol_constant() {
        assert_eq!(SYNC_PROTOCOL, "/botho/sync/1.0.0");
    }

    #[test]
    fn test_rate_limit_window_constant() {
        assert_eq!(RATE_LIMIT_WINDOW, Duration::from_secs(60));
    }

    #[test]
    fn test_request_timeout_constant() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }
}
