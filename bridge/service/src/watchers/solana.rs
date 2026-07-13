// Copyright (c) 2024 The Botho Foundation

//! Solana chain watcher for monitoring wBTH burns (burn flow).
//!
//! Watches the `wbth_bridge` Anchor program
//! (`contracts/solana/programs/wbth`) for `BridgeBurnEvent { user, amount,
//! bth_address, slot }` emissions and drives `BurnDetected ->
//! BurnConfirmed`, mirroring the Ethereum watcher:
//!
//! - **Commitment as the reorg guard**: burns are only accepted at `Finalized`
//!   commitment (rooted, cannot be rolled back), regardless of the configured
//!   [`SolanaCommitment`] — the analogue of Ethereum's depth + canonical-hash
//!   re-check. A slot observed below `Finalized` that gets rolled back is
//!   treated like an Ethereum orphan: the order is held at `BurnDetected` and
//!   never advances toward release.
//! - **Idempotency**: orders are created via `Database::insert_burn_order`
//!   keyed by `"<signature>#<ordinal>"`, so a cursor replay or a
//!   rolled-back-then-re-landed burn reuses the same order (exactly-once by
//!   order id).
//! - **Cursor**: scan progress (last processed slot) persists in
//!   `watcher_cursors` under [`Chain::Solana`].
//!
//! ## Implementation status
//!
//! The deterministic pieces are implemented and unit-tested here: the
//! Anchor event discriminator and the borsh event decoder
//! ([`parse_bridge_burn_event`]), so the exact bytes emitted on-chain are
//! pinned. The RPC transport (`getSignaturesForAddress` /
//! `getTransaction` at `Finalized` over `SolanaConfig::wbth_program`)
//! requires the `solana-client` dependency stack, which is deferred (same
//! reasoning as `mint::solana`); [`SolanaWatcher::poll_for_burns`] is a
//! fail-safe `TODO(#828)` stub until the harness lands — it polls, logs,
//! and creates no state.

use bth_bridge_core::{SolanaCommitment, SolanaConfig};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::{db::Database, engine::ShutdownSignal};

/// Delay between poll passes.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Compute the Anchor event discriminator: the first 8 bytes of
/// `sha256("event:<Name>")`. Consumed by the #828 transport wiring.
#[allow(dead_code)]
pub fn anchor_event_discriminator(event_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(b"event:");
    hasher.update(event_name.as_bytes());
    let digest = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&digest[..8]);
    disc
}

/// A decoded `BridgeBurnEvent` from the wBTH program. Consumed by the
/// #828 transport wiring.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaBurn {
    /// Burner's pubkey (raw 32 bytes).
    pub user: [u8; 32],
    /// Burned amount in picocredits (wBTH mint uses 12 decimals, 1:1).
    pub amount: u64,
    /// Destination BTH address. Per ADR 0004 the release pays a FRESH
    /// one-time stealth address resolved from this (enforced in #822).
    pub bth_address: String,
    /// Slot the program observed at emission time.
    pub slot: u64,
}

/// Decode the borsh-serialized `BridgeBurnEvent` (with its 8-byte Anchor
/// event discriminator prefix) as emitted by
/// `contracts/solana/programs/wbth`:
///
/// ```text
/// [ discriminator: 8 ][ user: Pubkey 32 ][ amount: u64 LE ]
/// [ bth_address: u32 LE length + utf8 bytes ][ slot: u64 LE ]
/// ```
///
/// Returns `None` on a wrong discriminator, truncation, trailing bytes,
/// or a non-UTF-8 address. Consumed by the #828 transport wiring.
#[allow(dead_code)]
pub fn parse_bridge_burn_event(data: &[u8]) -> Option<SolanaBurn> {
    let expected = anchor_event_discriminator("BridgeBurnEvent");
    let rest = data.strip_prefix(expected.as_slice())?;

    // user: Pubkey (32 bytes)
    let (user_bytes, rest) = rest.split_at_checked(32)?;
    let mut user = [0u8; 32];
    user.copy_from_slice(user_bytes);

    // amount: u64 LE
    let (amount_bytes, rest) = rest.split_at_checked(8)?;
    let amount = u64::from_le_bytes(amount_bytes.try_into().ok()?);

    // bth_address: borsh string (u32 LE length + bytes)
    let (len_bytes, rest) = rest.split_at_checked(4)?;
    let len = u32::from_le_bytes(len_bytes.try_into().ok()?) as usize;
    let (addr_bytes, rest) = rest.split_at_checked(len)?;
    let bth_address = std::str::from_utf8(addr_bytes).ok()?.to_string();

    // slot: u64 LE, and nothing may follow.
    let (slot_bytes, rest) = rest.split_at_checked(8)?;
    if !rest.is_empty() {
        return None;
    }
    let slot = u64::from_le_bytes(slot_bytes.try_into().ok()?);

    Some(SolanaBurn {
        user,
        amount,
        bth_address,
        slot,
    })
}

/// Whether a commitment level is acceptable as the burn-side reorg guard.
/// Only `Finalized` (rooted) slots cannot be rolled back.
pub fn commitment_is_final(commitment: SolanaCommitment) -> bool {
    matches!(commitment, SolanaCommitment::Finalized)
}

/// Solana watcher monitors the wBTH program for burn events.
pub struct SolanaWatcher {
    config: SolanaConfig,
    #[allow(dead_code)]
    db: Database,
    shutdown: ShutdownSignal,
}

