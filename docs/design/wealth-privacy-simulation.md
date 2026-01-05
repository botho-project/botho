# Wealth-Conditional Privacy Simulation: Implementation Plan

## Status

**Draft** - Implementation specification for privacy simulation extensions

## Overview

This document describes extensions to the cluster-tax simulation system to model and validate the wealth-conditional privacy design proposed in [wealth-conditional-privacy.md](wealth-conditional-privacy.md).

## Goals

1. **Validate pool sizes**: Ensure both private and transparent pools have adequate anonymity sets
2. **Measure transparency rates**: What fraction of outputs/value becomes transparent at various thresholds?
3. **Test gaming resistance**: Can adversaries structure transactions to avoid transparency?
4. **Optimize thresholds**: Find threshold parameters that balance privacy and transparency goals
5. **Model adversaries**: How much does amount visibility help attackers?

## Architecture

### New Module Structure

```
cluster-tax/src/simulation/
├── mod.rs                    # Add wealth_privacy export
├── privacy.rs                # Existing ring signature privacy
├── wealth_privacy.rs         # NEW: Wealth-conditional privacy
│   ├── types.rs              # OutputPrivacyLevel, PrivacyPolicy
│   ├── pools.rs              # SegregatedPools, pool generation
│   ├── adversaries.rs        # AmountCorrelationAdversary
│   ├── metrics.rs            # WealthPrivacyMetrics
│   └── monte_carlo.rs        # Extended Monte Carlo
└── ...
```

### Core Types

```rust
// simulation/wealth_privacy/types.rs

/// Privacy level for an output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputPrivacyLevel {
    /// Amount hidden (Pedersen commitment + Bulletproof)
    FullPrivate,
    /// Amount visible (plaintext)
    AmountVisible,
}

/// Policy parameters for wealth-conditional privacy
#[derive(Debug, Clone)]
pub struct PrivacyPolicy {
    /// Source wealth at or below: guaranteed full privacy
    pub full_privacy_threshold: u64,

    /// Source wealth at or above: guaranteed transparency
    pub transparency_threshold: u64,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            // 10,000 BTH in nanoBTH
            full_privacy_threshold: 10_000_000_000_000,
            // 100,000 BTH in nanoBTH
            transparency_threshold: 100_000_000_000_000,
        }
    }
}

impl PrivacyPolicy {
    /// Determine privacy level from source wealth
    pub fn determine(
        &self,
        source_wealth: u64,
        tx_entropy: &[u8; 32],
    ) -> OutputPrivacyLevel {
        if source_wealth <= self.full_privacy_threshold {
            OutputPrivacyLevel::FullPrivate
        } else if source_wealth >= self.transparency_threshold {
            OutputPrivacyLevel::AmountVisible
        } else {
            // Probabilistic zone: linear interpolation
            let p = self.transparency_probability(source_wealth);
            if deterministic_roll(tx_entropy) < p {
                OutputPrivacyLevel::AmountVisible
            } else {
                OutputPrivacyLevel::FullPrivate
            }
        }
    }

    /// Probability of transparency in the probabilistic zone
    pub fn transparency_probability(&self, source_wealth: u64) -> f64 {
        if source_wealth <= self.full_privacy_threshold {
            return 0.0;
        }
        if source_wealth >= self.transparency_threshold {
            return 1.0;
        }

        let range = self.transparency_threshold - self.full_privacy_threshold;
        let position = source_wealth - self.full_privacy_threshold;
        position as f64 / range as f64
    }

    /// Which zone does this source wealth fall into?
    pub fn zone(&self, source_wealth: u64) -> PrivacyZone {
        if source_wealth <= self.full_privacy_threshold {
            PrivacyZone::Private
        } else if source_wealth >= self.transparency_threshold {
            PrivacyZone::Transparent
        } else {
            PrivacyZone::Probabilistic
        }
    }
}

/// Zone classification for analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrivacyZone {
    Private,
    Probabilistic,
    Transparent,
}

/// Deterministic roll from transaction entropy
fn deterministic_roll(entropy: &[u8; 32]) -> f64 {
    let value = u64::from_le_bytes(entropy[0..8].try_into().unwrap());
    value as f64 / u64::MAX as f64
}
```

### Extended Output Type

