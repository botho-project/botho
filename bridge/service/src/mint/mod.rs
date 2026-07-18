// Copyright (c) 2024 The Botho Foundation

//! Destination-chain mint submission and confirmation.
//!
//! Each destination chain implements [`Minter`], which splits a mint into
//! three retryable stages so the engine can guarantee exactly-once minting:
//!
//! 1. [`Minter::prepare_mint`] — build and sign the transaction locally (no
//!    broadcast). The resulting [`PreparedMint`] carries the exact raw bytes so
//!    a retry re-broadcasts the SAME transaction (same nonce, same on-chain
//!    order id) instead of creating a competing one.
//! 2. [`Minter::broadcast`] — send the raw transaction. The engine persists the
//!    tx id to the `mints` idempotency table BEFORE the first broadcast.
//! 3. [`Minter::check_confirmation`] — poll until the confirmation requirement
//!    (`confirmations_required` blocks on Ethereum, the configured commitment
//!    on Solana) is met, reporting reorgs so the engine can unwind `MintPending
//!    -> DepositConfirmed` and re-submit.

pub mod ethereum;
mod keysource;
pub mod solana;

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, Chain, MintAuthorization};

/// Errors from mint submission / confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    /// Misconfiguration (bad address, missing key/Safe, ...).
    Config(String),
    /// The provided attestation does not authorize this mint.
    Attestation(String),
    /// RPC / network failure (retryable).
    Rpc(String),
    /// The Safe nonce the collected signatures are bound to no longer matches
    /// the Safe's current on-chain nonce (an unrelated Safe transaction
    /// executed between attestation collection and mint submission). Detected
    /// **before broadcast** so the engine re-authorizes and re-collects at the
    /// fresh nonce (#848) instead of broadcasting a transaction the Safe will
    /// reject. Retryable via re-authorization.
    StaleNonce(String),
}

impl std::fmt::Display for MintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MintError::Config(m) => write!(f, "config error: {}", m),
            MintError::Attestation(m) => write!(f, "attestation error: {}", m),
            MintError::Rpc(m) => write!(f, "rpc error: {}", m),
            MintError::StaleNonce(m) => write!(f, "stale safe nonce: {}", m),
        }
    }
}

impl std::error::Error for MintError {}

/// A fully signed, ready-to-broadcast mint transaction.
#[derive(Debug, Clone)]
pub struct PreparedMint {
    /// Destination-chain transaction id (0x-prefixed hash on Ethereum,
    /// base58 signature on Solana). Known before broadcast.
    pub tx_id: String,
    /// The exact signed bytes to (re)broadcast.
    pub raw: Vec<u8>,
}

/// Result of polling a submitted mint transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationStatus {
    /// Not yet at the required depth; keep the order in `MintPending`.
    Pending {
        /// Confirmations observed so far (0 = not yet mined / processed).
        confirmations: u64,
    },
    /// Required depth reached, block canonical, and the mint event with the
    /// bound order id is present: safe to advance to `Completed`.
    Confirmed,
    /// The transaction was dropped or its block reorged out before
    /// finality. The engine must roll the order back to `DepositConfirmed`
    /// and re-run submission against the same on-chain order id.
    Reorged,
    /// The transaction executed but the mint did not happen (e.g. the Safe
    /// inner call failed / the contract rejected the mint). Requires
    /// operator attention — auto-resubmitting could race a rate limit.
    Failed {
        /// Human-readable failure description.
        reason: String,
    },
}

/// A destination-chain minting backend.
#[async_trait]
pub trait Minter: Send + Sync {
    /// The chain this minter submits to.
    fn chain(&self) -> Chain;

    /// Build and sign the mint transaction for `order`, authorized by
    /// `auth` (the #824 threshold attestation). Does NOT broadcast.
    async fn prepare_mint(
        &self,
        order: &BridgeOrder,
        auth: &MintAuthorization,
    ) -> Result<PreparedMint, MintError>;

    /// Broadcast (or re-broadcast) a prepared transaction. Idempotent:
    /// "already known" responses are success.
    async fn broadcast(&self, prepared: &PreparedMint) -> Result<(), MintError>;

    /// Poll the confirmation state of a submitted mint transaction.
    async fn check_confirmation(
        &self,
        order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ConfirmationStatus, MintError>;
}
