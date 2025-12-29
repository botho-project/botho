// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Mix-in application of local peers traits to SyncConnection.
//! Post-SGX simplified version.

use crate::{
    consensus_msg::ConsensusMsg,
    error::RetryResult,
    traits::{ConsensusConnection, RetryableConsensusConnection},
};
use bth_common::ResponderId;
use bth_connection::{impl_sync_connection_retry, SyncConnection};
use bth_consensus_api::consensus_peer::ConsensusMsgResponse;
use bth_transaction_core::tx::TxHash;
use std::time::Duration;

/// Blanket implementation of RetryableConsensusConnection for SyncConnection
/// objects which own a ConsensusConnection.
impl<CC: ConsensusConnection> RetryableConsensusConnection for SyncConnection<CC> {
    fn remote_responder_id(&self) -> ResponderId {
        self.read().remote_responder_id()
    }

    fn send_consensus_msg(
        &self,
        msg: &ConsensusMsg,
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<ConsensusMsgResponse> {
        impl_sync_connection_retry!(
            self.write(),
            self.logger(),
            send_consensus_msg,
            retry_iterator,
            msg
        )
    }

    fn fetch_txs(
        &self,
        hashes: &[TxHash],
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<Vec<ConsensusMsg>> {
        impl_sync_connection_retry!(
            self.write(),
            self.logger(),
            fetch_txs,
            retry_iterator,
            hashes
        )
    }
}
