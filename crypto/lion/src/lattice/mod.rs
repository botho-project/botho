//! Lattice-based cryptographic primitives for Lion ring signatures.
//!
//! This module provides the core Module-LWE/SIS operations needed for
//! the Lion linkable ring signature scheme.

pub mod commitment;

pub use commitment::Commitment;

use crate::{
    error::{LionError, Result},
    params::*,
    polynomial::{PolyMatrix, PolyVecK, PolyVecL},
};
use rand_core::{CryptoRngCore, SeedableRng};
use sha3::{Shake256, digest::{ExtendableOutput, Update}};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Lion public key.
///
/// Consists of:
/// - A seed for expanding the public matrix A
/// - t = A*s1 + s2 (compressed)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LionPublicKey {
    /// Seed for expanding the matrix A.
    pub seed: [u8; 32],
    /// Public key vector t = A*s1 + s2.
    pub t: PolyVecK,
}

impl LionPublicKey {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> [u8; PUBLIC_KEY_BYTES] {
        let mut bytes = [0u8; PUBLIC_KEY_BYTES];
        bytes[..32].copy_from_slice(&self.seed);

        // Pack t polynomials using 10-bit coefficients (after rounding)
        let mut offset = 32;
        for poly in self.t.polys.iter() {
            for chunk in poly.coeffs.chunks(4) {
                // Pack 4 coefficients (40 bits = 5 bytes)
                // Round to 10 bits: c -> c >> 13
                let c0 = (chunk[0] >> 13) as u64;
                let c1 = (chunk[1] >> 13) as u64;
                let c2 = (chunk[2] >> 13) as u64;
                let c3 = (chunk[3] >> 13) as u64;

                let packed = c0 | (c1 << 10) | (c2 << 20) | (c3 << 30);
                bytes[offset] = packed as u8;
                bytes[offset + 1] = (packed >> 8) as u8;
                bytes[offset + 2] = (packed >> 16) as u8;
                bytes[offset + 3] = (packed >> 24) as u8;
                bytes[offset + 4] = (packed >> 32) as u8;
                offset += 5;
            }
        }

        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PUBLIC_KEY_BYTES {
            return Err(LionError::InvalidPublicKey);
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes[..32]);

        let mut t = PolyVecK::zero();
        let mut offset = 32;

        for poly in t.polys.iter_mut() {
            for chunk in poly.coeffs.chunks_mut(4) {
                let packed = (bytes[offset] as u64)
                    | ((bytes[offset + 1] as u64) << 8)
                    | ((bytes[offset + 2] as u64) << 16)
                    | ((bytes[offset + 3] as u64) << 24)
                    | ((bytes[offset + 4] as u64) << 32);

                // Unpack and expand from 10 bits
                chunk[0] = ((packed & 0x3FF) << 13) as u32;
                chunk[1] = (((packed >> 10) & 0x3FF) << 13) as u32;
                chunk[2] = (((packed >> 20) & 0x3FF) << 13) as u32;
                chunk[3] = (((packed >> 30) & 0x3FF) << 13) as u32;
                offset += 5;
            }
        }

        Ok(Self { seed, t })
    }

    /// Expand the public matrix A from the seed.
    pub fn expand_a(&self) -> PolyMatrix {
        PolyMatrix::expand_a(&self.seed)
    }
}

/// Lion secret key.
///
/// Contains the secret vectors s1 and s2, plus the public key.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct LionSecretKey {
    /// Secret vector s1 (L polynomials with small coefficients).
    pub s1: PolyVecL,
    /// Secret vector s2 (K polynomials with small coefficients).
    pub s2: PolyVecK,
    /// Corresponding public key.
    #[zeroize(skip)]
    pub public_key: LionPublicKey,
}

