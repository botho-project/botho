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

mod agent;
pub mod agents;
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
pub use metrics::{Metrics, SimulationMetrics};
pub use runner::{run_simulation, RoundSummary, SimulationConfig, SimulationResult};
pub use state::{MonetaryStats, SimulationState};
