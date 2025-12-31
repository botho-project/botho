pub mod minter;

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

use crate::block::difficulty::EmissionController;
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

pub use minter::{MintedMintingTx, Minter, MintingWork};

/// The main Botho node
pub struct Node {
    config: Config,
    /// Wallet is optional - relay/seed nodes don't need one
    wallet: Option<Wallet>,
    ledger: SharedLedger,
    mempool: SharedMempool,
    shutdown: Arc<AtomicBool>,
    minter: Option<Minter>,
    /// Receiver for minted minting transactions (to be submitted to consensus)
    minting_tx_receiver: Option<Receiver<MintedMintingTx>>,
    /// Directory containing config file (for finding pending_txs.bin)
    config_dir: PathBuf,
    /// Emission controller for tx-based monetary policy
    emission_controller: EmissionController,
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

        // Restore EmissionController from persisted chain state
        let state = ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;
        let emission_controller = EmissionController::from_chain_state(
            state.difficulty,
            state.total_mined,
            state.total_fees_burned,
            state.total_tx,
            state.epoch_tx,
            state.epoch_emission,
            state.epoch_burns,
            state.current_reward,
        );

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
            minter: None,
            minting_tx_receiver: None,
            config_dir,
            emission_controller,
        })
    }

    /// Check if this node has a wallet configured
    pub fn has_wallet(&self) -> bool {
        self.wallet.is_some()
    }

    fn print_status(&self) -> Result<()> {
        let ledger = self.ledger.read()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?;
        let state = ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

        // Calculate monetary stats
        let net_supply = state.total_mined.saturating_sub(state.total_fees_burned);
        // Use tx-based phase from EmissionController
        let phase = match self.emission_controller.phase() {
            crate::block::difficulty::Phase::Halving { epoch } => format!("Halving (epoch {})", epoch),
            crate::block::difficulty::Phase::Tail => "Tail Emission".to_string(),
        };
        // Use tx-based reward from EmissionController
        let current_reward = self.emission_controller.current_reward;

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
            "Block reward: {:.6} BTH",
            current_reward as f64 / 1_000_000_000_000.0
        );
        println!(
            "Net supply: {:.6} BTH (mined: {:.6}, burned: {:.6})",
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

    fn start_minting(&mut self) -> Result<()> {
        // Minting requires a wallet to receive rewards
        let wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Cannot mine without a wallet. Run 'botho init' to create one."))?;

        let threads = if self.config.minting.threads == 0 {
            num_cpus::get()
        } else {
            self.config.minting.threads as usize
        };

        info!("Starting minting with {} threads", threads);

        let mut minter = Minter::new(threads, wallet.default_address(), self.shutdown.clone());

        // Take the minting tx receiver
        self.minting_tx_receiver = minter.take_tx_receiver();

        // Set initial work from chain state
        let ledger = self.ledger.read()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?;
        let state = ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;
        drop(ledger);

        let work = MintingWork {
            prev_block_hash: state.tip_hash,
            height: state.height + 1,
            difficulty: state.difficulty,
            total_minted: state.total_mined,
            current_reward: self.emission_controller.current_reward,
        };
        minter.update_work(work);

        minter.start();
        self.minter = Some(minter);

        Ok(())
    }

    fn stop_minting(&mut self) {
        if let Some(minter) = self.minter.take() {
            minter.stop();
        }
    }

    // --- Network integration methods ---

    /// Get the config
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Start minting (public for network integration)
    pub fn start_minting_public(&mut self) -> Result<()> {
        self.start_minting()
    }

    /// Stop minting (public for network integration)
    pub fn stop_minting_public(&mut self) {
        self.stop_minting()
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

        // Remove confirmed transactions from mempool and update dynamic fee state
        if let Ok(mut mempool) = self.mempool.write() {
            mempool.remove_confirmed(&block.transactions);
            // Also clean up any now-invalid transactions
            if let Ok(ledger) = self.ledger.read() {
                mempool.remove_invalid(&*ledger);
            }
        }

        // Note: Dynamic fee update is handled by the caller who has access to
        // consensus timing information. See update_dynamic_fee_after_block().

        // Update emission controller with block data
        let tx_count = block.transactions.len() as u64;
        let reward_paid = block.minting_tx.reward;
        let fees_burned: u64 = block.transactions.iter().map(|tx| tx.fee).sum();

        let old_difficulty = self.emission_controller.difficulty;
        let (new_difficulty, new_reward) = self.emission_controller.record_block(
            tx_count,
            reward_paid,
            fees_burned,
        );

        if new_difficulty != old_difficulty {
            info!(
                "Difficulty adjustment at tx {}: {} -> {} (ratio: {:.2}x)",
                self.emission_controller.total_tx,
                old_difficulty,
                new_difficulty,
                new_difficulty as f64 / old_difficulty as f64
            );
        }

        // Persist emission controller state
        self.ledger.write()
            .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?
            .update_emission_state(
                new_difficulty,
                self.emission_controller.total_tx,
                self.emission_controller.epoch_tx,
                self.emission_controller.epoch_emission,
                self.emission_controller.epoch_burns,
                new_reward,
            )
            .map_err(|e| anyhow::anyhow!("Failed to persist emission state: {}", e))?;

        // Update minter with new work if minting
        if let Some(ref minter) = self.minter {
            if let Ok(ledger) = self.ledger.read() {
                if let Ok(state) = ledger.get_chain_state() {
                    let work = MintingWork {
                        prev_block_hash: state.tip_hash,
                        height: state.height + 1,
                        difficulty: new_difficulty,
                        total_minted: state.total_mined,
                        current_reward: self.emission_controller.current_reward,
                    };
                    info!(
                        height = work.height,
                        prev_hash = hex::encode(&work.prev_block_hash[0..8]),
                        "Updating minter work after block"
                    );
                    minter.update_work(work);
                }
            }
        }

        Ok(())
    }

    /// Check if we've minted a minting transaction (non-blocking)
    /// Returns the raw MintedMintingTx for consensus submission (doesn't build block)
    pub fn check_minted_minting_tx(&mut self) -> Result<Option<MintedMintingTx>> {
        if let Some(ref receiver) = self.minting_tx_receiver {
            if let Ok(mined) = receiver.try_recv() {
                return Ok(Some(mined));
            }
        }
        Ok(None)
    }

    /// Get the current work version from the minter
    /// Returns 0 if minting is not active
    pub fn current_minting_work_version(&self) -> u64 {
        self.minter.as_ref().map(|m| m.current_work_version()).unwrap_or(0)
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

    /// Update dynamic fee state after a block is finalized.
    ///
    /// Call this after each block is added to update fee calculations based on congestion.
    ///
    /// # Arguments
    /// * `tx_count` - Number of transactions in the finalized block
    /// * `max_tx_count` - Maximum transactions per block (from consensus config)
    /// * `at_min_block_time` - Whether block timing is at minimum (triggers fee adjustment)
    ///
    /// # Returns
    /// The new fee base, or None if mempool lock failed
    pub fn update_dynamic_fee_after_block(
        &self,
        tx_count: usize,
        max_tx_count: usize,
        at_min_block_time: bool,
    ) -> Option<u64> {
        self.mempool.write().ok().map(|mut mempool| {
            mempool.update_dynamic_fee(tx_count, max_tx_count, at_min_block_time)
        })
    }

    /// Get the current dynamic fee state for diagnostics/RPC
    pub fn dynamic_fee_state(&self) -> Option<bth_cluster_tax::DynamicFeeState> {
        self.mempool.read().ok().map(|mempool| mempool.dynamic_fee_state())
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
