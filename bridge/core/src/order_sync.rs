// Copyright (c) 2024 The Botho Foundation

//! Signed mint order-record replication (#1050 Phase 2).
//!
//! # Why this exists
//!
//! The #858 attestation transport ([`crate::attestation`]) exchanges
//! *envelopes* only — the order **records** they bind to never replicate
//! between federation members. For the RELEASE/burn leg that is fine:
//! deterministic burn-order ids ([`crate::order::derive_burn_order_uuid`],
//! Phase 1) let every member independently reconstruct the identical burn
//! record from the same finalized on-chain event, so envelopes aggregate with
//! zero cross-member trust.
//!
//! The MINT leg cannot be fixed that way. A mint order is created by a user
//! against ONE member's public API; its id lives in a user-chosen deposit memo
//! (not in independently-observable chain data), and each member's BTH watcher
//! only ever *matches* a deposit to an order it already has on record — it
//! never creates the order. So a member that never saw the order returns
//! `refused:unknown_order` and a genuinely distributed (multi-host,
//! separate-DB) federation can never reach threshold on a mint. This module is
//! the missing piece: the originating member replicates the mint order
//! **record** to its ≤5 elected peers (ADR 0010 / #1060) so each peer's own
//! watcher has something to independently confirm against.
//!
//! # The trust boundary (safety-critical)
//!
//! Accepting a replicated record grants **no trust whatsoever**. The wire
//! format ([`MintOrderShell`]) is *structurally incapable* of carrying trust:
//! it has no `status`, no `mint_authorization`, no `dest_tx`, no `source_tx`
//! field. A received record is reconstituted only ever as a fresh
//! `AwaitingDeposit` shell ([`MintOrderShell::into_awaiting_deposit_order`]).
//! Each receiving member still independently confirms the on-chain BTH deposit
//! through its OWN watcher before it will attest. The signature authenticates
//! only *which elected member proposed the order*, never the deposit's
//! validity — the difference between "seed a shell for my own watcher to
//! confirm" and "trust a peer's claim about funds". A compromised member can
//! therefore, at worst, seed order shells that go nowhere unless a real deposit
//! independently lands; it can never cause a mint.
//!
//! # Authentication
//!
//! A record is signed with the originating member's Ed25519 federation key
//! (the same identity used for BTH release attestations — the elected
//! multisig set). On receipt the signature is verified against the configured
//! federation member set; a record signed by a key that is not an elected
//! member is rejected (`unknown_signer`) before it is ever persisted. The
//! verify is parse-after-verify: the record JSON is parsed only after its
//! signature is confirmed over exactly the received bytes, and the parser
//! rejects unknown/duplicate keys so one signed byte string can never carry a
//! second logical record.

use chrono::{TimeZone, Utc};
use ed25519_dalek::{Signature as Ed25519Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{
    chains::Chain,
    order::{BridgeOrder, OrderStatus, OrderType},
};

/// Domain-separation tag mixed in before the record bytes when signing a
/// replicated order record. Distinct from the attestation domains so an
/// order-record signature can never be replayed as an attestation (or vice
/// versa). Changing this tag is a bridge-breaking change for in-flight
/// replication.
pub const ORDER_RECORD_DOMAIN_TAG: &[u8] = b"botho-bridge-order-record-v1";

/// The minimal, trust-free projection of a mint order that is replicated to
/// peers. Deliberately carries ONLY the fields a receiving member needs to
/// reconstruct an `AwaitingDeposit` shell for its own BTH watcher to match a
/// deposit against — and NOTHING that could advance an order or authorize a
/// mint. There is no `status`, `mint_authorization`, `dest_tx` or `source_tx`
/// field: the type itself is the trust boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintOrderShell {
    /// Order id (matches the id embedded in the deposit memo).
    pub id: Uuid,
    /// Destination chain the wBTH is minted on.
    pub dest_chain: Chain,
    /// Gross amount in picocredits.
    pub amount: u64,
    /// Bridge fee in picocredits.
    pub fee: u64,
    /// The bridge's BTH deposit (stealth) address the user pays into.
    pub bth_deposit_address: String,
    /// Destination address that receives the minted wBTH.
    pub dest_address: String,
    /// The 64-byte deposit memo (its first 16 bytes are the order id).
    pub memo: [u8; 64],
    /// Order creation time as Unix seconds — replicated so every member's
    /// expiry clock agrees.
    pub created_at_unix: i64,
}

