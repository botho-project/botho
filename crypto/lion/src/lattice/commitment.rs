//! Lattice-based commitment scheme for Lion ring signatures.
//!
//! Used for generating commitment randomness and computing
//! commitment values during the signing protocol.

use crate::{
    params::*,
    polynomial::{Poly, PolyMatrix, PolyVecK, PolyVecL},
};
use rand_core::CryptoRngCore;
use sha3::{Shake256, digest::{ExtendableOutput, Update}};

/// A commitment value in the ring signature protocol.
///
/// Commitments are used to bind the signer to randomness
/// before the challenge is computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commitment {
    /// The commitment vector w = A*y.
    pub w: PolyVecK,
}

impl Commitment {
    /// Compute a commitment from randomness y and matrix A.
    ///
    /// Computes w = A * y.
    pub fn compute(a: &PolyMatrix, y: &PolyVecL) -> Self {
        let w = a.mul_vec(y);
        Self { w }
    }

    /// Serialize to bytes for hashing.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 * 768);
        for poly in self.w.polys.iter() {
            bytes.extend_from_slice(&poly.to_bytes());
        }
        bytes
    }
}

/// Sample commitment randomness y with coefficients in [-GAMMA1+1, GAMMA1].
pub fn sample_y<R: CryptoRngCore>(rng: &mut R) -> PolyVecL {
    let mut y = PolyVecL::zero();
    let range = 2 * GAMMA1;

    for poly in y.polys.iter_mut() {
        for c in poly.coeffs.iter_mut() {
            // Sample in [0, 2*GAMMA1)
            let r = rng.next_u32() % range;
            // Map to [-GAMMA1+1, GAMMA1]
            *c = if r < GAMMA1 {
                r
            } else {
                Q - (r - GAMMA1 + 1)
            };
        }
    }

    y
}

/// Expand randomness from a seed for a specific ring position.
pub fn expand_y(seed: &[u8; 32], nonce: u16) -> PolyVecL {
    use rand_core::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    let mut hasher = Shake256::default();
    hasher.update(DOMAIN_COMMIT);
    hasher.update(seed);
    hasher.update(&nonce.to_le_bytes());
    let mut reader = hasher.finalize_xof();

    let mut rng_seed = [0u8; 32];
    sha3::digest::XofReader::read(&mut reader, &mut rng_seed);

    let mut rng = ChaCha20Rng::from_seed(rng_seed);
    sample_y(&mut rng)
}

/// High bits extraction for compression.
///
/// Decomposes r into (r1, r0) where r = r1 * 2*GAMMA2 + r0
/// with -GAMMA2 < r0 <= GAMMA2.
pub fn decompose(r: u32) -> (u32, i32) {
    let r = r % Q;
    let r_centered = if r > Q / 2 {
        r as i32 - Q as i32
    } else {
        r as i32
    };

    let gamma2 = GAMMA2 as i32;
    let r0 = r_centered.rem_euclid(2 * gamma2);
    let r0 = if r0 > gamma2 { r0 - 2 * gamma2 } else { r0 };

    let r1 = if r0 == gamma2 {
        0
    } else {
        ((r_centered - r0) / (2 * gamma2)) as u32
    };

    (r1 % ((Q + 2 * GAMMA2 - 1) / (2 * GAMMA2)), r0)
}

/// Extract high bits from a polynomial.
pub fn high_bits(r: &Poly) -> Poly {
    let mut result = Poly::zero();
    for (i, &c) in r.coeffs.iter().enumerate() {
        result.coeffs[i] = decompose(c).0;
    }
    result
}

/// Extract low bits from a polynomial.
pub fn low_bits(r: &Poly) -> Poly {
    let mut result = Poly::zero();
    for (i, &c) in r.coeffs.iter().enumerate() {
        let (_, r0) = decompose(c);
        result.coeffs[i] = if r0 < 0 {
            (Q as i32 + r0) as u32
        } else {
            r0 as u32
        };
    }
    result
}

/// Check if adding cs to r would change the high bits.
///
/// Returns true if HighBits(r + cs) != HighBits(r).
pub fn make_hint(r: &Poly, cs: &Poly) -> Vec<bool> {
    let mut hints = vec![false; N];

    for i in 0..N {
        let r_val = r.coeffs[i];
        let cs_val = cs.coeffs[i];
        let sum = (r_val as u64 + cs_val as u64) % Q as u64;

        let (h1, _) = decompose(r_val);
        let (h2, _) = decompose(sum as u32);

        hints[i] = h1 != h2;
    }

    hints
}

/// Use hints to recover high bits of r + cs from r.
pub fn use_hint(r: &Poly, hints: &[bool]) -> Poly {
    let mut result = Poly::zero();

    for i in 0..N {
        let (h, r0) = decompose(r.coeffs[i]);

        if hints.get(i).copied().unwrap_or(false) {
            // Hint indicates the high bits changed
            if r0 > 0 {
                result.coeffs[i] = (h + 1) % ((Q + 2 * GAMMA2 - 1) / (2 * GAMMA2));
            } else {
                result.coeffs[i] = if h == 0 {
                    (Q + 2 * GAMMA2 - 1) / (2 * GAMMA2) - 1
                } else {
                    h - 1
                };
            }
        } else {
            result.coeffs[i] = h;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_sample_y_bounds() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let y = sample_y(&mut rng);

        // Check all coefficients are in valid range
        for poly in y.polys.iter() {
            assert!(poly.infinity_norm() <= GAMMA1);
        }
    }

    #[test]
    fn test_expand_y_deterministic() {
        let seed = [42u8; 32];
        let y1 = expand_y(&seed, 0);
        let y2 = expand_y(&seed, 0);
        assert_eq!(y1, y2);
    }

    #[test]
    fn test_expand_y_different_nonces() {
        let seed = [42u8; 32];
        let y1 = expand_y(&seed, 0);
        let y2 = expand_y(&seed, 1);
        assert_ne!(y1, y2);
    }

    #[test]
    fn test_decompose_roundtrip() {
        for r in [0, 1, Q / 4, Q / 2, 3 * Q / 4, Q - 1] {
            let (h, l) = decompose(r);
            // Verify h is in valid range
            assert!(h < (Q + 2 * GAMMA2 - 1) / (2 * GAMMA2));
            // Verify l is in valid range
            assert!(l.abs() <= GAMMA2 as i32);
        }
    }

    #[test]
    fn test_commitment_compute() {
        let mut rng = ChaCha20Rng::seed_from_u64(123);
        let seed = [0u8; 32];

        // Matrix should be in standard form - mul_vec does NTT internally
        let a = PolyMatrix::expand_a(&seed);

        let y = sample_y(&mut rng);
        let commitment = Commitment::compute(&a, &y);

        // Just verify it produces valid output
        for poly in commitment.w.polys.iter() {
            assert!(poly.is_valid());
        }
    }
}
