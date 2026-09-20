//! Ordinary library composition on synthetic data only. No transaction codec.
use super::*;
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::{compat::random_scalar, Clsag, ReducedTxOut};
use bth_util_from_random::FromRandom;
use rand::{rngs::StdRng, SeedableRng};

const RING_SIZE: usize = 20;
const STATEMENT_DOMAIN: &[u8] = b"botho/inactive/ownership-statement-v1";
const ARITHMETIC_DOMAIN: &[u8] = b"botho/inactive/ownership-arithmetic-v1";
const PROOF_DOMAIN: &[u8] = b"botho/inactive/ownership-proof-v1";
const MESSAGE_DOMAIN: &[u8] = b"botho/inactive/ownership-message-v1";

#[derive(Clone)]
struct Public {
    arithmetic: Statement,
    rings: Vec<Vec<ReducedTxOut>>,
}
struct Secret {
    arithmetic: CombinedWitness,
    keys: Vec<RistrettoPrivate>,
    ring_blinds: Vec<curve25519_dalek::Scalar>,
    real_indices: Vec<usize>,
}
struct Envelope {
    arithmetic_proof: Vec<u8>,
    signatures: Vec<Clsag>,
}

fn digest(mut t: Transcript) -> [u8; 32] {
    let mut bytes = [0; 32];
    t.challenge_bytes(b"digest", &mut bytes);
    bytes
}

impl Public {
    fn check_shape(&self) -> Result<(), String> {
        self.arithmetic.audit()?;
        if self.rings.len() != self.arithmetic.inputs.len()
            || self.rings.iter().any(|r| r.len() != RING_SIZE)
        {
            return Err("ownership ring shape".into());
        }
        Ok(())
    }

    fn statement_digest(&self) -> Result<[u8; 32], String> {
        // Hashing is not validation. Keep a length guard for indexed ring access;
        // all proof entry points separately enforce the full supported statement.
        if self.rings.len() != self.arithmetic.inputs.len() {
            return Err("ownership ring count".into());
        }
        let mut t = Transcript::new(STATEMENT_DOMAIN);
        // Exhaustive destructuring makes newly added fields require an explicit
        // binding decision at compile time. This is not a serialization format.
        let Statement {
            version,
            network,
            genesis,
            parent,
            policy,
            body,
            height,
            expiry,
            significant_bits,
            base_unit,
            fee,
            inputs,
            outputs,
        } = &self.arithmetic;
        for (label, bytes) in [
            (b"network".as_slice(), network),
            (b"genesis", genesis),
            (b"parent", parent),
            (b"policy", policy),
            (b"fixture-body", body),
        ] {
            t.append_message(label, bytes);
        }
        for (label, value) in [
            (b"version".as_slice(), *version),
            (b"height", *height),
            (b"expiry", *expiry),
            (b"significant-bits", *significant_bits as u64),
            (b"base-unit", *base_unit),
            (b"fee", *fee),
            (b"input-count", inputs.len() as u64),
            (b"output-count", outputs.len() as u64),
        ] {
            t.append_u64(label, value);
        }
        for (index, input) in inputs.iter().enumerate() {
            t.append_u64(b"input-index", index as u64);
            let PublicStatement {
                version,
                network,
                context,
                token,
                input_factor,
                output_factor,
                elapsed,
                rate,
                year,
                horizon,
                amount_commitment,
                charge_commitment,
            } = input;
            for (label, value) in [
                (b"input-version".as_slice(), *version),
                (b"token", *token),
                (b"input-factor", *input_factor),
                (b"output-factor", *output_factor),
                (b"elapsed", *elapsed),
                (b"rate", *rate as u64),
                (b"year", *year),
                (b"horizon", *horizon),
            ] {
                t.append_u64(label, value);
            }
            for (label, bytes) in [
                (b"input-network".as_slice(), network),
                (b"input-context", context),
                (b"amount-commitment", amount_commitment),
                (b"charge-commitment", charge_commitment),
            ] {
                t.append_message(label, bytes);
            }
            let ring = &self.rings[index];
            t.append_u64(b"ring-count", ring.len() as u64);
            for (member_index, member) in ring.iter().enumerate() {
                let ReducedTxOut {
                    public_key,
                    target_key,
                    commitment,
                } = member;
                t.append_u64(b"member-index", member_index as u64);
                t.append_message(b"public-key", public_key.as_ref());
                t.append_message(b"target-key", target_key.as_ref());
                t.append_message(b"commitment", commitment.as_ref());
            }
        }
        for (index, output) in outputs.iter().enumerate() {
            t.append_u64(b"output-index", index as u64);
            t.append_message(b"output-commitment", output);
        }
        Ok(digest(t))
    }

