// Copyright (c) 2018-2022 The Botho Foundation

//! A trait which provides a common API for types which can be initialized
//! from data provided by random number generators.

#![no_std]

pub use rand_core::{CryptoRng, Rng};

/// A trait which can construct an object from a cryptographically secure
/// pseudo-random number generator.
pub trait FromRandom: Sized {
    /// Using a mutable RNG, take it's output to securely initialize the object
    fn from_random<R: CryptoRng>(csprng: &mut R) -> Self;
}

impl<const N: usize> FromRandom for [u8; N] {
    fn from_random<R: CryptoRng>(csprng: &mut R) -> [u8; N] {
        let mut result = [0u8; N];
        csprng.fill_bytes(&mut result);
        result
    }
}

/// System randomness as an infallible CSPRNG.
///
/// Drop-in replacement for the retired `rand_core::OsRng` of the
/// rand_core 0.6 line: a zero-sized handle to the operating system's
/// randomness source that implements the infallible [`CryptoRng`] trait by
/// panicking if the source fails — exactly the panic-on-failure semantics the
/// old `OsRng: RngCore` had.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OsRng;

impl rand_core::TryRng for OsRng {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut buf = [0u8; 4];
        getrandom::fill(&mut buf).expect("OS randomness source failed");
        Ok(u32::from_le_bytes(buf))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut buf = [0u8; 8];
        getrandom::fill(&mut buf).expect("OS randomness source failed");
        Ok(u64::from_le_bytes(buf))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(dst).expect("OS randomness source failed");
        Ok(())
    }
}

impl rand_core::TryCryptoRng for OsRng {}
