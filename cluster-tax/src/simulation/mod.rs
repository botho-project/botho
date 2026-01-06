//! Agent-based economic simulation for cluster taxation validation.
//!
//! This module provides infrastructure for simulating diverse economic actors
//! (whales, merchants, retail users, etc.) interacting under the cluster tax
//! mechanism, allowing empirical validation of parameter choices.
//!
//! ## Privacy Simulation
//!
//! The `privacy` submodule models the effective bits of privacy that users can
//! expect from ring signatures under various adversary models and network
//! conditions.
//!
//! ## Constrained Analysis
//!
//! The `constrained_analysis` submodule extends privacy simulation to analyze
//! the impact of wallet-side tag-based constraints (age similarity, factor ceiling)
//! on ring signature anonymity.
//!
//! ## Lottery Simulation
//!
//! The `lottery` submodule models the lottery-based fee redistribution system
//! as an alternative to cluster-based progressive fees.

mod agent;
pub mod agents;
#[cfg(any(feature = "cli", test))]
pub mod constrained_analysis;
pub mod lottery;
mod metrics;
#[cfg(any(feature = "cli", test))]
pub mod privacy;
mod runner;
mod state;

pub use agent::{Action, Agent, AgentId};
pub use agents::{
    MarketMakerAgent, MerchantAgent, MinterAgent, MixerServiceAgent, RetailUserAgent, WhaleAgent,
    WhaleStrategy,
};
pub use lottery::{
    run_sybil_test, LotteryConfig, LotterySimulation, SybilStrategy, SybilTestResult,
};
pub use metrics::{Metrics, SimulationMetrics};
pub use runner::{run_simulation, RoundSummary, SimulationConfig, SimulationResult};
pub use state::{MonetaryStats, SimulationState};
