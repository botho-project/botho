// Copyright (c) 2024 The Botho Foundation

//! BTH reserve-release submission and confirmation.
//!
//! The burn flow's destination side: on a confirmed wBTH burn, the bridge
//! pays the user's BTH address from the locked reserve. [`Releaser`] splits
//! a release into three retryable stages so the engine can guarantee
//! exactly-once releasing (mirroring the mint-side [`crate::mint::Minter`]):
//!
//! 1. [`Releaser::prepare_release`] — build and threshold-sign the BTH
//!    transaction locally (no broadcast). The resulting [`PreparedRelease`]
//!    carries the exact raw bytes so a retry re-broadcasts the SAME transaction
//!    (same inputs / key images) instead of signing a competing one — the
//!    engine persists both the tx hash AND the raw bytes to the
//!    `release_claims` table BEFORE the first broadcast, so a post-restart
//!    resume re-broadcasts rather than re-signs.
//! 2. [`Releaser::broadcast`] — submit via the node's `tx_submit` RPC.
//!    Idempotent: "already known" / key-image-already-spent-by-this-tx
//!    responses are success.
//! 3. [`Releaser::check_confirmation`] — poll until the configured depth
//!    (`release_confirmations_required`; 0 = SCP externalization finality) is
//!    met, reporting a provably-dead transaction so the engine can unwind
//!    `ReleasePending -> BurnConfirmed` and re-submit.
//!
//! Unlike the mint side, BTH has **no on-chain order-id guard**: the ONLY
//! double-release protections are the `release_claims` idempotency table,
//! the never-re-sign-after-record rule above, and key-image conflicts
//! between transactions that spend the same reserve inputs. The unwind edge
//! is therefore restricted to transactions that provably cannot land (see
//! [`ReleaseConfirmation::Dropped`]).

pub mod bth;

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, ReleaseAuthorization};

/// Errors from release submission / confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseError {
    /// Misconfiguration (missing reserve address, bad federation key, ...).
    Config(String),
    /// The provided attestation does not authorize this release.
    Attestation(String),
    /// RPC / network failure (retryable).
    /// TODO(#856): constructed by the live-node RPC wiring.
    #[allow(dead_code)]
    Rpc(String),
    /// Functionality not yet wired up (BTH wallet-send RPC, see #856).
    NotImplemented(String),
}

impl std::fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReleaseError::Config(m) => write!(f, "config error: {}", m),
            ReleaseError::Attestation(m) => write!(f, "attestation error: {}", m),
            ReleaseError::Rpc(m) => write!(f, "rpc error: {}", m),
            ReleaseError::NotImplemented(m) => write!(f, "not implemented: {}", m),
        }
    }
}

impl std::error::Error for ReleaseError {}

/// A fully signed, ready-to-broadcast BTH release transaction.
#[derive(Debug, Clone)]
pub struct PreparedRelease {
    /// BTH transaction hash. Known before broadcast.
    pub tx_hash: String,
    /// The exact signed bytes to (re)broadcast. Persisted alongside the
    /// hash so a crash after signing NEVER leads to re-signing with
    /// different inputs (which could double-spend the reserve).
    pub raw: Vec<u8>,
}

/// Result of polling a submitted release transaction.
///
/// The engine consumes every variant; production construction is the
/// `TODO(#856)` confirmation-polling wiring (tests construct them via the
/// mock releaser), hence the `dead_code` allowance.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseConfirmation {
    /// Not yet at the required depth; keep the order in `ReleasePending`.
    Pending {
        /// Confirmations observed so far (0 = not yet in a block).
        confirmations: u64,
    },
    /// Required depth reached (SCP finality by default) and the recipient
    /// output is present: safe to advance to `Released`.
    Confirmed,
    /// The transaction **provably cannot land**: its key images were spent
    /// by a different transaction, or it is permanently invalid against the
    /// chain. Implementations must NOT report a merely-unseen transaction
    /// as `Dropped` — with no on-chain order-id guard on BTH, unwinding and
    /// re-signing while the old tx could still land risks a double release.
    /// The engine reacts by rolling the order back to `BurnConfirmed` and
    /// re-running submission (which signs a fresh transaction).
    Dropped,
    /// The transaction landed but is wrong (e.g. pays the wrong output).
    /// Requires operator attention; the order is marked `Failed`.
    Failed {
        /// Human-readable failure description.
        reason: String,
    },
}

/// A BTH reserve-release backend.
#[async_trait]
pub trait Releaser: Send + Sync {
    /// Build and threshold-sign the release transaction for `order`,
    /// authorized by `auth` (the #824 threshold attestation). Verifies the
    /// attestation before any reserve key material is touched. Does NOT
    /// broadcast.
    async fn prepare_release(
        &self,
        order: &BridgeOrder,
        auth: &ReleaseAuthorization,
    ) -> Result<PreparedRelease, ReleaseError>;

    /// Broadcast (or re-broadcast) a prepared transaction. Idempotent:
    /// "already known" responses are success.
    async fn broadcast(&self, prepared: &PreparedRelease) -> Result<(), ReleaseError>;

    /// Poll the confirmation state of a submitted release transaction.
    async fn check_confirmation(
        &self,
        order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ReleaseConfirmation, ReleaseError>;
}
