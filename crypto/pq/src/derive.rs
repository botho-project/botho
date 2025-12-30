//! Key Derivation for Post-Quantum Keys
//!
//! This module provides deterministic derivation of ML-KEM and ML-DSA keypairs
//! from a BIP39 seed. This ensures that:
//!
//! 1. Users don't need to backup additional key material
//! 2. PQ keys can be regenerated from the same mnemonic
//! 3. Key derivation is deterministic and reproducible
//! 4. Full BIP39 key stretching is applied (PBKDF2 with 2048 iterations)
//!
//! # Derivation Path
//!
//! ```text
//! mnemonic + passphrase
//!    │
//!    └── PBKDF2-HMAC-SHA512 (2048 iterations) ──► 512-bit BIP39 seed
//!                                                      │
//!          ┌────────────────────────────────────────────┘
//!          │
//!          ├── HKDF(salt="botho-pq-v1", info="kem-seed") ──► ML-KEM-768 keypair
//!          │
//!          └── HKDF(salt="botho-pq-v1", info="sig-seed") ──► ML-DSA-65 keypair
//! ```
//!
//! # Security
//!
//! Using the BIP39 seed (rather than raw mnemonic bytes) provides:
//! - Key stretching via PBKDF2 (2048 iterations)
//! - Optional passphrase support for additional security
//! - Consistent entropy extraction regardless of mnemonic encoding

use crate::{MlDsa65KeyPair, MlKem768KeyPair, BOTHO_PQ_DOMAIN};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Size of a BIP39 seed in bytes (512 bits)
pub const BIP39_SEED_SIZE: usize = 64;

/// Seeds for PQ key generation, derived from BIP39 seed
#[derive(Zeroize, ZeroizeOnDrop)]
struct DerivedSeeds {
    kem_seed: [u8; 32],
    sig_seed: [u8; 32],
}

/// Post-quantum key material derived from a BIP39 seed
pub struct PqKeyMaterial {
    /// ML-KEM-768 keypair for key encapsulation (stealth addresses)
    pub kem_keypair: MlKem768KeyPair,

    /// ML-DSA-65 keypair for signatures (transaction signing)
    pub sig_keypair: MlDsa65KeyPair,
}

impl PqKeyMaterial {
    /// Get the ML-KEM public key bytes for inclusion in addresses
    pub fn kem_public_key_bytes(&self) -> &[u8; 1184] {
        self.kem_keypair.public_key().as_bytes()
    }

    /// Get the ML-DSA public key bytes for inclusion in addresses
    pub fn sig_public_key_bytes(&self) -> &[u8; 1952] {
        self.sig_keypair.public_key().as_bytes()
    }
}

/// Derive post-quantum keypairs from a BIP39 seed
///
/// This function takes a 64-byte BIP39 seed (the output of PBKDF2-HMAC-SHA512
/// on the mnemonic + passphrase) and derives both ML-KEM and ML-DSA keypairs
/// deterministically using HKDF.
///
/// # Arguments
///
/// * `seed` - A 64-byte BIP39 seed (from `bip39::Seed::new(&mnemonic, passphrase)`)
///
/// # Returns
///
/// A `PqKeyMaterial` struct containing both keypairs
///
/// # Example
///
/// ```ignore
/// use bip39::{Mnemonic, Language};
/// use bth_crypto_pq::derive_pq_keys_from_seed;
///
/// let mnemonic = Mnemonic::from_phrase("abandon abandon ...", Language::English).unwrap();
/// let seed = bip39::Seed::new(&mnemonic, "optional passphrase");
/// let keys = derive_pq_keys_from_seed(seed.as_bytes());
///
/// // Use keys.kem_keypair for stealth address key exchange
/// // Use keys.sig_keypair for transaction signing
/// ```
pub fn derive_pq_keys_from_seed(seed: &[u8; BIP39_SEED_SIZE]) -> PqKeyMaterial {
    let seeds = derive_seeds_from_bip39(seed);

    PqKeyMaterial {
        kem_keypair: MlKem768KeyPair::from_seed(&seeds.kem_seed),
        sig_keypair: MlDsa65KeyPair::from_seed(&seeds.sig_seed),
    }
}

/// Derive post-quantum keypairs from raw bytes (DEPRECATED)
///
/// **WARNING**: This function is deprecated. Use `derive_pq_keys_from_seed` instead,
/// which properly accepts a BIP39 seed with full PBKDF2 key stretching.
///
/// This function exists for backwards compatibility but bypasses BIP39's
/// key stretching, making keys easier to brute-force if the mnemonic is
/// partially compromised.
#[deprecated(
    since = "7.2.0",
    note = "Use derive_pq_keys_from_seed with a proper BIP39 seed instead"
)]
pub fn derive_pq_keys(input: &[u8]) -> PqKeyMaterial {
    // For backwards compatibility, use the input directly
    // This is less secure than using a proper BIP39 seed
    let hk = Hkdf::<Sha256>::new(Some(BOTHO_PQ_DOMAIN), input);

    let mut kem_seed = [0u8; 32];
    let mut sig_seed = [0u8; 32];

    hk.expand(b"kem-seed", &mut kem_seed)
        .expect("32 bytes is valid for HKDF-SHA256");

    hk.expand(b"sig-seed", &mut sig_seed)
        .expect("32 bytes is valid for HKDF-SHA256");

    let seeds = DerivedSeeds { kem_seed, sig_seed };

    PqKeyMaterial {
        kem_keypair: MlKem768KeyPair::from_seed(&seeds.kem_seed),
        sig_keypair: MlDsa65KeyPair::from_seed(&seeds.sig_seed),
    }
}

