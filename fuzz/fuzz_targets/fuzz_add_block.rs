#![no_main]

//! Fuzz target for `Ledger::add_block` — the C1–C4 block-acceptance surface.
//!
//! Blocks arrive over gossip from untrusted peers and bypass the mempool.
//! `add_block` is the single chokepoint that must reject every malformed or
//! consensus-violating block while NEVER panicking, and must never commit a
//! block that breaks the ledger's accounting invariants.
//!
//! ## Strategy
//!
//! We deserialize an arbitrary `Block` from raw bytes (the real wire format
//! is bincode; `Block` does not derive `Arbitrary`, and the existing
//! `fuzz_block` target establishes raw-bytes deserialization as the canonical
//! input shape). The decoded block is fed to a freshly-initialized
//! genesis ledger.
//!
//! ## Invariant asserted (issue #337, target 1)
//!
//! 1. `add_block` never panics — any input either deserializes-and-rejects or
//!    deserializes-and-accepts; it must not crash.
//! 2. It returns either `Ok(())` or a typed `LedgerError`. (Enforced by the
//!    type system + the no-panic guarantee; we additionally assert the result
//!    is observable without panicking.)
//! 3. On `Ok(())`, the post-state invariants MUST hold against the pre-state
//!    snapshot taken before the call:
//!      - `height == prev_height + 1`
//!      - `total_mined == prev_total_mined + block.minting_tx.reward`
//!      - supply conservation: `total_mined == Σ(UTXO values) +
//!        total_fees_burned + lottery_pool`
//!
//! A violation of any of these on an accepted block is a consensus break, so
//! we `assert!` (which aborts under libfuzzer and is reported as a crash).

use libfuzzer_sys::fuzz_target;

use botho::{block::Block, ledger::Ledger};

/// Build a fresh genesis ledger in a unique temp dir. Returns `None` if the
/// environment cannot provide a temp dir (should not happen in CI).
fn fresh_ledger() -> Option<(tempfile::TempDir, Ledger)> {
    let dir = tempfile::tempdir().ok()?;
    let ledger = Ledger::open(dir.path()).ok()?;
    Some((dir, ledger))
}

/// Sum the value of every UTXO currently in the ledger. Uses the public
/// snapshot API so we observe exactly what the ledger committed.
fn sum_utxo_values(ledger: &Ledger) -> u128 {
    let snapshot = ledger
        .create_snapshot()
        .expect("snapshot creatable after successful add_block");
    let utxos = snapshot
        .get_utxos()
        .expect("snapshot UTXOs decodable after successful add_block");
    utxos.iter().map(|u| u.output.amount as u128).sum()
}

fuzz_target!(|data: &[u8]| {
    // Decode an arbitrary block. Most random inputs fail here, which is fine:
    // we are fuzzing the *acceptance* path, not the parser (fuzz_block covers
    // parsing). A successful decode gives us a structurally-valid block to
    // drive through add_block.
    let block: Block = match bincode::deserialize::<Block>(data) {
        Ok(b) => b,
        Err(_) => return,
    };

    let (_dir, ledger) = match fresh_ledger() {
        Some(l) => l,
        None => return,
    };

    // Snapshot the pre-state so we can verify the post-conditions of an
    // accepted block.
    let prev = match ledger.get_chain_state() {
        Ok(s) => s,
        Err(_) => return,
    };
    let expected_reward = block.minting_tx.reward;

    // The call under test. Must never panic; returns Ok(()) or LedgerError.
    match ledger.add_block(&block) {
        Err(_e) => {
            // Rejection is the overwhelmingly common (and correct) outcome for
            // arbitrary blocks. Nothing to assert beyond "did not panic".
        }
        Ok(()) => {
            // The block was ACCEPTED. Every post-state invariant must hold.
            let post = ledger
                .get_chain_state()
                .expect("chain state readable after successful add_block");

            // Invariant 3a: height advanced by exactly one.
            assert_eq!(
                post.height,
                prev.height + 1,
                "accepted block did not advance height by exactly 1 \
                 (prev={}, post={})",
                prev.height,
                post.height
            );

            // Invariant 3b: total_mined grew by exactly the block reward.
            assert_eq!(
                post.total_mined,
                prev.total_mined + expected_reward,
                "accepted block did not increase total_mined by reward \
                 (prev={}, post={}, reward={})",
                prev.total_mined,
                post.total_mined,
                expected_reward
            );

            // Invariant 3c: supply conservation. Every credit ever minted is
            // accounted for by exactly one of: live UTXO value, burned fees,
            // or the carryover lottery pool. Computed in u128 to avoid the
            // fuzzer being able to mask a real break behind an arithmetic
            // overflow of the bookkeeping itself.
            let lottery_pool = ledger
                .get_lottery_pool()
                .expect("lottery pool readable after successful add_block");
            let utxo_sum = sum_utxo_values(&ledger);
            let accounted = utxo_sum + post.total_fees_burned as u128 + lottery_pool as u128;
            assert_eq!(
                post.total_mined as u128, accounted,
                "supply conservation violated after accepted block: \
                 total_mined={} != utxo_sum={} + burned={} + pool={}",
                post.total_mined, utxo_sum, post.total_fees_burned, lottery_pool
            );
        }
    }
});
