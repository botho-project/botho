//! Operator read surface (#707, P4.2 of the #695 proposal).
//!
//! This module holds the node-side state for the operator-only READ RPCs
//! (`operator_getQuorumInfo`, `operator_getAuditLog`). The token machinery
//! itself lives in [`super::auth`] (reusing the single audited HMAC path); the
//! request handlers live in [`super`] (`mod.rs`) alongside the other JSON-RPC
//! handlers.
//!
//! SCOPE: reads only. Nothing here mutates node state. The operator WRITE path
//! (signed quorum curation) is a separate, separately-reviewed deliverable
//! (#709, governed by `docs/security/quorum-write-path.md`).

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// One operator audit-log entry.
///
/// This is the wire+storage shape the write path (#709) will append to on
/// every verification outcome (applied / gate-refused / verify-refused), per
/// `docs/security/quorum-write-path.md` §6. In P4.2 the store is present but
/// always empty — there is no write path yet to append to it — so the shape is
/// defined here and `operator_getAuditLog` returns an empty list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorAuditEntry {
    /// Unix seconds when the action was processed.
    pub ts: u64,
    /// Fingerprint of the signing key (`signerKeyId`).
    pub signer_key_id: String,
    /// The attempted action (`quorum.pin_member`, etc.).
    pub action: String,
    /// Outcome: `applied` | `gate_refused` | `verify_refused:<reason>`.
    pub outcome: String,
}

/// Operator audit-log store: append-only, node-local.
///
/// P4.2 ships it EMPTY-BUT-PRESENT: the RPC surface and the store type exist so
/// the dashboard can render an (empty) audit panel today, and #709 only has to
/// wire the append side. Reads never fabricate entries (anti-#541): an empty
/// store returns an empty list, not a placeholder.
#[derive(Debug, Default)]
pub struct OperatorAuditLog {
    entries: RwLock<Vec<OperatorAuditEntry>>,
}

impl OperatorAuditLog {
    /// A fresh, empty audit-log store.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The most recent `limit` entries, newest first. Empty until #709 wires
    /// the write path in.
    pub fn recent(&self, limit: usize) -> Vec<OperatorAuditEntry> {
        let guard = match self.entries.read() {
            Ok(g) => g,
            // A poisoned lock must not fabricate data; report "no entries".
            Err(_) => return Vec::new(),
        };
        guard.iter().rev().take(limit).cloned().collect()
    }

    /// Append an entry (used by #709; unused in P4.2). Kept crate-visible so
    /// the write path has an obvious, single seam to call.
    #[allow(dead_code)]
    pub(crate) fn append(&self, entry: OperatorAuditEntry) {
        if let Ok(mut guard) = self.entries.write() {
            guard.push(entry);
        }
    }
}
