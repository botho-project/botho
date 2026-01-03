use bth_account_keys::PublicAddress;
use bth_transaction_types::{Network, TAG_WEIGHT_SCALE};
use heed::types::{Bytes, U64};
use heed::{Database, Env, EnvOpenOptions, RwTxn};
use rand::Rng;
use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::{ChainState, LedgerError};
use crate::block::Block;
use crate::decoy_selection::{DecoySelectionError, GammaDecoySelector, OutputCandidate};
use crate::transaction::{Transaction as BothoTransaction, TxOutput, Utxo, UtxoId};

/// LMDB-backed ledger storage using heed
pub struct Ledger {
    env: Env,
    /// The network this ledger belongs to
    network: Network,
    /// blocks: height (u64) -> Block (bytes)
    blocks_db: Database<U64<heed::byteorder::LE>, Bytes>,
    /// metadata: key (bytes) -> value (bytes)
    meta_db: Database<Bytes, Bytes>,
    /// utxos: UtxoId (36 bytes) -> Utxo (bytes)
    utxo_db: Database<Bytes, Bytes>,
    /// address_index: target_key (32 bytes) -> [UtxoId (36 bytes), ...]
    /// Maps target keys to their UTXOs for efficient lookups
    address_index_db: Database<Bytes, Bytes>,
    /// key_images: key_image (32 bytes) -> height (8 bytes)
    /// Tracks spent key images to prevent double-spending with ring signatures.
    key_images_db: Database<Bytes, Bytes>,
    /// tx_index: tx_hash (32 bytes) -> TxLocation (12 bytes: height u64 + tx_index u32)
    /// Maps transaction hashes to their location for fast lookups (exchange integration).
    tx_index_db: Database<Bytes, Bytes>,
    /// cluster_wealth: cluster_id (8 bytes) -> wealth (8 bytes)
    /// Tracks total value per cluster tag across all UTXOs for progressive fee calculation.
    /// Note: This is an approximation - with ring signatures, we cannot know which UTXO
    /// was actually spent, so spent UTXOs still contribute to cluster wealth until
    /// eventually removed by UTXO pruning (if implemented).
    cluster_wealth_db: Database<Bytes, Bytes>,
}

// Metadata keys
const META_HEIGHT: &[u8] = b"height";
const META_TIP_HASH: &[u8] = b"tip_hash";

/// Location of a transaction in the blockchain.
/// Used for fast transaction lookups (exchange integration).
#[derive(Debug, Clone, Copy)]
pub struct TxLocation {
    /// Block height containing the transaction
    pub block_height: u64,
    /// Index of the transaction within the block
    pub tx_index: u32,
}
const META_TOTAL_MINED: &[u8] = b"total_mined";
const META_FEES_BURNED: &[u8] = b"fees_burned";
const META_DIFFICULTY: &[u8] = b"difficulty";

// EmissionController state
const META_TOTAL_TX: &[u8] = b"total_tx";
const META_EPOCH_TX: &[u8] = b"epoch_tx";
const META_EPOCH_EMISSION: &[u8] = b"epoch_emission";
const META_EPOCH_BURNS: &[u8] = b"epoch_burns";
const META_CURRENT_REWARD: &[u8] = b"current_reward";

