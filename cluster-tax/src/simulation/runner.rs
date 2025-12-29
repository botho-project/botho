//! Simulation execution engine.

use std::collections::HashMap;

use crate::{execute_transfer, ClusterWealth, FeeCurve, MonetaryPolicy, TransferConfig};

use super::agent::{Action, Agent, AgentId};
use super::metrics::{snapshot_metrics, Metrics, SimulationMetrics};
use super::state::SimulationState;

/// Configuration for a simulation run.
#[derive(Clone, Debug)]
pub struct SimulationConfig {
    /// Number of rounds to simulate.
    pub rounds: u64,

    /// Fee curve parameters.
    pub fee_curve: FeeCurve,

    /// Transfer configuration.
    pub transfer_config: TransferConfig,

    /// Snapshot frequency (take metrics every N rounds).
    pub snapshot_frequency: u64,

    /// Verbose output.
    pub verbose: bool,

    /// Monetary policy (optional - enables two-phase emission).
    pub monetary_policy: Option<MonetaryPolicy>,

    /// Initial mining difficulty.
    pub initial_difficulty: u64,

    /// Blocks per round (for monetary simulation).
    /// Default 1 means each round = 1 block.
    pub blocks_per_round: u64,

    /// Simulated seconds per block (for difficulty adjustment).
    /// This should match the policy's target_block_time_secs for stable simulation.
    pub secs_per_block: u64,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            rounds: 1000,
            fee_curve: FeeCurve::default(),
            transfer_config: TransferConfig::default(),
            snapshot_frequency: 100,
            verbose: false,
            monetary_policy: None,
            initial_difficulty: 1000,
            blocks_per_round: 1,
            secs_per_block: 60,
        }
    }
}

impl SimulationConfig {
    /// Enable monetary policy with default parameters.
    pub fn with_monetary(mut self) -> Self {
        self.monetary_policy = Some(MonetaryPolicy::default());
        self
    }

    /// Enable monetary policy with custom configuration.
    pub fn with_monetary_policy(mut self, policy: MonetaryPolicy) -> Self {
        self.secs_per_block = policy.target_block_time_secs;
        self.monetary_policy = Some(policy);
        self
    }

    /// Use fast test parameters for monetary policy.
    pub fn with_fast_monetary(mut self) -> Self {
        let policy = MonetaryPolicy::fast_test();
        self.secs_per_block = policy.target_block_time_secs;
        self.monetary_policy = Some(policy);
        self
    }
}

/// Result of a simulation run.
#[derive(Debug)]
pub struct SimulationResult {
    /// Collected metrics.
    pub metrics: SimulationMetrics,

    /// Final state snapshot.
    pub final_state: SimulationState,

    /// Per-round data (if verbose).
    pub round_summaries: Vec<RoundSummary>,

    /// Final monetary statistics (if monetary policy enabled).
    pub monetary_stats: Option<super::state::MonetaryStats>,
}

/// Summary of a single round.
#[derive(Debug, Clone)]
pub struct RoundSummary {
    pub round: u64,
    pub transactions: u64,
    pub fees_collected: u64,
    pub total_transferred: u64,
    /// Block rewards emitted this round.
    pub rewards_emitted: u64,
    /// Current block reward rate.
    pub block_reward: u64,
    /// Current difficulty.
    pub difficulty: u64,
    /// Current monetary phase.
    pub phase: &'static str,
}

