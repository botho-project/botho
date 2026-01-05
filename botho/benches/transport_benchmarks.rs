// Copyright (c) 2024 Botho Foundation

//! Performance benchmarks comparing transport implementations.
//!
//! Run with: cargo bench -p botho --bench transport_benchmarks
//!
//! These benchmarks measure the performance characteristics of different
//! transport implementations:
//! - **Plain**: Standard TCP + Noise (baseline)
//! - **WebRTC**: WebRTC data channels (protocol obfuscation)
//! - **TLS Tunnel**: TLS tunnel (future implementation)
//!
//! # Target Metrics (from design doc)
//!
//! | Privacy Level | Latency Overhead |
//! |--------------|------------------|
//! | Standard     | < 200ms p99      |
//! | Maximum      | < 1s p99         |
//!
//! # Sections
//!
//! 1. Connection establishment benchmarks
//! 2. Message latency benchmarks
//! 3. Throughput benchmarks
//! 4. Network condition simulation
//! 5. Full transport comparison
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3.10)
//! - Issue: #211 (Performance benchmarks across transports)

use std::{
    io::Cursor,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use botho::network::transport::{
    bench::{
        BenchmarkScenario, LatencyCollector, NetworkConditions, ThroughputMeasurer,
        TransportBenchmark, BENCHMARK_MESSAGE_SIZES,
    },
    PlainTransport, PluggableTransport, TransportType, WebRtcTransport,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::RngCore;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

// =============================================================================
// Benchmark Helpers
// =============================================================================

/// Generate a random payload of the specified size.
fn random_payload(size: usize) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let mut payload = vec![0u8; size];
    rng.fill_bytes(&mut payload);
    payload
}

/// A mock connection for benchmarking transport overhead without network I/O.
///
/// This allows us to measure the pure transport layer overhead without
/// network variability affecting the results.
#[derive(Debug)]
struct MockConnection {
    read_buf: Cursor<Vec<u8>>,
    write_buf: Vec<u8>,
    /// Simulated one-way latency (for future use in delay simulation)
    #[allow(dead_code)]
    latency: Duration,
    /// Simulated latency jitter (for future use in delay simulation)
    #[allow(dead_code)]
    jitter: Duration,
}

impl MockConnection {
    fn new(data: Vec<u8>) -> Self {
        Self {
            read_buf: Cursor::new(data),
            write_buf: Vec::new(),
            latency: Duration::ZERO,
            jitter: Duration::ZERO,
        }
    }

    fn with_conditions(data: Vec<u8>, conditions: &NetworkConditions) -> Self {
        Self {
            read_buf: Cursor::new(data),
            write_buf: Vec::new(),
            latency: conditions.latency,
            jitter: conditions.jitter,
        }
    }

    fn written(&self) -> &[u8] {
        &self.write_buf
    }
}

impl AsyncRead for MockConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let pos = self.read_buf.position() as usize;
        let data = self.read_buf.get_ref();
        let remaining = &data[pos..];
        let to_read = std::cmp::min(remaining.len(), buf.remaining());
        buf.put_slice(&remaining[..to_read]);
        self.read_buf.set_position((pos + to_read) as u64);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MockConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.write_buf.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// =============================================================================
// Section 1: Connection Establishment Benchmarks
// =============================================================================

/// Benchmark transport creation time.
fn bench_transport_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transport creation");

    group.bench_function("plain", |b| {
        b.iter(|| {
            let transport = PlainTransport::new();
            black_box(transport)
        })
    });

    group.bench_function("webrtc", |b| {
        b.iter(|| {
            let transport = WebRtcTransport::with_defaults();
            black_box(transport)
        })
    });

    group.finish();
}

