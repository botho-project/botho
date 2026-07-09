//! Operator-signed quorum-curation write path — envelope verification and the
//! gate-routed apply outcome (#748, P4.4b of the #709 proposal).
//!
//! This is the **security core** of the operator-signed quorum-curation feature
//! (`docs/security/quorum-write-path.md` §3, §4). It owns:
//!
//! 1. The signed-action **envelope** (§3): canonical JSON, its domain-separated
//!    Ed25519 signature, and the v1 action allowlist.
//! 2. The **verifier** (§4): a fail-closed, first-failure-wins pipeline whose
//!    ordering places NO secret-dependent check before signature verification
//!    (oracle risk, §9). It reuses [`operator_key::fingerprint_hex`] (#747) for
//!    `signerKeyId` selection and [`operator_nonce::NonceStore`] (#749) for the
//!    reserve-then-apply replay check.
//! 3. The **outcome** type ([`OperatorActionOutcome`]) the apply path produces,
//!    structured so the audit log + rejected-requests counter (#750) can hook
//!    it cleanly without re-deriving anything.
//!
//! The apply itself — cloning the live `QuorumConfig`, running the EXISTING
//! gate (`gated_scp_quorum_set`), installing the resulting `QuorumSet`, and
//! persisting via `Config::save` — happens in the `commands::run` event loop
//! (§4 apply path), because it needs the consensus handle and
//! `NetworkDiscovery`, which live there. This module provides the pure,
//! unit-testable pieces the loop and the RPC handler compose; the mutation the
//! loop applies to its config clone is [`VerifiedAction::apply_to`], so the
//! "what changes" logic is tested here and the "how it is installed" logic is
//! the loop's existing gate call.
//!
//! ## Round-1 findings (security-reviewed on #708), each with a dedicated test
//!
//! - **Finding 1 — `dryRun` is a SIGNED field; parse-after-verify.** The RPC
//!   takes EXACTLY ONE argument (the envelope + signature); no sibling
//!   parameter influences processing. The verifier checks the signature over
//!   the RECEIVED canonical byte string and only then parses THOSE EXACT bytes
//!   — it never re-canonicalizes a separately-parsed object. Envelopes whose
//!   bytes parse to unknown or duplicate keys are rejected. See
//!   [`SignedEnvelope::verify_and_parse`] and the `finding1_*` tests.
//! - **Finding 2 — membership-1 hard floor, no override.** An action whose
//!   resulting membership is 1 (the node alone) is refused OUTRIGHT, with no
//!   `acknowledgeDegenerate` override; `acknowledgeDegenerate: true` is
//!   REQUIRED (and sufficient) only for edits leaving a <4-but->1-node quorum.
//!   See [`check_membership_floor`] and the `finding2_*` tests.

use crate::{
    operator_key::fingerprint_hex,
    operator_nonce::{NonceStore, ReserveOutcome},
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Serialize;
use serde_json::{Map, Value};

/// Domain separator prepended to the canonical envelope bytes before signing /
/// verifying (`docs/security/quorum-write-path.md` §3). Prevents cross-protocol
/// signature reuse: a signature over these bytes can only ever be an operator
/// action, never (say) a wallet transaction that happened to hash the same.
pub const DOMAIN_SEPARATOR: &[u8] = b"botho-operator-action-v1";

/// The only envelope version v1 verifiers accept. Unknown versions are rejected
/// with no downgrade path (§3).
pub const ENVELOPE_VERSION: u64 = 1;

/// Maximum lifetime of an envelope in seconds (`expiresAt - issuedAt <= 300`,
/// §3/§4 step 5).
pub const MAX_ENVELOPE_LIFETIME_SECS: u64 = 300;

/// Clock-skew allowance for the freshness check (`issuedAt - 30 <= now`, §4
/// step 5). Deliberately tighter than the BaaS webhook verifier — operator
/// actions are interactive and fleet nodes run NTP.
pub const CLOCK_SKEW_SECS: u64 = 30;

/// Upper bound on `max_auto_members` an operator action may set (§4 step 7:
/// "within sane bounds (0..=64)"). Mirrors the gate's own small-quorum ceiling.
pub const MAX_AUTO_MEMBERS_CEILING: u32 = 64;

/// The BFT floor: below this membership the quorum degenerates to n-of-n
/// crash-only tolerance (#509), so an edit leaving membership below it requires
/// a signed `acknowledgeDegenerate: true` (§3). Mirrors
/// `config::QuorumConfig::MIN_BFT_NODES`.
pub const MIN_BFT_NODES: usize = 4;

// ---------------------------------------------------------------------------
// v1 action allowlist (§3, verifier-level invariant)
// ---------------------------------------------------------------------------

/// The v1 action allowlist (`docs/security/quorum-write-path.md` §3). This is a
/// **verifier-level invariant, not just a scoping choice** — the mode/threshold
/// exclusion is the mitigation bounding a compromised-dashboard attack (§8.3)
/// to recoverable liveness. Any action outside this enum is rejected
/// fail-closed; adding a mode/threshold variant is gated on a fresh security
/// review (§3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    /// `quorum.pin_member` — add a base58 PeerId to `[network.quorum] members`.
    PinMember { peer_id: String },
    /// `quorum.unpin_member` — remove a base58 PeerId from the curated set.
    UnpinMember { peer_id: String },
    /// `quorum.set_max_auto_members` — set the auto-promotion cap (u32, bounded
    /// by [`MAX_AUTO_MEMBERS_CEILING`]).
    SetMaxAutoMembers { value: u32 },
}

impl ActionKind {
    /// The wire `action` string for this variant.
    pub fn name(&self) -> &'static str {
        match self {
            ActionKind::PinMember { .. } => "quorum.pin_member",
            ActionKind::UnpinMember { .. } => "quorum.unpin_member",
            ActionKind::SetMaxAutoMembers { .. } => "quorum.set_max_auto_members",
        }
    }

    /// The action's `params` object as it appears in the envelope, for the
    /// audit log (§6: refusals log the *attempted* mutation in `params`).
    /// Reconstructed from the parsed action so the audit entry never
    /// re-reads untrusted bytes.
    pub fn params_value(&self) -> Value {
        match self {
            ActionKind::PinMember { peer_id } | ActionKind::UnpinMember { peer_id } => {
                serde_json::json!({ "peerId": peer_id })
            }
            ActionKind::SetMaxAutoMembers { value } => serde_json::json!({ "value": value }),
        }
    }
}

// ---------------------------------------------------------------------------
// Rejection taxonomy
// ---------------------------------------------------------------------------

/// Why an operator action was rejected. Split so the apply path / #750 can log
/// authenticated refusals (post-signature) but only rate-limit + count
/// pre-signature failures (§6 review finding 3):
/// [`RejectReason::is_authenticated`] is the discriminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Step 1: `action_public_keys` is empty — no write surface at all.
    NotConfigured,
    /// Step 2: `signerKeyId` matched no configured pubkey (pre-signature).
    UnknownSigner,
    /// Step 1–3 structural: the request could not be parsed far enough to even
    /// attempt signature verification (missing/mis-typed transport fields,
    /// malformed signature hex). Pre-signature — unauthenticated caller.
    Malformed(String),
    /// Step 3: the signature did not verify over the canonical bytes with the
    /// domain separator (pre-signature — the caller is unauthenticated).
    BadSignature,
    /// Step 4: `targetNode` did not equal this node's PeerId (post-signature).
    WrongTarget,
    /// Step 5: freshness failed (expired, not-yet-valid beyond skew, or
    /// lifetime exceeded 300 s) (post-signature).
    Stale(String),
    /// Step 6: the nonce was already consumed — a replay (post-signature).
    ReplayedNonce,
    /// Step 7: payload validity failed — unknown/duplicate JSON keys, unknown
    /// action, unparseable peerId, out-of-range cap, or a degenerate-posture /
    /// solo-quorum policy violation (post-signature).
    InvalidPayload(String),
}

