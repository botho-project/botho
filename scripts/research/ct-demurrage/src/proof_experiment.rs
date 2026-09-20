//! Actual randomized proof experiment. All of this module is test-only.
mod combined;
use super::*;
use bth_crypto_ring_signature::{generators, CompressedCommitment};
use bulletproofs_og::{r1cs::R1CSProof, BulletproofGens};
use curve25519_dalek_v4::ristretto::CompressedRistretto;
use rand_chacha_fixture::ChaCha20Rng;
use rand_core_fixture::SeedableRng;
use std::time::Instant;

const DOMAIN: &[u8] = b"botho/inactive/ct-demurrage-r1cs";

#[derive(Clone, Debug)]
struct PublicStatement {
    version: u64,
    network: [u8; 32],
    context: [u8; 32],
    token: u64,
    input_factor: u64,
    output_factor: u64,
    elapsed: u64,
    rate: u32,
    year: u64,
    horizon: u64,
    amount_commitment: [u8; 32],
    charge_commitment: [u8; 32],
}
impl PublicStatement {
    fn transcript(&self, bindings: Bindings) -> Transcript {
        let mut t = Transcript::new(DOMAIN);
        // UNSAFE OMIT mode exists exclusively as a negative-test control.
        if bindings.statement {
            t.append_u64(b"experiment-version", self.version);
            t.append_message(b"network", &self.network);
            t.append_message(b"context", &self.context);
            t.append_u64(b"token", self.token);
            t.append_u64(b"input-factor", self.input_factor);
            t.append_u64(b"output-factor", self.output_factor);
            t.append_u64(b"elapsed", self.elapsed);
            t.append_u64(b"rate", self.rate as u64);
            t.append_u64(b"year", self.year);
            t.append_u64(b"horizon", self.horizon);
            t.append_message(b"amount-commitment", &self.amount_commitment);
            t.append_message(b"charge-commitment", &self.charge_commitment);
        }
        t
    }
    fn gens(&self) -> PedersenGens {
        let project = generators(self.token);
        // Same canonical point boundary as transaction/core range_proofs::convert_gens.
        PedersenGens {
            B: CompressedRistretto(project.B.compress().to_bytes())
                .decompress()
                .unwrap(),
            B_blinding: CompressedRistretto(project.B_blinding.compress().to_bytes())
                .decompress()
                .unwrap(),
        }
    }
}