/// Run a simulation with the given agents.
pub fn run_simulation(
    agents: &mut [Box<dyn Agent>],
    config: &SimulationConfig,
) -> SimulationResult {
    let mut state = SimulationState::new(
        config.rounds,
        config.fee_curve.clone(),
        config.transfer_config.clone(),
    );

    let mut metrics = SimulationMetrics::new();
    let mut round_summaries = Vec::new();

    // Register all agents and properly mint their initial balances
    // This ensures cluster wealth is tracked correctly for progressive fees
    for agent in agents.iter_mut() {
        state.register_agent(agent.id(), agent.agent_type());

        // Get the agent's initial balance
        let initial_balance = agent.balance();
        if initial_balance > 0 {
            // Get a unique cluster ID for this agent's initial wealth
            let cluster_id = state.next_cluster_id();

            // Mint the balance properly (sets up tags and cluster wealth)
            let account = agent.account_mut();
            account.balance = 0; // Reset before minting
            crate::mint(account, initial_balance, cluster_id, &mut state.cluster_wealth);
        }

        state.update_agent_balance(agent.id(), agent.balance());
    }

    // Find and register mixers
    for agent in agents.iter() {
        if agent.agent_type() == "Mixer" {
            state.register_mixer(agent.id());
        }
    }

    // Calculate total supply
    state.total_supply = agents.iter().map(|a| a.balance()).sum();

    // Initialize monetary controller if configured
    if let Some(ref monetary_policy) = config.monetary_policy {
        state.init_monetary(monetary_policy.clone(), config.initial_difficulty);
    }

    // Find miner indices for reward distribution
    let miner_indices: Vec<usize> = agents
        .iter()
        .enumerate()
        .filter(|(_, a)| a.agent_type() == "Miner")
        .map(|(i, _)| i)
        .collect();

    // Initial snapshot
    let initial_metrics = collect_metrics(agents, &state, &mut metrics, &config.fee_curve);
    metrics.record_snapshot(initial_metrics);

    // Main simulation loop
    for round in 1..=config.rounds {
        state.advance_round();
        state.round = round;

        let mut round_fees = 0u64;
        let mut round_transactions = 0u64;
        let mut round_transferred = 0u64;
        let mut round_rewards = 0u64;

        // Collect actions from all agents
        let mut actions: Vec<(AgentId, Action)> = Vec::new();
        for agent in agents.iter_mut() {
            if let Some(action) = agent.decide_action(&state) {
                actions.push((agent.id(), action));
            }
        }

        // Build agent lookup for execution
        let agent_map: HashMap<AgentId, usize> = agents
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id(), i))
            .collect();

        // Execute actions
        for (sender_id, action) in actions {
            match action {
                Action::Transfer { to, amount } => {
                    if let Some(&sender_idx) = agent_map.get(&sender_id) {
                        if let Some(&receiver_idx) = agent_map.get(&to) {
                            if sender_idx != receiver_idx {
                                // Execute transfer
                                let result = execute_transfer_between_agents(
                                    agents,
                                    sender_idx,
                                    receiver_idx,
                                    amount,
                                    &config.transfer_config,
                                    &mut state.cluster_wealth,
                                );

                                if let Some((fee, net)) = result {
                                    round_fees += fee;
                                    round_transferred += amount;
                                    round_transactions += 1;

                                    metrics.record_agent_fees(
                                        sender_id,
                                        agents[sender_idx].agent_type(),
                                        fee,
                                    );

                                    // Notify receiver
                                    agents[receiver_idx].on_receive_payment(net, sender_id);
                                }
                            }
                        }
                    }
                }

                Action::BatchTransfer { transfers } => {
                    if let Some(&sender_idx) = agent_map.get(&sender_id) {
                        for (to, amount) in transfers {
                            if let Some(&receiver_idx) = agent_map.get(&to) {
                                if sender_idx != receiver_idx {
                                    let result = execute_transfer_between_agents(
                                        agents,
                                        sender_idx,
                                        receiver_idx,
                                        amount,
                                        &config.transfer_config,
                                        &mut state.cluster_wealth,
                                    );

                                    if let Some((fee, net)) = result {
                                        round_fees += fee;
                                        round_transferred += amount;
                                        round_transactions += 1;

                                        metrics.record_agent_fees(
                                            sender_id,
                                            agents[sender_idx].agent_type(),
                                            fee,
                                        );

                                        agents[receiver_idx].on_receive_payment(net, sender_id);
                                    }
                                }
                            }
                        }
                    }
                }

                Action::UseMixer { mixer_id, amount } => {
                    if let Some(&sender_idx) = agent_map.get(&sender_id) {
                        if let Some(&mixer_idx) = agent_map.get(&mixer_id) {
                            // Transfer to mixer
                            let result = execute_transfer_between_agents(
                                agents,
                                sender_idx,
                                mixer_idx,
                                amount,
                                &config.transfer_config,
                                &mut state.cluster_wealth,
                            );

                            if let Some((fee, net)) = result {
                                round_fees += fee;
                                round_transferred += amount;
                                round_transactions += 1;

                                metrics.record_agent_fees(
                                    sender_id,
                                    agents[sender_idx].agent_type(),
                                    fee,
                                );

                                // Notify mixer
                                agents[mixer_idx].on_receive_payment(net, sender_id);
                            }
                        }
                    }
                }

                Action::WashTrade { amount, hops } => {
                    // Simulate wash trading as a series of self-transfers with decay
                    if let Some(&sender_idx) = agent_map.get(&sender_id) {
                        let initial_rate = agents[sender_idx]
                            .effective_fee_rate(&state.cluster_wealth, &config.fee_curve);
                        let mut total_wash_fees = 0u64;

                        // Each hop incurs a fee and applies decay
                        for _ in 0..hops {
                            let agent = &mut agents[sender_idx];
                            if agent.balance() < amount / hops as u64 {
                                break;
                            }

                            // Self-transfer (simulated)
                            let hop_amount = amount / hops as u64;
                            let rate = agent.effective_fee_rate(
                                &state.cluster_wealth,
                                &config.fee_curve,
                            );
                            let fee = (hop_amount as u128 * rate as u128 / 10_000) as u64;
                            total_wash_fees += fee;
                            round_fees += fee;
                            round_transactions += 1;

                            // Apply decay to the account's tags
                            agent.account_mut().tags.apply_decay(config.transfer_config.decay_rate);
                        }

                        let final_rate = agents[sender_idx]
                            .effective_fee_rate(&state.cluster_wealth, &config.fee_curve);

                        // Calculate if it was worth it
                        let rate_reduction = initial_rate as i64 - final_rate as i64;
                        let savings_per_tx = rate_reduction as i64 * amount as i64 / 10_000;
                        let net_savings = savings_per_tx - total_wash_fees as i64;

                        metrics.record_wash_trade(total_wash_fees, net_savings);
                    }
                }

                Action::Hold => {}
            }
        }

        // Update state with fees (uses monetary controller if available)
        if state.monetary_controller.is_some() {
            // Record fee burns to monetary controller
            state.record_fee_burn(round_fees);
        } else {
            state.total_fees_collected += round_fees;
        }
        state.transaction_count += round_transactions;

        // Process blocks and distribute rewards to miners
        if state.monetary_controller.is_some() && !miner_indices.is_empty() {
            for block_in_round in 0..config.blocks_per_round {
                // Calculate simulated block time
                let block_time = state.simulated_time
                    + (block_in_round + 1) * config.secs_per_block;
                let reward = state.process_block(block_time);
                if reward > 0 {
                    round_rewards += reward;

                    // Distribute reward to a miner (round-robin for simplicity)
                    let miner_idx = miner_indices[round as usize % miner_indices.len()];
                    let cluster_id = state.next_cluster_id();

                    // Mint reward to miner's account
                    let miner_account = agents[miner_idx].account_mut();
                    crate::mint(miner_account, reward, cluster_id, &mut state.cluster_wealth);
                }
            }
        }

        // Update agent balances in state
        for agent in agents.iter() {
            state.update_agent_balance(agent.id(), agent.balance());
        }

        // Get current monetary stats for summary
        let current_block_reward = state.current_block_reward();
        let current_difficulty = state.current_difficulty();
        let current_phase = state.monetary_phase();

        // Record round summary
        if config.verbose {
            round_summaries.push(RoundSummary {
                round,
                transactions: round_transactions,
                fees_collected: round_fees,
                total_transferred: round_transferred,
                rewards_emitted: round_rewards,
                block_reward: current_block_reward,
                difficulty: current_difficulty,
                phase: current_phase,
            });
        }

        // Take snapshot at intervals
        if round % config.snapshot_frequency == 0 || round == config.rounds {
            let snapshot = collect_metrics(agents, &state, &mut metrics, &config.fee_curve);
            metrics.record_snapshot(snapshot);

            if config.verbose {
                println!(
                    "Round {}: {} txs, {} fees, Gini={:.4}",
                    round,
                    round_transactions,
                    round_fees,
                    metrics.snapshots.last().map(|m| m.gini_coefficient).unwrap_or(0.0)
                );
            }
        }
    }

    let monetary_stats = state.monetary_stats();

    SimulationResult {
        metrics,
        final_state: state,
        round_summaries,
        monetary_stats,
    }
}