/// Benchmark transport type metadata access.
fn bench_transport_metadata(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transport metadata");

    let plain = PlainTransport::new();
    let webrtc = WebRtcTransport::with_defaults();

    group.bench_function("plain_type", |b| {
        b.iter(|| black_box(plain.transport_type()))
    });

    group.bench_function("plain_name", |b| b.iter(|| black_box(plain.name())));

    group.bench_function("plain_is_available", |b| {
        b.iter(|| black_box(plain.is_available()))
    });

    group.bench_function("webrtc_type", |b| b.iter(|| black_box(webrtc.ice_config())));

    group.finish();
}

/// Benchmark TransportType operations.
fn bench_transport_type_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("TransportType operations");

    group.bench_function("all_types", |b| b.iter(|| black_box(TransportType::all())));

    group.bench_function("name", |b| {
        b.iter(|| black_box(TransportType::WebRTC.name()))
    });

    group.bench_function("description", |b| {
        b.iter(|| black_box(TransportType::WebRTC.description()))
    });

    group.bench_function("is_obfuscated", |b| {
        b.iter(|| black_box(TransportType::WebRTC.is_obfuscated()))
    });

    group.bench_function("setup_overhead", |b| {
        b.iter(|| black_box(TransportType::WebRTC.setup_overhead()))
    });

    group.bench_function("parse_from_str", |b| {
        b.iter(|| black_box("webrtc".parse::<TransportType>()))
    });

    group.finish();
}

// =============================================================================
// Section 2: Message Latency Benchmarks
// =============================================================================

/// Benchmark latency collector operations.
fn bench_latency_collector(c: &mut Criterion) {
    let mut group = c.benchmark_group("Latency collector");

    // Adding samples
    group.bench_function("add_sample", |b| {
        let mut collector = LatencyCollector::with_capacity(10000);
        b.iter(|| {
            collector.add(Duration::from_micros(100));
            black_box(&collector);
        })
    });

    // Computing percentiles
    let mut collector = LatencyCollector::with_capacity(1000);
    for i in 0..1000 {
        collector.add(Duration::from_micros(i));
    }

    group.bench_function("p50_1000_samples", |b| {
        b.iter(|| black_box(collector.p50()))
    });

    group.bench_function("p99_1000_samples", |b| {
        b.iter(|| black_box(collector.p99()))
    });

    group.bench_function("mean_1000_samples", |b| {
        b.iter(|| black_box(collector.mean()))
    });

    group.finish();
}

/// Benchmark message preparation for different sizes.
fn bench_message_preparation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Message preparation");

    for size in BENCHMARK_MESSAGE_SIZES.iter() {
        let payload = random_payload(*size);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::new("size", size), &payload, |b, payload| {
            b.iter(|| {
                // Simulate message preparation overhead
                let mut prepared = payload.clone();
                prepared.extend_from_slice(&[0u8; 32]); // Add header
                black_box(prepared)
            })
        });
    }

    group.finish();
}

/// Benchmark mock connection read/write latency.
fn bench_mock_connection_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("Mock connection latency");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    for size in BENCHMARK_MESSAGE_SIZES.iter() {
        let payload = random_payload(*size);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::new("write", size), &payload, |b, payload| {
            b.iter(|| {
                rt.block_on(async {
                    use tokio::io::AsyncWriteExt;
                    let mut conn = MockConnection::new(vec![]);
                    conn.write_all(payload).await.unwrap();
                    black_box(conn.written().len())
                })
            })
        });

        group.bench_with_input(BenchmarkId::new("read", size), &payload, |b, payload| {
            b.iter(|| {
                rt.block_on(async {
                    use tokio::io::AsyncReadExt;
                    let mut conn = MockConnection::new(payload.clone());
                    let mut buf = vec![0u8; payload.len()];
                    conn.read_exact(&mut buf).await.unwrap();
                    black_box(buf.len())
                })
            })
        });
    }

    group.finish();
}

// =============================================================================
// Section 3: Throughput Benchmarks
// =============================================================================