/// There is no witness value (or amount argument) in this type or its builder.
#[derive(Debug, Default)]
struct CircuitLayout {
    wires: Vec<(String, usize)>,
    equations: Vec<Equation>,
    products: Vec<Product>,
    amount: usize,
    charge: usize,
}
impl CircuitLayout {
    fn wire(&mut self, name: &str, bits: usize) -> usize {
        let i = self.wires.len();
        self.wires.push((name.into(), bits));
        i
    }
    fn eq(&mut self, name: &str, left: Expr, right: Expr) {
        self.equations.push(Equation {
            name: name.into(),
            left,
            right,
        });
    }
    fn product(&mut self, name: &str, left: Expr, right: Expr, output: Expr) {
        self.products.push(Product {
            name: name.into(),
            left,
            right,
            output,
        });
    }
    fn annual(&mut self, label: &str, factor: u64, rate: u32) -> usize {
        let a = self.wire(&format!("{label}.a"), 83);
        let r = self.wire(&format!("{label}.r"), 26);
        let bar = self.wire(&format!("{label}.rbar"), 26);
        self.eq(
            label,
            Expr::wire(self.amount).scale(n(rate as u64) * n(factor.clamp(1000, 6000) - 1000)),
            Expr::wire(a).scale(n(K)).add(Expr::wire(r)),
        );
        self.eq(
            &format!("{label}.remainder"),
            Expr::wire(r).add(Expr::wire(bar)),
            Expr::constant(K - 1),
        );
        a
    }
    fn temporal(&mut self, label: &str, a: usize, elapsed: u64, year: u64) -> usize {
        let t = if year == 0 {
            n(0)
        } else {
            n(elapsed) * S / year
        };
        let c = self.wire(&format!("{label}.c"), 64);
        let excess = self.wire(&format!("{label}.excess"), 147);
        let r = self.wire(&format!("{label}.r"), 20);
        let bar = self.wire(&format!("{label}.rbar"), 20);
        self.eq(
            label,
            Expr::wire(a).scale(t),
            Expr::wire(c)
                .add(Expr::wire(excess))
                .scale(n(S))
                .add(Expr::wire(r)),
        );
        self.eq(
            &format!("{label}.remainder"),
            Expr::wire(r).add(Expr::wire(bar)),
            Expr::constant(S - 1),
        );
        self.product(
            &format!("{label}.cap"),
            Expr::wire(c),
            Expr::wire(excess),
            Expr::wire(excess).scale(n(M)),
        );
        c
    }
    fn new(s: &PublicStatement) -> Self {
        let mut c = Self::default();
        c.amount = c.wire("value", 64);
        let ai = c.annual("annual.in", s.input_factor, s.rate);
        let ao = c.annual("annual.out", s.output_factor, s.rate);
        let accrued = c.temporal("accrued", ai, s.elapsed, s.year);
        let ci = c.temporal("capital.in", ai, s.horizon, s.year);
        let co = c.temporal("capital.out", ao, s.horizon, s.year);
        let d = c.wire("difference", 64);
        let j = c.wire("reverse_difference", 64);
        c.eq(
            "difference",
            Expr::wire(ci).add(Expr::wire(j)),
            Expr::wire(co).add(Expr::wire(d)),
        );
        c.product(
            "difference",
            Expr::wire(d),
            Expr::wire(j),
            Expr::constant(0),
        );
        c.charge = c.wire("settled", 64);
        let x = c.wire("max.accrued_gap", 64);
        let y = c.wire("max.capital_gap", 64);
        c.eq(
            "max.accrued",
            Expr::wire(c.charge),
            Expr::wire(accrued).add(Expr::wire(x)),
        );
        c.eq(
            "max.capital",
            Expr::wire(c.charge),
            Expr::wire(d).add(Expr::wire(y)),
        );
        c.product("max", Expr::wire(x), Expr::wire(y), Expr::constant(0));
        c
    }
    fn allocate<CS: ConstraintSystem>(
        &self,
        cs: &mut CS,
        value: LinearCombination,
        charge: LinearCombination,
        witness: Option<&Witness>,
        bindings: Bindings,
    ) {
        let mut vars = Vec::new();
        for (i, (_, bits)) in self.wires.iter().enumerate() {
            let mut lc = LinearCombination::default();
            for bit in 0..*bits {
                let assignment = witness.map(|w| {
                    let b = Scalar::from(u64::from(w.values[i].bit(bit as u64)));
                    (b, Scalar::ONE - b)
                });
                let (a, b, o) = cs.allocate_multiplier(assignment).unwrap();
                cs.constrain(o.into());
                cs.constrain(a + b - Scalar::ONE);
                lc = lc + a * scalar(&(n(1) << bit));
            }
            vars.push(lc);
        }
        if bindings.amount {
            cs.constrain(vars[self.amount].clone() - value);
        }
        if bindings.charge {
            cs.constrain(vars[self.charge].clone() - charge);
        }
        for e in &self.equations {
            cs.constrain(e.left.lc(&vars) - e.right.lc(&vars));
        }
        for p in &self.products {
            let (_, _, o) = cs.multiply(p.left.lc(&vars), p.right.lc(&vars));
            cs.constrain(o - p.output.lc(&vars));
        }
    }
    fn assert_reference_parity(&self, c: &Candidate) {
        assert_eq!(
            self.wires,
            c.wires
                .iter()
                .map(|w| (w.name.clone(), w.bits))
                .collect::<Vec<_>>()
        );
        assert_eq!(self.equations, c.equations);
        assert_eq!(self.products, c.products);
        assert_eq!((self.amount, self.charge), (c.amount, c.charge));
    }
}

#[derive(Clone)]
struct Witness {
    values: Vec<BigUint>,
    amount: u64,
    charge: u64,
    amount_blind: Scalar,
    charge_blind: Scalar,
}
#[derive(Clone, Copy)]
struct Bindings {
    statement: bool,
    amount: bool,
    charge: bool,
}
impl Bindings {
    const COMPLETE: Self = Self {
        statement: true,
        amount: true,
        charge: true,
    };
}

