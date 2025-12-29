// Copyright (c) 2018-2022 The Botho Foundation

use crate::TxBuilderError;
use alloc::vec::Vec;
use bt_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bt_crypto_ring_signature_signer::{InputSecret, OneTimeKeyDeriveData, SignableInputRing};
use bt_transaction_core::{
    onetime_keys::create_shared_secret,
    tx::{TxIn, TxOut},
    TxOutConversionError,
};
use zeroize::Zeroize;

/// Credentials required to construct a ring signature for an input.
#[derive(Clone, Debug, Zeroize)]
#[zeroize(drop)]
pub struct InputCredentials {
    /// A "ring" containing "mixins" and the one "real" TxOut to be spent.
    pub ring: Vec<TxOut>,

    /// Index in `ring` of the "real" output being spent.
    pub real_index: usize,

    /// Secrets needed to spend the real output
    pub input_secret: InputSecret,
}

impl InputCredentials {
    /// Creates an InputCredential instance used to create and sign an Input.
    ///
    /// # Arguments
    /// * `ring` - A "ring" of transaction outputs.
    /// * `real_index` - Index in `ring` of the output being spent.
    /// * `onetime_key_derive_data` - Key derivation data for the output being
    ///   spent.
    /// * `view_private_key` - The view private key belonging to the owner of
    ///   the real output.
    pub fn new(
        ring: Vec<TxOut>,
        real_index: usize,
        onetime_key_derive_data: impl Into<OneTimeKeyDeriveData>,
        view_private_key: RistrettoPrivate,
    ) -> Result<Self, TxBuilderError> {
        if real_index >= ring.len() || ring.is_empty() {
            return Err(TxBuilderError::InvalidRingSize);
        }

        let real_input: TxOut = ring
            .get(real_index)
            .cloned()
            .ok_or(TxBuilderError::NoInputs)?;
        let real_output_public_key = RistrettoPublic::try_from(&real_input.public_key)?;

        // Note: The caller likely already has the shared secret if they already
        // unmasked this TxOut and are now trying to spend it, so as an
        // optimization we could avoid recomputing it.
        let tx_out_shared_secret = create_shared_secret(&real_output_public_key, &view_private_key);

        // Sort the ring by public key. This ensures that the ordering
        // of mixins in the transaction does not depend on the user's implementation for
        // obtaining mixins.
        let mut ring = ring;
        ring.sort_by(|a, b| a.public_key.cmp(&b.public_key));

        let real_index: usize = ring
            .iter()
            .position(|element| *element == real_input)
            .expect("Must still contain real input");

        let masked_amount = &ring[real_index].get_masked_amount()?;
        let (amount, blinding) = masked_amount.get_value(&tx_out_shared_secret)?;

        let onetime_key_derive_data = onetime_key_derive_data.into();
        let input_secret = InputSecret {
            onetime_key_derive_data,
            amount,
            blinding,
        };

        Ok(InputCredentials {
            ring,
            real_index,
            input_secret,
        })
    }

    /// Get the one-time private key from the InputCredentials, panicking if
    /// it doesn't contain this. This makes many tests much shorter.
    #[cfg(any(test, feature = "test-only"))]
    pub fn assert_has_onetime_private_key(&self) -> &RistrettoPrivate {
        match &self.input_secret.onetime_key_derive_data {
            OneTimeKeyDeriveData::OneTimeKey(key) => key,
            OneTimeKeyDeriveData::SubaddressIndex(_) => panic!("missing one time private key"),
        }
    }
}

impl TryFrom<InputCredentials> for SignableInputRing {
    type Error = TxOutConversionError;
    fn try_from(src: InputCredentials) -> Result<SignableInputRing, Self::Error> {
        Ok(SignableInputRing {
            members: src
                .ring
                .iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            real_input_index: src.real_index,
            input_secret: src.input_secret.clone(),
        })
    }
}

impl From<&InputCredentials> for TxIn {
    fn from(input_credential: &InputCredentials) -> TxIn {
        TxIn {
            ring: input_credential.ring.clone(),
            input_rules: None,
        }
    }
}