/// Benchmark throughput measurer operations.
fn bench_throughput_measurer(c: &mut Criterion) {
    let mut group = c.benchmark_group("Throughput measurer");

    group.bench_function("start", |b| {
        let mut measurer = ThroughputMeasurer::new();
        b.iter(|| {
            measurer.start();
            black_box(&measurer);
        })
    });

    group.bench_function("record", |b| {
        let mut measurer = ThroughputMeasurer::new();
        measurer.start();
        b.iter(|| {
            measurer.record(1000);
            black_box(&measurer);
        })
    });

    group.bench_function("throughput_calculation", |b| {
        let mut measurer = ThroughputMeasurer::new();
        measurer.start();
        for _ in 0..1000 {
            measurer.record(1000);
        }
        b.iter(|| black_box(measurer.throughput()))
    });

    group.finish();
}

/// Benchmark bulk data transfer simulation.
fn bench_bulk_transfer(c: &mut Criterion) {
    let mut group = c.benchmark_group("Bulk transfer");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Simulate different transfer sizes
    let transfer_sizes = [1024, 8192, 65536, 262144, 1048576]; // 1KB to 1MB

    for size in transfer_sizes.iter() {
        let payload = random_payload(*size);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(
            BenchmarkId::new("mock_transfer", size),
            &payload,
            |b, payload| {
                b.iter(|| {
                    rt.block_on(async {
                        use tokio::io::AsyncWriteExt;
                        let mut conn = MockConnection::new(vec![]);
                        conn.write_all(payload).await.unwrap();
                        black_box(conn.written().len())
                    })
                })
            },
        );
    }

    group.finish();
}

// =============================================================================
// Section 4: Network Condition Simulation
// =============================================================================

/// Benchmark network conditions configuration.
fn bench_network_conditions(c: &mut Criterion) {
    let mut group = c.benchmark_group("Network conditions");

    group.bench_function("lan", |b| b.iter(|| black_box(NetworkConditions::lan())));

    group.bench_function("wan", |b| b.iter(|| black_box(NetworkConditions::wan())));

    group.bench_function("mobile", |b| {
        b.iter(|| black_box(NetworkConditions::mobile()))
    });

    group.bench_function("lossy", |b| {
        b.iter(|| black_box(NetworkConditions::lossy()))
    });

    group.bench_function("satellite", |b| {
        b.iter(|| black_box(NetworkConditions::satellite()))
    });

    group.bench_function("rtt_calculation", |b| {
        let conditions = NetworkConditions::wan();
        b.iter(|| black_box(conditions.rtt()))
    });

    group.finish();
}

/// Benchmark mock connection with simulated network conditions.
fn bench_mock_with_conditions(c: &mut Criterion) {
    let mut group = c.benchmark_group("Mock with conditions");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let payload = random_payload(512);
    let conditions = [
        ("lan", NetworkConditions::lan()),
        ("wan", NetworkConditions::wan()),
        ("mobile", NetworkConditions::mobile()),
        ("lossy", NetworkConditions::lossy()),
    ];

    for (name, cond) in conditions.iter() {
        group.bench_with_input(
            BenchmarkId::new("write", name),
            &(payload.clone(), cond),
            |b, (payload, cond)| {
                b.iter(|| {
                    rt.block_on(async {
                        use tokio::io::AsyncWriteExt;
                        let mut conn = MockConnection::with_conditions(vec![], cond);
                        conn.write_all(payload).await.unwrap();
                        black_box(conn.written().len())
                    })
                })
            },
        );
    }

    group.finish();
}

// =============================================================================
// Section 5: Benchmark Scenario Operations
// =============================================================================