impl MintOrderShell {
    /// Project a mint order into its replicable shell.
    ///
    /// Fails for anything that must never be replicated: a non-mint order, an
    /// order that has already advanced past `AwaitingDeposit` (replicating a
    /// confirmed/authorized order would be a trust-boundary violation — peers
    /// derive confirmation independently), or a mint order with no memo (the
    /// memo is how peers' watchers match the deposit).
    pub fn from_order(order: &BridgeOrder) -> Result<Self, String> {
        if order.order_type != OrderType::Mint {
            return Err("only mint orders are replicated".to_string());
        }
        if order.status != OrderStatus::AwaitingDeposit {
            return Err(format!(
                "only AwaitingDeposit mint orders are replicated (status is {})",
                order.status
            ));
        }
        let memo = order
            .memo
            .ok_or_else(|| "mint order has no deposit memo to replicate".to_string())?;
        Ok(Self {
            id: order.id,
            dest_chain: order.dest_chain,
            amount: order.amount,
            fee: order.fee,
            bth_deposit_address: order.source_address.clone(),
            dest_address: order.dest_address.clone(),
            memo,
            created_at_unix: order.created_at.timestamp(),
        })
    }

    /// Reconstruct a fresh `AwaitingDeposit` mint order from the shell.
    ///
    /// This is the ONLY way a replicated record enters a receiving member's
    /// state, and it forces every safety-relevant field: status is
    /// `AwaitingDeposit`, and `mint_authorization` / `dest_tx` / `source_tx`
    /// are all `None` (via [`BridgeOrder::new_mint`]). The peer's own watcher
    /// must independently confirm the deposit before the order can advance.
    pub fn into_awaiting_deposit_order(self) -> BridgeOrder {
        let created_at = Utc
            .timestamp_opt(self.created_at_unix, 0)
            .single()
            .unwrap_or_else(Utc::now);
        let mut order = BridgeOrder::new_mint(
            self.dest_chain,
            self.amount,
            self.fee,
            self.bth_deposit_address,
            self.dest_address,
        );
        // new_mint already fixes: status = AwaitingDeposit, mint_authorization
        // = None, source_tx = None, dest_tx = None, dest_confirmed_at = None.
        // Override only the identity/timing/memo the shell replicates.
        order.id = self.id;
        order.memo = Some(self.memo);
        order.created_at = created_at;
        order.updated_at = created_at;
        order
    }

    /// The canonical (lexicographically key-ordered, whitespace-free,
    /// integers-only, memo-as-hex) JSON string that is signed. Both signer and
    /// verifier build the SAME bytes so verification is over an exact byte
    /// string.
    pub fn canonical_json(&self) -> String {
        let s = |v: &str| serde_json::to_string(v).expect("string serialization is infallible");
        format!(
            "{{\"amount\":{amount},\"bthDepositAddress\":{deposit},\"createdAt\":{created},\
             \"destAddress\":{dest},\"destChain\":{chain},\"fee\":{fee},\"id\":{id},\
             \"memo\":{memo}}}",
            amount = self.amount,
            deposit = s(&self.bth_deposit_address),
            created = self.created_at_unix,
            dest = s(&self.dest_address),
            chain = s(&self.dest_chain.to_string()),
            fee = self.fee,
            id = s(&self.id.to_string()),
            memo = s(&hex::encode(self.memo)),
        )
    }

