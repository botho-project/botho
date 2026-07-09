//! Operator read surface (#707, P4.2) + persisted audit log (#750, P4.4d).
//!
//! This module holds the node-side state for the operator-only READ RPCs
//! (`operator_getQuorumInfo`, `operator_getAuditLog`). The token machinery
//! itself lives in [`super::auth`] (reusing the single audited HMAC path); the
//! request handlers live in [`super`] (`mod.rs`) alongside the other JSON-RPC
//! handlers.
//!
//! SCOPE: reads only, PLUS the append-only audit STORE the write path feeds.
//! The audit store ([`OperatorAuditLog`]) is append-only JSONL at
//! `<data-dir>/operator-audit.jsonl` (`docs/security/quorum-write-path.md` §6).
//! The operator WRITE path itself (signed quorum curation — envelope verify +
//! gate-routed apply) lives in [`crate::operator_action`] +
//! `commands::run`; #750 only wires the audit APPEND + rejected-requests
//! counter onto the outcome that path already produces.
//!
//! ## Audit-log posture (§6, §8.4)
//!
//! - **Authenticated outcomes ONLY.** [`OperatorAuditLog::append`] is called
//!   for applied / gate-refused / post-signature refusals. Pre-signature
//!   failures (§4 steps 1–3: not-configured, unknown signer, bad signature) are
//!   reachable by ANY unauthenticated caller on the RPC port, so audit-logging
//!   them would hand out an unbounded disk-fill / log-spam primitive (round-1
//!   finding 3). Those instead go through
//!   [`OperatorAuditLog::note_pre_signature_rejection`], which increments a
//!   counter (surfaced in `node_getStatus`) and emits only RATE-LIMITED
//!   `debug!` tracing — never a file write.
//! - **Tracing mirror.** Every authenticated `append` also emits a `warn!`
//!   event (operator actions are rare + operationally significant) so journald
//!   / CloudWatch capture them independent of the JSONL file.
//! - **Tamper posture (§8.4).** The log is node-local and append-only *by
//!   convention*, NOT tamper-proof: a node-root attacker can rewrite it. That
//!   is acceptable — node root already loses that node — and fleet-level
//!   attribution survives via the OTHER nodes' logs plus the journald mirror.
//!   Deliberately NOT over-engineered: no signing, no merkle chain.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

/// Default file name for the persisted audit log, placed under the node's data
/// dir alongside `config.toml` / `operator-nonces.json` (see
/// [`OperatorAuditLog::path_from_data_dir`]).
pub const AUDIT_LOG_FILE: &str = "operator-audit.jsonl";

/// Minimum interval between pre-signature-rejection `debug!` traces. A flood of
/// unauthenticated bad requests must not spam the log (finding 3), so the
/// tracing is rate-limited to at most one line per this window; the COUNTER
/// still increments on every rejection so the flood remains observable in
/// `node_getStatus`.
const PRE_SIG_TRACE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// One operator audit-log entry — the full §6 shape
/// (`docs/security/quorum-write-path.md` §6).
///
/// This is the wire+storage shape the write path appends on every
/// **authenticated** verification outcome (applied / gate-refused /
/// post-signature verify-refused). Serialized as one JSON object per line
/// (JSONL) both on disk and in the `operator_getAuditLog` response, so the
/// dashboard renders EXCLUSIVELY from stored entries (anti-#541: never a
/// fabricated success state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorAuditEntry {
    /// Unix seconds when the action was processed.
    pub ts: u64,
    /// Fingerprint of the signing key (`signerKeyId`).
    #[serde(rename = "signerKeyId")]
    pub signer_key_id: String,
    /// `blake2b-256` hex of the canonical signed envelope bytes (§6).
    #[serde(rename = "envelopeHash")]
    pub envelope_hash: String,
    /// The attempted action (`quorum.pin_member`, etc.).
    pub action: String,
    /// The attempted action params (echoed for refusals, which log the
    /// *attempted* mutation but no new state).
    pub params: Value,
    /// Whether the action was a dry run.
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    /// Outcome: `applied` | `gate_refused` | `verify_refused:<reason>`.
    pub outcome: String,
    /// The quorum posture BEFORE the edit (present when known).
    #[serde(rename = "prevQuorum", skip_serializing_if = "Option::is_none")]
    pub prev_quorum: Option<Value>,
    /// The quorum posture AFTER the edit — present ONLY for `applied` (refusals
    /// have no new state, §6).
    #[serde(rename = "newQuorum", skip_serializing_if = "Option::is_none")]
    pub new_quorum: Option<Value>,
    /// The gate snapshot for this evaluation (intersection verdict + member
    /// counts), when the gate ran. Absent for pre-gate refusals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<Value>,
}