    fn arithmetic_transcript(&self) -> Result<Transcript, String> {
        self.check_shape()?;
        let mut t = Transcript::new(ARITHMETIC_DOMAIN);
        t.append_message(b"statement", &self.statement_digest()?);
        Ok(t)
    }
}

fn proof_digest(bytes: &[u8]) -> [u8; 32] {
    let mut t = Transcript::new(PROOF_DOMAIN);
    t.append_message(b"proof", bytes);
    digest(t)
}
fn message(statement: &[u8; 32], proof: &[u8; 32], index: usize) -> [u8; 32] {
    let mut t = Transcript::new(MESSAGE_DOMAIN);
    t.append_message(b"statement", statement);
    t.append_message(b"proof", proof);
    t.append_u64(b"input-index", index as u64);
    digest(t)
}
fn project_scalar(s: Scalar) -> curve25519_dalek::Scalar {
    Option::from(curve25519_dalek::Scalar::from_canonical_bytes(s.to_bytes())).unwrap()
}
fn pseudo(public: &Public, i: usize) -> CompressedCommitment {
    // The exact arithmetic P_i is the only source of the pseudo-output bytes.
    CompressedCommitment::try_from(&public.arithmetic.inputs[i].amount_commitment).unwrap()
}
fn sign_proof(public: &Public, secret: &Secret, proof: &[u8]) -> Result<Vec<Clsag>, String> {
    public.check_shape()?;
    let statement = public.statement_digest()?;
    let hash = proof_digest(proof);
    let mut rng = StdRng::try_from_rng(&mut bth_util_from_random::OsRng).unwrap();
    public
        .rings
        .iter()
        .enumerate()
        .map(|(i, ring)| {
            let w = &secret.arithmetic.inputs[i];
            Clsag::sign(
                &message(&statement, &hash, i),
                ring,
                secret.real_indices[i],
                &secret.keys[i],
                w.amount,
                &secret.ring_blinds[i],
                &project_scalar(w.amount_blind),
                &generators(0),
                &mut rng,
            )
            .map_err(|e| format!("ownership sign: {e:?}"))
        })
        .collect()
}
// This helper checks only signature association. Only verify() composes both
// proofs.
fn verify_ownership(public: &Public, e: &Envelope) -> Result<(), String> {
    public.check_shape()?;
    let statement = public.statement_digest()?;
    if e.signatures.len() != public.rings.len() {
        return Err("ownership signature count".into());
    }
    let hash = proof_digest(&e.arithmetic_proof);
    for (i, signature) in e.signatures.iter().enumerate() {
        signature
            .verify(
                &message(&statement, &hash, i),
                &public.rings[i],
                &pseudo(public, i),
            )
            .map_err(|e| format!("ownership verify: {e:?}"))?;
    }
    Ok(())
}
fn prove(public: &Public, secret: &Secret, bp: &BulletproofGens) -> Result<Envelope, String> {
    let (arithmetic_proof, _) = prove_control_with_transcript(
        &public.arithmetic,
        &secret.arithmetic,
        bp,
        Controls::COMPLETE,
        public.arithmetic_transcript()?,
    )?;
    let signatures = sign_proof(public, secret, &arithmetic_proof)?;
    Ok(Envelope {
        arithmetic_proof,
        signatures,
    })
}
fn verify(public: &Public, e: &Envelope, bp: &BulletproofGens) -> Result<(), String> {
    // Cheap public shape checks before expensive proof verification.
    public.check_shape()?;
    if e.signatures.len() != public.rings.len() {
        return Err("ownership signature count".into());
    }
    verify_control_with_transcript(
        &public.arithmetic,
        &e.arithmetic_proof,
        bp,
        Controls::COMPLETE,
        public.arithmetic_transcript()?,
    )?;
    verify_ownership(public, e)
}

