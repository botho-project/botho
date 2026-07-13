// Copyright (c) 2024 The Botho Foundation

//! Lightweight Solana JSON-RPC transport + legacy-transaction assembly (#857).
//!
//! The bridge deliberately avoids the heavy `solana-sdk` / `solana-client`
//! dependency stack. Everything it needs from Solana is small and stable:
//!
//! - **Ed25519** signing/verification — already available via `ed25519-dalek`
//!   (Solana verifies Ed25519 natively, so the #824 validator attestation
//!   signatures over the transaction message ARE valid transaction signatures,
//!   per ADR 0002).
//! - **base58** address/signature encoding — `bs58`.
//! - **The legacy transaction wire format** — a compact, versioned-only-by-
//!   convention structure reproduced here ([`LegacyMessage`] /
//!   [`Transaction`]). Solana's "legacy" (non-v0) transaction is what the
//!   Anchor `bridge_mint` instruction needs; it has no address-lookup tables.
//! - **A handful of JSON-RPC methods** — `getLatestBlockhash`,
//!   `sendTransaction`, `getSignatureStatuses`, `getSignaturesForAddress`,
//!   `getTransaction`, `getSlot` — spoken over `reqwest` as raw JSON.
//!
//! The [`SolanaRpc`] trait abstracts the transport so the mint/watcher logic
//! is unit-testable against mocked JSON-RPC responses without a live cluster.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// The System Program id (`11111111111111111111111111111111`), needed as an
/// account meta for the order-marker PDA `init` (rent) in `bridge_mint`.
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey([0u8; 32]);

/// The SPL Token program id (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`).
pub const TOKEN_PROGRAM_ID: Pubkey = Pubkey([
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237,
    95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
]);

/// A Solana Ed25519 public key / account address (raw 32 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    /// Parse a base58-encoded address.
    pub fn from_base58(s: &str) -> Result<Self, String> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|e| format!("invalid base58 pubkey {}: {}", s, e))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|v: Vec<u8>| format!("pubkey must be 32 bytes, got {}", v.len()))?;
        Ok(Pubkey(arr))
    }

    /// base58 rendering.
    pub fn to_base58(self) -> String {
        bs58::encode(self.0).into_string()
    }

    /// Derive a program address (PDA) for `seeds` under `program_id`, walking
    /// the bump seed down from 255 until the resulting point is off-curve
    /// (the standard `find_program_address` algorithm). Returns the address
    /// and the bump used.
    pub fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Option<(Pubkey, u8)> {
        for bump in (0u8..=255).rev() {
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest;
            for seed in seeds {
                hasher.update(seed);
            }
            hasher.update([bump]);
            hasher.update(program_id.0);
            hasher.update(b"ProgramDerivedAddress");
            let hash = hasher.finalize();
            let candidate: [u8; 32] = hash.into();
            // A PDA must NOT be a valid Ed25519 curve point.
            if !is_on_ed25519_curve(&candidate) {
                return Some((Pubkey(candidate), bump));
            }
        }
        None
    }
}

/// Whether a 32-byte value decompresses to a valid Ed25519 curve point.
/// PDAs are exactly the byte strings for which this is FALSE.
fn is_on_ed25519_curve(bytes: &[u8; 32]) -> bool {
    // curve25519-dalek is not a direct dep here; ed25519-dalek re-exports the
    // compressed-point decompression we need via VerifyingKey::from_bytes,
    // which fails precisely for off-curve encodings.
    ed25519_dalek::VerifyingKey::from_bytes(bytes).is_ok()
}

/// One account reference in a compiled instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountMeta {
    /// The account address.
    pub pubkey: Pubkey,
    /// Whether the account must sign the transaction.
    pub is_signer: bool,
    /// Whether the instruction may mutate the account.
    pub is_writable: bool,
}

