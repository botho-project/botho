// Copyright (c) 2018-2025 The Botho Foundation

//! Convert to/from bth_transaction_builder::TxBlueprint.

use crate::{external, ConversionError};
use bth_transaction_builder::TxBlueprint;
use bth_transaction_core::{ring_ct::InputRing, tx::TxIn, Amount};
use std::convert::{TryFrom, TryInto};

impl From<&TxBlueprint> for external::TxBlueprint {
    fn from(source: &TxBlueprint) -> Self {
        Self {
            inputs: source.inputs.iter().map(|input| input.into()).collect(),
            rings: source.rings.iter().map(|ring| ring.into()).collect(),
            outputs: source.outputs.iter().map(|output| output.into()).collect(),
            fee: Some((&source.fee).into()),
            tombstone_block: source.tombstone_block,
            block_version: *source.block_version,
            cluster_tags: source.cluster_tags.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<&external::TxBlueprint> for TxBlueprint {
    type Error = ConversionError;

    fn try_from(source: &external::TxBlueprint) -> Result<Self, Self::Error> {
        let inputs: Vec<TxIn> = source
            .inputs
            .iter()
            .map(|proto_input| proto_input.try_into())
            .collect::<Result<_, _>>()?;
        let rings: Vec<InputRing> = source
            .rings
            .iter()
            .map(|proto_ring| proto_ring.try_into())
            .collect::<Result<_, _>>()?;
        let outputs = source
            .outputs
            .iter()
            .map(|proto_output| proto_output.try_into())
            .collect::<Result<_, _>>()?;
        let fee: Amount = source.fee.as_ref().unwrap_or(&Default::default()).into();
        let tombstone_block = source.tombstone_block;
        let block_version = source.block_version.try_into()?;

        let cluster_tags = source
            .cluster_tags
            .as_ref()
            .map(|ct| ct.try_into())
            .transpose()?;

        Ok(TxBlueprint {
            inputs,
            rings,
            outputs,
            fee,
            tombstone_block,
            block_version,
            cluster_tags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_account_keys::AccountKey;
    use bth_blockchain_types::BlockVersion;
    use bth_crypto_ring_signature_signer::NoKeysRingSigner;
    use bth_transaction_builder::{
        test_utils::get_input_credentials, EmptyMemoBuilder, ReservedSubaddresses,
        SignedContingentInputBuilder, TransactionBuilder,
    };
    use bth_transaction_core::{
        constants::MILLIBTH_TO_NANOBTH, tokens::Bth, Amount, Token, TokenId,
    };
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    fn test_tx_blueprint_conversion() {
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);
        let block_version = BlockVersion::MAX;

        let alice = AccountKey::random(&mut rng);
        let bob = AccountKey::random(&mut rng);
        let charlie = AccountKey::random(&mut rng);

        let token2 = TokenId::from(2);

        let input_credentials_sci = get_input_credentials(
            block_version,
            Amount::new(1000, token2),
            &charlie,
            &mut rng,
        );
        // Global indices for the ring members
        let ring_global_indices: Vec<u64> = (0..input_credentials_sci.ring.len() as u64).collect();
        let mut sci_builder = SignedContingentInputBuilder::new(
            block_version,
            input_credentials_sci.clone(),
            ring_global_indices,
            EmptyMemoBuilder,
        )
        .unwrap();
        sci_builder
            .add_required_output(
                Amount::new(1000 * MILLIBTH_TO_NANOBTH, Bth::ID),
                &charlie.default_subaddress(),
                &mut rng,
            )
            .unwrap();
        let sci = sci_builder.build(&NoKeysRingSigner {}, &mut rng).unwrap();

        let mut transaction_builder = TransactionBuilder::new(
            block_version,
            Amount::new(Bth::MINIMUM_FEE, Bth::ID),
        )
        .unwrap();
        transaction_builder.add_input(get_input_credentials(
            block_version,
            Amount::new(1475 * MILLIBTH_TO_NANOBTH, Bth::ID),
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
                Amount::new(475 * MILLIBTH_TO_NANOBTH - Bth::MINIMUM_FEE, Bth::ID),
                &ReservedSubaddresses::from(&alice),
                &mut rng,
            )
            .unwrap();

        let blueprint_orig = transaction_builder.build_blueprint().unwrap();

        let blueprint_proto: external::TxBlueprint = (&blueprint_orig).into();
        let blueprint_recovered: TxBlueprint = (&blueprint_proto).try_into().unwrap();

        assert_eq!(blueprint_orig, blueprint_recovered);
    }
}
