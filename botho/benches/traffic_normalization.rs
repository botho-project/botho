// Copyright (c) 2024 Botho Foundation

//! Performance benchmarks for traffic normalization (Phase 2).
//!
//! Run with: cargo bench -p botho --bench traffic_normalization
//!
//! These benchmarks measure the performance overhead of:
//! - Message padding to fixed bucket sizes
//! - Timing jitter generation
//! - Constant-rate transmitter queue operations
//! - Cover traffic generation
//! - Full stack with all features enabled
//!
//! # Target Metrics (from issue #187)
//!
//! | Metric | Target (Standard) | Target (Maximum) |
//! |--------|------------------|------------------|
//! | Latency overhead | < 200ms p99 | < 1s p99 |
//! | Bandwidth overhead | < 50% | < 2x |
//! | CPU overhead | < 5% | < 10% |
//! | Memory overhead | < 10MB | < 50MB |

use botho::network::privacy::{
    cover::{CoverMessage, CoverTrafficGenerator},
    normalizer::{NormalizerConfig, TrafficNormalizer, PADDING_BUCKETS},
    transmitter::{ConstantRateConfig, ConstantRateTransmitter, OutgoingMessage},
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::RngCore;

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

/// Typical transaction sizes for testing.
const PAYLOAD_SIZES: [usize; 5] = [100, 500, 1500, 5000, 30000];

// =============================================================================
// Section 1: Padding-Only Benchmarks
// =============================================================================

/// Benchmark padding overhead per payload size.
fn bench_padding_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("Padding overhead");
    let normalizer = TrafficNormalizer::default();

    for size in PAYLOAD_SIZES.iter() {
        let payload = random_payload(*size);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::new("payload_size", size), size, |b, _| {
            b.iter(|| {
                let prepared = normalizer.prepare_message(black_box(&payload));
                black_box(prepared)
            })
        });
    }

    group.finish();
}

/// Benchmark bucket selection performance.
fn bench_bucket_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("Bucket selection");
    let normalizer = TrafficNormalizer::default();

    // Test boundary conditions around each bucket
    let boundary_sizes: [(usize, &str); 8] = [
        (100, "tiny_100"),
        (510, "below_512"),
        (512, "exact_512"),
        (2000, "below_2048"),
        (2048, "exact_2048"),
        (8000, "below_8192"),
        (32000, "below_32768"),
        (100000, "large_100k"),
    ];

    for (size, name) in boundary_sizes.iter() {
        let payload = random_payload(*size);

        group.bench_function(*name, |b| {
            b.iter(|| {
                let prepared = normalizer.prepare_message(black_box(&payload));
                black_box(prepared)
            })
        });
    }

    group.finish();
}

/// Benchmark padding with default (maximum privacy) config.
fn bench_padding_default(c: &mut Criterion) {
    let mut group = c.benchmark_group("Padding default");
    let payload = random_payload(500); // Typical transaction size

    let normalizer = TrafficNormalizer::default();

    group.bench_function("prepare_message", |b| {
        b.iter(|| {
            let prepared = normalizer.prepare_message(black_box(&payload));
            black_box(prepared)
        })
    });

    // Also benchmark minimal config for comparison
    let minimal_normalizer = TrafficNormalizer::minimal();

    group.bench_function("prepare_message_minimal", |b| {
        b.iter(|| {
            let prepared = minimal_normalizer.prepare_message(black_box(&payload));
            black_box(prepared)
        })
    });

    group.finish();
}

/// Measure actual bandwidth overhead from padding.
fn bench_bandwidth_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("Bandwidth overhead measurement");
    let normalizer = TrafficNormalizer::default();

    // Measure overhead for each bucket
    for &bucket in PADDING_BUCKETS.iter() {
        // Use a size that just fits in this bucket
        let original_size = bucket / 2;
        let payload = random_payload(original_size);

        group.bench_function(format!("bucket_{}", bucket), |b| {
            b.iter(|| {
                let prepared = normalizer.prepare_message(black_box(&payload));
                // Calculate overhead ratio
                let overhead = prepared.padding_overhead();
                black_box((prepared, overhead))
            })
        });
    }

    group.finish();
}

// =============================================================================
// Section 2: Jitter-Only Benchmarks
// =============================================================================

/// Benchmark jitter delay generation (no actual sleep).
fn bench_jitter_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Jitter generation");

    // Minimal - no jitter
    let minimal = TrafficNormalizer::minimal();
    group.bench_function("minimal_disabled", |b| {
        b.iter(|| black_box(minimal.generate_jitter()))
    });

    // Default - 100-300ms jitter
    let default_norm = TrafficNormalizer::default();
    group.bench_function("default_100_300ms", |b| {
        b.iter(|| black_box(default_norm.generate_jitter()))
    });

    group.finish();
}