impl RejectReason {
    /// Whether this rejection is for an AUTHENTICATED request (signature
    /// already verified). Per §6 review finding 3, only authenticated
    /// outcomes are audit-logged; pre-signature failures (`false` here) are
    /// reachable by any unauthenticated caller, so #750 must only
    /// rate-limit + count them.
    pub fn is_authenticated(&self) -> bool {
        !matches!(
            self,
            RejectReason::NotConfigured
                | RejectReason::UnknownSigner
                | RejectReason::Malformed(_)
                | RejectReason::BadSignature
        )
    }

    /// A short, stable machine tag for the audit log (`verify_refused:<tag>`,
    /// §6). Contains no secret material and no attacker-controlled free text.
    pub fn tag(&self) -> &'static str {
        match self {
            RejectReason::NotConfigured => "not_configured",
            RejectReason::UnknownSigner => "unknown_signer",
            RejectReason::Malformed(_) => "malformed",
            RejectReason::BadSignature => "bad_signature",
            RejectReason::WrongTarget => "wrong_target",
            RejectReason::Stale(_) => "stale",
            RejectReason::ReplayedNonce => "replayed_nonce",
            RejectReason::InvalidPayload(_) => "invalid_payload",
        }
    }

    /// A human-facing message (safe to return to the RPC caller). Constant per
    /// tag except for the payload-detail cases, whose detail is node-derived
    /// (never attacker free-text echoed back verbatim beyond a bounded reason).
    pub fn message(&self) -> String {
        match self {
            RejectReason::NotConfigured => "operator actions not configured".to_string(),
            RejectReason::UnknownSigner => "unknown signer".to_string(),
            RejectReason::Malformed(d) => format!("malformed request: {d}"),
            RejectReason::BadSignature => "signature verification failed".to_string(),
            RejectReason::WrongTarget => "envelope targetNode does not match this node".to_string(),
            RejectReason::Stale(d) => format!("envelope not fresh: {d}"),
            RejectReason::ReplayedNonce => "nonce already used (replay)".to_string(),
            RejectReason::InvalidPayload(d) => format!("invalid payload: {d}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Signed envelope: the ONE argument operator_submitAction accepts
// ---------------------------------------------------------------------------

/// The exactly-one argument `operator_submitAction` accepts (§4 finding 1): the
/// canonical envelope bytes and the detached signature over them. NOTHING else
/// is read from the RPC request — `dryRun` and every other field live INSIDE
/// the signed `envelope` string.
///
/// `envelope` is the raw UTF-8 canonical JSON string exactly as the operator
/// signed it. The verifier signs-checks THESE bytes (prefixed with the domain
/// separator) and only then parses them — it never re-serializes a parsed
/// object, so there is no canonicalization round-trip the operator's signature
/// might disagree with (finding 1).
#[derive(Debug, Clone)]
pub struct SignedEnvelope {
    /// The canonical JSON envelope string, verbatim, as signed.
    pub envelope: String,
    /// The detached Ed25519 signature over `DOMAIN_SEPARATOR || envelope`,
    /// lowercase hex (64 bytes → 128 hex chars).
    pub signature_hex: String,
}

/// A fully-parsed, structurally-validated envelope (after signature
/// verification). Produced by [`SignedEnvelope::verify_and_parse`]; consumed by
/// the payload-policy + nonce steps.
#[derive(Debug, Clone)]
pub struct ParsedEnvelope {
    pub action: ActionKind,
    pub dry_run: bool,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: String,
    pub signer_key_id: String,
    pub target_node: String,
    pub acknowledge_degenerate: bool,
}

impl SignedEnvelope {
    /// Extract the signed envelope from the single RPC `params` object. The
    /// ONLY two fields read are `envelope` (the canonical JSON string) and
    /// `signature`. Any other top-level param is IGNORED (it cannot influence
    /// processing — finding 1). A missing/mis-typed field is a pre-signature
    /// [`RejectReason::Malformed`].
    pub fn from_params(params: &Value) -> Result<Self, RejectReason> {
        let obj = params
            .as_object()
            .ok_or_else(|| RejectReason::Malformed("params must be an object".to_string()))?;
        let envelope = obj
            .get("envelope")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RejectReason::Malformed("missing string field `envelope`".to_string()))?
            .to_string();
        let signature_hex = obj
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RejectReason::Malformed("missing string field `signature`".to_string()))?
            .to_string();
        Ok(Self {
            envelope,
            signature_hex,
        })
    }

    /// Verify the signature over the RECEIVED canonical bytes (with the domain
    /// separator) against `verifying_key`, then — and ONLY then — parse those
    /// exact bytes into a [`ParsedEnvelope`] (finding 1: parse-after-verify).
    ///
    /// This is verification steps 3 and 7-structural. Steps 1/2 (config gate,
    /// signer selection) happen in [`verify_and_apply_prelude`] BEFORE this is
    /// called, because they select which `verifying_key` to hand in — but note
    /// neither of those steps depends on secret data (they are a non-empty
    /// check and a fingerprint-equality check), so no secret-dependent
    /// check precedes signature verification (§9 oracle-risk item).
    pub fn verify_and_parse(
        &self,
        verifying_key: &VerifyingKey,
    ) -> Result<ParsedEnvelope, RejectReason> {
        // --- Signature (step 3) over the RECEIVED bytes. --------------------
        let sig_bytes = hex::decode(self.signature_hex.trim())
            .map_err(|_| RejectReason::Malformed("signature is not valid hex".to_string()))?;
        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| RejectReason::Malformed("signature must be 64 bytes".to_string()))?;
        let signature = Signature::from_bytes(&sig_arr);

        let mut signed_message =
            Vec::with_capacity(DOMAIN_SEPARATOR.len() + self.envelope.as_bytes().len());
        signed_message.extend_from_slice(DOMAIN_SEPARATOR);
        signed_message.extend_from_slice(self.envelope.as_bytes());

        verifying_key
            .verify(&signed_message, &signature)
            .map_err(|_| RejectReason::BadSignature)?;

        // --- Parse-after-verify (step 7 structural). -----------------------
        // The signature is valid over exactly these bytes; NOW parse them. We
        // never re-canonicalize a separately-parsed object.
        parse_canonical_envelope(&self.envelope).map_err(RejectReason::InvalidPayload)
    }
}

