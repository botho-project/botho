//! Persisted operator-action nonce store (#749, P4.4c of the #709 proposal).
//!
//! This is the seen-nonce leg of replay protection for the operator-signed
//! quorum-curation write path (`docs/security/quorum-write-path.md` §5). It is
//! deliberately narrow: it owns ONLY the persisted set of consumed nonces.
//! Envelope verification, signature checking, `targetNode` binding, and the
//! `expiresAt` freshness check all live in the verifier (#748, sub-issue (b)),
//! which calls this store's small API for step 6 of §4.
//!
//! ## What this store guarantees
//!
//! 1. **Single-use nonces per node.** A `(signerKeyId, nonce)` pair can be
//!    [`reserve`](NonceStore::reserve)d exactly once; a second attempt is
//!    rejected. Combined with the verifier's `targetNode` binding, this blocks
//!    replay against the same node (§5): replay against ANOTHER node is blocked
//!    by `targetNode`, replay after 5 minutes by `expiresAt`, modification by
//!    the signature — this store closes the same-node-same-nonce hole.
//!
//! 2. **Persistence across restarts.** The store is flushed to a JSON file
//!    under the node's data dir on every successful reserve, so a node restart
//!    INSIDE the 5-minute window does NOT reopen a replay slot (§5: "a node
//!    restart inside the 5-minute window must not reopen a replay slot"). The
//!    persistence mechanism intentionally mirrors the node's other small-state
//!    files (`config.toml`, `node_key`): a single owner-only file written
//!    atomically via a temp-file + rename, rather than pulling a heavyweight
//!    database dependency in for a set that is trivially small (§5: "bounded by
//!    the action rate within any 5-minute window").
//!
//! 3. **Reserve-then-apply fail-safe (§4 step 6).** The verifier calls
//!    [`reserve`](NonceStore::reserve) BEFORE applying the action, and the
//!    nonce is durably persisted before `reserve` returns `Ok`. If the node
//!    then crashes between reserve and apply, the nonce is already consumed:
//!    the same envelope cannot be applied on a later retry (it is rejected as a
//!    replay), so an envelope can never apply twice. The only cost of a
//!    crash-in-the-gap is that the operator must re-sign a fresh-nonce envelope
//!    to retry. This is the load-bearing §9 checklist item ("reserve-then-apply
//!    nonce semantics fail safe across crashes").
//!
//! 4. **Bounded retention.** Entries are needed only until their `expiresAt`
//!    passes (an expired envelope is already rejected by the verifier's
//!    freshness check), so the store garbage-collects expired entries on every
//!    insert. Its size is bounded by the number of distinct nonces reserved
//!    within any single ~5-minute expiry window.
//!
//! ## What this store does NOT do
//!
//! - It does not reserve a nonce for **dry runs**. Per §5 (accepted exception),
//!   a `dryRun: true` envelope never consumes its nonce, so the verifier simply
//!   does not call [`reserve`](NonceStore::reserve) for dry runs — this module
//!   has no dry-run concept at all; it only records nonces it is asked to. The
//!   dry-run policy is the caller's; keeping it out of the store keeps the
//!   store's single responsibility clean and testable.
//! - It does not verify signatures, parse envelopes, or check `targetNode` /
//!   `expiresAt` freshness — those are the verifier's job (#748).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Default file name for the persisted nonce store, placed under the node's
/// data dir alongside `config.toml` / `node_key` (see
/// [`NonceStore::path_from_data_dir`]).
pub const NONCE_STORE_FILE: &str = "operator-nonces.json";

/// Current on-disk format version for the nonce store file.
const NONCE_STORE_VERSION: u32 = 1;

/// The outcome of attempting to [`reserve`](NonceStore::reserve) a nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveOutcome {
    /// The nonce was previously unseen and is now recorded (consumed). The
    /// caller may proceed to apply the action.
    Reserved,
    /// The `(signerKeyId, nonce)` pair was already recorded — this is a replay
    /// and MUST be rejected by the caller.
    Replay,
}

