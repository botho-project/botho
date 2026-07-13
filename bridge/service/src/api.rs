// Copyright (c) 2024 The Botho Foundation

//! Bridge HTTP API: proof-of-reserves (#825) + monitoring and the
//! circuit-breaker control surface (#827).
//!
//! Endpoints:
//! - `GET /health` -> `OK` (200) while healthy; `503` with a reason when the
//!   bridge is paused (breaker tripped / kill switch) or the DB is unreachable.
//!   Liveness probes and dashboards key off this.
//! - `GET /api/status` -> operational snapshot: pause state, order counts by
//!   status, actionable backlog, stuck-order count, component health
//!   (attestation signer / minters / releaser), latest peg verdict.
//! - `GET /metrics` -> the same data as Prometheus text (no external crate;
//!   hand-rolled exposition format) for alerting (`bridge/service/alerts.yml`).
//! - `POST /api/breaker` `{"paused": bool, "reason": "..."}` -> runtime
//!   kill-switch toggle. Pausing halts the submit stages (mints and releases);
//!   confirm stages keep running so in-flight orders settle. The listener binds
//!   localhost by default — anyone who can reach this endpoint is an operator.
//! - `GET /api/reserve/proof` -> the latest reconciliation snapshot:
//!   `{lockedReserve, ethSupply, solSupply, totalWrapped, drift, inTolerance,
//!   pegHealthy, reserveBalanceChecked, takenAt}` (503 until the first pass
//!   runs). `reserveBalanceChecked` (#846) distinguishes "custody checked OK"
//!   from "custody never checked".
//!
//! The full per-chain detail (including in-flight allowances) is written
//! to the `audit_log` by the reconciler; this surface serves the compact
//! snapshots the dashboard renders.

use std::net::ToSocketAddrs;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::db::{Database, ReserveSnapshot};

/// Shared state of the API router.
#[derive(Clone)]
pub struct ApiState {
    /// Bridge database (orders, breaker state, snapshots, health).
    pub db: Database,
    /// Age threshold in seconds after which a non-terminal post-deposit
    /// order counts as stuck.
    pub stuck_after_secs: i64,
}

/// Response body of `GET /api/reserve/proof`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveProofResponse {
    /// Total locked reserve per the ledger, picocredits.
    pub locked_reserve: u64,
    /// Verified wBTH totalSupply on Ethereum, picocredits.
    pub eth_supply: Option<u64>,
    /// Verified wBTH supply on Solana, picocredits (pending #853).
    pub sol_supply: Option<u64>,
    /// Σ of the verified supplies, picocredits.
    pub total_wrapped: Option<u64>,
    /// Σ(verified supply) − Σ(locked backing of verified chains).
    pub drift: i64,
    /// All verified chains within tolerance + in-flight allowance.
    pub in_tolerance: bool,
    /// Peg red/green state (tolerance AND custody legs).
    pub peg_healthy: bool,
    /// Whether the on-Botho reserve-balance custody leg was actually
    /// checked this pass (#846: `pegHealthy: true` with
    /// `reserveBalanceChecked: false` means "peg healthy, custody
    /// unverified").
    pub reserve_balance_checked: bool,
    /// When the reconciliation ran (unix seconds).
    pub taken_at: i64,
}

impl From<ReserveSnapshot> for ReserveProofResponse {
    fn from(s: ReserveSnapshot) -> Self {
        let total_wrapped = match (s.eth_supply, s.sol_supply) {
            (None, None) => None,
            (eth, sol) => Some(eth.unwrap_or(0).saturating_add(sol.unwrap_or(0))),
        };
        Self {
            locked_reserve: s.locked_reserve,
            eth_supply: s.eth_supply,
            sol_supply: s.sol_supply,
            total_wrapped,
            drift: s.drift,
            in_tolerance: s.in_tolerance,
            peg_healthy: s.peg_healthy,
            reserve_balance_checked: s.reserve_balance_checked,
            taken_at: s.taken_at,
        }
    }
}

/// One component row in `GET /api/status`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    /// Component name (`attestation`, `minter:ethereum`, `releaser:bth`, ...).
    pub component: String,
    /// Whether the component is available.
    pub healthy: bool,
    /// Human-readable detail.
    pub detail: String,
}

