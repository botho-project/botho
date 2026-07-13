// Copyright (c) 2024 The Botho Foundation

//! Attestation provider — the engine's source of [`MintAuthorization`]s and
//! [`ReleaseAuthorization`]s (#824).
//!
//! Per ADR 0002 the SCP validator set doubles as the bridge's t-of-n
//! federation. This module implements the validator attestation protocol on
//! top of the envelope machinery in `bth_bridge_core::attestation`:
//!
//! - **Ingest pipeline**
//!   ([`FederationAttestationProvider::submit_attestation`]): the single seam
//!   every federation attestation flows through, mirroring the operator-action
//!   verifier's fail-closed ordering — signer selection (public data only) →
//!   signature verification over the received, domain-separated bytes →
//!   parse-after-verify → freshness → durable nonce reserve
//!   (reserve-then-apply) → order binding → threshold aggregation with
//!   distinct-signer dedupe.
//! - **Key domains** (ADR 0002): BTH releases and Solana mints are Ed25519
//!   (validator node keys); Ethereum mints are secp256k1 — the payload
//!   signature is a Gnosis Safe owner signature over the EIP-712 SafeTx hash
//!   wrapping `bridgeMint(to, amount, orderId)` at an attested Safe nonce, so
//!   the aggregated signatures are directly consumable by
//!   `Safe.execTransaction`.
//! - **Local signing**: the bridge node self-attests with its own federation
//!   keys through the SAME ingest pipeline (its own envelopes get no special
//!   trust). Immediately after a self-attestation is accepted locally, the
//!   envelope is pushed to the other federation members over the #858 transport
//!   (an [`EnvelopePush`] broadcaster, wired to the peers' inbound `POST
//!   /api/attest` endpoints); inbound peer envelopes arrive at THIS node's
//!   endpoint and flow through [`submit_attestation`] — the same fail-closed
//!   pipeline as any local envelope. When no broadcaster is configured
//!   (single-node development, or threshold 1) a threshold above the
//!   locally-signable count simply never authorizes — fail-safe, orders stay in
//!   their confirmed state.
//!
//! [`submit_attestation`]: FederationAttestationProvider::submit_attestation
//!
//! [`StubAttestationProvider`] remains the development fallback when no
//! federation is configured, and [`DisabledAttestationProvider`] is the
//! fail-closed provider installed when a federation is configured but
//! invalid.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use alloy::{
    network::TransactionBuilder,
    primitives::{keccak256, Address, Signature as EcdsaSignature, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::{local::PrivateKeySigner, SignerSync},
    sol_types::SolCall,
};
use async_trait::async_trait;
use bth_bridge_core::{
    attestation::{
        canonical_attestation_envelope, check_attestation_freshness, check_order_binding,
        parse_attestation_envelope, peek_signer_key_id, peek_target_chain,
        sign_attestation_ed25519, AttestationEnvelope, AttestationKind, AttestationOutcome,
        AttestationRejectReason, AttestationSet, ParsedAttestation,
    },
    attestation_signed_message,
    nonce::{NonceStore, ReserveOutcome},
    AttestationSignature, BridgeConfig, BridgeOrder, Chain, MintAuthorization, OrderType,
    ReleaseAuthorization, SignatureScheme,
};
use chrono::Utc;
use ed25519_dalek::{SigningKey, VerifyingKey};
use tracing::{info, warn};
use uuid::Uuid;

use crate::mint::ethereum::{encode_bridge_mint_calldata, safe_tx_hash, IGnosisSafe};

/// Validity window the local signer stamps on its own attestations.
const SELF_ATTESTATION_LIFETIME_SECS: u64 = 120;

/// Outbound transport for freshly-signed local attestation envelopes (#858).
///
/// When this node self-attests (mint or release) it hands the accepted
/// envelope to the broadcaster, which pushes it to the other federation
/// members' inbound endpoints. Implementations are fire-and-forget with
/// respect to the local authorization path: a slow or unreachable peer must
/// never wedge or fail the local self-attestation (the peer will still
/// self-attest and push its own envelope back). The envelope is
/// self-authenticating, so the transport carries no trust — a dropped push
/// only delays reaching threshold, and re-authorization re-pushes.
pub trait EnvelopePush: Send + Sync {
    /// Push one signed envelope to every configured peer. Errors are the
    /// implementation's to log/absorb; the return is advisory.
    fn broadcast(&self, envelope: &AttestationEnvelope);
}

/// Source of threshold mint and release authorizations.
#[async_trait]
pub trait AttestationProvider: Send + Sync {
    /// Obtain a threshold authorization for minting `order` on its
    /// destination chain. Blocks (or errors) until the federation threshold
    /// is met — the engine never submits an unauthorized mint.
    async fn authorize_mint(&self, order: &BridgeOrder) -> Result<MintAuthorization, String>;

    /// Obtain a threshold authorization for releasing `order`'s BTH from
    /// the reserve. Blocks (or errors) until the federation threshold is
    /// met — the engine never signs an unauthorized reserve spend. The
    /// returned authorization is bound to this order's deterministic id,
    /// its exact `net_amount()`, and its exact destination address (see
    /// [`bth_bridge_core::release_payload_digest`]).
    async fn authorize_release(&self, order: &BridgeOrder) -> Result<ReleaseAuthorization, String>;
}

/// Development stub used when NO attestation federation is configured.
///
/// Returns an authorization bound to the order's on-chain id with an EMPTY
/// signature set and threshold 0. This satisfies the local threshold check,
/// but a real Gnosis Safe / on-chain multisig authority will reject the
/// submission (no owner signatures), so it cannot mint against production
/// contracts. Useful against dev deployments whose Safe threshold is 0 or
/// whose authority is a plain EOA.
pub struct StubAttestationProvider;

#[async_trait]
impl AttestationProvider for StubAttestationProvider {
    async fn authorize_mint(&self, order: &BridgeOrder) -> Result<MintAuthorization, String> {
        let scheme = match order.dest_chain {
            Chain::Ethereum => SignatureScheme::Secp256k1,
            Chain::Solana => SignatureScheme::Ed25519,
            Chain::Bth => return Err("cannot mint to the BTH chain".to_string()),
        };

        Ok(MintAuthorization {
            order_id: order.order_id_bytes(),
            scheme,
            threshold: 0,
            signatures: vec![],
            safe_nonce: None,
        })
    }

    async fn authorize_release(&self, order: &BridgeOrder) -> Result<ReleaseAuthorization, String> {
        if order.dest_chain != Chain::Bth {
            return Err("release authorizations are only for the BTH chain".to_string());
        }

        // Empty signature set with threshold 0: passes the local distinct-
        // signer count only when the configured federation threshold floor
        // is also 0 (development). Any production configuration
        // (release_threshold >= 1) rejects this stub before any reserve
        // key material is touched — and BthReleaser::new refuses a zero
        // threshold outright once release signers are configured (#842).
        Ok(ReleaseAuthorization {
            order_id: order.order_id_bytes(),
            amount: order.net_amount(),
            recipient: order.dest_address.clone(),
            threshold: 0,
            signatures: vec![],
        })
    }
}

/// Fail-closed provider installed when an attestation federation is
/// configured but invalid: every authorization request errors, so orders
/// stay in their confirmed, retryable state until the operator fixes the
/// configuration. Never silently downgrades to the permissive stub.
pub struct DisabledAttestationProvider {
    reason: String,
}

impl DisabledAttestationProvider {
    /// Build a disabled provider carrying the configuration error.
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

#[async_trait]
impl AttestationProvider for DisabledAttestationProvider {
    async fn authorize_mint(&self, _order: &BridgeOrder) -> Result<MintAuthorization, String> {
        Err(format!(
            "attestation provider disabled (fix the federation configuration): {}",
            self.reason
        ))
    }

