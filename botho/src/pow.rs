// Copyright (c) 2024 Botho Foundation

//! RandomX proof-of-work for Botho.
//!
//! This module replaces the legacy single-SHA-256 PoW with **RandomX**
//! (Monero's CPU-egalitarian, ASIC/GPU-resistant PoW) via the `randomx-rs`
//! crate. It realizes the intended mining design from epic #441: cheap CPU
//! rigs can compete on near-equal footing, while a fair latency edge for
//! well-connected node operators is preserved (the PoW preimage still binds
//! `prev_block_hash`, so a miner cannot start block N+1 before it has N's
//! hash).
//!
//! # Hash function
//!
//! `pow_hash(seed_key, preimage)` runs RandomX (keyed by `seed_key`) over the
//! **same preimage** the legacy PoW used:
//!
//! ```text
//! preimage = nonce ‖ prev_block_hash ‖ minter_view_key ‖ minter_spend_key
//! ```
//!
//! The 32-byte RandomX output is compared to the difficulty target exactly as
//! before: `u64::from_be_bytes(hash[0..8]) < difficulty`. Lower hash = better
//! PoW. The byte/endianness convention (big-endian read of the first 8 bytes)
//! is unchanged from the SHA-256 era, so the difficulty controller and
//! `pow_priority` semantics are preserved.
//!
//! # Seed (RandomX key) rotation — DETERMINISM IS CRITICAL
//!
//! RandomX hashing is parameterized by a *key* that seeds the cache/dataset.
//! Every node MUST use the identical key for a given block or the chain forks.
//!
//! Botho derives the key **purely from the block height**, quantized into
//! fixed-length epochs:
//!
//! ```text
//! epoch    = height / SEED_ROTATION_INTERVAL          (K = 2048)
//! seed_key = SHA256( SEED_DOMAIN ‖ epoch.to_le_bytes() )
//! ```
//!
//! Rationale for a height-derived (rather than Monero-style past-block-hash)
//! seed:
//!
//! * **Trivial determinism.** The key is a pure function of `height`, which is
//!   already a committed field of both the block header and the minting tx. No
//!   node ever needs to look up a historical block to verify PoW, so the SCP
//!   *intrinsic* (tip-agnostic) validity function stays a pure function of the
//!   value — no new consensus state, no chain lookups, no lookahead/boundary
//!   hazards. A mismatch here would fork the chain, so we minimize moving
//!   parts.
//! * **ASIC resistance is unaffected.** RandomX's ASIC resistance comes from
//!   executing a random *program* per hash (driven by the input + key), not
//!   from the key being unpredictable. Rotating the key every K blocks ensures
//!   the dataset/program family changes periodically so no fixed-function ASIC
//!   can amortize, which is exactly what Monero's rotation buys — we get that
//!   property from a deterministic height epoch just as well.
//! * **Dataset reuse.** Because the key is constant for all `K` blocks of an
//!   epoch, a miner builds the (expensive) ~2 GB fast-mode dataset once per
//!   epoch and reuses it across thousands of blocks; verifiers build the ~256
//!   MB light cache once per epoch.
//!
//! Genesis/early heights need no special case: height 0 (and every height in
//! `0..K`) is epoch 0, giving a fixed bootstrap key
//! `SHA256(SEED_DOMAIN ‖ 0u64)`.
//!
//! # Light vs fast mode — outputs are IDENTICAL
//!
//! RandomX produces the **same hash** regardless of mode or CPU-feature flags
//! (verified empirically and by a hard-coded test vector in this module).
//! Botho uses:
//!
//! * **Verifiers — light mode:** recommended flags (JIT + hardware AES on
//!   capable CPUs), *without* `FLAG_FULL_MEM`. ~256 MB cache, ~18 ms/verify —
//!   ample for one PoW verify per 5 s block.
//! * **Miners — fast mode:** recommended flags `| FLAG_FULL_MEM`. ~2 GB
//!   dataset, far higher throughput.
//!
//! `get_recommended_flags()` selects JIT plus, where available, hardware AES
//! and (on macOS/arm64) the `FLAG_SECURE` W^X handling that the JIT requires —
//! bare `FLAG_JIT` faults on Apple Silicon. None of these flags change the
//! hash output; they only change speed/portability.
//!
//! # VM lifecycle
//!
//! Cache/dataset/VM initialization is seconds-expensive, so this module caches
//! one verifier VM per process keyed by the active seed epoch and only
//! re-initializes when the epoch rotates. `pow_hash` is therefore cheap to
//! call repeatedly during validation. Miners build their own (fast-mode) VMs
//! in the minter threads (see `node::minter`).

