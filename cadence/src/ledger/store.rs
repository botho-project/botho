use lmdb::{Database, Environment, EnvironmentFlags, Transaction, WriteFlags};
use std::fs;
use std::path::Path;
use tracing::info;

use super::{ChainState, LedgerError};
use crate::block::Block;

/// LMDB-backed ledger storage
pub struct Ledger {
    env: Environment,
    /// blocks: height -> Block
    blocks_db: Database,
    /// metadata: key -> value (for chain state)
    meta_db: Database,
}

// Metadata keys (fixed size for LMDB compatibility)
const META_HEIGHT: &[u8; 6] = b"height";
const META_TIP_HASH: &[u8; 8] = b"tip_hash";
const META_TOTAL_MINED: &[u8; 11] = b"total_mined";
const META_DIFFICULTY: &[u8; 10] = b"difficulty";

impl Ledger {
    /// Open or create a ledger at the given path
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        // Create directory if needed
        fs::create_dir_all(path).map_err(|e| {
            LedgerError::Database(lmdb::Error::Other(e.raw_os_error().unwrap_or(0)))
        })?;

        let env = Environment::new()
            .set_flags(EnvironmentFlags::NO_SUB_DIR)
            .set_max_dbs(2)
            .set_map_size(1024 * 1024 * 1024) // 1GB
            .open(path.join("ledger.mdb").as_ref())?;

        let blocks_db = env.create_db(Some("blocks"), lmdb::DatabaseFlags::empty())?;
        let meta_db = env.create_db(Some("meta"), lmdb::DatabaseFlags::empty())?;

        let ledger = Self {
            env,
            blocks_db,
            meta_db,
        };

        // Initialize with genesis if empty
        if ledger.get_chain_state()?.height == 0 {
            let state = ledger.get_chain_state()?;
            if state.tip_hash == [0u8; 32] {
                info!("Initializing ledger with genesis block");
                ledger.init_genesis()?;
            }
        }