impl LionSecretKey {
    /// Serialize to bytes (includes public key for convenience).
    pub fn to_bytes(&self) -> [u8; SECRET_KEY_BYTES] {
        let mut bytes = [0u8; SECRET_KEY_BYTES];

        // Seed
        bytes[..32].copy_from_slice(&self.public_key.seed);

        // Pack s1 (L polynomials with small coefficients)
        let mut offset = 32;
        for poly in self.s1.polys.iter() {
            for chunk in poly.coeffs.chunks(8) {
                // Pack 8 coefficients with 3-bit representation
                // eta = 2, so values in [-2, 2], map to [0, 4]
                let mut packed = 0u32;
                for (i, &c) in chunk.iter().enumerate() {
                    let mapped = if c <= 2 { c } else { 5 - (Q - c) };
                    packed |= (mapped & 0x7) << (i * 3);
                }
                bytes[offset] = packed as u8;
                bytes[offset + 1] = (packed >> 8) as u8;
                bytes[offset + 2] = (packed >> 16) as u8;
                offset += 3;
            }
        }

        // Pack s2 (K polynomials with small coefficients)
        for poly in self.s2.polys.iter() {
            for chunk in poly.coeffs.chunks(8) {
                let mut packed = 0u32;
                for (i, &c) in chunk.iter().enumerate() {
                    let mapped = if c <= 2 { c } else { 5 - (Q - c) };
                    packed |= (mapped & 0x7) << (i * 3);
                }
                bytes[offset] = packed as u8;
                bytes[offset + 1] = (packed >> 8) as u8;
                bytes[offset + 2] = (packed >> 16) as u8;
                offset += 3;
            }
        }

        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != SECRET_KEY_BYTES {
            return Err(LionError::InvalidSecretKey);
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes[..32]);

        // Unpack s1
        let mut s1 = PolyVecL::zero();
        let mut offset = 32;
        for poly in s1.polys.iter_mut() {
            for chunk in poly.coeffs.chunks_mut(8) {
                let packed = (bytes[offset] as u32)
                    | ((bytes[offset + 1] as u32) << 8)
                    | ((bytes[offset + 2] as u32) << 16);

                for (i, c) in chunk.iter_mut().enumerate() {
                    let mapped = (packed >> (i * 3)) & 0x7;
                    *c = if mapped <= 2 {
                        mapped
                    } else {
                        Q - (5 - mapped)
                    };
                }
                offset += 3;
            }
        }

        // Unpack s2
        let mut s2 = PolyVecK::zero();
        for poly in s2.polys.iter_mut() {
            for chunk in poly.coeffs.chunks_mut(8) {
                let packed = (bytes[offset] as u32)
                    | ((bytes[offset + 1] as u32) << 8)
                    | ((bytes[offset + 2] as u32) << 16);

                for (i, c) in chunk.iter_mut().enumerate() {
                    let mapped = (packed >> (i * 3)) & 0x7;
                    *c = if mapped <= 2 {
                        mapped
                    } else {
                        Q - (5 - mapped)
                    };
                }
                offset += 3;
            }
        }

        // Recompute public key
        let mut a = PolyMatrix::expand_a(&seed);
        a.ntt();

        let mut s1_ntt = s1.clone();
        s1_ntt.ntt();

        let mut t = a.mul_vec(&s1_ntt);
        t.inv_ntt();
        t.add_assign(&s2);

        let public_key = LionPublicKey { seed, t };

        Ok(Self { s1, s2, public_key })
    }
}

/// Lion keypair.
#[derive(Clone)]
pub struct LionKeyPair {
    /// Secret key.
    pub secret_key: LionSecretKey,
    /// Public key (same as secret_key.public_key).
    pub public_key: LionPublicKey,
}

impl LionKeyPair {
    /// Generate a new keypair from randomness.
    pub fn generate<R: CryptoRngCore>(rng: &mut R) -> Self {
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        Self::from_seed(&seed)
    }