impl Ledger {
    /// Open or create a ledger at the given path (defaults to Testnet for backward compatibility)
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        Self::open_for_network(path, Network::Testnet)
    }

    /// Open or create a ledger at the given path for a specific network.
    ///
    /// The ledger will be initialized with the appropriate genesis block
    /// for the specified network if it's empty.
    pub fn open_for_network(path: &Path, network: Network) -> Result<Self, LedgerError> {
        // Create directory if needed
        fs::create_dir_all(path).map_err(|e| {
            LedgerError::Database(format!("Failed to create directory: {}", e))
        })?;

        // SAFETY: LMDB environment opening is marked unsafe in heed because:
        // 1. The same LMDB environment must not be opened multiple times concurrently
        // 2. The path must exist and be accessible
        // 3. The environment must not outlive the filesystem path
        // We satisfy these by: only opening once per LedgerStore, creating the directory
        // first, and storing the Env in the struct which owns it for its lifetime.
        let env = unsafe {
            EnvOpenOptions::new()
                .max_dbs(7)  // Increased for cluster_wealth_db
                .map_size(1024 * 1024 * 1024) // 1GB
                .open(path)
        }.map_err(|e| LedgerError::Database(format!("Failed to open environment: {}", e)))?;

        // Create/open databases
        let mut wtxn = env.write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        let blocks_db = env.create_database(&mut wtxn, Some("blocks"))
            .map_err(|e| LedgerError::Database(format!("Failed to create blocks db: {}", e)))?;
        let meta_db = env.create_database(&mut wtxn, Some("meta"))
            .map_err(|e| LedgerError::Database(format!("Failed to create meta db: {}", e)))?;
        let utxo_db = env.create_database(&mut wtxn, Some("utxos"))
            .map_err(|e| LedgerError::Database(format!("Failed to create utxos db: {}", e)))?;
        let address_index_db = env.create_database(&mut wtxn, Some("address_index"))
            .map_err(|e| LedgerError::Database(format!("Failed to create address_index db: {}", e)))?;
        let key_images_db = env.create_database(&mut wtxn, Some("key_images"))
            .map_err(|e| LedgerError::Database(format!("Failed to create key_images db: {}", e)))?;
        let tx_index_db = env.create_database(&mut wtxn, Some("tx_index"))
            .map_err(|e| LedgerError::Database(format!("Failed to create tx_index db: {}", e)))?;
        let cluster_wealth_db = env.create_database(&mut wtxn, Some("cluster_wealth"))
            .map_err(|e| LedgerError::Database(format!("Failed to create cluster_wealth db: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        let ledger = Self {
            env,
            network,
            blocks_db,
            meta_db,
            utxo_db,
            address_index_db,
            key_images_db,
            tx_index_db,
            cluster_wealth_db,
        };

        // Initialize with genesis if empty
        if ledger.get_chain_state()?.height == 0 {
            let state = ledger.get_chain_state()?;
            if state.tip_hash == [0u8; 32] {
                info!(network = %network, "Initializing ledger with genesis block");
                ledger.init_genesis()?;
            }
        }

        Ok(ledger)
    }

    /// Get the network this ledger belongs to.
    pub fn network(&self) -> Network {
        self.network
    }

    /// Initialize the ledger with the genesis block for this network.
    fn init_genesis(&self) -> Result<(), LedgerError> {
        let genesis = Block::genesis_for_network(self.network);
        let mut wtxn = self.env.write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        // Store genesis block
        let block_bytes = bincode::serialize(&genesis)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        self.blocks_db.put(&mut wtxn, &0u64, &block_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put block: {}", e)))?;

        // Initialize metadata
        let genesis_hash = genesis.hash();
        self.meta_db.put(&mut wtxn, META_HEIGHT, &0u64.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_TIP_HASH, &genesis_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_TOTAL_MINED, &0u64.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_FEES_BURNED, &0u64.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_DIFFICULTY, &crate::node::minter::INITIAL_DIFFICULTY.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Get the current chain state
    pub fn get_chain_state(&self) -> Result<ChainState, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let height = self.meta_db.get(&rtxn, META_HEIGHT)
            .map_err(|e| LedgerError::Database(format!("Failed to get height: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let tip_hash = self.meta_db.get(&rtxn, META_TIP_HASH)
            .map_err(|e| LedgerError::Database(format!("Failed to get tip_hash: {}", e)))?
            .map(|b| b.try_into().unwrap_or([0u8; 32]))
            .unwrap_or([0u8; 32]);

        let total_mined = self.meta_db.get(&rtxn, META_TOTAL_MINED)
            .map_err(|e| LedgerError::Database(format!("Failed to get total_mined: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let total_fees_burned = self.meta_db.get(&rtxn, META_FEES_BURNED)
            .map_err(|e| LedgerError::Database(format!("Failed to get fees_burned: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let difficulty = self.meta_db.get(&rtxn, META_DIFFICULTY)
            .map_err(|e| LedgerError::Database(format!("Failed to get difficulty: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(crate::node::minter::INITIAL_DIFFICULTY);

        // Get tip timestamp from the tip block (if exists)
        let tip_timestamp = if height > 0 {
            self.blocks_db.get(&rtxn, &height)
                .ok()
                .flatten()
                .and_then(|bytes| bincode::deserialize::<Block>(bytes).ok())
                .map(|block| block.header.timestamp)
                .unwrap_or(0)
        } else {
            0
        };

        // EmissionController state
        let total_tx = self.meta_db.get(&rtxn, META_TOTAL_TX)
            .map_err(|e| LedgerError::Database(format!("Failed to get total_tx: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let epoch_tx = self.meta_db.get(&rtxn, META_EPOCH_TX)
            .map_err(|e| LedgerError::Database(format!("Failed to get epoch_tx: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let epoch_emission = self.meta_db.get(&rtxn, META_EPOCH_EMISSION)
            .map_err(|e| LedgerError::Database(format!("Failed to get epoch_emission: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let epoch_burns = self.meta_db.get(&rtxn, META_EPOCH_BURNS)
            .map_err(|e| LedgerError::Database(format!("Failed to get epoch_burns: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let current_reward = self.meta_db.get(&rtxn, META_CURRENT_REWARD)
            .map_err(|e| LedgerError::Database(format!("Failed to get current_reward: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(crate::block::difficulty::INITIAL_REWARD);

        Ok(ChainState {
            height,
            tip_hash,
            tip_timestamp,
            total_mined,
            total_fees_burned,
            difficulty,
            total_tx,
            epoch_tx,
            epoch_emission,
            epoch_burns,
            current_reward,
        })
    }

    /// Get a block by height
    pub fn get_block(&self, height: u64) -> Result<Block, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let bytes = self.blocks_db.get(&rtxn, &height)
            .map_err(|e| LedgerError::Database(format!("Failed to get block: {}", e)))?
            .ok_or(LedgerError::BlockNotFound(height))?;

        bincode::deserialize(bytes).map_err(|e| LedgerError::Serialization(e.to_string()))
    }

    /// Get the tip (latest) block
    pub fn get_tip(&self) -> Result<Block, LedgerError> {
        let state = self.get_chain_state()?;
        self.get_block(state.height)
    }

    /// Get a block by its hash.
    ///
    /// Searches recent blocks (up to `lookback` blocks from tip) for a matching hash.
    /// This is used for compact block reconstruction when responding to GetBlockTxn requests.
    ///
    /// Returns `Ok(None)` if the block is not found within the lookback window.
    pub fn get_block_by_hash(
        &self,
        hash: &[u8; 32],
        lookback: u64,
    ) -> Result<Option<Block>, LedgerError> {
        let state = self.get_chain_state()?;

        // Quick check: is it the tip?
        if &state.tip_hash == hash {
            return self.get_block(state.height).map(Some);
        }

        // Search recent blocks
        let start_height = state.height.saturating_sub(lookback);
        for height in (start_height..state.height).rev() {
            match self.get_block(height) {
                Ok(block) => {
                    if &block.hash() == hash {
                        return Ok(Some(block));
                    }
                }
                Err(LedgerError::BlockNotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }

        Ok(None)
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
        let mut wtxn = self.env.write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        let block_bytes =
            bincode::serialize(block).map_err(|e| LedgerError::Serialization(e.to_string()))?;

        self.blocks_db.put(&mut wtxn, &block.height(), &block_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put block: {}", e)))?;

        let new_hash = block.hash();
        let new_height = block.height();
        let new_total_mined = state.total_mined + block.minting_tx.reward;

        // Sum transaction fees (these are burned, reducing circulating supply)
        let block_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
        let new_total_fees_burned = state.total_fees_burned + block_fees;

        // Create UTXO from minting reward (coinbase)
        let coinbase_utxo_id = UtxoId::new(new_hash, 0);
        let coinbase_utxo = Utxo {
            id: coinbase_utxo_id,
            output: block.minting_tx.to_tx_output(),
            created_at: new_height,
        };
        let coinbase_bytes = bincode::serialize(&coinbase_utxo)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        self.utxo_db.put(&mut wtxn, &coinbase_utxo_id.to_bytes(), &coinbase_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put coinbase utxo: {}", e)))?;
        // Add to address index
        self.add_to_address_index(&mut wtxn, &coinbase_utxo)?;
        // Update cluster wealth tracking
        self.update_cluster_wealth_for_output(&mut wtxn, &coinbase_utxo.output)?;
        debug!("Created coinbase UTXO at height {}", new_height);

        // Verify and process regular transactions
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            // Verify transaction signatures before processing
            self.verify_transaction(tx)?;

            let tx_hash = tx.hash();

            // Index transaction for fast lookups (exchange integration)
            self.add_tx_to_index(&mut wtxn, &tx_hash, new_height, tx_idx as u32)?;

            // Process spent inputs - record key images to prevent double-spend
            for input in tx.inputs.clsag() {
                self.record_key_image(&mut wtxn, &input.key_image, new_height)?;
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
                self.utxo_db.put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes)
                    .map_err(|e| LedgerError::Database(format!("Failed to put utxo: {}", e)))?;
                // Add to address index
                self.add_to_address_index(&mut wtxn, &utxo)?;
                // Update cluster wealth tracking
                self.update_cluster_wealth_for_output(&mut wtxn, output)?;
            }
        }

        self.meta_db.put(&mut wtxn, META_HEIGHT, &new_height.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_TIP_HASH, &new_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_TOTAL_MINED, &new_total_mined.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_FEES_BURNED, &new_total_fees_burned.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        info!(
            "Added block {} with hash {} ({} txs, {} fees burned)",
            new_height,
            hex::encode(&new_hash[0..8]),
            block.transactions.len(),
            block_fees
        );

        Ok(())
    }

    /// Update the difficulty in chain state
    pub fn set_difficulty(&self, difficulty: u64) -> Result<(), LedgerError> {
        let mut wtxn = self.env.write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_DIFFICULTY, &difficulty.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;
        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Update emission controller state in chain state
    pub fn update_emission_state(
        &self,
        difficulty: u64,
        total_tx: u64,
        epoch_tx: u64,
        epoch_emission: u64,
        epoch_burns: u64,
        current_reward: u64,
    ) -> Result<(), LedgerError> {
        let mut wtxn = self.env.write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        self.meta_db.put(&mut wtxn, META_DIFFICULTY, &difficulty.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_TOTAL_TX, &total_tx.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_tx: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_EPOCH_TX, &epoch_tx.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_tx: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_EPOCH_EMISSION, &epoch_emission.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_emission: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_EPOCH_BURNS, &epoch_burns.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_burns: {}", e)))?;
        self.meta_db.put(&mut wtxn, META_CURRENT_REWARD, &current_reward.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put current_reward: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Get blocks in a range (for syncing)
    pub fn get_blocks(&self, start_height: u64, count: usize) -> Result<Vec<Block>, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;
        let mut blocks = Vec::with_capacity(count);

        for height in start_height..(start_height + count as u64) {
            match self.blocks_db.get(&rtxn, &height) {
                Ok(Some(bytes)) => {
                    let block: Block = bincode::deserialize(bytes)
                        .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                    blocks.push(block);
                }
                Ok(None) => break,
                Err(e) => return Err(LedgerError::Database(format!("Failed to get block: {}", e))),
            }
        }

        Ok(blocks)
    }

    /// Get a specific UTXO by ID
    pub fn get_utxo(&self, id: &UtxoId) -> Result<Option<Utxo>, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        match self.utxo_db.get(&rtxn, &id.to_bytes()) {
            Ok(Some(bytes)) => {
                let utxo: Utxo = bincode::deserialize(bytes)
                    .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                Ok(Some(utxo))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(LedgerError::Database(format!("Failed to get utxo: {}", e))),
        }
    }

    /// Get all UTXOs belonging to an address (using address index)
    pub fn get_utxos_for_address(&self, address: &PublicAddress) -> Result<Vec<Utxo>, LedgerError> {
        let view_key = address.view_public_key().to_bytes();
        let spend_key = address.spend_public_key().to_bytes();
        let addr_key = Self::address_key(&view_key, &spend_key);

        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Look up UTXO IDs from the address index
        let id_bytes = match self.address_index_db.get(&rtxn, &addr_key) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => return Ok(Vec::new()),
            Err(e) => return Err(LedgerError::Database(format!("Failed to get address index: {}", e))),
        };

        // Parse each 36-byte UTXO ID and fetch the corresponding UTXO
        let mut utxos = Vec::new();
        for chunk in id_bytes.chunks(36) {
            if chunk.len() == 36 {
                if let Some(utxo_id) = UtxoId::from_bytes(chunk) {
                    // Fetch the UTXO by ID
                    if let Ok(Some(utxo_bytes)) = self.utxo_db.get(&rtxn, &utxo_id.to_bytes()) {
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
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;
        match self.utxo_db.get(&rtxn, &id.to_bytes()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(LedgerError::Database(format!("Failed to get utxo: {}", e))),
        }
    }

    /// Get a UTXO by its target_key (one-time stealth public key)
    pub fn get_utxo_by_target_key(&self, target_key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Look up UTXO IDs from the target_key index
        let id_bytes = match self.address_index_db.get(&rtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(e) => return Err(LedgerError::Database(format!("Failed to get address index: {}", e))),
        };

        // Get the first UTXO ID (there should typically be only one per target_key)
        if id_bytes.len() >= 36 {
            if let Some(utxo_id) = UtxoId::from_bytes(&id_bytes[0..36]) {
                if let Ok(Some(utxo_bytes)) = self.utxo_db.get(&rtxn, &utxo_id.to_bytes()) {
                    if let Ok(utxo) = bincode::deserialize::<Utxo>(utxo_bytes) {
                        return Ok(Some(utxo));
                    }
                }
            }
        }

        Ok(None)
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
        wtxn: &mut RwTxn,
        utxo: &Utxo,
    ) -> Result<(), LedgerError> {
        // Index by target_key for UTXO retrieval after stealth detection
        let target_key = &utxo.output.target_key;

        // Get existing IDs or empty vec
        let existing = match self.address_index_db.get(wtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => Vec::new(),
            Err(e) => return Err(LedgerError::Database(format!("Failed to get address index: {}", e))),
        };

        // Append the new UTXO ID
        let mut ids = existing;
        ids.extend_from_slice(&utxo.id.to_bytes());

        self.address_index_db.put(wtxn, target_key.as_slice(), &ids)
            .map_err(|e| LedgerError::Database(format!("Failed to put address index: {}", e)))?;

        Ok(())
    }

    /// Remove a UTXO ID from the address index
    fn remove_from_address_index(
        &self,
        wtxn: &mut RwTxn,
        utxo: &Utxo,
    ) -> Result<(), LedgerError> {
        let target_key = &utxo.output.target_key;

        // Get existing IDs
        let existing = match self.address_index_db.get(wtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => return Ok(()), // Nothing to remove
            Err(e) => return Err(LedgerError::Database(format!("Failed to get address index: {}", e))),
        };

        // Filter out the removed UTXO ID
        let utxo_id_bytes = utxo.id.to_bytes();
        let filtered: Vec<u8> = existing
            .chunks(36)
            .filter(|chunk| chunk != &utxo_id_bytes)
            .flat_map(|chunk| chunk.iter().copied())
            .collect();

        if filtered.is_empty() {
            // No more UTXOs for this target key, remove the entry
            let _ = self.address_index_db.delete(wtxn, target_key.as_slice());
        } else {
            self.address_index_db.put(wtxn, target_key.as_slice(), &filtered)
                .map_err(|e| LedgerError::Database(format!("Failed to put address index: {}", e)))?;
        }

        Ok(())
    }

    /// Verify all signatures in a transaction
    pub fn verify_transaction(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        // Verify key images haven't been spent (double-spend check)
        for (i, input) in tx.inputs.clsag().iter().enumerate() {
            if let Ok(Some(spent_height)) = self.is_key_image_spent(&input.key_image) {
                return Err(LedgerError::InvalidBlock(format!(
                    "Input {} uses key image already spent at height {}",
                    i, spent_height
                )));
            }
        }

        // Verify CLSAG ring signatures
        tx.verify_ring_signatures().map_err(|e| {
            LedgerError::InvalidBlock(format!("Invalid ring signature: {}", e))
        })?;

        Ok(())
    }

    // ========================================================================
    // Key Image Tracking (for Ring Signature Double-Spend Prevention)
    // ========================================================================

    /// Check if a key image has already been spent.
    pub fn is_key_image_spent(&self, key_image: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        match self.key_images_db.get(&rtxn, key_image.as_slice()) {
            Ok(Some(bytes)) if bytes.len() == 8 => {
                let height = u64::from_le_bytes(bytes.try_into().unwrap());
                Ok(Some(height))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(LedgerError::Database(format!("Failed to get key image: {}", e))),
        }
    }

    /// Record a key image as spent at the given block height.
    pub fn record_key_image(
        &self,
        wtxn: &mut RwTxn,
        key_image: &[u8; 32],
        height: u64,
    ) -> Result<(), LedgerError> {
        // Check if already exists
        if let Ok(Some(_)) = self.key_images_db.get(wtxn, key_image.as_slice()) {
            return Err(LedgerError::InvalidBlock("Key image already spent (double-spend)".to_string()));
        }

        self.key_images_db.put(wtxn, key_image.as_slice(), &height.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put key image: {}", e)))
    }

    /// Get a random sample of UTXOs for use as decoys in ring signatures.
    pub fn get_decoy_outputs(
        &self,
        count: usize,
        exclude: &[[u8; 32]], // target_keys to exclude
        min_confirmations: u64,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        use rand::seq::SliceRandom;

        let state = self.get_chain_state()?;
        let max_height = state.height.saturating_sub(min_confirmations);

        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Collect all eligible UTXOs
        let mut candidates: Vec<TxOutput> = Vec::new();

        // Iterate over all UTXOs
        let iter = self.utxo_db.iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    // Check confirmations
                    if utxo.created_at <= max_height {
                        // Check exclusion list
                        if !exclude.contains(&utxo.output.target_key) {
                            candidates.push(utxo.output);
                        }
                    }
                }
            }
        }

        // Randomly sample from candidates
        let mut rng = rand::thread_rng();
        candidates.shuffle(&mut rng);
        candidates.truncate(count);

        Ok(candidates)
    }

    /// Get decoys using OSPEAD-style gamma-weighted selection.
    ///
    /// This method selects decoys to match expected spend age patterns, making it
    /// harder for observers to distinguish real spends from decoys based on output age.
    /// Uses a gamma distribution to model real-world spending behavior.
    ///
    /// # Arguments
    /// * `count` - Number of decoys to select
    /// * `exclude` - Target keys to exclude (the real inputs)
    /// * `min_confirmations` - Minimum block confirmations required
    /// * `selector` - Optional custom gamma selector (uses default if None)
    ///
    /// # Returns
    /// Selected decoys weighted by age distribution
    pub fn get_decoy_outputs_ospead<R: Rng>(
        &self,
        count: usize,
        exclude: &[[u8; 32]],
        min_confirmations: u64,
        selector: Option<&GammaDecoySelector>,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        let state = self.get_chain_state()?;
        let current_height = state.height;
        let max_height = current_height.saturating_sub(min_confirmations);

        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Collect all eligible UTXOs with age information
        let mut candidates: Vec<OutputCandidate> = Vec::new();

        let iter = self.utxo_db.iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    // Check confirmations
                    if utxo.created_at <= max_height {
                        // Check exclusion list
                        if !exclude.contains(&utxo.output.target_key) {
                            candidates.push(OutputCandidate::from_utxo(&utxo, current_height));
                        }
                    }
                }
            }
        }

        // Use provided selector or create default
        let default_selector = GammaDecoySelector::new();
        let selector = selector.unwrap_or(&default_selector);

        // Use OSPEAD selection
        selector
            .select_decoys(&candidates, count, exclude, current_height, rng)
            .map_err(|e| match e {
                DecoySelectionError::InsufficientCandidates { required, available } => {
                    LedgerError::InvalidBlock(format!(
                        "Insufficient decoy candidates: need {}, have {}. \
                         The ledger needs more confirmed outputs for private transactions.",
                        required, available
                    ))
                }
                DecoySelectionError::InvalidDistribution => {
                    LedgerError::InvalidBlock("Invalid gamma distribution parameters".to_string())
                }
            })
    }

    /// Get decoys using OSPEAD selection, targeting specific ages for better anonymity.
    ///
    /// This version samples decoy ages based on the gamma distribution, then finds
    /// outputs that best match those ages. This creates rings where the age distribution
    /// matches expected real spending patterns.
    ///
    /// # Arguments
    /// * `count` - Number of decoys to select
    /// * `exclude` - Target keys to exclude
    /// * `min_confirmations` - Minimum block confirmations
    /// * `real_input_age` - Age in blocks of the real input being spent
    /// * `selector` - Optional custom gamma selector
    ///
    /// # Returns
    /// Selected decoys with age distribution matching spend patterns
    pub fn get_decoy_outputs_for_input<R: Rng>(
        &self,
        count: usize,
        exclude: &[[u8; 32]],
        min_confirmations: u64,
        real_input_age: u64,
        selector: Option<&GammaDecoySelector>,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        let state = self.get_chain_state()?;
        let current_height = state.height;
        let max_height = current_height.saturating_sub(min_confirmations);

        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let mut candidates: Vec<OutputCandidate> = Vec::new();

        let iter = self.utxo_db.iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    if utxo.created_at <= max_height {
                        if !exclude.contains(&utxo.output.target_key) {
                            candidates.push(OutputCandidate::from_utxo(&utxo, current_height));
                        }
                    }
                }
            }
        }

        let default_selector = GammaDecoySelector::new();
        let selector = selector.unwrap_or(&default_selector);

        selector
            .select_decoys_for_input(&candidates, count, exclude, real_input_age, rng)
            .map_err(|e| match e {
                DecoySelectionError::InsufficientCandidates { required, available } => {
                    LedgerError::InvalidBlock(format!(
                        "Insufficient decoy candidates: need {}, have {}",
                        required, available
                    ))
                }
                DecoySelectionError::InvalidDistribution => {
                    LedgerError::InvalidBlock("Invalid gamma distribution parameters".to_string())
                }
            })
    }

    /// Calculate effective anonymity for a ring given member ages.
    ///
    /// Returns a value between 1 (no privacy) and ring_size (perfect privacy).
    /// A value of 10+ with ring size 20 indicates good anonymity (1-in-10 or better).
    pub fn effective_anonymity(ring_ages: &[u64], selector: Option<&GammaDecoySelector>) -> f64 {
        let default_selector = GammaDecoySelector::new();
        let selector = selector.unwrap_or(&default_selector);
        selector.effective_anonymity(ring_ages)
    }

    // ========================================================================
    // Transaction Index (for Exchange Integration)
    // ========================================================================

    /// Add a transaction to the index.
    fn add_tx_to_index(
        &self,
        wtxn: &mut RwTxn,
        tx_hash: &[u8; 32],
        block_height: u64,
        tx_index: u32,
    ) -> Result<(), LedgerError> {
        // Encode location as 12 bytes: height (8) + tx_index (4)
        let mut location_bytes = [0u8; 12];
        location_bytes[0..8].copy_from_slice(&block_height.to_le_bytes());
        location_bytes[8..12].copy_from_slice(&tx_index.to_le_bytes());

        self.tx_index_db
            .put(wtxn, tx_hash.as_slice(), &location_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to index transaction: {}", e)))
    }

    /// Get the location of a transaction by its hash.
    ///
    /// Returns `Ok(Some(TxLocation))` if found, `Ok(None)` if not found.
    pub fn get_transaction_location(&self, tx_hash: &[u8; 32]) -> Result<Option<TxLocation>, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        match self.tx_index_db.get(&rtxn, tx_hash.as_slice()) {
            Ok(Some(bytes)) if bytes.len() == 12 => {
                let block_height = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                let tx_index = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                Ok(Some(TxLocation { block_height, tx_index }))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(LedgerError::Database(format!("Failed to get tx location: {}", e))),
        }
    }

    /// Get a transaction by its hash.
    ///
    /// Returns the transaction along with its block height and confirmation count.
    pub fn get_transaction(&self, tx_hash: &[u8; 32]) -> Result<Option<(BothoTransaction, u64, u64)>, LedgerError> {
        // Look up location in index
        let location = match self.get_transaction_location(tx_hash)? {
            Some(loc) => loc,
            None => return Ok(None),
        };

        // Get the block
        let block = self.get_block(location.block_height)?;

        // Get the transaction from the block
        let tx = block
            .transactions
            .get(location.tx_index as usize)
            .ok_or_else(|| LedgerError::Database("Transaction index out of bounds".to_string()))?;

        // Calculate confirmations
        let chain_state = self.get_chain_state()?;
        let confirmations = chain_state.height.saturating_sub(location.block_height) + 1;

        Ok(Some((tx.clone(), location.block_height, confirmations)))
    }

    /// Get the confirmation count for a transaction.
    ///
    /// Returns `Ok(Some(confirmations))` if found, `Ok(None)` if not found.
    /// Confirmations = current_height - tx_block_height + 1
    pub fn get_transaction_confirmations(&self, tx_hash: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        let location = match self.get_transaction_location(tx_hash)? {
            Some(loc) => loc,
            None => return Ok(None),
        };

        let chain_state = self.get_chain_state()?;
        let confirmations = chain_state.height.saturating_sub(location.block_height) + 1;
        Ok(Some(confirmations))
    }

    // ========================================================================
    // Cluster Wealth Tracking (for Progressive Fees)
    // ========================================================================
    //
    // # Privacy Implications
    //
    // Cluster wealth tracking enables progressive transaction fees but has privacy
    // considerations that users should understand:
    //
    // 1. **Cluster IDs are public**: Each transaction output has visible cluster tags
    //    that show what fraction of its value traces back to each cluster origin.
    //    This is inherent to the progressive fee design and visible on-chain.
    //
    // 2. **Wealth is observable**: Anyone can query cluster wealth from the public
    //    UTXO set. This reveals aggregate wealth concentrations but NOT individual
    //    wallet balances (UTXOs are stealth addresses).
    //
    // 3. **Ring signatures protect spending privacy**: While cluster wealth is visible,
    //    ring signatures hide which UTXO was actually spent in a transaction. The
    //    cluster tags on outputs inherit from the hidden real input's tags.
    //
    // 4. **Approximation due to ring signatures**: Since we cannot know which UTXO
    //    was spent (ring signature privacy), cluster wealth tracking is an
    //    approximation. Spent UTXOs continue contributing until explicitly pruned.
    //
    // 5. **Decay over time**: Cluster tags decay with each transaction (5% by default),
    //    so wealth attribution naturally fades as coins circulate.
    //
    // The progressive fee system intentionally uses visible cluster wealth to ensure
    // that large holders pay proportionally higher fees. This is a design choice that
    // trades some wealth privacy for fairer fee distribution.

    /// Update cluster wealth when a new output is created.
    ///
    /// Adds the output's weighted cluster contributions to the global wealth tracker.
    fn update_cluster_wealth_for_output(
        &self,
        wtxn: &mut RwTxn,
        output: &TxOutput,
    ) -> Result<(), LedgerError> {
        for entry in &output.cluster_tags.entries {
            // Contribution = output_amount × tag_weight / TAG_WEIGHT_SCALE
            let contribution = ((output.amount as u128) * (entry.weight as u128)
                / (TAG_WEIGHT_SCALE as u128)) as u64;

            if contribution > 0 {
                let cluster_key = entry.cluster_id.0.to_le_bytes();

                // Get current wealth
                let current = self.cluster_wealth_db
                    .get(wtxn, cluster_key.as_slice())
                    .map_err(|e| LedgerError::Database(format!("Failed to get cluster wealth: {}", e)))?
                    .map(|b: &[u8]| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
                    .unwrap_or(0);

                // Add contribution
                let new_wealth = current.saturating_add(contribution);
                self.cluster_wealth_db
                    .put(wtxn, cluster_key.as_slice(), &new_wealth.to_le_bytes())
                    .map_err(|e| LedgerError::Database(format!("Failed to update cluster wealth: {}", e)))?;
            }
        }
        Ok(())
    }

    /// Get the total wealth attributed to a specific cluster.
    ///
    /// Returns the sum of (amount × weight / TAG_WEIGHT_SCALE) for all UTXOs
    /// with tags referencing this cluster.
    pub fn get_cluster_wealth(&self, cluster_id: u64) -> Result<u64, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let cluster_key = cluster_id.to_le_bytes();
        match self.cluster_wealth_db.get(&rtxn, cluster_key.as_slice()) {
            Ok(Some(bytes)) if bytes.len() == 8 => {
                Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
            }
            Ok(_) => Ok(0),
            Err(e) => Err(LedgerError::Database(format!("Failed to get cluster wealth: {}", e))),
        }
    }

    /// Compute cluster wealth for a set of UTXOs identified by target keys.
    ///
    /// This is the primary method for wallets to estimate their cluster wealth
    /// for fee calculation. Wallets provide the target keys of their UTXOs,
    /// and this method returns the maximum cluster wealth across those UTXOs.
    ///
    /// # Arguments
    /// * `target_keys` - Target keys (stealth addresses) identifying the UTXOs
    ///
    /// # Returns
    /// A `ClusterWealthInfo` containing the maximum cluster wealth and breakdown
    pub fn compute_cluster_wealth_for_utxos(
        &self,
        target_keys: &[[u8; 32]],
    ) -> Result<ClusterWealthInfo, LedgerError> {
        use std::collections::HashMap;

        let mut cluster_wealths: HashMap<u64, u64> = HashMap::new();
        let mut total_value = 0u64;
        let mut utxo_count = 0usize;

        for target_key in target_keys {
            if let Some(utxo) = self.get_utxo_by_target_key(target_key)? {
                total_value = total_value.saturating_add(utxo.output.amount);
                utxo_count += 1;

                for entry in &utxo.output.cluster_tags.entries {
                    let contribution = ((utxo.output.amount as u128) * (entry.weight as u128)
                        / (TAG_WEIGHT_SCALE as u128)) as u64;
                    *cluster_wealths.entry(entry.cluster_id.0).or_insert(0) += contribution;
                }
            }
        }

        let max_cluster_wealth = cluster_wealths.values().copied().max().unwrap_or(0);
        let dominant_cluster = cluster_wealths
            .iter()
            .max_by_key(|(_, &wealth)| wealth)
            .map(|(&id, _)| id);

        Ok(ClusterWealthInfo {
            max_cluster_wealth,
            total_value,
            utxo_count,
            dominant_cluster_id: dominant_cluster,
            cluster_breakdown: cluster_wealths.into_iter().collect(),
        })
    }

    /// Get all cluster wealth entries for analytics.
    ///
    /// Returns all tracked cluster IDs and their total wealth.
    /// Useful for network-wide wealth distribution analysis.
    pub fn get_all_cluster_wealth(&self) -> Result<Vec<(u64, u64)>, LedgerError> {
        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let mut result = Vec::new();
        let iter = self.cluster_wealth_db.iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate cluster wealth: {}", e)))?;

        for item in iter {
            if let Ok((key, value)) = item {
                if key.len() == 8 && value.len() == 8 {
                    let cluster_id = u64::from_le_bytes(key.try_into().unwrap());
                    let wealth = u64::from_le_bytes(value.try_into().unwrap());
                    result.push((cluster_id, wealth));
                }
            }
        }

        Ok(result)
    }

    /// Rebuild cluster wealth index from UTXO set.
    ///
    /// Scans all UTXOs and rebuilds the cluster wealth index from scratch.
    /// Useful for database repair or migration.
    pub fn rebuild_cluster_wealth_index(&self) -> Result<usize, LedgerError> {
        use std::collections::HashMap;

        let rtxn = self.env.read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // First pass: compute wealth from all UTXOs
        let mut cluster_wealths: HashMap<u64, u64> = HashMap::new();
        let iter = self.utxo_db.iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate UTXOs: {}", e)))?;

        for item in iter {
            if let Ok((_, value)) = item {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    for entry in &utxo.output.cluster_tags.entries {
                        let contribution = ((utxo.output.amount as u128) * (entry.weight as u128)
                            / (TAG_WEIGHT_SCALE as u128)) as u64;
                        *cluster_wealths.entry(entry.cluster_id.0).or_insert(0) += contribution;
                    }
                }
            }
        }
        drop(rtxn);

        // Second pass: write to database
        let mut wtxn = self.env.write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        // Clear existing
        self.cluster_wealth_db.clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear cluster wealth: {}", e)))?;

        // Write new values
        for (cluster_id, wealth) in &cluster_wealths {
            self.cluster_wealth_db
                .put(&mut wtxn, &cluster_id.to_le_bytes(), &wealth.to_le_bytes())
                .map_err(|e| LedgerError::Database(format!("Failed to write cluster wealth: {}", e)))?;
        }

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        Ok(cluster_wealths.len())
    }

    // ========================================================================
    // Snapshot Support
    // ========================================================================

    /// Create a UTXO snapshot at the current chain height.
    ///
    /// This captures the complete UTXO set, key images, and cluster wealth
    /// for fast initial sync of new nodes.
    ///
    /// # Returns
    ///
    /// A `UtxoSnapshot` containing all state needed to bootstrap a node.
    pub fn create_snapshot(&self) -> Result<super::UtxoSnapshot, LedgerError> {
        use super::snapshot::UtxoSnapshot;

        let chain_state = self.get_chain_state()?;
        let tip = self.get_tip()?;
        let block_hash = tip.hash();

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Collect all UTXOs
        let mut utxos = Vec::new();
        let utxo_iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate UTXOs: {}", e)))?;

        for result in utxo_iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    utxos.push(utxo);
                }
            }
        }

        // Collect all key images
        let mut key_images = Vec::new();
        let ki_iter = self
            .key_images_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate key images: {}", e)))?;

        for result in ki_iter {
            if let Ok((key, value)) = result {
                if key.len() == 32 && value.len() == 8 {
                    let mut ki = [0u8; 32];
                    ki.copy_from_slice(key);
                    let height = u64::from_le_bytes(value.try_into().unwrap());
                    key_images.push((ki, height));
                }
            }
        }

        // Collect cluster wealth data
        let mut cluster_wealth = Vec::new();
        let cw_iter = self.cluster_wealth_db.iter(&rtxn).map_err(|e| {
            LedgerError::Database(format!("Failed to iterate cluster wealth: {}", e))
        })?;

        for result in cw_iter {
            if let Ok((key, value)) = result {
                if key.len() == 8 && value.len() == 8 {
                    let cluster_id = u64::from_le_bytes(key.try_into().unwrap());
                    let wealth = u64::from_le_bytes(value.try_into().unwrap());
                    cluster_wealth.push((cluster_id, wealth));
                }
            }
        }

        drop(rtxn);

        info!(
            height = chain_state.height,
            utxo_count = utxos.len(),
            key_image_count = key_images.len(),
            cluster_count = cluster_wealth.len(),
            "Creating UTXO snapshot"
        );

        UtxoSnapshot::new(
            chain_state.height,
            block_hash,
            chain_state,
            utxos,
            key_images,
            cluster_wealth,
        )
        .map_err(|e| LedgerError::Serialization(e.to_string()))
    }

    /// Load ledger state from a snapshot.
    ///
    /// This replaces the current ledger state with the snapshot data.
    /// The snapshot is verified before loading.
    ///
    /// # Arguments
    ///
    /// * `snapshot` - The snapshot to load
    /// * `expected_block_hash` - Optional block hash to verify against
    ///
    /// # Returns
    ///
    /// The number of UTXOs loaded.
    pub fn load_from_snapshot(
        &self,
        snapshot: &super::UtxoSnapshot,
        expected_block_hash: Option<&[u8; 32]>,
    ) -> Result<usize, LedgerError> {
        // Verify snapshot integrity
        snapshot
            .verify()
            .map_err(|e| LedgerError::InvalidBlock(format!("Snapshot verification failed: {}", e)))?;

        // Verify block hash if provided
        if let Some(expected) = expected_block_hash {
            if &snapshot.block_hash != expected {
                return Err(LedgerError::InvalidBlock(
                    "Block hash mismatch".to_string(),
                ));
            }
        }

        info!(
            height = snapshot.height,
            utxo_count = snapshot.utxo_count,
            key_image_count = snapshot.key_image_count,
            "Loading ledger from snapshot"
        );

        // Extract data from snapshot
        let utxos = snapshot
            .get_utxos()
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        let key_images = snapshot
            .get_key_images()
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        let cluster_wealth = snapshot
            .get_cluster_wealth()
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        // Clear existing data
        self.utxo_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear UTXO db: {}", e)))?;
        self.key_images_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear key images db: {}", e)))?;
        self.address_index_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear address index: {}", e)))?;
        self.cluster_wealth_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear cluster wealth: {}", e)))?;

        // Load UTXOs
        let utxo_count = utxos.len();
        for utxo in utxos {
            let utxo_bytes = bincode::serialize(&utxo)
                .map_err(|e| LedgerError::Serialization(e.to_string()))?;
            self.utxo_db
                .put(&mut wtxn, &utxo.id.to_bytes(), &utxo_bytes)
                .map_err(|e| LedgerError::Database(format!("Failed to put UTXO: {}", e)))?;

            // Rebuild address index
            self.add_to_address_index(&mut wtxn, &utxo)?;
        }

        // Load key images
        for (ki, height) in key_images {
            self.key_images_db
                .put(&mut wtxn, &ki, &height.to_le_bytes())
                .map_err(|e| LedgerError::Database(format!("Failed to put key image: {}", e)))?;
        }

        // Load cluster wealth
        for (cluster_id, wealth) in cluster_wealth {
            self.cluster_wealth_db
                .put(&mut wtxn, &cluster_id.to_le_bytes(), &wealth.to_le_bytes())
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to put cluster wealth: {}", e))
                })?;
        }

        // Update metadata
        self.meta_db
            .put(
                &mut wtxn,
                META_HEIGHT,
                &snapshot.chain_state.height.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TIP_HASH, &snapshot.block_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_TOTAL_MINED,
                &snapshot.chain_state.total_mined.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_FEES_BURNED,
                &snapshot.chain_state.total_fees_burned.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_DIFFICULTY,
                &snapshot.chain_state.difficulty.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_TOTAL_TX,
                &snapshot.chain_state.total_tx.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put total_tx: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_TX,
                &snapshot.chain_state.epoch_tx.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_tx: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_EMISSION,
                &snapshot.chain_state.epoch_emission.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_emission: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_BURNS,
                &snapshot.chain_state.epoch_burns.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_burns: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_CURRENT_REWARD,
                &snapshot.chain_state.current_reward.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put current_reward: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        info!(
            utxo_count = utxo_count,
            "Snapshot loaded successfully"
        );

        Ok(utxo_count)
    }

    /// Write a snapshot to a file.
    pub fn write_snapshot_to_file(
        &self,
        path: &std::path::Path,
    ) -> Result<u64, LedgerError> {
        let snapshot = self.create_snapshot()?;

        let file = std::fs::File::create(path)
            .map_err(|e| LedgerError::Database(format!("Failed to create file: {}", e)))?;

        let mut writer = std::io::BufWriter::new(file);
        snapshot
            .write_to(&mut writer)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;

        let size = writer
            .into_inner()
            .map_err(|e| LedgerError::Database(format!("Failed to flush: {}", e)))?
            .metadata()
            .map_err(|e| LedgerError::Database(format!("Failed to get metadata: {}", e)))?
            .len();

        info!(
            path = %path.display(),
            size_bytes = size,
            "Snapshot written to file"
        );

        Ok(size)
    }

    /// Load a snapshot from a file.
    pub fn load_snapshot_from_file(
        &self,
        path: &std::path::Path,
        expected_block_hash: Option<&[u8; 32]>,
    ) -> Result<usize, LedgerError> {
        let file = std::fs::File::open(path)
            .map_err(|e| LedgerError::Database(format!("Failed to open file: {}", e)))?;

        let reader = std::io::BufReader::new(file);
        let snapshot = super::UtxoSnapshot::read_from(reader)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;

        self.load_from_snapshot(&snapshot, expected_block_hash)
    }
}

/// Information about cluster wealth for a set of UTXOs.
///
/// Used by wallets to understand their cluster profile and estimate fees.
#[derive(Debug, Clone)]
pub struct ClusterWealthInfo {
    /// Maximum cluster wealth across all provided UTXOs.
    /// This is the value used for fee calculation (progressive fees).
    pub max_cluster_wealth: u64,

    /// Total value of the provided UTXOs.
    pub total_value: u64,

    /// Number of UTXOs found.
    pub utxo_count: usize,

    /// The cluster ID with the highest wealth (if any).
    pub dominant_cluster_id: Option<u64>,

    /// Breakdown of wealth by cluster ID: (cluster_id, wealth)
    pub cluster_breakdown: Vec<(u64, u64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_transaction_types::ClusterTagVector;
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

    #[test]
    fn test_key_image_tracking() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let key_image: [u8; 32] = [0xAB; 32];

        // Key image should not be spent initially
        assert!(ledger.is_key_image_spent(&key_image).unwrap().is_none());

        // Record key image as spent at height 10
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger.record_key_image(&mut wtxn, &key_image, 10).unwrap();
            wtxn.commit().unwrap();
        }

        // Now it should be spent
        let spent_height = ledger.is_key_image_spent(&key_image).unwrap();
        assert_eq!(spent_height, Some(10));
    }

    #[test]
    fn test_key_image_double_spend_rejected() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let key_image: [u8; 32] = [0xCD; 32];

        // Record first spend
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger.record_key_image(&mut wtxn, &key_image, 5).unwrap();
            wtxn.commit().unwrap();
        }

        // Try to record same key image again - should fail
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let result = ledger.record_key_image(&mut wtxn, &key_image, 10);
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_get_utxo_by_target_key() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Create a test UTXO
        let target_key: [u8; 32] = [0x42; 32];
        let utxo_id = UtxoId::new([0x11; 32], 0);
        let output = TxOutput {
            amount: 1_000_000,
            target_key,
            public_key: [0x33; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
        };
        let utxo = Utxo {
            id: utxo_id,
            output,
            created_at: 1,
        };

        // Store the UTXO
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            ledger.utxo_db.put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes).unwrap();
            ledger.add_to_address_index(&mut wtxn, &utxo).unwrap();
            wtxn.commit().unwrap();
        }

        // Look up by target_key
        let found = ledger.get_utxo_by_target_key(&target_key).unwrap();
        assert!(found.is_some());
        let found_utxo = found.unwrap();
        assert_eq!(found_utxo.output.amount, 1_000_000);
        assert_eq!(found_utxo.output.target_key, target_key);
    }

    #[test]
    fn test_get_utxo_by_target_key_not_found() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let nonexistent_key: [u8; 32] = [0xFF; 32];
        let result = ledger.get_utxo_by_target_key(&nonexistent_key).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_utxo_exists() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let utxo_id = UtxoId::new([0xDE; 32], 5);

        // Should not exist initially
        assert!(!ledger.utxo_exists(&utxo_id).unwrap());

        // Create and store UTXO
        let utxo = Utxo {
            id: utxo_id,
            output: TxOutput {
                amount: 500,
                target_key: [0x11; 32],
                public_key: [0x22; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
            },
            created_at: 0,
        };

        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            ledger.utxo_db.put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes).unwrap();
            wtxn.commit().unwrap();
        }

        // Now should exist
        assert!(ledger.utxo_exists(&utxo_id).unwrap());
    }

    #[test]
    fn test_get_utxo() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let utxo_id = UtxoId::new([0xAA; 32], 0);
        let amount = 12345u64;

        // Store UTXO
        let utxo = Utxo {
            id: utxo_id,
            output: TxOutput {
                amount,
                target_key: [0xBB; 32],
                public_key: [0xCC; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
            },
            created_at: 100,
        };

        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            ledger.utxo_db.put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes).unwrap();
            wtxn.commit().unwrap();
        }

        // Retrieve and verify
        let retrieved = ledger.get_utxo(&utxo_id).unwrap().unwrap();
        assert_eq!(retrieved.output.amount, amount);
        assert_eq!(retrieved.created_at, 100);
    }

    #[test]
    fn test_set_difficulty() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let new_difficulty = 0x00FF_FFFF_0000_0000u64;
        ledger.set_difficulty(new_difficulty).unwrap();

        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.difficulty, new_difficulty);
    }
}