fn fixture(count: usize) -> (Public, Secret) {
    let values: Vec<_> = (0..count)
        .map(|i| 1_000_000_000_000 + 1000 * i as u64)
        .collect();
    let (arithmetic, witness) = fixture_combined(
        &values,
        count,
        3,
        250_000_000_000,
        (6000, 3500, 1_234_567, 200, 6_307_200, 31_536_000),
    )
    .unwrap();
    assert!(witness.inputs.iter().all(|w| w.charge > 0));
    assert_eq!(
        arithmetic.inputs[0].gens().B.compress().to_bytes(),
        generators(0).B.compress().to_bytes()
    );
    assert_eq!(
        arithmetic.inputs[0].gens().B_blinding.compress().to_bytes(),
        generators(0).B_blinding.compress().to_bytes()
    );
    let mut rng = StdRng::seed_from_u64(1346);
    let mut public = Public {
        arithmetic,
        rings: vec![],
    };
    let mut secret = Secret {
        arithmetic: witness,
        keys: vec![],
        ring_blinds: vec![],
        real_indices: vec![],
    };
    for (i, value) in values.iter().enumerate() {
        let key = RistrettoPrivate::from_random(&mut rng);
        let blind = random_scalar(&mut rng);
        let real = (i + 7) % RING_SIZE;
        let mut ring: Vec<_> = (0..RING_SIZE)
            .map(|j| ReducedTxOut {
                public_key: CompressedRistrettoPublic::from_random(&mut rng),
                target_key: CompressedRistrettoPublic::from_random(&mut rng),
                commitment: CompressedCommitment::new(
                    value + j as u64,
                    random_scalar(&mut rng),
                    &generators(0),
                ),
            })
            .collect();
        ring[real].target_key = CompressedRistrettoPublic::from(RistrettoPublic::from(&key));
        ring[real].commitment = CompressedCommitment::new(*value, blind, &generators(0));
        assert_ne!(
            blind,
            project_scalar(secret.arithmetic.inputs[i].amount_blind)
        );
        assert_eq!(
            CompressedCommitment::new(
                *value,
                project_scalar(secret.arithmetic.inputs[i].amount_blind),
                &generators(0)
            ),
            pseudo(&public, i)
        );
        public.rings.push(ring);
        secret.keys.push(key);
        secret.ring_blinds.push(blind);
        secret.real_indices.push(real);
    }
    (public, secret)
}

#[test]
fn ownership_one_and_sixteen_round_trips() {
    let now = Instant::now();
    let bp = BulletproofGens::new(32768, 1);
    println!("ownership generator_setup={:?}", now.elapsed());
    for count in [1, 16] {
        let (public, secret) = fixture(count);
        let now = Instant::now();
        let mut e = prove(&public, &secret, &bp).unwrap();
        let prove_time = now.elapsed();
        let now = Instant::now();
        verify(&public, &e, &bp).unwrap();
        let verify_time = now.elapsed();
        if count == 16 {
            e.signatures.swap(0, 1);
            assert!(verify_ownership(&public, &e).is_err());
            e.signatures.swap(0, 1);
            let mut reordered = public.clone();
            reordered.arithmetic.inputs.swap(0, 1);
            reordered.rings.swap(0, 1);
            assert!(verify(&reordered, &e, &bp).is_err());
        }
        // Field bytes only: c0 + responses + two images. No transport framing.
        let clsag_fields: usize = e
            .signatures
            .iter()
            .map(|s| 32 * (s.responses.len() + 3))
            .sum();
        assert_eq!(clsag_fields, count * 736);
        println!("ownership inputs={count} ring=20 arithmetic_bytes={} clsag_field_bytes={clsag_fields} prove={prove_time:?} verify={verify_time:?}", e.arithmetic_proof.len());
    }
}

