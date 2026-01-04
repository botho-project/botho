// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Validation routines for a Botho transaction

mod error;
mod validate;

pub use self::{
    error::{TransactionValidationError, TransactionValidationResult},
    validate::{
        compute_progressive_fee, validate, validate_all_input_rules,
        validate_cluster_tag_inheritance, validate_cluster_tags_exist, validate_inputs_are_sorted,
        validate_key_images_are_unique, validate_masked_token_id_exists, validate_memo_exists,
        validate_number_of_inputs, validate_number_of_outputs, validate_outputs_are_sorted,
        validate_outputs_public_keys_are_unique, validate_progressive_fee,
        validate_ring_elements_are_sorted, validate_ring_elements_are_unique, validate_ring_sizes,
        validate_signature, validate_that_no_cluster_tags_exist,
        validate_that_no_masked_token_id_exists, validate_that_no_memo_exists, validate_tombstone,
        validate_transaction_fee, validate_tx_out, ClusterWealthLookup, ProgressiveFeeConfig,
    },
};
