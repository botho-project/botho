// Copyright (c) 2018-2024 The Botho Foundation

//! Utility for building and signing a transaction.
//!
//! See https://cryptonote.org/img/cryptonote_transaction.png

use crate::{
    input_materials::InputMaterials, InputCredentials, MemoBuilder, ReservedSubaddresses,
    TxBlueprint, TxBlueprintOutput, TxBuilderError,
};
use alloc::vec::Vec;
use core::{
    cmp::Ordering,
    fmt::Debug,
};
use bth_account_keys::PublicAddress;
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature_signer::RingSigner;
use bth_transaction_core::{
    onetime_keys::{create_shared_secret, create_tx_out_public_key},
    ring_ct::{InputRing, OutputSecret},
    ring_signature::Scalar,
    tokens::Mob,
    tx::{Tx, TxIn, TxOut},
    Amount, BlockVersion, ClusterTagVector, FeeMap, MemoContext, MemoPayload, NewMemoError,
    RevealedTxOut, RevealedTxOutError, Token, TokenId,
};
use bth_transaction_extra::{
    SignedContingentInput, SignedContingentInputError, TxOutConfirmationNumber, UnsignedTx,
};
use bth_util_from_random::FromRandom;
use bth_util_u64_ratio::U64Ratio;
use rand_core::{CryptoRng, RngCore};

/// A trait used to compare the transaction outputs
pub trait TxOutputsOrdering {
    /// comparer method
    fn cmp(a: &CompressedRistrettoPublic, b: &CompressedRistrettoPublic) -> Ordering;
}

/// Default implementation for transaction outputs
pub struct DefaultTxOutputsOrdering;

impl TxOutputsOrdering for DefaultTxOutputsOrdering {
    fn cmp(a: &CompressedRistrettoPublic, b: &CompressedRistrettoPublic) -> Ordering {
        a.cmp(b)
    }
}

/// Transaction output context is produced by add_output method
/// Used for receipt creation
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TxOutContext {
    /// TxOut public key that comes from a transaction builder
    /// add_output/add_change_output
    pub tx_out_public_key: CompressedRistrettoPublic,
    /// confirmation that comes from a transaction builder
    /// add_output/add_change_output
    pub confirmation: TxOutConfirmationNumber,
    /// Shared Secret that comes from a transaction builder
    /// add_output/add_change_output
    pub shared_secret: RistrettoPublic,
}

/// Helper utility for building and signing a CryptoNote-style transaction,
/// and attaching memos as appropriate.
#[derive(Clone, Debug)]
pub struct TransactionBuilder {
    /// The block version that we are targeting for this transaction
    block_version: BlockVersion,
    /// The input materials used to form the transaction.
    input_materials: Vec<InputMaterials>,
    /// The outputs that we will produce when going from a `TxBlueprint` to an
    /// `UnsignedTx`.
    outputs: Vec<TxBlueprintOutput>,
    /// The tombstone_block value, a block index in which the transaction
    /// expires, and can no longer be added to the blockchain
    tombstone_block: u64,
    /// The fee paid in connection to this transaction
    /// If mixed transactions feature is off, then everything must be this token
    /// id.
    fee: Amount,
    /// The minimum fee map, if available.
    fee_map: Option<FeeMap>,
    /// Cluster tag decay rate for inheritance computation.
    /// Expressed as parts per TAG_WEIGHT_SCALE (1_000_000 = 100% decay).
    /// Default is 0 (no decay).
    cluster_tag_decay_rate: u32,
}

impl TransactionBuilder {
    /// Initializes a new TransactionBuilder.
    ///
    /// # Arguments
    /// * `block_version` - The block version rules to use when building this
    ///   transaction
    /// * `fee` - The fee (and token id) to use for this transaction. Note: The
    ///   fee token id cannot be changed later, and before mixed transactions
    ///   feature, every input and output must have the same token id as the
    ///   fee.
    pub fn new(
        block_version: BlockVersion,
        fee: Amount,
    ) -> Result<Self, TxBuilderError> {
        Ok(Self {
            block_version,
            fee,
            fee_map: None,
            input_materials: Vec::new(),
            outputs: Vec::new(),
            tombstone_block: u64::MAX,
            cluster_tag_decay_rate: 0,
        })
    }

