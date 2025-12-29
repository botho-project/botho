// Copyright (c) 2018-2022 The Botho Foundation

use bt_blockchain_types::{BlockData, BlockMetadata};

/// A helper trait used by [crate::LedgerSyncService] for configuring what
/// metadata, if any, is appended for a given block.
pub trait BlockMetadataProvider {
    fn get_metadata(&self, block_data: &BlockData) -> Option<BlockMetadata>;
}

/// Default [BlockMetadataProvider], passes through the block metadata
/// unmodified.
#[derive(Copy, Clone, Default)]
pub struct PassThroughMetadataProvider {}

impl BlockMetadataProvider for PassThroughMetadataProvider {
    fn get_metadata(&self, block_data: &BlockData) -> Option<BlockMetadata> {
        block_data.metadata().cloned()
    }
}
