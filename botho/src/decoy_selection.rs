// Copyright (c) 2024 Botho Foundation

//! OSPEAD-Style Decoy Selection for Ring Signatures
//!
//! This module implements Optimal Selection Probability to Evade Analysis of
//! Decoys (OSPEAD), which matches decoy age distribution to actual spending
//! patterns using a gamma distribution.
//!
//! ## Key Concepts
//!
//! - **Spend Distribution**: Models how quickly outputs are typically spent
//!   after creation
//! - **Gamma Distribution**: Real spending follows a gamma distribution - most
//!   outputs are spent relatively quickly, with a long tail of outputs held for
//!   longer periods
//! - **Age-Weighted Selection**: Decoys are selected to match the expected
//!   spend distribution, making the real input indistinguishable from decoys
//! - **Cluster-Aware Selection**: Decoys are selected with similar cluster tag
//!   profiles to prevent fingerprinting attacks based on output tag inheritance
//!
//! ## Cluster Tag Privacy
//!
//! Botho's progressive fee system uses cluster tags to track coin ancestry.
//! These tags are visible on transaction outputs, which could enable
//! fingerprinting attacks:
//!
//! 1. Observer examines the ring of 20 possible inputs
//! 2. Compares each input's cluster tags to the output's tags (after decay)
//! 3. Identifies which input's tags best match the output pattern
//!
//! To mitigate this, OSPEAD prioritizes decoys with **similar cluster
//! profiles**, ensuring multiple ring members would produce plausible output
//! patterns.
//!
//! Target: Achieve 10+ effective anonymity with ring size 20, meaning at least
//! 10 ring members should be equally plausible based on both age and cluster
//! patterns.
//!
//! Note: Ring size 20 is larger than Monero's 16, providing stronger sender
//! privacy.

use rand::Rng;
use rand_distr::{Distribution, Gamma};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::transaction::{TxOutput, Utxo};

// ============================================================================
// Cluster Tag Types (simplified for decoy selection)
// ============================================================================

/// Cluster identifier for tracking coin ancestry.
pub type ClusterId = u64;

/// Weight of a cluster tag (parts per million, 1_000_000 = 100%).
pub type TagWeight = u32;

/// Scale factor for tag weights.
pub const TAG_WEIGHT_SCALE: TagWeight = 1_000_000;

/// Minimum similarity score to consider a decoy as cluster-compatible.
/// 0.7 means the cosine similarity must be at least 70%.
pub const MIN_CLUSTER_SIMILARITY: f64 = 0.7;

/// Maximum weight difference for dominant clusters (20% = 200,000 in scale).
pub const MAX_DOMINANT_WEIGHT_DIFF: TagWeight = 200_000;

/// Sparse cluster tag vector for an output.
///
/// Maps cluster IDs to weights indicating what fraction of the value
/// traces back to each cluster origin.
#[derive(Debug, Clone, Default)]
pub struct ClusterTags {
    /// Sparse map of cluster -> weight (parts per million).
    tags: HashMap<ClusterId, TagWeight>,
}

impl ClusterTags {
    /// Create an empty tag vector (fully diffused/background).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create a tag vector fully attributed to one cluster.
    pub fn single(cluster_id: ClusterId) -> Self {
        let mut tags = HashMap::new();
        tags.insert(cluster_id, TAG_WEIGHT_SCALE);
        Self { tags }
    }

    /// Create from a list of (cluster_id, weight) pairs.
    pub fn from_pairs(pairs: &[(ClusterId, TagWeight)]) -> Self {
        Self {
            tags: pairs.iter().cloned().collect(),
        }
    }

    /// Get the weight for a specific cluster.
    pub fn get(&self, cluster_id: ClusterId) -> TagWeight {
        self.tags.get(&cluster_id).copied().unwrap_or(0)
    }

    /// Total attributed weight.
    pub fn total_weight(&self) -> TagWeight {
        self.tags.values().sum::<TagWeight>().min(TAG_WEIGHT_SCALE)
    }

    /// Number of tracked clusters.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Returns true if fully diffused (no cluster attribution).
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Iterate over all (cluster, weight) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, TagWeight)> + '_ {
        self.tags.iter().map(|(&k, &v)| (k, v))
    }

    /// Get the top-N clusters by weight.
    pub fn top_clusters(&self, n: usize) -> Vec<(ClusterId, TagWeight)> {
        let mut entries: Vec<_> = self.tags.iter().map(|(&k, &v)| (k, v)).collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(n);
        entries
    }

    /// Compute cosine similarity with another tag vector.
    ///
    /// Returns a value between 0.0 (completely different) and 1.0 (identical).
    /// Empty vectors are considered similar to any vector (returns 1.0).
    pub fn cosine_similarity(&self, other: &ClusterTags) -> f64 {
        // If both are empty, they're identical
        if self.is_empty() && other.is_empty() {
            return 1.0;
        }

        // If one is empty (fully diffused), it's maximally similar to anything
        // This handles the case of heavily circulated coins
        if self.is_empty() || other.is_empty() {
            return 1.0;
        }

        // Collect all cluster IDs
        let all_clusters: HashSet<ClusterId> =
            self.tags.keys().chain(other.tags.keys()).copied().collect();

        // Compute dot product and magnitudes
        let mut dot_product: f64 = 0.0;
        let mut mag_self: f64 = 0.0;
        let mut mag_other: f64 = 0.0;

        for cluster in all_clusters {
            let w1 = self.get(cluster) as f64;
            let w2 = other.get(cluster) as f64;

            dot_product += w1 * w2;
            mag_self += w1 * w1;
            mag_other += w2 * w2;
        }

        let magnitude = (mag_self.sqrt() * mag_other.sqrt()).max(1.0);
        (dot_product / magnitude).clamp(0.0, 1.0)
    }

    /// Check if the dominant clusters (top-3) overlap with another vector.
    ///
    /// Returns true if at least 2 of the top-3 clusters match and their
    /// weights are within MAX_DOMINANT_WEIGHT_DIFF of each other.
    pub fn dominant_clusters_match(&self, other: &ClusterTags) -> bool {
        let self_top = self.top_clusters(3);
        let other_top = other.top_clusters(3);

        // If either has few clusters, be lenient
        if self_top.len() < 2 || other_top.len() < 2 {
            return true;
        }

        let other_ids: HashSet<ClusterId> = other_top.iter().map(|(id, _)| *id).collect();

        // Count matching clusters with similar weights
        let mut matches = 0;
        for (cluster, self_weight) in &self_top {
            if other_ids.contains(cluster) {
                let other_weight = other.get(*cluster);
                let diff = (*self_weight as i64 - other_weight as i64).unsigned_abs() as TagWeight;
                if diff <= MAX_DOMINANT_WEIGHT_DIFF {
                    matches += 1;
                }
            }
        }

        matches >= 2
    }

    /// Compute a combined similarity score considering both cosine similarity
    /// and dominant cluster matching.
    ///
    /// Returns a score between 0.0 and 1.0.
    pub fn combined_similarity(&self, other: &ClusterTags) -> f64 {
        let cosine = self.cosine_similarity(other);
        let dominant_match = if self.dominant_clusters_match(other) {
            1.0
        } else {
            0.5
        };

        // Weighted combination: 60% cosine, 40% dominant match
        0.6 * cosine + 0.4 * dominant_match
    }
}

