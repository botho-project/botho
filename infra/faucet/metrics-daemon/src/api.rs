//! HTTP API for serving historical metrics
//!
//! Endpoints:
//! - GET /api/metrics/history?metric=height&period=24h&granularity=5min
//! - GET /api/metrics/latest
//! - GET /health

use std::sync::{Arc, Mutex};
use anyhow::Result;
use axum::{
    Router,
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{CorsLayer, Any};

use crate::db::{MetricsDb, HistoryQuery, DataPoint};

/// Shared application state
type AppState = Arc<Mutex<MetricsDb>>;

/// History response
#[derive(Serialize)]
struct HistoryResponse {
    metric: String,
    period: String,
    granularity: String,
    data: Vec<DataPoint>,
}

/// Latest metrics response
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LatestResponse {
    timestamp: i64,
    height: u64,
    peer_count: f64,
    scp_peer_count: f64,
    mempool_size: f64,
    tx_delta: i64,
    uptime_seconds: u64,
    minting_active: bool,
}

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

/// Query parameters for history endpoint
#[derive(Debug, Deserialize)]
struct HistoryParams {
    metric: Option<String>,
    period: Option<String>,
    granularity: Option<String>,
}

/// Get historical metrics
async fn history(
    State(db): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> impl IntoResponse {
    let query = HistoryQuery {
        metric: params.metric.unwrap_or_else(|| "height".to_string()),
        period: params.period.unwrap_or_else(|| "24h".to_string()),
        granularity: params.granularity.unwrap_or_else(|| "5min".to_string()),
    };

    let db_lock = db.lock().unwrap();

    match db_lock.query_history(&query) {
        Ok(data) => {
            let response = HistoryResponse {
                metric: query.metric,
                period: query.period,
                granularity: query.granularity,
                data,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = ErrorResponse {
                error: e.to_string(),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}

/// Get latest metrics sample
async fn latest(State(db): State<AppState>) -> impl IntoResponse {
    let db_lock = db.lock().unwrap();

    match db_lock.get_latest() {
        Ok(Some(sample)) => {
            let response = LatestResponse {
                timestamp: sample.timestamp,
                height: sample.height,
                peer_count: sample.peer_count,
                scp_peer_count: sample.scp_peer_count,
                mempool_size: sample.mempool_size,
                tx_delta: sample.tx_delta,
                uptime_seconds: sample.uptime_seconds,
                minting_active: sample.minting_active,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(None) => {
            let response = ErrorResponse {
                error: "No data available".to_string(),
            };
            (StatusCode::NOT_FOUND, Json(response)).into_response()
        }
        Err(e) => {
            let response = ErrorResponse {
                error: e.to_string(),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}
