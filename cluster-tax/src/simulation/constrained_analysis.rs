//! Constrained Decoy Selection Privacy Analysis
//!
//! This module extends the privacy simulation framework to analyze the
//! impact of wallet-side tag-based constraints on ring signature anonymity.
//!
//! # Key Questions Addressed
//!
//! 1. **Effective Anonymity Set Size**: How do age and factor constraints
//!    reduce the anonymity set compared to unconstrained selection?
//!
//! 2. **Tag Distribution Impact**: Do certain tag profiles suffer more
//!    privacy loss than others under constraints?
//!
//! 3. **Optimal Parameters**: What constraint thresholds balance privacy
//!    with fee accuracy?
//!
//! # Reference
//!
//! - Issue #245: Wallet decoy selection constraints
//! - Issue #246: Privacy analysis (this module)
//! - `docs/design/ring-signature-tag-propagation.md`

use rand::Rng;
use std::collections::HashMap;

use super::privacy::{
    calculate_privacy_metrics, AgeAdversary, Adversary, ClusterAdversary, CombinedAdversary,
    DistributionStats, NaiveAdversary, OutputType, OutputPoolGenerator, PoolConfig,
    RingPrivacyMetrics, SimulatedOutput, MIN_AGE_BLOCKS,
};
use crate::tag::TAG_WEIGHT_SCALE;

// ============================================================================
// Constraint Configuration
// ============================================================================

/// Configuration for constrained decoy selection analysis.
#[derive(Debug, Clone)]
pub struct ConstraintConfig {
    /// Ring size (including real signer).
    pub ring_size: usize,

    /// Maximum age ratio (decoy_age / real_age or real_age / decoy_age).
    /// A value of 2.0 means decoys must be within 2x age of real input.
    pub max_age_ratio: f64,

    /// Maximum cluster factor ratio (decoy_factor / real_factor).
    /// A value of 1.5 means decoy factor must be <= 1.5x real factor.
    pub max_factor_ratio: f64,
}

impl Default for ConstraintConfig {
    fn default() -> Self {
        Self {
            ring_size: 11,
            max_age_ratio: 2.0,
            max_factor_ratio: 1.5,
        }
    }
}

impl ConstraintConfig {
    /// Number of decoys needed.
    pub fn decoys_needed(&self) -> usize {
        self.ring_size.saturating_sub(1)
    }
}

// ============================================================================
// Cluster Factor Calculation
// ============================================================================

/// Calculate the cluster factor for a simulated output.
///
/// This replicates the logic from wallet decoy_selection for simulation purposes.
pub fn calculate_cluster_factor(output: &SimulatedOutput) -> f64 {
    let total_attributed: u64 = output.cluster_tags.values().map(|&w| w as u64).sum();
    let attribution_pct = total_attributed as f64 / TAG_WEIGHT_SCALE as f64;

    // Linear interpolation: factor = 1.0 + (attribution_pct * 5.0)
    // At 0% attribution: 1.0 (anonymous)
    // At 100% attribution: 6.0 (full wealth)
    1.0 + (attribution_pct * 5.0)
}

// ============================================================================
// Constrained Decoy Selection
// ============================================================================

/// Filter outputs that meet the constraint criteria for a given real input.
pub fn filter_eligible_decoys<'a>(
    pool: &'a [SimulatedOutput],
    real_output: &SimulatedOutput,
    config: &ConstraintConfig,
) -> Vec<&'a SimulatedOutput> {
    let real_age = real_output.age_blocks;
    let real_factor = calculate_cluster_factor(real_output);

    // Age bounds
    let min_age = (real_age as f64 / config.max_age_ratio).ceil() as u64;
    let max_age = (real_age as f64 * config.max_age_ratio).floor() as u64;

    // Factor ceiling
    let max_factor = real_factor * config.max_factor_ratio;

    pool.iter()
        .filter(|o| o.id != real_output.id)
        .filter(|o| o.age_blocks >= MIN_AGE_BLOCKS)
        .filter(|o| o.age_blocks >= min_age && o.age_blocks <= max_age)
        .filter(|o| calculate_cluster_factor(o) <= max_factor)
        .collect()
}

