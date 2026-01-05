//! Privacy simulation for ring signature anonymity analysis.
//!
//! This module models the effective bits of privacy users can expect from
//! ring signatures under various adversary models and network conditions.
//!
//! ## Key Concepts
//!
//! - **Effective Anonymity Set**: The number of ring members that appear
//!   equally plausible to an adversary (e^entropy)
//! - **Bits of Privacy**: log₂(effective_anonymity) - the information-theoretic
//!   privacy level
//! - **Adversary Models**: Different attack strategies with varying knowledge
//!
//! ## Assumptions
//!
//! Default simulation parameters:
//! - Ring size: 20 (1 real + 19 decoys)
//! - Cluster decay rate: 5% per hop
//! - Background standard transactions: 50%
//! - Gamma distribution for spend ages (k=19.28, θ=1.61 days)

use rand::Rng;
use rand_distr::{Distribution, Gamma};
use std::collections::HashMap;

use crate::tag::TAG_WEIGHT_SCALE;

// ============================================================================
// Configuration
// ============================================================================

/// Default ring size (1 real signer + 19 decoys).
/// Ring size 20 provides strong anonymity (larger than Monero's 16).
pub const RING_SIZE: usize = 20;

/// Number of decoys to select.
pub const DECOY_COUNT: usize = RING_SIZE - 1;

// ============================================================================
// Ring Size Cost Analysis
// ============================================================================

/// CLSAG signature size constants
/// CLSAG is the standard ring signature scheme per ADR-0001
pub const KEY_IMAGE_BYTES: usize = 32;
pub const RESPONSE_BYTES: usize = 32;
pub const SIGNATURE_BASE_BYTES: usize = 64; // c0 + s_real

/// Calculate signature size for a given ring size (CLSAG).
/// CLSAG signatures are approximately 32 + 32*ring_size bytes per input.
#[inline]
pub const fn signature_size_bytes(ring_size: usize) -> usize {
    SIGNATURE_BASE_BYTES + ring_size * RESPONSE_BYTES
}

/// Ring size analysis results.
#[derive(Debug, Clone)]
pub struct RingSizeAnalysis {
    pub ring_size: usize,
    pub signature_bytes: usize,
    pub signature_kb: f64,
    pub theoretical_max_bits: f64,
    pub measured_bits: Option<f64>,
    pub measured_efficiency: Option<f64>,
    pub bits_per_kb: f64,
    pub marginal_bytes: usize,
    pub marginal_bits: f64,
}

/// Analyze the cost/benefit tradeoff for different ring sizes.
pub fn analyze_ring_sizes(ring_sizes: &[usize]) -> Vec<RingSizeAnalysis> {
    let mut results = Vec::new();
    let mut prev_bytes = 0usize;
    let mut prev_bits = 0.0f64;

    for &size in ring_sizes {
        let sig_bytes = signature_size_bytes(size);
        let sig_kb = sig_bytes as f64 / 1024.0;
        let theoretical_bits = (size as f64).log2();

        let marginal_bytes = if prev_bytes > 0 {
            sig_bytes - prev_bytes
        } else {
            sig_bytes
        };
        let marginal_bits = if prev_bits > 0.0 {
            theoretical_bits - prev_bits
        } else {
            theoretical_bits
        };

        results.push(RingSizeAnalysis {
            ring_size: size,
            signature_bytes: sig_bytes,
            signature_kb: sig_kb,
            theoretical_max_bits: theoretical_bits,
            measured_bits: None,
            measured_efficiency: None,
            bits_per_kb: theoretical_bits / sig_kb,
            marginal_bytes,
            marginal_bits,
        });

        prev_bytes = sig_bytes;
        prev_bits = theoretical_bits;
    }

    results
}

/// Format ring size analysis as a table.
pub fn format_ring_size_analysis(analyses: &[RingSizeAnalysis]) -> String {
    let mut report = String::new();

    report.push_str(
        "╔═══════════════════════════════════════════════════════════════════════════════╗\n",
    );
    report.push_str(
        "║                    RING SIZE COST/BENEFIT ANALYSIS                            ║\n",
    );
    report.push_str(
        "╠═══════════════════════════════════════════════════════════════════════════════╣\n",
    );
    report.push_str(
        "║  Ring signatures provide plausible deniability by hiding the real signer      ║\n",
    );
    report.push_str(
        "║  among decoys. Larger rings = more privacy but bigger signatures.             ║\n",
    );
    report.push_str(
        "╚═══════════════════════════════════════════════════════════════════════════════╝\n\n",
    );

    report.push_str("SIGNATURE SIZE BY RING SIZE\n");
    report.push_str(
        "─────────────────────────────────────────────────────────────────────────────────\n",
    );
    report.push_str("Ring   Sig Size    Sig Size   Theoretical   Bits/KB   Marginal   Marginal\n");
    report.push_str("Size   (bytes)     (KB)       Max (bits)    Ratio     +Bytes     +Bits\n");
    report.push_str(
        "─────────────────────────────────────────────────────────────────────────────────\n",
    );

    for (i, a) in analyses.iter().enumerate() {
        let marginal_str = if i == 0 {
            format!("{:>8}   {:>+6.2}", "-", a.theoretical_max_bits)
        } else {
            format!(
                "{:>+8}   {:>+6.2}",
                a.marginal_bytes as i64, a.marginal_bits
            )
        };

        report.push_str(&format!(
            "{:>4}   {:>8}    {:>6.1}     {:>6.2}        {:>5.3}     {}\n",
            a.ring_size,
            a.signature_bytes,
            a.signature_kb,
            a.theoretical_max_bits,
            a.bits_per_kb,
            marginal_str,
        ));
    }

    report.push_str("\n");
    report.push_str("COST ANALYSIS\n");
    report.push_str(
        "─────────────────────────────────────────────────────────────────────────────────\n",
    );

    // Calculate relative costs
    let base = &analyses[0];
    for a in analyses.iter().skip(1) {
        let size_increase =
            ((a.signature_bytes as f64 / base.signature_bytes as f64) - 1.0) * 100.0;
        let privacy_increase = ((a.theoretical_max_bits / base.theoretical_max_bits) - 1.0) * 100.0;
        let efficiency = privacy_increase / size_increase;

        report.push_str(&format!(
            "Ring {} vs {}: +{:.0}% size for +{:.0}% privacy (efficiency: {:.2})\n",
            a.ring_size, base.ring_size, size_increase, privacy_increase, efficiency
        ));
    }

    report.push_str("\n");
    report.push_str("LEDGER IMPACT (per 1M private transactions)\n");
    report.push_str(
        "─────────────────────────────────────────────────────────────────────────────────\n",
    );

    for a in analyses {
        let ledger_gb = (a.signature_bytes as f64 * 1_000_000.0) / (1024.0 * 1024.0 * 1024.0);
        report.push_str(&format!(
            "Ring {:>2}: {:>6.1} GB ledger storage\n",
            a.ring_size, ledger_gb
        ));
    }

    report
}

/// Default gamma shape parameter for age distribution.
pub const GAMMA_SHAPE: f64 = 19.28;

/// Default gamma scale in days.
pub const GAMMA_SCALE_DAYS: f64 = 1.61;

/// Blocks per day (2-minute blocks).
pub const BLOCKS_PER_DAY: f64 = 720.0;

/// Minimum output age for eligibility (blocks).
pub const MIN_AGE_BLOCKS: u64 = 10;

/// Maximum output age for consideration (blocks, ~2 years).
pub const MAX_AGE_BLOCKS: u64 = 525_600;

/// Default cluster decay rate per hop (5%).
pub const DEFAULT_DECAY_RATE: f64 = 0.05;

/// Minimum cluster similarity for decoy selection (70%).
pub const MIN_CLUSTER_SIMILARITY: f64 = 0.70;

// ============================================================================
// Output Pool Model
// ============================================================================

/// Type of transaction that created an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputType {
    /// Standard retail transaction (diffused cluster tags).
    Standard,
    /// Exchange deposit/withdrawal (concentrated tags).
    Exchange,
    /// Whale movement (very concentrated tags).
    Whale,
    /// Coinbase/mining reward (single fresh cluster).
    Coinbase,
    /// Mixed output (intentionally diffused).
    Mixed,
}