/// Parse the canonical envelope bytes into a [`ParsedEnvelope`], REJECTING
/// unknown or duplicate top-level keys (finding 1) and any type/shape error.
///
/// `serde_json` silently accepts duplicate object keys (last-wins), which would
/// let two different logical envelopes share one signed byte string. We defeat
/// that by parsing into a raw map with a manual duplicate-key check, then
/// validating the exact allowed key set — so an envelope with an unexpected or
/// repeated key is refused rather than reinterpreted.
fn parse_canonical_envelope(bytes: &str) -> Result<ParsedEnvelope, String> {
    // Duplicate-key detection: serde_json::Map keeps only the last value for a
    // duplicated key, so a Map alone cannot see duplicates. Re-scan with the
    // streaming deserializer's `MapAccess`-free trick: deserialize to Value but
    // first confirm no key repeats by a raw pass.
    reject_duplicate_top_level_keys(bytes)?;

    let value: Value =
        serde_json::from_str(bytes).map_err(|e| format!("envelope is not valid JSON: {e}"))?;
    let obj: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| "envelope must be a JSON object".to_string())?;

    // Unknown-key rejection (finding 1): every key must be a known v1 field.
    const KNOWN_KEYS: &[&str] = &[
        "action",
        "dryRun",
        "expiresAt",
        "issuedAt",
        "nonce",
        "params",
        "signerKeyId",
        "targetNode",
        "v",
        "acknowledgeDegenerate",
    ];
    for key in obj.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!("unknown envelope field `{key}`"));
        }
    }

    // Version (step: `v` must equal 1 exactly — no downgrade path).
    let v = get_u64(obj, "v")?;
    if v != ENVELOPE_VERSION {
        return Err(format!(
            "unsupported envelope version {v} (expected {ENVELOPE_VERSION})"
        ));
    }

    // Mandatory scalar fields.
    let dry_run = obj
        .get("dryRun")
        .ok_or_else(|| "missing field `dryRun`".to_string())?
        .as_bool()
        .ok_or_else(|| "`dryRun` must be a boolean".to_string())?;
    let issued_at = get_u64(obj, "issuedAt")?;
    let expires_at = get_u64(obj, "expiresAt")?;
    let nonce = get_str(obj, "nonce")?.to_string();
    let signer_key_id = get_str(obj, "signerKeyId")?.to_string();
    let target_node = get_str(obj, "targetNode")?.to_string();
    let action_name = get_str(obj, "action")?.to_string();

    // Optional field: acknowledgeDegenerate defaults to false when absent.
    let acknowledge_degenerate = match obj.get("acknowledgeDegenerate") {
        None => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| "`acknowledgeDegenerate` must be a boolean".to_string())?,
    };

    // `params` must be present (the canonical envelope always includes it, even
    // if empty for actions with no params) and an object.
    let params_obj = obj
        .get("params")
        .ok_or_else(|| "missing field `params`".to_string())?
        .as_object()
        .ok_or_else(|| "`params` must be an object".to_string())?;

    // Map the action string + params to a v1 ActionKind (fail-closed on
    // anything outside the allowlist — a verifier-level invariant, §3).
    let action = parse_action(&action_name, params_obj)?;

    Ok(ParsedEnvelope {
        action,
        dry_run,
        issued_at,
        expires_at,
        nonce,
        signer_key_id,
        target_node,
        acknowledge_degenerate,
    })
}

/// Reject any object (at the top level) that contains a duplicated key. This is
/// a raw structural pass because `serde_json::Map` collapses duplicates.
fn reject_duplicate_top_level_keys(bytes: &str) -> Result<(), String> {
    use serde::de::{Deserializer, MapAccess, Visitor};

    struct DupKeyVisitor;
    impl<'de> Visitor<'de> for DupKeyVisitor {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON object with unique keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut seen = std::collections::HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                // Consume the value so the deserializer keeps advancing.
                let _: serde::de::IgnoredAny = map.next_value()?;
                if !seen.insert(key.clone()) {
                    return Err(serde::de::Error::custom(format!("duplicate key `{key}`")));
                }
            }
            Ok(())
        }
    }

    let mut de = serde_json::Deserializer::from_str(bytes);
    de.deserialize_map(DupKeyVisitor)
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

/// Map an `action` string + its `params` object to a v1 [`ActionKind`], failing
/// closed on any action outside the allowlist (§3/§4 step 7).
fn parse_action(action: &str, params: &Map<String, Value>) -> Result<ActionKind, String> {
    match action {
        "quorum.pin_member" => {
            let peer_id = get_str(params, "peerId")?.to_string();
            Ok(ActionKind::PinMember { peer_id })
        }
        "quorum.unpin_member" => {
            let peer_id = get_str(params, "peerId")?.to_string();
            Ok(ActionKind::UnpinMember { peer_id })
        }
        "quorum.set_max_auto_members" => {
            let value = get_u64(params, "value")?;
            if value > MAX_AUTO_MEMBERS_CEILING as u64 {
                return Err(format!(
                    "maxAutoMembers {value} exceeds ceiling {MAX_AUTO_MEMBERS_CEILING}"
                ));
            }
            Ok(ActionKind::SetMaxAutoMembers {
                value: value as u32,
            })
        }
        other => Err(format!("action `{other}` is not in the v1 allowlist")),
    }
}

fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .ok_or_else(|| format!("missing field `{key}`"))?
        .as_str()
        .ok_or_else(|| format!("`{key}` must be a string"))
}

/// Read an integer field, REJECTING non-integers (§3: "integers only — no
/// floats"). `serde_json` parses `5.0` as a float, so `as_u64` returns `None`
/// for it — exactly the fail-closed behavior we want.
fn get_u64(obj: &Map<String, Value>, key: &str) -> Result<u64, String> {
    let v = obj
        .get(key)
        .ok_or_else(|| format!("missing field `{key}`"))?;
    v.as_u64()
        .ok_or_else(|| format!("`{key}` must be a non-negative integer"))
}