/// Calculate the eligible decoy pool size for each output in the pool.
pub fn analyze_eligibility(
    pool: &[SimulatedOutput],
    config: &ConstraintConfig,
) -> EligibilityAnalysis {
    let mut by_output_type: HashMap<OutputType, Vec<usize>> = HashMap::new();
    let mut overall_sizes = Vec::new();
    let mut insufficient_count = 0;

    for output in pool {
        let eligible = filter_eligible_decoys(pool, output, config);
        let eligible_count = eligible.len();

        overall_sizes.push(eligible_count);
        by_output_type
            .entry(output.output_type)
            .or_default()
            .push(eligible_count);

        if eligible_count < config.decoys_needed() {
            insufficient_count += 1;
        }
    }

    EligibilityAnalysis {
        pool_size: pool.len(),
        overall_stats: DistributionStats::from_samples(
            &overall_sizes.iter().map(|&s| s as f64).collect::<Vec<_>>(),
        ),
        by_output_type: by_output_type
            .into_iter()
            .map(|(k, v)| {
                let samples: Vec<f64> = v.iter().map(|&s| s as f64).collect();
                (k, DistributionStats::from_samples(&samples))
            })
            .collect(),
        insufficient_fraction: insufficient_count as f64 / pool.len() as f64,
        config: config.clone(),
    }
}

/// Results of eligibility analysis.
#[derive(Debug, Clone)]
pub struct EligibilityAnalysis {
    /// Total pool size.
    pub pool_size: usize,
    /// Statistics on eligible decoy counts.
    pub overall_stats: DistributionStats,
    /// Statistics by output type.
    pub by_output_type: HashMap<OutputType, DistributionStats>,
    /// Fraction of outputs with insufficient eligible decoys.
    pub insufficient_fraction: f64,
    /// Constraint configuration used.
    pub config: ConstraintConfig,
}

// ============================================================================
// Constrained Ring Simulation
// ============================================================================

/// Result of a constrained ring simulation.
#[derive(Debug, Clone)]
pub struct ConstrainedRingResult {
    /// Number of eligible decoys in the pool.
    pub eligible_pool_size: usize,
    /// Whether selection succeeded.
    pub selection_succeeded: bool,
    /// Whether fallback (relaxed constraints) was used.
    pub used_fallback: bool,
    /// Privacy metrics by adversary (if selection succeeded).
    pub metrics_by_adversary: Option<HashMap<String, RingPrivacyMetrics>>,
    /// Output type of real signer.
    pub real_signer_type: OutputType,
}

/// Simulate ring formation with constraints.
pub fn simulate_constrained_ring<R: Rng>(
    pool: &[SimulatedOutput],
    config: &ConstraintConfig,
    rng: &mut R,
) -> ConstrainedRingResult {
    if pool.len() < config.ring_size {
        return ConstrainedRingResult {
            eligible_pool_size: 0,
            selection_succeeded: false,
            used_fallback: false,
            metrics_by_adversary: None,
            real_signer_type: OutputType::Standard,
        };
    }

    // Select a real signer randomly
    let real_idx = rng.gen_range(0..pool.len());
    let real_output = &pool[real_idx];

    // Filter eligible decoys
    let eligible = filter_eligible_decoys(pool, real_output, config);
    let eligible_pool_size = eligible.len();

    // Check if we have enough decoys
    let decoys_needed = config.decoys_needed();
    let (decoys, used_fallback) = if eligible.len() >= decoys_needed {
        // Select from eligible pool (weighted by age)
        let age_adversary = AgeAdversary::default();
        let selected = select_weighted(
            &eligible,
            decoys_needed,
            |o| age_adversary.weight_for_age(o.age_blocks),
            rng,
        );
        (selected, false)
    } else {
        // Fallback: try with relaxed constraints
        let relaxed = ConstraintConfig {
            ring_size: config.ring_size,
            max_age_ratio: 4.0,
            max_factor_ratio: 2.5,
        };
        let relaxed_eligible = filter_eligible_decoys(pool, real_output, &relaxed);

        if relaxed_eligible.len() >= decoys_needed {
            let age_adversary = AgeAdversary::default();
            let selected = select_weighted(
                &relaxed_eligible,
                decoys_needed,
                |o| age_adversary.weight_for_age(o.age_blocks),
                rng,
            );
            (selected, true)
        } else {
            return ConstrainedRingResult {
                eligible_pool_size,
                selection_succeeded: false,
                used_fallback: true,
                metrics_by_adversary: None,
                real_signer_type: real_output.output_type,
            };
        }
    };

    // Form ring with real signer at random position
    let real_position = rng.gen_range(0..config.ring_size);
    let mut ring = Vec::with_capacity(config.ring_size);
    let mut decoy_iter = decoys.iter();

    for i in 0..config.ring_size {
        if i == real_position {
            ring.push(real_output.clone());
        } else {
            ring.push((*decoy_iter.next().unwrap()).clone());
        }
    }

    // Simulate output tags
    let output_tags = simulate_output_tags(real_output);

    // Analyze with adversaries
    let adversaries: Vec<Box<dyn Adversary>> = vec![
        Box::new(NaiveAdversary),
        Box::new(AgeAdversary::default()),
        Box::new(ClusterAdversary::default()),
        Box::new(CombinedAdversary::default()),
    ];

    let mut metrics_by_adversary = HashMap::new();
    for adversary in &adversaries {
        let probs = adversary.analyze(&ring, &output_tags);
        let metrics = calculate_privacy_metrics(&probs, real_position);
        metrics_by_adversary.insert(adversary.name().to_string(), metrics);
    }

    ConstrainedRingResult {
        eligible_pool_size,
        selection_succeeded: true,
        used_fallback,
        metrics_by_adversary: Some(metrics_by_adversary),
        real_signer_type: real_output.output_type,
    }
}

