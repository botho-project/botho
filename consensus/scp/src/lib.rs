// Copyright (c) 2018-2022 The Botho Foundation

#![doc = include_str!("../README.md")]
#![allow(non_snake_case)]
#![deny(missing_docs)]

pub mod ballot;
mod error;
pub mod msg;
pub mod node;
pub mod predicates;
pub mod quorum_set_ext;
pub mod scp_log;
pub mod slot;
pub mod slot_state;
#[cfg(any(test, feature = "test_utils"))]
pub mod test_utils;
mod utils;

#[doc(inline)]
pub use bth_consensus_scp_types::{core_types, quorum_set};

#[doc(inline)]
pub use crate::{
    core_types::{GenericNodeId, Identifier, SlotIndex, Value},
    error::{ScpError, ScpResult},
    msg::{Msg, Topic},
    node::{MockScpNode, Node, ScpNode},
    quorum_set::{QuorumSet, QuorumSetMember, QuorumSetMemberWrapper},
    quorum_set_ext::QuorumSetExt,
};

// Re-export logger utilities for consumers who need to construct SCP nodes
pub use bth_common::logger::{create_null_logger, Logger};