impl AccountMeta {
    /// A read-only, non-signer account.
    pub fn readonly(pubkey: Pubkey) -> Self {
        Self {
            pubkey,
            is_signer: false,
            is_writable: false,
        }
    }
    /// A writable, non-signer account.
    pub fn writable(pubkey: Pubkey) -> Self {
        Self {
            pubkey,
            is_signer: false,
            is_writable: true,
        }
    }
    /// A writable signer.
    pub fn writable_signer(pubkey: Pubkey) -> Self {
        Self {
            pubkey,
            is_signer: true,
            is_writable: true,
        }
    }
}

/// A program instruction prior to message compilation.
#[derive(Debug, Clone)]
pub struct Instruction {
    /// The program that executes this instruction.
    pub program_id: Pubkey,
    /// Accounts, in the order the program expects them.
    pub accounts: Vec<AccountMeta>,
    /// Opaque instruction data (Anchor discriminator + borsh args).
    pub data: Vec<u8>,
}

/// A compiled legacy transaction message: the exact bytes signers sign.
///
/// Layout (Solana legacy message):
/// ```text
/// [ num_required_signatures: u8 ]
/// [ num_readonly_signed_accounts: u8 ]
/// [ num_readonly_unsigned_accounts: u8 ]
/// [ account_keys: compact-array<Pubkey> ]
/// [ recent_blockhash: 32 ]
/// [ instructions: compact-array<CompiledInstruction> ]
/// ```
#[derive(Debug, Clone)]
pub struct LegacyMessage {
    /// Ordered account keys (signers-writable, signers-readonly,
    /// nonsigners-writable, nonsigners-readonly).
    pub account_keys: Vec<Pubkey>,
    /// Number of leading account keys that must sign.
    pub num_required_signatures: u8,
    /// Of the signers, how many are read-only.
    pub num_readonly_signed: u8,
    /// Of the non-signers, how many are read-only.
    pub num_readonly_unsigned: u8,
    /// The recent blockhash (32 bytes) binding the message to a slot window.
    pub recent_blockhash: [u8; 32],
    /// Compiled instructions referencing `account_keys` by index.
    pub instructions: Vec<CompiledInstruction>,
}

/// An instruction with its accounts resolved to indices into the message's
/// `account_keys`.
#[derive(Debug, Clone)]
pub struct CompiledInstruction {
    /// Index of the program id in `account_keys`.
    pub program_id_index: u8,
    /// Indices of the accounts in `account_keys`.
    pub account_indices: Vec<u8>,
    /// Instruction data.
    pub data: Vec<u8>,
}

/// Append a shortvec (compact-u16) length prefix.
fn push_compact_u16(out: &mut Vec<u8>, mut n: u16) {
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
}

