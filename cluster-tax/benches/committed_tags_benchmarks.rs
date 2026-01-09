//! Benchmarks for committed tags and entropy proof performance.
//!
//! These benchmarks measure proof sizes and performance characteristics
//! for the integrated Bulletproof + entropy approach, validating estimates
//! from the Phase A feasibility study (docs/design/entropy-proof-aggregation-research.md).
//!
//! ## Phase A Target Metrics
//!
//! | Metric | Estimated | Target |
//! |--------|-----------|--------|
//! | Combined proof size | ~900 bytes | ≤ 1000 bytes |
//! | Verification time | ~10ms | ≤ 15ms |
//! | Savings vs separate | ~33% | - |
//!
//! ## Benchmark Categories
//!
//! 1. **Collision Entropy** - Circuit-friendly entropy computation
//!    - `collision_sum` - Sum of squared weights calculation
//!    - `collision_entropy` - Full H₂ entropy calculation
//!    - `entropy_threshold` - Threshold check for Bulletproofs
//!
//! 2. **Schnorr Proofs** - Basic proof primitives
//!    - `schnorr_prove` - Proof generation
//!    - `schnorr_verify` - Proof verification
//!
//! 3. **Conservation Proofs** - Tag mass conservation
//!    - `conservation_prove` - Per-cluster proof generation
//!    - `conservation_verify` - Single proof verification
//!    - `batch_verify` - Batch verification for transactions
//!
//! 4. **Proof Sizes** - Size measurements for varying cluster counts

use bth_cluster_tax::{
    crypto::{
        blinding_generator, cluster_generator, CommittedTagMass, CommittedTagVector,
        CommittedTagVectorSecret, SchnorrProof, TagConservationProof, TagConservationProver,
        TagConservationVerifier,
    },
    ClusterId, TagVector, TagWeight, TAG_WEIGHT_SCALE,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use curve25519_dalek::scalar::Scalar;
use rand_core::OsRng;
use std::collections::HashMap;

/// Create a test secret for a given number of clusters.
fn create_test_secret(num_clusters: usize, value: u64) -> CommittedTagVectorSecret {
    let mut tags = HashMap::new();
    let weight_per_cluster = TAG_WEIGHT_SCALE / num_clusters as u32;

    for i in 0..num_clusters {
        tags.insert(ClusterId(i as u64), weight_per_cluster as TagWeight);
    }

    CommittedTagVectorSecret::from_plaintext(value, &tags, &mut OsRng)
}

/// Create a TagVector with N equal-weight clusters (for collision entropy benchmarks).
fn create_tag_vector(num_clusters: usize) -> TagVector {
    let mut tags = TagVector::new();
    if num_clusters == 0 {
        return tags;
    }

    let weight_per_cluster = TAG_WEIGHT_SCALE / num_clusters as u32;
    for i in 0..num_clusters {
        tags.set(ClusterId::new(i as u64), weight_per_cluster);
    }
    tags
}

// ============================================================================
// Collision Entropy Benchmarks (Circuit-Friendly Entropy for Bulletproofs)
// ============================================================================

/// Benchmark collision sum computation: sum of squared weights.
///
/// This is the core operation for circuit-friendly entropy checks:
/// H₂ ≥ threshold ⟺ sum_sq × 2^threshold ≤ total_sq
fn bench_collision_sum(c: &mut Criterion) {
    let mut group = c.benchmark_group("collision_entropy/collision_sum");

    for num_clusters in [1, 2, 3, 5, 8] {
        let tags = create_tag_vector(num_clusters);

        group.bench_with_input(
            BenchmarkId::from_parameter(num_clusters),
            &tags,
            |b, tags| {
                b.iter(|| black_box(tags.collision_sum()));
            },
        );
    }

    group.finish();
}

/// Benchmark full collision entropy calculation: H₂ = -log₂(Σ p²).
fn bench_collision_entropy(c: &mut Criterion) {
    let mut group = c.benchmark_group("collision_entropy/entropy_calc");

    for num_clusters in [1, 2, 3, 5, 8] {
        let tags = create_tag_vector(num_clusters);

        group.bench_with_input(
            BenchmarkId::from_parameter(num_clusters),
            &tags,
            |b, tags| {
                b.iter(|| black_box(tags.collision_entropy()));
            },
        );
    }

    group.finish();
}

/// Benchmark entropy threshold check (the circuit-friendly operation).
///
/// This is what would be proven in a Bulletproof: that collision entropy
/// meets a specified threshold without computing logarithms.
fn bench_entropy_threshold(c: &mut Criterion) {
    let mut group = c.benchmark_group("collision_entropy/threshold_check");

    for num_clusters in [1, 2, 3, 5, 8] {
        let tags = create_tag_vector(num_clusters);
        let threshold = 0.5; // 0.5 bits threshold

        group.bench_with_input(
            BenchmarkId::from_parameter(num_clusters),
            &tags,
            |b, tags| {
                b.iter(|| black_box(tags.meets_entropy_threshold(threshold)));
            },
        );
    }

    group.finish();
}

/// Compare Shannon vs Collision entropy computation.
fn bench_entropy_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("collision_entropy/comparison");

    let tags = create_tag_vector(4); // 4 clusters for interesting comparison

    group.bench_function("shannon_entropy", |b| {
        b.iter(|| black_box(tags.cluster_entropy()));
    });

    group.bench_function("collision_entropy", |b| {
        b.iter(|| black_box(tags.collision_entropy()));
    });

    group.finish();
}

