// Copyright (c) 2018-2022 The Botho Foundation

//! A registry of tokens

use bth_transaction_types::TokenId;

/// A generic representation of a token.
pub trait Token {
    /// Token Id.
    const ID: TokenId;

    /// Default mininum fee for this token.
    const MINIMUM_FEE: u64;
}

/// Exports structures which expose constants related to tokens.
///
/// If changing this, please keep it in sync with the enum defined in
/// external.proto
pub mod tokens {
    use super::*;
    use crate::constants::MICROBTH_TO_NANOBTH;

    /// The BTH token.
    pub struct Bth;
    impl Token for Bth {
        /// Token Id.
        const ID: TokenId = TokenId::BTH;

        /// Minimum fee, denominated in nanoBTH.
        const MINIMUM_FEE: u64 = 400 * MICROBTH_TO_NANOBTH;
    }
}
