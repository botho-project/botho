use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::onetime_keys::{create_tx_out_public_key, create_tx_out_target_key};
use bth_util_from_random::FromRandom;
use rand_core::OsRng;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};
use tracing::{info, trace, warn};

use crate::{
    block::{calculate_block_reward, MintingTx},
    pow::{self, FastDataset, FastHasher, LightHasher, MinerHasher},
};

/// How many times a worker retries building the fast-mode RandomX dataset
/// before falling back to light mode (#566, part C). A single transient dataset
/// allocation failure (e.g. momentary memory pressure / huge-pages contention)
/// should not permanently wedge the miner, but we must not busy-loop either.
const FAST_BUILD_ATTEMPTS: u32 = 3;

/// Base backoff between fast-mode dataset build retries, in milliseconds. Grows
/// exponentially (×1, ×2, …) across attempts. Kept small: the goal is to ride
/// out a transient failure, not to stall for long before degrading to light
/// mode (#566, part C).
const FAST_BUILD_BACKOFF_BASE_MS: u64 = 500;

/// Upper bound on the **auto-detected** minting thread count (used only when
/// `minting.threads == 0`).
///
/// With one shared RandomX dataset (#568) the thread count no longer drives
/// memory: the miner needs ~2 GB for the single shared fast-mode dataset plus a
/// small (~2 MB) scratchpad per thread, regardless of `N`. So this cap is NOT a
/// RAM guard (that hazard is gone) — it only avoids oversubscribing a many-core
/// box and starving the node's own consensus / RPC / networking work with
/// mining threads (the in-process contention noted in #441/#444). An operator
/// who really wants more threads sets `minting.threads` explicitly, which
/// bypasses this cap entirely.
pub const MAX_AUTO_MINT_THREADS: usize = 16;

/// Choose the default number of minting threads when `minting.threads == 0`.
///
/// Returns the detected CPU count clamped to `[1, MAX_AUTO_MINT_THREADS]`.
///
/// This is the RAM-aware default (#568): before dataset sharing, defaulting to
/// `num_cpus::get()` implied `N × 2 GB` of RandomX datasets and could silently
/// exceed RAM — on the 2-core/3.8 GB faucet box that demanded ~4 GB and
/// OOM-halted the chain at height 213 (#539). Now every thread shares ONE ~2 GB
/// dataset, so the auto default is RAM-safe for any `N`; the clamp purely caps
/// oversubscription on large boxes.
pub fn default_mint_threads(detected_cpus: usize) -> usize {
    detected_cpus.clamp(1, MAX_AUTO_MINT_THREADS)
}

/// Genesis / initial minting difficulty target for **RandomX**.
///
/// The PoW check is `pow_value(hash) < difficulty` (see [`pow::pow_value`] and
/// [`mine`]), so a single hash succeeds with probability `difficulty / 2^64`
/// and the expected number of hashes per block is `2^64 / difficulty`. Higher
/// numeric `difficulty` = EASIER (more hashes pass) = FEWER hashes/block =
/// FASTER blocks. To hold a target block time `T` (s) at network hashrate `H`
/// (hashes/sec):
///
/// ```text
/// difficulty = 2^64 / (H * T)
/// ```
///
/// ## Calibration (see #444)
///
/// The earlier value `9_000_000_000_000_000` (9e15) was sized for an *idle*
/// 2-thread benchmark of ~415 H/s (`2^64 / 9e15 ≈ 2049` hashes/block → ~4.9 s
/// at 415 H/s). On the live testnet that proved far too optimistic: a single
/// mining thread *in-process on the full node* (contending with consensus, RPC
/// and networking on a burstable 2-vCPU t4g.medium, see #441) sustains only
/// **~68 H/s**, so 9e15 yielded `2049 / 68 ≈ 30 s` blocks during the
/// genesis/bootstrap window — far above the 5 s target.
///
/// Recalibrating for the realistic in-process single-thread hashrate
/// (`H ≈ 68 H/s`, `T = 5 s`):
///
/// ```text
/// target hashes/block = H * T      = 68 * 5     = 340
/// difficulty          = 2^64 / 340             ≈ 5.42e16
/// ```
///
/// We use a clean `54_000_000_000_000_000` (5.4e16, ≈ `0x00BF_D8B6_C1DF_0000`),
/// i.e. `2^64 / 5.4e16 ≈ 341.6` expected hashes/block → `341.6 / 68 ≈ 5.0 s`.
/// This is ~6x the old value, which is correct: 9e15 gave ~30 s and we want
/// ~5 s (6x faster ⇒ 6x fewer hashes/block ⇒ 6x larger difficulty, since
/// higher difficulty = easier here).
///
/// Note this only sets the *genesis* starting point. The live
/// [`crate::block::EmissionController`] adjusts difficulty from an
/// emission-rate (not block-time) error signal, so it does not pull block time
/// back toward the 5 s target on its own; getting the genesis constant right is
/// therefore the primary lever for early block time. See #444 for the
/// convergence analysis.
///
/// NOTE: this is a consensus-relevant constant and requires a fresh genesis to
/// take effect (the old SHA-256 value was `0x00FF_FFFF_FFFF_FFFF`, ~256
/// hashes/block; #443 moved to RandomX).
pub const INITIAL_DIFFICULTY: u64 = 54_000_000_000_000_000;

