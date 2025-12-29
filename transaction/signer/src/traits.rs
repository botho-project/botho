// Copyright (c) 2018-2022 The Botho Foundation

//! Traits for implementation by transaction-signer implementations,
//! these allow signing operations to be abstract over a transaction signer
//! implementation.

use core::{convert::Infallible, fmt::Debug};

use bth_core::{
    account::{Account, PublicSubaddress, ViewAccount},
    keys::TxOutPublic,
    subaddress::Subaddress,
};

use bth_crypto_ring_signature::{onetime_keys::recover_onetime_private_key, KeyImage};

/// View only account provider
pub trait ViewAccountProvider {
    /// [ViewAccountProvider] error
    type Error: Send + Sync + Debug;

    /// Fetch view account object
    fn account(&self) -> Result<ViewAccount, Self::Error>;
}

/// Basic view account provider for [Account] type
impl ViewAccountProvider for Account {
    type Error = Infallible;

    /// Fetch view account object
    fn account(&self) -> Result<ViewAccount, Self::Error> {
        Ok(ViewAccount::from(self))
    }
}

impl<T: ViewAccountProvider> ViewAccountProvider for &T {
    type Error = <T as ViewAccountProvider>::Error;

    fn account(&self) -> Result<ViewAccount, Self::Error> {
        <T as ViewAccountProvider>::account(self)
    }
}

/// Transaction key image computer
pub trait KeyImageComputer {
    /// [`KeyImageComputer`] error
    type Error: Send + Sync + Debug;

    /// Compute key image for a given subaddress index and tx_out_public_key
    fn compute_key_image(
        &self,
        subaddress_index: u64,
        tx_out_public_key: &TxOutPublic,
    ) -> Result<KeyImage, Self::Error>;
}

impl<T: KeyImageComputer> KeyImageComputer for &T {
    type Error = <T as KeyImageComputer>::Error;

    fn compute_key_image(
        &self,
        subaddress_index: u64,
        tx_out_public_key: &TxOutPublic,
    ) -> Result<KeyImage, Self::Error> {
        <T as KeyImageComputer>::compute_key_image(self, subaddress_index, tx_out_public_key)
    }
}

/// Basic [KeyImageComputer] implementation for [Account] type
impl KeyImageComputer for Account {
    type Error = Infallible;

    /// Compute key image for a given subaddress index and tx_out_public_key
    fn compute_key_image(
        &self,
        subaddress_index: u64,
        tx_out_public_key: &TxOutPublic,
    ) -> Result<KeyImage, Self::Error> {
        // Compute subaddress from index
        let subaddress = self.subaddress(subaddress_index);

        // Recover onetime private key
        let onetime_private_key = recover_onetime_private_key(
            tx_out_public_key.as_ref(),
            self.view_private_key().as_ref(),
            subaddress.spend_private_key().as_ref(),
        );

        // Generate key image
        Ok(KeyImage::from(&onetime_private_key))
    }
}

/// Memo signer for generating memo HMACs
pub trait MemoHmacSigner {
    /// [`MemoHmacSigner`] error
    type Error: Send + Sync + Debug;

    /// Compute the HMAC signature for the provided memo and target address
    fn compute_memo_hmac_sig(
        &self,
        sender_subaddress_index: u64,
        tx_public_key: &TxOutPublic,
        target_subaddress: PublicSubaddress,
        memo_type: &[u8; 2],
        memo_data_sans_hmac: &[u8; 48],
    ) -> Result<[u8; 16], Self::Error>;
}

/// Memo signer impl for reference types
impl<T: MemoHmacSigner> MemoHmacSigner for &T {
    type Error = <T as MemoHmacSigner>::Error;

    fn compute_memo_hmac_sig(
        &self,
        sender_subaddress_index: u64,
        tx_public_key: &TxOutPublic,
        target_subaddress: PublicSubaddress,
        memo_type: &[u8; 2],
        memo_data_sans_hmac: &[u8; 48],
    ) -> Result<[u8; 16], Self::Error> {
        <T as MemoHmacSigner>::compute_memo_hmac_sig(
            self,
            sender_subaddress_index,
            tx_public_key,
            target_subaddress,
            memo_type,
            memo_data_sans_hmac,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_account_keys::AccountKey;
    use bth_crypto_keys::RistrettoPublic;
    use bth_util_from_random::FromRandom;
    use rand::{rngs::StdRng, SeedableRng};

    fn create_test_account() -> Account {
        let mut rng = StdRng::from_seed([42u8; 32]);
        let account_key = AccountKey::random(&mut rng);
        Account::new(
            account_key.view_private_key().clone().into(),
            account_key.spend_private_key().clone().into(),
        )
    }

    fn create_test_tx_out_public_key(rng: &mut (impl rand::RngCore + rand::CryptoRng)) -> TxOutPublic {
        let public = RistrettoPublic::from_random(rng);
        TxOutPublic::from(public)
    }

    #[test]
    fn view_account_provider_for_account() {
        let account = create_test_account();
        let view_account = account.account().unwrap();

        // Verify view account has same view private key
        assert_eq!(
            view_account.view_private_key().to_bytes(),
            account.view_private_key().to_bytes()
        );
    }

    #[test]
    fn view_account_provider_for_ref() {
        let account = create_test_account();
        let account_ref = &account;
        let view_account = account_ref.account().unwrap();

        assert_eq!(
            view_account.view_private_key().to_bytes(),
            account.view_private_key().to_bytes()
        );
    }

    #[test]
    fn key_image_computer_for_account() {
        let account = create_test_account();
        let mut rng = StdRng::from_seed([43u8; 32]);
        let tx_out_public_key = create_test_tx_out_public_key(&mut rng);

        // This should compute a key image without panicking
        let result = account.compute_key_image(0, &tx_out_public_key);
        assert!(result.is_ok());
    }

    #[test]
    fn key_image_computer_for_ref() {
        let account = create_test_account();
        let account_ref = &account;
        let mut rng = StdRng::from_seed([44u8; 32]);
        let tx_out_public_key = create_test_tx_out_public_key(&mut rng);

        let result = account_ref.compute_key_image(0, &tx_out_public_key);
        assert!(result.is_ok());
    }

    #[test]
    fn key_image_deterministic() {
        let account = create_test_account();
        let mut rng = StdRng::from_seed([45u8; 32]);
        let tx_out_public_key = create_test_tx_out_public_key(&mut rng);

        // Same inputs should produce same key image
        let ki1 = account.compute_key_image(0, &tx_out_public_key).unwrap();
        let ki2 = account.compute_key_image(0, &tx_out_public_key).unwrap();
        assert_eq!(ki1, ki2);
    }

    #[test]
    fn key_image_different_subaddresses() {
        let account = create_test_account();
        let mut rng = StdRng::from_seed([46u8; 32]);
        let tx_out_public_key = create_test_tx_out_public_key(&mut rng);

        // Different subaddresses should produce different key images
        let ki1 = account.compute_key_image(0, &tx_out_public_key).unwrap();
        let ki2 = account.compute_key_image(1, &tx_out_public_key).unwrap();
        assert_ne!(ki1, ki2);
    }
}
