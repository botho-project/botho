// Copyright (c) 2018-2022 The Botho Foundation

//! Errors that can occur when creating a new TxOut

use alloc::{format, string::String};
use core::str::Utf8Error;

use crate::MemoError;
use displaydoc::Display;
use bth_crypto_keys::KeyError;
use bth_transaction_types::AmountError;
use serde::{Deserialize, Serialize};

/// An error that occurs when creating a new TxOut
#[derive(Clone, Debug, Display)]
pub enum NewTxError {
    /// Amount: {0}
    Amount(AmountError),
    /// Memo: {0}
    Memo(NewMemoError),
}

impl From<AmountError> for NewTxError {
    fn from(src: AmountError) -> NewTxError {
        NewTxError::Amount(src)
    }
}

impl From<NewMemoError> for NewTxError {
    fn from(src: NewMemoError) -> NewTxError {
        NewTxError::Memo(src)
    }
}

/// An error that occurs when handling a TxOut
#[derive(Clone, Debug, Display, Ord, PartialOrd, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum TxOutConversionError {
    /// Unknown Masked Amount Version
    UnknownMaskedAmountVersion,
}

/// An error that occurs when view key matching a TxOut
#[derive(Clone, Debug, Display)]
pub enum ViewKeyMatchError {
    /// Key: {0}
    Key(KeyError),
    /// Amount: {0}
    Amount(AmountError),
    /// Unknown Masked Amount Version
    UnknownMaskedAmountVersion,
}

impl From<KeyError> for ViewKeyMatchError {
    fn from(src: KeyError) -> Self {
        Self::Key(src)
    }
}

impl From<AmountError> for ViewKeyMatchError {
    fn from(src: AmountError) -> Self {
        Self::Amount(src)
    }
}