    /// Add an Input to the transaction.
    ///
    /// # Arguments
    /// * `input_credentials` - Credentials required to construct a ring
    ///   signature for an input.
    pub fn add_input(&mut self, input_credentials: InputCredentials) {
        self.input_materials
            .push(InputMaterials::Signable(input_credentials));
    }

    /// Add a pre-signed Input to the transaction, also fulfilling any
    /// requirements imposed by the signed rules, so that our transaction
    /// will be valid.
    ///
    /// Note: Before adding a signed_contingent_input, you probably want to:
    /// * validate it (call .validate())
    /// * check if key image appeared already (call .key_image())
    /// * provide merkle proofs of membership for each ring member (see
    ///   .tx_out_global_indices)
    ///
    /// # Arguments
    /// * `signed_contingent_input` - The pre-signed input we are adding
    pub fn add_presigned_input(
        &mut self,
        sci: SignedContingentInput,
    ) -> Result<(), SignedContingentInputError> {
        if let Some(rules) = sci.tx_in.input_rules.as_ref() {
            // Check for any partial fill elements. These cannot be used with this API,
            // the caller must use add_presigned_partial_fill_input instead.
            if rules.partial_fill_change.is_some()
                || !rules.partial_fill_outputs.is_empty()
                || rules.min_partial_fill_value != 0
            {
                return Err(SignedContingentInputError::PartialFillInputNotAllowedHere);
            }
        }

        self.add_presigned_input_helper(sci)
    }

    /// Add a pre-signed Input with partial fill rules to the transaction, also
    /// fulfilling any requirements imposed by the signed rules, so that our
    /// transaction will be valid.
    ///
    /// Note: Before adding a signed_contingent_input, you probably want to:
    /// * validate it (call .validate())
    /// * check if key image appeared already (call .key_image())
    /// * provide merkle proofs of membership for each ring member (see
    ///   .tx_out_global_indices)
    ///
    /// # Arguments
    /// * `signed_contingent_input` - The pre-signed input we are adding
    /// * `sci_change_amount` - The amount of value of the SCI which we are
    ///   returning to the signer. This determines the "fill fraction" for the
    ///   partial-fill rules. This will be equal to the
    ///   real_change_output_amount.
    ///
    /// # Returns
    /// * A list of all outlay amounts deduced to fulfill the fractional output
    ///   rules, in the cheapest way possible.
    pub fn add_presigned_partial_fill_input(
        &mut self,
        sci: SignedContingentInput,
        sci_change_amount: Amount,
    ) -> Result<Vec<Amount>, SignedContingentInputError> {
        if !self.block_version.masked_amount_v2_is_supported() {
            return Err(
                SignedContingentInputError::FeatureNotSupportedAtBlockVersion(
                    *self.block_version,
                    "partial fills".into(),
                ),
            );
        }
        // Note: Botho does not use membership proofs
        let rules = sci
            .tx_in
            .input_rules
            .as_ref()
            .ok_or(SignedContingentInputError::MissingRules)?;
        let partial_fill_change = rules
            .partial_fill_change
            .as_ref()
            .ok_or(SignedContingentInputError::MissingPartialFillChange)?;
        let (partial_fill_change_amount, partial_fill_change_blinding) =
            partial_fill_change.reveal_amount()?;
        if partial_fill_change_amount.value == 0 {
            return Err(SignedContingentInputError::ZeroPartialFillChange);
        }
        if rules.min_partial_fill_value > partial_fill_change_amount.value {
            return Err(SignedContingentInputError::MinPartialFillValueExceedsPartialChange);
        }
        if partial_fill_change_amount.token_id != sci_change_amount.token_id {
            return Err(SignedContingentInputError::TokenIdMismatch);
        }
        // Check if the user-provided amont of change would violate the
        // min_partial_fill_value rule imposed by the originator. (This is the
        // same check performed by input rules validation.)
        if partial_fill_change_amount.value - rules.min_partial_fill_value < sci_change_amount.value
        {
            return Err(SignedContingentInputError::ChangeLimitExceeded);
        }

        let fill_fraction = U64Ratio::new(
            partial_fill_change_amount.value - sci_change_amount.value,
            partial_fill_change_amount.value,
        )
        .ok_or(SignedContingentInputError::ZeroPartialFillChange)?;

        // Ensure that we can reveal all amounts
        let partial_fill_outputs_and_amounts: Vec<(&RevealedTxOut, Amount, Scalar)> = rules
            .partial_fill_outputs
            .iter()
            .map(
                |r_tx_out| -> Result<(&RevealedTxOut, Amount, Scalar), RevealedTxOutError> {
                    let (amount, blinding) = r_tx_out.reveal_amount()?;
                    Ok((r_tx_out, amount, blinding))
                },
            )
            .collect::<Result<_, _>>()?;

        // Add fractional outputs (corresponding to partial fill outputs) into the list
        // which is added to tx prefix
        let fractional_amounts = partial_fill_outputs_and_amounts
            .into_iter()
            .map(
                |(r_tx_out, amount, blinding)| -> Result<Amount, RevealedTxOutError> {
                    let fractional_amount = Amount::new(
                        fill_fraction
                            .checked_mul_round_up(amount.value)
                            .expect("should be unreachable, because fill fraction is <= 1"),
                        amount.token_id,
                    );
                    let fractional_tx_out = r_tx_out.change_committed_amount(fractional_amount)?;

                    // Note: The blinding factor has to be the same as the blinding factor of the
                    // partial fill output that this tx out came from. This
                    // invariant of .change_committed_amount is checked by a debug assertion.
                    let output_secret = OutputSecret {
                        amount: fractional_amount,
                        blinding,
                    };

                    self.outputs.push(TxBlueprintOutput::Sci {
                        output: fractional_tx_out,
                        unmasked_amount: output_secret.into(),
                    });

                    Ok(fractional_amount)
                },
            )
            .collect::<Result<Vec<_>, _>>()?;

        // Add the fractional change output
        {
            let fractional_change =
                partial_fill_change.change_committed_amount(sci_change_amount)?;

            // Note: The blinding factor has to be the same as the blinding factor of the
            // partial fill change that this txout came from. This invariant of
            // .change_committed_amount is checked by a debug assertion.
            let output_secret = OutputSecret {
                amount: sci_change_amount,
                blinding: partial_fill_change_blinding,
            };

            self.outputs.push(TxBlueprintOutput::Sci {
                output: fractional_change,
                unmasked_amount: output_secret.into(),
            });
        }

        self.add_presigned_input_helper(sci)?;
        Ok(fractional_amounts)
    }

