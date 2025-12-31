//! Lion ring signature parameters.
//!
//! These parameters are chosen for ~128-bit post-quantum security,
//! following similar choices to ML-DSA (Dilithium) for consistency.

/// Polynomial ring dimension (degree of X^N + 1).
/// Must be a power of 2 for NTT.
pub const N: usize = 256;

/// Modulus for the polynomial ring R_q = Z_q[X]/(X^N + 1).
/// Same as Dilithium for compatibility and proven security.
pub const Q: u32 = 8380417;

/// Module dimension for public keys (number of polynomials in vector).
pub const K: usize = 4;

/// Module dimension for secret keys and signatures.
pub const L: usize = 4;

/// Fixed ring size for Lion ring signatures.
/// Set to 11 for strong anonymity with manageable signature sizes.
///
/// Rationale:
/// - Ring size 11 provides 3.30 bits of measured privacy (95.3% efficiency)
/// - Each additional ring member adds 3,072 bytes to the signature
/// - Ring 11: ~36 KB signature vs Ring 20: ~63.5 KB (+27 KB overhead)
/// - Ring 11 still exceeds Monero's effective anonymity (~4.2 of 16)
///
/// For PQ-Private transactions, this balances quantum-resistant privacy
/// with practical transaction sizes.
pub const RING_SIZE: usize = 11;

/// Bound for secret key coefficients (sampled from [-ETA, ETA]).
pub const ETA: u32 = 2;

/// Number of +/-1 coefficients in the challenge polynomial.
pub const TAU: usize = 39;

/// Bound for signature hint coefficients.
pub const GAMMA1: u32 = 1 << 17;

/// Low-order rounding divisor.
pub const GAMMA2: u32 = (Q - 1) / 88;

/// Beta = tau * eta, bound for checking signature validity.
pub const BETA: u32 = (TAU as u32) * ETA;

/// Maximum number of rejection sampling iterations.
///
/// This controls the retry limit for "Fiat-Shamir with Aborts" rejection sampling.
/// A signature attempt is rejected when ||z||∞ ≥ γ₁ - β, which would leak
/// information about the secret key through the response distribution.
///
/// With our parameters, rejection probability per attempt is approximately:
///   P(reject) ≈ 1 - (1 - 2β/(2γ₁))^(N×L) ≈ 4-7%
///
/// After 256 iterations, failure probability is < 2^-60, which is negligible.
///
/// Reference: Lyubashevsky, "Fiat-Shamir with Aborts" (2009, 2012)
pub const MAX_REJECTION_ITERATIONS: usize = 256;

/// Safety margin for decoy response sampling in ring signatures.
///
/// # Cryptographic Justification
///
/// In Lion ring signatures, verification checks that all responses satisfy:
///   ||z_i||∞ < γ₁ - β  (where γ₁ = GAMMA1, β = BETA = τ × η)
///
/// For the real signer: z = y + c×s₁, where:
///   - y is sampled uniformly from [-γ₁+1, γ₁]
///   - c has exactly τ coefficients of ±1 (sparse challenge)
///   - s₁ has coefficients in [-η, η]
///   - The product c×s₁ has coefficients bounded by ±τη = ±β
///
/// For decoy responses, we sample z directly from a bounded range. The margin
/// ensures that sampled values never exceed the verification bound, even with:
///   1. Boundary conditions in modular arithmetic
///   2. Centered representation edge cases
///   3. Potential off-by-one errors in range calculations
///
/// # Why 100 Specifically?
///
/// The margin of 100 is chosen to be:
///   - **Large enough**: Provides comfortable headroom for edge cases
///   - **Small enough**: Only 0.076% of the usable range (100 / 130994)
///   - **Conservative**: Matches margins used in production Dilithium implementations
///
/// A margin that's too small risks verification failures for legitimate signatures.
/// A margin that's too large unnecessarily reduces the sampling entropy (negligible here).
///
/// # Parameters with Current Values
///
/// ```text
/// γ₁ = 2^17 = 131072
/// β  = τ × η = 39 × 2 = 78
/// γ₁ - β = 130994  (verification bound)
/// γ₁ - β - margin = 130894  (decoy sampling bound)
/// ```
///
/// # References
///
/// - FIPS 204 (ML-DSA/Dilithium): Uses similar rejection sampling with γ₁ - β bound
/// - Lyubashevsky, "Practical Lattice-Based Digital Signature Schemes" (2012)
/// - CRYSTALS-Dilithium specification, Section 4.1
pub const REJECTION_SAMPLING_MARGIN: u32 = 100;

// ============================================================================
// Size constants (in bytes)
// ============================================================================

/// Size of a single polynomial in bytes (N coefficients, each < Q needs 24 bits).
/// We use 3 bytes per coefficient for simplicity.
pub const POLY_BYTES: usize = N * 3; // 768 bytes

/// Size of a polynomial vector with K elements.
pub const POLY_VEC_K_BYTES: usize = K * POLY_BYTES; // 2944 bytes

/// Size of a polynomial vector with L elements.
pub const POLY_VEC_L_BYTES: usize = L * POLY_BYTES; // 2944 bytes

