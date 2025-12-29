// Copyright (c) 2024 Cadence Foundation

//! Proof-of-Work consensus module for Cadence.
//!
//! This crate implements the PoW mining and difficulty adjustment
//! algorithms for Cadence. It provides:
//!
//! - Difficulty adjustment based on mining transaction rate
//! - Validation of mining transactions
//! - Genesis difficulty configuration

#![deny(missing_docs)]

pub mod difficulty;

pub use difficulty::{
    genesis_difficulty, next_difficulty, next_difficulty_with_timestamps, BlockDifficultyInfo,
    DIFFICULTY_LAG, DIFFICULTY_WINDOW, INITIAL_DIFFICULTY, MAX_ADJUSTMENT_FACTOR, MIN_DIFFICULTY,
    TARGET_MINING_TXS_PER_BLOCK,
};