    /// Parse a canonical record string, REJECTING unknown or duplicate keys
    /// and any type/shape error (parse-after-verify feeds this only bytes
    /// whose signature already verified).
    fn from_canonical_json(bytes: &str) -> Result<Self, String> {
        reject_duplicate_keys(bytes)?;
        let value: Value =
            serde_json::from_str(bytes).map_err(|e| format!("record is not valid JSON: {e}"))?;
        let obj: &Map<String, Value> = value
            .as_object()
            .ok_or_else(|| "record must be a JSON object".to_string())?;

        const KNOWN_KEYS: &[&str] = &[
            "amount",
            "bthDepositAddress",
            "createdAt",
            "destAddress",
            "destChain",
            "fee",
            "id",
            "memo",
        ];
        for key in obj.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                return Err(format!("unknown record field `{key}`"));
            }
        }

        let amount = get_u64(obj, "amount")?;
        let fee = get_u64(obj, "fee")?;
        let bth_deposit_address = get_str(obj, "bthDepositAddress")?.to_string();
        let dest_address = get_str(obj, "destAddress")?.to_string();
        let created_at_unix = obj
            .get("createdAt")
            .and_then(Value::as_i64)
            .ok_or_else(|| "field `createdAt` must be an integer".to_string())?;
        let dest_chain = get_str(obj, "destChain")?
            .parse::<Chain>()
            .map_err(|e| format!("bad destChain: {e}"))?;
        let id = get_str(obj, "id")?
            .parse::<Uuid>()
            .map_err(|_| "`id` is not a valid UUID".to_string())?;
        let memo_bytes = hex::decode(get_str(obj, "memo")?)
            .map_err(|_| "`memo` is not valid hex".to_string())?;
        let memo: [u8; 64] = memo_bytes
            .as_slice()
            .try_into()
            .map_err(|_| "`memo` must be 64 bytes".to_string())?;

        // Structural bind: the memo's first 16 bytes must equal the order id,
        // exactly as the deposit-matching path (`BridgeOrder::order_id_from_memo`)
        // will read it. A record whose memo does not carry its own id could
        // never be matched by a watcher and is rejected as malformed.
        match BridgeOrder::order_id_from_memo(&memo) {
            Some(memo_id) if memo_id == id => {}
            _ => return Err("record memo does not embed its own order id".to_string()),
        }

        Ok(Self {
            id,
            dest_chain,
            amount,
            fee,
            bth_deposit_address,
            dest_address,
            memo,
            created_at_unix,
        })
    }
}

/// A signed, replicable mint order record: the canonical record string exactly
/// as the originating member signed it, the signer's identity, and a detached
/// Ed25519 signature over `ORDER_RECORD_DOMAIN_TAG || record_bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRecordEnvelope {
    /// The canonical record JSON, verbatim, as signed.
    pub record: String,
    /// Signer identity: lowercase hex of the Ed25519 public key (64 chars).
    /// A routing hint only — a lying value selects a key the signature then
    /// fails to verify against.
    pub signer_key_id: String,
    /// Detached Ed25519 signature (lowercase hex, 64 bytes) over the
    /// domain-separated record bytes.
    pub signature_hex: String,
}

/// Why a replicated order record was refused. Mirrors the attestation reject
/// taxonomy so the transport can map each to a stable tag / HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderRecordRejectReason {
    /// Structurally invalid before any signature work (bad hex, bad JSON).
    Malformed(String),
    /// The signature did not verify against the selected member key.
    BadSignature,
    /// The `signer_key_id` names no configured (elected) federation member.
    UnknownSigner,
    /// The signature verified but the record body is not an acceptable
    /// replicable shell.
    InvalidRecord(String),
}

