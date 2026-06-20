use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::onetime_keys::{create_tx_out_public_key, create_tx_out_target_key};
use bth_util_from_random::FromRandom;
use rand_core::OsRng;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Instant,
};
use tracing::{info, trace};

use crate::{
    block::{calculate_block_reward, MintingTx},
    pow::{self, FastHasher},
};

/// Genesis / initial minting difficulty target for **RandomX**.
///
/// The PoW check is `pow_value(hash) < difficulty`, so a single hash succeeds
/// with probability `difficulty / 2^64` and the expected number of hashes per
/// block is `2^64 / difficulty`. To hold the 5 s target block time at network
/// hashrate `H` (hashes/sec): `difficulty = 2^64 / (H * 5)`.
///
/// RandomX runs at ~415 H/s on a 2-vCPU t4g.medium (the minimum *mining* tier,
/// see #441), orders of magnitude below SHA-256. Sizing genesis for a single
/// such node (the conservative floor — a fresh chain must not stall) gives:
///
/// ```text
/// difficulty = 2^64 / (415 * 5) ≈ 8.89e15
/// ```
///
/// We use a clean `9_000_000_000_000_000` (≈ 0x1F_F973_CAFA_8000), i.e. ~2049
/// expected hashes/block → ≈ 4.9 s at 415 H/s for one node, ≈ 2.5 s for two
/// nodes. The `EmissionController` then adapts difficulty downward (toward
/// `MIN_DIFFICULTY`) as hashrate grows. Higher numeric `difficulty` = EASIER
/// here (more hashes pass), so this is also the controller's ceiling
/// (`MAX_DIFFICULTY`).
///
/// NOTE: this is consensus-breaking vs the old SHA-256 value
/// (`0x00FF_FFFF_FFFF_FFFF`, ~256 hashes/block) and requires a fresh genesis.
pub const INITIAL_DIFFICULTY: u64 = 9_000_000_000_000_000;

/// Minting statistics
#[derive(Debug, Clone)]
pub struct MintingStats {
    pub total_hashes: u64,
    pub txs_found: u64,
    pub start_time: Instant,
}

impl MintingStats {
    pub fn hashrate(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.total_hashes as f64 / elapsed
        } else {
            0.0
        }
    }
}

/// A minted minting transaction ready to be submitted to consensus
#[derive(Debug, Clone)]
pub struct MintedMintingTx {
    /// The minting transaction with valid PoW
    pub minting_tx: MintingTx,
    /// PoW priority (higher = harder/better PoW)
    pub pow_priority: u64,
    /// Work version when this transaction was found
    /// Used to discard stale transactions from the channel
    pub work_version: u64,
}

/// Work unit for minters - what they should be minting
#[derive(Clone)]
pub struct MintingWork {
    pub prev_block_hash: [u8; 32],
    pub height: u64,
    pub difficulty: u64,
    /// Total minted (gross emission), in picocredits. Used for reward
    /// calculation. `u128` to match `ChainState.total_mined` (see #333).
    pub total_minted: u128,
}

/// The minter manages minting threads
pub struct Minter {
    threads: usize,
    address: PublicAddress,
    shutdown: Arc<AtomicBool>,
    total_hashes: Arc<AtomicU64>,
    txs_found: Arc<AtomicU64>,
    start_time: Instant,
    handles: Vec<JoinHandle<()>>,
    /// Channel for found minting transactions
    tx_sender: Sender<MintedMintingTx>,
    /// Receiver for found minting transactions (taken by the node)
    tx_receiver: Option<Receiver<MintedMintingTx>>,
    /// Current work (shared with threads)
    current_work: Arc<std::sync::RwLock<MintingWork>>,
    /// Signal to update work
    work_version: Arc<AtomicU64>,
}

