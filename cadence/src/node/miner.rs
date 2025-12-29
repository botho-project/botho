use mc_account_keys::PublicAddress;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tracing::info;

use crate::block::{calculate_block_reward, MiningTx};

/// Mining difficulty target (lower = harder)
/// Start with a very easy target for testing
pub const INITIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;

/// Mining statistics
#[derive(Debug, Clone)]
pub struct MiningStats {
    pub total_hashes: u64,
    pub txs_found: u64,
    pub start_time: Instant,
}

impl MiningStats {
    pub fn hashrate(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.total_hashes as f64 / elapsed
        } else {
            0.0
        }
    }
}

/// A mined mining transaction ready to be submitted to consensus
#[derive(Debug, Clone)]
pub struct MinedMiningTx {
    /// The mining transaction with valid PoW
    pub mining_tx: MiningTx,
    /// PoW priority (higher = harder/better PoW)
    pub pow_priority: u64,
}

/// Work unit for miners - what they should be mining on
#[derive(Clone)]
pub struct MiningWork {
    pub prev_block_hash: [u8; 32],
    pub height: u64,
    pub difficulty: u64,
    pub total_mined: u64,
}

/// The miner manages mining threads
pub struct Miner {
    threads: usize,
    address: PublicAddress,
    shutdown: Arc<AtomicBool>,
    total_hashes: Arc<AtomicU64>,
    txs_found: Arc<AtomicU64>,
    start_time: Instant,
    handles: Vec<JoinHandle<()>>,
    /// Channel for found mining transactions
    tx_sender: Sender<MinedMiningTx>,
    /// Receiver for found mining transactions (taken by the node)
    tx_receiver: Option<Receiver<MinedMiningTx>>,
    /// Current work (shared with threads)
    current_work: Arc<std::sync::RwLock<MiningWork>>,
    /// Signal to update work
    work_version: Arc<AtomicU64>,
}