        Ok(ledger)
    }

    /// Initialize the ledger with the genesis block
    fn init_genesis(&self) -> Result<(), LedgerError> {
        let genesis = Block::genesis();
        let mut txn = self.env.begin_rw_txn()?;

        // Store genesis block
        let block_bytes = bincode::serialize(&genesis)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        txn.put(
            self.blocks_db,
            &0u64.to_le_bytes(),
            &block_bytes,
            WriteFlags::empty(),
        )?;

        // Initialize metadata
        let genesis_hash = genesis.hash();
        txn.put(self.meta_db, META_HEIGHT, &0u64.to_le_bytes(), WriteFlags::empty())?;
        txn.put(self.meta_db, META_TIP_HASH, &genesis_hash, WriteFlags::empty())?;
        txn.put(self.meta_db, META_TOTAL_MINED, &0u64.to_le_bytes(), WriteFlags::empty())?;
        txn.put(
            self.meta_db,
            META_DIFFICULTY,
            &crate::node::miner::INITIAL_DIFFICULTY.to_le_bytes(),
            WriteFlags::empty(),
        )?;

        txn.commit()?;
        Ok(())
    }

    /// Get the current chain state
    pub fn get_chain_state(&self) -> Result<ChainState, LedgerError> {
        let txn = self.env.begin_ro_txn()?;

        let height = match txn.get(self.meta_db, &META_HEIGHT) {
            Ok(bytes) => u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])),
            Err(lmdb::Error::NotFound) => 0,
            Err(e) => return Err(e.into()),
        };

        let tip_hash = match txn.get(self.meta_db, &META_TIP_HASH) {
            Ok(bytes) => bytes.try_into().unwrap_or([0u8; 32]),
            Err(lmdb::Error::NotFound) => [0u8; 32],
            Err(e) => return Err(e.into()),
        };

        let total_mined = match txn.get(self.meta_db, &META_TOTAL_MINED) {
            Ok(bytes) => u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])),
            Err(lmdb::Error::NotFound) => 0,
            Err(e) => return Err(e.into()),
        };

        let difficulty = match txn.get(self.meta_db, &META_DIFFICULTY) {
            Ok(bytes) => u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])),
            Err(lmdb::Error::NotFound) => crate::node::miner::INITIAL_DIFFICULTY,
            Err(e) => return Err(e.into()),
        };

        Ok(ChainState {
            height,
            tip_hash,
            total_mined,
            difficulty,
        })
    }

    /// Get a block by height
    pub fn get_block(&self, height: u64) -> Result<Block, LedgerError> {
        let txn = self.env.begin_ro_txn()?;

        let bytes = txn
            .get(self.blocks_db, &height.to_le_bytes())
            .map_err(|_| LedgerError::BlockNotFound(height))?;

        bincode::deserialize(bytes).map_err(|e| LedgerError::Serialization(e.to_string()))
    }

    /// Get the tip (latest) block
    pub fn get_tip(&self) -> Result<Block, LedgerError> {
        let state = self.get_chain_state()?;
        self.get_block(state.height)
    }

    /// Add a new block to the chain
    pub fn add_block(&self, block: &Block) -> Result<(), LedgerError> {
        let state = self.get_chain_state()?;

        // Validate block height
        let expected_height = state.height + 1;
        if block.height() != expected_height {
            return Err(LedgerError::InvalidBlock(format!(
                "Expected height {}, got {}",
                expected_height,
                block.height()
            )));
        }

        // Validate prev_block_hash
        if block.header.prev_block_hash != state.tip_hash {
            return Err(LedgerError::InvalidBlock(
                "Previous block hash mismatch".to_string(),
            ));
        }

        // Validate PoW
        if !block.header.is_valid_pow() {
            return Err(LedgerError::InvalidBlock("Invalid proof of work".to_string()));
        }

        // Store block and update metadata
        let mut txn = self.env.begin_rw_txn()?;

        let block_bytes =
            bincode::serialize(block).map_err(|e| LedgerError::Serialization(e.to_string()))?;

        txn.put(
            self.blocks_db,
            &block.height().to_le_bytes(),
            &block_bytes,
            WriteFlags::empty(),
        )?;

        let new_hash = block.hash();
        let new_height = block.height();
        let new_total_mined = state.total_mined + block.mining_tx.reward;

        txn.put(
            self.meta_db,
            META_HEIGHT,
            &new_height.to_le_bytes(),
            WriteFlags::empty(),
        )?;
        txn.put(self.meta_db, META_TIP_HASH, &new_hash, WriteFlags::empty())?;
        txn.put(
            self.meta_db,
            META_TOTAL_MINED,
            &new_total_mined.to_le_bytes(),
            WriteFlags::empty(),
        )?;

        txn.commit()?;

        info!(
            "Added block {} with hash {}",
            new_height,
            hex::encode(&new_hash[0..8])
        );

        Ok(())
    }

    /// Update the difficulty in chain state
    pub fn set_difficulty(&self, difficulty: u64) -> Result<(), LedgerError> {
        let mut txn = self.env.begin_rw_txn()?;
        txn.put(
            self.meta_db,
            META_DIFFICULTY,
            &difficulty.to_le_bytes(),
            WriteFlags::empty(),
        )?;
        txn.commit()?;
        Ok(())
    }

    /// Get blocks in a range (for syncing)
    pub fn get_blocks(&self, start_height: u64, count: usize) -> Result<Vec<Block>, LedgerError> {
        let txn = self.env.begin_ro_txn()?;
        let mut blocks = Vec::with_capacity(count);

        for height in start_height..(start_height + count as u64) {
            match txn.get(self.blocks_db, &height.to_le_bytes()) {
                Ok(bytes) => {
                    let block: Block = bincode::deserialize(bytes)
                        .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                    blocks.push(block);
                }
                Err(lmdb::Error::NotFound) => break,
                Err(e) => return Err(e.into()),
            }
        }

        Ok(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ledger_open_and_genesis() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.height, 0);

        let genesis = ledger.get_block(0).unwrap();
        assert_eq!(genesis.height(), 0);
    }

    #[test]
    fn test_ledger_tip() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let tip = ledger.get_tip().unwrap();
        assert_eq!(tip.height(), 0);
    }
}
