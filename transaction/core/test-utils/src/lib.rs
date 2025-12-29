// Copyright (c) 2018-2023 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

mod mint;

pub use bth_account_keys::{AccountKey, PublicAddress, DEFAULT_SUBADDRESS_INDEX};
pub use bth_crypto_ring_signature_signer::NoKeysRingSigner;
pub use bth_transaction_core::{
    get_tx_out_shared_secret,
    onetime_keys::recover_onetime_private_key,
    ring_signature::KeyImage,
    tokens::Mob,
    tx::{Tx, TxOut, TxOutMembershipElement, TxOutMembershipHash},
    Amount, BlockVersion, Token,
};
pub use bth_util_serial::round_trip_message;
pub use mint::{
    create_mint_config_tx, create_mint_config_tx_and_signers, create_mint_tx,
    create_mint_tx_to_recipient, mint_config_tx_to_validated, sign_mint_config_tx_prefix,
    sign_mint_tx_prefix,
};

use bth_crypto_keys::RistrettoPrivate;
use mc_rand::{CryptoRng, RngCore};
use bth_util_from_random::FromRandom;

/// Generate a set of outputs that "mint" coins for each recipient.
pub fn get_outputs<T: RngCore + CryptoRng>(
    block_version: BlockVersion,
    recipient_and_amount: &[(PublicAddress, Amount)],
    rng: &mut T,
) -> Vec<TxOut> {
    recipient_and_amount
        .iter()
        .map(|(recipient, amount)| {
            TxOut::new(
                block_version,
                *amount,
                recipient,
                &RistrettoPrivate::from_random(rng),
            )
            .unwrap()
        })
        .collect()
}

/// Generate a dummy txout for testing.
pub fn create_test_tx_out(
    block_version: BlockVersion,
    rng: &mut (impl RngCore + CryptoRng),
) -> TxOut {
    let account_key = AccountKey::random(rng);
    TxOut::new(
        block_version,
        Amount::new(rng.next_u64(), Mob::ID),
        &account_key.default_subaddress(),
        &RistrettoPrivate::from_random(rng),
    )
    .unwrap()
}
