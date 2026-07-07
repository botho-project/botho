//! Metrics collection from a fleet of Botho nodes
//!
//! Every collection tick each configured node is polled concurrently.
//! Per-node failures are logged and skipped: nothing is recorded for a
//! failed poll (no fabricated samples), and one down node never blocks
//! collection from the others.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

use crate::db::{MetricsDb, MetricsSample};

/// A node to poll: display name + RPC URL
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub name: String,
    pub url: String,
}

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
pub struct NodeStatus {
    pub chain_height: u64,
    pub peer_count: u64,
    pub scp_peer_count: u64,
    pub mempool_size: u64,
    pub total_transactions: u64,
    pub uptime_seconds: u64,
    pub minting_active: bool,
}

/// Parse a JSON-RPC `node_getStatus` response body into a NodeStatus
pub fn parse_status_response(body: &str) -> Result<NodeStatus> {
    let rpc_response: RpcResponse =
        serde_json::from_str(body).context("Failed to parse RPC response")?;

    if let Some(error) = rpc_response.error {
        anyhow::bail!("RPC error {}: {}", error.code, error.message);
    }

    rpc_response.result.context("No result in RPC response")
}

/// Fetch node_getStatus from a single node
async fn fetch_status(client: &reqwest::Client, url: &str) -> Result<NodeStatus> {
    let request = RpcRequest {
        jsonrpc: "2.0",
        method: "node_getStatus",
        params: serde_json::json!({}),
        id: 1,
    };

    let response = client
        .post(url)
        .json(&request)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .context("Failed to connect to node")?;

    let body = response
        .text()
        .await
        .context("Failed to read RPC response body")?;

    parse_status_response(&body)
}

/// Poll every configured node concurrently and store one sample per node
/// that responded. Returns the number of nodes successfully collected.
pub async fn collect_metrics(
    client: &reqwest::Client,
    nodes: &[NodeConfig],
    db: &Arc<Mutex<MetricsDb>>,
) -> Result<usize> {
    // Round to nearest 5 minutes once, so all nodes in this tick share a
    // timestamp (one row per (node, timestamp)).
    let now = chrono::Utc::now().timestamp();
    let rounded_ts = (now / 300) * 300;

    // Poll all nodes concurrently; a slow/down node must not block others.
    let mut tasks = tokio::task::JoinSet::new();
    for node in nodes {
        let client = client.clone();
        let node = node.clone();
        tasks.spawn(async move {
            let status = fetch_status(&client, &node.url).await;
            (node, status)
        });
    }

    let mut collected = 0usize;
    while let Some(joined) = tasks.join_next().await {
        let (node, status) = joined.context("collection task panicked")?;

        let status = match status {
            Ok(status) => status,
            Err(e) => {
                // Record nothing for a failed poll; do not fabricate data.
                warn!("Failed to collect from node '{}': {:#}", node.name, e);
                continue;
            }
        };

        debug!(
            "Node '{}': height={}, peers={}",
            node.name, status.chain_height, status.peer_count
        );

        let mut db_lock = db.lock().unwrap();

        // Per-node tx delta
        let last_tx = db_lock
            .get_last_tx_count(&node.name)?
            .unwrap_or(status.total_transactions);
        let tx_delta = status.total_transactions.saturating_sub(last_tx) as i64;
        db_lock.set_last_tx_count(&node.name, status.total_transactions)?;

        let sample = MetricsSample {
            node: node.name.clone(),
            timestamp: rounded_ts,
            height: status.chain_height,
            peer_count: status.peer_count as f64,
            scp_peer_count: status.scp_peer_count as f64,
            mempool_size: status.mempool_size as f64,
            tx_delta,
            uptime_seconds: status.uptime_seconds,
            minting_active: status.minting_active,
        };
        db_lock.insert_sample(&sample)?;
        collected += 1;

        debug!("Stored metrics sample: {:?}", sample);
    }

    Ok(collected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_response_ok() {
        let body = r#"{
            "jsonrpc": "2.0",
            "result": {
                "chainHeight": 12345,
                "peerCount": 4,
                "scpPeerCount": 3,
                "mempoolSize": 7,
                "totalTransactions": 999,
                "uptimeSeconds": 86400,
                "mintingActive": true
            },
            "id": 1
        }"#;

        let status = parse_status_response(body).unwrap();
        assert_eq!(status.chain_height, 12345);
        assert_eq!(status.peer_count, 4);
        assert_eq!(status.scp_peer_count, 3);
        assert_eq!(status.mempool_size, 7);
        assert_eq!(status.total_transactions, 999);
        assert_eq!(status.uptime_seconds, 86400);
        assert!(status.minting_active);
    }

    #[test]
    fn test_parse_status_response_rpc_error() {
        let body =
            r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"method not found"},"id":1}"#;
        let err = parse_status_response(body).unwrap_err();
        assert!(err.to_string().contains("-32601"));
    }

    #[test]
    fn test_parse_status_response_missing_result() {
        let body = r#"{"jsonrpc":"2.0","id":1}"#;
        assert!(parse_status_response(body).is_err());
    }

    #[test]
    fn test_parse_status_response_garbage() {
        assert!(parse_status_response("not json").is_err());
    }
}
