#![no_main]

//! Fuzzing target for Lion lattice polynomial arithmetic.
//!
//! Security rationale: Polynomial operations are the foundation of lattice-based
//! cryptography. Errors in NTT, modular reduction, or coefficient handling could
//! lead to invalid signatures or verification failures.
//!
//! This target tests:
//! - NTT/inverse NTT consistency
//! - Polynomial arithmetic (add, sub, mul)
//! - Serialization roundtrips
//! - Edge cases (zero, max coefficients, etc.)

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use bth_crypto_lion::{
    params::{N, Q},
    polynomial::{Poly, PolyVecK, PolyVecL, PolyMatrix},
};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode for polynomial operations
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Test NTT multiplication correctness
    NttMultiplication(NttMulFuzz),
    /// Test arithmetic operations
    Arithmetic(ArithmeticFuzz),
    /// Test serialization
    Serialization(SerializationFuzz),
    /// Test matrix operations
    MatrixOps(MatrixFuzz),
    /// Test edge cases
    EdgeCases(EdgeCaseFuzz),
    /// Raw coefficient bytes
    RawCoefficients(RawCoeffFuzz),
}

/// NTT multiplication test
#[derive(Debug, Arbitrary)]
struct NttMulFuzz {
    /// Seeds for polynomials
    seed_a: [u8; 32],
    seed_b: [u8; 32],
    /// Use small coefficients (typical for signatures)
    use_small_coeffs: bool,
    /// Eta parameter for small sampling
    eta: u8,
}

/// Arithmetic operations test
#[derive(Debug, Arbitrary)]
struct ArithmeticFuzz {
    /// Seed for RNG
    seed: [u8; 32],
    /// Operations to perform
    ops: Vec<ArithOp>,
}

#[derive(Debug, Arbitrary, Clone)]
enum ArithOp {
    Add,
    Sub,
    Neg,
    ScalarMul(u32),
    Ntt,
    InvNtt,
}

/// Serialization test
#[derive(Debug, Arbitrary)]
struct SerializationFuzz {
    /// Coefficients (will be reduced mod Q)
    coefficients: Vec<u32>,
    /// Whether to corrupt bytes
    corrupt: bool,
    /// Position to corrupt
    corrupt_pos: u16,
    /// Corruption value
    corrupt_val: u8,
}

/// Matrix operations test
#[derive(Debug, Arbitrary)]
struct MatrixFuzz {
    /// Seed for matrix expansion
    matrix_seed: [u8; 32],
    /// Seed for vector
    vector_seed: [u8; 32],
    /// Compare NTT vs naive
    compare_methods: bool,
}

/// Edge case testing
#[derive(Debug, Arbitrary)]
struct EdgeCaseFuzz {
    /// Type of edge case
    case: EdgeCase,
}

#[derive(Debug, Arbitrary)]
enum EdgeCase {
    /// All zeros
    AllZero,
    /// All max (Q-1)
    AllMax,
    /// Alternating 0 and Q-1
    Alternating,
    /// Single non-zero coefficient
    SingleCoeff { index: u8, value: u32 },
    /// Challenge polynomial
    Challenge { seed: [u8; 32], tau: u8 },
}

/// Raw coefficient fuzzing
#[derive(Debug, Arbitrary)]
struct RawCoeffFuzz {
    /// Raw bytes
    bytes: Vec<u8>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create polynomial from seed with small coefficients
fn poly_from_seed_small(seed: [u8; 32], eta: u32) -> Poly {
    let mut rng = ChaCha20Rng::from_seed(seed);
    Poly::sample_small(&mut rng, eta)
}

/// Create polynomial from seed with uniform coefficients
fn poly_from_seed_uniform(seed: [u8; 32]) -> Poly {
    let mut rng = ChaCha20Rng::from_seed(seed);
    Poly::sample_uniform(&mut rng)
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::NttMultiplication(ntt) => {
            fuzz_ntt_multiplication(&ntt);
        }
        FuzzMode::Arithmetic(arith) => {
            fuzz_arithmetic(&arith);
        }
        FuzzMode::Serialization(ser) => {
            fuzz_serialization(&ser);
        }
        FuzzMode::MatrixOps(matrix) => {
            fuzz_matrix_ops(&matrix);
        }
        FuzzMode::EdgeCases(edge) => {
            fuzz_edge_cases(&edge);
        }
        FuzzMode::RawCoefficients(raw) => {
            fuzz_raw_coefficients(&raw);
        }
    }
});

