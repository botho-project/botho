// Copyright (c) 2018-2022 The Botho Foundation

//! Convert to/from external::AccountKey

use crate::{external, ConversionError};
use bth_account_keys::AccountKey;

impl From<&AccountKey> for external::AccountKey {
    fn from(src: &AccountKey) -> Self {
        Self {
            view_private_key: Some(src.view_private_key().into()),
            spend_private_key: Some(src.spend_private_key().into()),
        }
    }
}

impl TryFrom<&external::AccountKey> for AccountKey {
    type Error = ConversionError;

    fn try_from(src: &external::AccountKey) -> Result<Self, Self::Error> {
        let spend_private_key = src
            .spend_private_key
            .as_ref()
            .ok_or(bth_crypto_keys::KeyError::LengthMismatch(0, 32))
            .and_then(|key| bth_crypto_keys::RistrettoPrivate::try_from(&key.data[..]))?;

        let view_private_key = src
            .view_private_key
            .as_ref()
            .ok_or(bth_crypto_keys::KeyError::LengthMismatch(0, 32))
            .and_then(|key| bth_crypto_keys::RistrettoPrivate::try_from(&key.data[..]))?;

        // Note: fog fields are ignored - fog support removed
        Ok(AccountKey::new(&spend_private_key, &view_private_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, SeedableRng};

    // Test converting between external::AccountKey and account_keys::AccountKey
    #[test]
    fn test_account_key_conversion() {
        let mut rng: StdRng = SeedableRng::from_seed([123u8; 32]);

        // account_keys -> external
        let account_key = AccountKey::random(&mut rng);
        let proto_credentials = external::AccountKey::from(&account_key);
        assert_eq!(
            *proto_credentials.view_private_key.as_ref().unwrap(),
            external::RistrettoPrivate::from(account_key.view_private_key())
        );
        assert_eq!(
            *proto_credentials.spend_private_key.as_ref().unwrap(),
            external::RistrettoPrivate::from(account_key.spend_private_key())
        );

        // external -> account_keys
        let account_key2 = AccountKey::try_from(&proto_credentials).unwrap();
        assert_eq!(account_key, account_key2);
    }
}