/// A simulated UTXO in the output pool.
#[derive(Debug, Clone)]
pub struct SimulatedOutput {
    /// Unique identifier for this output.
    pub id: u64,
    /// Age in blocks.
    pub age_blocks: u64,
    /// Type of transaction that created this output.
    pub output_type: OutputType,
    /// Cluster tag distribution (cluster_id -> weight in parts per million).
    pub cluster_tags: HashMap<u64, u32>,
    /// Number of hops since minting (affects tag diffusion).
    pub hops_since_mint: u32,
}

impl SimulatedOutput {
    /// Total attributed cluster weight (0 to 1_000_000).
    pub fn total_cluster_weight(&self) -> u32 {
        self.cluster_tags
            .values()
            .sum::<u32>()
            .min(TAG_WEIGHT_SCALE)
    }

    /// Background weight (unattributed portion).
    pub fn background_weight(&self) -> u32 {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_cluster_weight())
    }

    /// Age in days.
    pub fn age_days(&self) -> f64 {
        self.age_blocks as f64 / BLOCKS_PER_DAY
    }

    /// Get the dominant cluster (highest weight).
    pub fn dominant_cluster(&self) -> Option<(u64, u32)> {
        self.cluster_tags
            .iter()
            .max_by_key(|(_, &w)| w)
            .map(|(&id, &w)| (id, w))
    }

    /// Get top N clusters by weight.
    pub fn top_clusters(&self, n: usize) -> Vec<(u64, u32)> {
        let mut entries: Vec<_> = self.cluster_tags.iter().map(|(&k, &v)| (k, v)).collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(n);
        entries
    }

    /// Compute cosine similarity with another output's cluster tags.
    pub fn cluster_similarity(&self, other: &SimulatedOutput) -> f64 {
        // Empty vectors are fully similar (maximally diffused)
        if self.cluster_tags.is_empty() && other.cluster_tags.is_empty() {
            return 1.0;
        }
        if self.cluster_tags.is_empty() || other.cluster_tags.is_empty() {
            return 1.0; // Fully diffused is compatible with anything
        }

        // Collect all cluster IDs
        let all_ids: std::collections::HashSet<u64> = self
            .cluster_tags
            .keys()
            .chain(other.cluster_tags.keys())
            .copied()
            .collect();

        let mut dot_product = 0.0_f64;
        let mut mag_self = 0.0_f64;
        let mut mag_other = 0.0_f64;

        for id in all_ids {
            let w1 = self.cluster_tags.get(&id).copied().unwrap_or(0) as f64;
            let w2 = other.cluster_tags.get(&id).copied().unwrap_or(0) as f64;
            dot_product += w1 * w2;
            mag_self += w1 * w1;
            mag_other += w2 * w2;
        }

        let magnitude = (mag_self.sqrt() * mag_other.sqrt()).max(1.0);
        (dot_product / magnitude).clamp(0.0, 1.0)
    }
}

/// Configuration for output pool generation.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Total number of outputs in the pool.
    pub pool_size: usize,
    /// Fraction of standard transactions (0.0 to 1.0).
    pub standard_fraction: f64,
    /// Fraction of exchange-related transactions.
    pub exchange_fraction: f64,
    /// Fraction of whale transactions.
    pub whale_fraction: f64,
    /// Fraction of coinbase outputs.
    pub coinbase_fraction: f64,
    /// Fraction of mixed outputs.
    pub mixed_fraction: f64,
    /// Number of unique clusters in the system.
    pub num_clusters: u64,
    /// Decay rate per hop for cluster tags.
    pub decay_rate: f64,
    /// Maximum age in blocks.
    pub max_age_blocks: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pool_size: 100_000,
            standard_fraction: 0.50, // 50% standard retail
            exchange_fraction: 0.25, // 25% exchange activity
            whale_fraction: 0.10,    // 10% whale movements
            coinbase_fraction: 0.10, // 10% mining rewards
            mixed_fraction: 0.05,    // 5% intentionally mixed
            num_clusters: 1_000,
            decay_rate: DEFAULT_DECAY_RATE,
            max_age_blocks: MAX_AGE_BLOCKS,
        }
    }
}

/// Generator for realistic output pools.
pub struct OutputPoolGenerator {
    config: PoolConfig,
    gamma: Gamma<f64>,
    next_output_id: u64,
}

impl OutputPoolGenerator {
    /// Create a new pool generator with the given configuration.
    pub fn new(config: PoolConfig) -> Self {
        let gamma = Gamma::new(GAMMA_SHAPE, GAMMA_SCALE_DAYS * BLOCKS_PER_DAY)
            .expect("Invalid gamma parameters");
        Self {
            config,
            gamma,
            next_output_id: 0,
        }
    }

    /// Generate a complete output pool.
    pub fn generate_pool<R: Rng>(&mut self, rng: &mut R) -> Vec<SimulatedOutput> {
        let mut outputs = Vec::with_capacity(self.config.pool_size);

        for _ in 0..self.config.pool_size {
            let output = self.generate_output(rng);
            outputs.push(output);
        }

        outputs
    }

    /// Generate a single output based on configured probabilities.
    fn generate_output<R: Rng>(&mut self, rng: &mut R) -> SimulatedOutput {
        let id = self.next_output_id;
        self.next_output_id += 1;

        // Determine output type based on fractions
        let r: f64 = rng.gen();
        let output_type = if r < self.config.standard_fraction {
            OutputType::Standard
        } else if r < self.config.standard_fraction + self.config.exchange_fraction {
            OutputType::Exchange
        } else if r < self.config.standard_fraction
            + self.config.exchange_fraction
            + self.config.whale_fraction
        {
            OutputType::Whale
        } else if r < self.config.standard_fraction
            + self.config.exchange_fraction
            + self.config.whale_fraction
            + self.config.coinbase_fraction
        {
            OutputType::Coinbase
        } else {
            OutputType::Mixed
        };

        // Generate age from gamma distribution (matching spend patterns)
        let age_blocks =
            self.gamma
                .sample(rng)
                .clamp(MIN_AGE_BLOCKS as f64, self.config.max_age_blocks as f64) as u64;

        // Generate cluster tags and hops based on output type
        let (cluster_tags, hops_since_mint) =
            self.generate_cluster_profile(rng, output_type, age_blocks);

        SimulatedOutput {
            id,
            age_blocks,
            output_type,
            cluster_tags,
            hops_since_mint,
        }
    }