/// How long a miner may report `active == true` while producing **zero new
/// hashes** before it is flagged as stalled (see [`MinterHealth`]).
///
/// Motivation (#538, part of #537): the live testnet sat **halted for ~50h**
/// because the faucet miner reported `active:true` at `hashrate 0.0` — a wedged
/// RandomX worker that was alive but not hashing — and nothing surfaced it. A
/// healthy in-process single-thread miner sustains ~68 H/s (see
/// [`INITIAL_DIFFICULTY`]), so a full window of *no* new hashes is a strong,
/// unambiguous wedged-worker signal. 90 s is long enough to ride out a normal
/// difficulty/block-time stall (5 s target) yet far below the ~50h blind spot.
pub const STUCK_MINER_SECS: u64 = 90;

/// Startup grace period: a freshly-started miner builds the ~2 GB RandomX
/// fast-mode dataset (seconds, sometimes longer under load — see [`mint_loop`])
/// before it can hash at all. We must not flag that legitimate warm-up as a
/// stall, so detection is suppressed until the miner has been active for at
/// least this long. Chosen comfortably above observed dataset-build times.
pub const STARTUP_GRACE_SECS: u64 = 60;

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

/// A point-in-time health snapshot of the miner, suitable for RPC payloads and
/// the periodic stall check. Produced by [`MinterHealth::snapshot`].
#[derive(Debug, Clone, Copy)]
pub struct MinterHealthSnapshot {
    /// Whether minting is currently enabled.
    pub active: bool,
    /// Cumulative hashes computed since the miner started.
    pub total_hashes: u64,
    /// Average hashrate (hashes/sec) since start.
    pub hashrate: f64,
    /// Seconds the miner has been active.
    pub uptime_secs: u64,
    /// Stall verdict: `true` iff the miner is active but has produced no new
    /// hashes for longer than [`STUCK_MINER_SECS`], past the startup grace.
    pub stalled: bool,
    /// Number of blocks this node has *won* — i.e. externalized blocks whose
    /// winning coinbase was minted by this node's address (#543). Distinct from
    /// `total_hashes`: a node can hash continuously yet win zero blocks if it
    /// is always out-PoW'd, so this is the positive "am I actually
    /// producing blocks?" signal that complements the stuck-miner detector
    /// (#538).
    pub blocks_found: u64,
}

/// Pure stall verdict used by both the live [`MinterHealth`] handle and unit
/// tests. Returns `true` iff the miner is **active**, past the startup grace
/// window, and has produced **no new hashes** for at least `stuck_secs`.
///
/// Splitting this out keeps the detection logic deterministic and testable
/// without spinning up a real RandomX worker (#538).
///
/// - `active`: whether minting is enabled.
/// - `uptime_secs`: seconds since the miner started.
/// - `secs_since_progress`: seconds since `total_hashes` last advanced.
/// - `grace_secs` / `stuck_secs`: the startup grace and stall thresholds.
pub fn evaluate_stall(
    active: bool,
    uptime_secs: u64,
    secs_since_progress: u64,
    grace_secs: u64,
    stuck_secs: u64,
) -> bool {
    // Inactive miners are never "stalled" — not minting is a deliberate state,
    // not a fault.
    if !active {
        return false;
    }
    // During warm-up (RandomX VM/dataset build) zero hashes is expected.
    if uptime_secs < grace_secs {
        return false;
    }
    secs_since_progress >= stuck_secs
}

/// Shared, cloneable health handle for a [`Minter`].
///
/// Holds the same `Arc`-backed counters the worker threads update, so callers
/// (the RPC layer and the periodic status loop) observe live progress without
/// owning the [`Minter`]. Detecting a stall requires tracking the *last* point
/// at which `total_hashes` advanced; that bookkeeping lives here in atomics so
/// the handle stays `Send + Sync` and cheaply `Clone`able across threads.
#[derive(Clone)]
pub struct MinterHealth {
    /// Whether minting is currently enabled. Kept in sync by the node when it
    /// starts/stops minting.
    active: Arc<AtomicBool>,
    /// The worker threads' cumulative hash counter (shared with [`Minter`]).
    total_hashes: Arc<AtomicU64>,
    /// When the miner started, for uptime/hashrate.
    start_time: Instant,
    /// `total_hashes` value observed at the last progress check.
    last_observed_hashes: Arc<AtomicU64>,
    /// Seconds-since-start at the last time progress was observed advancing.
    last_progress_secs: Arc<AtomicU64>,
    /// Count of blocks won by this node — incremented exactly once per
    /// externalized block whose winning coinbase belongs to this node's
    /// address (#543). The increment site is the externalize hook in
    /// `commands::run`, gated by [`MinterHealth::owns_coinbase`].
    blocks_found: Arc<AtomicU64>,
    /// This minter's view public key, captured at construction from the
    /// reward address. Used to decide whether an externalized block's coinbase
    /// was minted by *this* node (vs another node winning the slot).
    minter_view_key: [u8; 32],
    /// This minter's spend public key (see `minter_view_key`).
    minter_spend_key: [u8; 32],
}

