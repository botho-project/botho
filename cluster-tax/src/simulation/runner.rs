//! Simulation execution engine.

use std::collections::HashMap;

use crate::{execute_transfer, mint, ClusterId, ClusterWealth, FeeCurve, TransferConfig};

use super::agent::{Action, Agent, AgentId};
use super::agents::MixerServiceAgent;
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
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            rounds: 1000,
            fee_curve: FeeCurve::default(),
            transfer_config: TransferConfig::default(),
            snapshot_frequency: 100,
            verbose: false,
        }
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
}

/// Summary of a single round.
#[derive(Debug, Clone)]
pub struct RoundSummary {
    pub round: u64,
    pub transactions: u64,
    pub fees_collected: u64,
    pub total_transferred: u64,
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

    // Register all agents
    for agent in agents.iter() {
        state.register_agent(agent.id(), agent.agent_type());
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

        // Collect actions from all agents
        let mut actions: Vec<(AgentId, Action)> = Vec::new();
        for agent in agents.iter_mut() {
            if let Some(action) = agent.decide_action(&state) {
                actions.push((agent.id(), action));
            }
        }

        // Build agent lookup for execution
        let mut agent_map: HashMap<AgentId, usize> = agents
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

        // Update state
        state.total_fees_collected += round_fees;
        state.transaction_count += round_transactions;

        // Update agent balances in state
        for agent in agents.iter() {
            state.update_agent_balance(agent.id(), agent.balance());
        }

        // Record round summary
        if config.verbose {
            round_summaries.push(RoundSummary {
                round,
                transactions: round_transactions,
                fees_collected: round_fees,
                total_transferred: round_transferred,
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

    SimulationResult {
        metrics,
        final_state: state,
        round_summaries,
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
    use crate::simulation::agents::{MerchantAgent, RetailUserAgent};

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
}