    /// Add a pre-signed input to the transaction, fulfilling the MCIP 31 rules
    /// (but not checking any of the MCIP 42 rules).
    /// This is extracted to reduce code duplication between add_presigned_input
    /// and add_presigned_partial_fill_input.
    fn add_presigned_input_helper(
        &mut self,
        sci: SignedContingentInput,
    ) -> Result<(), SignedContingentInputError> {
        if *self.block_version != sci.block_version {
            return Err(SignedContingentInputError::BlockVersionMismatch(
                *self.block_version,
                sci.block_version,
            ));
        }
        // Note: Botho does not use membership proofs

        let rules = sci
            .tx_in
            .input_rules
            .as_ref()
            .ok_or(SignedContingentInputError::MissingRules)?;

        // Enforce all non-partial fill rules so that our transaction will be valid
        if rules.required_outputs.len() != sci.required_output_amounts.len() {
            return Err(SignedContingentInputError::WrongNumberOfRequiredOutputAmounts);
        }
        // 1. Required outputs
        for (required_output, unmasked_amount) in rules
            .required_outputs
            .iter()
            .zip(sci.required_output_amounts.iter())
        {
            // Check if the required output is already there
            if !self
                .outputs
                .iter()
                .any(|output| matches!(output, TxBlueprintOutput::Sci { output: tx_out, .. } if tx_out == required_output))
            {
                // If not, add it
                self.outputs
                    .push(TxBlueprintOutput::Sci {
                        output: required_output.clone(),
                        unmasked_amount: unmasked_amount.clone(),
                    });
            }
        }
        // 2. Max tombstone block
        if rules.max_tombstone_block != 0 {
            self.set_tombstone_block(rules.max_tombstone_block);
        }

        // Don't do anything about partial fill rules, caller was supposed to do
        // that if they are present.
        // Now just add the sci to the list of inputs.
        self.add_presigned_input_raw(sci);
        Ok(())
    }

