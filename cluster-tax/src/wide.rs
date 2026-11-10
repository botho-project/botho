//! Exact 256-bit intermediates for value-weighted means (#1876).
//!
//! The consensus fee floor takes value-weighted means of cumulative cluster
//! wealth: the ring floor (since #1686 the amount-matched per-input max,
//! [`crate::amount_matched_ring_floor_wealth`]: per input, the largest wealth
//! among ring members whose amount equals its pseudo-output amount, then the
//! mean of those maxima weighted by pseudo-output amount), each ring member's
//! tag-weighted wealth, and the output-tag wealth behind the claimed factor
//! (`botho`'s `effective_cluster_wealth_from_outputs`). The pre-#1686 ring
//! centroid ([`crate::ring_centroid_implied_factor`], kept for the
//! simulations) uses it too. Each one multiplies a u64 value
//! (or a u32 tag weight) by a full-u128 wealth, so a product, and the sum of
//! products, can exceed `u128::MAX`.
//!
//! Before protocol 9.0.0 those sums saturated at `u128::MAX` and were then
//! divided by an unsaturated denominator. That does not pin the result to the
//! maximum: it LOWERS it, so a larger ring got a lower fee floor (#1876).
//! This module keeps the numerator exact in 256 bits and returns
//! `floor(Σ a_i·b_i / d)`, clamped only at the end.
//!
//! # Policy
//!
//! - **Exact.** Whenever the true result fits in u128 it is returned exactly,
//!   so every input on which the old u128 arithmetic did not overflow gives the
//!   same result as before.
//! - **Final clamp.** A true result above `u128::MAX` is returned as
//!   `u128::MAX`. A weighted mean never exceeds its largest term, so for the
//!   ring centroid, and for well-formed tag vectors (weights summing to at most
//!   `TAG_WEIGHT_SCALE`), this cannot happen; it is reachable only through
//!   malformed weights. Clamping the final value is conservative: `u128::MAX`
//!   maps to the maximum cluster factor.
//! - **No reachable intermediate saturation.** The 256-bit numerator would need
//!   more than 2^64 products of a u64 and a u128 (or 2^32 of two u128s at their
//!   maxima) to overflow, and the u128 denominator more than 2^64 u64 addends;
//!   neither is possible for an in-memory slice. Both saturate rather than
//!   wrap, so even then the code is deterministic and panic-free.
//!
//! # Determinism
//!
//! CONSENSUS-CRITICAL: pure integer arithmetic on fixed-width limbs, no
//! floating point, no allocation, no panics (every division has a nonzero
//! divisor by construction).

/// An unsigned 256-bit integer as four little-endian u64 limbs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct U256([u64; 4]);

impl U256 {
    /// Zero.
    pub const ZERO: U256 = U256([0; 4]);
    /// 2^256 - 1.
    pub const MAX: U256 = U256([u64::MAX; 4]);

    /// Widen a u128.
    pub const fn from_u128(x: u128) -> U256 {
        U256([x as u64, (x >> 64) as u64, 0, 0])
    }

    /// The exact product of two u128s (always fits in 256 bits).
    pub const fn mul_u128(a: u128, b: u128) -> U256 {
        let (a0, a1) = (a as u64 as u128, (a >> 64) as u64 as u128);
        let (b0, b1) = (b as u64 as u128, (b >> 64) as u64 as u128);
        // Each partial product is < 2^128.
        let p00 = a0 * b0;
        let p01 = a0 * b1;
        let p10 = a1 * b0;
        let p11 = a1 * b1;

        let l0 = p00 as u64;
        // Column 1: high half of p00 plus low halves of p01 and p10 (< 3·2^64).
        let c1 = (p00 >> 64) + (p01 as u64 as u128) + (p10 as u64 as u128);
        let l1 = c1 as u64;
        // Column 2: carry, high halves of p01 and p10, low half of p11.
        let c2 = (c1 >> 64) + (p01 >> 64) + (p10 >> 64) + (p11 as u64 as u128);
        let l2 = c2 as u64;
        // Column 3: the product is < 2^256, so this cannot carry out.
        let l3 = ((c2 >> 64) + (p11 >> 64)) as u64;
        U256([l0, l1, l2, l3])
    }