fn project_commit(value: u64, blind: Scalar, token: u64) -> [u8; 32] {
    let v5 = Option::<curve25519_dalek::Scalar>::from(
        curve25519_dalek::Scalar::from_canonical_bytes(blind.to_bytes()),
    )
    .unwrap();
    *CompressedCommitment::new(value, v5, &generators(token)).as_ref()
}
fn fixture(
    value: u64,
    fi: u64,
    fo: u64,
    e: u64,
    rate: u32,
    year: u64,
    horizon: u64,
) -> (PublicStatement, Witness) {
    let c = Candidate::construct(value, fi, fo, e, rate, year, horizon);
    c.check(value).unwrap();
    // Reproducible OPENINGS for tests only. Upstream proof randomness is untouched.
    let mut rng = ChaCha20Rng::from_seed([0x85; 32]);
    let amount_blind = Scalar::random(&mut rng);
    let charge_blind = Scalar::random(&mut rng);
    let charge = c.result("settled");
    assert_eq!(
        charge,
        bth_cluster_tax::demurrage_charge(value, fi, e, rate, year).max(
            bth_cluster_tax::capitalized_reset_charge(value, fi, fo, horizon, rate, year)
        )
    );
    let s = PublicStatement {
        version: 1,
        network: [0x11; 32],
        context: [0x22; 32],
        token: 0,
        input_factor: fi,
        output_factor: fo,
        elapsed: e,
        rate,
        year,
        horizon,
        amount_commitment: project_commit(value, amount_blind, 0),
        charge_commitment: project_commit(charge, charge_blind, 0),
    };
    CircuitLayout::new(&s).assert_reference_parity(&c);
    let w = Witness {
        values: c.wires.into_iter().map(|w| w.value).collect(),
        amount: value,
        charge,
        amount_blind,
        charge_blind,
    };
    (s, w)
}

/// Intentionally does NOT call the integer checker: invalid-witness tests must
/// exercise cryptographic rejection, not get stopped by the reference oracle.
fn prove_control(
    s: &PublicStatement,
    w: &Witness,
    bp: &BulletproofGens,
    bindings: Bindings,
) -> Result<(Vec<u8>, Metrics), String> {
    let layout = CircuitLayout::new(s);
    if w.values.len() != layout.wires.len()
        || w.values
            .iter()
            .zip(&layout.wires)
            .any(|(v, (_, bits))| v > &max(*bits))
    {
        return Err("witness length/range".into());
    }
    let gens = s.gens();
    let mut t = s.transcript(bindings);
    let mut p = Prover::new(&gens, &mut t);
    let (cv, v) = p.commit(Scalar::from(w.amount), w.amount_blind);
    let (cz, z) = p.commit(Scalar::from(w.charge), w.charge_blind);
    if cv.to_bytes() != s.amount_commitment || cz.to_bytes() != s.charge_commitment {
        return Err("opening does not match project commitment".into());
    }
    layout.allocate(&mut p, v.into(), z.into(), Some(w), bindings);
    let metrics = p.metrics();
    p.prove(bp)
        .map(|p| (p.to_bytes(), metrics))
        .map_err(|e| e.to_string())
}
/// Public-only verification: no reference Candidate, values, openings, or
/// selectors.
fn verify_control(
    s: &PublicStatement,
    bytes: &[u8],
    bp: &BulletproofGens,
    bindings: Bindings,
) -> Result<Metrics, String> {
    let proof = R1CSProof::from_bytes(bytes).map_err(|e| e.to_string())?;
    let mut t = s.transcript(bindings);
    let mut v = Verifier::new(&mut t);
    let cv = v.commit(CompressedRistretto(s.amount_commitment));
    let cz = v.commit(CompressedRistretto(s.charge_commitment));
    CircuitLayout::new(s).allocate(&mut v, cv.into(), cz.into(), None, bindings);
    let metrics = v.metrics();
    v.verify(&proof, &s.gens(), bp).map_err(|e| e.to_string())?;
    Ok(metrics)
}

fn prove(
    s: &PublicStatement,
    w: &Witness,
    bp: &BulletproofGens,
) -> Result<(Vec<u8>, Metrics), String> {
    prove_control(s, w, bp, Bindings::COMPLETE)
}
fn verify(s: &PublicStatement, bytes: &[u8], bp: &BulletproofGens) -> Result<Metrics, String> {
    verify_control(s, bytes, bp, Bindings::COMPLETE)
}

