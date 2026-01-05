# Transport Layer Architecture

This document describes the architecture of botho's pluggable transport system, which enables protocol obfuscation to resist deep packet inspection (DPI) and censorship.

## Overview

The transport layer provides an abstraction over the raw network connection, allowing different transport implementations to be used interchangeably. This enables botho traffic to be disguised as common protocols like video calls (WebRTC) or HTTPS (TLS Tunnel).

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                    APPLICATION LAYER                        │
│                    (Gossipsub, SCP)                         │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────┐
│                  TRANSPORT LAYER                            │
│  ┌─────────────────────────────────────────────────────┐    │
│  │         TransportSelector / TransportManager        │    │
│  │  • Capability-based selection                       │    │
│  │  • Metrics-based optimization                       │    │
│  │  • Automatic fallback                               │    │
│  └─────────────────────────────────────────────────────┘    │
│         │                    │                    │         │
│  ┌──────▼──────┐     ┌───────▼───────┐    ┌───────▼──────┐  │
│  │    Plain    │     │    WebRTC     │    │  TLS Tunnel  │  │
│  │ TCP + Noise │     │ DTLS + SCTP   │    │   TLS 1.3    │  │
│  └─────────────┘     └───────────────┘    └──────────────┘  │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────┐
│                    NETWORK LAYER                            │
│                    (TCP, UDP)                               │
└─────────────────────────────────────────────────────────────┘
```

## Core Components

### PluggableTransport Trait

The central abstraction that all transports must implement:

```rust
#[async_trait]
pub trait PluggableTransport: Send + Sync + Debug {
    /// Get the transport type identifier
    fn transport_type(&self) -> TransportType;

    /// Get the human-readable name
    fn name(&self) -> &'static str;

    /// Check if transport is available and ready
    fn is_available(&self) -> bool;

    /// Establish an outbound connection
    async fn connect(
        &self,
        peer: &PeerId,
        addr: Option<&Multiaddr>,
    ) -> Result<BoxedConnection, TransportError>;

    /// Accept an inbound connection
    async fn accept(
        &self,
        stream: BoxedConnection,
    ) -> Result<BoxedConnection, TransportError>;
}
```

**Location**: `botho/src/network/transport/traits.rs`

### TransportType Enum

Identifies available transport types:

```rust
pub enum TransportType {
    Plain,      // TCP + Noise (default)
    WebRTC,     // DTLS + SCTP data channels
    TlsTunnel,  // TLS 1.3 tunnel
}
```

**Location**: `botho/src/network/transport/types.rs`

### TransportSelector

Manages transport selection and connection attempts:

```rust
pub struct TransportSelector {
    config: TransportConfig,
    capabilities: TransportCapabilities,
    metrics: Arc<RwLock<TransportMetrics>>,
    transports: Vec<Arc<dyn PluggableTransport>>,
}
```

**Key methods**:
- `select_for_peer()` - Choose best transport for a peer
- `connect()` - Connect using selected transport
- `connect_with_fallback()` - Connect with automatic fallback

**Location**: `botho/src/network/transport/manager.rs`

### TransportCapabilities

Advertises and parses transport capabilities for peer negotiation:

```rust
pub struct TransportCapabilities {
    pub supported: Vec<TransportType>,
    pub preferred: TransportType,
    pub nat_type: NatType,
}
```

**Location**: `botho/src/network/transport/capabilities.rs`

## Transport Implementations

### Plain Transport

Standard TCP connection with Noise protocol encryption. This is the default transport with best performance but no protocol obfuscation.

```rust
pub struct PlainTransport {
    timeout: Duration,
}
```

**Location**: `botho/src/network/transport/plain.rs`

### WebRTC Transport

Uses WebRTC data channels to make traffic indistinguishable from video calls.

```rust
pub struct WebRtcTransport {
    ice_config: IceConfig,
    gatherer: IceGatherer,
    stun_client: StunClient,
}
```

**Sub-modules**:
- `dtls.rs` - DTLS configuration and certificate handling
- `ice.rs` - ICE (Interactive Connectivity Establishment)
- `stun.rs` - STUN client for NAT detection

**Location**: `botho/src/network/transport/webrtc/`

### TLS Tunnel Transport

Wraps traffic in TLS 1.3 to look like HTTPS.

```rust
pub struct TlsTunnelTransport {
    client_config: Arc<rustls::ClientConfig>,
    server_config: Option<Arc<rustls::ServerConfig>>,
}
```

**Location**: `botho/src/network/transport/tls_tunnel.rs`

## Transport Selection Algorithm

Transport selection considers multiple factors:

### 1. Capability Matching

Both peers must support the transport:

```rust
fn best_common(our: &TransportCapabilities, peer: &TransportCapabilities) -> Option<TransportType> {
    // Find intersection of supported transports
    // Return highest preference from intersection
}
```

### 2. User Preference

The `TransportPreference` enum controls selection priority:

| Preference | Priority Order |
|------------|----------------|
| Privacy | WebRTC > TLS > Plain |
| Performance | Plain > TLS > WebRTC |
| Compatibility | WebRTC > TLS > Plain |
| Specific(type) | Only the specified type |

### 3. Metrics-Based Optimization

When `enable_metrics` is true, success rates influence selection:

```rust
// Prefer transports with higher success rates
if recommended_rate > base_rate + 0.15 {
    return recommended;
}
```

### 4. NAT Compatibility

NAT type affects WebRTC viability:

| NAT Type | WebRTC Support |
|----------|----------------|
| Open | Direct connection |
| FullCone | Direct connection |
| Restricted | May work |
| Symmetric | Requires TURN relay |

## Transport Negotiation Protocol

Peers negotiate transport selection during connection establishment:

```
┌─────────┐                              ┌─────────┐
│Initiator│                              │Responder│
└────┬────┘                              └────┬────┘
     │                                        │
     │ HELLO(capabilities, preferred)         │
     │─────────────────────────────────────── │
     │                                        │
     │        SELECT(chosen_transport)        │
     │ ────────────────────────────────────── │
     │                                        │
     │      [Proceed with chosen transport]   │
     │                                        │
