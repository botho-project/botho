//! Simulation state tracking.

use crate::{ClusterId, ClusterWealth, FeeCurve, TransferConfig};
use std::collections::HashMap;

use super::agent::AgentId;

/// Global simulation state accessible to all agents.
#[derive(Debug)]
pub struct SimulationState {
    /// Current simulation round.
    pub round: u64,

    /// Total rounds to run.
    pub total_rounds: u64,

    /// Global cluster wealth tracking.
    pub cluster_wealth: ClusterWealth,

    /// Fee curve parameters.
    pub fee_curve: FeeCurve,

    /// Transfer configuration.
    pub transfer_config: TransferConfig,

    /// Map of agent ID to their current balance (snapshot).
    pub agent_balances: HashMap<AgentId, u64>,

    /// Map of agent ID to their agent type.
    pub agent_types: HashMap<AgentId, &'static str>,

    /// Total money supply in the system.
    pub total_supply: u64,

    /// Total fees collected (burned) this simulation.
    pub total_fees_collected: u64,

    /// Running count of transactions.
    pub transaction_count: u64,

    /// Available mixer agent IDs.
    pub mixer_ids: Vec<AgentId>,

    /// Current effective fee rates by agent (updated periodically).
    pub fee_rates: HashMap<AgentId, u32>,

    /// Next cluster ID to assign for minting.
    next_cluster_id: u64,
}

impl SimulationState {
    /// Create a new simulation state.
    pub fn new(total_rounds: u64, fee_curve: FeeCurve, transfer_config: TransferConfig) -> Self {
        Self {
            round: 0,
            total_rounds,
            cluster_wealth: ClusterWealth::new(),
            fee_curve,
            transfer_config,
            agent_balances: HashMap::new(),
            agent_types: HashMap::new(),
            total_supply: 0,
            total_fees_collected: 0,
            transaction_count: 0,
            mixer_ids: Vec::new(),
            fee_rates: HashMap::new(),
            next_cluster_id: 0,
        }
    }

    /// Get the next cluster ID for minting.
    pub fn next_cluster_id(&mut self) -> ClusterId {
        let id = ClusterId::new(self.next_cluster_id);
        self.next_cluster_id += 1;
        id
    }

    /// Register a mixer's ID.
    pub fn register_mixer(&mut self, id: AgentId) {
        if !self.mixer_ids.contains(&id) {
            self.mixer_ids.push(id);
        }
    }

    /// Update agent balance snapshot.
    pub fn update_agent_balance(&mut self, id: AgentId, balance: u64) {
        self.agent_balances.insert(id, balance);
    }

    /// Register an agent's type.
    pub fn register_agent(&mut self, id: AgentId, agent_type: &'static str) {
        self.agent_types.insert(id, agent_type);
    }

    /// Record that fees were collected.
    pub fn record_fees(&mut self, fees: u64) {
        self.total_fees_collected += fees;
    }

    /// Record a transaction occurred.
    pub fn record_transaction(&mut self) {
        self.transaction_count += 1;
    }

    /// Update the fee rate for an agent.
    pub fn update_fee_rate(&mut self, id: AgentId, rate_bps: u32) {
        self.fee_rates.insert(id, rate_bps);
    }

    /// Get the average balance of all agents.
    pub fn average_balance(&self) -> u64 {
        if self.agent_balances.is_empty() {
            return 0;
        }
        let total: u64 = self.agent_balances.values().sum();
        total / self.agent_balances.len() as u64
    }

    /// Get the number of registered agents.
    pub fn num_agents(&self) -> usize {
        self.agent_balances.len()
    }

    /// Advance to the next round.
    pub fn advance_round(&mut self) {
        self.round += 1;
    }

    /// Get a random mixer ID if any exist.
    pub fn random_mixer(&self, rng: &mut impl rand::Rng) -> Option<AgentId> {
        if self.mixer_ids.is_empty() {
            None
        } else {
            Some(self.mixer_ids[rng.gen_range(0..self.mixer_ids.len())])
        }
    }

    /// Get agents sorted by balance (descending).
    pub fn agents_by_wealth(&self) -> Vec<(AgentId, u64)> {
        let mut agents: Vec<_> = self.agent_balances.iter().map(|(&k, &v)| (k, v)).collect();
        agents.sort_by(|a, b| b.1.cmp(&a.1));
        agents
    }
}

impl Default for SimulationState {
    fn default() -> Self {
        Self::new(1000, FeeCurve::default(), TransferConfig::default())
    }
}
