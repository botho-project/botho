// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Traits and objects specific to peering connections.
//! Post-SGX simplified version - no encrypted transactions.

use crate::{
    error::{Result, RetryResult},
    ConsensusMsg,
};
use bt_common::{NodeID, ResponderId};
use bt_connection::Connection;
use bt_consensus_api::consensus_peer::ConsensusMsgResponse;
use bt_transaction_core::tx::TxHash;
use std::time::Duration;

/// A trait which describes a connection from one consensus node to another.
/// Post-SGX simplified - transactions are sent directly without encryption.
pub trait ConsensusConnection: Connection {
    /// Retrieve the remote peer ResponderId.
    fn remote_responder_id(&self) -> ResponderId;

    /// Retrieve the local node ID.
    fn local_node_id(&self) -> NodeID;

    /// Send the given consensus message to the remote peer.
    fn send_consensus_msg(&mut self, msg: &ConsensusMsg) -> Result<ConsensusMsgResponse>;

    /// Retrieve transactions which match the provided hashes.
    /// Post-SGX: Returns ConsensusMsg instead of encrypted TxContext.
    fn fetch_txs(&mut self, hashes: &[TxHash]) -> Result<Vec<ConsensusMsg>>;
}

/// Retriable versions of the ConsensusConnection methods
pub trait RetryableConsensusConnection {
    /// Retrieve the remote peer ResponderId.
    fn remote_responder_id(&self) -> ResponderId;

    /// Retryable version of the consensus message transmitter
    fn send_consensus_msg(
        &self,
        msg: &ConsensusMsg,
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<ConsensusMsgResponse>;

    /// Retryable version of fetch_txs
    fn fetch_txs(
        &self,
        hashes: &[TxHash],
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<Vec<ConsensusMsg>>;
}