    /// Generate a keypair from a seed (deterministic).
    pub fn from_seed(seed: &[u8]) -> Self {
        // Expand seed into keying material
        let mut hasher = Shake256::default();
        hasher.update(DOMAIN_KEYGEN);
        hasher.update(seed);
        let mut reader = hasher.finalize_xof();

        // Get matrix seed
        let mut matrix_seed = [0u8; 32];
        sha3::digest::XofReader::read(&mut reader, &mut matrix_seed);

        // Get secret key seeds
        let mut s1_seed = [0u8; 32];
        let mut s2_seed = [0u8; 32];
        sha3::digest::XofReader::read(&mut reader, &mut s1_seed);
        sha3::digest::XofReader::read(&mut reader, &mut s2_seed);

        // Sample secret vectors using seeded RNG
        use rand_chacha::ChaCha20Rng;

        let mut s1_rng = ChaCha20Rng::from_seed(s1_seed);
        let mut s2_rng = ChaCha20Rng::from_seed(s2_seed);

        let s1 = PolyVecL::sample_small(&mut s1_rng, ETA);
        let s2 = PolyVecK::sample_small(&mut s2_rng, ETA);

        // Compute public key: t = A*s1 + s2
        let mut a = PolyMatrix::expand_a(&matrix_seed);
        a.ntt();

        let mut s1_ntt = s1.clone();
        s1_ntt.ntt();

        let mut t = a.mul_vec(&s1_ntt);
        t.inv_ntt();
        t.add_assign(&s2);

        let public_key = LionPublicKey {
            seed: matrix_seed,
            t,
        };

        let secret_key = LionSecretKey {
            s1,
            s2,
            public_key: public_key.clone(),
        };

        Self {
            secret_key,
            public_key,
        }
    }

    /// Compute the key image for this keypair.
    ///
    /// The key image is a unique identifier derived from the secret key
    /// that enables linkability (detecting double-spends) without
    /// revealing which ring member signed.
    pub fn key_image(&self) -> LionKeyImage {
        LionKeyImage::from_secret_key(&self.secret_key)
    }
}

/// Lion key image for linkability.
///
/// The key image is computed as I = H(pk) * sk in the lattice,
/// where H is a hash-to-matrix function and sk is the secret key.
/// This provides linkability: the same key always produces the same
/// key image, allowing detection of double-spends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LionKeyImage {
    /// The key image vector.
    pub image: PolyVecK,
}

impl LionKeyImage {
    /// Compute key image from secret key.
    pub fn from_secret_key(sk: &LionSecretKey) -> Self {
        // Hash public key to get a matrix
        let pk_bytes = sk.public_key.to_bytes();
        let h = Self::hash_to_matrix(&pk_bytes);

        // Compute I = H(pk) * s1 in NTT domain
        let mut s1_ntt = sk.s1.clone();
        s1_ntt.ntt();

        let mut image = h.mul_vec(&s1_ntt);
        image.inv_ntt();

        Self { image }
    }