#[test]
fn proof_round_trips_and_measurements() {
    let bp = BulletproofGens::new(2048, 1);
    for (name, v, fi, fo, e, r, y, h) in [
        (
            "ordinary", 10_000, 6000, 3500, 1_234_567, 200, 6_307_200, 31_536_000,
        ),
        (
            "zero-rate",
            10_000,
            6000,
            3500,
            1_234_567,
            0,
            6_307_200,
            31_536_000,
        ),
        ("zero-time", 10_000, 6000, 3500, 0, 200, 6_307_200, 0),
        ("zero-year", 10_000, 6000, 3500, M, 200, 0, 31_536_000),
        (
            "floor-boundary",
            51,
            6000,
            3500,
            0,
            200,
            6_307_200,
            31_536_000,
        ),
        ("maximum-domain", M, 6000, 5999, M, u32::MAX, 1, M),
        (
            "both-caps-cancel",
            M,
            6000,
            5999,
            0,
            u32::MAX,
            1,
            31_536_000,
        ),
        ("one-cap-difference-one", 100, 6000, 3500, 0, 200, 1, M - 1),
    ] {
        let (s, w) = fixture(v, fi, fo, e, r, y, h);
        let now = Instant::now();
        let (proof, p) = prove(&s, &w, &bp).unwrap();
        let pt = now.elapsed();
        let now = Instant::now();
        let metrics = verify(&s, &proof, &bp).unwrap();
        let vt = now.elapsed();
        println!("{name}: bytes={} prove={pt:?} verify={vt:?}", proof.len());
        assert_eq!(p.multipliers, 1412);
        assert_eq!(p.constraints, 2844);
        assert_eq!(p.multipliers, metrics.multipliers);
        assert_eq!(p.constraints, metrics.constraints);
        assert_eq!(proof.len(), 1121);
    }
}

