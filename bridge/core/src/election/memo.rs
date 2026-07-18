// Copyright (c) 2024 The Botho Foundation

//! Election memo-convention formats for the elected bridge multisig
//! (ADR 0010 option C / sub-variant **A1** — ballots are memo-convention
//! transactions on the Botho chain, no protocol change and no new tx type).
//!
//! Two memo kinds ride inside ordinary Botho transaction memos:
//!
//! - **Self-nomination** (`bridge.nominate`): a curated node stands for a term.
//!   Valid only *before* `openHeight`; one nomination per curated identity (ADR
//!   0010 "Candidacy = opt-in").
//! - **Ballot** (`bridge.ballot`): approval voting — the voter approves a
//!   subset of the candidates. One node, one vote; valid within
//!   `openHeight..=closeHeight`.
//!
//! Both are **domain-separated, canonical-JSON** payloads signed by the
//! node's long-lived curated identity key, mirroring the attestation-envelope
//! discipline in [`crate::attestation`] (sorted keys, no whitespace,
//! integers-only, unknown/duplicate keys rejected). The signed bytes are
//! `ELECTION_MEMO_DOMAIN || memo_bytes`, so an election signature can never
//! be replayed as an attestation, an operator action, or any other Ed25519
//! payload a node key signs.
//!
//! Everything here is a **pure function** — no wall-clock, no randomness, no
//! I/O — so the tally in [`crate::election::tally`] is reproducible by anyone
//! replaying the ledger.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Map, Value};

/// The only election-memo version verifiers accept. Unknown versions are
/// rejected fail-closed with no downgrade path.
pub const ELECTION_MEMO_VERSION: u64 = 1;

/// Domain-separation prefix every election-memo signature covers. Distinct
/// from the attestation-envelope domains (`botho-bridge-attest-*`) and the
/// term-document domain (`botho.bridge.term.v2:`), so an election signature is
/// confined to elections.
pub const ELECTION_MEMO_DOMAIN: &[u8] = b"botho.bridge.election.v1:";

/// Wire `kind` for a self-nomination memo.
pub const MEMO_KIND_NOMINATE: &str = "bridge.nominate";

/// Wire `kind` for a ballot memo.
pub const MEMO_KIND_BALLOT: &str = "bridge.ballot";

/// A parsed, structurally-validated election memo (after — or independent of
/// — signature verification). Field values are verbatim from the memo bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElectionMemo {
    /// A curated node stands for `term`. Valid only before `openHeight`.
    Nomination {
        /// The term this candidate stands for.
        term: u64,
        /// The standing node's curated identity id.
        node_id: String,
    },
    /// An approval-voting ballot: `node_id` approves everyone in `approvals`.
    Ballot {
        /// The term this ballot is cast in.
        term: u64,
        /// The voting node's curated identity id.
        node_id: String,
        /// The approved candidate ids, as they appeared in the memo. The
        /// canonical form is sorted+deduped; parsing does not re-canonicalise
        /// (the tally deduplicates and drops non-candidates itself).
        approvals: Vec<String>,
    },
}

impl ElectionMemo {
    /// The curated identity id that must have signed this memo.
    pub fn node_id(&self) -> &str {
        match self {
            ElectionMemo::Nomination { node_id, .. } | ElectionMemo::Ballot { node_id, .. } => {
                node_id
            }
        }
    }

    /// The term this memo pertains to.
    pub fn term(&self) -> u64 {
        match self {
            ElectionMemo::Nomination { term, .. } | ElectionMemo::Ballot { term, .. } => *term,
        }
    }
}

/// Build the canonical (sorted-key, whitespace-free) self-nomination memo
/// string for `node_id` standing in `term`.
pub fn canonical_nomination_memo(node_id: &str, term: u64) -> String {
    let s = |v: &str| serde_json::to_string(v).expect("string serialization is infallible");
    format!(
        "{{\"kind\":{kind},\"nodeId\":{node},\"term\":{term},\"v\":{v}}}",
        kind = s(MEMO_KIND_NOMINATE),
        node = s(node_id),
        v = ELECTION_MEMO_VERSION,
    )
}

/// Build the canonical (sorted-key, whitespace-free) ballot memo string. The
/// `approvals` list is **sorted and de-duplicated** so one logical ballot has
/// exactly one canonical byte encoding.
pub fn canonical_ballot_memo(node_id: &str, term: u64, approvals: &[String]) -> String {
    let s = |v: &str| serde_json::to_string(v).expect("string serialization is infallible");
    let mut sorted: Vec<&String> = approvals.iter().collect();
    sorted.sort();
    sorted.dedup();
    let arr = sorted.iter().map(|a| s(a)).collect::<Vec<_>>().join(",");
    format!(
        "{{\"approvals\":[{arr}],\"kind\":{kind},\"nodeId\":{node},\"term\":{term},\"v\":{v}}}",
        kind = s(MEMO_KIND_BALLOT),
        node = s(node_id),
        v = ELECTION_MEMO_VERSION,
    )
}

/// The exact byte string an election-memo signature covers:
/// `ELECTION_MEMO_DOMAIN || memo_bytes`.
pub fn election_signed_message(memo: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(ELECTION_MEMO_DOMAIN.len() + memo.len());
    msg.extend_from_slice(ELECTION_MEMO_DOMAIN);
    msg.extend_from_slice(memo.as_bytes());
    msg
}