/// Blocks per day (assuming 2-minute block time)
const BLOCKS_PER_DAY: f64 = 720.0;

/// Minimum age in blocks for decoy consideration (must be confirmed)
const MIN_DECOY_AGE_BLOCKS: u64 = 10;

/// Maximum age to consider for gamma sampling (prevents extreme outliers)
/// ~2 years in blocks
const MAX_DECOY_AGE_BLOCKS: u64 = 525_600;

/// Number of recent spends to track for distribution estimation
const SPEND_HISTORY_SIZE: usize = 10_000;

/// Default gamma distribution parameters (from Monero research).
/// Shape (k) = 19.28, Scale (θ) = 1.61 days
/// Mean = k * θ ≈ 31 days, which matches observed spending patterns
const DEFAULT_GAMMA_SHAPE: f64 = 19.28;
const DEFAULT_GAMMA_SCALE_DAYS: f64 = 1.61;

/// Age-similarity policy: maximum relative spread, in basis points, of a
/// selected decoy's age around the real input's age (1000 bps = ±10%).
///
/// # Why this exists (issue #596, consensus-economic dependency of #577-B1)
///
/// The #577 empirical gate (PR #595) selected the `ring_elapsed_quantile@max`
/// order-statistic as the demurrage clock: the charge is computed from the
/// **oldest** ring member's age, not the value-weighted centroid. This fully
/// neutralizes the H2 fresh-high-value-decoy age-dilution attack (an adversary
/// can no longer drag the elapsed time down with fresh decoys), but it makes
/// the wallet's decoy age spread consensus-economically load-bearing: an honest
/// spender whose ring contains a decoy much OLDER than their real input is
/// over-charged demurrage relative to their true holding time.
///
/// PR #595's sensitivity sweep measured the honest over-charge under `@max` as
/// a function of the ring's decoy age-spread, against the #314 tolerance
/// (≤15%):
///
/// | honest decoy age-spread | honest over-charge | within #314 tol (≤15%) |
/// |-------------------------|--------------------|------------------------|
/// | ±5%                     | 1.039              | yes                    |
/// | ±10%                    | 1.077              | yes                    |
/// | ±20%                    | 1.158              | no                     |
/// | ±35%                    | 1.277              | no                     |
/// | ±50%                    | 1.388              | no                     |
///
/// ±10% is the widest empirically-safe band (1.077 ≤ 1.15). We adopt ±10% as
/// the policy: it maximizes the eligible decoy pool (and hence anonymity) among
/// the #314-safe bands. Because `@max` reads the single oldest member, the
/// realized over-charge is bounded above by `max_ring_age / real_age`, which
/// this band caps at ≤1.10 by construction — strictly inside the #314 bound.
///
/// The gamma/OSPEAD age distribution is still applied WITHIN the band, so the
/// realistic recent-biased spend shape is preserved among age-similar decoys.
pub const AGE_SIMILARITY_SPREAD_BPS: u64 = 1_000;

/// Compute the inclusive `[min_age, max_age]` decoy-age band around a real
/// input's age under the [`AGE_SIMILARITY_SPREAD_BPS`] policy.
///
/// The lower bound is additionally floored at [`MIN_DECOY_AGE_BLOCKS`] so
/// decoys are always confirmed. A degenerate real age (below the confirmation
/// floor) can produce `min_age > max_age`, in which case no candidate is
/// in-band and callers fall back to nearest-age selection.
pub fn age_similarity_band(real_input_age: u64) -> (u64, u64) {
    let delta = (real_input_age as u128 * AGE_SIMILARITY_SPREAD_BPS as u128 / 10_000) as u64;
    let min_age = real_input_age
        .saturating_sub(delta)
        .max(MIN_DECOY_AGE_BLOCKS);
    let max_age = real_input_age.saturating_add(delta);
    (min_age, max_age)
}

/// An output candidate for decoy selection with age and cluster information.
#[derive(Debug, Clone)]
pub struct OutputCandidate {
    /// The output itself
    pub output: TxOutput,
    /// Block height when the output was created
    pub created_at: u64,
    /// Age in blocks (current_height - created_at)
    pub age_blocks: u64,
    /// Cluster tags for this output (for cluster-aware selection)
    pub cluster_tags: ClusterTags,
}

impl OutputCandidate {
    /// Create a new output candidate from a UTXO at the given height.
    ///
    /// Note: This creates an empty cluster tag vector. Use `with_cluster_tags`
    /// to include cluster information for cluster-aware decoy selection.
    pub fn from_utxo(utxo: &Utxo, current_height: u64) -> Self {
        let age_blocks = current_height.saturating_sub(utxo.created_at);
        Self {
            output: utxo.output.clone(),
            created_at: utxo.created_at,
            age_blocks,
            cluster_tags: ClusterTags::empty(),
        }
    }

    /// Create a new output candidate with cluster tags.
    pub fn from_utxo_with_tags(
        utxo: &Utxo,
        current_height: u64,
        cluster_tags: ClusterTags,
    ) -> Self {
        let age_blocks = current_height.saturating_sub(utxo.created_at);
        Self {
            output: utxo.output.clone(),
            created_at: utxo.created_at,
            age_blocks,
            cluster_tags,
        }
    }

    /// Add cluster tags to an existing candidate.
    pub fn with_cluster_tags(mut self, cluster_tags: ClusterTags) -> Self {
        self.cluster_tags = cluster_tags;
        self
    }

    /// Age in days (approximate, assuming 2-minute blocks).
    pub fn age_days(&self) -> f64 {
        self.age_blocks as f64 / BLOCKS_PER_DAY
    }

    /// Compute cluster similarity with another candidate.
    pub fn cluster_similarity(&self, other: &OutputCandidate) -> f64 {
        self.cluster_tags.combined_similarity(&other.cluster_tags)
    }
}

/// Tracks observed spend ages to estimate the actual spend distribution.
///
/// This enables real-time parameter updates for the gamma distribution,
/// adapting to actual network spending behavior.
#[derive(Debug, Clone)]
pub struct SpendDistribution {
    /// Recent spend ages in blocks (ring buffer)
    spend_ages: VecDeque<u64>,
    /// Estimated gamma shape parameter (k)
    gamma_shape: f64,
    /// Estimated gamma scale parameter (θ) in blocks
    gamma_scale_blocks: f64,
    /// Whether parameters have been updated from observations
    has_observations: bool,
}