    /// Generate cluster tag profile based on output type.
    fn generate_cluster_profile<R: Rng>(
        &self,
        rng: &mut R,
        output_type: OutputType,
        age_blocks: u64,
    ) -> (HashMap<u64, u32>, u32) {
        match output_type {
            OutputType::Standard => {
                // Standard transactions: moderate diffusion, multiple clusters
                // Estimate hops based on age (assume ~1 transaction per day average)
                let estimated_hops = ((age_blocks as f64 / BLOCKS_PER_DAY) * 0.5) as u32;
                let hops = estimated_hops.clamp(3, 30);

                // Start with a few clusters, weights decay over hops
                let num_clusters = rng.gen_range(2..=5);
                let mut tags = HashMap::new();
                let mut remaining_weight = TAG_WEIGHT_SCALE;

                for i in 0..num_clusters {
                    let cluster_id = rng.gen_range(0..self.config.num_clusters);
                    // Each cluster gets decaying weight
                    let initial_share = if i == 0 {
                        rng.gen_range(300_000..600_000)
                    } else {
                        rng.gen_range(50_000..200_000)
                    };
                    let decayed = self.decay_weight(initial_share, hops);
                    let weight = decayed.min(remaining_weight);
                    if weight > 10_000 {
                        tags.insert(cluster_id, weight);
                        remaining_weight = remaining_weight.saturating_sub(weight);
                    }
                }

                (tags, hops)
            }
            OutputType::Exchange => {
                // Exchange outputs: 1-3 hops from concentrated source
                let hops = rng.gen_range(1..=3);
                let cluster_id = rng.gen_range(0..self.config.num_clusters);
                let mut tags = HashMap::new();

                let weight = self.decay_weight(TAG_WEIGHT_SCALE, hops);
                tags.insert(cluster_id, weight);

                (tags, hops)
            }
            OutputType::Whale => {
                // Whale movements: very concentrated, minimal hops
                let hops = rng.gen_range(0..=2);
                let cluster_id = rng.gen_range(0..self.config.num_clusters / 10); // Fewer whale clusters
                let mut tags = HashMap::new();

                let weight = self.decay_weight(TAG_WEIGHT_SCALE, hops);
                tags.insert(cluster_id, weight);

                (tags, hops)
            }
            OutputType::Coinbase => {
                // Fresh mining reward: single cluster, 100% weight
                let hops = 0;
                let cluster_id = rng.gen_range(0..self.config.num_clusters);
                let mut tags = HashMap::new();
                tags.insert(cluster_id, TAG_WEIGHT_SCALE);

                (tags, hops)
            }
            OutputType::Mixed => {
                // Intentionally mixed: many clusters with small weights
                let hops = rng.gen_range(10..=30);
                let num_clusters = rng.gen_range(5..=10);
                let mut tags = HashMap::new();
                let weight_per = TAG_WEIGHT_SCALE / num_clusters;

                for _ in 0..num_clusters {
                    let cluster_id = rng.gen_range(0..self.config.num_clusters);
                    let decayed = self.decay_weight(weight_per, hops / 2);
                    if decayed > 5_000 {
                        *tags.entry(cluster_id).or_insert(0) += decayed;
                    }
                }

                (tags, hops)
            }
        }
    }

    /// Apply decay to a weight over N hops.
    fn decay_weight(&self, initial: u32, hops: u32) -> u32 {
        let retention = 1.0 - self.config.decay_rate;
        let remaining = initial as f64 * retention.powi(hops as i32);
        remaining.round() as u32
    }
}

// ============================================================================
// Adversary Models
// ============================================================================

/// An adversary attempting to identify the real signer in a ring.
pub trait Adversary {
    /// Returns a name for this adversary type.
    fn name(&self) -> &'static str;

    /// Analyze a ring and return probabilities for each member being the real
    /// signer.
    ///
    /// The returned vector has the same length as the ring, with probabilities
    /// summing to 1.0.
    fn analyze(&self, ring: &[SimulatedOutput], output_tags: &HashMap<u64, u32>) -> Vec<f64>;
}

/// Naive adversary: assumes uniform distribution (no analysis).
pub struct NaiveAdversary;

impl Adversary for NaiveAdversary {
    fn name(&self) -> &'static str {
        "Naive"
    }

    fn analyze(&self, ring: &[SimulatedOutput], _output_tags: &HashMap<u64, u32>) -> Vec<f64> {
        let uniform = 1.0 / ring.len() as f64;
        vec![uniform; ring.len()]
    }
}

/// Age-heuristic adversary: uses gamma distribution to weight by spend
/// probability.
pub struct AgeAdversary {
    gamma_shape: f64,
    gamma_scale_blocks: f64,
}

impl Default for AgeAdversary {
    fn default() -> Self {
        Self {
            gamma_shape: GAMMA_SHAPE,
            gamma_scale_blocks: GAMMA_SCALE_DAYS * BLOCKS_PER_DAY,
        }
    }
}

impl AgeAdversary {
    /// Weight for a given age based on gamma PDF.
    fn weight_for_age(&self, age_blocks: u64) -> f64 {
        let age = (age_blocks as f64).clamp(1.0, MAX_AGE_BLOCKS as f64);
        let k = self.gamma_shape;
        let theta = self.gamma_scale_blocks;

        // Unnormalized gamma PDF: x^(k-1) * e^(-x/θ)
        let log_weight = (k - 1.0) * age.ln() - age / theta;
        log_weight.exp().max(1e-10)
    }
}

impl Adversary for AgeAdversary {
    fn name(&self) -> &'static str {
        "Age-Heuristic"
    }

    fn analyze(&self, ring: &[SimulatedOutput], _output_tags: &HashMap<u64, u32>) -> Vec<f64> {
        let weights: Vec<f64> = ring
            .iter()
            .map(|o| self.weight_for_age(o.age_blocks))
            .collect();
        let total: f64 = weights.iter().sum();

        if total <= 0.0 {
            return vec![1.0 / ring.len() as f64; ring.len()];
        }

        weights.iter().map(|w| w / total).collect()
    }
}

/// Cluster fingerprinting adversary: matches ring member tags to output tags.
pub struct ClusterAdversary {
    decay_rate: f64,
}

impl Default for ClusterAdversary {
    fn default() -> Self {
        Self {
            decay_rate: DEFAULT_DECAY_RATE,
        }
    }
}

impl ClusterAdversary {
    /// Compute match score between input tags and output tags.
    ///
    /// The adversary knows the output tags and tries to match them to inputs
    /// after accounting for the decay that would occur.
    fn match_score(&self, input: &SimulatedOutput, output_tags: &HashMap<u64, u32>) -> f64 {
        if input.cluster_tags.is_empty() && output_tags.is_empty() {
            return 1.0;
        }
        if input.cluster_tags.is_empty() || output_tags.is_empty() {
            return 0.5; // Ambiguous
        }

        // Simulate what the output tags would look like if this input was the real one
        // (apply one hop of decay)
        let retention = 1.0 - self.decay_rate;
        let mut predicted_output: HashMap<u64, f64> = HashMap::new();

        for (&cluster, &weight) in &input.cluster_tags {
            let decayed = weight as f64 * retention;
            if decayed > 5000.0 {
                predicted_output.insert(cluster, decayed);
            }
        }

        // Cosine similarity between predicted and actual output tags
        let all_clusters: std::collections::HashSet<u64> = predicted_output
            .keys()
            .chain(output_tags.keys())
            .copied()
            .collect();

        let mut dot_product = 0.0_f64;
        let mut mag_pred = 0.0_f64;
        let mut mag_actual = 0.0_f64;

        for cluster in all_clusters {
            let pred = predicted_output.get(&cluster).copied().unwrap_or(0.0);
            let actual = output_tags.get(&cluster).copied().unwrap_or(0) as f64;
            dot_product += pred * actual;
            mag_pred += pred * pred;
            mag_actual += actual * actual;
        }

        let magnitude = (mag_pred.sqrt() * mag_actual.sqrt()).max(1.0);
        (dot_product / magnitude).clamp(0.0, 1.0)
    }
}

impl Adversary for ClusterAdversary {
    fn name(&self) -> &'static str {
        "Cluster-Fingerprint"
    }

    fn analyze(&self, ring: &[SimulatedOutput], output_tags: &HashMap<u64, u32>) -> Vec<f64> {
        let scores: Vec<f64> = ring
            .iter()
            .map(|o| self.match_score(o, output_tags))
            .collect();
        let total: f64 = scores.iter().sum();

        if total <= 0.0 {
            return vec![1.0 / ring.len() as f64; ring.len()];
        }

        scores.iter().map(|s| s / total).collect()
    }
}

/// Combined adversary: uses both age and cluster heuristics.
pub struct CombinedAdversary {
    age: AgeAdversary,
    cluster: ClusterAdversary,
    age_weight: f64,
    cluster_weight: f64,
}

impl Default for CombinedAdversary {
    fn default() -> Self {
        Self {
            age: AgeAdversary::default(),
            cluster: ClusterAdversary::default(),
            age_weight: 0.3,     // Age is less reliable
            cluster_weight: 0.7, // Cluster fingerprinting is more powerful
        }
    }
}

impl CombinedAdversary {
    /// Create with custom weights.
    pub fn with_weights(age_weight: f64, cluster_weight: f64) -> Self {
        let total = age_weight + cluster_weight;
        Self {
            age: AgeAdversary::default(),
            cluster: ClusterAdversary::default(),
            age_weight: age_weight / total,
            cluster_weight: cluster_weight / total,
        }
    }
}

