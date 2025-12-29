// Copyright (c) 2018-2022 The Botho Foundation

//! Errors which can occur in connection to RingMLSAG signatures

use displaydoc::Display;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// An error which can occur when signing or verifying an MLSAG
#[derive(Clone, Debug, Display, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub enum Error {
    /// Incorrect length for array copy, provided `{0}`, required `{1}`.
    LengthMismatch(usize, usize),

    /// Index out of bounds
    IndexOutOfBounds,

    /// Invalid curve point
    InvalidCurvePoint,

    /// The signature was not able to be validated
    InvalidSignature,

    /// Failed to compress/decompress a KeyImage
    InvalidKeyImage,

    /// Value not conserved
    ValueNotConserved,

    /// Unexpected tx_out index
    UnexpectedTxout,

    /// Invalid signing state
    InvalidState,
}

impl From<bt_util_repr_bytes::LengthMismatch> for Error {
    fn from(src: bt_util_repr_bytes::LengthMismatch) -> Self {
        Self::LengthMismatch(src.found, src.expected)
    }
}
