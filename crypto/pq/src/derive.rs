//! Key Derivation for Post-Quantum Keys
//!
//! This module provides deterministic derivation of ML-KEM and ML-DSA keypairs
//! from a mnemonic seed. This ensures that:
//!
//! 1. Users don't need to backup additional key material
//! 2. PQ keys can be regenerated from the same mnemonic
//! 3. Key derivation is deterministic and reproducible
//!
//! # Derivation Path
//!
//! ```text
//! mnemonic
//!    │
//!    ├── HKDF(salt="botho-pq-v1", info="kem-seed") ──► ML-KEM-768 keypair
//!    │
//!    └── HKDF(salt="botho-pq-v1", info="sig-seed") ──► ML-DSA-65 keypair
//! ```

use crate::{MlDsa65KeyPair, MlKem768KeyPair, BOTHO_PQ_DOMAIN};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Seeds for PQ key generation, derived from mnemonic
#[derive(Zeroize, ZeroizeOnDrop)]
struct DerivedSeeds {
    kem_seed: [u8; 32],
    sig_seed: [u8; 32],
}

/// Post-quantum key material derived from a mnemonic
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

/// Derive post-quantum keypairs from mnemonic bytes
///
/// This function takes the raw mnemonic bytes and derives both ML-KEM and
/// ML-DSA keypairs deterministically using HKDF.
///
/// # Arguments
///
/// * `mnemonic` - The BIP39 mnemonic phrase as bytes
///
/// # Returns
///
/// A `PqKeyMaterial` struct containing both keypairs
///
/// # Example
///
/// ```rust
/// use bth_crypto_pq::derive_pq_keys;
///
/// let mnemonic = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
/// let keys = derive_pq_keys(mnemonic);
///
/// // Use keys.kem_keypair for stealth address key exchange
/// // Use keys.sig_keypair for transaction signing
/// ```
pub fn derive_pq_keys(mnemonic: &[u8]) -> PqKeyMaterial {
    let seeds = derive_seeds(mnemonic);

    PqKeyMaterial {
        kem_keypair: MlKem768KeyPair::from_seed(&seeds.kem_seed),
        sig_keypair: MlDsa65KeyPair::from_seed(&seeds.sig_seed),
    }
}

/// Derive separate seeds for KEM and signature keypairs
fn derive_seeds(mnemonic: &[u8]) -> DerivedSeeds {
    let hk = Hkdf::<Sha256>::new(Some(BOTHO_PQ_DOMAIN), mnemonic);

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
    fn test_derive_pq_keys_deterministic() {
        let mnemonic = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        let keys1 = derive_pq_keys(mnemonic);
        let keys2 = derive_pq_keys(mnemonic);

        // Same mnemonic should produce identical keys
        assert_eq!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys2.kem_keypair.public_key().as_bytes()
        );
        assert_eq!(
            keys1.sig_keypair.public_key().as_bytes(),
            keys2.sig_keypair.public_key().as_bytes()
        );

        // Different mnemonic should produce different keys
        let keys3 = derive_pq_keys(b"different mnemonic");
        assert_ne!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys3.kem_keypair.public_key().as_bytes()
        );
    }

    #[test]
    fn test_derive_seeds_different_for_kem_and_sig() {
        let mnemonic = b"test mnemonic";
        let seeds = derive_seeds(mnemonic);

        assert_ne!(seeds.kem_seed, seeds.sig_seed);
    }

    #[test]
    fn test_derive_seeds_deterministic() {
        let mnemonic = b"test mnemonic";

        let seeds1 = derive_seeds(mnemonic);
        let seeds2 = derive_seeds(mnemonic);

        assert_eq!(seeds1.kem_seed, seeds2.kem_seed);
        assert_eq!(seeds1.sig_seed, seeds2.sig_seed);
    }

    #[test]
    fn test_derive_seeds_different_mnemonics() {
        let seeds1 = derive_seeds(b"mnemonic one");
        let seeds2 = derive_seeds(b"mnemonic two");

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
        let mnemonic = b"test mnemonic";
        let keys = derive_pq_keys(mnemonic);

        // Verify we can access the public key bytes
        assert_eq!(keys.kem_public_key_bytes().len(), 1184);
        assert_eq!(keys.sig_public_key_bytes().len(), 1952);
    }
}
