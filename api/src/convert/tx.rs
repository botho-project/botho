// Copyright (c) 2018-2022 The MobileCoin Foundation

//! Convert to/from external::Tx.

use crate::{external, ConversionError};
use mc_transaction_core::tx;

/// Convert mc_transaction_core::tx::Tx --> external::Tx.
impl From<&tx::Tx> for external::Tx {
    fn from(source: &tx::Tx) -> Self {
        Self {
            prefix: Some((&source.prefix).into()),
            signature: Some((&source.signature).into()),
        }
    }
}

/// Convert external::Tx --> mc_transaction_core::tx::Tx.
impl TryFrom<&external::Tx> for tx::Tx {
    type Error = ConversionError;

    fn try_from(source: &external::Tx) -> Result<Self, Self::Error> {
        let prefix = source
            .prefix
            .as_ref()
            .unwrap_or(&Default::default())
            .try_into()?;
        let signature = source
            .signature
            .as_ref()
            .unwrap_or(&Default::default())
            .try_into()?;
        Ok(tx::Tx { prefix, signature })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_account_keys::AccountKey;
    use mc_crypto_ring_signature_signer::NoKeysRingSigner;
    use mc_transaction_builder::{
        test_utils::{get_input_credentials, get_ring_global_indices},
        EmptyMemoBuilder, ReservedSubaddresses, SignedContingentInputBuilder, TransactionBuilder,
    };
    use mc_transaction_core::{
        constants::{MILLIMOB_TO_PICOMOB, RING_SIZE},
        tokens::Mob,
        tx::Tx,
        Amount, BlockVersion, Token, TokenId,
    };
    use prost::Message;
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    /// Tx --> externalTx --> Tx should be the identity function, for simple tx
    fn test_convert_tx() {
        // Generate a Tx to test with. This is copied from
        // transaction_builder.rs::test_simple_transaction
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);

        for block_version in BlockVersion::iterator() {
            let alice = AccountKey::random(&mut rng);
            let bob = AccountKey::random(&mut rng);

            let mut transaction_builder =
                TransactionBuilder::new(block_version, Amount::new(Mob::MINIMUM_FEE, Mob::ID))
                    .unwrap();

            transaction_builder.add_input(get_input_credentials(
                block_version,
                Amount::new(65536 + Mob::MINIMUM_FEE, Mob::ID),
                &alice,
                &mut rng,
            ));
            transaction_builder
                .add_output(
                    Amount::new(65536, Mob::ID),
                    &bob.default_subaddress(),
                    &mut rng,
                )
                .unwrap();

            let tx = transaction_builder
                .build(&NoKeysRingSigner {}, EmptyMemoBuilder, &mut rng)
                .unwrap();

            // decode(encode(tx)) should be the identity function.
            {
                let bytes = mc_util_serial::encode(&tx);
                let recovered_tx = mc_util_serial::decode(&bytes).unwrap();
                assert_eq!(tx, recovered_tx);
            }

            // Converting mc_transaction_core::Tx -> external::Tx -> mc_transaction_core::Tx
            // should be the identity function.
            {
                let external_tx: external::Tx = external::Tx::from(&tx);
                let recovered_tx: Tx = Tx::try_from(&external_tx).unwrap();
                assert_eq!(tx, recovered_tx);
            }

            // Encoding with prost, decoding with protobuf should be the identity function.
            {
                let bytes = mc_util_serial::encode(&tx);
                let recovered_tx = external::Tx::decode(bytes.as_slice()).unwrap();
                assert_eq!(recovered_tx, external::Tx::from(&tx));
            }

            // Encoding with protobuf, decoding with prost should be the identity function.
            {
                let external_tx: external::Tx = external::Tx::from(&tx);
                let bytes = external_tx.encode_to_vec();
                let recovered_tx: Tx = mc_util_serial::decode(&bytes).unwrap();
                assert_eq!(tx, recovered_tx);
            }
        }
    }