impl Minter {
    /// Create a new minter.
    ///
    /// Each minter owns its own `shutdown` flag (created here, initialized to
    /// `false`). This is deliberate: a minter must NOT share the node-wide
    /// shutdown flag, because [`Minter::stop`] sets the flag to `true`
    /// permanently. If the flag were shared, a subsequent `start_minting`
    /// would spawn threads that immediately observe `shutdown == true` and exit
    /// before ever picking up work — producing a "zombie minter" that reports
    /// active but never mines (see issue #388, the pause→resume cycle from
    /// #386/#387).
    pub fn new(threads: usize, address: PublicAddress) -> Self {
        let (tx_sender, tx_receiver) = channel();

        // Initialize with default work (will be updated before minting starts)
        let initial_work = MintingWork {
            prev_block_hash: [0u8; 32],
            height: 1,
            difficulty: INITIAL_DIFFICULTY,
            total_minted: 0,
        };

        Self {
            threads,
            address,
            shutdown: Arc::new(AtomicBool::new(false)),
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

    /// Take the minting tx receiver (can only be called once)
    pub fn take_tx_receiver(&mut self) -> Option<Receiver<MintedMintingTx>> {
        self.tx_receiver.take()
    }

    /// Update the work for all minting threads
    pub fn update_work(&self, work: MintingWork) {
        if let Ok(mut current) = self.current_work.write() {
            *current = work;
            drop(current);
            self.work_version.fetch_add(1, Ordering::SeqCst);
        }
        // If lock is poisoned, minting threads will detect stale work and exit
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
                mint_loop(
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
        // Signal shutdown to all minting threads
        self.shutdown.store(true, Ordering::SeqCst);
        // Wait for all threads to finish
        for handle in self.handles {
            let _ = handle.join();
        }
    }

    pub fn stats(&self) -> MintingStats {
        MintingStats {
            total_hashes: self.total_hashes.load(Ordering::Relaxed),
            txs_found: self.txs_found.load(Ordering::Relaxed),
            start_time: self.start_time,
        }
    }

    /// Get the current work version
    /// Used to filter out stale transactions from the channel
    pub fn current_work_version(&self) -> u64 {
        self.work_version.load(Ordering::SeqCst)
    }
}

/// The actual minting loop
fn mint_loop(
    thread_id: usize,
    address: PublicAddress,
    shutdown: Arc<AtomicBool>,
    total_hashes: Arc<AtomicU64>,
    txs_found: Arc<AtomicU64>,
    tx_sender: Sender<MintedMintingTx>,
    current_work: Arc<std::sync::RwLock<MintingWork>>,
    work_version: Arc<AtomicU64>,
) {
    // Each thread starts at a different nonce to avoid overlap
    let mut nonce: u64 = (thread_id as u64) << 56;
    let mut local_hashes: u64 = 0;
    let mut last_work_version = 0u64;
    let mut cached_work: Option<MintingWork> = None;

    const BATCH_SIZE: u64 = 10000;

    // Minter keys (constant for this session) - bound into the PoW preimage.
    let minter_view_key = address.view_public_key().to_bytes();
    let minter_spend_key = address.spend_public_key().to_bytes();

    // Stealth keys for the current minting work (regenerated when work changes)
    let mut cached_target_key = [0u8; 32];
    let mut cached_public_key = [0u8; 32];

    // RandomX fast-mode hasher (~2 GB dataset). Built lazily and reused across
    // all nonces/blocks within a seed epoch; rebuilt only when the seed key
    // rotates (every `pow::SEED_ROTATION_INTERVAL` blocks). Building it is
    // seconds-expensive, so we must NOT recreate it per hash or per block.
    let mut fast_hasher: Option<FastHasher> = None;

    while !shutdown.load(Ordering::Relaxed) {
        // Check if work has been updated
        let current_version = work_version.load(Ordering::Relaxed);
        if current_version != last_work_version || cached_work.is_none() {
            // If lock is poisoned, exit the minting loop gracefully
            let Ok(work_guard) = current_work.read() else {
                break;
            };
            cached_work = Some(work_guard.clone());
            last_work_version = current_version;
            info!(
                thread = thread_id,
                height = work_guard.height,
                prev_hash = hex::encode(&work_guard.prev_block_hash[0..8]),
                "Thread picked up new work"
            );
            // Reset nonce when work changes to avoid collisions
            nonce = (thread_id as u64) << 56;

            // Generate new stealth keys for this work unit
            let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);
            let target_key = create_tx_out_target_key(&tx_private_key, &address);
            let public_key = create_tx_out_public_key(&tx_private_key, address.spend_public_key());
            cached_target_key = target_key.to_bytes();
            cached_public_key = public_key.to_bytes();

            // (Re)build the RandomX fast-mode hasher if the seed epoch changed.
            // Most work updates (new tip every block) stay within the same
            // epoch, so this only does the expensive ~2 GB dataset build at
            // epoch boundaries.
            let seed_key = pow::seed_key_for_height(work_guard.height);
            let need_rebuild = fast_hasher
                .as_ref()
                .map(|h| h.seed_key() != seed_key)
                .unwrap_or(true);
            if need_rebuild {
                info!(
                    thread = thread_id,
                    height = work_guard.height,
                    "Building RandomX fast-mode dataset for new seed epoch (this takes a few seconds)"
                );
                match FastHasher::new(seed_key) {
                    Ok(h) => fast_hasher = Some(h),
                    Err(e) => {
                        tracing::error!(
                            thread = thread_id,
                            error = %e,
                            "Failed to build RandomX fast hasher; stopping minter thread"
                        );
                        break;
                    }
                }
            }
        }

        let work = cached_work.as_ref().unwrap();
        let hasher = fast_hasher.as_ref().expect("fast hasher built with work");

        // Compute PoW hash: RandomX(seed) over
        // nonce || prev_block_hash || minter_view_key || minter_spend_key.
        // Matches MintingTx::pow_hash / BlockHeader::pow_hash exactly (same
        // preimage, same per-height seed), so what we mine verifies in light
        // mode on every node.
        let preimage = pow::pow_preimage(
            nonce,
            &work.prev_block_hash,
            &minter_view_key,
            &minter_spend_key,
        );
        let hash = hasher.hash(&preimage);

        // Check if hash meets difficulty target
        let hash_value = pow::pow_value(&hash);

        if hash_value < work.difficulty {
            // Found a valid minting transaction!
            txs_found.fetch_add(1, Ordering::Relaxed);

            // Block-based halving: reward is calculated from height and total supply
            // using MonetaryPolicy with 5s block assumption
            let reward = calculate_block_reward(work.height, work.total_minted);

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            // Create the minting transaction with stealth output and PoW proof
            // Includes both minter identity (for PoW binding) and stealth keys (for private
            // output)
            let minting_tx = MintingTx {
                block_height: work.height,
                reward,
                minter_view_key: address.view_public_key().to_bytes(),
                minter_spend_key: address.spend_public_key().to_bytes(),
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

            trace!(
                "Thread {} found minting tx for height {}! Nonce: {}, Hash: {}, Priority: {}, Reward: {} picocredits",
                thread_id,
                work.height,
                nonce,
                hex::encode(&hash[0..8]),
                pow_priority,
                reward
            );

            // Send minting tx to main thread for consensus submission
            // Include work version so stale transactions can be filtered
            if tx_sender
                .send(MintedMintingTx {
                    minting_tx,
                    pow_priority,
                    work_version: last_work_version,
                })
                .is_err()
            {
                // Channel closed, exit
                break;
            }

            // Continue minting - multiple minters may find valid PoW
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The minter's fast-mode RandomX hash MUST equal the light-mode verify
    /// hash for the same preimage + seed — otherwise mined blocks would fail
    /// verification. Builds the ~2 GB dataset, so it is `#[ignore]`d by
    /// default.
    #[test]
    #[ignore = "builds the ~2 GB RandomX fast dataset; slow + RAM-heavy, run manually"]
    fn test_fast_hash_matches_light() {
        let nonce = 12345u64;
        let prev_hash = [0u8; 32];
        let view = [1u8; 32];
        let spend = [2u8; 32];

        let seed = pow::seed_key_for_height(1);
        let preimage = pow::pow_preimage(nonce, &prev_hash, &view, &spend);

        let hasher = FastHasher::new(seed).expect("fast hasher");
        let fast = hasher.hash(&preimage);
        let light = pow::verify_pow_hash(&seed, &preimage);
        assert_eq!(fast, light, "fast != light");

        // Determinism within the same hasher.
        assert_eq!(fast, hasher.hash(&preimage));

        // Different nonce changes the hash.
        let preimage2 = pow::pow_preimage(nonce + 1, &prev_hash, &view, &spend);
        assert_ne!(fast, hasher.hash(&preimage2));
    }

    /// Spin up a minter the way `Node::start_minting` does, run it until it
    /// picks up work and produces at least one minting tx, then stop it.
    /// Returns whether a minting tx was received within the timeout.
    fn run_minter_once(address: &PublicAddress) -> bool {
        let mut minter = Minter::new(1, address.clone());
        let rx = minter.take_tx_receiver().expect("receiver available");

        // Easy difficulty so PoW is found almost immediately.
        minter.update_work(MintingWork {
            prev_block_hash: [7u8; 32],
            height: 1,
            difficulty: INITIAL_DIFFICULTY,
            total_minted: 0,
        });
        minter.start();

        // A minting tx must arrive — this proves the thread picked up work and
        // mined a block, not merely that the thread is alive. With RandomX the
        // thread first builds the ~2 GB fast dataset (seconds) before mining, so
        // allow a generous timeout.
        let produced = rx.recv_timeout(std::time::Duration::from_secs(120)).is_ok();

        minter.stop();
        produced
    }

    /// Regression test for issue #388 (zombie minter).
    ///
    /// Previously, `Minter` shared the node-wide shutdown flag. `Minter::stop`
    /// set that flag to `true` permanently, so the SECOND minter created after
    /// a stop saw `shutdown == true` immediately and exited before picking up
    /// any work — the node reported "minting active" but produced no blocks.
    ///
    /// This test reproduces the start → stop → start cycle and asserts that the
    /// resumed minter ALSO produces a minting tx, exactly like a fresh startup.
    #[test]
    #[ignore = "RandomX minter builds the ~2 GB fast dataset and mines a real \
                PoW block; slow + RAM-heavy, run manually"]
    fn test_minter_resumes_work_after_stop_start_cycle() {
        let address = PublicAddress::from_random(&mut OsRng);

        // Fresh startup: produces work.
        assert!(
            run_minter_once(&address),
            "initial minter should pick up work and produce a minting tx"
        );

        // Resume after a stop: must ALSO produce work (the regression).
        assert!(
            run_minter_once(&address),
            "resumed minter (start after stop) should pick up work and \
             produce a minting tx — a no-op here is the #388 zombie-minter bug"
        );
    }
}
