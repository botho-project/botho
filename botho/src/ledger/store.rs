use bth_account_keys::PublicAddress;
use bth_crypto_keys::{RistrettoPublic, RistrettoSignature};
use bth_transaction_types::Network;
use lmdb::{Cursor, Database, Environment, EnvironmentFlags, Transaction, WriteFlags};
use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::{ChainState, LedgerError};
use crate::block::Block;
use crate::transaction::{Transaction as BothoTransaction, TxInputs, TxOutput, Utxo, UtxoId};

/// LMDB-backed ledger storage
pub struct Ledger {
    env: Environment,
    /// The network this ledger belongs to
    network: Network,
    /// blocks: height -> Block
    blocks_db: Database,
    /// metadata: key -> value (for chain state)
    meta_db: Database,
    /// utxos: UtxoId (36 bytes) -> Utxo
    utxo_db: Database,
    /// address_index: (view_key || spend_key) (64 bytes) -> [UtxoId (36 bytes), ...]
    /// Maps addresses to their UTXOs for efficient balance lookups
    address_index_db: Database,
    /// key_images: key_image (32 bytes) -> height (8 bytes)
    /// Tracks spent key images to prevent double-spending with ring signatures.
    /// Value is the block height where the key image was first seen.
    key_images_db: Database,
}

