// Copyright (c) 2018-2022 The Botho Foundation

//! Traits and wrapper types connected to Botho account keys.
//! This crate is intended to have a small footprint and be maximally portable.

#![no_std]
#![deny(missing_docs)]
#![deny(unsafe_code)]

mod traits;

pub use traits::RingCtAddress;
