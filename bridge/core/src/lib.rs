// Copyright (c) 2024 The Botho Foundation

//! Core types and logic for the BTH bridge.
//!
//! This crate provides the domain types for bridging BTH to wrapped tokens
//! on Ethereum and Solana, including:
//!
//! - Bridge orders and their state machine
//! - Chain-specific types
//! - Configuration structures
//! - Rate limiting logic

pub mod chains;
pub mod config;
pub mod order;

pub use chains::{Chain, ChainAddress};
pub use config::{BridgeConfig, BthConfig, EthereumConfig, SolanaConfig};
pub use order::{BridgeOrder, OrderStatus, OrderType};