/// Response body of `GET /api/status`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    /// Whether the circuit breaker / kill switch is engaged.
    pub paused: bool,
    /// Why, when paused.
    pub paused_reason: Option<String>,
    /// Order counts per status (`failed` folds all failure reasons).
    pub orders: std::collections::BTreeMap<String, i64>,
    /// Orders the engine still has to act on.
    pub actionable_backlog: u64,
    /// Post-deposit orders that have not advanced within the threshold.
    pub stuck_orders: u64,
    /// Signer / minter / releaser availability.
    pub components: Vec<ComponentStatus>,
    /// Latest reconciliation verdict (`null` until the first pass).
    pub peg_healthy: Option<bool>,
    /// When the latest reconciliation ran (unix seconds).
    pub peg_checked_at: Option<i64>,
}

/// Request body of `POST /api/breaker`.
#[derive(Debug, Deserialize)]
pub struct BreakerRequest {
    /// Desired pause state.
    pub paused: bool,
    /// Reason recorded with a pause (ignored on resume).
    #[serde(default)]
    pub reason: Option<String>,
}

/// Response body of `POST /api/breaker`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakerResponse {
    /// The pause state after the request.
    pub paused: bool,
    /// The recorded reason, when paused.
    pub paused_reason: Option<String>,
    /// Whether this request changed the state.
    pub changed: bool,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// Build the API router.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/api/status", get(status))
        .route("/api/breaker", post(breaker))
        .route("/api/reserve/proof", get(reserve_proof))
        .with_state(state)
}

/// Classify a bind spec as reachable only from loopback.
///
/// Returns `true` when **every** resolved socket address is a loopback IP
/// (`127.0.0.0/8` for IPv4, `::1` for IPv6) — i.e. the API is only reachable
/// from the local host. Returns `false` when any resolved address is
/// non-loopback (`0.0.0.0`, `::`, or a concrete external IP), meaning the
/// unauthenticated breaker/status surface is exposed beyond localhost.
///
/// An address that cannot be resolved is treated as non-loopback (`false`)
/// so callers err on the side of warning; in practice an unresolvable bind
/// spec already fails earlier at `TcpListener::bind`, so this only runs on a
/// successful bind.
fn is_loopback_bind(addr: &str) -> bool {
    match addr.to_socket_addrs() {
        Ok(mut resolved) => {
            let mut any = false;
            for sa in resolved.by_ref() {
                any = true;
                if !sa.ip().is_loopback() {
                    return false;
                }
            }
            // No addresses resolved -> cannot confirm loopback -> warn.
            any
        }
        Err(_) => false,
    }
}

/// Serve the API until shutdown.
pub async fn serve(
    addr: String,
    db: Database,
    stuck_after_secs: i64,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {} failed: {}", addr, e))?;
    info!("Bridge API listening on {}", addr);

    // The breaker/status control surface is unauthenticated; its only defense
    // is network placement (loopback by default). Warn loudly when the bind
    // exposes it beyond localhost so an operator who set `0.0.0.0` to scrape
    // /metrics remotely isn't silently exposing the un-pause kill switch.
    if !is_loopback_bind(&addr) {
        warn!(
            "Bridge API bound to non-loopback address {} — POST /api/breaker is an \
             UNAUTHENTICATED kill switch (pause/RESUME) and /api/status leaks operational \
             detail. Anyone who can reach this address can defeat a fail-closed auto-trip \
             during a peg incident. Put it behind a reverse proxy with auth or restrict access \
             with firewall rules; see docs/operations/runbooks/bridge-order-engine-recovery.md",
            addr
        );
    }

    axum::serve(
        listener,
        router(ApiState {
            db,
            stuck_after_secs,
        }),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.recv().await;
        info!("Bridge API shutting down");
    })
    .await
    .map_err(|e| format!("API server error: {}", e))
}

