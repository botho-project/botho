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
use rand::{SeedableRng, RngCore};
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

// =============================================================================
// Granular profiling benchmarks for identifying bottlenecks
// =============================================================================

use bth_crypto_lion::polynomial::{Poly, PolyMatrix, PolyVecK, PolyVecL};
use bth_crypto_lion::params::TAU;

/// Benchmark individual polynomial operations
fn bench_poly_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("Poly ops");

    // Create test polynomials
    let mut rng = ChaCha20Rng::seed_from_u64(42);
    let mut p1 = Poly::zero();
    let mut p2 = Poly::zero();
    for i in 0..256 {
        p1.coeffs[i] = rng.next_u32() % 8380417;
        p2.coeffs[i] = rng.next_u32() % 8380417;
    }

    // NTT forward
    group.bench_function("NTT forward", |b| {
        let mut p = p1.clone();
        b.iter(|| {
            p.ntt();
            black_box(&p);
            // Reset for next iteration
            p = p1.clone();
        })
    });

    // Polynomial multiplication via NTT (includes NTT forward + inverse internally)
    group.bench_function("ntt_mul", |b| {
        b.iter(|| {
            black_box(p1.ntt_mul(&p2))
        })
    });

    // Challenge polynomial sampling
    let seed = [42u8; 32];
    group.bench_function("sample_challenge", |b| {
        b.iter(|| {
            black_box(Poly::sample_challenge(&seed, TAU))
        })
    });

    group.finish();
}

/// Benchmark matrix operations (most expensive)
fn bench_matrix_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("Matrix ops");

    let seed = [42u8; 32];

    // Matrix expansion from seed
    group.bench_function("expand_a (K×L matrix)", |b| {
        b.iter(|| {
            black_box(PolyMatrix::expand_a(&seed))
        })
    });

    // Matrix-vector multiplication
    let a = PolyMatrix::expand_a(&seed);
    let mut rng = ChaCha20Rng::seed_from_u64(123);
    let mut v = PolyVecL::zero();
    for poly in v.polys.iter_mut() {
        for c in poly.coeffs.iter_mut() {
            *c = rng.next_u32() % 8380417;
        }
    }

    group.bench_function("mul_vec (A * v)", |b| {
        b.iter(|| {
            black_box(a.mul_vec(&v))
        })
    });

    group.finish();
}

/// Benchmark vector operations
fn bench_vector_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("Vector ops");

    let mut rng = ChaCha20Rng::seed_from_u64(456);

    // Create test vectors
    let mut v1 = PolyVecK::zero();
    let mut v2 = PolyVecK::zero();
    for poly in v1.polys.iter_mut() {
        for c in poly.coeffs.iter_mut() {
            *c = rng.next_u32() % 8380417;
        }
    }
    for poly in v2.polys.iter_mut() {
        for c in poly.coeffs.iter_mut() {
            *c = rng.next_u32() % 8380417;
        }
    }

    // Vector subtraction
    group.bench_function("PolyVecK sub_assign", |b| {
        let mut v = v1.clone();
        b.iter(|| {
            v.sub_assign(&v2);
            black_box(&v);
        })
    });

    // Infinity norm
    let vl = PolyVecL::zero();
    group.bench_function("PolyVecL infinity_norm", |b| {
        b.iter(|| {
            black_box(vl.infinity_norm())
        })
    });

    group.finish();
}

/// Benchmark hashing operations
fn bench_hash_ops(c: &mut Criterion) {
    use sha3::{Shake256, digest::{ExtendableOutput, Update}};

    let mut group = c.benchmark_group("Hash ops");

    let fixtures = BenchFixtures::new();
    let message = b"test message";

    // Challenge hash (used in signing/verification loop)
    group.bench_function("challenge hash (full)", |b| {
        b.iter(|| {
            let mut hasher = Shake256::default();
            hasher.update(b"botho-lion-challenge-v1");
            hasher.update(&(message.len() as u64).to_le_bytes());
            hasher.update(message);
            for pk in fixtures.ring.iter() {
                hasher.update(&pk.to_bytes());
            }
            hasher.update(&fixtures.keypairs[0].key_image().to_bytes());
            hasher.update(&(0u16).to_le_bytes());
            // Add commitment bytes (simulated)
            hasher.update(&[0u8; 3072]);
            let mut reader = hasher.finalize_xof();
            let mut seed = [0u8; 32];
            sha3::digest::XofReader::read(&mut reader, &mut seed);
            black_box(seed)
        })
    });

    group.finish();
}

/// Breakdown of signing time by operation
fn bench_sign_breakdown(c: &mut Criterion) {
    let fixtures = BenchFixtures::new();

    let mut group = c.benchmark_group("Sign breakdown");

    // Time to expand all matrices (7 members)
    group.bench_function("expand all A matrices (7)", |b| {
        b.iter(|| {
            for pk in fixtures.ring.iter() {
                black_box(pk.expand_a());
            }
        })
    });

    // Pre-expand matrices
    let matrices: Vec<_> = fixtures.ring.iter().map(|pk| pk.expand_a()).collect();

    // Time for 7 matrix-vector multiplications
    let mut rng = ChaCha20Rng::seed_from_u64(789);
    let mut v = PolyVecL::zero();
    for poly in v.polys.iter_mut() {
        for c in poly.coeffs.iter_mut() {
            *c = rng.next_u32() % 8380417;
        }
    }

    group.bench_function("7× matrix-vector mul", |b| {
        b.iter(|| {
            for a in matrices.iter() {
                black_box(a.mul_vec(&v));
            }
        })
    });

    // Time for polynomial-vector multiplications (c * t)
    let challenge = Poly::sample_challenge(&[42u8; 32], TAU);
    group.bench_function("7× poly-vec mul (c*t)", |b| {
        b.iter(|| {
            for pk in fixtures.ring.iter() {
                let mut result = PolyVecK::zero();
                for (r, t) in result.polys.iter_mut().zip(pk.t.polys.iter()) {
                    *r = challenge.ntt_mul(t);
                }
                black_box(result);
            }
        })
    });

    group.finish();
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
    // Profiling benchmarks
    bench_poly_ops,
    bench_matrix_ops,
    bench_vector_ops,
    bench_hash_ops,
    bench_sign_breakdown,
);

criterion_main!(benches);
