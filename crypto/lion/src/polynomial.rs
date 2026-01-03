//! Polynomial arithmetic for Lion ring signatures.
//!
//! Polynomials are elements of R_q = Z_q[X]/(X^N + 1) where N = 256 and Q = 8380417.
//! NTT (Number Theoretic Transform) is used for efficient multiplication.

use crate::params::{N, Q, QINV};
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

    /// Pointwise multiplication in NTT domain with Montgomery reduction.
    /// Both self and other must be in NTT domain.
    #[inline]
    pub fn pointwise_mul(&self, other: &Self) -> Self {
        let mut result = Self::zero();
        for i in 0..N {
            let a = self.coeffs[i] as i64;
            let b = other.coeffs[i] as i64;
            let prod = montgomery_reduce(a * b);
            result.coeffs[i] = caddq(prod) as u32;
        }
        result
    }

    /// Multiply two polynomials using NTT.
    /// This is the fast O(N log N) multiplication method.
    pub fn ntt_mul(&self, other: &Self) -> Self {
        let mut a = self.clone();
        let mut b = other.clone();
        a.ntt();
        b.ntt();
        let mut c = a.pointwise_mul(&b);
        c.inv_ntt();
        c
    }

    /// Naive polynomial multiplication in R_q = Z_q[X]/(X^N + 1).
    /// Slower than NTT but guaranteed correct.
    pub fn naive_mul(&self, other: &Self) -> Self {
        let mut result = Self::zero();

        // Compute product modulo X^N + 1
        for i in 0..N {
            for j in 0..N {
                let k = i + j;
                let prod = (self.coeffs[i] as u64 * other.coeffs[j] as u64) % Q as u64;

                if k < N {
                    // Normal addition
                    result.coeffs[k] = (result.coeffs[k] as u64 + prod) as u32 % Q;
                } else {
                    // Wrap around with negation (due to X^N = -1)
                    let idx = k - N;
                    result.coeffs[idx] = (result.coeffs[idx] as u64 + Q as u64 - prod) as u32 % Q;
                }
            }
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
//
// This NTT implementation is ported from the Dilithium reference implementation
// (pq-crystals/dilithium). The zetas are precomputed in Montgomery form with
// the correct bit-reversed ordering for the Cooley-Tukey butterfly structure.
//
// Key properties:
// - Works over Z_q[X]/(X^N + 1) with Q = 8380417, N = 256
// - Uses Montgomery representation for efficient modular arithmetic
// - Zetas are ζ^(brv(k)) in Montgomery form, where brv is bit-reversal

/// Precomputed zetas for forward NTT in Montgomery form.
/// These are ζ^(brv(k)) * 2^32 mod Q for the Cooley-Tukey butterfly.
/// Ported from Dilithium reference implementation.
const ZETAS: [i32; N] = [
    0, 25847, -2608894, -518909, 237124, -777960, -876248, 466468,
    1826347, 2353451, -359251, -2091905, 3119733, -2884855, 3111497, 2680103,
    2725464, 1024112, -1079900, 3585928, -549488, -1119584, 2619752, -2108549,
    -2118186, -3859737, -1399561, -3277672, 1757237, -19422, 4010497, 280005,
    2706023, 95776, 3077325, 3530437, -1661693, -3592148, -2537516, 3915439,
    -3861115, -3043716, 3574422, -2867647, 3539968, -300467, 2348700, -539299,
    -1699267, -1643818, 3505694, -3821735, 3507263, -2140649, -1600420, 3699596,
    811944, 531354, 954230, 3881043, 3900724, -2556880, 2071892, -2797779,
    -3930395, -1528703, -3677745, -3041255, -1452451, 3475950, 2176455, -1585221,
    -1257611, 1939314, -4083598, -1000202, -3190144, -3157330, -3632928, 126922,
    3412210, -983419, 2147896, 2715295, -2967645, -3693493, -411027, -2477047,
    -671102, -1228525, -22981, -1308169, -381987, 1349076, 1852771, -1430430,
    -3343383, 264944, 508951, 3097992, 44288, -1100098, 904516, 3958618,
    -3724342, -8578, 1653064, -3249728, 2389356, -210977, 759969, -1316856,
    189548, -3553272, 3159746, -1851402, -2409325, -177440, 1315589, 1341330,
    1285669, -1584928, -812732, -1439742, -3019102, -3881060, -3628969, 3839961,
    2091667, 3407706, 2316500, 3817976, -3342478, 2244091, -2446433, -3562462,
    266997, 2434439, -1235728, 3513181, -3520352, -3759364, -1197226, -3193378,
    900702, 1859098, 909542, 819034, 495491, -1613174, -43260, -522500,
    -655327, -3122442, 2031748, 3207046, -3556995, -525098, -768622, -3595838,
    342297, 286988, -2437823, 4108315, 3437287, -3342277, 1735879, 203044,
    2842341, 2691481, -2590150, 1265009, 4055324, 1247620, 2486353, 1595974,
    -3767016, 1250494, 2635921, -3548272, -2994039, 1869119, 1903435, -1050970,
    -1333058, 1237275, -3318210, -1430225, -451100, 1312455, 3306115, -1962642,
    -1279661, 1917081, -2546312, -1374803, 1500165, 777191, 2235880, 3406031,
    -542412, -2831860, -1671176, -1846953, -2584293, -3724270, 594136, -3776993,
    -2013608, 2432395, 2454455, -164721, 1957272, 3369112, 185531, -1207385,
    -3183426, 162844, 1616392, 3014001, 810149, 1652634, -3694233, -1799107,
    -3038916, 3523897, 3866901, 269760, 2213111, -975884, 1717735, 472078,
    -426683, 1723600, -1803090, 1910376, -1667432, -1104333, -260646, -3833893,
    -2939036, -2235985, -420899, -2286327, 183443, -976891, 1612842, -3545687,
    -554416, 3919660, -48306, -1362209, 3937738, 1400424, -846154, 1976782,
];

/// Scaling factor for inverse NTT: f = mont^2 / 256 mod Q
/// This combines the 1/N scaling with Montgomery correction.
const NTT_F: i32 = 41978;

/// Montgomery reduction: compute a * 2^(-32) mod Q.
///
/// For a in [-2^31*Q, 2^31*Q], returns r in (-Q, Q) with a ≡ r * 2^32 (mod Q).
#[inline(always)]
fn montgomery_reduce(a: i64) -> i32 {
    let t = (a as i32).wrapping_mul(QINV as i32);
    ((a - (t as i64) * (Q as i64)) >> 32) as i32
}

/// Reduce a coefficient to the range [0, Q).
#[inline]
fn reduce32(a: i32) -> i32 {
    let t = (a + (1 << 22)) >> 23;
    a - t * (Q as i32)
}

/// Conditionally add Q to ensure positive value.
#[inline]
fn caddq(a: i32) -> i32 {
    let mut a = a;
    a += (a >> 31) & (Q as i32);
    a
}

/// Forward NTT transformation (Cooley-Tukey, decimation-in-frequency).
///
/// Input: coefficients in standard order, each in [0, Q).
/// Output: coefficients in bit-reversed order, each in [0, Q).
///
/// After NTT, polynomials can be multiplied pointwise.
#[inline]
fn ntt_forward(coeffs: &mut [u32; N]) {
    // Convert to signed for internal computation
    let mut a: [i32; N] = [0; N];
    for i in 0..N {
        a[i] = coeffs[i] as i32;
    }

    let mut k = 0usize;
    let mut len = 128;

    while len >= 1 {
        let mut start = 0;
        while start < N {
            k += 1;
            let zeta = ZETAS[k];

            for j in start..(start + len) {
                let t = montgomery_reduce(zeta as i64 * a[j + len] as i64);
                a[j + len] = a[j] - t;
                a[j] += t;
            }
            start += 2 * len;
        }
        len >>= 1;
    }

    // Convert back to unsigned, reducing to [0, Q)
    for i in 0..N {
        coeffs[i] = caddq(reduce32(a[i])) as u32;
    }
}

/// Inverse NTT transformation (Gentleman-Sande, decimation-in-time).
///
/// Input: coefficients in bit-reversed order (from forward NTT).
/// Output: coefficients in standard order, each in [0, Q).
///
/// Includes the 1/N scaling factor combined with Montgomery correction.
#[inline]
fn ntt_inverse(coeffs: &mut [u32; N]) {
    // Convert to signed for internal computation
    let mut a: [i32; N] = [0; N];
    for i in 0..N {
        a[i] = coeffs[i] as i32;
    }

    let mut k = N;
    let mut len = 1;

    while len < N {
        let mut start = 0;
        while start < N {
            k -= 1;
            let zeta = -ZETAS[k];

            for j in start..(start + len) {
                let t = a[j];
                a[j] = t + a[j + len];
                a[j + len] = t - a[j + len];
                a[j + len] = montgomery_reduce(zeta as i64 * a[j + len] as i64);
            }
            start += 2 * len;
        }
        len <<= 1;
    }

    // Final scaling by f = mont^2/256 to get proper coefficients
    for i in 0..N {
        a[i] = montgomery_reduce(NTT_F as i64 * a[i] as i64);
    }

    // Convert back to unsigned, reducing to [0, Q)
    for i in 0..N {
        coeffs[i] = caddq(reduce32(a[i])) as u32;
    }
}

// ============================================================================
// Polynomial vector
// ============================================================================

/// A vector of K polynomials.
#[derive(Clone, Debug, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
#[derive(Default)]
pub struct PolyVecK {
    pub polys: [Poly; 4], // K = 4
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
#[derive(Default)]
pub struct PolyVecL {
    pub polys: [Poly; 4], // L = 4
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
        // Note: Parallelism at this level has too much overhead for 4 tasks.
        // Keep serial for now; parallelize at higher levels (batch operations).
        for row in self.rows.iter_mut() {
            row.ntt();
        }
    }

    /// Matrix-vector multiplication: A * s (both in NTT domain).
    /// Both matrix and vector must already be in NTT domain.
    /// Result is also in NTT domain.
    pub fn mul_vec_ntt_domain(&self, s: &PolyVecL) -> PolyVecK {
        let mut result = PolyVecK::zero();
        for (i, row) in self.rows.iter().enumerate() {
            for (j, poly) in row.polys.iter().enumerate() {
                let prod = poly.pointwise_mul(&s.polys[j]);
                result.polys[i].add_assign(&prod);
            }
        }
        result
    }

    /// Matrix-vector multiplication using NTT.
    /// Takes matrix and vector in standard form, returns result in standard form.
    /// This is the fast O(N log N) multiplication method.
    pub fn mul_vec(&self, s: &PolyVecL) -> PolyVecK {
        // Transform matrix and vector to NTT domain
        let mut a_ntt = self.clone();
        let mut s_ntt = s.clone();
        a_ntt.ntt();
        s_ntt.ntt();

        // Multiply in NTT domain
        let mut result = a_ntt.mul_vec_ntt_domain(&s_ntt);

        // Transform result back to standard form
        result.inv_ntt();

        result
    }

    /// Matrix-vector multiplication using naive polynomial multiplication.
    /// This is slower O(N²) but uses no NTT domain transformations.
    pub fn mul_vec_naive(&self, s: &PolyVecL) -> PolyVecK {
        let mut result = PolyVecK::zero();

        for (i, row) in self.rows.iter().enumerate() {
            for (j, poly) in row.polys.iter().enumerate() {
                let prod = poly.naive_mul(&s.polys[j]);
                result.polys[i].add_assign(&prod);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::SeedableRng;
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
    fn test_ntt_roundtrip_via_multiplication() {
        // Note: The Dilithium-style NTT doesn't support direct roundtrip
        // (ntt followed by inv_ntt) because of Montgomery domain handling.
        // The Montgomery factors only cancel properly during multiplication.
        // So we test roundtrip by multiplying by 1.
        let mut rng = ChaCha20Rng::seed_from_u64(123);
        let original = Poly::sample_uniform(&mut rng);

        // Multiply by 1 should return the same polynomial
        let one = Poly::constant(1);
        let result = original.ntt_mul(&one);

        for i in 0..N {
            assert_eq!(result.coeffs[i], original.coeffs[i]);
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

    #[test]
    fn test_matrix_mul_vec_matches_naive() {
        // NTT-based matrix-vector multiplication must match naive version
        let seed = [42u8; 32];
        let a = PolyMatrix::expand_a(&seed);

        let mut rng = ChaCha20Rng::seed_from_u64(123);
        let s = PolyVecL::sample_small(&mut rng, 2);

        let ntt_result = a.mul_vec(&s);
        let naive_result = a.mul_vec_naive(&s);

        for i in 0..4 {
            for j in 0..N {
                assert_eq!(
                    ntt_result.polys[i].coeffs[j],
                    naive_result.polys[i].coeffs[j],
                    "NTT and naive matrix mul differ at poly {} coeff {}",
                    i, j
                );
            }
        }
    }

    #[test]
    fn test_ntt_mul_matches_naive() {
        // This is the critical correctness test: NTT multiplication
        // must produce the same result as naive multiplication.

        // Test with multiple random polynomial pairs
        for seed in 0..10u64 {
            let mut rng = ChaCha20Rng::seed_from_u64(seed);
            let a = Poly::sample_uniform(&mut rng);
            let b = Poly::sample_uniform(&mut rng);

            let naive_result = a.naive_mul(&b);
            let ntt_result = a.ntt_mul(&b);

            for i in 0..N {
                assert_eq!(
                    ntt_result.coeffs[i], naive_result.coeffs[i],
                    "NTT and naive multiplication differ at coefficient {} (seed {})",
                    i, seed
                );
            }
        }
    }

    #[test]
    fn test_ntt_mul_with_small_polys() {
        // Test with small coefficient polynomials (typical for signatures)
        let mut rng = ChaCha20Rng::seed_from_u64(42);

        for _ in 0..5 {
            let a = Poly::sample_small(&mut rng, 2);
            let b = Poly::sample_small(&mut rng, 2);

            let naive_result = a.naive_mul(&b);
            let ntt_result = a.ntt_mul(&b);

            assert_eq!(ntt_result, naive_result);
        }
    }

    #[test]
    fn test_ntt_mul_identity() {
        // Multiplying by 1 (constant polynomial) should return the same polynomial
        let mut rng = ChaCha20Rng::seed_from_u64(123);
        let a = Poly::sample_uniform(&mut rng);
        let one = Poly::constant(1);

        let result = a.ntt_mul(&one);

        for i in 0..N {
            assert_eq!(result.coeffs[i], a.coeffs[i]);
        }
    }

    #[test]
    fn test_ntt_mul_zero() {
        // Multiplying by 0 should return zero
        let mut rng = ChaCha20Rng::seed_from_u64(456);
        let a = Poly::sample_uniform(&mut rng);
        let zero = Poly::zero();

        let result = a.ntt_mul(&zero);

        for i in 0..N {
            assert_eq!(result.coeffs[i], 0);
        }
    }

    #[test]
    fn test_ntt_mul_commutativity() {
        // a * b == b * a
        let mut rng = ChaCha20Rng::seed_from_u64(789);
        let a = Poly::sample_uniform(&mut rng);
        let b = Poly::sample_uniform(&mut rng);

        let ab = a.ntt_mul(&b);
        let ba = b.ntt_mul(&a);

        assert_eq!(ab, ba);
    }
}