/// Benchmark jitter distribution to verify uniform spread.
fn bench_jitter_distribution(c: &mut Criterion) {
    let mut group = c.benchmark_group("Jitter distribution");
    let normalizer = TrafficNormalizer::default();

    // Measure many samples to verify distribution
    group.bench_function("sample_1000", |b| {
        b.iter(|| {
            let mut sum = 0u128;
            for _ in 0..1000 {
                sum += normalizer.generate_jitter().as_millis();
            }
            black_box(sum)
        })
    });

    group.finish();
}

// =============================================================================
// Section 3: Constant-Rate Transmitter Benchmarks
// =============================================================================

/// Benchmark transmitter creation.
fn bench_transmitter_creation(c: &mut Criterion) {
    c.bench_function("Transmitter creation", |b| {
        b.iter(|| black_box(ConstantRateTransmitter::default()))
    });
}

/// Benchmark enqueue operation with varying queue depths.
fn bench_transmitter_enqueue(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transmitter enqueue");

    let queue_depths = [0, 10, 50, 99, 100];

    for depth in queue_depths.iter() {
        group.bench_with_input(BenchmarkId::new("depth", depth), depth, |b, &depth| {
            b.iter_batched(
                || {
                    let mut tx = ConstantRateTransmitter::default();
                    // Pre-fill to target depth
                    for i in 0..depth {
                        tx.enqueue(OutgoingMessage::transaction(vec![i as u8]));
                    }
                    tx
                },
                |mut tx| {
                    tx.enqueue(OutgoingMessage::transaction(vec![42]));
                    black_box(tx)
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

/// Benchmark tick operation (dequeue + cover traffic decision).
fn bench_transmitter_tick(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transmitter tick");

    // Tick with message available
    group.bench_function("with_message", |b| {
        b.iter_batched(
            || {
                let mut tx = ConstantRateTransmitter::default();
                tx.enqueue(OutgoingMessage::transaction(vec![1, 2, 3]));
                tx
            },
            |mut tx| {
                let msg = tx.tick();
                black_box((tx, msg))
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // Tick generating cover traffic
    group.bench_function("cover_traffic", |b| {
        b.iter_batched(
            ConstantRateTransmitter::default,
            |mut tx| {
                let msg = tx.tick();
                black_box((tx, msg))
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // Tick with cover disabled (returns None)
    group.bench_function("empty_no_cover", |b| {
        let config = ConstantRateConfig::new(2.0, false, 100);
        b.iter_batched(
            || ConstantRateTransmitter::new(config.clone()),
            |mut tx| {
                let msg = tx.tick();
                black_box((tx, msg))
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

/// Benchmark queue behavior under load (many enqueues/ticks).
fn bench_transmitter_under_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transmitter under load");

    // Simulate burst traffic: many enqueues, then draining
    group.bench_function("burst_100_enqueue", |b| {
        b.iter_batched(
            ConstantRateTransmitter::default,
            |mut tx| {
                for i in 0..100 {
                    tx.enqueue(OutgoingMessage::transaction(vec![i as u8]));
                }
                black_box(tx)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // Steady-state: interleaved enqueue/tick
    group.bench_function("steady_state_100", |b| {
        b.iter_batched(
            ConstantRateTransmitter::default,
            |mut tx| {
                for i in 0..100 {
                    tx.enqueue(OutgoingMessage::transaction(vec![i as u8]));
                    // Tick won't actually send due to rate limiting,
                    // but exercises the logic
                    let _ = tx.tick();
                }
                black_box(tx)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

/// Benchmark queue overflow behavior.
fn bench_transmitter_overflow(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transmitter overflow");

    // Small queue that overflows quickly
    let config = ConstantRateConfig::new(2.0, true, 10);

    group.bench_function("overflow_drop_oldest", |b| {
        b.iter_batched(
            || ConstantRateTransmitter::new(config.clone()),
            |mut tx| {
                // Overflow the queue
                for i in 0..20 {
                    tx.enqueue(OutgoingMessage::transaction(vec![i as u8]));
                }
                let dropped = tx.metrics().snapshot().messages_dropped;
                black_box((tx, dropped))
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

// =============================================================================
// Section 4: Cover Traffic Benchmarks
// =============================================================================

/// Benchmark cover message generation.
fn bench_cover_message_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cover message generation");

    // Default weighted distribution
    group.bench_function("default_distribution", |b| {
        b.iter(|| black_box(CoverMessage::generate()))
    });

    // Specific sizes
    for size in [200, 300, 450, 600].iter() {
        group.bench_with_input(BenchmarkId::new("fixed_size", size), size, |b, &size| {
            b.iter(|| black_box(CoverMessage::with_size(size)))
        });
    }

    group.finish();
}

/// Benchmark cover traffic generator with different configurations.
fn bench_cover_generator_configs(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cover generator configs");

    // Default
    let default_gen = CoverTrafficGenerator::default();
    group.bench_function("default", |b| b.iter(|| black_box(default_gen.generate())));

    // Uniform distribution
    let uniform_gen = CoverTrafficGenerator::uniform();
    group.bench_function("uniform", |b| b.iter(|| black_box(uniform_gen.generate())));

    // Heavy bias toward small
    let small_bias = CoverTrafficGenerator::with_weights([100, 10, 1]);
    group.bench_function("small_bias", |b| {
        b.iter(|| black_box(small_bias.generate()))
    });

    group.finish();
}

/// Benchmark batch cover generation.
fn bench_cover_batch_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cover batch generation");
    let generator = CoverTrafficGenerator::default();

    for batch_size in [10, 100, 1000].iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch", batch_size),
            batch_size,
            |b, &size| b.iter(|| black_box(generator.generate_batch(size))),
        );
    }

    group.finish();
}

/// Measure cover traffic bandwidth consumption.
fn bench_cover_bandwidth(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cover bandwidth measurement");
    let generator = CoverTrafficGenerator::default();

    // Generate many messages and measure total bytes
    group.bench_function("bytes_per_1000", |b| {
        b.iter(|| {
            let batch = generator.generate_batch(1000);
            let total_bytes: usize = batch.iter().map(|m| m.serialized_size()).sum();
            black_box(total_bytes)
        })
    });

    group.finish();
}

/// Benchmark cover message serialization.
fn bench_cover_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cover serialization");

    let msg = CoverMessage::with_size(400);

    group.bench_function("to_bytes", |b| b.iter(|| black_box(msg.to_bytes())));

    let bytes = msg.to_bytes();
    group.bench_function("from_bytes", |b| {
        b.iter(|| black_box(CoverMessage::from_bytes(&bytes)))
    });

    group.finish();
}

// =============================================================================
// Section 5: Full Stack Benchmarks
// =============================================================================

/// Benchmark complete message preparation pipeline.
fn bench_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("Full pipeline");
    let payload = random_payload(400); // Typical transaction

    // Default config (all features enabled)
    let normalizer = TrafficNormalizer::default();

    group.bench_function("prepare_default", |b| {
        b.iter(|| {
            // Prepare message (padding)
            let prepared = normalizer.prepare_message(black_box(&payload));

            // Generate jitter (no sleep, just duration calculation)
            let jitter = normalizer.generate_jitter();

            black_box((prepared, jitter))
        })
    });

    // Minimal config for comparison
    let minimal_normalizer = TrafficNormalizer::minimal();

    group.bench_function("prepare_minimal", |b| {
        b.iter(|| {
            let prepared = minimal_normalizer.prepare_message(black_box(&payload));
            let jitter = minimal_normalizer.generate_jitter();
            black_box((prepared, jitter))
        })
    });

    group.finish();
}

/// Benchmark end-to-end with transmitter.
fn bench_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("End-to-end");

    // Default: all features enabled (padding + jitter + cover traffic)
    group.bench_function("default", |b| {
        let normalizer = TrafficNormalizer::default();
        let payload = random_payload(400);

        b.iter_batched(
            ConstantRateTransmitter::default,
            |mut tx| {
                // Prepare with padding
                let prepared = normalizer.prepare_message(&payload);

                // Calculate jitter
                let _jitter = normalizer.generate_jitter();

                // Enqueue
                tx.enqueue(OutgoingMessage::transaction(prepared.payload));

                // Tick
                let msg = tx.tick();

                black_box((tx, msg))
            },
            criterion::BatchSize::SmallInput,
        )
    });

    // Minimal: no normalization (for baseline comparison)
    group.bench_function("minimal", |b| {
        let normalizer = TrafficNormalizer::minimal();
        let payload = random_payload(400);

        b.iter_batched(
            ConstantRateTransmitter::default,
            |mut tx| {
                // Prepare (no padding)
                let prepared = normalizer.prepare_message(&payload);

                // Enqueue
                tx.enqueue(OutgoingMessage::transaction(prepared.payload));

                // Tick
                let msg = tx.tick();

                black_box((tx, msg))
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

/// Benchmark comparison: baseline vs normalized.
fn bench_overhead_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("Overhead comparison");
    let payload = random_payload(400);

    // Baseline: no normalization
    group.bench_function("baseline_no_normalization", |b| {
        b.iter(|| {
            // Just pass through the payload
            let msg = OutgoingMessage::transaction(payload.clone());
            black_box(msg)
        })
    });

    // Minimal: no traffic normalization features
    let minimal = TrafficNormalizer::minimal();
    group.bench_function("minimal_no_normalization", |b| {
        b.iter(|| {
            let prepared = minimal.prepare_message(&payload);
            let msg = OutgoingMessage::transaction(prepared.payload);
            black_box(msg)
        })
    });

    // Default: all features enabled
    let default_norm = TrafficNormalizer::default();
    group.bench_function("default_all_features", |b| {
        b.iter(|| {
            let prepared = default_norm.prepare_message(&payload);
            let _jitter = default_norm.generate_jitter();
            let msg = OutgoingMessage::transaction(prepared.payload);
            black_box(msg)
        })
    });

    group.finish();
}

// =============================================================================
// Section 6: Memory and Throughput Benchmarks
// =============================================================================

/// Benchmark memory usage with large queues.
fn bench_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("Memory usage");

    // Create transmitter and fill with messages
    for queue_size in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("queue_size", queue_size),
            queue_size,
            |b, &size| {
                let config = ConstantRateConfig::new(2.0, true, size);
                b.iter_batched(
                    || ConstantRateTransmitter::new(config.clone()),
                    |mut tx| {
                        for _ in 0..size {
                            let payload = random_payload(400);
                            tx.enqueue(OutgoingMessage::transaction(payload));
                        }
                        black_box(tx)
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}

/// Benchmark throughput: messages per second.
fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("Throughput");
    let normalizer = TrafficNormalizer::default();
    let payload = random_payload(400);

    // Process many messages
    group.throughput(Throughput::Elements(1000));
    group.bench_function("process_1000_messages", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let prepared = normalizer.prepare_message(&payload);
                black_box(&prepared);
            }
        })
    });

    group.finish();
}

// =============================================================================
// Section 7: Latency Percentile Measurements
// =============================================================================

/// Benchmark to measure latency distribution for CI thresholds.
///
/// Note: This doesn't measure actual network latency, but the overhead
/// added by the normalization layer (padding + jitter generation).
fn bench_latency_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("Latency overhead");
    let payload = random_payload(400);

    // Minimal should add minimal overhead
    let minimal = TrafficNormalizer::minimal();
    group.bench_function("minimal_overhead", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let prepared = minimal.prepare_message(&payload);
            let _jitter = minimal.generate_jitter();
            let elapsed = start.elapsed();
            black_box((prepared, elapsed))
        })
    });

    // Default adds overhead from all features
    let default_norm = TrafficNormalizer::default();
    group.bench_function("default_overhead", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let prepared = default_norm.prepare_message(&payload);
            let _jitter = default_norm.generate_jitter();
            let elapsed = start.elapsed();
            black_box((prepared, elapsed))
        })
    });

    group.finish();
}

// =============================================================================
// Criterion Configuration
// =============================================================================

criterion_group!(
    padding_benches,
    bench_padding_overhead,
    bench_bucket_selection,
    bench_padding_default,
    bench_bandwidth_overhead,
);

criterion_group!(
    jitter_benches,
    bench_jitter_generation,
    bench_jitter_distribution,
);

criterion_group!(
    transmitter_benches,
    bench_transmitter_creation,
    bench_transmitter_enqueue,
    bench_transmitter_tick,
    bench_transmitter_under_load,
    bench_transmitter_overflow,
);

criterion_group!(
    cover_benches,
    bench_cover_message_generation,
    bench_cover_generator_configs,
    bench_cover_batch_generation,
    bench_cover_bandwidth,
    bench_cover_serialization,
);

criterion_group!(
    full_stack_benches,
    bench_full_pipeline,
    bench_end_to_end,
    bench_overhead_comparison,
);

criterion_group!(
    resource_benches,
    bench_memory_usage,
    bench_throughput,
    bench_latency_overhead,
);

criterion_main!(
    padding_benches,
    jitter_benches,
    transmitter_benches,
    cover_benches,
    full_stack_benches,
    resource_benches,
);
