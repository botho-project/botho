//! Minimal JSON-RPC client for the hosted Botho testnet nodes.
//!
//! The hosted testnet nodes (seed.botho.io, seed2 @34.220.36.204,
//! faucet.botho.io) expose their JSON-RPC over TLS behind nginx at the `/rpc`
//! path, returning camelCase fields. That transport differs from
//! `botho-wallet`'s `RpcPool` (which speaks plain `http://<socketaddr>` with
//! snake_case structs and a hardcoded discovery list), so the mobile bridge
//! uses this small client to talk to whichever node URL the app selected via
//! `set_node_url`.
//!
//! All cryptography (ownership scan, key-image derivation, CLSAG build/sign)
//! still runs through the shared, node-identical `bth_wasm_signer::core`
//! primitives -- this module is purely transport.

use bth_wasm_signer::core::LotteryContext;
use serde::Deserialize;
use serde_json::{json, Value};

/// A thin JSON-RPC 2.0 client bound to a single node base URL.
pub struct NodeRpc {
    client: reqwest::Client,
    /// Fully-qualified RPC endpoint (e.g. `https://faucet.botho.io/rpc`).
    endpoint: String,
}

/// A single output as returned by `chain_getOutputs`.
#[derive(Debug, Clone, Deserialize)]
pub struct RpcOutput {
    #[serde(rename = "targetKey")]
    pub target_key: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    #[serde(rename = "amountCommitment")]
    pub amount_commitment: String,
    /// The output's position within its creating transaction. Under protocol
    /// 6.0.0 this index is bound into the hybrid one-time key, so the RECEIVE
    /// scan needs it to detect hybrid outputs (issue #988). Coinbase outputs
    /// are reported as `u32::MAX` by the node and normalized to
    /// `MINTING_OUTPUT_INDEX` (0) by the consumer. Defaults to 0 when
    /// absent.
    #[serde(rename = "outputIndex", default)]
    pub output_index: u32,
    /// Hex-encoded ML-KEM-768 ciphertext, or `null` for a classical/legacy
    /// KEM-less output (issue #970). The scan decapsulates it to detect the
    /// hybrid one-time key.
    #[serde(rename = "kemCiphertext", default)]
    pub kem_ciphertext: Option<String>,
    /// The index the output's one-time key is derived at (`cryptoOutputIndex`).
    /// Equal to `outputIndex` for ordinary outputs, 0 for the coinbase, and
    /// `lotteryContext.baseIndex` for a V2 lottery payout. Absent on older
    /// nodes, in which case the normalized `outputIndex` is used.
    #[serde(rename = "cryptoOutputIndex", default)]
    pub crypto_output_index: Option<u32>,
    /// Lottery payout row. Without a `lotteryContext` it is a LEGACY (V1)
    /// payout that aliases its source's key: excluded from inputs and decoys
    /// (#1450).
    #[serde(default)]
    pub lottery: bool,
    /// V2 lottery payout derivation context (frozen #1641 RPC contract). The
    /// shared signer core scans the base target `P − T·G` at `baseIndex` and
    /// recovers the payout key with the `x·G == P` check (#1645).
    #[serde(rename = "lotteryContext", default)]
    pub lottery_context: Option<LotteryContext>,
}

/// Spent/pending status of a key image from `chain_areKeyImagesSpent`.
#[derive(Debug, Clone, Deserialize)]
pub struct KeyImageStatus {
    #[serde(rename = "keyImage")]
    pub key_image: String,
    #[serde(default)]
    pub spent: bool,
    #[serde(default)]
    pub pending: bool,
}

