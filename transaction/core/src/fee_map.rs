// Copyright (c) 2018-2022 The Botho Foundation

//! A helper object for maintaining a map of token id -> minimum fee.

use crate::{tokens::Bth, Token, TokenId};
use alloc::collections::BTreeMap;
use displaydoc::Display;
use bth_crypto_digestible::{Digestible, MerlinTranscript};
use serde::{Deserialize, Serialize};

/// The log base 2 of the smallest allowed minimum fee, in the smallest
/// representable units.
/// This minimum exists because it helps the computation of priority from fees
/// to work in a nice way.
///
/// Priority is computed by "normalizing" the fee for each token, using the
/// minimum fee. However, before dividing fee by minimum fee, we divide minimum
/// fee by (1 << 7) = 128.
///
/// This allows that if you increase the fee by e.g. 1%, then it always leads to
/// an integer difference in the priority and leads to your
/// transaction actually being ranked higher when the network sorts the tx's.
///
/// If we don't do this, then you can only increase the fee paid in increments
/// of the minimum fee to see an actual increase in priority. So effectively,
/// once the network is under load, the fees immediately double, then triple.
/// This seems undesirable.
///
/// (The choice of 128 is arbitrary, it's the first power of two >= 100.)
///
/// Because we divide minimum fee by by 128, and the result must be nonzero, we
/// must have that the minimum fee itself is at least as large as what we are
/// dividing by. This is fine because 128 in the smallest representable units is
/// a negligible amount of any currency.
///
/// The smallest allowed minimum fee is required to be a power of two, because
/// dividing by a power of two is fast and constant time.
pub const SMALLEST_MINIMUM_FEE_LOG2: u64 = 7;

/// A map of fee value by token id.
#[derive(Clone, Debug, Deserialize, Digestible, Eq, Hash, PartialEq, Serialize)]
pub struct FeeMap {
    /// The actual map of token_id to fee.
    /// Since we hash this map, it is important to use a BTreeMap as it
    /// guarantees iterating over the map is in sorted and predictable
    /// order.
    map: BTreeMap<TokenId, u64>,
}

impl Default for FeeMap {
    fn default() -> Self {
        let map = Self::default_map();

        Self { map }
    }
}

impl TryFrom<BTreeMap<TokenId, u64>> for FeeMap {
    type Error = Error;

    fn try_from(map: BTreeMap<TokenId, u64>) -> Result<Self, Self::Error> {
        Self::is_valid_map(&map)?;

        Ok(Self { map })
    }
}

impl AsRef<BTreeMap<TokenId, u64>> for FeeMap {
    fn as_ref(&self) -> &BTreeMap<TokenId, u64> {
        &self.map
    }
}

impl FeeMap {
    /// Create a fee map from an unsorted iterator.
    pub fn try_from_iter(iter: impl IntoIterator<Item = (TokenId, u64)>) -> Result<Self, Error> {
        let map = BTreeMap::from_iter(iter);
        Self::try_from(map)
    }

    /// Get the fee for a given token id, or None if no fee is set for that
    /// token.
    pub fn get_fee_for_token(&self, token_id: &TokenId) -> Option<u64> {
        self.map.get(token_id).cloned()
    }

    /// Update the fee map with a new one if provided, or reset it to the
    /// default.
    pub fn update_or_default(
        &mut self,
        minimum_fees: Option<BTreeMap<TokenId, u64>>,
    ) -> Result<(), Error> {
        if let Some(minimum_fees) = minimum_fees {
            Self::is_valid_map(&minimum_fees)?;

            self.map = minimum_fees;
        } else {
            self.map = Self::default_map();
        }

        Ok(())
    }

    /// Check if a given fee map is valid.
    pub fn is_valid_map(minimum_fees: &BTreeMap<TokenId, u64>) -> Result<(), Error> {
        // All minimum fees must be greater than 128 in the smallest representable unit.
        // This is because we divide the minimum fee by 128 when computing priority
        // numbers, to allow that increments of 1% of the minimum fee affect the
        // priority of a payment.
        if let Some((token_id, fee)) = minimum_fees
            .iter()
            .find(|(_token_id, fee)| (**fee >> SMALLEST_MINIMUM_FEE_LOG2) == 0)
        {
            return Err(Error::InvalidFeeTooSmall(*token_id, *fee));
        }

        if let Some((token_id, fee)) = minimum_fees
            .iter()
            .find(|(_token_id, fee)| (**fee % (1 << SMALLEST_MINIMUM_FEE_LOG2)) != 0)
        {
            return Err(Error::InvalidFeeNotDivisible(*token_id, *fee));
        }

        // Must have a minimum fee for BTH.
        if !minimum_fees.contains_key(&Bth::ID) {
            return Err(Error::MissingFee(Bth::ID));
        }

        // All good.
        Ok(())
    }

