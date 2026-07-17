// Copyright (c) 2024 The Botho Foundation

//! The electorate snapshot the tally binds to: a pinned view of the P4.4
//! operator-signed curated node set (#709).
//!
//! ADR 0010 fixes the electorate as "a snapshot of the P4.4 operator-curated
//! node set at a pinned height" — one node, one vote. The tally never reads
//! the *live* curation; it reads exactly this snapshot, so two verifiers who
//! pin the same height compute the same electorate and therefore the same
//! result. The snapshot also carries each node's long-lived Ed25519 curated
//! identity key, which is what election-memo signatures are checked against.

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::memo::verifying_key_from_hex;

/// One curated node in the electorate snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CuratedNode {
    /// The node's stable curated identity id (the vote/candidacy identity).
    pub node_id: String,
    /// The node's long-lived curated identity Ed25519 public key, lowercase
    /// hex (32 bytes). Election-memo signatures are verified against this.
    pub identity_pubkey_hex: String,
}

/// A pinned snapshot of the operator-curated node set, the electorate for one
/// election. `curation_doc_hash` + `snapshot_height` bind the tally to an
/// exact, replayable curation state (folded into the term document's
/// `electorate` block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurationSnapshot {
    /// Hash of the operator-signed curation document this snapshot is taken
    /// from (opaque to the tally; carried through to the term document).
    pub curation_doc_hash: String,
    /// The ledger height the curation set is pinned at.
    pub snapshot_height: u64,
    /// The curated nodes. Duplicate `node_id`s are rejected by
    /// [`CurationSnapshot::validate`].
    pub nodes: Vec<CuratedNode>,
}

impl CurationSnapshot {
    /// Structural validation independent of any election: unique, non-empty
    /// node ids and well-formed identity keys. Returns the offending detail.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for node in &self.nodes {
            if node.node_id.is_empty() {
                return Err("curation snapshot contains an empty nodeId".to_string());
            }
            if !seen.insert(node.node_id.as_str()) {
                return Err(format!("duplicate curated nodeId `{}`", node.node_id));
            }
            verifying_key_from_hex(&node.identity_pubkey_hex)
                .map_err(|e| format!("node `{}`: {e}", node.node_id))?;
        }
        Ok(())
    }

    /// The eligible-voter ids, **sorted** (the deterministic electorate list
    /// the term document records).
    pub fn eligible_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.nodes.iter().map(|n| n.node_id.clone()).collect();
        ids.sort();
        ids
    }

    /// The number of eligible voters (the quorum denominator).
    pub fn eligible_count(&self) -> usize {
        self.nodes.len()
    }

    /// Whether `node_id` is in the curated electorate.
    pub fn contains(&self, node_id: &str) -> bool {
        self.nodes.iter().any(|n| n.node_id == node_id)
    }

    /// The curated identity verifying key for `node_id`, if curated.
    pub fn verifying_key(&self, node_id: &str) -> Option<VerifyingKey> {
        self.nodes
            .iter()
            .find(|n| n.node_id == node_id)
            .and_then(|n| verifying_key_from_hex(&n.identity_pubkey_hex).ok())
    }
}
