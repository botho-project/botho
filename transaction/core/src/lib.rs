// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Botho transaction data types, transaction construction and validation
//! routines.

#![no_std]
#![deny(missing_docs)]

extern crate alloc;

#[macro_use]
extern crate lazy_static;

pub mod encrypted_fog_hint;
mod fee_map;
mod input_rules;
mod memo;
mod revealed_tx_out;
mod token;
mod tx_error;

pub mod membership_proofs;
pub mod mint;
pub mod range_proofs;
pub mod ring_ct;
pub mod tx;
pub mod tx_summary;
pub mod validation;

#[cfg(feature = "pq")]
pub mod quantum_private;

pub use fee_map::{Error as FeeMapError, FeeMap, SMALLEST_MINIMUM_FEE_LOG2};
pub use input_rules::{InputRuleError, InputRules};
pub use memo::{EncryptedMemo, MemoError, MemoPayload};
pub use revealed_tx_out::{try_reveal_amount, RevealedTxOut, RevealedTxOutError};
pub use token::{tokens, Token};
pub use tx::MemoContext;
pub use tx_error::{NewMemoError, NewTxError, TxOutConversionError, ViewKeyMatchError};
pub use tx_summary::TxSummaryNew;

// Re-export encrypted_fog_hint stub for backwards compatibility
pub use encrypted_fog_hint::{EncryptedFogHint, ENCRYPTED_FOG_HINT_LEN};

// Re-export from transaction-types, and some from RingSignature crate.
pub use bth_crypto_ring_signature::{Commitment, CompressedCommitment};
pub use bth_transaction_types::{
    constants, domain_separators, Amount, AmountError, BlockVersion, BlockVersionError,
    ClusterId, ClusterTagEntry, ClusterTagVector, MaskedAmount, MaskedAmountV1, MaskedAmountV2,
    TokenId, TxSummary, UnmaskedAmount, MAX_CLUSTER_TAGS, MIN_STORED_WEIGHT, TAG_WEIGHT_SCALE,
};

/// Re-export all of mc-crypto-ring-signature
pub mod ring_signature {
    pub use bth_crypto_ring_signature::*;
}

// Re-export the one-time keys module which historically lived in this crate
pub use bth_crypto_ring_signature::onetime_keys;

// Re-export some dependent types from mc-account-keys
pub use bth_account_keys::{AccountKey, PublicAddress};

use bth_crypto_keys::{KeyError, RistrettoPrivate, RistrettoPublic};
use onetime_keys::{create_shared_secret, recover_public_subaddress_spend_key};
use tx::TxOut;

/// Get the shared secret for a transaction output.
///
/// # Arguments
/// * `view_key` - The recipient's private View key.
/// * `tx_public_key` - The public key of the transaction.
pub fn get_tx_out_shared_secret(
    view_key: &RistrettoPrivate,
    tx_public_key: &RistrettoPublic,
) -> RistrettoPublic {
    create_shared_secret(tx_public_key, view_key)
}

/// Helper which checks if a particular subaddress of an account key matches a
/// TxOut
///
/// This is not the most efficient way to check when you have many subaddresses,
/// for that you should create a table and use
/// recover_public_subaddress_spend_key directly.
///
/// However some clients are only using one or two subaddresses.
/// Validating that a TxOut is owned by the change subaddress is a frequently
/// needed operation.
pub fn subaddress_matches_tx_out(
    acct: &AccountKey,
    subaddress_index: u64,
    output: &TxOut,
) -> Result<bool, KeyError> {
    let sub_addr_spend = recover_public_subaddress_spend_key(
        acct.view_private_key(),
        &RistrettoPublic::try_from(&output.target_key)?,
        &RistrettoPublic::try_from(&output.public_key)?,
    );
    Ok(sub_addr_spend == RistrettoPublic::from(&acct.subaddress_spend_private(subaddress_index)))
}

// Re-export quantum-private transaction types when pq feature is enabled
#[cfg(feature = "pq")]
pub use quantum_private::{
    QuantumPrivateError, QuantumPrivateTxIn, QuantumPrivateTxOut, TransactionType,
};
