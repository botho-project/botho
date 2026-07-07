//! HTTP API for serving fleet metrics
//!
//! Endpoints (contract for the wallet network dashboard, see issue #697):
//! - GET /api/metrics/latest -> JSON array, one entry per node: {node,
//!   timestamp, height, peerCount, scpPeerCount, mempoolSize, mintingActive,
//!   uptimeSeconds, heightStale}
//! - GET /api/metrics/history?node=<name>&resolution=5min|hourly|daily&
//!   since=<unix-seconds> -> JSON array of samples for that node: {timestamp,
//!   height, peerCount, scpPeerCount, mempoolSize, txTotal}
//! - GET /health

use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tower_http::cors::{Any, CorsLayer};

use crate::db::{MetricsDb, Resolution};

/// Shared application state
type AppState = Arc<Mutex<MetricsDb>>;

/// Error response
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

/// Start the API server
pub async fn serve(addr: String, db: Arc<Mutex<MetricsDb>>) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/metrics/history", get(history))
        .route("/api/metrics/latest", get(latest))
        .layer(cors)
        .with_state(db);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Health check endpoint
async fn health() -> impl IntoResponse {
    "OK"
}

/// Query parameters for the history endpoint
#[derive(Debug, Deserialize)]
struct HistoryParams {
    node: Option<String>,
    resolution: Option<String>,
    since: Option<i64>,
}

/// Get historical metrics for one node
async fn history(
    State(db): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> impl IntoResponse {
    let Some(node) = params.node else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "missing required query parameter: node",
        );
    };

    let resolution_str = params.resolution.as_deref().unwrap_or("5min");
    let Some(resolution) = Resolution::parse(resolution_str) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid resolution: expected 5min, hourly, or daily",
        );
    };

    let since = params.since.unwrap_or(0);

    let db_lock = db.lock().unwrap();
    match db_lock.query_node_history(&node, resolution, since) {
        Ok(data) => (StatusCode::OK, Json(data)).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Get latest sample for every node (empty array until first collection)
async fn latest(State(db): State<AppState>) -> impl IntoResponse {
    let db_lock = db.lock().unwrap();

    match db_lock.get_latest_per_node() {
        Ok(entries) => (StatusCode::OK, Json(entries)).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn error_response(status: StatusCode, message: &str) -> axum::response::Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}
