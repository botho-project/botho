# Transport Layer Security Considerations

This document describes the security properties, assumptions, and limitations of botho's pluggable transport system.

## Security Goals

The transport layer aims to achieve:

| Goal | Description |
|------|-------------|
| **Confidentiality** | All traffic encrypted; content not visible to observers |
| **Integrity** | Tampering detected; modified packets rejected |
| **Authentication** | Peer identity verified; MITM attacks prevented |
| **Protocol Obfuscation** | Traffic indistinguishable from legitimate protocols |
| **Censorship Resistance** | Traffic not easily blocked by DPI |

## What We Protect Against

### Deep Packet Inspection (DPI)

| Attack | Mitigation |
|--------|------------|
| Protocol identification | WebRTC/TLS transports mimic legitimate traffic |
| Payload inspection | All transports provide encryption |
| Pattern matching | Variable padding and timing (Phase 2) |

### Network-Level Attacks

| Attack | Mitigation |
|--------|------------|
| Passive eavesdropping | Strong encryption (DTLS 1.3, TLS 1.3, Noise) |
| Active MITM | Certificate verification, peer authentication |
| Replay attacks | Session-specific keys, sequence numbers |
| Downgrade attacks | Authenticated transport negotiation |

### Timing and Traffic Analysis

| Attack | Mitigation |
|--------|------------|
| Message timing correlation | Timing jitter (configurable) |
| Traffic volume analysis | Message padding to fixed sizes |
| Activity detection | Cover traffic generation |

## What We Don't Protect Against

### Limitations

| Limitation | Description |
|------------|-------------|
| **Endpoint compromise** | If your device is compromised, traffic encryption doesn't help |
| **Global adversary** | Traffic analysis across multiple network segments |
| **Metadata leakage** | IP addresses of peers you connect to |
| **Timing with global view** | End-to-end correlation with global network visibility |
| **Tor-style anonymity** | Transport obfuscation != sender anonymity (see Onion Gossip) |

### Trust Assumptions

| Component | Trust Assumption |
|-----------|------------------|
| **STUN servers** | Learn your IP address; can't see traffic content |
| **TURN servers** | See encrypted traffic; can correlate connections |
| **DNS** | May reveal STUN/TURN server lookups |
| **System CA store** | TLS certificate validation trusts system CAs |

## Transport-Specific Security

### Plain Transport (TCP + Noise)

**Encryption**: Noise Protocol (XX handshake pattern)

**Security properties**:
- Forward secrecy via ephemeral keys
- Mutual authentication
- No metadata protection (clearly botho traffic)

**Risks**:
- Easily identified by DPI as custom P2P protocol
- May be blocked in restrictive environments

### WebRTC Transport

**Encryption**: DTLS 1.3 (mandatory for WebRTC)

**Security properties**:
- Same encryption as video calling applications
- Traffic indistinguishable from Google Meet, Discord, etc.
- Built-in NAT traversal reduces need for relay trust

**Certificate handling**:
```rust
// Ephemeral certificates generated per session
pub struct EphemeralCertificate {
    certificate: Certificate,
    private_key: PrivateKey,
    fingerprint: CertificateFingerprint,
    created_at: Instant,
}
```

**Fingerprint exchange**: Certificate fingerprints exchanged via signaling channel, verified during DTLS handshake.

