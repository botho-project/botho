pub mod miner;

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use crate::block::{Block, BlockHeader};
use crate::block::difficulty::{calculate_new_difficulty, ADJUSTMENT_WINDOW};
use crate::commands::send::{load_pending_txs, clear_pending_txs};
use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::mempool::{Mempool, MempoolError};
use crate::transaction::Transaction;
use crate::wallet::Wallet;

/// Shared ledger type for RPC access
pub type SharedLedger = Arc<RwLock<Ledger>>;

/// Shared mempool type for RPC access
pub type SharedMempool = Arc<RwLock<Mempool>>;

/// Pending transactions file name
const PENDING_TXS_FILE: &str = "pending_txs.bin";

pub use miner::{MinedMiningTx, Miner, MiningWork};

/// The main Cadence node
pub struct Node {
    config: Config,
    wallet: Wallet,
    ledger: SharedLedger,
    mempool: SharedMempool,
    shutdown: Arc<AtomicBool>,
    miner: Option<Miner>,
    /// Receiver for mined mining transactions (to be submitted to consensus)
    mining_tx_receiver: Option<Receiver<MinedMiningTx>>,
    /// Directory containing config file (for finding pending_txs.bin)
    config_dir: PathBuf,
}

impl Node {
    /// Create a new node from config
    pub fn new(config: Config, config_path: &Path) -> Result<Self> {
        let wallet = Wallet::from_mnemonic(&config.wallet.mnemonic)?;

        // Open the ledger database (in same directory as config)
        let ledger_path = ledger_db_path_from_config(config_path);
        let ledger = Ledger::open(&ledger_path)
            .map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

        // Create shared ledger and mempool
        let ledger = Arc::new(RwLock::new(ledger));
        let mempool = Arc::new(RwLock::new(Mempool::new()));

        // Get config directory for finding pending transactions file
        let config_dir = config_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        Ok(Self {
            config,
            wallet,
            ledger,
            mempool,
            shutdown: Arc::new(AtomicBool::new(false)),
            miner: None,
            mining_tx_receiver: None,
            config_dir,
        })
    }

    /// Run the node (blocks until shutdown)
    pub fn run(&mut self, enable_mining: bool) -> Result<()> {
        info!("Starting Cadence node");

        // Set up Ctrl+C handler
        let shutdown = self.shutdown.clone();
        ctrlc::set_handler(move || {
            shutdown.store(true, Ordering::SeqCst);
        })?;

        // Load any pending transactions from file (created by `cadence send`)
        let _ = self.load_pending_transactions_from_file()?;

        // Display node info
        self.print_status()?;

        // Start mining if enabled
        if enable_mining {
            self.start_mining()?;
        }

        // Main loop
        let mut last_status = Instant::now();
        while !self.shutdown.load(Ordering::SeqCst) {
            // Check for found blocks
            self.process_mined_blocks()?;

            // TODO: Sync blocks from peers
            // TODO: Scan wallet for transactions
            // TODO: Process incoming transactions

            // Print status every 10 seconds
            if last_status.elapsed() >= Duration::from_secs(10) {
                self.print_mining_status()?;
                last_status = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(100));
        }

        info!("Shutting down...");
        self.stop_mining();

        Ok(())
    }

    fn print_status(&self) -> Result<()> {
        let ledger = self.ledger.read().unwrap();
        let state = ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

        println!();
        println!("=== Cadence Node ===");
        println!(
            "Address: {}",
            self.wallet.address_string().replace('\n', ", ")
        );
        println!("Chain height: {}", state.height);
        println!(
            "Total mined: {} credits",
            state.total_mined as f64 / 1_000_000_000_000.0
        );
        println!("Bootstrap peers: {} configured", self.config.network.bootstrap_peers.len());
        if self.config.network.bootstrap_peers.is_empty() {
            warn!("No bootstrap peers configured - add bootstrap_peers to config.toml");
        }
        println!();
        Ok(())
    }

    fn start_mining(&mut self) -> Result<()> {
        let threads = if self.config.mining.threads == 0 {
            num_cpus::get()
        } else {
            self.config.mining.threads as usize
        };

        info!("Starting mining with {} threads", threads);

        let mut miner = Miner::new(threads, self.wallet.default_address(), self.shutdown.clone());

        // Take the mining tx receiver
        self.mining_tx_receiver = miner.take_tx_receiver();

        // Set initial work from chain state
        let ledger = self.ledger.read().unwrap();
        let state = ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;
        drop(ledger);

        let work = MiningWork {
            prev_block_hash: state.tip_hash,
            height: state.height + 1,
            difficulty: state.difficulty,
            total_mined: state.total_mined,
        };
        miner.update_work(work);

        miner.start();
        self.miner = Some(miner);

        Ok(())
    }

