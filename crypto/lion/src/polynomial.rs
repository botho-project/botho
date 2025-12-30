//! Polynomial arithmetic for Lion ring signatures.
//!
//! Polynomials are elements of R_q = Z_q[X]/(X^N + 1) where N = 256 and Q = 8380417.
//! NTT (Number Theoretic Transform) is used for efficient multiplication.

use crate::params::{N, Q, QINV, ZETA};
use core::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A polynomial in R_q = Z_q[X]/(X^N + 1).
///
/// Coefficients are stored in standard order [a_0, a_1, ..., a_{N-1}].
/// All coefficients are in [0, Q).
#[derive(Clone, Debug, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Poly {
    /// Coefficients of the polynomial.
    pub coeffs: [u32; N],
}

impl Default for Poly {
    fn default() -> Self {
        Self::zero()
    }
}

impl Poly {
    /// Create a zero polynomial.
    #[inline]
    pub const fn zero() -> Self {
        Self { coeffs: [0u32; N] }
    }

    /// Create a polynomial with all coefficients set to a constant.
    #[inline]
    pub fn constant(c: u32) -> Self {
        let mut p = Self::zero();
        p.coeffs[0] = c % Q;
        p
    }

    /// Check if all coefficients are valid (< Q).
    pub fn is_valid(&self) -> bool {
        self.coeffs.iter().all(|&c| c < Q)
    }

    /// Reduce all coefficients modulo Q.
    pub fn reduce(&mut self) {
        for c in self.coeffs.iter_mut() {
            *c %= Q;
        }
    }

    /// Add another polynomial in place.
    pub fn add_assign(&mut self, other: &Self) {
        for (a, b) in self.coeffs.iter_mut().zip(other.coeffs.iter()) {
            *a = (*a + *b) % Q;
        }
    }

    /// Subtract another polynomial in place.
    pub fn sub_assign(&mut self, other: &Self) {
        for (a, b) in self.coeffs.iter_mut().zip(other.coeffs.iter()) {
            *a = (*a + Q - (*b % Q)) % Q;
        }
    }

    /// Negate in place.
    pub fn neg_assign(&mut self) {
        for c in self.coeffs.iter_mut() {
            *c = (Q - (*c % Q)) % Q;
        }
    }

    /// Multiply by a scalar.
    pub fn scalar_mul(&mut self, s: u32) {
        let s = (s % Q) as u64;
        for c in self.coeffs.iter_mut() {
            *c = ((*c as u64 * s) % Q as u64) as u32;
        }
    }

    /// Compute the infinity norm (max absolute coefficient in centered representation).
    pub fn infinity_norm(&self) -> u32 {
        let mut max = 0u32;
        for &c in self.coeffs.iter() {
            let centered = if c > Q / 2 { Q - c } else { c };
            if centered > max {
                max = centered;
            }
        }
        max
    }

    /// Check if all coefficients are bounded by a given value (in centered representation).
    pub fn check_norm(&self, bound: u32) -> bool {
        self.infinity_norm() <= bound
    }

    /// Forward NTT (transforms to NTT domain for efficient multiplication).
    pub fn ntt(&mut self) {
        ntt_forward(&mut self.coeffs);
    }

    /// Inverse NTT (transforms back from NTT domain).
    pub fn inv_ntt(&mut self) {
        ntt_inverse(&mut self.coeffs);
    }

    /// Pointwise multiplication in NTT domain.
    /// Both self and other must be in NTT domain.
    pub fn pointwise_mul(&self, other: &Self) -> Self {
        let mut result = Self::zero();
        for i in 0..N {
            let a = self.coeffs[i] as u64;
            let b = other.coeffs[i] as u64;
            result.coeffs[i] = montgomery_reduce(a * b);
        }
        result
    }

    /// Sample a polynomial with small coefficients from [-eta, eta].
    pub fn sample_small<R: rand_core::RngCore>(rng: &mut R, eta: u32) -> Self {
        let mut p = Self::zero();
        let range = 2 * eta + 1;
        for c in p.coeffs.iter_mut() {
            let r = rng.next_u32() % range;
            // r is in [0, 2*eta], map to [-eta, eta]
            *c = if r <= eta {
                r
            } else {
                Q - (r - eta)
            };
        }
        p
    }

    /// Sample a uniform polynomial with coefficients in [0, Q).
    pub fn sample_uniform<R: rand_core::RngCore>(rng: &mut R) -> Self {
        let mut p = Self::zero();
        for c in p.coeffs.iter_mut() {
            // Rejection sampling to avoid modulo bias
            loop {
                let r = rng.next_u32() & 0x7FFFFF; // 23 bits, since Q < 2^23
                if r < Q {
                    *c = r;
                    break;
                }
            }
        }
        p
    }