```rust
// simulation/wealth_privacy/types.rs (continued)

/// Output with wealth-conditional privacy information
#[derive(Debug, Clone)]
pub struct WealthAwareOutput {
    /// Unique identifier
    pub id: u64,

    /// Age in blocks
    pub age_blocks: u64,

    /// Output type (Standard, Exchange, Whale, etc.)
    pub output_type: OutputType,

    /// Cluster tag distribution
    pub cluster_tags: HashMap<u64, u32>,

    /// Transaction hops since minting
    pub hops_since_mint: u32,

    // === New fields for wealth-conditional privacy ===

    /// Source wealth derived from cluster tags
    pub source_wealth: u64,

    /// Output amount (visible or hidden depending on privacy_level)
    pub amount: u64,

    /// Privacy level for this output
    pub privacy_level: OutputPrivacyLevel,

    /// Transaction entropy used for probabilistic determination
    pub tx_entropy: [u8; 32],
}

impl WealthAwareOutput {
    /// Create from base SimulatedOutput with wealth/privacy info
    pub fn from_simulated(
        base: SimulatedOutput,
        cluster_wealth: &ClusterWealth,
        amount: u64,
        policy: &PrivacyPolicy,
        rng: &mut impl Rng,
    ) -> Self {
        // Calculate source wealth from cluster tags
        let source_wealth = calculate_source_wealth(&base.cluster_tags, cluster_wealth);

        // Generate transaction entropy
        let mut tx_entropy = [0u8; 32];
        rng.fill_bytes(&mut tx_entropy);

        // Determine privacy level
        let privacy_level = policy.determine(source_wealth, &tx_entropy);

        Self {
            id: base.id,
            age_blocks: base.age_blocks,
            output_type: base.output_type,
            cluster_tags: base.cluster_tags,
            hops_since_mint: base.hops_since_mint,
            source_wealth,
            amount,
            privacy_level,
            tx_entropy,
        }
    }
}

/// Calculate source wealth from cluster tags and global cluster wealth
fn calculate_source_wealth(
    tags: &HashMap<u64, u32>,
    cluster_wealth: &ClusterWealth,
) -> u64 {
    // Weighted sum: Σ (cluster_wealth[i] × tag_weight[i]) / total_weight
    let mut weighted_sum = 0u128;
    let mut total_weight = 0u64;

    for (&cluster_id, &weight) in tags {
        let cluster_w = cluster_wealth.get(ClusterId(cluster_id)).unwrap_or(0);
        weighted_sum += cluster_w as u128 * weight as u128;
        total_weight += weight as u64;
    }

    if total_weight == 0 {
        return 0;
    }

    (weighted_sum / total_weight as u128) as u64
}
```

### Segregated Pools

```rust
// simulation/wealth_privacy/pools.rs

/// Pools segregated by privacy level for decoy selection
#[derive(Debug, Clone)]
pub struct SegregatedPools {
    /// Outputs with full privacy (amounts hidden)
    pub private: Vec<WealthAwareOutput>,

    /// Outputs with visible amounts
    pub transparent: Vec<WealthAwareOutput>,
}

impl SegregatedPools {
    /// Create from a mixed pool of outputs
    pub fn from_outputs(outputs: Vec<WealthAwareOutput>) -> Self {
        let (private, transparent): (Vec<_>, Vec<_>) = outputs
            .into_iter()
            .partition(|o| o.privacy_level == OutputPrivacyLevel::FullPrivate);

        Self { private, transparent }
    }

    /// Get the appropriate pool for decoy selection
    pub fn pool_for(&self, level: OutputPrivacyLevel) -> &[WealthAwareOutput] {
        match level {
            OutputPrivacyLevel::FullPrivate => &self.private,
            OutputPrivacyLevel::AmountVisible => &self.transparent,
        }
    }

    /// Total outputs
    pub fn total_size(&self) -> usize {
        self.private.len() + self.transparent.len()
    }

    /// Transparency rate (fraction of outputs that are transparent)
    pub fn transparency_rate(&self) -> f64 {
        if self.total_size() == 0 {
            return 0.0;
        }
        self.transparent.len() as f64 / self.total_size() as f64
    }

    /// Value transparency rate (fraction of value that's transparent)
    pub fn value_transparency_rate(&self) -> f64 {
        let private_value: u64 = self.private.iter().map(|o| o.amount).sum();
        let transparent_value: u64 = self.transparent.iter().map(|o| o.amount).sum();
        let total = private_value + transparent_value;

        if total == 0 {
            return 0.0;
        }
        transparent_value as f64 / total as f64
    }

    /// Check if both pools have adequate anonymity sets
    pub fn pools_adequate(&self, min_pool_size: usize) -> PoolAdequacy {
        PoolAdequacy {
            private_adequate: self.private.len() >= min_pool_size,
            transparent_adequate: self.transparent.len() >= min_pool_size,
            private_size: self.private.len(),
            transparent_size: self.transparent.len(),
            min_required: min_pool_size,
        }
    }
}

/// Pool adequacy assessment
#[derive(Debug, Clone)]
pub struct PoolAdequacy {
    pub private_adequate: bool,
    pub transparent_adequate: bool,
    pub private_size: usize,
    pub transparent_size: usize,
    pub min_required: usize,
}

impl PoolAdequacy {
    pub fn both_adequate(&self) -> bool {
        self.private_adequate && self.transparent_adequate
    }
}
```