    async fn authorize_release(
        &self,
        _order: &BridgeOrder,
    ) -> Result<ReleaseAuthorization, String> {
        Err(format!(
            "attestation provider disabled (fix the federation configuration): {}",
            self.reason
        ))
    }
}

/// Source of the Gnosis Safe's current nonce (the value the SafeTx payload
/// signatures bind to). Abstracted so tests can inject a fixed nonce.
#[async_trait]
pub trait SafeNonceSource: Send + Sync {
    /// The Safe's current on-chain `nonce()`.
    async fn safe_nonce(&self) -> Result<u64, String>;
}

/// Production [`SafeNonceSource`]: reads `Safe.nonce()` over JSON-RPC.
pub struct RpcSafeNonceSource {
    provider: DynProvider,
    safe: Address,
}

impl RpcSafeNonceSource {
    /// Build from the configured Ethereum RPC URL and Safe address. Does
    /// not perform network I/O.
    pub fn new(rpc_url: &str, safe: Address) -> Result<Self, String> {
        let url = rpc_url
            .parse()
            .map_err(|e| format!("invalid ethereum rpc_url: {}", e))?;
        Ok(Self {
            provider: ProviderBuilder::new().connect_http(url).erased(),
            safe,
        })
    }
}

#[async_trait]
impl SafeNonceSource for RpcSafeNonceSource {
    async fn safe_nonce(&self) -> Result<u64, String> {
        let call = TransactionRequest::default()
            .with_to(self.safe)
            .with_input(IGnosisSafe::nonceCall {}.abi_encode());
        let ret = self
            .provider
            .call(call)
            .await
            .map_err(|e| format!("safe nonce() call failed: {}", e))?;
        let nonce = IGnosisSafe::nonceCall::abi_decode_returns(&ret)
            .map_err(|e| format!("safe nonce() decode failed: {}", e))?;
        u64::try_from(nonce).map_err(|_| "safe nonce exceeds u64".to_string())
    }
}

/// The Ethereum mint federation: Gnosis Safe owners (secp256k1, ADR 0002)
/// plus the Safe parameters the SafeTx payload digest binds to.
struct EthFederation {
    owners: Vec<Address>,
    threshold: u32,
    chain_id: u64,
    safe: Address,
    wbth: Address,
    nonce_source: Arc<dyn SafeNonceSource>,
}

/// An Ed25519 federation (Solana mints / BTH releases: validator node keys).
struct Ed25519Federation {
    signers: Vec<VerifyingKey>,
    threshold: u32,
}

/// Mutable collection state, behind one lock: the durable nonce store and
/// the per-`(order, action)` threshold sets.
struct Tracker {
    nonces: NonceStore,
    sets: HashMap<(Uuid, &'static str), AttestationSet>,
}

/// The real #824 provider: verifies, replay-checks, order-binds, and
/// aggregates federation attestations to threshold. See the module docs.
pub struct FederationAttestationProvider {
    eth: Option<EthFederation>,
    sol: Option<Ed25519Federation>,
    bth: Option<Ed25519Federation>,
    local_ed25519: Option<SigningKey>,
    local_secp256k1: Option<PrivateKeySigner>,
    tracker: Mutex<Tracker>,
    /// #858 outbound transport: pushes each accepted local envelope to the
    /// other federation members. `None` disables outbound push (single-node
    /// development / threshold 1) — the node then only ever counts its own
    /// signer, which is fail-safe.
    peer_push: Option<Arc<dyn EnvelopePush>>,
}

/// Parse a hex-encoded 32-byte Ed25519 public key list into verifying keys.
fn parse_ed25519_federation(signers: &[String], what: &str) -> Result<Vec<VerifyingKey>, String> {
    let mut keys = Vec::with_capacity(signers.len());
    for hex_key in signers {
        let bytes: [u8; 32] = hex::decode(hex_key.trim())
            .map_err(|e| format!("bad {} key hex {}: {}", what, hex_key, e))?
            .try_into()
            .map_err(|_| format!("{} key {} is not 32 bytes", what, hex_key))?;
        keys.push(
            VerifyingKey::from_bytes(&bytes)
                .map_err(|e| format!("{} key {} is invalid: {}", what, hex_key, e))?,
        );
    }
    Ok(keys)
}

/// Enforce a sane federation threshold at construction time: whenever a
/// federation is configured, a zero (or unsatisfiable) threshold is a
/// configuration error — a t-of-n federation with t = 0 would authorize
/// privileged actions with NO signatures (#842).
fn check_threshold(threshold: u32, n: usize, what: &str) -> Result<(), String> {
    if threshold == 0 {
        return Err(format!(
            "{}: threshold must be >= 1 when signers are configured \
             (threshold 0 would authorize with no signatures)",
            what
        ));
    }
    if threshold as usize > n {
        return Err(format!(
            "{}: threshold {} exceeds the {} configured signer(s)",
            what, threshold, n
        ));
    }
    Ok(())
}

/// Reject a federation configured with duplicate signer identities.
///
/// Duplicate owner addresses / signer pubkeys inflate the raw list length
/// `n` used by [`check_threshold`], so a `threshold == padded_n` config would
/// pass construction yet be unsatisfiable at runtime: the aggregator dedups
/// by signer identity ([`AttestationSet::insert`]), so the *effective*
/// federation is smaller than `n` and the threshold can never be reached —
/// the order wedges forever, fail-safe but silent. Rejecting the misconfig at
/// construction surfaces it immediately (#848).
///
/// `identities` must already be normalized (parsed/trimmed) so that surface
/// variants like `"0xAbc"` and `"0xabc"` collide on the same canonical bytes.
fn reject_duplicate_signers<T: Ord + Clone>(identities: &[T], what: &str) -> Result<(), String> {
    let mut seen = identities.to_vec();
    seen.sort();
    if seen.windows(2).any(|w| w[0] == w[1]) {
        return Err(format!(
            "{}: duplicate signer identities configured — every federation \
             member must be distinct (a repeated signer inflates the \
             configured count and makes a threshold equal to it unsatisfiable)",
            what
        ));
    }
    Ok(())
}

/// The signer identity string for an Ethereum owner address: lowercase hex,
/// no 0x prefix (40 chars).
fn eth_signer_key_id(address: &Address) -> String {
    hex::encode(address.as_slice())
}

impl FederationAttestationProvider {
    /// Build the provider from configuration.
    ///
    /// Returns `Ok(None)` when NO federation is configured anywhere (pure
    /// development — the engine falls back to [`StubAttestationProvider`]),
    /// `Ok(Some(_))` when at least one federation is configured and valid,
    /// and `Err` when a federation is configured but invalid (the engine
    /// then installs [`DisabledAttestationProvider`] — never the stub).
    pub fn from_config(config: &BridgeConfig) -> Result<Option<Self>, String> {
        let eth_cfg = &config.ethereum;
        let eth = if eth_cfg.mint_signers.is_empty() {
            None
        } else {
            let mut owners = Vec::with_capacity(eth_cfg.mint_signers.len());
            for s in &eth_cfg.mint_signers {
                owners.push(
                    s.trim()
                        .parse::<Address>()
                        .map_err(|e| format!("bad ethereum mint signer {}: {}", s, e))?,
                );
            }
            // Dedup on the parsed 20-byte address (not the raw string), so
            // checksummed / lowercase spellings of one owner collide.
            reject_duplicate_signers(&owners, "ethereum.mint_signers")?;
            check_threshold(
                eth_cfg.mint_threshold,
                owners.len(),
                "ethereum.mint_threshold",
            )?;
            let safe: Address = eth_cfg
                .safe_address
                .as_deref()
                .ok_or_else(|| {
                    "ethereum.safe_address is required for mint attestations (ADR 0002)".to_string()
                })?
                .parse()
                .map_err(|e| format!("invalid safe_address: {}", e))?;
            let wbth: Address = eth_cfg
                .wbth_contract
                .parse()
                .map_err(|e| format!("invalid wbth_contract: {}", e))?;
            let nonce_source: Arc<dyn SafeNonceSource> =
                Arc::new(RpcSafeNonceSource::new(&eth_cfg.rpc_url, safe)?);
            Some(EthFederation {
                owners,
                threshold: eth_cfg.mint_threshold,
                chain_id: eth_cfg.chain_id,
                safe,
                wbth,
                nonce_source,
            })
        };

        let sol = if config.solana.mint_signers.is_empty() {
            None
        } else {
            let signers = parse_ed25519_federation(&config.solana.mint_signers, "solana mint")?;
            reject_duplicate_signers(
                &signers.iter().map(|k| k.to_bytes()).collect::<Vec<_>>(),
                "solana.mint_signers",
            )?;
            check_threshold(
                config.solana.mint_threshold,
                signers.len(),
                "solana.mint_threshold",
            )?;
            Some(Ed25519Federation {
                signers,
                threshold: config.solana.mint_threshold,
            })
        };

        let bth = if config.bth.release_signers.is_empty() {
            None
        } else {
            let signers = parse_ed25519_federation(&config.bth.release_signers, "bth release")?;
            reject_duplicate_signers(
                &signers.iter().map(|k| k.to_bytes()).collect::<Vec<_>>(),
                "bth.release_signers",
            )?;
            check_threshold(
                config.bth.release_threshold,
                signers.len(),
                "bth.release_threshold",
            )?;
            Some(Ed25519Federation {
                signers,
                threshold: config.bth.release_threshold,
            })
        };

        if eth.is_none() && sol.is_none() && bth.is_none() {
            return Ok(None);
        }

        let local_ed25519 = match config.bridge.attestation_ed25519_key_file.as_deref() {
            Some(path) => {
                let raw = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read attestation_ed25519_key_file: {}", e))?;
                let bytes: [u8; 32] = hex::decode(raw.trim())
                    .map_err(|e| format!("bad ed25519 attestation key hex: {}", e))?
                    .try_into()
                    .map_err(|_| "ed25519 attestation key is not 32 bytes".to_string())?;
                Some(SigningKey::from_bytes(&bytes))
            }
            None => None,
        };
        let local_secp256k1 = match config.bridge.attestation_secp256k1_key_file.as_deref() {
            Some(path) => {
                let raw = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read attestation_secp256k1_key_file: {}", e))?;
                Some(
                    raw.trim()
                        .parse::<PrivateKeySigner>()
                        .map_err(|e| format!("bad secp256k1 attestation key: {}", e))?,
                )
            }
            None => None,
        };

        // Durable replay store: a restart inside an attestation's validity
        // window must not reopen a replay slot.
        let nonce_path = config
            .bridge
            .attestation_nonce_file
            .clone()
            .unwrap_or_else(|| format!("{}.attestation-nonces.json", config.bridge.db_path));
        let nonces = NonceStore::open(std::path::Path::new(&nonce_path))?;

        Ok(Some(Self {
            eth,
            sol,
            bth,
            local_ed25519,
            local_secp256k1,
            tracker: Mutex::new(Tracker {
                nonces,
                sets: HashMap::new(),
            }),
            peer_push: None,
        }))
    }

