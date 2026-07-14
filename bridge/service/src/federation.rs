// Copyright (c) 2024 The Botho Foundation

//! Federation attestation envelope transport (#858).
//!
//! The #824 attestation pipeline
//! ([`FederationAttestationProvider`](crate::attestation::FederationAttestationProvider))
//! verifies, replay-checks, order-binds, and thresholds signed envelopes —
//! but each validator bridge node only ever sees its OWN local signer's
//! envelope until they are exchanged over the network. Without that exchange
//! any threshold above 1 never authorizes in production (fail-safe by
//! design). This module is that exchange, per ADR 0002 (the SCP validator set
//! doubles as the t-of-n bridge federation):
//!
//! - **Inbound** ([`attest`]): an authenticated `POST /api/attest` endpoint
//!   that accepts an [`AttestationEnvelope`] (JSON), peeks its bound order id,
//!   routes it to the on-record order, and calls
//!   [`FederationAttestationProvider::submit_attestation`]. It is PURE
//!   TRANSPORT in front of the existing fail-closed verify pipeline — it never
//!   weakens verification. Adversarial envelopes (replayed nonce, wrong order,
//!   unknown signer) are rejected by that pipeline with the existing
//!   [`AttestationRejectReason`](bth_bridge_core::AttestationRejectReason)
//!   refuse reasons, surfaced as HTTP status codes.
//! - **Outbound** ([`PeerBroadcaster`]): when a node self-attests it pushes the
//!   accepted envelope to every configured peer's inbound endpoint.
//!
//! **Transport security.** Envelopes are self-authenticating (a signature
//! over domain-separated bytes plus a single-use replay nonce), so the
//! transport needs integrity / anti-DoS but NOT confidentiality. A shared
//! bearer secret gates the inbound endpoint — rejecting unauthenticated
//! floods BEFORE any signature work — and authenticates outbound pushes. The
//! raw envelope signature remains the only thing that authorizes an
//! attestation; the bearer token is a DoS fence, not a trust anchor (a
//! stolen token still cannot forge a federation signature).
//!
//! **Ethereum Safe nonce.** For Ethereum mints every collected payload
//! signature must bind the SAME Gnosis Safe nonce (signatures over different
//! nonces cannot share one `execTransaction`). The provider stamps its
//! self-attestation with the set's
//! [`safe_nonce`](bth_bridge_core::AttestationSet::safe_nonce) and pushes THAT
//! envelope, so peers ingesting it bind the same nonce; a peer whose set
//! already pinned a different nonce refuses the envelope
//! (`refused:invalid_payload`) rather than mixing incompatible signatures.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use bth_bridge_core::{peek_order_id, AttestationEnvelope};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, warn};

use crate::{
    attestation::{EnvelopePush, FederationAttestationProvider},
    db::Database,
};

/// Shared state of the inbound `POST /api/attest` endpoint.
#[derive(Clone)]
pub struct AttestState {
    /// The #824 provider whose fail-closed ingest pipeline every received
    /// envelope flows through.
    pub provider: Arc<FederationAttestationProvider>,
    /// Order lookup: the peeked order id is routed to this on-record order,
    /// then re-bound field-by-field by the verify pipeline.
    pub db: Database,
    /// Shared bearer secret a peer must present. `None` disables the auth
    /// gate (trusted private network only).
    pub inbound_auth_token: Option<String>,
}

/// The JSON body returned by the inbound endpoint (both accept and refuse).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestResponse {
    /// Whether the envelope was accepted into its threshold set.
    pub accepted: bool,
    /// Stable machine tag: `accepted` or `refused:<reason-tag>`.
    pub tag: String,
    /// Human-facing detail (safe to return to the submitter).
    pub message: String,
    /// Distinct valid signers collected for this `(order, action)` so far.
    pub signers: u32,
    /// The configured federation threshold `t`.
    pub threshold: u32,
}

/// Extract the bearer token from an `Authorization: Bearer <token>` header.
fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
}

