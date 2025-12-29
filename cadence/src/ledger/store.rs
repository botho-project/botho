use lmdb::{Cursor, Database, Environment, EnvironmentFlags, Transaction, WriteFlags};
use mc_account_keys::PublicAddress;
use mc_crypto_keys::{RistrettoPublic, RistrettoSignature};
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

use super::{ChainState, LedgerError};
use crate::block::Block;
use crate::transaction::{Transaction as CadenceTransaction, TxOutput, Utxo, UtxoId};

/// LMDB-backed ledger storage
pub struct Ledger {
    env: Environment,
    /// blocks: height -> Block
    blocks_db: Database,
    /// metadata: key -> value (for chain state)
    meta_db: Database,
    /// utxos: UtxoId (36 bytes) -> Utxo
    utxo_db: Database,
    /// address_index: (view_key || spend_key) (64 bytes) -> [UtxoId (36 bytes), ...]
    /// Maps addresses to their UTXOs for efficient balance lookups
    address_index_db: Database,
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
            .set_max_dbs(4)
            .set_map_size(1024 * 1024 * 1024) // 1GB
            .open(path.join("ledger.mdb").as_ref())?;

        let blocks_db = env.create_db(Some("blocks"), lmdb::DatabaseFlags::empty())?;
        let meta_db = env.create_db(Some("meta"), lmdb::DatabaseFlags::empty())?;
        let utxo_db = env.create_db(Some("utxos"), lmdb::DatabaseFlags::empty())?;
        let address_index_db = env.create_db(Some("address_index"), lmdb::DatabaseFlags::empty())?;