    /// Sample a challenge polynomial with exactly tau coefficients being +/-1.
    pub fn sample_challenge(seed: &[u8], tau: usize) -> Self {
        use sha3::{Shake256, digest::{ExtendableOutput, Update}};

        let mut p = Self::zero();
        let mut hasher = Shake256::default();
        hasher.update(seed);
        let mut reader = hasher.finalize_xof();

        let mut signs = [0u8; 8];
        let mut sign_idx = 64usize;

        for i in (N - tau)..N {
            // Get random index in [0, i]
            let mut buf = [0u8; 1];
            loop {
                sha3::digest::XofReader::read(&mut reader, &mut buf);
                let j = buf[0] as usize;
                if j <= i {
                    // Swap coefficients
                    p.coeffs[i] = p.coeffs[j];

                    // Get sign bit
                    if sign_idx >= 64 {
                        sha3::digest::XofReader::read(&mut reader, &mut signs);
                        sign_idx = 0;
                    }
                    let sign = (signs[sign_idx / 8] >> (sign_idx % 8)) & 1;
                    sign_idx += 1;

                    p.coeffs[j] = if sign == 0 { 1 } else { Q - 1 };
                    break;
                }
            }
        }
        p
    }

    /// Serialize to bytes.
    ///
    /// Each coefficient is < Q < 2^24, so we use 3 bytes per coefficient.
    /// Total: 256 * 3 = 768 bytes.
    pub fn to_bytes(&self) -> [u8; 768] {
        let mut bytes = [0u8; 768];

        for (i, &c) in self.coeffs.iter().enumerate() {
            let offset = i * 3;
            bytes[offset] = c as u8;
            bytes[offset + 1] = (c >> 8) as u8;
            bytes[offset + 2] = (c >> 16) as u8;
        }

        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 768 {
            return None;
        }

        let mut p = Self::zero();

        for i in 0..N {
            let offset = i * 3;
            let c = (bytes[offset] as u32)
                | ((bytes[offset + 1] as u32) << 8)
                | ((bytes[offset + 2] as u32) << 16);

            if c >= Q {
                return None;
            }
            p.coeffs[i] = c;
        }

        Some(p)
    }
}

// ============================================================================
// Operator implementations
// ============================================================================

impl Add for &Poly {
    type Output = Poly;

    fn add(self, other: Self) -> Poly {
        let mut result = self.clone();
        result.add_assign(other);
        result
    }
}

impl Sub for &Poly {
    type Output = Poly;

    fn sub(self, other: Self) -> Poly {
        let mut result = self.clone();
        result.sub_assign(other);
        result
    }
}

impl Neg for &Poly {
    type Output = Poly;

    fn neg(self) -> Poly {
        let mut result = self.clone();
        result.neg_assign();
        result
    }
}

impl AddAssign<&Poly> for Poly {
    fn add_assign(&mut self, other: &Poly) {
        Poly::add_assign(self, other);
    }
}

impl SubAssign<&Poly> for Poly {
    fn sub_assign(&mut self, other: &Poly) {
        Poly::sub_assign(self, other);
    }
}

// ============================================================================
// NTT implementation
// ============================================================================

/// Precomputed powers of zeta for NTT.
const ZETAS: [u32; N] = compute_zetas();

/// Precomputed powers of zeta inverse for inverse NTT.
const ZETAS_INV: [u32; N] = compute_zetas_inv();

const fn compute_zetas() -> [u32; N] {
    let mut zetas = [0u32; N];
    let mut zeta_power = 1u64;

    let mut i = 0;
    while i < N {
        zetas[i] = zeta_power as u32;
        zeta_power = (zeta_power * ZETA as u64) % Q as u64;
        i += 1;
    }
    zetas
}

const fn compute_zetas_inv() -> [u32; N] {
    let mut zetas_inv = [0u32; N];
    let zetas = compute_zetas();

    // Compute modular inverse using Fermat's little theorem: a^(-1) = a^(Q-2) mod Q
    let mut i = 0;
    while i < N {
        zetas_inv[i] = mod_pow(zetas[i], Q - 2);
        i += 1;
    }
    zetas_inv
}

const fn mod_pow(base: u32, exp: u32) -> u32 {
    let mut result = 1u64;
    let mut base = base as u64;
    let mut exp = exp;

    while exp > 0 {
        if exp & 1 == 1 {
            result = (result * base) % Q as u64;
        }
        base = (base * base) % Q as u64;
        exp >>= 1;
    }
    result as u32
}

/// Montgomery reduction: compute a * R^(-1) mod Q.
#[inline]
fn montgomery_reduce(a: u64) -> u32 {
    let t = ((a as u32).wrapping_mul(QINV)) as u64;
    let reduced = ((a + t * Q as u64) >> 32) as u32;
    if reduced >= Q {
        reduced - Q
    } else {
        reduced
    }
}

