// Copyright (c) 2024 Botho Foundation

//! Benchmarks for Lion lattice-based ring signatures.
//!
//! Run with: cargo bench -p bth-crypto-lion
//!
//! These benchmarks measure the performance of:
//! - Key generation (keypair and public key operations)
//! - Ring signature signing
//! - Ring signature verification
//! - Key image computation
//! - Serialization/deserialization

use bth_crypto_lion::{
    lattice::{LionKeyPair, LionPublicKey},
    params::{RING_SIZE, SIGNATURE_BYTES, PUBLIC_KEY_BYTES, SECRET_KEY_BYTES},
    ring_signature::{sign, verify, LionRingSignature},
    LionKeyImage,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

/// Pre-generate test fixtures for benchmarking
struct BenchFixtures {
    keypairs: Vec<LionKeyPair>,
    ring: Vec<LionPublicKey>,
    message: Vec<u8>,
    signature: LionRingSignature,
    signature_bytes: Vec<u8>,
}

impl BenchFixtures {
    fn new() -> Self {
        let mut rng = ChaCha20Rng::seed_from_u64(42);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let message = b"benchmark message for ring signature".to_vec();

        let signature = sign(
            &message,
            ring.as_slice(),
            3,
            &keypairs[3].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        let signature_bytes = signature.to_bytes();

        Self {
            keypairs,
            ring,
            message,
            signature,
            signature_bytes,
        }
    }
}

/// Benchmark keypair generation
fn bench_keygen(c: &mut Criterion) {
    let mut group = c.benchmark_group("Lion keygen");

    // Random keypair generation
    group.bench_function("keypair (random)", |b| {
        let mut rng = ChaCha20Rng::seed_from_u64(123);
        b.iter(|| {
            black_box(LionKeyPair::generate(&mut rng))
        })
    });

    // Deterministic keypair from seed
    let seed = [42u8; 32];
    group.bench_function("keypair (from seed)", |b| {
        b.iter(|| {
            black_box(LionKeyPair::from_seed(&seed))
        })
    });

    group.finish();
}

/// Benchmark ring signature signing
fn bench_sign(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();
    let mut rng = ChaCha20Rng::seed_from_u64(999);

    let mut group = c.benchmark_group("Lion sign");

    // Sign from different ring positions
    for real_index in [0, 3, 6].iter() {
        group.bench_with_input(
            BenchmarkId::new("position", real_index),
            real_index,
            |b, &idx| {
                b.iter(|| {
                    black_box(sign(
                        &fixtures.message,
                        fixtures.ring.as_slice(),
                        idx,
                        &fixtures.keypairs[idx].secret_key,
                        &mut rng,
                    ).expect("signing should succeed"))
                })
            },
        );
    }

    group.finish();
}

/// Benchmark ring signature verification
fn bench_verify(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();

    c.bench_function("Lion verify", |b| {
        b.iter(|| {
            black_box(verify(
                &fixtures.message,
                fixtures.ring.as_slice(),
                &fixtures.signature,
            ).expect("verification should succeed"))
        })
    });
}

/// Benchmark key image computation
fn bench_key_image(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();

    c.bench_function("Lion key image", |b| {
        b.iter(|| {
            black_box(LionKeyImage::from_secret_key(&fixtures.keypairs[0].secret_key))
        })
    });
}

/// Benchmark signature serialization
fn bench_serialize(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();

    let mut group = c.benchmark_group("Lion serialize");

    group.bench_function("signature to_bytes", |b| {
        b.iter(|| {
            black_box(fixtures.signature.to_bytes())
        })
    });

    group.bench_function("signature from_bytes", |b| {
        b.iter(|| {
            black_box(LionRingSignature::from_bytes(&fixtures.signature_bytes, RING_SIZE)
                .expect("deserialization should succeed"))
        })
    });

    group.bench_function("public_key to_bytes", |b| {
        b.iter(|| {
            black_box(fixtures.keypairs[0].public_key.to_bytes())
        })
    });

    group.bench_function("public_key from_bytes", |b| {
        let pk_bytes = fixtures.keypairs[0].public_key.to_bytes();
        b.iter(|| {
            black_box(LionPublicKey::from_bytes(&pk_bytes)
                .expect("deserialization should succeed"))
        })
    });

    group.finish();
}

/// Benchmark complete sign+verify cycle
fn bench_full_cycle(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();
    let mut rng = ChaCha20Rng::seed_from_u64(777);

    c.bench_function("Lion sign+verify cycle", |b| {
        b.iter(|| {
            let sig = sign(
                &fixtures.message,
                fixtures.ring.as_slice(),
                3,
                &fixtures.keypairs[3].secret_key,
                &mut rng,
            ).expect("signing should succeed");

            black_box(verify(
                &fixtures.message,
                fixtures.ring.as_slice(),
                &sig,
            ).expect("verification should succeed"))
        })
    });
}

/// Report signature and key sizes
fn bench_sizes(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();

    // This is not really a benchmark, but useful for reporting sizes
    println!("\n=== Lion Signature Sizes ===");
    println!("Public Key:  {} bytes", PUBLIC_KEY_BYTES);
    println!("Secret Key:  {} bytes", SECRET_KEY_BYTES);
    println!("Signature:   {} bytes ({:.1} KB)", SIGNATURE_BYTES, SIGNATURE_BYTES as f64 / 1024.0);
    println!("Ring Size:   {} members", RING_SIZE);
    println!("Per-member:  {} bytes", SIGNATURE_BYTES / RING_SIZE);
    println!();

    // Dummy benchmark to satisfy criterion
    c.bench_function("Lion size check", |b| {
        b.iter(|| {
            black_box(fixtures.signature.to_bytes().len())
        })
    });
}

criterion_group!(
    benches,
    bench_keygen,
    bench_sign,
    bench_verify,
    bench_key_image,
    bench_serialize,
    bench_full_cycle,
    bench_sizes,
);

criterion_main!(benches);