impl MinterHealth {
    fn new(total_hashes: Arc<AtomicU64>, start_time: Instant) -> Self {
        Self::with_address(total_hashes, start_time, [0u8; 32], [0u8; 32])
    }

    /// Construct a health handle bound to a specific minter address. The
    /// view/spend keys let the externalize hook attribute a won block to this
    /// node (#543).
    fn with_address(
        total_hashes: Arc<AtomicU64>,
        start_time: Instant,
        minter_view_key: [u8; 32],
        minter_spend_key: [u8; 32],
    ) -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            total_hashes,
            start_time,
            last_observed_hashes: Arc::new(AtomicU64::new(0)),
            last_progress_secs: Arc::new(AtomicU64::new(0)),
            blocks_found: Arc::new(AtomicU64::new(0)),
            minter_view_key,
            minter_spend_key,
        }
    }

    /// Fabricate a handle in a fully-controlled state for tests in *other*
    /// modules (the RPC layer asserts the `stalled`/`minerStalled` flags flow
    /// through `node_getStatus` / `minting_getStatus`). The `start_time` is
    /// backdated by `uptime_secs` so `snapshot()` reports a deterministic
    /// uptime and stall verdict without spinning up a real RandomX worker.
    #[doc(hidden)]
    pub fn for_test(active: bool, total_hashes: u64, uptime_secs: u64) -> Self {
        let start_time = Instant::now()
            .checked_sub(std::time::Duration::from_secs(uptime_secs))
            .unwrap_or_else(Instant::now);
        let h = Self::new(Arc::new(AtomicU64::new(total_hashes)), start_time);
        h.active.store(active, Ordering::SeqCst);
        // Pin "last progress" to start so secs_since_progress == uptime_secs:
        // i.e. no new hashes since the (backdated) start.
        h.last_observed_hashes.store(total_hashes, Ordering::SeqCst);
        h.last_progress_secs.store(0, Ordering::SeqCst);
        h
    }

    /// Mark whether minting is enabled. Resets the progress tracker on each
    /// active→edge so a restart gets a fresh grace window rather than
    /// inheriting a stale "no progress" timer.
    pub fn set_active(&self, active: bool) {
        let was_active = self.active.swap(active, Ordering::SeqCst);
        if active && !was_active {
            let now = self.start_time.elapsed().as_secs();
            self.last_observed_hashes
                .store(self.total_hashes.load(Ordering::Relaxed), Ordering::SeqCst);
            self.last_progress_secs.store(now, Ordering::SeqCst);
        }
    }

    /// Whether minting is currently enabled.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Whether the given coinbase keys belong to this node's minter address.
    ///
    /// The winning coinbase of an externalized block carries the minter's
    /// view/spend public keys ([`MintingTx::minter_view_key`] /
    /// [`MintingTx::minter_spend_key`]). Comparing both against this handle's
    /// captured keys tells us whether *this* node won the slot — the
    /// precondition for counting a block as "found" (#543).
    ///
    /// Returns `false` for an uninitialized (all-zero) key pair, so a handle
    /// not bound to a real address (e.g. the legacy [`MinterHealth::new`] path)
    /// never spuriously claims another node's coinbase.
    pub fn owns_coinbase(&self, view_key: &[u8; 32], spend_key: &[u8; 32]) -> bool {
        if self.minter_view_key == [0u8; 32] && self.minter_spend_key == [0u8; 32] {
            return false;
        }
        self.minter_view_key == *view_key && self.minter_spend_key == *spend_key
    }

    /// Record that this node won a block. Increments the `blocks_found` counter
    /// surfaced in `minting_getStatus` (#543). Call exactly once per
    /// externalized block whose coinbase satisfies [`Self::owns_coinbase`].
    pub fn increment_blocks_found(&self) {
        self.blocks_found.fetch_add(1, Ordering::SeqCst);
    }

    /// Current count of blocks won by this node (#543).
    pub fn blocks_found(&self) -> u64 {
        self.blocks_found.load(Ordering::SeqCst)
    }

    /// Advance the progress tracker and return the current health snapshot.
    ///
    /// This both *reads* the live counters and *updates* the "last progress"
    /// bookkeeping, so it is the single place the stall timer advances. The
    /// periodic status loop calls this on its tick; RPC handlers may call it
    /// too (the update is idempotent w.r.t. the verdict for a given
    /// instant).
    pub fn snapshot(&self) -> MinterHealthSnapshot {
        let active = self.active.load(Ordering::SeqCst);
        let total_hashes = self.total_hashes.load(Ordering::Relaxed);
        let uptime_secs = self.start_time.elapsed().as_secs();

        let hashrate = {
            let elapsed = self.start_time.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                total_hashes as f64 / elapsed
            } else {
                0.0
            }
        };

        // Update the last-progress timestamp if the counter advanced.
        let prev = self.last_observed_hashes.load(Ordering::SeqCst);
        if total_hashes > prev {
            self.last_observed_hashes
                .store(total_hashes, Ordering::SeqCst);
            self.last_progress_secs.store(uptime_secs, Ordering::SeqCst);
        }
        let last_progress = self.last_progress_secs.load(Ordering::SeqCst);
        let secs_since_progress = uptime_secs.saturating_sub(last_progress);

        let stalled = evaluate_stall(
            active,
            uptime_secs,
            secs_since_progress,
            STARTUP_GRACE_SECS,
            STUCK_MINER_SECS,
        );

        MinterHealthSnapshot {
            active,
            total_hashes,
            hashrate,
            uptime_secs,
            stalled,
            blocks_found: self.blocks_found.load(Ordering::SeqCst),
        }
    }

    /// Take a snapshot and, if stalled, emit a prominent warning. Returns the
    /// snapshot so callers can act on / report the verdict. Used by the
    /// periodic status loop as the chain's early-warning alarm (#538).
    pub fn check_and_warn(&self) -> MinterHealthSnapshot {
        let snap = self.snapshot();
        if snap.stalled {
            warn!(
                uptime_secs = snap.uptime_secs,
                total_hashes = snap.total_hashes,
                stuck_secs = STUCK_MINER_SECS,
                "RandomX miner stalled: active but 0 H/s for {}s — chain will halt; \
                 worker appears wedged (see #538). NOT auto-restarting (operator policy).",
                STUCK_MINER_SECS,
            );
        }
        snap
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
    /// Shared health handle for stall detection / RPC reporting (#538).
    health: MinterHealth,
    /// Count of mint worker threads still alive. Seeded to `threads` in
    /// [`Minter::start`] and decremented by each worker's [`WorkerExitGuard`]
    /// on exit (by ANY path, including a panic). When it reaches zero —
    /// i.e. every worker has died — the guard flips the health `active`
    /// flag false so a node with a dead mining thread can never report
    /// `active:true` (#566, the truthfulness fix for the #539 silent halt).
    live_workers: Arc<AtomicUsize>,
    /// The ONE fast-mode RandomX dataset (~2 GB) shared by every mining thread,
    /// bound to the current seed epoch. Built once (by whichever worker first
    /// reaches a new epoch) and reused by all threads via cheap per-VM clones,
    /// so the miner needs ~2 GB total + a small per-VM scratchpad instead of
    /// the `N × 2 GB` that OOM-halted the live testnet (#539/#568). Rebuilt
    /// only when the seed key rotates (every
    /// [`pow::SEED_ROTATION_INTERVAL`] blocks). The `Mutex` serializes the
    /// expensive build so two threads never race to allocate two datasets
    /// for the same epoch.
    fast_dataset: Arc<Mutex<Option<FastDataset>>>,
}