/// Benchmark scenario creation and metadata.
fn bench_benchmark_scenarios(c: &mut Criterion) {
    let mut group = c.benchmark_group("Benchmark scenarios");

    group.bench_function("typical_transaction", |b| {
        b.iter(|| black_box(BenchmarkScenario::typical_transaction()))
    });

    group.bench_function("block_sync", |b| {
        b.iter(|| black_box(BenchmarkScenario::block_sync()))
    });

    group.bench_function("realistic", |b| {
        b.iter(|| black_box(BenchmarkScenario::realistic()))
    });

    let scenario = BenchmarkScenario::typical_transaction();
    group.bench_function("name", |b| b.iter(|| black_box(scenario.name())));

    group.bench_function("message_sizes", |b| {
        b.iter(|| black_box(scenario.message_sizes()))
    });

    group.finish();
}

/// Benchmark TransportBenchmark result operations.
fn bench_transport_benchmark_results(c: &mut Criterion) {
    let mut group = c.benchmark_group("Benchmark results");

    group.bench_function("new", |b| {
        b.iter(|| black_box(TransportBenchmark::new(TransportType::Plain)))
    });

    let mut bench = TransportBenchmark::new(TransportType::Plain);
    bench.latency_p99 = Duration::from_millis(100);

    group.bench_function("meets_standard_target", |b| {
        b.iter(|| black_box(bench.meets_standard_target()))
    });

    group.bench_function("meets_maximum_target", |b| {
        b.iter(|| black_box(bench.meets_maximum_target()))
    });

    let baseline = TransportBenchmark {
        transport: TransportType::Plain,
        connection_time: Duration::from_millis(100),
        first_byte_latency: Duration::from_millis(10),
        throughput: 100_000_000.0,
        latency_p50: Duration::from_millis(5),
        latency_p99: Duration::from_millis(20),
        cpu_usage: 0.05,
        memory_bytes: 1024 * 1024,
        sample_count: 1000,
    };

    let webrtc = TransportBenchmark {
        transport: TransportType::WebRTC,
        connection_time: Duration::from_millis(300),
        first_byte_latency: Duration::from_millis(30),
        throughput: 80_000_000.0,
        latency_p50: Duration::from_millis(15),
        latency_p99: Duration::from_millis(60),
        cpu_usage: 0.08,
        memory_bytes: 2 * 1024 * 1024,
        sample_count: 1000,
    };

    group.bench_function("overhead_calculation", |b| {
        b.iter(|| black_box(webrtc.overhead_vs(&baseline)))
    });

    group.finish();
}

// =============================================================================
// Section 6: Full Transport Comparison (Simulated)
// =============================================================================

/// Simulate end-to-end transport benchmarks.
///
/// Note: This uses mock connections since real network connections would
/// have too much variability for benchmarking. The focus is on measuring
/// the transport layer overhead, not network I/O.
fn bench_simulated_transport_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transport comparison (simulated)");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let payload = random_payload(512); // Typical transaction size

    // Simulate plain transport (baseline)
    group.bench_function("plain_roundtrip", |b| {
        b.iter(|| {
            rt.block_on(async {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};

                // Simulate plain transport overhead
                let _transport = PlainTransport::new();

                let mut conn = MockConnection::new(payload.clone());
                let mut buf = vec![0u8; payload.len()];

                // Write then read back
                conn.write_all(&payload).await.unwrap();
                conn.read_exact(&mut buf).await.unwrap();

                black_box(buf.len())
            })
        })
    });

    // Simulate WebRTC transport (with additional overhead)
    group.bench_function("webrtc_roundtrip", |b| {
        b.iter(|| {
            rt.block_on(async {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};

                // Simulate WebRTC transport overhead (DTLS, SCTP framing)
                let _transport = WebRtcTransport::with_defaults();

                // Add DTLS/SCTP header overhead
                let mut framed_payload = vec![0u8; 32]; // Header overhead
                framed_payload.extend_from_slice(&payload);

                let mut conn = MockConnection::new(framed_payload.clone());
                let mut buf = vec![0u8; framed_payload.len()];

                conn.write_all(&framed_payload).await.unwrap();
                conn.read_exact(&mut buf).await.unwrap();

                black_box(buf.len())
            })
        })
    });

    group.finish();
}

