// Copyright (c) 2018-2022 The MobileCoin Foundation
// Copyright (c) 2024 Cadence Foundation

//! Note: With SGX removed, membership proofs are no longer needed.
//! The well_formed_check now just returns the current block index.

use crate::enclave_stubs::{TxContext, WellFormedTxContext};
use mc_transaction_core::{tx::TxHash, validation::TransactionValidationResult};
use std::sync::Arc;

#[cfg(test)]
use mockall::*;

/// The untrusted (i.e. non-enclave) part of validating and combining
/// transactions.
#[cfg_attr(test, automock)]
pub trait UntrustedInterfaces: Send + Sync {
    /// Performs **only** the untrusted part of the well-formed check.
    ///
    /// Returns the local ledger's current block index.
    /// Note: Membership proofs were removed with SGX.
    fn well_formed_check(&self, tx_context: &TxContext) -> TransactionValidationResult<u64>;

    /// Checks if a transaction is valid (see definition in validators.rs).
    fn is_valid(&self, context: Arc<WellFormedTxContext>) -> TransactionValidationResult<()>;

    /// Combines a set of "candidate values" into a "composite value".
    /// This assumes all values are well-formed and safe to append to the ledger
    /// individually.
    ///
    /// # Arguments
    /// * `tx_contexts` - "Candidate" transactions. Each is assumed to be
    ///   individually valid.
    /// * `max_elements` - Maximal number of elements to output.
    ///
    /// Returns a bounded, deterministically-ordered list of transactions that
    /// are safe to append to the ledger.
    fn combine(
        &self,
        tx_contexts: &[Arc<WellFormedTxContext>],
        max_elements: usize,
    ) -> Vec<TxHash>;
}