### Pool Generator

```rust
// simulation/wealth_privacy/pools.rs (continued)

/// Configuration for wealth-aware pool generation
#[derive(Debug, Clone)]
pub struct WealthAwarePoolConfig {
    /// Base pool configuration
    pub base: PoolConfig,

    /// Privacy policy
    pub policy: PrivacyPolicy,

    /// Cluster wealth distribution parameters
    pub cluster_wealth_gini: f64,  // 0.7 = concentrated, 0.3 = diffuse

    /// Amount distribution (log-normal parameters)
    pub amount_mean_log: f64,      // ln(mean amount)
    pub amount_std_log: f64,       // ln std deviation
}

impl Default for WealthAwarePoolConfig {
    fn default() -> Self {
        Self {
            base: PoolConfig::default(),
            policy: PrivacyPolicy::default(),
            cluster_wealth_gini: 0.6,
            amount_mean_log: 20.0,  // ~500M nanoBTH = 0.5 BTH
            amount_std_log: 3.0,
        }
    }
}

/// Generator for wealth-aware output pools
pub struct WealthAwarePoolGenerator {
    base_generator: OutputPoolGenerator,
    config: WealthAwarePoolConfig,
    cluster_wealth: ClusterWealth,
}

impl WealthAwarePoolGenerator {
    pub fn new(config: WealthAwarePoolConfig) -> Self {
        let base_generator = OutputPoolGenerator::new(config.base.clone());
        let cluster_wealth = generate_cluster_wealth(
            config.base.num_clusters,
            config.cluster_wealth_gini,
        );

        Self {
            base_generator,
            config,
            cluster_wealth,
        }
    }

    /// Generate a complete pool with privacy levels assigned
    pub fn generate<R: Rng>(&mut self, rng: &mut R) -> SegregatedPools {
        let mut outputs = Vec::with_capacity(self.config.base.pool_size);

        for _ in 0..self.config.base.pool_size {
            let base = self.base_generator.generate_output(rng);
            let amount = self.generate_amount(rng);

            let output = WealthAwareOutput::from_simulated(
                base,
                &self.cluster_wealth,
                amount,
                &self.config.policy,
                rng,
            );

            outputs.push(output);
        }

        SegregatedPools::from_outputs(outputs)
    }

    fn generate_amount<R: Rng>(&self, rng: &mut R) -> u64 {
        let log_amount = rng.sample(
            rand_distr::Normal::new(
                self.config.amount_mean_log,
                self.config.amount_std_log,
            ).unwrap()
        );
        log_amount.exp() as u64
    }

    pub fn cluster_wealth(&self) -> &ClusterWealth {
        &self.cluster_wealth
    }
}

/// Generate cluster wealth with specified Gini coefficient
fn generate_cluster_wealth(num_clusters: u64, target_gini: f64) -> ClusterWealth {
    // Use Pareto distribution to achieve target inequality
    let alpha = 1.0 / target_gini;  // Higher alpha = more equal

    let mut wealth = ClusterWealth::new();
    let mut rng = rand::thread_rng();

    for i in 0..num_clusters {
        let pareto = rand_distr::Pareto::new(1.0, alpha).unwrap();
        let w = (pareto.sample(&mut rng) * 1_000_000_000.0) as u64;
        wealth.set(ClusterId(i), w);
    }

    wealth
}
```