impl OrderRecordRejectReason {
    /// Stable machine tag (`refused:<tag>`), aligned with the attestation
    /// reject tags used on `/api/attest`.
    pub fn tag(&self) -> &'static str {
        match self {
            OrderRecordRejectReason::Malformed(_) => "malformed",
            OrderRecordRejectReason::BadSignature => "bad_signature",
            OrderRecordRejectReason::UnknownSigner => "unknown_signer",
            OrderRecordRejectReason::InvalidRecord(_) => "invalid_record",
        }
    }

    /// Human-facing detail (safe to return to the submitter).
    pub fn message(&self) -> String {
        match self {
            OrderRecordRejectReason::Malformed(m) => format!("malformed order record: {m}"),
            OrderRecordRejectReason::BadSignature => {
                "order record signature did not verify".to_string()
            }
            OrderRecordRejectReason::UnknownSigner => {
                "order record signer is not a federation member".to_string()
            }
            OrderRecordRejectReason::InvalidRecord(m) => format!("invalid order record: {m}"),
        }
    }
}

/// Build and sign a replicable order record from an `AwaitingDeposit` mint
/// order. The signer identity is the lowercase hex of the Ed25519 public key.
pub fn sign_order_record_ed25519(
    order: &BridgeOrder,
    signing_key: &SigningKey,
) -> Result<OrderRecordEnvelope, String> {
    let shell = MintOrderShell::from_order(order)?;
    let record = shell.canonical_json();
    let msg = signed_message(record.as_bytes());
    Ok(OrderRecordEnvelope {
        signer_key_id: hex::encode(signing_key.verifying_key().as_bytes()),
        signature_hex: hex::encode(signing_key.sign(&msg).to_bytes()),
        record,
    })
}

impl OrderRecordEnvelope {
    /// Verify a replicated order record against the elected federation member
    /// set and, on success, return the trust-free [`MintOrderShell`].
    ///
    /// Pipeline (no secret-dependent check precedes signature verification):
    /// 1. decode the signature (`Malformed` on bad hex/length);
    /// 2. select the member key named by `signer_key_id` — a name that is not
    ///    in the elected set is `UnknownSigner` (never trusted);
    /// 3. verify the signature over `domain || record_bytes` (`verify_strict`,
    ///    rejecting malleable/low-order signatures);
    /// 4. parse-after-verify the record bytes into a shell (unknown/duplicate
    ///    keys and any shape error rejected).
    pub fn verify_ed25519(
        &self,
        federation: &[VerifyingKey],
    ) -> Result<MintOrderShell, OrderRecordRejectReason> {
        let sig_bytes: [u8; 64] = hex::decode(self.signature_hex.trim())
            .map_err(|_| {
                OrderRecordRejectReason::Malformed("signature is not valid hex".to_string())
            })?
            .as_slice()
            .try_into()
            .map_err(|_| {
                OrderRecordRejectReason::Malformed("signature must be 64 bytes".to_string())
            })?;

        // Select the member key named by the (unverified) signer id. Not a
        // trust decision on its own: a lying id selects a key the signature
        // then fails to verify against.
        let key = federation
            .iter()
            .find(|k| hex::encode(k.as_bytes()) == self.signer_key_id)
            .ok_or(OrderRecordRejectReason::UnknownSigner)?;

        let msg = signed_message(self.record.as_bytes());
        key.verify_strict(&msg, &Ed25519Signature::from_bytes(&sig_bytes))
            .map_err(|_| OrderRecordRejectReason::BadSignature)?;

        // Parse-after-verify: signature is valid over exactly these bytes.
        MintOrderShell::from_canonical_json(&self.record)
            .map_err(OrderRecordRejectReason::InvalidRecord)
    }
}

/// The domain-separated message that is signed / verified.
fn signed_message(record_bytes: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(ORDER_RECORD_DOMAIN_TAG.len() + record_bytes.len());
    msg.extend_from_slice(ORDER_RECORD_DOMAIN_TAG);
    msg.extend_from_slice(record_bytes);
    msg
}

fn get_u64(obj: &Map<String, Value>, key: &str) -> Result<u64, String> {
    obj.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("field `{key}` must be an unsigned integer"))
}

fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("field `{key}` must be a string"))
}