impl LegacyMessage {
    /// Compile a message from a single fee-payer, an ordered set of required
    /// signer pubkeys (fee-payer first, then any additional signers), and the
    /// instructions.
    ///
    /// The account-ordering rules Solana enforces are reproduced exactly:
    /// the fee payer is `account_keys[0]` and is writable+signer; remaining
    /// accounts are partitioned into (writable signers, readonly signers,
    /// writable non-signers, readonly non-signers), each group deduplicated,
    /// with the strongest privilege observed across all instruction metas
    /// winning for any given key.
    pub fn compile(
        fee_payer: Pubkey,
        instructions: &[Instruction],
        recent_blockhash: [u8; 32],
    ) -> Self {
        use std::collections::BTreeMap;

        // Collect the strongest (is_signer, is_writable) seen per key.
        let mut privileges: BTreeMap<Pubkey, (bool, bool)> = BTreeMap::new();
        // The fee payer is always a writable signer.
        privileges.insert(fee_payer, (true, true));
        for ix in instructions {
            // The program id is a readonly non-signer account.
            privileges
                .entry(ix.program_id)
                .and_modify(|(s, w)| {
                    *s |= false;
                    *w |= false;
                })
                .or_insert((false, false));
            for meta in &ix.accounts {
                privileges
                    .entry(meta.pubkey)
                    .and_modify(|(s, w)| {
                        *s |= meta.is_signer;
                        *w |= meta.is_writable;
                    })
                    .or_insert((meta.is_signer, meta.is_writable));
            }
        }

        // Partition into the four ordered groups. The fee payer must lead the
        // writable-signer group, so pull it out first.
        let mut writable_signers = vec![fee_payer];
        let mut readonly_signers = Vec::new();
        let mut writable_nonsigners = Vec::new();
        let mut readonly_nonsigners = Vec::new();
        for (key, (is_signer, is_writable)) in privileges {
            if key == fee_payer {
                continue;
            }
            match (is_signer, is_writable) {
                (true, true) => writable_signers.push(key),
                (true, false) => readonly_signers.push(key),
                (false, true) => writable_nonsigners.push(key),
                (false, false) => readonly_nonsigners.push(key),
            }
        }

        let num_required_signatures = (writable_signers.len() + readonly_signers.len()) as u8;
        let num_readonly_signed = readonly_signers.len() as u8;
        let num_readonly_unsigned = readonly_nonsigners.len() as u8;

        let mut account_keys = Vec::new();
        account_keys.extend(writable_signers);
        account_keys.extend(readonly_signers);
        account_keys.extend(writable_nonsigners);
        account_keys.extend(readonly_nonsigners);

        let index_of = |key: &Pubkey| account_keys.iter().position(|k| k == key).unwrap() as u8;

        let compiled = instructions
            .iter()
            .map(|ix| CompiledInstruction {
                program_id_index: index_of(&ix.program_id),
                account_indices: ix.accounts.iter().map(|m| index_of(&m.pubkey)).collect(),
                data: ix.data.clone(),
            })
            .collect();

        LegacyMessage {
            account_keys,
            num_required_signatures,
            num_readonly_signed,
            num_readonly_unsigned,
            recent_blockhash,
            instructions: compiled,
        }
    }

    /// Serialize to the canonical legacy-message bytes signers sign over.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.num_required_signatures);
        out.push(self.num_readonly_signed);
        out.push(self.num_readonly_unsigned);

        push_compact_u16(&mut out, self.account_keys.len() as u16);
        for key in &self.account_keys {
            out.extend_from_slice(&key.0);
        }

        out.extend_from_slice(&self.recent_blockhash);

        push_compact_u16(&mut out, self.instructions.len() as u16);
        for ix in &self.instructions {
            out.push(ix.program_id_index);
            push_compact_u16(&mut out, ix.account_indices.len() as u16);
            out.extend_from_slice(&ix.account_indices);
            push_compact_u16(&mut out, ix.data.len() as u16);
            out.extend_from_slice(&ix.data);
        }
        out
    }
}

/// A signed legacy transaction: `compact-array<signature(64)>` then the
/// serialized message.
#[derive(Debug, Clone)]
pub struct Transaction {
    /// One 64-byte signature per required signer, in `account_keys` order.
    pub signatures: Vec<[u8; 64]>,
    /// The compiled message.
    pub message: LegacyMessage,
}

impl Transaction {
    /// Serialize to the wire format `sendTransaction` accepts.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_compact_u16(&mut out, self.signatures.len() as u16);
        for sig in &self.signatures {
            out.extend_from_slice(sig);
        }
        out.extend_from_slice(&self.message.serialize());
        out
    }

    /// The transaction id (first signature, base58) — known before broadcast.
    pub fn signature_base58(&self) -> Option<String> {
        self.signatures
            .first()
            .map(|s| bs58::encode(s).into_string())
    }
}

/// The confirmation state of a submitted signature, as reported by
/// `getSignatureStatuses`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureState {
    /// Not found (either never landed, or dropped after its blockhash
    /// expired). The caller distinguishes "still in flight" from "expired"
    /// using the returned blockhash validity, not this variant alone.
    Unknown,
    /// Landed at the given commitment, with `err` set if the transaction
    /// executed but failed.
    Landed {
        /// The confirmation status string
        /// (`processed`/`confirmed`/`finalized`).
        confirmation_status: Option<String>,
        /// The execution error, if the transaction failed on-chain.
        err: Option<String>,
    },
}