/// Operator audit-log store: append-only JSONL, node-local (§6).
///
/// Entries are persisted one-JSON-object-per-line to
/// `<data-dir>/operator-audit.jsonl` and mirrored in an in-memory `Vec` so
/// `operator_getAuditLog` reads are cheap and never touch the disk. On
/// [`open`](Self::open) the existing file is replayed into memory, so entries
/// survive restart. A store with no `path` (tests, relay nodes) keeps entries
/// in memory only.
///
/// The rejected-requests counter is SEPARATE from the entry store: it counts
/// pre-signature failures (finding 3) and is surfaced by `node_getStatus`;
/// those failures NEVER produce a stored entry.
#[derive(Debug)]
pub struct OperatorAuditLog {
    /// In-memory mirror of the on-disk JSONL (newest last). Reads serve from
    /// here; writes append here AND to the file.
    entries: RwLock<Vec<OperatorAuditEntry>>,
    /// The JSONL file path, or `None` for an in-memory-only store (tests /
    /// relay nodes). The `Mutex` serializes the append (open-append-flush)
    /// so concurrent appends cannot interleave partial lines.
    path: Mutex<Option<PathBuf>>,
    /// Count of PRE-signature rejected requests (finding 3): unauthenticated
    /// callers whose request failed before signature verification. Surfaced in
    /// `node_getStatus` as `operatorRejectedRequests`. These NEVER append an
    /// entry — the counter is the only durable trace beyond rate-limited
    /// `debug!`.
    rejected_requests: AtomicU64,
    /// Last time a pre-signature-rejection `debug!` line was emitted, for rate
    /// limiting (finding 3: no unbounded log growth under a flood).
    last_pre_sig_trace: Mutex<Option<Instant>>,
}

impl Default for OperatorAuditLog {
    fn default() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            path: Mutex::new(None),
            rejected_requests: AtomicU64::new(0),
            last_pre_sig_trace: Mutex::new(None),
        }
    }
}

impl OperatorAuditLog {
    /// A fresh, in-memory-only audit-log store (no persistence). Used by tests
    /// and relay nodes; `node_getStatus` still surfaces its counter.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Resolve the canonical audit-log path for a data dir: the directory that
    /// holds `config.toml` (the same dir as `operator-nonces.json`). Mirrors
    /// [`crate::operator_nonce::NonceStore::path_from_data_dir`].
    pub fn path_from_data_dir(data_dir: &Path) -> PathBuf {
        data_dir.join(AUDIT_LOG_FILE)
    }

