# Traffic Analysis Resistance Roadmap

**Status**: Draft
**Created**: 2025-01-03
**Updated**: 2025-01-03
**Authors**: Core Team

## Executive Summary

This document outlines a unified approach to traffic analysis resistance in botho's P2P network. The core insight is that **every node should be a relay** - there is no separate "relay layer." This eliminates incentive problems, prevents centralization, and maximizes the anonymity set.

## Design Philosophy

> "In a privacy network, there should be no special nodes. Every participant is equal."

Traditional relay networks create two classes: users and relays. This causes:
- Incentive problems (who pays relays?)
- Centralization pressure (well-resourced operators dominate)
- Reduced anonymity (smaller relay set = easier correlation)

Our approach: **Onion Gossip** - merge onion routing with gossipsub at the protocol level. Every node relays. No exceptions.

## Threat Model

### Adversary Capabilities

| Adversary Type | Capabilities | Examples |
|----------------|--------------|----------|
| **Passive Local** | Observes traffic on local network | ISP, coffee shop WiFi |
| **Passive Global** | Observes traffic across multiple network segments | Nation-state, large ISP coalitions |
| **Active Local** | Can inject/delay packets locally | Malicious router, local MITM |
| **Active Global** | Can inject/delay packets globally | Nation-state with infrastructure control |

### What We're Protecting Against

1. **Transaction Origin Detection**: Identifying who originated a transaction
2. **Peer Graph Analysis**: Mapping network topology to identify targets
3. **Activity Correlation**: Linking on-chain activity to IP addresses
4. **Behavioral Fingerprinting**: Identifying users by transaction timing
5. **Protocol Fingerprinting**: Identifying botho traffic for censorship

### Security Properties We Achieve

| Property | Mechanism |
|----------|-----------|
| **Sender Anonymity** | Onion routing through 3 random peers |
| **Relationship Anonymity** | Observers can't link sender to transaction |
| **Traffic Uniformity** | Padding and cover traffic normalize patterns |
| **Protocol Indistinguishability** | WebRTC framing hides protocol identity |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    UNIFIED PRIVACY ARCHITECTURE                          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│                        ┌─────────────────────┐                          │
│                        │  EVERY NODE IS THE  │                          │
│                        │       SAME          │                          │
│                        │                     │                          │
│                        │  • Sends traffic    │                          │
│                        │  • Relays traffic   │                          │
│                        │  • Receives traffic │                          │
│                        └──────────┬──────────┘                          │
│                                   │                                     │
│         ┌─────────────────────────┼─────────────────────────┐           │
│         │                         │                         │           │
│         ▼                         ▼                         ▼           │
│   ┌───────────┐           ┌───────────────┐          ┌────────────┐    │
│   │   FAST    │           │    PRIVATE    │          │  PROTOCOL  │    │
│   │   PATH    │           │     PATH      │          │ OBFUSCATION│    │
│   ├───────────┤           ├───────────────┤          ├────────────┤    │
│   │ Direct    │           │ Onion Gossip  │          │ WebRTC     │    │
│   │ Gossipsub │           │ (3-hop relay) │          │ Framing    │    │
│   ├───────────┤           ├───────────────┤          ├────────────┤    │
│   │ • SCP     │           │ • Transactions│          │ All traffic│    │
│   │ • Blocks  │           │ • Queries     │          │ looks like │    │
│   │ • Announce│           │ • Sync        │          │ video calls│    │
│   └───────────┘           └───────────────┘          └────────────┘    │
│         │                         │                         │           │
│         └─────────────────────────┼─────────────────────────┘           │
│                                   │                                     │
│                                   ▼                                     │
│                        ┌─────────────────────┐                          │
│                        │ TRAFFIC NORMAL-     │                          │
│                        │ IZATION LAYER       │                          │
│                        │                     │                          │
│                        │ • Fixed-size msgs   │                          │
│                        │ • Cover traffic     │                          │
│                        │ • Timing jitter     │                          │
│                        └─────────────────────┘                          │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Phase 1: Onion Gossip Core

### Overview

Onion Gossip merges onion routing with gossipsub. Every transaction is routed through a 3-hop circuit of randomly selected peers before being broadcast. Every node participates as a potential relay.