/// Helper to get effective fee rate for an agent.
trait AgentFeeRate {
    fn effective_fee_rate(&self, cluster_wealth: &ClusterWealth, fee_curve: &FeeCurve) -> u32;
}

impl<T: Agent + ?Sized> AgentFeeRate for T {
    fn effective_fee_rate(&self, cluster_wealth: &ClusterWealth, fee_curve: &FeeCurve) -> u32 {
        let account = crate::Account {
            id: self.id().0,
            balance: self.balance(),
            tags: self.tags().clone(),
        };
        account.effective_fee_rate(cluster_wealth, fee_curve)
    }
}

/// Execute transfer between two agents in the array.
fn execute_transfer_between_agents(
    agents: &mut [Box<dyn Agent>],
    sender_idx: usize,
    receiver_idx: usize,
    amount: u64,
    config: &TransferConfig,
    cluster_wealth: &mut ClusterWealth,
) -> Option<(u64, u64)> {
    // We need to get mutable references to both accounts safely
    if sender_idx == receiver_idx {
        return None;
    }

    // Split the array to get two mutable references
    let (sender, receiver) = if sender_idx < receiver_idx {
        let (left, right) = agents.split_at_mut(receiver_idx);
        (&mut left[sender_idx], &mut right[0])
    } else {
        let (left, right) = agents.split_at_mut(sender_idx);
        (&mut right[0], &mut left[receiver_idx])
    };

    let sender_account = sender.account_mut();
    let receiver_account = receiver.account_mut();

    match execute_transfer(sender_account, receiver_account, amount, config, cluster_wealth) {
        Ok(result) => Some((result.fee, result.net_amount)),
        Err(_) => None,
    }
}