/// Test NTT multiplication consistency
fn fuzz_ntt_multiplication(ntt: &NttMulFuzz) {
    let eta = (ntt.eta as u32 % 4).max(1);

    let a = if ntt.use_small_coeffs {
        poly_from_seed_small(ntt.seed_a, eta)
    } else {
        poly_from_seed_uniform(ntt.seed_a)
    };

    let b = if ntt.use_small_coeffs {
        poly_from_seed_small(ntt.seed_b, eta)
    } else {
        poly_from_seed_uniform(ntt.seed_b)
    };

    // NTT multiplication must match naive multiplication
    let ntt_result = a.ntt_mul(&b);
    let naive_result = a.naive_mul(&b);

    for i in 0..N {
        assert_eq!(
            ntt_result.coeffs[i], naive_result.coeffs[i],
            "NTT and naive multiplication differ at coefficient {}",
            i
        );
    }

    // Both results must be valid
    assert!(ntt_result.is_valid());
    assert!(naive_result.is_valid());

    // Multiplication should be commutative
    let ba_ntt = b.ntt_mul(&a);
    assert_eq!(ntt_result, ba_ntt, "Multiplication must be commutative");
}

/// Test arithmetic operations
fn fuzz_arithmetic(arith: &ArithmeticFuzz) {
    let mut rng = ChaCha20Rng::from_seed(arith.seed);
    let mut a = Poly::sample_uniform(&mut rng);
    let b = Poly::sample_uniform(&mut rng);

    // Keep copy of original
    let original = a.clone();

    // Apply operations (limit to prevent OOM)
    for op in arith.ops.iter().take(20) {
        match op {
            ArithOp::Add => {
                a.add_assign(&b);
            }
            ArithOp::Sub => {
                a.sub_assign(&b);
            }
            ArithOp::Neg => {
                a.neg_assign();
            }
            ArithOp::ScalarMul(s) => {
                a.scalar_mul(*s);
            }
            ArithOp::Ntt => {
                a.ntt();
            }
            ArithOp::InvNtt => {
                a.inv_ntt();
            }
        }

        // After each operation, polynomial should still be valid
        assert!(a.is_valid(), "Polynomial became invalid after {:?}", op);
    }

    // Test that add/sub are inverses
    let mut c = original.clone();
    c.add_assign(&b);
    c.sub_assign(&b);
    // Note: Due to modular arithmetic, this should give back original
    assert_eq!(c, original, "Add then subtract should return original");

    // Test that double negation is identity
    let mut d = original.clone();
    d.neg_assign();
    d.neg_assign();
    assert_eq!(d, original, "Double negation should be identity");
}

/// Test serialization roundtrip
fn fuzz_serialization(ser: &SerializationFuzz) {
    // Create polynomial from fuzz input
    let mut p = Poly::zero();
    for (i, &coeff) in ser.coefficients.iter().take(N).enumerate() {
        p.coeffs[i] = coeff % Q;
    }

    // Serialize
    let mut bytes = p.to_bytes();

    // Optionally corrupt
    if ser.corrupt && !bytes.is_empty() {
        let pos = (ser.corrupt_pos as usize) % bytes.len();
        bytes[pos] = ser.corrupt_val;
    }

    // Deserialize - should not panic
    match Poly::from_bytes(&bytes) {
        Some(recovered) => {
            if !ser.corrupt {
                // If not corrupted, should match original
                assert_eq!(p, recovered, "Serialization roundtrip failed");
            }
            // Either way, recovered polynomial should be valid
            assert!(recovered.is_valid());
        }
        None => {
            // Corruption may cause invalid bytes (coefficient >= Q)
            // This is expected behavior
        }
    }

    // Test with wrong-sized input
    let short = &bytes[..bytes.len().saturating_sub(1).max(0)];
    assert!(Poly::from_bytes(short).is_none(), "Short input should fail");

    let mut long = bytes.to_vec();
    long.push(0);
    assert!(Poly::from_bytes(&long).is_none(), "Long input should fail");
}