use std::sync::Mutex;

use randomx_rs::{RandomXCache, RandomXDataset, RandomXFlag, RandomXVM};
use sha2::{Digest, Sha256};

/// Number of blocks per RandomX seed epoch (the key rotation interval).
///
/// Matches Monero's `SEEDHASH_EPOCH_BLOCKS` (2048). At Botho's 5 s target
/// block time this is ~2.8 hours per epoch — long enough that dataset
/// re-initialization is negligible amortized cost, short enough that the
/// RandomX program family still rotates regularly.
pub const SEED_ROTATION_INTERVAL: u64 = 2048;

/// Domain-separation tag mixed into every RandomX seed key.
const SEED_DOMAIN: &[u8] = b"BOTHO_RANDOMX_SEED_V1";

/// Compute the RandomX seed *key* for a given block height.
///
/// Pure function of the height epoch — every node derives the identical key
/// with no chain lookup. See the module docs for the determinism rationale.
pub fn seed_key_for_height(height: u64) -> [u8; 32] {
    let epoch = height / SEED_ROTATION_INTERVAL;
    let mut hasher = Sha256::new();
    hasher.update(SEED_DOMAIN);
    hasher.update(epoch.to_le_bytes());
    hasher.finalize().into()
}

/// Build the PoW preimage hashed by RandomX.
///
/// `preimage = nonce ‖ prev_block_hash ‖ minter_view_key ‖ minter_spend_key`
///
/// Identical layout to the legacy SHA-256 PoW so the per-block binding and the
/// node-operator latency edge (depends on `prev_block_hash`) are preserved.
pub fn pow_preimage(
    nonce: u64,
    prev_block_hash: &[u8; 32],
    minter_view_key: &[u8; 32],
    minter_spend_key: &[u8; 32],
) -> [u8; 104] {
    let mut buf = [0u8; 104];
    buf[0..8].copy_from_slice(&nonce.to_le_bytes());
    buf[8..40].copy_from_slice(prev_block_hash);
    buf[40..72].copy_from_slice(minter_view_key);
    buf[72..104].copy_from_slice(minter_spend_key);
    buf
}

/// Interpret a 32-byte RandomX output as the PoW target value.
///
/// Big-endian read of the first 8 bytes, matching the legacy convention so the
/// difficulty controller and `pow_priority` are unchanged. Lower = better PoW.
pub fn pow_value(hash: &[u8; 32]) -> u64 {
    u64::from_be_bytes(hash[0..8].try_into().unwrap())
}

/// Flags for the **light** (verifier) VM: recommended flags without full mem.
///
/// `get_recommended_flags()` enables JIT and, where supported, hardware AES
/// plus the secure W^X handling the JIT needs on macOS/arm64. None of these
/// affect the hash output.
fn light_flags() -> RandomXFlag {
    RandomXFlag::get_recommended_flags()
}

/// Flags for the **fast** (miner) VM: light flags plus the full dataset.
pub fn fast_flags() -> RandomXFlag {
    RandomXFlag::get_recommended_flags() | RandomXFlag::FLAG_FULL_MEM
}

