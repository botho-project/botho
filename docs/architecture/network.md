# Network Architecture

This document describes Botho's P2P networking, peer discovery, and DDoS protections for external security auditors.

## Table of Contents

1. [P2P Protocol Overview](#p2p-protocol-overview)
2. [Gossip Topics](#gossip-topics)
3. [Peer Discovery](#peer-discovery)
4. [Message Types](#message-types)
5. [Sync Protocol](#sync-protocol)
6. [DDoS Protections](#ddos-protections)
7. [Compact Block Protocol](#compact-block-protocol)

---

## P2P Protocol Overview

Botho uses **libp2p** with **gossipsub** for peer-to-peer communication.

### Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           NETWORK LAYER                                      │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                          libp2p Swarm                                 │  │
│  │                                                                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  │  │
│  │  │  Transport  │  │  Gossipsub  │  │   Kademlia  │  │   Identify  │  │  │
│  │  │  (TCP/QUIC) │  │  (Pubsub)   │  │   (DHT)     │  │  Protocol   │  │  │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  │  │
│  │         │                │                │                │         │  │
│  │         └────────────────┴────────────────┴────────────────┘         │  │
│  │                                    │                                  │  │
│  └────────────────────────────────────┼──────────────────────────────────┘  │
│                                       │                                     │
│                                       ▼                                     │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                      BothoBehaviour                                   │  │
│  │                                                                       │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                │  │
│  │  │  Connection  │  │  Peer        │  │  Rate        │                │  │
│  │  │  Limiter     │  │  Reputation  │  │  Limiter     │                │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘                │  │
│  │                                                                       │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Components

| Component | Purpose | Location |
|-----------|---------|----------|
| `NetworkDiscovery` | Manages peer connections and discovery | `network/discovery.rs` |
| `BothoBehaviour` | Custom libp2p NetworkBehaviour | `network/mod.rs` |
| `ConnectionLimiter` | Limits connections per IP | `network/connection_limiter.rs` |
| `SyncRateLimiter` | Rate limits sync requests | `network/sync.rs` |

**Code Reference:** `botho/src/network/`

---

## Gossip Topics

All network communication uses gossipsub topics with version namespacing.

### Topic Definitions

| Topic | Version | Purpose | Message Type |
|-------|---------|---------|--------------|
| `botho/blocks/1.0.0` | 1.0.0 | Block announcements | `BlockAnnouncement` |
| `botho/transactions/1.0.0` | 1.0.0 | Transaction broadcasts | `Transaction` |
| `botho/scp/1.0.0` | 1.0.0 | SCP consensus messages | `ScpMessage` |
| `botho/compact-blocks/1.0.0` | 1.0.0 | Bandwidth-efficient block relay | `CompactBlock` |

### Message Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        GOSSIP MESSAGE FLOW                                   │
│                                                                              │
│  Transaction Created                                                         │
│         │                                                                    │
│         ▼                                                                    │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐                │
│  │   Validate   │────▶│   Gossip to  │────▶│   Peers      │                │
│  │   Locally    │     │   Topic      │     │   Receive    │                │
│  └──────────────┘     └──────────────┘     └──────┬───────┘                │
│                                                    │                        │
│                                                    ▼                        │
│                                           ┌──────────────┐                 │
│                                           │   Validate   │                 │
│                                           │   & Forward  │                 │
│                                           └──────────────┘                 │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Gossipsub Configuration

```rust
GossipsubConfig {
    mesh_n: 6,           // Target mesh size
    mesh_n_low: 4,       // Minimum mesh size
    mesh_n_high: 12,     // Maximum mesh size
    gossip_lazy: 6,      // Peers to gossip to
    heartbeat_interval: 1s,
    history_length: 5,
    history_gossip: 3,
}
```

---

## Peer Discovery

### Discovery Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         PEER DISCOVERY                                       │
│                                                                              │
│  1. Bootstrap                                                                │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Node starts → Dial bootstrap peers → Join gossip mesh                 │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  2. DHT Discovery                                                            │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Query Kademlia → Find peers near our ID → Dial discovered peers       │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  3. Gossip Discovery                                                         │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Receive gossip → Learn new peer addresses → Add to peer table         │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  4. Maintenance                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Periodic ping → Update last_seen → Evict stale peers                  │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Peer Table

```rust
struct PeerTableEntry {
    peer_id: PeerId,
    address: Multiaddr,
    last_seen: Timestamp,
    reputation: PeerReputation,
}
```

### Peer Reputation

```rust
struct PeerReputation {
    success_count: u64,
    failure_count: u64,
    latency_ema: f64,  // Exponential moving average
}

// Peers with <25% success rate are banned
fn is_banned(&self) -> bool {
    let total = self.success_count + self.failure_count;
    total > 10 && (self.success_count as f64 / total as f64) < 0.25
}
```

**Code Reference:** `botho/src/network/discovery.rs`

---

## Message Types

### Network Events

```rust
enum NetworkEvent {
    // Block-related
    BlockReceived { block: Block, from: PeerId },
    BlockAnnouncement { hash: Hash, height: u64, from: PeerId },

    // Transaction-related
    TransactionReceived { tx: Transaction, from: PeerId },

    // SCP-related
    ScpMessageReceived { msg: ScpMessage, from: PeerId },

    // Sync-related
    SyncRequest { request: SyncRequest, from: PeerId },
    SyncResponse { response: SyncResponse, from: PeerId },

    // Peer-related
    PeerConnected { peer_id: PeerId, address: Multiaddr },
    PeerDisconnected { peer_id: PeerId },
}
```

### Message Validation

All messages are validated before processing:

| Message Type | Validation |
|--------------|------------|
| Block | PoW, height, previous hash, signatures |
| Transaction | Signatures, range proofs, key images |
| SCP | Signature, slot range, ballot ordering |
| Sync | Request size, rate limiting |

---

## Sync Protocol

### Protocol Overview

The sync protocol allows nodes to catch up on missed blocks.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          SYNC PROTOCOL                                       │
│                                                                              │
│  Behind Node                           Ahead Node                           │
│  ────────────                          ──────────                           │
│       │                                     │                               │
│       │  SyncRequest(from_height, count)    │                               │
│       │────────────────────────────────────▶│                               │
│       │                                     │                               │
│       │  SyncResponse(blocks[])             │                               │
│       │◀────────────────────────────────────│                               │
│       │                                     │                               │
│       │  [Validate & Store Blocks]          │                               │
│       │                                     │                               │
│       │  SyncRequest(next_height, count)    │                               │
│       │────────────────────────────────────▶│                               │
│       │                                     │                               │
│                         ...                                                 │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Sync Messages

```rust
struct SyncRequest {
    from_height: u64,
    count: u32,  // Max blocks to request
}

struct SyncResponse {
    blocks: Vec<Block>,
}
```

### Chain Sync Manager

```rust
struct ChainSyncManager {
    current_height: u64,
    target_height: u64,
    peers_by_height: BTreeMap<u64, Vec<PeerId>>,
    pending_requests: HashMap<PeerId, SyncRequest>,
}
```

**Code Reference:** `botho/src/network/sync.rs`

---

## DDoS Protections

### Rate Limiting

| Limiter | Scope | Limits |
|---------|-------|--------|
| `SyncRateLimiter` | Per peer | `MAX_REQUESTS_PER_MINUTE` |
| Request size | Per request | `MAX_REQUEST_SIZE` bytes |
| Response size | Per response | `MAX_RESPONSE_SIZE` bytes |
| Connection | Per IP | `DEFAULT_MAX_CONNECTIONS_PER_IP` |

### Rate Limiter Implementation

```rust
struct SyncRateLimiter {
    requests: HashMap<PeerId, VecDeque<Timestamp>>,
    max_requests_per_minute: u32,
}

impl SyncRateLimiter {
    fn check(&mut self, peer: PeerId) -> bool {
        let now = Timestamp::now();
        let minute_ago = now - Duration::from_secs(60);

        let requests = self.requests.entry(peer).or_default();

        // Remove old requests
        while requests.front().map(|t| *t < minute_ago).unwrap_or(false) {
            requests.pop_front();
        }

        // Check limit
        if requests.len() >= self.max_requests_per_minute as usize {
            return false;
        }

        requests.push_back(now);
        true
    }
}
```

### Connection Limiting

```rust
struct ConnectionLimiter {
    connections_per_ip: HashMap<IpAddr, u32>,
    max_per_ip: u32,  // DEFAULT_MAX_CONNECTIONS_PER_IP
}

impl ConnectionLimiter {
    fn allow_connection(&mut self, ip: IpAddr) -> bool {
        let count = self.connections_per_ip.entry(ip).or_insert(0);
        if *count >= self.max_per_ip {
            return false;
        }
        *count += 1;
        true
    }
}
```

### Attack Mitigations

| Attack | Mitigation |
|--------|------------|
| **Flood** | Rate limiting per peer |
| **Eclipse** | Peer diversity requirements, reputation tracking |
| **Sybil** | Connection limits per IP, proof-of-work for resources |
| **Amplification** | Response size limits, request validation |

**Code Reference:** `botho/src/network/connection_limiter.rs`

---

## Compact Block Protocol

### Overview

Compact blocks reduce bandwidth for block propagation using short transaction IDs.

### Protocol Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                       COMPACT BLOCK PROTOCOL                                 │
│                                                                              │
│  Sender                                Receiver                             │
│  ──────                                ────────                             │
│     │                                      │                                │
│     │  CompactBlock(header, short_ids[])   │                                │
│     │─────────────────────────────────────▶│                                │
│     │                                      │                                │
│     │                                      │  [Check mempool for matches]   │
│     │                                      │                                │
│     │  GetBlockTxn(missing_indices[])      │                                │
│     │◀─────────────────────────────────────│  (if some missing)             │
│     │                                      │                                │
│     │  BlockTxn(transactions[])            │                                │
│     │─────────────────────────────────────▶│                                │
│     │                                      │                                │
│     │                                      │  [Reconstruct full block]      │
│     │                                      │                                │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Compact Block Structure

```rust
struct CompactBlock {
    header: BlockHeader,
    nonce: u64,  // For short ID computation
    short_ids: Vec<ShortId>,  // 6-byte transaction IDs
    prefilled_txs: Vec<(u16, Transaction)>,  // Coinbase and high-priority
}

struct ShortId([u8; 6]);  // BIP152-style short ID
```

### Short ID Computation

```
key = SHA256(block_header || nonce)[0..16]
short_id = SipHash-2-4(key, wtxid)[0..6]
```

### Reconstruction Result

```rust
enum ReconstructionResult {
    Success(Block),
    NeedMoreTxs(Vec<u16>),  // Indices of missing transactions
    Failed,
}
```

**Code Reference:** `botho/src/network/compact_block.rs`

---

## Security Audit Checklist

### P2P Protocol
- [ ] Messages validated before processing
- [ ] Peer reputation properly tracked
- [ ] Connection limits enforced

### Rate Limiting
- [ ] Per-peer rate limits work correctly
- [ ] Size limits prevent memory exhaustion
- [ ] Rate limiters can't be bypassed

### Sync Protocol
- [ ] Invalid blocks are rejected
- [ ] Checkpoint verification works
- [ ] Reorg depth is limited

### Compact Blocks
- [ ] Short IDs don't collide in practice
- [ ] Missing transaction requests are bounded
- [ ] Reconstruction validates result

### Attack Resistance
- [ ] Eclipse attack mitigated by peer diversity
- [ ] Sybil attack limited by connection caps
- [ ] Amplification prevented by response limits