```

**Location**: `botho/src/network/transport/negotiation.rs`

## Signaling Channel (WebRTC)

For WebRTC, SDP offer/answer exchange happens over the existing connection:

```rust
pub struct SignalingSession {
    session_id: SessionId,
    state: SignalingState,
    local_sdp: Option<String>,
    remote_sdp: Option<String>,
    local_candidates: Vec<IceCandidate>,
    remote_candidates: Vec<IceCandidate>,
}
```

**States**:
1. `New` - Session created
2. `OfferSent` / `OfferReceived` - SDP offer exchanged
3. `AnswerSent` / `AnswerReceived` - SDP answer exchanged
4. `Complete` - ICE candidates exchanged
5. `Failed` - Signaling failed

**Location**: `botho/src/network/transport/signaling.rs`

## HTTP/2 Framing (TLS Tunnel)

Optional HTTP/2 framing for maximum obfuscation:

```rust
pub struct Http2Wrapper {
    stream_id: u32,
}

impl Http2Wrapper {
    /// Wrap data in HTTP/2 DATA frame
    pub fn wrap(&mut self, data: &[u8]) -> Vec<u8>;

    /// Unwrap HTTP/2 DATA frame
    pub fn unwrap(&self, frame: &[u8]) -> Result<Vec<u8>, Http2FrameError>;
}
```

**Location**: `botho/src/network/transport/http2.rs`

## Error Handling

Transport errors are categorized for proper handling:

```rust
pub enum TransportError {
    ConnectionFailed(String),
    HandshakeFailed(String),
    NotSupported,
    NegotiationFailed(String),
    Timeout,
    ConnectionClosed,
    InvalidPeer(String),
    Configuration(String),
    Io(io::Error),
    Ice(IceError),
    Stun(StunError),
    WebRtc(WebRtcError),
    SignalingFailed(String),
    IceFailed(String),
    DataChannel(String),
}
```

**Retryable errors**: `Timeout`, `IceFailed`, `SignalingFailed`

**Location**: `botho/src/network/transport/error.rs`

## Metrics Collection

Transport metrics enable data-driven selection:

```rust
pub struct TransportMetrics {
    stats: HashMap<TransportType, TransportStats>,
}