impl Default for SpendDistribution {
    fn default() -> Self {
        Self::new()
    }
}

impl SpendDistribution {
    /// Create a new spend distribution with default parameters.
    pub fn new() -> Self {
        Self {
            spend_ages: VecDeque::with_capacity(SPEND_HISTORY_SIZE),
            gamma_shape: DEFAULT_GAMMA_SHAPE,
            gamma_scale_blocks: DEFAULT_GAMMA_SCALE_DAYS * BLOCKS_PER_DAY,
            has_observations: false,
        }
    }

    /// Record an observed spend (output age in blocks when spent).
    ///
    /// Call this when processing transactions to learn actual spend patterns.
    pub fn record_spend(&mut self, age_blocks: u64) {
        // Don't record very young outputs (likely coinbase or special cases)
        if age_blocks < MIN_DECOY_AGE_BLOCKS {
            return;
        }

        // Maintain fixed-size history
        if self.spend_ages.len() >= SPEND_HISTORY_SIZE {
            self.spend_ages.pop_front();
        }
        self.spend_ages.push_back(age_blocks);

        // Update parameters periodically (every 100 observations)
        if self.spend_ages.len() >= 100 && self.spend_ages.len().is_multiple_of(100) {
            self.update_parameters();
        }
    }

    /// Update gamma distribution parameters from observations using method of
    /// moments.
    fn update_parameters(&mut self) {
        if self.spend_ages.len() < 100 {
            return;
        }

        // Calculate mean and variance
        let n = self.spend_ages.len() as f64;
        let mean: f64 = self.spend_ages.iter().map(|&x| x as f64).sum::<f64>() / n;
        let variance: f64 = self
            .spend_ages
            .iter()
            .map(|&x| {
                let diff = x as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / n;

        // Method of moments for gamma distribution:
        // mean = k * θ, variance = k * θ²
        // Therefore: θ = variance / mean, k = mean / θ = mean² / variance
        if variance > 0.0 && mean > 0.0 {
            let theta = variance / mean;
            let k = mean / theta;

            // Sanity bounds to prevent extreme values
            if k >= 1.0 && k <= 100.0 && theta >= 1.0 && theta <= 10000.0 {
                self.gamma_shape = k;
                self.gamma_scale_blocks = theta;
                self.has_observations = true;
            }
        }
    }

    /// Get the current gamma shape parameter.
    pub fn shape(&self) -> f64 {
        self.gamma_shape
    }

    /// Get the current gamma scale parameter in blocks.
    pub fn scale_blocks(&self) -> f64 {
        self.gamma_scale_blocks
    }

    /// Check if parameters have been learned from observations.
    pub fn has_observations(&self) -> bool {
        self.has_observations
    }

    /// Number of recorded observations.
    pub fn observation_count(&self) -> usize {
        self.spend_ages.len()
    }

    /// Calculate the probability density at a given age (for
    /// debugging/analysis).
    pub fn pdf(&self, age_blocks: u64) -> f64 {
        // Validate distribution parameters
        if Gamma::new(self.gamma_shape, self.gamma_scale_blocks).is_err() {
            return 0.0;
        }

        // Gamma PDF: f(x) = x^(k-1) * e^(-x/θ) / (θ^k * Γ(k))
        let x = age_blocks as f64;
        if x > 0.0 {
            // Use the normalized density
            let k = self.gamma_shape;
            let theta = self.gamma_scale_blocks;
            let log_pdf = (k - 1.0) * x.ln() - x / theta - k * theta.ln() - ln_gamma(k);
            log_pdf.exp()
        } else {
            0.0
        }
    }
}

/// Approximation of log-gamma function using Stirling's formula.
fn ln_gamma(x: f64) -> f64 {
    // Using Lanczos approximation would be more accurate, but Stirling is
    // sufficient for our purposes and simpler
    if x <= 0.0 {
        return f64::INFINITY;
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x - 0.5) * x.ln() - x + 1.0 / (12.0 * x)
        - 1.0 / (360.0 * x * x * x)
}

/// OSPEAD-style gamma-weighted decoy selector.
///
/// Selects decoys to match the expected spend age distribution, making it
/// harder for observers to distinguish real spends from decoys based on age.
#[derive(Debug, Clone)]
pub struct GammaDecoySelector {
    /// The spend distribution model
    distribution: SpendDistribution,
}

impl Default for GammaDecoySelector {
    fn default() -> Self {
        Self::new()
    }
}

impl GammaDecoySelector {
    /// Create a new selector with default parameters.
    pub fn new() -> Self {
        Self {
            distribution: SpendDistribution::new(),
        }
    }

    /// Create a selector with a custom spend distribution.
    pub fn with_distribution(distribution: SpendDistribution) -> Self {
        Self { distribution }
    }

    /// Get a mutable reference to the underlying distribution for updates.
    pub fn distribution_mut(&mut self) -> &mut SpendDistribution {
        &mut self.distribution
    }

    /// Get the underlying distribution.
    pub fn distribution(&self) -> &SpendDistribution {
        &self.distribution
    }

    /// Select decoys using gamma-weighted age distribution.
    ///
    /// # Arguments
    /// * `candidates` - Available UTXO candidates with age info
    /// * `count` - Number of decoys to select
    /// * `exclude_keys` - Target keys to exclude (the real inputs)
    /// * `current_height` - Current blockchain height
    ///
    /// # Returns
    /// Selected decoys, or error if insufficient candidates
    pub fn select_decoys<R: Rng>(
        &self,
        candidates: &[OutputCandidate],
        count: usize,
        exclude_keys: &[[u8; 32]],
        _current_height: u64,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        // Filter out excluded keys and too-young outputs
        let eligible: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS && !exclude_keys.contains(&c.output.target_key)
            })
            .collect();

        if eligible.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: eligible.len(),
            });
        }

        // Validate gamma distribution parameters
        let _ = Gamma::new(
            self.distribution.gamma_shape,
            self.distribution.gamma_scale_blocks,
        )
        .map_err(|_| DecoySelectionError::InvalidDistribution)?;

        // Calculate weights for each candidate based on gamma PDF
        let weights: Vec<f64> = eligible
            .iter()
            .map(|c| self.weight_for_age(c.age_blocks))
            .collect();

        let total_weight: f64 = weights.iter().sum();
        if total_weight <= 0.0 {
            return Err(DecoySelectionError::InvalidDistribution);
        }

        // Weighted random sampling without replacement
        let mut selected = Vec::with_capacity(count);
        let mut remaining_weights = weights.clone();
        let mut remaining_indices: Vec<usize> = (0..eligible.len()).collect();

        for _ in 0..count {
            let current_total: f64 = remaining_weights.iter().sum();
            if current_total <= 0.0 {
                break;
            }

            // Sample from remaining candidates weighted by gamma PDF
            let sample = rng.gen::<f64>() * current_total;
            let mut cumulative = 0.0;
            let mut selected_idx = 0;

            for (i, &weight) in remaining_weights.iter().enumerate() {
                cumulative += weight;
                if cumulative >= sample {
                    selected_idx = i;
                    break;
                }
            }

            // Add selected output
            let original_idx = remaining_indices[selected_idx];
            selected.push(eligible[original_idx].output.clone());

            // Remove from remaining candidates
            remaining_indices.remove(selected_idx);
            remaining_weights.remove(selected_idx);
        }

        // If we didn't get enough from weighted sampling (edge case), fill uniformly
        while selected.len() < count && !remaining_indices.is_empty() {
            let idx = rng.gen_range(0..remaining_indices.len());
            let original_idx = remaining_indices[idx];
            selected.push(eligible[original_idx].output.clone());
            remaining_indices.remove(idx);
        }

        if selected.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: selected.len(),
            });
        }

        Ok(selected)
    }

    /// Calculate the selection weight for a given age using gamma PDF.
    ///
    /// Higher weight = more likely to be selected as decoy.
    fn weight_for_age(&self, age_blocks: u64) -> f64 {
        // Clamp age to reasonable bounds
        let age = (age_blocks as f64).clamp(1.0, MAX_DECOY_AGE_BLOCKS as f64);

        // Gamma PDF (unnormalized is fine for weights)
        let k = self.distribution.gamma_shape;
        let theta = self.distribution.gamma_scale_blocks;

        // f(x) ∝ x^(k-1) * e^(-x/θ)
        // Use log-space to avoid overflow
        let log_weight = (k - 1.0) * age.ln() - age / theta;

        // Convert back, with a floor to prevent zero weights
        log_weight.exp().max(1e-10)
    }

    /// Select decoys age-similar to the real input, sampled within the
    /// [`AGE_SIMILARITY_SPREAD_BPS`] band using the gamma spend distribution.
    ///
    /// # Consensus-economic role (issue #596)
    ///
    /// Under the `ring_elapsed_quantile@max` demurrage kernel (#577-B1 / PR
    /// #595) the charge is driven by the OLDEST ring member's age. A decoy much
    /// older than the real input therefore over-charges an honest spender. This
    /// method bounds every selected decoy's age to `age_similarity_band(real)`
    /// (±10% by policy), keeping the realized over-charge under `@max` at
    /// ≤1.10 — inside the #314 ≤15% tolerance. The gamma/OSPEAD age weighting
    /// is still applied WITHIN the band, so the realistic recent-biased
    /// spend shape is preserved among the age-similar candidates.
    ///
    /// # Liveness fallback
    ///
    /// If the in-band pool cannot supply `count` decoys (sparse chain / unusual
    /// real age), the remaining slots are filled with the NEAREST-aged eligible
    /// candidates. This preserves transaction liveness while still minimizing
    /// the realized max ring age — and hence the honest over-charge — to the
    /// tightest value the available pool allows, rather than reverting to the
    /// unbounded global-gamma sample that motivated this issue.
    pub fn select_decoys_for_input<R: Rng>(
        &self,
        candidates: &[OutputCandidate],
        count: usize,
        exclude_keys: &[[u8; 32]],
        real_input_age: u64,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        // Filter eligible candidates (confirmed, not excluded).
        let eligible: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS && !exclude_keys.contains(&c.output.target_key)
            })
            .collect();

        if eligible.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: eligible.len(),
            });
        }

        let gamma = Gamma::new(
            self.distribution.gamma_shape,
            self.distribution.gamma_scale_blocks,
        )
        .map_err(|_| DecoySelectionError::InvalidDistribution)?;

        // Age-similarity band around the real input's age (#596).
        let (min_age, max_age) = age_similarity_band(real_input_age);

        let mut selected = Vec::with_capacity(count);
        let mut used_keys = exclude_keys.to_vec();

        // Phase 1: draw from WITHIN the band. Sample a target age from the gamma
        // spend distribution, clamp it into the band, and pick the nearest-aged
        // in-band candidate. This keeps the realistic within-band distribution.
        for _ in 0..count {
            let target_age: f64 = gamma.sample(rng);
            let target_age_blocks = (target_age as u64).clamp(min_age, max_age);

            let best = eligible
                .iter()
                .filter(|c| {
                    c.age_blocks >= min_age
                        && c.age_blocks <= max_age
                        && !used_keys.contains(&c.output.target_key)
                })
                .min_by_key(|c| (c.age_blocks as i64 - target_age_blocks as i64).unsigned_abs());

            if let Some(candidate) = best {
                selected.push(candidate.output.clone());
                used_keys.push(candidate.output.target_key);
            } else {
                // Band exhausted; remaining slots handled by the fallback below.
                break;
            }
        }

        // Phase 2 (liveness fallback): fill any remaining slots with the
        // nearest-aged eligible candidates so over-charge stays as small as the
        // pool allows even when the band cannot be fully satisfied.
        if selected.len() < count {
            let mut leftovers: Vec<&&OutputCandidate> = eligible
                .iter()
                .filter(|c| !used_keys.contains(&c.output.target_key))
                .collect();
            leftovers.sort_by_key(|c| (c.age_blocks as i64 - real_input_age as i64).unsigned_abs());
            for candidate in leftovers {
                if selected.len() >= count {
                    break;
                }
                selected.push(candidate.output.clone());
                used_keys.push(candidate.output.target_key);
            }
        }

        if selected.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: selected.len(),
            });
        }

        Ok(selected)
    }

    /// Calculate effective anonymity set size.
    ///
    /// This estimates how many ring members appear equally likely to be the
    /// real spend based on age distribution. A higher number is better.
    ///
    /// Returns a value between 1 (no privacy) and ring_size (perfect privacy).
    pub fn effective_anonymity(&self, ring_ages: &[u64]) -> f64 {
        if ring_ages.is_empty() {
            return 0.0;
        }

        // Calculate probability for each member based on gamma distribution
        let probs: Vec<f64> = ring_ages
            .iter()
            .map(|&age| self.weight_for_age(age))
            .collect();

        let total: f64 = probs.iter().sum();
        if total <= 0.0 {
            return 0.0;
        }

        // Normalize to probabilities
        let normalized: Vec<f64> = probs.iter().map(|p| p / total).collect();

        // Calculate entropy: H = -Σ p_i * log(p_i)
        let entropy: f64 = normalized
            .iter()
            .filter(|&&p| p > 0.0)
            .map(|&p| -p * p.ln())
            .sum();

        // Effective anonymity = e^H (effective number of choices)
        entropy.exp()
    }

    /// Select decoys using cluster-aware OSPEAD algorithm.
    ///
    /// This is the recommended method for private transactions. It combines:
    /// 1. Age-weighted selection (gamma distribution)
    /// 2. Cluster similarity filtering (prevents tag fingerprinting)
    ///
    /// # Arguments
    /// * `candidates` - Available UTXO candidates with age and cluster info
    /// * `count` - Number of decoys to select
    /// * `real_input` - The real input being spent (for cluster matching)
    /// * `exclude_keys` - Target keys to exclude
    ///
    /// # Returns
    /// Selected decoys with similar cluster profiles, or error if insufficient
    /// candidates.
    ///
    /// # Privacy Guarantee
    /// When cluster-aware selection succeeds, at least `count` ring members
    /// will have cluster profiles similar enough that an observer cannot
    /// distinguish them based on output tag inheritance patterns.
    pub fn select_decoys_cluster_aware<R: Rng>(
        &self,
        candidates: &[OutputCandidate],
        count: usize,
        real_input: &OutputCandidate,
        exclude_keys: &[[u8; 32]],
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        // Filter candidates by:
        // 1. Minimum age
        // 2. Not excluded
        // 3. Cluster similarity above threshold
        let cluster_compatible: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                    && !exclude_keys.contains(&c.output.target_key)
                    && real_input.cluster_similarity(c) >= MIN_CLUSTER_SIMILARITY
            })
            .collect();

        // If we have enough cluster-compatible candidates, use those
        if cluster_compatible.len() >= count {
            return self.select_from_pool(&cluster_compatible, count, exclude_keys, rng);
        }

        // Fallback: relax cluster requirements but prefer similar ones
        // This happens early in network life or with unusual cluster profiles
        let eligible: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS && !exclude_keys.contains(&c.output.target_key)
            })
            .collect();

        if eligible.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: eligible.len(),
            });
        }

        // Score candidates by combined age + cluster similarity
        self.select_with_cluster_scoring(&eligible, count, real_input, exclude_keys, rng)
    }

    /// Select from a pre-filtered pool of candidates using age weighting.
    fn select_from_pool<R: Rng>(
        &self,
        pool: &[&OutputCandidate],
        count: usize,
        exclude_keys: &[[u8; 32]],
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        let _ = Gamma::new(
            self.distribution.gamma_shape,
            self.distribution.gamma_scale_blocks,
        )
        .map_err(|_| DecoySelectionError::InvalidDistribution)?;

        // Calculate age-based weights
        let weights: Vec<f64> = pool
            .iter()
            .map(|c| self.weight_for_age(c.age_blocks))
            .collect();

        let total_weight: f64 = weights.iter().sum();
        if total_weight <= 0.0 {
            return Err(DecoySelectionError::InvalidDistribution);
        }

        // Weighted sampling without replacement
        let mut selected = Vec::with_capacity(count);
        let mut remaining_weights = weights;
        let mut remaining_indices: Vec<usize> = (0..pool.len()).collect();
        let mut used_keys = exclude_keys.to_vec();

        while selected.len() < count && !remaining_indices.is_empty() {
            let current_total: f64 = remaining_weights.iter().sum();
            if current_total <= 0.0 {
                break;
            }

            let sample = rng.gen::<f64>() * current_total;
            let mut cumulative = 0.0;
            let mut selected_idx = 0;

            for (i, &weight) in remaining_weights.iter().enumerate() {
                cumulative += weight;
                if cumulative >= sample {
                    selected_idx = i;
                    break;
                }
            }

            let original_idx = remaining_indices[selected_idx];
            let candidate = pool[original_idx];

            if !used_keys.contains(&candidate.output.target_key) {
                selected.push(candidate.output.clone());
                used_keys.push(candidate.output.target_key);
            }

            remaining_indices.remove(selected_idx);
            remaining_weights.remove(selected_idx);
        }

        if selected.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: selected.len(),
            });
        }

        Ok(selected)
    }

    /// Select candidates with combined age and cluster scoring.
    ///
    /// Used as fallback when strict cluster filtering yields insufficient
    /// candidates.
    fn select_with_cluster_scoring<R: Rng>(
        &self,
        candidates: &[&OutputCandidate],
        count: usize,
        real_input: &OutputCandidate,
        exclude_keys: &[[u8; 32]],
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        // Combined score = age_weight * cluster_similarity
        // This prefers candidates that are both age-plausible and cluster-similar
        let scores: Vec<f64> = candidates
            .iter()
            .map(|c| {
                let age_weight = self.weight_for_age(c.age_blocks);
                let cluster_sim = real_input.cluster_similarity(c);
                // Boost cluster similarity importance
                age_weight * (cluster_sim * cluster_sim)
            })
            .collect();

        let total_score: f64 = scores.iter().sum();
        if total_score <= 0.0 {
            return Err(DecoySelectionError::InvalidDistribution);
        }

        // Weighted sampling
        let mut selected = Vec::with_capacity(count);
        let mut remaining_scores = scores;
        let mut remaining_indices: Vec<usize> = (0..candidates.len()).collect();
        let mut used_keys = exclude_keys.to_vec();

        while selected.len() < count && !remaining_indices.is_empty() {
            let current_total: f64 = remaining_scores.iter().sum();
            if current_total <= 0.0 {
                break;
            }

            let sample = rng.gen::<f64>() * current_total;
            let mut cumulative = 0.0;
            let mut selected_idx = 0;

            for (i, &score) in remaining_scores.iter().enumerate() {
                cumulative += score;
                if cumulative >= sample {
                    selected_idx = i;
                    break;
                }
            }

            let original_idx = remaining_indices[selected_idx];
            let candidate = candidates[original_idx];

            if !used_keys.contains(&candidate.output.target_key) {
                selected.push(candidate.output.clone());
                used_keys.push(candidate.output.target_key);
            }

            remaining_indices.remove(selected_idx);
            remaining_scores.remove(selected_idx);
        }

        if selected.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: selected.len(),
            });
        }

        Ok(selected)
    }

    /// Calculate effective anonymity considering both age and cluster
    /// similarity.
    ///
    /// This is a more accurate measure of privacy than age-only anonymity,
    /// as it accounts for cluster tag fingerprinting attacks.
    pub fn effective_anonymity_with_clusters(
        &self,
        ring: &[OutputCandidate],
        real_input: &OutputCandidate,
    ) -> f64 {
        if ring.is_empty() {
            return 0.0;
        }

        // For each ring member, calculate probability based on:
        // 1. Age plausibility (gamma distribution)
        // 2. Cluster similarity to the real input
        let probs: Vec<f64> = ring
            .iter()
            .map(|c| {
                let age_prob = self.weight_for_age(c.age_blocks);
                let cluster_sim = real_input.cluster_similarity(c);
                age_prob * cluster_sim
            })
            .collect();

        let total: f64 = probs.iter().sum();
        if total <= 0.0 {
            return 0.0;
        }

        // Normalize and calculate entropy
        let normalized: Vec<f64> = probs.iter().map(|p| p / total).collect();
        let entropy: f64 = normalized
            .iter()
            .filter(|&&p| p > 0.0)
            .map(|&p| -p * p.ln())
            .sum();

        entropy.exp()
    }

    /// Validate that a proposed ring will pass centroid-based consensus
    /// validation.
    ///
    /// This helps wallets verify their ring composition before creating the
    /// transaction. The ring must have output tags sufficiently similar to
    /// the value-weighted centroid of ring member tags.
    ///
    /// # Arguments
    /// * `ring` - The proposed ring members (real + decoys)
    /// * `output_tags` - The cluster tags that will be on the transaction
    ///   outputs
    /// * `threshold` - Minimum similarity required (recommend 0.7)
    ///
    /// # Returns
    /// * `Ok(similarity)` if the ring is valid, returning the actual similarity
    /// * `Err(similarity)` if the ring would fail validation, returning the
    ///   similarity
    pub fn validate_ring_centroid_compatibility(
        &self,
        ring: &[OutputCandidate],
        output_tags: &ClusterTags,
        threshold: f64,
    ) -> Result<f64, f64> {
        if ring.is_empty() {
            // Empty ring is invalid, but empty tags are maximally similar
            return if output_tags.is_empty() {
                Ok(1.0)
            } else {
                Err(0.0)
            };
        }

        // Compute value-weighted centroid
        let total_value: u64 = ring.iter().map(|c| c.output.amount).sum();
        if total_value == 0 {
            return if output_tags.is_empty() {
                Ok(1.0)
            } else {
                Err(0.0)
            };
        }

        // Accumulate weighted cluster masses
        let mut cluster_masses: std::collections::HashMap<ClusterId, u128> =
            std::collections::HashMap::new();

        for candidate in ring {
            for (cluster_id, weight) in candidate.cluster_tags.iter() {
                let mass = (candidate.output.amount as u128) * (weight as u128);
                *cluster_masses.entry(cluster_id).or_default() += mass;
            }
        }

        // Convert to centroid weights
        let centroid = ClusterTags::from_pairs(
            &cluster_masses
                .into_iter()
                .map(|(id, mass)| (id, (mass / (total_value as u128)) as TagWeight))
                .collect::<Vec<_>>(),
        );

        let similarity = centroid.cosine_similarity(output_tags);

        if similarity >= threshold {
            Ok(similarity)
        } else {
            Err(similarity)
        }
    }
}

