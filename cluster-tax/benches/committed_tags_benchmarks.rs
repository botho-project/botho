//! Benchmarks for committed tag operations.
//!
//! Measures performance of:
//! - Commitment creation (Pedersen commitments)
//! - Conservation proof generation and verification
//! - Schnorr proof operations
//! - Proof size measurements

use bth_cluster_tax::{
    crypto::{
        blinding_generator, cluster_generator, CommittedTagMass, CommittedTagVectorSecret,
        SchnorrProof, TagConservationProver, TagConservationVerifier,
    },
    ClusterId, TagWeight, TAG_WEIGHT_SCALE,
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
