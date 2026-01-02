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
    /// Simplest: tickets = value / cluster_factor
    /// No activity or fee tracking needed. Computed at draw time.
    /// Wash trading has negative EV (costs fees, doesn't change value).
    PureValueWeighted,
    /// Uniform per UTXO: each UTXO = 1 ticket regardless of value.
    /// Progressive via population statistics: more poor people than rich,
    /// so random UTXO is more likely to belong to poor person.
    /// Sybil-resistant if lottery_pool < UTXO_creation_cost × UTXO_count.
    UniformPerUtxo,
}

/// Maximum cluster factor for fee-proportional ticket calculation.
const MAX_CLUSTER_FACTOR: f64 = 6.0;

/// Distribution mode for lottery winnings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistributionMode {
    /// Accumulate fees in pool, draw periodically.
    Pooled,
    /// Immediately distribute to N random UTXOs per transaction.
    Immediate { winners_per_tx: u32 },
}

/// Selection mode for lottery winners.
///
/// This determines how lottery winners are selected from the UTXO set.
/// Different modes trade off between progressivity and Sybil resistance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectionMode {
    /// Uniform: each UTXO has equal chance (progressive but gameable).
    /// Vulnerability: 10 UTXOs = 10x lottery chances.
    Uniform,

    /// Value-weighted: probability proportional to UTXO value.
    /// Sybil-resistant but not progressive (same as holding).
    ValueWeighted,

    /// Square-root weighted: probability proportional to sqrt(value).
    /// Hybrid: some progressivity, harder to game.
    /// 10 UTXOs of value V each = sqrt(V)*10 weight
    /// 1 UTXO of value 10V = sqrt(10V) ≈ 3.16*sqrt(V) weight
    /// So splitting gives 3.16x advantage instead of 10x.
    SqrtWeighted,

    /// Log-weighted: probability proportional to 1 + log2(value).
    /// More progressive than sqrt, still Sybil-resistant.
    LogWeighted,

    /// Capped uniform: each UTXO = 1 ticket, but max N tickets per owner.
    /// Requires owner tracking (not privacy-preserving).
    CappedUniform { max_per_owner: u32 },

    /// Tunable hybrid: weight = α + (1-α) × normalized_value
    /// α = 1.0: pure uniform (10x gameable, fully progressive)
    /// α = 0.0: pure value-weighted (1x gameable, not progressive)
    /// α = 0.5: balanced hybrid
    ///
    /// This allows exploring the Pareto frontier between
    /// progressivity and Sybil resistance.
    Hybrid { alpha: f64 },

    /// Age-weighted: older UTXOs get more weight.
    /// weight = 1 + (age / max_age) × age_bonus
    /// Discourages rapid UTXO accumulation.
    /// Privacy cost: reveals approximate UTXO age through participation.
    AgeWeighted {
        max_age_blocks: u64,
        age_bonus: f64,
    },

    /// Cluster-factor weighted: lower factor = more weight.
    /// weight = value × (max_factor - factor + 1) / max_factor
    /// Progressive: commerce coins worth more than minter coins.
    /// Privacy cost: reveals coin origin (~1-2 bits).
    ClusterWeighted,
}

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

    /// Distribution mode: pooled vs immediate.
    pub distribution_mode: DistributionMode,

    /// Per-output fee multiplier (for superlinear fees).
    /// Total fee = base_fee × cluster_factor × outputs^output_fee_exponent
    pub output_fee_exponent: f64,

    /// Selection mode for lottery winners.
    pub selection_mode: SelectionMode,
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
            distribution_mode: DistributionMode::Pooled,
            output_fee_exponent: 1.0, // Linear by default
            selection_mode: SelectionMode::Uniform, // Default to uniform for backwards compat
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
            // Simplest model: just value / cluster_factor, no state tracking
            TicketModel::PureValueWeighted => self.base_tickets(),
            // Uniform: each UTXO = 1 ticket regardless of value
            // Progressive via population statistics
            TicketModel::UniformPerUtxo => 1.0,
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

    /// Simulate a transaction with immediate distribution to random UTXOs.
    ///
    /// Combined design:
    /// 1. Fee = base × cluster_factor × outputs^exponent (progressive + anti-Sybil)
    /// 2. 80% of fee immediately distributed to N random UTXOs (uniform selection)
    /// 3. 20% burned
    ///
    /// This is simpler than pooled distribution - no accumulation, no periodic draws.
    pub fn simulate_transaction_immediate(
        &mut self,
        base_fee: u64,
        num_outputs: u32,
        tx_model: TransactionModel,
    ) {
        let mut rng = rand::thread_rng();

        // Get all UTXOs (for lottery distribution, don't filter by min value)
        let all_utxos: Vec<u64> = self.utxos.keys().copied().collect();
        if all_utxos.len() < 2 {
            return;
        }

        // Select spender based on transaction model
        let spender_utxo_id = match tx_model {
            TransactionModel::ValueWeighted => {
                let eligible: Vec<(u64, u64)> = self
                    .utxos
                    .iter()
                    .filter(|(_, u)| u.value > 0)
                    .map(|(id, u)| (*id, u.value))
                    .collect();
                if eligible.is_empty() {
                    return;
                }
                let total: u64 = eligible.iter().map(|(_, v)| v).sum();
                if total == 0 {
                    return;
                }
                let roll = rng.gen_range(0..total);
                let mut cumulative = 0u64;
                let mut selected = eligible[0].0;
                for (id, value) in &eligible {
                    cumulative += value;
                    if cumulative > roll {
                        selected = *id;
                        break;
                    }
                }
                selected
            }
            TransactionModel::Uniform => {
                let eligible: Vec<u64> = self
                    .utxos
                    .iter()
                    .filter(|(_, u)| u.value > 0)
                    .map(|(id, _)| *id)
                    .collect();
                if eligible.is_empty() {
                    return;
                }
                eligible[rng.gen_range(0..eligible.len())]
            }
        };

        // Calculate fee: base × cluster_factor × outputs^exponent
        let (actual_fee, cluster_factor) = {
            let spender = match self.utxos.get(&spender_utxo_id) {
                Some(s) => s,
                None => return,
            };
            let output_multiplier = (num_outputs as f64).powf(self.config.output_fee_exponent);
            let fee = (base_fee as f64 * spender.cluster_factor * output_multiplier) as u64;
            let capped_fee = fee.min(spender.value / 2); // Don't take more than half
            (capped_fee, spender.cluster_factor)
        };

        if actual_fee == 0 {
            return;
        }

        // Deduct fee from spender
        if let Some(spender) = self.utxos.get_mut(&spender_utxo_id) {
            spender.value -= actual_fee;
            let owner_id = spender.owner_id;
            if let Some(owner) = self.owners.get_mut(&owner_id) {
                owner.total_fees_paid += actual_fee;
                owner.tx_count += 1;
            }
        }

        self.metrics.total_fees_collected += actual_fee;

        // Calculate distribution
        let to_distribute = (actual_fee as f64 * self.config.pool_fraction) as u64;
        let to_burn = actual_fee - to_distribute;
        self.total_burned += to_burn;

        // Immediately distribute to N random UTXOs (uniform selection)
        let winners_per_tx = match self.config.distribution_mode {
            DistributionMode::Immediate { winners_per_tx } => winners_per_tx,
            DistributionMode::Pooled => 4, // Default fallback
        };

        if to_distribute > 0 && !all_utxos.is_empty() {
            let per_winner = to_distribute / winners_per_tx as u64;
            if per_winner > 0 {
                // Select winners based on selection mode
                for _ in 0..winners_per_tx.min(all_utxos.len() as u32) {
                    let winner_id = self.select_winner_by_mode(&all_utxos, &mut rng);

                    if let Some(winner) = self.utxos.get_mut(&winner_id) {
                        winner.value += per_winner;
                        let owner_id = winner.owner_id;
                        if let Some(owner) = self.owners.get_mut(&owner_id) {
                            owner.total_winnings += per_winner;
                        }
                    }
                    self.metrics.total_distributed += per_winner;
                }
            }
        }
    }

    /// Select a winner based on the configured selection mode.
    fn select_winner_by_mode(&self, utxo_ids: &[u64], rng: &mut impl Rng) -> u64 {
        match self.config.selection_mode {
            SelectionMode::Uniform => {
                // Each UTXO has equal chance
                utxo_ids[rng.gen_range(0..utxo_ids.len())]
            }
            SelectionMode::ValueWeighted => {
                // Probability proportional to value
                let weights: Vec<(u64, u64)> = utxo_ids
                    .iter()
                    .filter_map(|id| self.utxos.get(id).map(|u| (*id, u.value)))
                    .collect();
                let total: u64 = weights.iter().map(|(_, v)| v).sum();
                if total == 0 {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                let roll = rng.gen_range(0..total);
                let mut cumulative = 0u64;
                for (id, value) in weights {
                    cumulative += value;
                    if cumulative > roll {
                        return id;
                    }
                }
                utxo_ids[0]
            }
            SelectionMode::SqrtWeighted => {
                // Probability proportional to sqrt(value)
                let weights: Vec<(u64, f64)> = utxo_ids
                    .iter()
                    .filter_map(|id| self.utxos.get(id).map(|u| (*id, (u.value as f64).sqrt())))
                    .collect();
                let total: f64 = weights.iter().map(|(_, w)| w).sum();
                if total <= 0.0 {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                let roll = rng.gen::<f64>() * total;
                let mut cumulative = 0.0;
                for (id, weight) in weights {
                    cumulative += weight;
                    if cumulative > roll {
                        return id;
                    }
                }
                utxo_ids[0]
            }
            SelectionMode::LogWeighted => {
                // Probability proportional to 1 + log2(value)
                let weights: Vec<(u64, f64)> = utxo_ids
                    .iter()
                    .filter_map(|id| {
                        self.utxos.get(id).map(|u| {
                            let w = if u.value > 0 {
                                1.0 + (u.value as f64).log2()
                            } else {
                                1.0
                            };
                            (*id, w)
                        })
                    })
                    .collect();
                let total: f64 = weights.iter().map(|(_, w)| w).sum();
                if total <= 0.0 {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                let roll = rng.gen::<f64>() * total;
                let mut cumulative = 0.0;
                for (id, weight) in weights {
                    cumulative += weight;
                    if cumulative > roll {
                        return id;
                    }
                }
                utxo_ids[0]
            }
            SelectionMode::CappedUniform { max_per_owner } => {
                // Count UTXOs per owner and cap at max_per_owner
                let mut owner_counts: std::collections::HashMap<u64, u32> =
                    std::collections::HashMap::new();
                let eligible: Vec<u64> = utxo_ids
                    .iter()
                    .filter(|id| {
                        if let Some(utxo) = self.utxos.get(id) {
                            let count = owner_counts.entry(utxo.owner_id).or_insert(0);
                            if *count < max_per_owner {
                                *count += 1;
                                return true;
                            }
                        }
                        false
                    })
                    .copied()
                    .collect();
                if eligible.is_empty() {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                eligible[rng.gen_range(0..eligible.len())]
            }
            SelectionMode::Hybrid { alpha } => {
                // weight = α + (1-α) × normalized_value
                // where normalized_value = value / max_value
                let max_value = utxo_ids
                    .iter()
                    .filter_map(|id| self.utxos.get(id).map(|u| u.value))
                    .max()
                    .unwrap_or(1) as f64;

                let weights: Vec<(u64, f64)> = utxo_ids
                    .iter()
                    .filter_map(|id| {
                        self.utxos.get(id).map(|u| {
                            let norm_value = u.value as f64 / max_value;
                            let weight = alpha + (1.0 - alpha) * norm_value;
                            (*id, weight)
                        })
                    })
                    .collect();

                let total: f64 = weights.iter().map(|(_, w)| w).sum();
                if total <= 0.0 {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                let roll = rng.gen::<f64>() * total;
                let mut cumulative = 0.0;
                for (id, weight) in weights {
                    cumulative += weight;
                    if cumulative > roll {
                        return id;
                    }
                }
                utxo_ids[0]
            }
            SelectionMode::AgeWeighted { max_age_blocks, age_bonus } => {
                // weight = 1 + (age / max_age) × age_bonus
                let current_block = self.current_block;
                let weights: Vec<(u64, f64)> = utxo_ids
                    .iter()
                    .filter_map(|id| {
                        self.utxos.get(id).map(|u| {
                            let age = current_block.saturating_sub(u.creation_block);
                            let age_ratio = (age as f64 / max_age_blocks as f64).min(1.0);
                            let weight = 1.0 + age_ratio * age_bonus;
                            (*id, weight)
                        })
                    })
                    .collect();

                let total: f64 = weights.iter().map(|(_, w)| w).sum();
                if total <= 0.0 {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                let roll = rng.gen::<f64>() * total;
                let mut cumulative = 0.0;
                for (id, weight) in weights {
                    cumulative += weight;
                    if cumulative > roll {
                        return id;
                    }
                }
                utxo_ids[0]
            }
            SelectionMode::ClusterWeighted => {
                // weight = value × (max_factor - factor + 1) / max_factor
                // Lower cluster factor = higher weight (progressive)
                const MAX_FACTOR: f64 = 6.0;
                let weights: Vec<(u64, f64)> = utxo_ids
                    .iter()
                    .filter_map(|id| {
                        self.utxos.get(id).map(|u| {
                            let factor_bonus = (MAX_FACTOR - u.cluster_factor + 1.0) / MAX_FACTOR;
                            let weight = u.value as f64 * factor_bonus;
                            (*id, weight)
                        })
                    })
                    .collect();

                let total: f64 = weights.iter().map(|(_, w)| w).sum();
                if total <= 0.0 {
                    return utxo_ids[rng.gen_range(0..utxo_ids.len())];
                }
                let roll = rng.gen::<f64>() * total;
                let mut cumulative = 0.0;
                for (id, weight) in weights {
                    cumulative += weight;
                    if cumulative > roll {
                        return id;
                    }
                }
                utxo_ids[0]
            }
        }
    }

    /// Advance blocks using immediate distribution mode.
    pub fn advance_blocks_immediate(
        &mut self,
        blocks: u64,
        txs_per_block: u32,
        tx_model: TransactionModel,
    ) {
        for _ in 0..blocks {
            self.current_block += 1;

            for _ in 0..txs_per_block {
                // Most transactions have 2 outputs (payment + change)
                // Occasionally more (batched payments)
                let num_outputs = 2; // Simplified
                self.simulate_transaction_immediate(self.config.base_fee, num_outputs, tx_model);
            }

            // No periodic drawing needed - distribution is immediate
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

    /// Test PureValueWeighted model - the simplest possible lottery.
    ///
    /// tickets = value / cluster_factor
    ///
    /// Properties:
    /// - No state tracking (computed at draw time)
    /// - Sybil-resistant (splitting doesn't change total weight)
    /// - Wash-resistant (value unchanged by transacting, just pay fees)
    #[test]
    fn test_pure_value_weighted_model() {
        let total_wealth = 100_000_000u64;

        let config = LotteryConfig {
            base_fee: 100,
            drawing_interval: 10,
            ticket_model: TicketModel::PureValueWeighted,
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

        let initial_gini = sim.calculate_gini();
        sim.advance_blocks_with_model(30_000, 20, TransactionModel::ValueWeighted);
        let final_gini_vw = sim.calculate_gini();
        let fees_vw = sim.metrics.total_fees_collected;

        // Reset and test with uniform
        let mut sim2 = LotterySimulation::new(
            LotteryConfig {
                base_fee: 100,
                drawing_interval: 10,
                ticket_model: TicketModel::PureValueWeighted,
                ..LotteryConfig::default()
            },
            FeeCurve::default_params(),
        );
        for _ in 0..10 {
            sim2.add_owner(total_wealth / 200, SybilStrategy::Normal);
        }
        for _ in 0..5 {
            sim2.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal);
        }
        for _ in 0..2 {
            sim2.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal);
        }
        sim2.current_block = 1000;
        sim2.advance_blocks_with_model(30_000, 20, TransactionModel::Uniform);
        let final_gini_uniform = sim2.calculate_gini();
        let fees_uniform = sim2.metrics.total_fees_collected;

        eprintln!("\n=== PURE VALUE-WEIGHTED MODEL ===");
        eprintln!("Initial Gini: {:.4}", initial_gini);
        eprintln!("");
        eprintln!("With ValueWeighted transactions (rich transact more):");
        eprintln!("  Final Gini: {:.4}", final_gini_vw);
        eprintln!("  Reduction: {:.1}%", (initial_gini - final_gini_vw) / initial_gini * 100.0);
        eprintln!("  Fees: {}", fees_vw);
        eprintln!("");
        eprintln!("With Uniform transactions (everyone transacts equally):");
        eprintln!("  Final Gini: {:.4}", final_gini_uniform);
        eprintln!("  Reduction: {:.1}%", (initial_gini - final_gini_uniform) / initial_gini * 100.0);
        eprintln!("  Fees: {}", fees_uniform);
        eprintln!("==================================\n");

        // PureValueWeighted reduces inequality with value-weighted transactions
        assert!(final_gini_vw < initial_gini, "Should reduce Gini with value-weighted tx");

        // NOTE: PureValueWeighted INCREASES inequality with uniform transactions!
        // This is because tickets ∝ value/factor, so rich still have more tickets.
        // Under uniform tx, rich don't pay proportionally more fees to fund the pool.
        // Rich contribute ~25% of fees but win ~55% of drawings → net gain for rich.
        //
        // This is a KNOWN LIMITATION. PureValueWeighted only works when rich
        // transact proportionally more than poor (realistic in practice).
        assert!(
            final_gini_uniform > initial_gini * 0.95,
            "Expected PureValueWeighted to not significantly reduce Gini under uniform tx"
        );
    }

    /// Test wash trading has negative EV with PureValueWeighted.
    ///
    /// Key insight: Your lottery weight = value / cluster_factor
    /// Wash trading doesn't change your value (minus fees), so:
    /// - Weight change ≈ 0 (or slightly negative due to fees)
    /// - Cost = fees paid
    /// - EV = -fees (always negative)
    #[test]
    fn test_pure_value_weighted_wash_resistance() {
        let config = LotteryConfig {
            base_fee: 1000,
            drawing_interval: 1000,
            ticket_model: TicketModel::PureValueWeighted,
            ..LotteryConfig::default()
        };
        let fee_curve = FeeCurve::default_params();
        let mut sim = LotterySimulation::new(config, fee_curve);

        // Create a wash trader with significant wealth
        let trader_id = sim.add_owner(10_000_000, SybilStrategy::Normal);
        sim.add_owner(90_000_000, SybilStrategy::Normal); // Rest of economy
        sim.current_block = 1000;

        // Get initial state
        let trader_utxo_id = *sim.owners.get(&trader_id).unwrap().utxo_ids.first().unwrap();
        let initial_value = sim.utxos.get(&trader_utxo_id).unwrap().value;
        let initial_factor = sim.utxos.get(&trader_utxo_id).unwrap().cluster_factor;
        let initial_tickets = initial_value as f64 / initial_factor;

        eprintln!("\n=== PURE VALUE-WEIGHTED WASH RESISTANCE ===");
        eprintln!("Initial state:");
        eprintln!("  Value: {}", initial_value);
        eprintln!("  Factor: {:.4}", initial_factor);
        eprintln!("  Tickets (value/factor): {:.2}", initial_tickets);

        // Simulate 100 wash trades (sending to self)
        let wash_count = 100;
        let mut total_fees_paid = 0u64;

        for _ in 0..wash_count {
            // Find current UTXO for trader
            let utxo_id = *sim.owners.get(&trader_id).unwrap().utxo_ids.first().unwrap();
            let utxo = sim.utxos.get(&utxo_id).unwrap();
            let fee = (sim.config.base_fee as f64 * utxo.cluster_factor) as u64;
            total_fees_paid += fee;

            // Execute wash trade (self-send)
            // This destroys current UTXO and creates new one with value - fee
            let new_value = utxo.value.saturating_sub(fee);
            let new_id = sim.next_utxo_id;
            sim.next_utxo_id += 1;

            let new_utxo = LotteryUtxo::new(
                new_id,
                trader_id,
                new_value,
                utxo.cluster_factor, // Factor unchanged for self-send
                sim.current_block,
            );
            sim.utxos.remove(&utxo_id);
            sim.utxos.insert(new_id, new_utxo);
            sim.owners.get_mut(&trader_id).unwrap().utxo_ids = vec![new_id];
        }

        // Get final state
        let final_utxo_id = *sim.owners.get(&trader_id).unwrap().utxo_ids.first().unwrap();
        let final_value = sim.utxos.get(&final_utxo_id).unwrap().value;
        let final_factor = sim.utxos.get(&final_utxo_id).unwrap().cluster_factor;
        let final_tickets = final_value as f64 / final_factor;

        let ticket_change = final_tickets - initial_tickets;
        let ticket_change_pct = ticket_change / initial_tickets * 100.0;

        eprintln!("");
        eprintln!("After {} wash trades:", wash_count);
        eprintln!("  Value: {} (lost {})", final_value, initial_value - final_value);
        eprintln!("  Factor: {:.4} (unchanged)", final_factor);
        eprintln!("  Tickets: {:.2} ({:+.2}, {:+.2}%)", final_tickets, ticket_change, ticket_change_pct);
        eprintln!("  Fees paid: {}", total_fees_paid);
        eprintln!("");
        eprintln!("Result: Wash trader LOST tickets (EV = -fees)");
        eprintln!("============================================\n");

        // Verify wash trading reduced tickets (negative EV)
        assert!(
            final_tickets < initial_tickets,
            "Wash trading should reduce tickets (lost fees)"
        );
        assert!(
            (initial_value - final_value) == total_fees_paid,
            "Value loss should equal fees paid"
        );
    }

    /// Comprehensive comparison of all three ticket models.
    #[test]
    fn test_all_ticket_models() {
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

        // Run 8 scenarios: 4 ticket models × 2 transaction models
        let scenarios = [
            ("ActivityBased + ValueWeighted", TicketModel::ActivityBased, TransactionModel::ValueWeighted),
            ("ActivityBased + Uniform", TicketModel::ActivityBased, TransactionModel::Uniform),
            ("FeeProportional + ValueWeighted", TicketModel::FeeProportional, TransactionModel::ValueWeighted),
            ("FeeProportional + Uniform", TicketModel::FeeProportional, TransactionModel::Uniform),
            ("PureValueWeighted + ValueWeighted", TicketModel::PureValueWeighted, TransactionModel::ValueWeighted),
            ("PureValueWeighted + Uniform", TicketModel::PureValueWeighted, TransactionModel::Uniform),
            ("UniformPerUtxo + ValueWeighted", TicketModel::UniformPerUtxo, TransactionModel::ValueWeighted),
            ("UniformPerUtxo + Uniform", TicketModel::UniformPerUtxo, TransactionModel::Uniform),
        ];

        eprintln!("\n=== ALL TICKET MODELS COMPARISON ===");
        eprintln!("{:<40} {:>10} {:>10} {:>10} {:>15}", "Scenario", "Init Gini", "Final", "Change", "Fees");
        eprintln!("{}", "-".repeat(90));

        for (name, ticket_model, tx_model) in scenarios {
            let mut sim = create_sim(ticket_model);
            let initial_gini = sim.calculate_gini();
            sim.advance_blocks_with_model(30_000, 20, tx_model);
            let final_gini = sim.calculate_gini();
            let change_pct = (initial_gini - final_gini) / initial_gini * 100.0;

            eprintln!(
                "{:<40} {:>10.4} {:>10.4} {:>+9.1}% {:>15}",
                name,
                initial_gini,
                final_gini,
                change_pct,
                sim.metrics.total_fees_collected
            );
        }
        eprintln!("====================================\n");
    }

    /// Test UniformPerUtxo with per-output fee economics.
    ///
    /// Key insight: In an unequal landscape, uniform random selection favors
    /// the many (poor) over the few (rich). If we make UTXO creation expensive
    /// enough, splitting becomes unprofitable, and the natural distribution
    /// of UTXOs follows population, not wealth.
    ///
    /// This test verifies:
    /// 1. The break-even economics of UTXO splitting
    /// 2. Whether UniformPerUtxo reduces inequality without cluster tracking
    #[test]
    fn test_uniform_per_utxo_economics() {
        // Economic parameters
        let base_fee: u64 = 100;
        let per_output_fee: u64 = 50; // Extra fee per output beyond 2
        let pool_fraction = 0.8;
        let winners_per_drawing = 4;
        let drawings_per_period = 100; // e.g., 100 blocks

        eprintln!("\n=== UNIFORM PER UTXO ECONOMICS ===");
        eprintln!("Parameters:");
        eprintln!("  Base fee: {}", base_fee);
        eprintln!("  Per-output fee (beyond 2): {}", per_output_fee);
        eprintln!("  Pool fraction: {:.0}%", pool_fraction * 100.0);
        eprintln!("  Winners per drawing: {}", winners_per_drawing);
        eprintln!("");

        // Simulate a system with varying UTXO counts
        for total_utxos in [1_000, 10_000, 100_000, 1_000_000u64] {
            // Assume each transaction creates ~1.5 UTXOs on average
            // and pool is funded from transaction fees
            let txs_per_period = total_utxos as f64 / 1.5 * 0.1; // 10% turnover
            let pool_per_period = txs_per_period * base_fee as f64 * pool_fraction;
            let prize_per_winner = pool_per_period / (drawings_per_period * winners_per_drawing) as f64;

            // Expected winnings per UTXO per period
            let win_prob_per_drawing = winners_per_drawing as f64 / total_utxos as f64;
            let expected_winnings = win_prob_per_drawing * prize_per_winner * drawings_per_period as f64;

            // Cost to create one extra UTXO (splitting)
            let split_cost = per_output_fee as f64;

            // Break-even analysis
            let periods_to_break_even = if expected_winnings > 0.0 {
                split_cost / expected_winnings
            } else {
                f64::INFINITY
            };

            eprintln!(
                "UTXOs: {:>10} | Prize/winner: {:>8.2} | EV/UTXO: {:>8.4} | Cost: {:>4} | Break-even: {:>8.1} periods",
                total_utxos,
                prize_per_winner,
                expected_winnings,
                per_output_fee,
                periods_to_break_even
            );
        }

        eprintln!("");
        eprintln!("Interpretation:");
        eprintln!("  If break-even > holding period, splitting is unprofitable.");
        eprintln!("  With large UTXO counts, expected value per UTXO is tiny.");
        eprintln!("  Per-output fees make splitting costly relative to expected winnings.");
        eprintln!("");

        // Now simulate the actual redistribution effect
        let total_wealth = 100_000_000u64;
        let config = LotteryConfig {
            base_fee: 100,
            drawing_interval: 10,
            ticket_model: TicketModel::UniformPerUtxo,
            ..LotteryConfig::default()
        };
        let fee_curve = FeeCurve::default_params();

        // Key test: create population where poor OUTNUMBER rich significantly
        // 100 poor people (100 BTH each = 10,000 total = 0.01%)
        // 10 middle people (100,000 BTH each = 1,000,000 total = 1%)
        // 1 rich person (98,990,000 BTH = 98.99%)
        let mut sim = LotterySimulation::new(config.clone(), fee_curve.clone());

        // Add 100 poor people
        for _ in 0..100 {
            sim.add_owner(100, SybilStrategy::Normal);
        }
        // Add 10 middle class
        for _ in 0..10 {
            sim.add_owner(100_000, SybilStrategy::Normal);
        }
        // Add 1 ultra-rich
        sim.add_owner(98_990_000, SybilStrategy::Normal);

        sim.current_block = 1000;

        let initial_gini = sim.calculate_gini();
        let initial_utxo_count = sim.utxos.len();

        eprintln!("Population simulation:");
        eprintln!("  100 poor (100 BTH each) = 0.01% of wealth, 90% of population");
        eprintln!("  10 middle (100K BTH each) = 1% of wealth, 9% of population");
        eprintln!("  1 rich (99M BTH) = 99% of wealth, 1% of population");
        eprintln!("  Initial UTXOs: {}", initial_utxo_count);
        eprintln!("  Initial Gini: {:.4}", initial_gini);
        eprintln!("");

        // Run simulation with uniform transaction pattern (everyone transacts equally)
        // This is the KEY test - uniform transactions, uniform lottery
        sim.advance_blocks_with_model(50_000, 20, TransactionModel::Uniform);

        let final_gini = sim.calculate_gini();
        let final_utxo_count = sim.utxos.len();
        let gini_change = initial_gini - final_gini;
        let gini_change_pct = gini_change / initial_gini * 100.0;

        eprintln!("After 50,000 blocks with uniform transactions:");
        eprintln!("  Final UTXOs: {}", final_utxo_count);
        eprintln!("  Final Gini: {:.4}", final_gini);
        eprintln!("  Gini change: {:+.4} ({:+.1}%)", gini_change, gini_change_pct);
        eprintln!("  Fees collected: {}", sim.metrics.total_fees_collected);
        eprintln!("  Pool distributed: {}", sim.metrics.total_distributed);
        eprintln!("");

        // The hypothesis: UniformPerUtxo with uniform transactions should be
        // progressive because 90% of UTXOs belong to poor people (by count).
        //
        // Each UTXO has equal chance of winning. Poor have 100 UTXOs,
        // rich has 1 UTXO. Poor win 100/111 = 90% of drawings!
        //
        // Meanwhile, fees might be paid more by the rich (higher value txs).
        // Net effect: redistribution from rich to poor.

        if gini_change > 0.0 {
            eprintln!("SUCCESS: UniformPerUtxo reduced inequality!");
            eprintln!("  This works because random UTXO selection favors the many (poor).");
        } else {
            eprintln!("NOTE: UniformPerUtxo did not reduce inequality in this scenario.");
            eprintln!("  This could be due to:");
            eprintln!("  - Rich fragmenting into many UTXOs through transactions");
            eprintln!("  - Fee structure not penalizing fragmentation enough");
        }
        eprintln!("================================\n");

        // The test passes regardless - we're gathering data about behavior
    }

    /// Test the "4 random winners per transaction" model.
    ///
    /// Instead of accumulating a pool and drawing periodically,
    /// each transaction immediately distributes some of its fee
    /// to 4 randomly selected UTXOs.
    #[test]
    fn test_immediate_distribution_model() {
        eprintln!("\n=== IMMEDIATE DISTRIBUTION MODEL ===");
        eprintln!("Each transaction picks 4 random UTXOs to receive a share of fees.");
        eprintln!("");

        // This is a simplified model:
        // - Transaction pays fee F
        // - 80% of F is distributed to 4 random UTXOs (20% each)
        // - 20% of F is burned

        let fee = 1000u64;
        let distribution_fraction = 0.8;
        let winners_per_tx = 4;
        let per_winner = (fee as f64 * distribution_fraction) / winners_per_tx as f64;

        eprintln!("Per transaction:");
        eprintln!("  Fee: {}", fee);
        eprintln!("  Distributed (80%): {}", (fee as f64 * distribution_fraction) as u64);
        eprintln!("  Per winner (4 winners): {:.0}", per_winner);
        eprintln!("  Burned (20%): {}", (fee as f64 * 0.2) as u64);
        eprintln!("");

        // Expected value analysis for different UTXO counts
        for total_utxos in [100, 1000, 10000, 100000u64] {
            // Probability of being selected as one of 4 winners
            // Approximation: 4/N (assuming sampling with replacement or N >> 4)
            let win_prob = 4.0 / total_utxos as f64;
            let expected_per_tx = win_prob * per_winner;

            eprintln!(
                "UTXOs: {:>6} | Win prob: {:.6} | EV per tx: {:.4}",
                total_utxos, win_prob, expected_per_tx
            );
        }

        eprintln!("");
        eprintln!("Key insight:");
        eprintln!("  With many UTXOs, expected value per UTXO per transaction is tiny.");
        eprintln!("  Creating an extra UTXO costs a full fee but gains tiny EV.");
        eprintln!("  Therefore splitting is unprofitable with large UTXO counts.");
        eprintln!("");

        // Now let's verify the break-even
        let total_utxos = 10000u64;
        let txs_per_day = 10000u64; // Example
        let win_prob = 4.0 / total_utxos as f64;
        let ev_per_tx = win_prob * per_winner;
        let ev_per_day = ev_per_tx * txs_per_day as f64;

        eprintln!("Break-even analysis (10,000 UTXOs, 10,000 tx/day):");
        eprintln!("  EV per UTXO per day: {:.2}", ev_per_day);
        eprintln!("  Cost to create UTXO: {}", fee);
        eprintln!("  Days to break even: {:.1}", fee as f64 / ev_per_day);

        // If break-even is long enough, splitting isn't worth it
        let days_to_break_even = fee as f64 / ev_per_day;
        if days_to_break_even > 30.0 {
            eprintln!("  -> Splitting takes >30 days to break even: UNPROFITABLE for most users");
        } else {
            eprintln!("  -> Splitting breaks even in <30 days: NEEDS HIGHER FEES");
        }
        eprintln!("====================================\n");
    }

    /// Test the combined design:
    /// 1. Cluster-factor fees (progressive taxation)
    /// 2. Superlinear per-output fees (anti-Sybil)
    /// 3. Immediate random distribution to 4 UTXOs (simple lottery)
    /// 4. No cluster tracking for lottery eligibility (simplicity)
    #[test]
    fn test_combined_design() {
        eprintln!("\n=== COMBINED DESIGN TEST ===");
        eprintln!("Design:");
        eprintln!("  1. Fee = base × cluster_factor × outputs^2 (progressive + anti-split)");
        eprintln!("  2. 80% immediately distributed to 4 random UTXOs");
        eprintln!("  3. 20% burned");
        eprintln!("  4. Uniform UTXO selection (no cluster tracking for lottery)");
        eprintln!("");

        let total_wealth = 100_000_000u64;

        // Test with different transaction patterns
        for (name, tx_model) in [
            ("ValueWeighted (rich transact more)", TransactionModel::ValueWeighted),
            ("Uniform (everyone transacts equally)", TransactionModel::Uniform),
        ] {
            let config = LotteryConfig {
                base_fee: 100,
                pool_fraction: 0.8,
                distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                output_fee_exponent: 2.0, // Quadratic: 2 outputs = 4×, 3 outputs = 9×
                min_utxo_value: 0, // No minimum - everyone can participate
                ..LotteryConfig::default()
            };
            let fee_curve = FeeCurve::default_params();
            let mut sim = LotterySimulation::new(config, fee_curve);

            // Create unequal population:
            // 10 poor (0.5% each = 5% total), 5 middle (5% each = 25%), 2 rich (35% each = 70%)
            for _ in 0..10 {
                sim.add_owner(total_wealth / 200, SybilStrategy::Normal); // 500,000 each
            }
            for _ in 0..5 {
                sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal); // 5,000,000 each
            }
            for _ in 0..2 {
                sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal); // 35,000,000 each
            }

            sim.current_block = 1000;

            let initial_gini = sim.calculate_gini();
            let initial_utxos = sim.utxos.len();

            // Run simulation with immediate distribution
            sim.advance_blocks_immediate(30_000, 20, tx_model);

            let final_gini = sim.calculate_gini();
            let final_utxos = sim.utxos.len();
            let gini_change = initial_gini - final_gini;
            let gini_change_pct = gini_change / initial_gini * 100.0;

            eprintln!("{}", name);
            eprintln!("  Initial: Gini={:.4}, UTXOs={}", initial_gini, initial_utxos);
            eprintln!("  Final:   Gini={:.4}, UTXOs={}", final_gini, final_utxos);
            eprintln!("  Change:  {:+.4} ({:+.1}%)", gini_change, gini_change_pct);
            eprintln!("  Fees collected: {}", sim.metrics.total_fees_collected);
            eprintln!("  Distributed: {}", sim.metrics.total_distributed);
            eprintln!("  Burned: {}", sim.total_burned);
            eprintln!("");
        }

        // Now test with more extreme inequality to see population statistics effect
        eprintln!("--- Extreme Inequality Test ---");
        eprintln!("Population: 100 poor (0.01% each), 10 middle (1% each), 1 ultra-rich (89%)");
        eprintln!("");

        for (name, tx_model) in [
            ("ValueWeighted", TransactionModel::ValueWeighted),
            ("Uniform", TransactionModel::Uniform),
        ] {
            let config = LotteryConfig {
                base_fee: 100,
                pool_fraction: 0.8,
                distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                output_fee_exponent: 2.0,
                min_utxo_value: 0,
                ..LotteryConfig::default()
            };
            let fee_curve = FeeCurve::default_params();
            let mut sim = LotterySimulation::new(config, fee_curve);

            // 100 poor: 10,000 BTH each (0.01% each, 1% total)
            for _ in 0..100 {
                sim.add_owner(10_000, SybilStrategy::Normal);
            }
            // 10 middle: 1,000,000 BTH each (1% each, 10% total)
            for _ in 0..10 {
                sim.add_owner(1_000_000, SybilStrategy::Normal);
            }
            // 1 ultra-rich: 89,000,000 BTH (89% of total)
            sim.add_owner(89_000_000, SybilStrategy::Normal);

            sim.current_block = 1000;

            let initial_gini = sim.calculate_gini();

            // Key insight: 111 people, 111 UTXOs initially
            // Poor have 100 UTXOs (90%), middle have 10 (9%), rich has 1 (1%)
            // Random selection should heavily favor the poor!

            sim.advance_blocks_immediate(50_000, 20, tx_model);

            let final_gini = sim.calculate_gini();
            let gini_change = initial_gini - final_gini;
            let gini_change_pct = gini_change / initial_gini * 100.0;

            eprintln!("{}: Gini {:.4} → {:.4} ({:+.1}%)",
                name, initial_gini, final_gini, gini_change_pct);
        }

        eprintln!("");
        eprintln!("================================\n");
    }

    /// Test superlinear fees discourage output splitting.
    #[test]
    fn test_superlinear_output_fees() {
        eprintln!("\n=== SUPERLINEAR OUTPUT FEE TEST ===");
        eprintln!("Fee = base × factor × outputs^exponent");
        eprintln!("");

        let base = 100u64;
        let factor = 3.0; // Medium cluster factor

        for exponent in [1.0, 1.5, 2.0, 3.0] {
            eprintln!("Exponent: {}", exponent);
            for outputs in 1..=10u32 {
                let multiplier = (outputs as f64).powf(exponent);
                let fee = (base as f64 * factor * multiplier) as u64;
                let per_output = fee / outputs as u64;
                eprintln!(
                    "  {} outputs: fee={:>6}, per_output={:>5}",
                    outputs, fee, per_output
                );
            }
            eprintln!("");
        }

        // With exponent=2.0:
        // 2 outputs: 100 × 3 × 4 = 1200 total, 600 per output
        // 10 outputs: 100 × 3 × 100 = 30000 total, 3000 per output
        // Cost per output scales 5× for going from 2→10 outputs

        eprintln!("Key insight: With exponent=2.0, creating 10 outputs costs");
        eprintln!("5× more per output than creating 2 outputs.");
        eprintln!("This makes splitting prohibitively expensive.");
        eprintln!("================================\n");
    }

    /// Compare all designs head-to-head.
    #[test]
    fn test_design_comparison() {
        eprintln!("\n=== COMPREHENSIVE DESIGN COMPARISON ===");
        eprintln!("");

        let total_wealth = 100_000_000u64;

        // Create a standardized population for fair comparison
        let create_population = |sim: &mut LotterySimulation| {
            // 10 poor (0.5% each), 5 middle (5% each), 2 rich (35% each)
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
        };

        eprintln!("{:<50} {:>10} {:>10} {:>10}",
            "Design", "Init Gini", "Final", "Change");
        eprintln!("{}", "-".repeat(85));

        // Test designs under ValueWeighted transactions
        let tx_model = TransactionModel::ValueWeighted;

        // 1. Pooled + FeeProportional (our previous best robust design)
        {
            let config = LotteryConfig {
                base_fee: 100,
                drawing_interval: 10,
                ticket_model: TicketModel::FeeProportional,
                distribution_mode: DistributionMode::Pooled,
                ..LotteryConfig::default()
            };
            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
            create_population(&mut sim);
            let initial = sim.calculate_gini();
            sim.advance_blocks_with_model(30_000, 20, tx_model);
            let final_gini = sim.calculate_gini();
            let change = (initial - final_gini) / initial * 100.0;
            eprintln!("{:<50} {:>10.4} {:>10.4} {:>+9.1}%",
                "Pooled + FeeProportional", initial, final_gini, change);
        }

        // 2. Pooled + PureValueWeighted
        {
            let config = LotteryConfig {
                base_fee: 100,
                drawing_interval: 10,
                ticket_model: TicketModel::PureValueWeighted,
                distribution_mode: DistributionMode::Pooled,
                ..LotteryConfig::default()
            };
            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
            create_population(&mut sim);
            let initial = sim.calculate_gini();
            sim.advance_blocks_with_model(30_000, 20, tx_model);
            let final_gini = sim.calculate_gini();
            let change = (initial - final_gini) / initial * 100.0;
            eprintln!("{:<50} {:>10.4} {:>10.4} {:>+9.1}%",
                "Pooled + PureValueWeighted", initial, final_gini, change);
        }

        // 3. Pooled + UniformPerUtxo (the statistical approach)
        {
            let config = LotteryConfig {
                base_fee: 100,
                drawing_interval: 10,
                ticket_model: TicketModel::UniformPerUtxo,
                distribution_mode: DistributionMode::Pooled,
                min_utxo_value: 0,
                ..LotteryConfig::default()
            };
            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
            create_population(&mut sim);
            let initial = sim.calculate_gini();
            sim.advance_blocks_with_model(30_000, 20, tx_model);
            let final_gini = sim.calculate_gini();
            let change = (initial - final_gini) / initial * 100.0;
            eprintln!("{:<50} {:>10.4} {:>10.4} {:>+9.1}%",
                "Pooled + UniformPerUtxo", initial, final_gini, change);
        }

        // 4. COMBINED: Immediate + Uniform + Superlinear fees
        {
            let config = LotteryConfig {
                base_fee: 100,
                pool_fraction: 0.8,
                distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                output_fee_exponent: 2.0,
                min_utxo_value: 0,
                ..LotteryConfig::default()
            };
            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
            create_population(&mut sim);
            let initial = sim.calculate_gini();
            sim.advance_blocks_immediate(30_000, 20, tx_model);
            let final_gini = sim.calculate_gini();
            let change = (initial - final_gini) / initial * 100.0;
            eprintln!("{:<50} {:>10.4} {:>10.4} {:>+9.1}%",
                "COMBINED: Immediate + Uniform + Superlinear", initial, final_gini, change);
        }

        eprintln!("");
        eprintln!("Note: All tests use ValueWeighted transactions (realistic scenario)");
        eprintln!("================================\n");
    }

    // ============================================================================
    // STRESS TESTS - Validating Realistic Assumptions
    // ============================================================================

    /// Test convergence rate under realistic transaction volumes.
    ///
    /// The "100% Gini reduction" result may be misleading if it requires
    /// unrealistic numbers of transactions. This test measures:
    /// 1. How many transactions to achieve meaningful (10%, 25%, 50%) Gini reduction
    /// 2. Whether convergence is realistic at typical blockchain tx rates
    #[test]
    fn test_time_to_equilibrium() {
        eprintln!("\n=== TIME TO EQUILIBRIUM STRESS TEST ===");
        eprintln!("Measuring convergence rate under realistic tx volumes.");
        eprintln!("");

        let total_wealth = 100_000_000u64;

        // Realistic blockchain parameters
        let block_time_seconds = 5; // 5 second blocks
        let txs_per_block = 50; // ~10 tx/second
        let blocks_per_day = 86400 / block_time_seconds;
        let txs_per_day = txs_per_block as u64 * blocks_per_day;

        eprintln!("Network parameters:");
        eprintln!("  Block time: {}s", block_time_seconds);
        eprintln!("  Txs per block: {}", txs_per_block);
        eprintln!("  Txs per day: {}", txs_per_day);
        eprintln!("");

        let config = LotteryConfig {
            base_fee: 100,
            pool_fraction: 0.8,
            distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
            output_fee_exponent: 2.0,
            min_utxo_value: 0,
            ..LotteryConfig::default()
        };

        let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

        // Create population: 10 poor, 5 middle, 2 rich
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

        let initial_gini = sim.calculate_gini();
        let target_10pct = initial_gini * 0.90;
        let target_25pct = initial_gini * 0.75;
        let target_50pct = initial_gini * 0.50;

        let mut reached_10pct = None;
        let mut reached_25pct = None;
        let mut reached_50pct = None;

        eprintln!("Initial Gini: {:.4}", initial_gini);
        eprintln!("Targets: 10%={:.4}, 25%={:.4}, 50%={:.4}", target_10pct, target_25pct, target_50pct);
        eprintln!("");

        // Simulate up to 1 year (in blocks)
        let blocks_per_year = blocks_per_day * 365;
        let check_interval = blocks_per_day; // Check once per simulated day

        let mut total_txs = 0u64;
        let mut days = 0u64;

        for day in 0..365 {
            sim.advance_blocks_immediate(check_interval, txs_per_block, TransactionModel::ValueWeighted);
            total_txs += check_interval * txs_per_block as u64;
            days = day + 1;

            let current_gini = sim.calculate_gini();

            if reached_10pct.is_none() && current_gini <= target_10pct {
                reached_10pct = Some((days, total_txs, current_gini));
                eprintln!("  10% reduction at day {}: Gini={:.4}, txs={}", days, current_gini, total_txs);
            }
            if reached_25pct.is_none() && current_gini <= target_25pct {
                reached_25pct = Some((days, total_txs, current_gini));
                eprintln!("  25% reduction at day {}: Gini={:.4}, txs={}", days, current_gini, total_txs);
            }
            if reached_50pct.is_none() && current_gini <= target_50pct {
                reached_50pct = Some((days, total_txs, current_gini));
                eprintln!("  50% reduction at day {}: Gini={:.4}, txs={}", days, current_gini, total_txs);
            }

            // Early exit if we've reached all targets
            if reached_50pct.is_some() {
                break;
            }

            // Progress indicator every 30 days
            if day % 30 == 29 {
                eprintln!("  Day {}: Gini={:.4}", days, current_gini);
            }
        }

        let final_gini = sim.calculate_gini();
        let total_reduction = (initial_gini - final_gini) / initial_gini * 100.0;

        eprintln!("");
        eprintln!("Final results after {} days ({} transactions):", days, total_txs);
        eprintln!("  Final Gini: {:.4}", final_gini);
        eprintln!("  Total reduction: {:.1}%", total_reduction);
        eprintln!("");

        // Analysis
        eprintln!("ANALYSIS:");
        if let Some((d, txs, _)) = reached_10pct {
            eprintln!("  10% reduction: {} days, {} txs", d, txs);
        } else {
            eprintln!("  10% reduction: NOT REACHED in {} days!", days);
        }
        if let Some((d, txs, _)) = reached_25pct {
            eprintln!("  25% reduction: {} days, {} txs", d, txs);
        } else {
            eprintln!("  25% reduction: NOT REACHED in {} days!", days);
        }
        if let Some((d, txs, _)) = reached_50pct {
            eprintln!("  50% reduction: {} days, {} txs", d, txs);
        } else {
            eprintln!("  50% reduction: NOT REACHED in {} days!", days);
        }

        eprintln!("");
        if reached_10pct.is_none() {
            eprintln!("WARNING: Even 10% Gini reduction not achieved in 1 year!");
            eprintln!("The lottery mechanism may be too slow for practical use.");
        } else if reached_25pct.is_none() {
            eprintln!("NOTE: Modest reduction achieved, but 25%+ takes >1 year.");
        } else {
            eprintln!("OK: Meaningful redistribution occurs within reasonable time.");
        }
        eprintln!("======================================\n");
    }

    /// Test slow UTXO accumulation gaming strategy.
    ///
    /// A patient attacker might accumulate many UTXOs through normal transactions
    /// over time, without paying splitting fees. This could game the lottery.
    ///
    /// Scenario: Attacker starts with 1 UTXO, conducts normal transactions,
    /// but always keeps change (accumulating UTXOs). After a year, do they
    /// have an unfair lottery advantage?
    #[test]
    fn test_slow_utxo_accumulation_gaming() {
        eprintln!("\n=== SLOW UTXO ACCUMULATION GAMING TEST ===");
        eprintln!("Can a patient attacker game the lottery by accumulating UTXOs?");
        eprintln!("");

        let total_wealth = 100_000_000u64;
        let attacker_wealth = total_wealth * 10 / 100; // 10% of total

        // Scenario: 1 attacker (10% wealth), 90 normal users (1% each)
        let config = LotteryConfig {
            base_fee: 100,
            pool_fraction: 0.8,
            distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
            output_fee_exponent: 2.0,
            min_utxo_value: 0,
            ..LotteryConfig::default()
        };

        // Baseline: Everyone behaves normally (1 UTXO each)
        let mut baseline_sim = LotterySimulation::new(config.clone(), FeeCurve::default_params());
        let attacker_id_baseline = baseline_sim.add_owner(attacker_wealth, SybilStrategy::Normal);
        for _ in 0..90 {
            baseline_sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
        }
        baseline_sim.current_block = 1000;

        let baseline_initial_utxos = baseline_sim.utxos.len();
        let baseline_attacker_utxos_initial = baseline_sim.owners.get(&attacker_id_baseline)
            .map(|o| o.utxo_ids.len()).unwrap_or(0);

        // Run simulation
        baseline_sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);

        let baseline_attacker_winnings = baseline_sim.owners.get(&attacker_id_baseline)
            .map(|o| o.total_winnings).unwrap_or(0);
        let baseline_attacker_fees = baseline_sim.owners.get(&attacker_id_baseline)
            .map(|o| o.total_fees_paid).unwrap_or(0);
        let baseline_final_utxos = baseline_sim.utxos.len();

        eprintln!("BASELINE (normal behavior):");
        eprintln!("  Attacker starts with: {} UTXOs", baseline_attacker_utxos_initial);
        eprintln!("  System UTXOs: {} -> {}", baseline_initial_utxos, baseline_final_utxos);
        eprintln!("  Attacker winnings: {}", baseline_attacker_winnings);
        eprintln!("  Attacker fees paid: {}", baseline_attacker_fees);
        eprintln!("  Attacker net: {:+}", baseline_attacker_winnings as i64 - baseline_attacker_fees as i64);
        eprintln!("");

        // Gaming scenario: Attacker fragments wealth into many UTXOs upfront
        // Using MaximizeSplit strategy
        let mut gaming_sim = LotterySimulation::new(config.clone(), FeeCurve::default_params());

        // Attacker uses MultiAccount to split into 10 UTXOs
        let attacker_id_gaming = gaming_sim.add_owner(attacker_wealth, SybilStrategy::MultiAccount { num_accounts: 10 });
        for _ in 0..90 {
            gaming_sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
        }
        gaming_sim.current_block = 1000;

        let gaming_initial_utxos = gaming_sim.utxos.len();
        let gaming_attacker_utxos_initial = gaming_sim.owners.get(&attacker_id_gaming)
            .map(|o| o.utxo_ids.len()).unwrap_or(0);

        // Run same simulation
        gaming_sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);

        let gaming_attacker_winnings = gaming_sim.owners.get(&attacker_id_gaming)
            .map(|o| o.total_winnings).unwrap_or(0);
        let gaming_attacker_fees = gaming_sim.owners.get(&attacker_id_gaming)
            .map(|o| o.total_fees_paid).unwrap_or(0);
        let gaming_final_utxos = gaming_sim.utxos.len();

        eprintln!("GAMING (10x UTXO split upfront):");
        eprintln!("  Attacker starts with: {} UTXOs", gaming_attacker_utxos_initial);
        eprintln!("  System UTXOs: {} -> {}", gaming_initial_utxos, gaming_final_utxos);
        eprintln!("  Attacker winnings: {}", gaming_attacker_winnings);
        eprintln!("  Attacker fees paid: {}", gaming_attacker_fees);
        eprintln!("  Attacker net: {:+}", gaming_attacker_winnings as i64 - gaming_attacker_fees as i64);
        eprintln!("");

        // Compare
        let winnings_ratio = gaming_attacker_winnings as f64 / baseline_attacker_winnings.max(1) as f64;
        let net_baseline = baseline_attacker_winnings as i64 - baseline_attacker_fees as i64;
        let net_gaming = gaming_attacker_winnings as i64 - gaming_attacker_fees as i64;

        eprintln!("COMPARISON:");
        eprintln!("  Winnings ratio (gaming/baseline): {:.2}x", winnings_ratio);
        eprintln!("  Net result baseline: {:+}", net_baseline);
        eprintln!("  Net result gaming: {:+}", net_gaming);
        eprintln!("");

        if net_gaming > net_baseline * 2 {
            eprintln!("WARNING: Gaming strategy gives >2x advantage!");
            eprintln!("The lottery is vulnerable to UTXO accumulation.");
        } else if net_gaming > net_baseline {
            eprintln!("CAUTION: Gaming has modest advantage ({:.1}x)", net_gaming as f64 / net_baseline.max(1) as f64);
        } else {
            eprintln!("OK: Gaming does not provide significant advantage.");
        }
        eprintln!("==========================================\n");
    }

    /// Test impact of exchange-like entities.
    ///
    /// Exchanges hold funds for millions of users in relatively few UTXOs.
    /// This breaks the "1 UTXO = 1 person" assumption central to the
    /// population statistics insight.
    ///
    /// Scenario: 50% of all funds are held by 3 exchanges (few UTXOs),
    /// while 50% are held by 1000 retail users (many UTXOs).
    /// Does redistribution flow to exchanges instead of actual poor users?
    #[test]
    fn test_exchange_entity_impact() {
        eprintln!("\n=== EXCHANGE ENTITY IMPACT TEST ===");
        eprintln!("Testing: Does lottery redistribution benefit exchanges?");
        eprintln!("");
        eprintln!("Scenario:");
        eprintln!("  3 exchanges holding 50% of funds (few UTXOs)");
        eprintln!("  1000 retail users holding 50% (many UTXOs)");
        eprintln!("");

        let total_wealth = 100_000_000u64;
        let exchange_wealth = total_wealth / 2; // 50% in exchanges
        let retail_wealth = total_wealth / 2;   // 50% in retail

        let config = LotteryConfig {
            base_fee: 100,
            pool_fraction: 0.8,
            distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
            output_fee_exponent: 2.0,
            min_utxo_value: 0,
            ..LotteryConfig::default()
        };

        let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

        // 3 exchanges: ~16.67% each (but only 1 UTXO each)
        let exchange_ids: Vec<u64> = (0..3)
            .map(|_| sim.add_owner(exchange_wealth / 3, SybilStrategy::Normal))
            .collect();

        // 1000 retail users: 0.05% each (1 UTXO each)
        let retail_ids: Vec<u64> = (0..1000)
            .map(|_| sim.add_owner(retail_wealth / 1000, SybilStrategy::Normal))
            .collect();

        sim.current_block = 1000;

        let initial_utxos = sim.utxos.len();
        let initial_gini = sim.calculate_gini();

        // Track initial wealth
        let initial_exchange_wealth: u64 = exchange_ids.iter()
            .map(|id| sim.owner_value(*id))
            .sum();
        let initial_retail_wealth: u64 = retail_ids.iter()
            .map(|id| sim.owner_value(*id))
            .sum();

        eprintln!("Initial state:");
        eprintln!("  Total UTXOs: {}", initial_utxos);
        eprintln!("  Exchange UTXOs: 3 (holding {})", initial_exchange_wealth);
        eprintln!("  Retail UTXOs: 1000 (holding {})", initial_retail_wealth);
        eprintln!("  Gini: {:.4}", initial_gini);
        eprintln!("");

        // Run simulation with value-weighted transactions
        // (Exchanges transact more because they hold more value)
        sim.advance_blocks_immediate(30_000, 20, TransactionModel::ValueWeighted);

        let final_gini = sim.calculate_gini();
        let final_utxos = sim.utxos.len();

        // Calculate final wealth
        let final_exchange_wealth: u64 = exchange_ids.iter()
            .map(|id| sim.owner_value(*id))
            .sum();
        let final_retail_wealth: u64 = retail_ids.iter()
            .map(|id| sim.owner_value(*id))
            .sum();

        let exchange_change = final_exchange_wealth as i64 - initial_exchange_wealth as i64;
        let retail_change = final_retail_wealth as i64 - initial_retail_wealth as i64;

        // Track winnings and fees separately
        let exchange_winnings: u64 = exchange_ids.iter()
            .filter_map(|id| sim.owners.get(id))
            .map(|o| o.total_winnings)
            .sum();
        let exchange_fees: u64 = exchange_ids.iter()
            .filter_map(|id| sim.owners.get(id))
            .map(|o| o.total_fees_paid)
            .sum();

        let retail_winnings: u64 = retail_ids.iter()
            .filter_map(|id| sim.owners.get(id))
            .map(|o| o.total_winnings)
            .sum();
        let retail_fees: u64 = retail_ids.iter()
            .filter_map(|id| sim.owners.get(id))
            .map(|o| o.total_fees_paid)
            .sum();

        eprintln!("Final state:");
        eprintln!("  Total UTXOs: {} ({:+})", final_utxos, final_utxos as i64 - initial_utxos as i64);
        eprintln!("  Gini: {:.4} ({:+.1}% change)", final_gini, (initial_gini - final_gini) / initial_gini * 100.0);
        eprintln!("");
        eprintln!("Exchange entities (3 entities, 3 UTXOs):");
        eprintln!("  Wealth: {} -> {} ({:+})", initial_exchange_wealth, final_exchange_wealth, exchange_change);
        eprintln!("  Fees paid: {}", exchange_fees);
        eprintln!("  Lottery winnings: {}", exchange_winnings);
        eprintln!("  Net: {:+}", exchange_winnings as i64 - exchange_fees as i64);
        eprintln!("");
        eprintln!("Retail users (1000 entities, 1000 UTXOs):");
        eprintln!("  Wealth: {} -> {} ({:+})", initial_retail_wealth, final_retail_wealth, retail_change);
        eprintln!("  Fees paid: {}", retail_fees);
        eprintln!("  Lottery winnings: {}", retail_winnings);
        eprintln!("  Net: {:+}", retail_winnings as i64 - retail_fees as i64);
        eprintln!("");

        // Key question: Did retail users (the "many") win proportionally more?
        // Retail has 1000 UTXOs vs 3 for exchanges = 333x more UTXOs
        // If lottery is purely uniform, retail should win 1000/1003 = 99.7%
        let total_winnings = exchange_winnings + retail_winnings;
        let retail_win_fraction = retail_winnings as f64 / total_winnings.max(1) as f64;
        let expected_fraction = 1000.0 / 1003.0; // Based on UTXO count

        eprintln!("ANALYSIS:");
        eprintln!("  Retail UTXO fraction: {:.1}% (1000/1003)", 1000.0 / 1003.0 * 100.0);
        eprintln!("  Retail winnings fraction: {:.1}%", retail_win_fraction * 100.0);
        eprintln!("  Expected (if uniform): {:.1}%", expected_fraction * 100.0);
        eprintln!("");

        if retail_win_fraction < 0.8 {
            eprintln!("WARNING: Retail users receiving <80% of lottery winnings!");
            eprintln!("Exchange entities are capturing disproportionate value.");
        } else if (retail_win_fraction - expected_fraction).abs() > 0.1 {
            eprintln!("CAUTION: Distribution deviates significantly from expected.");
        } else {
            eprintln!("OK: Distribution roughly matches UTXO proportions.");
        }

        // But the real issue: exchanges transact on behalf of users
        // Those fees come from user funds, but winnings go to the exchange
        eprintln!("");
        eprintln!("KEY INSIGHT:");
        eprintln!("  Exchanges paid {} in fees (from user funds)", exchange_fees);
        eprintln!("  Exchanges won {} from lottery", exchange_winnings);
        eprintln!("  This is a {} redistribution to exchanges",
            if exchange_winnings > exchange_fees { "NET POSITIVE" } else { "net negative" });
        eprintln!("");

        if exchange_winnings < exchange_fees && retail_winnings > retail_fees {
            eprintln!("POSITIVE: Lottery redistributes FROM exchanges TO retail!");
        } else if exchange_winnings > exchange_fees * 2 {
            eprintln!("NEGATIVE: Exchanges are gaining from the lottery system.");
        }
        eprintln!("==========================================\n");
    }

    /// Test break-even dynamics under various UTXO population sizes.
    ///
    /// The claim that "splitting takes a year to break even" needs validation
    /// across different network sizes and fee levels.
    #[test]
    fn test_breakeven_dynamics() {
        eprintln!("\n=== BREAK-EVEN DYNAMICS TEST ===");
        eprintln!("Testing whether UTXO splitting profitability claims hold.");
        eprintln!("");

        // Parameters from the design doc
        let base_fee = 100u64;
        let pool_fraction = 0.8;
        let winners_per_tx = 4;
        let burn_fraction = 0.2;

        // Various network sizes
        let scenarios = [
            ("Small network", 1_000u64, 100),     // 1K UTXOs, 100 tx/day
            ("Medium network", 10_000u64, 1_000), // 10K UTXOs, 1K tx/day
            ("Large network", 100_000u64, 10_000), // 100K UTXOs, 10K tx/day
            ("Massive network", 1_000_000u64, 100_000), // 1M UTXOs, 100K tx/day
        ];

        eprintln!("{:<20} {:>12} {:>12} {:>15} {:>15}", "Network", "UTXOs", "Tx/day", "Days to BE", "Profitable?");
        eprintln!("{}", "-".repeat(80));

        for (name, utxo_count, txs_per_day) in scenarios {
            // Average fee per transaction (assuming uniform distribution of wealth)
            let avg_fee = base_fee as f64 * 2.0; // Rough average cluster factor

            // Pool per day
            let pool_per_day = txs_per_day as f64 * avg_fee * pool_fraction;

            // Expected winnings per UTXO per day
            // Each tx has 4 winners, prize = (fee × 0.8) / 4
            // Probability of winning per tx = 4 / utxo_count
            // Expected winnings per UTXO per tx = (4/N) × (avg_fee × 0.8 / 4) = 0.8 × avg_fee / N
            let ev_per_utxo_per_tx = 0.8 * avg_fee / utxo_count as f64;
            let ev_per_utxo_per_day = ev_per_utxo_per_tx * txs_per_day as f64;

            // Cost to create one UTXO via splitting (quadratic fee)
            // Splitting 1->2 costs: base × factor × 2² = base × factor × 4
            // vs normal 2 outputs: same cost (so no penalty for 2 outputs)
            // But splitting 1->3 costs: base × factor × 9 vs base × factor × 4 = 2.25x more
            // Cost per extra UTXO ≈ 2 × base × factor (marginal cost)
            let split_cost = 2.0 * avg_fee;

            // Days to break even
            let days_to_breakeven = if ev_per_utxo_per_day > 0.0 {
                split_cost / ev_per_utxo_per_day
            } else {
                f64::INFINITY
            };

            let profitable = days_to_breakeven < 365.0;

            eprintln!(
                "{:<20} {:>12} {:>12} {:>15.1} {:>15}",
                name, utxo_count, txs_per_day, days_to_breakeven,
                if profitable { "YES (< 1 year)" } else { "NO (> 1 year)" }
            );
        }

        eprintln!("");
        eprintln!("ANALYSIS:");
        eprintln!("  - Smaller networks: Splitting may be profitable!");
        eprintln!("  - Larger networks: Splitting is unprofitable.");
        eprintln!("  - The mechanism's Sybil resistance scales with network size.");
        eprintln!("");
        eprintln!("IMPLICATION:");
        eprintln!("  Early-stage networks may need additional Sybil protection.");
        eprintln!("  Consider min_utxo_value or output count limits for bootstrap phase.");
        eprintln!("==========================================\n");
    }

    /// Test what happens when wealth inequality is extreme.
    ///
    /// The simulation showed 100% Gini reduction for "standard" inequality.
    /// What about when 1 entity holds 99% of all wealth?
    #[test]
    fn test_extreme_inequality_limits() {
        eprintln!("\n=== EXTREME INEQUALITY LIMITS TEST ===");
        eprintln!("Testing lottery behavior under pathological wealth distributions.");
        eprintln!("");

        let total_wealth = 100_000_000u64;

        // Scenarios with increasing inequality
        // Format: (name, rich_pct_of_wealth, rich_count, poor_count)
        let scenarios = [
            ("Moderate: 80/20", 80u64, 20u64, 80u64),    // 80% held by 20 rich, 20% by 80 poor
            ("High: 90/10", 90, 10, 90),                  // 90% held by 10 rich
            ("Extreme: 99/1", 99, 1, 99),                 // 99% held by 1 rich
            ("Pathological: 99.9/0.1", 999, 1, 999),      // 99.9% held by 1 (using 999/1000)
        ];

        eprintln!("{:<25} {:>10} {:>10} {:>12} {:>15}", "Scenario", "Init Gini", "Final Gini", "Change %", "Interpretation");
        eprintln!("{}", "-".repeat(80));

        for (name, rich_pct, rich_count, poor_count) in scenarios {
            let config = LotteryConfig {
                base_fee: 100,
                pool_fraction: 0.8,
                distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                output_fee_exponent: 2.0,
                min_utxo_value: 0,
                ..LotteryConfig::default()
            };

            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

            // Rich hold rich_pct / 1000 of wealth (to handle 99.9% case)
            let denominator = if rich_pct >= 100 { 1000u64 } else { 100u64 };
            let rich_wealth = total_wealth * rich_pct / denominator;
            let poor_wealth = total_wealth.saturating_sub(rich_wealth);

            for _ in 0..rich_count {
                sim.add_owner(rich_wealth / rich_count, SybilStrategy::Normal);
            }
            for _ in 0..poor_count {
                sim.add_owner(poor_wealth / poor_count, SybilStrategy::Normal);
            }

            sim.current_block = 1000;
            let initial_gini = sim.calculate_gini();

            // Run with value-weighted transactions (realistic)
            sim.advance_blocks_immediate(30_000, 20, TransactionModel::ValueWeighted);

            let final_gini = sim.calculate_gini();
            let change_pct = (initial_gini - final_gini) / initial_gini * 100.0;

            let interpretation = if change_pct > 50.0 {
                "Strong reduction"
            } else if change_pct > 20.0 {
                "Moderate reduction"
            } else if change_pct > 0.0 {
                "Weak reduction"
            } else {
                "INCREASED!"
            };

            eprintln!(
                "{:<25} {:>10.4} {:>10.4} {:>+11.1}% {:>15}",
                name, initial_gini, final_gini, change_pct, interpretation
            );
        }

        eprintln!("");
        eprintln!("ANALYSIS:");
        eprintln!("  The lottery's effectiveness may decrease with extreme inequality.");
        eprintln!("  This is because wealthy entities dominate transaction volume,");
        eprintln!("  and fees alone may not overcome the wealth concentration.");
        eprintln!("==========================================\n");
    }

    /// Compare selection modes for Sybil resistance.
    ///
    /// Tests whether alternative selection modes (sqrt, log, value-weighted)
    /// can mitigate the UTXO accumulation gaming attack while maintaining
    /// some level of progressivity.
    #[test]
    fn test_selection_mode_sybil_resistance() {
        eprintln!("\n=== SELECTION MODE SYBIL RESISTANCE TEST ===");
        eprintln!("Comparing how different selection modes handle UTXO splitting.");
        eprintln!("");

        let total_wealth = 100_000_000u64;
        let attacker_wealth = total_wealth * 10 / 100; // 10% of total

        // Selection modes to test
        let modes = [
            ("Uniform", SelectionMode::Uniform),
            ("ValueWeighted", SelectionMode::ValueWeighted),
            ("SqrtWeighted", SelectionMode::SqrtWeighted),
            ("LogWeighted", SelectionMode::LogWeighted),
        ];

        eprintln!("{:<20} {:>15} {:>15} {:>15} {:>15} {:>12}",
            "Mode", "1 UTXO Win", "10 UTXO Win", "Ratio", "Gaming Net", "Verdict");
        eprintln!("{}", "-".repeat(95));

        for (name, mode) in modes {
            // Baseline: attacker with 1 UTXO
            let baseline_winnings = {
                let config = LotteryConfig {
                    base_fee: 100,
                    pool_fraction: 0.8,
                    distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                    output_fee_exponent: 2.0,
                    min_utxo_value: 0,
                    selection_mode: mode,
                    ..LotteryConfig::default()
                };
                let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
                let attacker_id = sim.add_owner(attacker_wealth, SybilStrategy::Normal);
                for _ in 0..90 {
                    sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                }
                sim.current_block = 1000;
                sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);
                sim.owners.get(&attacker_id).map(|o| o.total_winnings).unwrap_or(0)
            };

            // Gaming: attacker with 10 UTXOs
            let (gaming_winnings, gaming_fees) = {
                let config = LotteryConfig {
                    base_fee: 100,
                    pool_fraction: 0.8,
                    distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                    output_fee_exponent: 2.0,
                    min_utxo_value: 0,
                    selection_mode: mode,
                    ..LotteryConfig::default()
                };
                let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
                let attacker_id = sim.add_owner(attacker_wealth, SybilStrategy::MultiAccount { num_accounts: 10 });
                for _ in 0..90 {
                    sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                }
                sim.current_block = 1000;
                sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);
                let owner = sim.owners.get(&attacker_id);
                (
                    owner.map(|o| o.total_winnings).unwrap_or(0),
                    owner.map(|o| o.total_fees_paid).unwrap_or(0),
                )
            };

            let ratio = gaming_winnings as f64 / baseline_winnings.max(1) as f64;
            let gaming_net = gaming_winnings as i64 - gaming_fees as i64;

            let verdict = if ratio <= 1.5 {
                "GOOD"
            } else if ratio <= 3.0 {
                "OK"
            } else if ratio <= 5.0 {
                "WEAK"
            } else {
                "VULNERABLE"
            };

            eprintln!(
                "{:<20} {:>15} {:>15} {:>14.2}x {:>15} {:>12}",
                name, baseline_winnings, gaming_winnings, ratio, gaming_net, verdict
            );
        }

        eprintln!("");
        eprintln!("ANALYSIS:");
        eprintln!("  Uniform:       ~10x advantage from splitting (most vulnerable)");
        eprintln!("  ValueWeighted: ~1x advantage (Sybil-resistant but not progressive)");
        eprintln!("  SqrtWeighted:  ~3.16x theoretical advantage (balanced)");
        eprintln!("  LogWeighted:   Variable advantage based on value distribution");
        eprintln!("");
        eprintln!("RECOMMENDATION:");
        eprintln!("  SqrtWeighted offers best balance of progressivity and Sybil resistance.");
        eprintln!("  It maintains sqrt(10) ≈ 3.16x advantage from splitting vs 10x uniform,");
        eprintln!("  while still favoring smaller UTXOs over pure value-weighting.");
        eprintln!("==========================================\n");
    }

    /// Test that SqrtWeighted selection still provides progressivity.
    ///
    /// The concern: if we switch from Uniform to SqrtWeighted, do we lose
    /// the progressive redistribution effect?
    #[test]
    fn test_sqrt_weighted_progressivity() {
        eprintln!("\n=== SQRT WEIGHTED PROGRESSIVITY TEST ===");
        eprintln!("Testing whether SqrtWeighted maintains progressive redistribution.");
        eprintln!("");

        let total_wealth = 100_000_000u64;

        // Compare Uniform vs SqrtWeighted for Gini reduction
        for (name, mode) in [
            ("Uniform", SelectionMode::Uniform),
            ("SqrtWeighted", SelectionMode::SqrtWeighted),
            ("ValueWeighted", SelectionMode::ValueWeighted),
        ] {
            let config = LotteryConfig {
                base_fee: 100,
                pool_fraction: 0.8,
                distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                output_fee_exponent: 2.0,
                min_utxo_value: 0,
                selection_mode: mode,
                ..LotteryConfig::default()
            };

            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

            // Create unequal population: 10 poor, 5 middle, 2 rich
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
            let initial_gini = sim.calculate_gini();

            sim.advance_blocks_immediate(30_000, 20, TransactionModel::ValueWeighted);

            let final_gini = sim.calculate_gini();
            let change_pct = (initial_gini - final_gini) / initial_gini * 100.0;

            eprintln!(
                "{:<15}: Gini {:.4} -> {:.4} ({:+.1}% reduction)",
                name, initial_gini, final_gini, change_pct
            );
        }

        eprintln!("");
        eprintln!("INTERPRETATION:");
        eprintln!("  If SqrtWeighted achieves significant Gini reduction (>50%),");
        eprintln!("  it can replace Uniform as the default while fixing the");
        eprintln!("  UTXO accumulation vulnerability.");
        eprintln!("==========================================\n");
    }

    /// Explore the Pareto frontier between progressivity and Sybil resistance.
    ///
    /// This test sweeps across the hybrid parameter α to find optimal trade-offs:
    /// - α = 1.0: Pure uniform (progressive, gameable)
    /// - α = 0.0: Pure value-weighted (Sybil-resistant, not progressive)
    /// - α in between: Various trade-offs
    ///
    /// We measure:
    /// 1. Gaming ratio (10 UTXO winnings / 1 UTXO winnings)
    /// 2. Gini reduction percentage
    /// 3. Implied "privacy cost" (conceptual)
    #[test]
    fn test_pareto_frontier_hybrid() {
        eprintln!("\n=== PARETO FRONTIER: HYBRID PARAMETER SWEEP ===");
        eprintln!("Exploring trade-offs between progressivity and Sybil resistance.");
        eprintln!("");
        eprintln!("α = 1.0: Pure uniform (progressive, 10x gameable)");
        eprintln!("α = 0.0: Pure value-weighted (not progressive, 1x gameable)");
        eprintln!("");

        let total_wealth = 100_000_000u64;
        let attacker_wealth = total_wealth * 10 / 100;

        // Sweep α from 0.0 to 1.0
        let alphas = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];

        eprintln!("{:>6} {:>12} {:>12} {:>12} {:>15}",
            "α", "Gaming Ratio", "Gini Δ%", "Poor Gain%", "Assessment");
        eprintln!("{}", "-".repeat(60));

        for alpha in alphas {
            // Measure gaming ratio
            let (baseline_win, gaming_win) = {
                let mode = SelectionMode::Hybrid { alpha };

                // Baseline: 1 UTXO
                let baseline = {
                    let config = LotteryConfig {
                        base_fee: 100,
                        pool_fraction: 0.8,
                        distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                        output_fee_exponent: 2.0,
                        min_utxo_value: 0,
                        selection_mode: mode,
                        ..LotteryConfig::default()
                    };
                    let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
                    let attacker_id = sim.add_owner(attacker_wealth, SybilStrategy::Normal);
                    for _ in 0..90 {
                        sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                    }
                    sim.current_block = 1000;
                    sim.advance_blocks_immediate(5_000, 20, TransactionModel::ValueWeighted);
                    sim.owners.get(&attacker_id).map(|o| o.total_winnings).unwrap_or(0)
                };

                // Gaming: 10 UTXOs
                let gaming = {
                    let config = LotteryConfig {
                        base_fee: 100,
                        pool_fraction: 0.8,
                        distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                        output_fee_exponent: 2.0,
                        min_utxo_value: 0,
                        selection_mode: mode,
                        ..LotteryConfig::default()
                    };
                    let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
                    let attacker_id = sim.add_owner(attacker_wealth, SybilStrategy::MultiAccount { num_accounts: 10 });
                    for _ in 0..90 {
                        sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                    }
                    sim.current_block = 1000;
                    sim.advance_blocks_immediate(5_000, 20, TransactionModel::ValueWeighted);
                    sim.owners.get(&attacker_id).map(|o| o.total_winnings).unwrap_or(0)
                };

                (baseline, gaming)
            };

            let gaming_ratio = gaming_win as f64 / baseline_win.max(1) as f64;

            // Measure Gini reduction and poor gain
            let (gini_change, poor_gain) = {
                let config = LotteryConfig {
                    base_fee: 100,
                    pool_fraction: 0.8,
                    distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                    output_fee_exponent: 2.0,
                    min_utxo_value: 0,
                    selection_mode: SelectionMode::Hybrid { alpha },
                    ..LotteryConfig::default()
                };
                let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

                // Track poor owners
                let mut poor_ids = Vec::new();
                for _ in 0..10 {
                    poor_ids.push(sim.add_owner(total_wealth / 200, SybilStrategy::Normal));
                }
                for _ in 0..5 {
                    sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal);
                }
                for _ in 0..2 {
                    sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal);
                }

                let initial_poor_wealth: u64 = poor_ids.iter().map(|id| sim.owner_value(*id)).sum();
                sim.current_block = 1000;
                let initial_gini = sim.calculate_gini();

                sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);

                let final_gini = sim.calculate_gini();
                let final_poor_wealth: u64 = poor_ids.iter().map(|id| sim.owner_value(*id)).sum();

                let gini_pct = (initial_gini - final_gini) / initial_gini * 100.0;
                let poor_pct = (final_poor_wealth as f64 - initial_poor_wealth as f64)
                    / initial_poor_wealth as f64 * 100.0;

                (gini_pct, poor_pct)
            };

            let assessment = if gaming_ratio <= 2.0 && gini_change > 20.0 {
                "★ OPTIMAL"
            } else if gaming_ratio <= 3.0 && gini_change > 10.0 {
                "GOOD"
            } else if gaming_ratio <= 5.0 {
                "ACCEPTABLE"
            } else {
                "POOR"
            };

            eprintln!(
                "{:>6.1} {:>11.2}x {:>11.1}% {:>11.1}% {:>15}",
                alpha, gaming_ratio, gini_change, poor_gain, assessment
            );
        }

        eprintln!("");
        eprintln!("INTERPRETATION:");
        eprintln!("  Look for α values where:");
        eprintln!("  - Gaming ratio ≤ 2x (acceptable Sybil resistance)");
        eprintln!("  - Gini reduction > 20% (meaningful progressivity)");
        eprintln!("  - Poor gain > 0% (actual redistribution to poor)");
        eprintln!("");
        eprintln!("  The optimal α represents the best trade-off point.");
        eprintln!("==========================================\n");
    }

    /// Test age-weighted selection for Sybil resistance.
    ///
    /// Age weighting discourages rapid UTXO accumulation by giving
    /// more lottery weight to older UTXOs.
    #[test]
    fn test_age_weighted_sybil_resistance() {
        eprintln!("\n=== AGE-WEIGHTED SYBIL RESISTANCE TEST ===");
        eprintln!("Testing whether age weighting discourages rapid UTXO accumulation.");
        eprintln!("");

        let total_wealth = 100_000_000u64;
        let attacker_wealth = total_wealth * 10 / 100;

        // Test different age bonus values
        let age_configs = [
            ("No bonus (uniform)", 0.0),
            ("1x bonus at max age", 1.0),
            ("2x bonus at max age", 2.0),
            ("5x bonus at max age", 5.0),
            ("10x bonus at max age", 10.0),
        ];

        eprintln!("{:<25} {:>15} {:>15} {:>12}",
            "Config", "Old UTXO Win", "New UTXO Win", "Ratio");
        eprintln!("{}", "-".repeat(70));

        for (name, age_bonus) in age_configs {
            let mode = SelectionMode::AgeWeighted {
                max_age_blocks: 10_000, // ~1 day at 5s blocks
                age_bonus,
            };

            // Old UTXO holder (created at block 0)
            let old_winnings = {
                let config = LotteryConfig {
                    base_fee: 100,
                    pool_fraction: 0.8,
                    distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                    output_fee_exponent: 2.0,
                    min_utxo_value: 0,
                    selection_mode: mode,
                    ..LotteryConfig::default()
                };
                let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

                // Old holder: created at block 0
                let old_id = sim.add_owner(attacker_wealth, SybilStrategy::Normal);

                // Others
                for _ in 0..90 {
                    sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                }

                // Start simulation at block 10000 (old UTXO is fully aged)
                sim.current_block = 10_000;

                sim.advance_blocks_immediate(5_000, 20, TransactionModel::ValueWeighted);
                sim.owners.get(&old_id).map(|o| o.total_winnings).unwrap_or(0)
            };

            // New UTXO holder (created at current block)
            let new_winnings = {
                let config = LotteryConfig {
                    base_fee: 100,
                    pool_fraction: 0.8,
                    distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                    output_fee_exponent: 2.0,
                    min_utxo_value: 0,
                    selection_mode: mode,
                    ..LotteryConfig::default()
                };
                let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

                // Others created at block 0
                for _ in 0..90 {
                    sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                }

                // Start simulation, then add new holder
                sim.current_block = 10_000;

                // New holder: created at current block (age = 0)
                let new_id = {
                    let owner_id = sim.owners.len() as u64 + 1;
                    let mut owner = LotteryOwner::new(owner_id, SybilStrategy::Normal);

                    let cluster_id = ClusterId::new(sim.next_cluster_id);
                    sim.next_cluster_id += 1;
                    sim.cluster_wealth.set(cluster_id, attacker_wealth);

                    let cluster_factor = sim.fee_curve.rate_bps(attacker_wealth) as f64 / 100.0;
                    let cluster_factor = cluster_factor.max(1.0).min(6.0);

                    let utxo_id = sim.next_utxo_id;
                    sim.next_utxo_id += 1;

                    // Created at current block (age = 0)
                    let utxo = LotteryUtxo::new(utxo_id, owner_id, attacker_wealth, cluster_factor, sim.current_block);
                    sim.utxos.insert(utxo_id, utxo);
                    owner.utxo_ids.push(utxo_id);
                    sim.owners.insert(owner_id, owner);
                    owner_id
                };

                sim.advance_blocks_immediate(5_000, 20, TransactionModel::ValueWeighted);
                sim.owners.get(&new_id).map(|o| o.total_winnings).unwrap_or(0)
            };

            let ratio = old_winnings as f64 / new_winnings.max(1) as f64;

            eprintln!(
                "{:<25} {:>15} {:>15} {:>11.2}x",
                name, old_winnings, new_winnings, ratio
            );
        }

        eprintln!("");
        eprintln!("INTERPRETATION:");
        eprintln!("  Higher age bonus = older UTXOs win more");
        eprintln!("  This discourages rapid UTXO splitting (new UTXOs have low weight)");
        eprintln!("  Privacy cost: reveals approximate UTXO age");
        eprintln!("==========================================\n");
    }

    /// Test cluster-weighted selection for progressivity.
    ///
    /// Cluster weighting gives more lottery weight to coins with lower
    /// cluster factors (commerce coins vs. minter coins).
    #[test]
    fn test_cluster_weighted_progressivity() {
        eprintln!("\n=== CLUSTER-WEIGHTED PROGRESSIVITY TEST ===");
        eprintln!("Testing whether cluster weighting provides progressive redistribution.");
        eprintln!("");

        let total_wealth = 100_000_000u64;

        // Compare ClusterWeighted vs ValueWeighted
        for (name, mode) in [
            ("ValueWeighted", SelectionMode::ValueWeighted),
            ("ClusterWeighted", SelectionMode::ClusterWeighted),
        ] {
            let config = LotteryConfig {
                base_fee: 100,
                pool_fraction: 0.8,
                distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                output_fee_exponent: 2.0,
                min_utxo_value: 0,
                selection_mode: mode,
                ..LotteryConfig::default()
            };

            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

            // Create population with different cluster factors
            // Poor: low wealth, low cluster factor (commerce)
            // Rich: high wealth, high cluster factor (minter)
            let mut poor_ids = Vec::new();
            let mut rich_ids = Vec::new();

            for _ in 0..10 {
                poor_ids.push(sim.add_owner(total_wealth / 200, SybilStrategy::Normal));
            }
            for _ in 0..5 {
                sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal);
            }
            for _ in 0..2 {
                rich_ids.push(sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal));
            }

            let initial_poor: u64 = poor_ids.iter().map(|id| sim.owner_value(*id)).sum();
            let initial_rich: u64 = rich_ids.iter().map(|id| sim.owner_value(*id)).sum();

            sim.current_block = 1000;
            let initial_gini = sim.calculate_gini();

            sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);

            let final_gini = sim.calculate_gini();
            let final_poor: u64 = poor_ids.iter().map(|id| sim.owner_value(*id)).sum();
            let final_rich: u64 = rich_ids.iter().map(|id| sim.owner_value(*id)).sum();

            let gini_change = (initial_gini - final_gini) / initial_gini * 100.0;
            let poor_change = (final_poor as i64 - initial_poor as i64) as f64 / initial_poor as f64 * 100.0;
            let rich_change = (final_rich as i64 - initial_rich as i64) as f64 / initial_rich as f64 * 100.0;

            eprintln!("{}", name);
            eprintln!("  Gini: {:.4} -> {:.4} ({:+.1}%)", initial_gini, final_gini, gini_change);
            eprintln!("  Poor: {} -> {} ({:+.1}%)", initial_poor, final_poor, poor_change);
            eprintln!("  Rich: {} -> {} ({:+.1}%)", initial_rich, final_rich, rich_change);
            eprintln!("");
        }

        eprintln!("INTERPRETATION:");
        eprintln!("  ClusterWeighted should show more redistribution from rich to poor");
        eprintln!("  because low-factor (commerce) coins have higher lottery weight.");
        eprintln!("  Privacy cost: reveals coin origin (~1-2 bits)");
        eprintln!("==========================================\n");
    }

    /// Comprehensive privacy-progressivity trade-off analysis.
    ///
    /// This test evaluates all selection modes on three dimensions:
    /// 1. Sybil resistance (gaming ratio)
    /// 2. Progressivity (Gini reduction)
    /// 3. Privacy cost (conceptual bits revealed)
    #[test]
    fn test_privacy_progressivity_tradeoff() {
        eprintln!("\n=== PRIVACY-PROGRESSIVITY TRADE-OFF ANALYSIS ===");
        eprintln!("Evaluating all selection modes on three dimensions.");
        eprintln!("");

        let total_wealth = 100_000_000u64;
        let attacker_wealth = total_wealth * 10 / 100;

        // Define modes with their privacy costs
        let modes: Vec<(&str, SelectionMode, f64)> = vec![
            ("Uniform", SelectionMode::Uniform, 0.0),
            ("ValueWeighted", SelectionMode::ValueWeighted, 0.0),
            ("SqrtWeighted", SelectionMode::SqrtWeighted, 0.0),
            ("Hybrid(0.3)", SelectionMode::Hybrid { alpha: 0.3 }, 0.0),
            ("Hybrid(0.5)", SelectionMode::Hybrid { alpha: 0.5 }, 0.0),
            ("AgeWeighted(2x)", SelectionMode::AgeWeighted { max_age_blocks: 10_000, age_bonus: 2.0 }, 0.5),
            ("AgeWeighted(5x)", SelectionMode::AgeWeighted { max_age_blocks: 10_000, age_bonus: 5.0 }, 0.5),
            ("ClusterWeighted", SelectionMode::ClusterWeighted, 1.5),
        ];

        eprintln!("{:<20} {:>10} {:>12} {:>12} {:>12}",
            "Mode", "Privacy", "Gaming", "Gini Δ%", "Score");
        eprintln!("{}", "-".repeat(70));

        let mut results: Vec<(&str, f64, f64, f64, f64)> = Vec::new();

        for (name, mode, privacy_cost) in &modes {
            // Measure gaming ratio
            let gaming_ratio = {
                let baseline = {
                    let config = LotteryConfig {
                        base_fee: 100,
                        pool_fraction: 0.8,
                        distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                        output_fee_exponent: 2.0,
                        min_utxo_value: 0,
                        selection_mode: *mode,
                        ..LotteryConfig::default()
                    };
                    let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
                    let attacker_id = sim.add_owner(attacker_wealth, SybilStrategy::Normal);
                    for _ in 0..90 {
                        sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                    }
                    sim.current_block = 10_000; // For age-weighted
                    sim.advance_blocks_immediate(5_000, 20, TransactionModel::ValueWeighted);
                    sim.owners.get(&attacker_id).map(|o| o.total_winnings).unwrap_or(0)
                };

                let gaming = {
                    let config = LotteryConfig {
                        base_fee: 100,
                        pool_fraction: 0.8,
                        distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                        output_fee_exponent: 2.0,
                        min_utxo_value: 0,
                        selection_mode: *mode,
                        ..LotteryConfig::default()
                    };
                    let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
                    let attacker_id = sim.add_owner(attacker_wealth, SybilStrategy::MultiAccount { num_accounts: 10 });
                    for _ in 0..90 {
                        sim.add_owner(total_wealth / 100, SybilStrategy::Normal);
                    }
                    sim.current_block = 10_000;
                    sim.advance_blocks_immediate(5_000, 20, TransactionModel::ValueWeighted);
                    sim.owners.get(&attacker_id).map(|o| o.total_winnings).unwrap_or(0)
                };

                gaming as f64 / baseline.max(1) as f64
            };

            // Measure Gini reduction
            let gini_change = {
                let config = LotteryConfig {
                    base_fee: 100,
                    pool_fraction: 0.8,
                    distribution_mode: DistributionMode::Immediate { winners_per_tx: 4 },
                    output_fee_exponent: 2.0,
                    min_utxo_value: 0,
                    selection_mode: *mode,
                    ..LotteryConfig::default()
                };
                let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

                for _ in 0..10 {
                    sim.add_owner(total_wealth / 200, SybilStrategy::Normal);
                }
                for _ in 0..5 {
                    sim.add_owner(total_wealth * 5 / 100, SybilStrategy::Normal);
                }
                for _ in 0..2 {
                    sim.add_owner(total_wealth * 35 / 100, SybilStrategy::Normal);
                }

                sim.current_block = 10_000;
                let initial = sim.calculate_gini();
                sim.advance_blocks_immediate(10_000, 20, TransactionModel::ValueWeighted);
                let final_gini = sim.calculate_gini();

                (initial - final_gini) / initial * 100.0
            };

            // Calculate score: higher is better
            // Score = Gini_reduction / (gaming_ratio × (1 + privacy_cost))
            let score = gini_change / (gaming_ratio * (1.0 + privacy_cost));

            results.push((name, *privacy_cost, gaming_ratio, gini_change, score));

            eprintln!(
                "{:<20} {:>9.1}b {:>11.2}x {:>11.1}% {:>11.2}",
                name, privacy_cost, gaming_ratio, gini_change, score
            );
        }

        // Find best options
        eprintln!("");
        eprintln!("PARETO-OPTIMAL POINTS:");

        // Sort by score descending
        let mut sorted = results.clone();
        sorted.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));

        for (i, (name, privacy, gaming, gini, score)) in sorted.iter().take(3).enumerate() {
            eprintln!(
                "  {}. {}: score={:.2} (privacy={:.1}b, gaming={:.2}x, gini={:.1}%)",
                i + 1, name, score, privacy, gaming, gini
            );
        }

        eprintln!("");
        eprintln!("RECOMMENDATIONS:");
        eprintln!("  - For maximum privacy: Use Hybrid(0.3) - 0 bits cost, ~3x gaming");
        eprintln!("  - For balanced approach: Use AgeWeighted(5x) - 0.5 bits cost");
        eprintln!("  - For maximum progressivity: Use ClusterWeighted - 1.5 bits cost");
        eprintln!("==========================================\n");
    }
}
