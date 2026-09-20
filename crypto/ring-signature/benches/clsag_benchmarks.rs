//! Valid library CLSAG operations: 2 is a small-ring library case, 20 the
//! current transaction default/minimum, and 32 a scaling case (not a protocol
//! maximum).
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::{
    compat::random_scalar, generators, Clsag, CompressedCommitment, PedersenGens, ReducedTxOut,
    Scalar,
};
use bth_util_from_random::FromRandom;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::{rngs::StdRng, SeedableRng};

struct Fixture {
    message: [u8; 32],
    ring: Vec<ReducedTxOut>,
    real_index: usize,
    key: RistrettoPrivate,
    value: u64,
    blinding: Scalar,
    pseudo_blinding: Scalar,
    generator: PedersenGens,
    pseudo_commitment: CompressedCommitment,
}
impl Fixture {
    fn new(size: usize) -> Self {
        // Fixed synthetic workload only; signing randomness is separate below.
        let mut rng = StdRng::seed_from_u64(1344 + size as u64);
        let generator = generators(0);
        let key = RistrettoPrivate::from_random(&mut rng);
        let value = 1_000_000_000_000;
        let blinding = random_scalar(&mut rng);
        let pseudo_blinding = random_scalar(&mut rng);
        let mut ring: Vec<_> = (0..size - 1)
            .map(|i| ReducedTxOut {
                public_key: CompressedRistrettoPublic::from_random(&mut rng),
                target_key: CompressedRistrettoPublic::from_random(&mut rng),
                commitment: CompressedCommitment::new(
                    value + i as u64,
                    random_scalar(&mut rng),
                    &generator,
                ),
            })
            .collect();
        let real_index = size / 2;
        ring.insert(
            real_index,
            ReducedTxOut {
                public_key: CompressedRistrettoPublic::from_random(&mut rng),
                target_key: CompressedRistrettoPublic::from(RistrettoPublic::from(&key)),
                commitment: CompressedCommitment::new(value, blinding, &generator),
            },
        );
        assert_eq!(ring.len(), size);
        let pseudo_commitment = CompressedCommitment::new(value, pseudo_blinding, &generator);
        Self {
            message: [0x44; 32],
            ring,
            real_index,
            key,
            value,
            blinding,
            pseudo_blinding,
            generator,
            pseudo_commitment,
        }
    }
    fn sign(&self, rng: &mut StdRng) -> Clsag {
        Clsag::sign(
            black_box(&self.message),
            black_box(&self.ring),
            self.real_index,
            &self.key,
            self.value,
            &self.blinding,
            &self.pseudo_blinding,
            &self.generator,
            rng,
        )
        .expect("ordinary CLSAG signing succeeds")
    }
    fn preflight(&self, rng: &mut StdRng) -> Clsag {
        let signature = self.sign(rng);
        assert_eq!(signature.responses.len(), self.ring.len());
        signature
            .verify(&self.message, &self.ring, &self.pseudo_commitment)
            .expect("valid CLSAG fixture verifies before timing");
        signature
    }
}

fn sign(c: &mut Criterion) {
    let mut group = c.benchmark_group("CLSAG sign");
    for size in [2, 20, 32] {
        let fixture = Fixture::new(size);
        // OS seeding and preflight are outside timing. RNG advances across every
        // call, including Criterion warmup; no nonce or per-iteration seed reset.
        let mut rng =
            StdRng::try_from_rng(&mut bth_util_from_random::OsRng).expect("OS entropy available");
        fixture.preflight(&mut rng);
        group.bench_with_input(BenchmarkId::new("ring_size", size), &size, |b, _| {
            b.iter(|| black_box(fixture.sign(&mut rng)))
        });
    }
    group.finish();
}

fn verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("CLSAG verify");
    for size in [2, 20, 32] {
        let fixture = Fixture::new(size);
        let mut rng =
            StdRng::try_from_rng(&mut bth_util_from_random::OsRng).expect("OS entropy available");
        let signature = fixture.preflight(&mut rng);
        group.bench_with_input(BenchmarkId::new("ring_size", size), &size, |b, _| {
            b.iter(|| {
                signature
                    .verify(
                        black_box(&fixture.message),
                        black_box(&fixture.ring),
                        black_box(&fixture.pseudo_commitment),
                    )
                    .expect("timed valid CLSAG verification succeeds");
                black_box(())
            })
        });
    }
    group.finish();
}
criterion_group!(benches, sign, verify);
criterion_main!(benches);