/// Derive separate seeds for KEM and signature keypairs from BIP39 seed
fn derive_seeds_from_bip39(seed: &[u8; BIP39_SEED_SIZE]) -> DerivedSeeds {
    let hk = Hkdf::<Sha256>::new(Some(BOTHO_PQ_DOMAIN), seed);

    let mut kem_seed = [0u8; 32];
    let mut sig_seed = [0u8; 32];

    hk.expand(b"kem-seed", &mut kem_seed)
        .expect("32 bytes is valid for HKDF-SHA256");

    hk.expand(b"sig-seed", &mut sig_seed)
        .expect("32 bytes is valid for HKDF-SHA256");

    DerivedSeeds { kem_seed, sig_seed }
}

/// Derive a one-time ML-DSA keypair from a shared secret
///
/// This is used to derive the PQ one-time signing key for a specific output,
/// similar to how classical one-time keys are derived in CryptoNote.
///
/// # Arguments
///
/// * `shared_secret` - The shared secret from ML-KEM decapsulation
/// * `output_index` - The output index within the transaction
///
/// # Returns
///
/// A one-time ML-DSA keypair for signing when spending this output
pub fn derive_onetime_sig_keypair(shared_secret: &[u8; 32], output_index: u32) -> MlDsa65KeyPair {
    let hk = Hkdf::<Sha256>::new(Some(b"botho-pq-onetime"), shared_secret);

    let mut seed = [0u8; 32];
    let info = [b"sig-", output_index.to_le_bytes().as_slice()].concat();
    hk.expand(&info, &mut seed)
        .expect("32 bytes is valid for HKDF-SHA256");

    MlDsa65KeyPair::from_seed(&seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_pq_keys_from_seed_deterministic() {
        let seed: [u8; BIP39_SEED_SIZE] = [42u8; 64];

        let keys1 = derive_pq_keys_from_seed(&seed);
        let keys2 = derive_pq_keys_from_seed(&seed);

        // Same seed should produce identical keys
        assert_eq!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys2.kem_keypair.public_key().as_bytes()
        );
        assert_eq!(
            keys1.sig_keypair.public_key().as_bytes(),
            keys2.sig_keypair.public_key().as_bytes()
        );

        // Different seed should produce different keys
        let seed2: [u8; BIP39_SEED_SIZE] = [43u8; 64];
        let keys3 = derive_pq_keys_from_seed(&seed2);
        assert_ne!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys3.kem_keypair.public_key().as_bytes()
        );
    }

    #[test]
    fn test_derive_seeds_different_for_kem_and_sig() {
        let seed: [u8; BIP39_SEED_SIZE] = [42u8; 64];
        let seeds = derive_seeds_from_bip39(&seed);

        assert_ne!(seeds.kem_seed, seeds.sig_seed);
    }

    #[test]
    fn test_derive_seeds_deterministic() {
        let seed: [u8; BIP39_SEED_SIZE] = [42u8; 64];

        let seeds1 = derive_seeds_from_bip39(&seed);
        let seeds2 = derive_seeds_from_bip39(&seed);

        assert_eq!(seeds1.kem_seed, seeds2.kem_seed);
        assert_eq!(seeds1.sig_seed, seeds2.sig_seed);
    }

    #[test]
    fn test_derive_seeds_different_inputs() {
        let seed1: [u8; BIP39_SEED_SIZE] = [1u8; 64];
        let seed2: [u8; BIP39_SEED_SIZE] = [2u8; 64];

        let seeds1 = derive_seeds_from_bip39(&seed1);
        let seeds2 = derive_seeds_from_bip39(&seed2);

        assert_ne!(seeds1.kem_seed, seeds2.kem_seed);
        assert_ne!(seeds1.sig_seed, seeds2.sig_seed);
    }

    #[test]
    fn test_derive_onetime_sig_keypair() {
        let shared_secret = [42u8; 32];

        let keypair0 = derive_onetime_sig_keypair(&shared_secret, 0);
        let keypair1 = derive_onetime_sig_keypair(&shared_secret, 1);

        // Different output indices should produce different keypairs
        assert_ne!(
            keypair0.public_key().as_bytes(),
            keypair1.public_key().as_bytes()
        );

        // Same inputs should produce identical keypairs
        let keypair0_again = derive_onetime_sig_keypair(&shared_secret, 0);
        assert_eq!(
            keypair0.public_key().as_bytes(),
            keypair0_again.public_key().as_bytes()
        );
    }

    #[test]
    fn test_pq_key_material_accessors() {
        let seed: [u8; BIP39_SEED_SIZE] = [42u8; 64];
        let keys = derive_pq_keys_from_seed(&seed);

        // Verify we can access the public key bytes
        assert_eq!(keys.kem_public_key_bytes().len(), 1184);
        assert_eq!(keys.sig_public_key_bytes().len(), 1952);
    }

    #[test]
    #[allow(deprecated)]
    fn test_deprecated_derive_pq_keys_backwards_compat() {
        // Ensure deprecated function still works for backwards compatibility
        let input = b"test input";
        let keys1 = derive_pq_keys(input);
        let keys2 = derive_pq_keys(input);

        // Same input produces same keys
        assert_eq!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys2.kem_keypair.public_key().as_bytes()
        );
    }
}