/// Select from candidates with weighted probability.
fn select_weighted<'a, R: Rng>(
    candidates: &[&'a SimulatedOutput],
    count: usize,
    weight_fn: impl Fn(&SimulatedOutput) -> f64,
    rng: &mut R,
) -> Vec<&'a SimulatedOutput> {
    if candidates.len() <= count {
        return candidates.to_vec();
    }

    let mut selected = Vec::with_capacity(count);
    let mut remaining: Vec<&SimulatedOutput> = candidates.to_vec();

    for _ in 0..count {
        if remaining.is_empty() {
            break;
        }

        let weights: Vec<f64> = remaining.iter().map(|o| weight_fn(o)).collect();
        let total: f64 = weights.iter().sum();

        if total <= 0.0 {
            let idx = rng.gen_range(0..remaining.len());
            selected.push(remaining.remove(idx));
            continue;
        }

        let sample = rng.gen::<f64>() * total;
        let mut cumulative = 0.0;
        let mut chosen_idx = 0;

        for (i, &w) in weights.iter().enumerate() {
            cumulative += w;
            if cumulative >= sample {
                chosen_idx = i;
                break;
            }
        }

        selected.push(remaining.remove(chosen_idx));
    }

    selected
}

/// Simulate output tags after spending.
fn simulate_output_tags(real_output: &SimulatedOutput) -> HashMap<u64, u32> {
    const DECAY_RATE: f64 = 0.05;
    let retention = 1.0 - DECAY_RATE;

    let mut output_tags = HashMap::new();
    for (&cluster, &weight) in &real_output.cluster_tags {
        let decayed = (weight as f64 * retention).round() as u32;
        if decayed > 5_000 {
            output_tags.insert(cluster, decayed);
        }
    }
    output_tags
}

// ============================================================================
// Comparative Analysis
// ============================================================================

/// Compare constrained vs unconstrained decoy selection.
#[derive(Debug, Clone)]
pub struct ComparativeAnalysis {
    /// Number of simulations.
    pub num_simulations: usize,

    /// Constraint configuration used.
    pub config: ConstraintConfig,

    /// Results for constrained selection.
    pub constrained: SelectionStats,

    /// Results for unconstrained selection.
    pub unconstrained: SelectionStats,

    /// Privacy reduction (constrained vs unconstrained).
    pub privacy_reduction: PrivacyReduction,
}

/// Statistics for a selection method.
#[derive(Debug, Clone)]
pub struct SelectionStats {
    /// Success rate.
    pub success_rate: f64,
    /// Fallback rate (for constrained).
    pub fallback_rate: f64,
    /// Bits of privacy by adversary.
    pub bits_by_adversary: HashMap<String, DistributionStats>,
    /// Effective anonymity by adversary.
    pub anonymity_by_adversary: HashMap<String, DistributionStats>,
    /// Identification rate by adversary.
    pub identification_rate: HashMap<String, f64>,
}