    /// Add a pre-signed Input to the transaction, without also fulfilling
    /// any of its rules. You will have to add any required outputs, adjust
    /// tombstone block, etc., for the transaction to be valid.
    ///
    /// Note: Before adding a signed_contingent_input, you probably want to:
    /// * validate it (call .validate())
    /// * check if key image appreared already (call .key_image())
    /// * provide merkle proofs of membership for each ring member (see
    ///   .tx_out_global_indices)
    ///
    /// # Arguments
    /// * `signed_contingent_input` - The pre-signed input we are adding
    pub fn add_presigned_input_raw(&mut self, sci: SignedContingentInput) {
        self.input_materials.push(InputMaterials::Presigned(sci));
    }

    /// Add a non-change output to the transaction.
    ///
    /// If a sender memo credential has been set, this will create an
    /// authenticated sender memo for the TxOut. Otherwise the memo will be
    /// unused.
    ///
    /// # Arguments
    /// * `amount` - The amount of this output
    /// * `recipient` - The recipient's public address
    /// * `rng` - RNG used to generate blinding for commitment
    pub fn add_output<RNG: CryptoRng + RngCore>(
        &mut self,
        amount: Amount,
        recipient: &PublicAddress,
        rng: &mut RNG,
    ) -> Result<TxOutContext, TxBuilderError> {
        self.add_output_with_tx_private_key(amount, recipient, None, rng)
    }

    /// Add a non-change output to the transaction, optionally with a specified
    /// tx_private_key.
    ///
    /// Specifying the tx_private_key gives you two things:
    ///
    /// * Together with the amount and recipient, fixes the generated TxOut
    ///   public key, target key, and blinding factor for amount. Because the
    ///   blockchain enforces that TxOut public keys are unique, this is a point
    ///   of mutual exclusion, and you can use this to create idempotent payment
    ///   interfaces.
    /// * If you know the tx private key, you can prove to an untrusting third
    ///   party what the amount and recipient of the tx out is. You can use this
    ///   resolve disputes. The TxOut shared secret and confirmation numbers
    ///   don't accomplish this because they don't reveal the recipient.
    ///
    /// If the tx_private_key is not pseudorandom, it will harm the privacy of
    /// transactions. For a merchant or exchange, a reasonable way to derive
    /// it is to hash the payment id, or the withdrawal id, together with a
    /// 32 byte secret. (You could use your private spend key or similar for
    /// example, but you may wish to rotate that from time to time, and you
    /// may not expect that idempotence would break across that key rotation.
    /// It's up to the application developer to decide the most suitable
    /// scheme.) You must ensure 32 bytes of pseudo-entropy to avoid
    /// undermining the transaction protocol.
    ///
    /// An alternative is to seed the RNG that is used with the transaction
    /// builder and then call add_output as usual. However, this approach
    /// means that upgrading your RNG is a way that idempotence can break,
    /// and in many cases, breaking idempotence means risk of double payments /
    /// loss of funds. Setting the tx_private_key directly is possibly
    /// simpler and with fewer hazards. Additionally, this approach allows
    /// you to easily determine and record the tx_private_key, which you may
    /// want for other reasons as described.
    ///
    /// # Arguments
    /// * `amount` - The amount of this output
    /// * `recipient` - The recipient's public address
    /// * `tx_private_key` - Optionally, a specific tx_private_key to use for
    ///   this output.
    /// * `rng` - RNG used to generate blinding for commitment
    pub fn add_output_with_tx_private_key<RNG: CryptoRng + RngCore>(
        &mut self,
        amount: Amount,
        recipient: &PublicAddress,
        tx_private_key: Option<RistrettoPrivate>,
        rng: &mut RNG,
    ) -> Result<TxOutContext, TxBuilderError> {
        self.add_output_internal(amount, recipient, tx_private_key, rng)
    }