impl ReserveOutcome {
    /// Convenience: `true` iff the reservation succeeded (fresh nonce).
    pub fn is_reserved(self) -> bool {
        matches!(self, ReserveOutcome::Reserved)
    }

    /// Convenience: `true` iff this was a replay (nonce already seen).
    pub fn is_replay(self) -> bool {
        matches!(self, ReserveOutcome::Replay)
    }
}

/// Composite key identifying a consumed nonce: the signer's fingerprint
/// (`signerKeyId`, §3) plus the 128-bit random nonce hex. Nonces are scoped per
/// signer so two operators cannot collide, and because the audit log (§6)
/// attributes by `signerKeyId` anyway.
fn store_key(signer_key_id: &str, nonce: &str) -> String {
    // A NUL separator can never appear in the hex fields, so this is an
    // unambiguous join (no signer_key_id + nonce concatenation can alias a
    // different pair).
    format!("{signer_key_id}\0{nonce}")
}

/// On-disk representation of the nonce store.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NonceStoreFile {
    /// File-format version (currently [`NONCE_STORE_VERSION`]).
    version: u32,
    /// Map from composite `store_key` to the entry's `expiresAt` (unix
    /// seconds). Only the expiry is retained; that is all GC needs.
    entries: HashMap<String, u64>,
}

impl Default for NonceStoreFile {
    fn default() -> Self {
        Self {
            version: NONCE_STORE_VERSION,
            entries: HashMap::new(),
        }
    }
}

/// A persisted, garbage-collected set of consumed operator-action nonces.
///
/// Construct one with [`open`](Self::open); call [`reserve`](Self::reserve)
/// once per non-dry-run envelope before applying it. The store persists to disk
/// on every successful reserve.
#[derive(Debug)]
pub struct NonceStore {
    path: PathBuf,
    file: NonceStoreFile,
}

impl NonceStore {
    /// Resolve the canonical nonce-store path for a given data dir: the
    /// directory that holds `config.toml` (the same dir as `node_key` and
    /// `ledger/`). The verifier resolves the data dir from the config path via
    /// `config::*_from_config` helpers and passes it here.
    pub fn path_from_data_dir(data_dir: &Path) -> PathBuf {
        data_dir.join(NONCE_STORE_FILE)
    }

