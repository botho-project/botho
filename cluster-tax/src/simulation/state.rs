//! Simulation state tracking.

use crate::{ClusterId, ClusterWealth, EmissionConfig, EmissionController, FeeCurve, TransferConfig};
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

    /// Adaptive emission controller for dynamic block rewards.
    pub emission_controller: Option<EmissionController>,

    /// Total block rewards emitted this simulation.
    pub total_rewards_emitted: u64,

    /// Fees burned in current round (for emission tracking).
    pub round_fees_burned: u64,
}

/// Emission statistics snapshot.
#[derive(Debug, Clone)]
pub struct EmissionStats {
    pub current_epoch: u64,
    pub current_block_reward: u64,
    pub total_emitted: u64,
    pub total_fees_burned: u64,
    pub net_supply_change: i64,
    pub effective_inflation_bps: i64,
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
            emission_controller: None,
            total_rewards_emitted: 0,
            round_fees_burned: 0,
        }
    }

    /// Create a new simulation state with adaptive emission.
    pub fn with_emission(
        total_rounds: u64,
        fee_curve: FeeCurve,
        transfer_config: TransferConfig,
        emission_config: EmissionConfig,
        initial_supply: u64,
    ) -> Self {
        let mut state = Self::new(total_rounds, fee_curve, transfer_config);
        state.emission_controller = Some(EmissionController::new(emission_config, initial_supply));
        state.total_supply = initial_supply;
        state
    }

    /// Initialize emission controller after agents are registered.
    pub fn init_emission(&mut self, emission_config: EmissionConfig) {
        self.emission_controller = Some(EmissionController::new(emission_config, self.total_supply));
    }

    /// Get the current block reward from emission controller.
    pub fn current_block_reward(&self) -> u64 {
        self.emission_controller
            .as_ref()
            .map(|ec| ec.block_reward())
            .unwrap_or(0)
    }

    /// Record fees burned (goes to emission controller).
    pub fn record_fee_burn(&mut self, amount: u64) {
        self.total_fees_collected += amount;
        self.round_fees_burned += amount;
        if let Some(ref mut ec) = self.emission_controller {
            ec.record_fee_burn(amount);
        }
    }

    /// Process a block: emits reward and returns the reward amount.
    pub fn process_block(&mut self) -> u64 {
        if let Some(ref mut ec) = self.emission_controller {
            let reward = ec.process_block();
            self.total_rewards_emitted += reward;
            self.total_supply = ec.state.total_supply;
            reward
        } else {
            0
        }
    }

    /// Reset round-specific counters.
    pub fn reset_round_counters(&mut self) {
        self.round_fees_burned = 0;
    }

    /// Get emission statistics if emission is enabled.
    pub fn emission_stats(&self) -> Option<EmissionStats> {
        self.emission_controller.as_ref().map(|ec| EmissionStats {
            current_epoch: ec.state.current_epoch,
            current_block_reward: ec.state.current_block_reward,
            total_emitted: ec.state.total_emitted,
            total_fees_burned: ec.state.total_fees_burned,
            net_supply_change: ec.state.net_supply_change(),
            effective_inflation_bps: ec.state.effective_inflation_bps(&ec.config),
        })
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
