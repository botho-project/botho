// Copyright (c) 2024 The Botho Foundation

//! Proof-of-reserves HTTP API (#825).
//!
//! Endpoints (contract for the metrics-daemon / `/network` dashboard):
//! - `GET /api/reserve/proof` -> the latest reconciliation snapshot:
//!   `{lockedReserve, ethSupply, solSupply, totalWrapped, drift, inTolerance,
//!   pegHealthy, takenAt}` (503 until the first pass runs).
//! - `GET /health` -> `OK`.
//!
//! The full per-chain detail (including in-flight allowances) is written
//! to the `audit_log` by the reconciler; this surface serves the compact
//! snapshot the dashboard renders.

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::broadcast;
use tracing::info;

use crate::db::{Database, ReserveSnapshot};

/// Response body of `GET /api/reserve/proof`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveProofResponse {
    /// Total locked reserve per the ledger, picocredits.
    pub locked_reserve: u64,
    /// Verified wBTH totalSupply on Ethereum, picocredits.
    pub eth_supply: Option<u64>,
    /// Verified wBTH supply on Solana, picocredits (pending #828).
    pub sol_supply: Option<u64>,
    /// Σ of the verified supplies, picocredits.
    pub total_wrapped: Option<u64>,
    /// Σ(verified supply) − Σ(locked backing of verified chains).
    pub drift: i64,
    /// All verified chains within tolerance + in-flight allowance.
    pub in_tolerance: bool,
    /// Peg red/green state (tolerance AND custody legs).
    pub peg_healthy: bool,
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
            taken_at: s.taken_at,
        }
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// Build the API router.
pub fn router(db: Database) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/reserve/proof", get(reserve_proof))
        .with_state(db)
}

/// Serve the API until shutdown.
pub async fn serve(
    addr: String,
    db: Database,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {} failed: {}", addr, e))?;
    info!("Proof-of-reserves API listening on {}", addr);

    axum::serve(listener, router(db))
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
            info!("Proof-of-reserves API shutting down");
        })
        .await
        .map_err(|e| format!("API server error: {}", e))
}

async fn health() -> impl IntoResponse {
    "OK"
}

/// Serve the latest proof-of-reserves snapshot.
async fn reserve_proof(State(db): State<Database>) -> axum::response::Response {
    match db.latest_reserve_snapshot() {
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

    fn snapshot() -> ReserveSnapshot {
        ReserveSnapshot {
            taken_at: 1_752_000_000,
            locked_reserve: 1_500,
            eth_supply: Some(1_000),
            sol_supply: None,
            drift: -500,
            in_tolerance: true,
            peg_healthy: true,
        }
    }

    #[test]
    fn test_response_contract_field_names() {
        // The exact JSON keys the metrics-daemon / dashboard hook consume
        // (issue #825 API contract).
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

    #[tokio::test]
    async fn test_endpoint_serves_latest_snapshot_over_http() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let (shutdown_tx, _) = broadcast::channel(1);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(db.clone());
        let mut shutdown_rx = shutdown_tx.subscribe();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                })
                .await
                .unwrap();
        });

        // Before any reconciliation: 503.
        let status = http_get(addr, "/api/reserve/proof").await.0;
        assert_eq!(status, 503);

        // Health always answers.
        let (status, body) = http_get(addr, "/health").await;
        assert_eq!(status, 200);
        assert_eq!(body, "OK");

        // After a snapshot: 200 with the contract body.
        db.insert_reserve_snapshot(&snapshot()).unwrap();
        let (status, body) = http_get(addr, "/api/reserve/proof").await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["lockedReserve"], 1_500);
        assert_eq!(json["inTolerance"], true);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    /// Minimal HTTP/1.1 GET over a raw socket (avoids an HTTP-client
    /// dev-dependency). Returns (status, body).
    async fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!(
                    "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    path, addr
                )
                .as_bytes(),
            )
            .await
            .unwrap();
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
