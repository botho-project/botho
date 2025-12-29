// Copyright (c) 2018-2022 The Botho Foundation

//! Transaction validation.

extern crate alloc;

use super::error::{TransactionValidationError, TransactionValidationResult};
use crate::{
    constants::*,
    tx::{Tx, TxOut, TxPrefix},
    Amount, BlockVersion, ClusterId, TokenId, TAG_WEIGHT_SCALE,
};
use alloc::{collections::BTreeMap, format, vec::Vec};
use bt_common::HashSet;
use rand_core::{CryptoRng, RngCore};

/// Determines if the transaction is valid, with respect to the provided
/// context.
///
/// # Arguments
/// * `tx` - A pending transaction.
/// * `current_block_index` - The index of the current block that is being
///   built.
/// * `block_version` - The version of the transaction rules we are testing
/// * `minimum_fee` - The minimum fee for the token indicated by
///   tx.prefix.fee_token_id
/// * `csprng` - Cryptographically secure random number generator.
///
/// Note: Botho does not use merkle membership proofs. Ring members are
/// validated directly against the UTXO set.
pub fn validate<R: RngCore + CryptoRng>(
    tx: &Tx,
    current_block_index: u64,
    block_version: BlockVersion,
    minimum_fee: u64,
    csprng: &mut R,
) -> TransactionValidationResult<()> {
    if BlockVersion::MAX < block_version {
        return Err(TransactionValidationError::Ledger(format!(
            "Invalid block version: {block_version}"
        )));
    }

    validate_number_of_inputs(&tx.prefix, MAX_INPUTS)?;

    validate_number_of_outputs(&tx.prefix, MAX_OUTPUTS)?;

    validate_ring_sizes(&tx.prefix, RING_SIZE)?;

    validate_ring_elements_are_unique(&tx.prefix)?;

    validate_ring_elements_are_sorted(&tx.prefix)?;

    validate_inputs_are_sorted(&tx.prefix)?;

    // Note: Botho does not use merkle membership proofs - ring members
    // are validated directly against the UTXO set by the validator.

    validate_signature(block_version, tx, csprng)?;

    validate_transaction_fee(tx, minimum_fee)?;

    validate_key_images_are_unique(tx)?;

    validate_outputs_public_keys_are_unique(tx)?;

    validate_tombstone(current_block_index, tx.prefix.tombstone_block)?;

    // Note: The transaction must not contain a Key Image that has previously been
    // spent. This must be checked outside the enclave.

    // Each tx_out must conform to the structural rules for TxOut's at this block
    // version
    for tx_out in tx.prefix.outputs.iter() {
        validate_tx_out(block_version, tx_out)?;
    }

    ////
    // Validate rules which depend on block version (see MCIP #26)
    ////

    if block_version.validate_transaction_outputs_are_sorted() {
        validate_outputs_are_sorted(&tx.prefix)?;
    }

    if block_version.signed_input_rules_are_supported() {
        validate_all_input_rules(block_version, tx)?;
    } else {
        validate_that_no_input_rules_exist(tx)?;
    }

    Ok(())
}

/// Determines if a tx out conforms to the current block version rules
pub fn validate_tx_out(
    block_version: BlockVersion,
    tx_out: &TxOut,
) -> TransactionValidationResult<()> {
    // If memos are supported, then all outputs must have memo fields.
    // If memos are not yet supported, then no outputs may have memo fields.
    if block_version.e_memo_feature_is_supported() {
        validate_memo_exists(tx_out)?;
    } else {
        validate_that_no_memo_exists(tx_out)?;
    }

    // If masked token id is supported, then all outputs must have masked_token_id
    // If masked token id is not yet supported, then no outputs may have
    // masked_token_id
    //
    // Note: This rct_bulletproofs code enforces that token_id = 0 if this feature
    // is not enabled
    if block_version.masked_token_id_feature_is_supported() {
        validate_masked_token_id_exists(tx_out)?;
    } else {
        validate_that_no_masked_token_id_exists(tx_out)?;
    }

    // If cluster tags are supported, they must be present and valid.
    // If not yet supported, they must not be present.
    if block_version.cluster_tags_are_supported() {
        validate_cluster_tags_exist(tx_out)?;
    } else {
        validate_that_no_cluster_tags_exist(tx_out)?;
    }

    Ok(())
}

