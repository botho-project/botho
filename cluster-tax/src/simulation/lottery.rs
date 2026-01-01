//! Lottery-based fee redistribution simulation.
//!
//! This module implements the lottery redistribution mechanism as an alternative
//! to cluster-based progressive fees. Instead of charging higher fees to wealthy
//! clusters, we redistribute fees to UTXO holders weighted by:
//!
//! 1. **Value / cluster_factor**: Progressive (low factor = more tickets/BTH)
//! 2. **Ring participation**: Rewards active anonymity set contributors
//!
//! Both components are value-weighted to prevent Sybil attacks.

use std::collections::HashMap;

use rand::Rng;

use crate::{ClusterId, ClusterWealth, FeeCurve};

/// Transaction frequency model for simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionModel {
    /// Value-weighted: probability proportional to UTXO value (rich transact more)
    ValueWeighted,
    /// Uniform: equal probability per UTXO (everyone transacts equally)
    Uniform,
}

/// Lottery ticket model - how tickets are earned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketModel {
    /// Original: tickets = value/factor × activity_multiplier
    /// Gameable through wash trading (activity multiplier farms)
    ActivityBased,
    /// Option C: tickets = fee_paid × (max_factor - your_factor) / max_factor
    /// Wash-trading resistant (more washes = more fees = benefits others)
    FeeProportional,
}

/// Maximum cluster factor for fee-proportional ticket calculation.
const MAX_CLUSTER_FACTOR: f64 = 6.0;

/// Configuration for the lottery system.
#[derive(Clone, Debug)]
pub struct LotteryConfig {
    /// Fraction of fees that go to lottery pool (remainder burned).
    pub pool_fraction: f64,

    /// Blocks between lottery drawings.
    pub drawing_interval: u64,

    /// Minimum UTXO age to participate (blocks).
    pub min_utxo_age: u64,

    /// Minimum UTXO value to participate (base units).
    pub min_utxo_value: u64,

    /// Lookback window for ring participation (blocks).
    pub activity_lookback: u64,

    /// Base fee per transaction (for Sybil cost analysis).
    pub base_fee: u64,

    /// Ticket model: how lottery tickets are earned.
    pub ticket_model: TicketModel,
}

impl Default for LotteryConfig {
    fn default() -> Self {
        Self {
            pool_fraction: 0.8,
            drawing_interval: 100,
            min_utxo_age: 720,
            min_utxo_value: 1_000,
            activity_lookback: 259_200, // ~30 days at 10s blocks
            base_fee: 1_000,
            ticket_model: TicketModel::ActivityBased,
        }
    }
}

/// A UTXO in the lottery system.
#[derive(Clone, Debug)]
pub struct LotteryUtxo {
    pub id: u64,
    pub owner_id: u64,
    pub value: u64,
    pub cluster_factor: f64,
    pub creation_block: u64,
    /// Accumulated activity contribution (value × selections / ring_size).
    pub activity_contribution: f64,
    /// Number of times selected as ring member.
    pub selection_count: u32,
    /// Accumulated tickets from fees paid (fee-proportional model).
    pub tickets_from_fees: f64,
}

impl LotteryUtxo {
    /// Create a new UTXO.
    pub fn new(id: u64, owner_id: u64, value: u64, cluster_factor: f64, block: u64) -> Self {
        Self {
            id,
            owner_id,
            value,
            cluster_factor,
            creation_block: block,
            activity_contribution: 0.0,
            selection_count: 0,
            tickets_from_fees: 0.0,
        }
    }

    /// Calculate base lottery tickets (value-weighted, cluster-adjusted).
    /// Used in ActivityBased model.
    pub fn base_tickets(&self) -> f64 {
        self.value as f64 / self.cluster_factor
    }

    /// Calculate activity multiplier (value-weighted).
    /// Used in ActivityBased model.
    pub fn activity_multiplier(&self) -> f64 {
        if self.value == 0 {
            return 1.0;
        }
        let ratio = self.activity_contribution / self.value as f64;
        1.0 + (1.0 + ratio).log2()
    }

    /// Calculate total lottery tickets for ActivityBased model.
    pub fn activity_tickets(&self) -> f64 {
        self.base_tickets() * self.activity_multiplier()
    }

    /// Calculate tickets based on the specified model.
    pub fn tickets_for_model(&self, model: TicketModel) -> f64 {
        match model {
            TicketModel::ActivityBased => self.activity_tickets(),
            TicketModel::FeeProportional => self.tickets_from_fees,
        }
    }

    /// Check if eligible for lottery.
    pub fn is_eligible(&self, current_block: u64, config: &LotteryConfig) -> bool {
        let age = current_block.saturating_sub(self.creation_block);
        age >= config.min_utxo_age && self.value >= config.min_utxo_value
    }

    /// Record ring participation.
    pub fn record_ring_participation(&mut self, ring_size: u32) {
        self.selection_count += 1;
        self.activity_contribution += self.value as f64 / ring_size as f64;
    }

    /// Record fee payment and calculate fee-proportional tickets.
    /// tickets = fee × (max_factor - your_factor) / max_factor
    /// Poor (factor 1.0) get ~0.83 tickets per fee unit
    /// Rich (factor 5.5) get ~0.08 tickets per fee unit
    pub fn record_fee_payment(&mut self, fee: u64) {
        let ticket_rate = (MAX_CLUSTER_FACTOR - self.cluster_factor) / MAX_CLUSTER_FACTOR;
        self.tickets_from_fees += fee as f64 * ticket_rate.max(0.0);
    }
}

/// An owner's complete holdings.
#[derive(Clone, Debug)]
pub struct LotteryOwner {
    pub id: u64,
    pub utxo_ids: Vec<u64>,
    pub strategy: SybilStrategy,
    /// Total fees paid.
    pub total_fees_paid: u64,
    /// Total lottery winnings.
    pub total_winnings: u64,
    /// Transactions made.
    pub tx_count: u64,
}