### New Adversary Models

```rust
// simulation/wealth_privacy/adversaries.rs

use super::*;

/// Adversary that exploits visible amounts for correlation attacks
pub struct AmountCorrelationAdversary {
    /// Tolerance for amount matching (0.01 = 1% variance allowed)
    pub amount_tolerance: f64,
}

impl Default for AmountCorrelationAdversary {
    fn default() -> Self {
        Self {
            amount_tolerance: 0.05,  // 5% tolerance
        }
    }
}

impl AmountCorrelationAdversary {
    /// Score how well input amount matches expected output amount
    fn amount_match_score(
        &self,
        input_amount: u64,
        output_amount: u64,
        fee_estimate: u64,
    ) -> f64 {
        let expected_output = input_amount.saturating_sub(fee_estimate);
        let diff = (output_amount as f64 - expected_output as f64).abs();
        let relative_diff = diff / expected_output.max(1) as f64;

        if relative_diff <= self.amount_tolerance {
            // Close match: high score
            1.0 - (relative_diff / self.amount_tolerance) * 0.5
        } else {
            // Poor match: low score (but not zero - could be change output)
            0.1
        }
    }
}

impl WealthPrivacyAdversary for AmountCorrelationAdversary {
    fn name(&self) -> &'static str {
        "Amount-Correlation"
    }

    fn analyze(
        &self,
        ring: &[WealthAwareOutput],
        output: &WealthAwareOutput,
    ) -> Vec<f64> {
        // Only useful when output amount is visible
        if output.privacy_level != OutputPrivacyLevel::AmountVisible {
            // Can't use amount correlation - uniform
            return vec![1.0 / ring.len() as f64; ring.len()];
        }

        let output_amount = output.amount;

        let scores: Vec<f64> = ring.iter().map(|input| {
            match input.privacy_level {
                OutputPrivacyLevel::AmountVisible => {
                    // Both amounts visible - can correlate
                    // Estimate fee as ~1% of input
                    let fee_est = input.amount / 100;
                    self.amount_match_score(input.amount, output_amount, fee_est)
                }
                OutputPrivacyLevel::FullPrivate => {
                    // Input amount hidden - no correlation possible
                    0.5  // Neutral score
                }
            }
        }).collect();

        normalize_probabilities(&scores)
    }
}

/// Combined adversary for wealth-conditional privacy scenarios
pub struct WealthAwareCombinedAdversary {
    age: AgeAdversary,
    cluster: ClusterAdversary,
    amount: AmountCorrelationAdversary,

    /// Weight for age heuristic (0.0 to 1.0)
    pub age_weight: f64,
    /// Weight for cluster fingerprinting (0.0 to 1.0)
    pub cluster_weight: f64,
    /// Weight for amount correlation (0.0 to 1.0, only for transparent)
    pub amount_weight: f64,
}

impl Default for WealthAwareCombinedAdversary {
    fn default() -> Self {
        Self {
            age: AgeAdversary::default(),
            cluster: ClusterAdversary::default(),
            amount: AmountCorrelationAdversary::default(),
            age_weight: 0.2,
            cluster_weight: 0.5,
            amount_weight: 0.3,
        }
    }
}

impl WealthPrivacyAdversary for WealthAwareCombinedAdversary {
    fn name(&self) -> &'static str {
        "Wealth-Aware-Combined"
    }

    fn analyze(
        &self,
        ring: &[WealthAwareOutput],
        output: &WealthAwareOutput,
    ) -> Vec<f64> {
        let age_probs = self.age.analyze_base(ring);
        let cluster_probs = self.cluster.analyze_base(ring, &output.cluster_tags);

        // Amount correlation only applies for transparent outputs
        let use_amount = output.privacy_level == OutputPrivacyLevel::AmountVisible;
        let amount_probs = if use_amount {
            self.amount.analyze(ring, output)
        } else {
            vec![1.0 / ring.len() as f64; ring.len()]
        };

        // Adjust weights based on output privacy
        let (aw, cw, mw) = if use_amount {
            (self.age_weight, self.cluster_weight, self.amount_weight)
        } else {
            // No amount info - redistribute weight
            let total = self.age_weight + self.cluster_weight;
            (self.age_weight / total, self.cluster_weight / total, 0.0)
        };

        // Combine using weighted geometric mean
        let combined: Vec<f64> = (0..ring.len())
            .map(|i| {
                let age = age_probs[i].max(1e-10);
                let cluster = cluster_probs[i].max(1e-10);
                let amount = amount_probs[i].max(1e-10);

                age.powf(aw) * cluster.powf(cw) * amount.powf(mw)
            })
            .collect();

        normalize_probabilities(&combined)
    }
}

fn normalize_probabilities(scores: &[f64]) -> Vec<f64> {
    let total: f64 = scores.iter().sum();
    if total <= 0.0 {
        return vec![1.0 / scores.len() as f64; scores.len()];
    }
    scores.iter().map(|s| s / total).collect()
}
```

