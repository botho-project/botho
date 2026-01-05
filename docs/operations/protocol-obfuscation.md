# Protocol Obfuscation Configuration Guide

This guide covers how to configure botho's pluggable transport system for protocol obfuscation, enabling traffic to resist deep packet inspection (DPI) and protocol-level blocking.

## Overview

Botho supports multiple transport types to help your traffic blend with common internet protocols:

| Transport | Appearance to DPI | NAT Traversal | Performance |
|-----------|-------------------|---------------|-------------|
| **Plain** | Custom P2P protocol | Limited | Best |
| **WebRTC** | Video call traffic | Excellent (ICE/STUN) | Good |
| **TLS Tunnel** | HTTPS traffic | Good | Good |

## Quick Start

### Enable Privacy-Focused Transports

Add to your `botho.toml` configuration:

```toml
[transport]
# Enable protocol obfuscation
enable_webrtc = true
enable_tls_tunnel = true

# Prefer privacy over performance
preference = "privacy"
```

### Verify Configuration

Check that transports are enabled:

```bash
botho config show transport
```

## Configuration Reference

### Transport Selection Preferences

The `preference` setting controls how transports are selected:

```toml
[transport]
# Options: "privacy", "performance", "compatibility"
preference = "privacy"
```

| Preference | Behavior |
|------------|----------|
| `privacy` | Prefers WebRTC, then TLS Tunnel, then Plain |
| `performance` | Prefers Plain, then TLS Tunnel, then WebRTC |
| `compatibility` | Prefers WebRTC (best NAT traversal) |

### Specific Transport Selection

Force a specific transport for all connections:

```toml
[transport]
# Force WebRTC for all connections
preference = { specific = "webrtc" }
```

### Full Configuration Example

```toml
[transport]
# Enable transports
enable_webrtc = true
enable_tls_tunnel = true

# Selection preference
preference = "privacy"

# Enable metrics-based transport improvement
enable_metrics = true

# Enable automatic fallback on connection failures
enable_fallback = true
max_fallback_attempts = 3

# Connection timeout (seconds)
connect_timeout_secs = 30

[transport.webrtc]
# STUN servers for NAT traversal
stun_servers = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
]

# ICE connection timeout (seconds)
ice_timeout_secs = 30

# Maximum ICE candidates to gather
max_candidates = 10

[transport.tls]
# Custom server name for SNI (optional)
# If not set, uses a random common domain
server_name = "cdn.example.com"

# Verify server certificates (set to false only for testing)
verify_certificates = true
```

## WebRTC Configuration

### STUN Server Configuration

STUN servers help discover your public IP address for NAT traversal:

```toml
[transport.webrtc]
stun_servers = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
    # Add additional STUN servers for redundancy
    "stun:stun.cloudflare.com:3478",
]
```

### Using TURN Servers

For restrictive networks that block direct peer connections, configure TURN relay servers:

```toml
[transport.webrtc]
stun_servers = ["stun:stun.l.google.com:19302"]

# TURN servers require authentication
[[transport.webrtc.turn_servers]]
url = "turn:turn.example.com:3478"
username = "your-username"
credential = "your-credential"
```

### ICE Timeout Configuration

Adjust timeout for slow networks:

```toml
[transport.webrtc]
# Increase timeout for high-latency networks
ice_timeout_secs = 60

# Limit candidates for faster connection (may reduce connectivity)
max_candidates = 5
```

## TLS Tunnel Configuration

### Server Name Indication (SNI)

The TLS tunnel can use custom SNI to blend with specific HTTPS traffic:

```toml
[transport.tls]
# Make traffic look like connections to a CDN
server_name = "cdn.cloudflare.com"
```

### Certificate Verification

```toml
[transport.tls]
# Production: always verify certificates
verify_certificates = true

# Custom CA certificates (PEM format)
# custom_ca_certs = ["/path/to/ca.pem"]
```

## Fallback Behavior

When a preferred transport fails, botho can automatically try alternatives:

