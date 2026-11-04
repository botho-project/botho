// Copyright (c) 2024 Botho Foundation

//! Differential tests of the difficulty controller and the dynamic slot
//! duration against the Lean model in `formal/` (#1841, epic #1704).
//!
//! The vectors live in `formal/Botho/Difficulty/Vectors.lean`. That file is
//! two things at once:
//!
//! - a Lean module whose theorems (`stepVectors_ok`, `runVectors_ok`,
//!   `slotVectors_ok`) make the Lean kernel check that the MODEL produces every
//!   expected output in it (`lake build` fails otherwise), and
//! - the input of the tests below, which check that the RUST code produces the
//!   same outputs.
//!
//! So the two agree on every vector, and neither side can drift without one
//! of the two checks failing. The vectors are regenerated from the Rust code
//! by the ignored test `regenerate_formal_vectors`:
//!
//! ```text
//! FORMAL_VECTORS_OUT=$PWD/formal/Botho/Difficulty/Vectors.lean \
//!   cargo test -p botho --lib formal_drt_difficulty::tests::regenerate_formal_vectors \
//!   -- --ignored
//! ```
//!
//! followed by `lake build` in `formal/` (which is where a model mismatch
//! would show up).
//!
//! The three wrappers below are also the Rust side of the random DRT legs
//! `difficulty_step`, `difficulty_run` and `slot_duration` in `formal-drt/`
//! (#1706), which reach them through the non-default `test-fixtures`
//! feature. The module is compiled only for tests and with that feature, so
//! no node build contains it, and it changes no consensus code: each wrapper
//! calls the production function unchanged.
//!
//! The wrappers target the protocol 8.0.0 code (#1869): the difficulty kernel
//! and the slot-duration code are unchanged by that release, and the one new
//! piece on the difficulty path, the 3 s gap floor `observed_block_time`, is
//! exercised by [`chain_rust`]. The Lean model's timestamp-validity layer
//! (`tsValid`, `observedSecs`, `applyChain` in `Warp.lean`) is the protocol
//! <= 7.x rule set and is kept as the record of finding #1848; it is not a
//! DRT leg.

use crate::{
    block::difficulty::{observed_block_time, EmissionController},
    consensus::{ConsensusConfig, ConsensusService},
    ledger::ChainState,
};
use bth_consensus_scp::{QuorumSet, QuorumSetMember};
use bth_consensus_scp_types::test_utils::test_node_id;
use std::time::Duration;

/// `EmissionController::compute_time_adjusted_difficulty(c, o, t)`.
pub fn step_rust(c: u64, o: u64, t: u64) -> u64 {
    EmissionController::compute_time_adjusted_difficulty(c, o, t)
}

/// Difficulty after `EmissionController::record_block(.., Some(g))` for each
/// gap `g`, starting from `d0`.
///
/// This is the controller kernel the Lean `run` models: it takes the observed
/// gap as given. Since protocol 8.0.0 (#1869, finding #1848) the node derives
/// that gap with [`observed_block_time`], which floors it at
/// `MIN_OBSERVED_BLOCK_TIME_SECS` (3 s); [`chain_rust`] is that production
/// feed, and the tests pin `chain_rust` to `run_rust` over the floored gaps.
pub fn run_rust(d0: u64, gaps: &[u64]) -> u64 {
    let mut ctl = EmissionController::new(d0);
    for &g in gaps {
        ctl.record_block(0, 0, 0, Some(g));
    }
    ctl.difficulty
}

/// Difficulty after the protocol 8.0.0 production feed (#1869): each block
/// stamped `ts` on a parent stamped `parent` is recorded with
/// `observed_block_time(parent, ts)` (as `node::commit_block_locked` does),
/// starting from `d0` on a tip stamped `parent0`.
pub fn chain_rust(d0: u64, parent0: u64, stamps: &[u64]) -> u64 {
    let mut ctl = EmissionController::new(d0);
    let mut parent = parent0;
    for &ts in stamps {
        ctl.record_block(0, 0, 0, observed_block_time(parent, ts));
        parent = ts;
    }
    ctl.difficulty
}

/// `ConsensusService::current_slot_duration().as_secs()` after
/// `record_block(ts, tx)` for each `(ts, tx)` of `feed`, on a solo service
/// with `slot_duration = cfg` s and `dynamic_timing = dynamic`.
pub fn slot_rust(dynamic: bool, cfg: u64, feed: &[(u64, u64)]) -> u64 {
    let node = test_node_id(1);
    let config = ConsensusConfig {
        slot_duration: Duration::from_secs(cfg),
        dynamic_timing: dynamic,
        ..Default::default()
    };
    let mut svc = ConsensusService::new(
        node.clone(),
        QuorumSet::new(1, vec![QuorumSetMember::Node(node)]),
        config,
        ChainState::default(),
    );
    for &(ts, tx) in feed {
        svc.record_block(ts, tx as usize);
    }
    svc.current_slot_duration().as_secs()
}

#[cfg(test)]
mod tests;