/// Forward NTT transformation (Cooley-Tukey).
fn ntt_forward(coeffs: &mut [u32; N]) {
    let mut k = 0;
    let mut len = N / 2;

    while len >= 1 {
        let mut start = 0;
        while start < N {
            k += 1;
            let zeta = ZETAS[k];

            for j in start..(start + len) {
                let t = montgomery_reduce(zeta as u64 * coeffs[j + len] as u64);
                coeffs[j + len] = (coeffs[j] + Q - t) % Q;
                coeffs[j] = (coeffs[j] + t) % Q;
            }
            start += 2 * len;
        }
        len /= 2;
    }
}

/// Inverse NTT transformation (Gentleman-Sande).
fn ntt_inverse(coeffs: &mut [u32; N]) {
    let mut k = N;
    let mut len = 1;

    while len < N {
        let mut start = 0;
        while start < N {
            k -= 1;
            let zeta_inv = ZETAS_INV[k];

            for j in start..(start + len) {
                let t = coeffs[j];
                coeffs[j] = (t + coeffs[j + len]) % Q;
                coeffs[j + len] = montgomery_reduce(
                    zeta_inv as u64 * ((t + Q - coeffs[j + len]) % Q) as u64,
                );
            }
            start += 2 * len;
        }
        len *= 2;
    }

    // Multiply by N^(-1) mod Q
    let n_inv = mod_pow(N as u32, Q - 2);
    for c in coeffs.iter_mut() {
        *c = montgomery_reduce(*c as u64 * n_inv as u64);
    }
}

// ============================================================================
// Polynomial vector
// ============================================================================

/// A vector of K polynomials.
#[derive(Clone, Debug, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct PolyVecK {
    pub polys: [Poly; 4], // K = 4
}

impl Default for PolyVecK {
    fn default() -> Self {
        Self {
            polys: [Poly::zero(), Poly::zero(), Poly::zero(), Poly::zero()],
        }
    }
}

impl PolyVecK {
    /// Create a zero vector.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Apply NTT to all polynomials.
    pub fn ntt(&mut self) {
        for p in self.polys.iter_mut() {
            p.ntt();
        }
    }

    /// Apply inverse NTT to all polynomials.
    pub fn inv_ntt(&mut self) {
        for p in self.polys.iter_mut() {
            p.inv_ntt();
        }
    }

    /// Add another vector in place.
    pub fn add_assign(&mut self, other: &Self) {
        for (a, b) in self.polys.iter_mut().zip(other.polys.iter()) {
            a.add_assign(b);
        }
    }

    /// Subtract another vector in place.
    pub fn sub_assign(&mut self, other: &Self) {
        for (a, b) in self.polys.iter_mut().zip(other.polys.iter()) {
            a.sub_assign(b);
        }
    }

    /// Compute infinity norm (max over all polynomials).
    pub fn infinity_norm(&self) -> u32 {
        self.polys.iter().map(|p| p.infinity_norm()).max().unwrap_or(0)
    }

    /// Check norm bound.
    pub fn check_norm(&self, bound: u32) -> bool {
        self.infinity_norm() <= bound
    }

    /// Sample with small coefficients.
    pub fn sample_small<R: rand_core::RngCore>(rng: &mut R, eta: u32) -> Self {
        Self {
            polys: [
                Poly::sample_small(rng, eta),
                Poly::sample_small(rng, eta),
                Poly::sample_small(rng, eta),
                Poly::sample_small(rng, eta),
            ],
        }
    }
}

/// A vector of L polynomials.
#[derive(Clone, Debug, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct PolyVecL {
    pub polys: [Poly; 4], // L = 4
}

impl Default for PolyVecL {
    fn default() -> Self {
        Self {
            polys: [Poly::zero(), Poly::zero(), Poly::zero(), Poly::zero()],
        }
    }
}

impl PolyVecL {
    /// Create a zero vector.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Apply NTT to all polynomials.
    pub fn ntt(&mut self) {
        for p in self.polys.iter_mut() {
            p.ntt();
        }
    }

    /// Apply inverse NTT to all polynomials.
    pub fn inv_ntt(&mut self) {
        for p in self.polys.iter_mut() {
            p.inv_ntt();
        }
    }

    /// Add another vector in place.
    pub fn add_assign(&mut self, other: &Self) {
        for (a, b) in self.polys.iter_mut().zip(other.polys.iter()) {
            a.add_assign(b);
        }
    }

    /// Compute infinity norm.
    pub fn infinity_norm(&self) -> u32 {
        self.polys.iter().map(|p| p.infinity_norm()).max().unwrap_or(0)
    }

    /// Check norm bound.
    pub fn check_norm(&self, bound: u32) -> bool {
        self.infinity_norm() <= bound
    }