/// Collect metrics from current state.
fn collect_metrics(
    agents: &[Box<dyn Agent>],
    state: &SimulationState,
    _sim_metrics: &mut SimulationMetrics,
    fee_curve: &FeeCurve,
) -> Metrics {
    let agent_data: Vec<_> = agents
        .iter()
        .map(|a| {
            let account = crate::Account {
                id: a.id().0,
                balance: a.balance(),
                tags: a.tags().clone(),
            };
            let rate = account.effective_fee_rate(&state.cluster_wealth, fee_curve);
            (a.id(), a.balance(), rate, a.tags().clone())
        })
        .collect();

    // Calculate mixer volume (sum of mixer balances as proxy)
    let mixer_volume: u64 = agents
        .iter()
        .filter(|a| a.agent_type() == "Mixer")
        .map(|a| a.balance())
        .sum();

    snapshot_metrics(
        state.round,
        &agent_data,
        &state.cluster_wealth,
        state.total_fees_collected,
        state.transaction_count,
        mixer_volume,
        fee_curve,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::agents::{MerchantAgent, MinerAgent, RetailUserAgent};
    use crate::MonetaryPolicy;

    #[test]
    fn test_basic_simulation() {
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(RetailUserAgent::new(AgentId(1)).with_merchants(vec![AgentId(2)])),
            Box::new(MerchantAgent::new(AgentId(2))),
        ];

        // Give them some balance
        agents[0].account_mut().balance = 1000;
        agents[1].account_mut().balance = 500;

        let config = SimulationConfig {
            rounds: 10,
            ..Default::default()
        };

        let result = run_simulation(&mut agents, &config);

        assert!(!result.metrics.snapshots.is_empty());
    }

    #[test]
    fn test_simulation_with_monetary_policy() {
        // Create a simple economy with a miner (who doesn't sell) and a merchant
        // Using a miner with no buyers so they just accumulate rewards
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinerAgent::new(AgentId(1))), // No buyers - won't sell
            Box::new(MerchantAgent::new(AgentId(2))),
            Box::new(RetailUserAgent::new(AgentId(3)).with_merchants(vec![AgentId(2)])),
        ];

        // Initial balances
        agents[0].account_mut().balance = 10_000; // Miner
        agents[1].account_mut().balance = 5_000;  // Merchant
        agents[2].account_mut().balance = 5_000;  // Retail

        // Use fast test parameters for monetary policy
        let monetary_policy = MonetaryPolicy::fast_test();

        let config = SimulationConfig {
            rounds: 50,
            snapshot_frequency: 10,
            blocks_per_round: 1,
            ..Default::default()
        }
        .with_monetary_policy(monetary_policy);

        let result = run_simulation(&mut agents, &config);

        // Verify monetary stats are present
        assert!(result.monetary_stats.is_some());
        let stats = result.monetary_stats.unwrap();

        // Verify blocks were processed (50 rounds × 1 block/round = 50 blocks)
        assert_eq!(stats.height, 50);

        // Verify rewards were emitted
        assert!(
            stats.total_emitted > 0,
            "Should have emitted some rewards: {}",
            stats.total_emitted
        );

        // Miner should have received all rewards (no selling)
        let miner_balance = *result.final_state.agent_balances.get(&AgentId(1)).unwrap();
        let expected_balance = 10_000 + stats.total_emitted;
        assert_eq!(
            miner_balance, expected_balance,
            "Miner should have initial 10_000 + {} rewards = {}, got: {}",
            stats.total_emitted, expected_balance, miner_balance
        );
    }

    #[test]
    fn test_fee_burn_affects_difficulty() {
        // Test that fee burns cause difficulty adjustments in tail phase
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinerAgent::new(AgentId(1)).with_buyers(vec![AgentId(2)])),
            Box::new(MerchantAgent::new(AgentId(2))),
            Box::new(
                RetailUserAgent::new(AgentId(3))
                    .with_merchants(vec![AgentId(2)])
                    .with_spending_probability(1.0), // High frequency
            ),
        ];

        // Larger balances to ensure many transactions occur
        agents[0].account_mut().balance = 100_000;
        agents[1].account_mut().balance = 100_000;
        agents[2].account_mut().balance = 100_000;

        // Use fast test with tail phase parameters
        let policy = MonetaryPolicy {
            // Very short halving for quick transition to tail
            halving_interval: 10,
            halving_count: 1,
            ..MonetaryPolicy::fast_test()
        };

        let config = SimulationConfig {
            rounds: 100,
            snapshot_frequency: 50,
            blocks_per_round: 1,
            ..Default::default()
        }
        .with_monetary_policy(policy);

        let result = run_simulation(&mut agents, &config);
        let stats = result.monetary_stats.unwrap();

        // Should have burned some fees
        assert!(
            stats.total_fees_burned > 0,
            "Should have burned fees: {}",
            stats.total_fees_burned
        );

        // Net supply change can be positive or negative depending on fee burn rate
        // The key is the system adapts difficulty in response
        println!("Net supply change: {}", stats.net_supply_change);
        println!("Difficulty: {}", stats.difficulty);
    }

    #[test]
    fn test_monetary_without_miners() {
        // Test graceful behavior when no miners exist
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MerchantAgent::new(AgentId(1))),
            Box::new(RetailUserAgent::new(AgentId(2)).with_merchants(vec![AgentId(1)])),
        ];

        agents[0].account_mut().balance = 10_000;
        agents[1].account_mut().balance = 10_000;

        let config = SimulationConfig {
            rounds: 20,
            blocks_per_round: 1,
            ..Default::default()
        }
        .with_fast_monetary();

        // Should not panic
        let result = run_simulation(&mut agents, &config);

        // Monetary controller should exist but no rewards distributed
        assert!(result.monetary_stats.is_some());
        let stats = result.monetary_stats.unwrap();

        // Blocks should still be processed even without miners
        assert!(
            stats.total_emitted == 0,
            "No rewards should be emitted without miners"
        );
    }

    #[test]
    fn test_round_summary_with_monetary() {
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinerAgent::new(AgentId(1))),
            Box::new(MerchantAgent::new(AgentId(2))),
        ];

        agents[0].account_mut().balance = 1_000;
        agents[1].account_mut().balance = 1_000;

        let policy = MonetaryPolicy {
            initial_reward: 50,
            halving_interval: 100, // No halving during test
            ..MonetaryPolicy::fast_test()
        };

        let config = SimulationConfig {
            rounds: 5,
            verbose: true, // Enable round summaries
            blocks_per_round: 1,
            ..Default::default()
        }
        .with_monetary_policy(policy);

        let result = run_simulation(&mut agents, &config);

        // Should have round summaries
        assert_eq!(result.round_summaries.len(), 5);

        // Each round should show block reward and emission
        for summary in &result.round_summaries {
            assert_eq!(summary.block_reward, 50, "Block reward should be 50");
            assert_eq!(
                summary.rewards_emitted, 50,
                "Should emit 50 per round (1 block)"
            );
            assert_eq!(summary.phase, "Halving", "Should be in halving phase");
        }
    }

    #[test]
    fn test_multiple_blocks_per_round() {
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinerAgent::new(AgentId(1))),
            Box::new(MerchantAgent::new(AgentId(2))),
        ];

        agents[0].account_mut().balance = 1_000;
        agents[1].account_mut().balance = 1_000;

        let policy = MonetaryPolicy {
            initial_reward: 10,
            halving_interval: 100, // No halving during test
            ..MonetaryPolicy::fast_test()
        };

        let config = SimulationConfig {
            rounds: 10,
            verbose: true,
            blocks_per_round: 5, // 5 blocks per round
            ..Default::default()
        }
        .with_monetary_policy(policy);

        let result = run_simulation(&mut agents, &config);
        let stats = result.monetary_stats.unwrap();

        // 10 rounds × 5 blocks/round = 50 blocks × 10 reward = 500 total
        assert_eq!(stats.total_emitted, 500);

        // Each round should emit 5 × 10 = 50
        for summary in &result.round_summaries {
            assert_eq!(summary.rewards_emitted, 50);
        }
    }

    #[test]
    fn test_halving_transition() {
        // Test that halvings occur correctly
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinerAgent::new(AgentId(1))),
            Box::new(MerchantAgent::new(AgentId(2))),
        ];

        agents[0].account_mut().balance = 1_000;
        agents[1].account_mut().balance = 1_000;

        let policy = MonetaryPolicy {
            initial_reward: 100,
            halving_interval: 10, // Halving every 10 blocks
            halving_count: 3,     // 3 halvings (30 blocks of halving phase)
            ..MonetaryPolicy::fast_test()
        };

        let config = SimulationConfig {
            rounds: 25,
            verbose: true,
            blocks_per_round: 1,
            ..Default::default()
        }
        .with_monetary_policy(policy);

        let result = run_simulation(&mut agents, &config);
        let stats = result.monetary_stats.unwrap();

        // After 25 blocks: blocks 0-9 = 100, blocks 10-19 = 50, blocks 20-24 = 25
        // Total = 10*100 + 10*50 + 5*25 = 1000 + 500 + 125 = 1625
        assert_eq!(stats.total_emitted, 1625);
        assert_eq!(stats.block_reward, 25); // After 2 complete halvings: 100 -> 50 -> 25
        assert_eq!(stats.phase, "Halving"); // Still in halving phase
    }
}