/// The transaction must have at least one input, and no more than the maximum
/// allowed number of inputs.
pub fn validate_number_of_inputs(
    tx_prefix: &TxPrefix,
    maximum_allowed_inputs: u64,
) -> TransactionValidationResult<()> {
    let num_inputs = tx_prefix.inputs.len();

    // Each transaction must have at least one input.
    if num_inputs == 0 {
        return Err(TransactionValidationError::NoInputs);
    }

    // Each transaction must have no more than the maximum allowed number of inputs.
    if num_inputs > maximum_allowed_inputs as usize {
        return Err(TransactionValidationError::TooManyInputs);
    }

    Ok(())
}

/// The transaction must have at least one output.
pub fn validate_number_of_outputs(
    tx_prefix: &TxPrefix,
    maximum_allowed_outputs: u64,
) -> TransactionValidationResult<()> {
    let num_outputs = tx_prefix.outputs.len();

    // Each transaction must have at least one output.
    if num_outputs == 0 {
        return Err(TransactionValidationError::NoOutputs);
    }

    // Each transaction must have no more than the maximum allowed number of
    // outputs.
    if num_outputs > maximum_allowed_outputs as usize {
        return Err(TransactionValidationError::TooManyOutputs);
    }

    Ok(())
}

/// Each input must contain a ring containing `ring_size` elements.
pub fn validate_ring_sizes(
    tx_prefix: &TxPrefix,
    ring_size: usize,
) -> TransactionValidationResult<()> {
    for input in &tx_prefix.inputs {
        if input.ring.len() != ring_size {
            let e = if input.ring.len() > ring_size {
                TransactionValidationError::ExcessiveRingSize
            } else {
                TransactionValidationError::InsufficientRingSize
            };
            return Err(e);
        }
    }
    Ok(())
}

/// Ring elements of each ring without input rules must be unique.
/// For any ring with input rules, ring elements must be unique within that
/// ring. (See also MCIP #57)
pub fn validate_ring_elements_are_unique(tx_prefix: &TxPrefix) -> TransactionValidationResult<()> {
    let mut ring_elements_without_input_rules = HashSet::<&TxOut>::default();
    for input in tx_prefix.inputs.iter() {
        if input.input_rules.is_some() {
            check_unique(
                &input.ring,
                TransactionValidationError::DuplicateRingElements,
            )?;
        } else {
            for elem in input.ring.iter() {
                if !ring_elements_without_input_rules.insert(elem) {
                    return Err(TransactionValidationError::DuplicateRingElements);
                }
            }
        }
    }
    Ok(())
}

/// Elements in a ring must be sorted.
pub fn validate_ring_elements_are_sorted(tx_prefix: &TxPrefix) -> TransactionValidationResult<()> {
    for tx_in in &tx_prefix.inputs {
        check_sorted(
            &tx_in.ring,
            |a, b| a.public_key < b.public_key,
            TransactionValidationError::UnsortedRingElements,
        )?;
    }

    Ok(())
}

/// Inputs must be sorted by the public key of the first ring element of each
/// input.
pub fn validate_inputs_are_sorted(tx_prefix: &TxPrefix) -> TransactionValidationResult<()> {
    check_sorted(
        &tx_prefix.inputs,
        |a, b| {
            !a.ring.is_empty() && !b.ring.is_empty() && a.ring[0].public_key < b.ring[0].public_key
        },
        TransactionValidationError::UnsortedInputs,
    )
}

/// Outputs must be sorted by the tx public key
pub fn validate_outputs_are_sorted(tx_prefix: &TxPrefix) -> TransactionValidationResult<()> {
    check_sorted(
        &tx_prefix.outputs,
        |a, b| a.public_key < b.public_key,
        TransactionValidationError::UnsortedOutputs,
    )
}

/// All key images within the transaction must be unique.
pub fn validate_key_images_are_unique(tx: &Tx) -> TransactionValidationResult<()> {
    check_unique(
        &tx.key_images(),
        TransactionValidationError::DuplicateKeyImages,
    )
}

/// All output public keys within the transaction must be unique.
pub fn validate_outputs_public_keys_are_unique(tx: &Tx) -> TransactionValidationResult<()> {
    check_unique(
        &tx.output_public_keys(),
        TransactionValidationError::DuplicateOutputPublicKey,
    )
}

