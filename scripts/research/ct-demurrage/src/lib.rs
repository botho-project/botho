//! Inactive arithmetic research: integer witness checker and R1CS allocation
//! census. The companion proof_experiment module exercises actual randomized
//! proofs. Neither is production proof code, consensus validation, or an
//! accepted ADR.
#![cfg(test)]

// Frozen source retained solely for explicitly historical discriminators.
mod legacy_kernel;
mod proof_experiment;
use bulletproofs_og::{
    r1cs::{ConstraintSystem, LinearCombination, Metrics, Prover, Verifier},
    PedersenGens,
};
use curve25519_dalek_v4::scalar::Scalar;
use merlin::Transcript;
use num_bigint::BigUint;

const K: u64 = 50_000_000;
const M: u64 = u64::MAX;
// Full-domain widths, checked against arbitrary ranged assignments below.
const EXCESS_BITS: usize = 147;
const REMAINDER_BITS: usize = 90;
const INPUT_RANGE_BITS: usize = 64 + 3 * (64 + EXCESS_BITS + 2 * REMAINDER_BITS) + 5 * 64;
const INPUT_PRODUCTS: usize = 5;
const INPUT_EQUATIONS: usize = 9;
const INPUT_MULTIPLIERS: usize = INPUT_RANGE_BITS + INPUT_PRODUCTS;
const INPUT_CONSTRAINTS: usize = 2 * INPUT_RANGE_BITS + 2 + INPUT_EQUATIONS + 3 * INPUT_PRODUCTS;
fn n(x: u64) -> BigUint {
    x.into()
}
fn max(bits: usize) -> BigUint {
    (n(1) << bits) - n(1)
}
fn order() -> BigUint {
    (n(1) << 252) + BigUint::parse_bytes(b"27742317777372353535851937790883648493", 10).unwrap()
}
fn scalar(x: &BigUint) -> Scalar {
    assert!(x < &order());
    let bytes = x.to_bytes_le();
    let mut fixed = [0; 32];
    fixed[..bytes.len()].copy_from_slice(&bytes);
    Option::<Scalar>::from(Scalar::from_canonical_bytes(fixed)).unwrap()
}

