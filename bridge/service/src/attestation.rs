// Copyright (c) 2024 The Botho Foundation

//! Attestation provider — the engine's source of [`MintAuthorization`]s.
//!
//! The real implementation is the #824 validator attestation protocol
//! (t-of-n threshold signing by the SCP validator federation, ADR 0002),
//! which mirrors the operator-signed-action envelope/nonce machinery from
//! P4.4. This module defines the interface the mint engine consumes and a
//! development stub, so #821 (mint submission) and #824 (signature
//! collection) can land independently.

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, Chain, MintAuthorization, SignatureScheme};

/// Source of threshold mint authorizations.
#[async_trait]
pub trait AttestationProvider: Send + Sync {
    /// Obtain a threshold authorization for minting `order` on its
    /// destination chain. Blocks (or errors) until the federation threshold
    /// is met — the engine never submits an unauthorized mint.
    async fn authorize_mint(&self, order: &BridgeOrder) -> Result<MintAuthorization, String>;
}

/// Development stub pending #824.
///
/// Returns an authorization bound to the order's on-chain id with an EMPTY
/// signature set and threshold 0. This satisfies the local threshold check,
/// but a real Gnosis Safe / on-chain multisig authority will reject the
/// submission (no owner signatures), so it cannot mint against production
/// contracts. Useful against dev deployments whose Safe threshold is 0 or
/// whose authority is a plain EOA.
pub struct StubAttestationProvider;

#[async_trait]
impl AttestationProvider for StubAttestationProvider {
    async fn authorize_mint(&self, order: &BridgeOrder) -> Result<MintAuthorization, String> {
        // TODO(#824): replace with the validator threshold-signing protocol.
        let scheme = match order.dest_chain {
            Chain::Ethereum => SignatureScheme::Secp256k1,
            Chain::Solana => SignatureScheme::Ed25519,
            Chain::Bth => return Err("cannot mint to the BTH chain".to_string()),
        };

        Ok(MintAuthorization {
            order_id: order.order_id_bytes(),
            scheme,
            threshold: 0,
            signatures: vec![],
        })
    }
}
