// Copyright (c) 2018-2022 The Botho Foundation

//! Proptest Fixtures.

pub use bt_crypto_ring_signature::{proptest_fixtures::*, CurveScalar, Scalar};

use crate::{amount::Amount, masked_amount::MaskedAmountV1, TokenId};
use bt_crypto_keys::RistrettoPublic;
use proptest::prelude::*;

prop_compose! {
    /// Generates an arbitrary masked_amount with value in [0,max_value].
    /// Of token_id = 0
    pub fn arbitrary_masked_amount(max_value: u64, shared_secret: RistrettoPublic)
                (value in 0..=max_value) -> MaskedAmountV1 {
            let amount = Amount {
                value,
                token_id: TokenId::BTH,
            };
            MaskedAmountV1::new(amount, &shared_secret).unwrap()
    }
}