/// A minimal Solana JSON-RPC surface, abstracted for testability.
#[async_trait]
pub trait SolanaRpc: Send + Sync {
    /// `getLatestBlockhash` -> (blockhash bytes, last_valid_block_height).
    async fn get_latest_blockhash(&self) -> Result<([u8; 32], u64), String>;

    /// `sendTransaction(base64(raw))`. Returns the signature (base58). An
    /// "already processed" style response is a success (idempotent
    /// re-broadcast) and returns the known signature.
    async fn send_transaction(&self, raw: &[u8]) -> Result<String, String>;

    /// `getSignatureStatuses([signature])`.
    async fn get_signature_status(&self, signature: &str) -> Result<SignatureState, String>;

    /// `getAccountInfo(address)` at `commitment`, returning the raw account
    /// data (base64-decoded), or `None` if the account does not exist.
    async fn get_account_data(
        &self,
        address: &str,
        commitment: &str,
    ) -> Result<Option<Vec<u8>>, String>;

    /// `getSignaturesForAddress(program, until, limit)` at `commitment`,
    /// newest-first, returning `(signature, slot)` pairs.
    async fn get_signatures_for_address(
        &self,
        address: &str,
        until: Option<&str>,
        commitment: &str,
    ) -> Result<Vec<(String, u64)>, String>;

    /// `getTransaction(signature)` at `commitment`, returning the program
    /// log lines (`meta.logMessages`) and the slot, or `None` if not found.
    async fn get_transaction_logs(
        &self,
        signature: &str,
        commitment: &str,
    ) -> Result<Option<(Vec<String>, u64)>, String>;

    /// `getTokenSupply(mint)` at `commitment`, returning the SPL mint's total
    /// supply in its RAW base units (the `amount` field — an integer scaled by
    /// the mint's decimals). The wBTH mint carries 12 decimals, matching the
    /// picocredit base unit, so the raw amount is already in picocredits
    /// (#853).
    ///
    /// A default `NotImplemented`-style error keeps the many mock transports
    /// in the codebase compiling; the live [`HttpSolanaRpc`] overrides it.
    async fn get_token_supply(&self, _mint: &str, _commitment: &str) -> Result<u128, String> {
        Err("getTokenSupply not implemented for this transport".to_string())
    }
}

/// A live JSON-RPC client over `reqwest`.
pub struct HttpSolanaRpc {
    url: String,
    client: reqwest::Client,
}

impl HttpSolanaRpc {
    /// Build a client for `url`. Does not perform network I/O.
    pub fn new(url: impl Into<String>) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("building solana http client: {}", e))?;
        Ok(Self {
            url: url.into(),
            client,
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
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
            .map_err(|e| format!("{} request failed: {}", method, e))?;
        let value: Value = resp
            .json()
            .await
            .map_err(|e| format!("{} response decode failed: {}", method, e))?;
        if let Some(err) = value.get("error") {
            return Err(format!("{} rpc error: {}", method, err));
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| format!("{} response missing result", method))
    }
}

#[async_trait]
impl SolanaRpc for HttpSolanaRpc {
    async fn get_latest_blockhash(&self) -> Result<([u8; 32], u64), String> {
        let result = self
            .call("getLatestBlockhash", json!([{"commitment": "finalized"}]))
            .await?;
        parse_latest_blockhash(&result)
    }