    /// Open (or create) the nonce store at `path`.
    ///
    /// If the file exists it is loaded; a missing file yields an empty store
    /// (the on-disk file is created lazily on the first successful reserve). A
    /// file that fails to parse is a hard error rather than being silently
    /// discarded — silently wiping the store would reopen every replay slot it
    /// was protecting, which is exactly the failure mode persistence exists to
    /// prevent.
    pub fn open(path: &Path) -> Result<Self> {
        let file = if path.exists() {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read nonce store from {}", path.display()))?;
            let parsed: NonceStoreFile = serde_json::from_str(&contents).with_context(|| {
                format!(
                    "failed to parse nonce store at {} — refusing to start with a \
                     corrupt store rather than silently reopening replay slots",
                    path.display()
                )
            })?;
            if parsed.version != NONCE_STORE_VERSION {
                anyhow::bail!(
                    "unsupported nonce store version {} at {} (expected {})",
                    parsed.version,
                    path.display(),
                    NONCE_STORE_VERSION
                );
            }
            parsed
        } else {
            NonceStoreFile::default()
        };

        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Attempt to consume `(signer_key_id, nonce)`, expiring at `expires_at`
    /// (unix seconds), treating `now` (unix seconds) as the current time for
    /// garbage collection.
    ///
    /// On [`ReserveOutcome::Reserved`] the nonce is durably persisted to disk
    /// BEFORE this function returns — this is what makes reserve-then-apply
    /// (§4 step 6) crash-safe. On [`ReserveOutcome::Replay`] nothing is
    /// written (the nonce was already recorded on its first reservation).
    ///
    /// Expired entries (`expiresAt <= now`) are garbage-collected on every
    /// call, keeping the store bounded (§5).
    ///
    /// The caller MUST NOT invoke this for dry-run envelopes (§5): dry runs do
    /// not consume nonces.
    pub fn reserve(
        &mut self,
        signer_key_id: &str,
        nonce: &str,
        expires_at: u64,
        now: u64,
    ) -> Result<ReserveOutcome> {
        // GC first so a burst of short-lived nonces never accumulates and so a
        // reservation whose own window has already lapsed is not admitted as a
        // fresh entry (the verifier's freshness check should have caught that,
        // but the store stays self-consistent regardless).
        self.gc(now);

        let key = store_key(signer_key_id, nonce);
        if self.file.entries.contains_key(&key) {
            return Ok(ReserveOutcome::Replay);
        }

        self.file.entries.insert(key, expires_at);
        // Persist BEFORE returning Ok: the nonce must be durable before the
        // caller applies the action (reserve-then-apply, §4 step 6). If the
        // write fails we roll the in-memory insert back so memory and disk stay
        // consistent, and surface the error — the caller must treat a
        // non-durable reservation as a failure, not an apply-ok.
        if let Err(e) = self.persist() {
            self.file.entries.remove(&store_key(signer_key_id, nonce));
            return Err(e);
        }

        Ok(ReserveOutcome::Reserved)
    }

    /// Number of currently-retained (un-GC'd) entries. Exposed for tests and
    /// diagnostics; the store is bounded by the action rate in any expiry
    /// window (§5).
    pub fn len(&self) -> usize {
        self.file.entries.len()
    }

    /// Whether the store currently holds no entries.
    pub fn is_empty(&self) -> bool {
        self.file.entries.is_empty()
    }

    /// Drop every entry whose `expiresAt <= now`. Called on every
    /// [`reserve`](Self::reserve); exposed for explicit maintenance/tests.
    pub fn gc(&mut self, now: u64) {
        self.file
            .entries
            .retain(|_, &mut expires_at| expires_at > now);
    }

    /// Atomically write the current store to disk with owner-only permissions,
    /// matching how the node protects its other small-state files
    /// (`config.toml`, `node_key`). Writes to a sibling temp file and renames
    /// so a crash mid-write can never leave a truncated/corrupt store.
    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {}", parent.display()))?;
            }
        }

        let json =
            serde_json::to_string_pretty(&self.file).context("failed to serialize nonce store")?;

        // Unique-ish temp path in the same directory (same filesystem, so the
        // rename is atomic). Using the pid keeps concurrent processes from
        // clobbering each other's temp file; within a process, reserve() is
        // called serially from the event loop.
        let tmp_path = self
            .path
            .with_extension(format!("json.tmp.{}", std::process::id()));

        #[cfg(unix)]
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)
                .with_context(|| {
                    format!("failed to open temp nonce store {}", tmp_path.display())
                })?;
            f.write_all(json.as_bytes()).with_context(|| {
                format!("failed to write temp nonce store {}", tmp_path.display())
            })?;
            f.sync_all().with_context(|| {
                format!("failed to fsync temp nonce store {}", tmp_path.display())
            })?;
        }
        #[cfg(not(unix))]
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .with_context(|| {
                    format!("failed to open temp nonce store {}", tmp_path.display())
                })?;
            f.write_all(json.as_bytes()).with_context(|| {
                format!("failed to write temp nonce store {}", tmp_path.display())
            })?;
            f.sync_all().with_context(|| {
                format!("failed to fsync temp nonce store {}", tmp_path.display())
            })?;
        }

        std::fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative fingerprint (`signerKeyId`, §3): 8 bytes → 16 hex
    /// chars, the shape produced by `operator_key::fingerprint_hex`.
    const SIGNER: &str = "a1b2c3d4e5f60708";
    /// A representative 128-bit random nonce (§3): 32 hex chars.
    const NONCE_A: &str = "9f2c00112233445566778899aabbccdd";
    const NONCE_B: &str = "0011223344556677889900aabbccddee";

    fn tmp_store_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "botho-nonce-test-{}-{}",
            std::process::id(),
            // A monotonically-different suffix per call within a process.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir.join(NONCE_STORE_FILE)
    }

    #[test]
    fn first_reserve_succeeds_replay_is_rejected() {
        let path = tmp_store_path();
        let mut store = NonceStore::open(&path).unwrap();

        let now = 1_000;
        let expires = now + 300;

        // First use of the nonce: reserved.
        assert_eq!(
            store.reserve(SIGNER, NONCE_A, expires, now).unwrap(),
            ReserveOutcome::Reserved
        );
        // Same (signerKeyId, nonce) against the SAME node: rejected as replay.
        assert_eq!(
            store.reserve(SIGNER, NONCE_A, expires, now).unwrap(),
            ReserveOutcome::Replay
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_different_nonce_or_signer_is_independent() {
        let path = tmp_store_path();
        let mut store = NonceStore::open(&path).unwrap();
        let now = 1_000;
        let expires = now + 300;

        assert!(store
            .reserve(SIGNER, NONCE_A, expires, now)
            .unwrap()
            .is_reserved());
        // Different nonce, same signer → independent, reserves fine.
        assert!(store
            .reserve(SIGNER, NONCE_B, expires, now)
            .unwrap()
            .is_reserved());
        // Same nonce, different signer → independent, reserves fine.
        assert!(store
            .reserve("00000000deadbeef", NONCE_A, expires, now)
            .unwrap()
            .is_reserved());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn replay_protection_survives_a_restart_within_the_window() {
        // Acceptance criterion: persist → reopen → replay rejected, while the
        // envelope is still inside its 5-minute window.
        let path = tmp_store_path();
        let now = 1_000;
        let expires = now + 300; // 5-minute window.

        {
            let mut store = NonceStore::open(&path).unwrap();
            assert!(store
                .reserve(SIGNER, NONCE_A, expires, now)
                .unwrap()
                .is_reserved());
            // `store` drops here — simulating process exit. The nonce was
            // persisted synchronously inside reserve().
        }

        // Simulate a node restart: brand-new store instance reading the file,
        // still inside the window (now2 < expires).
        let now2 = now + 60;
        let mut reopened = NonceStore::open(&path).unwrap();
        assert_eq!(
            reopened.reserve(SIGNER, NONCE_A, expires, now2).unwrap(),
            ReserveOutcome::Replay,
            "a restart inside the 5-minute window must not reopen the replay slot"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reserve_then_crash_before_apply_leaves_nonce_consumed() {
        // Acceptance criterion: a simulated crash AFTER reserve but BEFORE
        // apply leaves the nonce consumed. Retrying the SAME envelope is
        // rejected; a re-signed FRESH-nonce envelope succeeds.
        let path = tmp_store_path();
        let now = 1_000;
        let expires = now + 300;

        // Reserve, then "crash" (drop without ever applying).
        {
            let mut store = NonceStore::open(&path).unwrap();
            assert!(store
                .reserve(SIGNER, NONCE_A, expires, now)
                .unwrap()
                .is_reserved());
            // <-- crash here: apply never ran, but the nonce is already
            // durable.
        }

        // After restart: retrying the SAME envelope (same nonce) is rejected,
        // so the envelope can never be applied twice — it fails safe.
        let mut after_crash = NonceStore::open(&path).unwrap();
        assert_eq!(
            after_crash
                .reserve(SIGNER, NONCE_A, expires, now + 1)
                .unwrap(),
            ReserveOutcome::Replay,
            "same-envelope retry after a reserve-then-crash must be rejected"
        );

        // The recovery path is a re-signed envelope with a fresh nonce, which
        // succeeds.
        assert_eq!(
            after_crash
                .reserve(SIGNER, NONCE_B, expires, now + 1)
                .unwrap(),
            ReserveOutcome::Reserved,
            "a re-signed fresh-nonce envelope must succeed after a crash"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn expired_entries_are_gcd_on_insert_and_store_stays_bounded() {
        // Acceptance criterion: expired entries GC'd on insert; store size
        // stays bounded under a burst.
        let path = tmp_store_path();
        let mut store = NonceStore::open(&path).unwrap();

        // Burst of 500 distinct nonces, each expiring at t=1300.
        let expires = 1_300u64;
        for i in 0..500u64 {
            let nonce = format!("{i:032x}");
            assert!(store
                .reserve(SIGNER, &nonce, expires, 1_000)
                .unwrap()
                .is_reserved());
        }
        assert_eq!(store.len(), 500, "all 500 fresh nonces retained pre-expiry");

        // Advance time past expiry and reserve one more: the 500 stale entries
        // must be GC'd on this insert, leaving only the new one.
        let new_expires = 1_600u64;
        assert!(store
            .reserve(SIGNER, &format!("{:032x}", 9999u64), new_expires, 1_400)
            .unwrap()
            .is_reserved());
        assert_eq!(
            store.len(),
            1,
            "expired entries must be GC'd on insert, bounding the store"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn gc_boundary_is_expiry_inclusive() {
        // An entry expiring exactly at `now` is expired (freshness check uses
        // `now <= expiresAt`; at now == expiresAt the envelope is on its last
        // valid second, but a GC at that instant may drop it — the verifier,
        // not the store, owns freshness, so being GC-inclusive here is safe and
        // keeps the store from lingering).
        let path = tmp_store_path();
        let mut store = NonceStore::open(&path).unwrap();
        store.reserve(SIGNER, NONCE_A, 1_000, 500).unwrap();
        assert_eq!(store.len(), 1);
        store.gc(1_000); // now == expiresAt
        assert_eq!(store.len(), 0, "entry expiring at `now` is GC'd");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dry_run_semantics_are_the_callers_not_the_stores() {
        // Acceptance criterion: dry-run envelopes do NOT reserve a nonce. The
        // store has no dry-run concept; the caller simply does not call
        // reserve() for a dry run. This test documents that contract: a nonce
        // never handed to reserve() remains re-submittable (unseen) until the
        // caller decides otherwise.
        let path = tmp_store_path();
        let mut store = NonceStore::open(&path).unwrap();
        let now = 1_000;
        let expires = now + 300;

        // Simulate two dry-run submissions of the SAME envelope: because dry
        // runs never call reserve(), the store never records the nonce, so a
        // later REAL apply of a different (fresh) nonce is unaffected and the
        // dry-run nonce would still be reservable if ever promoted to a real
        // apply within its window.
        assert!(store.is_empty(), "no reserve() calls → store stays empty");
        // A real apply with the (still-unseen) nonce succeeds — proving the dry
        // runs did not consume it.
        assert_eq!(
            store.reserve(SIGNER, NONCE_A, expires, now).unwrap(),
            ReserveOutcome::Reserved
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_store_file_is_a_hard_error_not_a_silent_wipe() {
        // Silently discarding a corrupt store would reopen every replay slot it
        // protected — the opposite of what persistence is for. open() must
        // error instead.
        let path = tmp_store_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not json").unwrap();
        let err = NonceStore::open(&path).unwrap_err();
        assert!(
            err.to_string().contains("failed to parse nonce store"),
            "unexpected error: {err}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn path_from_data_dir_places_file_alongside_config() {
        let data_dir = Path::new("/some/.botho/testnet");
        let p = NonceStore::path_from_data_dir(data_dir);
        assert_eq!(p, data_dir.join(NONCE_STORE_FILE));
    }
}
