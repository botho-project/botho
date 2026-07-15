// Copyright (c) 2024 The Botho Foundation

//! Thin JSON-RPC client for a live BTH node (#856).
//!
//! This is the transport the [`crate::watchers::bth::NodeBthClient`] deposit
//! scan and the [`crate::release::bth::BthReleaser`] release path use to talk
//! to a real node over `BthConfig::rpc_url`. It is deliberately a small,
//! typed wrapper over exactly the node methods the bridge needs:
//!
//! - `getChainInfo` — tip height (finality cursor).
//! - `chain_getOutputs` — the wallet-sync output stream over a height range,
//!   used for BOTH deposit view-key scanning AND release decoy gathering (the
//!   node returns the same transparent-amount output shape the web wallet
//!   consumes).
//! - `tx_submit` — broadcast a bincode-serialized signed transaction.
//! - `getTransaction` — a submitted tx's inclusion height + confirmations.
//! - `chain_areKeyImagesSpent` — the double-spend set, used to detect a
//!   provably-dead release (its inputs spent by a *different* tx) so the engine
//!   can safely unwind and re-sign.
//!
//! Every method returns [`RpcError`], which the callers map onto their own
//! retryable error variants — a transport failure never fabricates success.

use serde::Deserialize;
use serde_json::{json, Value};

/// A JSON-RPC transport failure or a node-side error response.
#[derive(Debug, Clone)]
pub enum RpcError {
    /// Network / HTTP failure (retryable).
    Transport(String),
    /// The node returned a JSON-RPC `error` object.
    Node { code: i64, message: String },
    /// A well-formed response whose `result` did not match the expected
    /// shape (a node/bridge version skew — treated as retryable transport
    /// noise rather than a silent wrong answer).
    Decode(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Transport(m) => write!(f, "rpc transport error: {m}"),
            RpcError::Node { code, message } => {
                write!(f, "node rpc error {code}: {message}")
            }
            RpcError::Decode(m) => write!(f, "rpc decode error: {m}"),
        }
    }
}

impl std::error::Error for RpcError {}

/// One output as returned by `chain_getOutputs`, with the transparent amount
/// already recovered from the little-endian `amountCommitment` bytes.
///
/// This is exactly the data the web wallet's `scanOwnedOutputs` /
/// decoy-selection consumes, so the bridge reuses the node's own output view
/// rather than re-deriving anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcOutput {
    /// Hash of the transaction (or block, for coinbase/lottery) the output
    /// lives in, hex-encoded.
    pub tx_hash: String,
    /// Output index within its transaction (`u32::MAX` marks coinbase;
    /// `1 + i` under the block hash marks a lottery payout).
    pub output_index: u32,
    /// Hex-encoded 32-byte one-time target (stealth spend) key.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key.
    pub public_key: String,
    /// Transparent amount in picocredits (recovered from `amountCommitment`).
    pub amount: u64,
    /// Cluster tag weights as `[cluster_id, weight_ppm]` pairs
    /// (`TAG_WEIGHT_SCALE == 1_000_000` == 100%). An output with **no**
    /// explicit cluster weight is factor-1 / background (ADR 0003).
    pub cluster_tags: Vec<(u64, u64)>,
    /// Hex-encoded encrypted memo ciphertext (66 bytes) if the output carries
    /// one, else `None`. Only the recipient's view key can decrypt it; the
    /// deposit watcher reads the destination memo (order UUID) from it.
    pub e_memo: Option<String>,
    /// Hex-encoded unified ML-KEM-768 ciphertext (1088 bytes) for a hybrid
    /// output, else `None` (classical/legacy). Emitted by the node as
    /// `kemCiphertext` (issue #970). The deposit watcher decapsulates it with
    /// the reserve's ML-KEM secret to detect hybrid deposits; without it a
    /// hybrid deposit would be silently missed.
    pub kem_ciphertext: Option<String>,
}

impl RpcOutput {
    /// Total explicit (non-background) cluster weight in ppm. Factor-1
    /// (wrap-eligible per ADR 0003) outputs have `explicit_cluster_weight() ==
    /// 0` — all of their value is background/commerce.
    pub fn explicit_cluster_weight(&self) -> u64 {
        self.cluster_tags.iter().map(|(_, w)| *w).sum()
    }
}

/// A block's bridge-relevant outputs, as decoded from `chain_getOutputs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcBlockOutputs {
    /// Block height.
    pub height: u64,
    /// Every output the node exposed at this height.
    pub outputs: Vec<RpcOutput>,
}

/// The spent-status of one key image (`chain_areKeyImagesSpent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyImageStatus {
    /// Recorded in the on-chain double-spend set.
    pub spent: bool,
    /// In-flight in the mempool (an unmined spend).
    pub pending: bool,
}

