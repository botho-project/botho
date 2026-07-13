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
use bth_bridge_core::{
    BridgeOrder, Chain, MintAuthorization, ReleaseAuthorization, SignatureScheme,
};

/// Source of threshold mint and release authorizations.
#[async_trait]
pub trait AttestationProvider: Send + Sync {
    /// Obtain a threshold authorization for minting `order` on its
    /// destination chain. Blocks (or errors) until the federation threshold
    /// is met — the engine never submits an unauthorized mint.
    async fn authorize_mint(&self, order: &BridgeOrder) -> Result<MintAuthorization, String>;

    /// Obtain a threshold authorization for releasing `order`'s BTH from
    /// the reserve. Blocks (or errors) until the federation threshold is
    /// met — the engine never signs an unauthorized reserve spend. The
    /// returned authorization is bound to this order's deterministic id,
    /// its exact `net_amount()`, and its exact destination address (see
    /// [`bth_bridge_core::release_payload_digest`]).
    async fn authorize_release(&self, order: &BridgeOrder) -> Result<ReleaseAuthorization, String>;
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

    async fn authorize_release(&self, order: &BridgeOrder) -> Result<ReleaseAuthorization, String> {
        // TODO(#824): replace with the validator threshold-signing protocol
        // (Ed25519 signatures over release_payload_digest, mirroring the
        // operator-signed-action envelope/nonce machinery per ADR 0002).
        if order.dest_chain != Chain::Bth {
            return Err("release authorizations are only for the BTH chain".to_string());
        }

        // Empty signature set with threshold 0: passes the local distinct-
        // signer count only when the configured federation threshold floor
        // is also 0 (development). Any production configuration
        // (release_threshold >= 1) rejects this stub before any reserve
        // key material is touched — and release construction itself is
        // additionally gated on #828.
        Ok(ReleaseAuthorization {
            order_id: order.order_id_bytes(),
            amount: order.net_amount(),
            recipient: order.dest_address.clone(),
            threshold: 0,
            signatures: vec![],
        })
    }
}