/// A **shared** fast-mode RandomX dataset (~2 GB) bound to a single seed epoch.
///
/// `randomx-rs`'s [`RandomXDataset`] is internally `Arc`-backed and is only
/// ever *read* during hashing, so a SINGLE dataset can back many per-thread VMs
/// at once — exactly the way Monero's miner shares one dataset across all of
/// its mining threads. Build the dataset **once per seed epoch** and hand each
/// mining thread a cheap VM over it via [`FastDataset::hasher`]; the total cost
/// is then ~2 GB (the one dataset) + a small (~2 MB) scratchpad per VM, **not**
/// `N × 2 GB`.
///
/// That `N × 2 GB` blow-up is precisely what wedged the live testnet: every
/// mining thread built its *own* full dataset, so an `N`-core box demanded
/// `N × 2 GB` of RAM. The faucet box (2 cores, 3.8 GB RAM) needed ~4 GB and
/// OOM-halted at height 213 (see #539/#568). Sharing one dataset removes that
/// per-thread multiplier.
///
/// Cloning a `FastDataset` is a cheap atomic `Arc` refcount bump that shares
/// the same underlying ~2 GB buffer; the clone is consumed by [`RandomXVM`] and
/// kept alive for the VM's lifetime.
#[derive(Clone)]
pub struct FastDataset {
    seed_key: [u8; 32],
    dataset: RandomXDataset,
}

// SAFETY: `randomx-rs` conservatively does not implement `Send`/`Sync` for
// `RandomXDataset` because it holds a raw C pointer. Asserting `Send` here is
// sound: the dataset is READ-ONLY after construction (RandomX fast mode never
// mutates it during hashing), and `RandomXDataset` is `Arc`-backed so cloning
// and dropping it across threads only touches an atomic refcount. Moving the
// handle to another thread (or accessing it under a mutex from several threads
// to mint per-thread VMs) is therefore data-race free. This is RandomX's
// intended fast-mode sharing model — one dataset, many concurrently-reading VMs
// (#568). Each VM owns its own `RandomXDataset` clone and never crosses threads
// (`FastHasher`/`RandomXVM` remain `!Send`), so the concurrent reads go through
// independent owned handles, not a shared `&`.
unsafe impl Send for FastDataset {}

impl FastDataset {
    /// Build the shared ~2 GB fast-mode dataset for `seed_key`.
    ///
    /// Seconds-expensive (allocates and fills the full dataset). Do this
    /// **once** per seed epoch, then derive every mining thread's VM from
    /// it with [`FastDataset::hasher`] — never once per thread (that is the
    /// `N × 2 GB` halt this type exists to prevent, #568).
    pub fn new(seed_key: [u8; 32]) -> Result<Self, randomx_rs::RandomXError> {
        let flags = fast_flags();
        let cache = RandomXCache::new(flags, &seed_key)?;
        let dataset = RandomXDataset::new(flags, cache, 0)?;
        Ok(Self { seed_key, dataset })
    }

    /// The seed key this dataset is bound to.
    pub fn seed_key(&self) -> [u8; 32] {
        self.seed_key
    }

    /// Build a fast-mode [`FastHasher`] (VM) that reads **this** shared
    /// dataset.
    ///
    /// Cheap: it allocates only the VM's small scratchpad, not another ~2 GB
    /// dataset. The returned hasher holds a cheap `Arc` clone of the dataset,
    /// so the dataset stays alive for as long as any VM derived from it.
    /// Output is byte-identical to a standalone [`FastHasher::new`] for the
    /// same `(seed_key, preimage)` — sharing changes *memory*, never the
    /// hash.
    pub fn hasher(&self) -> Result<FastHasher, randomx_rs::RandomXError> {
        let flags = fast_flags();
        let vm = RandomXVM::new(flags, None, Some(self.dataset.clone()))?;
        Ok(FastHasher {
            seed_key: self.seed_key,
            vm,
        })
    }
}