#[test]
fn ownership_two_valid_proofs_require_resigning() {
    let bp = BulletproofGens::new(2048, 1);
    let (public, secret) = fixture(1);
    let mut first = prove(&public, &secret, &bp).unwrap();
    verify(&public, &first, &bp).unwrap();
    let second = prove(&public, &secret, &bp).unwrap();
    verify(&public, &second, &bp).unwrap();
    assert_ne!(first.arithmetic_proof, second.arithmetic_proof);
    first.arithmetic_proof = second.arithmetic_proof;
    assert!(verify_ownership(&public, &first).is_err());
    assert!(verify(&public, &first, &bp).is_err());
    first.signatures = sign_proof(&public, &secret, &first.arithmetic_proof).unwrap();
    verify(&public, &first, &bp).unwrap();
}

#[test]
fn ownership_statement_field_coverage() {
    let (public, _) = fixture(2);
    let original = public.statement_digest().unwrap();
    // Binding independently of arithmetic admissibility: individual base/fee
    // changes can make a bucket noncanonical. Shape rejection is tested below.
    type Mutation = (&'static str, fn(&mut Public));
    let mutations: &[Mutation] = &[
        ("version", |p| p.arithmetic.version += 1),
        ("height", |p| p.arithmetic.height += 1),
        ("expiry", |p| p.arithmetic.expiry += 1),
        ("token", |p| p.arithmetic.inputs[0].token = 1),
        ("network", |p| p.arithmetic.network[0] ^= 1),
        ("genesis", |p| p.arithmetic.genesis[0] ^= 1),
        ("parent", |p| p.arithmetic.parent[0] ^= 1),
        ("policy", |p| p.arithmetic.policy[0] ^= 1),
        ("fixture body", |p| p.arithmetic.body[0] ^= 1),
        ("height and expiry", |p| {
            p.arithmetic.height += 1;
            p.arithmetic.expiry += 1;
        }),
        ("significant bits", |p| p.arithmetic.significant_bits = 4),
        ("base", |p| p.arithmetic.base_unit -= 1),
        ("fee", |p| p.arithmetic.fee += 1),
        ("input version", |p| p.arithmetic.inputs[0].version += 1),
        ("input network", |p| p.arithmetic.inputs[0].network[0] ^= 1),
        ("input context", |p| p.arithmetic.inputs[0].context[0] ^= 1),
        ("input factor", |p| p.arithmetic.inputs[0].input_factor -= 1),
        ("output factor", |p| {
            p.arithmetic.inputs[0].output_factor += 1
        }),
        ("elapsed", |p| p.arithmetic.inputs[0].elapsed += 1),
        ("rate", |p| p.arithmetic.inputs[0].rate += 1),
        ("year", |p| p.arithmetic.inputs[0].year += 1),
        ("horizon", |p| p.arithmetic.inputs[0].horizon += 1),
        ("amount", |p| {
            p.arithmetic.inputs[0].amount_commitment = p.arithmetic.inputs[1].amount_commitment
        }),
        ("charge", |p| {
            p.arithmetic.inputs[0].charge_commitment = p.arithmetic.inputs[1].charge_commitment
        }),
        ("output", |p| p.arithmetic.outputs.swap(0, 1)),
        ("input order", |p| p.arithmetic.inputs.swap(0, 1)),
        ("ring order", |p| p.rings.swap(0, 1)),
        ("member order", |p| p.rings[0].swap(0, 1)),
        ("member public key", |p| {
            p.rings[0][0].public_key = p.rings[0][1].public_key
        }),
        ("member target key", |p| {
            p.rings[0][0].target_key = p.rings[0][1].target_key
        }),
        ("member commitment", |p| {
            p.rings[0][0].commitment = p.rings[0][1].commitment
        }),
        ("input count", |p| {
            p.arithmetic.inputs.pop();
            p.rings.pop();
        }),
        ("output count", |p| {
            p.arithmetic.outputs.pop();
        }),
    ];
    for (name, mutate) in mutations {
        let mut changed = public.clone();
        mutate(&mut changed);
        assert_ne!(
            changed
                .statement_digest()
                .unwrap_or_else(|e| panic!("{name}: {e}")),
            original,
            "{name}"
        );
    }
    for mutate in [
        (|p: &mut Public| p.arithmetic.version += 1) as fn(&mut Public),
        |p| p.arithmetic.inputs[0].token = 1,
        |p| p.arithmetic.expiry += 1,
        |p| {
            p.rings[0].pop();
        },
        |p| {
            p.rings.pop();
        },
        |p| p.arithmetic.inputs.clear(),
        |p| p.arithmetic.outputs.clear(),
    ] {
        let mut changed = public.clone();
        mutate(&mut changed);
        assert!(changed.check_shape().is_err());
    }
    let hash = [3; 32];
    assert_ne!(message(&original, &hash, 0), message(&original, &hash, 1));
    assert_ne!(proof_digest(&[1, 2]), proof_digest(&[1, 2, 0]));
}

#[test]
fn ownership_public_binding_and_detached_pseudo_rejections() {
    let bp = BulletproofGens::new(2048, 1);
    let (public, secret) = fixture(1);
    let mut e = prove(&public, &secret, &bp).unwrap();
    verify(&public, &e, &bp).unwrap();
    // Same arithmetic remains valid, but the public transcript has changed.
    for mutate in [
        (|p: &mut Public| p.arithmetic.network[0] ^= 1) as fn(&mut Public),
        |p| p.arithmetic.policy[0] ^= 1,
        |p| p.arithmetic.body[0] ^= 1,
        |p| p.rings[0].swap(0, 1),
        |p| p.rings[0][0].public_key = p.rings[0][1].public_key,
    ] {
        let mut changed = public.clone();
        mutate(&mut changed);
        assert!(verify_control_with_transcript(
            &changed.arithmetic,
            &e.arithmetic_proof,
            &bp,
            Controls::COMPLETE,
            changed.arithmetic_transcript().unwrap()
        )
        .is_err());
        assert!(verify_ownership(&changed, &e).is_err());
        assert!(verify(&changed, &e, &bp).is_err());
    }
    let original = e.signatures[0].clone();
    e.signatures.clear();
    assert_eq!(
        verify(&public, &e, &bp).unwrap_err(),
        "ownership signature count"
    );
    e.signatures = vec![original.clone(), original.clone()];
    assert_eq!(
        verify(&public, &e, &bp).unwrap_err(),
        "ownership signature count"
    );

    // A perfectly valid ordinary CLSAG proof for a different pseudo-output
    // cannot stand in for ownership of the R1CS input P_i.
    let w = &secret.arithmetic.inputs[0];
    let alternate_blind = project_scalar(w.amount_blind) + curve25519_dalek::Scalar::ONE;
    let alternate_pseudo = CompressedCommitment::new(w.amount, alternate_blind, &generators(0));
    let msg = message(
        &public.statement_digest().unwrap(),
        &proof_digest(&e.arithmetic_proof),
        0,
    );
    let mut rng = StdRng::try_from_rng(&mut bth_util_from_random::OsRng).unwrap();
    let alternate = Clsag::sign(
        &msg,
        &public.rings[0],
        secret.real_indices[0],
        &secret.keys[0],
        w.amount,
        &secret.ring_blinds[0],
        &alternate_blind,
        &generators(0),
        &mut rng,
    )
    .unwrap();
    alternate
        .verify(&msg, &public.rings[0], &alternate_pseudo)
        .unwrap();
    e.signatures = vec![alternate];
    assert!(verify_ownership(&public, &e).is_err());
    assert!(verify(&public, &e, &bp).is_err());
    e.signatures = vec![original];
    verify(&public, &e, &bp).unwrap();
}