pub struct TransportStats {
    total_attempts: u64,
    successful: u64,
    failed: u64,
    timeouts: u64,
    latency_samples: Vec<Duration>,
}
```

**Key methods**:
- `record()` - Record connection result
- `success_rate()` - Get success rate for transport
- `recommend()` - Recommend best transport based on metrics

**Location**: `botho/src/network/transport/metrics.rs`

## Adding a New Transport

To add a new transport implementation:

### 1. Add Transport Type

```rust
// In types.rs
pub enum TransportType {
    Plain,
    WebRTC,
    TlsTunnel,
    MyNewTransport,  // Add here
}
```

### 2. Implement PluggableTransport

```rust
// In my_transport.rs
pub struct MyTransport { /* ... */ }

#[async_trait]
impl PluggableTransport for MyTransport {
    fn transport_type(&self) -> TransportType {
        TransportType::MyNewTransport
    }

    fn name(&self) -> &'static str {
        "my-transport"
    }

    async fn connect(
        &self,
        peer: &PeerId,
        addr: Option<&Multiaddr>,
    ) -> Result<BoxedConnection, TransportError> {
        // Implementation
    }

    async fn accept(
        &self,
        stream: BoxedConnection,
    ) -> Result<BoxedConnection, TransportError> {
        // Implementation
    }
}
```

### 3. Register in TransportSelector

```rust
// In manager.rs, create_transports()
if config.enable_my_transport {
    transports.push(Arc::new(MyTransport::new()));
}
```

### 4. Add Configuration

```rust
// In config.rs
pub struct TransportConfig {
    // ...
    pub enable_my_transport: bool,
    pub my_transport_config: Option<MyTransportConfig>,
}
```

### 5. Update Capabilities

```rust
// In capabilities.rs
fn create_capabilities(config: &TransportConfig, nat_type: NatType) -> TransportCapabilities {
    let mut supported = vec![CapabilityTransportType::Plain];

    if config.enable_my_transport {
        supported.push(CapabilityTransportType::MyTransport);
    }
    // ...
}
```

## Testing Transports

### Unit Tests

Each transport module includes unit tests:

```bash
cargo test --package botho --lib network::transport
```

### Integration Tests

Test transport negotiation between peers:

```bash
cargo test --package botho --test transport_integration
```

### Fingerprinting Resistance Tests

Verify traffic is indistinguishable from legitimate protocols:

```rust
// In fingerprint.rs
pub fn kolmogorov_smirnov(sample1: &[f64], sample2: &[f64]) -> KsTestResult {
    // Statistical test for distribution similarity
}
```

**Location**: `botho/src/network/transport/fingerprint.rs`

### Benchmarks

Compare transport performance:

```bash
cargo bench --package botho -- transport
```

**Location**: `botho/src/network/transport/bench.rs`

## Module Structure

```
botho/src/network/transport/
├── mod.rs              # Module root, re-exports
├── traits.rs           # PluggableTransport trait
├── types.rs            # TransportType enum
├── config.rs           # Configuration types
├── manager.rs          # TransportSelector
├── capabilities.rs     # Capability advertising
├── negotiation.rs      # Transport negotiation protocol
├── signaling.rs        # WebRTC signaling
├── error.rs            # Error types
├── metrics.rs          # Metrics collection
├── fingerprint.rs      # Fingerprinting tests
├── bench.rs            # Benchmarks
├── plain.rs            # Plain transport
├── tls_tunnel.rs       # TLS tunnel transport
├── http2.rs            # HTTP/2 framing
└── webrtc/
    ├── mod.rs          # WebRTC transport
    ├── dtls.rs         # DTLS configuration
    ├── ice.rs          # ICE gathering
    └── stun.rs         # STUN client
```

## See Also

- [Protocol Obfuscation Configuration](../operations/protocol-obfuscation.md) - User guide
- [Transport Security](../security/transport-security.md) - Security considerations
- [Traffic Privacy Roadmap](../design/traffic-privacy-roadmap.md) - Design document
- [Network Architecture](./network.md) - Overall network design