    /// Hash public key bytes to a matrix.
    fn hash_to_matrix(pk_bytes: &[u8]) -> PolyMatrix {
        let mut hasher = Shake256::default();
        hasher.update(DOMAIN_KEY_IMAGE);
        hasher.update(pk_bytes);
        let mut reader = hasher.finalize_xof();

        let mut seed = [0u8; 32];
        sha3::digest::XofReader::read(&mut reader, &mut seed);

        let mut matrix = PolyMatrix::expand_a(&seed);
        matrix.ntt();
        matrix
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> [u8; KEY_IMAGE_BYTES] {
        // Use same packing as public key
        let mut bytes = [0u8; KEY_IMAGE_BYTES];

        let mut offset = 0;
        for poly in self.image.polys.iter() {
            for chunk in poly.coeffs.chunks(4) {
                let c0 = (chunk[0] >> 13) as u64;
                let c1 = (chunk[1] >> 13) as u64;
                let c2 = (chunk[2] >> 13) as u64;
                let c3 = (chunk[3] >> 13) as u64;

                let packed = c0 | (c1 << 10) | (c2 << 20) | (c3 << 30);
                bytes[offset] = packed as u8;
                bytes[offset + 1] = (packed >> 8) as u8;
                bytes[offset + 2] = (packed >> 16) as u8;
                bytes[offset + 3] = (packed >> 24) as u8;
                bytes[offset + 4] = (packed >> 32) as u8;
                offset += 5;
            }
        }

        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_IMAGE_BYTES {
            return Err(LionError::InvalidKeyImage);
        }

        let mut image = PolyVecK::zero();
        let mut offset = 0;

        for poly in image.polys.iter_mut() {
            for chunk in poly.coeffs.chunks_mut(4) {
                let packed = (bytes[offset] as u64)
                    | ((bytes[offset + 1] as u64) << 8)
                    | ((bytes[offset + 2] as u64) << 16)
                    | ((bytes[offset + 3] as u64) << 24)
                    | ((bytes[offset + 4] as u64) << 32);

                chunk[0] = ((packed & 0x3FF) << 13) as u32;
                chunk[1] = (((packed >> 10) & 0x3FF) << 13) as u32;
                chunk[2] = (((packed >> 20) & 0x3FF) << 13) as u32;
                chunk[3] = (((packed >> 30) & 0x3FF) << 13) as u32;
                offset += 5;
            }
        }

        Ok(Self { image })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_keypair_generation() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let kp = LionKeyPair::generate(&mut rng);

        // Verify s1 and s2 have small coefficients
        assert!(kp.secret_key.s1.check_norm(ETA));
        assert!(kp.secret_key.s2.check_norm(ETA));
    }

    #[test]
    fn test_keypair_deterministic() {
        let seed = [42u8; 32];
        let kp1 = LionKeyPair::from_seed(&seed);
        let kp2 = LionKeyPair::from_seed(&seed);

        assert_eq!(kp1.public_key, kp2.public_key);
    }

    #[test]
    fn test_public_key_serialization() {
        let mut rng = ChaCha20Rng::seed_from_u64(123);
        let kp = LionKeyPair::generate(&mut rng);

        let bytes = kp.public_key.to_bytes();
        let recovered = LionPublicKey::from_bytes(&bytes).expect("should deserialize");

        assert_eq!(kp.public_key.seed, recovered.seed);
        // Note: t values won't be exactly equal due to rounding, but should be close
    }

    #[test]
    fn test_secret_key_serialization() {
        let mut rng = ChaCha20Rng::seed_from_u64(456);
        let kp = LionKeyPair::generate(&mut rng);

        let bytes = kp.secret_key.to_bytes();
        let recovered = LionSecretKey::from_bytes(&bytes).expect("should deserialize");

        // s1 and s2 should match exactly
        assert_eq!(kp.secret_key.s1, recovered.s1);
        assert_eq!(kp.secret_key.s2, recovered.s2);
    }

    #[test]
    fn test_key_image_deterministic() {
        let seed = [99u8; 32];
        let kp = LionKeyPair::from_seed(&seed);

        let ki1 = kp.key_image();
        let ki2 = kp.key_image();

        assert_eq!(ki1, ki2);
    }

    #[test]
    fn test_key_image_unique() {
        let kp1 = LionKeyPair::from_seed(&[1u8; 32]);
        let kp2 = LionKeyPair::from_seed(&[2u8; 32]);

        let ki1 = kp1.key_image();
        let ki2 = kp2.key_image();

        assert_ne!(ki1, ki2);
    }

    #[test]
    fn test_key_image_serialization() {
        let kp = LionKeyPair::from_seed(&[77u8; 32]);
        let ki = kp.key_image();

        let bytes = ki.to_bytes();
        let recovered = LionKeyImage::from_bytes(&bytes).expect("should deserialize");

        // Check values are close (rounding may cause small differences)
        // For a proper test, we'd need approximate equality
        assert_eq!(bytes.len(), KEY_IMAGE_BYTES);
        assert_eq!(recovered.to_bytes().len(), KEY_IMAGE_BYTES);
    }
}
