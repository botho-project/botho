// Copyright (c) 2018-2022 The Botho Foundation

//! Connection support

mod credentials;
mod error;
mod manager;
mod sync;
mod thick;
mod traits;

pub use crate::{
    credentials::{
        AnyCredentialsError, AnyCredentialsProvider, AuthenticationError, BasicCredentials,
        CredentialsProvider, CredentialsProviderError, HardcodedCredentialsProvider,
    },
    error::{Error, Result, RetryError, RetryResult},
    manager::ConnectionManager,
    sync::SyncConnection,
    thick::{EvidenceKind, ThickClient, ThickClientAttestationError},
    traits::{
        AttestationError, AttestedConnection, BlockInfo, BlockchainConnection, Connection,
        RetryableBlockchainConnection, RetryableUserTxConnection, UserTxConnection,
    },
};

pub use bt_common::trace_time as _trace_time;
pub use bt_consensus_api::consensus_common::ProposeTxResult;
pub use retry as _retry;