    /// Attach the #858 outbound transport: every accepted local
    /// self-attestation envelope is pushed to the other federation members.
    /// Consuming builder so the provider can be wrapped in an `Arc` after
    /// wiring (the transport is set once, at engine start).
    pub fn with_peer_push(mut self, push: Arc<dyn EnvelopePush>) -> Self {
        self.peer_push = Some(push);
        self
    }

    /// Test-support constructor for the #858 transport harness: a BTH-release
    /// federation (Ed25519) with an in-memory nonce store and no peer push
    /// (the caller attaches a real [`PeerBroadcaster`] via [`with_peer_push`]
    /// so envelopes travel over the wire, not by direct injection).
    #[cfg(test)]
    pub(crate) fn new_bth_for_test(
        signers: &[VerifyingKey],
        threshold: u32,
        local: SigningKey,
    ) -> Self {
        Self {
            eth: None,
            sol: None,
            bth: Some(Ed25519Federation {
                signers: signers.to_vec(),
                threshold,
            }),
            local_ed25519: Some(local),
            local_secp256k1: None,
            tracker: Mutex::new(Tracker {
                nonces: NonceStore::in_memory(),
                sets: HashMap::new(),
            }),
            peer_push: None,
        }
    }

    /// Test-support constructor for the #858 transport harness: an
    /// Ethereum-mint federation (secp256k1 Safe owners) reading the Safe
    /// nonce from a fixed source, with no peer push (the caller attaches a
    /// real [`PeerBroadcaster`]).
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_eth_for_test(
        owners: &[Address],
        threshold: u32,
        chain_id: u64,
        safe: Address,
        wbth: Address,
        nonce_source: Arc<dyn SafeNonceSource>,
        local: PrivateKeySigner,
    ) -> Self {
        Self {
            eth: Some(EthFederation {
                owners: owners.to_vec(),
                threshold,
                chain_id,
                safe,
                wbth,
                nonce_source,
            }),
            sol: None,
            bth: None,
            local_ed25519: None,
            local_secp256k1: Some(local),
            tracker: Mutex::new(Tracker {
                nonces: NonceStore::in_memory(),
                sets: HashMap::new(),
            }),
            peer_push: None,
        }
    }

    /// Distinct signers collected so far for `(order, action)` — test-only
    /// observation of an [`AttestationSet`]'s progress WITHOUT touching it
    /// (the #858 harness asserts a wired envelope actually reached a node).
    #[cfg(test)]
    pub(crate) fn distinct_signers_for_test(&self, order_id: Uuid, action: &'static str) -> u32 {
        self.progress(order_id, action)
    }

    /// Build a release [`AttestationKind`] for `order` (test-only; the real
    /// path is `release_kind`, which is private).
    #[cfg(test)]
    pub(crate) fn release_kind_for_test(order: &BridgeOrder) -> AttestationKind {
        Self::release_kind(order).unwrap()
    }

    /// The configured threshold for a target chain's federation.
    fn threshold_for(&self, chain: Chain) -> Option<u32> {
        match chain {
            Chain::Ethereum => self.eth.as_ref().map(|f| f.threshold),
            Chain::Solana => self.sol.as_ref().map(|f| f.threshold),
            Chain::Bth => self.bth.as_ref().map(|f| f.threshold),
        }
    }

    /// Distinct signers collected so far for `(order, action)`.
    fn progress(&self, order_id: Uuid, action: &'static str) -> u32 {
        self.tracker
            .lock()
            .expect("tracker lock poisoned")
            .sets
            .get(&(order_id, action))
            .map(|s| s.distinct_signers())
            .unwrap_or(0)
    }

    /// Ingest one federation attestation for `order` at the current wall
    /// clock. This is the RPC seam validators submit envelopes through: the
    /// #858 inbound endpoint (`POST /api/attest`) peeks the envelope's order
    /// id, routes it to the on-record order, then calls this — which runs the
    /// full fail-closed verify pipeline (the endpoint is pure transport in
    /// front of verification, never a trusted inject).
    pub fn submit_attestation(
        &self,
        envelope: &AttestationEnvelope,
        order: &BridgeOrder,
    ) -> AttestationOutcome {
        self.submit_attestation_at(envelope, order, Utc::now().timestamp().max(0) as u64)
    }

    /// [`submit_attestation`](Self::submit_attestation) with an explicit
    /// clock, for deterministic tests.
    pub fn submit_attestation_at(
        &self,
        envelope: &AttestationEnvelope,
        order: &BridgeOrder,
        now: u64,
    ) -> AttestationOutcome {
        let outcome = self.verify_and_aggregate(envelope, order, now);
        if outcome.accepted {
            info!(
                "attestation accepted: order={} action={:?} signer={:?} progress={}/{}",
                order.id, outcome.action, outcome.signer_key_id, outcome.signers, outcome.threshold
            );
        } else {
            warn!(
                "attestation refused: order={} tag={} detail={}",
                order.id, outcome.tag, outcome.message
            );
        }
        outcome
    }

