// Copyright (c) 2024 Botho Foundation

//! Peer discovery and gossip networking using libp2p.
//!
//! ## Protocol Versioning
//!
//! This module implements version negotiation for protocol upgrades:
//!
//! - **Protocol Version**: Embedded in the libp2p identify protocol's
//!   agent_version field as `botho/<version>/<block_version>`. This allows
//!   peers to discover compatibility during connection establishment.
//!
//! - **Minimum Supported Version**: Defines the oldest protocol version this
//!   node will connect to. Peers below this threshold receive a warning but are
//!   not disconnected (graceful degradation).
//!
//! - **Upgrade Announcements**: A dedicated gossipsub topic allows validators
//!   and seed nodes to broadcast upcoming network upgrades.

use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity},
    identify, identity, noise,
    request_response::{self, InboundRequestId, OutboundRequestId, ResponseChannel},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// Rate limiting
use bth_gossip::{GossipMessageType, PeerRateLimitConfig, PeerRateLimiter, RateLimitResult};

use bth_transaction_types::{
    BlockVersion, MAX_BLOCK_SIZE, MAX_SCP_MESSAGE_SIZE, MAX_TRANSACTION_SIZE,
};

use crate::{
    block::{Block, MintingTx},
    consensus::ScpMessage,
    network::{
        compact_block::{BlockTxn, CompactBlock, GetBlockTxn},
        pex::{PeerSource, PexManager, PexMessage, MAX_PEX_MESSAGE_SIZE},
        sync::{create_sync_behaviour, SyncCodec, SyncRequest, SyncResponse},
    },
    transaction::Transaction,
};

/// Current protocol version string.
/// Format: major.minor.patch
/// - Major: Breaking changes requiring hard fork
/// - Minor: Soft fork compatible changes
/// - Patch: Bug fixes, no consensus impact
///
/// Bumped to 2.0.0 for the multi-input CLSAG balance fix (audit finding I4):
/// `ClsagRingInput` now carries a per-input `pseudo_output_amount`, a
/// consensus-breaking change to the transaction wire format. Nodes running the
/// old (1.x) format are consensus-incompatible and are disconnected by the
/// major-version check. Coordinate with the planned testnet reset.
///
/// Bumped to 3.0.0 for the cycle-6 H1 consensus work (issue #606, successor to
/// #323). Current `main` now enforces block-acceptance rules that the running
/// 2.0.0 chain does not: the deterministic consensus fee floor in `add_block`
/// (H1, PR #602), block fee-sum overflow rejection (#601), and fail-closed
/// key-image double-spend checks in the store and mempool (#600/#564/#598).
/// These are consensus-incompatible with the 2.0.0 chain's already-produced
/// history, so a rolling upgrade is impossible — a coordinated testnet reset
/// with a fresh genesis is required.
///
/// A MAJOR bump (not 2.1.0) is required because the peer-disconnect mechanism
/// (`is_consensus_compatible` / `consensus_incompatibility`) compares major
/// versions only: minor/patch differences within the same major merely warn
/// (graceful soft-fork behavior). Only a major bump actually disconnects
/// 2.0.0 peers instead of letting them silently fork against the new
/// block-acceptance rules. `MIN_SUPPORTED_PROTOCOL_VERSION` is raised in
/// lockstep.
///
/// Bumped to 4.0.0 for the coordinated testnet reset deploying the #626
/// consensus changes and the ratified #605 semantics. #626 replaces the C7
/// consensus fee floor with a log-domain fee curve and widens cluster-wealth
/// accounting to u128 (#627/#628/#629), and #605 ratifies the cluster-wealth
/// decay semantics that ride the same reset. Because #626 changes the C7 fee
/// floor a block must satisfy to be accepted, 3.0.0 peers are
/// consensus-incompatible with the reset chain's block-acceptance rules — they
/// would compute a different floor and fork. As with the 2.x -> 3.0.0 bump this
/// must be MAJOR: `is_consensus_compatible` / `consensus_incompatibility`
/// compare major versions only, so only a major bump disconnects 3.0.0 peers
/// rather than letting them silently fork. `MIN_SUPPORTED_PROTOCOL_VERSION` is
/// raised in lockstep.
///
/// Bumped to 4.1.0 for the #694 nanoBTH -> picocredits unit migration
/// (decision #649). This retires the two-tier unit system: the RPC contract's
/// unit declarations change (`baseRate` and cluster-wealth fields are
/// picocredit-denominated), and wallets/adapters must be updated in lockstep.
/// The bump is MINOR, not major, because no consensus rule changes: the fee
/// curve, consensus fee floor (`CONSENSUS_FEE_BASE`), emission constants
/// (`mainnet_policy`) and all on-chain amounts were already picocredit-native
/// since the 4.0.0 reset (#626) — every migrated value is identical in BTH
/// terms, so 4.0.0 peers accept exactly the same blocks. Per the #608 lesson:
/// major = consensus-breaking disconnect, minor = warn (soft, RPC-shape-only).
/// PROTOCOL 5.0.0 (ADR 0007, #938): bridge-import cluster tagging. Unwrapping
/// wBTH → BTH now tags the minted output 100% to a block-epoch import cluster
/// `c_import(⌊height/K⌋)` (K = 17,280 blocks) instead of returning it at
/// factor-1 (background), and the consensus fee floor prices any import-tagged
/// value at ≥ F = 1.5× on that fraction until it circulates the tag off. This
/// is a CONSENSUS-BREAKING change: a 4.x peer applies no import floor and would
/// accept/produce blocks whose release outputs the 5.0.0 chain now expects to
/// be import-tagged and floored, so the two fork. The bump is therefore MAJOR
/// (`is_consensus_compatible` compares majors only — a minor bump would merely
/// warn, leaving 4.x peers connected and silently forking) and
/// `MIN_SUPPORTED_PROTOCOL_VERSION` rises to 5.0.0 in lockstep (pre-mainnet
/// testnet reset — no in-place migration; the new `bridge_import_clusters`
/// index is built from genesis). Interacts with #925 (the remaining spend-to-
/// background reset door) only in that both touch the factor-floor area; they
/// are SEPARATE mechanisms.
/// PROTOCOL 6.0.0 (#925, background-reset leak): the consensus fee floor now
/// prices a demurrage class DOWNGRADE — a spend whose declared output factor
/// drops below the composed ring-implied input floor — at capitalized future
/// demurrage over `SETTLEMENT_HORIZON_BLOCKS` (`max(accrued,
/// capitalized_reset)` via `bth_cluster_tax::spend_demurrage_charge`), routed
/// to the lottery pool. This closes the last domestic reset door (spend
/// young-wealthy → background paid ≈0). It is CONSENSUS-BREAKING: a 5.x peer
/// charges only accrued-to-date demurrage and would accept/produce blocks whose
/// deflating spends the 6.0.0 chain now requires to pay the higher downgrade
/// floor, so the two fork. MAJOR bump (`is_consensus_compatible` is major-only)
/// with `MIN_SUPPORTED_PROTOCOL_VERSION` rising to 6.0.0 in lockstep
/// (pre-mainnet testnet reset). Shares the single `SETTLEMENT_HORIZON_BLOCKS`
/// dial with #831.
pub const PROTOCOL_VERSION: &str = "6.0.0";

/// Minimum supported protocol version.
/// Peers below this version are consensus-incompatible and are disconnected.
/// Raised to 4.0.0 alongside `PROTOCOL_VERSION` for the reset deploying the
/// #626 log-domain fee curve (u128 cluster wealth, #627/#628/#629) and the
/// ratified #605 semantics: 3.0.0 peers apply the old C7 fee floor and would
/// accept/produce blocks the reset chain rejects, so they must be dropped
/// rather than allowed to fork. The bump is MAJOR because the disconnect check
/// (`is_consensus_compatible`) is major-only; a minor bump would only warn.
///
/// Deliberately NOT raised for the 4.1.0 minor bump (#694 unit migration):
/// 4.0.0 peers remain consensus-compatible (no block-acceptance rule changed),
/// so they kept connecting rather than being disconnected.
///
/// Raised to 5.0.0 for the ADR 0007 bridge-import cluster tagging + ≥F import
/// floor (#938): 4.x peers apply no import floor and would fork the
/// import-tagged/floored chain, so they must be disconnected (major-only
/// check).
///
/// Raised to 6.0.0 for the #925 downgrade-charge consensus rule: 5.x peers
/// price only accrued-to-date demurrage and would fork the chain that now
/// charges the capitalized reset on deflating spends, so they must be
/// disconnected.
pub const MIN_SUPPORTED_PROTOCOL_VERSION: &str = "6.0.0";

/// Topic for block announcements
const BLOCKS_TOPIC: &str = "botho/blocks/1.0.0";

/// Topic for transaction announcements
const TRANSACTIONS_TOPIC: &str = "botho/transactions/1.0.0";

/// Topic for SCP consensus messages
const SCP_TOPIC: &str = "botho/scp/1.0.0";

/// Topic for minting-transaction announcements.
///
/// Minting transactions referenced by an SCP `ConsensusValue` are not part of
/// the mempool transaction flow (`TRANSACTIONS_TOPIC` carries user
/// `Transaction`s), so a node that only learns a peer's minting `tx_hash` from
/// an SCP nominate/ballot message has no way to validate it. Without the raw
/// bytes the SCP slot rejects the peer's message (validity_fn: "Transaction not
/// in cache") and nomination can never reach quorum. This topic propagates the
/// minting-tx bytes so every consensus participant can validate a proposed
/// minting value. See issue #409.
const MINTING_TXS_TOPIC: &str = "botho/minting-txs/1.0.0";

/// Topic for compact block announcements
const COMPACT_BLOCKS_TOPIC: &str = "botho/compact-blocks/1.0.0";

