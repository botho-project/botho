// Copyright (c) 2024 The Botho Foundation

//! Persisted attestation nonce store — the seen-nonce leg of replay
//! protection for the federation attestation protocol (#824).
//!
//! This is a port of the node's operator-action nonce store
//! (`botho/src/operator_nonce.rs`, #749) into the bridge crates, with the
//! same guarantees:
//!
//! 1. **Single-use nonces.** A `(signer_key_id, nonce)` pair can be
//!    [`reserve`](NonceStore::reserve)d exactly once; a second attempt is a
//!    [`ReserveOutcome::Replay`].
//! 2. **Persistence across restarts.** File-backed stores flush atomically
//!    (temp file + rename, owner-only permissions) on every successful reserve,
//!    so a bridge restart inside an attestation's validity window does not
//!    reopen a replay slot.
//! 3. **Reserve-then-apply fail-safe.** The verifier reserves the nonce BEFORE
//!    counting the attestation toward the threshold; a crash in the gap leaves
//!    the nonce consumed, so a single signed envelope can never count twice.
//!    The recovery path is a re-signed fresh-nonce envelope.
//! 4. **Bounded retention.** Expired entries are garbage-collected on every
//!    reserve, bounding the store by the attestation rate within one validity
//!    window.
//!
//! An [`in_memory`](NonceStore::in_memory) mode exists for tests and for
//! deployments that have not configured a persistence path — the engine
//! logs loudly when it falls back to it, because a restart then DOES reopen
//! replay slots (mitigated by the freshness window).

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Current on-disk format version for the nonce store file.
const NONCE_STORE_VERSION: u32 = 1;

/// The outcome of attempting to [`reserve`](NonceStore::reserve) a nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveOutcome {
    /// The nonce was previously unseen and is now recorded (consumed). The
    /// caller may proceed to count the attestation.
    Reserved,
    /// The `(signer_key_id, nonce)` pair was already recorded — this is a
    /// replay and MUST be rejected by the caller.
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

