//! Performance benchmarks for post-quantum cryptographic operations.
//!
//! Run with: cargo bench -p bth-crypto-pq
//!
//! These benchmarks measure the performance impact of quantum-safe signatures
//! and key encapsulation compared to what would be expected from classical
//! crypto.

use bth_crypto_pq::{derive_onetime_sig_keypair, derive_pq_keys, MlDsa65KeyPair, MlKem768KeyPair};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

/// Benchmark ML-KEM-768 key generation
fn bench_mlkem_keygen(c: &mut Criterion) {
    c.bench_function("ML-KEM-768 keygen (random)", |b| {
        b.iter(|| black_box(MlKem768KeyPair::generate()))
    });

    let seed = [0u8; 32];
    c.bench_function("ML-KEM-768 keygen (from seed)", |b| {
        b.iter(|| black_box(MlKem768KeyPair::from_seed(&seed)))
    });
}

/// Benchmark ML-DSA-65 key generation
fn bench_mldsa_keygen(c: &mut Criterion) {
    c.bench_function("ML-DSA-65 keygen (random)", |b| {
        b.iter(|| black_box(MlDsa65KeyPair::generate()))
    });

    let seed = [0u8; 32];
    c.bench_function("ML-DSA-65 keygen (from seed)", |b| {
        b.iter(|| black_box(MlDsa65KeyPair::from_seed(&seed)))
    });
}

/// Benchmark ML-KEM-768 encapsulation
fn bench_mlkem_encapsulate(c: &mut Criterion) {
    let keypair = MlKem768KeyPair::generate();
    let public_key = keypair.public_key();

    c.bench_function("ML-KEM-768 encapsulate", |b| {
        b.iter(|| black_box(public_key.encapsulate()))
    });
}

/// Benchmark ML-KEM-768 decapsulation
fn bench_mlkem_decapsulate(c: &mut Criterion) {
    let keypair = MlKem768KeyPair::generate();
    let (ciphertext, _) = keypair.public_key().encapsulate();

    c.bench_function("ML-KEM-768 decapsulate", |b| {
        b.iter(|| black_box(keypair.decapsulate(&ciphertext).unwrap()))
    });
}

/// Benchmark ML-DSA-65 signing
fn bench_mldsa_sign(c: &mut Criterion) {
    let keypair = MlDsa65KeyPair::generate();

    let mut group = c.benchmark_group("ML-DSA-65 sign");

    for msg_size in [32, 64, 256, 1024].iter() {
        let message = vec![0u8; *msg_size];
        group.bench_with_input(
            BenchmarkId::new("message_bytes", msg_size),
            msg_size,
            |b, _| b.iter(|| black_box(keypair.sign(&message))),
        );
    }
    group.finish();
}

/// Benchmark ML-DSA-65 verification
fn bench_mldsa_verify(c: &mut Criterion) {
    let keypair = MlDsa65KeyPair::generate();

    let mut group = c.benchmark_group("ML-DSA-65 verify");

    for msg_size in [32, 64, 256, 1024].iter() {
        let message = vec![0u8; *msg_size];
        let signature = keypair.sign(&message);
        let public_key = keypair.public_key();

        group.bench_with_input(
            BenchmarkId::new("message_bytes", msg_size),
            msg_size,
            |b, _| b.iter(|| black_box(public_key.verify(&message, &signature).unwrap())),
        );
    }
    group.finish();
}

/// Benchmark full key derivation from mnemonic
fn bench_derive_pq_keys(c: &mut Criterion) {
    let mnemonic = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    c.bench_function("derive_pq_keys (full wallet keygen)", |b| {
        b.iter(|| black_box(derive_pq_keys(mnemonic)))
    });
}

/// Benchmark one-time keypair derivation (used per transaction output)
fn bench_onetime_keypair(c: &mut Criterion) {
    let shared_secret = [42u8; 32];

    c.bench_function("derive_onetime_sig_keypair", |b| {
        b.iter(|| black_box(derive_onetime_sig_keypair(&shared_secret, 0)))
    });
}