impl Miner {
    pub fn new(threads: usize, address: PublicAddress, shutdown: Arc<AtomicBool>) -> Self {
        let (tx_sender, tx_receiver) = channel();

        // Initialize with default work (will be updated before mining starts)
        let initial_work = MiningWork {
            prev_block_hash: [0u8; 32],
            height: 1,
            difficulty: INITIAL_DIFFICULTY,
            total_mined: 0,
        };

        Self {
            threads,
            address,
            shutdown,
            total_hashes: Arc::new(AtomicU64::new(0)),
            txs_found: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            handles: Vec::new(),
            tx_sender,
            tx_receiver: Some(tx_receiver),
            current_work: Arc::new(std::sync::RwLock::new(initial_work)),
            work_version: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Take the mining tx receiver (can only be called once)
    pub fn take_tx_receiver(&mut self) -> Option<Receiver<MinedMiningTx>> {
        self.tx_receiver.take()
    }

    /// Update the work for all mining threads
    pub fn update_work(&self, work: MiningWork) {
        {
            let mut current = self.current_work.write().unwrap();
            *current = work;
        }
        self.work_version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn start(&mut self) {
        for thread_id in 0..self.threads {
            let shutdown = self.shutdown.clone();
            let total_hashes = self.total_hashes.clone();
            let blocks_found = self.blocks_found.clone();
            let address = self.address.clone();
            let block_sender = self.block_sender.clone();
            let current_work = self.current_work.clone();
            let work_version = self.work_version.clone();

            let handle = thread::spawn(move || {
                mine_loop(
                    thread_id,
                    address,
                    shutdown,
                    total_hashes,
                    blocks_found,
                    block_sender,
                    current_work,
                    work_version,
                );
            });

            self.handles.push(handle);
        }
    }

    pub fn stop(self) {
        // Shutdown signal should already be set
        for handle in self.handles {
            let _ = handle.join();
        }
    }

    pub fn stats(&self) -> MiningStats {
        MiningStats {
            total_hashes: self.total_hashes.load(Ordering::Relaxed),
            blocks_found: self.blocks_found.load(Ordering::Relaxed),
            start_time: self.start_time,
        }
    }
}

/// The actual mining loop
fn mine_loop(
    thread_id: usize,
    address: PublicAddress,
    shutdown: Arc<AtomicBool>,
    total_hashes: Arc<AtomicU64>,
    blocks_found: Arc<AtomicU64>,
    block_sender: Sender<MinedBlock>,
    current_work: Arc<std::sync::RwLock<MiningWork>>,
    work_version: Arc<AtomicU64>,
) {
    // Each thread starts at a different nonce to avoid overlap
    let mut nonce: u64 = (thread_id as u64) << 56;
    let mut local_hashes: u64 = 0;
    let mut last_work_version = 0u64;
    let mut cached_work: Option<MiningWork> = None;

    const BATCH_SIZE: u64 = 10000;

    // Pre-compute the address bytes
    let miner_view_key = address.view_public_key().to_bytes();
    let miner_spend_key = address.spend_public_key().to_bytes();
    let address_bytes = [miner_view_key, miner_spend_key].concat();

    while !shutdown.load(Ordering::Relaxed) {
        // Check if work has been updated
        let current_version = work_version.load(Ordering::Relaxed);
        if current_version != last_work_version || cached_work.is_none() {
            cached_work = Some(current_work.read().unwrap().clone());
            last_work_version = current_version;
            // Reset nonce when work changes to avoid collisions
            nonce = (thread_id as u64) << 56;
        }

        let work = cached_work.as_ref().unwrap();

        // Compute PoW hash: SHA256(nonce || prev_block_hash || miner_address)
        let hash = compute_pow_hash(nonce, &work.prev_block_hash, &address_bytes);

        // Check if hash meets difficulty target
        let hash_value = u64::from_be_bytes(hash[0..8].try_into().unwrap());

        if hash_value < work.difficulty {
            // Found a valid block!
            blocks_found.fetch_add(1, Ordering::Relaxed);

            let reward = calculate_block_reward(work.height, work.total_mined);

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let block = Block {
                header: BlockHeader {
                    version: 1,
                    prev_block_hash: work.prev_block_hash,
                    tx_root: [0u8; 32], // No transactions in mined blocks yet
                    timestamp,
                    height: work.height,
                    difficulty: work.difficulty,
                    nonce,
                    miner_view_key,
                    miner_spend_key,
                },
                mining_tx: MiningTx {
                    block_height: work.height,
                    reward,
                    recipient_view_key: miner_view_key,
                    recipient_spend_key: miner_spend_key,
                    output_public_key: [0u8; 32], // TODO: generate one-time key
                    prev_block_hash: work.prev_block_hash,
                    difficulty: work.difficulty,
                    nonce,
                    timestamp,
                },
                transactions: Vec::new(),
            };

            info!(
                "Thread {} found block {}! Nonce: {}, Hash: {}, Reward: {} picocredits",
                thread_id,
                work.height,
                nonce,
                hex::encode(&hash[0..8]),
                reward
            );

            // Send block to main thread
            if block_sender.send(MinedBlock { block }).is_err() {
                // Channel closed, exit
                break;
            }

            // Wait for work to be updated with new block
            // (We'll continue mining on the same work until it's updated,
            // which is suboptimal but simple)
        }

        nonce = nonce.wrapping_add(1);
        local_hashes += 1;

        // Periodically update global counter
        if local_hashes >= BATCH_SIZE {
            total_hashes.fetch_add(local_hashes, Ordering::Relaxed);
            local_hashes = 0;
        }
    }

    // Flush remaining hashes
    if local_hashes > 0 {
        total_hashes.fetch_add(local_hashes, Ordering::Relaxed);
    }
}

/// Compute the PoW hash: SHA256(nonce || prev_block_hash || address)
fn compute_pow_hash(nonce: u64, prev_block_hash: &[u8; 32], address_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(nonce.to_le_bytes());
    hasher.update(prev_block_hash);
    hasher.update(address_bytes);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_pow_hash() {
        let nonce = 12345u64;
        let prev_hash = [0u8; 32];
        let address = vec![1u8; 64];

        let hash = compute_pow_hash(nonce, &prev_hash, &address);
        assert_eq!(hash.len(), 32);

        // Same inputs should produce same hash
        let hash2 = compute_pow_hash(nonce, &prev_hash, &address);
        assert_eq!(hash, hash2);

        // Different nonce should produce different hash
        let hash3 = compute_pow_hash(nonce + 1, &prev_hash, &address);
        assert_ne!(hash, hash3);
    }
}