/// Decrements the live-worker count when a mint worker thread exits, and flips
/// the miner's health `active` flag false once the LAST worker is gone.
///
/// Because this runs in `Drop`, it fires on **every** termination path of a
/// worker — normal return, an early `break` (poisoned work lock, closed
/// channel, unbuildable hasher), or a panic (Drop runs while the thread
/// unwinds). That is the property the #539 post-mortem needed: previously a
/// dead mint thread left `active:true` with 0 H/s for ~50h. See #566.
struct WorkerExitGuard {
    live_workers: Arc<AtomicUsize>,
    health: MinterHealth,
}

impl Drop for WorkerExitGuard {
    fn drop(&mut self) {
        // `fetch_sub` returns the PREVIOUS value; `== 1` means this was the
        // last live worker, so the miner is now fully dead.
        if self.live_workers.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.health.set_active(false);
        }
    }
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

        let total_hashes = Arc::new(AtomicU64::new(0));
        let start_time = Instant::now();
        // Health handle shares the worker threads' hash counter so callers can
        // observe live progress (and detect a stall) without owning the Minter.
        // It also captures this minter's view/spend keys so the externalize
        // hook can attribute won blocks to this node (#543).
        let health = MinterHealth::with_address(
            total_hashes.clone(),
            start_time,
            address.view_public_key().to_bytes(),
            address.spend_public_key().to_bytes(),
        );

        Self {
            threads,
            address,
            shutdown: Arc::new(AtomicBool::new(false)),
            total_hashes,
            txs_found: Arc::new(AtomicU64::new(0)),
            start_time,
            handles: Vec::new(),
            tx_sender,
            tx_receiver: Some(tx_receiver),
            current_work: Arc::new(std::sync::RwLock::new(initial_work)),
            work_version: Arc::new(AtomicU64::new(0)),
            health,
            live_workers: Arc::new(AtomicUsize::new(0)),
            fast_dataset: Arc::new(Mutex::new(None)),
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
        // Seed the live-worker count and mark the miner active BEFORE spawning,
        // so the per-worker exit guard can authoritatively flip `active` false
        // once the LAST worker dies (#566). Ordering matters: if we set active
        // *after* spawning, a worker that died instantly could be overwritten
        // back to active=true, re-creating the very "active but dead" lie this
        // fix exists to prevent.
        self.live_workers.store(self.threads, Ordering::SeqCst);
        self.health.set_active(true);

        for thread_id in 0..self.threads {
            let shutdown = self.shutdown.clone();
            let total_hashes = self.total_hashes.clone();
            let txs_found = self.txs_found.clone();
            let address = self.address.clone();
            let tx_sender = self.tx_sender.clone();
            let current_work = self.current_work.clone();
            let work_version = self.work_version.clone();
            let live_workers = self.live_workers.clone();
            let health = self.health.clone();
            let fast_dataset = self.fast_dataset.clone();

            let handle = thread::spawn(move || {
                // The exit guard fires on EVERY way this worker can terminate —
                // normal return, an early `break`, or a panic (Drop runs during
                // unwind) — flipping `active` false once the last worker exits so
                // a node with a dead mining thread never reports `active:true`
                // (#566).
                let _exit_guard = WorkerExitGuard {
                    live_workers,
                    health,
                };
                mint_loop(
                    thread_id,
                    address,
                    shutdown,
                    total_hashes,
                    txs_found,
                    tx_sender,
                    current_work,
                    work_version,
                    fast_dataset,
                );
            });

            self.handles.push(handle);
        }
    }

    pub fn stop(self) {
        // Mark inactive first so the stall detector immediately stops flagging.
        self.health.set_active(false);
        // Signal shutdown to all minting threads
        self.shutdown.store(true, Ordering::SeqCst);
        // Wait for all threads to finish
        for handle in self.handles {
            let _ = handle.join();
        }
    }

    /// Clone the shared health handle for stall detection / RPC reporting.
    ///
    /// The handle observes live worker progress and outlives a single
    /// start/stop cycle; the node hands a clone to the RPC layer and the
    /// periodic status loop (#538).
    pub fn health(&self) -> MinterHealth {
        self.health.clone()
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
    fast_dataset: Arc<Mutex<Option<FastDataset>>>,
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

    // RandomX mining hasher. Built lazily and reused across all nonces/blocks
    // within a seed epoch; rebuilt only when the seed key rotates (every
    // `pow::SEED_ROTATION_INTERVAL` blocks). Building it is seconds-expensive,
    // so we must NOT recreate it per hash or per block.
    //
    // In fast mode the VM reads the minter-wide SHARED ~2 GB dataset
    // (`fast_dataset`); all threads' VMs read the same dataset, so the miner's
    // footprint is ~2 GB total + a small per-VM scratchpad, NOT `N × 2 GB` (the
    // OOM that halted the live testnet — #539/#568). If
    // that dataset cannot be built — even after bounded retries — the miner
    // transparently falls back to a light-mode hasher (~256 MB cache, much
    // slower) and KEEPS HASHING rather than dying, so an undersized box degrades
    // gracefully instead of silently halting the chain (#566). Light and fast
    // produce identical PoW output, so this is consensus-safe.
    let mut hasher: Option<MinerHasher> = None;

    while !shutdown.load(Ordering::Relaxed) {
        // Check if work has been updated
        let current_version = work_version.load(Ordering::Relaxed);
        if current_version != last_work_version || cached_work.is_none() {
            // If lock is poisoned, exit the minting loop gracefully
            let Ok(work_guard) = current_work.read() else {
                break;
            };
            // Clone the work and RELEASE the read lock before the seconds-
            // expensive hasher build below. Holding the read lock across the
            // dataset build (and now its retry/backoff window, #566) would block
            // `update_work`'s write lock for that whole duration.
            let work = work_guard.clone();
            drop(work_guard);
            cached_work = Some(work.clone());
            last_work_version = current_version;
            info!(
                thread = thread_id,
                height = work.height,
                prev_hash = hex::encode(&work.prev_block_hash[0..8]),
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

            // (Re)build the RandomX mining hasher if the seed epoch changed.
            // Most work updates (new tip every block) stay within the same
            // epoch, so this only does the expensive dataset build at epoch
            // boundaries.
            let seed_key = pow::seed_key_for_height(work.height);
            let need_rebuild = hasher
                .as_ref()
                .map(|h| h.seed_key() != seed_key)
                .unwrap_or(true);
            if need_rebuild {
                info!(
                    thread = thread_id,
                    height = work.height,
                    "Building RandomX mining hasher for new seed epoch (fast-mode \
                     dataset build takes a few seconds)"
                );
                // Drop this thread's previous-epoch hasher BEFORE building the
                // next one. Its VM holds an `Arc` clone of the old shared
                // dataset, so releasing it lets the old ~2 GB buffer free as soon
                // as the last worker rotates, keeping the epoch-boundary peak
                // bounded rather than stacking old+new across all threads (#568).
                hasher = None;
                match build_mining_hasher(thread_id, seed_key, &shutdown, &fast_dataset) {
                    Some(h) => hasher = Some(h),
                    None => {
                        // No hasher could be built (fast retries exhausted AND
                        // the light-mode fallback also failed) or shutdown was
                        // requested mid-build. Either way we cannot hash; exit.
                        // The exit guard flips the health `active` flag false so
                        // the dead thread is reported truthfully (#566).
                        break;
                    }
                }
            }
        }

        // Safe: `cached_work`/`hasher` are populated together in the block above
        // (or we `break`ed out), and once set they persist across iterations.
        let work = cached_work.as_ref().unwrap();
        let hasher = hasher.as_ref().expect("mining hasher built with work");

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

/// Obtain a fast-mode VM over the minter-wide **shared** ~2 GB dataset for
/// `seed_key`, (re)building the dataset at most once per seed epoch (#568).
///
/// The first worker to reach a new epoch pays the seconds-expensive ~2 GB build
/// while holding the `Mutex`; every other worker then gets a cheap VM over the
/// same dataset (a small per-VM scratchpad). This is what turns the miner's
/// memory footprint from `N × 2 GB` into ~2 GB + small per-VM scratchpads — the
/// fix for the live-testnet OOM halt (#539). The previous epoch's dataset is
/// dropped before the new one is allocated so the cell never holds two
/// datasets.
fn obtain_shared_fast_hasher(
    fast_dataset: &Mutex<Option<FastDataset>>,
    seed_key: [u8; 32],
) -> Result<FastHasher, randomx_rs::RandomXError> {
    // Recover from a poisoned lock rather than panicking: a worker that died
    // mid-build must not permanently wedge the rest of the miner. The dataset is
    // immutable once built, so the contents behind a poisoned lock are sound.
    let mut guard = fast_dataset
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let needs_build = guard
        .as_ref()
        .map(|d| d.seed_key() != seed_key)
        .unwrap_or(true);
    if needs_build {
        // Free the stale (previous-epoch) dataset BEFORE allocating the new one
        // so the cell holds at most one ~2 GB buffer at a time.
        *guard = None;
        *guard = Some(FastDataset::new(seed_key)?);
    }
    guard
        .as_ref()
        .expect("shared dataset present after build")
        .hasher()
}

/// Build the RandomX mining hasher for `seed_key`, resiliently (#566, parts
/// B+C) over the minter-wide shared dataset (#568).
///
/// Strategy:
/// 1. Try to build a fast-mode hasher (VM) over the **shared** ~2 GB dataset
///    (built once per epoch, see [`obtain_shared_fast_hasher`]). On failure,
///    retry up to [`FAST_BUILD_ATTEMPTS`] times with exponential backoff
///    ([`FAST_BUILD_BACKOFF_BASE_MS`]) — a single transient allocation failure
///    must not permanently wedge the miner, but we must not busy-loop either.
/// 2. If fast mode is still unavailable, fall back to a **light-mode** hasher
///    (~256 MB cache) and keep mining (slower) instead of dying. Light and fast
///    produce identical PoW output, so this is consensus-safe.
///
/// Returns `None` only when neither mode can be built, or when `shutdown` is
/// requested mid-build (so a stopping minter exits promptly rather than
/// sleeping out its full backoff). A `None` return causes the worker to exit,
/// which — via the exit guard — flips the health `active` flag false (#566,
/// part A).
fn build_mining_hasher(
    thread_id: usize,
    seed_key: [u8; 32],
    shutdown: &AtomicBool,
    fast_dataset: &Mutex<Option<FastDataset>>,
) -> Option<MinerHasher> {
    let mut last_err: Option<randomx_rs::RandomXError> = None;

    for attempt in 1..=FAST_BUILD_ATTEMPTS {
        if shutdown.load(Ordering::Relaxed) {
            return None;
        }
        match obtain_shared_fast_hasher(fast_dataset, seed_key) {
            Ok(h) => return Some(MinerHasher::Fast(h)),
            Err(e) => {
                warn!(
                    thread = thread_id,
                    attempt,
                    max_attempts = FAST_BUILD_ATTEMPTS,
                    error = %e,
                    "RandomX fast-mode dataset build failed; retrying with backoff"
                );
                last_err = Some(e);
                if attempt < FAST_BUILD_ATTEMPTS {
                    // Exponential backoff (×1, ×2, …), slept in short steps so a
                    // shutdown request is honored promptly.
                    let backoff_ms = FAST_BUILD_BACKOFF_BASE_MS << (attempt - 1);
                    let mut slept = 0u64;
                    while slept < backoff_ms {
                        if shutdown.load(Ordering::Relaxed) {
                            return None;
                        }
                        let step = (backoff_ms - slept).min(100);
                        thread::sleep(std::time::Duration::from_millis(step));
                        slept += step;
                    }
                }
            }
        }
    }

    if shutdown.load(Ordering::Relaxed) {
        return None;
    }

    // Fast mode exhausted — degrade to light mode and KEEP MINING (#566, part B).
    warn!(
        thread = thread_id,
        attempts = FAST_BUILD_ATTEMPTS,
        last_error = ?last_err,
        "RandomX fast-mode dataset unavailable after retries; FALLING BACK to \
         DEGRADED light-mode mining (much slower, but produces consensus-identical \
         PoW so blocks still verify everywhere — see #566). Check that the box has \
         enough RAM / huge-pages configured to restore fast-mode throughput."
    );
    match LightHasher::new(seed_key) {
        Ok(h) => Some(MinerHasher::Light(h)),
        Err(e) => {
            tracing::error!(
                thread = thread_id,
                error = %e,
                "RandomX light-mode hasher ALSO failed to build; minter thread \
                 cannot hash and will exit (active flag will be cleared, #566)"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- Stuck-miner detector (#538) -----

    /// Active + no new hashes past the stall window (and past grace) → stalled.
    #[test]
    fn test_stall_active_zero_hashrate_past_window() {
        // uptime past grace, no progress for >= STUCK_MINER_SECS.
        assert!(evaluate_stall(
            true,
            STARTUP_GRACE_SECS + STUCK_MINER_SECS + 5,
            STUCK_MINER_SECS + 5,
            STARTUP_GRACE_SECS,
            STUCK_MINER_SECS,
        ));
        // Exactly at the threshold also counts as stalled.
        assert!(evaluate_stall(
            true,
            STARTUP_GRACE_SECS + STUCK_MINER_SECS,
            STUCK_MINER_SECS,
            STARTUP_GRACE_SECS,
            STUCK_MINER_SECS,
        ));
    }

    /// Active + healthy progress (recent hashes) → not stalled.
    #[test]
    fn test_stall_active_healthy_hashrate_not_flagged() {
        // Plenty of uptime, but progress observed just 1s ago.
        assert!(!evaluate_stall(
            true,
            STARTUP_GRACE_SECS + STUCK_MINER_SECS + 100,
            1,
            STARTUP_GRACE_SECS,
            STUCK_MINER_SECS,
        ));
    }

    /// Inactive (minting disabled) → never flagged, regardless of timers.
    #[test]
    fn test_stall_inactive_never_flagged() {
        assert!(!evaluate_stall(
            false,
            STARTUP_GRACE_SECS + STUCK_MINER_SECS + 1000,
            STUCK_MINER_SECS + 1000,
            STARTUP_GRACE_SECS,
            STUCK_MINER_SECS,
        ));
    }

    /// Within startup grace + 0 hashes → not yet flagged (RandomX warm-up).
    #[test]
    fn test_stall_within_startup_grace_not_flagged() {
        // Just under the grace window with zero progress: must NOT flag.
        assert!(!evaluate_stall(
            true,
            STARTUP_GRACE_SECS - 1,
            STARTUP_GRACE_SECS - 1,
            STARTUP_GRACE_SECS,
            STUCK_MINER_SECS,
        ));
    }

    /// The live `MinterHealth` handle: an inactive handle reports inactive and
    /// unstalled; activating it starts the grace window so a brand-new miner is
    /// not immediately flagged.
    #[test]
    fn test_minter_health_handle_inactive_then_active() {
        let total_hashes = Arc::new(AtomicU64::new(0));
        let health = MinterHealth::new(total_hashes.clone(), Instant::now());

        // Fresh handle: not active, not stalled.
        let snap = health.snapshot();
        assert!(!snap.active);
        assert!(!snap.stalled);

        // Activate: still within grace (uptime ~0), so not stalled even at 0 H/s.
        health.set_active(true);
        let snap = health.snapshot();
        assert!(snap.active);
        assert!(!snap.stalled, "fresh active miner must not be flagged");

        // Progress advances the hashrate readout.
        total_hashes.store(1_000, Ordering::SeqCst);
        let snap = health.snapshot();
        assert_eq!(snap.total_hashes, 1_000);

        // Deactivate: never stalled.
        health.set_active(false);
        assert!(!health.snapshot().stalled);
    }

    /// `blocks_found` starts at 0 and is advanced only by
    /// `increment_blocks_found`, and the count is reflected in the snapshot
    /// (#543).
    #[test]
    fn test_minter_health_blocks_found_counter() {
        let health = MinterHealth::new(Arc::new(AtomicU64::new(0)), Instant::now());
        assert_eq!(health.snapshot().blocks_found, 0, "fresh handle => 0");

        health.increment_blocks_found();
        health.increment_blocks_found();
        assert_eq!(health.blocks_found(), 2);
        assert_eq!(health.snapshot().blocks_found, 2);
    }

    /// `owns_coinbase` matches this node's address keys and rejects others; an
    /// unbound (all-zero key) handle never claims ownership (#543).
    #[test]
    fn test_minter_health_owns_coinbase() {
        let mine_view = [7u8; 32];
        let mine_spend = [9u8; 32];
        let health = MinterHealth::with_address(
            Arc::new(AtomicU64::new(0)),
            Instant::now(),
            mine_view,
            mine_spend,
        );

        // Exact match on both keys => owned.
        assert!(health.owns_coinbase(&mine_view, &mine_spend));
        // Another node's coinbase => not owned.
        assert!(!health.owns_coinbase(&[1u8; 32], &[2u8; 32]));
        // Right view key but wrong spend key => not owned (both must match).
        assert!(!health.owns_coinbase(&mine_view, &[2u8; 32]));

        // Unbound handle (legacy `new` path => zero keys) never claims a
        // coinbase, even one with all-zero keys.
        let unbound = MinterHealth::new(Arc::new(AtomicU64::new(0)), Instant::now());
        assert!(!unbound.owns_coinbase(&[0u8; 32], &[0u8; 32]));
    }

    // ----- Truthful `active` flag on worker death (#566) -----

    /// The exit guard must flip the health `active` flag false only once the
    /// LAST worker exits — not while any worker is still alive. This is the
    /// core truthfulness mechanism that prevents a node with a dead mining
    /// thread from reporting `active:true` (the #539 silent halt).
    #[test]
    fn test_active_flips_false_when_last_worker_exits() {
        let health = MinterHealth::new(Arc::new(AtomicU64::new(0)), Instant::now());
        health.set_active(true);
        assert!(health.is_active());

        // Two live workers.
        let live = Arc::new(AtomicUsize::new(2));
        let g1 = WorkerExitGuard {
            live_workers: live.clone(),
            health: health.clone(),
        };
        let g2 = WorkerExitGuard {
            live_workers: live.clone(),
            health: health.clone(),
        };

        // First worker exits: one remains, so the miner is still active.
        drop(g1);
        assert!(
            health.is_active(),
            "miner must stay active while a worker remains"
        );

        // Last worker exits: the miner is now fully dead and must report so.
        drop(g2);
        assert!(
            !health.is_active(),
            "active must be false once the LAST worker exits (#566)"
        );
    }

    /// A worker that exits via a PANIC must still flip `active` false — the
    /// exit guard's `Drop` runs while the thread unwinds. This is the
    /// `.expect(\"RandomX ... hash failed\")` death path called out in
    /// #566/#539.
    #[test]
    fn test_active_flips_false_on_worker_panic() {
        let health = MinterHealth::new(Arc::new(AtomicU64::new(0)), Instant::now());
        health.set_active(true);
        let live = Arc::new(AtomicUsize::new(1));

        let h = health.clone();
        let l = live.clone();
        let handle = thread::spawn(move || {
            let _exit_guard = WorkerExitGuard {
                live_workers: l,
                health: h,
            };
            panic!("simulated RandomX hash failure");
        });
        // Join swallows the panic; the guard's Drop has already run during unwind.
        let _ = handle.join();

        assert!(
            !health.is_active(),
            "panic-exit of the last worker must still flip active false (#566)"
        );
    }

    /// The RAM-aware auto thread default (#568): clamps the detected CPU count
    /// to `[1, MAX_AUTO_MINT_THREADS]`. With one shared dataset this is no
    /// longer a RAM lever (it was `N × 2 GB` before), so the clamp is
    /// purely an oversubscription guard.
    #[test]
    fn test_default_mint_threads_clamps() {
        // At least one thread even if detection reports an absurd 0.
        assert_eq!(default_mint_threads(0), 1);
        // Typical small boxes pass through unchanged.
        assert_eq!(default_mint_threads(1), 1);
        assert_eq!(default_mint_threads(2), 2);
        assert_eq!(default_mint_threads(8), 8);
        // The cap value itself passes through.
        assert_eq!(
            default_mint_threads(MAX_AUTO_MINT_THREADS),
            MAX_AUTO_MINT_THREADS
        );
        // Many-core boxes are clamped to the oversubscription cap.
        assert_eq!(default_mint_threads(128), MAX_AUTO_MINT_THREADS);
        assert_eq!(
            default_mint_threads(MAX_AUTO_MINT_THREADS + 1),
            MAX_AUTO_MINT_THREADS
        );
    }

    /// Documents and locks the genesis difficulty calibration (#444).
    ///
    /// The PoW check is `pow_value(hash) < difficulty`, so expected hashes per
    /// block = `2^64 / difficulty`. We can't time real 5 s blocks in a unit
    /// test (RandomX is slow), but we can assert the arithmetic that the
    /// constant is derived from:
    ///
    ///   block_time ≈ (2^64 / INITIAL_DIFFICULTY) / hashrate
    ///
    /// at the measured in-process single-thread hashrate of ~68 H/s lands at
    /// ~5 s, not the ~30 s seen with the previous 9e15 value.
    #[test]
    fn test_initial_difficulty_calibration() {
        // Measured in-process single-thread hashrate on the live minter (#444).
        const MEASURED_HASHRATE: f64 = 68.0;
        const TARGET_BLOCK_TIME_SECS: f64 = 5.0;

        // Pin the calibrated value so a future edit can't silently regress it.
        assert_eq!(INITIAL_DIFFICULTY, 54_000_000_000_000_000);

        // Expected hashes per block = 2^64 / difficulty.
        let two_pow_64 = 2.0_f64.powi(64);
        let hashes_per_block = two_pow_64 / INITIAL_DIFFICULTY as f64;

        // ~341.6 hashes/block at the new constant.
        assert!(
            (hashes_per_block - 341.6).abs() < 1.0,
            "expected ~341.6 hashes/block, got {hashes_per_block}"
        );

        // At ~68 H/s that is ~5 s/block (the target), within 0.5 s.
        let block_time = hashes_per_block / MEASURED_HASHRATE;
        assert!(
            (block_time - TARGET_BLOCK_TIME_SECS).abs() < 0.5,
            "expected ~5 s/block at {MEASURED_HASHRATE} H/s, got {block_time} s"
        );

        // Direction sanity check against the comparison operator: a LARGER
        // difficulty constant means MORE hashes pass (`hash < difficulty`),
        // hence FEWER hashes/block, hence FASTER blocks. The previous 9e15
        // value therefore produced MORE hashes/block and SLOWER blocks.
        let old_difficulty = 9_000_000_000_000_000_u64;
        let old_hashes_per_block = two_pow_64 / old_difficulty as f64;
        assert!(
            old_hashes_per_block > hashes_per_block,
            "larger difficulty must mean fewer hashes/block (faster blocks)"
        );
        // And it reproduces the observed ~30 s data point at 68 H/s.
        let old_block_time = old_hashes_per_block / MEASURED_HASHRATE;
        assert!(
            (old_block_time - 30.0).abs() < 2.0,
            "old 9e15 should reproduce the observed ~30 s/block, got {old_block_time} s"
        );
    }

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