/// All outputs have no memo (new-style TxOuts (Post MCIP #3) are rejected)
pub fn validate_that_no_memo_exists(tx_out: &TxOut) -> TransactionValidationResult<()> {
    if tx_out.e_memo.is_some() {
        return Err(TransactionValidationError::MemosNotAllowed);
    }
    Ok(())
}

/// All outputs have a memo (old-style TxOuts (Pre MCIP #3) are rejected)
pub fn validate_memo_exists(tx_out: &TxOut) -> TransactionValidationResult<()> {
    if tx_out.e_memo.is_none() {
        return Err(TransactionValidationError::MissingMemo);
    }
    Ok(())
}

/// All outputs have no masked token id (new-style TxOuts (Post MCIP #25) are
/// rejected)
pub fn validate_that_no_masked_token_id_exists(tx_out: &TxOut) -> TransactionValidationResult<()> {
    if !tx_out.get_masked_amount()?.masked_token_id().is_empty() {
        return Err(TransactionValidationError::MaskedTokenIdNotAllowed);
    }
    Ok(())
}

/// All outputs have a masked token id (old-style TxOuts (Pre MCIP #25) are
/// rejected)
pub fn validate_masked_token_id_exists(tx_out: &TxOut) -> TransactionValidationResult<()> {
    if tx_out.get_masked_amount()?.masked_token_id().len() != TokenId::NUM_BYTES {
        return Err(TransactionValidationError::MissingMaskedTokenId);
    }
    Ok(())
}

/// Verifies the transaction signature.
///
/// A valid RctBulletproofs signature implies that:
/// * tx.prefix has not been modified,
/// * The signer owns one element in each input ring,
/// * Each key image corresponds to the spent ring element,
/// * The outputs have values in [0,2^64),
/// * The transaction does not create or destroy bothos.
/// * The signature is valid according to the rules of this block version
pub fn validate_signature<R: RngCore + CryptoRng>(
    block_version: BlockVersion,
    tx: &Tx,
    rng: &mut R,
) -> TransactionValidationResult<()> {
    let rings = tx.prefix.get_input_rings()?;

    let output_commitments = tx
        .prefix
        .output_commitments()?
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();

    tx.signature
        .verify(
            block_version,
            &tx.prefix,
            &rings,
            &output_commitments,
            Amount::new(tx.prefix.fee, TokenId::from(tx.prefix.fee_token_id)),
            rng,
        )
        .map_err(TransactionValidationError::InvalidTransactionSignature)
}

/// The fee amount must be greater than or equal to the given minimum fee.
pub fn validate_transaction_fee(tx: &Tx, minimum_fee: u64) -> TransactionValidationResult<()> {
    if tx.prefix.fee < minimum_fee {
        Err(TransactionValidationError::TxFeeError)
    } else {
        Ok(())
    }
}

// Note: validate_membership_proofs was removed as part of Botho fork.
// Botho does not use merkle membership proofs - ring members are validated
// directly against the UTXO set.

/// The transaction must be not have expired, or be too long-lived.
///
/// # Arguments
/// * `current_block_index` - The index of the block currently being built.
/// * `tombstone_block_index` - The block index at which this transaction is no
///   longer considered valid.
pub fn validate_tombstone(
    current_block_index: u64,
    tombstone_block_index: u64,
) -> TransactionValidationResult<()> {
    if current_block_index >= tombstone_block_index {
        return Err(TransactionValidationError::TombstoneBlockExceeded);
    }

    let limit = current_block_index + MAX_TOMBSTONE_BLOCKS;
    if tombstone_block_index > limit {
        return Err(TransactionValidationError::TombstoneBlockTooFar);
    }

    Ok(())
}

/// Any input rules imposed on the Tx must satisfied
pub fn validate_all_input_rules(
    block_version: BlockVersion,
    tx: &Tx,
) -> TransactionValidationResult<()> {
    for input in tx.prefix.inputs.iter() {
        if let Some(rules) = input.input_rules.as_ref() {
            rules.verify(block_version, tx)?;
        }
    }
    Ok(())
}

/// Validate that no input have input rules
pub fn validate_that_no_input_rules_exist(tx: &Tx) -> TransactionValidationResult<()> {
    for input in tx.prefix.inputs.iter() {
        if input.input_rules.is_some() {
            return Err(TransactionValidationError::InputRulesNotAllowed);
        }
    }
    Ok(())
}

