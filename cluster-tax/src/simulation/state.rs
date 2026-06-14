//! Simulation state tracking.

use crate::{
    ClusterId, ClusterWealth, DifficultyController, FeeCurve, MonetaryPolicy, TransferConfig,
};
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

    /// Total fees collected (burned) this simulation. Cumulative over the
    /// whole sim; widened to u128 because at full-supply scale cumulative
    /// fees can exceed u64::MAX (~1.84e19 picocredits). See issue #341.
    pub total_fees_collected: u128,

    /// Running count of transactions.
    pub transaction_count: u64,

    /// Available mixer agent IDs.
    pub mixer_ids: Vec<AgentId>,

    /// Current effective fee rates by agent (updated periodically).
    pub fee_rates: HashMap<AgentId, u32>,

    /// Next cluster ID to assign for minting.
    next_cluster_id: u64,

    /// Two-phase monetary policy controller.
    pub monetary_controller: Option<DifficultyController>,

    /// Total block rewards emitted this simulation. Cumulative emission
    /// reaches ~1.22e21 picocredits at Phase-1 scale, well past u64::MAX
    /// (~1.84e19), so this is widened to u128. Per-block rewards stay u64.
    /// See issue #341 / sibling node fix #333.
    pub total_rewards_emitted: u128,

    /// Fees burned in current round (for monetary tracking).
    pub round_fees_burned: u64,

    /// Simulated time in seconds (for difficulty adjustment).
    pub simulated_time: u64,
}

/// Monetary statistics snapshot.
#[derive(Debug, Clone)]
pub struct MonetaryStats {
    pub height: u64,
    pub phase: &'static str,
    pub current_halving: Option<u32>,
    pub blocks_until_halving: Option<u64>,
    pub block_reward: u64,
    pub difficulty: u64,
    pub total_supply: u64,
    /// Cumulative rewards emitted; u128 so it does not narrow the widened
    /// `SimulationState::total_rewards_emitted` at full-supply scale (#341).
    pub total_emitted: u128,
    /// Cumulative fees burned; u128 for the same supply-scale reason (#341).
    pub total_fees_burned: u128,
    pub net_supply_change: i64,
    pub effective_inflation_bps: i64,
    pub estimated_block_time: f64,
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
            monetary_controller: None,
            total_rewards_emitted: 0,
            round_fees_burned: 0,
            simulated_time: 0,
        }
    }

    /// Initialize monetary controller after agents are registered.
    pub fn init_monetary(&mut self, policy: MonetaryPolicy, initial_difficulty: u64) {
        self.monetary_controller = Some(DifficultyController::new(
            policy,
            self.total_supply,
            initial_difficulty,
            0, // Start time
        ));
    }

    /// Get the current block reward from monetary controller.
    pub fn current_block_reward(&self) -> u64 {
        self.monetary_controller
            .as_ref()
            .map(|mc| mc.block_reward())
            .unwrap_or(0)
    }

    /// Get the current difficulty from monetary controller.
    pub fn current_difficulty(&self) -> u64 {
        self.monetary_controller
            .as_ref()
            .map(|mc| mc.state.difficulty)
            .unwrap_or(1)
    }

    /// Get the current monetary phase.
    pub fn monetary_phase(&self) -> &'static str {
        self.monetary_controller
            .as_ref()
            .map(|mc| mc.phase())
            .unwrap_or("None")
    }

    /// Record fees burned (goes to monetary controller).
    pub fn record_fee_burn(&mut self, amount: u64) {
        self.total_fees_collected += amount as u128;
        self.round_fees_burned += amount;
        if let Some(ref mut mc) = self.monetary_controller {
            mc.record_fee_burn(amount);
        }
    }

    /// Process a block: emits reward and returns the reward amount.
    /// `block_time` is the simulated time when this block was mined.
    pub fn process_block(&mut self, block_time: u64) -> u64 {
        if let Some(ref mut mc) = self.monetary_controller {
            let reward = mc.process_block(block_time);
            self.total_rewards_emitted += reward as u128;
            self.total_supply = mc.state.total_supply;
            self.simulated_time = block_time;
            reward
        } else {
            0
        }
    }

    /// Reset round-specific counters.
    pub fn reset_round_counters(&mut self) {
        self.round_fees_burned = 0;
    }

    /// Get monetary statistics if monetary controller is enabled.
    pub fn monetary_stats(&self) -> Option<MonetaryStats> {
        self.monetary_controller.as_ref().map(|mc| {
            let stats = mc.stats(self.simulated_time);
            MonetaryStats {
                height: stats.height,
                phase: stats.phase,
                current_halving: stats.current_halving,
                blocks_until_halving: stats.blocks_until_halving,
                block_reward: stats.block_reward,
                difficulty: stats.difficulty,
                total_supply: stats.total_supply,
                total_emitted: stats.total_rewards_emitted as u128,
                total_fees_burned: stats.total_fees_burned as u128,
                net_supply_change: stats.net_supply_change,
                effective_inflation_bps: stats.effective_inflation_bps,
                estimated_block_time: stats.estimated_block_time,
            }
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
        self.total_fees_collected += fees as u128;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for #341: the cumulative simulation supply accumulators are
    /// u128, so emission/fees past u64::MAX picocredits (~1.84e19) accumulate
    /// exactly with no wrap or panic. At Phase-1 scale (~1.22e21 pico) the old
    /// u64 fields overflowed silently in release and panicked in debug.
    #[test]
    fn total_rewards_emitted_accumulates_past_u64_max() {
        let mut state = SimulationState::default();

        // Pre-seed the cumulative accumulator just below the u64 boundary,
        // then add enough per-block rewards (each a valid u64) to cross it.
        // This mirrors how `process_block` does `+= reward as u128`.
        state.total_rewards_emitted = (u64::MAX - 10) as u128;

        let per_block: u64 = 100; // per-step reward stays u64
        let blocks: u128 = 50;
        for _ in 0..blocks {
            state.total_rewards_emitted += per_block as u128;
        }

        let expected = (u64::MAX - 10) as u128 + per_block as u128 * blocks;
        assert!(
            expected > u64::MAX as u128,
            "test must cross the u64 boundary to be meaningful"
        );
        assert_eq!(
            state.total_rewards_emitted, expected,
            "cumulative emission must accumulate exactly past u64::MAX"
        );
    }

    /// Regression for #341: cumulative `total_fees_collected` (driven via the
    /// public `record_fees` API) must also accumulate exactly past u64::MAX.
    #[test]
    fn total_fees_collected_accumulates_past_u64_max() {
        let mut state = SimulationState::default();

        // Seed near the boundary, then drive the public API past it.
        state.total_fees_collected = (u64::MAX - 5) as u128;

        let per_round_fee: u64 = u64::MAX / 4; // ~4.6e18 each, valid u64
        let rounds: u128 = 3;
        for _ in 0..rounds {
            state.record_fees(per_round_fee);
        }

        let expected = (u64::MAX - 5) as u128 + per_round_fee as u128 * rounds;
        assert!(
            expected > u64::MAX as u128,
            "test must cross the u64 boundary to be meaningful"
        );
        assert_eq!(
            state.total_fees_collected, expected,
            "cumulative fees must accumulate exactly past u64::MAX"
        );
    }
}
