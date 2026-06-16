// Copyright (c) 2024 Botho Foundation

//! Client-side (WebAssembly) Botho transaction builder + CLSAG signer.
//!
//! This crate exposes the node-identical CLSAG transaction build + sign path
//! (from [`bth_transaction_clsag`]) to JavaScript via `wasm-bindgen`, so the
//! browser wallet can construct, sign, and serialize a transaction entirely
//! client-side. **The spend key never leaves the browser** — it is passed in
//! by the caller (who holds it locally) and is used only to recover the
//! one-time private key and produce the ring signature.
//!
//! # Wire format
//!
//! [`build_and_sign`] returns the **bincode** serialization of a
//! [`bth_transaction_clsag::Transaction`] as a hex string. This is exactly the
//! `tx_hex` payload the node accepts in its `tx_submit` JSON-RPC method, so the
//! output round-trips through the same Rust verifier the node uses.
//!
//! # Decoy selection
//!
//! Decoy (ring-member) selection is performed off this crate: the JS caller
//! fetches candidate outputs via RPC (`chain_getOutputs`) and passes them in as
//! `decoys`. This keeps the wasm surface small and the privacy-sensitive
//! mixing policy in one place. The signer shuffles the real input among the
//! decoys so its position is hidden.
//!
//! # Testing
//!
//! The pure-Rust [`core`] module is exercised by native `cargo test`
//! (sign -> verify round-trip), which is the highest-value provable slice:
//! correctness of the produced transaction against the same verifier the node
//! runs. The `wasm-bindgen` layer is a thin serde shim over it.

pub mod core;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use crate::core::{build_and_sign_inner, scan_owned_outputs_inner, ScanRequest, SignRequest};
    use wasm_bindgen::prelude::*;

    /// Build and CLSAG-sign a Botho transaction entirely client-side.
    ///
    /// `request` is a JS object matching [`SignRequest`] (camelCase fields):
    /// spend/view private keys (hex), the real input UTXOs, the decoy ring
    /// members, recipient address keys, amount, fee, and chain height.
    ///
    /// Returns the hex-encoded bincode bytes of the signed transaction, ready
    /// to submit via the `tx_submit` RPC. Throws a JS error on any failure
    /// (bad keys, insufficient decoys, balance mismatch, etc.).
    #[wasm_bindgen(js_name = buildAndSign)]
    pub fn build_and_sign(request: JsValue) -> Result<String, JsError> {
        let req: SignRequest = serde_wasm_bindgen::from_value(request)
            .map_err(|e| JsError::new(&format!("invalid request: {e}")))?;
        build_and_sign_inner(&req).map_err(|e| JsError::new(&e))
    }

    /// Identify which of the supplied chain outputs belong to the account.
    ///
    /// `request` is a JS object matching [`ScanRequest`]: the spend/view
    /// private keys (hex) and the candidate outputs (as returned by
    /// `chain_getOutputs`, with the transparent amount). Returns the owned
    /// outputs (with recovered subaddress index), serialized as a JS value.
    /// Uses the node-identical `belongs_to` check so ownership detection
    /// cannot drift from the node.
    #[wasm_bindgen(js_name = scanOwnedOutputs)]
    pub fn scan_owned_outputs(request: JsValue) -> Result<JsValue, JsError> {
        let req: ScanRequest = serde_wasm_bindgen::from_value(request)
            .map_err(|e| JsError::new(&format!("invalid scan request: {e}")))?;
        let owned = scan_owned_outputs_inner(&req).map_err(|e| JsError::new(&e))?;
        serde_wasm_bindgen::to_value(&owned)
            .map_err(|e| JsError::new(&format!("failed to serialize owned outputs: {e}")))
    }

    /// The CLSAG ring size the network requires (decoys + 1 real input).
    #[wasm_bindgen(js_name = ringSize)]
    pub fn ring_size() -> usize {
        bth_transaction_clsag::DEFAULT_RING_SIZE
    }

    /// The minimum transaction fee in picocredits.
    #[wasm_bindgen(js_name = minFee)]
    pub fn min_fee() -> u64 {
        bth_transaction_clsag::MIN_TX_FEE
    }
}