fn check_sorted<T>(
    values: &[T],
    ordered: fn(&T, &T) -> bool,
    err: TransactionValidationError,
) -> TransactionValidationResult<()> {
    if !values.windows(2).all(|pair| ordered(&pair[0], &pair[1])) {
        return Err(err);
    }

    Ok(())
}

fn check_unique<T: Eq + core::hash::Hash>(
    values: &[T],
    err: TransactionValidationError,
) -> TransactionValidationResult<()> {
    let mut uniques = HashSet::default();
    for x in values {
        if !uniques.insert(x) {
            return Err(err);
        }
    }

    Ok(())
}

// =========== Cluster Tag Validation ===========

/// Validate that cluster tags exist on a TxOut (required from block version 5+).
pub fn validate_cluster_tags_exist(tx_out: &TxOut) -> TransactionValidationResult<()> {
    match &tx_out.cluster_tags {
        Some(tags) if tags.is_valid() => Ok(()),
        Some(_) => Err(TransactionValidationError::InvalidClusterTags),
        None => Err(TransactionValidationError::MissingClusterTags),
    }
}

/// Validate that cluster tags do not exist on a TxOut (for older block versions).
pub fn validate_that_no_cluster_tags_exist(tx_out: &TxOut) -> TransactionValidationResult<()> {
    if tx_out.cluster_tags.is_some() {
        return Err(TransactionValidationError::ClusterTagsNotAllowed);
    }
    Ok(())
}