/// Privacy reduction metrics.
#[derive(Debug, Clone)]
pub struct PrivacyReduction {
    /// Mean bits reduction by adversary.
    pub bits_reduction: HashMap<String, f64>,
    /// Mean anonymity reduction by adversary.
    pub anonymity_reduction: HashMap<String, f64>,
    /// Increased identification rate by adversary.
    pub identification_increase: HashMap<String, f64>,
}

/// Run comparative analysis between constrained and unconstrained selection.
pub fn run_comparative_analysis<R: Rng>(
    pool: &[SimulatedOutput],
    config: &ConstraintConfig,
    num_simulations: usize,
    rng: &mut R,
) -> ComparativeAnalysis {
    // Collect constrained results
    let mut constrained_results = Vec::new();
    let mut constrained_fallback_count = 0;

    for _ in 0..num_simulations {
        let result = simulate_constrained_ring(pool, config, rng);
        if result.used_fallback {
            constrained_fallback_count += 1;
        }
        constrained_results.push(result);
    }

    // Collect unconstrained results (using very relaxed constraints)
    let unconstrained_config = ConstraintConfig {
        ring_size: config.ring_size,
        max_age_ratio: 1000.0, // Effectively no age constraint
        max_factor_ratio: 1000.0, // Effectively no factor constraint
    };

    let mut unconstrained_results = Vec::new();
    for _ in 0..num_simulations {
        let result = simulate_constrained_ring(pool, &unconstrained_config, rng);
        unconstrained_results.push(result);
    }

    // Compute stats
    let constrained = compute_selection_stats(&constrained_results, constrained_fallback_count);
    let unconstrained = compute_selection_stats(&unconstrained_results, 0);

    // Compute reduction
    let privacy_reduction = compute_privacy_reduction(&constrained, &unconstrained);

    ComparativeAnalysis {
        num_simulations,
        config: config.clone(),
        constrained,
        unconstrained,
        privacy_reduction,
    }
}

fn compute_selection_stats(results: &[ConstrainedRingResult], fallback_count: usize) -> SelectionStats {
    let success_count = results.iter().filter(|r| r.selection_succeeded).count();
    let success_rate = success_count as f64 / results.len() as f64;
    let fallback_rate = fallback_count as f64 / results.len() as f64;

    let mut bits_samples: HashMap<String, Vec<f64>> = HashMap::new();
    let mut anonymity_samples: HashMap<String, Vec<f64>> = HashMap::new();
    let mut identified_counts: HashMap<String, usize> = HashMap::new();

    for result in results {
        if let Some(metrics) = &result.metrics_by_adversary {
            for (name, m) in metrics {
                bits_samples.entry(name.clone()).or_default().push(m.bits_of_privacy);
                anonymity_samples.entry(name.clone()).or_default().push(m.effective_anonymity);
                if m.real_signer_rank == 1 {
                    *identified_counts.entry(name.clone()).or_default() += 1;
                }
            }
        }
    }

    let bits_by_adversary: HashMap<String, DistributionStats> = bits_samples
        .iter()
        .map(|(k, v)| (k.clone(), DistributionStats::from_samples(v)))
        .collect();

    let anonymity_by_adversary: HashMap<String, DistributionStats> = anonymity_samples
        .iter()
        .map(|(k, v)| (k.clone(), DistributionStats::from_samples(v)))
        .collect();

    let total_successful = success_count.max(1) as f64;
    let identification_rate: HashMap<String, f64> = identified_counts
        .iter()
        .map(|(k, &v)| (k.clone(), v as f64 / total_successful))
        .collect();

    SelectionStats {
        success_rate,
        fallback_rate,
        bits_by_adversary,
        anonymity_by_adversary,
        identification_rate,
    }
}

fn compute_privacy_reduction(
    constrained: &SelectionStats,
    unconstrained: &SelectionStats,
) -> PrivacyReduction {
    let mut bits_reduction = HashMap::new();
    let mut anonymity_reduction = HashMap::new();
    let mut identification_increase = HashMap::new();

    for name in constrained.bits_by_adversary.keys() {
        if let (Some(c), Some(u)) = (
            constrained.bits_by_adversary.get(name),
            unconstrained.bits_by_adversary.get(name),
        ) {
            bits_reduction.insert(name.clone(), u.mean - c.mean);
        }

        if let (Some(c), Some(u)) = (
            constrained.anonymity_by_adversary.get(name),
            unconstrained.anonymity_by_adversary.get(name),
        ) {
            anonymity_reduction.insert(name.clone(), u.mean - c.mean);
        }

        if let (Some(&c), Some(&u)) = (
            constrained.identification_rate.get(name),
            unconstrained.identification_rate.get(name),
        ) {
            identification_increase.insert(name.clone(), c - u);
        }
    }

    PrivacyReduction {
        bits_reduction,
        anonymity_reduction,
        identification_increase,
    }
}

