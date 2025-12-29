// Copyright (c) 2018-2022 The Botho Foundation

//! Convert to/from external::PublicAddress

use crate::{external, ConversionError};
use bth_account_keys::PublicAddress;

impl From<&PublicAddress> for external::PublicAddress {
    fn from(src: &PublicAddress) -> Self {
        Self {
            view_public_key: Some(src.view_public_key().into()),
            spend_public_key: Some(src.spend_public_key().into()),
        }
    }
}

impl TryFrom<&external::PublicAddress> for PublicAddress {
    type Error = ConversionError;

    fn try_from(src: &external::PublicAddress) -> Result<Self, Self::Error> {
        let spend_public_key = src
            .spend_public_key
            .as_ref()
            .ok_or(bth_crypto_keys::KeyError::LengthMismatch(0, 32))
            .and_then(|key| bth_crypto_keys::RistrettoPublic::try_from(&key.data[..]))?;

        let view_public_key = src
            .view_public_key
            .as_ref()
            .ok_or(bth_crypto_keys::KeyError::LengthMismatch(0, 32))
            .and_then(|key| bth_crypto_keys::RistrettoPublic::try_from(&key.data[..]))?;

        Ok(PublicAddress::new(&spend_public_key, &view_public_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_account_keys::AccountKey;
    use rand::{rngs::StdRng, SeedableRng};

    // Test converting between external::PublicAddress and
    // account_keys::PublicAddress
    #[test]
    fn test_public_address_conversion() {
        let mut rng: StdRng = SeedableRng::from_seed([123u8; 32]);

        // public_address -> external
        let public_address = AccountKey::random(&mut rng).default_subaddress();
        let proto_credentials = external::PublicAddress::from(&public_address);
        assert_eq!(
            *proto_credentials.view_public_key.as_ref().unwrap(),
            external::CompressedRistretto::from(public_address.view_public_key())
        );
        assert_eq!(
            *proto_credentials.spend_public_key.as_ref().unwrap(),
            external::CompressedRistretto::from(public_address.spend_public_key())
        );

        // external -> public_address
        let public_address2 = PublicAddress::try_from(&proto_credentials).unwrap();
        assert_eq!(public_address, public_address2);
    }
}