    async fn send_transaction(&self, raw: &[u8]) -> Result<String, String> {
        let encoded = base64_encode(raw);
        let params = json!([
            encoded,
            {"encoding": "base64", "skipPreflight": false, "preflightCommitment": "finalized"}
        ]);
        match self.call("sendTransaction", params).await {
            Ok(v) => v
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "sendTransaction result was not a signature string".to_string()),
            Err(e) => {
                // Idempotent re-broadcast: the node already processed this
                // exact transaction. The on-chain per-order marker PDA
                // guarantees the mint itself is exactly-once regardless.
                let lower = e.to_lowercase();
                if lower.contains("already processed") || lower.contains("alreadyprocessed") {
                    Err(AlreadyProcessed.to_string())
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn get_signature_status(&self, signature: &str) -> Result<SignatureState, String> {
        let result = self
            .call(
                "getSignatureStatuses",
                json!([[signature], {"searchTransactionHistory": true}]),
            )
            .await?;
        parse_signature_status(&result)
    }

    async fn get_account_data(
        &self,
        address: &str,
        commitment: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let result = self
            .call(
                "getAccountInfo",
                json!([address, {"commitment": commitment, "encoding": "base64"}]),
            )
            .await?;
        parse_account_data(&result)
    }

    async fn get_signatures_for_address(
        &self,
        address: &str,
        until: Option<&str>,
        commitment: &str,
    ) -> Result<Vec<(String, u64)>, String> {
        let mut opts = serde_json::Map::new();
        opts.insert("commitment".to_string(), json!(commitment));
        if let Some(u) = until {
            opts.insert("until".to_string(), json!(u));
        }
        let result = self
            .call(
                "getSignaturesForAddress",
                json!([address, Value::Object(opts)]),
            )
            .await?;
        parse_signatures_for_address(&result)
    }

    async fn get_transaction_logs(
        &self,
        signature: &str,
        commitment: &str,
    ) -> Result<Option<(Vec<String>, u64)>, String> {
        let result = self
            .call(
                "getTransaction",
                json!([
                    signature,
                    {"commitment": commitment, "maxSupportedTransactionVersion": 0, "encoding": "json"}
                ]),
            )
            .await?;
        parse_transaction_logs(&result)
    }

    async fn get_token_supply(&self, mint: &str, commitment: &str) -> Result<u128, String> {
        let result = self
            .call("getTokenSupply", json!([mint, {"commitment": commitment}]))
            .await?;
        parse_token_supply(&result)
    }
}

/// Marker string used to signal an idempotent "already processed" broadcast
/// up to the [`crate::mint::Minter`] layer without importing this module's
/// error type there.
pub const ALREADY_PROCESSED_MARKER: &str = "solana:already-processed";

struct AlreadyProcessed;
impl std::fmt::Display for AlreadyProcessed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", ALREADY_PROCESSED_MARKER)
    }
}

// === JSON-RPC response parsers (pure, unit-tested against captured shapes) ===

/// Parse a `getLatestBlockhash` result into (blockhash, last_valid_height).
pub fn parse_latest_blockhash(result: &Value) -> Result<([u8; 32], u64), String> {
    let value = result
        .get("value")
        .ok_or_else(|| "getLatestBlockhash missing value".to_string())?;
    let blockhash_b58 = value
        .get("blockhash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "getLatestBlockhash missing blockhash".to_string())?;
    let bytes = bs58::decode(blockhash_b58)
        .into_vec()
        .map_err(|e| format!("invalid blockhash base58: {}", e))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("blockhash must be 32 bytes, got {}", v.len()))?;
    let last_valid = value
        .get("lastValidBlockHeight")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok((arr, last_valid))
}

/// Parse a `getSignatureStatuses` result for a single signature.
pub fn parse_signature_status(result: &Value) -> Result<SignatureState, String> {
    let entry = result
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| "getSignatureStatuses missing value[0]".to_string())?;
    if entry.is_null() {
        return Ok(SignatureState::Unknown);
    }
    let confirmation_status = entry
        .get("confirmationStatus")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let err = entry.get("err").and_then(|e| {
        if e.is_null() {
            None
        } else {
            Some(e.to_string())
        }
    });
    Ok(SignatureState::Landed {
        confirmation_status,
        err,
    })
}