impl LotteryOwner {
    pub fn new(id: u64, strategy: SybilStrategy) -> Self {
        Self {
            id,
            utxo_ids: Vec::new(),
            strategy,
            total_fees_paid: 0,
            total_winnings: 0,
            tx_count: 0,
        }
    }
}

/// Sybil attack strategy for testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SybilStrategy {
    /// Normal behavior: single account, consolidate UTXOs.
    Normal,
    /// Split into N accounts.
    MultiAccount { num_accounts: u32 },
    /// Aggressive splitting: maximize UTXO count.
    MaximizeSplit,
}

/// Lottery simulation state.
pub struct LotterySimulation {
    pub config: LotteryConfig,
    pub fee_curve: FeeCurve,
    pub current_block: u64,

    /// All UTXOs.
    pub utxos: HashMap<u64, LotteryUtxo>,
    next_utxo_id: u64,

    /// All owners.
    pub owners: HashMap<u64, LotteryOwner>,

    /// Cluster wealth tracking.
    pub cluster_wealth: ClusterWealth,
    next_cluster_id: u64,

    /// Lottery pool.
    pub lottery_pool: u64,

    /// Total burned.
    pub total_burned: u64,

    /// Ring size for simulated transactions.
    pub ring_size: u32,

    /// Metrics.
    pub metrics: LotteryMetrics,
}

/// Metrics tracked during simulation.
#[derive(Clone, Debug, Default)]
pub struct LotteryMetrics {
    pub drawings_held: u64,
    pub total_distributed: u64,
    pub total_fees_collected: u64,

    /// Gini snapshots: (block, gini).
    pub gini_snapshots: Vec<(u64, f64)>,

    /// Tickets by wealth quintile.
    pub tickets_by_quintile: [f64; 5],

    /// Winnings by wealth quintile.
    pub winnings_by_quintile: [u64; 5],

    /// Sybil analysis.
    pub sybil_results: Vec<SybilAnalysisResult>,
}

/// Result of Sybil strategy comparison.
#[derive(Clone, Debug)]
pub struct SybilAnalysisResult {
    pub strategy: SybilStrategy,
    pub total_value: u64,
    pub num_utxos: usize,
    pub total_tickets: f64,
    pub tickets_per_value: f64,
    pub total_fees: u64,
    pub total_winnings: u64,
    pub net_result: i64,
}

impl LotterySimulation {
    /// Create a new simulation.
    pub fn new(config: LotteryConfig, fee_curve: FeeCurve) -> Self {
        Self {
            config,
            fee_curve,
            current_block: 0,
            utxos: HashMap::new(),
            next_utxo_id: 1,
            owners: HashMap::new(),
            cluster_wealth: ClusterWealth::new(),
            next_cluster_id: 1,
            lottery_pool: 0,
            total_burned: 0,
            ring_size: 11,
            metrics: LotteryMetrics::default(),
        }
    }

    /// Add an owner with initial wealth.
    pub fn add_owner(&mut self, wealth: u64, strategy: SybilStrategy) -> u64 {
        let owner_id = self.owners.len() as u64 + 1;
        let mut owner = LotteryOwner::new(owner_id, strategy);

        // Create UTXOs based on strategy
        let utxo_count = match strategy {
            SybilStrategy::Normal => 1,
            SybilStrategy::MultiAccount { num_accounts } => num_accounts,
            SybilStrategy::MaximizeSplit => {
                // Split into minimum-value UTXOs
                (wealth / self.config.min_utxo_value.max(1)) as u32
            }
        };

        let value_per_utxo = wealth / utxo_count.max(1) as u64;

        // Create cluster for this owner's minted wealth
        let cluster_id = ClusterId::new(self.next_cluster_id);
        self.next_cluster_id += 1;
        self.cluster_wealth.set(cluster_id, wealth);

        // Calculate cluster factor
        let cluster_factor = self.fee_curve.rate_bps(wealth) as f64 / 100.0;
        let cluster_factor = cluster_factor.max(1.0).min(6.0);

        for _ in 0..utxo_count {
            if value_per_utxo >= self.config.min_utxo_value {
                let utxo_id = self.next_utxo_id;
                self.next_utxo_id += 1;

                let utxo =
                    LotteryUtxo::new(utxo_id, owner_id, value_per_utxo, cluster_factor, 0);

                self.utxos.insert(utxo_id, utxo);
                owner.utxo_ids.push(utxo_id);
            }
        }

        self.owners.insert(owner_id, owner);
        owner_id
    }

