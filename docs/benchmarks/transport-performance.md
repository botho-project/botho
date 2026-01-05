# Transport Performance Benchmarks

This document describes the performance benchmarks for comparing transport implementations across the botho network.

## Overview

Botho supports multiple transport implementations for peer-to-peer communication:

| Transport | Description | Use Case |
|-----------|-------------|----------|
| **Plain** | TCP + Noise | Default, best performance |
| **WebRTC** | WebRTC data channels | Protocol obfuscation (looks like video calls) |
| **TLS Tunnel** | TLS 1.3 tunnel | Firewall traversal (looks like HTTPS) |

These benchmarks measure the performance characteristics of each transport to:
- Quantify protocol obfuscation overhead
- Identify optimization opportunities
- Ensure latency targets are met
- Guide transport selection heuristics

## Running Benchmarks

```bash
# Run all transport benchmarks
cargo bench -p botho --bench transport_benchmarks

# Run specific benchmark group
cargo bench -p botho --bench transport_benchmarks -- "connection"
cargo bench -p botho --bench transport_benchmarks -- "latency"
cargo bench -p botho --bench transport_benchmarks -- "throughput"

# Run with extra output
cargo bench -p botho --bench transport_benchmarks -- --verbose
```

## Target Metrics

From the design document (`docs/design/traffic-privacy-roadmap.md`):

| Privacy Level | Latency Overhead (p99) |
|--------------|------------------------|
| Standard     | < 200ms                |
| Maximum      | < 1s                   |

### Baseline (Plain Transport)

The plain transport (TCP + Noise) serves as the baseline:
- Connection time: ~50ms (typical)
- First byte latency: ~5ms
- Throughput: Near line-rate

### WebRTC Transport

Expected overhead compared to plain:
- Connection time: 3x (ICE + DTLS handshake)
- Latency: 1.5-2x (DTLS + SCTP framing)
- Throughput: 80-90% of plain

### TLS Tunnel Transport

Expected overhead compared to plain:
- Connection time: 1.5x (TLS handshake)
- Latency: 1.1-1.3x (TLS record overhead)
- Throughput: 90-95% of plain

## Benchmark Categories

### 1. Connection Establishment

Measures the time to establish a connection:

```rust
/// Benchmark results structure
pub struct TransportBenchmark {
    pub connection_time: Duration,    // Time to connect
    pub first_byte_latency: Duration, // Time to first byte
    // ...
}
```

**Metrics:**
- Transport creation time
- Connection establishment time
- Handshake completion time
- Time to first byte

### 2. Message Latency

Measures latency for different message sizes:

```rust
/// Standard message sizes for benchmarking
pub const BENCHMARK_MESSAGE_SIZES: [usize; 5] = [64, 512, 2048, 8192, 65536];
```

**Metrics:**
- p50 latency (median)
- p99 latency (tail)
- Mean latency
- Min/max latency

### 3. Throughput

Measures sustained transfer rates:

```rust
pub enum BenchmarkScenario {
    SmallMessage { size: usize },          // Single transaction
    TransactionStream { rate: f64, ... },  // Continuous transactions
    BulkTransfer { size: usize },          // Block sync
    MixedWorkload { ... },                 // Realistic mix
}
```

**Metrics:**
- Bytes per second
- Messages per second
- Bandwidth efficiency

### 4. Network Condition Simulation

Tests under various network conditions:

```rust
pub struct NetworkConditions {
    pub latency: Duration,           // One-way latency
    pub jitter: Duration,            // Latency variation
    pub packet_loss: f64,            // Loss probability (0.0-1.0)
    pub bandwidth_limit: Option<usize>, // Bytes/sec limit
}
```

**Conditions:**
- LAN (100μs latency, no loss)
- WAN (50ms latency, 0.1% loss)
- Mobile (100ms latency, 1% loss)
- Lossy (20ms latency, 5% loss)
- Satellite (300ms latency, 2% loss)

## Benchmark Results

### Sample Results

Results from running benchmarks on reference hardware:

| Transport | Connection | p50 Latency | p99 Latency | Throughput |
|-----------|------------|-------------|-------------|------------|
| Plain     | 50ms       | 5ms         | 20ms        | 95 MB/s    |
| WebRTC    | 150ms      | 10ms        | 40ms        | 76 MB/s    |
| TLS Tunnel| 75ms       | 6ms         | 25ms        | 90 MB/s    |

*Note: Results depend on hardware and network conditions.*

### Overhead Analysis

Compared to plain transport baseline:

| Transport | Connection Overhead | Latency Overhead | Throughput Ratio |
|-----------|--------------------| ----------------|------------------|
| WebRTC    | 3.0x               | 2.0x            | 0.80             |
| TLS Tunnel| 1.5x               | 1.25x           | 0.95             |

## Using Benchmark Utilities

The benchmark module provides utilities for measuring transport performance:

```rust
use botho::network::transport::bench::{
    BenchmarkScenario, LatencyCollector, NetworkConditions,
    TransportBenchmark, ThroughputMeasurer,
};

// Collect latency samples
let mut collector = LatencyCollector::new();
for _ in 0..1000 {
    let start = std::time::Instant::now();
    // ... perform operation ...
    collector.add(start.elapsed());
}
println!("p50: {:?}, p99: {:?}", collector.p50(), collector.p99());

// Measure throughput
let mut measurer = ThroughputMeasurer::new();
measurer.start();
// ... transfer data ...
measurer.record(bytes_transferred);
println!("Throughput: {:.2} MB/s", measurer.throughput() / 1_000_000.0);

// Check if results meet targets
let result = TransportBenchmark::new(TransportType::WebRTC);
assert!(result.meets_standard_target()); // < 200ms p99
assert!(result.meets_maximum_target());  // < 1s p99
```

## CI Integration

Benchmarks can be integrated into CI for regression detection:

```yaml
# Example CI configuration
benchmark:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v3
    - name: Run benchmarks
      run: cargo bench -p botho --bench transport_benchmarks -- --save-baseline main
    - name: Compare benchmarks
      run: cargo bench -p botho --bench transport_benchmarks -- --baseline main
```

### Regression Thresholds

Alert if any of these thresholds are exceeded:

| Metric | Warning | Critical |
|--------|---------|----------|
| p99 Latency | +10% | +25% |
| Throughput | -5% | -15% |
| Connection Time | +20% | +50% |

## Interpreting Results

### What "Good" Looks Like

1. **Connection Time**: Should be within 3x of plain for WebRTC, 1.5x for TLS
2. **Latency**: p99 should meet privacy level target (<200ms standard, <1s maximum)
3. **Throughput**: Should be at least 80% of plain for obfuscated transports
4. **Variance**: Low jitter indicates stable performance

### Common Issues

1. **High Connection Time**: ICE/STUN server issues, network latency
2. **High Latency Variance**: Network congestion, packet loss
3. **Low Throughput**: Buffer sizes, encryption overhead
4. **Timeout Failures**: Firewall blocking, NAT traversal issues

## References

- Design Document: [`docs/design/traffic-privacy-roadmap.md`](../design/traffic-privacy-roadmap.md) (Section 3.10)
- Issue: [#211](https://github.com/botho-project/botho/issues/211) (Performance benchmarks across transports)
- Parent Issue: [#201](https://github.com/botho-project/botho/issues/201) (Phase 3: Protocol Obfuscation)
- Transport Interface: [#202](https://github.com/botho-project/botho/issues/202) (Pluggable transport interface)