impl NodeRpc {
    /// Build a client for a node base URL.
    ///
    /// `base_url` may be a bare host (`https://faucet.botho.io`) or already
    /// include the `/rpc` path; both are normalized to the `/rpc` endpoint.
    pub fn new(base_url: &str) -> Self {
        let trimmed = base_url.trim_end_matches('/');
        let endpoint = if trimmed.ends_with("/rpc") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/rpc")
        };
        Self {
            client: reqwest::Client::new(),
            endpoint,
        }
    }

    /// Execute a JSON-RPC call, returning the `result` value or an error
    /// string.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1,
        });

        let resp = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request to {} failed: {e}", self.endpoint))?;

        if !resp.status().is_success() {
            return Err(format!("node returned HTTP {}", resp.status()));
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| format!("invalid JSON-RPC response: {e}"))?;

        if let Some(err) = value.get("error") {
            if !err.is_null() {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown RPC error");
                return Err(format!("RPC error ({method}): {msg}"));
            }
        }

        value
            .get("result")
            .cloned()
            .ok_or_else(|| format!("RPC response for {method} had no result"))
    }

    /// Current chain height (`getChainInfo.height`).
    pub async fn chain_height(&self) -> Result<u64, String> {
        let result = self.call("getChainInfo", json!({})).await?;
        result
            .get("height")
            .and_then(|h| h.as_u64())
            .ok_or_else(|| "getChainInfo response missing height".to_string())
    }

    /// All outputs in `[start, end]` flattened across blocks.
    ///
    /// The node clamps one `chain_getOutputs` call to 1000 blocks (#1656), so
    /// this pages ([`page_blocks`]); the wallet's `[0, tip]` scan would
    /// otherwise silently stop at block 999 and miss later funds.
    pub async fn get_outputs(&self, start: u64, end: u64) -> Result<Vec<RpcOutput>, String> {
        let blocks = page_blocks(start, end, |from| async move {
            let result = self
                .call(
                    "chain_getOutputs",
                    json!({ "start_height": from, "end_height": end }),
                )
                .await?;
            result
                .as_array()
                .cloned()
                .ok_or_else(|| "chain_getOutputs did not return an array".to_string())
        })
        .await?;

        let mut outputs = Vec::new();
        for block in &blocks {
            if let Some(arr) = block.get("outputs").and_then(|o| o.as_array()) {
                for out in arr {
                    if let Ok(parsed) = serde_json::from_value::<RpcOutput>(out.clone()) {
                        outputs.push(parsed);
                    }
                }
            }
        }
        Ok(outputs)
    }

    /// Query spent/pending status for a batch of hex key images.
    pub async fn are_key_images_spent(
        &self,
        key_images: &[String],
    ) -> Result<Vec<KeyImageStatus>, String> {
        let result = self
            .call(
                "chain_areKeyImagesSpent",
                json!({ "keyImages": key_images }),
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|e| format!("invalid chain_areKeyImagesSpent response: {e}"))
    }

    /// Submit a hex-encoded signed transaction, returning its tx hash.
    pub async fn submit_transaction(&self, tx_hex: &str) -> Result<String, String> {
        let result = self.call("tx_submit", json!({ "tx_hex": tx_hex })).await?;
        result
            .get("txHash")
            .or_else(|| result.get("tx_hash"))
            .and_then(|h| h.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "tx_submit response missing txHash".to_string())
    }
}

/// Collect the `chain_getOutputs` blocks of `[start, end]` page by page
/// (#1656). `fetch(from)` requests `[from, end]`; the node serves a dense
/// prefix (at most its per-call cap, never past the tip). Resume at the last
/// served `height` + 1 until `end` is reached or a page is empty (past the
/// tip). A page that does not advance is an error, never an infinite loop.
pub async fn page_blocks<F, Fut>(start: u64, end: u64, mut fetch: F) -> Result<Vec<Value>, String>
where
    F: FnMut(u64) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Value>, String>>,
{
    let mut blocks = Vec::new();
    let mut next = start;
    while next <= end {
        let page = fetch(next).await?;
        let Some(last) = page.last() else { break };
        let last = last
            .get("height")
            .and_then(|h| h.as_u64())
            .ok_or_else(|| "chain_getOutputs block missing height".to_string())?;
        if last < next {
            return Err(format!(
                "chain_getOutputs page from {next} ended at {last}: no progress"
            ));
        }
        blocks.extend(page);
        if last >= end {
            break;
        }
        next = last + 1;
    }
    Ok(blocks)
}

/// Recover the transparent u64 amount from an `amountCommitment` hex string
/// (stored as little-endian bytes, matching the node's transparent-amount
/// model).
pub fn amount_from_commitment(hex_str: &str) -> u64 {
    match hex::decode(hex_str) {
        Ok(bytes) if bytes.len() >= 8 => {
            u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node with blocks `0..=tip` that serves at most `cap` per call.
    fn capped_page(tip: u64, cap: u64, from: u64, end: u64) -> Vec<Value> {
        (from..=end.min(from.saturating_add(cap - 1)))
            .take_while(|h| *h <= tip)
            .map(|h| json!({"height": h, "outputs": []}))
            .collect()
    }

    fn heights(blocks: &[Value]) -> Vec<u64> {
        blocks
            .iter()
            .map(|b| b["height"].as_u64().unwrap())
            .collect()
    }

    /// #1656: the wallet's `[0, tip]` scan against a node that clamps each
    /// call to 1000 blocks returns every block exactly once.
    #[tokio::test]
    async fn page_blocks_covers_the_range_across_the_server_cap() {
        let tip = 2_500u64;
        let calls = std::cell::Cell::new(0);
        let blocks = page_blocks(0, tip, |from| {
            calls.set(calls.get() + 1);
            let page = capped_page(tip, 1000, from, tip);
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(heights(&blocks), (0..=tip).collect::<Vec<_>>());
        assert_eq!(calls.get(), 3);

        // Past the tip: stops at the first empty page.
        let blocks = page_blocks(2_498, 9_999, |from| {
            let page = capped_page(tip, 2, from, 9_999);
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(heights(&blocks), vec![2_498, 2_499, 2_500]);

        // A non-advancing page is an error, not a hang.
        assert!(page_blocks(10, 20, |_| async {
            Ok(vec![json!({"height": 3, "outputs": []})])
        })
        .await
        .is_err());
    }
}