### Metrics

```rust
// simulation/wealth_privacy/metrics.rs

/// Comprehensive metrics for wealth-conditional privacy analysis
#[derive(Debug, Clone, Default)]
pub struct WealthPrivacyMetrics {
    // === Pool Composition ===

    /// Total outputs analyzed
    pub total_outputs: usize,
    /// Size of private pool
    pub private_pool_size: usize,
    /// Size of transparent pool
    pub transparent_pool_size: usize,
    /// Fraction of outputs that are transparent
    pub transparency_rate: f64,

    // === Value Distribution ===

    /// Total value in private pool
    pub private_pool_value: u64,
    /// Total value in transparent pool
    pub transparent_pool_value: u64,
    /// Fraction of value that's transparent
    pub value_transparency_rate: f64,

    // === Zone Distribution ===

    /// Outputs in guaranteed private zone
    pub outputs_in_private_zone: usize,
    /// Outputs in probabilistic zone
    pub outputs_in_probabilistic_zone: usize,
    /// Outputs in guaranteed transparent zone
    pub outputs_in_transparent_zone: usize,

    /// Transparency rate within probabilistic zone
    pub probabilistic_zone_transparency_rate: f64,

    // === By Source Wealth Quintile ===

    /// Transparency rate by source wealth quintile (Q1=poorest)
    pub transparency_by_quintile: [f64; 5],
    /// Average source wealth by quintile
    pub avg_source_wealth_by_quintile: [u64; 5],

    // === Anonymity Metrics ===

    /// Effective anonymity in private pool (vs various adversaries)
    pub private_pool_anonymity: HashMap<String, DistributionStats>,
    /// Effective anonymity in transparent pool
    pub transparent_pool_anonymity: HashMap<String, DistributionStats>,

    /// Bits of privacy in private pool
    pub private_pool_bits: HashMap<String, DistributionStats>,
    /// Bits of privacy in transparent pool
    pub transparent_pool_bits: HashMap<String, DistributionStats>,

    // === Gaming Indicators ===

    /// Transactions that appear to be structuring attempts
    pub suspected_structuring_count: u64,
    /// Hops needed to decay from transparent to private zone
    pub decay_chain_to_private: DistributionStats,
}

impl WealthPrivacyMetrics {
    /// Create from pool analysis
    pub fn from_pools(
        pools: &SegregatedPools,
        policy: &PrivacyPolicy,
    ) -> Self {
        let total = pools.total_size();
        let private_len = pools.private.len();
        let transparent_len = pools.transparent.len();

        // Calculate zone distribution
        let mut private_zone = 0usize;
        let mut prob_zone = 0usize;
        let mut transparent_zone = 0usize;
        let mut prob_zone_transparent = 0usize;

        for o in pools.private.iter().chain(pools.transparent.iter()) {
            match policy.zone(o.source_wealth) {
                PrivacyZone::Private => private_zone += 1,
                PrivacyZone::Probabilistic => {
                    prob_zone += 1;
                    if o.privacy_level == OutputPrivacyLevel::AmountVisible {
                        prob_zone_transparent += 1;
                    }
                }
                PrivacyZone::Transparent => transparent_zone += 1,
            }
        }

        // Calculate quintile stats
        let all_outputs: Vec<_> = pools.private.iter()
            .chain(pools.transparent.iter())
            .collect();
        let (transparency_by_quintile, avg_wealth_by_quintile) =
            calculate_quintile_stats(&all_outputs);

        Self {
            total_outputs: total,
            private_pool_size: private_len,
            transparent_pool_size: transparent_len,
            transparency_rate: pools.transparency_rate(),

            private_pool_value: pools.private.iter().map(|o| o.amount).sum(),
            transparent_pool_value: pools.transparent.iter().map(|o| o.amount).sum(),
            value_transparency_rate: pools.value_transparency_rate(),

            outputs_in_private_zone: private_zone,
            outputs_in_probabilistic_zone: prob_zone,
            outputs_in_transparent_zone: transparent_zone,
            probabilistic_zone_transparency_rate: if prob_zone > 0 {
                prob_zone_transparent as f64 / prob_zone as f64
            } else {
                0.0
            },

            transparency_by_quintile,
            avg_source_wealth_by_quintile: avg_wealth_by_quintile,

            private_pool_anonymity: HashMap::new(),
            transparent_pool_anonymity: HashMap::new(),
            private_pool_bits: HashMap::new(),
            transparent_pool_bits: HashMap::new(),

            suspected_structuring_count: 0,
            decay_chain_to_private: DistributionStats::default(),
        }
    }

    /// Generate human-readable report
    pub fn format_report(&self) -> String {
        let mut report = String::new();

        report.push_str("╔════════════════════════════════════════════════════════════════╗\n");
        report.push_str("║        WEALTH-CONDITIONAL PRIVACY SIMULATION REPORT            ║\n");
        report.push_str("╠════════════════════════════════════════════════════════════════╣\n");

        report.push_str(&format!(
            "║  Total Outputs: {:>8}                                       ║\n",
            self.total_outputs
        ));
        report.push_str(&format!(
            "║  Private Pool:  {:>8}  ({:>5.1}%)                             ║\n",
            self.private_pool_size,
            (1.0 - self.transparency_rate) * 100.0
        ));
        report.push_str(&format!(
            "║  Transparent:   {:>8}  ({:>5.1}%)                             ║\n",
            self.transparent_pool_size,
            self.transparency_rate * 100.0
        ));
        report.push_str("╚════════════════════════════════════════════════════════════════╝\n\n");

        report.push_str("ZONE DISTRIBUTION\n");
        report.push_str("─────────────────────────────────────────────────────────────────\n");
        report.push_str(&format!(
            "  Private zone:      {:>8}  (guaranteed private)\n",
            self.outputs_in_private_zone
        ));
        report.push_str(&format!(
            "  Probabilistic:     {:>8}  ({:.1}% became transparent)\n",
            self.outputs_in_probabilistic_zone,
            self.probabilistic_zone_transparency_rate * 100.0
        ));
        report.push_str(&format!(
            "  Transparent zone:  {:>8}  (guaranteed transparent)\n",
            self.outputs_in_transparent_zone
        ));

        report.push_str("\nTRANSPARENCY BY SOURCE WEALTH QUINTILE\n");
        report.push_str("─────────────────────────────────────────────────────────────────\n");
        report.push_str("Quintile     Avg Source Wealth     Transparency Rate\n");
        for (i, (&rate, &wealth)) in self.transparency_by_quintile.iter()
            .zip(self.avg_source_wealth_by_quintile.iter())
            .enumerate()
        {
            let label = match i {
                0 => "Q1 (poorest)",
                1 => "Q2",
                2 => "Q3",
                3 => "Q4",
                4 => "Q5 (richest)",
                _ => "?",
            };
            report.push_str(&format!(
                "{:<12}  {:>18}     {:>6.1}%\n",
                label,
                format_wealth(wealth),
                rate * 100.0
            ));
        }

        report.push_str("\nVALUE DISTRIBUTION\n");
        report.push_str("─────────────────────────────────────────────────────────────────\n");
        report.push_str(&format!(
            "  Value in private pool:     {}\n",
            format_wealth(self.private_pool_value)
        ));
        report.push_str(&format!(
            "  Value in transparent pool: {}\n",
            format_wealth(self.transparent_pool_value)
        ));
        report.push_str(&format!(
            "  Value transparency rate:   {:.1}%\n",
            self.value_transparency_rate * 100.0
        ));

        // Add anonymity metrics if available
        if !self.private_pool_bits.is_empty() {
            report.push_str("\nEFFECTIVE BITS OF PRIVACY BY POOL\n");
            report.push_str("─────────────────────────────────────────────────────────────────\n");

            for adversary in ["Naive", "Combined", "Wealth-Aware-Combined"] {
                if let (Some(priv_stats), Some(trans_stats)) = (
                    self.private_pool_bits.get(adversary),
                    self.transparent_pool_bits.get(adversary),
                ) {
                    report.push_str(&format!(
                        "{:<20}  Private: {:.2} bits   Transparent: {:.2} bits\n",
                        adversary,
                        priv_stats.mean,
                        trans_stats.mean
                    ));
                }
            }
        }

        report
    }
}

fn calculate_quintile_stats(outputs: &[&WealthAwareOutput]) -> ([f64; 5], [u64; 5]) {
    if outputs.is_empty() {
        return ([0.0; 5], [0; 5]);
    }

    // Sort by source wealth
    let mut sorted: Vec<_> = outputs.iter().map(|o| (o.source_wealth, o.privacy_level)).collect();
    sorted.sort_by_key(|(w, _)| *w);

    let n = sorted.len();
    let quintile_size = (n + 4) / 5;

    let mut transparency = [0.0; 5];
    let mut wealth = [0u64; 5];
    let mut counts = [0usize; 5];

    for (i, (w, level)) in sorted.into_iter().enumerate() {
        let q = (i / quintile_size).min(4);
        if level == OutputPrivacyLevel::AmountVisible {
            transparency[q] += 1.0;
        }
        wealth[q] += w;
        counts[q] += 1;
    }

    for q in 0..5 {
        if counts[q] > 0 {
            transparency[q] /= counts[q] as f64;
            wealth[q] /= counts[q] as u64;
        }
    }

    (transparency, wealth)
}

fn format_wealth(nanobth: u64) -> String {
    let bth = nanobth as f64 / 1_000_000_000.0;
    if bth >= 1_000_000.0 {
        format!("{:.2}M BTH", bth / 1_000_000.0)
    } else if bth >= 1_000.0 {
        format!("{:.2}K BTH", bth / 1_000.0)
    } else {
        format!("{:.2} BTH", bth)
    }
}
```

