//! Economic metrics for simulation analysis.
//!
//! Tracks:
//! - Gini coefficient of wealth distribution
//! - Effective fee rates by wealth quintile
//! - Tag entropy (how diffuse are tags?)
//! - Wash trading profitability
//! - Mixer utilization

use crate::tag::{TagVector, TAG_WEIGHT_SCALE};
use crate::{ClusterWealth, FeeCurve};
use std::collections::HashMap;

use super::agent::AgentId;

/// Snapshot of metrics at a point in time.
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    /// Simulation round.
    pub round: u64,

    /// Gini coefficient of wealth (0 = perfect equality, 1 = perfect inequality).
    pub gini_coefficient: f64,

    /// Total wealth in the system.
    pub total_wealth: u64,

    /// Number of agents.
    pub num_agents: usize,

    /// Average fee rate by wealth quintile (in basis points).
    /// Index 0 = poorest 20%, Index 4 = richest 20%.
    pub fee_rate_by_quintile: [f64; 5],

    /// Total fees collected (cumulative).
    pub total_fees_collected: u64,

    /// Total number of transactions.
    pub transaction_count: u64,

    /// Tag entropy (Shannon entropy of cluster attribution).
    pub tag_entropy: f64,

    /// Mixer volume this period.
    pub mixer_volume: u64,

    /// Mixer utilization rate (fraction of wealth flowing through mixers).
    pub mixer_utilization: f64,

    /// Average cluster wealth.
    pub avg_cluster_wealth: u64,

    /// Number of active clusters.
    pub active_clusters: usize,

    /// Wealth held by top 1% of agents.
    pub top_1_pct_wealth_share: f64,

    /// Wealth held by top 10% of agents.
    pub top_10_pct_wealth_share: f64,
}

/// Time series of metrics.
#[derive(Clone, Debug, Default)]
pub struct SimulationMetrics {
    /// Snapshots taken during simulation.
    pub snapshots: Vec<Metrics>,

    /// Per-agent fee totals for analysis.
    pub agent_fees: HashMap<AgentId, u64>,

    /// Per-agent type fee totals.
    pub fees_by_agent_type: HashMap<String, u64>,

    /// Wash trading attempts and outcomes.
    pub wash_trade_attempts: u64,
    pub wash_trade_fees_paid: u64,
    pub wash_trade_savings: i64,
}

impl SimulationMetrics {
    /// Create new empty metrics tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a metrics snapshot.
    pub fn record_snapshot(&mut self, metrics: Metrics) {
        self.snapshots.push(metrics);
    }

    /// Record fees paid by an agent.
    pub fn record_agent_fees(&mut self, agent: AgentId, agent_type: &str, fees: u64) {
        *self.agent_fees.entry(agent).or_insert(0) += fees;
        *self
            .fees_by_agent_type
            .entry(agent_type.to_string())
            .or_insert(0) += fees;
    }

    /// Record wash trade attempt.
    pub fn record_wash_trade(&mut self, fees_paid: u64, savings: i64) {
        self.wash_trade_attempts += 1;
        self.wash_trade_fees_paid += fees_paid;
        self.wash_trade_savings += savings;
    }

    /// Get the final Gini coefficient.
    pub fn final_gini(&self) -> Option<f64> {
        self.snapshots.last().map(|m| m.gini_coefficient)
    }

    /// Get the change in Gini over time.
    pub fn gini_change(&self) -> Option<f64> {
        if self.snapshots.len() < 2 {
            return None;
        }
        let first = self.snapshots.first()?.gini_coefficient;
        let last = self.snapshots.last()?.gini_coefficient;
        Some(last - first)
    }

    /// Get average fee rate by quintile across all snapshots.
    pub fn avg_fee_rates_by_quintile(&self) -> [f64; 5] {
        if self.snapshots.is_empty() {
            return [0.0; 5];
        }

        let mut totals = [0.0; 5];
        for snapshot in &self.snapshots {
            for (i, &rate) in snapshot.fee_rate_by_quintile.iter().enumerate() {
                totals[i] += rate;
            }
        }

        let n = self.snapshots.len() as f64;
        totals.iter().map(|&t| t / n).collect::<Vec<_>>().try_into().unwrap()
    }

