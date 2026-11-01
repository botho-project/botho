//! Simulation execution engine.

use std::collections::HashMap;

use crate::{
    consensus_fee::{ClaimedClass, SimConsensusFee},
    execute_transfer, execute_transfer_with_fee,
    fee_curve::ClusterFactorCurve,
    ClusterWealth, FeeCurve, MonetaryPolicy, TagVector, TransferConfig, TAG_WEIGHT_SCALE,
};

use super::{
    agent::{Action, Agent, AgentId},
    metrics::{snapshot_metrics, Metrics, SimulationMetrics},
    state::SimulationState,
};

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

    /// Initial minting difficulty.
    pub initial_difficulty: u64,

    /// Blocks per round (for monetary simulation).
    /// Default 1 means each round = 1 block.
    pub blocks_per_round: u64,

    /// Simulated seconds per block (for difficulty adjustment).
    /// This should match the policy's target_block_time_secs for stable
    /// simulation.
    pub secs_per_block: u64,

    /// Fee model (#1695). `Some` prices every transfer and wash hop with the
    /// shared consensus fee model (`crate::consensus_fee_model`), converting
    /// sim units to picocredits via its `pico_per_unit`. `None` keeps the
    /// legacy linear-bps `fee_curve` / `transfer_config.fee_curve` pricing.
    pub consensus_fee: Option<SimConsensusFee>,
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
            consensus_fee: None,
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

    /// Exact per-agent fees in picocredits as priced by the consensus fee
    /// model (empty under the legacy fee curve). Balances are charged in sim
    /// units with the sub-unit remainder carried (see `SimConsensusFee`).
    pub agent_fees_pico: HashMap<AgentId, u128>,

    /// Total picocredits priced by the consensus fee model (0 under legacy).
    pub total_fees_pico: u128,
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
    let mut consensus_fee = config.consensus_fee.clone();
    let mut agent_fees_pico: HashMap<AgentId, u128> = HashMap::new();

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
            crate::mint(
                account,
                initial_balance,
                cluster_id,
                &mut state.cluster_wealth,
            );
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

    // Find minter indices for reward distribution
    let minter_indices: Vec<usize> = agents
        .iter()
        .enumerate()
        .filter(|(_, a)| a.agent_type() == "Minter")
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
                                    consensus_fee.as_mut().map(|cf| (cf, round)),
                                    &mut agent_fees_pico,
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
                                        consensus_fee.as_mut().map(|cf| (cf, round)),
                                        &mut agent_fees_pico,
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
                                consensus_fee.as_mut().map(|cf| (cf, round)),
                                &mut agent_fees_pico,
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
                    // Consensus fee model (#1695): each hop is a 1-in/1-out
                    // self-transfer whose tag decay is a class downgrade,
                    // priced by the shared model (capitalized reset charge
                    // once demurrage is active).
                    if let (Some(cf), Some(&sender_idx)) =
                        (consensus_fee.as_mut(), agent_map.get(&sender_id))
                    {
                        let hop_amount = amount / hops.max(1) as u64;
                        let ppu = cf.pico_per_unit;
                        let curve = ClusterFactorCurve::default();
                        let initial_w = claimed_wealth_pico(
                            agents[sender_idx].tags(),
                            &state.cluster_wealth,
                            ppu,
                        );
                        let mut wash_fees_pico: u128 = 0;
                        let mut wash_fees_units = 0u64;
                        for _ in 0..hops {
                            let agent = &mut agents[sender_idx];
                            if agent.balance() < hop_amount {
                                break;
                            }
                            let w_in =
                                claimed_wealth_pico(agent.tags(), &state.cluster_wealth, ppu);
                            agent
                                .account_mut()
                                .tags
                                .apply_decay(config.transfer_config.decay_rate);
                            let w_out =
                                claimed_wealth_pico(agent.tags(), &state.cluster_wealth, ppu);
                            let before = cf.total_fee_pico;
                            let units = cf.charge_transfer(
                                1,
                                1,
                                ClaimedClass::Wealth(w_out),
                                curve.factor(w_in),
                                0,
                                round,
                                hop_amount,
                            );
                            wash_fees_pico += cf.total_fee_pico - before;
                            wash_fees_units += units;
                            round_fees += units;
                            round_transactions += 1;
                        }
                        *agent_fees_pico.entry(sender_id).or_insert(0) += wash_fees_pico;
                        let final_w = claimed_wealth_pico(
                            agents[sender_idx].tags(),
                            &state.cluster_wealth,
                            ppu,
                        );
                        // Savings: the fee difference on one future 1-in/2-out
                        // transfer of `amount` at the old vs new class.
                        let future_fee = |w: u128| {
                            cf.fee_pico(&crate::consensus_fee::ConsensusFeeInputs {
                                size: crate::consensus_fee::consensus_tx_size_bytes(1, 2, 0),
                                n_out: 2,
                                n_memo: 0,
                                claimed: ClaimedClass::Wealth(w),
                                floor: curve.factor(w),
                                elapsed: 0,
                                height: cf.height_offset.saturating_add(round),
                                output_sum: cf.units_to_pico(amount),
                            }) as i128
                        };
                        let savings_pico = future_fee(initial_w) - future_fee(final_w);
                        let net_pico = savings_pico - wash_fees_pico as i128;
                        let net_units = (net_pico / ppu as i128)
                            .clamp(i64::MIN as i128, i64::MAX as i128)
                            as i64;
                        metrics.record_wash_trade(wash_fees_units, net_units);
                        continue;
                    }

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
                            let rate =
                                agent.effective_fee_rate(&state.cluster_wealth, &config.fee_curve);
                            let fee = (hop_amount as u128 * rate as u128 / 10_000) as u64;
                            total_wash_fees += fee;
                            round_fees += fee;
                            round_transactions += 1;

                            // Apply decay to the account's tags
                            agent
                                .account_mut()
                                .tags
                                .apply_decay(config.transfer_config.decay_rate);
                        }

                        let final_rate = agents[sender_idx]
                            .effective_fee_rate(&state.cluster_wealth, &config.fee_curve);

                        // Calculate if it was worth it. i128 intermediate
                        // (#694): pico-scale amounts × bps can exceed
                        // i64::MAX; the divided result fits i64 at sim scale.
                        let rate_reduction = initial_rate as i64 - final_rate as i64;
                        let savings_per_tx = (rate_reduction as i128 * amount as i128 / 10_000)
                            .clamp(i64::MIN as i128, i64::MAX as i128)
                            as i64;
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
            state.total_fees_collected += round_fees as u128;
        }
        state.transaction_count += round_transactions;

        // Process blocks and distribute rewards to minters
        if state.monetary_controller.is_some() && !minter_indices.is_empty() {
            for block_in_round in 0..config.blocks_per_round {
                // Calculate simulated block time
                let block_time =
                    state.simulated_time + (block_in_round + 1) * config.secs_per_block;
                let reward = state.process_block(block_time);
                if reward > 0 {
                    round_rewards += reward;

                    // Distribute reward to a minter (round-robin for simplicity)
                    let minter_idx = minter_indices[round as usize % minter_indices.len()];
                    let cluster_id = state.next_cluster_id();

                    // Mint reward to minter's account
                    let minter_account = agents[minter_idx].account_mut();
                    crate::mint(
                        minter_account,
                        reward,
                        cluster_id,
                        &mut state.cluster_wealth,
                    );
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
                eprintln!(
                    "Round {}: {} txs, {} fees, Gini={:.4}",
                    round,
                    round_transactions,
                    round_fees,
                    metrics
                        .snapshots
                        .last()
                        .map(|m| m.gini_coefficient)
                        .unwrap_or(0.0)
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
        agent_fees_pico,
        total_fees_pico: consensus_fee.map(|cf| cf.total_fee_pico).unwrap_or(0),
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
    consensus_fee: Option<(&mut SimConsensusFee, u64)>,
    agent_fees_pico: &mut HashMap<AgentId, u128>,
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

    let sender_id = sender.id();
    let sender_account = sender.account_mut();
    let receiver_account = receiver.account_mut();

    let result = match consensus_fee {
        // Consensus fee model (#1695): a 1-in/2-out in-class transfer (the
        // sim does not model rings or coin ages, so floor = claimed factor
        // and elapsed = 0: the size/output base, no demurrage).
        Some((cf, round)) => {
            if sender_account.balance < amount {
                return None;
            }
            let w = claimed_wealth_pico(&sender_account.tags, cluster_wealth, cf.pico_per_unit);
            let factor = ClusterFactorCurve::default().factor(w);
            let before = cf.total_fee_pico;
            let fee = cf.charge_transfer(1, 2, ClaimedClass::Wealth(w), factor, 0, round, amount);
            *agent_fees_pico.entry(sender_id).or_insert(0) += cf.total_fee_pico - before;
            execute_transfer_with_fee(
                sender_account,
                receiver_account,
                amount,
                fee,
                config,
                cluster_wealth,
            )
        }
        None => execute_transfer(
            sender_account,
            receiver_account,
            amount,
            config,
            cluster_wealth,
        ),
    };
    match result {
        Ok(result) => Some((result.fee, result.net_amount)),
        Err(_) => None,
    }
}

/// Effective cluster wealth of a tag vector in picocredits: Σ weight × the
/// cluster's (sim-unit) wealth × `pico_per_unit`, the value-normalized form of
/// the ledger's `effective_cluster_wealth_from_outputs`.
fn claimed_wealth_pico(
    tags: &TagVector,
    cluster_wealth: &ClusterWealth,
    pico_per_unit: u128,
) -> u128 {
    tags.iter()
        .map(|(cluster, weight)| {
            (cluster_wealth.get(cluster) as u128)
                .saturating_mul(pico_per_unit)
                .saturating_mul(weight as u128)
                / TAG_WEIGHT_SCALE as u128
        })
        .fold(0u128, |a, b| a.saturating_add(b))
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
    use crate::{
        simulation::agents::{MerchantAgent, MinterAgent, RetailUserAgent},
        MonetaryPolicy,
    };

    /// #1695: with the consensus fee model, transfers are priced in
    /// picocredits by `consensus_fee_model` (size × factor × min(n,10)²),
    /// orders of magnitude below the legacy linear-bps curve, and the exact
    /// pico total is reported.
    #[test]
    fn consensus_fee_model_prices_transfers_in_pico() {
        use crate::consensus_fee::{ConsensusFeeParams, FeeModelMode};
        let run = |consensus: bool| {
            let mut agents: Vec<Box<dyn Agent>> = vec![
                Box::new(RetailUserAgent::new(AgentId(1)).with_merchants(vec![AgentId(2)])),
                Box::new(MerchantAgent::new(AgentId(2))),
            ];
            agents[0].account_mut().balance = 1000;
            agents[1].account_mut().balance = 500;
            let config = SimulationConfig {
                rounds: 200,
                snapshot_frequency: 100,
                consensus_fee: consensus.then(|| {
                    SimConsensusFee::new(
                        ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly),
                        crate::fee_curve::PICO_PER_BTH,
                    )
                }),
                ..Default::default()
            };
            run_simulation(&mut agents, &config)
        };
        let legacy = run(false);
        let consensus = run(true);
        assert!(legacy.final_state.total_fees_collected > 0);
        assert!(consensus.total_fees_pico > 0, "consensus fees are priced");
        // 1 unit = 1 BTH: thousands of pico per tx never reach one unit.
        assert_eq!(consensus.final_state.total_fees_collected, 0);
        let per_tx = consensus.total_fees_pico / consensus.final_state.transaction_count as u128;
        // Background 1-in/2-out: 990 B × 1-6× × 4.
        assert!((3_960..=6 * 3_960).contains(&per_tx), "per-tx {per_tx}");
        assert_eq!(
            consensus.agent_fees_pico.values().sum::<u128>(),
            consensus.total_fees_pico
        );
    }

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
        // Create a simple economy with a minter (who doesn't sell) and a merchant
        // Using a minter with no buyers so they just accumulate rewards
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinterAgent::new(AgentId(1))), // No buyers - won't sell
            Box::new(MerchantAgent::new(AgentId(2))),
            Box::new(RetailUserAgent::new(AgentId(3)).with_merchants(vec![AgentId(2)])),
        ];

        // Initial balances
        agents[0].account_mut().balance = 10_000; // Minter
        agents[1].account_mut().balance = 5_000; // Merchant
        agents[2].account_mut().balance = 5_000; // Retail

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

        // Minter should have received all rewards (no selling)
        let minter_balance = *result.final_state.agent_balances.get(&AgentId(1)).unwrap();
        // total_emitted is u128 (#341); at this test's scale it fits in u64.
        let expected_balance = 10_000 + stats.total_emitted as u64;
        assert_eq!(
            minter_balance, expected_balance,
            "Minter should have initial 10_000 + {} rewards = {}, got: {}",
            stats.total_emitted, expected_balance, minter_balance
        );
    }

    #[test]
    fn test_fee_burn_affects_difficulty() {
        // Test that fee burns cause difficulty adjustments in tail phase
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinterAgent::new(AgentId(1)).with_buyers(vec![AgentId(2)])),
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
        eprintln!("Net supply change: {}", stats.net_supply_change);
        eprintln!("Difficulty: {}", stats.difficulty);
    }

    #[test]
    fn test_monetary_without_minters() {
        // Test graceful behavior when no minters exist
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

        // Blocks should still be processed even without minters
        assert!(
            stats.total_emitted == 0,
            "No rewards should be emitted without minters"
        );
    }

    #[test]
    fn test_round_summary_with_monetary() {
        let mut agents: Vec<Box<dyn Agent>> = vec![
            Box::new(MinterAgent::new(AgentId(1))),
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
            Box::new(MinterAgent::new(AgentId(1))),
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
            Box::new(MinterAgent::new(AgentId(1))),
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

        // Node height convention (#1699): genesis (height 0) mints nothing, so
        // the 25 simulated blocks are heights 1-25: heights 1-9 = 100,
        // 10-19 = 50, 20-25 = 25.
        // Total = 9*100 + 10*50 + 6*25 = 900 + 500 + 150 = 1550
        assert_eq!(stats.total_emitted, 1550);
        assert_eq!(stats.block_reward, 25); // After 2 complete halvings: 100 -> 50 -> 25
        assert_eq!(stats.phase, "Halving"); // Still in halving phase
    }
}