    /// The fail-closed ingest pipeline. Ordering mirrors the operator-action
    /// verifier: no secret-dependent step precedes signature verification,
    /// the nonce is durably reserved BEFORE the attestation can count
    /// (reserve-then-apply), and every failure is first-failure-wins.
    fn verify_and_aggregate(
        &self,
        envelope: &AttestationEnvelope,
        order: &BridgeOrder,
        now: u64,
    ) -> AttestationOutcome {
        // Steps 1-2: routing + signer selection over PUBLIC data. The peeks
        // read unverified bytes but drive no security decision: a lying
        // value selects a key/domain the signature then fails against.
        let chain = match peek_target_chain(&envelope.envelope) {
            Ok(c) => c,
            Err(r) => return AttestationOutcome::refuse(&r, None, 0, 0),
        };
        let Some(threshold) = self.threshold_for(chain) else {
            return AttestationOutcome::refuse(&AttestationRejectReason::NotConfigured, None, 0, 0);
        };
        let signer_key_id = match peek_signer_key_id(&envelope.envelope) {
            Ok(id) => id,
            Err(r) => return AttestationOutcome::refuse(&r, None, 0, threshold),
        };

        // Step 3: signature verification + parse-after-verify, per the
        // ADR 0002 key domain for the target chain.
        let refuse = |r: AttestationRejectReason, parsed: Option<&ParsedAttestation>| {
            let signers = parsed
                .map(|p| self.progress(p.action.order_uuid(), p.action.name()))
                .unwrap_or(0);
            AttestationOutcome::refuse(&r, parsed, signers, threshold)
        };

        let (parsed, signer_bytes) = match chain {
            Chain::Ethereum => {
                let fed = self.eth.as_ref().expect("threshold_for checked eth");
                let Some(owner) = fed
                    .owners
                    .iter()
                    .find(|a| eth_signer_key_id(a) == signer_key_id)
                else {
                    return refuse(AttestationRejectReason::UnknownSigner, None);
                };
                match verify_and_parse_secp256k1(envelope, *owner, fed) {
                    Ok(p) => (p, owner.as_slice().to_vec()),
                    Err(r) => return refuse(r, None),
                }
            }
            Chain::Solana | Chain::Bth => {
                let fed = match chain {
                    Chain::Solana => self.sol.as_ref().expect("threshold_for checked sol"),
                    _ => self.bth.as_ref().expect("threshold_for checked bth"),
                };
                let Some(key) = fed
                    .signers
                    .iter()
                    .find(|k| hex::encode(k.as_bytes()) == signer_key_id)
                else {
                    return refuse(AttestationRejectReason::UnknownSigner, None);
                };
                match envelope.verify_and_parse_ed25519(key) {
                    Ok(p) => (p, key.as_bytes().to_vec()),
                    Err(r) => return refuse(r, None),
                }
            }
        };

        // Step 4: freshness — a captured envelope dies with its window.
        if let Err(r) = check_attestation_freshness(&parsed, now) {
            return refuse(r, Some(&parsed));
        }

        let mut tracker = self.tracker.lock().expect("tracker lock poisoned");

        // Step 5: durable nonce reserve (reserve-then-apply). Each signer
        // can consume a given nonce exactly once; a crash after the reserve
        // fails safe (the envelope can never count twice — the signer
        // re-signs with a fresh nonce).
        match tracker
            .nonces
            .reserve(&parsed.signer_key_id, &parsed.nonce, parsed.expires_at, now)
        {
            Ok(ReserveOutcome::Reserved) => {}
            Ok(ReserveOutcome::Replay) => {
                drop(tracker);
                return refuse(AttestationRejectReason::ReplayedNonce, Some(&parsed));
            }
            Err(e) => {
                drop(tracker);
                return refuse(AttestationRejectReason::Internal(e), Some(&parsed));
            }
        }

        // Step 6: order binding — the attestation must match the on-record
        // order in every field (id, amount, recipient, chains, source tx).
        if let Err(r) = check_order_binding(&parsed, order) {
            drop(tracker);
            return refuse(r, Some(&parsed));
        }

        // Step 7: threshold aggregation, deduped by signer identity.
        let payload_sig = match hex::decode(envelope.payload_signature_hex.trim()) {
            Ok(b) => b,
            Err(_) => {
                drop(tracker);
                return refuse(
                    AttestationRejectReason::Malformed("payload signature is not hex".to_string()),
                    Some(&parsed),
                );
            }
        };
        let set = tracker
            .sets
            .entry((order.id, parsed.action.name()))
            .or_insert_with(|| AttestationSet::for_attestation(&parsed));
        match set.insert(
            &parsed,
            AttestationSignature {
                signer: signer_bytes,
                signature: payload_sig,
            },
        ) {
            Ok(_new_signer) => {
                let signers = set.distinct_signers();
                drop(tracker);
                AttestationOutcome::accept(&parsed, signers, threshold)
            }
            Err(e) => {
                let signers = set.distinct_signers();
                drop(tracker);
                AttestationOutcome::refuse(
                    &AttestationRejectReason::InvalidPayload(e),
                    Some(&parsed),
                    signers,
                    threshold,
                )
            }
        }
    }

    /// Build the [`AttestationKind`] for a mint order.
    fn mint_kind(order: &BridgeOrder, safe_nonce: Option<u64>) -> Result<AttestationKind, String> {
        if order.order_type != OrderType::Mint {
            return Err("not a mint order".to_string());
        }
        Ok(AttestationKind::MintWbth {
            dest_chain: order.dest_chain,
            dest_address: order.dest_address.clone(),
            amount: order.net_amount(),
            order_id: order.id,
            source_tx: order
                .source_tx
                .clone()
                .ok_or_else(|| "order has no confirmed deposit tx on record".to_string())?,
            safe_nonce,
        })
    }

    /// Build the [`AttestationKind`] for a burn order's release.
    fn release_kind(order: &BridgeOrder) -> Result<AttestationKind, String> {
        if order.order_type != OrderType::Burn || order.dest_chain != Chain::Bth {
            return Err("not a BTH-destined burn order".to_string());
        }
        Ok(AttestationKind::ReleaseBth {
            source_chain: order.source_chain,
            bth_address: order.dest_address.clone(),
            amount: order.net_amount(),
            order_id: order.id,
            source_tx: order
                .source_tx
                .clone()
                .ok_or_else(|| "order has no confirmed burn tx on record".to_string())?,
        })
    }

    /// Self-attest with the local Ed25519 key (Solana mints / BTH releases)
    /// through the SAME ingest pipeline as any other federation member.
    fn self_attest_ed25519(
        &self,
        kind: &AttestationKind,
        order: &BridgeOrder,
    ) -> Result<(), String> {
        let Some(sk) = &self.local_ed25519 else {
            return Ok(()); // no local signer — rely on peers (#858)
        };
        let my_id = hex::encode(sk.verifying_key().as_bytes());
        if self.set_contains(order.id, kind.name(), &my_id) {
            return Ok(()); // already counted; do not burn another nonce
        }

        let now = Utc::now().timestamp().max(0) as u64;
        let envelope = sign_attestation_ed25519(
            kind,
            sk,
            &Uuid::new_v4().simple().to_string(),
            now,
            now + SELF_ATTESTATION_LIFETIME_SECS,
        )?;
        let outcome = self.submit_attestation_at(&envelope, order, now);
        if !outcome.accepted {
            return Err(format!(
                "self-attestation refused ({}): {}",
                outcome.tag, outcome.message
            ));
        }
        // #858: push our accepted envelope to the other federation members
        // so their nodes reach threshold. Fire-and-forget — a peer failure
        // never fails our local authorization path.
        self.push_to_peers(&envelope);
        Ok(())
    }

    /// Hand a freshly-accepted local envelope to the #858 outbound transport
    /// (no-op when no broadcaster is wired).
    fn push_to_peers(&self, envelope: &AttestationEnvelope) {
        if let Some(push) = &self.peer_push {
            push.broadcast(envelope);
        }
    }

    fn set_contains(&self, order_id: Uuid, action: &'static str, signer_key_id: &str) -> bool {
        self.tracker
            .lock()
            .expect("tracker lock poisoned")
            .sets
            .get(&(order_id, action))
            .map(|s| s.contains_signer(signer_key_id))
            .unwrap_or(false)
    }