### CLI Commands

```rust
// bin/sim.rs additions

/// Analyze wealth-conditional privacy
#[derive(Parser)]
pub struct WealthPrivacyCmd {
    /// Full privacy threshold in BTH
    #[arg(long, default_value = "10000")]
    full_threshold: f64,

    /// Transparency threshold in BTH
    #[arg(long, default_value = "100000")]
    transparency_threshold: f64,

    /// Number of outputs in pool
    #[arg(long, default_value = "100000")]
    pool_size: usize,

    /// Number of ring simulations per pool
    #[arg(short, long, default_value = "10000")]
    num_simulations: usize,

    /// Ring size
    #[arg(long, default_value = "20")]
    ring_size: usize,

    /// Cluster wealth Gini coefficient
    #[arg(long, default_value = "0.6")]
    cluster_gini: f64,
}

/// Sweep threshold parameters
#[derive(Parser)]
pub struct ThresholdSweepCmd {
    /// Minimum full threshold to test (BTH)
    #[arg(long, default_value = "1000")]
    min_full: f64,

    /// Maximum full threshold to test (BTH)
    #[arg(long, default_value = "100000")]
    max_full: f64,

    /// Minimum transparency threshold (BTH)
    #[arg(long, default_value = "10000")]
    min_trans: f64,

    /// Maximum transparency threshold (BTH)
    #[arg(long, default_value = "1000000")]
    max_trans: f64,

    /// Number of steps per dimension
    #[arg(long, default_value = "10")]
    steps: usize,

    /// Minimum acceptable pool size
    #[arg(long, default_value = "1000")]
    min_pool_size: usize,

    /// Output CSV file
    #[arg(long)]
    output: Option<PathBuf>,
}

/// Analyze gaming resistance
#[derive(Parser)]
pub struct GamingAnalysisCmd {
    /// Number of whale agents
    #[arg(long, default_value = "100")]
    num_whales: usize,

    /// Initial whale wealth in BTH
    #[arg(long, default_value = "1000000")]
    whale_wealth: f64,

    /// Simulation rounds
    #[arg(long, default_value = "1000")]
    rounds: u64,

    /// Decay rate per hop
    #[arg(long, default_value = "0.05")]
    decay_rate: f64,

    /// Threshold in BTH
    #[arg(long, default_value = "100000")]
    threshold: f64,
}
```