    /// Add a standard change output to the transaction.
    ///
    /// The change output is meant to send any value in the inputs not already
    /// sent via outputs or fee, back to the sender's address.
    /// The caller should ensure that the math adds up, and that
    /// change_value + total_outlays + fee = total_input_value
    ///
    /// (Here, outlay means a non-change output).
    ///
    /// A change output should be sent to the dedicated change subaddress of the
    /// sender.
    ///
    /// If provided, a Destination memo is attached to this output, which allows
    /// for recoverable transaction history.
    ///
    /// The use of dedicated change subaddress for change outputs allows to
    /// authenticate the contents of destination memos, which are otherwise
    /// unauthenticated.
    ///
    /// CHANGE OUTPUTS FOR GIFT CODES:
    /// -------------------------------
    /// Change outputs can track info about funding, redeeming or cancelling
    /// gift codes via memos which can are documented in transaction/std/memo
    ///
    /// A gift code is funded with add_gift_code_output. Any value remaining +
    /// the optional GiftCodeFundingMemo is written to the change output
    ///
    /// For gift code redemption & cancellation, the amount of the gift code is
    /// sent to the change address of the caller. In these cases the amount
    /// passed to this method should be: amount = gift_code_amount - fee.
    /// -------------------------------
    ///
    /// # Arguments
    /// * `amount` - The amount of this change output.
    /// * `change_destination` - An object including both a primary address and
    ///   a change subaddress to use to create this change output. The change
    ///   subaddress owns the change output. These can both be obtained from an
    ///   account key, but this API does not require the account key.
    /// * `rng` - RNG used to generate blinding for commitment
    pub fn add_change_output<RNG: CryptoRng + RngCore>(
        &mut self,
        amount: Amount,
        change_destination: &ReservedSubaddresses,
        rng: &mut RNG,
    ) -> Result<TxOutContext, TxBuilderError> {
        if !self.block_version.mixed_transactions_are_supported()
            && self.fee.token_id != amount.token_id
        {
            return Err(TxBuilderError::MixedTransactionsNotAllowed(
                self.fee.token_id,
                amount.token_id,
            ));
        }

        let tx_private_key = RistrettoPrivate::from_random(rng);

        self.outputs.push(TxBlueprintOutput::Change {
            change_destination: change_destination.clone(),
            amount,
            tx_private_key,
        });

        let shared_secret = create_shared_secret(
            change_destination.change_subaddress.view_public_key(),
            &tx_private_key,
        );
        let confirmation = TxOutConfirmationNumber::from(&shared_secret);

        let tx_out_public_key = create_tx_out_public_key(
            &tx_private_key,
            change_destination.change_subaddress.spend_public_key(),
        )
        .into();

        Ok(TxOutContext {
            tx_out_public_key,
            confirmation,
            shared_secret,
        })
    }

    /// Add an output to the reserved subaddress for gift codes
    ///
    /// The gift code subaddress is meant for reserving TxOuts for usage
    /// at a later time. This enables functionality like sending "gift codes"
    /// to individuals who may not have a Botho account and "red envelopes".
    ///
    /// The caller should ensure that the math adds up, and that
    /// change_value + gift_code_amount + fee = total_input_value
    ///
    /// # Arguments
    /// * `amount` - The amount of the "gift code"
    /// * `reserved_subaddresses` - A ReservedSubaddresses object which provides
    ///   all standard reserved addresses for the caller.
    /// * `rng` - RNG used to generate blinding for commitment
    pub fn add_gift_code_output<RNG: CryptoRng + RngCore>(
        &mut self,
        amount: Amount,
        reserved_subaddresses: &ReservedSubaddresses,
        rng: &mut RNG,
    ) -> Result<TxOutContext, TxBuilderError> {
        self.add_output_internal(
            amount,
            &reserved_subaddresses.gift_code_subaddress,
            None,
            rng,
        )
    }

    /// Add an output to the transaction.
    ///
    /// # Arguments
    /// * `amount` - The amount of this output
    /// * `recipient` - The recipient's public address
    /// * `tx_private_key` - Optional. If unspecified, generated randomly using
    ///   rng.
    /// * `rng` - RNG used to generate tx private key (if not specified)
    fn add_output_internal<RNG: CryptoRng + RngCore>(
        &mut self,
        amount: Amount,
        recipient: &PublicAddress,
        tx_private_key: Option<RistrettoPrivate>,
        rng: &mut RNG,
    ) -> Result<TxOutContext, TxBuilderError> {
        if !self.block_version.mixed_transactions_are_supported()
            && self.fee.token_id != amount.token_id
        {
            return Err(TxBuilderError::MixedTransactionsNotAllowed(
                self.fee.token_id,
                amount.token_id,
            ));
        }

        let tx_private_key = tx_private_key.unwrap_or_else(|| RistrettoPrivate::from_random(rng));

        self.outputs.push(TxBlueprintOutput::Recipient {
            recipient: recipient.clone(),
            amount,
            tx_private_key,
        });

        let shared_secret = create_shared_secret(recipient.view_public_key(), &tx_private_key);
        let confirmation = TxOutConfirmationNumber::from(&shared_secret);

        let tx_out_public_key =
            create_tx_out_public_key(&tx_private_key, recipient.spend_public_key()).into();

        Ok(TxOutContext {
            tx_out_public_key,
            confirmation,
            shared_secret,
        })
    }

