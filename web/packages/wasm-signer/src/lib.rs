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
    use crate::core::{
        build_and_sign_inner, compute_owned_output_key_images_inner, decode_address_to_dto,
        encode_address_from_hex, scan_owned_outputs_inner, KeyImageRequest, ScanRequest,
        SignRequest,
    };
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

    /// Compute the key image for each of the wallet's owned outputs.
    ///
    /// `request` is a JS object matching [`KeyImageRequest`]: the spend/view
    /// private keys (hex) and the owned outputs (as returned by
    /// `scanOwnedOutputs`). Returns each output annotated with its hex-encoded
    /// key image. The wallet passes these key images to the node's
    /// `chain_areKeyImagesSpent` RPC to learn which owned outputs are already
    /// spent, so it can exclude them from its balance and spendable selection.
    /// Uses the node-identical derivation, so spent-status cannot drift.
    #[wasm_bindgen(js_name = computeOwnedOutputKeyImages)]
    pub fn compute_owned_output_key_images(request: JsValue) -> Result<JsValue, JsError> {
        let req: KeyImageRequest = serde_wasm_bindgen::from_value(request)
            .map_err(|e| JsError::new(&format!("invalid key image request: {e}")))?;
        let result = compute_owned_output_key_images_inner(&req).map_err(|e| JsError::new(&e))?;
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsError::new(&format!("failed to serialize key images: {e}")))
    }

    /// Decode a `botho://2/…` / `tbotho://2/…` address string into its hex
    /// components (`network`, `viewPublicKey`, `spendPublicKey`,
    /// `kemPublicKey`, `dsaPublicKey`).
    ///
    /// The browser wallet uses this shared Rust codec instead of a hand-rolled
    /// base58 decoder in JavaScript, so its parsing is byte-identical to the
    /// node and mobile encoders (ADR 0008 D5). Old 64-byte v1 addresses and the
    /// retired quantum prefixes throw a clear error.
    #[wasm_bindgen(js_name = decodeAddress)]
    pub fn decode_address(address: &str) -> Result<JsValue, JsError> {
        let dto = decode_address_to_dto(address).map_err(|e| JsError::new(&e))?;
        serde_wasm_bindgen::to_value(&dto)
            .map_err(|e| JsError::new(&format!("failed to serialize decoded address: {e}")))
    }

    /// Encode a `botho://2/…` / `tbotho://2/…` address string from hex key
    /// components via the shared codec.
    #[wasm_bindgen(js_name = encodeAddress)]
    pub fn encode_address(
        view_hex: &str,
        spend_hex: &str,
        kem_hex: &str,
        dsa_hex: &str,
        testnet: bool,
    ) -> Result<String, JsError> {
        encode_address_from_hex(view_hex, spend_hex, kem_hex, dsa_hex, testnet)
            .map_err(|e| JsError::new(&e))
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