    /// If the threshold is met for `(order, action)`, take the collected
    /// signatures (dropping the set — a later re-authorization, e.g. after
    /// a reorg unwind, re-collects against fresh chain state).
    fn take_if_threshold_met(
        &self,
        order_id: Uuid,
        action: &'static str,
        threshold: u32,
    ) -> Option<Vec<AttestationSignature>> {
        let mut tracker = self.tracker.lock().expect("tracker lock poisoned");
        let met = tracker
            .sets
            .get(&(order_id, action))
            .map(|s| s.is_threshold_met(threshold))
            .unwrap_or(false);
        if !met {
            return None;
        }
        tracker
            .sets
            .remove(&(order_id, action))
            .map(|s| s.signatures())
    }
}

/// Verify a secp256k1 (Ethereum-domain) attestation envelope against a Safe
/// owner address and parse it.
///
/// - The envelope signature is ECDSA over `keccak256(domain || bytes)`, checked
///   by address recovery against the expected owner.
/// - The payload signature is the Gnosis Safe owner signature over the EIP-712
///   SafeTx hash wrapping `bridgeMint(to, amount, orderId)` at the attested
///   Safe nonce — exactly the bytes `assemble_safe_signatures` later hands to
///   `Safe.execTransaction`.
fn verify_and_parse_secp256k1(
    envelope: &AttestationEnvelope,
    expected_owner: Address,
    fed: &EthFederation,
) -> Result<ParsedAttestation, AttestationRejectReason> {
    let env_sig =
        EcdsaSignature::from_raw(&hex::decode(envelope.signature_hex.trim()).map_err(|_| {
            AttestationRejectReason::Malformed("signature is not valid hex".to_string())
        })?)
        .map_err(|_| {
            AttestationRejectReason::Malformed("signature is not a 65-byte {r,s,v}".to_string())
        })?;

    let msg = attestation_signed_message(Chain::Ethereum, envelope.envelope.as_bytes());
    let recovered = env_sig
        .recover_address_from_prehash(&keccak256(&msg))
        .map_err(|_| AttestationRejectReason::BadSignature)?;
    if recovered != expected_owner {
        return Err(AttestationRejectReason::BadSignature);
    }

    // Parse-after-verify: the signature is valid over exactly these bytes.
    let parsed = parse_attestation_envelope(&envelope.envelope)
        .map_err(AttestationRejectReason::InvalidPayload)?;

    let AttestationKind::MintWbth {
        dest_chain: Chain::Ethereum,
        dest_address,
        amount,
        safe_nonce: Some(safe_nonce),
        ..
    } = &parsed.action
    else {
        return Err(AttestationRejectReason::InvalidPayload(
            "secp256k1 attestations are only for Ethereum mints".to_string(),
        ));
    };

    // The payload signature must be the owner's signature over THIS order's
    // SafeTx digest (binding chain id, Safe, calldata = order id + amount +
    // recipient, and the attested Safe nonce).
    let to: Address = dest_address.parse().map_err(|e| {
        AttestationRejectReason::InvalidPayload(format!("invalid destAddress: {}", e))
    })?;
    let calldata =
        encode_bridge_mint_calldata(to, U256::from(*amount), parsed.action.order_id_bytes());
    let digest = safe_tx_hash(
        fed.chain_id,
        fed.safe,
        fed.wbth,
        &calldata,
        U256::from(*safe_nonce),
    );
    let payload_sig = EcdsaSignature::from_raw(
        &hex::decode(envelope.payload_signature_hex.trim()).map_err(|_| {
            AttestationRejectReason::Malformed("payload signature is not valid hex".to_string())
        })?,
    )
    .map_err(|_| {
        AttestationRejectReason::Malformed("payload signature is not a 65-byte {r,s,v}".to_string())
    })?;
    let payload_signer = payload_sig
        .recover_address_from_prehash(&digest)
        .map_err(|_| AttestationRejectReason::BadSignature)?;
    if payload_signer != expected_owner {
        return Err(AttestationRejectReason::BadSignature);
    }

    Ok(parsed)
}

/// Build and secp256k1-sign a complete Ethereum mint attestation envelope
/// (validator-side tooling; also used by tests). The signer identity is the
/// lowercase hex of the owner's Ethereum address; the payload signature is
/// the Safe owner signature over the SafeTx hash at `kind.safe_nonce`.
pub fn sign_attestation_secp256k1(
    kind: &AttestationKind,
    signer: &PrivateKeySigner,
    chain_id: u64,
    safe: Address,
    wbth: Address,
    nonce: &str,
    issued_at: u64,
    expires_at: u64,
) -> Result<AttestationEnvelope, String> {
    let AttestationKind::MintWbth {
        dest_chain: Chain::Ethereum,
        dest_address,
        amount,
        safe_nonce: Some(safe_nonce),
        ..
    } = kind
    else {
        return Err(
            "secp256k1 attestations are only for Ethereum mints (with a Safe nonce)".to_string(),
        );
    };

    let signer_key_id = eth_signer_key_id(&signer.address());
    let envelope =
        canonical_attestation_envelope(kind, &signer_key_id, nonce, issued_at, expires_at);

    let msg = attestation_signed_message(Chain::Ethereum, envelope.as_bytes());
    let env_sig = signer
        .sign_hash_sync(&keccak256(&msg))
        .map_err(|e| format!("envelope signing failed: {}", e))?;

    let to: Address = dest_address
        .parse()
        .map_err(|e| format!("invalid destAddress: {}", e))?;
    let calldata = encode_bridge_mint_calldata(to, U256::from(*amount), kind.order_id_bytes());
    let digest = safe_tx_hash(chain_id, safe, wbth, &calldata, U256::from(*safe_nonce));
    let payload_sig = signer
        .sign_hash_sync(&digest)
        .map_err(|e| format!("payload signing failed: {}", e))?;

    Ok(AttestationEnvelope {
        envelope,
        signature_hex: hex::encode(env_sig.as_bytes()),
        payload_signature_hex: hex::encode(payload_sig.as_bytes()),
    })
}

#[async_trait]
impl AttestationProvider for FederationAttestationProvider {
    async fn authorize_mint(&self, order: &BridgeOrder) -> Result<MintAuthorization, String> {
        match order.dest_chain {
            Chain::Ethereum => {
                let fed = self.eth.as_ref().ok_or_else(|| {
                    "no Ethereum mint federation configured — refusing to authorize (fail-safe)"
                        .to_string()
                })?;

                // Stale-nonce eviction (#848 / #849). A partial set is pinned
                // to the Safe nonce of its first attestation. If an unrelated
                // Safe transaction executed while the set was below threshold,
                // the Safe's on-chain nonce advances past the pinned nonce and
                // every fresh-nonce attestation is (correctly) refused as a
                // nonce mismatch — the set can never reach a usable threshold
                // and the order wedges at DepositConfirmed until a restart.
                //
                // Read the live nonce OUTSIDE the tracker lock (RPC), then
                // re-lock to evict only when the pinned nonce is STRICTLY
                // behind. An equal nonce is the healthy case; a set pinned
                // AHEAD of the read (a transient stale on-chain read) is left
                // untouched — we never evict on a nonce that only appears to
                // have moved. Dropping the set is safe: the collected
                // signatures bind the stale nonce and are unusable anyway, and
                // the durable nonce store forces each peer to re-sign with a
                // fresh envelope nonce on re-collection (no replay risk).
                let pinned_nonce = {
                    let tracker = self.tracker.lock().expect("tracker lock poisoned");
                    tracker
                        .sets
                        .get(&(order.id, "bridge.mint_wbth"))
                        .and_then(|s| s.safe_nonce())
                };
                if let Some(pinned) = pinned_nonce {
                    let live = fed.nonce_source.safe_nonce().await?;
                    if pinned < live {
                        let mut tracker = self.tracker.lock().expect("tracker lock poisoned");
                        // Re-check under the lock: only evict if the set is
                        // still pinned to the same stale nonce (it may have
                        // been re-collected or taken between the RPC read and
                        // re-acquiring the lock).
                        if tracker
                            .sets
                            .get(&(order.id, "bridge.mint_wbth"))
                            .and_then(|s| s.safe_nonce())
                            == Some(pinned)
                        {
                            tracker.sets.remove(&(order.id, "bridge.mint_wbth"));
                        }
                    }
                }

                // Self-attest with the local Safe owner key, binding to the
                // set's Safe nonce (or the current on-chain nonce for a
                // fresh set) so all collected signatures share one SafeTx.
                if let Some(signer) = &self.local_secp256k1 {
                    let my_id = eth_signer_key_id(&signer.address());
                    if !self.set_contains(order.id, "bridge.mint_wbth", &my_id) {
                        let safe_nonce = {
                            let tracker = self.tracker.lock().expect("tracker lock poisoned");
                            tracker
                                .sets
                                .get(&(order.id, "bridge.mint_wbth"))
                                .and_then(|s| s.safe_nonce())
                        };
                        let safe_nonce = match safe_nonce {
                            Some(n) => n,
                            None => fed.nonce_source.safe_nonce().await?,
                        };
                        let kind = Self::mint_kind(order, Some(safe_nonce))?;
                        let now = Utc::now().timestamp().max(0) as u64;
                        let envelope = sign_attestation_secp256k1(
                            &kind,
                            signer,
                            fed.chain_id,
                            fed.safe,
                            fed.wbth,
                            &Uuid::new_v4().simple().to_string(),
                            now,
                            now + SELF_ATTESTATION_LIFETIME_SECS,
                        )?;
                        let outcome = self.submit_attestation_at(&envelope, order, now);
                        if !outcome.accepted {
                            return Err(format!(
                                "self-attestation refused ({}): {}",
                                outcome.tag, outcome.message
                            ));
                        }
                        // #858: push our Safe-owner envelope (bound to the
                        // set's Safe nonce, so peers bind the SAME nonce) to
                        // the other members. Fire-and-forget.
                        self.push_to_peers(&envelope);
                    }
                }

                // #858: peer envelopes arrive asynchronously at this node's
                // inbound endpoint and flow through submit_attestation into
                // the same set. If the threshold is not yet met we fail-safe
                // (the order stays retryable and re-authorizes on the next
                // pass, re-pushing our envelope).
                let collected = self.progress(order.id, "bridge.mint_wbth");
                // Snapshot the Safe nonce the set is pinned to BEFORE
                // take_if_threshold_met removes the set, so the authorization
                // can carry it for the minter's pre-broadcast cross-check
                // (#848).
                let collected_nonce = {
                    let tracker = self.tracker.lock().expect("tracker lock poisoned");
                    tracker
                        .sets
                        .get(&(order.id, "bridge.mint_wbth"))
                        .and_then(|s| s.safe_nonce())
                };
                match self.take_if_threshold_met(order.id, "bridge.mint_wbth", fed.threshold) {
                    Some(signatures) => Ok(MintAuthorization {
                        order_id: order.order_id_bytes(),
                        scheme: SignatureScheme::Secp256k1,
                        threshold: fed.threshold,
                        signatures,
                        safe_nonce: collected_nonce,
                    }),
                    None => Err(format!(
                        "attestation threshold not met ({}/{} distinct signers); \
                         awaiting peer envelopes (#858 transport)",
                        collected, fed.threshold
                    )),
                }
            }
            Chain::Solana => {
                let fed = self.sol.as_ref().ok_or_else(|| {
                    "no Solana mint federation configured — refusing to authorize (fail-safe)"
                        .to_string()
                })?;
                let kind = Self::mint_kind(order, None)?;
                self.self_attest_ed25519(&kind, order)?;

                // #858: self_attest_ed25519 already pushed our envelope to
                // peers; peer envelopes arrive at the inbound endpoint. Below
                // threshold is fail-safe (retries + re-pushes next pass).
                let collected = self.progress(order.id, "bridge.mint_wbth");
                match self.take_if_threshold_met(order.id, "bridge.mint_wbth", fed.threshold) {
                    Some(signatures) => Ok(MintAuthorization {
                        order_id: order.order_id_bytes(),
                        scheme: SignatureScheme::Ed25519,
                        threshold: fed.threshold,
                        signatures,
                        // Solana mints are blockhash-bound, not Safe-nonce
                        // bound: no Safe nonce to cross-check.
                        safe_nonce: None,
                    }),
                    None => Err(format!(
                        "attestation threshold not met ({}/{} distinct signers); \
                         federation transport pending #858",
                        collected, fed.threshold
                    )),
                }
            }
            Chain::Bth => Err("cannot mint to the BTH chain".to_string()),
        }
    }