    /// `self + other`, or `None` on overflow past 2^256 - 1.
    pub const fn checked_add(self, other: U256) -> Option<U256> {
        let mut out = [0u64; 4];
        let mut carry = 0u128;
        let mut i = 0;
        while i < 4 {
            let s = self.0[i] as u128 + other.0[i] as u128 + carry;
            out[i] = s as u64;
            carry = s >> 64;
            i += 1;
        }
        if carry == 0 {
            Some(U256(out))
        } else {
            None
        }
    }

    /// `self + other`, saturating at 2^256 - 1.
    pub const fn saturating_add(self, other: U256) -> U256 {
        match self.checked_add(other) {
            Some(s) => s,
            None => U256::MAX,
        }
    }

    /// The value as u128, or `u128::MAX` if it does not fit.
    pub const fn to_u128_saturating(self) -> u128 {
        if self.0[2] != 0 || self.0[3] != 0 {
            u128::MAX
        } else {
            (self.0[0] as u128) | ((self.0[1] as u128) << 64)
        }
    }

    /// `floor(self / d)`, or `None` if `d == 0`.
    pub const fn checked_div_u128(self, d: u128) -> Option<U256> {
        if d == 0 {
            return None;
        }
        let mut q = [0u64; 4];
        if d <= u64::MAX as u128 {
            // Short division by one limb: the running remainder is < d < 2^64,
            // so `(r << 64) | limb` fits in u128.
            let mut r: u128 = 0;
            let mut i = 4;
            while i > 0 {
                i -= 1;
                let cur = (r << 64) | self.0[i] as u128;
                q[i] = (cur / d) as u64;
                r = cur % d;
            }
        } else {
            // Restoring binary long division with a u128 remainder r < d.
            // Shifting r left by one can carry out of bit 127; the true value
            // `2r + bit` is then at least 2^128 > d, and `2r + bit - d < d`,
            // so the wrapping subtraction yields the exact new remainder.
            let mut r: u128 = 0;
            let mut bit = 256;
            while bit > 0 {
                bit -= 1;
                let limb = bit / 64;
                let shift = bit % 64;
                let incoming = ((self.0[limb] >> shift) & 1) as u128;
                let carried_out = r >> 127 == 1;
                r = (r << 1) | incoming;
                if carried_out || r >= d {
                    r = r.wrapping_sub(d);
                    q[limb] |= 1u64 << shift;
                }
            }
        }
        Some(U256(q))
    }
}

/// `floor(a · b / d)` computed exactly, clamped at `u128::MAX`; `0` if
/// `d == 0`.
pub const fn mul_div_floor(a: u128, b: u128, d: u128) -> u128 {
    match U256::mul_u128(a, b).checked_div_u128(d) {
        Some(q) => q.to_u128_saturating(),
        None => 0,
    }
}

/// An exact accumulator for `floor(Σ a_i·b_i / Σ w_j)`.
///
/// Products go into a 256-bit numerator ([`Self::add_product`]); weights go
/// into the denominator ([`Self::add_weight`]). The two are separate because
/// `effective_cluster_wealth_from_outputs` weights tag wealth by a tag's value
/// fraction but divides by the outputs' total value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeightedMean {
    numerator: U256,
    denominator: u128,
}

impl WeightedMean {
    /// An empty accumulator (mean 0).
    pub const fn new() -> Self {
        WeightedMean {
            numerator: U256::ZERO,
            denominator: 0,
        }
    }

    /// Add the exact product `a · b` to the numerator.
    pub fn add_product(&mut self, a: u128, b: u128) {
        self.numerator = self.numerator.saturating_add(U256::mul_u128(a, b));
    }

    /// Add `w` to the denominator.
    pub fn add_weight(&mut self, w: u128) {
        self.denominator = self.denominator.saturating_add(w);
    }