/// Constant-time-ish comparison for the shared bearer secret. Envelopes are
/// self-authenticating so this token is a DoS fence, not the security
/// boundary; a length-independent compare still avoids a trivial timing
/// oracle on the secret.
fn token_matches(expected: &str, presented: &str) -> bool {
    let a = expected.as_bytes();
    let b = presented.as_bytes();
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

/// Inbound attestation endpoint: `POST /api/attest`.
///
/// Pure transport in front of the #824 verify pipeline. The flow is:
/// 1. bearer-auth gate (anti-DoS; rejected BEFORE any signature work);
/// 2. peek the (unverified) order id purely to route to the on-record order — a
///    lying id selects an order the signature/order-binding then rejects;
/// 3. hand the RECEIVED envelope + order to `submit_attestation`, which runs
///    the full fail-closed pipeline (signer selection → signature → parse →
///    freshness → durable nonce reserve → order binding → aggregation).
///
/// The verdict maps to HTTP status so a peer can distinguish transport
/// failures from attestation refusals: `200` accepted, `202` a valid-but-
/// not-yet-threshold or benign refusal, `401` bad bearer, `400` malformed /
/// bad-signature / unknown-signer, `404` no such order, `409` a replay /
/// wrong-order / stale post-signature refusal, `503` an internal error.
pub async fn attest(
    State(state): State<AttestState>,
    headers: HeaderMap,
    Json(envelope): Json<AttestationEnvelope>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // 1. Bearer-auth gate (anti-DoS). Only enforced when configured.
    if let Some(expected) = &state.inbound_auth_token {
        match bearer(&headers) {
            Some(presented) if token_matches(expected, &presented) => {}
            _ => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(AttestResponse {
                        accepted: false,
                        tag: "refused:unauthorized".to_string(),
                        message: "missing or invalid bearer token".to_string(),
                        signers: 0,
                        threshold: 0,
                    }),
                )
                    .into_response();
            }
        }
    }

    // 2. Route the peeked (unverified) order id to the on-record order.
    let order_id = match peek_order_id(&envelope.envelope) {
        Ok(id) => id,
        Err(r) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(AttestResponse {
                    accepted: false,
                    tag: format!("refused:{}", r.tag()),
                    message: r.message(),
                    signers: 0,
                    threshold: 0,
                }),
            )
                .into_response();
        }
    };
    let order = match state.db.get_order(&order_id) {
        Ok(Some(order)) => order,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(AttestResponse {
                    accepted: false,
                    tag: "refused:unknown_order".to_string(),
                    message: format!("no order on record for {order_id}"),
                    signers: 0,
                    threshold: 0,
                }),
            )
                .into_response();
        }
        Err(e) => {
            warn!("attest endpoint: order lookup failed: {e}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(AttestResponse {
                    accepted: false,
                    tag: "refused:internal".to_string(),
                    message: "order lookup failed".to_string(),
                    signers: 0,
                    threshold: 0,
                }),
            )
                .into_response();
        }
    };

    // 3. Full fail-closed verify pipeline (the endpoint adds NO trust).
    let outcome = state.provider.submit_attestation(&envelope, &order);

    // 3a. Equivocation detection (detection-only; funds behavior unchanged).
    // A VERIFIED envelope from a known signer that conflicts with what that
    // signer already attested for this order (different payload digest / Safe
    // nonce) is a governance-relevant Byzantine signal — audit it distinctly
    // from replayed_nonce / wrong_order / invalid_payload. The threshold set
    // is unchanged: an equivocating signer still counts once.
    if outcome.equivocation {
        let signer = outcome.signer_key_id.as_deref().unwrap_or("<unknown>");
        let action = outcome.action.as_deref().unwrap_or("<unknown>");
        let details = format!(
            "federation signer equivocated: signer={signer} order={} action={action} \
             (conflicting attestation bytes for one order; funds neutralized, still counts once)",
            order.id
        );
        warn!("attestation equivocation detected: {details}");
        if let Err(e) = state
            .db
            .log_audit(Some(&order.id), "attestation_equivocation", &details)
        {
            // Audit is observability; a failed insert must not change the
            // funds-safe HTTP verdict below.
            warn!("failed to log attestation_equivocation audit event: {e}");
        }
    }

    let status = if outcome.accepted {
        StatusCode::OK
    } else {
        match outcome.tag.as_str() {
            "refused:malformed" | "refused:bad_signature" | "refused:unknown_signer" => {
                StatusCode::BAD_REQUEST
            }
            "refused:replayed_nonce"
            | "refused:wrong_order"
            | "refused:stale"
            | "refused:invalid_payload" => StatusCode::CONFLICT,
            "refused:internal" => StatusCode::SERVICE_UNAVAILABLE,
            // not_configured / below_threshold / anything else: the envelope
            // was structurally fine, it just did not advance a set here.
            _ => StatusCode::ACCEPTED,
        }
    };
    (
        status,
        Json(AttestResponse {
            accepted: outcome.accepted,
            tag: outcome.tag,
            message: outcome.message,
            signers: outcome.signers,
            threshold: outcome.threshold,
        }),
    )
        .into_response()
}

/// Build the inbound attestation router (`POST /api/attest`).
pub fn router(state: AttestState) -> axum::Router {
    axum::Router::new()
        .route("/api/attest", axum::routing::post(attest))
        .with_state(state)
}

/// Serve the inbound attestation endpoint until shutdown. Empty `addr`
/// disables the endpoint (the caller checks this before spawning).
pub async fn serve(
    addr: String,
    state: AttestState,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr} (attest) failed: {e}"))?;
    tracing::info!("Bridge attestation endpoint listening on {addr}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
            tracing::info!("Bridge attestation endpoint shutting down");
        })
        .await
        .map_err(|e| format!("attest server error: {e}"))
}

/// Outbound transport ([`EnvelopePush`]): pushes each accepted local envelope
/// to every configured peer's inbound `POST /api/attest` endpoint.
///
/// Fire-and-forget with respect to the local authorization path: each push
/// is spawned onto the tokio runtime and its result only logged. A slow or
/// unreachable peer never wedges or fails the local self-attestation — the
/// peer will self-attest and push its own envelope back, and re-authorization
/// re-pushes. Uses a minimal raw-socket HTTP/1.1 POST (no HTTP-client
/// dependency; the envelope is self-authenticating so plain HTTP integrity +
/// the bearer fence suffice for v1 — mTLS can wrap it at the transport layer).
pub struct PeerBroadcaster {
    peers: Vec<PeerEndpoint>,
    auth_token: Option<String>,
    timeout: std::time::Duration,
}