/// Peek the `signerKeyId` out of the (still-unverified) envelope bytes, ONLY to
/// select which configured pubkey to verify against. This reads an untrusted
/// string, but it drives NO security decision on its own: the signature is
/// subsequently checked over the exact received bytes, and if the presented
/// `signerKeyId` names a key the bytes were not actually signed with, the
/// signature check fails (step 3) and the envelope is rejected. It is purely a
/// key-selection hint. The authoritative `signerKeyId` used everywhere else
/// comes from the post-verification [`ParsedEnvelope`].
///
/// Returns [`RejectReason::Malformed`] (pre-signature, unauthenticated) if the
/// bytes are not a JSON object with a string `signerKeyId`.
pub fn peek_signer_key_id(envelope: &str) -> Result<String, RejectReason> {
    let value: Value = serde_json::from_str(envelope)
        .map_err(|_| RejectReason::Malformed("envelope is not valid JSON".to_string()))?;
    value
        .as_object()
        .and_then(|o| o.get("signerKeyId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| RejectReason::Malformed("envelope missing string `signerKeyId`".to_string()))
}

// ---------------------------------------------------------------------------
// Verification steps 1, 2 (config gate + signer selection) — no secrets
// ---------------------------------------------------------------------------

/// Select the verifying key for `signer_key_id` out of the configured
/// `action_public_keys`, performing steps 1 (config gate) and 2 (signer known).
///
/// Neither step depends on secret data: step 1 is a non-empty check and step 2
/// is a fingerprint-equality scan over PUBLIC keys — so this places no
/// secret-dependent check ahead of signature verification (§9 oracle-risk
/// item).
///
/// `action_public_keys` are lowercase-hex Ed25519 public keys, provisioned from
/// config (SSH trust domain). A key that fails to decode is skipped (a
/// mis-provisioned entry must not brick the whole surface); if NO configured
/// key matches the presented `signerKeyId`, this is
/// [`RejectReason::UnknownSigner`].
pub fn select_verifying_key(
    action_public_keys: &[String],
    signer_key_id: &str,
) -> Result<VerifyingKey, RejectReason> {
    // Step 1: config gate.
    if action_public_keys.is_empty() {
        return Err(RejectReason::NotConfigured);
    }

    // Step 2: signer known — find the configured pubkey whose fingerprint equals
    // the presented signerKeyId, and return its parsed VerifyingKey.
    for key_hex in action_public_keys {
        let Ok(bytes) = hex::decode(key_hex.trim()) else {
            continue;
        };
        let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) else {
            continue;
        };
        if fingerprint_hex(&arr) != signer_key_id {
            continue;
        }
        // Fingerprint matches; the pubkey must be a valid Ed25519 point.
        match VerifyingKey::from_bytes(&arr) {
            Ok(vk) => return Ok(vk),
            Err(_) => continue,
        }
    }
    Err(RejectReason::UnknownSigner)
}

// ---------------------------------------------------------------------------
// Verification steps 4, 5 (target binding, freshness) — post-signature
// ---------------------------------------------------------------------------

/// Step 4: `targetNode` must equal this node's own PeerId (§4). Binds the
/// envelope to exactly one node so a captured envelope cannot be replayed
/// against a different node.
pub fn check_target(parsed: &ParsedEnvelope, local_peer_id: &str) -> Result<(), RejectReason> {
    if parsed.target_node == local_peer_id {
        Ok(())
    } else {
        Err(RejectReason::WrongTarget)
    }
}

/// Step 5: freshness — `issuedAt - 30 <= now <= expiresAt` and
/// `expiresAt - issuedAt <= 300` (§4). All arithmetic is saturating so a
/// crafted `issuedAt`/`expiresAt` can never panic.
pub fn check_freshness(parsed: &ParsedEnvelope, now: u64) -> Result<(), RejectReason> {
    if parsed.expires_at < parsed.issued_at {
        return Err(RejectReason::Stale(
            "expiresAt precedes issuedAt".to_string(),
        ));
    }
    if parsed.expires_at.saturating_sub(parsed.issued_at) > MAX_ENVELOPE_LIFETIME_SECS {
        return Err(RejectReason::Stale(format!(
            "lifetime exceeds {MAX_ENVELOPE_LIFETIME_SECS}s"
        )));
    }
    if now < parsed.issued_at.saturating_sub(CLOCK_SKEW_SECS) {
        return Err(RejectReason::Stale("issuedAt is in the future".to_string()));
    }
    if now > parsed.expires_at {
        return Err(RejectReason::Stale("expired".to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification step 7 (payload policy): peerId parse, degenerate/solo floor
// ---------------------------------------------------------------------------

/// A verified action ready to apply. Constructed by
/// [`VerifiedAction::check_payload`] (verification step 7, structural), which
/// validates the payload SHAPE (peerId parse). The membership floors (finding 2
/// + degenerate posture) are evaluated SEPARATELY by [`check_membership_floor`]
/// against the AUTHORITATIVE resulting membership the gate computes, because in
/// `Recommended` mode connected auto-peers count toward membership and only the
/// gate knows the exact figure (§3: "evaluated at apply time against the
/// then-connected peer set").
#[derive(Debug, Clone)]
pub struct VerifiedAction {
    pub parsed: ParsedEnvelope,
}

impl VerifiedAction {
    /// Verification step 7 SHAPE check (§4): the payload is structurally valid.
    ///
    /// - `pin_member` / `unpin_member`: the `peerId` must parse as a base58
    ///   PeerId (mirroring the gate's parse-and-warn at run.rs:2279).
    /// - `set_max_auto_members`: the value was already range-checked at parse
    ///   time (0..=64).
    ///
    /// The membership-1 hard floor (finding 2) and degenerate-posture rule are
    /// NOT checked here — they need the gate's authoritative
    /// resulting-membership count and so run in [`check_membership_floor`]
    /// after the gate evaluates.
    pub fn check_payload(parsed: ParsedEnvelope) -> Result<VerifiedAction, RejectReason> {
        match &parsed.action {
            ActionKind::PinMember { peer_id } | ActionKind::UnpinMember { peer_id } => {
                if peer_id.parse::<libp2p::PeerId>().is_err() {
                    return Err(RejectReason::InvalidPayload(
                        "peerId is not a base58 PeerId".to_string(),
                    ));
                }
            }
            ActionKind::SetMaxAutoMembers { .. } => {}
        }
        Ok(VerifiedAction { parsed })
    }

    /// Apply this action's mutation to a `[network.quorum]` clone (the event
    /// loop's config clone). This is the ONLY mutation the write path performs
    /// on quorum inputs; the resulting clone is then handed to the EXISTING
    /// gate (`gated_scp_quorum_set`) — there is NO second QuorumSet
    /// constructor.
    ///
    /// Returns whether the mutation actually changed anything (an unpin of an
    /// absent member, or a cap set to its current value, is a no-op the caller
    /// may still gate + persist for idempotence, but the flag lets #750 record
    /// it accurately).
    pub fn apply_to(&self, quorum: &mut crate::config::QuorumConfig) -> bool {
        match &self.parsed.action {
            ActionKind::PinMember { peer_id } => {
                if quorum.members.iter().any(|m| m == peer_id) {
                    false
                } else {
                    quorum.members.push(peer_id.clone());
                    true
                }
            }
            ActionKind::UnpinMember { peer_id } => {
                let before = quorum.members.len();
                quorum.members.retain(|m| m != peer_id);
                quorum.members.len() != before
            }
            ActionKind::SetMaxAutoMembers { value } => {
                if quorum.max_auto_members == *value {
                    false
                } else {
                    quorum.max_auto_members = *value;
                    true
                }
            }
        }
    }
}

/// Evaluate the membership floors against the AUTHORITATIVE membership counts
/// the gate computed (§3, "evaluated at apply time against the then-connected
/// peer set"). `previous_membership` is the quorum-set member count BEFORE the
/// edit; `resulting_membership` is what the gate would install AFTER it. Both
/// INCLUDE self.
///
/// Rules:
/// - **Membership-1 hard floor (finding 2):** a resulting membership of 1 (the
///   node alone) is refused OUTRIGHT — no `acknowledgeDegenerate` override. A
///   1-of-1 quorum trivially passes the gate's intersection check yet lets the
///   node self-fork, which the gate's node-local FBAS model cannot see (§3).
/// - **Degenerate posture (§3):** a resulting membership >1 but <4 (below the
///   BFT floor) requires a signed `acknowledgeDegenerate: true` — but ONLY when
///   the edit SHRINKS membership into (or further within) the degenerate band.
///   An edit that grows or holds membership (e.g. pinning a new member) never
///   needs the acknowledgment, even if the result is still small.
pub fn check_membership_floor(
    parsed: &ParsedEnvelope,
    previous_membership: usize,
    resulting_membership: usize,
) -> Result<(), RejectReason> {
    // Membership-1 hard floor: never allowed, no override.
    if resulting_membership <= 1 {
        return Err(RejectReason::InvalidPayload(
            "action would leave a solo (membership-1) quorum — refused outright \
             (no acknowledgeDegenerate override; membership 1 is never allowed)"
                .to_string(),
        ));
    }

    // Degenerate posture: <4 AND the edit shrinks membership → require ack.
    let shrinks = resulting_membership < previous_membership;
    if resulting_membership < MIN_BFT_NODES && shrinks && !parsed.acknowledge_degenerate {
        return Err(RejectReason::InvalidPayload(format!(
            "action would leave a degenerate ({resulting_membership}-node, below the \
             {MIN_BFT_NODES}-node BFT floor) quorum; set acknowledgeDegenerate:true in \
             the signed envelope to proceed"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Nonce step (step 6) — reserve-then-apply, #749
// ---------------------------------------------------------------------------

/// Verification step 6 (§4): reserve the nonce (reserve-then-apply, #749), so a
/// crash between reserve and apply fails safe (the envelope can never apply
/// twice). Dry runs MUST NOT call this (§5) — the caller checks `dry_run`
/// first.
///
/// Returns `Ok(())` on a fresh reservation, [`RejectReason::ReplayedNonce`] on
/// a replay, and a wrapped error if the durable write itself failed (the caller
/// must treat a non-durable reservation as a hard failure, not an apply-ok).
pub fn reserve_nonce(
    store: &mut NonceStore,
    parsed: &ParsedEnvelope,
    now: u64,
) -> Result<(), OperatorActionError> {
    match store.reserve(&parsed.signer_key_id, &parsed.nonce, parsed.expires_at, now) {
        Ok(ReserveOutcome::Reserved) => Ok(()),
        Ok(ReserveOutcome::Replay) => {
            Err(OperatorActionError::Rejected(RejectReason::ReplayedNonce))
        }
        Err(e) => Err(OperatorActionError::NonceStore(e.to_string())),
    }
}

/// An error the apply path can surface distinct from a clean rejection: a
/// non-durable nonce reservation or another infrastructure failure. Kept
/// separate from [`RejectReason`] so #750 logs infra failures differently from
/// authenticated verification refusals.
#[derive(Debug, Clone)]
pub enum OperatorActionError {
    /// A verification/policy rejection (the normal "no" outcome).
    Rejected(RejectReason),
    /// The nonce store could not durably persist the reservation.
    NonceStore(String),
}

// ---------------------------------------------------------------------------
// Apply-channel request — the (envelope, responder) the RPC sends the loop
// ---------------------------------------------------------------------------

/// The message the RPC handler sends over the bounded mpsc channel into the
/// `commands::run` event loop (§4 apply path, mirroring the #674 `tx_relay`
/// seam). It carries the already-verified envelope plus a oneshot responder the
/// loop uses to return the [`OperatorActionOutcome`] synchronously to the RPC
/// caller.
///
/// The RPC handler performs the SECRET-FREE verification (steps 1–5: config
/// gate, signer selection, signature, target, freshness) BEFORE sending, so the
/// event loop only ever receives an authenticated, fresh, correctly-targeted
/// envelope. The loop owns steps 6 (nonce) and 7-apply (payload policy against
/// the live peer set, the gate, install, persist) because those need the live
/// `NonceStore`, connected-peer set, consensus handle, and config — all of
/// which live in the loop.
pub struct OperatorActionRequest {
    /// The verified, parsed envelope (post steps 1–5).
    pub parsed: ParsedEnvelope,
    /// Oneshot channel back to the RPC handler for the outcome.
    pub responder: tokio::sync::oneshot::Sender<OperatorActionOutcome>,
}

// ---------------------------------------------------------------------------
// Outcome type — the structured result #750 hooks the audit log onto
// ---------------------------------------------------------------------------

/// The verdict of an operator action: applied, gate-refused, or verify-refused.
/// This is the single structured value the event loop returns to the RPC
/// handler AND the seam #750 attaches the JSONL audit append + counter to — it
/// carries everything §6's audit entry needs (signer, action, dryRun, outcome,
/// prev/new quorum, gate snapshot) without #750 re-deriving anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorActionOutcome {
    /// The terminal outcome class.
    pub outcome: OutcomeClass,
    /// Whether this was a dry run (steps 1–4 only; never mutated/persisted).
    pub dry_run: bool,
    /// Signer fingerprint (`signerKeyId`) for AUTHENTICATED outcomes; `None`
    /// for pre-signature rejections (which #750 must NOT audit-log, §6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_key_id: Option<String>,
    /// The attempted action name (`quorum.pin_member`, …), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// The attempted action's `params` object, for the audit entry (§6:
    /// refusals log the *attempted* mutation). `None` for pre-signature
    /// rejections (no parsed action). Not serialized on the RPC wire (the
    /// caller already has the signed envelope); used only to build the
    /// audit entry.
    #[serde(skip)]
    pub audit_params: Option<Value>,
    /// Human-facing detail (rejection reason / applied summary).
    pub message: String,
    /// For refusals: the machine tag (`gate_refused` | `verify_refused:<tag>`);
    /// for applies, `applied`. #750 writes this verbatim into the audit entry.
    pub audit_tag: String,
    /// Whether this outcome is AUTHENTICATED (signature verified). #750 only
    /// audit-logs authenticated outcomes; unauthenticated ones are rate-limited
    /// + counted (§6 review finding 3).
    pub authenticated: bool,
    /// The `[network.quorum]` posture BEFORE the edit, when known (set by the
    /// event loop, which alone sees the live config). Feeds the audit entry's
    /// `prevQuorum` (§6). `None` for pre-gate handler-side rejections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_quorum: Option<QuorumPosture>,
    /// The resulting `[network.quorum]` posture — present for `applied`, and
    /// for `dryRun` previews it is the HYPOTHETICAL resulting posture;
    /// `None` for refusals (no new state exists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resulting_quorum: Option<QuorumPosture>,
    /// The gate snapshot for this evaluation (intersection verdict + member
    /// counts), when the gate ran. `None` for pre-gate rejections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateVerdict>,
}

/// The terminal outcome class of an operator action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeClass {
    /// The gate accepted; the edit was installed + persisted (or, for a dry
    /// run, WOULD BE accepted).
    Applied,
    /// The gate refused (intersection violation) — previous set kept, nothing
    /// persisted.
    GateRefused,
    /// A verification/policy check refused before the gate ran.
    VerifyRefused,
}

/// A compact view of a resulting `[network.quorum]` posture, for the outcome
/// and #750's audit entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuorumPosture {
    pub mode: String,
    pub members: Vec<String>,
    pub max_auto_members: u32,
}

impl QuorumPosture {
    /// Snapshot a `[network.quorum]` config into a posture view.
    pub fn from_config(quorum: &crate::config::QuorumConfig) -> Self {
        let mode = match quorum.mode {
            crate::config::QuorumMode::Explicit => "explicit",
            crate::config::QuorumMode::Recommended => "recommended",
        }
        .to_string();
        Self {
            mode,
            members: quorum.members.clone(),
            max_auto_members: quorum.max_auto_members,
        }
    }
}

/// The gate's verdict for one evaluation, mirrored from
/// `consensus::QuorumGateSnapshot` into a serializable shape for the outcome
/// and #750's audit entry. (We do not serialize the whole snapshot to keep the
/// RPC shape stable and to avoid the per-peer targeting map on the write
/// surface.)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateVerdict {
    pub intersection_refused: bool,
    pub curated_members: usize,
    pub auto_members: usize,
    pub suppressed_peers: usize,
    pub max_auto_members: u32,
    /// Fault-tolerant posture: membership >= 4 (BFT floor). Convenience for the
    /// dashboard; derived from the counts, not a new gate concept.
    pub fault_tolerant: bool,
    /// Degenerate posture: membership < 4. The complement of `fault_tolerant`.
    pub degenerate: bool,
}

impl GateVerdict {
    /// Build a verdict from a gate snapshot (`curated_members` + `auto_members`
    /// + self = membership).
    pub fn from_snapshot(snap: &crate::consensus::QuorumGateSnapshot) -> Self {
        let membership = 1 + snap.curated_members + snap.auto_members;
        Self {
            intersection_refused: snap.intersection_refused,
            curated_members: snap.curated_members,
            auto_members: snap.auto_members,
            suppressed_peers: snap.suppressed_peers,
            max_auto_members: snap.max_auto_members,
            fault_tolerant: membership >= MIN_BFT_NODES,
            degenerate: membership < MIN_BFT_NODES,
        }
    }
}

impl OperatorActionOutcome {
    /// Build a rejection outcome from a [`RejectReason`], carrying the signer /
    /// action only when the request was authenticated (so #750 never audit-logs
    /// a pre-signature failure, §6). `parsed` is `Some` once the envelope
    /// parsed (i.e. after signature verification), giving the signer +
    /// action.
    pub fn rejected(reason: &RejectReason, parsed: Option<&ParsedEnvelope>) -> Self {
        let authenticated = reason.is_authenticated();
        let (signer_key_id, action, audit_params, dry_run) = match parsed {
            Some(p) => (
                Some(p.signer_key_id.clone()),
                Some(p.action.name().to_string()),
                Some(p.action.params_value()),
                p.dry_run,
            ),
            None => (None, None, None, false),
        };
        Self {
            outcome: OutcomeClass::VerifyRefused,
            dry_run,
            signer_key_id,
            action,
            audit_params,
            message: reason.message(),
            audit_tag: format!("verify_refused:{}", reason.tag()),
            authenticated,
            prev_quorum: None,
            resulting_quorum: None,
            gate: None,
        }
    }

    /// Build a gate-refusal outcome (intersection violation): authenticated,
    /// nothing persisted, previous set kept.
    pub fn gate_refused(parsed: &ParsedEnvelope, gate: GateVerdict) -> Self {
        Self {
            outcome: OutcomeClass::GateRefused,
            dry_run: parsed.dry_run,
            signer_key_id: Some(parsed.signer_key_id.clone()),
            action: Some(parsed.action.name().to_string()),
            audit_params: Some(parsed.action.params_value()),
            message: "quorum promotion gate refused the edit (would admit disjoint quorums — \
                      fork risk); previous quorum set kept, nothing persisted"
                .to_string(),
            audit_tag: "gate_refused".to_string(),
            authenticated: true,
            prev_quorum: None,
            resulting_quorum: None,
            gate: Some(gate),
        }
    }

    /// Build an applied outcome (gate accepted). For a dry run this is the
    /// hypothetical verdict; `resulting_quorum` is the posture the edit WOULD
    /// produce.
    pub fn applied(parsed: &ParsedEnvelope, resulting: QuorumPosture, gate: GateVerdict) -> Self {
        let message = if parsed.dry_run {
            "dry run: gate would accept the edit (not applied, not persisted, nonce not consumed)"
                .to_string()
        } else {
            "operator action applied: quorum edit installed and persisted".to_string()
        };
        Self {
            outcome: OutcomeClass::Applied,
            dry_run: parsed.dry_run,
            signer_key_id: Some(parsed.signer_key_id.clone()),
            action: Some(parsed.action.name().to_string()),
            audit_params: Some(parsed.action.params_value()),
            message,
            audit_tag: "applied".to_string(),
            authenticated: true,
            prev_quorum: None,
            resulting_quorum: Some(resulting),
            gate: Some(gate),
        }
    }

    /// Attach the pre-edit quorum posture (`prevQuorum`, §6). Only the event
    /// loop knows the live config, so it sets this on the outcome before the
    /// audit hook reads it. A no-op for outcomes that have no meaningful prior
    /// state (pre-gate handler rejections).
    pub fn with_prev_quorum(mut self, prev: QuorumPosture) -> Self {
        self.prev_quorum = Some(prev);
        self
    }

    /// Build the §6 audit entry for this AUTHENTICATED outcome.
    ///
    /// The caller supplies `envelope_hash` (the `blake2b-256` hex of the
    /// canonical signed envelope bytes — computed via
    /// [`crate::operator_key::blake2b_256_hex`], the one blake2b helper) and
    /// the wall-clock `ts`, because the outcome itself is
    /// transport-agnostic. Every other field is derived from the outcome —
    /// #750 re-derives nothing.
    ///
    /// Returns `None` for a pre-signature (unauthenticated) outcome: those are
    /// NEVER audit-logged (finding 3); the caller counts them instead.
    /// `newQuorum` is populated ONLY for a real `applied` outcome (not dry
    /// runs, which install nothing).
    pub fn to_audit_entry(
        &self,
        envelope_hash: String,
        ts: u64,
    ) -> Option<crate::rpc::OperatorAuditEntry> {
        if !self.authenticated {
            return None;
        }
        let to_val = |p: &QuorumPosture| serde_json::to_value(p).ok();
        // newQuorum only for a REAL applied edit (§6): a dry run installs
        // nothing, and refusals have no new state.
        let new_quorum = if self.outcome == OutcomeClass::Applied && !self.dry_run {
            self.resulting_quorum.as_ref().and_then(to_val)
        } else {
            None
        };
        Some(crate::rpc::OperatorAuditEntry {
            ts,
            signer_key_id: self.signer_key_id.clone().unwrap_or_default(),
            envelope_hash,
            action: self.action.clone().unwrap_or_default(),
            params: self.audit_params.clone().unwrap_or(Value::Null),
            dry_run: self.dry_run,
            outcome: self.audit_tag.clone(),
            prev_quorum: self.prev_quorum.as_ref().and_then(to_val),
            new_quorum,
            gate: self
                .gate
                .as_ref()
                .and_then(|g| serde_json::to_value(g).ok()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{QuorumConfig, QuorumMode};
    use ed25519_dalek::{Signer, SigningKey};

    /// Deterministic signing key for tests (fixed seed → stable fingerprint).
    fn test_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn pubkey_hex(sk: &SigningKey) -> String {
        hex::encode(sk.verifying_key().to_bytes())
    }

    fn signer_id(sk: &SigningKey) -> String {
        fingerprint_hex(&sk.verifying_key().to_bytes())
    }

    /// Build a canonical envelope string (keys sorted lexicographically, no
    /// insignificant whitespace, integers only) for the given fields, then sign
    /// it. Returns `(SignedEnvelope, canonical_string)`.
    ///
    /// The key ordering here is the CANONICAL order the operator's signer would
    /// produce; the verifier does not depend on this test builder — it verifies
    /// over whatever bytes it receives.
    fn build_signed(
        sk: &SigningKey,
        action: &str,
        params_json: &str,
        target: &str,
        issued_at: u64,
        expires_at: u64,
        nonce: &str,
        dry_run: bool,
        acknowledge_degenerate: Option<bool>,
    ) -> SignedEnvelope {
        // Canonical field order (lexicographic): acknowledgeDegenerate (when
        // present) sorts first, then action, dryRun, expiresAt, issuedAt, nonce,
        // params, signerKeyId, targetNode, v.
        let ack_lead = match acknowledge_degenerate {
            Some(b) => format!("\"acknowledgeDegenerate\":{b},"),
            None => String::new(),
        };
        let canonical = format!(
            "{{{ack_lead}\"action\":\"{action}\",\"dryRun\":{dry_run},\
             \"expiresAt\":{expires_at},\"issuedAt\":{issued_at},\"nonce\":\"{nonce}\",\
             \"params\":{params_json},\"signerKeyId\":\"{signer}\",\
             \"targetNode\":\"{target}\",\"v\":1}}",
            signer = signer_id(sk),
        );
        sign_str(sk, &canonical)
    }

    fn sign_str(sk: &SigningKey, canonical: &str) -> SignedEnvelope {
        let mut msg = Vec::new();
        msg.extend_from_slice(DOMAIN_SEPARATOR);
        msg.extend_from_slice(canonical.as_bytes());
        let sig = sk.sign(&msg);
        SignedEnvelope {
            envelope: canonical.to_string(),
            signature_hex: hex::encode(sig.to_bytes()),
        }
    }

    // A valid base58 PeerId (this node's own, used as targetNode) and a few
    // distinct peer IDs for members. Generated from libp2p ed25519 keys.
    fn a_peer_id(seed: u8) -> String {
        let kp = libp2p::identity::Keypair::ed25519_from_bytes([seed; 32]).unwrap();
        libp2p::PeerId::from(kp.public()).to_string()
    }

    fn now() -> u64 {
        1_800_000_000
    }

    fn keys_for(sk: &SigningKey) -> Vec<String> {
        vec![pubkey_hex(sk)]
    }

    // -- Verification-order rejection tests (one per case, AC line 1) --------

    #[test]
    fn not_configured_when_action_public_keys_empty() {
        let sk = test_key(1);
        let err = select_verifying_key(&[], &signer_id(&sk)).unwrap_err();
        assert_eq!(err, RejectReason::NotConfigured);
        assert!(!err.is_authenticated(), "not-configured is pre-signature");
    }

    #[test]
    fn unknown_signer_rejected() {
        let configured = test_key(1);
        let attacker = test_key(99);
        // signerKeyId that matches no configured key.
        let err = select_verifying_key(&keys_for(&configured), &signer_id(&attacker)).unwrap_err();
        assert_eq!(err, RejectReason::UnknownSigner);
        assert!(!err.is_authenticated(), "unknown-signer is pre-signature");
    }

    #[test]
    fn bad_signature_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let mut env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        // Corrupt the signature.
        env.signature_hex.replace_range(0..2, "ff");
        let vk = sk.verifying_key();
        let err = env.verify_and_parse(&vk).unwrap_err();
        assert_eq!(err, RejectReason::BadSignature);
        assert!(!err.is_authenticated());
    }

    #[test]
    fn tampered_envelope_bytes_fail_signature() {
        // Modifying ANY field after signing breaks the signature (finding 1: the
        // node acts only on the signed canonical bytes).
        let sk = test_key(1);
        let target = a_peer_id(200);
        let mut env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        // Flip dryRun in the raw bytes without re-signing.
        env.envelope = env.envelope.replace("\"dryRun\":false", "\"dryRun\":true");
        let vk = sk.verifying_key();
        assert_eq!(
            env.verify_and_parse(&vk).unwrap_err(),
            RejectReason::BadSignature
        );
    }

    #[test]
    fn wrong_target_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let parsed = env.verify_and_parse(&sk.verifying_key()).unwrap();
        // Bind to a DIFFERENT local peer id.
        let other = a_peer_id(201);
        let err = check_target(&parsed, &other).unwrap_err();
        assert_eq!(err, RejectReason::WrongTarget);
        assert!(err.is_authenticated(), "wrong-target is post-signature");
    }

    #[test]
    fn expired_envelope_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now() - 200,
            now() - 100, // expired 100s ago
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let parsed = env.verify_and_parse(&sk.verifying_key()).unwrap();
        let err = check_freshness(&parsed, now()).unwrap_err();
        assert!(matches!(err, RejectReason::Stale(_)));
    }

    #[test]
    fn future_skew_envelope_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        // issuedAt far in the future (beyond the 30s skew).
        let env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now() + 100,
            now() + 300,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let parsed = env.verify_and_parse(&sk.verifying_key()).unwrap();
        let err = check_freshness(&parsed, now()).unwrap_err();
        assert!(matches!(err, RejectReason::Stale(_)));
    }

    #[test]
    fn lifetime_over_300s_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 400, // 400s > 300s cap
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let parsed = env.verify_and_parse(&sk.verifying_key()).unwrap();
        let err = check_freshness(&parsed, now()).unwrap_err();
        assert!(matches!(err, RejectReason::Stale(_)));
    }

    #[test]
    fn valid_envelope_within_window_is_fresh() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 200,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let parsed = env.verify_and_parse(&sk.verifying_key()).unwrap();
        assert!(check_freshness(&parsed, now()).is_ok());
        assert!(check_target(&parsed, &target).is_ok());
    }

    // -- Finding 1: parse-after-verify, unknown/duplicate keys, dryRun signed --

    #[test]
    fn finding1_unknown_key_rejected_after_verify() {
        // An envelope with an extra top-level key is rejected at the
        // parse-after-verify step (NOT reinterpreted). The signature is over the
        // exact bytes, so we must sign the extra-key string too.
        let sk = test_key(1);
        let target = a_peer_id(200);
        let canonical = format!(
            "{{\"action\":\"quorum.set_max_auto_members\",\"dryRun\":false,\
             \"evil\":1,\"expiresAt\":{e},\"issuedAt\":{i},\
             \"nonce\":\"00112233445566778899aabbccddeeff\",\"params\":{{\"value\":8}},\
             \"signerKeyId\":\"{s}\",\"targetNode\":\"{t}\",\"v\":1}}",
            e = now() + 100,
            i = now(),
            s = signer_id(&sk),
            t = target,
        );
        let env = sign_str(&sk, &canonical);
        let err = env.verify_and_parse(&sk.verifying_key()).unwrap_err();
        match err {
            RejectReason::InvalidPayload(d) => assert!(d.contains("unknown envelope field")),
            other => panic!("expected InvalidPayload, got {other:?}"),
        }
    }

    #[test]
    fn finding1_duplicate_key_rejected_after_verify() {
        // Two `dryRun` keys — serde_json would last-wins this; we must reject it
        // so one signed byte string cannot mean two logical envelopes.
        let sk = test_key(1);
        let target = a_peer_id(200);
        let canonical = format!(
            "{{\"action\":\"quorum.set_max_auto_members\",\"dryRun\":false,\"dryRun\":true,\
             \"expiresAt\":{e},\"issuedAt\":{i},\
             \"nonce\":\"00112233445566778899aabbccddeeff\",\"params\":{{\"value\":8}},\
             \"signerKeyId\":\"{s}\",\"targetNode\":\"{t}\",\"v\":1}}",
            e = now() + 100,
            i = now(),
            s = signer_id(&sk),
            t = target,
        );
        let env = sign_str(&sk, &canonical);
        let err = env.verify_and_parse(&sk.verifying_key()).unwrap_err();
        match err {
            RejectReason::InvalidPayload(d) => assert!(d.contains("duplicate key")),
            other => panic!("expected InvalidPayload duplicate-key, got {other:?}"),
        }
    }

    #[test]
    fn finding1_dryrun_is_signed_not_a_sibling_param() {
        // The dryRun value comes from the SIGNED bytes. A dryRun:true envelope
        // and a dryRun:false envelope are DIFFERENT signed strings; flipping the
        // value out-of-band (a sibling RPC param) is structurally impossible
        // because from_params reads only `envelope` + `signature`.
        let sk = test_key(1);
        let target = a_peer_id(200);
        let dry = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            true,
            None,
        );
        let real = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            now(),
            now() + 100,
            "aabbccddeeff00112233445566778899",
            false,
            None,
        );
        // Different byte strings (dryRun differs) → different signatures.
        assert_ne!(dry.envelope, real.envelope);
        assert_ne!(dry.signature_hex, real.signature_hex);
        let pd = dry.verify_and_parse(&sk.verifying_key()).unwrap();
        let pr = real.verify_and_parse(&sk.verifying_key()).unwrap();
        assert!(pd.dry_run);
        assert!(!pr.dry_run);

        // from_params ignores any sibling param: injecting dryRun at the top
        // level of the RPC params cannot change the parsed value.
        let params = serde_json::json!({
            "envelope": real.envelope,
            "signature": real.signature_hex,
            "dryRun": true, // attacker-injected sibling — MUST be ignored
        });
        let extracted = SignedEnvelope::from_params(&params).unwrap();
        let parsed = extracted.verify_and_parse(&sk.verifying_key()).unwrap();
        assert!(
            !parsed.dry_run,
            "sibling dryRun must not influence processing"
        );
    }

    #[test]
    fn integers_only_floats_rejected() {
        // A float where an integer is required (issuedAt) must be rejected.
        let sk = test_key(1);
        let target = a_peer_id(200);
        let canonical = format!(
            "{{\"action\":\"quorum.set_max_auto_members\",\"dryRun\":false,\
             \"expiresAt\":{e},\"issuedAt\":1800000000.5,\
             \"nonce\":\"00112233445566778899aabbccddeeff\",\"params\":{{\"value\":8}},\
             \"signerKeyId\":\"{s}\",\"targetNode\":\"{t}\",\"v\":1}}",
            e = now() + 100,
            s = signer_id(&sk),
            t = target,
        );
        let env = sign_str(&sk, &canonical);
        let err = env.verify_and_parse(&sk.verifying_key()).unwrap_err();
        assert!(matches!(err, RejectReason::InvalidPayload(_)));
    }

    // -- Allowlist tests (AC line 4) -----------------------------------------

    #[test]
    fn non_allowlist_action_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.set_mode", // NOT in the v1 allowlist
            "{\"mode\":\"explicit\"}",
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let err = env.verify_and_parse(&sk.verifying_key()).unwrap_err();
        match err {
            RejectReason::InvalidPayload(d) => assert!(d.contains("not in the v1 allowlist")),
            other => panic!("expected allowlist rejection, got {other:?}"),
        }
    }

    #[test]
    fn set_max_auto_members_over_ceiling_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":65}", // > 64 ceiling
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let err = env.verify_and_parse(&sk.verifying_key()).unwrap_err();
        assert!(matches!(err, RejectReason::InvalidPayload(_)));
    }

    #[test]
    fn wrong_version_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let canonical = format!(
            "{{\"action\":\"quorum.set_max_auto_members\",\"dryRun\":false,\
             \"expiresAt\":{e},\"issuedAt\":{i},\
             \"nonce\":\"00112233445566778899aabbccddeeff\",\"params\":{{\"value\":8}},\
             \"signerKeyId\":\"{s}\",\"targetNode\":\"{t}\",\"v\":2}}",
            e = now() + 100,
            i = now(),
            s = signer_id(&sk),
            t = target,
        );
        let env = sign_str(&sk, &canonical);
        let err = env.verify_and_parse(&sk.verifying_key()).unwrap_err();
        assert!(matches!(err, RejectReason::InvalidPayload(_)));
    }

    // -- Payload policy: peerId parse + membership floors --------------------

    fn parse_ok(env: &SignedEnvelope, sk: &SigningKey) -> ParsedEnvelope {
        env.verify_and_parse(&sk.verifying_key()).unwrap()
    }

    #[test]
    fn unparseable_peer_id_rejected() {
        let sk = test_key(1);
        let target = a_peer_id(200);
        let env = build_signed(
            &sk,
            "quorum.pin_member",
            "{\"peerId\":\"not-a-peer-id\"}",
            &target,
            now(),
            now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
            None,
        );
        let parsed = parse_ok(&env, &sk);
        let err = VerifiedAction::check_payload(parsed).unwrap_err();
        match err {
            RejectReason::InvalidPayload(d) => assert!(d.contains("base58 PeerId")),
            other => panic!("expected peerId parse rejection, got {other:?}"),
        }
    }

    #[test]
    fn finding2_membership_1_refused_even_with_acknowledge() {
        // Resulting membership 1 (node alone) is refused OUTRIGHT even with
        // acknowledgeDegenerate:true. The floor takes the AUTHORITATIVE resulting
        // membership the gate would compute (here: 1).
        let sk = test_key(1);
        let parsed = dummy_parsed_ack(ActionKind::UnpinMember {
            peer_id: a_peer_id(1),
        });
        let _ = &sk;
        // previous membership 2, resulting 1.
        let err = check_membership_floor(&parsed, 2, 1).unwrap_err();
        match err {
            RejectReason::InvalidPayload(d) => {
                assert!(d.contains("membership-1") || d.contains("solo"));
                assert!(d.contains("never allowed") || d.contains("no acknowledgeDegenerate"));
            }
            other => panic!("expected membership-1 hard floor, got {other:?}"),
        }
    }

    #[test]
    fn finding2_below_4_refused_without_acknowledge_accepted_with() {
        // Resulting membership 2 (<4) via a SHRINK: refused WITHOUT
        // acknowledgeDegenerate, accepted WITH it. (previous 3 → resulting 2.)
        let parsed_no = dummy_parsed(ActionKind::UnpinMember {
            peer_id: a_peer_id(1),
        });
        let err = check_membership_floor(&parsed_no, 3, 2).unwrap_err();
        match err {
            RejectReason::InvalidPayload(d) => assert!(d.contains("degenerate")),
            other => panic!("expected degenerate refusal, got {other:?}"),
        }

        let parsed_yes = dummy_parsed_ack(ActionKind::UnpinMember {
            peer_id: a_peer_id(1),
        });
        assert!(check_membership_floor(&parsed_yes, 3, 2).is_ok());

        // The apply mutation reduces the curated set correctly.
        let verified = VerifiedAction { parsed: parsed_yes };
        let mut q = QuorumConfig {
            mode: QuorumMode::Recommended,
            members: vec![a_peer_id(1), a_peer_id(2)],
            ..QuorumConfig::default()
        };
        assert!(verified.apply_to(&mut q));
        assert_eq!(q.members, vec![a_peer_id(2)]);
    }

    #[test]
    fn pin_growing_membership_needs_no_acknowledge() {
        // A pin that GROWS membership into a still-small set (previous 2 →
        // resulting 3, <4 but not a shrink) needs no acknowledgment.
        let parsed = dummy_parsed(ActionKind::PinMember {
            peer_id: a_peer_id(5),
        });
        assert!(
            check_membership_floor(&parsed, 2, 3).is_ok(),
            "a pin that grows membership must not require acknowledgeDegenerate"
        );
    }

    #[test]
    fn apply_to_pin_unpin_setcap_roundtrip() {
        let mut q = QuorumConfig {
            members: vec![],
            max_auto_members: 8,
            ..QuorumConfig::default()
        };
        let peer = a_peer_id(7);

        // pin
        let pin = VerifiedAction {
            parsed: dummy_parsed(ActionKind::PinMember {
                peer_id: peer.clone(),
            }),
        };
        assert!(pin.apply_to(&mut q));
        assert_eq!(q.members, vec![peer.clone()]);
        // pin again is a no-op
        assert!(!pin.apply_to(&mut q));

        // set cap
        let cap = VerifiedAction {
            parsed: dummy_parsed(ActionKind::SetMaxAutoMembers { value: 3 }),
        };
        assert!(cap.apply_to(&mut q));
        assert_eq!(q.max_auto_members, 3);
        assert!(!cap.apply_to(&mut q)); // no-op

        // unpin
        let unpin = VerifiedAction {
            parsed: dummy_parsed(ActionKind::UnpinMember {
                peer_id: peer.clone(),
            }),
        };
        assert!(unpin.apply_to(&mut q));
        assert!(q.members.is_empty());
        assert!(!unpin.apply_to(&mut q)); // no-op
    }

    fn dummy_parsed(action: ActionKind) -> ParsedEnvelope {
        ParsedEnvelope {
            action,
            dry_run: false,
            issued_at: now(),
            expires_at: now() + 100,
            nonce: "00112233445566778899aabbccddeeff".to_string(),
            signer_key_id: "a1b2c3d4e5f60708".to_string(),
            target_node: a_peer_id(200),
            acknowledge_degenerate: false,
        }
    }

    fn dummy_parsed_ack(action: ActionKind) -> ParsedEnvelope {
        ParsedEnvelope {
            acknowledge_degenerate: true,
            ..dummy_parsed(action)
        }
    }

    // -- Nonce step: dry-run must not reserve; replay rejected ----------------

    fn tmp_nonce_store() -> NonceStore {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("botho-opaction-test-{}-{}", std::process::id(), n));
        NonceStore::open(&NonceStore::path_from_data_dir(&dir)).unwrap()
    }

    #[test]
    fn reserve_nonce_then_replay_rejected() {
        let mut store = tmp_nonce_store();
        let parsed = dummy_parsed(ActionKind::SetMaxAutoMembers { value: 8 });
        assert!(reserve_nonce(&mut store, &parsed, now()).is_ok());
        // Second reserve of the same nonce → replay.
        let err = reserve_nonce(&mut store, &parsed, now()).unwrap_err();
        assert!(matches!(
            err,
            OperatorActionError::Rejected(RejectReason::ReplayedNonce)
        ));
    }

    #[test]
    fn dry_run_never_reserves_nonce() {
        // A dry run must not touch the nonce store: the store is only consulted
        // by the (real-apply) reserve step, which the caller skips for dry runs.
        // This test documents the contract at the module level: reserve_nonce is
        // never called for a dry run, so the SAME nonce remains reservable for a
        // later real apply.
        let mut store = tmp_nonce_store();
        // (dry run: no reserve_nonce call at all)
        assert!(store.is_empty());
        // The real apply of the same nonce still succeeds.
        let parsed = dummy_parsed(ActionKind::SetMaxAutoMembers { value: 8 });
        assert!(reserve_nonce(&mut store, &parsed, now()).is_ok());
    }

    // -- Outcome shaping (for #750) ------------------------------------------

    #[test]
    fn pre_signature_rejection_is_not_authenticated_and_carries_no_signer() {
        let outcome = OperatorActionOutcome::rejected(&RejectReason::NotConfigured, None);
        assert!(!outcome.authenticated);
        assert!(outcome.signer_key_id.is_none());
        assert_eq!(outcome.audit_tag, "verify_refused:not_configured");
    }

    #[test]
    fn post_signature_rejection_is_authenticated_and_carries_signer() {
        let sk = test_key(1);
        let parsed = dummy_parsed(ActionKind::PinMember {
            peer_id: a_peer_id(1),
        });
        let mut p = parsed;
        p.signer_key_id = signer_id(&sk);
        let outcome = OperatorActionOutcome::rejected(&RejectReason::WrongTarget, Some(&p));
        assert!(outcome.authenticated);
        assert_eq!(
            outcome.signer_key_id.as_deref(),
            Some(p.signer_key_id.as_str())
        );
        assert_eq!(outcome.audit_tag, "verify_refused:wrong_target");
        assert_eq!(outcome.action.as_deref(), Some("quorum.pin_member"));
    }
}