    /// Sets the tombstone block.
    ///
    /// # Arguments
    /// * `tombstone_block` - Tombstone block number.
    pub fn set_tombstone_block(&mut self, tombstone_block: u64) -> u64 {
        self.tombstone_block = tombstone_block;
        self.tombstone_block
    }

    /// Sets the transaction fee.
    ///
    /// # Arguments
    /// * `fee_value` - Transaction fee value, in smallest representable units.
    pub fn set_fee(&mut self, fee_value: u64) -> Result<(), TxBuilderError> {
        self.fee.value = fee_value;
        Ok(())
    }

    /// Gets the transaction fee.
    pub fn get_fee(&self) -> u64 {
        self.fee.value
    }

    /// Gets the fee token id
    pub fn get_fee_token_id(&self) -> TokenId {
        self.fee.token_id
    }

    /// Sets the minimum fee map.
    /// This is used to allow the enclave to reject the transaction if the
    /// client has used a different fee map than consensus is using, which
    /// could result in an information disclosure attack. (This is a
    /// TOB-MCCT-5 mitigation)
    pub fn set_fee_map(&mut self, fee_map: FeeMap) {
        self.fee_map = Some(fee_map);
    }

    /// Sets the cluster tag decay rate for tag inheritance.
    ///
    /// When outputs inherit cluster tags from inputs, the weights are
    /// multiplied by (1 - decay_rate/TAG_WEIGHT_SCALE). A decay rate of
    /// 100_000 (10%) means tags lose 10% of their weight per transaction.
    ///
    /// # Arguments
    /// * `decay_rate` - Decay rate in parts per million (0 = no decay,
    ///   1_000_000 = full decay)
    pub fn set_cluster_tag_decay_rate(&mut self, decay_rate: u32) {
        self.cluster_tag_decay_rate = decay_rate;
    }

    /// Return blueprint that together with a memo builder can be used to
    /// produce an unsigned tx.
    pub fn build_blueprint(mut self) -> Result<TxBlueprint, TxBuilderError> {
        // Note: Origin block has block version zero, so some clients like slam that
        // start with a bootstrapped ledger will target block version 0. However,
        // block version zero has no special rules and so targeting block version 0
        // should be the same as targeting block version 1, for the transaction
        // builder. This test is mainly here in case we decide that the
        // transaction builder should stop supporting sufficiently old block
        // versions in the future, then we can replace the zero here with
        // something else.
        if self.block_version < BlockVersion::default() {
            return Err(TxBuilderError::BlockVersionTooOld(*self.block_version, 0));
        }

        if self.block_version > BlockVersion::MAX {
            return Err(TxBuilderError::BlockVersionTooNew(
                *self.block_version,
                *BlockVersion::MAX,
            ));
        }

        if !self.block_version.masked_token_id_feature_is_supported()
            && self.fee.token_id != Mob::ID
        {
            return Err(TxBuilderError::FeatureNotSupportedAtBlockVersion(
                *self.block_version,
                "nonzero token id",
            ));
        }

        if self.input_materials.is_empty() {
            return Err(TxBuilderError::NoInputs);
        }

        // All inputs must have rings of the same size.
        if self
            .input_materials
            .windows(2)
            .any(|win| win[0].ring_size() != win[1].ring_size())
        {
            return Err(TxBuilderError::InvalidRingSize);
        }

        for input in self.input_materials.iter() {
            if !self.block_version.mixed_transactions_are_supported()
                && input.amount().token_id != self.fee.token_id
            {
                return Err(TxBuilderError::MixedTransactionsNotAllowed(
                    self.fee.token_id,
                    input.amount().token_id,
                ));
            }

            if let InputMaterials::Presigned(_) = input {
                if !self.block_version.signed_input_rules_are_supported() {
                    return Err(TxBuilderError::SignedInputRulesNotAllowed);
                }
            }
            // Note: Botho does not use membership proofs
        }

        // Construct a list of sorted inputs.
        // Inputs are sorted by the first ring element's public key. Note that each ring
        // is also sorted.
        self.input_materials
            .sort_by(|a, b| a.sort_key().cmp(b.sort_key()));

        // Compute inherited cluster tags if block version supports them
        let cluster_tags = if self.block_version.cluster_tags_are_supported() {
            // Collect (tags, value) pairs from all inputs that have cluster tags
            let input_tags: Vec<(ClusterTagVector, u64)> = self
                .input_materials
                .iter()
                .filter_map(|input| {
                    input
                        .cluster_tags()
                        .map(|tags| (tags.clone(), input.amount().value))
                })
                .collect();

            if input_tags.is_empty() {
                // No inputs have tags, outputs get empty tags
                Some(ClusterTagVector::empty())
            } else {
                Some(ClusterTagVector::merge_weighted(
                    &input_tags,
                    self.cluster_tag_decay_rate,
                ))
            }
        } else {
            None
        };

        let inputs: Vec<TxIn> = self.input_materials.iter().map(TxIn::from).collect();

        let rings = self
            .input_materials
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<InputRing>, _>>()?;

        Ok(TxBlueprint {
            inputs,
            rings,
            outputs: self.outputs,
            fee: self.fee,
            tombstone_block: self.tombstone_block,
            block_version: self.block_version,
            cluster_tags,
        })
    }

