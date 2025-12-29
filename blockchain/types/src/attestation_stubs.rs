// Copyright (c) 2024 Botho Foundation
//! Stub types for removed SGX attestation functionality.
//! These are placeholder types that maintain API compatibility while
//! attestation is not used in Botho's PoW consensus.

use alloc::string::String;
use alloc::vec::Vec;
use bt_crypto_digestible::Digestible;
use serde::{Deserialize, Serialize};

/// Stub for VerificationReport - attestation is not used in Botho
#[derive(Clone, Deserialize, Digestible, Eq, Hash, PartialEq, Serialize, ::prost::Message)]
pub struct VerificationReport {
    /// Signature over the report (stub - always empty)
    #[prost(message, optional, tag = 1)]
    pub sig: Option<VerificationSignature>,
    /// Certificate chain (stub - always empty)
    #[prost(bytes, repeated, tag = 2)]
    pub chain: Vec<Vec<u8>>,
    /// HTTP body (stub - always empty)
    #[prost(string, tag = 3)]
    pub http_body: String,
}

/// Stub for VerificationSignature - attestation is not used in Botho
#[derive(Clone, Deserialize, Digestible, Eq, Hash, PartialEq, Serialize, ::prost::Message)]
pub struct VerificationSignature {
    /// Signature bytes (stub - always empty)
    #[prost(bytes, tag = 1)]
    pub contents: Vec<u8>,
}