    fn stop_mining(&mut self) {
        if let Some(miner) = self.miner.take() {
            miner.stop();
        }
    }

    fn process_mined_blocks(&mut self) -> Result<()> {
        // Collect mining transactions first to avoid borrow issues
        let mining_txs: Vec<MinedMiningTx> = if let Some(ref receiver) = self.mining_tx_receiver {
            let mut collected = Vec::new();
            while let Ok(mined) = receiver.try_recv() {
                collected.push(mined);
            }
            collected
        } else {
            Vec::new()
        };

        // Process collected mining transactions
        // TODO: Submit to consensus instead of building blocks directly
        for mined in mining_txs {
            let mining_tx = &mined.mining_tx;
            info!(
                "Processing mining tx for height {} with priority {}",
                mining_tx.block_height,
                mined.pow_priority
            );

            // Get pending transactions from mempool (limit to 100 per block)
            let pending_txs = self.get_pending_transactions(100);
            let tx_count = pending_txs.len();

            // Calculate transaction merkle root
            let tx_root = if pending_txs.is_empty() {
                [0u8; 32]
            } else {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                for tx in &pending_txs {
                    hasher.update(tx.hash());
                }
                hasher.finalize().into()
            };

            // Build a block from the mining transaction and pending txs
            // (In full consensus mode, this would come from externalized values)
            // Get miner's public address for block header (for PoW binding)
            let miner_address = self.wallet.default_address();
            let miner_view_key = miner_address.view_public_key().to_bytes();
            let miner_spend_key = miner_address.spend_public_key().to_bytes();

            let block = Block {
                header: BlockHeader {
                    version: 1,
                    prev_block_hash: mining_tx.prev_block_hash,
                    tx_root,
                    timestamp: mining_tx.timestamp,
                    height: mining_tx.block_height,
                    difficulty: mining_tx.difficulty,
                    nonce: mining_tx.nonce,
                    miner_view_key,
                    miner_spend_key,
                },
                mining_tx: mining_tx.clone(),
                transactions: pending_txs,
            };

            // Add to ledger
            let add_result = self.ledger.write().unwrap().add_block(&block);
            match add_result {
                Ok(()) => {
                    info!(
                        "Block {} added to ledger! Reward: {} credits, {} txs included",
                        block.height(),
                        mining_tx.reward as f64 / 1_000_000_000_000.0,
                        tx_count
                    );

                    // Remove confirmed transactions from mempool
                    if !block.transactions.is_empty() {
                        if let Ok(mut mempool) = self.mempool.write() {
                            mempool.remove_confirmed(&block.transactions);
                        }
                    }

                    // Check if we need to adjust difficulty
                    let new_height = block.height();
                    if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
                        self.adjust_difficulty(new_height)?;
                    }

                    // Update miner with new work
                    if let Some(ref miner) = self.miner {
                        let ledger = self.ledger.read().unwrap();
                        let state = ledger.get_chain_state().map_err(|e| {
                            anyhow::anyhow!("Failed to get chain state: {}", e)
                        })?;

                        let work = MiningWork {
                            prev_block_hash: state.tip_hash,
                            height: state.height + 1,
                            difficulty: state.difficulty,
                            total_mined: state.total_mined,
                        };
                        miner.update_work(work);
                    }
                }
                Err(e) => {
                    // Block might be stale (we already have a block at this height)
                    error!("Failed to add block: {}", e);
                }
            }
        }
        Ok(())
    }

    fn adjust_difficulty(&mut self, current_height: u64) -> Result<()> {
        // Get the blocks in the adjustment window
        let window_start = current_height.saturating_sub(ADJUSTMENT_WINDOW);

        // Get start and end blocks
        let ledger = self.ledger.read().unwrap();
        let start_block = ledger
            .get_block(window_start)
            .map_err(|e| anyhow::anyhow!("Failed to get start block: {}", e))?;
        let end_block = ledger
            .get_block(current_height)
            .map_err(|e| anyhow::anyhow!("Failed to get end block: {}", e))?;
        drop(ledger);

        let current_difficulty = end_block.header.difficulty;
        let blocks_in_window = current_height - window_start;

        let new_difficulty = calculate_new_difficulty(
            current_difficulty,
            start_block.header.timestamp,
            end_block.header.timestamp,
            blocks_in_window,
        );

        if new_difficulty != current_difficulty {
            info!(
                "Difficulty adjustment at height {}: {} -> {} (ratio: {:.2}x)",
                current_height,
                current_difficulty,
                new_difficulty,
                new_difficulty as f64 / current_difficulty as f64
            );

            self.ledger.write().unwrap()
                .set_difficulty(new_difficulty)
                .map_err(|e| anyhow::anyhow!("Failed to set difficulty: {}", e))?;
        }

        Ok(())
    }

    fn print_mining_status(&self) -> Result<()> {
        if let Some(ref miner) = self.miner {
            let stats = miner.stats();
            let ledger = self.ledger.read().unwrap();
            let state = ledger
                .get_chain_state()
                .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

            println!(
                "[Mining] Height: {} | Hashrate: {:.2} H/s | Txs found: {} | Mined: {:.6} credits",
                state.height,
                stats.hashrate(),
                stats.txs_found,
                state.total_mined as f64 / 1_000_000_000_000.0
            );
        }
        Ok(())
    }

    // --- Network integration methods ---

    /// Get the config
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Start mining (public for network integration)
    pub fn start_mining_public(&mut self) -> Result<()> {
        self.start_mining()
    }

    /// Stop mining (public for network integration)
    pub fn stop_mining_public(&mut self) {
        self.stop_mining()
    }

    /// Print status (public for network integration)
    pub fn print_status_public(&self) -> Result<()> {
        self.print_status()
    }

    /// Add a block received from the network
    pub fn add_block_from_network(&mut self, block: &crate::block::Block) -> Result<()> {
        info!(
            "Adding block {} from network (hash: {})",
            block.height(),
            hex::encode(&block.hash()[0..8])
        );

        self.ledger.write().unwrap()
            .add_block(block)
            .map_err(|e| anyhow::anyhow!("Failed to add network block: {}", e))?;

        // Remove confirmed transactions from mempool
        if let Ok(mut mempool) = self.mempool.write() {
            let ledger = self.ledger.read().unwrap();
            mempool.remove_confirmed(&block.transactions);
            // Also clean up any now-invalid transactions
            mempool.remove_invalid(&*ledger);
        }

        // Check if we need to adjust difficulty
        let new_height = block.height();
        if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
            self.adjust_difficulty(new_height)?;
        }

        // Update miner with new work if mining
        if let Some(ref miner) = self.miner {
            let ledger = self.ledger.read().unwrap();
            let state = ledger
                .get_chain_state()
                .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

            let work = MiningWork {
                prev_block_hash: state.tip_hash,
                height: state.height + 1,
                difficulty: state.difficulty,
                total_mined: state.total_mined,
            };
            miner.update_work(work);
        }

        Ok(())
    }

    /// Check if we've mined a mining transaction (non-blocking)
    /// Returns a block built from the mining transaction
    /// TODO: In full consensus mode, this would submit to consensus instead
    pub fn check_mined_block(&mut self) -> Result<Option<crate::block::Block>> {
        if let Some(ref receiver) = self.mining_tx_receiver {
            if let Ok(mined) = receiver.try_recv() {
                let mining_tx = &mined.mining_tx;
                info!(
                    "Mined mining tx for height {} with priority {}",
                    mining_tx.block_height,
                    mined.pow_priority
                );

                // Get pending transactions from mempool
                let pending_txs = self.get_pending_transactions(100);

                // Calculate transaction merkle root
                let tx_root = if pending_txs.is_empty() {
                    [0u8; 32]
                } else {
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    for tx in &pending_txs {
                        hasher.update(tx.hash());
                    }
                    hasher.finalize().into()
                };

                // Build a block from the mining transaction
                // Get miner's public address for block header (for PoW binding)
                let miner_address = self.wallet.default_address();
                let miner_view_key = miner_address.view_public_key().to_bytes();
                let miner_spend_key = miner_address.spend_public_key().to_bytes();

                let block = Block {
                    header: BlockHeader {
                        version: 1,
                        prev_block_hash: mining_tx.prev_block_hash,
                        tx_root,
                        timestamp: mining_tx.timestamp,
                        height: mining_tx.block_height,
                        difficulty: mining_tx.difficulty,
                        nonce: mining_tx.nonce,
                        miner_view_key,
                        miner_spend_key,
                    },
                    mining_tx: mining_tx.clone(),
                    transactions: pending_txs,
                };

                // Add to our ledger
                self.ledger.write().unwrap()
                    .add_block(&block)
                    .map_err(|e| anyhow::anyhow!("Failed to add mined block: {}", e))?;

                // Check if we need to adjust difficulty
                let new_height = block.height();
                if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
                    self.adjust_difficulty(new_height)?;
                }

                // Update miner with new work
                if let Some(ref miner) = self.miner {
                    let ledger = self.ledger.read().unwrap();
                    let state = ledger
                        .get_chain_state()
                        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

                    let work = MiningWork {
                        prev_block_hash: state.tip_hash,
                        height: state.height + 1,
                        difficulty: state.difficulty,
                        total_mined: state.total_mined,
                    };
                    miner.update_work(work);
                }

                // Remove confirmed transactions from mempool
                if !block.transactions.is_empty() {
                    if let Ok(mut mempool) = self.mempool.write() {
                        mempool.remove_confirmed(&block.transactions);
                    }
                }

                return Ok(Some(block));
            }
        }
        Ok(None)
    }

    /// Check if we've mined a mining transaction (non-blocking)
    /// Returns the raw MinedMiningTx for consensus submission (doesn't build block)
    pub fn check_mined_mining_tx(&mut self) -> Result<Option<MinedMiningTx>> {
        if let Some(ref receiver) = self.mining_tx_receiver {
            if let Ok(mined) = receiver.try_recv() {
                return Ok(Some(mined));
            }
        }
        Ok(None)
    }

    // --- Mempool methods ---

    /// Submit a transaction to the mempool
    pub fn submit_transaction(&self, tx: Transaction) -> Result<[u8; 32], MempoolError> {
        let ledger = self.ledger.read()
            .map_err(|_| MempoolError::LedgerError("Ledger lock poisoned".to_string()))?;
        let mut mempool = self.mempool.write()
            .map_err(|_| MempoolError::LedgerError("Mempool lock poisoned".to_string()))?;
        mempool.add_tx(tx, &*ledger)
    }

    /// Get pending transaction count
    pub fn pending_tx_count(&self) -> usize {
        self.mempool.read().map(|m| m.len()).unwrap_or(0)
    }

    /// Get transactions from mempool for block building
    pub fn get_pending_transactions(&self, max_count: usize) -> Vec<Transaction> {
        self.mempool.read()
            .map(|m| m.get_transactions(max_count))
            .unwrap_or_default()
    }

    /// Get the shared mempool reference
    pub fn shared_mempool(&self) -> SharedMempool {
        self.mempool.clone()
    }

    /// Get the shared ledger reference
    pub fn shared_ledger(&self) -> SharedLedger {
        self.ledger.clone()
    }

    /// Get the wallet's view public key bytes
    pub fn wallet_view_key(&self) -> [u8; 32] {
        self.wallet.default_address().view_public_key().to_bytes()
    }

    /// Get the wallet's spend public key bytes
    pub fn wallet_spend_key(&self) -> [u8; 32] {
        self.wallet.default_address().spend_public_key().to_bytes()
    }

    /// Clean up invalid transactions from mempool
    pub fn cleanup_mempool(&self) {
        if let Ok(mut mempool) = self.mempool.write() {
            let ledger = self.ledger.read().unwrap();
            mempool.remove_invalid(&*ledger);
            mempool.evict_old();
        }
    }

    /// Load pending transactions from file (created by `cadence send`)
    /// Returns the transactions that were loaded for broadcasting
    pub fn load_pending_transactions(&self) -> Result<Vec<Transaction>> {
        self.load_pending_transactions_from_file()
    }

    /// Load pending transactions from file (created by `cadence send`)
    fn load_pending_transactions_from_file(&self) -> Result<Vec<Transaction>> {
        let pending_path = self.config_dir.join(PENDING_TXS_FILE);

        match load_pending_txs(&pending_path) {
            Ok(txs) if txs.is_empty() => {
                // No pending transactions
                Ok(Vec::new())
            }
            Ok(txs) => {
                info!("Loading {} pending transactions from file", txs.len());

                let mut loaded_txs = Vec::new();
                let mut failed = 0;

                for tx in txs {
                    let tx_clone = tx.clone();
                    match self.submit_transaction(tx) {
                        Ok(hash) => {
                            info!("Loaded pending tx: {}", hex::encode(&hash[0..8]));
                            loaded_txs.push(tx_clone);
                        }
                        Err(e) => {
                            warn!("Failed to load pending tx: {}", e);
                            failed += 1;
                        }
                    }
                }

                info!(
                    "Loaded {} pending transactions ({} failed)",
                    loaded_txs.len(), failed
                );

                // Clear the pending file since we've loaded them
                if let Err(e) = clear_pending_txs(&pending_path) {
                    warn!("Failed to clear pending transactions file: {}", e);
                }

                Ok(loaded_txs)
            }
            Err(e) => {
                // File might not exist, which is fine
                if pending_path.exists() {
                    warn!("Failed to load pending transactions: {}", e);
                }
                Ok(Vec::new())
            }
        }
    }
}