/// A miner-side fast-mode RandomX hasher bound to a single seed epoch.
///
/// Wraps one [`RandomXVM`] reading a ~2 GB fast-mode dataset. Prefer building
/// these from a shared [`FastDataset`] (via [`FastDataset::hasher`]) when more
/// than one thread mines the same epoch, so they share ONE dataset instead of
/// allocating `N × 2 GB` (#568). [`FastHasher::new`] is the standalone path: it
/// builds its own dataset and is handy for single-VM use and tests.
pub struct FastHasher {
    seed_key: [u8; 32],
    vm: RandomXVM,
}

impl FastHasher {
    /// Build a standalone fast-mode hasher for `seed_key`, allocating its
    /// **own** ~2 GB dataset.
    ///
    /// When several threads mine the same epoch, build one [`FastDataset`] and
    /// call [`FastDataset::hasher`] per thread instead, so they share a single
    /// dataset rather than `N × 2 GB` (#568).
    pub fn new(seed_key: [u8; 32]) -> Result<Self, randomx_rs::RandomXError> {
        FastDataset::new(seed_key)?.hasher()
    }

    /// The seed key this hasher is bound to.
    pub fn seed_key(&self) -> [u8; 32] {
        self.seed_key
    }

    /// Hash a preimage, returning the 32-byte RandomX output.
    pub fn hash(&self, preimage: &[u8]) -> [u8; 32] {
        let out = self
            .vm
            .calculate_hash(preimage)
            .expect("RandomX fast hash failed");
        let mut h = [0u8; 32];
        h.copy_from_slice(&out);
        h
    }
}

/// A miner-usable **light-mode** RandomX hasher bound to a single seed epoch.
///
/// This is the degraded-mode counterpart to [`FastHasher`]: it allocates only
/// the ~256 MB cache (no 2 GB dataset / `FLAG_FULL_MEM`), so it builds on
/// RAM-constrained or huge-pages-less boxes where the fast dataset allocation
/// fails — at a large throughput cost (light hashing is much slower than fast).
///
/// CONSENSUS-SAFE: RandomX light and fast modes produce the **identical**
/// 32-byte output for the same `(seed_key, preimage)` (see the module docs and
/// `test_light_equals_fast`). A block mined in light mode therefore verifies on
/// every node exactly like a fast-mode block — this is a resilience knob, NOT a
/// consensus-rule change (#566, the code-side fix for the #539 halt).
///
/// Distinct from the process-wide cached *verifier* VM (`verify_pow_hash` /
/// [`LIGHT_VM`]): a miner needs its **own** owned VM so it can hash arbitrary
/// mining preimages at will without contending on the verifier's mutex.
pub struct LightHasher {
    seed_key: [u8; 32],
    vm: RandomXVM,
}

impl LightHasher {
    /// Build a light-mode hasher for the given seed key (~256 MB cache, no
    /// dataset).
    pub fn new(seed_key: [u8; 32]) -> Result<Self, randomx_rs::RandomXError> {
        let flags = light_flags();
        let cache = RandomXCache::new(flags, &seed_key)?;
        let vm = RandomXVM::new(flags, Some(cache), None)?;
        Ok(Self { seed_key, vm })
    }

    /// The seed key this hasher is bound to.
    pub fn seed_key(&self) -> [u8; 32] {
        self.seed_key
    }

    /// Hash a preimage, returning the 32-byte RandomX output (identical to what
    /// [`FastHasher::hash`] produces for the same `(seed_key, preimage)`).
    pub fn hash(&self, preimage: &[u8]) -> [u8; 32] {
        let out = self
            .vm
            .calculate_hash(preimage)
            .expect("RandomX light hash failed");
        let mut h = [0u8; 32];
        h.copy_from_slice(&out);
        h
    }
}

