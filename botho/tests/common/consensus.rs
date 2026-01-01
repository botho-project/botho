// Copyright (c) 2024 Botho Foundation
//
//! Consensus types for test network coordination.

use std::sync::Arc;

use bth_consensus_scp::msg::Msg;

use botho::{block::MintingTx, transaction::Transaction};

/// A value to be agreed upon by consensus.
/// Wraps transaction hashes with priority for ordering.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ConsensusValue {
    /// Hash of the transaction or minting tx
    pub tx_hash: [u8; 32],
    /// Priority (PoW difficulty for minting, timestamp for regular tx)
    pub priority: u64,
    /// Whether this is a minting transaction
    pub is_minting: bool,
}

impl bth_crypto_digestible::Digestible for ConsensusValue {
    fn append_to_transcript<DT: bth_crypto_digestible::DigestTranscript>(
        &self,
        context: &'static [u8],
        transcript: &mut DT,
    ) {
        self.tx_hash.append_to_transcript(context, transcript);
        self.priority.append_to_transcript(context, transcript);
        self.is_minting.append_to_transcript(context, transcript);
    }
}

impl std::fmt::Display for ConsensusValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CV({}...{}, p={}, m={})",
            hex::encode(&self.tx_hash[0..4]),
            hex::encode(&self.tx_hash[28..32]),
            self.priority,
            self.is_minting
        )
    }
}

/// Messages passed between test nodes
pub enum TestNodeMessage {
    /// A minting transaction (coinbase) to propose
    MintingTx(MintingTx),
    /// A regular transaction to propose
    Transaction(Transaction),
    /// SCP consensus message from a peer
    ScpMsg(Arc<Msg<ConsensusValue>>),
    /// Signal to stop the node
    Stop,
}
