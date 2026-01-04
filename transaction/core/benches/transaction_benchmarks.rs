//! Performance benchmarks for transaction operations.
//!
//! Run with: cargo bench -p bth-transaction-core
//!
//! These benchmarks measure performance of:
//! - Range proof generation and verification
//! - RctBulletproofs signing and verification

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use curve25519_dalek::scalar::Scalar;
use rand::{rngs::StdRng, RngCore, SeedableRng};

use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::{generators, CompressedCommitment, PedersenGens, ReducedTxOut};
use bth_crypto_ring_signature_signer::{
    InputSecret, NoKeysRingSigner, OneTimeKeyDeriveData, SignableInputRing,
};
use bth_transaction_core::{
    range_proofs::{check_range_proofs, generate_range_proofs},
    ring_ct::{InputRing, OutputSecret, SignatureRctBulletproofs, SignedInputRing},
    tx::{TxIn, TxPrefix},
    Amount, BlockVersion, TokenId,
};
use bth_util_from_random::FromRandom;

/// Test parameters for creating valid RctBulletproofs signatures
struct SignatureParams {
    tx_prefix: TxPrefix,
    rings: Vec<SignableInputRing>,
    output_secrets: Vec<OutputSecret>,
    block_version: BlockVersion,
}

impl SignatureParams {
    fn generator(&self) -> PedersenGens {
        generators(self.tx_prefix.fee_token_id)
    }

    fn random(
        block_version: BlockVersion,
        num_inputs: usize,
        num_mixins: usize,
        seed: u64,
    ) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut tx_prefix = TxPrefix::default();

        let token_id: u64 = if block_version.masked_token_id_feature_is_supported() {
            rng.next_u64()
        } else {
            0
        };

        tx_prefix.fee_token_id = token_id;

        let generator = generators(token_id);

        let mut rings = Vec::new();

        for _input_idx in 0..num_inputs {
            let mut ring_members: Vec<ReducedTxOut> = Vec::new();

            // Create random mixins
            for _ in 0..num_mixins {
                let public_key = CompressedRistrettoPublic::from_random(&mut rng);
                let target_key = CompressedRistrettoPublic::from_random(&mut rng);
                let commitment = {
                    let value = rng.next_u64();
                    let blinding = Scalar::random(&mut rng);
                    CompressedCommitment::new(value, blinding, &generator)
                };
                ring_members.push(ReducedTxOut {
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

            let reduced_tx_out = ReducedTxOut {
                target_key: CompressedRistrettoPublic::from(RistrettoPublic::from(
                    &onetime_private_key,
                )),
                public_key: CompressedRistrettoPublic::from_random(&mut rng),
                commitment,
            };

            let real_input_index = rng.next_u64() as usize % (num_mixins + 1);
            ring_members.insert(real_input_index, reduced_tx_out);

            let onetime_key_derive_data = OneTimeKeyDeriveData::OneTimeKey(onetime_private_key);

            rings.push(SignableInputRing {
                members: ring_members,
                real_input_index,
                input_secret: InputSecret {
                    onetime_key_derive_data,
                    amount: Amount::new(value, TokenId::from(token_id)),
                    blinding,
                },
            });

            tx_prefix.inputs.push(TxIn::default());
        }

        // Create one output with the same value as each input
        let output_secrets: Vec<_> = rings
            .iter()
            .map(|ring| {
                let blinding = Scalar::random(&mut rng);
                OutputSecret {
                    amount: ring.input_secret.amount,
                    blinding,
                }
            })
            .collect();

        SignatureParams {
            tx_prefix,
            rings,
            output_secrets,
            block_version,
        }
    }

    fn get_output_commitments(&self) -> Vec<CompressedCommitment> {
        self.output_secrets
            .iter()
            .map(|secret| {
                CompressedCommitment::new(
                    secret.amount.value,
                    secret.blinding,
                    &generators(*secret.amount.token_id),
                )
            })
            .collect()
    }

    fn get_signed_input_rings(&self) -> Vec<SignedInputRing> {
        self.rings.iter().map(SignedInputRing::from).collect()
    }

    fn get_input_rings(&self) -> Vec<InputRing> {
        self.rings
            .iter()
            .cloned()
            .map(InputRing::Signable)
            .collect()
    }

    fn get_fee_amount(&self) -> Amount {
        Amount::new(
            self.tx_prefix.fee,
            TokenId::from(self.tx_prefix.fee_token_id),
        )
    }

    fn sign(&self, seed: u64) -> SignatureRctBulletproofs {
        let mut rng = StdRng::seed_from_u64(seed);
        SignatureRctBulletproofs::sign(
            self.block_version,
            &self.tx_prefix,
            &self.get_input_rings(),
            &self.output_secrets,
            self.get_fee_amount(),
            &NoKeysRingSigner {},
            &mut rng,
        )
        .expect("signing should succeed")
    }
}

/// Benchmark range proof generation with different numbers of outputs
fn bench_range_proof_generate(c: &mut Criterion) {
    let mut group = c.benchmark_group("Range proof generate");

    for num_outputs in [1, 2, 4, 8, 16] {
        let mut rng = StdRng::seed_from_u64(42);
        let generator = generators(0);

        let values: Vec<u64> = (0..num_outputs).map(|_| rng.next_u64()).collect();
        let blindings: Vec<Scalar> = (0..num_outputs).map(|_| Scalar::random(&mut rng)).collect();

        group.bench_with_input(
            BenchmarkId::new("num_outputs", num_outputs),
            &num_outputs,
            |b, _| {
                let mut bench_rng = StdRng::seed_from_u64(12345);
                b.iter(|| {
                    black_box(
                        generate_range_proofs(&values, &blindings, &generator, &mut bench_rng)
                            .unwrap(),
                    )
                })
            },
        );
    }
    group.finish();
}

/// Benchmark range proof verification with different numbers of outputs
fn bench_range_proof_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("Range proof verify");

    for num_outputs in [1, 2, 4, 8, 16] {
        let mut rng = StdRng::seed_from_u64(42);
        let generator = generators(0);

        let values: Vec<u64> = (0..num_outputs).map(|_| rng.next_u64()).collect();
        let blindings: Vec<Scalar> = (0..num_outputs).map(|_| Scalar::random(&mut rng)).collect();

        let (proof, commitments) =
            generate_range_proofs(&values, &blindings, &generator, &mut rng).unwrap();

        group.bench_with_input(
            BenchmarkId::new("num_outputs", num_outputs),
            &num_outputs,
            |b, _| {
                let mut bench_rng = StdRng::seed_from_u64(12345);
                b.iter(|| {
                    black_box(check_range_proofs(
                        &proof,
                        &commitments,
                        &generator,
                        &mut bench_rng,
                    ))
                })
            },
        );
    }
    group.finish();
}

/// Benchmark RctBulletproofs signing with different input counts
fn bench_rct_bulletproofs_sign(c: &mut Criterion) {
    let mut group = c.benchmark_group("RctBulletproofs sign");
    let num_mixins = 10; // Standard ring size of 11

    for num_inputs in [1, 2, 4, 8] {
        let params = SignatureParams::random(BlockVersion::THREE, num_inputs, num_mixins, 42);

        group.bench_with_input(
            BenchmarkId::new("num_inputs", num_inputs),
            &num_inputs,
            |b, _| {
                let mut seed = 12345u64;
                b.iter(|| {
                    seed += 1;
                    black_box(params.sign(seed))
                })
            },
        );
    }
    group.finish();
}

/// Benchmark RctBulletproofs verification with different input counts
fn bench_rct_bulletproofs_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("RctBulletproofs verify");
    let num_mixins = 10; // Standard ring size of 11