    /// `floor(numerator / denominator)`, clamped at `u128::MAX`; `0` when the
    /// denominator is zero.
    pub fn floor(&self) -> u128 {
        match self.numerator.checked_div_u128(self.denominator) {
            Some(q) => q.to_u128_saturating(),
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference: a u128 pair (hi, lo) interpreted as `hi·2^128 + lo` only
    /// for values that fit, checked through u128 where possible.
    fn small(x: U256) -> Option<u128> {
        if x.0[2] == 0 && x.0[3] == 0 {
            Some(x.to_u128_saturating())
        } else {
            None
        }
    }

    #[test]
    fn mul_matches_u128_when_it_fits() {
        let cases: [(u128, u128); 6] = [
            (0, 0),
            (1, u128::MAX),
            (u64::MAX as u128, u64::MAX as u128),
            (123_456_789, 987_654_321_000_000_000_000),
            (1 << 63, 1 << 64),
            (3, u128::MAX / 3),
        ];
        for (a, b) in cases {
            assert_eq!(small(U256::mul_u128(a, b)), a.checked_mul(b), "{a} * {b}");
        }
    }

    #[test]
    fn mul_max_by_max() {
        // (2^128 - 1)^2 = 2^256 - 2^129 + 1.
        let p = U256::mul_u128(u128::MAX, u128::MAX);
        assert_eq!(p, U256([1, 0, u64::MAX - 1, u64::MAX]));
    }

    #[test]
    fn mul_then_div_round_trips() {
        let values = [
            1u128,
            2,
            u64::MAX as u128,
            u64::MAX as u128 + 1,
            1_000_000,
            100_000_000_000_000_000_000,
            u128::MAX / 7,
            u128::MAX,
        ];
        for &a in &values {
            for &b in &values {
                let p = U256::mul_u128(a, b);
                assert_eq!(
                    p.checked_div_u128(a).and_then(small),
                    Some(b),
                    "{a}*{b}/{a}"
                );
                assert_eq!(
                    p.checked_div_u128(b).and_then(small),
                    Some(a),
                    "{a}*{b}/{b}"
                );
            }
        }
    }

    #[test]
    fn div_matches_u128_when_it_fits() {
        let nums = [
            0u128,
            1,
            999,
            u64::MAX as u128,
            1 << 100,
            u128::MAX,
            u128::MAX - 12345,
        ];
        let dens = [
            1u128,
            3,
            1_000_000,
            u64::MAX as u128,
            u64::MAX as u128 + 1,
            1 << 127,
            u128::MAX,
        ];
        for &n in &nums {
            for &d in &dens {
                let q = U256::from_u128(n).checked_div_u128(d).and_then(small);
                assert_eq!(q, Some(n / d), "{n} / {d}");
            }
        }
        assert_eq!(U256::from_u128(5).checked_div_u128(0), None);
    }

    #[test]
    fn mul_div_floor_is_exact_and_clamps() {
        // floor(u128::MAX * 3 / 4) = floor((2^128 - 1) * 3 / 4) = 3·2^126 - 1.
        assert_eq!(mul_div_floor(u128::MAX, 3, 4), 3 * (1u128 << 126) - 1);
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, u128::MAX), u128::MAX);
        assert_eq!(
            mul_div_floor(u128::MAX, 2, 1),
            u128::MAX,
            "clamped at the end"
        );
        assert_eq!(mul_div_floor(7, 9, 0), 0);
        assert_eq!(
            mul_div_floor(999_999, 1u128 << 120, 1_000_000),
            ((1u128 << 120) / 1_000_000) * 999_999
                + ((1u128 << 120) % 1_000_000) * 999_999 / 1_000_000
        );
    }

    #[test]
    fn weighted_mean_of_equal_terms_is_the_term() {
        // The #1876 homogeneous ring: 20 × (1e18, 1e20). Σ v·w = 2e39 > u128::MAX.
        let mut m = WeightedMean::new();
        for _ in 0..20 {
            m.add_product(1_000_000_000_000_000_000, 100_000_000_000_000_000_000);
            m.add_weight(1_000_000_000_000_000_000);
        }
        assert_eq!(m.floor(), 100_000_000_000_000_000_000);
        assert_eq!(WeightedMean::new().floor(), 0);
    }
}
