pub mod miner;

use anyhow::Result;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use crate::block::difficulty::{calculate_new_difficulty, ADJUSTMENT_WINDOW};
use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::wallet::Wallet;

pub use miner::{MinedBlock, Miner, MiningWork};

/// The main Cadence node
pub struct Node {
    config: Config,
    wallet: Wallet,
    ledger: Ledger,
    shutdown: Arc<AtomicBool>,
    miner: Option<Miner>,
    block_receiver: Option<Receiver<MinedBlock>>,
}

impl Node {
    /// Create a new node from config
    pub fn new(config: Config, config_path: &Path) -> Result<Self> {
        let wallet = Wallet::from_mnemonic(&config.wallet.mnemonic)?;

        // Open the ledger database (in same directory as config)
        let ledger_path = ledger_db_path_from_config(config_path);
        let ledger = Ledger::open(&ledger_path)
            .map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

        Ok(Self {
            config,
            wallet,
            ledger,
            shutdown: Arc::new(AtomicBool::new(false)),
            miner: None,
            block_receiver: None,
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
        let state = self
            .ledger
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

        // Take the block receiver
        self.block_receiver = miner.take_block_receiver();

        // Set initial work from chain state
        let state = self
            .ledger
            .get_chain_state()
            .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

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
        // Collect blocks first to avoid borrow issues
        let blocks: Vec<MinedBlock> = if let Some(ref receiver) = self.block_receiver {
            let mut collected = Vec::new();
            while let Ok(mined) = receiver.try_recv() {
                collected.push(mined);
            }
            collected
        } else {
            Vec::new()
        };

        // Process collected blocks
        for mined in blocks {
            info!(
                "Processing mined block {} with hash {}",
                mined.block.height(),
                hex::encode(&mined.block.hash()[0..8])
            );

            // Add to ledger
            match self.ledger.add_block(&mined.block) {
                Ok(()) => {
                    info!(
                        "Block {} added to ledger! Reward: {} credits",
                        mined.block.height(),
                        mined.block.mining_tx.reward as f64 / 1_000_000_000_000.0
                    );

                    // Check if we need to adjust difficulty
                    let new_height = mined.block.height();
                    if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
                        self.adjust_difficulty(new_height)?;
                    }

                    // Update miner with new work
                    if let Some(ref miner) = self.miner {
                        let state = self.ledger.get_chain_state().map_err(|e| {
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
        let start_block = self
            .ledger
            .get_block(window_start)
            .map_err(|e| anyhow::anyhow!("Failed to get start block: {}", e))?;
        let end_block = self
            .ledger
            .get_block(current_height)
            .map_err(|e| anyhow::anyhow!("Failed to get end block: {}", e))?;

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

            self.ledger
                .set_difficulty(new_difficulty)
                .map_err(|e| anyhow::anyhow!("Failed to set difficulty: {}", e))?;
        }

        Ok(())
    }

    fn print_mining_status(&self) -> Result<()> {
        if let Some(ref miner) = self.miner {
            let stats = miner.stats();
            let state = self
                .ledger
                .get_chain_state()
                .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

            println!(
                "[Mining] Height: {} | Hashrate: {:.2} H/s | Blocks: {} | Mined: {:.6} credits",
                state.height,
                stats.hashrate(),
                stats.blocks_found,
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

        self.ledger
            .add_block(block)
            .map_err(|e| anyhow::anyhow!("Failed to add network block: {}", e))?;

        // Check if we need to adjust difficulty
        let new_height = block.height();
        if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
            self.adjust_difficulty(new_height)?;
        }

        // Update miner with new work if mining
        if let Some(ref miner) = self.miner {
            let state = self
                .ledger
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

    /// Check if we've mined a block (non-blocking)
    pub fn check_mined_block(&mut self) -> Result<Option<crate::block::Block>> {
        if let Some(ref receiver) = self.block_receiver {
            if let Ok(mined) = receiver.try_recv() {
                info!(
                    "Mined block {} with hash {}",
                    mined.block.height(),
                    hex::encode(&mined.block.hash()[0..8])
                );

                // Add to our ledger
                self.ledger
                    .add_block(&mined.block)
                    .map_err(|e| anyhow::anyhow!("Failed to add mined block: {}", e))?;

                // Check if we need to adjust difficulty
                let new_height = mined.block.height();
                if new_height > 0 && new_height % ADJUSTMENT_WINDOW == 0 {
                    self.adjust_difficulty(new_height)?;
                }

                // Update miner with new work
                if let Some(ref miner) = self.miner {
                    let state = self
                        .ledger
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

                return Ok(Some(mined.block));
            }
        }
        Ok(None)
    }
}