/// Test matrix operations
fn fuzz_matrix_ops(matrix: &MatrixFuzz) {
    // Expand matrix from seed
    let a = PolyMatrix::expand_a(&matrix.matrix_seed);

    // Create vector
    let mut rng = ChaCha20Rng::from_seed(matrix.vector_seed);
    let s = PolyVecL::sample_small(&mut rng, 2);

    // Compute matrix-vector product
    let result = a.mul_vec(&s);

    // Result should be valid
    for poly in result.polys.iter() {
        assert!(poly.is_valid());
    }

    if matrix.compare_methods {
        // Compare NTT-based vs naive multiplication
        let naive_result = a.mul_vec_naive(&s);

        for i in 0..4 {
            for j in 0..N {
                assert_eq!(
                    result.polys[i].coeffs[j],
                    naive_result.polys[i].coeffs[j],
                    "NTT and naive matrix mul differ at poly {} coeff {}",
                    i, j
                );
            }
        }
    }

    // Matrix expansion should be deterministic
    let a2 = PolyMatrix::expand_a(&matrix.matrix_seed);
    assert_eq!(a, a2, "Matrix expansion must be deterministic");
}

/// Test edge cases
fn fuzz_edge_cases(edge: &EdgeCaseFuzz) {
    let p = match &edge.case {
        EdgeCase::AllZero => Poly::zero(),
        EdgeCase::AllMax => {
            let mut p = Poly::zero();
            for c in p.coeffs.iter_mut() {
                *c = Q - 1;
            }
            p
        }
        EdgeCase::Alternating => {
            let mut p = Poly::zero();
            for (i, c) in p.coeffs.iter_mut().enumerate() {
                *c = if i % 2 == 0 { 0 } else { Q - 1 };
            }
            p
        }
        EdgeCase::SingleCoeff { index, value } => {
            let mut p = Poly::zero();
            let idx = (*index as usize) % N;
            p.coeffs[idx] = *value % Q;
            p
        }
        EdgeCase::Challenge { seed, tau } => {
            let tau = (*tau as usize % 64).max(1);
            Poly::sample_challenge(seed, tau)
        }
    };

    // Polynomial should be valid
    assert!(p.is_valid());

    // Serialization should work
    let bytes = p.to_bytes();
    let recovered = Poly::from_bytes(&bytes).expect("Valid polynomial should deserialize");
    assert_eq!(p, recovered);

    // Arithmetic with edge cases should work
    let mut rng = ChaCha20Rng::from_seed([0u8; 32]);
    let other = Poly::sample_uniform(&mut rng);

    let sum = &p + &other;
    assert!(sum.is_valid());

    let diff = &p - &other;
    assert!(diff.is_valid());

    let neg = -&p;
    assert!(neg.is_valid());

    // Multiplication with edge cases
    let prod = p.ntt_mul(&other);
    assert!(prod.is_valid());

    // Multiplication by zero should give zero
    let zero = Poly::zero();
    let zero_prod = p.ntt_mul(&zero);
    assert_eq!(zero_prod, zero, "Multiplication by zero should give zero");

    // Multiplication by one should give identity
    let one = Poly::constant(1);
    let one_prod = p.ntt_mul(&one);
    assert_eq!(one_prod, p, "Multiplication by one should be identity");
}

/// Test raw coefficient bytes
fn fuzz_raw_coefficients(raw: &RawCoeffFuzz) {
    // Try to parse as polynomial bytes
    let _ = Poly::from_bytes(&raw.bytes);

    // Try with padded/truncated versions
    if raw.bytes.len() >= 768 {
        let _ = Poly::from_bytes(&raw.bytes[..768]);
    }

    // Create polynomial from raw coefficients as u32s
    let mut p = Poly::zero();
    for (i, chunk) in raw.bytes.chunks(4).enumerate() {
        if i >= N {
            break;
        }
        let mut bytes = [0u8; 4];
        bytes[..chunk.len()].copy_from_slice(chunk);
        let val = u32::from_le_bytes(bytes) % Q;
        p.coeffs[i] = val;
    }

    // Operations on this polynomial should not panic
    assert!(p.is_valid());
    let _ = p.to_bytes();
    let _ = p.infinity_norm();
    let _ = p.check_norm(1000);

    // NTT should work
    let mut p_ntt = p.clone();
    p_ntt.ntt();
    assert!(p_ntt.is_valid());
}