/// A submitted transaction's inclusion state (`getTransaction`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxInclusion {
    /// The node has never seen this tx (not in a block, not in the mempool).
    Unknown,
    /// In the mempool, not yet mined.
    Pending,
    /// Included at `block_height` with `confirmations` depth (1 == just
    /// included; the node counts the including block itself).
    Confirmed {
        /// Height of the block that included the transaction.
        block_height: u64,
        /// Confirmation depth (>= 1).
        confirmations: u64,
    },
}

/// Thin JSON-RPC client over a node's HTTP endpoint.
pub struct BthNodeRpc {
    url: String,
    client: reqwest::Client,
}

impl BthNodeRpc {
    /// Build a client for `rpc_url`. Does not perform network I/O.
    pub fn new(rpc_url: impl Into<String>) -> Result<Self, RpcError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| RpcError::Transport(format!("build http client: {e}")))?;
        Ok(Self {
            url: rpc_url.into(),
            client,
        })
    }

    /// Issue a single JSON-RPC 2.0 call and return its `result` value.
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| RpcError::Transport(format!("{method}: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| RpcError::Transport(format!("{method}: read body: {e}")))?;
        if !status.is_success() {
            return Err(RpcError::Transport(format!(
                "{method}: HTTP {status}: {text}"
            )));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| RpcError::Decode(format!("{method}: {e}: {text}")))?;

        if let Some(err) = value.get("error").filter(|e| !e.is_null()) {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("<no message>")
                .to_string();
            return Err(RpcError::Node { code, message });
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Decode(format!("{method}: response has no result")))
    }

    /// Current chain tip height (`getChainInfo`).
    pub async fn chain_tip(&self) -> Result<u64, RpcError> {
        let result = self.call("getChainInfo", json!({})).await?;
        result
            .get("height")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError::Decode("getChainInfo: missing height".to_string()))
    }

    /// Outputs across `[start_height, end_height]` (`chain_getOutputs`).
    ///
    /// The node returns one entry per block that exists in the range; a range
    /// that runs past the tip simply yields fewer blocks (never an error), so
    /// the caller can detect "block not available yet" by its absence.
    pub async fn get_outputs(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<RpcBlockOutputs>, RpcError> {
        let result = self
            .call(
                "chain_getOutputs",
                json!({ "start_height": start_height, "end_height": end_height }),
            )
            .await?;
        let blocks = result
            .as_array()
            .ok_or_else(|| RpcError::Decode("chain_getOutputs: result is not an array".into()))?;
        blocks.iter().map(decode_block_outputs).collect()
    }

    /// Broadcast a bincode-serialized signed transaction (`tx_submit`).
    ///
    /// Returns the node-reported tx hash on success. Idempotency is handled by
    /// the caller: an "already known" style rejection of our OWN recorded tx is
    /// treated as success there (the tx was already submitted before a
    /// restart), because the node has no order-id guard on BTH.
    pub async fn submit_tx(&self, tx_hex: &str) -> Result<String, RpcError> {
        let result = self.call("tx_submit", json!({ "tx_hex": tx_hex })).await?;
        result
            .get("txHash")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| RpcError::Decode("tx_submit: missing txHash".to_string()))
    }

    /// Inclusion state of a submitted transaction (`getTransaction`).
    pub async fn get_transaction(&self, tx_hash: &str) -> Result<TxInclusion, RpcError> {
        let result = self
            .call("getTransaction", json!({ "tx_hash": tx_hash }))
            .await?;
        let status = result
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Ok(match status {
            "confirmed" => {
                let block_height = result
                    .get("blockHeight")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        RpcError::Decode("getTransaction: confirmed without blockHeight".into())
                    })?;
                let confirmations = result
                    .get("confirmations")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1);
                TxInclusion::Confirmed {
                    block_height,
                    confirmations,
                }
            }
            "pending" => TxInclusion::Pending,
            _ => TxInclusion::Unknown,
        })
    }

    /// Spent-status of each supplied key image, preserving order
    /// (`chain_areKeyImagesSpent`).
    pub async fn are_key_images_spent(
        &self,
        key_images: &[String],
    ) -> Result<Vec<KeyImageStatus>, RpcError> {
        let result = self
            .call(
                "chain_areKeyImagesSpent",
                json!({ "keyImages": key_images }),
            )
            .await?;
        let arr = result.as_array().ok_or_else(|| {
            RpcError::Decode("chain_areKeyImagesSpent: result is not an array".into())
        })?;
        Ok(arr
            .iter()
            .map(|entry| KeyImageStatus {
                spent: entry.get("spent").and_then(Value::as_bool).unwrap_or(false),
                pending: entry
                    .get("pending")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .collect())
    }
}

/// Serde shape of one output inside a `chain_getOutputs` block.
#[derive(Deserialize)]
struct RawOutput {
    #[serde(rename = "txHash")]
    tx_hash: String,
    #[serde(rename = "outputIndex")]
    output_index: u32,
    #[serde(rename = "targetKey")]
    target_key: String,
    #[serde(rename = "publicKey")]
    public_key: String,
    #[serde(rename = "amountCommitment")]
    amount_commitment: String,
    #[serde(rename = "clusterTags", default)]
    cluster_tags: Vec<[u64; 2]>,
    #[serde(rename = "eMemo", default)]
    e_memo: Option<String>,
    #[serde(rename = "kemCiphertext", default)]
    kem_ciphertext: Option<String>,
}

/// Decode the transparent amount from the node's little-endian
/// `amountCommitment` hex (the node emits `amount.to_le_bytes()`).
fn decode_amount(hex_le: &str) -> Result<u64, RpcError> {
    let bytes =
        hex::decode(hex_le).map_err(|e| RpcError::Decode(format!("amountCommitment hex: {e}")))?;
    if bytes.len() != 8 {
        return Err(RpcError::Decode(format!(
            "amountCommitment must be 8 bytes, got {}",
            bytes.len()
        )));
    }
    let mut le = [0u8; 8];
    le.copy_from_slice(&bytes);
    Ok(u64::from_le_bytes(le))
}

fn decode_block_outputs(block: &Value) -> Result<RpcBlockOutputs, RpcError> {
    let height = block
        .get("height")
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::Decode("chain_getOutputs block: missing height".into()))?;
    let raw_outputs = block
        .get("outputs")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::Decode("chain_getOutputs block: missing outputs".into()))?;
    let mut outputs = Vec::with_capacity(raw_outputs.len());
    for raw in raw_outputs {
        let parsed: RawOutput = serde_json::from_value(raw.clone())
            .map_err(|e| RpcError::Decode(format!("chain_getOutputs output: {e}")))?;
        outputs.push(RpcOutput {
            tx_hash: parsed.tx_hash,
            output_index: parsed.output_index,
            target_key: parsed.target_key,
            public_key: parsed.public_key,
            amount: decode_amount(&parsed.amount_commitment)?,
            cluster_tags: parsed
                .cluster_tags
                .into_iter()
                .map(|[id, w]| (id, w))
                .collect(),
            e_memo: parsed.e_memo,
            kem_ciphertext: parsed.kem_ciphertext,
        });
    }
    Ok(RpcBlockOutputs { height, outputs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_amount_reads_little_endian() {
        // 1_000_000_000_000 picocredits (1 BTH) as le bytes.
        let hex_le = hex::encode(1_000_000_000_000u64.to_le_bytes());
        assert_eq!(decode_amount(&hex_le).unwrap(), 1_000_000_000_000);
        assert_eq!(decode_amount(&hex::encode(0u64.to_le_bytes())).unwrap(), 0);
    }

    #[test]
    fn decode_amount_rejects_wrong_length() {
        assert!(matches!(decode_amount("00"), Err(RpcError::Decode(_))));
        assert!(matches!(decode_amount("zz"), Err(RpcError::Decode(_))));
    }

    #[test]
    fn decode_block_outputs_parses_node_shape() {
        // Exactly the JSON shape handle_get_outputs emits, including a
        // coinbase (outputIndex u32::MAX) and cluster tags.
        let block = json!({
            "height": 42,
            "outputs": [
                {
                    "txHash": "aa",
                    "outputIndex": u32::MAX,
                    "targetKey": "bb",
                    "publicKey": "cc",
                    "amountCommitment": hex::encode(500u64.to_le_bytes()),
                    "clusterTags": [],
                    "coinbase": true
                },
                {
                    "txHash": "dd",
                    "outputIndex": 0,
                    "targetKey": "ee",
                    "publicKey": "ff",
                    "amountCommitment": hex::encode(2_000u64.to_le_bytes()),
                    "clusterTags": [[7, 250_000], [9, 100_000]],
                    "eMemo": "abcd"
                }
            ]
        });
        let decoded = decode_block_outputs(&block).unwrap();
        assert_eq!(decoded.height, 42);
        assert_eq!(decoded.outputs.len(), 2);
        assert_eq!(decoded.outputs[0].amount, 500);
        assert_eq!(decoded.outputs[0].output_index, u32::MAX);
        assert_eq!(decoded.outputs[0].explicit_cluster_weight(), 0);
        assert_eq!(decoded.outputs[0].e_memo, None);
        assert_eq!(decoded.outputs[1].amount, 2_000);
        assert_eq!(
            decoded.outputs[1].cluster_tags,
            vec![(7, 250_000), (9, 100_000)]
        );
        assert_eq!(decoded.outputs[1].explicit_cluster_weight(), 350_000);
        assert_eq!(decoded.outputs[1].e_memo.as_deref(), Some("abcd"));
    }
}