        let ledger = Self {
            env,
            blocks_db,
            meta_db,
            utxo_db,
            address_index_db,
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

        // Create UTXO from mining reward (coinbase)
        // Use block hash as the "tx_hash" for mining rewards
        let coinbase_utxo_id = UtxoId::new(new_hash, 0);
        let coinbase_utxo = Utxo {
            id: coinbase_utxo_id,
            output: TxOutput {
                amount: block.mining_tx.reward,
                recipient_view_key: block.mining_tx.recipient_view_key,
                recipient_spend_key: block.mining_tx.recipient_spend_key,
                output_public_key: block.mining_tx.output_public_key,
            },
            created_at: new_height,
        };
        let coinbase_bytes = bincode::serialize(&coinbase_utxo)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        txn.put(
            self.utxo_db,
            &coinbase_utxo_id.to_bytes(),
            &coinbase_bytes,
            WriteFlags::empty(),
        )?;
        // Add to address index
        self.add_to_address_index(&mut txn, &coinbase_utxo)?;
        debug!("Created coinbase UTXO at height {}", new_height);

        // Verify and process regular transactions
        for tx in &block.transactions {
            // Verify transaction signatures before processing
            self.verify_transaction(tx)?;

            let tx_hash = tx.hash();

            // Remove spent UTXOs (inputs)
            for input in &tx.inputs {
                let spent_id = UtxoId::new(input.tx_hash, input.output_index);

                // Get the UTXO first so we can remove it from the address index
                if let Ok(utxo_bytes) = txn.get(self.utxo_db, &spent_id.to_bytes()) {
                    if let Ok(spent_utxo) = bincode::deserialize::<Utxo>(utxo_bytes) {
                        // Remove from address index
                        self.remove_from_address_index(&mut txn, &spent_utxo)?;
                    }
                    // Remove from UTXO database
                    txn.del(self.utxo_db, &spent_id.to_bytes(), None)?;
                } else {
                    // UTXO not found - this is a validation error
                    // For now, log and continue (validation should catch this earlier)
                    debug!("Warning: UTXO not found when spending");
                }
            }

            // Add new UTXOs (outputs)
            for (idx, output) in tx.outputs.iter().enumerate() {
                let utxo_id = UtxoId::new(tx_hash, idx as u32);
                let utxo = Utxo {
                    id: utxo_id,
                    output: output.clone(),
                    created_at: new_height,
                };
                let utxo_bytes = bincode::serialize(&utxo)
                    .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                txn.put(
                    self.utxo_db,
                    &utxo_id.to_bytes(),
                    &utxo_bytes,
                    WriteFlags::empty(),
                )?;
                // Add to address index
                self.add_to_address_index(&mut txn, &utxo)?;
            }
        }

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
            "Added block {} with hash {} ({} txs)",
            new_height,
            hex::encode(&new_hash[0..8]),
            block.transactions.len()
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

    /// Get a specific UTXO by ID
    pub fn get_utxo(&self, id: &UtxoId) -> Result<Option<Utxo>, LedgerError> {
        let txn = self.env.begin_ro_txn()?;

        match txn.get(self.utxo_db, &id.to_bytes()) {
            Ok(bytes) => {
                let utxo: Utxo = bincode::deserialize(bytes)
                    .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                Ok(Some(utxo))
            }
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all UTXOs belonging to an address (using address index)
    pub fn get_utxos_for_address(&self, address: &PublicAddress) -> Result<Vec<Utxo>, LedgerError> {
        let view_key = address.view_public_key().to_bytes();
        let spend_key = address.spend_public_key().to_bytes();
        let addr_key = Self::address_key(&view_key, &spend_key);

        let txn = self.env.begin_ro_txn()?;

        // Look up UTXO IDs from the address index
        let id_bytes = match txn.get(self.address_index_db, &addr_key) {
            Ok(bytes) => bytes,
            Err(lmdb::Error::NotFound) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        // Parse each 36-byte UTXO ID and fetch the corresponding UTXO
        let mut utxos = Vec::new();
        for chunk in id_bytes.chunks(36) {
            if chunk.len() == 36 {
                if let Some(utxo_id) = UtxoId::from_bytes(chunk) {
                    // Fetch the UTXO by ID
                    if let Ok(utxo_bytes) = txn.get(self.utxo_db, &utxo_id.to_bytes()) {
                        if let Ok(utxo) = bincode::deserialize::<Utxo>(utxo_bytes) {
                            utxos.push(utxo);
                        }
                    }
                }
            }
        }

        Ok(utxos)
    }

    /// Get balance for an address (sum of all UTXOs)
    pub fn get_balance(&self, address: &PublicAddress) -> Result<u64, LedgerError> {
        let utxos = self.get_utxos_for_address(address)?;
        Ok(utxos.iter().map(|u| u.output.amount).sum())
    }

    /// Check if a UTXO exists (for transaction validation)
    pub fn utxo_exists(&self, id: &UtxoId) -> Result<bool, LedgerError> {
        let txn = self.env.begin_ro_txn()?;
        match txn.get(self.utxo_db, &id.to_bytes()) {
            Ok(_) => Ok(true),
            Err(lmdb::Error::NotFound) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Compute address key from view and spend keys for index lookup
    fn address_key(view_key: &[u8; 32], spend_key: &[u8; 32]) -> [u8; 64] {
        let mut key = [0u8; 64];
        key[0..32].copy_from_slice(view_key);
        key[32..64].copy_from_slice(spend_key);
        key
    }

    /// Add a UTXO ID to the address index
    fn add_to_address_index(
        &self,
        txn: &mut lmdb::RwTransaction,
        utxo: &Utxo,
    ) -> Result<(), LedgerError> {
        let addr_key = Self::address_key(
            &utxo.output.recipient_view_key,
            &utxo.output.recipient_spend_key,
        );

        // Get existing IDs or empty vec
        let existing = match txn.get(self.address_index_db, &addr_key) {
            Ok(bytes) => bytes.to_vec(),
            Err(lmdb::Error::NotFound) => Vec::new(),
            Err(e) => return Err(e.into()),
        };

        // Append the new UTXO ID
        let mut ids = existing;
        ids.extend_from_slice(&utxo.id.to_bytes());

        txn.put(
            self.address_index_db,
            &addr_key,
            &ids,
            WriteFlags::empty(),
        )?;

        Ok(())
    }

    /// Remove a UTXO ID from the address index
    fn remove_from_address_index(
        &self,
        txn: &mut lmdb::RwTransaction,
        utxo: &Utxo,
    ) -> Result<(), LedgerError> {
        let addr_key = Self::address_key(
            &utxo.output.recipient_view_key,
            &utxo.output.recipient_spend_key,
        );

        // Get existing IDs
        let existing = match txn.get(self.address_index_db, &addr_key) {
            Ok(bytes) => bytes.to_vec(),
            Err(lmdb::Error::NotFound) => return Ok(()), // Nothing to remove
            Err(e) => return Err(e.into()),
        };

        // Filter out the removed UTXO ID
        let utxo_id_bytes = utxo.id.to_bytes();
        let filtered: Vec<u8> = existing
            .chunks(36)
            .filter(|chunk| chunk != &utxo_id_bytes)
            .flat_map(|chunk| chunk.iter().copied())
            .collect();

        if filtered.is_empty() {
            // No more UTXOs for this address, remove the entry
            let _ = txn.del(self.address_index_db, &addr_key, None);
        } else {
            txn.put(
                self.address_index_db,
                &addr_key,
                &filtered,
                WriteFlags::empty(),
            )?;
        }

        Ok(())
    }

    /// Verify all signatures in a transaction
    ///
    /// For each input, this looks up the UTXO being spent and verifies
    /// the signature against the UTXO's spend public key.
    pub fn verify_transaction(&self, tx: &CadenceTransaction) -> Result<(), LedgerError> {
        let signing_hash = tx.signing_hash();

        for (i, input) in tx.inputs.iter().enumerate() {
            // Look up the UTXO being spent
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            let utxo = self.get_utxo(&utxo_id)?.ok_or_else(|| {
                LedgerError::InvalidBlock(format!(
                    "Input {} references non-existent UTXO {}:{}",
                    i,
                    hex::encode(&input.tx_hash[0..8]),
                    input.output_index
                ))
            })?;

            // Reconstruct the public spend key from the UTXO
            let spend_public = RistrettoPublic::try_from(&utxo.output.recipient_spend_key[..])
                .map_err(|_| {
                    LedgerError::InvalidBlock(format!(
                        "Input {} has invalid spend key in UTXO",
                        i
                    ))
                })?;

            // Parse the signature
            let signature = RistrettoSignature::try_from(input.signature.as_slice()).map_err(
                |_| {
                    LedgerError::InvalidBlock(format!(
                        "Input {} has invalid signature format (expected 64 bytes, got {})",
                        i,
                        input.signature.len()
                    ))
                },
            )?;

            // Verify the signature
            spend_public
                .verify_schnorrkel(b"cadence-tx-v1", &signing_hash, &signature)
                .map_err(|_| {
                    LedgerError::InvalidBlock(format!(
                        "Input {} has invalid signature",
                        i
                    ))
                })?;
        }

        Ok(())
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
