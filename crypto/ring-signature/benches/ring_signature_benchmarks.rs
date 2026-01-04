//! Performance benchmarks for ring signature operations.
//!
//! Run with: cargo bench -p bth-crypto-ring-signature
//!
//! These benchmarks measure MLSAG sign/verify performance with different ring
//! sizes.

use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::{
    generators, CompressedCommitment, PedersenGens, ReducedTxOut, RingMLSAG, Scalar,
};
use bth_util_from_random::FromRandom;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::{rngs::StdRng, CryptoRng, RngCore, SeedableRng};

/// Parameters for creating a test MLSAG
struct TestRingParams {
    message: [u8; 32],
    ring: Vec<ReducedTxOut>,
    real_index: usize,
    onetime_private_key: RistrettoPrivate,
    value: u64,
    blinding: Scalar,
    pseudo_output_blinding: Scalar,
    generator: PedersenGens,
}

impl TestRingParams {
    fn random(num_mixins: usize, seed: u64) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);

        let mut message = [0u8; 32];
        rng.fill_bytes(&mut message);

        // Use the generators function with a deterministic index
        let generator = generators(seed);
        let pseudo_output_blinding = Scalar::random(&mut rng);

        // Create mixin ring members
        let mut ring: Vec<ReducedTxOut> = Vec::with_capacity(num_mixins + 1);
        for _ in 0..num_mixins {
            let public_key = CompressedRistrettoPublic::from_random(&mut rng);
            let target_key = CompressedRistrettoPublic::from_random(&mut rng);
            let commitment = {
                let value = rng.next_u64();
                let blinding = Scalar::random(&mut rng);
                CompressedCommitment::new(value, blinding, &generator)
            };
            ring.push(ReducedTxOut {
                public_key,
                target_key,
                commitment,
            });
        }

        // The real input
        let onetime_private_key = RistrettoPrivate::from_random(&mut rng);
        let value = rng.next_u64();
        let blinding = Scalar::random(&mut rng);
        let commitment = CompressedCommitment::new(value, blinding, &generator);

        let real_index = num_mixins; // Put real input at the end
        ring.push(ReducedTxOut {
            target_key: CompressedRistrettoPublic::from(RistrettoPublic::from(
                &onetime_private_key,
            )),
            public_key: CompressedRistrettoPublic::from_random(&mut rng),
            commitment,
        });

        Self {
            message,
            ring,
            real_index,
            onetime_private_key,
            value,
            blinding,
            pseudo_output_blinding,
            generator,
        }
    }

    fn sign<R: RngCore + CryptoRng>(&self, rng: &mut R) -> RingMLSAG {
        RingMLSAG::sign(
            &self.message,
            &self.ring,
            self.real_index,
            &self.onetime_private_key,
            self.value,
            &self.blinding,
            &self.pseudo_output_blinding,
            &self.generator,
            rng,
        )
        .expect("signing should succeed")
    }

    fn output_commitment(&self) -> CompressedCommitment {
        CompressedCommitment::new(self.value, self.pseudo_output_blinding, &self.generator)
    }
}

/// Benchmark MLSAG signing with different ring sizes
fn bench_mlsag_sign(c: &mut Criterion) {
    let mut group = c.benchmark_group("MLSAG sign");

    // Ring sizes: 11 (default), 16, 32
    for ring_size in [11, 16, 32] {
        let num_mixins = ring_size - 1;
        let params = TestRingParams::random(num_mixins, 42);

        group.bench_with_input(
            BenchmarkId::new("ring_size", ring_size),
            &ring_size,
            |b, _| {
                let mut rng = StdRng::seed_from_u64(12345);
                b.iter(|| black_box(params.sign(&mut rng)))
            },
        );
    }
    group.finish();
}

/// Benchmark MLSAG verification with different ring sizes
fn bench_mlsag_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("MLSAG verify");

    for ring_size in [11, 16, 32] {
        let num_mixins = ring_size - 1;
        let params = TestRingParams::random(num_mixins, 42);
        let mut rng = StdRng::seed_from_u64(12345);
        let signature = params.sign(&mut rng);
        let output_commitment = params.output_commitment();

        group.bench_with_input(
            BenchmarkId::new("ring_size", ring_size),
            &ring_size,
            |b, _| {
                b.iter(|| {
                    black_box(signature.verify(&params.message, &params.ring, &output_commitment))
                })
            },
        );
    }
    group.finish();
}

/// Benchmark batch verification (serial baseline)
fn bench_mlsag_verify_batch_serial(c: &mut Criterion) {
    let mut group = c.benchmark_group("MLSAG verify batch (serial)");

    let ring_size = 11;
    let num_mixins = ring_size - 1;

    for batch_size in [1, 2, 4, 8, 16] {
        // Pre-generate signatures
        let items: Vec<_> = (0..batch_size)
            .map(|i| {
                let params = TestRingParams::random(num_mixins, 100 + i as u64);
                let mut rng = StdRng::seed_from_u64(200 + i as u64);
                let sig = params.sign(&mut rng);
                let output = params.output_commitment();
                (params, sig, output)
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("batch_size", batch_size),
            &batch_size,
            |b, _| {
                b.iter(|| {
                    for (params, sig, output) in &items {
                        black_box(sig.verify(&params.message, &params.ring, output).unwrap());
                    }
                })
            },
        );
    }
    group.finish();
}

/// Benchmark batch verification using mlsag_verify_batch (parallel when feature
/// enabled)
fn bench_mlsag_verify_batch_api(c: &mut Criterion) {
    use bth_crypto_ring_signature::mlsag_verify_batch;

    let mut group = c.benchmark_group("MLSAG verify batch (API)");

    let ring_size = 11;
    let num_mixins = ring_size - 1;

    for batch_size in [1, 2, 4, 8, 16] {
        // Pre-generate signatures
        let items: Vec<_> = (0..batch_size)
            .map(|i| {
                let params = TestRingParams::random(num_mixins, 100 + i as u64);
                let mut rng = StdRng::seed_from_u64(200 + i as u64);
                let sig = params.sign(&mut rng);
                let output = params.output_commitment();
                (params, sig, output)
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("batch_size", batch_size),
            &batch_size,
            |b, _| {
                b.iter(|| {
                    let batch_items: Vec<_> = items
                        .iter()
                        .map(|(params, sig, output)| {
                            (
                                params.message.as_slice(),
                                params.ring.as_slice(),
                                output,
                                sig,
                            )
                        })
                        .collect();
                    black_box(mlsag_verify_batch(batch_items))
                })
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_mlsag_sign,
    bench_mlsag_verify,
    bench_mlsag_verify_batch_serial,
    bench_mlsag_verify_batch_api,
);

criterion_main!(benches);