    /// Iterate over all entries in the fee map.
    pub fn iter(&self) -> impl Iterator<Item = (&TokenId, &u64)> {
        self.map.iter()
    }

    /// Helper method for constructing the default fee map.
    pub fn default_map() -> BTreeMap<TokenId, u64> {
        let mut map = BTreeMap::new();
        map.insert(Bth::ID, Bth::MINIMUM_FEE);
        map
    }

    /// Get a canonical digest of the minimum fee map
    pub fn canonical_digest(&self) -> [u8; 32] {
        self.digest32::<MerlinTranscript>(b"mc-fee-map")
    }
}

/// Fee Map error type.
#[derive(Clone, Debug, Deserialize, Display, PartialEq, PartialOrd, Serialize)]
pub enum Error {
    /// Token `{0}` has invalid fee (too small) `{1}`
    InvalidFeeTooSmall(TokenId, u64),

    /// Token `{0}` has invalid fee (not divisible) `{1}`
    InvalidFeeNotDivisible(TokenId, u64),

    /// Token `{0}` is missing from the fee map
    MissingFee(TokenId),
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Valid fee maps ids should be accepted
    #[test]
    fn valid_fee_maps_accepted() {
        let fee_map1 = FeeMap::try_from_iter([(Bth::ID, 1024), (TokenId::from(2), 80000)]).unwrap();
        assert!(fee_map1.get_fee_for_token(&Bth::ID).is_some());

        let fee_map2 = FeeMap::try_from_iter([(Bth::ID, 1024), (TokenId::from(2), 3072)]).unwrap();
        assert!(fee_map2.get_fee_for_token(&Bth::ID).is_some());

        let fee_map3 = FeeMap::try_from_iter([(Bth::ID, 1024), (TokenId::from(30), 3072)]).unwrap();
        assert!(fee_map3.get_fee_for_token(&Bth::ID).is_some());
    }

    /// Invalid fee maps are rejected.
    #[test]
    fn invalid_fee_maps_are_rejected() {
        let test_token_id = TokenId::from(2);

        // Missing BTH is not allowed
        assert_eq!(
            FeeMap::is_valid_map(&BTreeMap::default()),
            Err(Error::MissingFee(Bth::ID)),
        );

        assert_eq!(
            FeeMap::is_valid_map(&BTreeMap::from_iter(vec![(test_token_id, 1024)])),
            Err(Error::MissingFee(Bth::ID)),
        );

        // All fees must be >0
        assert_eq!(
            FeeMap::is_valid_map(&BTreeMap::from_iter(vec![(Bth::ID, 0)])),
            Err(Error::InvalidFeeTooSmall(Bth::ID, 0)),
        );

        assert_eq!(
            FeeMap::is_valid_map(&BTreeMap::from_iter(vec![(Bth::ID, 10)])),
            Err(Error::InvalidFeeTooSmall(Bth::ID, 10)),
        );

        assert_eq!(
            FeeMap::is_valid_map(&BTreeMap::from_iter(vec![
                (Bth::ID, 1024),
                (test_token_id, 0)
            ])),
            Err(Error::InvalidFeeTooSmall(test_token_id, 0)),
        );

        // All fees must be evenly divisible by smallest minimum fee
        assert_eq!(
            FeeMap::try_from_iter([(Bth::ID, 1023), (TokenId::from(2), 80000)]),
            Err(Error::InvalidFeeNotDivisible(Bth::ID, 1023))
        );

        assert_eq!(
            FeeMap::try_from_iter([(Bth::ID, 1024), (TokenId::from(2), 80001)]),
            Err(Error::InvalidFeeNotDivisible(TokenId::from(2), 80001))
        );
    }

    #[test]
    fn test_fee_map_default() {
        let fee_map = FeeMap::default();
        assert!(fee_map.get_fee_for_token(&Bth::ID).is_some());
        assert_eq!(fee_map.get_fee_for_token(&Bth::ID).unwrap(), Bth::MINIMUM_FEE);
    }