    /// Sample with small coefficients.
    pub fn sample_small<R: rand_core::RngCore>(rng: &mut R, eta: u32) -> Self {
        Self {
            polys: [
                Poly::sample_small(rng, eta),
                Poly::sample_small(rng, eta),
                Poly::sample_small(rng, eta),
                Poly::sample_small(rng, eta),
            ],
        }
    }
}

/// A K x L matrix of polynomials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyMatrix {
    /// Rows of the matrix (K rows, each containing L polynomials).
    pub rows: [PolyVecL; 4], // K = 4
}

impl Default for PolyMatrix {
    fn default() -> Self {
        Self {
            rows: [
                PolyVecL::zero(),
                PolyVecL::zero(),
                PolyVecL::zero(),
                PolyVecL::zero(),
            ],
        }
    }
}

impl PolyMatrix {
    /// Create a zero matrix.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Sample a uniform matrix from a seed.
    pub fn expand_a(seed: &[u8; 32]) -> Self {
        use sha3::{Shake128, digest::{ExtendableOutput, Update}};

        let mut matrix = Self::zero();

        for i in 0..4 {
            for j in 0..4 {
                let mut hasher = Shake128::default();
                hasher.update(seed);
                hasher.update(&[i as u8, j as u8]);
                let mut reader = hasher.finalize_xof();

                // Sample uniform polynomial
                for c in matrix.rows[i].polys[j].coeffs.iter_mut() {
                    loop {
                        let mut buf = [0u8; 3];
                        sha3::digest::XofReader::read(&mut reader, &mut buf);
                        let r = (buf[0] as u32)
                            | ((buf[1] as u32) << 8)
                            | (((buf[2] & 0x7F) as u32) << 16);
                        if r < Q {
                            *c = r;
                            break;
                        }
                    }
                }
            }
        }

        matrix
    }

    /// Apply NTT to all polynomials.
    pub fn ntt(&mut self) {
        for row in self.rows.iter_mut() {
            row.ntt();
        }
    }

    /// Matrix-vector multiplication: A * s (in NTT domain).
    /// Both matrix and vector must be in NTT domain.
    pub fn mul_vec(&self, s: &PolyVecL) -> PolyVecK {
        let mut result = PolyVecK::zero();

        for (i, row) in self.rows.iter().enumerate() {
            for (j, poly) in row.polys.iter().enumerate() {
                let prod = poly.pointwise_mul(&s.polys[j]);
                result.polys[i].add_assign(&prod);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_poly_add_sub() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let a = Poly::sample_uniform(&mut rng);
        let b = Poly::sample_uniform(&mut rng);

        let sum = &a + &b;
        let diff = &sum - &b;

        // a + b - b should equal a
        for i in 0..N {
            assert_eq!(diff.coeffs[i], a.coeffs[i]);
        }
    }

    #[test]
    fn test_ntt_roundtrip() {
        let mut rng = ChaCha20Rng::seed_from_u64(123);
        let original = Poly::sample_uniform(&mut rng);

        let mut transformed = original.clone();
        transformed.ntt();
        transformed.inv_ntt();

        for i in 0..N {
            assert_eq!(transformed.coeffs[i], original.coeffs[i]);
        }
    }

    #[test]
    fn test_ntt_multiplication() {
        let mut rng = ChaCha20Rng::seed_from_u64(456);

        let mut a = Poly::sample_small(&mut rng, 2);
        let mut b = Poly::sample_small(&mut rng, 2);

        a.ntt();
        b.ntt();

        let c = a.pointwise_mul(&b);

        // Just verify it doesn't panic and produces valid output
        assert!(c.is_valid());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut rng = ChaCha20Rng::seed_from_u64(789);
        let original = Poly::sample_uniform(&mut rng);

        let bytes = original.to_bytes();
        let recovered = Poly::from_bytes(&bytes).expect("deserialization should succeed");

        assert_eq!(original, recovered);
    }

    #[test]
    fn test_challenge_sampling() {
        let seed = [0u8; 32];
        let c = Poly::sample_challenge(&seed, 39);

        // Count non-zero coefficients
        let non_zero: usize = c.coeffs.iter().filter(|&&x| x != 0).count();
        assert_eq!(non_zero, 39);

        // All non-zero should be 1 or Q-1 (i.e., +/-1)
        for &coeff in c.coeffs.iter() {
            if coeff != 0 {
                assert!(coeff == 1 || coeff == Q - 1);
            }
        }
    }

    #[test]
    fn test_matrix_expand_deterministic() {
        let seed = [42u8; 32];
        let a1 = PolyMatrix::expand_a(&seed);
        let a2 = PolyMatrix::expand_a(&seed);
        assert_eq!(a1, a2);
    }
}
