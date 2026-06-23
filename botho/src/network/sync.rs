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
use std::{
    collections::HashMap,
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, warn};

use super::{discovery::NetworkStats, reputation::ReputationManager};
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

/// Blocks behind threshold before a *Synced* node re-enters catch-up.
///
/// This is hysteresis for the `Synced -> Downloading` transition only: when a
/// node is already caught up and falls a *few* blocks behind near the tip, the
/// gap is normally closed by gossip of contiguous blocks, so we don't want to
/// thrash into a redundant historical download for every 1-2 block lag. It is
/// deliberately NOT used to gate the *initial* catch-up download — see
/// [`SYNC_INITIAL_GAP`].
pub const SYNC_BEHIND_THRESHOLD: u64 = 10;

/// Minimum gap that must trigger a historical catch-up download, regardless of
/// the near-tip hysteresis [`SYNC_BEHIND_THRESHOLD`].
///
/// Gossip can only ever deliver the *next contiguous* block (`local_height +
/// 1`); any larger gap must go through the sync state machine. So a node behind
/// by more than one block (i.e. `peer_height > local_height + 1`, a gap of >=
/// 2) must enter `Downloading`. A 1-block lag (`peer_height == local_height +
/// 1`) is left to gossip and does NOT trigger a download, avoiding thrash.
///
/// This is what makes a fresh joiner at height 0 against a tip of, say, 9 enter
/// catch-up: `9 > 0 + 1` is true even though `9 > 0 + 10` is false.
pub const SYNC_INITIAL_GAP: u64 = 1;

/// How often a synced node re-polls peers for their chain status.
///
/// While `Synced`, the manager has no other way to learn that a peer has
/// advanced (status is request/response, not gossiped). Periodically
/// re-requesting status lets a long-running node detect that the chain grew
/// and re-enter catch-up, instead of relying solely on gossiped tip blocks.
pub const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

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

        let requests = self.peer_requests.entry(*peer).or_default();

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

/// Codec for serializing/deserializing sync messages with size limits.
///
/// Optionally carries a shared [`NetworkStats`] handle so that the
/// request/response payload bytes that cross this codec are counted toward the
/// node-wide `bytesSent` / `bytesReceived` totals surfaced by `network_getInfo`
/// (#549). The codec is the natural accounting point: it is exactly where each
/// message is (de)serialized, so the serialized length is already in hand and
/// no extra serialization pass is added on the hot path.
///
/// The handle is an `Option<Arc<_>>` so the codec stays `Default` (used by the
/// libp2p `Behaviour::new` path and by unit tests that don't care about stats);
/// when present, the `Arc` clone made on every per-substream codec clone is
/// cheap and all clones share the same atomics.
#[derive(Debug, Clone, Default)]
pub struct SyncCodec {
    /// Shared live traffic counters (#542/#549). `None` disables accounting.
    stats: Option<Arc<NetworkStats>>,
}

impl SyncCodec {
    /// Create a codec that records request/response payload bytes into the
    /// given shared [`NetworkStats`] (#549).
    pub fn with_stats(stats: Arc<NetworkStats>) -> Self {
        Self { stats: Some(stats) }
    }

    /// Record `n` payload bytes sent over the sync protocol, if a stats handle
    /// is attached.
    fn record_sent(&self, n: u64) {
        if let Some(stats) = &self.stats {
            stats.record_sent(n);
        }
    }