/// Parse a `getSignaturesForAddress` result into (signature, slot) pairs.
/// Failed transactions (non-null `err`) are excluded — a failed
/// `bridge_burn` emitted no event.
pub fn parse_signatures_for_address(result: &Value) -> Result<Vec<(String, u64)>, String> {
    let arr = result
        .as_array()
        .ok_or_else(|| "getSignaturesForAddress result was not an array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        if entry.get("err").map(|e| !e.is_null()).unwrap_or(false) {
            continue;
        }
        let sig = entry
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "signature entry missing signature".to_string())?;
        let slot = entry.get("slot").and_then(|v| v.as_u64()).unwrap_or(0);
        out.push((sig.to_string(), slot));
    }
    Ok(out)
}

/// Parse a `getTransaction` result into (log lines, slot), or `None` if the
/// transaction is not found / not yet available.
pub fn parse_transaction_logs(result: &Value) -> Result<Option<(Vec<String>, u64)>, String> {
    if result.is_null() {
        return Ok(None);
    }
    let slot = result.get("slot").and_then(|v| v.as_u64()).unwrap_or(0);
    // A transaction that executed with an error emitted no trustworthy burn
    // event; drop it.
    if result
        .get("meta")
        .and_then(|m| m.get("err"))
        .map(|e| !e.is_null())
        .unwrap_or(false)
    {
        return Ok(Some((Vec::new(), slot)));
    }
    let logs = result
        .get("meta")
        .and_then(|m| m.get("logMessages"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some((logs, slot)))
}

/// Parse a `getTokenSupply` result into the RAW supply (base units), reading
/// the `value.amount` string (a decimal integer already scaled by the mint's
/// decimals). The wBTH mint's 12 decimals make this value picocredits (#853).
pub fn parse_token_supply(result: &Value) -> Result<u128, String> {
    let value = result
        .get("value")
        .ok_or_else(|| "getTokenSupply missing value".to_string())?;
    let amount = value
        .get("amount")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "getTokenSupply missing value.amount".to_string())?;
    amount
        .parse::<u128>()
        .map_err(|e| format!("getTokenSupply amount {amount:?} is not a u128: {e}"))
}

/// Parse a `getAccountInfo` result into the raw account data.
pub fn parse_account_data(result: &Value) -> Result<Option<Vec<u8>>, String> {
    let value = result.get("value");
    let value = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| "getAccountInfo value.data[0] missing (expected base64)".to_string())?;
    let bytes = base64_decode(data).map_err(|e| format!("account data base64: {}", e))?;
    Ok(Some(bytes))
}

/// Standard base64 decode (accepts padding). Local, to avoid a `base64` dep.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 char {}", c)),
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|b| *b != b'=' && !b.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        let mut valid = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= (val(c)? as u32) << (18 - 6 * i);
            valid += 1;
        }
        if valid >= 2 {
            out.push((n >> 16) as u8);
        }
        if valid >= 3 {
            out.push((n >> 8) as u8);
        }
        if valid >= 4 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// Standard base64 (with padding) — the encoding `sendTransaction` expects.
