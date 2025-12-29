// Copyright (c) 2018-2022 The Botho Foundation

//! Convert to/from external:MintTx/MintTxPrefix.

use crate::{external, ConversionError};
use bth_transaction_core::mint::{MintTx, MintTxPrefix};

/// Convert MintTxPrefix --> external::MintTxPrefix.
impl From<&MintTxPrefix> for external::MintTxPrefix {
    fn from(src: &MintTxPrefix) -> Self {
        Self {
            token_id: src.token_id,
            amount: src.amount,
            view_public_key: Some((&src.view_public_key).into()),
            spend_public_key: Some((&src.spend_public_key).into()),
            nonce: src.nonce.clone(),
            tombstone_block: src.tombstone_block,
        }
    }
}

/// Convert external::MintTxPrefix --> MintTxPrefix.
impl TryFrom<&external::MintTxPrefix> for MintTxPrefix {
    type Error = ConversionError;

    fn try_from(source: &external::MintTxPrefix) -> Result<Self, Self::Error> {
        let view_public_key = source
            .view_public_key
            .as_ref()
            .unwrap_or(&Default::default())
            .try_into()?;
        let spend_public_key = source
            .spend_public_key
            .as_ref()
            .unwrap_or(&Default::default())
            .try_into()?;
        // Note: e_fog_hint is ignored - fog support removed

        Ok(Self {
            token_id: source.token_id,
            amount: source.amount,
            view_public_key,
            spend_public_key,
            nonce: source.nonce.clone(),
            tombstone_block: source.tombstone_block,
        })
    }
}

/// Convert MintTx --> external::MintTx.
impl From<&MintTx> for external::MintTx {
    fn from(src: &MintTx) -> Self {
        Self {
            prefix: Some((&src.prefix).into()),
            signature: Some((&src.signature).into()),
        }
    }
}

/// Convert external::MintTx --> MintTx.
impl TryFrom<&external::MintTx> for MintTx {
    type Error = ConversionError;

    fn try_from(source: &external::MintTx) -> Result<Self, Self::Error> {
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

        Ok(Self { prefix, signature })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::ed25519_multisig::tests::test_multi_sig;
    use bth_crypto_keys::RistrettoPublic;
    use bth_util_from_random::FromRandom;
    use bth_util_serial::{decode, encode};
    use prost::Message;
    use rand_core::{RngCore, SeedableRng};
    use rand_hc::Hc128Rng;

    #[test]
    // MintTx -> external::MintTx -> MintTx should be the identity
    // function.
    fn test_convert_mint_tx() {
        let mut rng = Hc128Rng::from_seed([1u8; 32]);

        let source = MintTx {
            prefix: MintTxPrefix {
                token_id: rng.next_u64(),
                amount: rng.next_u64(),
                view_public_key: RistrettoPublic::from_random(&mut rng),
                spend_public_key: RistrettoPublic::from_random(&mut rng),
                nonce: vec![3u8; 32],
                tombstone_block: rng.next_u64(),
            },
            signature: test_multi_sig(),
        };

        // decode(encode(source)) should be the identity function.
        {
            let bytes = encode(&source);
            let recovered = decode(&bytes).unwrap();
            assert_eq!(source, recovered);
        }

        // Converting bth_transaction_core::mint::MintTx -> external::MintTx ->
        // bth_transaction_core::mint::MintTx should be the identity function.
        {
            let external = external::MintTx::from(&source);
            let recovered = MintTx::try_from(&external).unwrap();
            assert_eq!(source, recovered);
        }

        // Encoding with prost, decoding with protobuf should be the identity
        // function.
        {
            let bytes = encode(&source);
            let recovered = external::MintTx::decode(bytes.as_slice()).unwrap();
            assert_eq!(recovered, external::MintTx::from(&source));
        }

        // Encoding with protobuf, decoding with prost should be the identity function.
        {
            let external = external::MintTx::from(&source);
            let bytes = external.encode_to_vec();
            let recovered: MintTx = decode(&bytes).unwrap();
            assert_eq!(source, recovered);
        }
    }
}