## Simulation Scenarios

### Scenario 1: Baseline Pool Analysis

**Goal**: Measure pool composition at default thresholds.

```bash
cluster-tax-sim wealth-privacy \
    --full-threshold 10000 \
    --transparency-threshold 100000 \
    --pool-size 100000 \
    --num-simulations 10000
```

**Expected Output**:
- Transparency rate ~10-20% (mostly whale/exchange outputs)
- Private pool adequate (80K+ outputs)
- Transparent pool adequate (10K+ outputs)
- Transparency concentrated in Q5 (richest quintile)

### Scenario 2: Threshold Sensitivity

**Goal**: Find optimal threshold range.

```bash
cluster-tax-sim threshold-sweep \
    --min-full 1000 --max-full 100000 \
    --min-trans 10000 --max-trans 1000000 \
    --steps 20 \
    --min-pool-size 1000 \
    --output threshold_sweep.csv
```

**Expected Output**:
- Heatmap of viable threshold combinations
- Identify constraints (pool size minimums)
- Recommend conservative starting values

### Scenario 3: Gaming Resistance

**Goal**: Verify whales can't easily escape transparency.

```bash
cluster-tax-sim gaming-analysis \
    --num-whales 100 \
    --whale-wealth 1000000 \
    --rounds 1000 \
    --decay-rate 0.05 \
    --threshold 100000
```