// ============================================================================
// Parameter Sweep
// ============================================================================

/// Result of a parameter sweep.
#[derive(Debug, Clone)]
pub struct ParameterSweepResult {
    /// Age ratio tested.
    pub age_ratio: f64,
    /// Factor ratio tested.
    pub factor_ratio: f64,
    /// Mean eligible pool size.
    pub mean_eligible_size: f64,
    /// Insufficient decoy fraction.
    pub insufficient_fraction: f64,
    /// Mean bits of privacy (Combined adversary).
    pub mean_bits_privacy: f64,
    /// Success rate.
    pub success_rate: f64,
}

/// Sweep over constraint parameters to find optimal settings.
pub fn parameter_sweep<R: Rng>(
    pool: &[SimulatedOutput],
    ring_size: usize,
    age_ratios: &[f64],
    factor_ratios: &[f64],
    num_simulations: usize,
    rng: &mut R,
) -> Vec<ParameterSweepResult> {
    let mut results = Vec::new();

    for &age_ratio in age_ratios {
        for &factor_ratio in factor_ratios {
            let config = ConstraintConfig {
                ring_size,
                max_age_ratio: age_ratio,
                max_factor_ratio: factor_ratio,
            };

            // Analyze eligibility
            let eligibility = analyze_eligibility(pool, &config);

            // Run simulations
            let mut sim_results = Vec::new();
            for _ in 0..num_simulations {
                let result = simulate_constrained_ring(pool, &config, rng);
                sim_results.push(result);
            }

            let success_count = sim_results.iter().filter(|r| r.selection_succeeded).count();
            let success_rate = success_count as f64 / num_simulations as f64;

            // Get mean bits for Combined adversary
            let bits: Vec<f64> = sim_results
                .iter()
                .filter_map(|r| r.metrics_by_adversary.as_ref())
                .filter_map(|m| m.get("Combined"))
                .map(|m| m.bits_of_privacy)
                .collect();
            let mean_bits = if bits.is_empty() {
                0.0
            } else {
                bits.iter().sum::<f64>() / bits.len() as f64
            };

            results.push(ParameterSweepResult {
                age_ratio,
                factor_ratio,
                mean_eligible_size: eligibility.overall_stats.mean,
                insufficient_fraction: eligibility.insufficient_fraction,
                mean_bits_privacy: mean_bits,
                success_rate,
            });
        }
    }

    results
}

// ============================================================================
// Report Formatting
// ============================================================================