    /// Summary statistics.
    pub fn summary(&self) -> MetricsSummary {
        let first = self.snapshots.first();
        let last = self.snapshots.last();

        MetricsSummary {
            initial_gini: first.map(|m| m.gini_coefficient).unwrap_or(0.0),
            final_gini: last.map(|m| m.gini_coefficient).unwrap_or(0.0),
            total_fees: last.map(|m| m.total_fees_collected).unwrap_or(0),
            total_transactions: last.map(|m| m.transaction_count).unwrap_or(0),
            avg_fee_by_quintile: self.avg_fee_rates_by_quintile(),
            wash_trade_attempts: self.wash_trade_attempts,
            wash_trade_net_savings: self.wash_trade_savings,
            mixer_utilization: last.map(|m| m.mixer_utilization).unwrap_or(0.0),
        }
    }
}

/// Summary of simulation results.
#[derive(Clone, Debug)]
pub struct MetricsSummary {
    pub initial_gini: f64,
    pub final_gini: f64,
    pub total_fees: u64,
    pub total_transactions: u64,
    pub avg_fee_by_quintile: [f64; 5],
    pub wash_trade_attempts: u64,
    pub wash_trade_net_savings: i64,
    pub mixer_utilization: f64,
}

/// Calculate Gini coefficient from a list of wealth values.
pub fn calculate_gini(wealths: &[u64]) -> f64 {
    if wealths.is_empty() {
        return 0.0;
    }

    let n = wealths.len();
    if n == 1 {
        return 0.0;
    }

    let total: u64 = wealths.iter().sum();
    if total == 0 {
        return 0.0;
    }

    // Sort wealths
    let mut sorted: Vec<u64> = wealths.to_vec();
    sorted.sort_unstable();

    // Calculate Gini using the formula:
    // G = (2 * Σ(i * x_i) - (n + 1) * Σx_i) / (n * Σx_i)
    let sum_indexed: u64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &x)| (i as u64 + 1) * x)
        .sum();

    let numerator = 2.0 * sum_indexed as f64 - (n as f64 + 1.0) * total as f64;
    let denominator = n as f64 * total as f64;

    (numerator / denominator).clamp(0.0, 1.0)
}

/// Calculate Shannon entropy of tag distribution.
pub fn calculate_tag_entropy(tags: &TagVector, _cluster_wealth: &ClusterWealth) -> f64 {
    let mut entropy = 0.0;

    // Entropy of the tag vector itself
    for (_cluster, weight) in tags.iter() {
        if weight > 0 {
            let p = weight as f64 / TAG_WEIGHT_SCALE as f64;
            if p > 0.0 {
                entropy -= p * p.log2();
            }
        }
    }

    // Add background contribution
    let bg = tags.background() as f64 / TAG_WEIGHT_SCALE as f64;
    if bg > 0.0 {
        entropy -= bg * bg.log2();
    }

    entropy
}

/// Calculate average tag entropy across all accounts.
pub fn calculate_system_entropy(
    agent_tags: &[(AgentId, TagVector)],
    cluster_wealth: &ClusterWealth,
) -> f64 {
    if agent_tags.is_empty() {
        return 0.0;
    }

    let total_entropy: f64 = agent_tags
        .iter()
        .map(|(_, tags)| calculate_tag_entropy(tags, cluster_wealth))
        .sum();

    total_entropy / agent_tags.len() as f64
}

/// Calculate fee rates by wealth quintile.
///
/// Returns array where index 0 = poorest 20%, index 4 = richest 20%.
pub fn calculate_fee_rates_by_quintile(
    agents: &[(AgentId, u64, u32)], // (id, balance, fee_rate_bps)
) -> [f64; 5] {
    if agents.is_empty() {
        return [0.0; 5];
    }

    // Sort by wealth
    let mut sorted = agents.to_vec();
    sorted.sort_by(|a, b| a.1.cmp(&b.1));

    let n = sorted.len();
    let quintile_size = (n + 4) / 5; // Ceiling division

    let mut quintile_rates = [0.0; 5];
    let mut quintile_counts = [0usize; 5];

    for (i, (_, _, rate)) in sorted.iter().enumerate() {
        let quintile = (i / quintile_size).min(4);
        quintile_rates[quintile] += *rate as f64;
        quintile_counts[quintile] += 1;
    }

    for i in 0..5 {
        if quintile_counts[i] > 0 {
            quintile_rates[i] /= quintile_counts[i] as f64;
        }
    }

    quintile_rates
}