    /// Get owner's total value.
    pub fn owner_value(&self, owner_id: u64) -> u64 {
        self.owners
            .get(&owner_id)
            .map(|o| {
                o.utxo_ids
                    .iter()
                    .filter_map(|id| self.utxos.get(id))
                    .map(|u| u.value)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Get owner's total tickets using the configured ticket model.
    pub fn owner_tickets(&self, owner_id: u64) -> f64 {
        let ticket_model = self.config.ticket_model;
        self.owners
            .get(&owner_id)
            .map(|o| {
                o.utxo_ids
                    .iter()
                    .filter_map(|id| self.utxos.get(id))
                    .filter(|u| u.is_eligible(self.current_block, &self.config))
                    .map(|u| u.tickets_for_model(ticket_model))
                    .sum()
            })
            .unwrap_or(0.0)
    }

    /// Simulate a transaction with configurable spender selection model.
    pub fn simulate_transaction_with_model(&mut self, fee: u64, model: TransactionModel) {
        match model {
            TransactionModel::ValueWeighted => self.simulate_transaction(fee),
            TransactionModel::Uniform => self.simulate_transaction_uniform(fee),
        }
    }

    /// Simulate a transaction with uniform spender selection.
    /// Each UTXO has equal probability of being the spender.
    pub fn simulate_transaction_uniform(&mut self, fee: u64) {
        let eligible_utxos: Vec<u64> = self
            .utxos
            .iter()
            .filter(|(_, u)| u.is_eligible(self.current_block, &self.config))
            .map(|(id, _)| *id)
            .collect();

        if eligible_utxos.len() < self.ring_size as usize {
            return;
        }

        let mut rng = rand::thread_rng();

        // Uniform selection - each UTXO equally likely to be spender
        let spender_idx = rng.gen_range(0..eligible_utxos.len());
        let spender_utxo_id = eligible_utxos[spender_idx];

        // Random selection for remaining ring members (decoys)
        let mut selected = vec![spender_utxo_id];
        let mut available: Vec<u64> = eligible_utxos
            .iter()
            .copied()
            .filter(|id| *id != spender_utxo_id)
            .collect();

        for _ in 1..self.ring_size {
            if available.is_empty() {
                break;
            }
            let idx = rng.gen_range(0..available.len());
            selected.push(available.remove(idx));
        }

        // The first ring member is the "spender" - deduct fee from them
        if let Some(spender) = self.utxos.get_mut(&spender_utxo_id) {
            // Calculate fee based on cluster factor (progressive)
            let actual_fee =
                ((fee as f64 * spender.cluster_factor) as u64).min(spender.value / 10);

            if actual_fee > 0 && spender.value > actual_fee {
                spender.value -= actual_fee;

                // Record fee-proportional tickets (for FeeProportional model)
                spender.record_fee_payment(actual_fee);

                // Track fee payment
                let owner_id = spender.owner_id;
                if let Some(owner) = self.owners.get_mut(&owner_id) {
                    owner.total_fees_paid += actual_fee;
                    owner.tx_count += 1;
                }

                // Collect fees
                let to_pool = (actual_fee as f64 * self.config.pool_fraction) as u64;
                let to_burn = actual_fee - to_pool;
                self.lottery_pool += to_pool;
                self.total_burned += to_burn;
                self.metrics.total_fees_collected += actual_fee;
            }
        }

        // Record participation for each ring member
        for utxo_id in selected {
            if let Some(utxo) = self.utxos.get_mut(&utxo_id) {
                utxo.record_ring_participation(self.ring_size);
            }
        }
    }

    /// Simulate a transaction (for ring participation).
    /// Spender selection is value-weighted (more value = more likely to transact).
    /// This models realistic transaction patterns where wealthier entities transact more.
    pub fn simulate_transaction(&mut self, fee: u64) {
        // Select ring members (random UTXOs)
        let eligible_utxos: Vec<(u64, u64)> = self
            .utxos
            .iter()
            .filter(|(_, u)| u.is_eligible(self.current_block, &self.config))
            .map(|(id, u)| (*id, u.value))
            .collect();

        if eligible_utxos.len() < self.ring_size as usize {
            return;
        }

        let mut rng = rand::thread_rng();

        // Value-weighted selection for spender (models realistic tx patterns)
        let total_value: u64 = eligible_utxos.iter().map(|(_, v)| v).sum();
        if total_value == 0 {
            return;
        }

        let spender_roll = rng.gen_range(0..total_value);
        let mut cumulative = 0u64;
        let mut spender_utxo_id = eligible_utxos[0].0;
        for (id, value) in &eligible_utxos {
            cumulative += value;
            if cumulative > spender_roll {
                spender_utxo_id = *id;
                break;
            }
        }

        // Random selection for remaining ring members (decoys)
        let mut selected = vec![spender_utxo_id];
        let mut available: Vec<u64> = eligible_utxos
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| *id != spender_utxo_id)
            .collect();

        for _ in 1..self.ring_size {
            if available.is_empty() {
                break;
            }
            let idx = rng.gen_range(0..available.len());
            selected.push(available.remove(idx));
        }

        // The first ring member is the "spender" - deduct fee from them
        if let Some(spender) = self.utxos.get_mut(&spender_utxo_id) {
            // Calculate fee based on cluster factor (progressive)
            let actual_fee =
                ((fee as f64 * spender.cluster_factor) as u64).min(spender.value / 10);

            if actual_fee > 0 && spender.value > actual_fee {
                spender.value -= actual_fee;

                // Record fee-proportional tickets (for FeeProportional model)
                spender.record_fee_payment(actual_fee);

                // Track fee payment
                let owner_id = spender.owner_id;
                if let Some(owner) = self.owners.get_mut(&owner_id) {
                    owner.total_fees_paid += actual_fee;
                    owner.tx_count += 1;
                }

                // Collect fees
                let to_pool = (actual_fee as f64 * self.config.pool_fraction) as u64;
                let to_burn = actual_fee - to_pool;
                self.lottery_pool += to_pool;
                self.total_burned += to_burn;
                self.metrics.total_fees_collected += actual_fee;
            }
        }

        // Record participation for each ring member
        for utxo_id in selected {
            if let Some(utxo) = self.utxos.get_mut(&utxo_id) {
                utxo.record_ring_participation(self.ring_size);
            }
        }
    }

    /// Run a lottery drawing.
    pub fn run_drawing(&mut self) {
        if self.lottery_pool == 0 {
            return;
        }

        let ticket_model = self.config.ticket_model;
        let eligible: Vec<(u64, f64)> = self
            .utxos
            .iter()
            .filter(|(_, u)| u.is_eligible(self.current_block, &self.config))
            .map(|(id, u)| (*id, u.tickets_for_model(ticket_model)))
            .collect();

        if eligible.is_empty() {
            return;
        }

        let total_tickets: f64 = eligible.iter().map(|(_, t)| t).sum();
        if total_tickets <= 0.0 {
            return;
        }

        // Distribute pool proportionally to tickets
        let pool = self.lottery_pool;
        self.lottery_pool = 0;

        // Collect payouts first to avoid borrow issues
        let payouts: Vec<(u64, u64, usize)> = eligible
            .iter()
            .filter_map(|(utxo_id, tickets)| {
                let share = tickets / total_tickets;
                let payout = (pool as f64 * share) as u64;
                if payout > 0 {
                    self.utxos.get(utxo_id).map(|utxo| {
                        let quintile = self.wealth_quintile(utxo.owner_id);
                        (*utxo_id, payout, quintile)
                    })
                } else {
                    None
                }
            })
            .collect();

        for (utxo_id, payout, quintile) in payouts {
            // Actually redistribute wealth by adding to UTXO value
            if let Some(utxo) = self.utxos.get_mut(&utxo_id) {
                utxo.value += payout;
                let owner_id = utxo.owner_id;
                if let Some(owner) = self.owners.get_mut(&owner_id) {
                    owner.total_winnings += payout;
                }
            }
            self.metrics.winnings_by_quintile[quintile] += payout;
        }

        self.metrics.drawings_held += 1;
        self.metrics.total_distributed += pool;
    }

    /// Determine wealth quintile (0-4) for an owner.
    fn wealth_quintile(&self, owner_id: u64) -> usize {
        let owner_wealth = self.owner_value(owner_id);
        let mut all_wealths: Vec<u64> = self.owners.keys().map(|id| self.owner_value(*id)).collect();
        all_wealths.sort();

        if all_wealths.is_empty() {
            return 2;
        }

        let rank = all_wealths.iter().position(|&w| w >= owner_wealth).unwrap_or(0);
        let percentile = rank * 100 / all_wealths.len();

        match percentile {
            0..=19 => 0,
            20..=39 => 1,
            40..=59 => 2,
            60..=79 => 3,
            _ => 4,
        }
    }

    /// Calculate current Gini coefficient.
    pub fn calculate_gini(&self) -> f64 {
        let mut wealths: Vec<f64> = self
            .owners
            .keys()
            .map(|id| self.owner_value(*id) as f64)
            .collect();

        if wealths.is_empty() {
            return 0.0;
        }

        wealths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let n = wealths.len() as f64;
        let total: f64 = wealths.iter().sum();

        if total == 0.0 {
            return 0.0;
        }

        let mut gini_sum = 0.0;
        for (i, &w) in wealths.iter().enumerate() {
            gini_sum += (2.0 * (i as f64 + 1.0) - n - 1.0) * w;
        }

        gini_sum / (n * total)
    }

    /// Advance simulation by N blocks using default (value-weighted) model.
    pub fn advance_blocks(&mut self, blocks: u64, txs_per_block: u32) {
        self.advance_blocks_with_model(blocks, txs_per_block, TransactionModel::ValueWeighted);
    }

    /// Advance simulation by N blocks with specified transaction model.
    pub fn advance_blocks_with_model(
        &mut self,
        blocks: u64,
        txs_per_block: u32,
        model: TransactionModel,
    ) {
        for _ in 0..blocks {
            self.current_block += 1;

            // Simulate transactions
            for _ in 0..txs_per_block {
                self.simulate_transaction_with_model(self.config.base_fee, model);
            }

            // Run drawing if at interval
            if self.current_block % self.config.drawing_interval == 0 {
                self.run_drawing();
            }
        }
    }

    /// Run Sybil analysis comparing strategies.
    pub fn analyze_sybil_strategies(&mut self) -> Vec<SybilAnalysisResult> {
        let mut results = Vec::new();

        for (owner_id, owner) in &self.owners {
            let total_value = self.owner_value(*owner_id);
            let num_utxos = owner.utxo_ids.len();
            let total_tickets = self.owner_tickets(*owner_id);
            let tickets_per_value = if total_value > 0 {
                total_tickets / total_value as f64
            } else {
                0.0
            };

            results.push(SybilAnalysisResult {
                strategy: owner.strategy,
                total_value,
                num_utxos,
                total_tickets,
                tickets_per_value,
                total_fees: owner.total_fees_paid,
                total_winnings: owner.total_winnings,
                net_result: owner.total_winnings as i64 - owner.total_fees_paid as i64,
            });
        }

        results
    }

    /// Snapshot current state for metrics.
    pub fn snapshot_metrics(&mut self) {
        let gini = self.calculate_gini();
        self.metrics.gini_snapshots.push((self.current_block, gini));

        // Track tickets by quintile
        for (owner_id, _) in &self.owners {
            let tickets = self.owner_tickets(*owner_id);
            let quintile = self.wealth_quintile(*owner_id);
            self.metrics.tickets_by_quintile[quintile] += tickets;
        }
    }
}

/// Run a complete Sybil resistance test.
pub fn run_sybil_test(
    total_wealth: u64,
    num_normal_owners: u32,
    num_sybil_owners: u32,
    sybil_accounts: u32,
    simulation_blocks: u64,
    txs_per_block: u32,
) -> SybilTestResult {
    let config = LotteryConfig::default();
    let fee_curve = FeeCurve::default_params();
    let mut sim = LotterySimulation::new(config, fee_curve);

    let wealth_per_owner = total_wealth / (num_normal_owners + num_sybil_owners) as u64;

    // Add normal owners
    for _ in 0..num_normal_owners {
        sim.add_owner(wealth_per_owner, SybilStrategy::Normal);
    }

    // Add Sybil attackers
    for _ in 0..num_sybil_owners {
        sim.add_owner(
            wealth_per_owner,
            SybilStrategy::MultiAccount {
                num_accounts: sybil_accounts,
            },
        );
    }

    // Initial snapshot
    sim.snapshot_metrics();

    // Run simulation
    sim.advance_blocks(simulation_blocks, txs_per_block);

    // Final snapshot
    sim.snapshot_metrics();

    // Analyze results
    let analysis = sim.analyze_sybil_strategies();

    let normal_results: Vec<_> = analysis
        .iter()
        .filter(|r| r.strategy == SybilStrategy::Normal)
        .collect();
    let sybil_results: Vec<_> = analysis
        .iter()
        .filter(|r| matches!(r.strategy, SybilStrategy::MultiAccount { .. }))
        .collect();

    let avg_normal_tickets_per_value = if !normal_results.is_empty() {
        normal_results.iter().map(|r| r.tickets_per_value).sum::<f64>() / normal_results.len() as f64
    } else {
        0.0
    };

    let avg_sybil_tickets_per_value = if !sybil_results.is_empty() {
        sybil_results.iter().map(|r| r.tickets_per_value).sum::<f64>() / sybil_results.len() as f64
    } else {
        0.0
    };

    let avg_normal_winnings: u64 = if !normal_results.is_empty() {
        normal_results.iter().map(|r| r.total_winnings).sum::<u64>() / normal_results.len() as u64
    } else {
        0
    };

    let avg_sybil_winnings: u64 = if !sybil_results.is_empty() {
        sybil_results.iter().map(|r| r.total_winnings).sum::<u64>() / sybil_results.len() as u64
    } else {
        0
    };

    SybilTestResult {
        normal_tickets_per_value: avg_normal_tickets_per_value,
        sybil_tickets_per_value: avg_sybil_tickets_per_value,
        ticket_ratio: avg_sybil_tickets_per_value / avg_normal_tickets_per_value.max(0.001),
        normal_avg_winnings: avg_normal_winnings,
        sybil_avg_winnings: avg_sybil_winnings,
        winnings_ratio: avg_sybil_winnings as f64 / avg_normal_winnings.max(1) as f64,
        initial_gini: sim.metrics.gini_snapshots.first().map(|(_, g)| *g).unwrap_or(0.0),
        final_gini: sim.metrics.gini_snapshots.last().map(|(_, g)| *g).unwrap_or(0.0),
        gini_change: sim.metrics.gini_snapshots.last().map(|(_, g)| *g).unwrap_or(0.0)
            - sim.metrics.gini_snapshots.first().map(|(_, g)| *g).unwrap_or(0.0),
        sybil_profitable: avg_sybil_winnings > avg_normal_winnings,
    }
}

/// Result of a Sybil resistance test.
#[derive(Clone, Debug)]
pub struct SybilTestResult {
    /// Tickets per value for normal strategy.
    pub normal_tickets_per_value: f64,
    /// Tickets per value for Sybil strategy.
    pub sybil_tickets_per_value: f64,
    /// Ratio of Sybil to normal tickets per value.
    pub ticket_ratio: f64,
    /// Average winnings for normal owners.
    pub normal_avg_winnings: u64,
    /// Average winnings for Sybil owners.
    pub sybil_avg_winnings: u64,
    /// Ratio of Sybil to normal winnings.
    pub winnings_ratio: f64,
    /// Initial Gini coefficient.
    pub initial_gini: f64,
    /// Final Gini coefficient.
    pub final_gini: f64,
    /// Change in Gini.
    pub gini_change: f64,
    /// Whether Sybil strategy was profitable.
    pub sybil_profitable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_weighting_prevents_sybil() {
        let config = LotteryConfig::default();
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Add one normal owner with 10,000 BTH
        let normal_id = sim.add_owner(10_000_000, SybilStrategy::Normal);

        // Add one Sybil owner with 10,000 BTH split 10 ways
        let sybil_id = sim.add_owner(
            10_000_000,
            SybilStrategy::MultiAccount { num_accounts: 10 },
        );

        // Make UTXOs eligible
        sim.current_block = 1000;

        // Check tickets
        let normal_tickets = sim.owner_tickets(normal_id);
        let sybil_tickets = sim.owner_tickets(sybil_id);

        // Should be approximately equal (within 10%)
        let ratio = sybil_tickets / normal_tickets;
        assert!(
            (0.9..=1.1).contains(&ratio),
            "Sybil should not have significant ticket advantage: ratio = {:.2} \
             (normal={:.0}, sybil={:.0})",
            ratio,
            normal_tickets,
            sybil_tickets
        );
    }

    #[test]
    fn test_cluster_factor_is_progressive() {
        let config = LotteryConfig::default();
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Poor owner (low cluster factor)
        let poor_id = sim.add_owner(100_000, SybilStrategy::Normal);

        // Wealthy owner (high cluster factor)
        let rich_id = sim.add_owner(10_000_000, SybilStrategy::Normal);

        sim.current_block = 1000;

        let poor_tickets = sim.owner_tickets(poor_id);
        let poor_value = sim.owner_value(poor_id);
        let poor_tickets_per_value = poor_tickets / poor_value as f64;

        let rich_tickets = sim.owner_tickets(rich_id);
        let rich_value = sim.owner_value(rich_id);
        let rich_tickets_per_value = rich_tickets / rich_value as f64;

        // Poor should get more tickets per value
        assert!(
            poor_tickets_per_value > rich_tickets_per_value,
            "Poor should get more tickets per BTH: poor={:.4}, rich={:.4}",
            poor_tickets_per_value,
            rich_tickets_per_value
        );
    }

    #[test]
    fn test_activity_is_value_weighted() {
        let config = LotteryConfig::default();
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Two owners with same wealth, different UTXO counts
        let single_id = sim.add_owner(1_000_000, SybilStrategy::Normal);
        let split_id = sim.add_owner(
            1_000_000,
            SybilStrategy::MultiAccount { num_accounts: 10 },
        );

        sim.current_block = 1000;

        // Simulate ring participation
        // Each UTXO gets selected proportionally
        for utxo in sim.utxos.values_mut() {
            // Simulate 10 selections per UTXO
            for _ in 0..10 {
                utxo.record_ring_participation(11);
            }
        }

        // Check activity multipliers are similar
        let single_tickets = sim.owner_tickets(single_id);
        let split_tickets = sim.owner_tickets(split_id);

        let ratio = split_tickets / single_tickets;
        assert!(
            (0.8..=1.2).contains(&ratio),
            "Split should not have significant activity advantage: ratio = {:.2}",
            ratio
        );
    }

    #[test]
    fn test_sybil_not_profitable() {
        let result = run_sybil_test(
            100_000_000, // 100M total wealth
            10,          // 10 normal owners
            10,          // 10 Sybil owners
            10,          // 10 accounts each
            10_000,      // 10k blocks
            10,          // 10 txs per block
        );

        // Sybil should not have significant advantage (within 10%)
        // With value-weighted selection, Sybil actually gets selected LESS
        // (many small UTXOs vs one large), which is even better for Sybil resistance.
        assert!(
            result.winnings_ratio < 1.10,
            "Sybil should not have >10% advantage: ratio={:.4}, normal={}, sybil={}",
            result.winnings_ratio,
            result.normal_avg_winnings,
            result.sybil_avg_winnings
        );
    }

    #[test]
    fn test_lottery_reduces_gini_with_inequality() {
        // Test that lottery reduces Gini when starting with unequal distribution
        let config = LotteryConfig {
            base_fee: 100,
            drawing_interval: 10,
            ..LotteryConfig::default()
        };
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Create unequal distribution (need >=11 UTXOs for ring)
        // 10 poor (5%), 5 middle (25%), 2 rich (70%)
        let total = 100_000_000u64;
        for _ in 0..10 {
            sim.add_owner(total / 200, SybilStrategy::Normal); // 0.5% each
        }
        for _ in 0..5 {
            sim.add_owner(total * 5 / 100, SybilStrategy::Normal); // 5% each
        }
        for _ in 0..2 {
            sim.add_owner(total * 35 / 100, SybilStrategy::Normal); // 35% each
        }

        sim.current_block = 1000;
        let initial_gini = sim.calculate_gini();

        sim.advance_blocks(20_000, 20);
        let final_gini = sim.calculate_gini();

        // Lottery should reduce Gini with unequal starting distribution
        assert!(
            final_gini < initial_gini,
            "Lottery should reduce Gini: initial={:.4}, final={:.4}",
            initial_gini,
            final_gini
        );
    }

    #[test]
    fn test_progressive_distribution() {
        let config = LotteryConfig {
            base_fee: 100,
            drawing_interval: 10,
            ..LotteryConfig::default()
        };
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Need at least 11 UTXOs for ring (ring_size = 11)
        // Create wealth distribution with more owners
        for _ in 0..8 {
            sim.add_owner(100_000, SybilStrategy::Normal); // 8 poor
        }
        for _ in 0..4 {
            sim.add_owner(1_000_000, SybilStrategy::Normal); // 4 medium
        }
        for _ in 0..3 {
            sim.add_owner(10_000_000, SybilStrategy::Normal); // 3 rich
        }

        sim.current_block = 1000;

        // Run simulation
        sim.advance_blocks(10_000, 10);
        sim.snapshot_metrics();

        // Total winnings should be positive (some distribution occurred)
        let total_winnings: u64 = sim.metrics.winnings_by_quintile.iter().sum();
        assert!(
            total_winnings > 0,
            "Should have distributed some winnings"
        );
    }

    /// Compare lottery redistribution vs cluster tax for Gini reduction.
    ///
    /// This test runs both approaches with equivalent parameters and compares
    /// how effectively each reduces wealth inequality (Gini coefficient).
    #[test]
    fn test_lottery_vs_cluster_tax_gini() {
        use crate::simulation::{
            run_simulation, AgentId, MerchantAgent, MinterAgent, RetailUserAgent, SimulationConfig,
        };
        use crate::simulation::agent::Agent;

        // === LOTTERY SIMULATION ===
        // Use lower fees to prevent poor UTXOs from being drained too quickly
        let lottery_config = LotteryConfig {
            base_fee: 100, // Lower base fee for realistic simulation
            drawing_interval: 10, // More frequent drawings
            ..LotteryConfig::default()
        };
        let fee_curve = FeeCurve::default_params();
        let mut lottery_sim = LotterySimulation::new(lottery_config, fee_curve.clone());

        // Need enough UTXOs to form rings (ring_size = 11)
        // Create highly unequal initial distribution (Gini ~0.7)
        // Use more owners to ensure we have enough UTXOs for rings
        let total_wealth = 100_000_000u64;

        // 10 poor (0.5% each = 5%), 5 middle (5% each = 25%), 2 rich (35% each = 70%)
        for _ in 0..10 {
            lottery_sim.add_owner(total_wealth / 200, SybilStrategy::Normal); // 0.5%
        }
        for _ in 0..5 {
            lottery_sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal); // 5%
        }
        for _ in 0..2 {
            lottery_sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal); // 35%
        }

        lottery_sim.current_block = 1000;
        let lottery_initial_gini = lottery_sim.calculate_gini();

        // Run lottery simulation
        lottery_sim.advance_blocks(50_000, 20);
        let lottery_final_gini = lottery_sim.calculate_gini();
        let lottery_gini_reduction = lottery_initial_gini - lottery_final_gini;


        // === CLUSTER TAX SIMULATION ===
        // Create equivalent agents for the cluster tax simulation
        let mut agents: Vec<Box<dyn Agent>> = Vec::new();

        // 10 poor retail users (0.5% each = 5%)
        for i in 0..10 {
            let mut agent = RetailUserAgent::new(AgentId(i + 1))
                .with_merchants(vec![AgentId(11), AgentId(12), AgentId(13)]);
            agent.account_mut().balance = total_wealth / 200;
            agents.push(Box::new(agent));
        }

        // 5 middle merchants (5% each = 25%)
        for i in 0..5 {
            let mut agent = MerchantAgent::new(AgentId(i + 11));
            agent.account_mut().balance = total_wealth * 5 / 100;
            agents.push(Box::new(agent));
        }

        // 2 rich minters (35% each = 70%)
        for i in 0..2 {
            let mut agent = MinterAgent::new(AgentId(i + 16))
                .with_buyers(vec![AgentId(1), AgentId(2), AgentId(3)]);
            agent.account_mut().balance = total_wealth * 35 / 100;
            agents.push(Box::new(agent));
        }

        let cluster_config = SimulationConfig {
            rounds: 5000, // ~50,000 blocks at 10 txs/round = similar scale
            snapshot_frequency: 500,
            ..Default::default()
        };

        let result = run_simulation(&mut agents, &cluster_config);

        let cluster_initial_gini = result
            .metrics
            .snapshots
            .first()
            .map(|m| m.gini_coefficient)
            .unwrap_or(0.0);
        let cluster_final_gini = result
            .metrics
            .snapshots
            .last()
            .map(|m| m.gini_coefficient)
            .unwrap_or(0.0);
        let cluster_gini_reduction = cluster_initial_gini - cluster_final_gini;

        // Report results
        eprintln!("\n=== GINI REDUCTION COMPARISON ===");
        eprintln!("Lottery System:");
        eprintln!("  Initial Gini: {:.4}", lottery_initial_gini);
        eprintln!("  Final Gini:   {:.4}", lottery_final_gini);
        eprintln!("  Reduction:    {:.4}", lottery_gini_reduction);
        eprintln!("Cluster Tax System:");
        eprintln!("  Initial Gini: {:.4}", cluster_initial_gini);
        eprintln!("  Final Gini:   {:.4}", cluster_final_gini);
        eprintln!("  Reduction:    {:.4}", cluster_gini_reduction);
        eprintln!("==================================\n");

        // Both systems should reduce Gini somewhat (or at least not increase it much)
        assert!(
            lottery_final_gini <= lottery_initial_gini + 0.05,
            "Lottery should not significantly increase Gini"
        );
        assert!(
            cluster_final_gini <= cluster_initial_gini + 0.05,
            "Cluster tax should not significantly increase Gini"
        );

        // Note: Which is more effective depends on many parameters.
        // This test primarily validates that both approaches work.
    }