/// Liveness + breaker state: 200 `OK` while operating, 503 when paused
/// or the DB is unreachable.
async fn health(State(state): State<ApiState>) -> axum::response::Response {
    match state.db.is_paused() {
        Ok(None) => (StatusCode::OK, "OK").into_response(),
        Ok(Some(reason)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("paused: {}", reason),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
            .into_response(),
    }
}

fn collect_status(state: &ApiState) -> Result<StatusResponse, String> {
    let paused_reason = state.db.is_paused()?;
    let orders: std::collections::BTreeMap<String, i64> =
        state.db.order_status_counts()?.into_iter().collect();
    let actionable_backlog = state.db.actionable_backlog()?;
    let stuck_orders = state.db.stuck_orders(state.stuck_after_secs)?.len() as u64;
    let components = state
        .db
        .component_health()?
        .into_iter()
        .map(|(component, healthy, detail)| ComponentStatus {
            component,
            healthy,
            detail,
        })
        .collect();
    let snapshot = state.db.latest_reserve_snapshot()?;

    Ok(StatusResponse {
        paused: paused_reason.is_some(),
        paused_reason,
        orders,
        actionable_backlog,
        stuck_orders,
        components,
        peg_healthy: snapshot.as_ref().map(|s| s.peg_healthy),
        peg_checked_at: snapshot.as_ref().map(|s| s.taken_at),
    })
}

/// Operational status snapshot.
async fn status(State(state): State<ApiState>) -> axum::response::Response {
    match collect_status(&state) {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
            .into_response(),
    }
}

/// Prometheus text exposition of the status snapshot.
async fn metrics(State(state): State<ApiState>) -> axum::response::Response {
    let status = match collect_status(&state) {
        Ok(s) => s,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("# error: {}", e)).into_response()
        }
    };
    let locked = state.db.locked_reserve_total().unwrap_or(0);
    let snapshot = state.db.latest_reserve_snapshot().ok().flatten();

    let mut out = String::new();
    out.push_str("# TYPE bridge_paused gauge\n");
    out.push_str(&format!("bridge_paused {}\n", status.paused as u8));
    out.push_str("# TYPE bridge_orders gauge\n");
    for (bucket, count) in &status.orders {
        out.push_str(&format!(
            "bridge_orders{{status=\"{}\"}} {}\n",
            bucket, count
        ));
    }
    out.push_str("# TYPE bridge_actionable_backlog gauge\n");
    out.push_str(&format!(
        "bridge_actionable_backlog {}\n",
        status.actionable_backlog
    ));
    out.push_str("# TYPE bridge_stuck_orders gauge\n");
    out.push_str(&format!("bridge_stuck_orders {}\n", status.stuck_orders));
    out.push_str("# TYPE bridge_component_healthy gauge\n");
    for component in &status.components {
        out.push_str(&format!(
            "bridge_component_healthy{{component=\"{}\"}} {}\n",
            component.component, component.healthy as u8
        ));
    }
    out.push_str("# TYPE bridge_reserve_locked_picocredits gauge\n");
    out.push_str(&format!("bridge_reserve_locked_picocredits {}\n", locked));
    if let Some(s) = snapshot {
        out.push_str("# TYPE bridge_peg_healthy gauge\n");
        out.push_str(&format!("bridge_peg_healthy {}\n", s.peg_healthy as u8));
        out.push_str("# TYPE bridge_reserve_drift_picocredits gauge\n");
        out.push_str(&format!("bridge_reserve_drift_picocredits {}\n", s.drift));
        out.push_str("# TYPE bridge_reserve_balance_checked gauge\n");
        out.push_str(&format!(
            "bridge_reserve_balance_checked {}\n",
            s.reserve_balance_checked as u8
        ));
    }

    (StatusCode::OK, out).into_response()
}

/// Runtime kill-switch toggle.
async fn breaker(
    State(state): State<ApiState>,
    Json(request): Json<BreakerRequest>,
) -> axum::response::Response {
    let reason = request
        .reason
        .clone()
        .unwrap_or_else(|| "manual operator action via /api/breaker".to_string());

    let changed = match state.db.set_paused(request.paused, Some(&reason)) {
        Ok(changed) => changed,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
                .into_response()
        }
    };

    if changed {
        let action = if request.paused {
            "breaker_tripped"
        } else {
            "breaker_resumed"
        };
        if let Err(e) = state
            .db
            .log_audit(None, action, &format!("via /api/breaker: {}", reason))
        {
            warn!("breaker audit log failed: {}", e);
        }
        warn!(
            "Circuit breaker {} via API ({})",
            if request.paused { "PAUSED" } else { "RESUMED" },
            reason
        );
    }

    let paused_reason = state.db.is_paused().unwrap_or(None);
    (
        StatusCode::OK,
        Json(BreakerResponse {
            paused: paused_reason.is_some(),
            paused_reason,
            changed,
        }),
    )
        .into_response()
}