**Expected Output**:
- Average hops needed to reach private zone
- Cost (fees) of decay strategy
- Time required (with age-gated decay)
- Confirm splitting doesn't help

## Success Criteria

| Metric | Target | Rationale |
|--------|--------|-----------|
| Private pool size | ≥10× ring size | Adequate decoy selection |
| Transparent pool size | ≥10× ring size | Adequate decoy selection |
| Private pool anonymity | ≥3.5 bits | Comparable to current system |
| Transparency rate (outputs) | 5-20% | Balance privacy/transparency |
| Transparency rate (value) | 10-40% | Large values more likely transparent |
| Q1-Q3 transparency | <5% | Normal users retain privacy |
| Q5 transparency | >30% | Wealthy users face scrutiny |
| Hops to escape (whale) | ≥30 | Gaming requires real commerce |

## Implementation Phases

### Phase 1: Core Types (Week 1)
- [ ] Add `OutputPrivacyLevel` enum
- [ ] Implement `PrivacyPolicy` with threshold logic
- [ ] Create `WealthAwareOutput` type
- [ ] Unit tests for privacy determination

### Phase 2: Pool Infrastructure (Week 2)
- [ ] Implement `SegregatedPools`
- [ ] Create `WealthAwarePoolGenerator`
- [ ] Add source wealth calculation from cluster tags
- [ ] Integration tests for pool generation

### Phase 3: Adversary Models (Week 3)
- [ ] Implement `AmountCorrelationAdversary`
- [ ] Create `WealthAwareCombinedAdversary`
- [ ] Update ring simulation for segregated pools
- [ ] Benchmark adversary effectiveness

### Phase 4: Metrics & Reporting (Week 4)
- [ ] Implement `WealthPrivacyMetrics`
- [ ] Add quintile analysis
- [ ] Create report formatter
- [ ] CSV export functionality

### Phase 5: CLI & Scenarios (Week 5)
- [ ] Add `wealth-privacy` command
- [ ] Add `threshold-sweep` command
- [ ] Add `gaming-analysis` command
- [ ] Documentation and examples

### Phase 6: Validation (Week 6)
- [ ] Run baseline scenarios
- [ ] Perform threshold sensitivity analysis
- [ ] Test gaming resistance
- [ ] Document findings and recommendations

## References

- [Wealth-Conditional Privacy Design](wealth-conditional-privacy.md)
- [Existing Privacy Simulation](../../cluster-tax/src/simulation/privacy.rs)
- [Cluster Tag Decay](cluster-tag-decay.md)
- [Progressive Fees](../concepts/progressive-fees.md)