/// Maximum number of bytes read from a peer's push response.
///
/// `push_one` only parses the HTTP status line (`HTTP/1.1 <status> ...`) and
/// discards the body, so the read is capped here rather than using an unbounded
/// `read_to_end`. 8 KiB comfortably covers the status line plus any response
/// headers a well-behaved peer sends, while bounding the worst-case transient
/// heap a slow/malicious peer could grow during the push timeout window.
pub const MAX_PEER_RESPONSE_BYTES: u64 = 8 * 1024;

/// Parse the HTTP status code out of a raw peer response
/// (`HTTP/1.1 <status> ...`): the second whitespace-separated token, or `0`
/// when there is no parseable status.
///
/// Pure and synchronous — the single source of truth for the status-line
/// parse. The async I/O wrapper [`read_status_line`] caps the bytes it feeds
/// in at [`MAX_PEER_RESPONSE_BYTES`]; this function itself allocates at most
/// once (the `from_utf8_lossy` copy when the input is not valid UTF-8),
/// proportional to the input it is given. Exposed through the crate's
/// library target so the fuzz crate can drive it with adversarial bytes
/// (#897).
pub fn parse_status_line(response: &[u8]) -> u16 {
    let text = String::from_utf8_lossy(response);
    text.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Read a peer's push response and parse the HTTP status code from the status
/// line (`HTTP/1.1 <status> ...`).
///
/// Only the status line is needed, so the read is capped at
/// [`MAX_PEER_RESPONSE_BYTES`] via [`AsyncReadExt::take`] rather than an
/// unbounded `read_to_end`. This bounds the transient per-task heap a
/// slow/malicious peer could grow by streaming body bytes for the whole push
/// timeout window. Truncation is harmless: the status line is always first, so
/// the capped buffer still contains everything the parse needs. A response with
/// no parseable status yields `0`.
async fn read_status_line<R>(reader: R) -> std::io::Result<u16>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut response = Vec::new();
    reader
        .take(MAX_PEER_RESPONSE_BYTES)
        .read_to_end(&mut response)
        .await?;
    Ok(parse_status_line(&response))
}

/// A parsed peer base URL split into the pieces the raw-socket client needs.
#[derive(Clone)]
struct PeerEndpoint {
    /// `host:port` connect target.
    authority: String,
    /// The `Host:` header value (authority without any userinfo).
    host_header: String,
    /// Request path (base path + `/api/attest`).
    path: String,
}

impl PeerBroadcaster {
    /// Build from configured peer base URLs (e.g.
    /// `http://bridge-2.internal:9742`). Unparseable peers are skipped with a
    /// warning rather than failing construction (a bad peer entry must not
    /// disable the whole node's outbound path). Returns `None` when no peer
    /// parses (nothing to broadcast to).
    pub fn new(
        peers: &[String],
        auth_token: Option<String>,
        timeout: std::time::Duration,
    ) -> Option<Self> {
        let parsed: Vec<PeerEndpoint> = peers
            .iter()
            .filter_map(|p| match parse_peer(p) {
                Ok(ep) => Some(ep),
                Err(e) => {
                    warn!("federation peer `{p}` ignored: {e}");
                    None
                }
            })
            .collect();
        if parsed.is_empty() {
            return None;
        }
        Some(Self {
            peers: parsed,
            auth_token,
            timeout,
        })
    }

    /// Send one envelope to one peer over a raw HTTP/1.1 connection.
    async fn push_one(
        peer: PeerEndpoint,
        body: String,
        auth_token: Option<String>,
        timeout: std::time::Duration,
    ) {
        let attempt = async {
            let mut stream = tokio::net::TcpStream::connect(&peer.authority)
                .await
                .map_err(|e| format!("connect {}: {e}", peer.authority))?;
            let auth = match &auth_token {
                Some(t) => format!("Authorization: Bearer {t}\r\n"),
                None => String::new(),
            };
            let request = format!(
                "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 {}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                peer.path,
                peer.host_header,
                auth,
                body.len(),
                body
            );
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|e| format!("write {}: {e}", peer.authority))?;
            let status = read_status_line(&mut stream)
                .await
                .map_err(|e| format!("read {}: {e}", peer.authority))?;
            Ok::<u16, String>(status)
        };

        match tokio::time::timeout(timeout, attempt).await {
            Ok(Ok(status)) => {
                debug!("pushed attestation to {} -> HTTP {status}", peer.authority);
            }
            Ok(Err(e)) => warn!("attestation push to {} failed: {e}", peer.authority),
            Err(_) => warn!(
                "attestation push to {} timed out after {:?}",
                peer.authority, timeout
            ),
        }
    }
}

impl EnvelopePush for PeerBroadcaster {
    fn broadcast(&self, envelope: &AttestationEnvelope) {
        let body = match serde_json::to_string(envelope) {
            Ok(b) => b,
            Err(e) => {
                warn!("cannot serialize envelope for peer push: {e}");
                return;
            }
        };
        for peer in &self.peers {
            let peer = peer.clone();
            let body = body.clone();
            let auth_token = self.auth_token.clone();
            let timeout = self.timeout;
            // Fire-and-forget: never block or fail the local authorize path.
            tokio::spawn(Self::push_one(peer, body, auth_token, timeout));
        }
    }
}