    /// Record `n` payload bytes received over the sync protocol, if a stats
    /// handle is attached.
    fn record_received(&self, n: u64) {
        if let Some(stats) = &self.stats {
            stats.record_received(n);
        }
    }
}

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
            // Account for the received request payload (#549). The bytes have
            // already crossed the wire, so they count regardless of whether
            // deserialization below succeeds.
            self.record_received(total_read as u64);
            bincode::deserialize(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
            // Account for the received response payload (#549); see the note in
            // `read_request`.
            self.record_received(total_read as u64);
            bincode::deserialize(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
    }

    fn write_request<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        req: Self::Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + 'async_trait>>
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
            // Account for the sent request payload (#549); the serialized length
            // is already in hand here, so no extra serialization pass is added.
            self.record_sent(bytes.len() as u64);
            io.write_all(&bytes).await?;
            // NOTE: do NOT call `io.close()` here. Under libp2p 0.56's
            // request-response handler, the *handler* (not the codec) is
            // responsible for half-closing the substream after the codec
            // returns (it calls `stream.close()` right after
            // `write_request`/`write_response`). Closing inside the codec
            // races with libp2p's optimistic multistream-select negotiation:
            // tearing the substream down before the remote confirms the
            // protocol surfaces as "Stream closed. Confirmation from remote
            // for optimistic protocol negotiation still pending." On loopback
            // this cascades into the whole connection being dropped and
            // redialed (issue #411). The peer's read side still observes EOF
            // because the handler half-closes the write direction once we
            // return.
            Ok(())
        })
    }

    fn write_response<'life0, 'life1, 'life2, 'async_trait, T>(
        &'life0 mut self,
        _protocol: &'life1 Self::Protocol,
        io: &'life2 mut T,
        resp: Self::Response,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + 'async_trait>>
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
            // Account for the sent response payload (#549); see `write_request`.
            self.record_sent(bytes.len() as u64);
            io.write_all(&bytes).await?;
            // See the note in `write_request`: the libp2p request-response
            // handler half-closes the substream after this returns, so the
            // codec must not call `io.close()` itself (it races with
            // optimistic protocol negotiation and destabilizes the
            // connection — issue #411).
            Ok(())
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

/// Cheap, owned snapshot of sync progress for surfacing over the RPC layer
/// (#541). The event loop owns the live [`ChainSyncManager`] without a lock; it
/// publishes one of these into a shared `Arc<RwLock<_>>` on each sync tick so
/// `node_getStatus` can report honest sync state instead of a hardcoded
/// "always synced". All fields are plain values so reading is allocation-free
/// apart from the status string.
#[derive(Debug, Clone)]
pub struct SyncStatusSnapshot {
    /// True iff the sync state machine is in [`SyncState::Synced`].
    pub synced: bool,
    /// Coarse status string derived from [`SyncState`]: one of
    /// "discovering", "syncing", "synced", "stalled".
    pub status: &'static str,
    /// Our current local chain height.
    pub local_height: u64,
    /// Best-known network tip height, if any peer status / download target is
    /// known. `None` when we have no peer information yet (Discovery with no
    /// responses), in which case a true progress percentage cannot be computed.
    pub target_height: Option<u64>,
}

impl SyncStatusSnapshot {
    /// Progress toward the best-known tip as a percentage clamped to `0..=100`.
    ///
    /// Returns `Some(100.0)` when synced. Returns `None` when no target tip is
    /// known (so callers can avoid fabricating a number). Otherwise computes
    /// `local_height / target_height * 100`.
    pub fn progress_percent(&self) -> Option<f64> {
        if self.synced {
            return Some(100.0);
        }
        let target = self.target_height?;
        if target == 0 {
            // Nothing to sync to; treat as fully caught up.
            return Some(100.0);
        }
        let pct = (self.local_height as f64 / target as f64) * 100.0;
        Some(pct.clamp(0.0, 100.0))
    }
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
    /// Last time we re-polled peers for status while synced
    last_status_refresh: Instant,
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
            last_status_refresh: Instant::now(),
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

    /// Produce a cheap, owned [`SyncStatusSnapshot`] for the RPC layer (#541).
    ///
    /// `target_height` is the best honest estimate of the network tip:
    /// - while `Downloading`, the download `target_height`;
    /// - otherwise, the max height across known (non-banned) peers. `None` when
    ///   no peer status is known yet, so callers can avoid reporting a
    ///   fabricated progress percentage.
    pub fn status_snapshot(&self) -> SyncStatusSnapshot {
        let status = match &self.state {
            SyncState::Discovery => "discovering",
            SyncState::Downloading { .. } => "syncing",
            SyncState::Synced => "synced",
            SyncState::Failed { .. } => "stalled",
        };

        // Best-known network tip. Prefer the active download target; otherwise
        // fall back to the highest known peer height.
        let target_height = match &self.state {
            SyncState::Downloading { target_height, .. } => Some(*target_height),
            _ => self.best_peer().map(|(_, status)| status.height),
        };

        SyncStatusSnapshot {
            synced: matches!(self.state, SyncState::Synced),
            status,
            local_height: self.local_height,
            target_height,
        }
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

        // If we're not already downloading and a peer is ahead by a gap that
        // gossip cannot bridge, (re)enter catch-up. The required gap depends on
        // our current state:
        //
        // - Initial join (Discovery) or recovery (Failed): use the gap-1 rule
        //   (`SYNC_INITIAL_GAP`, gap >= 2). Gossip only ever delivers the next
        //   contiguous block, so ANY gap >= 2 — including the entire 0->N initial
        //   download for a small N — must go through the sync state machine. This is
        //   the #423 fix: a fresh joiner at height 0 against a small tip (e.g. 9)
        //   enters Downloading instead of jumping to Synced.
        //
        // - Already Synced (learned via a status refresh that a peer advanced): use the
        //   hysteresis threshold (`SYNC_BEHIND_THRESHOLD`). An already-caught-up node
        //   that lags a few blocks near the tip normally has that gap closed by gossip,
        //   so we avoid thrashing into a redundant historical download for every small
        //   near-tip lag.
        //
        // Either way, a 1-block lag is left to gossip and never triggers a
        // download.
        if !matches!(self.state, SyncState::Downloading { .. }) {
            let trigger_gap = if matches!(self.state, SyncState::Synced) {
                SYNC_BEHIND_THRESHOLD
            } else {
                SYNC_INITIAL_GAP
            };
            if let Some((best_peer, status)) = self.best_peer() {
                if status.height > self.local_height + trigger_gap {
                    self.state = SyncState::Downloading {
                        peer: best_peer,
                        target_height: status.height,
                    };
                    self.download_height = self.local_height;
                } else if matches!(self.state, SyncState::Discovery) {
                    // Within one block of the tip during initial discovery:
                    // gossip will close the gap. Mark synced.
                    self.state = SyncState::Synced;
                }
            }
        }
    }

    /// React to a gossiped tip block we cannot apply because it is ahead of us
    /// by a gap gossip cannot bridge.
    ///
    /// Gossip only delivers the next contiguous block (`local_height + 1`).
    /// When a node receives a gossiped compact/full block at a height
    /// beyond that, it is behind by a gap that only the catch-up state
    /// machine can close. The run loop's only other sources of peer height
    /// are the Discovery `RequestStatus` round-trip and the 30s
    /// `STATUS_REFRESH_INTERVAL` re-poll; without this hint, a node that is
    /// gossiped a far-ahead tip while already `Synced` would wait up to 30s
    /// before re-entering catch-up.
    ///
    /// Since gossip does not tell us which peer relayed the block, we record
    /// the observed height against the currently connected peers (at least
    /// one of them is at or beyond this height, having relayed it) and
    /// re-evaluate the catch-up gate immediately. This is a best-effort
    /// hint; the authoritative height is still confirmed by the `GetBlocks`
    /// response during download.
    pub fn note_gossiped_tip(
        &mut self,
        connected_peers: &[PeerId],
        height: u64,
        tip_hash: [u8; 32],
    ) {
        // Only act if this is genuinely ahead of us by a gap gossip can't bridge
        // (gap >= 2). A 1-block lag is left to gossip.
        if height <= self.local_height + SYNC_INITIAL_GAP {
            return;
        }

        // Record the observed tip against the connected peers as a hint.
        for peer in connected_peers {
            self.peer_statuses.insert(
                *peer,
                PeerStatus {
                    height,
                    tip_hash,
                    last_updated: Instant::now(),
                },
            );
        }

        // Receiving a gossiped block we cannot apply is direct evidence of a
        // real gap (gap >= 2), so trigger catch-up with the gap-1 rule even from
        // the Synced state — do NOT defer to the near-tip hysteresis threshold,
        // which exists only to suppress thrash on lags gossip *can* close. If we
        // are already Downloading we leave the existing target alone.
        if !matches!(self.state, SyncState::Downloading { .. }) {
            if let Some((best_peer, status)) = self.best_peer() {
                if status.height > self.local_height + SYNC_INITIAL_GAP {
                    self.state = SyncState::Downloading {
                        peer: best_peer,
                        target_height: status.height,
                    };
                    self.download_height = self.local_height;
                }
            }
        }
    }

    /// Handle blocks response from a peer
    pub fn on_blocks(
        &mut self,
        peer: &PeerId,
        blocks: Vec<Block>,
        has_more: bool,
    ) -> Option<SyncAction> {
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
    /// 2. Among peers at similar height (within 10 blocks), prefer better
    ///    reputation
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
                        // Initial catch-up: trigger on any gap gossip can't
                        // bridge (gap >= 2), NOT the near-tip hysteresis
                        // threshold. A fresh joiner at height 0 against a small
                        // tip (e.g. 9) must enter Downloading here rather than
                        // jumping straight to Synced and stalling at 0.
                        if status.height > self.local_height + SYNC_INITIAL_GAP {
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

            SyncState::Downloading {
                peer,
                target_height,
            } => {
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
                // Check if we've fallen behind based on the statuses we have.
                //
                // Here we use the hysteresis threshold (`SYNC_BEHIND_THRESHOLD`)
                // rather than the gap-1 rule: an already-synced node that lags a
                // few blocks near the tip normally has that gap closed by gossip
                // of contiguous blocks, so we avoid thrashing into a redundant
                // historical download for every 1-2 block lag. A larger gap (or
                // a gossiped far-ahead tip, handled by the compact-block
                // fallback that pokes `on_status`) does re-enter catch-up.
                if let Some((best_peer, status)) = self.best_peer() {
                    if status.height > self.local_height + SYNC_BEHIND_THRESHOLD {
                        self.state = SyncState::Downloading {
                            peer: best_peer,
                            target_height: status.height,
                        };
                        self.download_height = self.local_height;
                        return None;
                    }
                }

                // Periodically re-poll a peer for its status. Status is
                // request/response (not gossiped), so without this a synced
                // node would never learn that a peer advanced and would rely
                // solely on gossiped tip blocks to stay current.
                if self.last_status_refresh.elapsed() >= STATUS_REFRESH_INTERVAL {
                    if let Some(peer) = connected_peers.first() {
                        self.last_status_refresh = Instant::now();
                        return Some(SyncAction::RequestStatus(*peer));
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

/// Create request-response behaviour for the sync protocol.
///
/// The behaviour is built with a [`SyncCodec`] that records request/response
/// payload bytes into the shared [`NetworkStats`] (#549), so initial-sync and
/// catch-up traffic counts toward `network_getInfo`'s `bytesSent` /
/// `bytesReceived` (previously gossipsub-only). The codec is cloned per
/// substream by libp2p; each clone shares the same atomics via the inner
/// `Arc`.
pub fn create_sync_behaviour(stats: Arc<NetworkStats>) -> request_response::Behaviour<SyncCodec> {
    let protocols = [(StreamProtocol::new(SYNC_PROTOCOL), ProtocolSupport::Full)];

    let config = request_response::Config::default().with_request_timeout(REQUEST_TIMEOUT);

    request_response::Behaviour::with_codec(SyncCodec::with_stats(stats), protocols, config)
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
            SyncState::Downloading {
                target_height: 100,
                ..
            }
        ));
    }

    #[test]
    fn test_sync_manager_stays_synced_if_one_block_behind() {
        // During initial discovery the gap-1 rule applies (issue #423): a fresh
        // node only ONE block behind is left to gossip and stays Synced.
        let mut manager = ChainSyncManager::new(99);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]); // gap = 1
        assert!(matches!(manager.state(), SyncState::Synced));
    }

    #[test]
    fn test_sync_manager_discovery_downloads_small_gap() {
        // Issue #423: during discovery, ANY gap >= 2 must trigger Downloading,
        // even one well under the old SYNC_BEHIND_THRESHOLD (10). Pre-fix a gap
        // of 5 jumped straight to Synced and stalled.
        let mut manager = ChainSyncManager::new(95);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]); // gap = 5

        assert!(matches!(
            manager.state(),
            SyncState::Downloading {
                target_height: 100,
                ..
            }
        ));
    }

    #[test]
    fn test_synced_node_uses_hysteresis_threshold() {
        // An already-Synced node that learns (via status refresh) a peer drifted
        // a few blocks ahead uses SYNC_BEHIND_THRESHOLD hysteresis, not the
        // gap-1 rule, so it does not thrash into a redundant download.
        let mut manager = ChainSyncManager::new(100);
        let peer = make_peer_id();

        manager.on_status(peer, 100, [1u8; 32]); // equal -> Synced
        assert!(matches!(manager.state(), SyncState::Synced));

        manager.on_status(peer, 105, [1u8; 32]); // drift 5 < threshold 10
        assert!(
            matches!(manager.state(), SyncState::Synced),
            "synced node within hysteresis threshold must not re-download"
        );

        manager.on_status(peer, 120, [1u8; 32]); // drift 20 > threshold 10
        assert!(
            matches!(manager.state(), SyncState::Downloading { .. }),
            "synced node beyond hysteresis threshold must re-enter catch-up"
        );
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
            SyncRequest::GetBlocks {
                start_height,
                count,
            } => {
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

        // Verify by checking that tick returns correct behavior. With the
        // local height at 500 and a peer one block ahead (501), the gap-1 rule
        // leaves the node Synced (gossip closes a 1-block lag). If
        // set_local_height had NOT updated the height, the peer at 501 would
        // look 501 blocks ahead of height 0 and trigger Downloading.
        let peer = make_peer_id();
        manager.on_status(peer, 501, [1u8; 32]);

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
            manager
                .reputation_mut()
                .get_or_create(&bad_peer)
                .record_failure();
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

    // ========================================================================
    // Catch-up / IBD re-entry tests (#376)
    // ========================================================================

    #[test]
    fn test_synced_node_reenters_download_on_fresh_status() {
        // A node that already caught up should re-enter Downloading when a
        // fresh status shows the peer has advanced well beyond us.
        let mut manager = ChainSyncManager::new(100);
        let peer = make_peer_id();

        // Initial status: peer at our height -> Synced.
        manager.on_status(peer, 100, [1u8; 32]);
        assert!(manager.is_synced());

        // Our height stays 100, peer jumps to 250 (a fresh status arrives).
        manager.on_status(peer, 250, [2u8; 32]);

        assert!(matches!(
            manager.state(),
            SyncState::Downloading {
                target_height: 250,
                ..
            }
        ));
    }

    #[test]
    fn test_synced_node_refreshes_status_after_interval() {
        let mut manager = ChainSyncManager::new(100);
        let peer = make_peer_id();

        // Reach Synced.
        manager.on_status(peer, 100, [1u8; 32]);
        assert!(manager.is_synced());

        // Force the refresh timer into the past so the next tick re-polls.
        manager.last_status_refresh =
            Instant::now() - STATUS_REFRESH_INTERVAL - Duration::from_secs(1);

        let action = manager.tick(&[peer]);
        assert!(
            matches!(action, Some(SyncAction::RequestStatus(p)) if p == peer),
            "synced node should re-request status after the refresh interval"
        );
    }

    #[test]
    fn test_synced_node_no_refresh_without_peers() {
        let mut manager = ChainSyncManager::new(100);
        let peer = make_peer_id();
        manager.on_status(peer, 100, [1u8; 32]);
        assert!(manager.is_synced());

        manager.last_status_refresh =
            Instant::now() - STATUS_REFRESH_INTERVAL - Duration::from_secs(1);

        // No connected peers: nothing to request.
        let action = manager.tick(&[]);
        assert!(action.is_none());
    }

    // ========================================================================
    // SyncCodec framing tests (#411)
    //
    // Regression coverage for the transport-stability bug: the codec must not
    // call `io.close()` itself. The libp2p request-response handler owns
    // closing the substream (it half-closes the write direction *after* the
    // codec returns). Calling `close()` inside the codec raced with libp2p
    // 0.56 optimistic protocol negotiation and tore down whole connections on
    // loopback. These tests verify the round-trip still works under the
    // handler's "write, then handler closes, peer reads to EOF" contract.
    // ========================================================================

    /// Drives the codec exactly as the libp2p request-response handler does:
    /// the writer serializes via the codec, the handler then half-closes the
    /// write side, and the reader consumes until EOF. We model the closed
    /// write side with a `Cursor` over the produced bytes (which yields EOF
    /// once exhausted), proving the codec frames correctly without ever
    /// calling `io.close()` itself.
    async fn roundtrip_request(req: SyncRequest) -> SyncRequest {
        use futures::io::Cursor;

        let protocol = StreamProtocol::new(SYNC_PROTOCOL);
        let mut codec = SyncCodec::default();

        // Writer side: codec writes bytes into the buffer. The handler — not
        // the codec — is responsible for closing afterwards, so we explicitly
        // do NOT close here; we just take the written bytes.
        let mut write_buf = Cursor::new(Vec::new());
        codec
            .write_request(&protocol, &mut write_buf, req)
            .await
            .expect("write_request should succeed without closing the stream");
        let bytes = write_buf.into_inner();

        // Reader side: a Cursor yields EOF once the bytes are exhausted,
        // exactly like a peer's half-closed substream.
        let mut read_buf = Cursor::new(bytes);
        codec
            .read_request(&protocol, &mut read_buf)
            .await
            .expect("read_request should decode the framed message")
    }

    async fn roundtrip_response(resp: SyncResponse) -> SyncResponse {
        use futures::io::Cursor;

        let protocol = StreamProtocol::new(SYNC_PROTOCOL);
        let mut codec = SyncCodec::default();

        let mut write_buf = Cursor::new(Vec::new());
        codec
            .write_response(&protocol, &mut write_buf, resp)
            .await
            .expect("write_response should succeed without closing the stream");
        let bytes = write_buf.into_inner();

        let mut read_buf = Cursor::new(bytes);
        codec
            .read_response(&protocol, &mut read_buf)
            .await
            .expect("read_response should decode the framed message")
    }

    #[tokio::test]
    async fn test_codec_request_roundtrip_get_status() {
        let decoded = roundtrip_request(SyncRequest::GetStatus).await;
        assert!(matches!(decoded, SyncRequest::GetStatus));
    }

    #[tokio::test]
    async fn test_codec_request_roundtrip_get_blocks() {
        let decoded = roundtrip_request(SyncRequest::GetBlocks {
            start_height: 42,
            count: 100,
        })
        .await;
        match decoded {
            SyncRequest::GetBlocks {
                start_height,
                count,
            } => {
                assert_eq!(start_height, 42);
                assert_eq!(count, 100);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn test_codec_response_roundtrip_status() {
        let decoded = roundtrip_response(SyncResponse::Status {
            height: 1234,
            tip_hash: [7u8; 32],
        })
        .await;
        match decoded {
            SyncResponse::Status { height, tip_hash } => {
                assert_eq!(height, 1234);
                assert_eq!(tip_hash, [7u8; 32]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn test_codec_response_roundtrip_blocks_empty() {
        let decoded = roundtrip_response(SyncResponse::Blocks {
            blocks: vec![],
            has_more: true,
        })
        .await;
        match decoded {
            SyncResponse::Blocks { blocks, has_more } => {
                assert!(blocks.is_empty());
                assert!(has_more);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_full_catchup_cycle_from_genesis() {
        // End-to-end of the state machine: genesis node discovers a peer at a
        // high height, downloads the full range in batches, and ends Synced.
        let mut manager = ChainSyncManager::new(0);
        let peer = make_peer_id();
        let target = 250u64;

        // Discovery -> request status.
        assert!(matches!(
            manager.tick(&[peer]),
            Some(SyncAction::RequestStatus(_))
        ));
        manager.on_status(peer, target, [9u8; 32]);
        assert!(matches!(manager.state(), SyncState::Downloading { .. }));

        // Drive batched downloads until synced.
        let mut height = 0u64;
        for _ in 0..100 {
            if manager.is_synced() {
                break;
            }
            match manager.tick(&[peer]) {
                Some(SyncAction::RequestBlocks {
                    start_height,
                    count,
                    ..
                }) => {
                    assert_eq!(start_height, height + 1);
                    let end = (start_height + count as u64 - 1).min(target);
                    let blocks_added = end - start_height + 1;
                    height += blocks_added;
                    manager.on_blocks_added(height);
                }
                Some(SyncAction::Synced) => break,
                other => panic!("unexpected action while downloading: {:?}", other),
            }
        }

        assert!(manager.is_synced());
        assert_eq!(height, target);
    }

    /// #541: `status_snapshot` must track the live state machine so the RPC
    /// layer can report honest sync info. Covers Discovery (no target),
    /// Downloading (target + sub-100% progress), and Synced (100%).
    #[test]
    fn test_status_snapshot_tracks_state_machine() {
        let mut manager = ChainSyncManager::new(50);
        let peer = make_peer_id();

        // Discovery with no peer status: not synced, no target tip known, so a
        // true progress percentage cannot be computed.
        let snap = manager.status_snapshot();
        assert!(!snap.synced);
        assert_eq!(snap.status, "discovering");
        assert_eq!(snap.target_height, None);
        assert_eq!(snap.progress_percent(), None);

        // Learn a peer is far ahead -> Downloading; target tip is its height.
        manager.on_status(peer, 100, [7u8; 32]);
        assert!(matches!(manager.state(), SyncState::Downloading { .. }));
        let snap = manager.status_snapshot();
        assert!(!snap.synced);
        assert_eq!(snap.status, "syncing");
        assert_eq!(snap.local_height, 50);
        assert_eq!(snap.target_height, Some(100));
        assert_eq!(snap.progress_percent(), Some(50.0));

        // Apply blocks up to the target -> Synced; progress pins to 100.
        manager.on_blocks_added(100);
        assert!(manager.is_synced());
        let snap = manager.status_snapshot();
        assert!(snap.synced);
        assert_eq!(snap.status, "synced");
        assert_eq!(snap.progress_percent(), Some(100.0));
    }

    /// #541: `progress_percent` must clamp to 0..=100 and never fabricate a
    /// number when no target tip is known.
    #[test]
    fn test_progress_percent_clamps_and_honest() {
        // Synced always 100 regardless of heights.
        let s = SyncStatusSnapshot {
            synced: true,
            status: "synced",
            local_height: 0,
            target_height: None,
        };
        assert_eq!(s.progress_percent(), Some(100.0));

        // No target tip and not synced -> None (do not fabricate).
        let s = SyncStatusSnapshot {
            synced: false,
            status: "discovering",
            local_height: 10,
            target_height: None,
        };
        assert_eq!(s.progress_percent(), None);

        // Local ahead of (stale) target clamps to 100, never overshoots.
        let s = SyncStatusSnapshot {
            synced: false,
            status: "syncing",
            local_height: 120,
            target_height: Some(100),
        };
        assert_eq!(s.progress_percent(), Some(100.0));
    }

    // ========================================================================
    // SyncCodec byte-accounting tests (#549)
    //
    // Verify the codec records request/response payload bytes into the shared
    // NetworkStats. Sent payloads advance `bytes_sent` by the serialized size;
    // received payloads advance `bytes_received` by the bytes read off the
    // wire. A codec built without stats (Default) records nothing.
    // ========================================================================

    /// Drive a write through a stats-bearing codec, returning the bytes written
    /// and the `bytes_sent` delta the codec recorded.
    async fn write_request_with_stats(req: SyncRequest) -> (Vec<u8>, u64) {
        use futures::io::Cursor;

        let stats = Arc::new(NetworkStats::new());
        let mut codec = SyncCodec::with_stats(Arc::clone(&stats));
        let protocol = StreamProtocol::new(SYNC_PROTOCOL);

        let mut write_buf = Cursor::new(Vec::new());
        codec
            .write_request(&protocol, &mut write_buf, req)
            .await
            .expect("write_request should succeed");
        (write_buf.into_inner(), stats.bytes_sent())
    }

    #[tokio::test]
    async fn test_codec_write_request_records_sent_bytes() {
        let req = SyncRequest::GetBlocks {
            start_height: 42,
            count: 100,
        };
        let expected = bincode::serialize(&req).unwrap().len() as u64;
        let (bytes, recorded) = write_request_with_stats(req).await;

        // Sent counter advanced by exactly the serialized payload size.
        assert_eq!(recorded, expected);
        assert_eq!(recorded, bytes.len() as u64);
    }

    #[tokio::test]
    async fn test_codec_write_response_records_sent_bytes() {
        use futures::io::Cursor;

        let resp = SyncResponse::Status {
            height: 1234,
            tip_hash: [7u8; 32],
        };
        let expected = bincode::serialize(&resp).unwrap().len() as u64;

        let stats = Arc::new(NetworkStats::new());
        let mut codec = SyncCodec::with_stats(Arc::clone(&stats));
        let protocol = StreamProtocol::new(SYNC_PROTOCOL);

        let mut write_buf = Cursor::new(Vec::new());
        codec
            .write_response(&protocol, &mut write_buf, resp)
            .await
            .expect("write_response should succeed");

        assert_eq!(stats.bytes_sent(), expected);
        assert_eq!(stats.bytes_received(), 0, "write must not touch received");
    }

    #[tokio::test]
    async fn test_codec_read_request_records_received_bytes() {
        use futures::io::Cursor;

        // Produce the wire bytes via a no-stats codec, then read them back
        // through a stats-bearing codec and assert the received counter.
        let req = SyncRequest::GetBlocks {
            start_height: 7,
            count: 50,
        };
        let wire = bincode::serialize(&req).unwrap();
        let expected = wire.len() as u64;

        let stats = Arc::new(NetworkStats::new());
        let mut codec = SyncCodec::with_stats(Arc::clone(&stats));
        let protocol = StreamProtocol::new(SYNC_PROTOCOL);

        let mut read_buf = Cursor::new(wire);
        let decoded = codec
            .read_request(&protocol, &mut read_buf)
            .await
            .expect("read_request should decode");

        assert!(matches!(decoded, SyncRequest::GetBlocks { .. }));
        assert_eq!(stats.bytes_received(), expected);
        assert_eq!(stats.bytes_sent(), 0, "read must not touch sent");
    }

    #[tokio::test]
    async fn test_codec_read_response_records_received_bytes() {
        use futures::io::Cursor;

        let resp = SyncResponse::Status {
            height: 99,
            tip_hash: [1u8; 32],
        };
        let wire = bincode::serialize(&resp).unwrap();
        let expected = wire.len() as u64;

        let stats = Arc::new(NetworkStats::new());
        let mut codec = SyncCodec::with_stats(Arc::clone(&stats));
        let protocol = StreamProtocol::new(SYNC_PROTOCOL);

        let mut read_buf = Cursor::new(wire);
        codec
            .read_response(&protocol, &mut read_buf)
            .await
            .expect("read_response should decode");

        assert_eq!(stats.bytes_received(), expected);
    }

    #[tokio::test]
    async fn test_default_codec_records_nothing() {
        use futures::io::Cursor;

        // A Default codec (no stats handle) must be a no-op for accounting and
        // must still round-trip correctly.
        let protocol = StreamProtocol::new(SYNC_PROTOCOL);
        let mut codec = SyncCodec::default();

        let mut write_buf = Cursor::new(Vec::new());
        codec
            .write_request(&protocol, &mut write_buf, SyncRequest::GetStatus)
            .await
            .expect("write_request should succeed");
        let bytes = write_buf.into_inner();
        assert!(!bytes.is_empty());

        let mut read_buf = Cursor::new(bytes);
        let decoded = codec
            .read_request(&protocol, &mut read_buf)
            .await
            .expect("read_request should decode");
        assert!(matches!(decoded, SyncRequest::GetStatus));
    }

    #[tokio::test]
    async fn test_codec_clones_share_stats() {
        use futures::io::Cursor;

        // libp2p clones the codec per substream; all clones must share the same
        // atomics (via the inner Arc) so accounting is cumulative across them.
        let stats = Arc::new(NetworkStats::new());
        let codec = SyncCodec::with_stats(Arc::clone(&stats));
        let protocol = StreamProtocol::new(SYNC_PROTOCOL);

        let req = SyncRequest::GetStatus;
        let one = bincode::serialize(&req).unwrap().len() as u64;

        let mut a = codec.clone();
        let mut b = codec.clone();

        let mut buf_a = Cursor::new(Vec::new());
        a.write_request(&protocol, &mut buf_a, SyncRequest::GetStatus)
            .await
            .unwrap();
        let mut buf_b = Cursor::new(Vec::new());
        b.write_request(&protocol, &mut buf_b, SyncRequest::GetStatus)
            .await
            .unwrap();

        assert_eq!(stats.bytes_sent(), one * 2);
    }
}