/// Calculate wealth concentration metrics.
pub fn calculate_concentration(wealths: &[u64]) -> (f64, f64) {
    if wealths.is_empty() {
        return (0.0, 0.0);
    }

    let total: u64 = wealths.iter().sum();
    if total == 0 {
        return (0.0, 0.0);
    }

    let mut sorted = wealths.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a)); // Descending

    let n = sorted.len();

    // Top 1% wealth share
    let top_1_pct_count = (n as f64 * 0.01).max(1.0) as usize;
    let top_1_wealth: u64 = sorted.iter().take(top_1_pct_count).sum();
    let top_1_share = top_1_wealth as f64 / total as f64;

    // Top 10% wealth share
    let top_10_pct_count = (n as f64 * 0.10).max(1.0) as usize;
    let top_10_wealth: u64 = sorted.iter().take(top_10_pct_count).sum();
    let top_10_share = top_10_wealth as f64 / total as f64;

    (top_1_share, top_10_share)
}

/// Create a metrics snapshot from current simulation state.
pub fn snapshot_metrics(
    round: u64,
    agent_data: &[(AgentId, u64, u32, TagVector)], // (id, balance, fee_rate, tags)
    cluster_wealth: &ClusterWealth,
    total_fees: u64,
    transaction_count: u64,
    mixer_volume: u64,
    _fee_curve: &FeeCurve,
) -> Metrics {
    let wealths: Vec<u64> = agent_data.iter().map(|(_, b, _, _)| *b).collect();
    let total_wealth: u64 = wealths.iter().sum();
    let num_agents = agent_data.len();

    let gini = calculate_gini(&wealths);

    let agent_rates: Vec<_> = agent_data
        .iter()
        .map(|(id, balance, rate, _)| (*id, *balance, *rate))
        .collect();
    let fee_rate_by_quintile = calculate_fee_rates_by_quintile(&agent_rates);

    let agent_tags: Vec<_> = agent_data
        .iter()
        .map(|(id, _, _, tags)| (*id, tags.clone()))
        .collect();
    let tag_entropy = calculate_system_entropy(&agent_tags, cluster_wealth);

    let (top_1_share, top_10_share) = calculate_concentration(&wealths);

    let mixer_utilization = if total_wealth > 0 {
        mixer_volume as f64 / total_wealth as f64
    } else {
        0.0
    };

    let avg_cluster_wealth = if cluster_wealth.len() > 0 {
        cluster_wealth.total() / cluster_wealth.len() as u64
    } else {
        0
    };

    Metrics {
        round,
        gini_coefficient: gini,
        total_wealth,
        num_agents,
        fee_rate_by_quintile,
        total_fees_collected: total_fees,
        transaction_count,
        tag_entropy,
        mixer_volume,
        mixer_utilization,
        avg_cluster_wealth,
        active_clusters: cluster_wealth.len(),
        top_1_pct_wealth_share: top_1_share,
        top_10_pct_wealth_share: top_10_share,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gini_perfect_equality() {
        let wealths = vec![100, 100, 100, 100, 100];
        let gini = calculate_gini(&wealths);
        assert!(gini < 0.01, "Perfect equality should have Gini near 0: {gini}");
    }

    #[test]
    fn test_gini_high_inequality() {
        let wealths = vec![0, 0, 0, 0, 1000];
        let gini = calculate_gini(&wealths);
        assert!(gini > 0.7, "High inequality should have high Gini: {gini}");
    }

    #[test]
    fn test_gini_moderate() {
        let wealths = vec![10, 20, 30, 40, 100];
        let gini = calculate_gini(&wealths);
        assert!(gini > 0.2 && gini < 0.5, "Moderate inequality: {gini}");
    }

    #[test]
    fn test_fee_rates_by_quintile() {
        let agents = vec![
            (AgentId(1), 100, 500),   // Q1 (poorest)
            (AgentId(2), 200, 400),   // Q2
            (AgentId(3), 300, 300),   // Q3
            (AgentId(4), 400, 200),   // Q4
            (AgentId(5), 1000, 1000), // Q5 (richest)
        ];

        let rates = calculate_fee_rates_by_quintile(&agents);

        // Richest should pay highest rate
        assert!(rates[4] > rates[0], "Richest quintile should pay more");
    }

    #[test]
    fn test_concentration() {
        let wealths = vec![1, 2, 3, 4, 90]; // One agent has 90% of wealth
        let (top_1, top_10) = calculate_concentration(&wealths);

        assert!(top_1 > 0.8, "Top 1% should have most wealth: {top_1}");
        // With 5 agents, top 10% is 1 agent (ceiling), which holds 90/100 = 0.9
        assert!(top_10 >= 0.9, "Top 10% should have almost all: {top_10}");
    }
}
