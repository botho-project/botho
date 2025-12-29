use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::onetime_keys::{create_tx_out_public_key, create_tx_out_target_key};
use bth_util_from_random::FromRandom;
use rand_core::OsRng;
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
            let txs_found = self.txs_found.clone();
            let address = self.address.clone();
            let tx_sender = self.tx_sender.clone();
            let current_work = self.current_work.clone();
            let work_version = self.work_version.clone();

            let handle = thread::spawn(move || {
                mine_loop(
                    thread_id,
                    address,
                    shutdown,
                    total_hashes,
                    txs_found,
                    tx_sender,
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
            txs_found: self.txs_found.load(Ordering::Relaxed),
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
    txs_found: Arc<AtomicU64>,
    tx_sender: Sender<MinedMiningTx>,
    current_work: Arc<std::sync::RwLock<MiningWork>>,
    work_version: Arc<AtomicU64>,
) {
    // Each thread starts at a different nonce to avoid overlap
    let mut nonce: u64 = (thread_id as u64) << 56;
    let mut local_hashes: u64 = 0;
    let mut last_work_version = 0u64;
    let mut cached_work: Option<MiningWork> = None;

    const BATCH_SIZE: u64 = 10000;

    // Stealth keys for the current mining work
    let mut cached_target_key = [0u8; 32];
    let mut cached_public_key = [0u8; 32];

    while !shutdown.load(Ordering::Relaxed) {
        // Check if work has been updated
        let current_version = work_version.load(Ordering::Relaxed);
        if current_version != last_work_version || cached_work.is_none() {
            cached_work = Some(current_work.read().unwrap().clone());
            last_work_version = current_version;
            // Reset nonce when work changes to avoid collisions
            nonce = (thread_id as u64) << 56;

            // Generate new stealth keys for this work unit
            let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);
            let target_key = create_tx_out_target_key(&tx_private_key, &address);
            let public_key =
                create_tx_out_public_key(&tx_private_key, address.spend_public_key());
            cached_target_key = target_key.to_bytes();
            cached_public_key = public_key.to_bytes();
        }

        let work = cached_work.as_ref().unwrap();

        // Compute PoW hash: SHA256(nonce || prev_block_hash || target_key || public_key)
        // Using stealth keys in PoW binds the proof to the specific output
        let stealth_bytes = [cached_target_key, cached_public_key].concat();
        let hash = compute_pow_hash(nonce, &work.prev_block_hash, &stealth_bytes);

        // Check if hash meets difficulty target
        let hash_value = u64::from_be_bytes(hash[0..8].try_into().unwrap());

        if hash_value < work.difficulty {
            // Found a valid mining transaction!
            txs_found.fetch_add(1, Ordering::Relaxed);

            let reward = calculate_block_reward(work.height, work.total_mined);

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            // Create the mining transaction with stealth output and PoW proof
            // Includes both miner identity (for PoW binding) and stealth keys (for private output)
            let mining_tx = MiningTx {
                block_height: work.height,
                reward,
                miner_view_key: address.view_public_key().to_bytes(),
                miner_spend_key: address.spend_public_key().to_bytes(),
                target_key: cached_target_key,
                public_key: cached_public_key,
                prev_block_hash: work.prev_block_hash,
                difficulty: work.difficulty,
                nonce,
                timestamp,
            };

            // Calculate PoW priority (higher = better PoW)
            // Invert hash value so lower hash = higher priority
            let pow_priority = u64::MAX - hash_value;

            info!(
                "Thread {} found mining tx for height {}! Nonce: {}, Hash: {}, Priority: {}, Reward: {} picocredits",
                thread_id,
                work.height,
                nonce,
                hex::encode(&hash[0..8]),
                pow_priority,
                reward
            );

            // Send mining tx to main thread for consensus submission
            if tx_sender
                .send(MinedMiningTx {
                    mining_tx,
                    pow_priority,
                })
                .is_err()
            {
                // Channel closed, exit
                break;
            }

            // Continue mining - multiple miners may find valid PoW
            // The best one (highest priority) will win in consensus
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
