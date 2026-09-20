//! Inactive arithmetic composition only: no transaction codec or ownership
//! proof.
use super::*;

const MAX_COUNT: usize = 16;
const DOMAIN_COMBINED: &[u8] = b"botho/inactive/combined-charge-v1";

#[derive(Clone)]
struct Statement {
    version: u64,
    network: [u8; 32],
    genesis: [u8; 32],
    parent: [u8; 32],
    policy: [u8; 32],
    body: [u8; 32], // Opaque fixture digest, NOT authenticated transaction state.
    height: u64,
    expiry: u64,
    significant_bits: u8,
    base_unit: u64,
    fee: u64,
    inputs: Vec<PublicStatement>,
    outputs: Vec<[u8; 32]>,
}
#[derive(Clone)]
struct OutputWitness {
    value: u64,
    bits_value: BigUint,
    blind: Scalar,
}
#[derive(Clone)]
struct CombinedWitness {
    inputs: Vec<Witness>,
    outputs: Vec<OutputWitness>,
    lo_gap: BigUint,
    hi_gap: BigUint,
}
#[derive(Clone, Copy)]
struct Controls {
    input_bindings: Bindings,
    statement: bool,
    output_binding: bool,
    lower: bool,
    upper: bool,
    conservation: bool,
    group_balance: bool,
}
impl Controls {
    const COMPLETE: Self = Self {
        input_bindings: Bindings::COMPLETE,
        statement: true,
        output_binding: true,
        lower: true,
        upper: true,
        conservation: true,
        group_balance: true,
    };
}