#[derive(Clone, Debug)]
struct Wire {
    name: String,
    value: BigUint,
    bits: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Expr {
    terms: Vec<(usize, BigUint)>,
    constant: BigUint,
}
impl Expr {
    fn wire(i: usize) -> Self {
        Self {
            terms: vec![(i, n(1))],
            ..Self::default()
        }
    }
    fn constant(c: u64) -> Self {
        Self {
            constant: n(c),
            ..Self::default()
        }
    }
    fn add(mut self, rhs: Self) -> Self {
        self.terms.extend(rhs.terms);
        self.constant += rhs.constant;
        self
    }
    fn scale(mut self, c: BigUint) -> Self {
        for (_, a) in &mut self.terms {
            *a *= &c;
        }
        self.constant *= c;
        self
    }
    fn eval(&self, wires: &[Wire], bound: bool) -> BigUint {
        self.terms.iter().fold(self.constant.clone(), |a, (i, c)| {
            a + c * if bound {
                max(wires[*i].bits)
            } else {
                wires[*i].value.clone()
            }
        })
    }
    fn lc(&self, vars: &[LinearCombination]) -> LinearCombination {
        self.terms.iter().fold(
            LinearCombination::from(scalar(&self.constant)),
            |a, (i, c)| a + vars[*i].clone() * scalar(c),
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Equation {
    name: String,
    left: Expr,
    right: Expr,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Product {
    name: String,
    left: Expr,
    right: Expr,
    output: Expr,
}
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    wires: Vec<Wire>,
    equations: Vec<Equation>,
    products: Vec<Product>,
    amount: usize,
    charge: usize,
}

impl Candidate {
    fn wire(&mut self, name: &str, value: BigUint, bits: usize) -> usize {
        let i = self.wires.len();
        self.wires.push(Wire {
            name: name.into(),
            value,
            bits,
        });
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
    fn exact_charge(
        &mut self,
        label: &str,
        factor: u64,
        rate: u32,
        elapsed: u64,
        year: u64,
    ) -> usize {
        // Public year-zero branch: 0 = c + excess + r and r + rbar = 0
        // force every charge witness to zero without division by zero.
        let (coefficient, denominator) = if year == 0 {
            (n(0), n(1))
        } else {
            (
                n(rate as u64) * n(factor.clamp(1000, 6000) - 1000) * n(elapsed),
                n(K) * n(year),
            )
        };
        let numerator = &self.wires[self.amount].value * &coefficient;
        let quotient = &numerator / &denominator;
        let capped = quotient.clone().min(n(M));
        let c = self.wire(&format!("{label}.c"), capped.clone(), 64);
        let excess = self.wire(&format!("{label}.excess"), quotient - capped, EXCESS_BITS);
        let r = self.wire(
            &format!("{label}.r"),
            &numerator % &denominator,
            REMAINDER_BITS,
        );
        let bar = self.wire(
            &format!("{label}.rbar"),
            &denominator - n(1) - &self.wires[r].value,
            REMAINDER_BITS,
        );
        self.eq(
            label,
            Expr::wire(self.amount).scale(coefficient),
            Expr::wire(c)
                .add(Expr::wire(excess))
                .scale(denominator.clone())
                .add(Expr::wire(r)),
        );
        self.eq(
            &format!("{label}.remainder"),
            Expr::wire(r).add(Expr::wire(bar)),
            Expr::constant(1).scale(denominator - n(1)),
        );
        // C*E=M*E is (M-C)*E=0, written with nonnegative sides.
        self.product(
            &format!("{label}.cap"),
            Expr::wire(c),
            Expr::wire(excess),
            Expr::wire(excess).scale(n(M)),
        );
        c
    }
    /// Public factors/time/rate/year; private original amount. Horizon is
    /// explicit so the generic capitalized kernel and its fixed spend
    /// horizon can both be tested.
    pub fn construct(
        value: u64,
        input: u64,
        output: u64,
        elapsed: u64,
        rate: u32,
        year: u64,
        horizon: u64,
    ) -> Self {
        let mut cs = Self::default();
        cs.amount = cs.wire("value", n(value), 64);
        let accrued = cs.exact_charge("accrued", input, rate, elapsed, year);
        let ci = cs.exact_charge("capital.in", input, rate, horizon, year);
        let co = cs.exact_charge("capital.out", output, rate, horizon, year);
        // Clamping and capped charge are monotone in the raw factor, so the
        // positive difference is also zero whenever production's raw-factor
        // early return (output >= input) applies.
        let vi = &cs.wires[ci].value;
        let vo = &cs.wires[co].value;
        let (d, j) = if vi >= vo {
            (vi - vo, n(0))
        } else {
            (n(0), vo - vi)
        };
        let d = cs.wire("difference", d, 64);
        let j = cs.wire("reverse_difference", j, 64);
        cs.eq(
            "difference",
            Expr::wire(ci).add(Expr::wire(j)),
            Expr::wire(co).add(Expr::wire(d)),
        );
        cs.product(
            "difference",
            Expr::wire(d),
            Expr::wire(j),
            Expr::constant(0),
        );
        let z = cs.wires[accrued]
            .value
            .clone()
            .max(cs.wires[d].value.clone());
        let x = &z - &cs.wires[accrued].value;
        let y = &z - &cs.wires[d].value;
        cs.charge = cs.wire("settled", z, 64);
        let x = cs.wire("max.accrued_gap", x, 64);
        let y = cs.wire("max.capital_gap", y, 64);
        cs.eq(
            "max.accrued",
            Expr::wire(cs.charge),
            Expr::wire(accrued).add(Expr::wire(x)),
        );
        cs.eq(
            "max.capital",
            Expr::wire(cs.charge),
            Expr::wire(d).add(Expr::wire(y)),
        );
        cs.product("max", Expr::wire(x), Expr::wire(y), Expr::constant(0));
        cs
    }
    /// Exact inequalities AND field equations. The supplied amount is the value
    /// bound to the external input commitment; never a second free amount
    /// witness.
    pub fn check(&self, committed_amount: u64) -> Result<(), String> {
        let l = order();
        if self.wires[self.amount].value != n(committed_amount) {
            return Err("amount commitment binding".into());
        }
        for w in &self.wires {
            if w.value > max(w.bits) {
                return Err(format!("range: {}", w.name));
            }
        }
        for e in &self.equations {
            let lb = e.left.eval(&self.wires, true);
            let rb = e.right.eval(&self.wires, true);
            if lb >= l || rb >= l {
                return Err(format!("no-wrap bound: {}", e.name));
            }
            let left = e.left.eval(&self.wires, false);
            let right = e.right.eval(&self.wires, false);
            if left != right || scalar(&left) != scalar(&right) {
                return Err(format!("equation: {}", e.name));
            }
        }
        for p in &self.products {
            let lb = p.left.eval(&self.wires, true) * p.right.eval(&self.wires, true);
            let rb = p.output.eval(&self.wires, true);
            if lb >= l || rb >= l {
                return Err(format!("product no-wrap: {}", p.name));
            }
            let a = p.left.eval(&self.wires, false);
            let b = p.right.eval(&self.wires, false);
            let c = p.output.eval(&self.wires, false);
            if &a * &b != c || scalar(&a) * scalar(&b) != scalar(&c) {
                return Err(format!("product: {}", p.name));
            }
        }
        Ok(())
    }
    pub fn result(&self, name: &str) -> u64 {
        self.wires
            .iter()
            .find(|w| w.name == name)
            .unwrap()
            .value
            .clone()
            .try_into()
            .unwrap()
    }
    /// Actual pinned R1CS allocations, but no proof or verification is
    /// performed. Decompose directly into boolean multiplier variables to
    /// avoid extra unconstrained allocated field wires. Value and charge
    /// bind external commits.
    fn allocate<CS: ConstraintSystem>(
        &self,
        cs: &mut CS,
        external_value: LinearCombination,
        external_charge: LinearCombination,
        witness: bool,
    ) {
        let mut vars = Vec::new();
        for w in &self.wires {
            let mut lc = LinearCombination::default();
            for bit in 0..w.bits {
                let b = Scalar::from(u64::from(w.value.bit(bit as u64)));
                let assignment = witness.then_some((b, Scalar::ONE - b));
                let (a, b, o) = cs.allocate_multiplier(assignment).unwrap();
                cs.constrain(o.into());
                cs.constrain(a + b - Scalar::ONE);
                lc = lc + a * scalar(&(n(1) << bit));
            }
            vars.push(lc);
        }
        cs.constrain(vars[self.amount].clone() - external_value);
        cs.constrain(vars[self.charge].clone() - external_charge);
        for e in &self.equations {
            cs.constrain(e.left.lc(&vars) - e.right.lc(&vars));
        }
        for p in &self.products {
            let (_, _, o) = cs.multiply(p.left.lc(&vars), p.right.lc(&vars));
            cs.constrain(o - p.output.lc(&vars));
        }
    }
    pub fn allocation_metrics(&self) -> (Metrics, Metrics) {
        let pc = PedersenGens::default();
        let mut pt = Transcript::new(b"inactive-ct-constraint-census");
        let mut prover = Prover::new(&pc, &mut pt);
        let (cv, v) = prover.commit(scalar(&self.wires[self.amount].value), Scalar::ONE);
        let (cz, z) = prover.commit(scalar(&self.wires[self.charge].value), Scalar::from(2u64));
        self.allocate(&mut prover, v.into(), z.into(), true);
        let mut vt = Transcript::new(b"inactive-ct-constraint-census");
        let mut verifier = Verifier::new(&mut vt);
        let v = verifier.commit(cv);
        let z = verifier.commit(cz);
        self.allocate(&mut verifier, v.into(), z.into(), false);
        (prover.metrics(), verifier.metrics())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_cluster_tax::{
        capitalized_reset_charge, demurrage_charge, spend_demurrage_charge, BLOCKS_PER_YEAR_5S,
        SETTLEMENT_HORIZON_BLOCKS,
    };
    fn differential(v: u64, fi: u64, fo: u64, e: u64, r: u32, y: u64, h: u64) {
        let c = Candidate::construct(v, fi, fo, e, r, y, h);
        c.check(v).unwrap();
        let accrued = demurrage_charge(v, fi, e, r, y);
        let capital = capitalized_reset_charge(v, fi, fo, h, r, y);
        assert_eq!(c.result("accrued.c"), accrued);
        assert_eq!(c.result("capital.in.c"), demurrage_charge(v, fi, h, r, y));
        assert_eq!(c.result("capital.out.c"), demurrage_charge(v, fo, h, r, y));
        assert_eq!(c.result("difference"), capital);
        assert_eq!(c.result("settled"), accrued.max(capital));
        if h == SETTLEMENT_HORIZON_BLOCKS {
            assert_eq!(
                c.result("settled"),
                spend_demurrage_charge(v, fi, fo, e, r, y)
            );
        }
    }
    #[test]
    fn exhaustive_small_domain() {
        for v in 0..8 {
            for fi in 999..1003 {
                for fo in 999..1003 {
                    for e in 0..3 {
                        for r in 0..3 {
                            for y in 0..3 {
                                differential(v, fi, fo, e, r, y, SETTLEMENT_HORIZON_BLOCKS);
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn deterministic_wide_domain() {
        // Independent fixed xorshift stream, no probabilistic pass criterion.
        let mut state = 0x8a55_7102_cc09_ddf1u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for i in 0..2048 {
            let v = next();
            let fi = next() % 7000;
            let fo = next() % 7000;
            let e = next();
            let r = next() as u32;
            let y = match i % 4 {
                0 => 1,
                1 => BLOCKS_PER_YEAR_5S,
                2 => next(),
                _ => 0,
            };
            differential(v, fi, fo, e, r, y, SETTLEMENT_HORIZON_BLOCKS);
        }
    }
    #[test]
    fn floor_clamp_and_saturation_boundaries() {
        for v in [0, 1, 49, 50, 51, 99, 100, 101, M / 2, M - 1, M] {
            for (fi, fo) in [
                (0, M),
                (999, 1000),
                (1001, 1000),
                (6000, 3500),
                (M, 6001),
                (6000, 5999),
            ] {
                for r in [0, 1, 200, u32::MAX] {
                    for (e, y) in [
                        (0, 0),
                        (1, 1),
                        (M, 1),
                        (M, M),
                        (SETTLEMENT_HORIZON_BLOCKS, BLOCKS_PER_YEAR_5S),
                    ] {
                        differential(v, fi, fo, e, r, y, SETTLEMENT_HORIZON_BLOCKS);
                    }
                }
            }
        }
        // Independently capped charges cancel, including true u128 overflow.
        let c = Candidate::construct(M, 6000, 5999, 0, u32::MAX, 1, SETTLEMENT_HORIZON_BLOCKS);
        c.check(M).unwrap();
        assert_eq!(c.result("capital.in.c"), M);
        assert_eq!(c.result("capital.out.c"), M);
        assert_eq!(c.result("difference"), 0);
        assert_eq!(c.result("settled"), 0);
        differential(M, 6000, 5999, M, u32::MAX, 1, SETTLEMENT_HORIZON_BLOCKS);
        // One cap, adjacent unsaturated charge: the difference is ONE, not MAX.
        let c = Candidate::construct(100, 6000, 3500, 0, 200, 1, M - 1);
        c.check(100).unwrap();
        assert_eq!(c.result("capital.in.c"), M);
        assert_eq!(c.result("capital.out.c"), M - 1);
        assert_eq!(c.result("difference"), 1);
        differential(100, 6000, 3500, 0, 200, 1, M - 1);
        assert_eq!(
            demurrage_charge(49, 6000, SETTLEMENT_HORIZON_BLOCKS, 200, BLOCKS_PER_YEAR_5S),
            4
        );
        assert_eq!(
            capitalized_reset_charge(
                51,
                6000,
                3500,
                SETTLEMENT_HORIZON_BLOCKS,
                200,
                BLOCKS_PER_YEAR_5S
            ),
            3
        );
    }
    #[test]
    fn exact_quotient_remainder_and_cap_boundaries() {
        assert_eq!(SETTLEMENT_HORIZON_BLOCKS, 31_536_000);
        // N/D is just below, at and just above integer boundaries.
        for (value, quotient, remainder) in [
            (49, 0, 49_000_000),
            (50, 1, 0),
            (51, 1, 1_000_000),
            (99, 1, 49_000_000),
            (100, 2, 0),
            (101, 2, 1_000_000),
        ] {
            let c = Candidate::construct(value, 6000, 6000, 1, 200, 1, 0);
            differential(value, 6000, 6000, 1, 200, 1, 0);
            assert_eq!(c.result("accrued.c"), quotient);
            assert_eq!(c.result("accrued.r"), remainder);
        }
        for (value, elapsed, capped, excess) in [(M - 1, 1, M - 1, 0), (M, 1, M, 0), (M, 2, M, M)] {
            let c = Candidate::construct(value, 6000, 6000, elapsed, 10_000, 1, 0);
            differential(value, 6000, 6000, elapsed, 10_000, 1, 0);
            assert_eq!(c.result("accrued.c"), capped);
            assert_eq!(c.result("accrued.excess"), excess);
        }
        // Full public horizons and raw-factor early returns, including factors
        // that clamp to equal endpoints despite different raw ordering.
        for horizon in [0, 1, M] {
            for elapsed in [0, 1, M] {
                for year in [0, 1, M] {
                    for rate in [0, 200, u32::MAX] {
                        for (fi, fo) in [(0, M), (M, 0), (6000, M), (M, 6000), (999, 0), (0, 999)] {
                            differential(M, fi, fo, elapsed, rate, year, horizon);
                        }
                    }
                }
            }
        }
        for (elapsed, accrued_wins) in [(1, false), (10 * BLOCKS_PER_YEAR_5S, true)] {
            let c = Candidate::construct(
                10_000,
                6000,
                3500,
                elapsed,
                200,
                BLOCKS_PER_YEAR_5S,
                SETTLEMENT_HORIZON_BLOCKS,
            );
            differential(
                10_000,
                6000,
                3500,
                elapsed,
                200,
                BLOCKS_PER_YEAR_5S,
                SETTLEMENT_HORIZON_BLOCKS,
            );
            assert_eq!(c.result("accrued.c") > c.result("difference"), accrued_wins);
        }
    }
    fn alter(c: &mut Candidate, name: &str, up: bool) {
        let w = c.wires.iter_mut().find(|w| w.name == name).unwrap();
        if up {
            w.value += n(1);
        } else {
            w.value -= n(1);
        }
    }
    #[test]
    fn adversarial_witness_mutations() {
        let c = Candidate::construct(M, 6000, 3500, M, u32::MAX, 1, SETTLEMENT_HORIZON_BLOCKS);
        for i in 0..c.wires.len() {
            let mut bad = c.clone();
            bad.wires[i].value += n(1);
            assert!(bad.check(M).is_err(), "accepted {}", bad.wires[i].name);
        }
        let mut bad = c.clone();
        bad.wires[0].value += order();
        assert!(bad.check(M).is_err());
        assert!(c.check(M - 1).unwrap_err().contains("binding"));
        // Preserve all linear equalities but violate the cap complementarity.
        let mut bad = c.clone();
        for name in ["accrued.c", "settled", "max.capital_gap"] {
            alter(&mut bad, name, false);
        }
        alter(&mut bad, "accrued.excess", true);
        assert_eq!(bad.check(M).unwrap_err(), "product: accrued.cap");
        // Preserve subtraction and max equations but make both differences positive.
        let mut bad = c.clone();
        alter(&mut bad, "difference", true);
        alter(&mut bad, "reverse_difference", true);
        alter(&mut bad, "max.capital_gap", false);
        assert_eq!(bad.check(M).unwrap_err(), "product: difference");
        let mut bad = Candidate::construct(
            100,
            6000,
            1000,
            1,
            200,
            BLOCKS_PER_YEAR_5S,
            SETTLEMENT_HORIZON_BLOCKS,
        );
        for name in ["settled", "max.accrued_gap", "max.capital_gap"] {
            alter(&mut bad, name, true);
        }
        assert_eq!(bad.check(100).unwrap_err(), "product: max");
        // A scalar-order alias cannot sneak through a permissive field-only equation.
        let mut bad = c.clone();
        let w = bad
            .wires
            .iter_mut()
            .find(|w| w.name == "accrued.excess")
            .unwrap();
        w.value += order();
        assert!(bad.check(M).unwrap_err().starts_with("range:"));
        // A widened domain is itself rejected by the symbolic no-wrap audit.
        let mut bad = c;
        bad.wires[bad.amount].bits = 253;
        assert!(bad.check(M).unwrap_err().contains("no-wrap"));
    }
    #[test]
    fn global_bounds_and_actual_r1cs_census() {
        let numerator = n(M) * n(u32::MAX as u64) * n(5000) * n(M);
        let denominator = n(K) * n(M);
        let quotient = &numerator / K; // smallest positive year = 1
        let rhs = &denominator * (max(64) + max(EXCESS_BITS)) + max(REMAINDER_BITS);
        let remainder_sum = max(REMAINDER_BITS) * 2u64;
        let cap_product = max(64) * max(EXCESS_BITS);
        assert_eq!(numerator.bits(), 173);
        assert_eq!(denominator.bits(), REMAINDER_BITS as u64);
        assert_eq!(quotient.bits(), EXCESS_BITS as u64);
        assert_eq!(rhs.bits(), 237);
        assert_eq!(remainder_sum.bits(), 91);
        assert_eq!(cap_product.bits(), 211);
        for bound in [&numerator, &rhs, &remainder_sum, &cap_product] {
            assert!(bound < &order());
        }
        // Both sides are evaluated at independent maxima for EVERY ranged wire,
        // including assignments that violate the equations and cap relation.
        for year in [0, 1, M] {
            let c = Candidate::construct(M, M, M, M, u32::MAX, year, M);
            c.check(M).unwrap();
            for e in &c.equations {
                assert!(e.left.eval(&c.wires, true) < order(), "{} lhs", e.name);
                assert!(e.right.eval(&c.wires, true) < order(), "{} rhs", e.name);
            }
            for p in &c.products {
                assert!(p.left.eval(&c.wires, true) * p.right.eval(&c.wires, true) < order());
                assert!(p.output.eval(&c.wires, true) < order());
            }
        }
        let c = Candidate::construct(M, 6000, 6000, M, u32::MAX, 1, M);
        assert_eq!(
            c.wires.iter().map(|w| w.bits).sum::<usize>(),
            INPUT_RANGE_BITS
        );
        assert_eq!(c.products.len(), INPUT_PRODUCTS);
        assert_eq!(c.equations.len(), INPUT_EQUATIONS);
        let (p, v) = c.allocation_metrics();
        println!(
            "actual R1CS: {} multipliers, {} linear constraints, {} external commitments",
            p.multipliers, p.constraints, 2
        );
        assert_eq!(p.multipliers, INPUT_MULTIPLIERS);
        assert_eq!(p.constraints, INPUT_CONSTRAINTS);
        assert_eq!(p.multipliers, v.multipliers);
        assert_eq!(p.constraints, v.constraints);
        assert_eq!(p.phase_two_constraints, 0);
    }
}