    /// Return low level data to sign and construct transactions with external
    /// signers
    ///
    /// # Arguments
    /// * `memo_builder` - An object which creates memos for the TxOuts in this
    ///   transaction
    pub fn build_unsigned(
        self,
        memo_builder: impl MemoBuilder,
    ) -> Result<UnsignedTx, TxBuilderError> {
        self.build_unsigned_with_sorter::<DefaultTxOutputsOrdering>(memo_builder)
    }

    /// Consume the builder and return the transaction.
    pub fn build<RNG: CryptoRng + RngCore, S: RingSigner + ?Sized>(
        self,
        ring_signer: &S,
        memo_builder: impl MemoBuilder,
        rng: &mut RNG,
    ) -> Result<Tx, TxBuilderError> {
        self.build_with_comparer_internal::<RNG, DefaultTxOutputsOrdering, S>(
            ring_signer,
            memo_builder,
            rng,
        )
    }

    /// Consume the builder and return the transaction with a comparer.
    /// Used only in testing library.
    #[cfg(feature = "test-only")]
    pub fn build_with_sorter<
        RNG: CryptoRng + RngCore,
        O: TxOutputsOrdering,
        S: RingSigner + ?Sized,
    >(
        self,
        ring_signer: &S,
        memo_builder: impl MemoBuilder,
        rng: &mut RNG,
    ) -> Result<Tx, TxBuilderError> {
        self.build_with_comparer_internal::<RNG, O, S>(ring_signer, memo_builder, rng)
    }

    /// Return low level data to sign and construct transactions with external
    /// signers.
    ///
    /// Allows specifying a custom output ordering, which is useful for internal
    /// testing.
    fn build_unsigned_with_sorter<O: TxOutputsOrdering>(
        self,
        memo_builder: impl MemoBuilder,
    ) -> Result<UnsignedTx, TxBuilderError> {
        self.build_blueprint()?.to_unsigned_tx::<O>(memo_builder)
    }

    /// Consume the builder and return the transaction with a comparer
    /// (internal usage only).
    fn build_with_comparer_internal<
        RNG: CryptoRng + RngCore,
        O: TxOutputsOrdering,
        S: RingSigner + ?Sized,
    >(
        self,
        ring_signer: &S,
        memo_builder: impl MemoBuilder,
        rng: &mut RNG,
    ) -> Result<Tx, TxBuilderError> {
        let fee_map = self.fee_map.clone();
        let unsigned_tx = self.build_unsigned_with_sorter::<O>(memo_builder)?;
        Ok(unsigned_tx.sign(ring_signer, fee_map.as_ref(), rng)?)
    }
}