fn bucket(d: u128, bits: u8) -> u128 {
    assert!((2..=4).contains(&bits));
    assert!(d <= MAX_COUNT as u128 * M as u128);
    if d == 0 {
        return 0;
    }
    let exponent = 127 - d.leading_zeros();
    let step = 1u128 << (exponent + 1).saturating_sub(bits as u32);
    d.div_ceil(step) * step
}
fn preimage(q: u128, bits: u8) -> Result<(u128, u128), String> {
    let upper = MAX_COUNT as u128 * M as u128;
    let first_above = |limit: u128| {
        let (mut lo, mut hi) = (0, upper + 1);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if bucket(mid, bits) <= limit {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    };
    let lo = if q == 0 { 0 } else { first_above(q - 1) };
    let end = first_above(q);
    if lo >= end || lo > upper || bucket(lo, bits) != q {
        return Err("noncanonical bucket endpoint".into());
    }
    Ok((lo, end - 1))
}

/// Audit all possible bit-ranged assignments, never only honest witnesses.
fn audit_input(layout: &CircuitLayout) -> Result<(), String> {
    let wires: Vec<_> = layout
        .wires
        .iter()
        .map(|(name, bits)| Wire {
            name: name.clone(),
            bits: *bits,
            value: n(0),
        })
        .collect();
    let l = order();
    for e in &layout.equations {
        if e.left.eval(&wires, true) >= l || e.right.eval(&wires, true) >= l {
            return Err(format!("linear no-wrap: {}", e.name));
        }
    }
    for p in &layout.products {
        if p.left.eval(&wires, true) * p.right.eval(&wires, true) >= l
            || p.output.eval(&wires, true) >= l
        {
            return Err(format!("product no-wrap: {}", p.name));
        }
    }
    Ok(())
}

impl Statement {
    fn endpoints(&self) -> Result<(u128, u128), String> {
        if !(1..=MAX_COUNT).contains(&self.inputs.len())
            || !(1..=MAX_COUNT).contains(&self.outputs.len())
            || !(2..=4).contains(&self.significant_bits)
            || self.version != 1
            || self.height != self.expiry
            || self.inputs.iter().any(|i| i.token != 0)
        {
            return Err("unsupported research statement".into());
        }
        let base = self.base_unit as u128 * self.inputs.len().max(self.outputs.len()) as u128;
        let q = (self.fee as u128)
            .checked_sub(base)
            .ok_or("fee below base")?;
        preimage(q, self.significant_bits)
    }
    fn audit(&self) -> Result<(u128, u128), String> {
        let (lo, hi) = self.endpoints()?;
        for input in &self.inputs {
            audit_input(&CircuitLayout::new(input))?;
        }
        let sum_inputs = n(M) * self.inputs.len();
        let sum_charges = sum_inputs.clone();
        // Do NOT assume conservation to bound invalid in-range assignments:
        // sixteen outputs PLUS a u64 fee need 69 bits, not 68.
        let output_plus_fee = n(M) * self.outputs.len() + n(self.fee);
        for side in [
            sum_inputs,
            output_plus_fee,
            BigUint::from(lo) + max(68),
            sum_charges + max(68),
            BigUint::from(hi),
        ] {
            if side >= order() {
                return Err("aggregate no-wrap".into());
            }
        }
        Ok((lo, hi))
    }
    fn transcript(&self, control: Controls) -> Transcript {
        let mut t = Transcript::new(DOMAIN_COMBINED);
        if control.statement {
            t.append_u64(b"version", self.version);
            for (label, bytes) in [
                (b"network".as_slice(), self.network),
                (b"genesis", self.genesis),
                (b"parent", self.parent),
                (b"policy", self.policy),
                (b"fixture-body", self.body),
            ] {
                t.append_message(label, &bytes);
            }
            for (label, value) in [
                (b"height".as_slice(), self.height),
                (b"expiry", self.expiry),
                (b"significant-bits", self.significant_bits as u64),
                (b"base-unit", self.base_unit),
                (b"fee", self.fee),
                (b"inputs", self.inputs.len() as u64),
                (b"outputs", self.outputs.len() as u64),
            ] {
                t.append_u64(label, value);
            }
            for i in &self.inputs {
                for (label, value) in [
                    (b"input-version".as_slice(), i.version),
                    (b"token", i.token),
                    (b"input-factor", i.input_factor),
                    (b"output-factor", i.output_factor),
                    (b"elapsed", i.elapsed),
                    (b"rate", i.rate as u64),
                    (b"year", i.year),
                    (b"horizon", i.horizon),
                ] {
                    t.append_u64(label, value);
                }
                t.append_message(b"input-network", &i.network);
                t.append_message(b"input-context", &i.context);
            }
        }
        // Commitments also bind intrinsically via Prover/Verifier::commit.
        t
    }
    fn group_balance(&self) -> Result<(), String> {
        let decode = |bytes| {
            CompressedRistretto(bytes)
                .decompress()
                .ok_or("invalid point")
        };
        let mut balance = self.inputs[0].gens().B * -Scalar::from(self.fee);
        for i in &self.inputs {
            balance += decode(i.amount_commitment)?;
        }
        for c in &self.outputs {
            balance -= decode(*c)?;
        }
        if balance != curve25519_dalek_v4::ristretto::RistrettoPoint::default() {
            return Err("commitment conservation".into());
        }
        Ok(())
    }
}

fn range<CS: ConstraintSystem>(
    cs: &mut CS,
    bits: usize,
    value: Option<&BigUint>,
) -> LinearCombination {
    let mut result = LinearCombination::default();
    for bit in 0..bits {
        let assignment = value.map(|v| {
            let b = Scalar::from(u64::from(v.bit(bit as u64)));
            (b, Scalar::ONE - b)
        });
        let (a, b, o) = cs.allocate_multiplier(assignment).unwrap();
        cs.constrain(o.into());
        cs.constrain(a + b - Scalar::ONE);
        result = result + a * scalar(&(n(1) << bit));
    }
    result
}
fn sum(vars: &[LinearCombination]) -> LinearCombination {
    vars.iter()
        .cloned()
        .fold(LinearCombination::default(), |a, b| a + b)
}
fn allocate<CS: ConstraintSystem>(
    cs: &mut CS,
    s: &Statement,
    amounts: &[LinearCombination],
    charges: &[LinearCombination],
    outputs: &[LinearCombination],
    witness: Option<&CombinedWitness>,
    control: Controls,
) -> Result<(), String> {
    let (lo, hi) = s.audit()?;
    for (index, input) in s.inputs.iter().enumerate() {
        CircuitLayout::new(input).allocate(
            cs,
            amounts[index].clone(),
            charges[index].clone(),
            witness.map(|w| &w.inputs[index]),
            control.input_bindings,
        );
    }
    for (index, out) in outputs.iter().enumerate() {
        let bits = range(cs, 64, witness.map(|w| &w.outputs[index].bits_value));
        if control.output_binding {
            cs.constrain(bits - out.clone());
        }
    }
    let lo_gap = range(cs, 68, witness.map(|w| &w.lo_gap));
    let hi_gap = range(cs, 68, witness.map(|w| &w.hi_gap));
    if control.lower {
        cs.constrain(sum(charges) - scalar(&BigUint::from(lo)) - lo_gap);
    }
    if control.upper {
        cs.constrain(LinearCombination::from(scalar(&BigUint::from(hi))) - sum(charges) - hi_gap);
    }
    if control.conservation {
        cs.constrain(sum(amounts) - sum(outputs) - Scalar::from(s.fee));
    }
    Ok(())
}
fn prove_control(
    s: &Statement,
    w: &CombinedWitness,
    bp: &BulletproofGens,
    c: Controls,
) -> Result<(Vec<u8>, Metrics), String> {
    s.audit()?;
    if w.inputs.len() != s.inputs.len()
        || w.outputs.len() != s.outputs.len()
        || w.lo_gap > max(68)
        || w.hi_gap > max(68)
        || w.outputs.iter().any(|o| o.bits_value > max(64))
    {
        return Err("witness shape/range".into());
    }
    for (input, w) in s.inputs.iter().zip(&w.inputs) {
        let layout = CircuitLayout::new(input);
        if w.values.len() != layout.wires.len()
            || w.values
                .iter()
                .zip(&layout.wires)
                .any(|(v, (_, bits))| v > &max(*bits))
        {
            return Err("input witness range".into());
        }
    }
    let gens = s.inputs[0].gens();
    let mut t = s.transcript(c);
    let mut p = Prover::new(&gens, &mut t);
    let mut amounts = Vec::new();
    let mut charges = Vec::new();
    let mut outputs = Vec::new();
    for (s, w) in s.inputs.iter().zip(&w.inputs) {
        let (cv, v) = p.commit(Scalar::from(w.amount), w.amount_blind);
        let (cd, d) = p.commit(Scalar::from(w.charge), w.charge_blind);
        if cv.to_bytes() != s.amount_commitment || cd.to_bytes() != s.charge_commitment {
            return Err("input opening".into());
        }
        amounts.push(v.into());
        charges.push(d.into());
    }
    for (c, w) in s.outputs.iter().zip(&w.outputs) {
        let (cv, v) = p.commit(Scalar::from(w.value), w.blind);
        if cv.to_bytes() != *c {
            return Err("output opening".into());
        }
        outputs.push(v.into());
    }
    // Deliberately no integer-equation or group-balance oracle before proving.
    allocate(&mut p, s, &amounts, &charges, &outputs, Some(w), c)?;
    let metrics = p.metrics();
    Ok((p.prove(bp).map_err(|e| e.to_string())?.to_bytes(), metrics))
}
fn verify_control(
    s: &Statement,
    bytes: &[u8],
    bp: &BulletproofGens,
    c: Controls,
) -> Result<Metrics, String> {
    s.audit()?;
    if c.group_balance {
        s.group_balance()?;
    }
    let proof = R1CSProof::from_bytes(bytes).map_err(|e| e.to_string())?;
    let mut t = s.transcript(c);
    let mut v = Verifier::new(&mut t);
    let mut amounts = Vec::new();
    let mut charges = Vec::new();
    let mut outputs = Vec::new();
    for i in &s.inputs {
        amounts.push(v.commit(CompressedRistretto(i.amount_commitment)).into());
        charges.push(v.commit(CompressedRistretto(i.charge_commitment)).into());
    }
    for c in &s.outputs {
        outputs.push(v.commit(CompressedRistretto(*c)).into());
    }
    allocate(&mut v, s, &amounts, &charges, &outputs, None, c)?;
    let metrics = v.metrics();
    v.verify(&proof, &s.inputs[0].gens(), bp)
        .map_err(|e| e.to_string())?;
    Ok(metrics)
}

fn fixture_combined(
    values: &[u64],
    output_count: usize,
    bits: u8,
    base: u64,
    factors: (u64, u64, u64, u32, u64, u64),
) -> Result<(Statement, CombinedWitness), String> {
    let (fi, fo, elapsed, rate, year, horizon) = factors;
    let mut rng = ChaCha20Rng::from_seed([0x07; 32]);
    let mut s = Statement {
        version: 1,
        network: [1; 32],
        genesis: [2; 32],
        parent: [3; 32],
        policy: [4; 32],
        body: [5; 32],
        height: 10,
        expiry: 10,
        significant_bits: bits,
        base_unit: base,
        fee: 0,
        inputs: vec![],
        outputs: vec![],
    };
    let mut w = CombinedWitness {
        inputs: vec![],
        outputs: vec![],
        lo_gap: n(0),
        hi_gap: n(0),
    };
    for value in values {
        let (mut input, mut witness) = fixture(*value, fi, fo, elapsed, rate, year, horizon);
        witness.amount_blind = Scalar::random(&mut rng);
        witness.charge_blind = Scalar::random(&mut rng);
        input.amount_commitment = project_commit(*value, witness.amount_blind, 0);
        input.charge_commitment = project_commit(witness.charge, witness.charge_blind, 0);
        s.inputs.push(input);
        w.inputs.push(witness);
    }
    let charge: u128 = w.inputs.iter().map(|w| w.charge as u128).sum();
    let fee = base as u128 * values.len().max(output_count) as u128 + bucket(charge, bits);
    s.fee = fee.try_into().map_err(|_| "unrepresentable fee")?;
    let available = values
        .iter()
        .map(|v| *v as u128)
        .sum::<u128>()
        .checked_sub(fee)
        .ok_or("unaffordable")?;
    if output_count == 0 || output_count > MAX_COUNT {
        return Err("output count".into());
    }
    let mut blind_sum = w
        .inputs
        .iter()
        .fold(Scalar::ZERO, |a, i| a + i.amount_blind);
    for index in 0..output_count {
        let value = available / output_count as u128
            + u128::from((index as u128) < available % output_count as u128);
        let value: u64 = value.try_into().map_err(|_| "output range")?;
        let blind = if index + 1 == output_count {
            blind_sum
        } else {
            Scalar::random(&mut rng)
        };
        blind_sum -= blind;
        s.outputs.push(project_commit(value, blind, 0));
        w.outputs.push(OutputWitness {
            value,
            bits_value: n(value),
            blind,
        });
    }
    let (lo, hi) = s.endpoints()?;
    w.lo_gap = BigUint::from(charge - lo);
    w.hi_gap = BigUint::from(hi - charge);
    Ok((s, w))
}

#[test]
fn bucket_family_and_all_witness_bounds() {
    for bits in 2..=4 {
        // Independent small-domain partition, including octave transitions.
        let mut buckets = std::collections::BTreeMap::<u128, Vec<u128>>::new();
        for d in 0u128..4096 {
            let mut step = 1;
            while step * (1u128 << bits) <= d {
                step *= 2;
            }
            let q = if d == 0 {
                0
            } else {
                ((d - 1) / step + 1) * step
            };
            assert_eq!(bucket(d, bits), q);
            buckets.entry(q).or_default().push(d);
        }
        for (q, members) in buckets {
            let (lo, hi) = preimage(q, bits).unwrap();
            assert_eq!(lo, members[0]);
            if hi < 4096 {
                assert_eq!(hi, *members.last().unwrap());
            }
            assert_eq!(bucket(lo, bits), q);
            assert_eq!(bucket(hi, bits), q);
            if lo != 0 {
                assert_ne!(bucket(lo - 1, bits), q);
            }
            assert_ne!(bucket(hi + 1, bits), q);
        }
        let upper = 16 * M as u128;
        // Large-domain octave boundaries, including the u64 fee boundary.
        for exponent in 0..=68 {
            let transition = 1u128 << exponent;
            for d in [transition.saturating_sub(1), transition, transition + 1] {
                if d > upper {
                    continue;
                }
                let q = bucket(d, bits);
                let (lo, hi) = preimage(q, bits).unwrap();
                assert!(lo <= d && d <= hi);
                assert_eq!(bucket(lo, bits), q);
                assert_eq!(bucket(hi, bits), q);
                if lo > 0 {
                    assert!(bucket(lo - 1, bits) < q);
                }
                if hi < upper {
                    assert!(bucket(hi + 1, bits) > q);
                }
                // Above singleton buckets, the immediate endpoint neighbor is
                // not another legal endpoint. Reject it rather than treating
                // every public fee as an upper-bound-only bucket.
                if q > (1u128 << bits) && q <= upper {
                    assert!(preimage(q - 1, bits).is_err());
                }
            }
        }
        assert_eq!(bucket(upper, bits), 1u128 << 68);
    }
    assert_eq!(preimage(8, 2).unwrap(), (7, 8));
    assert_eq!(preimage(12, 2).unwrap(), (9, 12));
    assert!(preimage(11, 2).is_err());
    assert_eq!((n(M) * 16usize + n(M)).bits(), 69);
    let (mut s, mut w) = fixture_combined(
        &[10_000],
        1,
        2,
        0,
        (6000, 3500, 0, 200, 6_307_200, 31_536_000),
    )
    .unwrap();
    let mut layout = CircuitLayout::new(&s.inputs[0]);
    audit_input(&layout).unwrap();
    layout
        .wires
        .iter_mut()
        .find(|(name, _)| name == "accrued.excess")
        .unwrap()
        .1 = 252;
    assert!(audit_input(&layout).unwrap_err().contains("no-wrap"));
    let bp = BulletproofGens::new(2048, 1);
    w.inputs[0].values[0] += order();
    assert_eq!(
        prove_control(&s, &w, &bp, Controls::COMPLETE).unwrap_err(),
        "input witness range"
    );
    s.inputs = vec![s.inputs[0].clone(); 17];
    assert!(s.audit().is_err());
    assert!(
        fixture_combined(&[M], 1, 2, 0, (6000, 6000, M, u32::MAX, 1, M))
            .err()
            .unwrap()
            == "unrepresentable fee"
    );
    assert!(
        fixture_combined(&[1], 1, 2, 2, (1000, 1000, 0, 0, 1, 0))
            .err()
            .unwrap()
            == "unaffordable"
    );
}

#[test]
fn combined_round_trips_and_measurements() {
    let setup = Instant::now();
    let bp = BulletproofGens::new(32768, 1);
    println!(
        "combined generator setup={:?}; capacity=32768",
        setup.elapsed()
    );
    let ordinary = (6000, 3500, 1_234_567, 200, 6_307_200, 31_536_000);
    for bits in 2..=4 {
        for (name, values, outs, base, params) in [
            (
                "ordinary-1",
                vec![1_000_000_000_000],
                1,
                250_000_000_000,
                ordinary,
            ),
            (
                "ordinary-16",
                vec![1_000_000_000_000; 16],
                16,
                250_000_000_000,
                ordinary,
            ),
        ] {
            let (s, w) = fixture_combined(&values, outs, bits, base, params).unwrap();
            for sample in 0..3 {
                let now = Instant::now();
                let (proof, p) = prove_control(&s, &w, &bp, Controls::COMPLETE).unwrap();
                let prove_time = now.elapsed();
                let now = Instant::now();
                let v = verify_control(&s, &proof, &bp, Controls::COMPLETE).unwrap();
                let verify_time = now.elapsed();
                assert_eq!(
                    (p.multipliers, p.constraints),
                    (v.multipliers, v.constraints)
                );
                assert_eq!(p.multipliers, 1412 * values.len() + 64 * outs + 136);
                assert!(p.multipliers <= 65536 && p.constraints <= 262144);
                // Proposed section includes its length and count plus C_di vector.
                let section_bytes = 4 + 1 + 32 * values.len() + proof.len();
                assert!(section_bytes <= 32768);
                println!("combined {name} s={bits} sample={sample}: multipliers={} constraints={} proof={} section={} prove={prove_time:?} verify={verify_time:?}",
                    p.multipliers,p.constraints,proof.len(),section_bytes);
            }
        }
    }
    for (name, values, outputs, params) in [
        ("zero", vec![0], 1, (1000, 1000, 0, 0, 0, 0)),
        ("maximum-sum", vec![M; 16], 16, (1000, 1000, 0, 0, 1, 0)),
        (
            "caps-cancel",
            vec![M],
            1,
            (6000, 5999, 0, u32::MAX, 1, 31_536_000),
        ),
        (
            "cap-difference-one",
            vec![100],
            1,
            (6000, 3500, 0, 200, 1, M - 1),
        ),
        ("one-to-sixteen", vec![10000], 16, ordinary),
        ("sixteen-to-one", vec![10000; 16], 1, ordinary),
    ] {
        let (s, w) = fixture_combined(&values, outputs, 2, 0, params).unwrap();
        let (proof, _) = prove_control(&s, &w, &bp, Controls::COMPLETE).unwrap();
        verify_control(&s, &proof, &bp, Controls::COMPLETE).unwrap();
        println!("combined boundary {name}: pass");
    }
}

fn baseline() -> (Statement, CombinedWitness) {
    fixture_combined(
        &[10000],
        1,
        2,
        0,
        (6000, 3500, 0, 200, 6_307_200, 31_536_000),
    )
    .unwrap()
}
fn set_output(s: &mut Statement, w: &mut CombinedWitness, value: u64) {
    w.outputs[0].value = value;
    w.outputs[0].bits_value = n(value);
    s.outputs[0] = project_commit(value, w.outputs[0].blind, 0);
}
/// A deliberately detached control verifies; the same in-range witness with
/// complete equations generates proof bytes but is rejected cryptographically.
fn omission(
    s: &Statement,
    w: &CombinedWitness,
    bp: &BulletproofGens,
    omitted: Controls,
    full: Controls,
) {
    let (proof, _) = prove_control(s, w, bp, omitted).unwrap();
    verify_control(s, &proof, bp, omitted).unwrap();
    let (proof, _) = prove_control(s, w, bp, full).unwrap();
    assert!(verify_control(s, &proof, bp, full).is_err());
}
#[test]
fn combined_omission_controls_reach_verifier() {
    let bp = BulletproofGens::new(2048, 1);
    let full = Controls::COMPLETE;
    println!("control input amount");
    let (s, mut w) = baseline();
    let (_, other) = fixture(10001, 6000, 3500, 0, 200, 6_307_200, 31_536_000);
    assert_eq!(other.charge, w.inputs[0].charge);
    w.inputs[0].values = other.values;
    omission(
        &s,
        &w,
        &bp,
        Controls {
            input_bindings: Bindings {
                amount: false,
                ..Bindings::COMPLETE
            },
            ..full
        },
        full,
    );

    println!("control charge");
    let (mut s, mut w) = baseline();
    w.inputs[0].charge += 1;
    s.inputs[0].charge_commitment = project_commit(w.inputs[0].charge, w.inputs[0].charge_blind, 0);
    w.lo_gap += n(1);
    w.hi_gap -= n(1);
    omission(
        &s,
        &w,
        &bp,
        Controls {
            input_bindings: Bindings {
                charge: false,
                ..Bindings::COMPLETE
            },
            ..full
        },
        full,
    );

    println!("control output range");
    let (s, mut w) = baseline();
    w.outputs[0].bits_value += n(1);
    omission(
        &s,
        &w,
        &bp,
        Controls {
            output_binding: false,
            ..full
        },
        full,
    );

    for (q, lower) in [(768u64, true), (256, false)] {
        println!("control bucket {q}");
        let (mut s, mut w) = baseline();
        s.fee = q;
        set_output(&mut s, &mut w, 10000 - q);
        let (lo, hi) = s.endpoints().unwrap();
        let d = w.inputs[0].charge as u128;
        assert!(if lower { d < lo } else { d > hi });
        w.lo_gap = BigUint::from(d.saturating_sub(lo));
        w.hi_gap = BigUint::from(hi.saturating_sub(d));
        omission(
            &s,
            &w,
            &bp,
            Controls {
                lower: !lower,
                upper: lower,
                ..full
            },
            full,
        );
    }
    let (mut s, mut w) = baseline();
    println!("control conservation");
    let value = w.outputs[0].value + 1;
    set_output(&mut s, &mut w, value);
    assert_eq!(s.group_balance().unwrap_err(), "commitment conservation");
    // Disable the independent group check ONLY to isolate the integer equation.
    omission(
        &s,
        &w,
        &bp,
        Controls {
            conservation: false,
            group_balance: false,
            ..full
        },
        Controls {
            group_balance: false,
            ..full
        },
    );
}

#[test]
fn combined_public_statement_and_opening_rejections() {
    let bp = BulletproofGens::new(2048, 1);
    let full = Controls::COMPLETE;
    let (s, w) = baseline();
    let (proof, _) = prove_control(&s, &w, &bp, full).unwrap();
    let omitted = Controls {
        statement: false,
        ..full
    };
    let (unbound, _) = prove_control(&s, &w, &bp, omitted).unwrap();
    // Each context change leaves all arithmetic and commitments identical.
    for field in 0..8 {
        let mut changed = s.clone();
        match field {
            0 => changed.network[0] ^= 1,
            1 => changed.genesis[0] ^= 1,
            2 => changed.parent[0] ^= 1,
            3 => changed.policy[0] ^= 1,
            4 => changed.body[0] ^= 1,
            5 => {
                changed.height += 1;
                changed.expiry += 1;
            }
            6 => changed.inputs[0].context[0] ^= 1,
            _ => changed.inputs[0].network[0] ^= 1,
        }
        assert!(verify_control(&changed, &proof, &bp, full).is_err());
        verify_control(&changed, &unbound, &bp, omitted).unwrap();
    }
    for field in 0..9 {
        let mut changed = s.clone();
        match field {
            0 => changed.fee += 1,
            1 => changed.base_unit += 1,
            2 => changed.significant_bits = 3,
            3 => changed.inputs[0].rate += 1,
            4 => {
                changed.inputs[0].amount_commitment =
                    project_commit(10001, w.inputs[0].amount_blind, 0)
            }
            5 => {
                changed.inputs[0].charge_commitment =
                    project_commit(999, w.inputs[0].charge_blind, 0)
            }
            6 => {
                changed.outputs[0] =
                    project_commit(w.outputs[0].value, w.outputs[0].blind + Scalar::ONE, 0)
            }
            7 => changed.outputs.push(changed.outputs[0]),
            _ => changed.inputs[0].token = 1,
        }
        assert!(verify_control(&changed, &proof, &bp, full).is_err());
    }
    let mut bad = w.clone();
    bad.outputs[0].blind += Scalar::ONE;
    assert_eq!(
        prove_control(&s, &bad, &bp, full).unwrap_err(),
        "output opening"
    );
    let mut bad = w.clone();
    bad.inputs[0].amount_blind += Scalar::ONE;
    assert_eq!(
        prove_control(&s, &bad, &bp, full).unwrap_err(),
        "input opening"
    );
    for bytes in [
        vec![],
        proof[..proof.len() - 1].to_vec(),
        [proof.as_slice(), &[0]].concat(),
    ] {
        assert!(verify_control(&s, &bytes, &bp, full).is_err());
    }
    let (s, w) = fixture_combined(
        &[10000, 11000],
        2,
        2,
        0,
        (6000, 3500, 0, 200, 6_307_200, 31_536_000),
    )
    .unwrap();
    let bp = BulletproofGens::new(4096, 1);
    let (proof, _) = prove_control(&s, &w, &bp, full).unwrap();
    let mut changed = s.clone();
    changed.inputs.swap(0, 1);
    assert!(verify_control(&changed, &proof, &bp, full).is_err());
    let mut changed = s;
    changed.outputs.swap(0, 1);
    assert!(verify_control(&changed, &proof, &bp, full).is_err());
}