    #[test]
    /// Tx --> externalTx --> Tx should be the identity function, for tx with
    /// input rules
    fn test_convert_tx_with_input_rules() {
        // Generate a Tx to test with. This is copied from
        // transaction_builder.rs::test_simple_transaction
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);

        for block_version in BlockVersion::iterator().skip(3) {
            let alice = AccountKey::random(&mut rng);
            let bob = AccountKey::random(&mut rng);
            let charlie = AccountKey::random(&mut rng);

            let token2 = TokenId::from(2);

            // Charlie makes a signed contingent input, offering 1000 token2's for 1 MOB
            let input_credentials = get_input_credentials(
                block_version,
                Amount::new(1000, token2),
                &charlie,
                &mut rng,
            );
            let ring_global_indices = get_ring_global_indices(RING_SIZE);
            let mut sci_builder = SignedContingentInputBuilder::new(
                block_version,
                input_credentials,
                ring_global_indices,
                EmptyMemoBuilder,
            )
            .unwrap();

            sci_builder
                .add_required_output(
                    Amount::new(1000 * MILLIMOB_TO_PICOMOB, Mob::ID),
                    &charlie.default_subaddress(),
                    &mut rng,
                )
                .unwrap();

            let sci = sci_builder.build(&NoKeysRingSigner {}, &mut rng).unwrap();

            // Alice sends this token2 amount to Bob from Charlie, paying Charlie 1 MOB
            // as he desires, and returning .475 MOB as change to herself.
            let mut transaction_builder =
                TransactionBuilder::new(block_version, Amount::new(Mob::MINIMUM_FEE, Mob::ID))
                    .unwrap();

            transaction_builder.add_input(get_input_credentials(
                block_version,
                Amount::new(1475 * MILLIMOB_TO_PICOMOB, Mob::ID),
                &alice,
                &mut rng,
            ));
            transaction_builder.add_presigned_input(sci).unwrap();

            transaction_builder
                .add_output(
                    Amount::new(1000, token2),
                    &bob.default_subaddress(),
                    &mut rng,
                )
                .unwrap();

            transaction_builder
                .add_change_output(
                    Amount::new(475 * MILLIMOB_TO_PICOMOB - Mob::MINIMUM_FEE, Mob::ID),
                    &ReservedSubaddresses::from(&alice),
                    &mut rng,
                )
                .unwrap();

            let tx = transaction_builder
                .build(&NoKeysRingSigner {}, EmptyMemoBuilder, &mut rng)
                .unwrap();

            // decode(encode(tx)) should be the identity function.
            {
                let bytes = mc_util_serial::encode(&tx);
                let recovered_tx = mc_util_serial::decode(&bytes).unwrap();
                assert_eq!(tx, recovered_tx);
            }

            // Converting mc_transaction_core::Tx -> external::Tx -> mc_transaction_core::Tx
            // should be the identity function.
            {
                let external_tx: external::Tx = external::Tx::from(&tx);
                let recovered_tx: Tx = Tx::try_from(&external_tx).unwrap();
                assert_eq!(tx, recovered_tx);
            }

            // Encoding with prost, decoding with protobuf should be the identity function.
            {
                let bytes = mc_util_serial::encode(&tx);
                let recovered_tx = external::Tx::decode(bytes.as_slice()).unwrap();
                assert_eq!(recovered_tx, external::Tx::from(&tx));
            }

            // Encoding with protobuf, decoding with prost should be the identity function.
            {
                let external_tx: external::Tx = external::Tx::from(&tx);
                let bytes = external_tx.encode_to_vec();
                let recovered_tx: Tx = mc_util_serial::decode(&bytes).unwrap();
                assert_eq!(tx, recovered_tx);
            }
        }
    }

    #[test]
    /// Tx --> externalTx --> Tx should be the identity function, for tx with
    /// partial fill input rules
    fn test_convert_tx_with_partial_fill_input_rules() {
        // Generate a Tx to test with. This is copied from
        // transaction_builder.rs::test_simple_transaction
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);

        for block_version in BlockVersion::iterator().skip(3) {
            let alice = AccountKey::random(&mut rng);
            let bob = AccountKey::random(&mut rng);
            let charlie = AccountKey::random(&mut rng);

            let token2 = TokenId::from(2);

            // Charlie makes a signed contingent input, offering 1000 token2's for 1 MOB
            let input_credentials = get_input_credentials(
                block_version,
                Amount::new(1000, token2),
                &charlie,
                &mut rng,
            );
            let ring_global_indices = get_ring_global_indices(RING_SIZE);
            let mut sci_builder = SignedContingentInputBuilder::new(
                block_version,
                input_credentials,
                ring_global_indices,
                EmptyMemoBuilder,
            )
            .unwrap();

            // Originator requests an output worth 1MOB destined to themselves
            sci_builder
                .add_partial_fill_output(
                    Amount::new(1000 * MILLIMOB_TO_PICOMOB, Mob::ID),
                    &charlie.default_subaddress(),
                    &mut rng,
                )
                .unwrap();

            // Change amount matches the input value
            sci_builder
                .add_partial_fill_change_output(
                    Amount::new(1000, token2),
                    &ReservedSubaddresses::from(&charlie),
                    &mut rng,
                )
                .unwrap();
            let sci = sci_builder.build(&NoKeysRingSigner {}, &mut rng).unwrap();

            // Alice sends 250 token2 to Bob from Charlie, paying Charlie .25 MOB
            // as he desires, and returning .475 MOB as change to herself.
            let mut transaction_builder =
                TransactionBuilder::new(block_version, Amount::new(Mob::MINIMUM_FEE, Mob::ID))
                    .unwrap();

            transaction_builder.add_input(get_input_credentials(
                block_version,
                Amount::new(1475 * MILLIMOB_TO_PICOMOB, Mob::ID),
                &alice,
                &mut rng,
            ));
            transaction_builder
                .add_presigned_partial_fill_input(sci, Amount::new(750, token2))
                .unwrap();

            transaction_builder
                .add_output(
                    Amount::new(250, token2),
                    &bob.default_subaddress(),
                    &mut rng,
                )
                .unwrap();

            transaction_builder
                .add_change_output(
                    Amount::new(
                        (475 + (1000 - 250)) * MILLIMOB_TO_PICOMOB - Mob::MINIMUM_FEE,
                        Mob::ID,
                    ),
                    &ReservedSubaddresses::from(&alice),
                    &mut rng,
                )
                .unwrap();

            let tx = transaction_builder
                .build(&NoKeysRingSigner {}, EmptyMemoBuilder, &mut rng)
                .unwrap();

            // decode(encode(tx)) should be the identity function.
            {
                let bytes = mc_util_serial::encode(&tx);
                let recovered_tx = mc_util_serial::decode(&bytes).unwrap();
                assert_eq!(tx, recovered_tx);
            }

            // Converting mc_transaction_core::Tx -> external::Tx -> mc_transaction_core::Tx
            // should be the identity function.
            {
                let external_tx: external::Tx = external::Tx::from(&tx);
                let recovered_tx: Tx = Tx::try_from(&external_tx).unwrap();
                assert_eq!(tx, recovered_tx);
            }

            // Encoding with prost, decoding with protobuf should be the identity function.
            {
                let bytes = mc_util_serial::encode(&tx);
                let recovered_tx = external::Tx::decode(bytes.as_slice()).unwrap();
                assert_eq!(recovered_tx, external::Tx::from(&tx));
            }

            // Encoding with protobuf, decoding with prost should be the identity function.
            {
                let external_tx: external::Tx = external::Tx::from(&tx);
                let bytes = external_tx.encode_to_vec();
                let recovered_tx: Tx = mc_util_serial::decode(&bytes).unwrap();
                assert_eq!(tx, recovered_tx);
            }
        }
    }
}