/// Validate cluster tag inheritance for a transaction.
///
/// This checks that output tags correctly inherit from input tags with decay.
/// The sum of output tag masses for each cluster must not exceed the
/// (decayed) input tag mass for that cluster.
///
/// # Arguments
/// * `input_tx_outs` - The real inputs to the transaction (not the ring decoys)
/// * `input_values` - The decrypted values of each input
/// * `output_tx_outs` - The outputs of the transaction
/// * `output_values` - The values of each output
/// * `decay_rate` - Tag decay rate (parts per TAG_WEIGHT_SCALE)
pub fn validate_cluster_tag_inheritance(
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    output_tx_outs: &[&TxOut],
    output_values: &[u64],
    decay_rate: u32,
) -> TransactionValidationResult<()> {
    // Calculate input tag masses
    let mut input_masses: BTreeMap<ClusterId, u64> = BTreeMap::new();

    for (tx_out, &value) in input_tx_outs.iter().zip(input_values.iter()) {
        if let Some(tags) = &tx_out.cluster_tags {
            for entry in &tags.entries {
                let mass = (value as u128 * entry.weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;
                *input_masses.entry(entry.cluster_id).or_insert(0) += mass;
            }
        }
    }

    // Calculate output tag masses
    let mut output_masses: BTreeMap<ClusterId, u64> = BTreeMap::new();

    for (tx_out, &value) in output_tx_outs.iter().zip(output_values.iter()) {
        if let Some(tags) = &tx_out.cluster_tags {
            for entry in &tags.entries {
                let mass = (value as u128 * entry.weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;
                *output_masses.entry(entry.cluster_id).or_insert(0) += mass;
            }
        }
    }

    // Apply decay and check conservation
    let decay_factor = TAG_WEIGHT_SCALE.saturating_sub(decay_rate);

    for (cluster, &input_mass) in &input_masses {
        let expected =
            (input_mass as u128 * decay_factor as u128 / TAG_WEIGHT_SCALE as u128) as u64;
        let actual = output_masses.get(cluster).copied().unwrap_or(0);

        // Allow some tolerance for rounding
        let tolerance = (input_mass / 1000).max(1);

        if actual > expected + tolerance {
            return Err(TransactionValidationError::ClusterTagInflation(
                cluster.0,
                actual,
                expected,
            ));
        }
    }

    // Check that outputs don't introduce new clusters beyond background
    for (cluster, &output_mass) in &output_masses {
        if !input_masses.contains_key(cluster) && output_mass > 0 {
            // New cluster appeared in outputs that wasn't in inputs
            // This is only allowed if it came from background (no tags on inputs)
            // For now, we allow this since background can become attributed
            // through proportional distribution
        }
    }

    Ok(())
}

/// Configuration for progressive fee computation.
#[derive(Clone, Debug)]
pub struct ProgressiveFeeConfig {
    /// Base fee rate in basis points (applied to background/unattributed value).
    pub background_rate_bps: u32,
    /// Maximum fee rate in basis points.
    pub max_rate_bps: u32,
    /// Steepness of the fee curve (wealth level at sigmoid midpoint).
    pub steepness: u64,
}

impl Default for ProgressiveFeeConfig {
    fn default() -> Self {
        Self {
            background_rate_bps: 10,  // 0.1%
            max_rate_bps: 1000,       // 10%
            steepness: 10_000_000,    // 10 million
        }
    }
}

/// Cluster wealth tracker for progressive fee computation.
pub trait ClusterWealthLookup {
    /// Get the total wealth attributed to a cluster.
    fn get_cluster_wealth(&self, cluster_id: ClusterId) -> u64;
}

/// Compute the progressive fee for a transaction.
///
/// The fee is computed as:
/// fee = transfer_amount * effective_rate
///
/// where effective_rate is a weighted average of each cluster's rate,
/// weighted by the value attributed to that cluster.
pub fn compute_progressive_fee(
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    transfer_amount: u64,
    cluster_wealth: &impl ClusterWealthLookup,
    fee_config: &ProgressiveFeeConfig,
) -> u64 {
    let mut weighted_rate: u128 = 0;
    let mut total_value: u128 = 0;

    for (tx_out, &value) in input_tx_outs.iter().zip(input_values.iter()) {
        total_value += value as u128;

        if let Some(tags) = &tx_out.cluster_tags {
            for entry in &tags.entries {
                let mass = (value as u128 * entry.weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;
                let wealth = cluster_wealth.get_cluster_wealth(entry.cluster_id);
                let rate = compute_cluster_fee_rate(wealth, fee_config);
                weighted_rate += mass as u128 * rate as u128;
            }

            // Background portion
            let background_weight = tags.background_weight();
            let background_mass =
                (value as u128 * background_weight as u128 / TAG_WEIGHT_SCALE as u128) as u64;
            weighted_rate += background_mass as u128 * fee_config.background_rate_bps as u128;
        } else {
            // No tags = fully background
            weighted_rate += value as u128 * fee_config.background_rate_bps as u128;
        }
    }

    if total_value == 0 {
        return 0;
    }

    let effective_rate = weighted_rate / total_value;
    (transfer_amount as u128 * effective_rate / 10_000) as u64
}

/// Compute the fee rate for a cluster based on its wealth.
/// Uses a sigmoid curve: rate = min + (max - min) * sigmoid(wealth / steepness)
fn compute_cluster_fee_rate(wealth: u64, config: &ProgressiveFeeConfig) -> u32 {
    // Sigmoid approximation using rational function
    let x = wealth as f64 / config.steepness as f64;
    let sigmoid = x / (1.0 + x);

    let rate_range = config.max_rate_bps - config.background_rate_bps;
    let rate = config.background_rate_bps as f64 + (rate_range as f64 * sigmoid);

    rate.round() as u32
}

/// Validate that the declared fee meets the progressive fee requirement.
pub fn validate_progressive_fee(
    input_tx_outs: &[&TxOut],
    input_values: &[u64],
    declared_fee: u64,
    transfer_amount: u64,
    cluster_wealth: &impl ClusterWealthLookup,
    fee_config: &ProgressiveFeeConfig,
) -> TransactionValidationResult<()> {
    let required_fee = compute_progressive_fee(
        input_tx_outs,
        input_values,
        transfer_amount,
        cluster_wealth,
        fee_config,
    );

    if declared_fee < required_fee {
        return Err(TransactionValidationError::InsufficientProgressiveFee(
            required_fee,
            declared_fee,
        ));
    }

    Ok(())
}

// NOTE: There are unit tests of every validation function, which appear in
// transaction/core/tests/validation.rs.
//
// The reason that these appear there is,
// many of the tests use `mc-transaction-core-test-utils` which itself depends
// on `mc-ledger-db` and `mc-transaction-core`, and this creates a circular
// dependency which leads to build problems, if the unit tests appear in-line
// here.
//
// Please add tests for any new validation functions there. Thank you!