// ============================================================================
// Proof Size Measurements
// ============================================================================

/// Measure and report proof sizes for varying cluster counts.
///
/// This validates the Phase A estimates:
/// - Combined proof size: ~900 bytes (target ≤ 1000 bytes)
fn bench_proof_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("proof_sizes");

    for num_clusters in [1, 2, 3, 5, 8] {
        group.bench_with_input(
            BenchmarkId::new("measure", num_clusters),
            &num_clusters,
            |b, &n| {
                b.iter(|| {
                    let input_secret = create_test_secret(n, 1_000_000);
                    let decay_rate = 50_000;
                    let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

                    let prover = TagConservationProver::new(
                        vec![input_secret.clone()],
                        vec![output_secret.clone()],
                        decay_rate,
                    );
                    let proof = prover.prove(&mut OsRng).expect("Should generate proof");

                    // Measure sizes
                    let conservation_size = proof.to_bytes().len();
                    let input_vector_size = input_secret.commit().to_bytes().len();
                    let output_vector_size = output_secret.commit().to_bytes().len();
                    let total = conservation_size + input_vector_size + output_vector_size;

                    black_box((conservation_size, input_vector_size, output_vector_size, total))
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Batch Verification Benchmarks
// ============================================================================

/// Benchmark batch verification of conservation proofs.
///
/// Tests verification throughput for different batch sizes, simulating
/// block validation scenarios.
fn bench_batch_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("conservation_batch_verify");

    for batch_size in [1, 10, 50, 100] {
        let num_clusters = 3; // Typical transaction
        let decay_rate = 50_000;

        // Pre-generate proofs and verifiers
        let proofs_and_verifiers: Vec<_> = (0..batch_size)
            .map(|_| {
                let input_secret = create_test_secret(num_clusters, 1_000_000);
                let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

                let prover = TagConservationProver::new(
                    vec![input_secret.clone()],
                    vec![output_secret.clone()],
                    decay_rate,
                );
                let proof = prover.prove(&mut OsRng).expect("Should generate proof");

                let verifier = TagConservationVerifier::new(
                    vec![input_secret.commit()],
                    vec![output_secret.commit()],
                    decay_rate,
                );

                (verifier, proof)
            })
            .collect();

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &proofs_and_verifiers,
            |b, proofs_and_verifiers| {
                b.iter(|| {
                    let mut all_valid = true;
                    for (verifier, proof) in proofs_and_verifiers {
                        all_valid &= verifier.verify(proof);
                    }
                    black_box(all_valid)
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// Basic Benchmarks (Original)
// ============================================================================

/// Benchmark single Pedersen commitment creation.
fn bench_commitment_creation(c: &mut Criterion) {
    let cluster = ClusterId(42);
    let mass = 500_000u64;

    c.bench_function("commitment_create", |b| {
        b.iter(|| {
            let blinding = Scalar::random(&mut OsRng);
            black_box(CommittedTagMass::new(cluster, mass, blinding))
        })
    });
}

/// Benchmark cluster generator derivation.
fn bench_cluster_generator(c: &mut Criterion) {
    c.bench_function("cluster_generator", |b| {
        b.iter(|| black_box(cluster_generator(ClusterId(42))))
    });
}

/// Benchmark committed tag vector creation for different cluster counts.
fn bench_vector_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_creation");

    for num_clusters in [1, 3, 5, 8] {
        group.throughput(Throughput::Elements(num_clusters as u64));
        group.bench_with_input(
            BenchmarkId::new("clusters", num_clusters),
            &num_clusters,
            |b, &n| {
                let secret = create_test_secret(n, 1_000_000);
                b.iter(|| black_box(secret.commit()))
            },
        );
    }

    group.finish();
}

/// Benchmark Schnorr proof generation.
fn bench_schnorr_prove(c: &mut Criterion) {
    let x = Scalar::random(&mut OsRng);

    c.bench_function("schnorr_prove", |b| {
        b.iter(|| black_box(SchnorrProof::prove(x, b"test_context", &mut OsRng)))
    });
}

/// Benchmark Schnorr proof verification.
fn bench_schnorr_verify(c: &mut Criterion) {
    let x = Scalar::random(&mut OsRng);
    let p = (x * blinding_generator()).compress();
    let proof = SchnorrProof::prove(x, b"test_context", &mut OsRng);

    c.bench_function("schnorr_verify", |b| {
        b.iter(|| black_box(proof.verify(&p, b"test_context")))
    });
}

/// Benchmark conservation proof generation for different cluster counts.
fn bench_conservation_prove(c: &mut Criterion) {
    let mut group = c.benchmark_group("conservation_prove");

    for num_clusters in [1, 3, 5, 8] {
        group.throughput(Throughput::Elements(num_clusters as u64));
        group.bench_with_input(
            BenchmarkId::new("clusters", num_clusters),
            &num_clusters,
            |b, &n| {
                let input_secret = create_test_secret(n, 1_000_000);
                let decay_rate = 50_000; // 5% decay
                let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

                let prover = TagConservationProver::new(
                    vec![input_secret.clone()],
                    vec![output_secret],
                    decay_rate,
                );

                b.iter(|| black_box(prover.prove(&mut OsRng)))
            },
        );
    }

    group.finish();
}

/// Benchmark conservation proof verification for different cluster counts.
fn bench_conservation_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("conservation_verify");

    for num_clusters in [1, 3, 5, 8] {
        group.throughput(Throughput::Elements(num_clusters as u64));
        group.bench_with_input(
            BenchmarkId::new("clusters", num_clusters),
            &num_clusters,
            |b, &n| {
                let input_secret = create_test_secret(n, 1_000_000);
                let decay_rate = 50_000;
                let output_secret = input_secret.apply_decay(decay_rate, &mut OsRng);

                let prover = TagConservationProver::new(
                    vec![input_secret.clone()],
                    vec![output_secret.clone()],
                    decay_rate,
                );

                let proof = prover.prove(&mut OsRng).expect("should generate proof");

                let verifier = TagConservationVerifier::new(
                    vec![input_secret.commit()],
                    vec![output_secret.commit()],
                    decay_rate,
                );

                b.iter(|| black_box(verifier.verify(&proof)))
            },
        );
    }

    group.finish();
}

/// Benchmark multi-input conservation proofs (simulating real transactions).
fn bench_multi_input_conservation(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_input_conservation");

    // Test with 2 inputs, varying cluster counts
    for num_clusters in [1, 3, 5] {
        group.bench_with_input(
            BenchmarkId::new("2inputs_clusters", num_clusters),
            &num_clusters,
            |b, &n| {
                let input1 = create_test_secret(n, 500_000);
                let input2 = create_test_secret(n, 500_000);
                let decay_rate = 50_000;

                // Merge inputs and apply decay
                let merged =
                    CommittedTagVectorSecret::merge(&[input1.clone(), input2.clone()], &mut OsRng);
                let output = merged.apply_decay(decay_rate, &mut OsRng);

                let prover =
                    TagConservationProver::new(vec![input1, input2], vec![output], decay_rate);

                b.iter(|| black_box(prover.prove(&mut OsRng)))
            },
        );
    }

    group.finish();
}

/// Benchmark decay application.
fn bench_decay_application(c: &mut Criterion) {
    let mut group = c.benchmark_group("decay_application");

    for num_clusters in [1, 3, 5, 8] {
        group.bench_with_input(
            BenchmarkId::new("clusters", num_clusters),
            &num_clusters,
            |b, &n| {
                let secret = create_test_secret(n, 1_000_000);
                let decay_rate = 50_000;

                b.iter(|| black_box(secret.apply_decay(decay_rate, &mut OsRng)))
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    // Collision entropy benchmarks (new for #263)
    bench_collision_sum,
    bench_collision_entropy,
    bench_entropy_threshold,
    bench_entropy_comparison,
    // Proof size measurements (new for #263)
    bench_proof_sizes,
    // Batch verification (new for #263)
    bench_batch_verify,
    // Original benchmarks
    bench_commitment_creation,
    bench_cluster_generator,
    bench_vector_creation,
    bench_schnorr_prove,
    bench_schnorr_verify,
    bench_conservation_prove,
    bench_conservation_verify,
    bench_multi_input_conservation,
    bench_decay_application,
);

criterion_main!(benches);