#[test]
fn public_binding_replay_controls() {
    let bp = BulletproofGens::new(2048, 1);
    // Every mutation below preserves circuit coefficients exactly. Arithmetic
    // equivalence alone must NOT permit crossing experiment/network/context inputs.
    let (s, w) = fixture(100, 999, 999, 0, 0, M, 0);
    let (bound, _) = prove(&s, &w, &bp).unwrap();
    let omitted = Bindings {
        statement: false,
        ..Bindings::COMPLETE
    };
    let (unbound, _) = prove_control(&s, &w, &bp, omitted).unwrap();
    type Mutation = (&'static str, fn(&mut PublicStatement));
    let changes: [Mutation; 9] = [
        ("version", |s| s.version += 1),
        ("network", |s| s.network[0] ^= 1),
        ("context", |s| s.context[0] ^= 1),
        ("input-factor", |s| s.input_factor -= 1),
        ("output-factor", |s| s.output_factor -= 1),
        ("elapsed", |s| s.elapsed += 1),
        ("rate", |s| s.rate += 1),
        ("year", |s| s.year -= 1),
        ("horizon", |s| s.horizon += 1),
    ];
    for (name, mutate) in changes {
        let mut changed = s.clone();
        mutate(&mut changed);
        assert_eq!(
            CircuitLayout::new(&s).equations,
            CircuitLayout::new(&changed).equations
        );
        assert!(
            verify(&changed, &bound, &bp).is_err(),
            "bound replay accepted: {name}"
        );
        assert!(
            verify_control(&changed, &unbound, &bp, omitted).is_ok(),
            "control did not replay: {name}"
        );
    }
    let mut changed = s.clone();
    changed.token = 1;
    assert!(verify(&changed, &bound, &bp).is_err());
    // Commitments are also automatically transcript-bound by the R1CS commit API.
    // The explicit append is redundant; the external circuit equalities are NOT.
    for amount in [true, false] {
        let mut changed = s.clone();
        if amount {
            changed.amount_commitment = project_commit(w.amount + 1, w.amount_blind, 0);
        } else {
            changed.charge_commitment = project_commit(w.charge + 1, w.charge_blind, 0);
        }
        assert!(verify(&changed, &bound, &bp).is_err());
        assert!(verify_control(&changed, &unbound, &bp, omitted).is_err());
    }
    let mut changed = s.clone();
    changed.amount_commitment = [0xff; 32];
    assert!(verify(&changed, &bound, &bp).is_err());
    for len in [0, 1, bound.len() / 2, bound.len() - 1] {
        assert!(verify(&s, &bound[..len], &bp).is_err());
    }
    let mut corrupted = bound.clone();
    corrupted[50] ^= 1;
    assert!(verify(&s, &corrupted, &bp).is_err());
    let mut appended = bound;
    appended.push(0);
    assert!(verify(&s, &appended, &bp).is_err());
}

#[test]
fn external_commitment_equality_controls() {
    let bp = BulletproofGens::new(2048, 1);
    for amount in [true, false] {
        let (mut s, mut w) = fixture(10_000, 6000, 3500, 1_234_567, 200, 6_307_200, 31_536_000);
        // Circuit witness still describes the original value/charge, but the real
        // external commitment now has a different (known) opening.
        if amount {
            w.amount += 1;
            s.amount_commitment = project_commit(w.amount, w.amount_blind, 0);
        } else {
            w.charge += 1;
            s.charge_commitment = project_commit(w.charge, w.charge_blind, 0);
        }
        let omitted = Bindings {
            amount: !amount,
            charge: amount,
            ..Bindings::COMPLETE
        };
        let (unbound, _) = prove_control(&s, &w, &bp, omitted).unwrap();
        assert!(verify_control(&s, &unbound, &bp, omitted).is_ok());
        let (bound, _) = prove(&s, &w, &bp).unwrap();
        assert!(verify(&s, &bound, &bp).is_err());
    }
}

fn change(w: &mut Witness, layout: &CircuitLayout, name: &str, up: bool) {
    let i = layout.wires.iter().position(|(n, _)| n == name).unwrap();
    if up {
        w.values[i] += n(1);
    } else {
        w.values[i] -= n(1);
    }
}
#[test]
fn invalid_witness_relations_reach_crypto_verification() {
    let bp = BulletproofGens::new(2048, 1);
    for kind in ["cap", "difference", "max", "annual-floor", "time-remainder"] {
        let (mut s, mut w) = if kind == "max" {
            fixture(100, 6000, 1000, 1, 200, 6_307_200, 31_536_000)
        } else {
            fixture(M, 6000, 3500, M, u32::MAX, 1, 31_536_000)
        };
        let layout = CircuitLayout::new(&s);
        match kind {
            "cap" => {
                for name in ["accrued.c", "settled", "max.capital_gap"] {
                    change(&mut w, &layout, name, false);
                }
                change(&mut w, &layout, "accrued.excess", true);
                w.charge -= 1;
            }
            "difference" => {
                change(&mut w, &layout, "difference", true);
                change(&mut w, &layout, "reverse_difference", true);
                change(&mut w, &layout, "max.capital_gap", false);
            }
            "max" => {
                for name in ["settled", "max.accrued_gap", "max.capital_gap"] {
                    change(&mut w, &layout, name, true);
                }
                w.charge += 1;
            }
            "annual-floor" => change(&mut w, &layout, "annual.in.a", false),
            "time-remainder" => change(&mut w, &layout, "accrued.r", true),
            _ => unreachable!(),
        }
        s.charge_commitment = project_commit(w.charge, w.charge_blind, 0);
        // No integer checker is called: a proof is actually produced, then rejected.
        let (proof, _) = prove(&s, &w, &bp).unwrap();
        assert!(verify(&s, &proof, &bp).is_err(), "accepted invalid {kind}");
    }
}

#[test]
fn project_generator_and_encoding_parity() {
    for token in [0, 1, M] {
        let (mut s, mut w) = fixture(100, 6000, 3500, 10, 200, 6_307_200, 31_536_000);
        s.token = token;
        for amount in [0, 1, M] {
            w.amount = amount;
            assert_eq!(
                s.gens()
                    .commit(Scalar::from(amount), w.amount_blind)
                    .compress()
                    .to_bytes(),
                project_commit(amount, w.amount_blind, token)
            );
        }
        assert_eq!(
            s.gens().B.compress().to_bytes(),
            generators(token).B.compress().to_bytes()
        );
        assert_eq!(
            s.gens().B_blinding.compress().to_bytes(),
            generators(token).B_blinding.compress().to_bytes()
        );
        assert_ne!(s.gens().B.compress(), PedersenGens::default().B.compress());
    }
}