/// An error that occurs when creating a new Memo for a TxOut
///
/// These errors are usually created by a MemoBuilder.
/// We have included error codes for some known useful error conditions.
/// For a custom MemoBuilder, you can try to reuse those, or use the Other
/// error code.
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum NewMemoError {
    /// Limits for '{0}' value exceeded
    LimitsExceeded(&'static str),
    /// Multiple change outputs not supported
    MultipleChangeOutputs,
    /// Creating more outputs after the change output is not supported
    OutputsAfterChange,
    /// Changing the fee after the change output is not supported
    FeeAfterChange,
    /// Invalid recipient address
    InvalidRecipient,
    /// Multiple outputs are not supported
    MultipleOutputs,
    /// Missing output
    MissingOutput,
    /// Missing required input to build the memo: {0}
    MissingInput(String),
    /// Mixed Token Ids are not supported in these memos
    MixedTokenIds,
    /// Destination memo is not supported
    DestinationMemoNotAllowed,
    /// Improperly configured input: {0}
    BadInputs(String),
    /// Creation
    Creation(MemoError),
    /// Utf-8 did not properly decode
    Utf8Decoding,
    /// Attempted value: {1} > Max Value: {0}
    MaxFeeExceeded(u64, u64),
    /// Payment request and intent ID both are set
    RequestAndIntentIdSet,
    /// Defragmentation transaction with non-zero change
    DefragWithChange,
    /// Other: {0}
    Other(String),
}

impl From<MemoError> for NewMemoError {
    fn from(src: MemoError) -> Self {
        match src {
            MemoError::Utf8Decoding => Self::Utf8Decoding,
            MemoError::BadLength(byte_len) => Self::BadInputs(format!(
                "Input of length: {byte_len} exceeded max byte length"
            )),
            MemoError::MaxFeeExceeded(max_fee, attempted_fee) => {
                Self::MaxFeeExceeded(max_fee, attempted_fee)
            }
        }
    }
}

impl From<Utf8Error> for NewMemoError {
    fn from(_: Utf8Error) -> Self {
        Self::Utf8Decoding
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn test_new_tx_error_from_amount_error() {
        let amount_err = AmountError::InconsistentCommitment;
        let tx_err: NewTxError = amount_err.into();
        match tx_err {
            NewTxError::Amount(_) => {}
            _ => panic!("Expected Amount variant"),
        }
    }

    #[test]
    fn test_new_tx_error_from_memo_error() {
        let memo_err = NewMemoError::MultipleChangeOutputs;
        let tx_err: NewTxError = memo_err.into();
        match tx_err {
            NewTxError::Memo(_) => {}
            _ => panic!("Expected Memo variant"),
        }
    }

    #[test]
    fn test_new_tx_error_display() {
        let amount_err = AmountError::InconsistentCommitment;
        let tx_err: NewTxError = amount_err.into();
        let display = tx_err.to_string();
        assert!(display.contains("Amount"));
    }

    #[test]
    fn test_tx_out_conversion_error() {
        let err = TxOutConversionError::UnknownMaskedAmountVersion;
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_view_key_match_error_from_key_error() {
        let key_err = KeyError::LengthMismatch(32, 64);
        let match_err: ViewKeyMatchError = key_err.into();
        match match_err {
            ViewKeyMatchError::Key(_) => {}
            _ => panic!("Expected Key variant"),
        }
    }

    #[test]
    fn test_view_key_match_error_from_amount_error() {
        let amount_err = AmountError::InconsistentCommitment;
        let match_err: ViewKeyMatchError = amount_err.into();
        match match_err {
            ViewKeyMatchError::Amount(_) => {}
            _ => panic!("Expected Amount variant"),
        }
    }

    #[test]
    fn test_new_memo_error_variants() {
        // Test various NewMemoError variants
        let err1 = NewMemoError::LimitsExceeded("test");
        assert!(err1.to_string().contains("test"));

        let err2 = NewMemoError::MultipleChangeOutputs;
        assert!(!err2.to_string().is_empty());

        let err3 = NewMemoError::OutputsAfterChange;
        assert!(!err3.to_string().is_empty());

        let err4 = NewMemoError::FeeAfterChange;
        assert!(!err4.to_string().is_empty());

        let err5 = NewMemoError::InvalidRecipient;
        assert!(!err5.to_string().is_empty());

        let err6 = NewMemoError::MultipleOutputs;
        assert!(!err6.to_string().is_empty());

        let err7 = NewMemoError::MissingOutput;
        assert!(!err7.to_string().is_empty());

        let err8 = NewMemoError::MissingInput("amount".to_string());
        assert!(err8.to_string().contains("amount"));

        let err9 = NewMemoError::MixedTokenIds;
        assert!(!err9.to_string().is_empty());

        let err10 = NewMemoError::DestinationMemoNotAllowed;
        assert!(!err10.to_string().is_empty());

        let err11 = NewMemoError::BadInputs("bad".to_string());
        assert!(err11.to_string().contains("bad"));

        let err12 = NewMemoError::Utf8Decoding;
        assert!(!err12.to_string().is_empty());

        let err13 = NewMemoError::MaxFeeExceeded(100, 200);
        assert!(err13.to_string().contains("100"));
        assert!(err13.to_string().contains("200"));

        let err14 = NewMemoError::RequestAndIntentIdSet;
        assert!(!err14.to_string().is_empty());

        let err15 = NewMemoError::DefragWithChange;
        assert!(!err15.to_string().is_empty());

        let err16 = NewMemoError::Other("custom".to_string());
        assert!(err16.to_string().contains("custom"));
    }

    #[test]
    fn test_new_memo_error_from_memo_error() {
        let memo_err = MemoError::Utf8Decoding;
        let new_memo_err: NewMemoError = memo_err.into();
        assert_eq!(new_memo_err, NewMemoError::Utf8Decoding);

        let memo_err2 = MemoError::BadLength(100);
        let new_memo_err2: NewMemoError = memo_err2.into();
        match new_memo_err2 {
            NewMemoError::BadInputs(_) => {}
            _ => panic!("Expected BadInputs variant"),
        }

        let memo_err3 = MemoError::MaxFeeExceeded(100, 200);
        let new_memo_err3: NewMemoError = memo_err3.into();
        assert_eq!(new_memo_err3, NewMemoError::MaxFeeExceeded(100, 200));
    }

    #[test]
    fn test_new_memo_error_equality() {
        let err1 = NewMemoError::MultipleChangeOutputs;
        let err2 = NewMemoError::MultipleChangeOutputs;
        let err3 = NewMemoError::OutputsAfterChange;

        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }
}