/// Implemented locally to avoid adding a `base64` crate for ~20 lines.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compact_u16_encoding() {
        let mut out = Vec::new();
        push_compact_u16(&mut out, 0);
        assert_eq!(out, vec![0]);

        let mut out = Vec::new();
        push_compact_u16(&mut out, 127);
        assert_eq!(out, vec![0x7f]);

        // 128 -> two bytes (0x80, 0x01).
        let mut out = Vec::new();
        push_compact_u16(&mut out, 128);
        assert_eq!(out, vec![0x80, 0x01]);

        // 16384 -> three bytes.
        let mut out = Vec::new();
        push_compact_u16(&mut out, 16384);
        assert_eq!(out, vec![0x80, 0x80, 0x01]);
    }

    #[test]
    fn test_base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_base64_decode_roundtrip() {
        for v in [
            b"".to_vec(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
            vec![0u8, 255, 128, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        ] {
            let encoded = base64_encode(&v);
            assert_eq!(base64_decode(&encoded).unwrap(), v, "roundtrip {:?}", v);
        }
    }

    #[test]
    fn test_parse_account_data() {
        // Missing -> None.
        assert_eq!(parse_account_data(&json!({"value": null})).unwrap(), None);

        let data = vec![1u8, 2, 3, 4];
        let encoded = base64_encode(&data);
        let result = json!({"value": {"data": [encoded, "base64"], "owner": "x"}});
        assert_eq!(parse_account_data(&result).unwrap(), Some(data));
    }

    #[test]
    fn test_pubkey_base58_roundtrip() {
        let system = SYSTEM_PROGRAM_ID.to_base58();
        assert_eq!(system, "11111111111111111111111111111111");
        let token = TOKEN_PROGRAM_ID.to_base58();
        assert_eq!(token, "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
        let parsed = Pubkey::from_base58(&token).unwrap();
        assert_eq!(parsed, TOKEN_PROGRAM_ID);
    }

    #[test]
    fn test_find_program_address_is_off_curve_and_deterministic() {
        let program = Pubkey([9u8; 32]);
        let (pda, bump) = Pubkey::find_program_address(&[b"bridge"], &program).unwrap();
        // PDAs must be off the Ed25519 curve.
        assert!(!is_on_ed25519_curve(&pda.0));
        // Deterministic.
        let (pda2, bump2) = Pubkey::find_program_address(&[b"bridge"], &program).unwrap();
        assert_eq!(pda, pda2);
        assert_eq!(bump, bump2);
        // Different seeds -> different address.
        let (other, _) = Pubkey::find_program_address(&[b"order"], &program).unwrap();
        assert_ne!(pda, other);
    }

    #[test]
    fn test_message_compile_orders_accounts_and_privileges() {
        let payer = Pubkey([1u8; 32]);
        let signer2 = Pubkey([2u8; 32]);
        let writable = Pubkey([3u8; 32]);
        let readonly = Pubkey([4u8; 32]);
        let program = Pubkey([5u8; 32]);

        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::writable_signer(signer2),
                AccountMeta::writable(writable),
                AccountMeta::readonly(readonly),
            ],
            data: vec![0xAA, 0xBB],
        };
        let msg = LegacyMessage::compile(payer, &[ix], [7u8; 32]);

        // Fee payer leads; two required signatures (payer + signer2).
        assert_eq!(msg.account_keys[0], payer);
        assert_eq!(msg.num_required_signatures, 2);
        assert_eq!(msg.num_readonly_signed, 0);
        // program + readonly are readonly-unsigned.
        assert_eq!(msg.num_readonly_unsigned, 2);
        // Signers come before non-signers.
        assert!(msg.account_keys[..2].contains(&payer));
        assert!(msg.account_keys[..2].contains(&signer2));
        // The compiled instruction references program by its key index.
        let compiled = &msg.instructions[0];
        assert_eq!(
            msg.account_keys[compiled.program_id_index as usize],
            program
        );
        assert_eq!(compiled.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_transaction_serialize_shape() {
        let payer = Pubkey([1u8; 32]);
        let program = Pubkey([5u8; 32]);
        let ix = Instruction {
            program_id: program,
            accounts: vec![],
            data: vec![1, 2, 3],
        };
        let msg = LegacyMessage::compile(payer, &[ix], [0u8; 32]);
        let tx = Transaction {
            signatures: vec![[9u8; 64]],
            message: msg,
        };
        let bytes = tx.serialize();
        // compact-array len (1) + 64-byte sig + message.
        assert_eq!(bytes[0], 1);
        assert_eq!(&bytes[1..65], &[9u8; 64]);
        assert_eq!(&bytes[65..], &tx.message.serialize()[..]);
        // The tx id is the base58 of the first signature.
        assert_eq!(
            tx.signature_base58().unwrap(),
            bs58::encode([9u8; 64]).into_string()
        );
    }

    #[test]
    fn test_parse_latest_blockhash() {
        let bh = bs58::encode([3u8; 32]).into_string();
        let result = json!({
            "context": {"slot": 100},
            "value": {"blockhash": bh, "lastValidBlockHeight": 300}
        });
        let (hash, last_valid) = parse_latest_blockhash(&result).unwrap();
        assert_eq!(hash, [3u8; 32]);
        assert_eq!(last_valid, 300);
    }

    #[test]
    fn test_parse_signature_status_variants() {
        // Null -> unknown (dropped / never landed).
        let unknown = json!({"value": [null]});
        assert_eq!(
            parse_signature_status(&unknown).unwrap(),
            SignatureState::Unknown
        );

        // Landed & finalized, no error.
        let finalized = json!({"value": [{"confirmationStatus": "finalized", "err": null}]});
        assert_eq!(
            parse_signature_status(&finalized).unwrap(),
            SignatureState::Landed {
                confirmation_status: Some("finalized".to_string()),
                err: None,
            }
        );

        // Landed but failed.
        let failed = json!({"value": [{"confirmationStatus": "confirmed", "err": {"InstructionError": [0, "Custom"]}}]});
        match parse_signature_status(&failed).unwrap() {
            SignatureState::Landed { err: Some(_), .. } => {}
            other => panic!("expected landed-with-error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_signatures_for_address_skips_failed() {
        let result = json!([
            {"signature": "sigA", "slot": 10, "err": null},
            {"signature": "sigB", "slot": 11, "err": {"InstructionError": []}},
            {"signature": "sigC", "slot": 12}
        ]);
        let pairs = parse_signatures_for_address(&result).unwrap();
        assert_eq!(
            pairs,
            vec![("sigA".to_string(), 10), ("sigC".to_string(), 12)]
        );
    }

    #[test]
    fn test_parse_token_supply() {
        // The RPC returns the raw supply as a decimal string under
        // value.amount (12-decimal wBTH mint => picocredits).
        let result = json!({
            "context": {"slot": 100},
            "value": {"amount": "1500000000000", "decimals": 12, "uiAmount": 1.5, "uiAmountString": "1.5"}
        });
        assert_eq!(parse_token_supply(&result).unwrap(), 1_500_000_000_000);

        // Zero supply.
        let zero = json!({"value": {"amount": "0", "decimals": 12}});
        assert_eq!(parse_token_supply(&zero).unwrap(), 0);

        // A very large supply still fits u128 (u64::MAX * scale).
        let big = json!({"value": {"amount": "340282366920938463463374607431768211455"}});
        assert_eq!(parse_token_supply(&big).unwrap(), u128::MAX);

        // Missing / malformed shapes are errors, never a silent zero.
        assert!(parse_token_supply(&json!({})).is_err());
        assert!(parse_token_supply(&json!({"value": {}})).is_err());
        assert!(parse_token_supply(&json!({"value": {"amount": "not-a-number"}})).is_err());
    }

    #[test]
    fn test_parse_transaction_logs() {
        // Not found.
        assert_eq!(parse_transaction_logs(&Value::Null).unwrap(), None);

        // Success with logs.
        let ok = json!({
            "slot": 55,
            "meta": {"err": null, "logMessages": ["Program log: hello", "Program data: AAA="]}
        });
        let (logs, slot) = parse_transaction_logs(&ok).unwrap().unwrap();
        assert_eq!(slot, 55);
        assert_eq!(logs.len(), 2);

        // Executed-with-error -> empty logs (no trustworthy event).
        let failed =
            json!({"slot": 7, "meta": {"err": {"x": 1}, "logMessages": ["Program log: x"]}});
        let (logs, slot) = parse_transaction_logs(&failed).unwrap().unwrap();
        assert_eq!(slot, 7);
        assert!(logs.is_empty());
    }
}
