// Copyright (c) 2018-2022 The Botho Foundation

//! Blockchain data structures.

#![no_std]
#![deny(missing_docs)]

extern crate alloc;

pub mod crypto;
mod attestation_stubs;

mod block;
mod block_contents;
mod block_data;
mod block_id;
mod block_metadata;
mod block_signature;
mod error;

pub use crate::{
    block::{compute_block_id, Block, BlockIndex, MAX_BLOCK_VERSION},
    block_contents::{BlockContents, BlockContentsHash},
    block_data::BlockData,
    block_id::BlockID,
    block_metadata::{AttestationEvidence, BlockMetadata, BlockMetadataContents},
    block_signature::BlockSignature,
    error::ConvertError,
};

// Use stub types instead of removed mc-attest-verifier-types
pub use crate::attestation_stubs::{VerificationReport, VerificationSignature};
pub use bth_common::NodeID;
pub use bth_consensus_scp_types::{QuorumSet, QuorumSetMember, QuorumSetMemberWrapper};
pub use bth_transaction_types::{BlockVersion, BlockVersionError, BlockVersionIterator};
