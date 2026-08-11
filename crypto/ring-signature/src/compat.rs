// Copyright (c) 2018-2026 The Botho Foundation

//! Byte-level replacements for curve25519-dalek convenience constructors
//! whose trait generations changed in dalek 5.
//!
//! curve25519-dalek 5 moved `Scalar::random` / `RistrettoPoint::random`
//! behind rand_core 0.10 traits and `Scalar::from_hash` /
//! `RistrettoPoint::from_hash` behind digest 0.11 traits. This workspace
//! is on rand_core 0.10 (so `random` could call upstream directly) but
//! remains on digest 0.10, so these helpers re-implement the exact upstream
//! algorithms at the byte level:
//!
//! * `random`: draw 64 uniform bytes, then reduce (scalars) or apply the
//!   one-way uniform map (points) — identical to dalek 4 and dalek 5.
//! * `from_hash`: finalize the 64-byte digest, then reduce / map — identical to
//!   dalek 4 and dalek 5.
//!
//! Because the byte streams and reductions are unchanged, all derived keys,
//! signatures, and digests are bit-identical to the dalek 4 era.

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
use digest::{generic_array::typenum::U64, Digest};
use rand_core::CryptoRng;

/// Hash to a `Scalar` from a 512-bit digest (replaces `Scalar::from_hash`).
pub fn scalar_from_hash<D: Digest<OutputSize = U64>>(hasher: D) -> Scalar {
    let mut output = [0u8; 64];
    output.copy_from_slice(hasher.finalize().as_slice());
    Scalar::from_bytes_mod_order_wide(&output)
}

/// Hash to a `RistrettoPoint` from a 512-bit digest (replaces
/// `RistrettoPoint::from_hash`).
pub fn point_from_hash<D: Digest<OutputSize = U64>>(hasher: D) -> RistrettoPoint {
    let mut output = [0u8; 64];
    output.copy_from_slice(hasher.finalize().as_slice());
    RistrettoPoint::from_uniform_bytes(&output)
}

/// Draw a uniformly random `Scalar` from a workspace CSPRNG (replaces
/// `Scalar::random`).
pub fn random_scalar<R: CryptoRng + ?Sized>(rng: &mut R) -> Scalar {
    let mut bytes = [0u8; 64];
    rng.fill_bytes(&mut bytes);
    Scalar::from_bytes_mod_order_wide(&bytes)
}

/// Draw a uniformly random `RistrettoPoint` from a workspace CSPRNG
/// (replaces `RistrettoPoint::random`).
pub fn random_point<R: CryptoRng + ?Sized>(rng: &mut R) -> RistrettoPoint {
    let mut bytes = [0u8; 64];
    rng.fill_bytes(&mut bytes);
    RistrettoPoint::from_uniform_bytes(&bytes)
}