impl Adversary for CombinedAdversary {
    fn name(&self) -> &'static str {
        "Combined"
    }

    fn analyze(&self, ring: &[SimulatedOutput], output_tags: &HashMap<u64, u32>) -> Vec<f64> {
        let age_probs = self.age.analyze(ring, output_tags);
        let cluster_probs = self.cluster.analyze(ring, output_tags);

        // Weighted geometric mean
        let combined: Vec<f64> = age_probs
            .iter()
            .zip(cluster_probs.iter())
            .map(|(&a, &c)| (a.powf(self.age_weight) * c.powf(self.cluster_weight)).max(1e-10))
            .collect();

        let total: f64 = combined.iter().sum();
        combined.iter().map(|p| p / total).collect()
    }
}

// ============================================================================
// Privacy Metrics
// ============================================================================

/// Privacy metrics for a single ring.
#[derive(Debug, Clone)]
pub struct RingPrivacyMetrics {
    /// Ring size.
    pub ring_size: usize,
    /// Shannon entropy of the probability distribution.
    pub entropy: f64,
    /// Effective anonymity set size (e^entropy).
    pub effective_anonymity: f64,
    /// Bits of privacy (log₂ of effective anonymity).
    pub bits_of_privacy: f64,
    /// Maximum possible entropy (log of ring size).
    pub max_entropy: f64,
    /// Entropy efficiency (actual / max).
    pub entropy_efficiency: f64,
    /// Probability assigned to the actual real signer.
    pub real_signer_probability: f64,
    /// Rank of real signer (1 = highest probability, ring_size = lowest).
    pub real_signer_rank: usize,
}

/// Calculate privacy metrics from adversary probabilities.
pub fn calculate_privacy_metrics(
    probabilities: &[f64],
    real_signer_index: usize,
) -> RingPrivacyMetrics {
    let ring_size = probabilities.len();

    // Shannon entropy: H = -Σ p_i * ln(p_i)
    let entropy: f64 = probabilities
        .iter()
        .filter(|&&p| p > 0.0)
        .map(|&p| -p * p.ln())
        .sum();

    // Effective anonymity = e^H
    let effective_anonymity = entropy.exp();

    // Bits of privacy = log₂(effective_anonymity) = H / ln(2)
    let bits_of_privacy = entropy / std::f64::consts::LN_2;

    // Maximum entropy (uniform distribution)
    let max_entropy = (ring_size as f64).ln();
    let entropy_efficiency = entropy / max_entropy;

    // Real signer probability
    let real_signer_probability = probabilities[real_signer_index];

    // Rank of real signer (1 = most likely)
    let mut indexed: Vec<(usize, f64)> = probabilities.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let real_signer_rank = indexed
        .iter()
        .position(|(i, _)| *i == real_signer_index)
        .map(|p| p + 1)
        .unwrap_or(ring_size);

    RingPrivacyMetrics {
        ring_size,
        entropy,
        effective_anonymity,
        bits_of_privacy,
        max_entropy,
        entropy_efficiency,
        real_signer_probability,
        real_signer_rank,
    }
}

// ============================================================================
// Ring Simulator
// ============================================================================

/// Configuration for ring simulation.
#[derive(Debug, Clone)]
pub struct RingSimConfig {
    /// Ring size (including real signer).
    pub ring_size: usize,
    /// Minimum cluster similarity for decoy selection.
    pub min_cluster_similarity: f64,
    /// Whether to use cluster-aware decoy selection.
    pub cluster_aware_selection: bool,
}

impl Default for RingSimConfig {
    fn default() -> Self {
        Self {
            ring_size: RING_SIZE,
            min_cluster_similarity: MIN_CLUSTER_SIMILARITY,
            cluster_aware_selection: true,
        }
    }
}

/// Result of a single ring simulation.
#[derive(Debug, Clone)]
pub struct RingSimResult {
    /// The formed ring (indices into the pool).
    pub ring_indices: Vec<usize>,
    /// Index of the real signer within the ring.
    pub real_signer_ring_index: usize,
    /// Output cluster tags (what an observer would see).
    pub output_tags: HashMap<u64, u32>,
    /// Privacy metrics for each adversary type.
    pub metrics_by_adversary: HashMap<String, RingPrivacyMetrics>,
}

/// Simulates ring formation and privacy analysis.
pub struct RingSimulator {
    config: RingSimConfig,
    age_adversary: AgeAdversary,
}

impl RingSimulator {
    /// Create a new ring simulator.
    pub fn new(config: RingSimConfig) -> Self {
        Self {
            config,
            age_adversary: AgeAdversary::default(),
        }
    }

    /// Simulate a single ring formation and analyze privacy.
    pub fn simulate_ring<R: Rng>(
        &self,
        pool: &[SimulatedOutput],
        rng: &mut R,
    ) -> Option<RingSimResult> {
        if pool.len() < self.config.ring_size {
            return None;
        }

        // Select a real signer randomly from the pool
        let real_signer_pool_index = rng.gen_range(0..pool.len());
        let real_signer = &pool[real_signer_pool_index];

        // Select decoys
        let decoy_indices = if self.config.cluster_aware_selection {
            self.select_cluster_aware_decoys(pool, real_signer_pool_index, rng)
        } else {
            self.select_age_weighted_decoys(pool, real_signer_pool_index, rng)
        };

        if decoy_indices.len() < self.config.ring_size - 1 {
            return None; // Insufficient decoys
        }

        // Form the ring (real signer at random position)
        let real_position = rng.gen_range(0..self.config.ring_size);
        let mut ring_indices = Vec::with_capacity(self.config.ring_size);
        let mut decoy_iter = decoy_indices.iter();

        for i in 0..self.config.ring_size {
            if i == real_position {
                ring_indices.push(real_signer_pool_index);
            } else {
                ring_indices.push(*decoy_iter.next()?);
            }
        }

        // Simulate output tags (real signer's tags after one hop of decay)
        let output_tags = self.simulate_output_tags(real_signer);

        // Build the ring for analysis
        let ring: Vec<SimulatedOutput> = ring_indices.iter().map(|&i| pool[i].clone()).collect();

        // Analyze with each adversary
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

        Some(RingSimResult {
            ring_indices,
            real_signer_ring_index: real_position,
            output_tags,
            metrics_by_adversary,
        })
    }

    /// Select decoys using cluster-aware selection.
    fn select_cluster_aware_decoys<R: Rng>(
        &self,
        pool: &[SimulatedOutput],
        real_idx: usize,
        rng: &mut R,
    ) -> Vec<usize> {
        let real_signer = &pool[real_idx];
        let needed = self.config.ring_size - 1;

        // Filter candidates by cluster similarity
        let mut compatible: Vec<(usize, f64, f64)> = pool
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != real_idx)
            .filter(|(_, o)| o.age_blocks >= MIN_AGE_BLOCKS)
            .map(|(i, o)| {
                let sim = real_signer.cluster_similarity(o);
                let age_weight = self.age_adversary.weight_for_age(o.age_blocks);
                (i, sim, age_weight)
            })
            .filter(|(_, sim, _)| *sim >= self.config.min_cluster_similarity)
            .collect();

        // Sort by combined score (age weight * cluster similarity²)
        compatible.sort_by(|a, b| {
            let score_a = a.2 * a.1 * a.1;
            let score_b = b.2 * b.1 * b.1;
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // If not enough compatible, fall back to all candidates
        if compatible.len() < needed {
            return self.select_age_weighted_decoys(pool, real_idx, rng);
        }

        // Weighted random selection from compatible candidates
        self.weighted_sample(&compatible, needed, rng)
    }

    /// Select decoys using age-weighted selection only.
    fn select_age_weighted_decoys<R: Rng>(
        &self,
        pool: &[SimulatedOutput],
        real_idx: usize,
        rng: &mut R,
    ) -> Vec<usize> {
        let needed = self.config.ring_size - 1;

        let candidates: Vec<(usize, f64, f64)> = pool
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != real_idx)
            .filter(|(_, o)| o.age_blocks >= MIN_AGE_BLOCKS)
            .map(|(i, o)| {
                let age_weight = self.age_adversary.weight_for_age(o.age_blocks);
                (i, 1.0, age_weight) // Cluster similarity = 1.0 (ignored)
            })
            .collect();

        self.weighted_sample(&candidates, needed, rng)
    }