/// Benchmark complete quantum-private output creation flow
fn bench_pq_output_creation(c: &mut Criterion) {
    let recipient_keys = derive_pq_keys(b"recipient mnemonic");
    let recipient_kem_pk = recipient_keys.kem_keypair.public_key();

    c.bench_function("PQ output creation (encap + derive)", |b| {
        b.iter(|| {
            // Sender encapsulates to recipient
            let (ciphertext, shared_secret) = recipient_kem_pk.encapsulate();
            // Derive one-time signing keypair
            let onetime = derive_onetime_sig_keypair(shared_secret.as_bytes(), 0);
            black_box((ciphertext, onetime.public_key().as_bytes().to_vec()))
        })
    });
}

/// Benchmark complete quantum-private input signing flow
fn bench_pq_input_signing(c: &mut Criterion) {
    let recipient_keys = derive_pq_keys(b"recipient mnemonic");
    let (ciphertext, _) = recipient_keys.kem_keypair.public_key().encapsulate();

    // Recipient decapsulates and derives keypair
    let shared_secret = recipient_keys.kem_keypair.decapsulate(&ciphertext).unwrap();
    let onetime_keypair = derive_onetime_sig_keypair(shared_secret.as_bytes(), 0);

    let tx_message = [0u8; 32]; // Transaction signing hash

    c.bench_function("PQ input signing (decap + derive + sign)", |b| {
        b.iter(|| {
            // Full flow: decapsulate, derive keypair, sign
            let ss = recipient_keys.kem_keypair.decapsulate(&ciphertext).unwrap();
            let keypair = derive_onetime_sig_keypair(ss.as_bytes(), 0);
            let sig = keypair.sign(&tx_message);
            black_box(sig)
        })
    });
}

/// Benchmark complete quantum-private input verification flow
fn bench_pq_input_verification(c: &mut Criterion) {
    let keypair = MlDsa65KeyPair::generate();
    let tx_message = [0u8; 32];
    let signature = keypair.sign(&tx_message);
    let public_key = keypair.public_key();

    c.bench_function("PQ input verification", |b| {
        b.iter(|| black_box(public_key.verify(&tx_message, &signature).unwrap()))
    });
}

/// Benchmark transaction with multiple inputs/outputs
fn bench_multi_io_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transaction signing overhead");

    // Pre-generate keypairs
    let keypairs: Vec<_> = (0..10).map(|_| MlDsa65KeyPair::generate()).collect();
    let tx_message = [0u8; 32];

    for num_inputs in [1, 2, 5, 10].iter() {
        group.bench_with_input(
            BenchmarkId::new("inputs", num_inputs),
            num_inputs,
            |b, &n| {
                b.iter(|| {
                    // Sign with n keypairs (simulating n inputs)
                    let sigs: Vec<_> = keypairs
                        .iter()
                        .take(n)
                        .map(|kp| kp.sign(&tx_message))
                        .collect();
                    black_box(sigs)
                })
            },
        );
    }
    group.finish();
}

/// Benchmark transaction verification with multiple inputs
fn bench_multi_input_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transaction verification overhead");

    // Pre-generate keypairs and signatures
    let keypairs: Vec<_> = (0..10).map(|_| MlDsa65KeyPair::generate()).collect();
    let tx_message = [0u8; 32];
    let signatures: Vec<_> = keypairs.iter().map(|kp| kp.sign(&tx_message)).collect();

    for num_inputs in [1, 2, 5, 10].iter() {
        group.bench_with_input(
            BenchmarkId::new("inputs", num_inputs),
            num_inputs,
            |b, &n| {
                b.iter(|| {
                    // Verify n signatures
                    for i in 0..n {
                        keypairs[i]
                            .public_key()
                            .verify(&tx_message, &signatures[i])
                            .unwrap();
                    }
                })
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_mlkem_keygen,
    bench_mldsa_keygen,
    bench_mlkem_encapsulate,
    bench_mlkem_decapsulate,
    bench_mldsa_sign,
    bench_mldsa_verify,
    bench_derive_pq_keys,
    bench_onetime_keypair,
    bench_pq_output_creation,
    bench_pq_input_signing,
    bench_pq_input_verification,
    bench_multi_io_transaction,
    bench_multi_input_verification,
);

criterion_main!(benches);
