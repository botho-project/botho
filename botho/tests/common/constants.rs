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

/// Trivial PoW difficulty for fast testing
pub const TRIVIAL_DIFFICULTY: u64 = u64::MAX - 1;
