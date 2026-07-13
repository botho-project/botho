// Copyright (c) 2024 The Botho Foundation

//! Chain watchers for monitoring deposits and burns (#823).
//!
//! Each source chain gets a watcher that drives events into the order
//! state machine with three shared safety properties:
//!
//! 1. **Durable cursors** — per-chain scan progress lives in the
//!    `watcher_cursors` table and is persisted only after a block is fully
//!    processed, so a restart resumes without missing events and a crash
//!    replays (never skips) the in-flight block.
//! 2. **Idempotent event→order creation** — deposits dedup on
//!    `processed_deposits` (by tx hash) and burns on `processed_burns` (by
//!    `"<source_tx>#<ordinal>"`), independent of the cursor, so cursor rewinds
//!    and reorg re-adds can never double-process.
//! 3. **Reorg safety** — orders only advance to their `*Confirmed` state at the
//!    chain's finality: SCP finality on BTH, `confirmations_required` depth
//!    plus a canonical-block-hash re-check on Ethereum, and `Finalized`
//!    commitment on Solana.

mod bth;
mod ethereum;
mod solana;

pub use bth::BthWatcher;
pub use ethereum::EthereumWatcher;
pub use solana::SolanaWatcher;

/// Errors from chain watchers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum WatchError {
    /// Misconfiguration (bad address, unparsable URL, ...).
    Config(String),
    /// RPC / network failure (retryable next poll).
    Rpc(String),
    /// Database failure.
    Db(String),
    /// Transport not yet wired up (BTH websocket / Solana RPC, see #828).
    NotImplemented(String),
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchError::Config(m) => write!(f, "config error: {}", m),
            WatchError::Rpc(m) => write!(f, "rpc error: {}", m),
            WatchError::Db(m) => write!(f, "db error: {}", m),
            WatchError::NotImplemented(m) => write!(f, "not implemented: {}", m),
        }
    }
}

impl std::error::Error for WatchError {}
