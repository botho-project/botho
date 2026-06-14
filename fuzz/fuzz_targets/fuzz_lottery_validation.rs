#![no_main]

//! Fuzz target for `validate_block_lottery` — the lottery fee-redistribution
//! acceptance check (`botho/src/consensus/lottery.rs`).
//!
//! This is consensus-critical economic state: the function decides whether a
//! block's claimed lottery split (80% pool / 20% burn) and payouts match the
//! deterministic pool accounting, and it returns the new carryover pool. A
//! panic here crashes validators; a wrong `Ok` mints or destroys supply.
//!
//! ## Strategy
//!
//! We deserialize an arbitrary `Block` (giving arbitrary `lottery_summary`,
//! `lottery_outputs`, `minting_tx`, fees) and synthesize an arbitrary
//! candidate set, stored pool, and prev-block hash. We then drive the REAL
//! `validate_block_lottery`.
//!
//! ## Invariants asserted (issue #337, target 3)
//!
//! 1. **Never panics** on any input (including degenerate candidate sets,
//!    zero/overflow payouts, empty/huge output vectors).
//! 2. **Deterministic**: the same input produces the same result. We call it
//!    twice and assert the `Ok`/`Err` discriminant and `Ok` payload match.
//! 3. **Pool-accounting correctness (80/20)**: whenever it returns `Ok`, the
//!    block's claimed split must agree with the independently-recomputed
//!    `compute_pool_accounting` — specifically `amount_burned` must equal the
//!    accounting's `fee_burn`, and `pool_distributed` must equal the capped
//!    `payout`. A different split that nonetheless returned `Ok` is a break.
//!    (`compute_pool_accounting` is the *input* contract, recomputed here via
//!    the public API from independent ground-truth fee/emission/pool values —
//!    not the function under test — so the cross-check is non-tautological.)

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use botho::{
    block::Block,
    consensus::lottery::{compute_pool_accounting, validate_block_lottery, LotteryFeeConfig},
};
use bth_cluster_tax::{LotteryCandidate, TagVector};

#[derive(Debug, Arbitrary)]
struct FuzzCandidate {
    id: [u8; 36],
    value: u64,
    cluster_factor: u64,
    creation_block: u64,
}

#[derive(Debug, Arbitrary)]
struct FuzzData {
    /// Raw bytes decoded into a Block (arbitrary lottery_summary / outputs).
    block_bytes: Vec<u8>,
    candidates: Vec<FuzzCandidate>,
    stored_pool: u64,
    prev_block_hash: [u8; 32],
}

fn build_candidate(f: &FuzzCandidate) -> LotteryCandidate {
    // LotteryCandidate::new clamps cluster_factor into [FACTOR_SCALE,
    // MAX_FACTOR_SCALED] and derives entropy from an (empty) tag vector.
    LotteryCandidate::new(
        f.id,
        f.value,
        f.cluster_factor,
        &TagVector::new(),
        f.creation_block,
    )
}

fuzz_target!(|data: FuzzData| {
    let block: Block = match bincode::deserialize::<Block>(&data.block_bytes) {
        Ok(b) => b,
        Err(_) => return,
    };

    let candidates: Vec<LotteryCandidate> = data
        .candidates
        .iter()
        .take(64)
        .map(build_candidate)
        .collect();

    let config = LotteryFeeConfig::default();
    let stored_pool = data.stored_pool;
    let prev = data.prev_block_hash;

    // Invariant 1: never panics (the call returning is the assertion).
    let r1 = validate_block_lottery(&block, &candidates, stored_pool, &prev, &config);
    // Invariant 2: deterministic — same inputs, same result.
    let r2 = validate_block_lottery(&block, &candidates, stored_pool, &prev, &config);
    match (&r1, &r2) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "validate_block_lottery non-deterministic Ok value"),
        (Err(a), Err(b)) => assert_eq!(a, b, "validate_block_lottery non-deterministic Err value"),
        _ => panic!("validate_block_lottery non-deterministic Ok/Err discriminant"),
    }

    // Invariant 3: an accepted block's claimed split must match the
    // independently-recomputed pool accounting (the 80/20 contract).
    if r1.is_ok() {
        let total_fees = block.total_fees();
        let emission_share = block.minting_tx.lottery_emission_share();
        let accounting = compute_pool_accounting(
            total_fees,
            emission_share,
            stored_pool,
            block.minting_tx.reward,
            &config,
        );

        // The burn share must always equal the fee burn (both branches).
        assert_eq!(
            block.lottery_summary.amount_burned, accounting.fee_burn,
            "accepted lottery has burn {} != expected fee_burn {} (80/20 split broken)",
            block.lottery_summary.amount_burned, accounting.fee_burn
        );
        // Pool distribution depends on the branch:
        //  - winners present: pool_distributed must equal the capped payout;
        //  - no winners: the pool share carries over, so pool_distributed == 0 (the
        //    carryover is reflected in the returned new pool, not the summary).
        //    Asserting `== payout` unconditionally would be a FALSE POSITIVE on valid
        //    no-winner blocks where payout > 0.
        if block.lottery_outputs.is_empty() {
            assert_eq!(
                block.lottery_summary.pool_distributed, 0,
                "accepted no-winner lottery must distribute 0, got {}",
                block.lottery_summary.pool_distributed
            );
        } else {
            assert_eq!(
                block.lottery_summary.pool_distributed, accounting.payout,
                "accepted lottery distributed {} != expected capped payout {}",
                block.lottery_summary.pool_distributed, accounting.payout
            );
        }
    }
});