    for num_inputs in [1, 2, 4, 8] {
        let params = SignatureParams::random(BlockVersion::THREE, num_inputs, num_mixins, 42);
        let signature = params.sign(12345);

        group.bench_with_input(
            BenchmarkId::new("num_inputs", num_inputs),
            &num_inputs,
            |b, _| {
                let mut rng = StdRng::seed_from_u64(67890);
                b.iter(|| {
                    black_box(signature.verify(
                        params.block_version,
                        &params.tx_prefix,
                        &params.get_signed_input_rings(),
                        &params.get_output_commitments(),
                        params.get_fee_amount(),
                        &mut rng,
                    ))
                })
            },
        );
    }
    group.finish();
}

/// Benchmark full transaction validation (the hot path for block validation)
fn bench_tx_validation_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("Full tx validation");
    let num_mixins = 10;

    // Typical transaction sizes
    for (num_inputs, num_outputs) in [(1, 2), (2, 2), (4, 4)] {
        let mut params = SignatureParams::random(BlockVersion::THREE, num_inputs, num_mixins, 42);

        // Adjust outputs to match requested count
        while params.output_secrets.len() > num_outputs {
            params.output_secrets.pop();
        }

        let signature = params.sign(12345);
        let label = format!("{num_inputs}in_{num_outputs}out");

        group.bench_with_input(BenchmarkId::new("tx_size", &label), &label, |b, _| {
            let mut rng = StdRng::seed_from_u64(67890);
            b.iter(|| {
                black_box(signature.verify(
                    params.block_version,
                    &params.tx_prefix,
                    &params.get_signed_input_rings(),
                    &params.get_output_commitments(),
                    params.get_fee_amount(),
                    &mut rng,
                ))
            })
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_range_proof_generate,
    bench_range_proof_verify,
    bench_rct_bulletproofs_sign,
    bench_rct_bulletproofs_verify,
    bench_tx_validation_full,
);

criterion_main!(benches);