/// Topic for upgrade announcements.
/// Validators and seed nodes publish upcoming network upgrades here.
const UPGRADE_ANNOUNCEMENTS_TOPIC: &str = "botho/upgrades/1.0.0";

/// Topic for peer exchange (PEX) messages
const PEX_TOPIC: &str = "botho/pex/1.0.0";

/// Parsed protocol version from peer agent string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolVersion {
    /// Major version (breaking changes)
    pub major: u32,
    /// Minor version (soft fork compatible)
    pub minor: u32,
    /// Patch version (bug fixes)
    pub patch: u32,
    /// Maximum block version supported by peer
    pub block_version: Option<u32>,
}

impl ProtocolVersion {
    /// Parse a version string like "1.0.0" or agent string like "botho/1.0.0/5"
    pub fn parse(s: &str) -> Option<Self> {
        // Handle full agent string format: "botho/1.0.0/5"
        let version_str = if s.starts_with("botho/") {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() >= 2 {
                parts[1]
            } else {
                return None;
            }
        } else {
            s
        };

        let parts: Vec<&str> = version_str.split('.').collect();
        if parts.len() != 3 {
            return None;
        }

        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts[2].parse().ok()?;

        // Try to parse block version from agent string
        let block_version = if s.starts_with("botho/") {
            let agent_parts: Vec<&str> = s.split('/').collect();
            if agent_parts.len() >= 3 {
                agent_parts[2].parse().ok()
            } else {
                None
            }
        } else {
            None
        };

        Some(Self {
            major,
            minor,
            patch,
            block_version,
        })
    }

    /// Check if this version is compatible with another version.
    /// Returns true if major versions match and this version >= other.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        if self.major != other.major {
            return false;
        }
        if self.minor > other.minor {
            return true;
        }
        if self.minor == other.minor && self.patch >= other.patch {
            return true;
        }
        false
    }

    /// Check consensus compatibility: major versions must match.
    ///
    /// Major version bumps mark consensus-breaking changes; peers on a
    /// different major are DISCONNECTED (this is what makes a coordinated
    /// upgrade enforceable — old nodes are excluded from the new network
    /// rather than silently diverging). Minor/patch differences within the
    /// same major only warn, allowing graceful soft-fork upgrades.
    pub fn is_consensus_compatible(&self, other: &Self) -> bool {
        self.major == other.major
    }

    /// Decide whether a peer must be disconnected as consensus-incompatible.
    ///
    /// Returns the peer version to report in the disconnect event, or `None`
    /// if the peer is compatible. An unparseable agent string fails closed
    /// (reported as the sentinel version 0.0.0): a peer that cannot state a
    /// valid protocol version cannot be assumed to share our consensus
    /// rules, and honest-but-misconfigured peers are exactly what this
    /// check exists to exclude.
    pub fn consensus_incompatibility(peer_version: &Option<Self>, local: &Self) -> Option<Self> {
        match peer_version {
            Some(pv) => (!pv.is_consensus_compatible(local)).then(|| pv.clone()),
            None => Some(Self {
                major: 0,
                minor: 0,
                patch: 0,
                block_version: None,
            }),
        }
    }

    /// Create agent version string for libp2p identify protocol.
    pub fn to_agent_string(&self, block_version: u32) -> String {
        format!(
            "botho/{}.{}.{}/{}",
            self.major, self.minor, self.patch, block_version
        )
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(bv) = self.block_version {
            write!(f, " (block v{})", bv)?;
        }
        Ok(())
    }
}

/// Entry in the peer table
#[derive(Debug, Clone)]
pub struct PeerTableEntry {
    pub peer_id: PeerId,
    pub address: Option<Multiaddr>,
    pub last_seen: std::time::Instant,
    /// Peer's protocol version (parsed from identify agent_version)
    pub protocol_version: Option<ProtocolVersion>,
    /// Whether this peer's version is below minimum supported
    pub version_warning: bool,
    /// Peer's transport capabilities (parsed from identify agent_version)
    pub transport_capabilities: Option<super::transport::TransportCapabilities>,
}

/// Upgrade announcement broadcast via gossipsub.
///
/// Validators and seed nodes publish these to notify the network
/// of upcoming protocol upgrades.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeAnnouncement {
    /// New protocol version after upgrade
    pub target_version: String,
    /// Target block version after upgrade
    pub target_block_version: u32,
    /// Block height at which upgrade activates (0 = time-based)
    pub activation_height: Option<u64>,
    /// Unix timestamp at which upgrade activates (0 = height-based)
    pub activation_timestamp: Option<u64>,
    /// Human-readable description of the upgrade
    pub description: String,
    /// Whether this is a hard fork (breaking) or soft fork
    pub is_hard_fork: bool,
    /// Minimum version required after upgrade
    pub min_version_after: String,
}

/// Events from the network layer
#[derive(Debug)]
pub enum NetworkEvent {
    /// A new block was received from a peer
    NewBlock(Block),
    /// A new transaction was received from a peer
    NewTransaction(Transaction),
    /// A minting transaction proposed for consensus was received from a peer.
    /// Registered into the consensus tx cache so the local SCP node can
    /// validate the corresponding `ConsensusValue` (see issue #409).
    NewMintingTx(MintingTx),
    /// An SCP consensus message was received
    ScpMessage(ScpMessage),
    /// A compact block was received (for bandwidth-efficient relay)
    NewCompactBlock(CompactBlock),
    /// A request for missing transactions was received
    GetBlockTxn { peer: PeerId, request: GetBlockTxn },
    /// Missing transactions were received
    BlockTxn(BlockTxn),
    /// A new peer was discovered
    PeerDiscovered(PeerId),
    /// A peer disconnected
    PeerDisconnected(PeerId),
    /// A sync request was received (need to respond)
    SyncRequest {
        peer: PeerId,
        request_id: InboundRequestId,
        request: SyncRequest,
        channel: ResponseChannel<SyncResponse>,
    },
    /// A sync response was received
    SyncResponse {
        peer: PeerId,
        request_id: OutboundRequestId,
        response: SyncResponse,
    },
    /// An upgrade announcement was received from the network.
    /// Node operators should take action based on the announcement.
    UpgradeAnnouncement(UpgradeAnnouncement),
    /// A peer with an outdated protocol version was detected.
    /// This is informational; the peer is not disconnected.
    PeerVersionWarning {
        peer: PeerId,
        peer_version: ProtocolVersion,
        min_version: ProtocolVersion,
    },
    /// A peer with a consensus-incompatible protocol version (different
    /// major) was detected. The caller MUST disconnect this peer: major
    /// version bumps mark consensus-breaking changes, and keeping such
    /// peers connected risks silent divergence instead of a clean
    /// coordinated upgrade.
    PeerVersionIncompatible {
        peer: PeerId,
        peer_version: ProtocolVersion,
        local_version: ProtocolVersion,
    },
    /// New peer addresses received via PEX (connect to these)
    PexAddresses(Vec<Multiaddr>),
}

/// Network behaviour combining gossipsub, identify, and sync request-response
#[derive(NetworkBehaviour)]
pub struct BothoBehaviour {
    /// Gossipsub for block propagation
    pub gossipsub: gossipsub::Behaviour,
    /// Request-response for chain sync
    pub sync: request_response::Behaviour<SyncCodec>,
    /// Identify protocol for version negotiation
    pub identify: identify::Behaviour,
}

/// Live, node-wide network traffic and connection-direction counters (#542).
///
/// These are the real values surfaced by `network_getInfo` as `bytesSent`,
/// `bytesReceived`, `inboundCount`, and `outboundCount` (previously hardcoded
/// to `0`). Counters are lock-free [`AtomicU64`]s so the hot send/receive paths
/// never take a lock, and the RPC layer reads a cheap snapshot via a shared
/// [`Arc`].
///
/// ## What is counted
///
/// - **Byte counters** track *application-layer payload* bytes from both
///   message paths:
///   - *Gossipsub*: the serialized length of every message published
///     (`bytes_sent`) and the length of every gossipsub message received
///     (`bytes_received`) — blocks, transactions, SCP consensus, compact
///     blocks, minting txs, PEX, and upgrade announcements.
///   - *Sync request/response*: the serialized length of every sync request and
///     response written/read at the [`SyncCodec`] boundary (#549), so
///     initial-sync and catch-up block downloads — which flow over libp2p
///     `request_response`, not gossipsub — are counted too.
/// - **Connection counters** track the libp2p connection direction: a
///   connection we dialed is *outbound*, a connection a remote peer dialed is
///   *inbound*. Counted on first establishment to a peer and decremented when
///   the last connection to that peer closes.
///
/// ## Known gaps (intentional — see #542)
///
/// - Transport framing overhead (Noise handshake, yamux framing, TCP headers)
///   is NOT counted; these are payload counters, not raw wire counters (#550,
///   still open).
///
/// [`SyncCodec`]: crate::network::sync::SyncCodec
#[derive(Debug, Default)]
pub struct NetworkStats {
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    inbound_count: AtomicU64,
    outbound_count: AtomicU64,
}

impl NetworkStats {
    /// Create a fresh set of zeroed counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `n` bytes sent on the wire (serialized gossipsub payload).
    pub fn record_sent(&self, n: u64) {
        self.bytes_sent.fetch_add(n, Ordering::Relaxed);
    }

    /// Record `n` bytes received on the wire (gossipsub message payload).
    pub fn record_received(&self, n: u64) {
        self.bytes_received.fetch_add(n, Ordering::Relaxed);
    }