/// Parse a peer base URL into a raw-socket target. Accepts `http://` and
/// `https://` schemes (the scheme only informs the default port here — TLS
/// termination for `https` peers is expected at a reverse proxy / mTLS layer
/// in front of the plain endpoint for v1). Rejects anything without a host.
fn parse_peer(url: &str) -> Result<PeerEndpoint, String> {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s, r),
        None => ("http", url),
    };
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        other => return Err(format!("unsupported scheme `{other}`")),
    };
    // Split authority from path.
    let (authority, base_path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err("empty host".to_string());
    }
    // The connect target needs an explicit port; the Host header keeps the
    // authority verbatim.
    let connect = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:{default_port}")
    };
    let path = format!("{base_path}/api/attest");
    Ok(PeerEndpoint {
        authority: connect,
        host_header: authority.to_string(),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_peer_defaults_port_and_appends_path() {
        let ep = parse_peer("http://bridge-2.internal:9742").unwrap();
        assert_eq!(ep.authority, "bridge-2.internal:9742");
        assert_eq!(ep.host_header, "bridge-2.internal:9742");
        assert_eq!(ep.path, "/api/attest");

        // No port -> default 80 for http, 443 for https.
        let ep = parse_peer("http://peer.example").unwrap();
        assert_eq!(ep.authority, "peer.example:80");
        let ep = parse_peer("https://peer.example").unwrap();
        assert_eq!(ep.authority, "peer.example:443");

        // A base path is preserved and the endpoint path appended.
        let ep = parse_peer("http://gw.example:8080/bridge/").unwrap();
        assert_eq!(ep.authority, "gw.example:8080");
        assert_eq!(ep.path, "/bridge/api/attest");

        // Scheme-less is treated as http.
        let ep = parse_peer("127.0.0.1:9742").unwrap();
        assert_eq!(ep.authority, "127.0.0.1:9742");
    }

    #[test]
    fn parse_peer_rejects_bad_input() {
        assert!(parse_peer("ftp://x").is_err());
        assert!(parse_peer("http://").is_err());
    }

    #[test]
    fn token_matches_is_exact() {
        assert!(token_matches("s3cret", "s3cret"));
        assert!(!token_matches("s3cret", "s3cre"));
        assert!(!token_matches("s3cret", "s3crett"));
        assert!(!token_matches("s3cret", "wrong"));
        assert!(token_matches("", ""));
    }

    #[test]
    fn bearer_extracts_token() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer abc123".parse().unwrap(),
        );
        assert_eq!(bearer(&h), Some("abc123".to_string()));

        let empty = HeaderMap::new();
        assert_eq!(bearer(&empty), None);
    }
}

/// Two-node federation transport tests (#858 DoD): envelopes travel through
/// the real `POST /api/attest` endpoint over the wire — NO direct injection
/// into any node's `AttestationSet`. Each node runs its own provider + DB +
/// inbound endpoint, and pushes its self-attestation to the OTHER node via a
/// real [`PeerBroadcaster`].
#[cfg(test)]
#[allow(clippy::type_complexity)]
mod federation_transport_tests {
    use super::*;
    use crate::attestation::{AttestationProvider, FederationAttestationProvider, SafeNonceSource};
    use alloy::{
        primitives::{Address, B256},
        signers::local::PrivateKeySigner,
    };
    use async_trait::async_trait;
    use bth_bridge_core::{BridgeOrder, Chain, OrderStatus, OrderType};
    use ed25519_dalek::SigningKey;
    use std::time::Duration;
    use tokio::sync::broadcast;

    const AUTH: &str = "shared-federation-secret";

