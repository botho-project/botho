// Copyright (c) 2024 Botho Foundation
//
//! Shared constants for e2e integration tests.

use botho::transaction::PICOCREDITS_PER_CREDIT;

/// Default number of nodes in the test network
pub const DEFAULT_NUM_NODES: usize = 5;

/// Default quorum threshold (k=3 for 5 nodes is BFT optimal: 2f+1 where f=1)
pub const DEFAULT_QUORUM_K: usize = 3;

/// Initial block reward (50 BTH in picocredits)
pub const INITIAL_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

/// SCP timebase for testing (faster than production)
pub const SCP_TIMEBASE_MS: u64 = 100;

/// Default maximum values per slot
pub const DEFAULT_MAX_SLOT_VALUES: usize = 50;

/// Minimum ring size for CLSAG signatures (matches production)
pub const TEST_RING_SIZE: usize = 20;

/// Trivial PoW difficulty for fast testing.
///
/// Must match the chain's initial difficulty
/// (`block::difficulty::INITIAL_DIFFICULTY`
/// / `node::minter::INITIAL_DIFFICULTY` = `0x00FF_FFFF_FFFF_FFFF`) — block
/// acceptance now enforces `header.difficulty == chain.difficulty` (audit
/// cycle 6, C1). A trivially-easy difficulty here is fine for tests because
/// 1/256 of nonces solve, but it has to be the *same* trivial value the
/// ledger initializes with.
pub const TRIVIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;