    async fn authorize_release(&self, order: &BridgeOrder) -> Result<ReleaseAuthorization, String> {
        let fed = self.bth.as_ref().ok_or_else(|| {
            "no BTH release federation configured — refusing to authorize (fail-safe)".to_string()
        })?;
        let kind = Self::release_kind(order)?;
        self.self_attest_ed25519(&kind, order)?;

        // #858: self_attest_ed25519 already pushed our envelope to peers;
        // peer envelopes arrive at the inbound endpoint. Below threshold is
        // fail-safe (retries + re-pushes next pass).
        let collected = self.progress(order.id, "bridge.release_bth");
        match self.take_if_threshold_met(order.id, "bridge.release_bth", fed.threshold) {
            Some(signatures) => Ok(ReleaseAuthorization {
                order_id: order.order_id_bytes(),
                amount: order.net_amount(),
                recipient: order.dest_address.clone(),
                threshold: fed.threshold,
                signatures,
            }),
            None => Err(format!(
                "attestation threshold not met ({}/{} distinct signers); \
                 federation transport pending #858",
                collected, fed.threshold
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use bth_bridge_core::OrderStatus;

    const NOW: u64 = 1_700_000_000;

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn eth_signer(seed: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([seed; 32])).unwrap()
    }

    fn empty_tracker() -> Mutex<Tracker> {
        Mutex::new(Tracker {
            nonces: NonceStore::in_memory(),
            sets: HashMap::new(),
        })
    }

    /// Provider with only a BTH release federation (Ed25519).
    fn bth_provider(
        keys: &[&SigningKey],
        threshold: u32,
        local: Option<SigningKey>,
    ) -> FederationAttestationProvider {
        FederationAttestationProvider {
            eth: None,
            sol: None,
            bth: Some(Ed25519Federation {
                signers: keys.iter().map(|k| k.verifying_key()).collect(),
                threshold,
            }),
            local_ed25519: local,
            local_secp256k1: None,
            tracker: empty_tracker(),
            peer_push: None,
        }
    }

    struct FixedSafeNonce(u64);

    #[async_trait]
    impl SafeNonceSource for FixedSafeNonce {
        async fn safe_nonce(&self) -> Result<u64, String> {
            Ok(self.0)
        }
    }

    /// A Safe nonce source whose reported value can be advanced mid-test, to
    /// simulate an unrelated Safe transaction executing while a set is below
    /// threshold (#848 eviction path).
    struct MutableSafeNonce(std::sync::atomic::AtomicU64);

    #[async_trait]
    impl SafeNonceSource for MutableSafeNonce {
        async fn safe_nonce(&self) -> Result<u64, String> {
            Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
        }
    }

    const CHAIN_ID: u64 = 1;

    fn safe_addr() -> Address {
        Address::repeat_byte(0x5a)
    }

    fn wbth_addr() -> Address {
        Address::repeat_byte(0xeb)
    }

    /// Provider with only an Ethereum mint federation (secp256k1 Safe
    /// owners), reading the Safe nonce from a fixed test source.
    fn eth_provider(
        owners: &[&PrivateKeySigner],
        threshold: u32,
        safe_nonce: u64,
        local: Option<PrivateKeySigner>,
    ) -> FederationAttestationProvider {
        FederationAttestationProvider {
            eth: Some(EthFederation {
                owners: owners.iter().map(|s| s.address()).collect(),
                threshold,
                chain_id: CHAIN_ID,
                safe: safe_addr(),
                wbth: wbth_addr(),
                nonce_source: Arc::new(FixedSafeNonce(safe_nonce)),
            }),
            sol: None,
            bth: None,
            local_ed25519: None,
            local_secp256k1: local,
            tracker: empty_tracker(),
            peer_push: None,
        }
    }

    /// Like [`eth_provider`] but sharing a caller-held [`MutableSafeNonce`] so
    /// the test can advance the on-chain nonce mid-run.
    fn eth_provider_with_nonce_source(
        owners: &[&PrivateKeySigner],
        threshold: u32,
        nonce_source: Arc<MutableSafeNonce>,
        local: Option<PrivateKeySigner>,
    ) -> FederationAttestationProvider {
        FederationAttestationProvider {
            eth: Some(EthFederation {
                owners: owners.iter().map(|s| s.address()).collect(),
                threshold,
                chain_id: CHAIN_ID,
                safe: safe_addr(),
                wbth: wbth_addr(),
                nonce_source,
            }),
            sol: None,
            bth: None,
            local_ed25519: None,
            local_secp256k1: local,
            tracker: empty_tracker(),
            peer_push: None,
        }
    }

    fn burn_order() -> BridgeOrder {
        let mut order = BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_user_stealth_addr".to_string(),
            "0xburntx".to_string(),
        );
        order.set_status(OrderStatus::BurnConfirmed);
        order
    }

    fn eth_mint_order() -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_deposit_addr".to_string(),
            format!("0x{}", hex::encode([0x11u8; 20])),
        );
        order.source_tx = Some("bth_deposit_tx".to_string());
        order.set_status(OrderStatus::DepositConfirmed);
        order
    }

    fn sol_mint_order() -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Solana,
            1_000_000_000_000,
            1_000_000_000,
            "bth_deposit_addr".to_string(),
            "So1anaRecipient1111111111111111111111111111".to_string(),
        );
        order.source_tx = Some("bth_deposit_tx".to_string());
        order.set_status(OrderStatus::DepositConfirmed);
        order
    }

    /// Ed25519-sign a release attestation for `order` with `sk`.
    fn release_envelope(order: &BridgeOrder, sk: &SigningKey, nonce: &str) -> AttestationEnvelope {
        let kind = FederationAttestationProvider::release_kind(order).unwrap();
        sign_attestation_ed25519(&kind, sk, nonce, NOW, NOW + 120).unwrap()
    }

    /// secp256k1-sign an Ethereum mint attestation for `order`.
    fn eth_mint_envelope(
        order: &BridgeOrder,
        signer: &PrivateKeySigner,
        safe_nonce: u64,
        nonce: &str,
    ) -> AttestationEnvelope {
        let kind = FederationAttestationProvider::mint_kind(order, Some(safe_nonce)).unwrap();
        sign_attestation_secp256k1(
            &kind,
            signer,
            CHAIN_ID,
            safe_addr(),
            wbth_addr(),
            nonce,
            NOW,
            NOW + 120,
        )
        .unwrap()
    }

    // -- Ed25519 (BTH release) pipeline -----------------------------------

    #[test]
    fn pipeline_accepts_valid_release_attestation_then_rejects_replay() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let provider = bth_provider(&[&k1, &k2], 2, None);
        let order = burn_order();

        let envelope = release_envelope(&order, &k1, "nonce-a");
        let outcome = provider.submit_attestation_at(&envelope, &order, NOW);
        assert!(outcome.accepted, "{}: {}", outcome.tag, outcome.message);
        assert_eq!((outcome.signers, outcome.threshold), (1, 2));

