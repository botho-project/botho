// Copyright (c) 2018-2022 The Botho Foundation

use crate::{TransactionFetcherError, TransactionsFetcher};
use bt_blockchain_types::{Block, BlockData};
use bt_common::ResponderId;
use bt_ledger_db::Ledger;

impl TransactionFetcherError for String {}

#[derive(Clone)]
pub struct MockTransactionsFetcher<L: Ledger + Sync> {
    pub ledger: L,
}

impl<L: Ledger + Sync> MockTransactionsFetcher<L> {
    pub fn new(ledger: L) -> Self {
        Self { ledger }
    }
}

impl<L: Ledger + Sync> TransactionsFetcher for MockTransactionsFetcher<L> {
    type Error = String;

    fn get_block_data(
        &self,
        _safe_responder_ids: &[ResponderId],
        block: &Block,
    ) -> Result<BlockData, Self::Error> {
        self.ledger
            .get_block_data(block.index)
            .map_err(|e| format!("Error getting data for block #{}: {:?}", block.index, e))
    }
}