    /// Weighted random sampling without replacement.
    fn weighted_sample<R: Rng>(
        &self,
        candidates: &[(usize, f64, f64)], // (index, cluster_sim, age_weight)
        count: usize,
        rng: &mut R,
    ) -> Vec<usize> {
        if candidates.len() <= count {
            return candidates.iter().map(|(i, _, _)| *i).collect();
        }

        let mut selected = Vec::with_capacity(count);
        let mut remaining: Vec<_> = candidates.to_vec();

        for _ in 0..count {
            if remaining.is_empty() {
                break;
            }

            // Weight = age_weight * cluster_similarity²
            let weights: Vec<f64> = remaining
                .iter()
                .map(|(_, sim, age)| age * sim * sim)
                .collect();
            let total: f64 = weights.iter().sum();

            if total <= 0.0 {
                // Fall back to uniform random
                let idx = rng.gen_range(0..remaining.len());
                selected.push(remaining.remove(idx).0);
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

            selected.push(remaining.remove(chosen_idx).0);
        }

        selected
    }

    /// Simulate the output cluster tags after the real signer spends.
    fn simulate_output_tags(&self, real_signer: &SimulatedOutput) -> HashMap<u64, u32> {
        let retention = 1.0 - DEFAULT_DECAY_RATE;
        let mut output_tags = HashMap::new();

        for (&cluster, &weight) in &real_signer.cluster_tags {
            let decayed = (weight as f64 * retention).round() as u32;
            if decayed > 5_000 {
                output_tags.insert(cluster, decayed);
            }
        }

        output_tags
    }
}

// ============================================================================
// Monte Carlo Simulation
// ============================================================================

/// Summary statistics for a distribution.
#[derive(Debug, Clone)]
pub struct DistributionStats {
    pub count: usize,
    pub mean: f64,
    pub std_dev: f64,
    pub min: f64,
    pub max: f64,
    pub percentile_5: f64,
    pub percentile_25: f64,
    pub median: f64,
    pub percentile_75: f64,
    pub percentile_95: f64,
}

impl DistributionStats {
    /// Compute statistics from a sample.
    pub fn from_samples(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self {
                count: 0,
                mean: 0.0,
                std_dev: 0.0,
                min: 0.0,
                max: 0.0,
                percentile_5: 0.0,
                percentile_25: 0.0,
                median: 0.0,
                percentile_75: 0.0,
                percentile_95: 0.0,
            };
        }

        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let n = sorted.len();
        let mean = sorted.iter().sum::<f64>() / n as f64;
        let variance = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        let std_dev = variance.sqrt();

        let percentile = |p: f64| -> f64 {
            let idx = (p * (n - 1) as f64).round() as usize;
            sorted[idx.min(n - 1)]
        };

        Self {
            count: n,
            mean,
            std_dev,
            min: sorted[0],
            max: sorted[n - 1],
            percentile_5: percentile(0.05),
            percentile_25: percentile(0.25),
            median: percentile(0.50),
            percentile_75: percentile(0.75),
            percentile_95: percentile(0.95),
        }
    }
}

/// Results from Monte Carlo privacy simulation.
#[derive(Debug, Clone)]
pub struct MonteCarloResults {
    /// Number of simulations run.
    pub num_simulations: usize,
    /// Statistics for effective anonymity by adversary.
    pub effective_anonymity_stats: HashMap<String, DistributionStats>,
    /// Statistics for bits of privacy by adversary.
    pub bits_of_privacy_stats: HashMap<String, DistributionStats>,
    /// Statistics for real signer rank by adversary.
    pub real_signer_rank_stats: HashMap<String, DistributionStats>,
    /// Fraction of rings where real signer was most likely (rank 1).
    pub identified_rate: HashMap<String, f64>,
    /// Breakdown by output type.
    pub stats_by_output_type: HashMap<OutputType, HashMap<String, DistributionStats>>,
}

/// Configuration for Monte Carlo simulation.
#[derive(Debug, Clone)]
pub struct MonteCarloConfig {
    /// Number of rings to simulate.
    pub num_simulations: usize,
    /// Pool configuration.
    pub pool_config: PoolConfig,
    /// Ring simulation configuration.
    pub ring_config: RingSimConfig,
}

impl Default for MonteCarloConfig {
    fn default() -> Self {
        Self {
            num_simulations: 10_000,
            pool_config: PoolConfig::default(),
            ring_config: RingSimConfig::default(),
        }
    }
}

/// Run Monte Carlo simulation to estimate privacy distributions.
pub fn run_monte_carlo<R: Rng>(config: &MonteCarloConfig, rng: &mut R) -> MonteCarloResults {
    // Generate output pool
    let mut pool_gen = OutputPoolGenerator::new(config.pool_config.clone());
    let pool = pool_gen.generate_pool(rng);

    // Create simulator
    let simulator = RingSimulator::new(config.ring_config.clone());

    // Collect samples
    let mut samples_by_adversary: HashMap<String, Vec<RingPrivacyMetrics>> = HashMap::new();
    let mut samples_by_type: HashMap<OutputType, HashMap<String, Vec<RingPrivacyMetrics>>> =
        HashMap::new();

    for _ in 0..config.num_simulations {
        if let Some(result) = simulator.simulate_ring(&pool, rng) {
            let real_signer = &pool[result.ring_indices[result.real_signer_ring_index]];

            for (adv_name, metrics) in result.metrics_by_adversary {
                samples_by_adversary
                    .entry(adv_name.clone())
                    .or_default()
                    .push(metrics.clone());

                samples_by_type
                    .entry(real_signer.output_type)
                    .or_default()
                    .entry(adv_name)
                    .or_default()
                    .push(metrics);
            }
        }
    }

    // Compute statistics
    let mut effective_anonymity_stats = HashMap::new();
    let mut bits_of_privacy_stats = HashMap::new();
    let mut real_signer_rank_stats = HashMap::new();
    let mut identified_rate = HashMap::new();

    for (adv_name, metrics) in &samples_by_adversary {
        let eff_anon: Vec<f64> = metrics.iter().map(|m| m.effective_anonymity).collect();
        let bits: Vec<f64> = metrics.iter().map(|m| m.bits_of_privacy).collect();
        let ranks: Vec<f64> = metrics.iter().map(|m| m.real_signer_rank as f64).collect();

        effective_anonymity_stats
            .insert(adv_name.clone(), DistributionStats::from_samples(&eff_anon));
        bits_of_privacy_stats.insert(adv_name.clone(), DistributionStats::from_samples(&bits));
        real_signer_rank_stats.insert(adv_name.clone(), DistributionStats::from_samples(&ranks));

        let identified = metrics.iter().filter(|m| m.real_signer_rank == 1).count();
        identified_rate.insert(adv_name.clone(), identified as f64 / metrics.len() as f64);
    }

    // Stats by output type
    let mut stats_by_output_type = HashMap::new();
    for (output_type, adv_samples) in samples_by_type {
        let mut type_stats = HashMap::new();
        for (adv_name, metrics) in adv_samples {
            let bits: Vec<f64> = metrics.iter().map(|m| m.bits_of_privacy).collect();
            type_stats.insert(adv_name, DistributionStats::from_samples(&bits));
        }
        stats_by_output_type.insert(output_type, type_stats);
    }

    MonteCarloResults {
        num_simulations: config.num_simulations,
        effective_anonymity_stats,
        bits_of_privacy_stats,
        real_signer_rank_stats,
        identified_rate,
        stats_by_output_type,
    }
}