/// Errors that can occur during decoy selection.
#[derive(Debug, Clone)]
pub enum DecoySelectionError {
    /// Not enough eligible candidates in the UTXO set
    InsufficientCandidates { required: usize, available: usize },
    /// Invalid gamma distribution parameters
    InvalidDistribution,
}

impl std::fmt::Display for DecoySelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientCandidates {
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient decoy candidates: need {}, have {}",
                    required, available
                )
            }
            Self::InvalidDistribution => {
                write!(f, "Invalid gamma distribution parameters")
            }
        }
    }
}

impl std::error::Error for DecoySelectionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_transaction_types::ClusterTagVector;

    fn make_candidate(
        target_key: [u8; 32],
        age_blocks: u64,
        current_height: u64,
    ) -> OutputCandidate {
        OutputCandidate {
            output: TxOutput {
                amount: 1000,
                target_key,
                public_key: [0u8; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
            },
            created_at: current_height.saturating_sub(age_blocks),
            age_blocks,
            cluster_tags: ClusterTags::empty(),
        }
    }

    fn make_candidate_with_tags(
        target_key: [u8; 32],
        age_blocks: u64,
        current_height: u64,
        cluster_tags: ClusterTags,
    ) -> OutputCandidate {
        OutputCandidate {
            output: TxOutput {
                amount: 1000,
                target_key,
                public_key: [0u8; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
            },
            created_at: current_height.saturating_sub(age_blocks),
            age_blocks,
            cluster_tags,
        }
    }

    #[test]
    fn test_spend_distribution_defaults() {
        let dist = SpendDistribution::new();
        assert!(!dist.has_observations());
        assert!((dist.shape() - DEFAULT_GAMMA_SHAPE).abs() < 0.01);
    }

    #[test]
    fn test_spend_distribution_updates() {
        let mut dist = SpendDistribution::new();

        // Record 200 spends with known distribution
        let mut rng = rand::thread_rng();
        for _ in 0..200 {
            // Simulate spends around 30 days (21600 blocks)
            let age = 15000 + rng.gen_range(0..15000);
            dist.record_spend(age);
        }

        assert!(dist.has_observations());
        assert!(dist.observation_count() >= 200);
    }

    #[test]
    fn test_decoy_selection_basic() {
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;

        // Create 20 candidates with varying ages
        let candidates: Vec<OutputCandidate> = (0..20)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8;
                let age = 100 + (i as u64 * 500); // Ages from 100 to 9600 blocks
                make_candidate(key, age, current_height)
            })
            .collect();

        let mut rng = rand::thread_rng();
        let exclude: Vec<[u8; 32]> = vec![];

        let decoys = selector.select_decoys(&candidates, 6, &exclude, current_height, &mut rng);
        assert!(decoys.is_ok());
        let decoys = decoys.unwrap();
        assert_eq!(decoys.len(), 6);
    }

    #[test]
    fn test_decoy_selection_excludes_keys() {
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;

        let candidates: Vec<OutputCandidate> = (0..20)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8;
                make_candidate(key, 1000 + i as u64 * 100, current_height)
            })
            .collect();

        let mut rng = rand::thread_rng();

        // Exclude first 5 keys
        let exclude: Vec<[u8; 32]> = (0..5)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8;
                key
            })
            .collect();

        let decoys = selector.select_decoys(&candidates, 6, &exclude, current_height, &mut rng);
        assert!(decoys.is_ok());
        let decoys = decoys.unwrap();

        // Verify none of the excluded keys are in the result
        for decoy in &decoys {
            assert!(!exclude.contains(&decoy.target_key));
        }
    }

    #[test]
    fn test_decoy_selection_insufficient_candidates() {
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;

        // Only 3 candidates
        let candidates: Vec<OutputCandidate> = (0..3)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8;
                make_candidate(key, 1000, current_height)
            })
            .collect();

        let mut rng = rand::thread_rng();
        let exclude: Vec<[u8; 32]> = vec![];

        // Try to select 6 decoys
        let result = selector.select_decoys(&candidates, 6, &exclude, current_height, &mut rng);
        assert!(matches!(
            result,
            Err(DecoySelectionError::InsufficientCandidates { .. })
        ));
    }

    #[test]
    fn test_effective_anonymity() {
        let selector = GammaDecoySelector::new();

        // Ring with similar ages should have higher anonymity
        let similar_ages = vec![1000, 1100, 1050, 980, 1020, 1080, 1010];
        let similar_anon = selector.effective_anonymity(&similar_ages);

        // Ring with diverse ages should have lower anonymity
        let diverse_ages = vec![100, 1000, 10000, 50000, 100000, 200000, 500000];
        let diverse_anon = selector.effective_anonymity(&diverse_ages);

        // Similar ages should provide better anonymity
        assert!(similar_anon > diverse_anon);
        eprintln!("Similar ages anonymity: {:.2}", similar_anon);
        eprintln!("Diverse ages anonymity: {:.2}", diverse_anon);
    }

    #[test]
    fn test_gamma_weighting_prefers_realistic_ages() {
        let selector = GammaDecoySelector::new();

        // With default parameters (mean ~30 days = ~21600 blocks),
        // ages around that range should have higher weight than extremes
        let weight_young = selector.weight_for_age(100); // 3 hours
        let weight_medium = selector.weight_for_age(21600); // 30 days
        let weight_old = selector.weight_for_age(525600); // 2 years

        eprintln!("Weight at 100 blocks: {}", weight_young);
        eprintln!("Weight at 21600 blocks: {}", weight_medium);
        eprintln!("Weight at 525600 blocks: {}", weight_old);

        // Medium-aged outputs should be preferred
        assert!(weight_medium > weight_young);
        assert!(weight_medium > weight_old);
    }

    // =========================================================================
    // Cluster Tag Tests
    // =========================================================================

    #[test]
    fn test_cluster_tags_empty() {
        let tags = ClusterTags::empty();
        assert!(tags.is_empty());
        assert_eq!(tags.len(), 0);
        assert_eq!(tags.total_weight(), 0);
    }

    #[test]
    fn test_cluster_tags_single() {
        let tags = ClusterTags::single(42);
        assert!(!tags.is_empty());
        assert_eq!(tags.len(), 1);
        assert_eq!(tags.get(42), TAG_WEIGHT_SCALE);
        assert_eq!(tags.total_weight(), TAG_WEIGHT_SCALE);
    }

    #[test]
    fn test_cluster_tags_from_pairs() {
        let tags = ClusterTags::from_pairs(&[
            (1, 500_000), // 50%
            (2, 300_000), // 30%
            (3, 200_000), // 20%
        ]);
        assert_eq!(tags.len(), 3);
        assert_eq!(tags.get(1), 500_000);
        assert_eq!(tags.get(2), 300_000);
        assert_eq!(tags.get(3), 200_000);
        assert_eq!(tags.total_weight(), TAG_WEIGHT_SCALE);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let tags1 = ClusterTags::single(42);
        let tags2 = ClusterTags::single(42);
        let sim = tags1.cosine_similarity(&tags2);
        assert!((sim - 1.0).abs() < 0.001, "Expected 1.0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_different() {
        let tags1 = ClusterTags::single(1);
        let tags2 = ClusterTags::single(2);
        let sim = tags1.cosine_similarity(&tags2);
        assert!(sim < 0.1, "Expected ~0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_partial_overlap() {
        let tags1 = ClusterTags::from_pairs(&[(1, 800_000), (2, 200_000)]);
        let tags2 = ClusterTags::from_pairs(&[(1, 600_000), (3, 400_000)]);
        let sim = tags1.cosine_similarity(&tags2);
        // Should have partial similarity due to shared cluster 1
        assert!(
            sim > 0.3 && sim < 0.9,
            "Expected partial similarity, got {sim}"
        );
    }

    #[test]
    fn test_cosine_similarity_empty_vectors() {
        let empty1 = ClusterTags::empty();
        let empty2 = ClusterTags::empty();
        let single = ClusterTags::single(42);

        // Empty vectors are considered fully similar
        assert!((empty1.cosine_similarity(&empty2) - 1.0).abs() < 0.001);
        // Empty to non-empty is also similar (fully diffused coins)
        assert!((empty1.cosine_similarity(&single) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_dominant_clusters_match() {
        let tags1 = ClusterTags::from_pairs(&[(1, 500_000), (2, 300_000), (3, 200_000)]);
        let tags2 = ClusterTags::from_pairs(&[(1, 450_000), (2, 350_000), (4, 200_000)]);
        // Top-3 overlap: clusters 1 and 2 match with similar weights
        assert!(tags1.dominant_clusters_match(&tags2));
    }

    #[test]
    fn test_dominant_clusters_no_match() {
        let tags1 = ClusterTags::from_pairs(&[(1, 500_000), (2, 300_000), (3, 200_000)]);
        let tags2 = ClusterTags::from_pairs(&[(10, 500_000), (20, 300_000), (30, 200_000)]);
        // No overlap in top-3 clusters
        assert!(!tags1.dominant_clusters_match(&tags2));
    }

    #[test]
    fn test_cluster_aware_selection_prefers_similar() {
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;

        // Create a real input with specific cluster profile
        let real_tags = ClusterTags::from_pairs(&[(1, 800_000), (2, 200_000)]);
        let mut real_key = [0u8; 32];
        real_key[0] = 255;
        let real_input = make_candidate_with_tags(real_key, 5000, current_height, real_tags);

        // Create candidates: some similar, some different
        let mut candidates = Vec::new();

        // Similar cluster profiles (should be preferred)
        for i in 0..10 {
            let mut key = [0u8; 32];
            key[0] = i as u8;
            let tags = ClusterTags::from_pairs(&[
                (1, 750_000 + (i as u32 * 10_000)),
                (2, 250_000 - (i as u32 * 10_000)),
            ]);
            candidates.push(make_candidate_with_tags(
                key,
                3000 + i * 100,
                current_height,
                tags,
            ));
        }

        // Different cluster profiles
        for i in 10..20 {
            let mut key = [0u8; 32];
            key[0] = i as u8;
            let tags = ClusterTags::from_pairs(&[(100, 900_000), (200, 100_000)]);
            candidates.push(make_candidate_with_tags(
                key,
                3000 + i * 100,
                current_height,
                tags,
            ));
        }

        let mut rng = rand::thread_rng();
        let exclude = vec![real_key];

        let result =
            selector.select_decoys_cluster_aware(&candidates, 6, &real_input, &exclude, &mut rng);

        assert!(result.is_ok());
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 6);

        // Most selected decoys should be from the similar group (keys 0-9)
        let similar_count = decoys.iter().filter(|d| d.target_key[0] < 10).count();

        eprintln!(
            "Selected {} decoys from similar cluster group",
            similar_count
        );
        assert!(
            similar_count >= 4,
            "Expected at least 4 similar, got {similar_count}"
        );
    }

    // =========================================================================
    // Age-similarity policy tests (issue #596)
    // =========================================================================

    #[test]
    fn test_age_similarity_band_bounds() {
        // ±10% band around a 30-day (~21600-block) input.
        let (min_age, max_age) = age_similarity_band(21_600);
        assert_eq!(min_age, 19_440); // 21600 - 10%
        assert_eq!(max_age, 23_760); // 21600 + 10%

        // Proportional band for a small age.
        let (min_small, max_small) = age_similarity_band(50);
        assert_eq!(min_small, 45); // 50 - 10%
        assert_eq!(max_small, 55); // 50 + 10%

        // Lower bound floors at the confirmation minimum when the proportional
        // band would dip below it.
        let (min_floor, max_floor) = age_similarity_band(11);
        assert_eq!(min_floor, MIN_DECOY_AGE_BLOCKS); // max(11 - 1, 10) = 10
        assert_eq!(max_floor, 12);
    }

    /// The core #596 guarantee: for an honest spender, the realized demurrage
    /// over-charge under the `ring_elapsed_quantile@max` kernel (PR #595) stays
    /// within the #314 tolerance (≤15%). The age-similarity band caps it at
    /// ≤1.10 by construction even when the candidate pool contains decoys far
    /// older than the real input.
    #[test]
    fn test_honest_overcharge_under_max_within_314_bound() {
        use bth_cluster_tax::demurrage::{demurrage_charge, ring_elapsed_quantile};

        const BLOCKS_PER_YEAR: u64 = 6_307_200; // 5s blocks
        const RATE_BPS: u32 = 200; // 2%/yr
        const VALUE: u64 = 1_000_000;
        // Worst case for demurrage magnitude: full 6x cluster factor.
        const FACTOR: u64 = 6_000;

        let selector = GammaDecoySelector::new();
        let current_height = 1_000_000u64;
        let real_input_age = 21_600u64; // ~30 days

        // Candidate pool spanning a WIDE age range, from far younger to ~2.8x
        // the real age — decoys that would over-charge the honest spender under
        // @max if the band were not enforced.
        let mut candidates = Vec::new();
        for i in 0..300u64 {
            let mut key = [0u8; 32];
            key[0] = (i % 251) as u8;
            key[1] = (i / 251) as u8;
            let age = 1_000 + i * 200; // 1000 .. 60800 blocks
            candidates.push(make_candidate(key, age, current_height));
        }

        let mut real_key = [0u8; 32];
        real_key[0] = 254;
        let exclude = vec![real_key];

        // Demurrage on the spender's TRUE holding age.
        let true_charge =
            demurrage_charge(VALUE, FACTOR, real_input_age, RATE_BPS, BLOCKS_PER_YEAR);
        assert!(true_charge > 0);

        let mut rng = rand::thread_rng();
        let mut worst_overcharge = 1.0f64;

        for _ in 0..300 {
            let decoys = selector
                .select_decoys_for_input(&candidates, 10, &exclude, real_input_age, &mut rng)
                .expect("age-similar selection should succeed with a dense pool");

            // Reconstruct ring members as (value, creation_height): the real
            // input plus the selected decoys.
            let mut members: Vec<(u64, u64)> = vec![(VALUE, current_height - real_input_age)];
            for d in &decoys {
                let age = candidates
                    .iter()
                    .find(|c| c.output.target_key == d.target_key)
                    .expect("decoy must come from the candidate pool")
                    .age_blocks;
                members.push((VALUE, current_height - age));
            }

            // @max order-statistic over ages (quantile_bps = 10000).
            let max_elapsed = ring_elapsed_quantile(&members, current_height, 10_000);
            let charge_max =
                demurrage_charge(VALUE, FACTOR, max_elapsed, RATE_BPS, BLOCKS_PER_YEAR);
            let overcharge = charge_max as f64 / true_charge as f64;
            worst_overcharge = worst_overcharge.max(overcharge);
        }

        eprintln!(
            "worst honest over-charge under ring_elapsed_quantile@max: {:.4}",
            worst_overcharge
        );

        // #314 bound (≤15%).
        assert!(
            worst_overcharge <= 1.15,
            "over-charge {worst_overcharge} breaches the #314 tolerance"
        );
        // ±10% band caps it at ≤1.10 by construction (small slack for integer
        // rounding in demurrage_charge).
        assert!(
            worst_overcharge <= 1.101,
            "over-charge {worst_overcharge} exceeds the ±10% band ceiling"
        );
    }

    /// Selection must never pick a decoy outside the age band when the band has
    /// enough candidates to fill the ring.
    #[test]
    fn test_age_similar_selection_stays_in_band() {
        let selector = GammaDecoySelector::new();
        let current_height = 1_000_000u64;
        let real_input_age = 21_600u64;
        let (min_age, max_age) = age_similarity_band(real_input_age);

        // Dense in-band pool plus far-out decoys that must never be chosen.
        let mut candidates = Vec::new();
        for i in 0..300u64 {
            let mut key = [0u8; 32];
            key[0] = (i % 251) as u8;
            key[1] = (i / 251) as u8;
            let age = 1_000 + i * 200;
            candidates.push(make_candidate(key, age, current_height));
        }

        let mut rng = rand::thread_rng();
        let exclude: Vec<[u8; 32]> = vec![];

        for _ in 0..100 {
            let decoys = selector
                .select_decoys_for_input(&candidates, 10, &exclude, real_input_age, &mut rng)
                .expect("selection should succeed");
            for d in &decoys {
                let age = candidates
                    .iter()
                    .find(|c| c.output.target_key == d.target_key)
                    .unwrap()
                    .age_blocks;
                assert!(
                    age >= min_age && age <= max_age,
                    "decoy age {age} outside band [{min_age}, {max_age}]"
                );
            }
        }
    }

    #[test]
    fn test_effective_anonymity_with_clusters() {
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;

        // Real input with cluster 1
        let real_tags = ClusterTags::single(1);
        let mut real_key = [0u8; 32];
        real_key[0] = 255;
        let real_input =
            make_candidate_with_tags(real_key, 5000, current_height, real_tags.clone());

        // Ring where all members have similar clusters (ring size 20)
        let similar_ring: Vec<OutputCandidate> = (0u64..20)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8;
                let tags = ClusterTags::from_pairs(&[(1, 900_000 + (i as u32) * 5_000)]);
                make_candidate_with_tags(key, 5000 + i * 100, current_height, tags)
            })
            .collect();

        let anon = selector.effective_anonymity_with_clusters(&similar_ring, &real_input);
        eprintln!("Similar clusters effective anonymity: {:.2}", anon);

        // With 20 similar-cluster ring members, effective anonymity should be high
        // (>10) Note: 12+ indicates excellent anonymity set (more than half the
        // ring is plausible)
        assert!(
            anon > 10.0,
            "Expected high anonymity with similar clusters, got {:.2}",
            anon
        );

        // Verify combined_similarity works as expected
        let high_match = ClusterTags::single(1);
        let low_match = ClusterTags::single(999);
        let high_sim = real_tags.combined_similarity(&high_match);
        let low_sim = real_tags.combined_similarity(&low_match);

        eprintln!(
            "High match similarity: {:.2}, Low match: {:.2}",
            high_sim, low_sim
        );
        assert!(
            high_sim > low_sim,
            "Matching cluster should have higher similarity"
        );
    }
}
