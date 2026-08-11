// Copyright (c) 2018-2026 The Botho Foundation

//! Bridges workspace (rand_core 0.10) CSPRNGs into the rand_core 0.6 traits
//! still consumed by dalek-4-era external crates (schnorrkel-og transcript
//! signing, bulletproofs-og range proofs).
//!
//! All methods forward directly, so the byte stream drawn from the underlying
//! RNG is identical to the rand_core 0.6 era — deterministic key derivation
//! and signing are bit-for-bit unchanged.

use rand_core::CryptoRng;

/// Wraps a workspace (rand_core 0.10) CSPRNG, exposing the rand_core 0.6
/// `RngCore` + `CryptoRng` traits that dalek-4-era crates expect.
pub struct Rng06Compat<'a, R: CryptoRng + ?Sized>(pub &'a mut R);

impl<R: CryptoRng + ?Sized> rand_core_v06::RngCore for Rng06Compat<'_, R> {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest)
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core_v06::Error> {
        self.0.fill_bytes(dest);
        Ok(())
    }
}

impl<R: CryptoRng + ?Sized> rand_core_v06::CryptoRng for Rng06Compat<'_, R> {}