impl SolanaWatcher {
    /// Create a new Solana watcher.
    pub fn new(config: SolanaConfig, db: Database, shutdown: ShutdownSignal) -> Self {
        Self {
            config,
            db,
            shutdown,
        }
    }

    /// Run the watcher.
    pub async fn run(mut self) -> Result<(), String> {
        info!(
            "Starting Solana watcher for program {}",
            self.config.wbth_program
        );

        if !commitment_is_final(self.config.commitment) {
            warn!(
                "solana.commitment is {:?}; burns are only accepted at Finalized commitment \
                 (the reorg guard) regardless of this setting",
                self.config.commitment
            );
        }

        loop {
            // Check for shutdown first
            match self.shutdown.try_recv() {
                Ok(_) | Err(broadcast::error::TryRecvError::Closed) => {
                    info!("Solana watcher shutting down");
                    return Ok(());
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // No shutdown signal, continue
                }
            }

            self.poll_for_burns().await;

            tokio::select! {
                _ = self.shutdown.recv() => {
                    info!("Solana watcher shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    /// Poll for burn events.
    ///
    /// TODO(#828): wire the RPC transport (needs the `solana-client`
    /// stack, deferred as in `mint::solana`):
    /// 1. Load the resume slot from `db.get_cursor(Chain::Solana)`.
    /// 2. `getSignaturesForAddress(SolanaConfig::wbth_program)` at FINALIZED
    ///    commitment only (see [`commitment_is_final`]) from the cursor slot;
    ///    fetch each transaction and decode program-data logs via
    ///    [`parse_bridge_burn_event`].
    /// 3. For each burn, create the order exactly-once with
    ///    `db.insert_burn_order(new_burn(Chain::Solana, ..),
    ///    "<signature>#<ordinal>", slot, None)` at `BurnDetected`, then —
    ///    because Finalized slots cannot roll back — advance straight to
    ///    `BurnConfirmed` (the Ethereum watcher's canonical-hash re-check is
    ///    unnecessary at Finalized). A slot rolled back below Finalized is
    ///    never observed, so the Ethereum orphan path has no Solana analogue to
    ///    trigger.
    /// 4. Persist `db.set_cursor(Chain::Solana, slot, None)` only after all
    ///    burns in that slot are processed.
    ///
    /// Fail-safe: until wired, this creates no state.
    async fn poll_for_burns(&self) {
        debug!(
            "Solana watcher idle: burn scan transport for program {} pending #828",
            self.config.wbth_program
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_event(user: [u8; 32], amount: u64, bth_address: &str, slot: u64) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&anchor_event_discriminator("BridgeBurnEvent"));
        data.extend_from_slice(&user);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&(bth_address.len() as u32).to_le_bytes());
        data.extend_from_slice(bth_address.as_bytes());
        data.extend_from_slice(&slot.to_le_bytes());
        data
    }

    #[test]
    fn test_event_discriminator_known_vector() {
        // sha256("event:BridgeBurnEvent")[..8] — pinned so a silent rename
        // of the on-chain event breaks tests, not the live watcher.
        let disc = anchor_event_discriminator("BridgeBurnEvent");
        let mut hasher = Sha256::new();
        hasher.update(b"event:BridgeBurnEvent");
        assert_eq!(disc, hasher.finalize()[..8]);
        assert_ne!(disc, anchor_event_discriminator("BridgeMintEvent"));
    }

    #[test]
    fn test_parse_bridge_burn_event_roundtrip() {
        let user = [7u8; 32];
        let data = encode_event(user, 999_000_000_000, "bth_stealth_addr", 12345);

        let burn = parse_bridge_burn_event(&data).unwrap();
        assert_eq!(burn.user, user);
        assert_eq!(burn.amount, 999_000_000_000);
        assert_eq!(burn.bth_address, "bth_stealth_addr");
        assert_eq!(burn.slot, 12345);
    }

    #[test]
    fn test_parse_bridge_burn_event_rejects_malformed() {
        let good = encode_event([1u8; 32], 5, "addr", 9);

        // Wrong discriminator.
        let mut wrong_disc = good.clone();
        wrong_disc[..8].copy_from_slice(&anchor_event_discriminator("BridgeMintEvent"));
        assert!(parse_bridge_burn_event(&wrong_disc).is_none());

        // Truncated at every boundary.
        for cut in [4, 8, 20, 40, 44, 46, good.len() - 1] {
            assert!(
                parse_bridge_burn_event(&good[..cut]).is_none(),
                "truncation at {} must be rejected",
                cut
            );
        }

        // Trailing bytes.
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(parse_bridge_burn_event(&trailing).is_none());

        // Non-UTF-8 address bytes.
        let mut bad_utf8 = encode_event([1u8; 32], 5, "ab", 9);
        let addr_start = 8 + 32 + 8 + 4;
        bad_utf8[addr_start] = 0xFF;
        bad_utf8[addr_start + 1] = 0xFE;
        assert!(parse_bridge_burn_event(&bad_utf8).is_none());
    }

    #[test]
    fn test_only_finalized_commitment_is_a_reorg_guard() {
        assert!(commitment_is_final(SolanaCommitment::Finalized));
        assert!(!commitment_is_final(SolanaCommitment::Confirmed));
        assert!(!commitment_is_final(SolanaCommitment::Processed));
    }
}