// Metadata keys (fixed size for LMDB compatibility)
const META_HEIGHT: &[u8; 6] = b"height";
const META_TIP_HASH: &[u8; 8] = b"tip_hash";
const META_TOTAL_MINED: &[u8; 11] = b"total_mined";
const META_FEES_BURNED: &[u8; 11] = b"fees_burned";
const META_DIFFICULTY: &[u8; 10] = b"difficulty";

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
            LedgerError::Database(lmdb::Error::Other(e.raw_os_error().unwrap_or(0)))
        })?;

        let env = Environment::new()
            .set_flags(EnvironmentFlags::NO_SUB_DIR)
            .set_max_dbs(5)
            .set_map_size(1024 * 1024 * 1024) // 1GB
            .open(path.join("ledger.mdb").as_ref())?;

        let blocks_db = env.create_db(Some("blocks"), lmdb::DatabaseFlags::empty())?;
        let meta_db = env.create_db(Some("meta"), lmdb::DatabaseFlags::empty())?;
        let utxo_db = env.create_db(Some("utxos"), lmdb::DatabaseFlags::empty())?;
        let address_index_db = env.create_db(Some("address_index"), lmdb::DatabaseFlags::empty())?;
        let key_images_db = env.create_db(Some("key_images"), lmdb::DatabaseFlags::empty())?;

        let ledger = Self {
            env,
            network,
            blocks_db,
            meta_db,
            utxo_db,
            address_index_db,
            key_images_db,
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
        txn.put(self.meta_db, META_FEES_BURNED, &0u64.to_le_bytes(), WriteFlags::empty())?;
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

        let total_fees_burned = match txn.get(self.meta_db, &META_FEES_BURNED) {
            Ok(bytes) => u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])),
            Err(lmdb::Error::NotFound) => 0,
            Err(e) => return Err(e.into()),
        };

        let difficulty = match txn.get(self.meta_db, &META_DIFFICULTY) {
            Ok(bytes) => u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])),
            Err(lmdb::Error::NotFound) => crate::node::miner::INITIAL_DIFFICULTY,
            Err(e) => return Err(e.into()),
        };

        // Get tip timestamp from the tip block (if exists)
        let tip_timestamp = if height > 0 {
            match txn.get(self.blocks_db, &height.to_le_bytes()) {
                Ok(bytes) => {
                    if let Ok(block) = bincode::deserialize::<Block>(bytes) {
                        block.header.timestamp
                    } else {
                        0
                    }
                }
                Err(_) => 0,
            }
        } else {
            0
        };

        Ok(ChainState {
            height,
            tip_hash,
            tip_timestamp,
            total_mined,
            total_fees_burned,
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

        // Sum transaction fees (these are burned, reducing circulating supply)
        let block_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
        let new_total_fees_burned = state.total_fees_burned + block_fees;

        // Create UTXO from mining reward (coinbase)
        // Use block hash as the "tx_hash" for mining rewards
        // The mining tx has stealth output keys (target_key, public_key)
        let coinbase_utxo_id = UtxoId::new(new_hash, 0);
        let coinbase_utxo = Utxo {
            id: coinbase_utxo_id,
            output: block.mining_tx.to_tx_output(),
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

            // Process spent inputs based on type
            match &tx.inputs {
                TxInputs::Simple(inputs) => {
                    // Remove spent UTXOs (inputs)
                    for input in inputs {
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
                }
                TxInputs::Ring(ring_inputs) => {
                    // For ring signature transactions, record key images to prevent double-spend
                    // The actual UTXO being spent is hidden within the ring
                    for ring_input in ring_inputs {
                        self.record_key_image(&mut txn, &ring_input.key_image, new_height)?;
                    }
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
        txn.put(
            self.meta_db,
            META_FEES_BURNED,
            &new_total_fees_burned.to_le_bytes(),
            WriteFlags::empty(),
        )?;

        txn.commit()?;

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

    /// Get a UTXO by its target_key (one-time stealth public key)
    ///
    /// This is useful for ring signature validation where we need to look up
    /// UTXOs by their target_key to determine their amounts.
    pub fn get_utxo_by_target_key(&self, target_key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError> {
        let txn = self.env.begin_ro_txn()?;

        // Look up UTXO IDs from the target_key index
        let id_bytes = match txn.get(self.address_index_db, target_key) {
            Ok(bytes) => bytes,
            Err(lmdb::Error::NotFound) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        // Get the first UTXO ID (there should typically be only one per target_key)
        if id_bytes.len() >= 36 {
            if let Some(utxo_id) = UtxoId::from_bytes(&id_bytes[0..36]) {
                if let Ok(utxo_bytes) = txn.get(self.utxo_db, &utxo_id.to_bytes()) {
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
    ///
    /// NOTE: With stealth addresses, we index by target_key (one-time key) instead of
    /// recipient address. This allows retrieving UTXOs after the wallet has identified
    /// them via stealth scanning, but doesn't reveal address linkage.
    fn add_to_address_index(
        &self,
        txn: &mut lmdb::RwTransaction,
        utxo: &Utxo,
    ) -> Result<(), LedgerError> {
        // Index by target_key for UTXO retrieval after stealth detection
        let target_key = &utxo.output.target_key;

        // Get existing IDs or empty vec
        let existing = match txn.get(self.address_index_db, target_key) {
            Ok(bytes) => bytes.to_vec(),
            Err(lmdb::Error::NotFound) => Vec::new(),
            Err(e) => return Err(e.into()),
        };

        // Append the new UTXO ID
        let mut ids = existing;
        ids.extend_from_slice(&utxo.id.to_bytes());

        txn.put(
            self.address_index_db,
            target_key,
            &ids,
            WriteFlags::empty(),
        )?;

        Ok(())
    }

    /// Remove a UTXO ID from the address index
    ///
    /// NOTE: With stealth addresses, UTXOs are indexed by target_key (one-time key).
    fn remove_from_address_index(
        &self,
        txn: &mut lmdb::RwTransaction,
        utxo: &Utxo,
    ) -> Result<(), LedgerError> {
        let target_key = &utxo.output.target_key;

        // Get existing IDs
        let existing = match txn.get(self.address_index_db, target_key) {
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
            // No more UTXOs for this target key, remove the entry
            let _ = txn.del(self.address_index_db, target_key, None);
        } else {
            txn.put(
                self.address_index_db,
                target_key,
                &filtered,
                WriteFlags::empty(),
            )?;
        }

        Ok(())
    }

    /// Verify all signatures in a transaction
    ///
    /// For Simple inputs: looks up the UTXO being spent and verifies
    /// the signature against the UTXO's one-time target key (stealth address).
    ///
    /// For Ring inputs: verifies the ring signature and checks that key images
    /// haven't been spent (double-spend prevention).
    ///
    /// With stealth addresses, the target_key is the one-time public spend key
    /// `P = Hs(r * C) * G + D`. The spender proves ownership using the
    /// corresponding one-time private key `x = Hs(a * R) + d`.
    pub fn verify_transaction(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        match &tx.inputs {
            TxInputs::Simple(inputs) => {
                let signing_hash = tx.signing_hash();

                for (i, input) in inputs.iter().enumerate() {
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

                    // Get the one-time target key (stealth spend public key)
                    let target_public = RistrettoPublic::try_from(&utxo.output.target_key[..])
                        .map_err(|_| {
                            LedgerError::InvalidBlock(format!(
                                "Input {} has invalid target key in UTXO",
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

                    // Verify the signature against the one-time target key
                    target_public
                        .verify_schnorrkel(b"botho-tx-v1", &signing_hash, &signature)
                        .map_err(|_| {
                            LedgerError::InvalidBlock(format!(
                                "Input {} has invalid signature",
                                i
                            ))
                        })?;
                }
            }
            TxInputs::Ring(ring_inputs) => {
                // Verify key images haven't been spent (double-spend check)
                for (i, ring_input) in ring_inputs.iter().enumerate() {
                    if let Ok(Some(spent_height)) = self.is_key_image_spent(&ring_input.key_image) {
                        return Err(LedgerError::InvalidBlock(format!(
                            "Ring input {} uses key image already spent at height {}",
                            i, spent_height
                        )));
                    }
                }

                // Verify ring signatures
                tx.verify_ring_signatures().map_err(|e| {
                    LedgerError::InvalidBlock(format!("Invalid ring signature: {}", e))
                })?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // Key Image Tracking (for Ring Signature Double-Spend Prevention)
    // ========================================================================

    /// Check if a key image has already been spent.
    ///
    /// Key images are used with ring signatures to prevent double-spending
    /// without revealing which output was actually spent. Each output can
    /// only produce one unique key image, so tracking spent key images
    /// prevents the same output from being spent twice.
    ///
    /// # Arguments
    /// * `key_image` - The 32-byte key image to check
    ///
    /// # Returns
    /// `Some(height)` if the key image was spent at that block height,
    /// `None` if the key image has never been seen.
    pub fn is_key_image_spent(&self, key_image: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        let txn = self.env.begin_ro_txn()?;

        match txn.get(self.key_images_db, key_image) {
            Ok(bytes) => {
                if bytes.len() == 8 {
                    let height = u64::from_le_bytes(bytes.try_into().unwrap());
                    Ok(Some(height))
                } else {
                    Ok(None)
                }
            }
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Record a key image as spent at the given block height.
    ///
    /// This should be called when processing a block that contains
    /// ring signature transactions. The key image is recorded along
    /// with the height where it was first seen.
    ///
    /// # Arguments
    /// * `txn` - The write transaction to use
    /// * `key_image` - The 32-byte key image to record
    /// * `height` - The block height where this key image was spent
    pub fn record_key_image(
        &self,
        txn: &mut lmdb::RwTransaction,
        key_image: &[u8; 32],
        height: u64,
    ) -> Result<(), LedgerError> {
        txn.put(
            self.key_images_db,
            key_image,
            &height.to_le_bytes(),
            WriteFlags::NO_OVERWRITE, // Fail if already exists (double-spend attempt)
        )
        .map_err(|e| {
            if matches!(e, lmdb::Error::KeyExist) {
                LedgerError::InvalidBlock("Key image already spent (double-spend)".to_string())
            } else {
                e.into()
            }
        })
    }

    /// Get a random sample of UTXOs for use as decoys in ring signatures.
    ///
    /// Selects UTXOs that are suitable for use as decoys:
    /// - Must have been confirmed for at least `min_confirmations` blocks
    /// - Excludes the specified real inputs to avoid including them as decoys
    ///
    /// # Arguments
    /// * `count` - Number of decoy UTXOs to return
    /// * `exclude` - UTXOs to exclude (the real inputs)
    /// * `min_confirmations` - Minimum confirmations required for decoys
    ///
    /// # Returns
    /// A vector of TxOutputs suitable for use as ring decoys.
    pub fn get_decoy_outputs(
        &self,
        count: usize,
        exclude: &[[u8; 32]], // target_keys to exclude
        min_confirmations: u64,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        use rand::seq::SliceRandom;

        let state = self.get_chain_state()?;
        let max_height = state.height.saturating_sub(min_confirmations);

        let txn = self.env.begin_ro_txn()?;

        // Collect all eligible UTXOs
        // Note: This is a simple implementation that scans all UTXOs.
        // For production, consider using a separate index or reservoir sampling.
        let mut candidates: Vec<TxOutput> = Vec::new();

        // Create a cursor to iterate over all UTXOs
        let mut cursor = txn.open_ro_cursor(self.utxo_db)?;

        for result in cursor.iter() {
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

        drop(cursor);
        drop(txn);

        // Randomly sample from candidates
        let mut rng = rand::thread_rng();
        candidates.shuffle(&mut rng);
        candidates.truncate(count);

        Ok(candidates)
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

    #[test]
    fn test_key_image_tracking() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let key_image: [u8; 32] = [0xAB; 32];

        // Key image should not be spent initially
        assert!(ledger.is_key_image_spent(&key_image).unwrap().is_none());

        // Record key image as spent at height 10
        {
            let mut txn = ledger.env.begin_rw_txn().unwrap();
            ledger.record_key_image(&mut txn, &key_image, 10).unwrap();
            txn.commit().unwrap();
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
            let mut txn = ledger.env.begin_rw_txn().unwrap();
            ledger.record_key_image(&mut txn, &key_image, 5).unwrap();
            txn.commit().unwrap();
        }

        // Try to record same key image again - should fail
        {
            let mut txn = ledger.env.begin_rw_txn().unwrap();
            let result = ledger.record_key_image(&mut txn, &key_image, 10);
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
        };
        let utxo = Utxo {
            id: utxo_id,
            output,
            created_at: 1,
        };

        // Store the UTXO
        {
            let mut txn = ledger.env.begin_rw_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            txn.put(
                ledger.utxo_db,
                &utxo_id.to_bytes(),
                &utxo_bytes,
                lmdb::WriteFlags::empty(),
            ).unwrap();
            ledger.add_to_address_index(&mut txn, &utxo).unwrap();
            txn.commit().unwrap();
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
            },
            created_at: 0,
        };

        {
            let mut txn = ledger.env.begin_rw_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            txn.put(
                ledger.utxo_db,
                &utxo_id.to_bytes(),
                &utxo_bytes,
                lmdb::WriteFlags::empty(),
            ).unwrap();
            txn.commit().unwrap();
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
            },
            created_at: 100,
        };

        {
            let mut txn = ledger.env.begin_rw_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            txn.put(
                ledger.utxo_db,
                &utxo_id.to_bytes(),
                &utxo_bytes,
                lmdb::WriteFlags::empty(),
            ).unwrap();
            txn.commit().unwrap();
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