    fn ed_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn eth_key(seed: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([seed; 32])).unwrap()
    }

    struct FixedNonce(u64);
    #[async_trait]
    impl SafeNonceSource for FixedNonce {
        async fn safe_nonce(&self) -> Result<u64, String> {
            Ok(self.0)
        }
    }

    /// A node whose inbound endpoint is bound (port known) but not yet
    /// serving — so we can wire each node's outbound push to the OTHER
    /// node's URL before either starts serving, breaking the circular
    /// endpoint/provider dependency.
    struct PendingNode {
        listener: tokio::net::TcpListener,
        url: String,
        db: Database,
    }

    async fn pending_node(order: &BridgeOrder) -> PendingNode {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        PendingNode {
            listener,
            url: format!("http://{addr}"),
            db: db_with_order(order),
        }
    }

    /// A broadcaster pushing to one peer URL with the shared bearer secret.
    fn push_to(url: &str) -> Arc<PeerBroadcaster> {
        Arc::new(
            PeerBroadcaster::new(
                &[url.to_string()],
                Some(AUTH.to_string()),
                Duration::from_secs(5),
            )
            .unwrap(),
        )
    }

    /// Start serving `provider` on a pre-bound listener; returns the shutdown
    /// sender + task handle.
    fn serve_pending(
        node: PendingNode,
        provider: Arc<FederationAttestationProvider>,
    ) -> (broadcast::Sender<()>, tokio::task::JoinHandle<()>) {
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let state = AttestState {
            provider,
            db: node.db,
            inbound_auth_token: Some(AUTH.to_string()),
        };
        let app = router(state);
        let handle = tokio::spawn(async move {
            axum::serve(node.listener, app)
                .with_graceful_shutdown(async move {
                    let mut rx = shutdown_rx;
                    let _ = rx.recv().await;
                })
                .await
                .unwrap();
        });
        (shutdown_tx, handle)
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

    /// Persist `order` into a fresh in-memory DB (each node has its own; the
    /// endpoint looks the routed order up here).
    fn db_with_order(order: &BridgeOrder) -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.insert_order(order).unwrap();
        db
    }

    /// Bind an endpoint on an ephemeral port and start serving `provider`
    /// immediately (for single-node adversarial tests). Returns the base URL,
    /// shutdown sender, and task handle.
    async fn spawn_endpoint(
        provider: Arc<FederationAttestationProvider>,
        db: Database,
    ) -> (String, broadcast::Sender<()>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let node = PendingNode {
            listener,
            url: url.clone(),
            db,
        };
        let (sd, handle) = serve_pending(node, provider);
        (url, sd, handle)
    }

    /// Poll until `cond` holds or the deadline elapses (peer pushes are
    /// fire-and-forget over the network, so propagation is asynchronous).
    async fn wait_for(mut cond: impl FnMut() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not met within timeout (envelope did not propagate over the wire)");
    }

    #[tokio::test]
    async fn two_node_release_authorizes_over_the_wire_no_injection() {
        // Federation {A, B}, threshold 2. Each node self-attests with its
        // OWN key and pushes to the other's endpoint over the wire; neither
        // ever inserts into the other's set directly.
        let (ka, kb) = (ed_key(11), ed_key(22));
        let federation = vec![ka.verifying_key(), kb.verifying_key()];
        let order = burn_order();

        // Bind both endpoints (ports known) BEFORE wiring push, so each
        // node's outbound broadcaster targets the other's real URL.
        let pending_a = pending_node(&order).await;
        let pending_b = pending_node(&order).await;

        let node_a = Arc::new(
            FederationAttestationProvider::new_bth_for_test(&federation, 2, ka)
                .with_peer_push(push_to(&pending_b.url)),
        );
        let node_b = Arc::new(
            FederationAttestationProvider::new_bth_for_test(&federation, 2, kb)
                .with_peer_push(push_to(&pending_a.url)),
        );

        let (sd_a, srv_a) = serve_pending(pending_a, node_a.clone());
        let (sd_b, srv_b) = serve_pending(pending_b, node_b.clone());

        // -- Drive the protocol --------------------------------------------
        // A self-attests (1/2 locally) and pushes A's envelope to B's wire.
        assert!(
            node_a.authorize_release(&order).await.is_err(),
            "A alone is 1/2"
        );
        // B receives A's envelope over the wire -> B set reaches 1/2.
        wait_for(|| node_b.distinct_signers_for_test(order.id, "bridge.release_bth") == 1).await;

        // B self-attests (now 2/2 at B) and returns a full authorization;
        // B also pushes B's envelope back to A's wire.
        let auth_b = node_b
            .authorize_release(&order)
            .await
            .expect("B reaches threshold via A's wired envelope + its own");
        assert_eq!(auth_b.signatures.len(), 2);
        assert!(auth_b.meets_threshold());

        // A receives B's envelope over the wire -> A set reaches 2/2.
        wait_for(|| node_a.distinct_signers_for_test(order.id, "bridge.release_bth") == 2).await;
        let auth_a = node_a
            .authorize_release(&order)
            .await
            .expect("A reaches threshold via B's wired envelope + its own");
        assert_eq!(auth_a.signatures.len(), 2);

        // Both authorizations verify against the pinned federation — proof
        // the wired envelopes carry real federation signatures.
        crate::release::bth::validate_release_attestation(&order, &auth_a, &federation, 2)
            .expect("A's collected authorization must satisfy the release validator");
        crate::release::bth::validate_release_attestation(&order, &auth_b, &federation, 2)
            .expect("B's collected authorization must satisfy the release validator");

        let _ = sd_a.send(());
        let _ = sd_b.send(());
        let _ = srv_a.await;
        let _ = srv_b.await;
    }

    #[tokio::test]
    async fn two_node_eth_mint_authorizes_over_the_wire_no_injection() {
        // Federation {A, B} Safe owners, threshold 2, both binding Safe
        // nonce 7 (the #848/#849 nonce-agreement seam: peers must sign the
        // SAME nonce or the set refuses to mix them).
        const NONCE: u64 = 7;
        let (oa, ob) = (eth_key(11), eth_key(22));
        let owners = vec![oa.address(), ob.address()];
        let safe = Address::repeat_byte(0x5a);
        let wbth = Address::repeat_byte(0xeb);
        let chain_id = 1u64;
        let order = eth_mint_order();

        let mk = |local: PrivateKeySigner| {
            FederationAttestationProvider::new_eth_for_test(
                &owners,
                2,
                chain_id,
                safe,
                wbth,
                Arc::new(FixedNonce(NONCE)),
                local,
            )
        };

        let pending_a = pending_node(&order).await;
        let pending_b = pending_node(&order).await;
        let node_a = Arc::new(mk(oa).with_peer_push(push_to(&pending_b.url)));
        let node_b = Arc::new(mk(ob).with_peer_push(push_to(&pending_a.url)));
        let (sd_a, srv_a) = serve_pending(pending_a, node_a.clone());
        let (sd_b, srv_b) = serve_pending(pending_b, node_b.clone());

        // A self-attests (1/2) and pushes to B.
        assert!(
            node_a.authorize_mint(&order).await.is_err(),
            "A alone is 1/2"
        );
        wait_for(|| node_b.distinct_signers_for_test(order.id, "bridge.mint_wbth") == 1).await;

        // B self-attests -> 2/2, returns a full authorization.
        let auth = node_b
            .authorize_mint(&order)
            .await
            .expect("B reaches threshold via A's wired envelope + its own");
        assert_eq!(auth.signatures.len(), 2);
        assert!(auth.meets_threshold());

        // Every payload signature is a Safe owner signature over THIS order's
        // SafeTx digest at the SAME nonce 7 — directly execTransaction-ready.
        let digest = {
            use alloy::primitives::U256;
            let to: Address = order.dest_address.parse().unwrap();
            let calldata = crate::mint::ethereum::encode_bridge_mint_calldata(
                to,
                U256::from(order.net_amount()),
                order.order_id_bytes(),
            );
            crate::mint::ethereum::safe_tx_hash(chain_id, safe, wbth, &calldata, U256::from(NONCE))
        };
        for sig in &auth.signatures {
            use alloy::primitives::Signature;
            let ecdsa = Signature::from_raw(&sig.signature).unwrap();
            let recovered = ecdsa.recover_address_from_prehash(&digest).unwrap();
            assert!(owners.contains(&recovered), "signature binds a Safe owner");
        }
        assert_eq!(order.order_type, OrderType::Mint);

        let _ = sd_a.send(());
        let _ = sd_b.send(());
        let _ = srv_a.await;
        let _ = srv_b.await;
    }

    // -- Adversarial endpoint tests (rejected by the verify pipeline) -------

    /// Send a raw JSON body to a node's `/api/attest` and return (status,
    /// body). Mirrors the api.rs test client (no HTTP-client dep).
    async fn post_attest(base_url: &str, auth: Option<&str>, body: &str) -> (u16, String) {
        let addr = base_url.trim_start_matches("http://");
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let auth_hdr = match auth {
            Some(t) => format!("Authorization: Bearer {t}\r\n"),
            None => String::new(),
        };
        let request = format!(
            "POST /api/attest HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             {auth_hdr}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response).to_string();
        let status: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    /// Build a single node with an endpoint for adversarial tests.
    async fn single_node_endpoint(
        order: &BridgeOrder,
    ) -> (
        String,
        Arc<FederationAttestationProvider>,
        broadcast::Sender<()>,
        tokio::task::JoinHandle<()>,
        SigningKey,
        SigningKey,
    ) {
        let (ka, kb) = (ed_key(11), ed_key(22));
        let federation = vec![ka.verifying_key(), kb.verifying_key()];
        let provider = Arc::new(FederationAttestationProvider::new_bth_for_test(
            &federation,
            2,
            ka.clone(),
        ));
        let (url, sd, srv) = spawn_endpoint(provider.clone(), db_with_order(order)).await;
        (url, provider, sd, srv, ka, kb)
    }

    #[tokio::test]
    async fn endpoint_rejects_unknown_signer() {
        let order = burn_order();
        let (url, provider, sd, srv, _ka, _kb) = single_node_endpoint(&order).await;

        // An envelope from a non-member key: rejected pre-signature.
        let byz = ed_key(99);
        let kind = FederationAttestationProvider::release_kind_for_test(&order);
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let env =
            bth_bridge_core::sign_attestation_ed25519(&kind, &byz, "adv-unknown", now, now + 120)
                .unwrap();
        let (status, body) =
            post_attest(&url, Some(AUTH), &serde_json::to_string(&env).unwrap()).await;
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("unknown_signer"), "{body}");
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.release_bth"),
            0
        );

        let _ = sd.send(());
        let _ = srv.await;
    }

    #[tokio::test]
    async fn endpoint_rejects_wrong_order() {
        // Sign a valid envelope for `order`, but the DB record under that
        // same order id has a DIFFERENT net amount — the peeked id routes to
        // it, the signature verifies, and order binding then rejects the
        // amount mismatch (post-signature `wrong_order`).
        let order = burn_order();
        let (ka, kb) = (ed_key(11), ed_key(22));
        let federation = vec![ka.verifying_key(), kb.verifying_key()];
        let provider = Arc::new(FederationAttestationProvider::new_bth_for_test(
            &federation,
            2,
            ka.clone(),
        ));

        let kind = FederationAttestationProvider::release_kind_for_test(&order);
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let env =
            bth_bridge_core::sign_attestation_ed25519(&kind, &ka, "adv-wrong", now, now + 120)
                .unwrap();

        // Persist a tampered copy under the same order id (fee changed so the
        // net amount differs from the signed envelope).
        let mut tampered = order.clone();
        tampered.fee = order.fee + 1;
        assert_ne!(tampered.net_amount(), order.net_amount());
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.insert_order(&tampered).unwrap();
        let (url, sd, srv) = spawn_endpoint(provider.clone(), db).await;

        let (status, body) =
            post_attest(&url, Some(AUTH), &serde_json::to_string(&env).unwrap()).await;
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("wrong_order"), "{body}");
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.release_bth"),
            0
        );

        // A completely absent order id routes to a 404 (unknown_order).
        let absent = burn_order();
        let kind2 = FederationAttestationProvider::release_kind_for_test(&absent);
        let env2 =
            bth_bridge_core::sign_attestation_ed25519(&kind2, &ka, "adv-absent", now, now + 120)
                .unwrap();
        let (status, body) =
            post_attest(&url, Some(AUTH), &serde_json::to_string(&env2).unwrap()).await;
        assert_eq!(status, 404, "{body}");
        assert!(body.contains("unknown_order"), "{body}");

        let _ = sd.send(());
        let _ = srv.await;
    }

    #[tokio::test]
    async fn endpoint_rejects_replayed_envelope() {
        let order = burn_order();
        let (url, provider, sd, srv, ka, _kb) = single_node_endpoint(&order).await;

        let kind = FederationAttestationProvider::release_kind_for_test(&order);
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let env =
            bth_bridge_core::sign_attestation_ed25519(&kind, &ka, "adv-replay", now, now + 120)
                .unwrap();
        let body = serde_json::to_string(&env).unwrap();

        // First submission accepted.
        let (status, resp) = post_attest(&url, Some(AUTH), &body).await;
        assert_eq!(status, 200, "{resp}");
        assert!(resp.contains("accepted"), "{resp}");
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.release_bth"),
            1
        );

        // The SAME envelope again: the durable nonce reserve rejects it.
        let (status, resp) = post_attest(&url, Some(AUTH), &body).await;
        assert_eq!(status, 409, "{resp}");
        assert!(resp.contains("replayed_nonce"), "{resp}");
        // Still exactly one distinct signer — the replay never double-counts.
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.release_bth"),
            1
        );

        let _ = sd.send(());
        let _ = srv.await;
    }

    #[tokio::test]
    async fn endpoint_rejects_bad_bearer() {
        let order = burn_order();
        let (url, provider, sd, srv, ka, _kb) = single_node_endpoint(&order).await;

        let kind = FederationAttestationProvider::release_kind_for_test(&order);
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let env = bth_bridge_core::sign_attestation_ed25519(&kind, &ka, "adv-auth", now, now + 120)
            .unwrap();
        let body = serde_json::to_string(&env).unwrap();

        // Wrong token: 401 BEFORE any signature work.
        let (status, resp) = post_attest(&url, Some("wrong"), &body).await;
        assert_eq!(status, 401, "{resp}");
        // No token at all: 401.
        let (status, _resp) = post_attest(&url, None, &body).await;
        assert_eq!(status, 401);
        // Nothing was ingested.
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.release_bth"),
            0
        );

        let _ = sd.send(());
        let _ = srv.await;
    }

    // -- Equivocation detection (#859) --------------------------------------

    /// Build an Ethereum-mint `AttestationKind` for `order` at `safe_nonce`
    /// (the private `mint_kind` is not exposed; the fields are fully
    /// determined by the order so we construct it directly).
    fn eth_mint_kind(order: &BridgeOrder, safe_nonce: u64) -> bth_bridge_core::AttestationKind {
        bth_bridge_core::AttestationKind::MintWbth {
            dest_chain: order.dest_chain,
            dest_address: order.dest_address.clone(),
            amount: order.net_amount(),
            order_id: order.id,
            source_tx: order.source_tx.clone().unwrap(),
            safe_nonce: Some(safe_nonce),
        }
    }

    #[tokio::test]
    async fn endpoint_audits_equivocation_exactly_once_per_signer() {
        // A VERIFIED Safe owner that already counted for an order then submits
        // a CONFLICTING attestation (a DIFFERENT Safe nonce for the same
        // order) — the classic equivocation move. It must:
        //   * raise a distinct `attestation_equivocation` audit event,
        //   * fire that alarm EXACTLY ONCE per signer (repeated conflicts do not spam
        //     the log),
        //   * NEVER inflate the threshold set (funds-safety unchanged), and
        //   * not fire for benign identical re-sends or for a second honest signer.
        use crate::attestation::sign_attestation_secp256k1;

        const N7: u64 = 7;
        let (oa, ob) = (eth_key(11), eth_key(22));
        let owners = vec![oa.address(), ob.address()];
        let safe = Address::repeat_byte(0x5a);
        let wbth = Address::repeat_byte(0xeb);
        let chain_id = 1u64;
        let order = eth_mint_order();

        let provider = Arc::new(FederationAttestationProvider::new_eth_for_test(
            &owners,
            2,
            chain_id,
            safe,
            wbth,
            Arc::new(FixedNonce(N7)),
            oa.clone(),
        ));

        // Hold a clone of the DB the endpoint audits into so we can count rows.
        let db = db_with_order(&order);
        let audit_db = db.clone();
        let (url, sd, srv) = spawn_endpoint(provider.clone(), db).await;

        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let sign = |safe_nonce: u64, replay_nonce: &str| {
            let kind = eth_mint_kind(&order, safe_nonce);
            sign_attestation_secp256k1(
                &kind,
                &oa,
                chain_id,
                safe,
                wbth,
                replay_nonce,
                now,
                now + 120,
            )
            .unwrap()
        };

        // 1) Owner A attests at Safe nonce 7 — accepted, counts once.
        let e_ok = sign(N7, "eq-ok");
        let (status, body) =
            post_attest(&url, Some(AUTH), &serde_json::to_string(&e_ok).unwrap()).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.mint_wbth"),
            1
        );
        assert_eq!(
            audit_db
                .count_audit_action("attestation_equivocation")
                .unwrap(),
            0
        );

        // 2) Owner A EQUIVOCATES: same order, CONFLICTING Safe nonce 8. The set refuses
        //    to combine it (invalid_payload) AND flags it as an equivocation — one
        //    audit row.
        let e_conflict = sign(8, "eq-conflict-1");
        let (status, body) = post_attest(
            &url,
            Some(AUTH),
            &serde_json::to_string(&e_conflict).unwrap(),
        )
        .await;
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("invalid_payload"), "{body}");
        // Still exactly one counted signer — the conflict never counts.
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.mint_wbth"),
            1
        );
        assert_eq!(
            audit_db
                .count_audit_action("attestation_equivocation")
                .unwrap(),
            1
        );

        // 3) Owner A keeps equivocating (Safe nonce 9): still refused, but the alarm
        //    fires EXACTLY ONCE per signer — no second audit row.
        let e_conflict2 = sign(9, "eq-conflict-2");
        let (status, _body) = post_attest(
            &url,
            Some(AUTH),
            &serde_json::to_string(&e_conflict2).unwrap(),
        )
        .await;
        assert_eq!(status, 409);
        assert_eq!(
            audit_db
                .count_audit_action("attestation_equivocation")
                .unwrap(),
            1,
            "equivocation must be audited exactly once per signer"
        );

        // 4) Owner A benignly re-sends its ORIGINAL nonce-7 attestation bytes (fresh
        //    anti-replay nonce): a benign duplicate, NOT an alarm.
        let e_benign = sign(N7, "eq-benign");
        let (status, body) =
            post_attest(&url, Some(AUTH), &serde_json::to_string(&e_benign).unwrap()).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            audit_db
                .count_audit_action("attestation_equivocation")
                .unwrap(),
            1,
            "a benign identical re-send must not raise the equivocation alarm"
        );

        // 5) A DIFFERENT honest owner (B) attests at Safe nonce 7 — accepted, reaches
        //    threshold, and raises NO equivocation alarm.
        let e_b = {
            let kind = eth_mint_kind(&order, N7);
            sign_attestation_secp256k1(&kind, &ob, chain_id, safe, wbth, "eq-b", now, now + 120)
                .unwrap()
        };
        let (status, body) =
            post_attest(&url, Some(AUTH), &serde_json::to_string(&e_b).unwrap()).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            provider.distinct_signers_for_test(order.id, "bridge.mint_wbth"),
            2
        );
        assert_eq!(
            audit_db
                .count_audit_action("attestation_equivocation")
                .unwrap(),
            1,
            "two distinct honest signers must not trigger the equivocation alarm"
        );

        let _ = sd.send(());
        let _ = srv.await;
    }

    // -- Bounded peer-response read (#874) ----------------------------------

    /// A well-behaved peer returning `HTTP/1.1 200 OK` followed by a short body
    /// still parses to status 200 (a short read below the cap is unaffected).
    #[tokio::test]
    async fn read_status_line_parses_normal_short_response() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        server
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await
            .unwrap();
        server.shutdown().await.unwrap();
        let status = read_status_line(&mut client).await.unwrap();
        assert_eq!(status, 200);
    }

    /// A peer that streams a valid status line followed by far more than the
    /// cap still parses the status without reading (or allocating) past the
    /// cap.
    #[tokio::test]
    async fn read_status_line_is_bounded_on_flood() {
        // Status line + headers, then a body larger than MAX_PEER_RESPONSE_BYTES.
        let mut payload =
            b"HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\n\r\n".to_vec();
        let flood_len = (MAX_PEER_RESPONSE_BYTES as usize) * 4;
        payload.extend(std::iter::repeat_n(b'x', flood_len));

        // Instrument the read path: `take(cap).read_to_end` must never buffer
        // more than the cap even though the peer sent 4x that.
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(payload.clone());
            (&mut tokio::io::BufReader::new(cursor))
                .take(MAX_PEER_RESPONSE_BYTES)
                .read_to_end(&mut buf)
                .await
                .unwrap();
        }
        assert_eq!(
            buf.len() as u64,
            MAX_PEER_RESPONSE_BYTES,
            "read must stop at the cap even under a flood"
        );

        // And the shared helper still parses the status from the capped buffer.
        let reader = tokio::io::BufReader::new(std::io::Cursor::new(payload));
        let status = read_status_line(reader).await.unwrap();
        assert_eq!(status, 202, "status line parses despite oversized body");
    }

    /// A peer that closes immediately after the status line (short read, well
    /// below the cap) still parses correctly.
    #[tokio::test]
    async fn read_status_line_handles_status_only_response() {
        let reader = std::io::Cursor::new(b"HTTP/1.1 500 Internal Server Error\r\n\r\n".to_vec());
        let status = read_status_line(reader).await.unwrap();
        assert_eq!(status, 500);
    }

    /// An empty / unparseable response yields status 0 (unchanged behavior).
    #[tokio::test]
    async fn read_status_line_defaults_to_zero_on_empty() {
        let reader = std::io::Cursor::new(Vec::<u8>::new());
        let status = read_status_line(reader).await.unwrap();
        assert_eq!(status, 0);
    }
}
