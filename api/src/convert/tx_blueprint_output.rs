// Copyright (c) 2018-2025 The Botho Foundation

//! Convert to/from bth_transaction_builder::TxBlueprintOutput.

use crate::{external, ConversionError};
use bth_transaction_builder::TxBlueprintOutput;

impl From<&TxBlueprintOutput> for external::TxBlueprintOutput {
    fn from(source: &TxBlueprintOutput) -> Self {
        match source {
            TxBlueprintOutput::Recipient {
                recipient,
                amount,
                tx_private_key,
            } => {
                let proto_recipient = external::TxBlueprintOutputRecipient {
                    recipient: Some(recipient.into()),
                    amount: Some(amount.into()),
                    tx_private_key: Some(tx_private_key.into()),
                };
                external::TxBlueprintOutput {
                    output: Some(external::tx_blueprint_output::Output::Recipient(
                        proto_recipient,
                    )),
                }
            }

            TxBlueprintOutput::Change {
                change_destination,
                amount,
                tx_private_key,
            } => {
                let proto_change = external::TxBlueprintOutputChange {
                    change_destination: Some(change_destination.into()),
                    amount: Some(amount.into()),
                    tx_private_key: Some(tx_private_key.into()),
                };
                external::TxBlueprintOutput {
                    output: Some(external::tx_blueprint_output::Output::Change(proto_change)),
                }
            }

            TxBlueprintOutput::Sci {
                output,
                unmasked_amount,
            } => {
                let proto_sci = external::TxBlueprintOutputSci {
                    output: Some(output.into()),
                    unmasked_amount: Some(unmasked_amount.into()),
                };
                external::TxBlueprintOutput {
                    output: Some(external::tx_blueprint_output::Output::Sci(proto_sci)),
                }
            }
        }
    }
}

impl TryFrom<&external::TxBlueprintOutput> for TxBlueprintOutput {
    type Error = ConversionError;

    fn try_from(source: &external::TxBlueprintOutput) -> Result<Self, Self::Error> {
        match source
            .output
            .as_ref()
            .ok_or_else(|| ConversionError::MissingField("output".to_string()))?
        {
            external::tx_blueprint_output::Output::Recipient(proto_recipient) => {
                let recipient = proto_recipient
                    .recipient
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("recipient".to_string()))?
                    .try_into()?;
                let amount = proto_recipient
                    .amount
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("amount".to_string()))?
                    .into();
                // Note: e_fog_hint is ignored - fog support removed
                let tx_private_key = proto_recipient
                    .tx_private_key
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("tx_private_key".to_string()))?
                    .try_into()?;
                Ok(TxBlueprintOutput::Recipient {
                    recipient,
                    amount,
                    tx_private_key,
                })
            }
            external::tx_blueprint_output::Output::Change(proto_change) => {
                let change_destination = proto_change
                    .change_destination
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("change_destination".to_string()))?
                    .try_into()?;
                let amount = proto_change
                    .amount
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("amount".to_string()))?
                    .into();
                // Note: e_fog_hint is ignored - fog support removed
                let tx_private_key = proto_change
                    .tx_private_key
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("tx_private_key".to_string()))?
                    .try_into()?;
                Ok(TxBlueprintOutput::Change {
                    change_destination,
                    amount,
                    tx_private_key,
                })
            }
            external::tx_blueprint_output::Output::Sci(proto_sci) => {
                let output = proto_sci
                    .output
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("output".to_string()))?
                    .try_into()?;
                let unmasked_amount = proto_sci
                    .unmasked_amount
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField("unmasked_amount".to_string()))?
                    .try_into()?;
                Ok(TxBlueprintOutput::Sci {
                    output,
                    unmasked_amount,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_account_keys::{AccountKey, PublicAddress};
    use bth_crypto_keys::RistrettoPrivate;
    use bth_transaction_core::{
        ring_signature::CurveScalar, tokens::Mob, tx::TxOut, Amount, BlockVersion, Token,
        UnmaskedAmount,
    };
    use bth_transaction_extra::ReservedSubaddresses;
    use bth_util_from_random::FromRandom;
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    fn test_tx_blueprint_output_conversion() {
        let mut rng: StdRng = SeedableRng::from_seed([4u8; 32]);
        let block_version = BlockVersion::MAX;

        let recipient_orig = {
            let recipient = PublicAddress::from_random(&mut rng);
            let amount = Amount::new(1000, Mob::ID);
            let tx_private_key = RistrettoPrivate::from_random(&mut rng);
            TxBlueprintOutput::Recipient {
                recipient,
                amount,
                tx_private_key,
            }
        };
        let recipient_proto: external::TxBlueprintOutput = (&recipient_orig).into();
        let recipient_recovered: TxBlueprintOutput = (&recipient_proto).try_into().unwrap();
        assert_eq!(recipient_orig, recipient_recovered);

        let change_orig = {
            let account_key = AccountKey::random(&mut rng);
            let change_destination = ReservedSubaddresses::from(&account_key);
            let amount = Amount::new(500, Mob::ID);
            let tx_private_key = RistrettoPrivate::from_random(&mut rng);
            TxBlueprintOutput::Change {
                change_destination,
                amount,
                tx_private_key,
            }
        };
        let change_proto: external::TxBlueprintOutput = (&change_orig).into();
        let change_recovered: TxBlueprintOutput = (&change_proto).try_into().unwrap();
        assert_eq!(change_orig, change_recovered);

        let sci_orig = {
            let recipient = PublicAddress::from_random(&mut rng);
            let amount = Amount::new(100, Mob::ID);
            let tx_private_key = RistrettoPrivate::from_random(&mut rng);
            let output = TxOut::new(block_version, amount, &recipient, &tx_private_key).unwrap();
            let unmasked_amount = UnmaskedAmount {
                value: amount.value,
                token_id: *amount.token_id,
                blinding: CurveScalar::from_random(&mut rng),
            };
            TxBlueprintOutput::Sci {
                output,
                unmasked_amount,
            }
        };
        let sci_proto: external::TxBlueprintOutput = (&sci_orig).into();
        let sci_recovered: TxBlueprintOutput = (&sci_proto).try_into().unwrap();
        assert_eq!(sci_orig, sci_recovered);
    }
}