/// Ed25519-sign an election memo with a node's long-lived curated identity
/// key, returning the lowercase-hex detached signature.
pub fn sign_election_memo_ed25519(memo: &str, signing_key: &SigningKey) -> String {
    hex::encode(signing_key.sign(&election_signed_message(memo)).to_bytes())
}

/// Verify a detached election-memo signature against the signer's curated
/// identity key (`verify_strict`, rejecting malleable / low-order signatures).
pub fn verify_election_memo(
    memo: &str,
    signature_hex: &str,
    verifying_key: &VerifyingKey,
) -> Result<(), String> {
    let raw =
        hex::decode(signature_hex.trim()).map_err(|_| "signature is not valid hex".to_string())?;
    let sig_bytes: [u8; 64] = raw
        .as_slice()
        .try_into()
        .map_err(|_| "ed25519 signature must be 64 bytes".to_string())?;
    let sig = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify_strict(&election_signed_message(memo), &sig)
        .map_err(|_| "signature verification failed".to_string())
}

/// Decode a 32-byte Ed25519 public key from lowercase hex.
pub fn verifying_key_from_hex(hex_str: &str) -> Result<VerifyingKey, String> {
    let raw = hex::decode(hex_str.trim()).map_err(|_| "pubkey is not valid hex".to_string())?;
    let bytes: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| "ed25519 pubkey must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| "invalid ed25519 pubkey".to_string())
}

/// Parse canonical election-memo bytes, REJECTING unknown or duplicate keys
/// (at any nesting level) and any type/shape error, so a single signed byte
/// string can never carry two logical memos.
pub fn parse_election_memo(bytes: &str) -> Result<ElectionMemo, String> {
    reject_duplicate_keys(bytes)?;

    let value: Value =
        serde_json::from_str(bytes).map_err(|e| format!("memo is not valid JSON: {e}"))?;
    let obj: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| "memo must be a JSON object".to_string())?;

    let v = get_u64(obj, "v")?;
    if v != ELECTION_MEMO_VERSION {
        return Err(format!(
            "unsupported memo version {v} (expected {ELECTION_MEMO_VERSION})"
        ));
    }

    let kind = get_str(obj, "kind")?.to_string();
    let term = get_u64(obj, "term")?;
    let node_id = get_str(obj, "nodeId")?.to_string();

    match kind.as_str() {
        MEMO_KIND_NOMINATE => {
            const KNOWN: &[&str] = &["kind", "nodeId", "term", "v"];
            check_known_keys(obj, KNOWN)?;
            Ok(ElectionMemo::Nomination { term, node_id })
        }
        MEMO_KIND_BALLOT => {
            const KNOWN: &[&str] = &["approvals", "kind", "nodeId", "term", "v"];
            check_known_keys(obj, KNOWN)?;
            let raw = obj
                .get("approvals")
                .ok_or_else(|| "missing field `approvals`".to_string())?
                .as_array()
                .ok_or_else(|| "`approvals` must be an array".to_string())?;
            let mut approvals = Vec::with_capacity(raw.len());
            for a in raw {
                approvals.push(
                    a.as_str()
                        .ok_or_else(|| "each approval must be a string".to_string())?
                        .to_string(),
                );
            }
            Ok(ElectionMemo::Ballot {
                term,
                node_id,
                approvals,
            })
        }
        other => Err(format!("memo kind `{other}` is not in the v1 allowlist")),
    }
}

fn check_known_keys(obj: &Map<String, Value>, known: &[&str]) -> Result<(), String> {
    for key in obj.keys() {
        if !known.contains(&key.as_str()) {
            return Err(format!("unknown memo field `{key}`"));
        }
    }
    Ok(())
}

fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .ok_or_else(|| format!("missing field `{key}`"))?
        .as_str()
        .ok_or_else(|| format!("`{key}` must be a string"))
}

/// Read an integer field, REJECTING non-integers (`serde_json` parses `5.0`
/// as a float, so `as_u64` returns `None` — fail-closed).
fn get_u64(obj: &Map<String, Value>, key: &str) -> Result<u64, String> {
    obj.get(key)
        .ok_or_else(|| format!("missing field `{key}`"))?
        .as_u64()
        .ok_or_else(|| format!("`{key}` must be a non-negative integer"))
}

/// Reject any JSON value containing a duplicated object key at ANY nesting
/// level. `serde_json::Map` silently collapses duplicates (last-wins), which
/// would let two logical memos share one signed byte string. Mirrors the
/// proven guard in [`crate::attestation`].
fn reject_duplicate_keys(bytes: &str) -> Result<(), String> {
    use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

    struct AnyChecker;
    impl<'de> DeserializeSeed<'de> for AnyChecker {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_any(AnyVisitor)
        }
    }

    struct AnyVisitor;
    impl<'de> Visitor<'de> for AnyVisitor {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("any JSON value with unique object keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut seen = std::collections::HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                map.next_value_seed(AnyChecker)?;
                if !seen.insert(key.clone()) {
                    return Err(serde::de::Error::custom(format!("duplicate key `{key}`")));
                }
            }
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(AnyChecker)?.is_some() {}
            Ok(())
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
    }

    let mut de = serde_json::Deserializer::from_str(bytes);
    AnyChecker
        .deserialize(&mut de)
        .map_err(|e| format!("{e}"))?;
    Ok(())
}
