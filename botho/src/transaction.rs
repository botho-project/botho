// Copyright (c) 2024 Botho Foundation

//! Transaction types for value transfers with CryptoNote-style stealth
//! addresses and ring signatures for sender privacy.
//!
//! The implementation lives in the standalone [`bth_transaction_clsag`] crate
//! so that it can be reused outside the node binary (for example, compiled to
//! WebAssembly for client-side transaction signing in the browser wallet —
//! see `web/packages/wasm-signer`). This module re-exports the full public API
//! unchanged, so existing `crate::transaction::*` and `botho::transaction::*`
//! paths continue to resolve to the same types and wire format.

pub use bth_transaction_clsag::*;