### How It Works

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         ONION GOSSIP FLOW                                │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   Alice wants to broadcast transaction T                                │
│                                                                         │
│   STEP 1: Build Circuit                                                 │
│   ══════════════════════                                                │
│   Select 3 random peers from gossipsub mesh:                            │
│   Circuit = [Peer_X, Peer_Y, Peer_Z]                                    │
│                                                                         │
│   STEP 2: Onion Wrap                                                    │
│   ══════════════════                                                    │
│   Each layer encrypted to one hop:                                      │
│                                                                         │
│   ┌─────────────────────────────────────────────────┐                   │
│   │ Encrypt_X( next=Y,                              │                   │
│   │   ┌─────────────────────────────────────────┐   │                   │
│   │   │ Encrypt_Y( next=Z,                      │   │                   │
│   │   │   ┌─────────────────────────────────┐   │   │                   │
│   │   │   │ Encrypt_Z( action=BROADCAST,    │   │   │                   │
│   │   │   │            payload=T )          │   │   │                   │
│   │   │   └─────────────────────────────────┘   │   │                   │
│   │   └─────────────────────────────────────────┘   │                   │
│   └─────────────────────────────────────────────────┘                   │
│                                                                         │
│   STEP 3: Relay Chain                                                   │
│   ═══════════════════                                                   │
│                                                                         │
│   Alice ───► Peer_X ───► Peer_Y ───► Peer_Z ───► Gossipsub              │
│          │           │           │           │                          │
│          │           │           │           └─► Decrypts, sees T       │
│          │           │           │               Broadcasts to network  │
│          │           │           │               (appears as origin)    │
│          │           │           │                                      │
│          │           │           └─► Decrypts, sees "forward to Z"      │
│          │           │               (doesn't know T or Alice)          │
│          │           │                                                  │
│          │           └─► Decrypts, sees "forward to Y"                  │
│          │               (doesn't know T, Z, or Alice)                  │
│          │                                                              │
│          └─► Encrypted blob                                             │
│              (knows Alice, but not T or destination)                    │
│                                                                         │
│   RESULT: No single node knows both origin AND content                  │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Dual-Path Architecture

Not all messages need privacy. SCP consensus needs low latency:

```rust
/// Message routing decision
enum MessagePath {
    /// Direct gossipsub - low latency, visible origin
    Fast,
    /// Onion gossip - higher latency, hidden origin
    Private,
}

fn select_path(msg: &Message) -> MessagePath {
    match msg.message_type() {
        // Consensus is time-critical, doesn't reveal tx origin
        MessageType::ScpStatement => MessagePath::Fast,
        MessageType::ScpNominate => MessagePath::Fast,

        // Block propagation is public information
        MessageType::BlockHeader => MessagePath::Fast,
        MessageType::BlockBody => MessagePath::Fast,

        // Transactions reveal origin - MUST be private
        MessageType::Transaction => MessagePath::Private,

        // Sync requests could reveal wallet addresses
        MessageType::SyncRequest => MessagePath::Private,

        // Peer announcements are public
        MessageType::PeerAnnouncement => MessagePath::Fast,
    }
}
```

### Technical Design

#### 1.1 Node Structure

Every node includes relay capability as a core feature:

```rust
/// Core botho node with integrated relay
pub struct BothoNode {
    /// Node identity
    peer_id: PeerId,
    keypair: Keypair,

    /// Standard gossipsub for fast path
    gossipsub: Gossipsub,

    /// Onion relay state (every node has this)
    relay: RelayState,

    /// Pre-built outbound circuits for private path
    circuits: CircuitPool,

    /// Peer connections (shared between fast and private paths)
    peers: PeerManager,
}

/// Relay state - manages circuits where we're a hop
struct RelayState {
    /// Circuit keys for decryption (we're a relay hop)
    circuit_keys: HashMap<CircuitId, CircuitHopKey>,

    /// Circuits we've created (we're the origin)
    our_circuits: HashMap<CircuitId, OutboundCircuit>,

    /// Rate limiting per peer
    relay_limits: HashMap<PeerId, RateLimiter>,
}

/// Key material for one hop of a circuit
struct CircuitHopKey {
    /// Symmetric key for this hop
    key: SymmetricKey,

    /// Next hop (None if we're the exit)
    next_hop: Option<PeerId>,

    /// Circuit creation time
    created_at: Instant,

    /// Is this an exit hop (we broadcast the message)?
    is_exit: bool,
}
```

#### 1.2 Circuit Construction

Circuits are built through existing gossipsub peers:

```rust
/// Pool of pre-built circuits for quick sending
struct CircuitPool {
    /// Active circuits ready for use
    active: Vec<OutboundCircuit>,

    /// Minimum circuits to maintain
    min_circuits: usize,

    /// Circuit rotation interval
    rotation_interval: Duration,

    /// Background circuit builder
    builder: CircuitBuilder,
}

impl CircuitPool {
    /// Default configuration
    fn default() -> Self {
        Self {
            active: Vec::new(),
            min_circuits: 3,
            rotation_interval: Duration::from_secs(600), // 10 minutes
            builder: CircuitBuilder::new(),
        }
    }

    /// Get a random circuit for sending
    fn get_circuit(&self) -> Option<&OutboundCircuit> {
        self.active.choose(&mut rand::thread_rng())
    }

    /// Background task: maintain circuit pool
    async fn maintain(&mut self, peers: &PeerManager) {
        loop {
            // Remove expired circuits
            self.active.retain(|c| !c.is_expired());

            // Build new circuits if needed
            while self.active.len() < self.min_circuits {
                if let Ok(circuit) = self.builder.build(peers).await {
                    self.active.push(circuit);
                }
            }

            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }
}

/// Circuit through 3 relay hops
struct OutboundCircuit {
    /// Unique circuit identifier
    id: CircuitId,

    /// Ordered relay hops
    hops: [PeerId; 3],

    /// Symmetric keys for each hop (for onion encryption)
    hop_keys: [SymmetricKey; 3],

    /// When this circuit was built
    created_at: Instant,

    /// When to rotate (randomized around rotation_interval)
    expires_at: Instant,
}

impl OutboundCircuit {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}
```

#### 1.3 Circuit Building Protocol

```rust
/// Builds circuits through existing peers
struct CircuitBuilder {
    /// Minimum relay score to consider
    min_relay_score: f64,
}

impl CircuitBuilder {
    /// Build a new 3-hop circuit
    async fn build(&self, peers: &PeerManager) -> Result<OutboundCircuit, CircuitError> {
        // Select 3 diverse peers (different subnets)
        let hops = self.select_diverse_hops(peers, 3)?;

        // Perform telescoping handshake
        let hop_keys = self.telescoping_handshake(&hops).await?;

        // Calculate expiry with jitter
        let base_lifetime = Duration::from_secs(600);
        let jitter = Duration::from_secs(rand::thread_rng().gen_range(0..180));
        let expires_at = Instant::now() + base_lifetime + jitter;

        Ok(OutboundCircuit {
            id: CircuitId::random(),
            hops: [hops[0].clone(), hops[1].clone(), hops[2].clone()],
            hop_keys: [hop_keys[0].clone(), hop_keys[1].clone(), hop_keys[2].clone()],
            created_at: Instant::now(),
            expires_at,
        })
    }

    /// Select hops from different network segments
    fn select_diverse_hops(
        &self,
        peers: &PeerManager,
        count: usize,
    ) -> Result<Vec<PeerId>, CircuitError> {
        let candidates: Vec<_> = peers
            .connected_peers()
            .filter(|p| p.relay_score() >= self.min_relay_score)
            .collect();

        if candidates.len() < count {
            return Err(CircuitError::InsufficientPeers);
        }

        // Weighted random selection, ensuring subnet diversity
        let mut selected = Vec::new();
        let mut used_subnets = HashSet::new();

        for _ in 0..count {
            let hop = candidates
                .iter()
                .filter(|p| !used_subnets.contains(&p.subnet_prefix()))
                .choose_weighted(&mut rand::thread_rng(), |p| p.relay_score())
                .ok_or(CircuitError::InsufficientDiversity)?;

            selected.push(hop.peer_id.clone());
            used_subnets.insert(hop.subnet_prefix());
        }

        Ok(selected)
    }

    /// Establish keys with each hop via telescoping handshake
    async fn telescoping_handshake(
        &self,
        hops: &[PeerId],
    ) -> Result<Vec<SymmetricKey>, CircuitError> {
        let mut keys = Vec::new();

        // Handshake with each hop, extending through previous hops
        for (i, hop) in hops.iter().enumerate() {
            let handshake = CircuitHandshake::new();

            if i == 0 {
                // Direct handshake with first hop
                let key = handshake.perform_direct(hop).await?;
                keys.push(key);
            } else {
                // Handshake through existing circuit portion
                let key = handshake.perform_extended(&keys, &hops[..i], hop).await?;
                keys.push(key);
            }
        }

        Ok(keys)
    }
}
```

#### 1.4 Onion Encryption

```rust
impl OutboundCircuit {
    /// Wrap a message in onion layers
    fn wrap(&self, payload: &[u8]) -> OnionMessage {
        let mut wrapped = payload.to_vec();

        // Wrap from innermost (exit) to outermost (first hop)
        // Exit hop gets: [ACTION_BROADCAST][payload]
        wrapped = self.encrypt_exit_layer(&self.hop_keys[2], &wrapped);

        // Middle hop gets: [FORWARD][next_hop][encrypted_inner]
        wrapped = self.encrypt_forward_layer(&self.hop_keys[1], &self.hops[2], &wrapped);

        // First hop gets: [FORWARD][next_hop][encrypted_inner]
        wrapped = self.encrypt_forward_layer(&self.hop_keys[0], &self.hops[1], &wrapped);

        OnionMessage {
            circuit_id: self.id,
            payload: wrapped,
        }
    }

    fn encrypt_exit_layer(&self, key: &SymmetricKey, payload: &[u8]) -> Vec<u8> {
        let mut plaintext = vec![LayerType::Exit as u8];
        plaintext.extend_from_slice(payload);

        encrypt_authenticated(key, &plaintext)
    }

    fn encrypt_forward_layer(
        &self,
        key: &SymmetricKey,
        next_hop: &PeerId,
        inner: &[u8],
    ) -> Vec<u8> {
        let mut plaintext = vec![LayerType::Forward as u8];
        plaintext.extend_from_slice(&next_hop.to_bytes());
        plaintext.extend_from_slice(inner);

        encrypt_authenticated(key, &plaintext)
    }
}

/// Encrypt with ChaCha20-Poly1305
fn encrypt_authenticated(key: &SymmetricKey, plaintext: &[u8]) -> Vec<u8> {
    let nonce = generate_random_nonce();
    let ciphertext = ChaCha20Poly1305::encrypt(key, &nonce, plaintext);

    // [nonce (12 bytes)][ciphertext][tag (16 bytes)]
    let mut output = Vec::with_capacity(12 + ciphertext.len());
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&ciphertext);
    output
}
```

#### 1.5 Relay Message Handling

```rust
impl BothoNode {
    /// Handle incoming onion relay message
    async fn handle_relay_message(&mut self, from: PeerId, msg: OnionMessage) {
        // Rate limit per peer
        if !self.relay.relay_limits
            .entry(from.clone())
            .or_insert_with(RateLimiter::new)
            .check()
        {
            log::warn!("Rate limited relay from {}", from);
            return;
        }

        // Look up circuit key
        let Some(hop_key) = self.relay.circuit_keys.get(&msg.circuit_id) else {
            // Unknown circuit - could be stale or invalid
            return;
        };

        // Decrypt one layer
        let decrypted = match decrypt_authenticated(&hop_key.key, &msg.payload) {
            Ok(d) => d,
            Err(_) => {
                log::warn!("Failed to decrypt relay message");
                return;
            }
        };

        // Parse layer type
        let layer_type = LayerType::from_byte(decrypted[0]);
        let inner = &decrypted[1..];

        match layer_type {
            LayerType::Forward => {
                // We're a middle hop - extract next hop and forward
                let (next_hop, payload) = parse_forward_layer(inner);

                let forwarded = OnionMessage {
                    circuit_id: msg.circuit_id,
                    payload: payload.to_vec(),
                };

                self.send_to_peer(&next_hop, Message::OnionRelay(forwarded)).await;
            }

            LayerType::Exit => {
                // We're the exit - process the actual message
                self.handle_exit_payload(inner).await;
            }
        }
    }

    /// Handle decrypted payload at exit hop
    async fn handle_exit_payload(&mut self, payload: &[u8]) {
        let inner_msg: InnerMessage = deserialize(payload);

        match inner_msg {
            InnerMessage::Transaction(tx) => {
                // Validate transaction
                if let Err(e) = self.validate_transaction(&tx) {
                    log::warn!("Invalid transaction from circuit: {}", e);
                    return;
                }

                // Broadcast via gossipsub (we appear as origin)
                self.gossipsub.publish(TOPIC_TRANSACTIONS, serialize(&tx));
            }

            InnerMessage::SyncRequest(req) => {
                // Handle sync request, send response back
                let response = self.handle_sync_request(req).await;
                // Response goes back through a different mechanism
                // (not detailed here - could use reply circuits)
            }
        }
    }
}
```

#### 1.6 Relay Capacity and Selection

Nodes advertise their relay capacity; circuit builders prefer high-capacity nodes:

```rust
/// Self-assessed relay capacity (advertised to peers)
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayCapacity {
    /// Available bandwidth for relaying (bytes/sec)
    pub bandwidth_bps: u64,

    /// Average uptime over last 24h (0.0 - 1.0)
    pub uptime_ratio: f64,

    /// Whether behind restrictive NAT
    pub nat_type: NatType,

    /// Current load (0.0 - 1.0)
    pub current_load: f64,
}

impl RelayCapacity {
    /// Calculate relay score for circuit selection
    pub fn relay_score(&self) -> f64 {
        let mut score = 0.0;

        // Bandwidth: up to 0.4 for 10 MB/s+
        let bw_score = (self.bandwidth_bps as f64 / 10_000_000.0).min(1.0) * 0.4;
        score += bw_score;

        // Uptime: up to 0.3
        score += self.uptime_ratio * 0.3;

        // NAT penalty
        match self.nat_type {
            NatType::Open => score += 0.2,
            NatType::FullCone => score += 0.15,
            NatType::Restricted => score += 0.1,
            NatType::Symmetric => score += 0.0,
        }

        // Load penalty
        score *= 1.0 - (self.current_load * 0.5);

        // Minimum score ensures everyone participates
        score.max(0.1)
    }
}
```

### Integration with Existing Stack

| Component | Changes |
|-----------|---------|
| `gossip/src/behaviour.rs` | Add `OnionRelay` message type |
| `gossip/src/rate_limit.rs` | Add relay-specific rate limits |
| `botho/src/network/discovery.rs` | Include `RelayCapacity` in peer info |
| `botho/src/network/mod.rs` | Add `CircuitPool` and `RelayState` |
| Transaction pool | Route through `CircuitPool` before broadcast |

### Bandwidth Analysis

```
Assumptions:
- Network size: 10,000 nodes
- Transaction rate: 100 tx/sec
- Transaction size: 500 bytes
- Circuit hops: 3
- Gossipsub fanout: 6

Current bandwidth per node (gossipsub only):
  Receive: 100 tx/s × 500B × 6 fanout = 300 KB/s

With Onion Gossip:
  Additional relay traffic per node:
    - Each tx traverses 3 hops
    - Probability of being a hop: 3 / 10,000 = 0.0003
    - Relay traffic: 100 tx/s × 0.0003 × 500B = 15 bytes/sec

  Onion overhead per message: ~150 bytes (nonces, MACs)
    - Additional: 100 tx/s × 150B × 6 fanout = 90 KB/s

  TOTAL additional: ~90 KB/s (30% increase)

This is very manageable.
```

### Security Properties

| Attack | Mitigation | Residual Risk |
|--------|-----------|---------------|
| **Observe all hops** | Random hop selection, subnet diversity | Adversary needs ~30% of network |
| **Timing correlation** | Phase 2 adds cover traffic | Mitigated with traffic normalization |
| **Circuit fingerprinting** | Fixed-size messages, regular rotation | Low |
| **Malicious exit** | Exits only see transaction, not origin | Acceptable (tx is public anyway) |
| **Sybil attack** | Existing PEX protections, subnet limits | Requires massive investment |

### Milestone Checklist

- [ ] **1.1**: Core data structures (`RelayState`, `CircuitPool`, `OutboundCircuit`)
- [ ] **1.2**: Circuit handshake protocol
- [ ] **1.3**: Onion encryption/decryption
- [ ] **1.4**: Relay message handling
- [ ] **1.5**: Circuit pool maintenance (background builder)
- [ ] **1.6**: Integration with transaction broadcast
- [ ] **1.7**: Relay capacity advertisement
- [ ] **1.8**: Circuit selection with diversity requirements
- [ ] **1.9**: Dual-path routing (fast vs private)
- [ ] **1.10**: Rate limiting for relay traffic
- [ ] **1.11**: Metrics and monitoring
- [ ] **1.12**: Integration tests with simulated network
- [ ] **1.13**: Security audit

---

## Phase 2: Traffic Normalization

### Overview

Even with onion routing, traffic patterns leak information. This phase makes all traffic look uniform through padding, constant-rate transmission, and cover traffic.

### Technical Design

#### 2.1 Message Padding

All messages padded to fixed bucket sizes:

```rust
/// Standard message size buckets
const SIZE_BUCKETS: [usize; 5] = [
    512,      // Tiny: pings, acks
    2048,     // Small: typical transactions
    8192,     // Medium: multi-input transactions
    32768,    // Large: block headers
    131072,   // XLarge: block bodies (128 KB)
];

/// Pad message to next bucket size
fn pad_to_bucket(payload: &[u8]) -> Vec<u8> {
    // Find smallest bucket that fits payload + length header
    let needed = payload.len() + 2; // 2 bytes for length
    let bucket_size = SIZE_BUCKETS
        .iter()
        .find(|&&size| size >= needed)
        .copied()
        .unwrap_or(SIZE_BUCKETS[SIZE_BUCKETS.len() - 1]);

    let mut padded = Vec::with_capacity(bucket_size);

    // Length header (2 bytes, little-endian)
    padded.extend_from_slice(&(payload.len() as u16).to_le_bytes());

    // Actual payload
    padded.extend_from_slice(payload);

    // Random padding (not zeros - distinguishable)
    let padding_len = bucket_size - padded.len();
    let mut rng = rand::thread_rng();
    for _ in 0..padding_len {
        padded.push(rng.gen());
    }

    padded
}

/// Remove padding and extract original message
fn unpad(padded: &[u8]) -> Result<&[u8], PaddingError> {
    if padded.len() < 2 {
        return Err(PaddingError::TooShort);
    }

    let len = u16::from_le_bytes([padded[0], padded[1]]) as usize;

    if padded.len() < 2 + len {
        return Err(PaddingError::InvalidLength);
    }

    Ok(&padded[2..2 + len])
}
```

#### 2.2 Constant-Rate Transmission

Optional mode for high-privacy users:

```rust
/// Constant-rate transmitter configuration
pub struct ConstantRateConfig {
    /// Target messages per second
    pub messages_per_second: f64,

    /// Generate cover traffic when queue is empty
    pub cover_traffic: bool,

    /// Maximum queue depth before dropping old messages
    pub max_queue_depth: usize,
}

impl Default for ConstantRateConfig {
    fn default() -> Self {
        Self {
            messages_per_second: 2.0,  // 1 message every 500ms
            cover_traffic: true,
            max_queue_depth: 100,
        }
    }
}

/// Transmitter that sends at constant rate
pub struct ConstantRateTransmitter {
    config: ConstantRateConfig,
    queue: VecDeque<QueuedMessage>,
    last_send: Instant,
    circuits: Arc<CircuitPool>,
}

impl ConstantRateTransmitter {
    /// Add message to queue (called when user creates transaction)
    pub fn enqueue(&mut self, msg: OutgoingMessage) {
        if self.queue.len() >= self.config.max_queue_depth {
            // Drop oldest message
            self.queue.pop_front();
        }
        self.queue.push_back(QueuedMessage {
            message: msg,
            queued_at: Instant::now(),
        });
    }

    /// Called on timer - sends next message or cover traffic
    pub async fn tick(&mut self) -> Option<()> {
        let interval = Duration::from_secs_f64(1.0 / self.config.messages_per_second);

        if self.last_send.elapsed() < interval {
            return None;
        }

        self.last_send = Instant::now();

        // Send real message if available
        if let Some(queued) = self.queue.pop_front() {
            self.send_message(queued.message).await;
        } else if self.config.cover_traffic {
            // Send cover traffic
            self.send_cover().await;
        }

        Some(())
    }

    async fn send_cover(&self) {
        let cover = CoverMessage::generate();
        let circuit = self.circuits.get_circuit().unwrap();

        let wrapped = circuit.wrap(&cover.serialize());
        // Send through circuit - exit will silently drop
    }
}

/// Cover message indistinguishable from real transaction
#[derive(Serialize, Deserialize)]
struct CoverMessage {
    /// Message type marker (only visible after decryption)
    msg_type: MessageType,
    /// Random data matching typical transaction size
    payload: Vec<u8>,
}

impl CoverMessage {
    fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let size = rng.gen_range(200..600); // Match transaction size distribution

        Self {
            msg_type: MessageType::Cover,
            payload: (0..size).map(|_| rng.gen()).collect(),
        }
    }
}
```

#### 2.3 Timing Jitter

Add random delays to messages:

```rust
/// Add jitter to message timing
pub struct TimingJitter {
    /// Base delay range in milliseconds
    base_range: (u64, u64),
}

impl TimingJitter {
    /// Default: 50-200ms jitter
    pub fn default() -> Self {
        Self {
            base_range: (50, 200),
        }
    }

    /// Calculate delay for a message
    pub fn delay(&self) -> Duration {
        let mut rng = rand::thread_rng();
        let ms = rng.gen_range(self.base_range.0..=self.base_range.1);
        Duration::from_millis(ms)
    }
}

/// Apply jitter before sending
async fn send_with_jitter(msg: Message, jitter: &TimingJitter) {
    tokio::time::sleep(jitter.delay()).await;
    send(msg).await;
}
```

### Privacy Levels

Users can choose their privacy/performance tradeoff:

```rust
/// Privacy level configuration
pub enum PrivacyLevel {
    /// Standard: Onion routing only
    /// - Latency: ~100ms added
    /// - Bandwidth: ~30% overhead
    Standard,

    /// Enhanced: Onion + padding + jitter
    /// - Latency: ~200-400ms added
    /// - Bandwidth: ~50% overhead
    Enhanced,

    /// Maximum: Onion + padding + constant-rate + cover traffic
    /// - Latency: Variable (queue-based)
    /// - Bandwidth: ~2x (cover traffic)
    Maximum,
}

impl PrivacyLevel {
    pub fn to_config(&self) -> PrivacyConfig {
        match self {
            Self::Standard => PrivacyConfig {
                onion_routing: true,
                padding: false,
                timing_jitter: false,
                constant_rate: false,
                cover_traffic: false,
            },
            Self::Enhanced => PrivacyConfig {
                onion_routing: true,
                padding: true,
                timing_jitter: true,
                constant_rate: false,
                cover_traffic: false,
            },
            Self::Maximum => PrivacyConfig {
                onion_routing: true,
                padding: true,
                timing_jitter: true,
                constant_rate: true,
                cover_traffic: true,
            },
        }
    }
}
```

### Milestone Checklist

- [ ] **2.1**: Message padding implementation
- [ ] **2.2**: Unpadding with validation
- [ ] **2.3**: Constant-rate transmitter
- [ ] **2.4**: Cover traffic generation
- [ ] **2.5**: Cover traffic handling (exit drops silently)
- [ ] **2.6**: Timing jitter implementation
- [ ] **2.7**: Privacy level configuration
- [ ] **2.8**: Integration with onion gossip
- [ ] **2.9**: Performance benchmarks
- [ ] **2.10**: Statistical indistinguishability tests

---

## Phase 3: Protocol Obfuscation

### Overview

Make botho traffic indistinguishable from common protocols to prevent protocol-level blocking and deep packet inspection.

### Approach: WebRTC Data Channels

WebRTC is ideal because:
- Widely used (video calls, gaming, file sharing)
- Already uses DTLS encryption
- Designed for P2P with NAT traversal
- Traffic patterns match our needs
- Blocking would break legitimate video calling

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    PROTOCOL STACK COMPARISON                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   Current Stack                    Obfuscated Stack                     │
│   ═════════════                    ════════════════                     │
│                                                                         │
│   ┌─────────────┐                 ┌─────────────────┐                   │
│   │ Application │                 │   Application   │                   │
│   │ (Gossipsub) │                 │   (Gossipsub)   │                   │
│   └──────┬──────┘                 └────────┬────────┘                   │
│          │                                 │                            │
│   ┌──────▼──────┐                 ┌────────▼────────┐                   │
│   │   Yamux     │                 │     Yamux       │                   │
│   └──────┬──────┘                 └────────┬────────┘                   │
│          │                                 │                            │
│   ┌──────▼──────┐                 ┌────────▼────────┐                   │
│   │   Noise     │                 │  SCTP/DataChan  │ ◄── WebRTC       │
│   └──────┬──────┘                 └────────┬────────┘                   │
│          │                                 │                            │
│   ┌──────▼──────┐                 ┌────────▼────────┐                   │
│   │    TCP      │                 │   DTLS 1.3      │ ◄── WebRTC       │
│   └─────────────┘                 └────────┬────────┘                   │
│                                            │                            │
│                                   ┌────────▼────────┐                   │
│                                   │    ICE/UDP      │ ◄── WebRTC       │
│                                   └─────────────────┘                   │
│                                                                         │
│   DPI sees: "Custom P2P"          DPI sees: "Video call"               │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Technical Design

#### 3.1 Pluggable Transport Interface

Support multiple obfuscation methods:

```rust
/// Pluggable transport trait
#[async_trait]
pub trait PluggableTransport: Send + Sync {
    /// Human-readable name
    fn name(&self) -> &'static str;

    /// Wrap an outgoing connection
    async fn wrap_outbound(
        &self,
        stream: TcpStream,
        peer: &PeerId,
    ) -> Result<Box<dyn AsyncReadWrite>, TransportError>;

    /// Accept an incoming connection
    async fn wrap_inbound(
        &self,
        stream: TcpStream,
    ) -> Result<Box<dyn AsyncReadWrite>, TransportError>;
}

/// Available transport types
pub enum TransportType {
    /// Standard TCP + Noise (current)
    Plain,

    /// WebRTC data channels
    WebRTC,

    /// TLS 1.3 tunnel (looks like HTTPS)
    TlsTunnel,

    /// obfs4-style randomized
    Obfs4,
}
```

#### 3.2 WebRTC Transport

```rust
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;

pub struct WebRtcTransport {
    /// STUN servers for ICE
    stun_servers: Vec<String>,
}

impl WebRtcTransport {
    pub fn new() -> Self {
        Self {
            stun_servers: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
            ],
        }
    }
}

#[async_trait]
impl PluggableTransport for WebRtcTransport {
    fn name(&self) -> &'static str {
        "webrtc"
    }

    async fn wrap_outbound(
        &self,
        _stream: TcpStream,
        peer: &PeerId,
    ) -> Result<Box<dyn AsyncReadWrite>, TransportError> {
        // Create peer connection
        let config = RTCConfiguration {
            ice_servers: self.stun_servers
                .iter()
                .map(|url| RTCIceServer {
                    urls: vec![url.clone()],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        let peer_connection = RTCPeerConnection::new(config).await?;

        // Create data channel for botho traffic
        let data_channel = peer_connection
            .create_data_channel("data", None)
            .await?;

        // Create and send offer (via signaling channel)
        let offer = peer_connection.create_offer(None).await?;
        peer_connection.set_local_description(offer.clone()).await?;

        // Exchange SDP via existing connection...
        // (implementation details omitted)

        // Wait for ICE completion
        wait_for_ice_complete(&peer_connection).await?;

        // Return wrapped connection
        Ok(Box::new(WebRtcConnection {
            peer_connection,
            data_channel,
        }))
    }

    async fn wrap_inbound(
        &self,
        stream: TcpStream,
    ) -> Result<Box<dyn AsyncReadWrite>, TransportError> {
        // Similar but receiving offer instead of creating
        // ...
    }
}

/// WebRTC connection wrapper
struct WebRtcConnection {
    peer_connection: RTCPeerConnection,
    data_channel: RTCDataChannel,
}

impl AsyncRead for WebRtcConnection {
    // Read from data channel
}

impl AsyncWrite for WebRtcConnection {
    // Write to data channel
}
```

#### 3.3 Transport Selection

```rust
/// Transport manager handles transport selection
pub struct TransportManager {
    available: Vec<Box<dyn PluggableTransport>>,
    preferred: TransportType,
}

impl TransportManager {
    pub fn new(config: TransportConfig) -> Self {
        let mut available: Vec<Box<dyn PluggableTransport>> = Vec::new();

        // Always support plain transport
        available.push(Box::new(PlainTransport::new()));

        if config.enable_webrtc {
            available.push(Box::new(WebRtcTransport::new()));
        }

        if config.enable_tls_tunnel {
            available.push(Box::new(TlsTunnelTransport::new(config.tls_config)));
        }

        Self {
            available,
            preferred: config.preferred_transport,
        }
    }

    /// Select transport for a new connection
    pub fn select(&self, peer: &PeerInfo) -> &dyn PluggableTransport {
        // Try preferred transport if peer supports it
        if peer.supports_transport(&self.preferred) {
            return self.get_transport(&self.preferred);
        }

        // Fall back to any common transport
        for transport in &self.available {
            if peer.supports_transport_name(transport.name()) {
                return transport.as_ref();
            }
        }

        // Last resort: plain
        self.get_transport(&TransportType::Plain)
    }
}
```

### Fingerprinting Resistance

Verify traffic is indistinguishable:

```rust
#[cfg(test)]
mod fingerprint_tests {
    use super::*;
    use statistical_tests::*;

    /// Capture traffic patterns
    struct TrafficPattern {
        packet_sizes: Vec<usize>,
        inter_arrival_times: Vec<Duration>,
        flow_duration: Duration,
    }

    #[tokio::test]
    async fn test_webrtc_indistinguishable() {
        // Capture botho WebRTC traffic
        let botho_pattern = capture_botho_webrtc_traffic().await;

        // Capture real Google Meet traffic
        let meet_pattern = capture_google_meet_traffic().await;

        // Kolmogorov-Smirnov test for packet size distribution
        let ks_sizes = kolmogorov_smirnov(
            &botho_pattern.packet_sizes,
            &meet_pattern.packet_sizes,
        );
        assert!(ks_sizes.p_value > 0.05, "Packet sizes distinguishable");

        // K-S test for inter-arrival times
        let ks_times = kolmogorov_smirnov(
            &botho_pattern.inter_arrival_times.iter().map(|d| d.as_micros()).collect(),
            &meet_pattern.inter_arrival_times.iter().map(|d| d.as_micros()).collect(),
        );
        assert!(ks_times.p_value > 0.05, "Timing patterns distinguishable");
    }
}
```

### Milestone Checklist

- [ ] **3.1**: Pluggable transport interface
- [ ] **3.2**: WebRTC data channel transport
- [ ] **3.3**: DTLS integration
- [ ] **3.4**: ICE/STUN for NAT traversal
- [ ] **3.5**: Signaling channel for SDP exchange
- [ ] **3.6**: Transport negotiation protocol
- [ ] **3.7**: TLS tunnel transport (optional)
- [ ] **3.8**: Transport selection logic
- [ ] **3.9**: Fingerprinting resistance tests
- [ ] **3.10**: Performance benchmarks across transports
- [ ] **3.11**: Documentation
- [ ] **3.12**: Security audit

---

## Implementation Timeline

```
         2025 Q1          2025 Q2          2025 Q3          2025 Q4
        ═══════════      ═══════════      ═══════════      ═══════════

Phase 1  ████████████████████████████
Onion    Design    Implementation   Testing   Audit
Gossip   & Proto   & Integration    & Fixes

Phase 2                  ████████████████████████████
Traffic                  Design    Implementation   Testing
Normal

Phase 3                              ████████████████████████████████
Protocol                             Design    Impl    Test    Audit
Obfusc
```

## Dependencies

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         DEPENDENCY GRAPH                                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   Phase 1: Onion Gossip Core                                            │
│        │                                                                │
│        │ (provides private transport)                                   │
│        │                                                                │
│        ├────────────────────────────────────┐                           │
│        │                                    │                           │
│        ▼                                    ▼                           │
│   Phase 2: Traffic Normalization    Phase 3: Protocol Obfuscation       │
│   (adds uniformity to traffic)      (disguises transport)               │
│        │                                    │                           │
│        │                                    │                           │
│        └────────────────┬───────────────────┘                           │
│                         │                                               │
│                         ▼                                               │
│                  Full Privacy Stack                                     │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘

Phase 2 and Phase 3 can proceed in parallel after Phase 1 is complete.
```

## Configuration

Default configuration for various use cases:

```rust
/// Privacy configuration presets
pub struct PrivacyPresets;

impl PrivacyPresets {
    /// Standard user: balance of privacy and performance
    pub fn standard() -> PrivacyConfig {
        PrivacyConfig {
            // Phase 1: Onion Gossip
            onion_gossip: OnionGossipConfig {
                enabled: true,
                circuit_hops: 3,
                circuit_lifetime: Duration::from_secs(600),
                min_relay_score: 0.2,
            },

            // Phase 2: Traffic Normalization
            traffic_normalization: TrafficNormConfig {
                padding: true,
                constant_rate: false,
                cover_traffic: false,
                timing_jitter: true,
                jitter_range: (50, 150),
            },

            // Phase 3: Protocol Obfuscation
            transport: TransportConfig {
                preferred: TransportType::Plain,
                enable_webrtc: false,
                enable_tls_tunnel: false,
            },
        }
    }

    /// High-risk user: maximum privacy
    pub fn maximum() -> PrivacyConfig {
        PrivacyConfig {
            onion_gossip: OnionGossipConfig {
                enabled: true,
                circuit_hops: 3,
                circuit_lifetime: Duration::from_secs(300), // Faster rotation
                min_relay_score: 0.3,
            },

            traffic_normalization: TrafficNormConfig {
                padding: true,
                constant_rate: true,
                cover_traffic: true,
                timing_jitter: true,
                jitter_range: (100, 300),
            },

            transport: TransportConfig {
                preferred: TransportType::WebRTC,
                enable_webrtc: true,
                enable_tls_tunnel: true,
            },
        }
    }

    /// Censored region: focus on reachability
    pub fn censorship_resistant() -> PrivacyConfig {
        PrivacyConfig {
            onion_gossip: OnionGossipConfig {
                enabled: true,
                circuit_hops: 2, // Fewer hops for reliability
                circuit_lifetime: Duration::from_secs(600),
                min_relay_score: 0.1, // Accept more relays
            },

            traffic_normalization: TrafficNormConfig {
                padding: true,
                constant_rate: false,
                cover_traffic: false,
                timing_jitter: false,
            },

            transport: TransportConfig {
                preferred: TransportType::WebRTC, // Looks like video call
                enable_webrtc: true,
                enable_tls_tunnel: true,
            },
        }
    }
}
```

## Success Metrics

| Metric | Target | Measurement Method |
|--------|--------|-------------------|
| Transaction origin privacy | <5% attribution accuracy | Simulated adversary with 10% of nodes |
| Peer graph obscurity | >90% false edges | Graph analysis resistance test |
| Protocol detection rate | <5% by DPI | Test against commercial DPI tools |
| Latency overhead (standard) | <200ms p99 | Production monitoring |
| Latency overhead (maximum) | <1s p99 | Production monitoring |
| Bandwidth overhead | <50% for standard | Traffic analysis |
| Relay participation | >95% of nodes | Network monitoring |

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Latency impacts UX | Medium | Medium | Privacy levels, async transactions |
| Implementation bugs | Medium | High | Phased rollout, extensive testing, audit |
| WebRTC complexity | Medium | Medium | Start with plain, add WebRTC later |
| Adversary adapts | Low | Medium | Modular design, pluggable transports |
| Bandwidth costs | Low | Low | Efficient implementation, cover traffic optional |

## Open Questions

1. **Default privacy level**: Should new users start with "standard" or "enhanced"?

2. **Mobile optimization**: How to reduce bandwidth for mobile nodes while maintaining privacy?

3. **Circuit building latency**: Pre-build circuits on startup? Background vs on-demand?

4. **Relay incentives (future)**: If we ever need explicit incentives, how to add without breaking privacy?

5. **Light client support**: How do SPV-style clients participate in relay network?

---

## Appendix A: Comparison with Existing Systems

| System | Approach | Anonymity Set | Latency | Complexity |
|--------|----------|---------------|---------|------------|
| **Bitcoin** | Direct gossip | None | Low | Low |
| **Monero** | Dandelion++ | Stem path | Low | Low |
| **Zcash** | Direct gossip | None | Low | Low |
| **Tor** | Dedicated relays | ~6000 relays | High | High |
| **I2P** | All nodes relay | All nodes | High | High |
| **Nym** | Mixnet + staking | Mix nodes | Very High | Very High |
| **Botho (proposed)** | All nodes relay + WebRTC | All nodes | Medium | Medium |

## Appendix B: References

- [I2P Technical Introduction](https://geti2p.net/en/docs/how/tech-intro)
- [Tor Design Document](https://svn.torproject.org/svn/projects/design-paper/tor-design.pdf)
- [Dandelion++: Lightweight Cryptocurrency Networking](https://arxiv.org/abs/1805.11060)
- [WebRTC Security Architecture](https://www.w3.org/TR/webrtc/#security-considerations)
- [Traffic Analysis Attacks and Defenses](https://www.freehaven.net/anonbib/)
- [libp2p Specifications](https://github.com/libp2p/specs)

## Appendix C: Glossary

| Term | Definition |
|------|------------|
| **Circuit** | A path through multiple relay nodes for onion routing |
| **Cover traffic** | Dummy messages sent to obscure real traffic patterns |
| **Exit node** | The final relay that broadcasts a message to the network |
| **Onion encryption** | Layered encryption where each relay removes one layer |
| **Relay score** | A node's self-assessed capacity to relay traffic |
| **Telescoping handshake** | Building a circuit by extending one hop at a time |