    /// Open (or create) a PERSISTED audit log at `path`, replaying any existing
    /// entries into memory so they survive restart (§6: "entries must persist
    /// and survive restart").
    ///
    /// A missing file yields an empty store (the file is created lazily on the
    /// first append). Malformed lines are SKIPPED with a `warn!` rather than
    /// failing to start — unlike the nonce store, a partially-unreadable audit
    /// log must not brick the node (the log is observability, not a safety
    /// gate), and the tamper posture (§8.4) already treats the file as
    /// best-effort. We keep every line we CAN parse so the operator still sees
    /// the surviving history.
    pub fn open(path: &Path) -> Arc<Self> {
        let mut entries = Vec::new();
        if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    for (lineno, line) in contents.lines().enumerate() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<OperatorAuditEntry>(line) {
                            Ok(entry) => entries.push(entry),
                            Err(e) => warn!(
                                path = %path.display(),
                                line = lineno + 1,
                                error = %e,
                                "skipping unparseable operator audit-log line on load"
                            ),
                        }
                    }
                }
                Err(e) => warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to read operator audit log; starting with in-memory history only"
                ),
            }
        }

        Arc::new(Self {
            entries: RwLock::new(entries),
            path: Mutex::new(Some(path.to_path_buf())),
            rejected_requests: AtomicU64::new(0),
            last_pre_sig_trace: Mutex::new(None),
        })
    }

    /// The most recent `limit` entries, newest first. Serves from the in-memory
    /// mirror; renders EXCLUSIVELY from stored entries (anti-#541).
    pub fn recent(&self, limit: usize) -> Vec<OperatorAuditEntry> {
        let guard = match self.entries.read() {
            Ok(g) => g,
            // A poisoned lock must not fabricate data; report "no entries".
            Err(_) => return Vec::new(),
        };
        guard.iter().rev().take(limit).cloned().collect()
    }

    /// The current pre-signature rejected-requests count (finding 3), surfaced
    /// in `node_getStatus`.
    pub fn rejected_requests(&self) -> u64 {
        self.rejected_requests.load(Ordering::Relaxed)
    }

    /// Append one AUTHENTICATED audit entry: persist it as a JSONL line (§6)
    /// AND emit the `warn!` tracing mirror.
    ///
    /// The caller MUST only pass authenticated outcomes (applied / gate-refused
    /// / post-signature verify-refused). Pre-signature failures go through
    /// [`note_pre_signature_rejection`](Self::note_pre_signature_rejection)
    /// instead — they NEVER reach here (finding 3).
    ///
    /// A file-write failure is logged and swallowed (the in-memory mirror + the
    /// `warn!` still capture the event): the audit log is observability, not a
    /// safety gate, and must not fail an already-applied action.
    pub fn append(&self, entry: OperatorAuditEntry) {
        // Tracing mirror (§6): every authenticated entry emits a warn! event so
        // journald/CloudWatch capture it independent of the JSONL file.
        warn!(
            target: "operator_audit",
            ts = entry.ts,
            signer_key_id = %entry.signer_key_id,
            envelope_hash = %entry.envelope_hash,
            action = %entry.action,
            dry_run = entry.dry_run,
            outcome = %entry.outcome,
            "operator action audit entry"
        );

        // Persist as a single JSONL line (append-only). Serialize BEFORE taking
        // any lock so a serialization error cannot poison the path mutex.
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to serialize operator audit entry; not persisted");
                // Still record in memory so the read RPC reflects it this run.
                if let Ok(mut guard) = self.entries.write() {
                    guard.push(entry);
                }
                return;
            }
        };

        if let Ok(path_guard) = self.path.lock() {
            if let Some(path) = path_guard.as_ref() {
                if let Err(e) = append_line(path, &line) {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to persist operator audit entry to JSONL; kept in memory only"
                    );
                }
            }
        }

        // Mirror into memory for cheap, disk-free reads.
        if let Ok(mut guard) = self.entries.write() {
            guard.push(entry);
        }
    }

    /// Record a PRE-signature rejected request (finding 3): increment the
    /// counter surfaced in `node_getStatus` and emit ONLY rate-limited `debug!`
    /// tracing. This NEVER appends a JSONL entry — pre-signature failures are
    /// reachable by any unauthenticated caller, so persisting them would be an
    /// unbounded disk-fill / log-spam primitive.
    ///
    /// `reason_tag` is a short, stable machine tag (no attacker free-text, no
    /// secrets) for the rate-limited trace line.
    pub fn note_pre_signature_rejection(&self, reason_tag: &str) {
        let count = self.rejected_requests.fetch_add(1, Ordering::Relaxed) + 1;

        // Rate-limit the debug! so a flood cannot grow the log without bound.
        let now = Instant::now();
        let should_trace = match self.last_pre_sig_trace.lock() {
            Ok(mut last) => {
                let due = last
                    .map(|t| now.duration_since(t) >= PRE_SIG_TRACE_INTERVAL)
                    .unwrap_or(true);
                if due {
                    *last = Some(now);
                }
                due
            }
            // On a poisoned lock, err toward silence (never toward a flood).
            Err(_) => false,
        };
        if should_trace {
            debug!(
                target: "operator_audit",
                reason = reason_tag,
                total_rejected = count,
                "pre-signature operator request rejected (unauthenticated; not audit-logged, \
                 counter only — trace rate-limited)"
            );
        }
    }
}