    /// Compare lottery effectiveness under different transaction frequency models.
    ///
    /// This test compares:
    /// - Value-weighted: Rich transact more (proportional to holdings)
    /// - Uniform: Everyone transacts equally (each UTXO equally likely)
    #[test]
    fn test_transaction_model_comparison() {
        let total_wealth = 100_000_000u64;

        // Helper to create a simulation with unequal distribution
        let create_sim = || {
            let config = LotteryConfig {
                base_fee: 100,
                drawing_interval: 10,
                ..LotteryConfig::default()
            };
            let fee_curve = FeeCurve::default_params();
            let mut sim = LotterySimulation::new(config, fee_curve);

            // 10 poor (0.5% each = 5%), 5 middle (5% each = 25%), 2 rich (35% each = 70%)
            for _ in 0..10 {
                sim.add_owner(total_wealth / 200, SybilStrategy::Normal);
            }
            for _ in 0..5 {
                sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal);
            }
            for _ in 0..2 {
                sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal);
            }
            sim.current_block = 1000;
            sim
        };

        // === VALUE-WEIGHTED MODEL ===
        let mut sim_value = create_sim();
        let value_initial_gini = sim_value.calculate_gini();
        sim_value.advance_blocks_with_model(30_000, 20, TransactionModel::ValueWeighted);
        let value_final_gini = sim_value.calculate_gini();
        let value_reduction = value_initial_gini - value_final_gini;

        // === UNIFORM MODEL ===
        let mut sim_uniform = create_sim();
        let uniform_initial_gini = sim_uniform.calculate_gini();
        sim_uniform.advance_blocks_with_model(30_000, 20, TransactionModel::Uniform);
        let uniform_final_gini = sim_uniform.calculate_gini();
        let uniform_reduction = uniform_initial_gini - uniform_final_gini;

        // Report results
        eprintln!("\n=== TRANSACTION MODEL COMPARISON ===");
        eprintln!("Value-Weighted (rich transact more):");
        eprintln!("  Initial Gini: {:.4}", value_initial_gini);
        eprintln!("  Final Gini:   {:.4}", value_final_gini);
        eprintln!("  Reduction:    {:.4} ({:.1}%)", value_reduction, value_reduction / value_initial_gini * 100.0);
        eprintln!("  Fees collected: {}", sim_value.metrics.total_fees_collected);
        eprintln!("  Pool distributed: {}", sim_value.metrics.total_distributed);
        eprintln!("");
        eprintln!("Uniform (everyone transacts equally):");
        eprintln!("  Initial Gini: {:.4}", uniform_initial_gini);
        eprintln!("  Final Gini:   {:.4}", uniform_final_gini);
        eprintln!("  Reduction:    {:.4} ({:.1}%)", uniform_reduction, uniform_reduction / uniform_initial_gini * 100.0);
        eprintln!("  Fees collected: {}", sim_uniform.metrics.total_fees_collected);
        eprintln!("  Pool distributed: {}", sim_uniform.metrics.total_distributed);
        eprintln!("=====================================\n");

        // Both models should be tested - we're interested in seeing the difference
        // The test passes as long as it runs; the output tells us what happens
    }

    /// Compare ActivityBased vs FeeProportional ticket models.
    ///
    /// This tests the wash-trading resistant FeeProportional model against
    /// the original ActivityBased model under both transaction patterns.
    #[test]
    fn test_ticket_model_comparison() {
        let total_wealth = 100_000_000u64;

        // Helper to create a simulation with specified ticket model
        let create_sim = |ticket_model: TicketModel| {
            let config = LotteryConfig {
                base_fee: 100,
                drawing_interval: 10,
                ticket_model,
                ..LotteryConfig::default()
            };
            let fee_curve = FeeCurve::default_params();
            let mut sim = LotterySimulation::new(config, fee_curve);

            // 10 poor (0.5% each = 5%), 5 middle (5% each = 25%), 2 rich (35% each = 70%)
            for _ in 0..10 {
                sim.add_owner(total_wealth / 200, SybilStrategy::Normal);
            }
            for _ in 0..5 {
                sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal);
            }
            for _ in 0..2 {
                sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal);
            }
            sim.current_block = 1000;
            sim
        };

        // Run 4 scenarios: 2 ticket models × 2 transaction models
        let scenarios = [
            ("ActivityBased + ValueWeighted", TicketModel::ActivityBased, TransactionModel::ValueWeighted),
            ("ActivityBased + Uniform", TicketModel::ActivityBased, TransactionModel::Uniform),
            ("FeeProportional + ValueWeighted", TicketModel::FeeProportional, TransactionModel::ValueWeighted),
            ("FeeProportional + Uniform", TicketModel::FeeProportional, TransactionModel::Uniform),
        ];

        eprintln!("\n=== TICKET MODEL COMPARISON ===");
        eprintln!("{:<35} {:>12} {:>12} {:>12} {:>15}", "Scenario", "Init Gini", "Final Gini", "Change", "Fees Collected");
        eprintln!("{}", "-".repeat(90));

        for (name, ticket_model, tx_model) in scenarios {
            let mut sim = create_sim(ticket_model);
            let initial_gini = sim.calculate_gini();
            sim.advance_blocks_with_model(30_000, 20, tx_model);
            let final_gini = sim.calculate_gini();
            let change = initial_gini - final_gini;
            let change_pct = change / initial_gini * 100.0;

            eprintln!(
                "{:<35} {:>12.4} {:>12.4} {:>+11.1}% {:>15}",
                name,
                initial_gini,
                final_gini,
                change_pct,
                sim.metrics.total_fees_collected
            );
        }
        eprintln!("================================\n");
    }

    /// Test that FeeProportional model is wash-trading resistant.
    ///
    /// Key property: An individual's tickets/fee ratio is fixed by their cluster factor.
    /// Wash trading cannot increase this ratio - you get exactly what you pay for.
    #[test]
    fn test_fee_proportional_wash_resistance() {
        // Test that individual ticket rates match expected formula
        // tickets = fee × (max_factor - your_factor) / max_factor

        let config = LotteryConfig {
            base_fee: 100,
            drawing_interval: 1000, // No drawings during this test
            ticket_model: TicketModel::FeeProportional,
            ..LotteryConfig::default()
        };
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Create one poor owner (low factor) and one rich owner (high factor)
        let poor_id = sim.add_owner(100_000, SybilStrategy::Normal); // Low wealth → low factor
        let rich_id = sim.add_owner(50_000_000, SybilStrategy::Normal); // High wealth → high factor

        sim.current_block = 1000;

        // Get their cluster factors
        let poor_utxo = sim.utxos.values().find(|u| u.owner_id == poor_id).unwrap();
        let rich_utxo = sim.utxos.values().find(|u| u.owner_id == rich_id).unwrap();
        let poor_factor = poor_utxo.cluster_factor;
        let rich_factor = rich_utxo.cluster_factor;

        // Expected ticket rates
        let poor_expected_rate = (MAX_CLUSTER_FACTOR - poor_factor) / MAX_CLUSTER_FACTOR;
        let rich_expected_rate = (MAX_CLUSTER_FACTOR - rich_factor) / MAX_CLUSTER_FACTOR;

        eprintln!("\n=== FEE-PROPORTIONAL TICKET RATES ===");
        eprintln!("Poor owner (factor {:.2}): expected rate = {:.4}", poor_factor, poor_expected_rate);
        eprintln!("Rich owner (factor {:.2}): expected rate = {:.4}", rich_factor, rich_expected_rate);

        // Simulate some fees manually
        let test_fee = 1000u64;
        let poor_utxo_id = *sim.owners.get(&poor_id).unwrap().utxo_ids.first().unwrap();
        let rich_utxo_id = *sim.owners.get(&rich_id).unwrap().utxo_ids.first().unwrap();

        // Record fee payments
        sim.utxos.get_mut(&poor_utxo_id).unwrap().record_fee_payment(test_fee);
        sim.utxos.get_mut(&rich_utxo_id).unwrap().record_fee_payment(test_fee);

        let poor_tickets = sim.utxos.get(&poor_utxo_id).unwrap().tickets_from_fees;
        let rich_tickets = sim.utxos.get(&rich_utxo_id).unwrap().tickets_from_fees;

        let poor_actual_rate = poor_tickets / test_fee as f64;
        let rich_actual_rate = rich_tickets / test_fee as f64;

        eprintln!("Poor actual rate: {:.4} (expected {:.4})", poor_actual_rate, poor_expected_rate);
        eprintln!("Rich actual rate: {:.4} (expected {:.4})", rich_actual_rate, rich_expected_rate);
        eprintln!("Poor gets {:.1}x more tickets per fee than rich", poor_actual_rate / rich_actual_rate);
        eprintln!("======================================\n");

        // Verify rates match expected formula
        assert!(
            (poor_actual_rate - poor_expected_rate).abs() < 0.001,
            "Poor rate should match formula"
        );
        assert!(
            (rich_actual_rate - rich_expected_rate).abs() < 0.001,
            "Rich rate should match formula"
        );

        // Key property: rate is fixed, doesn't increase with more fees
        sim.utxos.get_mut(&poor_utxo_id).unwrap().record_fee_payment(test_fee * 100);
        let poor_after_more = sim.utxos.get(&poor_utxo_id).unwrap().tickets_from_fees;
        let poor_rate_after = (poor_after_more - poor_tickets) / (test_fee * 100) as f64;

        assert!(
            (poor_rate_after - poor_expected_rate).abs() < 0.001,
            "Rate should be constant regardless of volume: before={:.4}, after={:.4}",
            poor_actual_rate,
            poor_rate_after
        );
    }
}