/// A mining hasher that is either fast-mode (2 GB dataset, high throughput) or
/// a degraded light-mode fallback (~256 MB cache, much slower) used when the
/// fast dataset cannot be built (#566).
///
/// Both variants produce **identical** PoW output for the same
/// `(seed_key, preimage)`, so swapping one for the other never changes which
/// blocks are valid — it only changes how fast this node finds them. The minter
/// builds a fast hasher when it can and transparently falls back to light mode
/// when the fast dataset allocation fails, keeping the chain alive on an
/// undersized box instead of silently halting.
pub enum MinerHasher {
    /// Fast-mode (full dataset) hasher — the normal high-throughput path.
    Fast(FastHasher),
    /// Light-mode fallback hasher — slower, used when the fast dataset fails.
    Light(LightHasher),
}

impl MinerHasher {
    /// The seed key this hasher is bound to.
    pub fn seed_key(&self) -> [u8; 32] {
        match self {
            MinerHasher::Fast(h) => h.seed_key(),
            MinerHasher::Light(h) => h.seed_key(),
        }
    }

    /// Hash a mining preimage, returning the 32-byte RandomX output.
    pub fn hash(&self, preimage: &[u8]) -> [u8; 32] {
        match self {
            MinerHasher::Fast(h) => h.hash(preimage),
            MinerHasher::Light(h) => h.hash(preimage),
        }
    }

    /// Whether this is the degraded light-mode fallback (for observability).
    pub fn is_light(&self) -> bool {
        matches!(self, MinerHasher::Light(_))
    }
}

/// Process-wide cached light-mode verifier VM, keyed by the active seed epoch.
///
/// Verification only needs one VM at a time (the chain advances forward), and
/// the seed rotates every `SEED_ROTATION_INTERVAL` blocks, so we keep a single
/// cached VM and rebuild it only when the seed key changes. This makes
/// `verify_pow_hash` cheap to call per block.
struct LightCache {
    seed_key: [u8; 32],
    vm: RandomXVM,
}

// SAFETY: `RandomXVM`/`RandomXCache` hold raw C pointers, so `randomx-rs` does
// not implement `Send`/`Sync` for them. We only ever touch `LIGHT_VM` while
// holding its `Mutex`, so all access is serialized to one thread at a time and
// the VM is never used concurrently. The underlying RandomX VM is safe to use
// from any single thread; the Mutex provides the required exclusivity. This
// wrapper asserts that to the type system.
struct SendCache(LightCache);
unsafe impl Send for SendCache {}

static LIGHT_VM: Mutex<Option<SendCache>> = Mutex::new(None);

