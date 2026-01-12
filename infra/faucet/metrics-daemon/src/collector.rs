//! Metrics collection from Botho node

use std::sync::{Arc, Mutex};
use anyhow::{Result, Context};
use serde::Deserialize;
use tracing::debug;

use crate::db::{MetricsDb, MetricsSample};

/// JSON-RPC request structure
#[derive(serde::Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    method: &'static str,
    params: serde_json::Value,
    id: u32,
}

/// JSON-RPC response structure
#[derive(Deserialize)]
struct RpcResponse {
    result: Option<NodeStatus>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Node status from node_getStatus RPC
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeStatus {
    chain_height: u64,
    peer_count: u64,
    scp_peer_count: u64,
    mempool_size: u64,
    total_transactions: u64,
    uptime_seconds: u64,
    minting_active: bool,
}

/// Collect metrics from the node and store in database
pub async fn collect_metrics(node_url: &str, db: &Arc<Mutex<MetricsDb>>) -> Result<()> {
    // Build RPC request
    let request = RpcRequest {
        jsonrpc: "2.0",
        method: "node_getStatus",
        params: serde_json::json!({}),
        id: 1,
    };

    // Call node RPC
    let client = reqwest::Client::new();
    let response = client
        .post(node_url)
        .json(&request)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .context("Failed to connect to node")?;

    let rpc_response: RpcResponse = response
        .json()
        .await
        .context("Failed to parse RPC response")?;

    // Check for RPC error
    if let Some(error) = rpc_response.error {
        anyhow::bail!("RPC error {}: {}", error.code, error.message);
    }

    let status = rpc_response.result
        .context("No result in RPC response")?;

    debug!("Received node status: height={}, peers={}", status.chain_height, status.peer_count);

    // Calculate tx_delta
    let mut db_lock = db.lock().unwrap();
    let last_tx = db_lock.get_last_tx_count()?.unwrap_or(status.total_transactions);
    let tx_delta = status.total_transactions.saturating_sub(last_tx) as i64;

    // Update last tx count
    db_lock.set_last_tx_count(status.total_transactions)?;

    // Create sample
    let now = chrono::Utc::now().timestamp();
    // Round to nearest 5 minutes for consistent timestamps
    let rounded_ts = (now / 300) * 300;

    let sample = MetricsSample {
        timestamp: rounded_ts,
        height: status.chain_height,
        peer_count: status.peer_count as f64,
        scp_peer_count: status.scp_peer_count as f64,
        mempool_size: status.mempool_size as f64,
        tx_delta,
        uptime_seconds: status.uptime_seconds,
        minting_active: status.minting_active,
    };

    // Store sample
    db_lock.insert_sample(&sample)?;

    debug!("Stored metrics sample: {:?}", sample);

    Ok(())
}