**Risks**:
- STUN servers see your IP address
- TURN servers can correlate connections (but can't see content)
- ICE candidates may reveal local network topology

### TLS Tunnel Transport

**Encryption**: TLS 1.3

**Security properties**:
- Traffic looks like HTTPS
- SNI can mimic legitimate domains
- Widely permitted through firewalls

**Certificate verification**:
```rust
pub struct TlsConfig {
    /// Whether to verify server certificates
    pub verify_certificates: bool,

    /// Custom CA certificates (optional)
    pub custom_ca_certs: Option<Vec<String>>,
}
```

**Risks**:
- Self-signed certificates require `verify_certificates: false`
- SNI reveals intended domain (use domain fronting cautiously)
- TLS fingerprinting may identify non-browser clients

## Transport Negotiation Security

### Negotiation Protocol

Transport negotiation is authenticated to prevent downgrade attacks:

```
┌─────────┐                              ┌─────────┐
│Initiator│                              │Responder│
└────┬────┘                              └────┬────┘
     │                                        │
     │  HELLO(caps, preferred, signature)     │
     │────────────────────────────────────────│
     │                                        │
     │       SELECT(chosen, signature)        │
     │────────────────────────────────────────│
     │                                        │
```

**Properties**:
- Messages signed with peer's identity key
- Prevents MITM from forcing weaker transport
- Version negotiation prevents protocol downgrades

### Downgrade Prevention

An attacker cannot force use of a weaker transport:

1. Capabilities are signed by peer identity
2. Selection is cryptographically bound to capabilities
3. Mismatch causes connection rejection

## Session ID Security

Signaling sessions use random identifiers:

```rust
pub const SESSION_ID_LEN: usize = 16;

pub struct SessionId([u8; SESSION_ID_LEN]);

impl SessionId {
    pub fn generate() -> Self {
        let mut bytes = [0u8; SESSION_ID_LEN];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }
}
```

**Properties**:
- 128-bit random IDs prevent prediction
- Session IDs not reused across connections
- Timeout cleanup prevents resource exhaustion

## Resource Exhaustion Protection

### Signaling Limits

```rust
pub const MAX_SESSIONS_PER_PEER: usize = 5;
pub const MAX_ICE_CANDIDATES_PER_SESSION: usize = 20;
pub const MAX_SDP_SIZE: usize = 10 * 1024;  // 10 KB
pub const MAX_ICE_CANDIDATE_SIZE: usize = 512;
pub const DEFAULT_SIGNALING_TIMEOUT_SECS: u64 = 60;
```

### Rate Limiting

Connection attempts are rate-limited per peer to prevent DoS:

```rust
// Per-peer rate limiting for relay traffic
relay_limits: HashMap<PeerId, RateLimiter>
```

## DTLS Certificate Security

### Ephemeral Certificates

WebRTC uses ephemeral certificates:

```rust
impl EphemeralCertificate {
    pub fn generate() -> Result<Self, DtlsError> {
        // Generate new keypair
        let key_pair = KeyPair::generate()?;

        // Self-signed certificate
        let certificate = Certificate::from_params(
            CertificateParams {
                subject_alt_names: vec![],
                distinguished_name: DistinguishedName::new(),
                not_before: Utc::now(),
                not_after: Utc::now() + DEFAULT_CERTIFICATE_LIFETIME,
                // ...
            },
            &key_pair,
        )?;

        Ok(Self { certificate, private_key: key_pair, /* ... */ })
    }
}
```

**Security properties**:
- No long-term certificate compromise risk
- New identity per session (unless explicitly reused)
- Fingerprint verification via signaling prevents MITM

### Browser Cipher Suites

We use the same cipher suites as browsers for better obfuscation:

```rust
pub const BROWSER_CIPHER_SUITES: &[CipherSuite] = &[
    CipherSuite::TLS13_AES_256_GCM_SHA384,
    CipherSuite::TLS13_AES_128_GCM_SHA256,
    CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
];
```

## ICE Security

### NAT Type Detection

NAT type affects security posture:

| NAT Type | Security Implication |
|----------|---------------------|
| Open | Direct connections, lower relay trust |
| Full Cone | Direct connections possible |
| Restricted | May need relay, more metadata exposure |
| Symmetric | Requires TURN, relay sees connection patterns |

### STUN Server Trust

STUN servers learn your public IP address but:
- Cannot see traffic content (only reflexive address discovery)
- Can be self-hosted for zero trust
- Multiple servers provide redundancy

```rust
// Default STUN servers (public, operated by Google)
stun_servers: vec![
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
]
```

**Recommendation**: For maximum privacy, run your own STUN server.

### TURN Server Trust

TURN servers relay encrypted traffic but:
- Can see connection patterns (who connects to whom)
- Cannot see message content (encrypted by DTLS)
- Should be trusted infrastructure or self-hosted

**Recommendation**: Only use TURN servers you operate or trust.

## Error Handling Security

Errors are designed to not leak sensitive information:

```rust
impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Generic messages, no internal details
            TransportError::HandshakeFailed(_) =>
                write!(f, "handshake failed"),
            TransportError::NegotiationFailed(_) =>
                write!(f, "negotiation failed"),
            // ...
        }
    }
}
```

## Fingerprinting Resistance

### Traffic Pattern Tests

We verify traffic is statistically indistinguishable from legitimate protocols:

```rust
pub fn kolmogorov_smirnov(sample1: &[f64], sample2: &[f64]) -> KsTestResult {
    // Two-sample K-S test for distribution similarity
    // p-value > 0.05 means samples likely from same distribution
}
```

**Test criteria**:
- Packet size distribution matches reference protocol
- Inter-arrival time distribution matches reference
- Flow characteristics match reference

### What We Test Against

| Reference Protocol | Transport |
|-------------------|-----------|
| Google Meet | WebRTC |
| HTTPS to CDN | TLS Tunnel |

## Recommendations

### For Maximum Privacy

1. **Use WebRTC transport** - Traffic looks like video calls
2. **Enable cover traffic** (Phase 2) - Hide activity patterns
3. **Use self-hosted STUN/TURN** - Minimize third-party trust
4. **Enable timing jitter** - Reduce correlation attacks
5. **Combine with Onion Gossip** (Phase 1) - Hide transaction origin

### For Restrictive Networks

1. **Use TLS Tunnel** - Rarely blocked, looks like HTTPS
2. **Configure domain fronting** - If needed (use cautiously)
3. **Add TURN servers** - For symmetric NAT environments

### For Operators

1. **Monitor transport metrics** - Detect failures early
2. **Rotate certificates** - Don't reuse across sessions
3. **Update cipher suites** - Match current browser defaults
4. **Log responsibly** - Don't log sensitive connection details

## Security Audit Status

| Component | Audit Status | Last Audited |
|-----------|--------------|--------------|
| Transport negotiation | Pending | - |
| WebRTC implementation | Pending | - |
| TLS tunnel implementation | Pending | - |
| DTLS certificate handling | Pending | - |
| Signaling protocol | Pending | - |

## Known Issues

1. **TLS fingerprinting**: TLS client fingerprint may differ from browsers
2. **ICE candidate leakage**: Local IPs may be exposed in candidates
3. **DNS leakage**: STUN/TURN hostname resolution may leak

## References

- [WebRTC Security Architecture (W3C)](https://www.w3.org/TR/webrtc/#security-considerations)
- [DTLS 1.3 (RFC 9147)](https://datatracker.ietf.org/doc/html/rfc9147)
- [TLS 1.3 (RFC 8446)](https://datatracker.ietf.org/doc/html/rfc8446)
- [ICE (RFC 8445)](https://datatracker.ietf.org/doc/html/rfc8445)
- [STUN (RFC 5389)](https://datatracker.ietf.org/doc/html/rfc5389)
- [Noise Protocol Framework](https://noiseprotocol.org/noise.html)

## See Also

- [Transport Architecture](../architecture/transport.md) - Technical design
- [Protocol Obfuscation Configuration](../operations/protocol-obfuscation.md) - User guide
- [Threat Model](./threat-model.md) - Overall threat analysis
- [Traffic Privacy Roadmap](../design/traffic-privacy-roadmap.md) - Design document