        // The SAME envelope again: the durable nonce reserve rejects it.
        let replay = provider.submit_attestation_at(&envelope, &order, NOW);
        assert!(!replay.accepted);
        assert_eq!(replay.tag, "refused:replayed_nonce");
        assert!(replay.authenticated, "replays are post-signature refusals");
        assert_eq!(provider.progress(order.id, "bridge.release_bth"), 1);
    }

    #[test]
    fn pipeline_rejects_unknown_signer_without_counting_it() {
        let (k1, k2, byzantine) = (signing_key(1), signing_key(2), signing_key(66));
        let provider = bth_provider(&[&k1, &k2], 2, None);
        let order = burn_order();

        let envelope = release_envelope(&order, &byzantine, "nonce-b");
        let outcome = provider.submit_attestation_at(&envelope, &order, NOW);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:unknown_signer");
        assert!(!outcome.authenticated, "unknown signer is pre-signature");
        assert_eq!(provider.progress(order.id, "bridge.release_bth"), 0);
    }

    #[test]
    fn pipeline_rejects_tampered_envelope_bytes() {
        let k1 = signing_key(1);
        let provider = bth_provider(&[&k1], 1, None);
        let order = burn_order();

        let mut envelope = release_envelope(&order, &k1, "nonce-c");
        // Inflate the attested amount (net = 999_000_000_000 picocredits).
        let tampered = envelope.envelope.replace("999000000000", "999000000001");
        assert_ne!(tampered, envelope.envelope, "tamper must change the bytes");
        envelope.envelope = tampered;
        let outcome = provider.submit_attestation_at(&envelope, &order, NOW);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:bad_signature");
        assert_eq!(provider.progress(order.id, "bridge.release_bth"), 0);
    }

    #[test]
    fn pipeline_rejects_cross_order_reuse_even_with_a_fresh_nonce() {
        let k1 = signing_key(1);
        let provider = bth_provider(&[&k1], 1, None);
        let order_a = burn_order();
        let order_b = burn_order(); // distinct UUID

        let envelope = release_envelope(&order_a, &k1, "nonce-d");
        let outcome = provider.submit_attestation_at(&envelope, &order_b, NOW);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:wrong_order");
        assert_eq!(provider.progress(order_b.id, "bridge.release_bth"), 0);
        assert_eq!(provider.progress(order_a.id, "bridge.release_bth"), 0);
    }

    #[test]
    fn pipeline_rejects_expired_attestation() {
        let k1 = signing_key(1);
        let provider = bth_provider(&[&k1], 1, None);
        let order = burn_order();

        let envelope = release_envelope(&order, &k1, "nonce-e");
        let outcome = provider.submit_attestation_at(&envelope, &order, NOW + 121);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:stale");
    }

    #[test]
    fn pipeline_refuses_attestation_for_an_unconfigured_target_chain() {
        let k1 = signing_key(1);
        // BTH-only provider: a Solana mint attestation has no federation.
        let provider = bth_provider(&[&k1], 1, None);
        let order = sol_mint_order();
        let kind = FederationAttestationProvider::mint_kind(&order, None).unwrap();
        let envelope = sign_attestation_ed25519(&kind, &k1, "nonce-f", NOW, NOW + 120).unwrap();

        let outcome = provider.submit_attestation_at(&envelope, &order, NOW);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:not_configured");
    }

    #[test]
    fn pipeline_same_signer_with_fresh_nonces_counts_once_toward_threshold() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let provider = bth_provider(&[&k1, &k2], 2, None);
        let order = burn_order();

        let first = release_envelope(&order, &k1, "nonce-g1");
        assert!(provider.submit_attestation_at(&first, &order, NOW).accepted);

        // The same signer re-signs with a FRESH nonce: not a replay, but it
        // must not double-count.
        let second = release_envelope(&order, &k1, "nonce-g2");
        let outcome = provider.submit_attestation_at(&second, &order, NOW);
        assert!(outcome.accepted);
        assert_eq!(outcome.signers, 1, "same signer counts once");
        assert!(provider
            .take_if_threshold_met(order.id, "bridge.release_bth", 2)
            .is_none());
    }

    #[tokio::test]
    async fn authorize_release_meets_threshold_and_output_verifies_downstream() {
        let (k1, k2, k3) = (signing_key(1), signing_key(2), signing_key(3));
        // Local signer k1, federation {k1, k2, k3}, threshold 2.
        let provider = bth_provider(&[&k1, &k2, &k3], 2, Some(k1.clone()));
        let order = burn_order();

        // 1/2: only the local self-attestation — fail-safe, no authorization.
        let below = provider.authorize_release(&order).await;
        assert!(below.is_err());
        assert!(below.unwrap_err().contains("1/2"), "progress is reported");

        // Peer k2 submits (transport = #858; here injected directly).
        let peer = release_envelope(&order, &k2, "nonce-h");
        assert!(provider.submit_attestation_at(&peer, &order, NOW).accepted);

        // 2/2: authorized, with one payload signature per distinct signer.
        let auth = provider.authorize_release(&order).await.unwrap();
        assert_eq!(auth.order_id, order.order_id_bytes());
        assert_eq!(auth.amount, order.net_amount());
        assert_eq!(auth.recipient, order.dest_address);
        assert_eq!(auth.threshold, 2);
        assert_eq!(auth.signatures.len(), 2);

        // End-to-end: the authorization is exactly what the release path's
        // own validator (#840) accepts against the pinned federation.
        let federation = vec![k1.verifying_key(), k2.verifying_key(), k3.verifying_key()];
        crate::release::bth::validate_release_attestation(&order, &auth, &federation, 2)
            .expect("collected authorization must satisfy the release validator");
    }

    // -- secp256k1 (Ethereum mint) pipeline --------------------------------

    #[tokio::test]
    async fn authorize_mint_eth_collects_safe_owner_signatures_to_threshold() {
        let (o1, o2) = (eth_signer(1), eth_signer(2));
        let provider = eth_provider(&[&o1, &o2], 2, 7, None);
        let order = eth_mint_order();

        // Below threshold: fail-safe.
        let e1 = eth_mint_envelope(&order, &o1, 7, "nonce-i1");
        assert!(provider.submit_attestation_at(&e1, &order, NOW).accepted);
        assert!(provider.authorize_mint(&order).await.is_err());

        let e2 = eth_mint_envelope(&order, &o2, 7, "nonce-i2");
        assert!(provider.submit_attestation_at(&e2, &order, NOW).accepted);

        let auth = provider.authorize_mint(&order).await.unwrap();
        assert_eq!(auth.scheme, SignatureScheme::Secp256k1);
        assert_eq!(auth.threshold, 2);
        assert_eq!(auth.signatures.len(), 2);
        assert!(auth.meets_threshold());
        // The authorization carries the Safe nonce the signatures are bound
        // to, so the minter can cross-check it pre-broadcast (#848).
        assert_eq!(auth.safe_nonce, Some(7));

        // Every collected payload signature is a Safe owner signature over
        // THIS order's SafeTx digest at the attested Safe nonce — directly
        // consumable by Safe.execTransaction.
        let to: Address = order.dest_address.parse().unwrap();
        let calldata =
            encode_bridge_mint_calldata(to, U256::from(order.net_amount()), order.order_id_bytes());
        let digest = safe_tx_hash(
            CHAIN_ID,
            safe_addr(),
            wbth_addr(),
            &calldata,
            U256::from(7u64),
        );
        let owners: Vec<Address> = vec![o1.address(), o2.address()];
        for sig in &auth.signatures {
            let ecdsa = EcdsaSignature::from_raw(&sig.signature).unwrap();
            let recovered = ecdsa.recover_address_from_prehash(&digest).unwrap();
            assert!(owners.contains(&recovered));
            assert_eq!(sig.signer, recovered.as_slice().to_vec());
        }
    }

    #[test]
    fn pipeline_rejects_eth_attestation_at_a_different_safe_nonce() {
        let (o1, o2) = (eth_signer(1), eth_signer(2));
        let provider = eth_provider(&[&o1, &o2], 2, 7, None);
        let order = eth_mint_order();

        let e1 = eth_mint_envelope(&order, &o1, 7, "nonce-j1");
        assert!(provider.submit_attestation_at(&e1, &order, NOW).accepted);

        // Signatures over different Safe nonces cannot share one
        // execTransaction — the set refuses to mix them.
        let e2 = eth_mint_envelope(&order, &o2, 8, "nonce-j2");
        let outcome = provider.submit_attestation_at(&e2, &order, NOW);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:invalid_payload");
        assert_eq!(provider.progress(order.id, "bridge.mint_wbth"), 1);
    }

    #[tokio::test]
    async fn authorize_mint_eth_evicts_a_set_pinned_behind_the_onchain_nonce() {
        // A partial set is pinned to Safe nonce N (below threshold). An
        // unrelated Safe transaction then advances the on-chain nonce to N+1.
        // Without eviction, every fresh-nonce attestation is refused as a
        // nonce mismatch and the order wedges forever (#848 / #849). The
        // aggregator must drop the stale set and re-collect at N+1.
        use std::sync::atomic::{AtomicU64, Ordering};
        let (o1, o2) = (eth_signer(1), eth_signer(2));
        let source = Arc::new(MutableSafeNonce(AtomicU64::new(7)));
        // The local Safe owner is o1: authorize_mint self-attests.
        let provider =
            eth_provider_with_nonce_source(&[&o1, &o2], 2, Arc::clone(&source), Some(o1.clone()));
        let order = eth_mint_order();

        // First pass at nonce 7: self-attest (o1) pins the set to 7, below
        // threshold (needs 2), so authorize_mint fails safe.
        assert!(provider.authorize_mint(&order).await.is_err());
        assert_eq!(provider.progress(order.id, "bridge.mint_wbth"), 1);

        // An unrelated Safe tx executes: the on-chain nonce advances to 8.
        // The set is still pinned to 7. A peer envelope at the fresh nonce 8
        // cannot join the stale set (correct within-set refusal).
        source.0.store(8, Ordering::SeqCst);
        let e2_at_8 = eth_mint_envelope(&order, &o2, 8, "nonce-evict-1");
        let refused = provider.submit_attestation_at(&e2_at_8, &order, NOW);
        assert!(
            !refused.accepted,
            "fresh-nonce attestation cannot join a stale set"
        );

        // Next authorize_mint pass evicts the stale (nonce-7) set and
        // re-collects fresh at nonce 8: o1 self-attests at 8, then an o2
        // envelope at 8 reaches threshold.
        assert!(provider.authorize_mint(&order).await.is_err()); // evicted + re-self-attested (1/2)
        let e2_fresh = eth_mint_envelope(&order, &o2, 8, "nonce-evict-2");
        assert!(
            provider
                .submit_attestation_at(&e2_fresh, &order, NOW)
                .accepted,
            "after eviction the set is pinned to 8 and accepts the o2 envelope"
        );

        let auth = provider
            .authorize_mint(&order)
            .await
            .expect("order recovers at the fresh nonce without a restart");
        assert_eq!(auth.signatures.len(), 2);
        assert_eq!(auth.safe_nonce, Some(8), "re-collected at the fresh nonce");
    }

    #[tokio::test]
    async fn authorize_mint_eth_does_not_evict_on_equal_or_stale_read() {
        // Guard the eviction: an EQUAL on-chain nonce is the healthy case and
        // must not drop the set; a transient on-chain read reported BEHIND the
        // pinned nonce must also never evict (#848 edge cases).
        use std::sync::atomic::{AtomicU64, Ordering};
        let (o1, o2) = (eth_signer(1), eth_signer(2));
        let source = Arc::new(MutableSafeNonce(AtomicU64::new(7)));
        let provider =
            eth_provider_with_nonce_source(&[&o1, &o2], 2, Arc::clone(&source), Some(o1.clone()));
        let order = eth_mint_order();

        // Pin the set to nonce 7 (o1 self-attest, below threshold).
        assert!(provider.authorize_mint(&order).await.is_err());
        assert_eq!(provider.progress(order.id, "bridge.mint_wbth"), 1);

        // Equal nonce: no eviction — the set survives (still 1/2 at nonce 7).
        assert!(provider.authorize_mint(&order).await.is_err());
        assert_eq!(provider.progress(order.id, "bridge.mint_wbth"), 1);

        // A transient stale read reports the nonce BEHIND the pinned value:
        // must not evict.
        source.0.store(5, Ordering::SeqCst);
        assert!(provider.authorize_mint(&order).await.is_err());
        assert_eq!(
            provider.progress(order.id, "bridge.mint_wbth"),
            1,
            "a behind-read must not drop the set"
        );

        // With the nonce back at 7, the o2 envelope completes the set.
        source.0.store(7, Ordering::SeqCst);
        let e2 = eth_mint_envelope(&order, &o2, 7, "nonce-equal-1");
        assert!(provider.submit_attestation_at(&e2, &order, NOW).accepted);
        let auth = provider.authorize_mint(&order).await.unwrap();
        assert_eq!(auth.signatures.len(), 2);
        assert_eq!(auth.safe_nonce, Some(7));
    }

    #[tokio::test]
    async fn authorize_mint_solana_carries_no_safe_nonce() {
        let (k1, k2) = (signing_key(10), signing_key(11));
        let provider = FederationAttestationProvider {
            eth: None,
            sol: Some(Ed25519Federation {
                signers: vec![k1.verifying_key(), k2.verifying_key()],
                threshold: 1,
            }),
            bth: None,
            local_ed25519: Some(k1.clone()),
            local_secp256k1: None,
            tracker: empty_tracker(),
            peer_push: None,
        };
        let order = sol_mint_order();
        let auth = provider.authorize_mint(&order).await.unwrap();
        assert_eq!(auth.scheme, SignatureScheme::Ed25519);
        assert_eq!(
            auth.safe_nonce, None,
            "Solana mints are not Safe-nonce bound"
        );
    }

    #[test]
    fn pipeline_rejects_eth_envelope_from_a_non_owner() {
        let (o1, byzantine) = (eth_signer(1), eth_signer(66));
        let provider = eth_provider(&[&o1], 1, 7, None);
        let order = eth_mint_order();

        let envelope = eth_mint_envelope(&order, &byzantine, 7, "nonce-k");
        let outcome = provider.submit_attestation_at(&envelope, &order, NOW);
        assert!(!outcome.accepted);
        assert_eq!(outcome.tag, "refused:unknown_signer");
    }

    // -- construction / configuration ---------------------------------------

    #[test]
    fn from_config_none_when_no_federation_configured() {
        let config = BridgeConfig::default();
        assert!(FederationAttestationProvider::from_config(&config)
            .unwrap()
            .is_none());
    }

    #[test]
    fn from_config_rejects_zero_or_unsatisfiable_threshold() {
        // #842 at the provider layer: signers with threshold 0 is a
        // configuration error, never a permissive federation.
        let mut config = BridgeConfig::default();
        config.solana.mint_signers = vec![hex::encode(signing_key(1).verifying_key().as_bytes())];
        config.solana.mint_threshold = 0;
        let err = FederationAttestationProvider::from_config(&config)
            .err()
            .expect("threshold 0 with signers must be refused");
        assert!(err.contains("threshold must be >= 1"), "{err}");

        config.solana.mint_threshold = 2; // only one signer
        let err = FederationAttestationProvider::from_config(&config)
            .err()
            .expect("threshold above n must be refused");
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn from_config_rejects_duplicate_ed25519_signers() {
        // A repeated signer inflates the configured count; the aggregator
        // dedups by identity, so a threshold equal to the padded count is
        // unsatisfiable. Reject the misconfig at construction (#848).
        let key = hex::encode(signing_key(1).verifying_key().as_bytes());
        let mut config = BridgeConfig::default();
        config.solana.mint_signers = vec![key.clone(), key];
        config.solana.mint_threshold = 2;
        let err = FederationAttestationProvider::from_config(&config)
            .err()
            .expect("duplicate solana signers must be refused");
        assert!(err.contains("duplicate signer identities"), "{err}");

        // Distinct signers still construct.
        let mut config = BridgeConfig::default();
        config.solana.mint_signers = vec![
            hex::encode(signing_key(1).verifying_key().as_bytes()),
            hex::encode(signing_key(2).verifying_key().as_bytes()),
        ];
        config.solana.mint_threshold = 2;
        assert!(FederationAttestationProvider::from_config(&config).is_ok());
    }

    #[test]
    fn from_config_rejects_duplicate_eth_owners_checksum_insensitive() {
        // The same owner spelled two ways (checksummed vs lowercase) must
        // collide on the parsed 20-byte address, not the raw string (#848).
        let addr = format!("0x{}", hex::encode([0x11u8; 20]));
        let mut config = BridgeConfig::default();
        config.ethereum.mint_signers = vec![addr.to_uppercase().replace("0X", "0x"), addr.clone()];
        config.ethereum.mint_threshold = 2;
        config.ethereum.safe_address = Some(format!("0x{}", hex::encode([0x22u8; 20])));
        config.ethereum.wbth_contract = format!("0x{}", hex::encode([0x33u8; 20]));
        let err = FederationAttestationProvider::from_config(&config)
            .err()
            .expect("duplicate eth owners must be refused");
        assert!(err.contains("duplicate signer identities"), "{err}");
    }
}