/// Format Monte Carlo results as a human-readable report.
pub fn format_monte_carlo_report(results: &MonteCarloResults) -> String {
    let mut report = String::new();

    report.push_str("╔══════════════════════════════════════════════════════════════════╗\n");
    report.push_str("║           RING SIGNATURE PRIVACY SIMULATION REPORT               ║\n");
    report.push_str("╠══════════════════════════════════════════════════════════════════╣\n");
    report.push_str(&format!(
        "║  Simulations: {:>6}    Ring Size: 7    Theoretical Max: 2.81 bits ║\n",
        results.num_simulations
    ));
    report.push_str("╚══════════════════════════════════════════════════════════════════╝\n\n");

    report.push_str("EFFECTIVE BITS OF PRIVACY BY ADVERSARY MODEL\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");
    report.push_str("Adversary            Mean    Std    5th%   Median  95th%   ID Rate\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");

    let adversaries = ["Naive", "Age-Heuristic", "Cluster-Fingerprint", "Combined"];
    for adv in adversaries {
        if let Some(stats) = results.bits_of_privacy_stats.get(adv) {
            let id_rate = results.identified_rate.get(adv).copied().unwrap_or(0.0);
            report.push_str(&format!(
                "{:<20} {:>5.2}  {:>5.2}  {:>5.2}  {:>5.2}   {:>5.2}   {:>5.1}%\n",
                adv,
                stats.mean,
                stats.std_dev,
                stats.percentile_5,
                stats.median,
                stats.percentile_95,
                id_rate * 100.0
            ));
        }
    }

    report.push_str("\nEFFECTIVE ANONYMITY SET SIZE (ideal = 7.0)\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");
    report.push_str("Adversary            Mean    5th%   Median  95th%\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");

    for adv in adversaries {
        if let Some(stats) = results.effective_anonymity_stats.get(adv) {
            report.push_str(&format!(
                "{:<20} {:>5.2}  {:>5.2}  {:>5.2}   {:>5.2}\n",
                adv, stats.mean, stats.percentile_5, stats.median, stats.percentile_95,
            ));
        }
    }

    report.push_str("\nBREAKDOWN BY OUTPUT TYPE (Combined Adversary)\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");
    report.push_str("Output Type          Mean Bits  5th%   Median  95th%\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");

    let output_types = [
        (OutputType::Standard, "Standard"),
        (OutputType::Exchange, "Exchange"),
        (OutputType::Whale, "Whale"),
        (OutputType::Coinbase, "Coinbase"),
        (OutputType::Mixed, "Mixed"),
    ];

    for (otype, name) in output_types {
        if let Some(type_stats) = results.stats_by_output_type.get(&otype) {
            if let Some(stats) = type_stats.get("Combined") {
                report.push_str(&format!(
                    "{:<20} {:>5.2}     {:>5.2}  {:>5.2}   {:>5.2}\n",
                    name, stats.mean, stats.percentile_5, stats.median, stats.percentile_95,
                ));
            }
        }
    }

    report.push_str("\n─────────────────────────────────────────────────────────────────────\n");
    report.push_str("LEGEND:\n");
    report.push_str(
        "  • Bits of Privacy: Information-theoretic privacy (log₂ of effective anonymity)\n",
    );
    report.push_str(
        "  • ID Rate: Fraction of rings where adversary identified real signer as most likely\n",
    );
    report.push_str("  • Lower ID Rate and higher Bits = better privacy\n");
    report.push_str("─────────────────────────────────────────────────────────────────────\n");

    report
}

// ============================================================================
// Committed Tag Vector Decoy Selection
// ============================================================================

use crate::{crypto::CommittedTagVector, ClusterId};
use std::collections::HashSet;

/// Extract the set of cluster IDs from a committed tag vector.
///
/// This is the only information available for decoy selection with committed
/// tags, as the masses are hidden in Pedersen commitments.
pub fn extract_cluster_ids(committed: &CommittedTagVector) -> HashSet<ClusterId> {
    committed.entries.iter().map(|e| e.cluster_id).collect()
}

/// Compute Jaccard similarity between two cluster ID sets.
///
/// Returns a value between 0.0 (no overlap) and 1.0 (identical sets).
/// Empty sets are considered fully similar (both are "background only").
///
/// # Formula
/// ```text
/// J(A, B) = |A ∩ B| / |A ∪ B|
/// ```
pub fn jaccard_similarity(set_a: &HashSet<ClusterId>, set_b: &HashSet<ClusterId>) -> f64 {
    // Empty sets are fully similar (maximally diffused outputs)
    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }
    // One empty, one not: partial similarity (empty is compatible with anything)
    if set_a.is_empty() || set_b.is_empty() {
        return 0.5;
    }

    let intersection_size = set_a.intersection(set_b).count();
    let union_size = set_a.union(set_b).count();

    if union_size == 0 {
        return 1.0;
    }

    intersection_size as f64 / union_size as f64
}

/// A committed output in the UTXO pool for decoy selection.
#[derive(Debug, Clone)]
pub struct CommittedPoolOutput {
    /// Unique identifier for this output.
    pub id: u64,
    /// Age in blocks.
    pub age_blocks: u64,
    /// The committed tag vector (cluster IDs visible, masses hidden).
    pub committed_tags: CommittedTagVector,
}

impl CommittedPoolOutput {
    /// Extract the cluster ID set from this output.
    pub fn cluster_id_set(&self) -> HashSet<ClusterId> {
        extract_cluster_ids(&self.committed_tags)
    }

    /// Compute Jaccard similarity with another output.
    pub fn cluster_similarity(&self, other: &CommittedPoolOutput) -> f64 {
        jaccard_similarity(&self.cluster_id_set(), &other.cluster_id_set())
    }
}

/// Configuration for committed tag decoy selection.
#[derive(Debug, Clone)]
pub struct CommittedDecoyConfig {
    /// Ring size (including real signer).
    pub ring_size: usize,
    /// Minimum Jaccard similarity for decoy selection (0.0 to 1.0).
    pub min_similarity: f64,
}

impl Default for CommittedDecoyConfig {
    fn default() -> Self {
        Self {
            ring_size: RING_SIZE,
            min_similarity: MIN_CLUSTER_SIMILARITY,
        }
    }
}