/// Composite key identifying a consumed nonce: the signer identity plus the
/// nonce. Scoped per signer so two federation members cannot collide. A NUL
/// separator can never appear in the hex fields, so the join is unambiguous.
fn store_key(signer_key_id: &str, nonce: &str) -> String {
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

/// A persisted, garbage-collected set of consumed attestation nonces.
#[derive(Debug)]
pub struct NonceStore {
    /// `None` for in-memory stores (tests / unconfigured deployments).
    path: Option<PathBuf>,
    file: NonceStoreFile,
}

impl NonceStore {
    /// Open (or create) the nonce store at `path`.
    ///
    /// A missing file yields an empty store (created lazily on the first
    /// successful reserve). A file that fails to parse is a hard error
    /// rather than being silently discarded — silently wiping the store
    /// would reopen every replay slot it was protecting.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = if path.exists() {
            let contents = std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read nonce store {}: {}", path.display(), e))?;
            let parsed: NonceStoreFile = serde_json::from_str(&contents).map_err(|e| {
                format!(
                    "failed to parse nonce store {} — refusing to continue with a corrupt \
                     store rather than silently reopening replay slots: {}",
                    path.display(),
                    e
                )
            })?;
            if parsed.version != NONCE_STORE_VERSION {
                return Err(format!(
                    "unsupported nonce store version {} at {} (expected {})",
                    parsed.version,
                    path.display(),
                    NONCE_STORE_VERSION
                ));
            }
            parsed
        } else {
            NonceStoreFile::default()
        };

        Ok(Self {
            path: Some(path.to_path_buf()),
            file,
        })
    }

    /// A store with no persistence. Replay protection holds only within the
    /// process lifetime; the attestation freshness window bounds the
    /// residual restart exposure.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            file: NonceStoreFile::default(),
        }
    }

    /// Attempt to consume `(signer_key_id, nonce)`, expiring at `expires_at`
    /// (unix seconds), treating `now` as the current time for garbage
    /// collection.
    ///
    /// On [`ReserveOutcome::Reserved`] a file-backed store durably persists
    /// the nonce BEFORE returning — reserve-then-apply is crash-safe. On
    /// [`ReserveOutcome::Replay`] nothing is written.
    pub fn reserve(
        &mut self,
        signer_key_id: &str,
        nonce: &str,
        expires_at: u64,
        now: u64,
    ) -> Result<ReserveOutcome, String> {
        // GC first so bursts never accumulate.
        self.gc(now);

        let key = store_key(signer_key_id, nonce);
        if self.file.entries.contains_key(&key) {
            return Ok(ReserveOutcome::Replay);
        }

        self.file.entries.insert(key.clone(), expires_at);
        // Persist BEFORE returning Ok. If the write fails, roll the
        // in-memory insert back so memory and disk stay consistent — the
        // caller must treat a non-durable reservation as failure.
        if let Err(e) = self.persist() {
            self.file.entries.remove(&key);
            return Err(e);
        }

        Ok(ReserveOutcome::Reserved)
    }

    /// Number of currently-retained (un-GC'd) entries.
    pub fn len(&self) -> usize {
        self.file.entries.len()
    }

    /// Whether the store currently holds no entries.
    pub fn is_empty(&self) -> bool {
        self.file.entries.is_empty()
    }

    /// Drop every entry whose `expires_at <= now`.
    pub fn gc(&mut self, now: u64) {
        self.file
            .entries
            .retain(|_, &mut expires_at| expires_at > now);
    }

    /// Atomically write the current store to disk with owner-only
    /// permissions (no-op for in-memory stores). Writes a sibling temp file
    /// and renames so a crash mid-write can never leave a corrupt store.
    fn persist(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!("failed to create directory {}: {}", parent.display(), e)
                })?;
            }
        }

        let json = serde_json::to_string_pretty(&self.file)
            .map_err(|e| format!("failed to serialize nonce store: {}", e))?;

        // Unique-ish temp path in the same directory (same filesystem, so
        // the rename is atomic).
        let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));

        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp_path).map_err(|e| {
                format!(
                    "failed to open temp nonce store {}: {}",
                    tmp_path.display(),
                    e
                )
            })?;
            f.write_all(json.as_bytes()).map_err(|e| {
                format!(
                    "failed to write temp nonce store {}: {}",
                    tmp_path.display(),
                    e
                )
            })?;
            f.sync_all().map_err(|e| {
                format!(
                    "failed to fsync temp nonce store {}: {}",
                    tmp_path.display(),
                    e
                )
            })?;
        }

        std::fs::rename(&tmp_path, path).map_err(|e| {
            format!(
                "failed to rename {} -> {}: {}",
                tmp_path.display(),
                path.display(),
                e
            )
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNER: &str = "a1b2c3d4e5f60708";
    const NONCE_A: &str = "9f2c00112233445566778899aabbccdd";
    const NONCE_B: &str = "0011223344556677889900aabbccddee";

    fn tmp_store_path() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bth-bridge-nonce-test-{}-{}",
            std::process::id(),
            n
        ));
        dir.join("attestation-nonces.json")
    }

    #[test]
    fn first_reserve_succeeds_replay_is_rejected() {
        let mut store = NonceStore::in_memory();
        let now = 1_000;
        let expires = now + 300;

        assert_eq!(
            store.reserve(SIGNER, NONCE_A, expires, now).unwrap(),
            ReserveOutcome::Reserved
        );
        assert_eq!(
            store.reserve(SIGNER, NONCE_A, expires, now).unwrap(),
            ReserveOutcome::Replay
        );
        // Different nonce / different signer are independent.
        assert!(store
            .reserve(SIGNER, NONCE_B, expires, now)
            .unwrap()
            .is_reserved());
        assert!(store
            .reserve("00000000deadbeef", NONCE_A, expires, now)
            .unwrap()
            .is_reserved());
    }

    #[test]
    fn replay_protection_survives_a_restart_within_the_window() {
        let path = tmp_store_path();
        let now = 1_000;
        let expires = now + 300;

        {
            let mut store = NonceStore::open(&path).unwrap();
            assert!(store
                .reserve(SIGNER, NONCE_A, expires, now)
                .unwrap()
                .is_reserved());
            // `store` drops here — simulating process exit.
        }

        let now2 = now + 60;
        let mut reopened = NonceStore::open(&path).unwrap();
        assert_eq!(
            reopened.reserve(SIGNER, NONCE_A, expires, now2).unwrap(),
            ReserveOutcome::Replay,
            "a restart inside the window must not reopen the replay slot"
        );
        // A re-signed fresh-nonce envelope is the recovery path.
        assert!(reopened
            .reserve(SIGNER, NONCE_B, expires, now2)
            .unwrap()
            .is_reserved());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn expired_entries_are_gcd_on_insert() {
        let mut store = NonceStore::in_memory();
        let expires = 1_300u64;
        for i in 0..100u64 {
            let nonce = format!("{i:032x}");
            assert!(store
                .reserve(SIGNER, &nonce, expires, 1_000)
                .unwrap()
                .is_reserved());
        }
        assert_eq!(store.len(), 100);

        // Past expiry: the stale entries are GC'd on the next insert.
        assert!(store
            .reserve(SIGNER, &format!("{:032x}", 9999u64), 1_600, 1_400)
            .unwrap()
            .is_reserved());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn corrupt_store_file_is_a_hard_error_not_a_silent_wipe() {
        let path = tmp_store_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not json").unwrap();
        let err = NonceStore::open(&path).unwrap_err();
        assert!(err.contains("failed to parse nonce store"), "{err}");
        std::fs::remove_file(&path).ok();
    }
}
