// Copyright (c) 2018-2023 The Botho Foundation

// Copyright (c) 2018-2022 The Botho Foundation

#![cfg_attr(not(feature = "std"), no_std)]
#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

extern crate alloc;

#[cfg(feature = "bth-account-keys")]
mod data;
mod error;
mod report;
mod verifier;

#[cfg(feature = "bth-account-keys")]
pub use data::{verify_tx_summary, TxOutSummaryUnblindingData, TxSummaryUnblindingData};

pub use error::Error;
pub use report::{TotalKind, TransactionEntity, TxSummaryUnblindingReport};
pub use verifier::TxSummaryStreamingVerifierCtx;