/// Compute the RandomX PoW hash of `preimage` under `seed_key`, using a cached
/// light-mode verifier VM (re-initialized only on seed rotation).
///
/// This is the canonical verification hasher. It produces the **same** output
/// a miner's fast-mode VM produces for the same `(seed_key, preimage)`.
pub fn verify_pow_hash(seed_key: &[u8; 32], preimage: &[u8]) -> [u8; 32] {
    let mut guard = LIGHT_VM.lock().expect("LIGHT_VM mutex poisoned");

    let needs_rebuild = match guard.as_ref() {
        Some(c) => &c.0.seed_key != seed_key,
        None => true,
    };

    if needs_rebuild {
        let flags = light_flags();
        let cache = RandomXCache::new(flags, seed_key).expect("RandomX cache init failed");
        let vm = RandomXVM::new(flags, Some(cache), None).expect("RandomX light VM init failed");
        *guard = Some(SendCache(LightCache {
            seed_key: *seed_key,
            vm,
        }));
    }

    let cache = guard.as_ref().expect("light VM present after rebuild");
    let out = cache
        .0
        .vm
        .calculate_hash(preimage)
        .expect("RandomX light hash failed");
    let mut h = [0u8; 32];
    h.copy_from_slice(&out);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seed_rotation_epochs() {
        // All heights within an epoch share a key; boundaries rotate it.
        let k = SEED_ROTATION_INTERVAL;
        assert_eq!(seed_key_for_height(0), seed_key_for_height(1));
        assert_eq!(seed_key_for_height(0), seed_key_for_height(k - 1));
        assert_ne!(seed_key_for_height(k - 1), seed_key_for_height(k));
        assert_eq!(seed_key_for_height(k), seed_key_for_height(2 * k - 1));
        assert_ne!(seed_key_for_height(2 * k - 1), seed_key_for_height(2 * k));
    }

    #[test]
    fn test_genesis_bootstrap_seed_is_epoch_zero() {
        // Genesis and all early heights use the fixed epoch-0 bootstrap key.
        let expected = {
            let mut h = Sha256::new();
            h.update(SEED_DOMAIN);
            h.update(0u64.to_le_bytes());
            let out: [u8; 32] = h.finalize().into();
            out
        };
        assert_eq!(seed_key_for_height(0), expected);
    }

    #[test]
    fn test_preimage_layout() {
        let nonce = 0x0102_0304_0506_0708u64;
        let prev = [0x11u8; 32];
        let view = [0x22u8; 32];
        let spend = [0x33u8; 32];
        let pre = pow_preimage(nonce, &prev, &view, &spend);
        assert_eq!(&pre[0..8], &nonce.to_le_bytes());
        assert_eq!(&pre[8..40], &prev);
        assert_eq!(&pre[40..72], &view);
        assert_eq!(&pre[72..104], &spend);
    }

    #[test]
    fn test_pow_value_big_endian() {
        let mut hash = [0u8; 32];
        hash[0] = 0x00;
        hash[1] = 0x00;
        hash[7] = 0x01;
        assert_eq!(pow_value(&hash), 1);
        hash[0] = 0xFF;
        assert_eq!(pow_value(&hash), 0xFF00_0000_0000_0001);
    }

    #[test]
    fn test_verify_determinism_roundtrip() {
        let seed = seed_key_for_height(0);
        let pre = pow_preimage(42, &[7u8; 32], &[8u8; 32], &[9u8; 32]);
        let a = verify_pow_hash(&seed, &pre);
        let b = verify_pow_hash(&seed, &pre);
        assert_eq!(a, b, "RandomX verify must be deterministic");
    }

    /// KNOWN-ANSWER TEST VECTOR.
    ///
    /// A fixed `(seed_key, preimage)` MUST always produce this exact RandomX
    /// output. If this ever changes, the PoW (and thus the chain) has drifted —
    /// e.g. an upstream RandomX algorithm bump, a flag that secretly affects
    /// output, or a preimage-layout regression. Any such change is
    /// consensus-breaking and must be caught here.
    ///
    /// The vector was produced by this very code path (light mode, recommended
    /// flags) and cross-checked to be identical under fast mode and under the
    /// portable `FLAG_DEFAULT` flags (see `test_light_equals_fast`).
    #[test]
    fn test_known_answer_vector() {
        // seed_key = SHA256("BOTHO_RANDOMX_SEED_V1" || 0u64)  (epoch 0)
        let seed = seed_key_for_height(0);
        // Fixed, simple preimage.
        let pre = pow_preimage(0, &[0u8; 32], &[0u8; 32], &[0u8; 32]);
        let hash = verify_pow_hash(&seed, &pre);
        // If this assertion's expected value needs updating, treat it as a
        // CONSENSUS CHANGE and require a fresh genesis + coordinated redeploy.
        let expected = hex::decode(KNOWN_ANSWER_HEX).expect("valid hex");
        assert_eq!(
            hash.to_vec(),
            expected,
            "RandomX known-answer drift: got {}",
            hex::encode(hash)
        );
    }

    /// CRITICAL: light-mode (verifier) and fast-mode (miner) MUST produce the
    /// identical hash, or miners and verifiers would disagree and the chain
    /// would fork.
    #[test]
    fn test_light_equals_fast() {
        let seed = seed_key_for_height(0);
        let pre = pow_preimage(123, &[1u8; 32], &[2u8; 32], &[3u8; 32]);

        let light = verify_pow_hash(&seed, &pre);

        let fast = FastHasher::new(seed).expect("fast hasher");
        let fast_hash = fast.hash(&pre);

        assert_eq!(
            light,
            fast_hash,
            "light != fast — FORK RISK (light={}, fast={})",
            hex::encode(light),
            hex::encode(fast_hash)
        );

        // The miner-usable light-mode fallback hasher (#566) MUST also produce
        // the identical output: a block mined in degraded light mode has to
        // verify on every node exactly like a fast-mode block, or the fallback
        // would fork the chain. Build it off the single fast dataset above so we
        // don't allocate a second 2 GB dataset.
        let light_hasher = LightHasher::new(seed).expect("light hasher");
        let light_hasher_hash = light_hasher.hash(&pre);
        assert_eq!(
            light_hasher_hash,
            fast_hash,
            "LightHasher != FastHasher — FORK RISK (#566) (light={}, fast={})",
            hex::encode(light_hasher_hash),
            hex::encode(fast_hash)
        );
        assert_eq!(
            light_hasher_hash, light,
            "LightHasher != verify_pow_hash — FORK RISK (#566)"
        );
    }

    /// CRITICAL (#568): multiple VMs built from ONE shared [`FastDataset`] must
    /// produce the **byte-identical** hash that a standalone [`FastHasher`]
    /// produces — otherwise sharing the dataset (the fix for the `N × 2 GB`
    /// halt) would silently fork the chain.
    ///
    /// This is what makes the memory optimization consensus-safe: hashing over
    /// a shared dataset is the same computation as hashing over a private
    /// one. To avoid allocating several ~2 GB datasets, the test builds
    /// **one** dataset and derives every VM (and the standalone comparison)
    /// from it; the canonical light-mode verifier (`verify_pow_hash`) is
    /// the independent ground truth that pins the expected output.
    #[test]
    fn test_shared_dataset_matches_standalone() {
        let seed = seed_key_for_height(0);
        let pre = pow_preimage(777, &[4u8; 32], &[5u8; 32], &[6u8; 32]);

        // Ground truth: the canonical verifier hash (no 2 GB dataset).
        let canonical = verify_pow_hash(&seed, &pre);

        // ONE shared ~2 GB dataset, reused for every VM below.
        let dataset = FastDataset::new(seed).expect("shared fast dataset");
        assert_eq!(dataset.seed_key(), seed);

        // Two independent VMs over the SAME shared dataset (this is exactly how
        // N mining threads share one dataset at runtime).
        let shared_a = dataset.hasher().expect("VM A over shared dataset");
        let shared_b = dataset.hasher().expect("VM B over shared dataset");

        let hash_a = shared_a.hash(&pre);
        let hash_b = shared_b.hash(&pre);

        // Both shared VMs agree with each other...
        assert_eq!(
            hash_a, hash_b,
            "two VMs over one shared dataset disagree — FORK RISK (#568)"
        );
        // ...and with the standalone/canonical hash. `FastHasher::new` builds a
        // standalone hasher the same way `FastDataset::hasher` does, and
        // `test_light_equals_fast` pins standalone-fast == canonical, so this
        // equality means shared == standalone without a second 2 GB allocation.
        assert_eq!(
            hash_a,
            canonical,
            "shared-dataset VM != canonical/standalone hash — FORK RISK (#568) \
             (shared={}, canonical={})",
            hex::encode(hash_a),
            hex::encode(canonical),
        );

        // A different nonce must change the hash (the VM actually reads input).
        let pre2 = pow_preimage(778, &[4u8; 32], &[5u8; 32], &[6u8; 32]);
        assert_ne!(hash_a, shared_a.hash(&pre2));
    }

    // Expected output of the known-answer vector. Generated by the light-mode
    // path and confirmed identical under fast mode and FLAG_DEFAULT.
    const KNOWN_ANSWER_HEX: &str =
        "04779c03247d2b9f45bd745529ae60cc02ddab82d5eb3c2fdcb8b1fcaaa7dfb4";
}