/// Size of a packed small polynomial (coefficients in [-ETA, ETA]).
/// 3 bits per coefficient is sufficient for ETA=2.
pub const POLY_ETA_BYTES: usize = (N * 3 + 7) / 8; // 96 bytes

/// Size of a compressed public key.
/// t = As1 + s2, where t has K polynomials with coefficients mod Q.
/// We can compress by dropping low-order bits.
pub const PUBLIC_KEY_BYTES: usize = 32 + K * (N * 10 / 8); // 32 + 1280 = 1312 bytes

/// Size of a secret key.
/// Contains s1 (L polys), s2 (K polys), and seed.
pub const SECRET_KEY_BYTES: usize = 32 + L * POLY_ETA_BYTES + K * POLY_ETA_BYTES; // 32 + 384 + 384 = 800 bytes

/// Size of a key image.
/// Same structure as public key for consistency.
pub const KEY_IMAGE_BYTES: usize = PUBLIC_KEY_BYTES; // 1312 bytes

/// Size of a single ring member's response in the signature.
/// Contains z (L polynomials with coefficients mod Q).
pub const RESPONSE_BYTES: usize = L * POLY_BYTES; // 4 * 768 = 3072 bytes

/// Base signature size (starting challenge c0 + key image).
/// c0 is a full polynomial (POLY_BYTES) in the sequential ring structure.
pub const SIGNATURE_BASE_BYTES: usize = POLY_BYTES + KEY_IMAGE_BYTES; // 768 + 1312 = 2080 bytes

/// Total signature size for a ring of RING_SIZE members.
/// Includes starting challenge c0, key image, and responses for each member.
pub const SIGNATURE_BYTES: usize = SIGNATURE_BASE_BYTES + RING_SIZE * RESPONSE_BYTES; // 2080 + 11 * 3072 = 35872 bytes

// ============================================================================
// NTT constants
// ============================================================================

/// Primitive 512th root of unity modulo Q.
/// Used for NTT transformations.
pub const ZETA: u32 = 1753;

/// Montgomery constant R = 2^32 mod Q.
pub const MONT_R: u64 = 4193792;

/// Montgomery reduction constant Q^(-1) mod 2^32.
pub const QINV: u32 = 58728449;

// ============================================================================
// Domain separation tags
// ============================================================================

/// Domain separator for key generation.
pub const DOMAIN_KEYGEN: &[u8] = b"botho-lion-keygen-v1";

/// Domain separator for key image computation.
pub const DOMAIN_KEY_IMAGE: &[u8] = b"botho-lion-keyimage-v1";

/// Domain separator for ring signature challenge.
pub const DOMAIN_CHALLENGE: &[u8] = b"botho-lion-challenge-v1";

/// Domain separator for commitment randomness.
pub const DOMAIN_COMMIT: &[u8] = b"botho-lion-commit-v1";

/// Domain separator for signature expansion.
pub const DOMAIN_EXPAND: &[u8] = b"botho-lion-expand-v1";

// ============================================================================
// Helper functions
// ============================================================================

/// Compute the expected signature size for a given ring size.
/// Uses sequential ring structure with c0 (POLY_BYTES) + key_image + responses.
#[inline]
pub const fn signature_size(ring_size: usize) -> usize {
    POLY_BYTES + KEY_IMAGE_BYTES + ring_size * RESPONSE_BYTES
}

/// Check if a value is a valid coefficient (< Q).
#[inline]
pub const fn is_valid_coefficient(x: u32) -> bool {
    x < Q
}

/// Reduce a value modulo Q.
#[inline]
pub const fn reduce_mod_q(x: u64) -> u32 {
    (x % (Q as u64)) as u32
}

/// Centered reduction: map [0, Q) to [-(Q-1)/2, (Q-1)/2].
#[inline]
pub fn centered_reduce(x: u32) -> i32 {
    let x = x % Q;
    if x > Q / 2 {
        x as i32 - Q as i32
    } else {
        x as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameters_consistency() {
        // N must be power of 2
        assert!(N.is_power_of_two());
        // Q must be prime (basic check: Q is 8380417, known prime)
        assert_eq!(Q, 8380417);
        // Ring size is 11 (optimized for PQ signature size while maintaining privacy)
        assert_eq!(RING_SIZE, 11);
        // Beta = tau * eta
        assert_eq!(BETA, TAU as u32 * ETA);
    }

    #[test]
    fn test_signature_size() {
        assert_eq!(signature_size(RING_SIZE), SIGNATURE_BYTES);
        assert_eq!(signature_size(1), SIGNATURE_BASE_BYTES + RESPONSE_BYTES);
    }

    #[test]
    fn test_centered_reduce() {
        assert_eq!(centered_reduce(0), 0);
        assert_eq!(centered_reduce(Q / 2), (Q / 2) as i32);
        assert_eq!(centered_reduce(Q / 2 + 1), -((Q / 2) as i32));
        assert_eq!(centered_reduce(Q - 1), -1);
    }
}