/// Benchmark simulated latency measurements.
fn bench_simulated_latency_measurements(c: &mut Criterion) {
    let mut group = c.benchmark_group("Latency measurement simulation");

    // Measure the overhead of collecting latency samples
    group.bench_function("collect_1000_samples", |b| {
        b.iter(|| {
            let mut collector = LatencyCollector::with_capacity(1000);
            for _ in 0..1000 {
                let start = std::time::Instant::now();
                // Simulate some work
                black_box(random_payload(64));
                collector.add(start.elapsed());
            }
            black_box((collector.p50(), collector.p99()))
        })
    });

    // Measure the overhead of generating benchmark results
    group.bench_function("generate_benchmark_result", |b| {
        b.iter(|| {
            let mut bench = TransportBenchmark::new(TransportType::Plain);
            bench.connection_time = Duration::from_millis(50);
            bench.first_byte_latency = Duration::from_millis(5);
            bench.throughput = 100_000_000.0;
            bench.latency_p50 = Duration::from_millis(5);
            bench.latency_p99 = Duration::from_millis(20);
            bench.sample_count = 1000;
            black_box(bench)
        })
    });

    group.finish();
}

// =============================================================================
// Section 7: Serialization Benchmarks
// =============================================================================

/// Benchmark serialization of benchmark results.
fn bench_result_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("Result serialization");

    let bench = TransportBenchmark {
        transport: TransportType::WebRTC,
        connection_time: Duration::from_millis(300),
        first_byte_latency: Duration::from_millis(30),
        throughput: 80_000_000.0,
        latency_p50: Duration::from_millis(15),
        latency_p99: Duration::from_millis(60),
        cpu_usage: 0.08,
        memory_bytes: 2 * 1024 * 1024,
        sample_count: 1000,
    };

    group.bench_function("serialize_json", |b| {
        b.iter(|| black_box(serde_json::to_string(&bench).unwrap()))
    });

    let json = serde_json::to_string(&bench).unwrap();
    group.bench_function("deserialize_json", |b| {
        b.iter(|| black_box(serde_json::from_str::<TransportBenchmark>(&json).unwrap()))
    });

    group.finish();
}

/// Benchmark network conditions serialization.
fn bench_conditions_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("Conditions serialization");

    let conditions = NetworkConditions::mobile();

    group.bench_function("serialize_json", |b| {
        b.iter(|| black_box(serde_json::to_string(&conditions).unwrap()))
    });

    let json = serde_json::to_string(&conditions).unwrap();
    group.bench_function("deserialize_json", |b| {
        b.iter(|| black_box(serde_json::from_str::<NetworkConditions>(&json).unwrap()))
    });

    group.finish();
}

// =============================================================================
// Criterion Configuration
// =============================================================================

criterion_group!(
    connection_benches,
    bench_transport_creation,
    bench_transport_metadata,
    bench_transport_type_ops,
);

criterion_group!(
    latency_benches,
    bench_latency_collector,
    bench_message_preparation,
    bench_mock_connection_latency,
);

criterion_group!(
    throughput_benches,
    bench_throughput_measurer,
    bench_bulk_transfer,
);

criterion_group!(
    network_condition_benches,
    bench_network_conditions,
    bench_mock_with_conditions,
);

criterion_group!(
    scenario_benches,
    bench_benchmark_scenarios,
    bench_transport_benchmark_results,
);

criterion_group!(
    comparison_benches,
    bench_simulated_transport_comparison,
    bench_simulated_latency_measurements,
);

criterion_group!(
    serialization_benches,
    bench_result_serialization,
    bench_conditions_serialization,
);

criterion_main!(
    connection_benches,
    latency_benches,
    throughput_benches,
    network_condition_benches,
    scenario_benches,
    comparison_benches,
    serialization_benches,
);