    /// Record a newly established connection in the given direction
    /// (`inbound == true` for a remote-dialed connection).
    pub fn record_connection_opened(&self, inbound: bool) {
        if inbound {
            self.inbound_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.outbound_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a fully-closed connection in the given direction. Saturates at 0
    /// so a spurious close can never underflow the counter.
    pub fn record_connection_closed(&self, inbound: bool) {
        let counter = if inbound {
            &self.inbound_count
        } else {
            &self.outbound_count
        };
        // Compare-and-swap loop to saturate at zero (avoids u64 underflow).
        let mut current = counter.load(Ordering::Relaxed);
        while current > 0 {
            match counter.compare_exchange_weak(
                current,
                current - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Total application-layer bytes sent since startup.
    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }

    /// Total application-layer bytes received since startup.
    pub fn bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    /// Current number of inbound (remote-dialed) connections.
    pub fn inbound_count(&self) -> u64 {
        self.inbound_count.load(Ordering::Relaxed)
    }

    /// Current number of outbound (locally-dialed) connections.
    pub fn outbound_count(&self) -> u64 {
        self.outbound_count.load(Ordering::Relaxed)
    }
}

/// Network discovery and gossip service
pub struct NetworkDiscovery {
    /// Persistent libp2p identity keypair (issue #439).
    ///
    /// This is the single, canonical identity for the node: it is used both to
    /// derive `local_peer_id` and to build the swarm in [`start`], so the
    /// logged peer ID always matches the swarm's actual peer ID. Persisting it
    /// to disk keeps the peer ID stable across restarts.
    keypair: identity::Keypair,
    /// Local peer ID
    local_peer_id: PeerId,
    /// Gossip port
    port: u16,
    /// Bootstrap peers
    bootstrap_peers: Vec<String>,
    /// Sender for network events
    event_tx: mpsc::Sender<NetworkEvent>,
    /// Receiver for network events (taken by consumer)
    event_rx: Option<mpsc::Receiver<NetworkEvent>>,
    /// Known peers
    peers: HashMap<PeerId, PeerTableEntry>,
    /// Peers subscribed to compact blocks topic (support bandwidth
    /// optimization)
    compact_block_peers: HashSet<PeerId>,
    /// PEX manager for decentralized peer discovery
    pex_manager: PexManager,
    /// Per-peer rate limiter for gossipsub messages (DoS protection)
    rate_limiter: PeerRateLimiter,
    /// Live traffic / connection-direction counters surfaced by
    /// `network_getInfo` (#542). Shared (cheap [`Arc`] clone) with the RPC
    /// layer via [`stats`](Self::stats); incremented on the send/receive
    /// and connect/disconnect paths.
    stats: Arc<NetworkStats>,
}

impl NetworkDiscovery {
    /// Create a new network discovery service.
    ///
    /// Generates an ephemeral identity keypair. Production startup should
    /// prefer [`with_keypair`](Self::with_keypair) with a persisted key so
    /// the peer ID is stable across restarts (issue #439).
    pub fn new(port: u16, bootstrap_peers: Vec<String>) -> Self {
        Self::with_rate_limit_config(port, bootstrap_peers, PeerRateLimitConfig::default())
    }

    /// Create a new network discovery service with custom rate limit
    /// configuration.
    ///
    /// Generates an ephemeral identity keypair. Production startup should
    /// prefer [`with_keypair`](Self::with_keypair) with a persisted key so
    /// the peer ID is stable across restarts (issue #439).
    pub fn with_rate_limit_config(
        port: u16,
        bootstrap_peers: Vec<String>,
        rate_limit_config: PeerRateLimitConfig,
    ) -> Self {
        Self::with_keypair(
            identity::Keypair::generate_ed25519(),
            port,
            bootstrap_peers,
            rate_limit_config,
        )
    }

    /// Create a new network discovery service from an explicit identity
    /// keypair (issue #439).
    ///
    /// The supplied keypair is the node's single canonical identity: it is used
    /// both for the logged/queried `local_peer_id` and to build the swarm in
    /// [`start`](Self::start). Passing a keypair loaded from disk keeps the
    /// peer ID stable across restarts (the prerequisite for durable DNS-seed
    /// discovery), and ensures a startup logs exactly ONE peer ID.
    pub fn with_keypair(
        keypair: identity::Keypair,
        port: u16,
        bootstrap_peers: Vec<String>,
        rate_limit_config: PeerRateLimitConfig,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);

        let local_peer_id = PeerId::from(keypair.public());

        info!("Local peer ID: {}", local_peer_id);
        info!(
            "Rate limiting: {} (limits: tx={}/min, blocks={}/min, scp={}/min)",
            if rate_limit_config.enabled {
                "enabled"
            } else {
                "disabled"
            },
            rate_limit_config.message_limits.transactions_per_minute,
            rate_limit_config.message_limits.blocks_per_minute,
            rate_limit_config.message_limits.consensus_per_minute,
        );

        Self {
            keypair,
            local_peer_id,
            port,
            bootstrap_peers,
            event_tx,
            event_rx: Some(event_rx),
            peers: HashMap::new(),
            compact_block_peers: HashSet::new(),
            pex_manager: PexManager::new(),
            rate_limiter: PeerRateLimiter::new(rate_limit_config),
            stats: Arc::new(NetworkStats::new()),
        }
    }

    /// Get a shared handle to the live network-traffic counters (#542).
    ///
    /// `commands::run` clones this once at startup and hands it to the RPC
    /// layer, which reads `bytesSent` / `bytesReceived` / `inboundCount` /
    /// `outboundCount` from it for `network_getInfo`. The clone is a cheap
    /// [`Arc`] bump; both sides observe the same atomics.
    pub fn stats(&self) -> Arc<NetworkStats> {
        Arc::clone(&self.stats)
    }

    /// Borrow the live network-traffic counters (#542). Used by `commands::run`
    /// to record sent bytes at the static `broadcast_*` call sites, which take
    /// the swarm by `&mut` but have no `self`. `discovery` and the `swarm` are
    /// independent values, so this immutable borrow coexists with `&mut swarm`.
    pub fn stats_ref(&self) -> &NetworkStats {
        &self.stats
    }

    /// Get the local peer ID
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<NetworkEvent>> {
        self.event_rx.take()
    }

    /// Get current peer count
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get peer table entries
    pub fn peer_table(&self) -> Vec<PeerTableEntry> {
        self.peers.values().cloned().collect()
    }

    /// Check if a peer supports compact blocks (subscribed to the topic)
    pub fn peer_supports_compact_blocks(&self, peer_id: &PeerId) -> bool {
        self.compact_block_peers.contains(peer_id)
    }

    /// Count of connected peers that don't support compact blocks
    ///
    /// These "legacy" peers need full block broadcasts.
    pub fn legacy_peer_count(&self) -> usize {
        self.peers
            .keys()
            .filter(|p| !self.compact_block_peers.contains(p))
            .count()
    }

    /// Check if all connected peers support compact blocks
    pub fn all_peers_support_compact_blocks(&self) -> bool {
        self.peers
            .keys()
            .all(|p| self.compact_block_peers.contains(p))
    }

    /// Get the current protocol version
    pub fn protocol_version() -> &'static str {
        PROTOCOL_VERSION
    }

    /// Get peers with version warnings (below minimum supported)
    pub fn peers_with_version_warnings(&self) -> Vec<&PeerTableEntry> {
        self.peers.values().filter(|p| p.version_warning).collect()
    }

    /// Get count of peers with outdated versions
    pub fn outdated_peer_count(&self) -> usize {
        self.peers.values().filter(|p| p.version_warning).count()
    }

    /// Map a gossipsub topic to a rate limit message type
    fn topic_to_message_type(topic: &str) -> GossipMessageType {
        if topic == BLOCKS_TOPIC || topic == COMPACT_BLOCKS_TOPIC {
            GossipMessageType::Block
        } else if topic == TRANSACTIONS_TOPIC {
            GossipMessageType::Transaction
        } else if topic == SCP_TOPIC {
            GossipMessageType::Consensus
        } else if topic == PEX_TOPIC {
            GossipMessageType::PeerExchange
        } else if topic == UPGRADE_ANNOUNCEMENTS_TOPIC {
            GossipMessageType::Announcement
        } else {
            GossipMessageType::Other
        }
    }

    /// Get peers flagged for disconnection due to rate limit violations
    /// and clear the internal list.
    pub fn take_rate_limited_peers(&mut self) -> Vec<PeerId> {
        self.rate_limiter.take_flagged_peers()
    }

    /// Get the number of tracked peers in the rate limiter
    pub fn rate_limited_peer_count(&self) -> usize {
        self.rate_limiter.tracked_peer_count()
    }

    /// Remove a peer from rate limiting when disconnected
    pub fn on_peer_disconnected(&mut self, peer: &PeerId) {
        self.rate_limiter.remove_peer(peer);
    }

    /// Start the network service (runs in background)
    pub async fn start(&mut self) -> anyhow::Result<Swarm<BothoBehaviour>> {
        // Build the swarm from the node's canonical identity keypair (issue
        // #439). Using `with_existing_identity` (rather than the previous
        // `with_new_identity`, which minted a SECOND, throwaway keypair) means
        // the swarm's peer ID matches `self.local_peer_id` and is stable across
        // restarts when the keypair was loaded from disk.
        //
        // Clone the shared traffic counters up front so the `with_behaviour`
        // closure can hand them to the sync codec without borrowing `self`
        // (#549).
        let stats = Arc::clone(&self.stats);
        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(self.keypair.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                // Configure gossipsub with message size limits
                // Use MAX_BLOCK_SIZE as the limit since blocks are the largest messages
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(1))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .max_transmit_size(MAX_BLOCK_SIZE)
                    .build()
                    .map_err(std::io::Error::other)?;

                let gossipsub = gossipsub::Behaviour::new(
                    MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .map_err(std::io::Error::other)?;

                // Create sync request-response behaviour, wiring in the shared
                // traffic counters so request/response payload bytes are
                // counted by `network_getInfo` (#549).
                let sync = create_sync_behaviour(Arc::clone(&stats));

                // Configure identify protocol with version information
                // Agent version format: "botho/<protocol_version>/<block_version>"
                let agent_version = format!("botho/{}/{}", PROTOCOL_VERSION, *BlockVersion::MAX);
                let identify_config =
                    identify::Config::new("/botho/id/1.0.0".to_string(), key.public())
                        .with_agent_version(agent_version);
                let identify = identify::Behaviour::new(identify_config);

                Ok(BothoBehaviour {
                    gossipsub,
                    sync,
                    identify,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // Subscribe to blocks topic
        let blocks_topic = IdentTopic::new(BLOCKS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&blocks_topic)?;

        // Subscribe to transactions topic
        let transactions_topic = IdentTopic::new(TRANSACTIONS_TOPIC);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&transactions_topic)?;

        // Subscribe to SCP consensus topic
        let scp_topic = IdentTopic::new(SCP_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&scp_topic)?;

        // Subscribe to minting-transactions topic (issue #409)
        let minting_txs_topic = IdentTopic::new(MINTING_TXS_TOPIC);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&minting_txs_topic)?;

        // Subscribe to compact blocks topic
        let compact_blocks_topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&compact_blocks_topic)?;

        // Subscribe to upgrade announcements topic
        let upgrade_topic = IdentTopic::new(UPGRADE_ANNOUNCEMENTS_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&upgrade_topic)?;

        // Subscribe to PEX topic
        let pex_topic = IdentTopic::new(PEX_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&pex_topic)?;

        // Listen on the configured port
        let listen_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", self.port).parse()?;
        swarm.listen_on(listen_addr)?;

        // Connect to bootstrap peers
        for peer_addr in &self.bootstrap_peers {
            match peer_addr.parse::<Multiaddr>() {
                Ok(addr) => {
                    info!("Dialing bootstrap peer: {}", addr);
                    if let Err(e) = swarm.dial(addr.clone()) {
                        warn!("Failed to dial {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    warn!("Invalid bootstrap peer address {}: {}", peer_addr, e);
                }
            }
        }

        // The swarm now derives from the same canonical keypair, so its peer ID
        // already equals `self.local_peer_id`; this assignment is a no-op kept
        // for clarity. Guard against any future drift in debug builds.
        debug_assert_eq!(self.local_peer_id, *swarm.local_peer_id());
        self.local_peer_id = *swarm.local_peer_id();
        info!("Network started on port {}", self.port);

        Ok(swarm)
    }

    /// Broadcast a new block to the network
    pub fn broadcast_block(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        block: &Block,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(BLOCKS_TOPIC);
        let block_bytes = bincode::serialize(block)?;
        let len = block_bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, block_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish block: {:?}", e))?;
        stats.record_sent(len);

        debug!("Broadcast block {} to network", block.height());
        Ok(())
    }

    /// Broadcast a transaction to the network
    pub fn broadcast_transaction(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        tx: &Transaction,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(TRANSACTIONS_TOPIC);
        let tx_bytes = bincode::serialize(tx)?;
        let len = tx_bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, tx_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish transaction: {:?}", e))?;
        stats.record_sent(len);

        debug!(
            "Broadcast transaction {} to network",
            hex::encode(&tx.hash()[0..8])
        );
        Ok(())
    }

    /// Broadcast a minting transaction to the network.
    ///
    /// Proposing minters call this so peers can validate the minting
    /// `ConsensusValue` that references this tx when it appears in an SCP
    /// message (issue #409).
    pub fn broadcast_minting_tx(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        minting_tx: &MintingTx,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(MINTING_TXS_TOPIC);
        let tx_bytes = bincode::serialize(minting_tx)?;
        let len = tx_bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, tx_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish minting tx: {:?}", e))?;
        stats.record_sent(len);

        debug!(
            "Broadcast minting tx {} to network",
            hex::encode(&minting_tx.hash()[0..8])
        );
        Ok(())
    }

    /// Broadcast an SCP consensus message to the network
    pub fn broadcast_scp(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        msg: &ScpMessage,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(SCP_TOPIC);
        let msg_bytes = bincode::serialize(msg)?;
        let len = msg_bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, msg_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish SCP message: {:?}", e))?;
        stats.record_sent(len);

        debug!(slot = msg.slot_index, "Broadcast SCP message");
        Ok(())
    }

    /// Broadcast a compact block to the network (bandwidth-efficient relay)
    pub fn broadcast_compact_block(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        compact_block: &CompactBlock,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        let bytes = bincode::serialize(compact_block)?;
        let len = bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish compact block: {:?}", e))?;
        stats.record_sent(len);

        debug!(
            height = compact_block.height(),
            txs = compact_block.short_ids.len(),
            "Broadcast compact block"
        );
        Ok(())
    }

    /// Request missing transactions for compact block reconstruction
    pub fn request_block_txns(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        request: &GetBlockTxn,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        let bytes = bincode::serialize(request)?;
        let len = bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish GetBlockTxn: {:?}", e))?;
        stats.record_sent(len);

        debug!(
            block = hex::encode(&request.block_hash[0..8]),
            missing = request.indices.len(),
            "Requested missing transactions"
        );
        Ok(())
    }

    /// Respond with missing transactions for compact block reconstruction
    pub fn respond_block_txns(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        response: &BlockTxn,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(COMPACT_BLOCKS_TOPIC);
        let bytes = bincode::serialize(response)?;
        let len = bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish BlockTxn: {:?}", e))?;
        stats.record_sent(len);

        debug!(
            block = hex::encode(&response.block_hash[0..8]),
            txs = response.txs.len(),
            "Sent missing transactions"
        );
        Ok(())
    }

    /// Broadcast a PEX message with known peers
    pub fn broadcast_pex(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        message: &PexMessage,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(PEX_TOPIC);
        let bytes = bincode::serialize(message)?;
        let len = bytes.len() as u64;

        // Size check
        if bytes.len() > MAX_PEX_MESSAGE_SIZE {
            return Err(anyhow::anyhow!(
                "PEX message too large: {} bytes (max: {})",
                bytes.len(),
                MAX_PEX_MESSAGE_SIZE
            ));
        }

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish PEX message: {:?}", e))?;
        stats.record_sent(len);

        debug!(peers = message.entries.len(), "Broadcast PEX message");
        Ok(())
    }

    /// Get the PEX manager for external use
    pub fn pex_manager(&self) -> &PexManager {
        &self.pex_manager
    }

    /// Get mutable PEX manager
    pub fn pex_manager_mut(&mut self) -> &mut PexManager {
        &mut self.pex_manager
    }

    /// Check if we should broadcast PEX and do it if ready
    ///
    /// Call this periodically (e.g., every minute) to share known peers.
    pub fn maybe_broadcast_pex(&mut self, swarm: &mut Swarm<BothoBehaviour>) {
        if !self.pex_manager.should_broadcast() {
            return;
        }

        // Collect shareable peers
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let peers: Vec<_> = self
            .peers
            .values()
            .filter_map(|entry| {
                entry.address.as_ref().map(|addr| {
                    let last_seen =
                        current_time - entry.last_seen.elapsed().as_secs().min(current_time);
                    (entry.peer_id, addr.clone(), last_seen)
                })
            })
            .collect();

        if let Some(message) = self.pex_manager.prepare_broadcast(peers) {
            if let Err(e) = Self::broadcast_pex(swarm, &self.stats, &message) {
                warn!("Failed to broadcast PEX: {}", e);
            } else {
                self.pex_manager.record_broadcast();
            }
        }
    }

    /// Record a peer with its discovery source for eclipse attack prevention
    pub fn record_peer_source(&mut self, peer_id: PeerId, addr: &Multiaddr, source: PeerSource) {
        self.pex_manager
            .source_tracker
            .record_peer(peer_id, addr, source);
    }

    /// Broadcast a block with bandwidth optimization.
    ///
    /// Always sends a compact block. Only sends the full block if there are
    /// legacy peers that don't support compact block relay.
    pub fn broadcast_block_smart(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        block: &Block,
        legacy_peers_exist: bool,
    ) -> anyhow::Result<()> {
        // Always send compact block (bandwidth-efficient for upgraded peers)
        let compact_block = CompactBlock::from_block(block);
        Self::broadcast_compact_block(swarm, stats, &compact_block)?;

        // Only send full block if there are legacy peers
        if legacy_peers_exist {
            Self::broadcast_block(swarm, stats, block)?;
            debug!(height = block.height(), "Sent full block for legacy peers");
        } else {
            debug!(
                height = block.height(),
                "Skipped full block - all peers support compact blocks"
            );
        }

        Ok(())
    }

    /// Broadcast an upgrade announcement to the network.
    ///
    /// This should only be called by validators or seed nodes to notify
    /// the network of upcoming protocol upgrades.
    pub fn broadcast_upgrade_announcement(
        swarm: &mut Swarm<BothoBehaviour>,
        stats: &NetworkStats,
        announcement: &UpgradeAnnouncement,
    ) -> anyhow::Result<()> {
        let topic = IdentTopic::new(UPGRADE_ANNOUNCEMENTS_TOPIC);
        let bytes = bincode::serialize(announcement)?;
        let len = bytes.len() as u64;

        swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, bytes)
            .map_err(|e| anyhow::anyhow!("Failed to publish upgrade announcement: {:?}", e))?;
        stats.record_sent(len);

        info!(
            target_version = %announcement.target_version,
            target_block_version = announcement.target_block_version,
            is_hard_fork = announcement.is_hard_fork,
            "Broadcast upgrade announcement"
        );
        Ok(())
    }

    /// Process a swarm event
    pub fn process_event(
        &mut self,
        event: SwarmEvent<BothoBehaviourEvent>,
    ) -> Option<NetworkEvent> {
        match event {
            SwarmEvent::Behaviour(BothoBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                // Account for received bytes (#542) before any rate-limit drop:
                // the payload already crossed the wire regardless of whether we
                // act on it, so it counts toward `bytesReceived`.
                self.stats.record_received(message.data.len() as u64);

                // Determine which topic this message is from
                let topic = message.topic.as_str();

                // Per-peer rate limiting (DoS protection)
                // Check rate limit before processing message
                if let Some(peer) = message.source {
                    let msg_type = Self::topic_to_message_type(topic);
                    match self.rate_limiter.record_message_typed(&peer, msg_type) {
                        RateLimitResult::Allowed => {
                            // Message allowed, continue processing
                        }
                        RateLimitResult::RateLimited {
                            violations,
                            remaining,
                            message_type,
                        } => {
                            warn!(
                                %peer,
                                ?message_type,
                                violations,
                                remaining,
                                "Rate limited message from peer"
                            );
                            return None;
                        }
                        RateLimitResult::Disconnect => {
                            warn!(
                                %peer,
                                "Peer exceeded rate limit threshold, flagged for disconnection"
                            );
                            return None;
                        }
                    }
                }

                if topic == BLOCKS_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_BLOCK_SIZE {
                        warn!(
                            "Rejected oversized block message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_BLOCK_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as a block
                    match bincode::deserialize::<Block>(&message.data) {
                        Ok(block) => {
                            info!(
                                "Received block {} from network (hash: {})",
                                block.height(),
                                hex::encode(&block.hash()[0..8])
                            );
                            return Some(NetworkEvent::NewBlock(block));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize block from gossip: {}", e);
                        }
                    }
                } else if topic == TRANSACTIONS_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_TRANSACTION_SIZE {
                        warn!(
                            "Rejected oversized transaction message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_TRANSACTION_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as a transaction
                    match bincode::deserialize::<Transaction>(&message.data) {
                        Ok(tx) => {
                            debug!(
                                "Received transaction {} from network",
                                hex::encode(&tx.hash()[0..8])
                            );
                            return Some(NetworkEvent::NewTransaction(tx));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize transaction from gossip: {}", e);
                        }
                    }
                } else if topic == MINTING_TXS_TOPIC {
                    // Check size before deserialization (DoS protection).
                    // A minting tx is small; reuse the transaction size cap.
                    if message.data.len() > MAX_TRANSACTION_SIZE {
                        warn!(
                            "Rejected oversized minting tx message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_TRANSACTION_SIZE
                        );
                        return None;
                    }

                    match bincode::deserialize::<MintingTx>(&message.data) {
                        Ok(minting_tx) => {
                            debug!(
                                "Received minting tx {} from network",
                                hex::encode(&minting_tx.hash()[0..8])
                            );
                            return Some(NetworkEvent::NewMintingTx(minting_tx));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize minting tx from gossip: {}", e);
                        }
                    }
                } else if topic == SCP_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_SCP_MESSAGE_SIZE {
                        warn!(
                            "Rejected oversized SCP message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_SCP_MESSAGE_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as an SCP message
                    match bincode::deserialize::<ScpMessage>(&message.data) {
                        Ok(scp_msg) => {
                            debug!(
                                slot = scp_msg.slot_index,
                                "Received SCP message from network"
                            );
                            return Some(NetworkEvent::ScpMessage(scp_msg));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize SCP message from gossip: {}", e);
                        }
                    }
                } else if topic == COMPACT_BLOCKS_TOPIC {
                    // Compact block messages can be: CompactBlock, GetBlockTxn, or BlockTxn
                    // Size limit is same as full blocks
                    if message.data.len() > MAX_BLOCK_SIZE {
                        warn!(
                            "Rejected oversized compact block message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_BLOCK_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as CompactBlock first (most common)
                    if let Ok(compact_block) = bincode::deserialize::<CompactBlock>(&message.data) {
                        info!(
                            "Received compact block {} from network ({} txs, {} bytes)",
                            compact_block.height(),
                            compact_block.short_ids.len(),
                            message.data.len()
                        );
                        return Some(NetworkEvent::NewCompactBlock(compact_block));
                    }

                    // Try GetBlockTxn
                    if let Ok(request) = bincode::deserialize::<GetBlockTxn>(&message.data) {
                        debug!(
                            "Received GetBlockTxn for block {} ({} indices)",
                            hex::encode(&request.block_hash[0..8]),
                            request.indices.len()
                        );
                        let peer = message.source.unwrap_or(PeerId::random());
                        return Some(NetworkEvent::GetBlockTxn { peer, request });
                    }

                    // Try BlockTxn
                    if let Ok(response) = bincode::deserialize::<BlockTxn>(&message.data) {
                        debug!(
                            "Received BlockTxn for block {} ({} txs)",
                            hex::encode(&response.block_hash[0..8]),
                            response.txs.len()
                        );
                        return Some(NetworkEvent::BlockTxn(response));
                    }

                    warn!("Failed to deserialize compact block message");
                } else if topic == UPGRADE_ANNOUNCEMENTS_TOPIC {
                    // Upgrade announcement messages are relatively small
                    const MAX_UPGRADE_MESSAGE_SIZE: usize = 4096;
                    if message.data.len() > MAX_UPGRADE_MESSAGE_SIZE {
                        warn!(
                            "Rejected oversized upgrade announcement: {} bytes (max: {})",
                            message.data.len(),
                            MAX_UPGRADE_MESSAGE_SIZE
                        );
                        return None;
                    }

                    match bincode::deserialize::<UpgradeAnnouncement>(&message.data) {
                        Ok(announcement) => {
                            info!(
                                target_version = %announcement.target_version,
                                target_block_version = announcement.target_block_version,
                                is_hard_fork = announcement.is_hard_fork,
                                description = %announcement.description,
                                "Received upgrade announcement from network"
                            );
                            return Some(NetworkEvent::UpgradeAnnouncement(announcement));
                        }
                        Err(e) => {
                            warn!("Failed to deserialize upgrade announcement: {}", e);
                        }
                    }
                } else if topic == PEX_TOPIC {
                    // Check size before deserialization (DoS protection)
                    if message.data.len() > MAX_PEX_MESSAGE_SIZE {
                        warn!(
                            "Rejected oversized PEX message: {} bytes (max: {})",
                            message.data.len(),
                            MAX_PEX_MESSAGE_SIZE
                        );
                        return None;
                    }

                    // Try to deserialize as PEX message
                    match bincode::deserialize::<PexMessage>(&message.data) {
                        Ok(pex_msg) => {
                            let peer = message.source.unwrap_or(PeerId::random());
                            debug!(
                                %peer,
                                entries = pex_msg.entries.len(),
                                "Received PEX message"
                            );

                            // Process through PEX manager
                            let valid_addrs = self.pex_manager.process_incoming(&peer, &pex_msg);

                            if !valid_addrs.is_empty() {
                                return Some(NetworkEvent::PexAddresses(valid_addrs));
                            }
                        }
                        Err(e) => {
                            warn!("Failed to deserialize PEX message from gossip: {}", e);
                        }
                    }
                }

                None
            }

            // Track peers subscribing to compact blocks topic
            SwarmEvent::Behaviour(BothoBehaviourEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic },
            )) => {
                if topic.as_str() == COMPACT_BLOCKS_TOPIC {
                    self.compact_block_peers.insert(peer_id);
                    debug!(%peer_id, "Peer subscribed to compact blocks");
                }
                None
            }

            // Track peers unsubscribing from compact blocks topic
            SwarmEvent::Behaviour(BothoBehaviourEvent::Gossipsub(
                gossipsub::Event::Unsubscribed { peer_id, topic },
            )) => {
                if topic.as_str() == COMPACT_BLOCKS_TOPIC {
                    self.compact_block_peers.remove(&peer_id);
                    debug!(%peer_id, "Peer unsubscribed from compact blocks");
                }
                None
            }

            // Handle sync request-response events
            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::Message { peer, message, .. },
            )) => match message {
                request_response::Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    debug!(%peer, ?request, "Received sync request");
                    Some(NetworkEvent::SyncRequest {
                        peer,
                        request_id,
                        request,
                        channel,
                    })
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    debug!(%peer, "Received sync response");
                    Some(NetworkEvent::SyncResponse {
                        peer,
                        request_id,
                        response,
                    })
                }
            },

            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::OutboundFailure {
                    peer,
                    request_id,
                    error,
                    ..
                },
            )) => {
                warn!(%peer, ?request_id, %error, "Sync request failed");
                Some(NetworkEvent::SyncResponse {
                    peer,
                    request_id,
                    response: SyncResponse::Error(error.to_string()),
                })
            }

            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::InboundFailure {
                    peer,
                    request_id,
                    error,
                    ..
                },
            )) => {
                warn!(%peer, ?request_id, %error, "Inbound sync request failed");
                None
            }

            SwarmEvent::Behaviour(BothoBehaviourEvent::Sync(
                request_response::Event::ResponseSent { .. },
            )) => None,

            // Handle identify protocol events for version tracking
            SwarmEvent::Behaviour(BothoBehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                // Parse the agent_version to extract protocol version
                let peer_version = ProtocolVersion::parse(&info.agent_version);
                let min_version = ProtocolVersion::parse(MIN_SUPPORTED_PROTOCOL_VERSION);
                let local_version = ProtocolVersion::parse(PROTOCOL_VERSION);

                let version_warning = match (&peer_version, &min_version) {
                    (Some(pv), Some(mv)) => !pv.is_compatible_with(mv),
                    _ => false,
                };

                // Parse transport capabilities from agent_version
                let transport_caps = super::transport::TransportCapabilities::from_agent_version(
                    &info.agent_version,
                );

                // Update peer entry with version and transport information
                if let Some(entry) = self.peers.get_mut(&peer_id) {
                    entry.protocol_version = peer_version.clone();
                    entry.version_warning = version_warning;
                    entry.transport_capabilities = transport_caps.clone();
                    entry.last_seen = std::time::Instant::now();
                }

                // Consensus incompatibility (different major, or an
                // unparseable agent string): the caller must disconnect.
                // Major bumps mark consensus-breaking changes; this check is
                // what makes coordinated upgrades enforceable instead of
                // silently divergent.
                if let Some(lv) = &local_version {
                    if let Some(pv) = ProtocolVersion::consensus_incompatibility(&peer_version, lv)
                    {
                        warn!(
                            %peer_id,
                            peer_version = %pv,
                            local_version = %lv,
                            agent_version = %info.agent_version,
                            "Peer protocol version is consensus-incompatible; disconnecting"
                        );
                        self.peers.remove(&peer_id);
                        return Some(NetworkEvent::PeerVersionIncompatible {
                            peer: peer_id,
                            peer_version: pv,
                            local_version: lv.clone(),
                        });
                    }
                }

                if version_warning {
                    if let (Some(pv), Some(mv)) = (peer_version.clone(), min_version) {
                        warn!(
                            %peer_id,
                            peer_version = %pv,
                            min_version = %mv,
                            "Peer has outdated protocol version"
                        );
                        return Some(NetworkEvent::PeerVersionWarning {
                            peer: peer_id,
                            peer_version: pv,
                            min_version: mv,
                        });
                    }
                }

                if let Some(pv) = peer_version {
                    debug!(
                        %peer_id,
                        protocol_version = %pv,
                        agent_version = %info.agent_version,
                        has_transport_caps = transport_caps.is_some(),
                        "Identified peer version"
                    );
                }

                None
            }

            SwarmEvent::Behaviour(BothoBehaviourEvent::Identify(
                identify::Event::Sent { .. }
                | identify::Event::Pushed { .. }
                | identify::Event::Error { .. },
            )) => None,

            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
                None
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                endpoint,
                num_established,
                ..
            } => {
                info!("Connected to peer: {}", peer_id);
                // Count the connection direction exactly once per peer (#542):
                // libp2p can briefly hold multiple connections to the same peer
                // (concurrent dials), so only the FIRST established connection
                // (num_established == 1) bumps the inbound/outbound counter, to
                // mirror the per-peer accounting used for `peer_count`. A
                // connection we dialed is outbound; one a remote dialed is
                // inbound.
                if num_established.get() == 1 {
                    self.stats.record_connection_opened(!endpoint.is_dialer());
                }
                self.peers.insert(
                    peer_id,
                    PeerTableEntry {
                        peer_id,
                        address: None,
                        last_seen: std::time::Instant::now(),
                        protocol_version: None, // Will be set when identify completes
                        version_warning: false,
                        transport_capabilities: None, // Will be set when identify completes
                    },
                );
                Some(NetworkEvent::PeerDiscovered(peer_id))
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                endpoint,
                num_established,
                ..
            } => {
                // Mirror the per-peer connection-direction accounting from
                // ConnectionEstablished (#542): only decrement once the LAST
                // connection to this peer is gone (num_established == 0), using
                // the same dialer/listener split.
                if num_established == 0 {
                    self.stats.record_connection_closed(!endpoint.is_dialer());
                }
                // libp2p emits ConnectionClosed per-connection. When two nodes
                // dial each other concurrently they briefly hold redundant
                // connections; closing one must NOT be treated as a full
                // disconnect, or the peer is dropped from `self.peers` while
                // still connected — which collapses the SCP quorum back below
                // threshold and stalls consensus (issue #409). Only report the
                // peer as gone once no connections remain.
                if num_established > 0 {
                    debug!(
                        "Redundant connection to {} closed ({} still established)",
                        peer_id, num_established
                    );
                    return None;
                }

                info!("Disconnected from peer: {}", peer_id);
                self.peers.remove(&peer_id);
                self.compact_block_peers.remove(&peer_id);
                // Clean up rate limiter state for disconnected peer
                self.rate_limiter.remove_peer(&peer_id);
                Some(NetworkEvent::PeerDisconnected(peer_id))
            }
            _ => None,
        }
    }

    /// Send a sync request to a peer
    pub fn send_sync_request(
        swarm: &mut Swarm<BothoBehaviour>,
        peer: PeerId,
        request: SyncRequest,
    ) -> OutboundRequestId {
        swarm.behaviour_mut().sync.send_request(&peer, request)
    }

    /// Send a sync response
    pub fn send_sync_response(
        swarm: &mut Swarm<BothoBehaviour>,
        channel: ResponseChannel<SyncResponse>,
        response: SyncResponse,
    ) -> Result<(), SyncResponse> {
        swarm.behaviour_mut().sync.send_response(channel, response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // NetworkStats tests (#542)
    // ========================================================================

    #[test]
    fn test_network_stats_default_is_zero() {
        let stats = NetworkStats::new();
        assert_eq!(stats.bytes_sent(), 0);
        assert_eq!(stats.bytes_received(), 0);
        assert_eq!(stats.inbound_count(), 0);
        assert_eq!(stats.outbound_count(), 0);
    }

    #[test]
    fn test_network_stats_byte_counters_accumulate() {
        // Simulates the send/receive sites incrementing the counters: each call
        // mirrors a serialized publish or a received gossipsub payload.
        let stats = NetworkStats::new();

        stats.record_sent(100);
        stats.record_sent(250);
        assert_eq!(stats.bytes_sent(), 350);

        stats.record_received(40);
        stats.record_received(60);
        stats.record_received(1);
        assert_eq!(stats.bytes_received(), 101);

        // Sent and received are independent.
        assert_eq!(stats.bytes_sent(), 350);
    }

    #[test]
    fn test_network_stats_inbound_outbound_independent() {
        let stats = NetworkStats::new();

        // Two inbound (remote-dialed) and one outbound (locally-dialed).
        stats.record_connection_opened(true);
        stats.record_connection_opened(true);
        stats.record_connection_opened(false);
        assert_eq!(stats.inbound_count(), 2);
        assert_eq!(stats.outbound_count(), 1);

        // Closing decrements the matching direction only.
        stats.record_connection_closed(true);
        assert_eq!(stats.inbound_count(), 1);
        assert_eq!(stats.outbound_count(), 1);

        stats.record_connection_closed(false);
        assert_eq!(stats.inbound_count(), 1);
        assert_eq!(stats.outbound_count(), 0);
    }

    #[test]
    fn test_network_stats_close_saturates_at_zero() {
        // A spurious close (more closes than opens) must never underflow the
        // u64 counter into a huge value.
        let stats = NetworkStats::new();
        stats.record_connection_closed(true);
        stats.record_connection_closed(false);
        assert_eq!(stats.inbound_count(), 0);
        assert_eq!(stats.outbound_count(), 0);
    }

    #[test]
    fn test_network_stats_shared_handle_observes_same_atomics() {
        // The RPC layer reads from a cloned Arc; both handles must observe the
        // same underlying counters (this is the live snapshot contract).
        let stats = Arc::new(NetworkStats::new());
        let rpc_view = Arc::clone(&stats);

        stats.record_sent(512);
        stats.record_received(128);
        stats.record_connection_opened(true);

        assert_eq!(rpc_view.bytes_sent(), 512);
        assert_eq!(rpc_view.bytes_received(), 128);
        assert_eq!(rpc_view.inbound_count(), 1);
        assert_eq!(rpc_view.outbound_count(), 0);
    }

    #[test]
    fn test_connected_point_direction_maps_to_counter() {
        // Verifies the exact direction predicate used in `process_event`:
        // a Dialer endpoint (we dialed) is outbound; a Listener endpoint
        // (remote dialed us) is inbound. This guards against the inbound/
        // outbound mapping silently inverting.
        use libp2p::core::{transport::PortUse, ConnectedPoint, Endpoint};

        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/9000".parse().unwrap();

        let dialer = ConnectedPoint::Dialer {
            address: addr.clone(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        };
        let listener = ConnectedPoint::Listener {
            local_addr: addr.clone(),
            send_back_addr: addr.clone(),
        };

        // `process_event` records `!endpoint.is_dialer()` as the `inbound` flag.
        let stats = NetworkStats::new();
        stats.record_connection_opened(!dialer.is_dialer()); // outbound
        stats.record_connection_opened(!listener.is_dialer()); // inbound

        assert_eq!(stats.outbound_count(), 1, "dialer must count as outbound");
        assert_eq!(stats.inbound_count(), 1, "listener must count as inbound");
    }

    #[test]
    fn test_discovery_exposes_shared_stats_handle() {
        // `discovery.stats()` and `discovery.stats_ref()` must reference the
        // same counters the event loop mutates, so the RPC handle stays live.
        let discovery = NetworkDiscovery::new(0, vec![]);
        let handle = discovery.stats();

        discovery.stats_ref().record_sent(64);
        assert_eq!(handle.bytes_sent(), 64);
    }

    // ========================================================================
    // PeerTableEntry tests
    // ========================================================================

    #[test]
    fn test_peer_table_entry_creation() {
        let peer_id = PeerId::random();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
            protocol_version: None,
            version_warning: false,
            transport_capabilities: None,
        };

        assert_eq!(entry.peer_id, peer_id);
        assert!(entry.address.is_none());
        assert!(entry.protocol_version.is_none());
        assert!(!entry.version_warning);
        assert!(entry.transport_capabilities.is_none());
    }

    #[test]
    fn test_peer_table_entry_with_address() {
        let peer_id = PeerId::random();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/9000".parse().unwrap();
        let entry = PeerTableEntry {
            peer_id,
            address: Some(addr.clone()),
            last_seen: std::time::Instant::now(),
            protocol_version: None,
            version_warning: false,
            transport_capabilities: None,
        };

        assert_eq!(entry.address, Some(addr));
    }

    #[test]
    fn test_peer_table_entry_with_version() {
        let peer_id = PeerId::random();
        let version = ProtocolVersion::parse("botho/1.0.0/5").unwrap();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
            protocol_version: Some(version.clone()),
            version_warning: false,
            transport_capabilities: None,
        };

        assert_eq!(entry.protocol_version, Some(version));
    }

    #[test]
    fn test_peer_table_entry_clone() {
        let peer_id = PeerId::random();
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
            protocol_version: None,
            version_warning: false,
            transport_capabilities: None,
        };

        let cloned = entry.clone();
        assert_eq!(cloned.peer_id, entry.peer_id);
    }

    #[test]
    fn test_peer_table_entry_with_transport_capabilities() {
        use super::super::transport::{
            CapabilityTransportType, NegotiationNatType, TransportCapabilities,
        };

        let peer_id = PeerId::random();
        let caps = TransportCapabilities::new(
            vec![
                CapabilityTransportType::WebRTC,
                CapabilityTransportType::Plain,
            ],
            CapabilityTransportType::WebRTC,
            NegotiationNatType::Open,
        );
        let entry = PeerTableEntry {
            peer_id,
            address: None,
            last_seen: std::time::Instant::now(),
            protocol_version: None,
            version_warning: false,
            transport_capabilities: Some(caps.clone()),
        };

        assert!(entry.transport_capabilities.is_some());
        let stored_caps = entry.transport_capabilities.unwrap();
        assert!(stored_caps.supports(CapabilityTransportType::WebRTC));
        assert_eq!(stored_caps.nat_type, NegotiationNatType::Open);
    }

    // ========================================================================
    // ProtocolVersion tests
    // ========================================================================

    #[test]
    fn test_protocol_version_parse_simple() {
        let version = ProtocolVersion::parse("1.0.0").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 0);
        assert!(version.block_version.is_none());
    }

    #[test]
    fn test_protocol_version_parse_agent_string() {
        let version = ProtocolVersion::parse("botho/1.2.3/5").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 2);
        assert_eq!(version.patch, 3);
        assert_eq!(version.block_version, Some(5));
    }

    #[test]
    fn test_protocol_version_parse_agent_without_block_version() {
        let version = ProtocolVersion::parse("botho/1.0.0").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 0);
        assert!(version.block_version.is_none());
    }

    #[test]
    fn test_protocol_version_parse_invalid() {
        assert!(ProtocolVersion::parse("invalid").is_none());
        assert!(ProtocolVersion::parse("1.0").is_none());
        assert!(ProtocolVersion::parse("").is_none());
    }

    #[test]
    fn test_protocol_version_is_compatible() {
        let v1_0_0 = ProtocolVersion::parse("1.0.0").unwrap();
        let v1_0_1 = ProtocolVersion::parse("1.0.1").unwrap();
        let v1_1_0 = ProtocolVersion::parse("1.1.0").unwrap();
        let v2_0_0 = ProtocolVersion::parse("2.0.0").unwrap();

        // Same version is compatible
        assert!(v1_0_0.is_compatible_with(&v1_0_0));

        // Higher patch is compatible with lower
        assert!(v1_0_1.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v1_0_1));

        // Higher minor is compatible with lower
        assert!(v1_1_0.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v1_1_0));

        // Different major is not compatible
        assert!(!v2_0_0.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v2_0_0));
    }

    #[test]
    fn test_protocol_version_consensus_compatibility() {
        let v1_0_0 = ProtocolVersion::parse("1.0.0").unwrap();
        let v1_9_9 = ProtocolVersion::parse("1.9.9").unwrap();
        let v2_0_0 = ProtocolVersion::parse("2.0.0").unwrap();

        // Same major: consensus-compatible in BOTH directions (an old node
        // warns about a newer minor but is not disconnected — soft forks
        // upgrade gracefully)
        assert!(v1_0_0.is_consensus_compatible(&v1_9_9));
        assert!(v1_9_9.is_consensus_compatible(&v1_0_0));

        // Different major: incompatible both ways (peer must be
        // disconnected — this enforces coordinated upgrades)
        assert!(!v1_0_0.is_consensus_compatible(&v2_0_0));
        assert!(!v2_0_0.is_consensus_compatible(&v1_0_0));
    }

    #[test]
    fn test_consensus_incompatibility_decision() {
        let local = ProtocolVersion::parse("1.2.3").unwrap();

        // Same major: compatible, no disconnect
        let same_major = ProtocolVersion::parse("1.0.0").unwrap();
        assert_eq!(
            ProtocolVersion::consensus_incompatibility(&Some(same_major), &local),
            None
        );

        // Different major: disconnect, reporting the peer's version
        let other_major = ProtocolVersion::parse("2.0.0").unwrap();
        let reported =
            ProtocolVersion::consensus_incompatibility(&Some(other_major.clone()), &local);
        assert_eq!(reported, Some(other_major));

        // Unparseable agent string fails closed: disconnect with 0.0.0 sentinel
        let garbage = ProtocolVersion::parse("not-a-version");
        assert_eq!(garbage, None);
        let reported = ProtocolVersion::consensus_incompatibility(&garbage, &local);
        assert_eq!(
            reported,
            Some(ProtocolVersion {
                major: 0,
                minor: 0,
                patch: 0,
                block_version: None
            })
        );
    }

    #[test]
    fn test_protocol_version_to_agent_string() {
        let version = ProtocolVersion::parse("1.0.0").unwrap();
        let agent = version.to_agent_string(5);
        assert_eq!(agent, "botho/1.0.0/5");
    }

    #[test]
    fn test_protocol_version_display() {
        let version = ProtocolVersion::parse("botho/1.2.3/5").unwrap();
        let display = format!("{}", version);
        assert_eq!(display, "1.2.3 (block v5)");

        let version_no_block = ProtocolVersion::parse("1.2.3").unwrap();
        let display_no_block = format!("{}", version_no_block);
        assert_eq!(display_no_block, "1.2.3");
    }

    // ========================================================================
    // UpgradeAnnouncement tests
    // ========================================================================

    #[test]
    fn test_upgrade_announcement_serialization() {
        let announcement = UpgradeAnnouncement {
            target_version: "1.1.0".to_string(),
            target_block_version: 6,
            activation_height: Some(100000),
            activation_timestamp: None,
            description: "Test upgrade".to_string(),
            is_hard_fork: false,
            min_version_after: "1.1.0".to_string(),
        };

        let serialized = bincode::serialize(&announcement).unwrap();
        let deserialized: UpgradeAnnouncement = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.target_version, "1.1.0");
        assert_eq!(deserialized.target_block_version, 6);
        assert_eq!(deserialized.activation_height, Some(100000));
        assert!(deserialized.activation_timestamp.is_none());
        assert!(!deserialized.is_hard_fork);
    }

    #[test]
    fn test_upgrade_announcement_hard_fork() {
        let announcement = UpgradeAnnouncement {
            target_version: "2.0.0".to_string(),
            target_block_version: 7,
            activation_height: None,
            activation_timestamp: Some(1700000000),
            description: "Major protocol upgrade".to_string(),
            is_hard_fork: true,
            min_version_after: "2.0.0".to_string(),
        };

        assert!(announcement.is_hard_fork);
        assert_eq!(announcement.activation_timestamp, Some(1700000000));
    }

    // ========================================================================
    // NetworkDiscovery tests
    // ========================================================================

    #[test]
    fn test_network_discovery_new() {
        let discovery = NetworkDiscovery::new(9000, vec![]);

        assert_eq!(discovery.peer_count(), 0);
        assert!(discovery.peer_table().is_empty());
    }

    #[test]
    fn test_network_discovery_with_bootstrap_peers() {
        let bootstrap = vec![
            "/ip4/192.168.1.1/tcp/9000".to_string(),
            "/ip4/192.168.1.2/tcp/9000".to_string(),
        ];
        let discovery = NetworkDiscovery::new(9001, bootstrap);

        // Bootstrap peers are stored but not yet connected
        assert_eq!(discovery.peer_count(), 0);
    }

    #[test]
    fn test_network_discovery_local_peer_id() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        let peer_id = discovery.local_peer_id();

        // Should be a valid peer ID
        assert!(!peer_id.to_string().is_empty());
    }

    #[test]
    fn test_network_discovery_take_event_receiver_once() {
        let mut discovery = NetworkDiscovery::new(9000, vec![]);

        // First take should succeed
        let rx1 = discovery.take_event_receiver();
        assert!(rx1.is_some());

        // Second take should return None
        let rx2 = discovery.take_event_receiver();
        assert!(rx2.is_none());
    }

    #[test]
    fn test_network_discovery_peer_table_empty() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        let table = discovery.peer_table();

        assert!(table.is_empty());
        assert_eq!(discovery.peer_count(), 0);
    }

    // ========================================================================
    // NetworkEvent tests
    // ========================================================================

    #[test]
    fn test_network_event_peer_discovered_debug() {
        let peer_id = PeerId::random();
        let event = NetworkEvent::PeerDiscovered(peer_id);

        // Should implement Debug
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PeerDiscovered"));
    }

    #[test]
    fn test_network_event_peer_disconnected_debug() {
        let peer_id = PeerId::random();
        let event = NetworkEvent::PeerDisconnected(peer_id);

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PeerDisconnected"));
    }

    // ========================================================================
    // Topic constant tests
    // ========================================================================

    #[test]
    fn test_topic_constants_are_valid() {
        assert!(!BLOCKS_TOPIC.is_empty());
        assert!(!TRANSACTIONS_TOPIC.is_empty());
        assert!(!SCP_TOPIC.is_empty());
        assert!(!UPGRADE_ANNOUNCEMENTS_TOPIC.is_empty());

        // Topics should follow naming convention
        assert!(BLOCKS_TOPIC.starts_with("botho/"));
        assert!(TRANSACTIONS_TOPIC.starts_with("botho/"));
        assert!(SCP_TOPIC.starts_with("botho/"));
        assert!(UPGRADE_ANNOUNCEMENTS_TOPIC.starts_with("botho/"));
    }

    #[test]
    fn test_topics_are_versioned() {
        assert!(BLOCKS_TOPIC.contains("/1.0.0"));
        assert!(TRANSACTIONS_TOPIC.contains("/1.0.0"));
        assert!(SCP_TOPIC.contains("/1.0.0"));
        assert!(UPGRADE_ANNOUNCEMENTS_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_topics_are_unique() {
        assert_ne!(BLOCKS_TOPIC, TRANSACTIONS_TOPIC);
        assert_ne!(BLOCKS_TOPIC, SCP_TOPIC);
        assert_ne!(TRANSACTIONS_TOPIC, SCP_TOPIC);
        assert_ne!(UPGRADE_ANNOUNCEMENTS_TOPIC, BLOCKS_TOPIC);
        assert_ne!(UPGRADE_ANNOUNCEMENTS_TOPIC, TRANSACTIONS_TOPIC);
        assert_ne!(UPGRADE_ANNOUNCEMENTS_TOPIC, SCP_TOPIC);
    }

    #[test]
    fn test_upgrade_announcements_topic() {
        assert_eq!(UPGRADE_ANNOUNCEMENTS_TOPIC, "botho/upgrades/1.0.0");
    }

    // ========================================================================
    // Protocol version constant tests
    // ========================================================================

    #[test]
    fn test_protocol_version_constant() {
        // Bumped to 4.0.0 for the coordinated reset deploying the #626
        // consensus changes (log-domain fee curve replacing the C7 fee
        // floor; u128 cluster wealth, #627/#628/#629) and the ratified #605
        // cluster-wealth decay semantics. #626 changes the fee floor a block
        // must satisfy to be accepted, so 3.0.0 peers are consensus-
        // incompatible with the reset chain and a fresh genesis is required.
        // A MAJOR bump is required because `is_consensus_compatible` (the
        // peer-disconnect gate) compares majors only — a minor bump would
        // merely warn, leaving 3.0.0 peers connected and silently forking.
        //
        // Bumped to 4.1.0 (MINOR) for the #694 nanoBTH -> picocredits
        // migration: the RPC contract's declared units change but no
        // consensus rule does, so 4.0.x peers stay connected (warn-only).
        //
        // Bumped to 5.0.0 (MAJOR) for ADR 0007 bridge-import cluster tagging +
        // the >=F import floor (#938): a consensus-breaking fee-floor rule.
        // MAJOR (not minor) because `is_consensus_compatible` is major-only, so
        // a minor bump would leave 4.x peers connected and silently forking.
        //
        // Bumped to 6.0.0 (MAJOR) for the #925 downgrade charge: the consensus
        // fee floor now prices a demurrage class downgrade at capitalized future
        // demurrage, another consensus-breaking fee-floor rule. MAJOR so 5.x
        // peers (accrued-only) are disconnected rather than left forking.
        assert_eq!(PROTOCOL_VERSION, "6.0.0");
        let parsed = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        assert_eq!(parsed.major, 6);
        assert_eq!(parsed.minor, 0);
        assert_eq!(parsed.patch, 0);
    }

    /// #925 is a consensus-breaking MAJOR bump (5.x → 6.0.0): 5.x peers price
    /// only accrued-to-date demurrage and would silently fork the chain that
    /// now charges the capitalized downgrade reset, so they must be
    /// DISCONNECTED.
    #[test]
    fn test_v5_peers_are_consensus_incompatible_after_issue_925() {
        let local = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        let v5_peer = ProtocolVersion::parse("5.0.0").unwrap();
        assert!(!v5_peer.is_consensus_compatible(&local));
        assert_eq!(
            ProtocolVersion::consensus_incompatibility(&Some(v5_peer.clone()), &local),
            Some(v5_peer)
        );
    }

    /// ADR 0007 (#938) is a consensus-breaking MAJOR bump (4.x → 5.0.0), so 4.x
    /// peers must now be DISCONNECTED as consensus-incompatible — they apply no
    /// import floor and would silently fork the import-tagged/floored chain.
    #[test]
    fn test_v4_peers_are_consensus_incompatible_after_adr0007() {
        let local = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        let v4_peer = ProtocolVersion::parse("4.1.0").unwrap();
        assert!(!v4_peer.is_consensus_compatible(&local));
        assert_eq!(
            ProtocolVersion::consensus_incompatibility(&Some(v4_peer.clone()), &local),
            Some(v4_peer)
        );
    }

    #[test]
    fn test_min_supported_protocol_version_constant() {
        assert_eq!(MIN_SUPPORTED_PROTOCOL_VERSION, "6.0.0");
        let parsed = ProtocolVersion::parse(MIN_SUPPORTED_PROTOCOL_VERSION).unwrap();
        assert_eq!(parsed.major, 6);
        assert_eq!(parsed.minor, 0);
    }

    /// Regression guard (originally #606): a consensus-breaking reset must
    /// actually DISCONNECT the immediately-preceding major's peers, not just
    /// warn. Pins the major-only disconnect semantics for the 2.0.0 pre-reset
    /// chain against the live constants.
    #[test]
    fn test_v2_peers_are_consensus_incompatible_with_current() {
        let local = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        let old_peer = ProtocolVersion::parse("2.0.0").unwrap();
        assert!(!old_peer.is_consensus_compatible(&local));
        assert_eq!(
            ProtocolVersion::consensus_incompatibility(&Some(old_peer.clone()), &local),
            Some(old_peer)
        );
    }

    /// Regression guard for the #605/#626 reset: the 4.0.0 deploy must
    /// actually DISCONNECT 3.0.0 peers (the H1 fee-floor chain), not merely
    /// warn — #626's log-domain fee curve changes the C7 floor, so a 3.0.0
    /// peer would fork. Pins the major-only disconnect semantics against the
    /// live constants so a future accidental minor-only bump of a consensus-
    /// breaking change fails the suite.
    #[test]
    fn test_v3_peers_are_consensus_incompatible_with_current() {
        let local = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        let old_peer = ProtocolVersion::parse("3.0.0").unwrap();
        assert!(!old_peer.is_consensus_compatible(&local));
        assert_eq!(
            ProtocolVersion::consensus_incompatibility(&Some(old_peer.clone()), &local),
            Some(old_peer)
        );
    }

    #[test]
    fn test_current_version_compatible_with_min() {
        let current = ProtocolVersion::parse(PROTOCOL_VERSION).unwrap();
        let min = ProtocolVersion::parse(MIN_SUPPORTED_PROTOCOL_VERSION).unwrap();
        assert!(current.is_compatible_with(&min));
    }

    // ========================================================================
    // Compact block subscription tracking tests
    // ========================================================================

    #[test]
    fn test_compact_blocks_topic_constant() {
        assert_eq!(COMPACT_BLOCKS_TOPIC, "botho/compact-blocks/1.0.0");
        assert!(COMPACT_BLOCKS_TOPIC.starts_with("botho/"));
        assert!(COMPACT_BLOCKS_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_compact_block_peers_initially_empty() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        assert_eq!(discovery.legacy_peer_count(), 0);
        assert!(discovery.all_peers_support_compact_blocks());
    }

    #[test]
    fn test_peer_supports_compact_blocks_false_for_unknown() {
        let discovery = NetworkDiscovery::new(9000, vec![]);
        let peer_id = PeerId::random();

        assert!(!discovery.peer_supports_compact_blocks(&peer_id));
    }

    #[test]
    fn test_legacy_peer_count_with_no_peers() {
        let discovery = NetworkDiscovery::new(9000, vec![]);

        // No peers = no legacy peers
        assert_eq!(discovery.legacy_peer_count(), 0);
        assert!(discovery.all_peers_support_compact_blocks());
    }

    // ========================================================================
    // PEX integration tests
    // ========================================================================

    #[test]
    fn test_pex_topic_constant() {
        assert_eq!(PEX_TOPIC, "botho/pex/1.0.0");
        assert!(PEX_TOPIC.starts_with("botho/"));
        assert!(PEX_TOPIC.contains("/1.0.0"));
    }

    #[test]
    fn test_network_discovery_has_pex_manager() {
        let discovery = NetworkDiscovery::new(9000, vec![]);

        // PEX manager should be initialized
        assert!(discovery.pex_manager().should_broadcast());
    }

    #[test]
    fn test_pex_manager_access() {
        let mut discovery = NetworkDiscovery::new(9000, vec![]);

        // Should be able to access PEX manager mutably
        discovery.pex_manager_mut().record_broadcast();
        assert!(!discovery.pex_manager().should_broadcast());
    }

    #[test]
    fn test_record_peer_source() {
        let mut discovery = NetworkDiscovery::new(9000, vec![]);
        let peer = PeerId::random();
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();

        discovery.record_peer_source(peer, &addr, PeerSource::Bootstrap);

        assert_eq!(
            discovery.pex_manager().source_tracker.get_source(&peer),
            Some(PeerSource::Bootstrap)
        );
    }

    #[test]
    fn test_network_event_pex_addresses() {
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();
        let event = NetworkEvent::PexAddresses(vec![addr.clone()]);

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PexAddresses"));
    }
}
