// Copyright (c) 2018-2024 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Utilities that help with testing the transaction builder and related objects

use crate::{
    EmptyMemoBuilder, InputCredentials, MemoBuilder, ReservedSubaddresses, TransactionBuilder,
    TxBuilderError,
};
use alloc::vec::Vec;
use bth_account_keys::{AccountKey, PublicAddress, DEFAULT_SUBADDRESS_INDEX};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature_signer::{NoKeysRingSigner, OneTimeKeyDeriveData};
use bth_transaction_core::{
    constants::RING_SIZE,
    onetime_keys::*,
    tokens::Mob,
    tx::{Tx, TxOut},
    Amount, BlockVersion, MemoContext, MemoPayload, NewMemoError, Token, TokenId,
};
use bth_transaction_extra::UnsignedTx;
use bth_util_from_random::FromRandom;
use rand::{rngs::StdRng, CryptoRng, RngCore, SeedableRng};

/// Creates a TxOut that sends `value` to `recipient`.
///
/// Note: This is only used in test code
///
/// # Arguments
/// * `block_version` - Block version for the TxOut
/// * `amount` - Amount of the output
/// * `recipient` - Recipient's address.
/// * `rng` - Entropy for the encryption.
///
/// # Returns
/// * A transaction output, and the shared secret for this TxOut.
pub fn create_output<RNG: CryptoRng + RngCore>(
    block_version: BlockVersion,
    amount: Amount,
    recipient: &PublicAddress,
    rng: &mut RNG,
) -> Result<(TxOut, RistrettoPublic), TxBuilderError> {
    let tx_private_key = RistrettoPrivate::from_random(rng);
    let (tx_out, shared_secret) = crate::transaction_builder::create_output_internal(
        block_version,
        amount,
        recipient,
        |_| Ok(MemoPayload::default()),
        &tx_private_key,
    )?;
    Ok((tx_out, shared_secret))
}

/// Creates a ring of of TxOuts.
///
/// # Arguments
/// * `block_version` - The block version for the TxOut's
/// * `amount` - Amount for the real element
/// * `ring_size` - Number of elements in the ring.
/// * `account` - Owner of one of the ring elements.
/// * `rng` - Randomness.
///
/// Returns (ring, real_index)
pub fn get_ring<RNG: CryptoRng + RngCore>(
    block_version: BlockVersion,
    amount: Amount,
    ring_size: usize,
    account: &AccountKey,
    rng: &mut RNG,
) -> (Vec<TxOut>, usize) {
    let mut ring: Vec<TxOut> = Vec::new();

    // Create ring_size - 1 mixins with assorted token ids
    for idx in 0..ring_size - 1 {
        let address = AccountKey::random(rng).default_subaddress();
        let token_id = if block_version.masked_token_id_feature_is_supported() {
            TokenId::from(idx as u64)
        } else {
            Mob::ID
        };
        let amount = Amount::new(amount.value, token_id);
        let (tx_out, _) =
            create_output(block_version, amount, &address, rng).unwrap();
        ring.push(tx_out);
    }

    // Insert the real element.
    let real_index = (rng.next_u64() % ring_size as u64) as usize;
    let (tx_out, _) = create_output(
        block_version,
        amount,
        &account.default_subaddress(),
        rng,
    )
    .unwrap();
    ring.insert(real_index, tx_out);
    assert_eq!(ring.len(), ring_size);

    (ring, real_index)
}

/// Creates an `InputCredentials` for an account.
///
/// # Arguments
/// * `block_version` - Block version to use for the tx outs
/// * `amount` - Amount for the real element
/// * `account` - Owner of one of the ring elements.
/// * `rng` - Randomness.
///
/// Returns (input_credentials)
pub fn get_input_credentials<RNG: CryptoRng + RngCore>(
    block_version: BlockVersion,
    amount: Amount,
    account: &AccountKey,
    rng: &mut RNG,
) -> InputCredentials {
    let (ring, real_index) = get_ring(block_version, amount, RING_SIZE, account, rng);
    let real_output = ring[real_index].clone();

    let onetime_private_key = recover_onetime_private_key(
        &RistrettoPublic::try_from(&real_output.public_key).unwrap(),
        account.view_private_key(),
        &account.subaddress_spend_private(DEFAULT_SUBADDRESS_INDEX),
    );
    let onetime_key_derive_data = OneTimeKeyDeriveData::OneTimeKey(onetime_private_key);

    InputCredentials::new(
        ring,
        real_index,
        onetime_key_derive_data,
        *account.view_private_key(),
    )
    .unwrap()
}

/// Generate fake ring global indices for testing.
/// These are just sequential indices starting from 0.
pub fn get_ring_global_indices(ring_size: usize) -> Vec<u64> {
    (0..ring_size).map(|i| i as u64).collect()
}

/// Uses TransactionBuilder to build a generic transaction for testing.
pub fn get_unsigned_transaction<RNG: RngCore + CryptoRng>(
    block_version: BlockVersion,
    token_id: TokenId,
    num_inputs: usize,
    num_outputs: usize,
    sender: &AccountKey,
    recipient: &AccountKey,
    rng: &mut RNG,
) -> Result<UnsignedTx, TxBuilderError> {
    let mut transaction_builder = TransactionBuilder::new(
        block_version,
        Amount::new(Mob::MINIMUM_FEE, token_id),
    )
    .unwrap();
    let input_value = 1000;
    let output_value = 10;

    // Set the fee so that sum(inputs) = sum(outputs) + fee.
    let fee = num_inputs as u64 * input_value - num_outputs as u64 * output_value;
    transaction_builder.set_fee(fee).unwrap();

    // Inputs
    for _i in 0..num_inputs {
        let input_credentials = get_input_credentials(
            block_version,
            Amount {
                value: input_value,
                token_id,
            },
            sender,
            rng,
        );
        transaction_builder.add_input(input_credentials);
    }

    // Outputs
    for _i in 0..num_outputs {
        transaction_builder
            .add_output(
                Amount::new(output_value, token_id),
                &recipient.default_subaddress(),
                rng,
            )
            .unwrap();
    }
    transaction_builder.build_unsigned(EmptyMemoBuilder)
}

/// Uses TransactionBuilder to build a generic transaction for testing.
pub fn get_transaction<RNG: RngCore + CryptoRng>(
    block_version: BlockVersion,
    token_id: TokenId,
    num_inputs: usize,
    num_outputs: usize,
    sender: &AccountKey,
    recipient: &AccountKey,
    rng: &mut RNG,
) -> Result<Tx, TxBuilderError> {
    let unsigned_tx = get_unsigned_transaction(
        block_version,
        token_id,
        num_inputs,
        num_outputs,
        sender,
        recipient,
        rng,
    )?;
    Ok(unsigned_tx.sign(&NoKeysRingSigner {}, None, rng)?)
}

/// Build simulated change memo with amount
pub fn build_change_memo_with_amount(
    builder: &mut impl MemoBuilder,
    change_amount: Amount,
) -> Result<MemoPayload, NewMemoError> {
    // Create simulated context
    let mut rng: StdRng = SeedableRng::from_seed([0u8; 32]);
    let alice = AccountKey::random(&mut rng);
    let alice_address_book = ReservedSubaddresses::from(&alice);
    let change_tx_pubkey = RistrettoPublic::from_random(&mut rng);
    let memo_context = MemoContext {
        tx_public_key: &change_tx_pubkey,
    };

    //Build memo
    builder.make_memo_for_change_output(change_amount, &alice_address_book, memo_context)
}