    #[test]
    fn test_fee_map_get_nonexistent_token() {
        let fee_map = FeeMap::default();
        let fake_token = TokenId::from(999);
        assert!(fee_map.get_fee_for_token(&fake_token).is_none());
    }

    #[test]
    fn test_fee_map_iter() {
        let fee_map = FeeMap::try_from_iter([
            (Bth::ID, 1024),
            (TokenId::from(2), 2048),
            (TokenId::from(3), 4096),
        ])
        .unwrap();

        let entries: Vec<_> = fee_map.iter().collect();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_fee_map_as_ref() {
        let fee_map = FeeMap::default();
        let map_ref: &BTreeMap<TokenId, u64> = fee_map.as_ref();
        assert!(map_ref.contains_key(&Bth::ID));
    }

    #[test]
    fn test_fee_map_update_or_default_with_some() {
        let mut fee_map = FeeMap::default();
        let new_fees = BTreeMap::from_iter(vec![
            (Bth::ID, 2048),
            (TokenId::from(2), 4096),
        ]);

        fee_map.update_or_default(Some(new_fees)).unwrap();
        assert_eq!(fee_map.get_fee_for_token(&Bth::ID).unwrap(), 2048);
        assert_eq!(fee_map.get_fee_for_token(&TokenId::from(2)).unwrap(), 4096);
    }

    #[test]
    fn test_fee_map_update_or_default_with_none() {
        let mut fee_map = FeeMap::try_from_iter([
            (Bth::ID, 2048),
            (TokenId::from(2), 4096),
        ])
        .unwrap();

        fee_map.update_or_default(None).unwrap();
        // Should reset to default
        assert_eq!(fee_map.get_fee_for_token(&Bth::ID).unwrap(), Bth::MINIMUM_FEE);
        assert!(fee_map.get_fee_for_token(&TokenId::from(2)).is_none());
    }

    #[test]
    fn test_fee_map_update_or_default_with_invalid() {
        let mut fee_map = FeeMap::default();
        let invalid_fees = BTreeMap::from_iter(vec![
            (Bth::ID, 100), // Too small
        ]);

        let result = fee_map.update_or_default(Some(invalid_fees));
        assert!(result.is_err());
    }

    #[test]
    fn test_fee_map_canonical_digest_deterministic() {
        let fee_map1 = FeeMap::try_from_iter([
            (Bth::ID, 1024),
            (TokenId::from(2), 2048),
        ])
        .unwrap();

        let fee_map2 = FeeMap::try_from_iter([
            (TokenId::from(2), 2048), // Different order
            (Bth::ID, 1024),
        ])
        .unwrap();

        // Canonical digest should be the same regardless of insertion order
        assert_eq!(fee_map1.canonical_digest(), fee_map2.canonical_digest());
    }

    #[test]
    fn test_fee_map_canonical_digest_different_for_different_fees() {
        let fee_map1 = FeeMap::try_from_iter([(Bth::ID, 1024)]).unwrap();
        let fee_map2 = FeeMap::try_from_iter([(Bth::ID, 2048)]).unwrap();

        assert_ne!(fee_map1.canonical_digest(), fee_map2.canonical_digest());
    }

    #[test]
    fn test_fee_map_clone() {
        let fee_map = FeeMap::try_from_iter([
            (Bth::ID, 1024),
            (TokenId::from(2), 2048),
        ])
        .unwrap();

        let cloned = fee_map.clone();
        assert_eq!(fee_map, cloned);
    }

    #[test]
    fn test_fee_map_try_from_btreemap() {
        let map = BTreeMap::from_iter(vec![
            (Bth::ID, 1024),
            (TokenId::from(2), 2048),
        ]);

        let fee_map = FeeMap::try_from(map).unwrap();
        assert_eq!(fee_map.get_fee_for_token(&Bth::ID).unwrap(), 1024);
    }

    #[test]
    fn test_error_display() {
        let err1 = Error::InvalidFeeTooSmall(Bth::ID, 10);
        assert!(err1.to_string().contains("too small"));

        let err2 = Error::InvalidFeeNotDivisible(Bth::ID, 1023);
        assert!(err2.to_string().contains("not divisible"));

        let err3 = Error::MissingFee(Bth::ID);
        assert!(err3.to_string().contains("missing"));
    }

    #[test]
    fn test_smallest_minimum_fee_log2() {
        assert_eq!(SMALLEST_MINIMUM_FEE_LOG2, 7);
        assert_eq!(1u64 << SMALLEST_MINIMUM_FEE_LOG2, 128);
    }
}