/// Creates a TxOut that sends `value` to `recipient` using the provided
/// `tx_private_key`.
///
/// # Arguments
/// * `block_version` - Block version rules to conform to
/// * `value` - Value of the output, in picoMOB.
/// * `recipient` - Recipient's address.
/// * `memo_fn` - The memo function to use -- see TxOut::new_with_memo docu
/// * `tx_private_key` - The tx private key to use. This should be pseudorandom.
///
/// # Returns
/// * TxOut
/// * tx_out_shared_secret
pub(crate) fn create_output_internal(
    block_version: BlockVersion,
    amount: Amount,
    recipient: &PublicAddress,
    memo_fn: impl FnOnce(MemoContext) -> Result<MemoPayload, NewMemoError>,
    tx_private_key: &RistrettoPrivate,
) -> Result<(TxOut, RistrettoPublic), TxBuilderError> {
    let tx_out = TxOut::new_with_memo(
        block_version,
        amount,
        recipient,
        tx_private_key,
        memo_fn,
    )?;

    let shared_secret = create_shared_secret(recipient.view_public_key(), tx_private_key);
    Ok((tx_out, shared_secret))
}


#[cfg(test)]
pub mod transaction_builder_tests {
    use super::*;
    use crate::{test_utils::get_input_credentials, EmptyMemoBuilder};
    use alloc::vec;
    use bth_account_keys::{AccountKey, DEFAULT_SUBADDRESS_INDEX};
    use bth_crypto_ring_signature_signer::NoKeysRingSigner;
    use bth_transaction_core::{
        constants::MILLIMOB_TO_PICOMOB,
        ring_signature::KeyImage,
        subaddress_matches_tx_out,
        validation::{validate_signature, validate_tx_out},
    };
    use rand::{rngs::StdRng, SeedableRng};

    // Helper which produces a list of block_version, TokenId pairs to iterate over
    // in tests
    fn get_block_version_token_id_pairs() -> Vec<(BlockVersion, TokenId)> {
        vec![
            (BlockVersion::try_from(0).unwrap(), TokenId::from(0)),
            (BlockVersion::try_from(1).unwrap(), TokenId::from(0)),
            (BlockVersion::try_from(2).unwrap(), TokenId::from(0)),
            (BlockVersion::try_from(2).unwrap(), TokenId::from(1)),
            (BlockVersion::try_from(2).unwrap(), TokenId::from(2)),
        ]
    }

    #[test]
    // Spend a single input and send its full value to a single recipient.
    fn test_simple_transaction() {
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);

        for (block_version, token_id) in get_block_version_token_id_pairs() {
            let sender = AccountKey::random(&mut rng);
            let recipient = AccountKey::random(&mut rng);
            let value = 1475 * MILLIMOB_TO_PICOMOB;
            let amount = Amount { value, token_id };

            // Create input credentials
            let input_credentials =
                get_input_credentials(block_version, amount, &sender, &mut rng);

            let ring_size = input_credentials.ring.len();
            let key_image = KeyImage::from(input_credentials.assert_has_onetime_private_key());

            let mut transaction_builder = TransactionBuilder::new(
                block_version,
                Amount::new(Mob::MINIMUM_FEE, token_id),
            )
            .unwrap();

            transaction_builder.add_input(input_credentials);
            let TxOutContext { confirmation, .. } = transaction_builder
                .add_output(
                    Amount::new(value - Mob::MINIMUM_FEE, token_id),
                    &recipient.default_subaddress(),
                    &mut rng,
                )
                .unwrap();

            let tx = transaction_builder
                .build(&NoKeysRingSigner {}, EmptyMemoBuilder, &mut rng)
                .unwrap();

            // The transaction should have a single input.
            assert_eq!(tx.prefix.inputs.len(), 1);

            // Ring size should match
            assert_eq!(tx.prefix.inputs[0].ring.len(), ring_size);

            let expected_key_images = vec![key_image];
            assert_eq!(tx.key_images(), expected_key_images);

            // The transaction should have one output.
            assert_eq!(tx.prefix.outputs.len(), 1);

            let output: &TxOut = tx.prefix.outputs.first().unwrap();

            validate_tx_out(block_version, output).unwrap();

            // The output should belong to the correct recipient.
            assert!(
                subaddress_matches_tx_out(&recipient, DEFAULT_SUBADDRESS_INDEX, output).unwrap()
            );

            // The output should have the correct value and confirmation number
            {
                let public_key = RistrettoPublic::try_from(&output.public_key).unwrap();
                assert!(confirmation.validate(&public_key, recipient.view_private_key()));
            }

            // The transaction should have a valid signature.
            assert!(validate_signature(block_version, &tx, &mut rng).is_ok());
        }
    }
}