```toml
[transport]
# Enable automatic fallback
enable_fallback = true

# Maximum attempts before giving up
max_fallback_attempts = 3
```

**Fallback order** (privacy preference):
1. WebRTC (if enabled and peer supports)
2. TLS Tunnel (if enabled and peer supports)
3. Plain (always available)

## Monitoring and Metrics

### Enable Transport Metrics

```toml
[transport]
enable_metrics = true
```

Metrics are exposed at the standard metrics endpoint:

```bash
curl http://localhost:9090/metrics | grep transport_
```

### Available Metrics

| Metric | Description |
|--------|-------------|
| `transport_connections_total` | Total connections by transport type |
| `transport_success_rate` | Success rate per transport |
| `transport_latency_seconds` | Connection latency histogram |
| `transport_fallback_total` | Number of fallback attempts |

## Troubleshooting

### WebRTC Connection Failures

**Symptoms**: WebRTC connections time out or fail to establish.

**Check NAT type**:
```bash
botho network nat-type
```

**Common NAT types and implications**:
| NAT Type | WebRTC Support |
|----------|----------------|
| Open | Excellent |
| Full Cone | Good |
| Restricted | May need TURN |
| Symmetric | Requires TURN relay |

**Solutions**:
1. Add TURN servers for symmetric NAT
2. Increase ICE timeout for slow networks
3. Check firewall allows UDP traffic

### Firewall Configuration

WebRTC requires UDP traffic. Configure your firewall:

```bash
# Allow STUN traffic
iptables -A OUTPUT -p udp --dport 3478 -j ACCEPT
iptables -A OUTPUT -p udp --dport 19302 -j ACCEPT

# Allow ephemeral ports for ICE
iptables -A OUTPUT -p udp --dport 49152:65535 -j ACCEPT
```

### TLS Tunnel Issues

**Symptoms**: TLS connections fail with certificate errors.

**Solutions**:
1. Ensure system time is correct (certificate validation requires accurate time)
2. Update CA certificates: `update-ca-certificates`
3. Check `verify_certificates` is not accidentally disabled in production

### Connection Timeout

**Symptoms**: All transport connections time out.

**Solutions**:
1. Increase `connect_timeout_secs`
2. Check network connectivity
3. Verify peer is reachable
4. Check for ISP-level blocking

### Debug Logging

Enable transport debug logs:

```bash
RUST_LOG=botho::network::transport=debug botho start
```

## Best Practices

### For Privacy

1. **Enable both WebRTC and TLS Tunnel** for maximum flexibility
2. **Use privacy preference** to prioritize obfuscated transports
3. **Enable fallback** to ensure connectivity when preferred transport fails
4. **Monitor metrics** to detect transport issues

### For Restricted Networks

1. **Configure TURN servers** for corporate firewalls
2. **Use TLS Tunnel** which is rarely blocked
3. **Set custom SNI** to match allowed domains

### For Performance

1. **Use performance preference** when obfuscation isn't needed
2. **Disable unused transports** to reduce connection overhead
3. **Monitor latency metrics** to identify slow transports

## Environment Variables

Override configuration with environment variables:

| Variable | Description |
|----------|-------------|
| `BOTHO_TRANSPORT_PREFERENCE` | Transport preference |
| `BOTHO_TRANSPORT_WEBRTC` | Enable WebRTC (`true`/`false`) |
| `BOTHO_TRANSPORT_TLS` | Enable TLS Tunnel (`true`/`false`) |
| `BOTHO_TRANSPORT_TIMEOUT` | Connection timeout in seconds |

Example:
```bash
BOTHO_TRANSPORT_PREFERENCE=privacy \
BOTHO_TRANSPORT_WEBRTC=true \
botho start
```

## See Also

- [Transport Architecture](../architecture/transport.md) - Technical details
- [Transport Security](../security/transport-security.md) - Security considerations
- [Network Architecture](../architecture/network.md) - Overall network design
- [Threat Model](../security/threat-model.md) - Security threat analysis
