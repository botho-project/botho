// Copyright (c) 2024 The Botho Foundation

//! BTH bridge service library target (#897).
//!
//! This crate is first and foremost the `bth-bridge` **binary** (see
//! `src/main.rs`). This library target exists for exactly one reason: the
//! service-internal *pure byte parsers* sit on adversarial input paths
//! (Solana account bytes fetched over RPC, peer HTTP responses, federation
//! configuration) and must be reachable from the out-of-workspace
//! `botho-fuzz` crate, which can only link a `[lib]` target (#892 Tier B /
//! Option B1).
//!
//! **This is NOT a public API.** The service internals (engine, database,
//! watchers, RPC clients, attestation provider, ...) stay private: the only
//! items exported here are the side-effect-free parsers under fuzz, the
//! supporting types needed to name their signatures, and a `#[doc(hidden)]`
//! facade with the two entry-point types the sibling binary wires together.
//! Do not grow this surface without the same scrutiny a bridge-core `pub`
//! item gets — this crate holds custody-relevant logic (ADR 0002).

// The full module tree lives here (moved from `main.rs` in #897) so the
// parsers are compiled exactly once and the existing unit tests keep running
// unmodified under the library test harness.
#[cfg(test)]
mod adversarial_tests;
mod api;
mod attestation;
#[cfg(test)]
mod bth_fork_tests;
mod bth_keys;
mod bth_rpc;
mod bth_scan;
#[cfg(test)]
mod chaos_tests;
mod db;
#[cfg(test)]
mod defi_round_trip_tests;
#[cfg(test)]
mod e2e_full_loop_tests;
mod engine;
mod federation;
#[cfg(test)]
mod fork_tests;
mod mint;
mod public_api;
mod release;
mod reserve;
#[cfg(test)]
mod solana_devnet_tests;
mod solana_rpc;
#[cfg(test)]
mod uniswap_fork_tests;
mod watchers;

// ============================================================================
// Fuzzable pure parsers (#897) — the ONLY supported exports of this library.
// ============================================================================

/// Federation-config duplicate-signer rejection (service-root
/// `attestation.rs`; distinct from bridge-core's `attestation.rs`).
pub use attestation::reject_duplicate_signers;
/// Recipient-address decode for the native-BTH release path (#1076): the
/// attacker-controlled `bthAddress` from a wBTH burn becomes the release
/// recipient, so its decode sits on an adversarial input path.
pub use bth_scan::decode_recipient_address;
/// HTTP status-line parsing for peer push responses, plus the read cap the
/// async I/O layer applies before the parse ever sees the bytes.
pub use federation::{parse_status_line, MAX_PEER_RESPONSE_BYTES};
/// Pre-broadcast Gnosis Safe nonce cross-check (#848).
pub use mint::ethereum::check_attested_nonce;
/// Solana `Bridge` account-layout parsers (#850 layout).
pub use mint::solana::{
    parse_bridge_mint, parse_bridge_mint_authority, BRIDGE_MINT_AUTHORITY_OFFSET,
    BRIDGE_MINT_OFFSET,
};
/// Supporting types required to name the parser signatures above.
pub use mint::MintError;
/// Proof-of-reserves verdict math (#1078): the pure, side-effect-free
/// drift/tolerance/aggregation + `peg_healthy` derivation the live
/// `Reconciler` runs, exposed so the `fuzz_bridge_reserve_math` coverage-guided
/// target drives the exact same function (no test-only reimplementation that
/// could drift from production). A false-healthy peg is the highest-severity
/// bridge failure, so this custody-relevant math sits under continuous fuzzing.
pub use reserve::{reserve_verdict, ChainFigure, ChainReserveStatus, ReserveVerdict};
pub use solana_rpc::Pubkey;
/// Ethereum burn-log decode (#1076): fuzz seams that assemble a `BridgeBurn`
/// RPC log from attacker-controlled parts / raw ABI bytes and run it through
/// the production `decode_burn_log`, plus the decoded event type. The
/// `bthAddress` string and `uint256` amount are fully attacker-controlled.
pub use watchers::ethereum::{fuzz_decode_burn_log_parts, fuzz_decode_burn_log_raw, BurnEvent};

/// `check_attested_nonce` takes the Safe's on-chain nonce as an
/// `alloy::primitives::U256`; re-exported so callers (the fuzz crate) do not
/// need a direct `alloy` dependency.
pub use alloy::primitives::U256;

/// Entry-point types for the sibling `bth-bridge` binary ONLY.
///
/// Hidden from docs and unsupported as an external API: the binary and the
/// library ship from the same package, so this facade is the minimal seam
/// that lets `main.rs` wire config -> database -> engine without making the
/// service internals `pub`.
#[doc(hidden)]
pub mod bin_support {
    pub use crate::{db::Database, engine::BridgeEngine};
}
