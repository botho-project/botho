// Copyright (c) 2018-2026 The Botho Foundation

//! Byte-level replacements for curve25519-dalek convenience constructors
//! whose trait generations changed in dalek 5.
//!
//! curve25519-dalek 5 moved `Scalar::random` behind rand_core 0.10 traits and
//! `Scalar::from_hash` / `RistrettoPoint::from_hash` behind digest 0.11
//! traits. This workspace is on rand_core 0.10 but remains on digest 0.10,
//! so these helpers re-implement the exact upstream algorithms at the byte
//! level. Outputs are bit-identical to the dalek 4 era.

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
use rand_core::CryptoRng;
use sha2::{Digest, Sha512};

/// Hash to a `Scalar` from a `Sha512` digest (replaces `Scalar::from_hash`).
pub(crate) fn scalar_from_hash(hasher: Sha512) -> Scalar {
    let mut output = [0u8; 64];
    output.copy_from_slice(hasher.finalize().as_slice());
    Scalar::from_bytes_mod_order_wide(&output)
}

/// Hash to a `RistrettoPoint` from a `Sha512` digest (replaces
/// `RistrettoPoint::from_hash`).
pub(crate) fn point_from_hash(hasher: Sha512) -> RistrettoPoint {
    let mut output = [0u8; 64];
    output.copy_from_slice(hasher.finalize().as_slice());
    RistrettoPoint::from_uniform_bytes(&output)
}

/// Draw a uniformly random `Scalar` from a workspace CSPRNG (replaces
/// `Scalar::random`).
pub(crate) fn random_scalar<R: CryptoRng + ?Sized>(rng: &mut R) -> Scalar {
    let mut bytes = [0u8; 64];
    rng.fill_bytes(&mut bytes);
    Scalar::from_bytes_mod_order_wide(&bytes)
}