/// Append a single line (plus newline) to a file, creating it if absent, with
/// owner-only permissions — matching how the node protects its other
/// small-state files (`config.toml`, `operator-nonces.json`).
///
/// This is a plain open-append-write, NOT the nonce store's atomic
/// temp-file+rename: the audit log is append-only and each line is
/// self-contained JSONL, so a torn tail line on a crash costs at most the last
/// entry (recoverable via the `warn!` journald mirror, §6) rather than
/// corrupting the whole file — and rewriting the entire file per entry would
/// not scale. Append is the right primitive for an append-only log.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A process-unique, thread-safe temp path per call. A wall-clock suffix
    /// (`SystemTime::now`) collides when two parallel test threads start within
    /// the same clock tick (the #749 lesson); an atomic counter cannot.
    fn tmp_audit_path() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("botho-audit-test-{}-{}", std::process::id(), n));
        dir.join(AUDIT_LOG_FILE)
    }

    fn applied_entry(ts: u64) -> OperatorAuditEntry {
        OperatorAuditEntry {
            ts,
            signer_key_id: "a1b2c3d4e5f60708".to_string(),
            envelope_hash: "deadbeef".repeat(8),
            action: "quorum.pin_member".to_string(),
            params: json!({"peerId": "12D3KooWfake"}),
            dry_run: false,
            outcome: "applied".to_string(),
            prev_quorum: Some(json!({"mode": "recommended", "members": [], "maxAutoMembers": 8})),
            new_quorum: Some(
                json!({"mode": "recommended", "members": ["12D3KooWfake"], "maxAutoMembers": 8}),
            ),
            gate: Some(json!({"intersectionRefused": false, "members": 5})),
        }
    }

    fn refusal_entry(ts: u64, outcome: &str) -> OperatorAuditEntry {
        OperatorAuditEntry {
            ts,
            signer_key_id: "a1b2c3d4e5f60708".to_string(),
            envelope_hash: "cafe".repeat(16),
            action: "quorum.unpin_member".to_string(),
            params: json!({"peerId": "12D3KooWfake"}),
            dry_run: false,
            outcome: outcome.to_string(),
            prev_quorum: Some(json!({"mode": "recommended", "members": [], "maxAutoMembers": 8})),
            new_quorum: None,
            gate: None,
        }
    }

    // -- Full §6 shape: applied / gate_refused / verify_refused --------------

    #[test]
    fn applied_entry_carries_full_shape_including_new_quorum() {
        let log = OperatorAuditLog::new();
        log.append(applied_entry(1000));
        let got = log.recent(10);
        assert_eq!(got.len(), 1);
        let e = &got[0];
        assert_eq!(e.outcome, "applied");
        assert!(e.new_quorum.is_some(), "applied MUST carry newQuorum");
        assert!(e.prev_quorum.is_some());
        assert!(e.gate.is_some());
        assert_eq!(e.envelope_hash.len(), 64, "blake2b-256 hex is 64 chars");
    }

    #[test]
    fn refusals_omit_new_quorum() {
        let log = OperatorAuditLog::new();
        log.append(refusal_entry(1000, "gate_refused"));
        log.append(refusal_entry(1001, "verify_refused:replayed_nonce"));
        for e in log.recent(10) {
            assert!(
                e.new_quorum.is_none(),
                "refusal {} must have no newQuorum (no new state, §6)",
                e.outcome
            );
        }
    }

    #[test]
    fn recent_is_newest_first() {
        let log = OperatorAuditLog::new();
        log.append(applied_entry(1000));
        log.append(refusal_entry(2000, "gate_refused"));
        log.append(applied_entry(3000));
        let got = log.recent(10);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].ts, 3000, "newest first");
        assert_eq!(got[1].ts, 2000);
        assert_eq!(got[2].ts, 1000);
    }

    #[test]
    fn recent_respects_limit() {
        let log = OperatorAuditLog::new();
        for i in 0..10 {
            log.append(applied_entry(1000 + i));
        }
        assert_eq!(log.recent(3).len(), 3);
        assert_eq!(log.recent(3)[0].ts, 1009, "limit takes the newest N");
    }

    // -- Persistence + restart survival (§6) ---------------------------------

    #[test]
    fn entries_persist_to_jsonl_and_survive_restart() {
        let path = tmp_audit_path();
        {
            let log = OperatorAuditLog::open(&path);
            log.append(applied_entry(1000));
            log.append(refusal_entry(2000, "verify_refused:wrong_target"));
            // drop => simulate node exit; each append flushed synchronously.
        }

        // The on-disk file is JSONL: one JSON object per line.
        let raw = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2, "one JSONL line per entry");
        for line in &lines {
            serde_json::from_str::<OperatorAuditEntry>(line)
                .expect("each line must be a standalone JSON object");
        }

        // Restart: a fresh store replays the file, newest-first.
        let reopened = OperatorAuditLog::open(&path);
        let got = reopened.recent(10);
        assert_eq!(got.len(), 2, "entries survive restart");
        assert_eq!(got[0].ts, 2000, "still newest-first after restart");
        assert_eq!(got[1].ts, 1000);

        // A further append accumulates rather than truncating.
        reopened.append(applied_entry(3000));
        let after = OperatorAuditLog::open(&path).recent(10);
        assert_eq!(after.len(), 3, "append-only: prior entries retained");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn on_disk_json_uses_the_camelcase_wire_keys() {
        let path = tmp_audit_path();
        let log = OperatorAuditLog::open(&path);
        log.append(applied_entry(1000));
        let raw = std::fs::read_to_string(&path).unwrap();
        // §6 wire shape.
        assert!(raw.contains("\"signerKeyId\""));
        assert!(raw.contains("\"envelopeHash\""));
        assert!(raw.contains("\"dryRun\""));
        assert!(raw.contains("\"prevQuorum\""));
        assert!(raw.contains("\"newQuorum\""));
        std::fs::remove_file(&path).ok();
    }

    // -- FINDING 3: pre-signature failures NEVER append; counter increments ---

    #[test]
    fn finding3_pre_signature_flood_bumps_counter_while_jsonl_stays_empty() {
        // The load-bearing finding-3 test: under a flood of pre-signature
        // failures the rejected-requests counter increments, while the audit
        // JSONL file is NEVER written (no unbounded disk-fill primitive).
        let path = tmp_audit_path();
        let log = OperatorAuditLog::open(&path);

        for _ in 0..10_000 {
            log.note_pre_signature_rejection("bad_signature");
        }

        assert_eq!(
            log.rejected_requests(),
            10_000,
            "counter must increment on every pre-signature rejection"
        );
        // The JSONL file must NOT exist / be empty: pre-signature failures never
        // write an entry (finding 3).
        assert!(
            !path.exists(),
            "pre-signature failures must NOT create the audit JSONL file"
        );
        assert!(
            log.recent(10).is_empty(),
            "pre-signature failures must NOT appear in the audit log"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn finding3_authenticated_append_coexists_with_pre_sig_counter() {
        // A mix: authenticated appends land in the JSONL; pre-signature
        // rejections only bump the counter. The two channels never cross.
        let path = tmp_audit_path();
        let log = OperatorAuditLog::open(&path);

        log.append(applied_entry(1000));
        for _ in 0..500 {
            log.note_pre_signature_rejection("unknown_signer");
        }
        log.append(refusal_entry(2000, "gate_refused"));

        assert_eq!(log.rejected_requests(), 500);
        // Only the TWO authenticated entries are in the file.
        let raw = std::fs::read_to_string(&path).unwrap();
        let lines = raw.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(lines, 2, "only authenticated outcomes are persisted");
        assert_eq!(log.recent(10).len(), 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn in_memory_only_store_still_counts_pre_sig_rejections() {
        // Relay nodes / tests use an in-memory-only store (no path). The counter
        // still works (surfaced in node_getStatus) and no file is touched.
        let log = OperatorAuditLog::new();
        for _ in 0..7 {
            log.note_pre_signature_rejection("not_configured");
        }
        assert_eq!(log.rejected_requests(), 7);
        assert!(log.recent(10).is_empty());
    }

    #[test]
    fn malformed_lines_are_skipped_on_load_not_fatal() {
        // Unlike the nonce store, a partially-corrupt audit log must not brick
        // the node: parseable lines survive, garbage is skipped.
        let path = tmp_audit_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let good = serde_json::to_string(&applied_entry(1000)).unwrap();
        std::fs::write(&path, format!("{good}\nthis is not json\n{good}\n")).unwrap();

        let log = OperatorAuditLog::open(&path);
        assert_eq!(
            log.recent(10).len(),
            2,
            "the two good lines load; the garbage line is skipped"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn path_from_data_dir_places_file_alongside_config() {
        let data_dir = Path::new("/some/.botho/testnet");
        let p = OperatorAuditLog::path_from_data_dir(data_dir);
        assert_eq!(p, data_dir.join(AUDIT_LOG_FILE));
    }
}