/// Format comparative analysis as a report.
pub fn format_comparative_report(analysis: &ComparativeAnalysis) -> String {
    let mut report = String::new();

    report.push_str("╔══════════════════════════════════════════════════════════════════════════════╗\n");
    report.push_str("║           CONSTRAINED VS UNCONSTRAINED DECOY SELECTION ANALYSIS              ║\n");
    report.push_str("╠══════════════════════════════════════════════════════════════════════════════╣\n");
    report.push_str(&format!(
        "║  Simulations: {:>6}  Ring Size: {:>2}  Age Ratio: {:.1}x  Factor Ratio: {:.1}x    ║\n",
        analysis.num_simulations,
        analysis.config.ring_size,
        analysis.config.max_age_ratio,
        analysis.config.max_factor_ratio
    ));
    report.push_str("╚══════════════════════════════════════════════════════════════════════════════╝\n\n");

    report.push_str("SUCCESS RATES\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str(&format!(
        "Constrained:   Success: {:>5.1}%   Fallback: {:>5.1}%\n",
        analysis.constrained.success_rate * 100.0,
        analysis.constrained.fallback_rate * 100.0
    ));
    report.push_str(&format!(
        "Unconstrained: Success: {:>5.1}%\n",
        analysis.unconstrained.success_rate * 100.0
    ));

    report.push_str("\nBITS OF PRIVACY BY ADVERSARY\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str("Adversary            Constrained    Unconstrained    Reduction\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    let adversaries = ["Naive", "Age-Heuristic", "Cluster-Fingerprint", "Combined"];
    for adv in adversaries {
        let c_bits = analysis.constrained.bits_by_adversary.get(adv).map(|s| s.mean).unwrap_or(0.0);
        let u_bits = analysis.unconstrained.bits_by_adversary.get(adv).map(|s| s.mean).unwrap_or(0.0);
        let reduction = analysis.privacy_reduction.bits_reduction.get(adv).copied().unwrap_or(0.0);

        report.push_str(&format!(
            "{:<20} {:>6.2}          {:>6.2}          {:>+6.2}\n",
            adv, c_bits, u_bits, -reduction
        ));
    }

    report.push_str("\nIDENTIFICATION RATES (lower is better)\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str("Adversary            Constrained    Unconstrained    Change\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    for adv in adversaries {
        let c_rate = analysis.constrained.identification_rate.get(adv).copied().unwrap_or(0.0);
        let u_rate = analysis.unconstrained.identification_rate.get(adv).copied().unwrap_or(0.0);
        let increase = analysis.privacy_reduction.identification_increase.get(adv).copied().unwrap_or(0.0);

        report.push_str(&format!(
            "{:<20} {:>5.1}%          {:>5.1}%           {:>+5.1}%\n",
            adv, c_rate * 100.0, u_rate * 100.0, increase * 100.0
        ));
    }

    report.push_str("\n─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str("INTERPRETATION:\n");
    report.push_str("  • Negative reduction = constraints IMPROVE privacy (unlikely)\n");
    report.push_str("  • Positive reduction = constraints REDUCE privacy\n");
    report.push_str("  • Ideal: small reduction with high success rate and no fallbacks\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    report
}

/// Format parameter sweep results as a table.
pub fn format_parameter_sweep(results: &[ParameterSweepResult]) -> String {
    let mut report = String::new();

    report.push_str("╔══════════════════════════════════════════════════════════════════════════════╗\n");
    report.push_str("║                    CONSTRAINT PARAMETER SWEEP RESULTS                        ║\n");
    report.push_str("╚══════════════════════════════════════════════════════════════════════════════╝\n\n");

    report.push_str("Age    Factor   Eligible    Insufficient   Success    Bits of\n");
    report.push_str("Ratio  Ratio    Pool Size   Fraction       Rate       Privacy\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    for r in results {
        report.push_str(&format!(
            "{:>4.1}x  {:>5.1}x   {:>8.1}    {:>10.1}%    {:>6.1}%    {:>6.2}\n",
            r.age_ratio,
            r.factor_ratio,
            r.mean_eligible_size,
            r.insufficient_fraction * 100.0,
            r.success_rate * 100.0,
            r.mean_bits_privacy
        ));
    }

    report.push_str("\n─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str("INTERPRETATION:\n");
    report.push_str("  • Higher eligible pool = more decoy choices\n");
    report.push_str("  • Lower insufficient fraction = fewer fallbacks needed\n");
    report.push_str("  • Higher bits of privacy = better anonymity\n");
    report.push_str("  • Optimal: balance between eligible pool size and privacy bits\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    report
}

// ============================================================================
// Ring Size Recommendations
// ============================================================================

/// Analysis for ring size recommendation.
#[derive(Debug, Clone)]
pub struct RingSizeRecommendation {
    /// Minimum acceptable ring size.
    pub minimum_ring_size: usize,
    /// Recommended ring size.
    pub recommended_ring_size: usize,
    /// Analysis by ring size.
    pub by_ring_size: Vec<RingSizeAnalysis>,
    /// Rationale.
    pub rationale: String,
}

/// Analysis for a specific ring size.
#[derive(Debug, Clone)]
pub struct RingSizeAnalysis {
    /// Ring size.
    pub ring_size: usize,
    /// Mean bits of privacy (Combined adversary).
    pub mean_bits_privacy: f64,
    /// 5th percentile bits (worst case).
    pub p5_bits_privacy: f64,
    /// Success rate with constraints.
    pub success_rate: f64,
    /// Mean eligible pool size.
    pub mean_eligible_size: f64,
}

/// Analyze ring sizes and generate recommendation.
pub fn analyze_ring_sizes<R: Rng>(
    pool: &[SimulatedOutput],
    ring_sizes: &[usize],
    constraint_config: &ConstraintConfig,
    num_simulations: usize,
    rng: &mut R,
) -> RingSizeRecommendation {
    let mut analyses = Vec::new();

    for &ring_size in ring_sizes {
        let config = ConstraintConfig {
            ring_size,
            max_age_ratio: constraint_config.max_age_ratio,
            max_factor_ratio: constraint_config.max_factor_ratio,
        };

        // Analyze eligibility
        let eligibility = analyze_eligibility(pool, &config);

        // Run simulations
        let mut sim_results = Vec::new();
        for _ in 0..num_simulations {
            let result = simulate_constrained_ring(pool, &config, rng);
            sim_results.push(result);
        }

        let success_count = sim_results.iter().filter(|r| r.selection_succeeded).count();
        let success_rate = success_count as f64 / num_simulations as f64;

        // Get bits distribution for Combined adversary
        let bits: Vec<f64> = sim_results
            .iter()
            .filter_map(|r| r.metrics_by_adversary.as_ref())
            .filter_map(|m| m.get("Combined"))
            .map(|m| m.bits_of_privacy)
            .collect();

        let stats = DistributionStats::from_samples(&bits);

        analyses.push(RingSizeAnalysis {
            ring_size,
            mean_bits_privacy: stats.mean,
            p5_bits_privacy: stats.percentile_5,
            success_rate,
            mean_eligible_size: eligibility.overall_stats.mean,
        });
    }

    // Determine minimum: at least 2 bits of privacy at 5th percentile
    let minimum = analyses
        .iter()
        .filter(|a| a.p5_bits_privacy >= 2.0 && a.success_rate >= 0.95)
        .min_by_key(|a| a.ring_size)
        .map(|a| a.ring_size)
        .unwrap_or(11);

    // Recommended: at least 3 bits of privacy mean with good success rate
    let recommended = analyses
        .iter()
        .filter(|a| a.mean_bits_privacy >= 3.0 && a.success_rate >= 0.99)
        .min_by_key(|a| a.ring_size)
        .map(|a| a.ring_size)
        .unwrap_or(minimum.max(11));

    let rationale = format!(
        "Minimum ring size {} provides 2+ bits privacy at 5th percentile with 95%+ success rate. \
         Recommended ring size {} provides 3+ bits mean privacy with 99%+ success rate. \
         For comparison, Monero uses ring size 16.",
        minimum, recommended
    );

    RingSizeRecommendation {
        minimum_ring_size: minimum,
        recommended_ring_size: recommended,
        by_ring_size: analyses,
        rationale,
    }
}

/// Format ring size recommendation as a report.
pub fn format_ring_size_report(rec: &RingSizeRecommendation) -> String {
    let mut report = String::new();

    report.push_str("╔══════════════════════════════════════════════════════════════════════════════╗\n");
    report.push_str("║                        RING SIZE ANALYSIS REPORT                             ║\n");
    report.push_str("╚══════════════════════════════════════════════════════════════════════════════╝\n\n");

    report.push_str(&format!("MINIMUM RING SIZE:     {}\n", rec.minimum_ring_size));
    report.push_str(&format!("RECOMMENDED RING SIZE: {}\n\n", rec.recommended_ring_size));

    report.push_str("ANALYSIS BY RING SIZE\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str("Ring    Mean     5th%     Success    Eligible\n");
    report.push_str("Size    Bits     Bits     Rate       Pool Size\n");
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    for a in &rec.by_ring_size {
        let marker = if a.ring_size == rec.recommended_ring_size {
            " <-- RECOMMENDED"
        } else if a.ring_size == rec.minimum_ring_size {
            " <-- MINIMUM"
        } else {
            ""
        };

        report.push_str(&format!(
            "{:>4}    {:>5.2}    {:>5.2}    {:>5.1}%     {:>8.1}{}\n",
            a.ring_size,
            a.mean_bits_privacy,
            a.p5_bits_privacy,
            a.success_rate * 100.0,
            a.mean_eligible_size,
            marker
        ));
    }

    report.push_str("\n─────────────────────────────────────────────────────────────────────────────────\n");
    report.push_str("RATIONALE:\n");
    report.push_str(&format!("{}\n", rec.rationale));
    report.push_str("─────────────────────────────────────────────────────────────────────────────────\n");

    report
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_pool() -> Vec<SimulatedOutput> {
        let config = PoolConfig {
            pool_size: 10_000,
            ..Default::default()
        };
        let mut gen = OutputPoolGenerator::new(config);
        let mut rng = rand::thread_rng();
        gen.generate_pool(&mut rng)
    }

    #[test]
    fn test_cluster_factor_calculation() {
        let mut output = SimulatedOutput {
            id: 1,
            age_blocks: 1000,
            output_type: OutputType::Standard,
            cluster_tags: HashMap::new(),
            hops_since_mint: 5,
        };

        // Empty tags = factor 1.0
        assert!((calculate_cluster_factor(&output) - 1.0).abs() < 0.001);

        // 50% attribution = factor 3.5
        output.cluster_tags.insert(1, TAG_WEIGHT_SCALE / 2);
        assert!((calculate_cluster_factor(&output) - 3.5).abs() < 0.001);

        // 100% attribution = factor 6.0
        output.cluster_tags.clear();
        output.cluster_tags.insert(1, TAG_WEIGHT_SCALE);
        assert!((calculate_cluster_factor(&output) - 6.0).abs() < 0.001);
    }

    #[test]
    fn test_eligibility_analysis() {
        let pool = create_test_pool();
        let config = ConstraintConfig::default();

        let analysis = analyze_eligibility(&pool, &config);

        // Should have positive mean eligible size
        assert!(analysis.overall_stats.mean > 0.0);

        // Should have low insufficient fraction with default params
        assert!(
            analysis.insufficient_fraction < 0.5,
            "Too many insufficient: {}",
            analysis.insufficient_fraction
        );
    }

    #[test]
    fn test_constrained_ring_simulation() {
        let pool = create_test_pool();
        let config = ConstraintConfig::default();
        let mut rng = rand::thread_rng();

        let result = simulate_constrained_ring(&pool, &config, &mut rng);

        // Should succeed with reasonable pool
        assert!(result.selection_succeeded, "Simulation should succeed");

        // Should have metrics
        assert!(result.metrics_by_adversary.is_some());
        let metrics = result.metrics_by_adversary.unwrap();
        assert!(metrics.contains_key("Combined"));
    }

    #[test]
    fn test_comparative_analysis() {
        let pool = create_test_pool();
        let config = ConstraintConfig::default();
        let mut rng = rand::thread_rng();

        let analysis = run_comparative_analysis(&pool, &config, 100, &mut rng);

        // Both should have reasonable success rates
        assert!(analysis.constrained.success_rate > 0.5);
        assert!(analysis.unconstrained.success_rate > 0.9);

        // Privacy reduction should be small (< 1 bit)
        if let Some(&reduction) = analysis.privacy_reduction.bits_reduction.get("Combined") {
            assert!(
                reduction < 1.0,
                "Privacy reduction too large: {}",
                reduction
            );
        }
    }

    #[test]
    fn test_parameter_sweep() {
        let pool = create_test_pool();
        let mut rng = rand::thread_rng();

        let results = parameter_sweep(
            &pool,
            11,
            &[1.5, 2.0, 3.0],
            &[1.25, 1.5, 2.0],
            50,
            &mut rng,
        );

        assert_eq!(results.len(), 9); // 3 x 3

        // Looser constraints should have higher eligible pool sizes
        let tight = results.iter().find(|r| r.age_ratio == 1.5 && r.factor_ratio == 1.25);
        let loose = results.iter().find(|r| r.age_ratio == 3.0 && r.factor_ratio == 2.0);

        if let (Some(t), Some(l)) = (tight, loose) {
            assert!(
                l.mean_eligible_size > t.mean_eligible_size,
                "Loose constraints should have larger eligible pool"
            );
        }
    }

    #[test]
    fn test_ring_size_recommendation() {
        let pool = create_test_pool();
        let config = ConstraintConfig::default();
        let mut rng = rand::thread_rng();

        let rec = analyze_ring_sizes(&pool, &[7, 11, 16, 20], &config, 50, &mut rng);

        // Should have a minimum and recommended
        assert!(rec.minimum_ring_size >= 7);
        assert!(rec.recommended_ring_size >= rec.minimum_ring_size);

        // Should have analyses for all ring sizes
        assert_eq!(rec.by_ring_size.len(), 4);
    }
}