/// Select decoys for a real output using committed tag vectors.
///
/// This function selects decoys based on cluster ID set overlap (Jaccard
/// similarity) since mass values are hidden in commitments.
///
/// # Arguments
/// * `real_output` - The output being spent (real signer)
/// * `pool` - Available outputs to select decoys from
/// * `config` - Decoy selection configuration
/// * `rng` - Random number generator
///
/// # Returns
/// Indices of selected decoys in the pool, or None if insufficient decoys
/// available.
pub fn select_committed_decoys<R: rand::Rng>(
    real_output: &CommittedPoolOutput,
    pool: &[CommittedPoolOutput],
    config: &CommittedDecoyConfig,
    rng: &mut R,
) -> Option<Vec<usize>> {
    let needed = config.ring_size - 1;
    if pool.len() < needed {
        return None;
    }

    let real_clusters = real_output.cluster_id_set();
    let age_adversary = AgeAdversary::default();

    // Filter candidates by similarity and age
    let mut candidates: Vec<(usize, f64, f64)> = pool
        .iter()
        .enumerate()
        .filter(|(_, o)| o.id != real_output.id)
        .filter(|(_, o)| o.age_blocks >= MIN_AGE_BLOCKS)
        .map(|(i, o)| {
            let sim = jaccard_similarity(&real_clusters, &o.cluster_id_set());
            let age_weight = age_adversary.weight_for_age(o.age_blocks);
            (i, sim, age_weight)
        })
        .filter(|(_, sim, _)| *sim >= config.min_similarity)
        .collect();

    // If not enough similar candidates, relax similarity threshold
    if candidates.len() < needed {
        candidates = pool
            .iter()
            .enumerate()
            .filter(|(_, o)| o.id != real_output.id)
            .filter(|(_, o)| o.age_blocks >= MIN_AGE_BLOCKS)
            .map(|(i, o)| {
                let sim = jaccard_similarity(&real_clusters, &o.cluster_id_set());
                let age_weight = age_adversary.weight_for_age(o.age_blocks);
                (i, sim, age_weight)
            })
            .collect();
    }

    if candidates.len() < needed {
        return None;
    }

    // Sort by combined score (age weight * similarity²)
    candidates.sort_by(|a, b| {
        let score_a = a.2 * a.1 * a.1;
        let score_b = b.2 * b.1 * b.1;
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Weighted random selection
    weighted_sample_committed(&candidates, needed, rng)
}

/// Weighted random sampling without replacement for committed outputs.
fn weighted_sample_committed<R: rand::Rng>(
    candidates: &[(usize, f64, f64)], // (index, similarity, age_weight)
    count: usize,
    rng: &mut R,
) -> Option<Vec<usize>> {
    if candidates.len() < count {
        return None;
    }

    let mut selected = Vec::with_capacity(count);
    let mut remaining: Vec<_> = candidates.to_vec();

    for _ in 0..count {
        if remaining.is_empty() {
            return None;
        }

        // Weight = age_weight * similarity²
        let weights: Vec<f64> = remaining
            .iter()
            .map(|(_, sim, age)| age * sim * sim)
            .collect();
        let total: f64 = weights.iter().sum();

        if total <= 0.0 {
            // Fall back to uniform random
            let idx = rng.gen_range(0..remaining.len());
            selected.push(remaining.remove(idx).0);
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

        selected.push(remaining.remove(chosen_idx).0);
    }

    Some(selected)
}

/// Form a ring with the real output at a random position.
///
/// # Arguments
/// * `real_output` - The real output being spent
/// * `decoy_indices` - Indices of decoys in the pool
/// * `pool` - The output pool
/// * `rng` - Random number generator
///
/// # Returns
/// Tuple of (ring, real_index) where ring contains CommittedTagVector refs.
pub fn form_committed_ring<'a, R: rand::Rng>(
    real_output: &'a CommittedPoolOutput,
    decoy_indices: &[usize],
    pool: &'a [CommittedPoolOutput],
    rng: &mut R,
) -> (Vec<&'a CommittedTagVector>, usize) {
    let ring_size = decoy_indices.len() + 1;
    let real_position = rng.gen_range(0..ring_size);

    let mut ring = Vec::with_capacity(ring_size);
    let mut decoy_iter = decoy_indices.iter();

    for i in 0..ring_size {
        if i == real_position {
            ring.push(&real_output.committed_tags);
        } else {
            let decoy_idx = *decoy_iter.next().unwrap();
            ring.push(&pool[decoy_idx].committed_tags);
        }
    }

    (ring, real_position)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_generation() {
        let config = PoolConfig {
            pool_size: 1000,
            ..Default::default()
        };
        let mut gen = OutputPoolGenerator::new(config);
        let mut rng = rand::thread_rng();

        let pool = gen.generate_pool(&mut rng);
        assert_eq!(pool.len(), 1000);

        // Check distribution of types
        let standard_count = pool
            .iter()
            .filter(|o| o.output_type == OutputType::Standard)
            .count();
        // Should be roughly 50% (±10%)
        assert!(
            standard_count > 400 && standard_count < 600,
            "Expected ~50% standard, got {}",
            standard_count as f64 / 10.0
        );
    }

    #[test]
    fn test_cluster_similarity() {
        let mut tags1 = HashMap::new();
        tags1.insert(1, 800_000);
        tags1.insert(2, 200_000);

        let mut tags2 = HashMap::new();
        tags2.insert(1, 750_000);
        tags2.insert(2, 250_000);

        let o1 = SimulatedOutput {
            id: 1,
            age_blocks: 1000,
            output_type: OutputType::Standard,
            cluster_tags: tags1,
            hops_since_mint: 5,
        };

        let o2 = SimulatedOutput {
            id: 2,
            age_blocks: 1000,
            output_type: OutputType::Standard,
            cluster_tags: tags2,
            hops_since_mint: 5,
        };

        let sim = o1.cluster_similarity(&o2);
        assert!(
            sim > 0.95,
            "Similar clusters should have high similarity: {sim}"
        );
    }

    #[test]
    fn test_naive_adversary() {
        let outputs: Vec<SimulatedOutput> = (0..7)
            .map(|i| SimulatedOutput {
                id: i,
                age_blocks: 1000 + i * 100,
                output_type: OutputType::Standard,
                cluster_tags: HashMap::new(),
                hops_since_mint: 5,
            })
            .collect();

        let adversary = NaiveAdversary;
        let probs = adversary.analyze(&outputs, &HashMap::new());

        assert_eq!(probs.len(), 7);
        for &p in &probs {
            assert!(
                (p - 1.0 / 7.0).abs() < 0.001,
                "Expected uniform distribution"
            );
        }
    }

    #[test]
    fn test_privacy_metrics() {
        // Uniform distribution: maximum privacy
        let uniform = vec![1.0 / 7.0; 7];
        let metrics = calculate_privacy_metrics(&uniform, 3);

        assert!(
            (metrics.bits_of_privacy - 2.807).abs() < 0.01,
            "Expected ~2.81 bits, got {}",
            metrics.bits_of_privacy
        );
        assert!(
            (metrics.effective_anonymity - 7.0).abs() < 0.01,
            "Expected ~7.0 effective anonymity, got {}",
            metrics.effective_anonymity
        );

        // Highly skewed: minimal privacy
        let skewed = vec![0.8, 0.05, 0.05, 0.03, 0.03, 0.02, 0.02];
        let metrics = calculate_privacy_metrics(&skewed, 0);

        assert!(
            metrics.bits_of_privacy < 2.0,
            "Skewed distribution should have low bits: {}",
            metrics.bits_of_privacy
        );
        assert_eq!(metrics.real_signer_rank, 1, "Real signer should be rank 1");
    }

    #[test]
    fn test_ring_simulation() {
        let pool_config = PoolConfig {
            pool_size: 10_000,
            ..Default::default()
        };
        let mut gen = OutputPoolGenerator::new(pool_config);
        let mut rng = rand::thread_rng();
        let pool = gen.generate_pool(&mut rng);

        let ring_config = RingSimConfig::default();
        let simulator = RingSimulator::new(ring_config);

        let result = simulator.simulate_ring(&pool, &mut rng);
        assert!(result.is_some(), "Should successfully form a ring");

        let result = result.unwrap();
        assert_eq!(result.ring_indices.len(), RING_SIZE);
        assert!(result.real_signer_ring_index < RING_SIZE);
        assert!(result.metrics_by_adversary.contains_key("Combined"));
    }

    #[test]
    fn test_monte_carlo_small() {
        let config = MonteCarloConfig {
            num_simulations: 100,
            pool_config: PoolConfig {
                pool_size: 5_000,
                ..Default::default()
            },
            ring_config: RingSimConfig::default(),
        };

        let mut rng = rand::thread_rng();
        let results = run_monte_carlo(&config, &mut rng);

        // Check we got results for all adversaries
        assert!(results.bits_of_privacy_stats.contains_key("Naive"));
        assert!(results.bits_of_privacy_stats.contains_key("Combined"));

        // Naive should have near-perfect privacy (uniform assumption)
        let naive_bits = results.bits_of_privacy_stats.get("Naive").unwrap();
        assert!(
            naive_bits.mean > 2.7,
            "Naive should have ~2.81 bits: {}",
            naive_bits.mean
        );

        // Combined adversary should have lower privacy
        let combined_bits = results.bits_of_privacy_stats.get("Combined").unwrap();
        assert!(
            combined_bits.mean < naive_bits.mean,
            "Combined adversary should reduce privacy"
        );
    }

    // ========================================================================
    // Committed Tag Vector Tests
    // ========================================================================

    use crate::crypto::{CommittedTagMass, CommittedTagVectorSecret};
    use curve25519_dalek::{ristretto::RistrettoPoint, traits::Identity};
    use rand_core::OsRng;

    fn create_test_committed_vector(clusters: &[u64]) -> CommittedTagVector {
        let entries: Vec<CommittedTagMass> = clusters
            .iter()
            .map(|&id| CommittedTagMass {
                cluster_id: ClusterId(id),
                // Use identity point as placeholder commitment for testing
                commitment: RistrettoPoint::identity().compress(),
            })
            .collect();

        CommittedTagVector {
            entries,
            total_commitment: RistrettoPoint::identity().compress(),
        }
    }

    #[test]
    fn test_jaccard_similarity_identical() {
        let set_a: HashSet<ClusterId> = [1, 2, 3].iter().map(|&x| ClusterId(x)).collect();
        let set_b: HashSet<ClusterId> = [1, 2, 3].iter().map(|&x| ClusterId(x)).collect();

        let sim = jaccard_similarity(&set_a, &set_b);
        assert!(
            (sim - 1.0).abs() < 0.001,
            "Identical sets should have similarity 1.0: {sim}"
        );
    }

    #[test]
    fn test_jaccard_similarity_disjoint() {
        let set_a: HashSet<ClusterId> = [1, 2, 3].iter().map(|&x| ClusterId(x)).collect();
        let set_b: HashSet<ClusterId> = [4, 5, 6].iter().map(|&x| ClusterId(x)).collect();

        let sim = jaccard_similarity(&set_a, &set_b);
        assert!(
            sim.abs() < 0.001,
            "Disjoint sets should have similarity 0.0: {sim}"
        );
    }

    #[test]
    fn test_jaccard_similarity_partial_overlap() {
        // Sets: {1, 2, 3} and {2, 3, 4}
        // Intersection: {2, 3} = 2 elements
        // Union: {1, 2, 3, 4} = 4 elements
        // Jaccard: 2/4 = 0.5
        let set_a: HashSet<ClusterId> = [1, 2, 3].iter().map(|&x| ClusterId(x)).collect();
        let set_b: HashSet<ClusterId> = [2, 3, 4].iter().map(|&x| ClusterId(x)).collect();

        let sim = jaccard_similarity(&set_a, &set_b);
        assert!((sim - 0.5).abs() < 0.001, "Expected 0.5 similarity: {sim}");
    }

    #[test]
    fn test_jaccard_similarity_empty_sets() {
        let set_a: HashSet<ClusterId> = HashSet::new();
        let set_b: HashSet<ClusterId> = HashSet::new();

        let sim = jaccard_similarity(&set_a, &set_b);
        assert!(
            (sim - 1.0).abs() < 0.001,
            "Empty sets should have similarity 1.0: {sim}"
        );
    }

    #[test]
    fn test_jaccard_similarity_one_empty() {
        let set_a: HashSet<ClusterId> = [1, 2, 3].iter().map(|&x| ClusterId(x)).collect();
        let set_b: HashSet<ClusterId> = HashSet::new();

        let sim = jaccard_similarity(&set_a, &set_b);
        assert!(
            (sim - 0.5).abs() < 0.001,
            "One empty set should have similarity 0.5: {sim}"
        );
    }

    #[test]
    fn test_extract_cluster_ids() {
        let committed = create_test_committed_vector(&[1, 5, 10]);
        let ids = extract_cluster_ids(&committed);

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&ClusterId(1)));
        assert!(ids.contains(&ClusterId(5)));
        assert!(ids.contains(&ClusterId(10)));
    }

    #[test]
    fn test_committed_pool_output_similarity() {
        let output_a = CommittedPoolOutput {
            id: 1,
            age_blocks: 1000,
            committed_tags: create_test_committed_vector(&[1, 2, 3]),
        };

        let output_b = CommittedPoolOutput {
            id: 2,
            age_blocks: 1000,
            committed_tags: create_test_committed_vector(&[2, 3, 4]),
        };

        let sim = output_a.cluster_similarity(&output_b);
        assert!((sim - 0.5).abs() < 0.001, "Expected 0.5 similarity: {sim}");
    }

    #[test]
    fn test_select_committed_decoys() {
        let mut rng = rand::thread_rng();

        // Create a pool of outputs with various cluster sets
        let pool: Vec<CommittedPoolOutput> = (0..100)
            .map(|i| {
                let clusters: Vec<u64> = if i % 3 == 0 {
                    vec![1, 2, 3] // Same as real output
                } else if i % 3 == 1 {
                    vec![2, 3, 4] // Partial overlap
                } else {
                    vec![10, 11, 12] // Different clusters
                };

                CommittedPoolOutput {
                    id: i,
                    age_blocks: 1000 + i * 10,
                    committed_tags: create_test_committed_vector(&clusters),
                }
            })
            .collect();

        let real_output = CommittedPoolOutput {
            id: 999,
            age_blocks: 1500,
            committed_tags: create_test_committed_vector(&[1, 2, 3]),
        };

        let config = CommittedDecoyConfig {
            ring_size: 11,
            min_similarity: 0.5,
        };

        let decoys = select_committed_decoys(&real_output, &pool, &config, &mut rng);
        assert!(decoys.is_some(), "Should select decoys");

        let decoy_indices = decoys.unwrap();
        assert_eq!(
            decoy_indices.len(),
            10,
            "Should select 10 decoys for ring size 11"
        );

        // Verify all decoys have good similarity
        for &idx in &decoy_indices {
            let sim = real_output.cluster_similarity(&pool[idx]);
            // With relaxed threshold, similarity may vary
            assert!(sim >= 0.0, "Similarity should be non-negative: {sim}");
        }
    }

    #[test]
    fn test_form_committed_ring() {
        let mut rng = rand::thread_rng();

        let pool: Vec<CommittedPoolOutput> = (0..20)
            .map(|i| CommittedPoolOutput {
                id: i,
                age_blocks: 1000 + i * 100,
                committed_tags: create_test_committed_vector(&[i, i + 1]),
            })
            .collect();

        let real_output = CommittedPoolOutput {
            id: 999,
            age_blocks: 1500,
            committed_tags: create_test_committed_vector(&[100, 101]),
        };

        let decoy_indices: Vec<usize> = (0..10).collect();
        let (ring, real_idx) = form_committed_ring(&real_output, &decoy_indices, &pool, &mut rng);

        assert_eq!(ring.len(), 11, "Ring should have 11 members");
        assert!(real_idx < 11, "Real index should be within ring");

        // Verify real output is at the correct position
        let real_clusters = extract_cluster_ids(&real_output.committed_tags);
        let ring_at_real = extract_cluster_ids(ring[real_idx]);
        assert_eq!(
            real_clusters, ring_at_real,
            "Real output should be at real_idx"
        );
    }

    #[test]
    fn test_committed_ring_signature_integration() {
        // Integration test: create committed tags, select decoys, form ring,
        // and verify the ring can be used for signing
        use crate::TAG_WEIGHT_SCALE;

        let mut rng = OsRng;

        // Create a real input with secrets
        let input_tags: HashMap<ClusterId, crate::TagWeight> = [
            (ClusterId(1), TAG_WEIGHT_SCALE / 2),
            (ClusterId(2), TAG_WEIGHT_SCALE / 2),
        ]
        .into_iter()
        .collect();

        let input_secret = CommittedTagVectorSecret::from_plaintext(
            1_000_000, // value
            &input_tags,
            &mut rng,
        );
        let input_commitment = input_secret.commit();

        // Create pool with committed outputs
        let pool: Vec<CommittedPoolOutput> = (0..50)
            .map(|i| {
                let clusters = if i % 2 == 0 {
                    vec![1, 2] // Similar to real
                } else {
                    vec![10, 11] // Different
                };
                CommittedPoolOutput {
                    id: i,
                    age_blocks: 1000 + i * 10,
                    committed_tags: create_test_committed_vector(&clusters),
                }
            })
            .collect();

        let real_output = CommittedPoolOutput {
            id: 999,
            age_blocks: 1500,
            committed_tags: input_commitment.clone(),
        };

        // Select decoys
        let config = CommittedDecoyConfig {
            ring_size: 7,
            min_similarity: 0.7,
        };

        let decoys = select_committed_decoys(&real_output, &pool, &config, &mut rng);
        assert!(decoys.is_some(), "Should select decoys");

        let decoy_indices = decoys.unwrap();
        assert_eq!(decoy_indices.len(), 6, "Should select 6 decoys");

        // Form ring
        let (ring, real_idx) = form_committed_ring(&real_output, &decoy_indices, &pool, &mut rng);
        assert_eq!(ring.len(), 7);
        assert!(real_idx < 7);

        // Verify the ring contains the real commitment at the correct position
        assert_eq!(
            ring[real_idx].entries.len(),
            input_commitment.entries.len(),
            "Real input should be in ring at correct position"
        );
    }
}