/// Reject a JSON object with duplicate keys (a serde_json `Map` silently keeps
/// the last), so one signed byte string cannot smuggle a second value.
fn reject_duplicate_keys(bytes: &str) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_str(bytes);
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    serde::Deserializer::deserialize_map(&mut de, DuplicateKeyGuard { seen: &mut seen })
        .map_err(|e| e.to_string())
}

/// A serde visitor that only walks the top-level object keys and errors on the
/// first duplicate. It does not build a value — it is used purely for the
/// duplicate-key check before the real parse.
struct DuplicateKeyGuard<'a> {
    seen: &'a mut std::collections::BTreeSet<String>,
}

impl<'de> serde::de::Visitor<'de> for DuplicateKeyGuard<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON object with unique keys")
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            if !self.seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!("duplicate key `{key}`")));
            }
            // Consume (and ignore) the value.
            let _ = map.next_value::<serde::de::IgnoredAny>()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sample_order() -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_deposit_stealth_addr".to_string(),
            format!("0x{}", hex::encode([0x11u8; 20])),
        );
        order.generate_memo();
        order
    }

    #[test]
    fn sign_verify_round_trip_yields_awaiting_deposit_shell() {
        let signer = key(1);
        let federation = vec![signer.verifying_key(), key(2).verifying_key()];
        let order = sample_order();

        let env = sign_order_record_ed25519(&order, &signer).unwrap();
        let shell = env
            .verify_ed25519(&federation)
            .expect("valid record verifies");

        assert_eq!(shell.id, order.id);
        assert_eq!(shell.amount, order.amount);
        assert_eq!(shell.fee, order.fee);
        assert_eq!(shell.dest_chain, order.dest_chain);
        assert_eq!(shell.dest_address, order.dest_address);
        assert_eq!(shell.memo, order.memo.unwrap());

        // Reconstruction is ALWAYS an AwaitingDeposit shell with no trust
        // carried over the wire.
        let rebuilt = shell.into_awaiting_deposit_order();
        assert_eq!(rebuilt.id, order.id);
        assert_eq!(rebuilt.status, OrderStatus::AwaitingDeposit);
        assert!(rebuilt.mint_authorization.is_none());
        assert!(rebuilt.source_tx.is_none());
        assert!(rebuilt.dest_tx.is_none());
        assert_eq!(rebuilt.order_type, OrderType::Mint);
        // The reconstructed memo still embeds the id the watcher matches on.
        assert_eq!(
            BridgeOrder::order_id_from_memo(&rebuilt.memo.unwrap()),
            Some(order.id)
        );
    }

    #[test]
    fn record_from_unknown_signer_is_rejected() {
        // Signed by a key that is NOT in the elected federation set.
        let outsider = key(99);
        let federation = vec![key(1).verifying_key(), key(2).verifying_key()];
        let env = sign_order_record_ed25519(&sample_order(), &outsider).unwrap();
        assert_eq!(
            env.verify_ed25519(&federation).unwrap_err(),
            OrderRecordRejectReason::UnknownSigner
        );
    }

    #[test]
    fn tampered_record_body_is_rejected() {
        let signer = key(1);
        let federation = vec![signer.verifying_key()];
        let order = sample_order();
        let mut env = sign_order_record_ed25519(&order, &signer).unwrap();

        // Tamper the signed body (inflate the amount) while keeping the
        // signature — verification must fail.
        env.record = env
            .record
            .replace(&order.amount.to_string(), &(order.amount + 1).to_string());
        assert_eq!(
            env.verify_ed25519(&federation).unwrap_err(),
            OrderRecordRejectReason::BadSignature
        );
    }

    #[test]
    fn relabeled_signer_id_is_rejected() {
        // A valid signature from member A, relabeled to claim member B's id.
        // B is in the set (so not UnknownSigner) but B did not sign these
        // bytes, so verify_strict fails.
        let (a, b) = (key(1), key(2));
        let federation = vec![a.verifying_key(), b.verifying_key()];
        let mut env = sign_order_record_ed25519(&sample_order(), &a).unwrap();
        env.signer_key_id = hex::encode(b.verifying_key().as_bytes());
        assert_eq!(
            env.verify_ed25519(&federation).unwrap_err(),
            OrderRecordRejectReason::BadSignature
        );
    }

    #[test]
    fn only_awaiting_deposit_mint_orders_replicate() {
        // A confirmed mint order must never be projected into a shell — peers
        // derive confirmation independently, never from replication.
        let mut order = sample_order();
        order.set_status(OrderStatus::DepositConfirmed);
        assert!(MintOrderShell::from_order(&order).is_err());

        // Burn orders are never replicated through this path.
        let burn = BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "0xsource".to_string(),
            "bth_addr".to_string(),
            "0xburntx".to_string(),
            0,
        );
        assert!(MintOrderShell::from_order(&burn).is_err());
    }

    #[test]
    fn malformed_signature_hex_is_rejected() {
        let signer = key(1);
        let federation = vec![signer.verifying_key()];
        let mut env = sign_order_record_ed25519(&sample_order(), &signer).unwrap();
        env.signature_hex = "not-hex".to_string();
        assert!(matches!(
            env.verify_ed25519(&federation).unwrap_err(),
            OrderRecordRejectReason::Malformed(_)
        ));
    }

    #[test]
    fn record_with_duplicate_keys_is_rejected() {
        let signer = key(1);
        let federation = vec![signer.verifying_key()];
        // Craft a record with a duplicated key and sign it, so the signature
        // verifies but the parse-after-verify duplicate guard rejects it.
        let shell = MintOrderShell::from_order(&sample_order()).unwrap();
        let base = shell.canonical_json();
        let dup = base.replacen('{', "{\"fee\":1,", 1);
        let env = OrderRecordEnvelope {
            signature_hex: hex::encode(signer.sign(&signed_message(dup.as_bytes())).to_bytes()),
            signer_key_id: hex::encode(signer.verifying_key().as_bytes()),
            record: dup,
        };
        assert!(matches!(
            env.verify_ed25519(&federation).unwrap_err(),
            OrderRecordRejectReason::InvalidRecord(_)
        ));
    }

    #[test]
    fn record_memo_must_embed_its_own_id() {
        // A record whose memo does not carry its own id is malformed: no
        // watcher could ever match it, and it must not be persisted as a shell.
        let signer = key(1);
        let federation = vec![signer.verifying_key()];
        let mut shell = MintOrderShell::from_order(&sample_order()).unwrap();
        shell.memo = [0xabu8; 64]; // memo id no longer equals shell.id
        let record = shell.canonical_json();
        let env = OrderRecordEnvelope {
            signature_hex: hex::encode(signer.sign(&signed_message(record.as_bytes())).to_bytes()),
            signer_key_id: hex::encode(signer.verifying_key().as_bytes()),
            record,
        };
        assert!(matches!(
            env.verify_ed25519(&federation).unwrap_err(),
            OrderRecordRejectReason::InvalidRecord(_)
        ));
    }

    #[test]
    fn canonical_json_is_stable() {
        // The signed byte string must be reproducible field-for-field.
        let mut order = sample_order();
        // Pin every field so the vector is deterministic.
        order.id = Uuid::parse_str("00000000-0000-0000-0000-0000000000aa").unwrap();
        order.amount = 5;
        order.fee = 1;
        order.source_address = "deposit".to_string();
        order.dest_address = "dest".to_string();
        order.created_at = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
        order.generate_memo();
        let shell = MintOrderShell::from_order(&order).unwrap();
        let expected = format!(
            "{{\"amount\":5,\"bthDepositAddress\":\"deposit\",\"createdAt\":1700000000,\
             \"destAddress\":\"dest\",\"destChain\":\"ethereum\",\"fee\":1,\
             \"id\":\"00000000-0000-0000-0000-0000000000aa\",\"memo\":\"{}\"}}",
            hex::encode(order.memo.unwrap())
        );
        assert_eq!(shell.canonical_json(), expected);
    }
}
