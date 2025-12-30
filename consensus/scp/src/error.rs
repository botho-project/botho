// Copyright (c) 2018-2022 The Botho Foundation

//! Error types for the SCP consensus module.

use thiserror::Error;

/// Errors that can occur in SCP consensus operations.
#[derive(Debug, Error)]
pub enum ScpError {
    /// Invalid ballot state: {0}
    #[error("Invalid ballot state: {0}")]
    InvalidBallot(String),

    /// Quorum not found for the given predicate
    #[error("Quorum not found")]
    QuorumNotFound,

    /// Invalid slot state: {0}
    #[error("Invalid slot state: {0}")]
    InvalidSlotState(String),

    /// Invariant violation in prepare phase: {0}
    #[error("Prepare phase invariant violation: {0}")]
    PrepareInvariantViolation(String),

    /// Invariant violation in commit phase: {0}
    #[error("Commit phase invariant violation: {0}")]
    CommitInvariantViolation(String),

    /// Invariant violation in externalize phase: {0}
    #[error("Externalize phase invariant violation: {0}")]
    ExternalizeInvariantViolation(String),

    /// Serialization error: {0}
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// IO error: {0}
    #[error("IO error: {0}")]
    IoError(String),

    /// Unexpected None value: {0}
    #[error("Unexpected None value: {0}")]
    UnexpectedNone(String),

    /// Arithmetic overflow: {0}
    #[error("Arithmetic overflow: {0}")]
    ArithmeticOverflow(String),

    /// Message validation failed: {0}
    #[error("Message validation failed: {0}")]
    MessageValidation(String),
}

impl From<std::io::Error> for ScpError {
    fn from(err: std::io::Error) -> Self {
        ScpError::IoError(err.to_string())
    }
}

impl From<serde_json::Error> for ScpError {
    fn from(err: serde_json::Error) -> Self {
        ScpError::SerializationError(err.to_string())
    }
}

/// Result type for SCP operations.
pub type ScpResult<T> = Result<T, ScpError>;
