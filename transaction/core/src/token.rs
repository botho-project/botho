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
    use crate::constants::MICROBTH_TO_PICOCREDITS;

    /// The BTH token.
    pub struct Bth;
    impl Token for Bth {
        /// Token Id.
        const ID: TokenId = TokenId::BTH;

        /// Minimum fee, denominated in picocredits (#694; formerly the same
        /// 400 microBTH expressed in nanoBTH).
        const MINIMUM_FEE: u64 = 400 * MICROBTH_TO_PICOCREDITS;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MICROBTH_TO_PICOCREDITS;

    #[test]
    fn test_bth_token_id() {
        assert_eq!(tokens::Bth::ID, TokenId::BTH);
    }

    #[test]
    fn test_bth_minimum_fee() {
        // Minimum fee should be 400 microBTH in picocredits.
        let expected_fee = 400 * MICROBTH_TO_PICOCREDITS;
        assert_eq!(tokens::Bth::MINIMUM_FEE, expected_fee);
        // Pin the BTH-denominated value across the #694 re-denomination:
        // 400 microBTH = 0.0004 BTH = 400,000,000 picocredits (raw value
        // scaled x1000 from the former 400,000 nanoBTH; identical in BTH).
        assert_eq!(tokens::Bth::MINIMUM_FEE, 400_000_000);
    }

    #[test]
    fn test_bth_minimum_fee_is_valid() {
        // The minimum fee should be >= 128 (SMALLEST_MINIMUM_FEE)
        // and divisible by 128
        let fee = tokens::Bth::MINIMUM_FEE;
        assert!(fee >= 128);
        assert_eq!(fee % 128, 0);
    }
}