/// Serve the latest proof-of-reserves snapshot.
async fn reserve_proof(State(state): State<ApiState>) -> axum::response::Response {
    match state.db.latest_reserve_snapshot() {
        Ok(Some(snapshot)) => {
            (StatusCode::OK, Json(ReserveProofResponse::from(snapshot))).into_response()
        }
        Ok(None) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "no reconciliation has run yet".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_bridge_core::{BridgeOrder, Chain, OrderStatus};

    fn snapshot() -> ReserveSnapshot {
        ReserveSnapshot {
            taken_at: 1_752_000_000,
            locked_reserve: 1_500,
            eth_supply: Some(1_000),
            sol_supply: None,
            drift: -500,
            in_tolerance: true,
            peg_healthy: true,
            reserve_balance_checked: false,
        }
    }

    fn test_state() -> (ApiState, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        (
            ApiState {
                db: db.clone(),
                stuck_after_secs: 3_600,
            },
            db,
        )
    }

    #[test]
    fn test_is_loopback_bind_classification() {
        // #855: loopback binds must NOT warn.
        assert!(is_loopback_bind("127.0.0.1:9741"));
        assert!(is_loopback_bind("127.0.0.5:9741")); // 127.0.0.0/8
        assert!(is_loopback_bind("[::1]:9741"));

        // Non-loopback binds MUST warn (helper returns false).
        assert!(!is_loopback_bind("0.0.0.0:9741"));
        assert!(!is_loopback_bind("[::]:9741"));
        assert!(!is_loopback_bind("192.0.2.10:9741")); // concrete external IP
    }

    #[test]
    fn test_response_contract_field_names() {
        // The exact JSON keys the metrics-daemon / dashboard hook consume
        // (issue #825 API contract; #846 adds reserveBalanceChecked).
        let response = ReserveProofResponse::from(snapshot());
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();

        for key in [
            "lockedReserve",
            "ethSupply",
            "solSupply",
            "totalWrapped",
            "drift",
            "inTolerance",
            "pegHealthy",
            "reserveBalanceChecked",
            "takenAt",
        ] {
            assert!(json.get(key).is_some(), "missing contract field {}", key);
        }
        assert_eq!(json["lockedReserve"], 1_500);
        assert_eq!(json["ethSupply"], 1_000);
        assert_eq!(json["solSupply"], serde_json::Value::Null);
        assert_eq!(json["totalWrapped"], 1_000);
        assert_eq!(json["drift"], -500);
        assert_eq!(json["pegHealthy"], true);
        assert_eq!(
            json["reserveBalanceChecked"], false,
            "pegHealthy without custody verification must be distinguishable (#846)"
        );
    }

    #[test]
    fn test_total_wrapped_none_until_any_chain_verified() {
        let mut s = snapshot();
        s.eth_supply = None;
        s.sol_supply = None;
        let response = ReserveProofResponse::from(s);
        assert_eq!(response.total_wrapped, None);

        let mut s = snapshot();
        s.sol_supply = Some(500);
        let response = ReserveProofResponse::from(s);
        assert_eq!(response.total_wrapped, Some(1_500));
    }

    async fn spawn_server(
        state: ApiState,
    ) -> (
        std::net::SocketAddr,
        broadcast::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown_tx, _) = broadcast::channel(1);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state);
        let mut shutdown_rx = shutdown_tx.subscribe();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                })
                .await
                .unwrap();
        });
        (addr, shutdown_tx, server)
    }

    #[tokio::test]
    async fn test_endpoint_serves_latest_snapshot_over_http() {
        let (state, db) = test_state();
        let (addr, shutdown_tx, server) = spawn_server(state).await;

        // Before any reconciliation: 503.
        let status = http_request(addr, "GET", "/api/reserve/proof", None)
            .await
            .0;
        assert_eq!(status, 503);

        // Health answers while unpaused.
        let (status, body) = http_request(addr, "GET", "/health", None).await;
        assert_eq!(status, 200);
        assert_eq!(body, "OK");

        // After a snapshot: 200 with the contract body.
        db.insert_reserve_snapshot(&snapshot()).unwrap();
        let (status, body) = http_request(addr, "GET", "/api/reserve/proof", None).await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["lockedReserve"], 1_500);
        assert_eq!(json["inTolerance"], true);
        assert_eq!(json["reserveBalanceChecked"], false);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_breaker_toggle_and_health_reflect_pause() {
        let (state, db) = test_state();
        let (addr, shutdown_tx, server) = spawn_server(state).await;

        // Trip the breaker over the API.
        let (status, body) = http_request(
            addr,
            "POST",
            "/api/breaker",
            Some(r#"{"paused": true, "reason": "incident drill"}"#),
        )
        .await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["paused"], true);
        assert_eq!(json["changed"], true);
        assert_eq!(db.is_paused().unwrap().as_deref(), Some("incident drill"));
        assert_eq!(db.count_audit_action("breaker_tripped").unwrap(), 1);

        // /health now reports unhealthy (alert trigger: breaker tripped).
        let (status, body) = http_request(addr, "GET", "/health", None).await;
        assert_eq!(status, 503);
        assert!(body.contains("incident drill"), "{}", body);

        // /metrics exposes the breaker gauge for alerting.
        let (status, body) = http_request(addr, "GET", "/metrics", None).await;
        assert_eq!(status, 200);
        assert!(body.contains("bridge_paused 1"), "{}", body);

        // Resume.
        let (status, body) =
            http_request(addr, "POST", "/api/breaker", Some(r#"{"paused": false}"#)).await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["paused"], false);
        assert_eq!(db.count_audit_action("breaker_resumed").unwrap(), 1);
        let (status, _) = http_request(addr, "GET", "/health", None).await;
        assert_eq!(status, 200);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_status_and_metrics_expose_backlog_stuck_and_components() {
        let (state, db) = test_state();

        // A fresh actionable order + one stuck for 3 hours.
        let mut fresh = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000,
            0,
            "bth".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        fresh.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&fresh).unwrap();

        let mut stuck = BridgeOrder::new_mint(
            Chain::Ethereum,
            2_000,
            0,
            "bth".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        stuck.set_status(OrderStatus::MintPending);
        stuck.updated_at = chrono::Utc::now() - chrono::Duration::hours(3);
        db.insert_order(&stuck).unwrap();

        db.set_component_health("attestation", false, "federation misconfigured")
            .unwrap();

        let (addr, shutdown_tx, server) = spawn_server(state).await;

        let (status, body) = http_request(addr, "GET", "/api/status", None).await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["paused"], false);
        assert_eq!(json["orders"]["deposit_confirmed"], 1);
        assert_eq!(json["orders"]["mint_pending"], 1);
        assert_eq!(json["actionableBacklog"], 2);
        assert_eq!(json["stuckOrders"], 1, "stuck-order alert trigger");
        assert_eq!(json["components"][0]["component"], "attestation");
        assert_eq!(
            json["components"][0]["healthy"], false,
            "signer-down alert trigger"
        );
        assert_eq!(json["pegHealthy"], serde_json::Value::Null);

        let (status, body) = http_request(addr, "GET", "/metrics", None).await;
        assert_eq!(status, 200);
        assert!(body.contains("bridge_actionable_backlog 2"), "{}", body);
        assert!(body.contains("bridge_stuck_orders 1"), "{}", body);
        assert!(
            body.contains("bridge_component_healthy{component=\"attestation\"} 0"),
            "{}",
            body
        );

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    /// Minimal HTTP/1.1 request over a raw socket (avoids an HTTP-client
    /// dev-dependency). Returns (status, body).
    async fn http_request(
        addr: std::net::SocketAddr,
        method: &str,
        path: &str,
        json_body: Option<&str>,
    ) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = match json_body {
            Some(body) => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                method,
                path,
                addr,
                body.len(),
                body
            ),
            None => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                method, path, addr
            ),
        };
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
}
