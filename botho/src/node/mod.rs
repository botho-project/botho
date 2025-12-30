pub mod miner;

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

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

/// The main Botho node
pub struct Node {
    config: Config,
    /// Wallet is optional - relay/seed nodes don't need one
    wallet: Option<Wallet>,
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
        // Wallet is optional - only create if mnemonic is configured
        let wallet = if let Some(mnemonic) = config.mnemonic() {
            Some(Wallet::from_mnemonic(mnemonic)?)
        } else {
            info!("Running in relay mode (no wallet configured)");
            None
        };

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

    /// Check if this node has a wallet configured
    pub fn has_wallet(&self) -> bool {
        self.wallet.is_some()
    }

    fn print_status(&self) -> Result<()> {
        use crate::monetary::mainnet_policy;

        let ledger = self.ledger.read()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?;
        let state = ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

        // Calculate monetary stats
        let policy = mainnet_policy();
        let net_supply = state.total_mined.saturating_sub(state.total_fees_burned);
        let phase = if policy.is_halving_phase(state.height) {
            "Halving"
        } else {
            "Tail Emission"
        };
        let current_reward = crate::block::calculate_block_reward_v2(state.height + 1, net_supply);

        println!();
        println!("=== Botho Node ===");
        if let Some(ref wallet) = self.wallet {
            println!(
                "Address: {}",
                wallet.address_string().replace('\n', ", ")
            );
        } else {
            println!("Mode: Relay (no wallet)");
        }
        println!("Chain height: {}", state.height);
        println!("Phase: {}", phase);
        println!(
            "Block reward: {:.6} credits",
            current_reward as f64 / 1_000_000_000_000.0
        );
        println!(
            "Net supply: {:.6} credits (mined: {:.6}, burned: {:.6})",
            net_supply as f64 / 1_000_000_000_000.0,
            state.total_mined as f64 / 1_000_000_000_000.0,
            state.total_fees_burned as f64 / 1_000_000_000_000.0
        );
        println!("Bootstrap peers: {} configured", self.config.network.bootstrap_peers.len());
        if self.config.network.bootstrap_peers.is_empty() {
            warn!("No bootstrap peers configured - add bootstrap_peers to config.toml");
        }
        println!();
        Ok(())
    }

    fn start_mining(&mut self) -> Result<()> {
        // Mining requires a wallet to receive rewards
        let wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Cannot mine without a wallet. Run 'botho init' to create one."))?;

        let threads = if self.config.mining.threads == 0 {
            num_cpus::get()
        } else {
            self.config.mining.threads as usize
        };

        info!("Starting mining with {} threads", threads);

        let mut miner = Miner::new(threads, wallet.default_address(), self.shutdown.clone());

        // Take the mining tx receiver
        self.mining_tx_receiver = miner.take_tx_receiver();

        // Set initial work from chain state
        let ledger = self.ledger.read()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?;
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

    fn adjust_difficulty(&mut self, current_height: u64) -> Result<()> {
        // Get the blocks in the adjustment window
        let window_start = current_height.saturating_sub(ADJUSTMENT_WINDOW);

        // Get start and end blocks
        let ledger = self.ledger.read()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?;
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

            self.ledger.write()
                .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?
                .set_difficulty(new_difficulty)
                .map_err(|e| anyhow::anyhow!("Failed to set difficulty: {}", e))?;
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

        self.ledger.write()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?
            .add_block(block)
            .map_err(|e| anyhow::anyhow!("Failed to add network block: {}", e))?;

        // Remove confirmed transactions from mempool
        if let Ok(mut mempool) = self.mempool.write() {
            mempool.remove_confirmed(&block.transactions);
            // Also clean up any now-invalid transactions
            if let Ok(ledger) = self.ledger.read() {
                mempool.remove_invalid(&*ledger);
            }
        }

        // Check if we need to adjust difficulty
        let new_height = block.height();
        if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
            self.adjust_difficulty(new_height)?;
        }

        // Update miner with new work if mining
        if let Some(ref miner) = self.miner {
            if let Ok(ledger) = self.ledger.read() {
                if let Ok(state) = ledger.get_chain_state() {
                    let work = MiningWork {
                        prev_block_hash: state.tip_hash,
                        height: state.height + 1,
                        difficulty: state.difficulty,
                        total_mined: state.total_mined,
                    };
                    miner.update_work(work);
                }
            }
        }

        Ok(())
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

    /// Get the wallet's view public key bytes (None if no wallet configured)
    pub fn wallet_view_key(&self) -> Option<[u8; 32]> {
        self.wallet.as_ref().map(|w| w.default_address().view_public_key().to_bytes())
    }

    /// Get the wallet's spend public key bytes (None if no wallet configured)
    pub fn wallet_spend_key(&self) -> Option<[u8; 32]> {
        self.wallet.as_ref().map(|w| w.default_address().spend_public_key().to_bytes())
    }

    /// Clean up invalid transactions from mempool
    pub fn cleanup_mempool(&self) {
        if let Ok(mut mempool) = self.mempool.write() {
            if let Ok(ledger) = self.ledger.read() {
                mempool.remove_invalid(&*ledger);
            }
            mempool.evict_old();
        }
    }

    /// Load pending transactions from file (created by `botho send`)
    /// Returns the transactions that were loaded for broadcasting
    pub fn load_pending_transactions(&self) -> Result<Vec<Transaction>> {
        self.load_pending_transactions_from_file()
    }

    /// Load pending transactions from file (created by `botho send`)
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
