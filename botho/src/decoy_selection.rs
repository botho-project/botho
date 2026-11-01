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
//!
//! ## Per-input ring placement (#1692 / #1755)
//!
//! The ring producers (`GammaDecoySelector::select_decoys_for_input`, the
//! `botho-wallet` CLI and the web/Snap picker) do NOT gamma-weight decoys for a
//! known real input. They draw a random-offset age window of width ±10%
//! ([`draw_age_similarity_window`]) that contains the real input's age at a
//! uniformly random position, and sample decoys uniformly inside it, so the
//! real input's age rank in the ring is uniform. The gamma model below still
//! backs [`GammaDecoySelector::select_decoys`] and the anonymity estimators.

use rand::{seq::SliceRandom, Rng, RngExt};
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

/// Seconds per day.
const SECS_PER_DAY: u64 = 24 * 60 * 60;

/// Blocks per day at the mainnet monetary policy's target block time
/// (`crate::monetary::mainnet_policy().target_block_time_secs`, 5 s →
/// 17,280/day). Derived, not restated, so the gamma spend model's day-based
/// parameters track the chain's actual block time (#1692; it used to assume
/// 2-minute blocks, 720/day).
pub const BLOCKS_PER_DAY: u64 =
    SECS_PER_DAY / crate::monetary::mainnet_policy().target_block_time_secs;

/// Minimum age in blocks for decoy consideration (must be confirmed).
/// `pub` so `tests/sim_code_parity.rs` can pin the wallet and sim copies
/// (#1702).
pub const MIN_DECOY_AGE_BLOCKS: u64 = 10;

/// Maximum age to consider for gamma sampling (prevents extreme outliers):
/// ~2 years in blocks at [`BLOCKS_PER_DAY`]. `pub` for
/// `tests/sim_code_parity.rs` (#1702).
pub const MAX_DECOY_AGE_BLOCKS: u64 = 2 * 365 * BLOCKS_PER_DAY;

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
/// the #314-safe bands.
///
/// # Random-offset placement (#1692 / #1755, operator ruling 2026-11-01)
///
/// The band's total width stays `2 × spread` (20% of the real age), but its
/// POSITION is drawn per ring so the real input sits at a uniformly random
/// point inside it ([`draw_age_similarity_window`]), and decoys are drawn
/// uniformly from the candidates inside it. A band centred on the real input
/// makes the real input the median-age ring member under any symmetric in-band
/// draw (#1755), and the old gamma-target-clamped-into-the-band draw made it
/// the youngest or oldest member at most ages (#1692). With the offset band,
/// all ring members are exchangeable given the window, so over a pool of
/// locally uniform age density the real input's age rank is uniform.
///
/// Over-charge under `@max`: the realized over-charge is bounded by
/// `max_ring_age / real_age`, which the offset window caps at
/// `1 + 2 × spread` = 1.20 for a single ring (real input at the window's young
/// edge), versus 1.10 for the centred band. The #314 tolerance is checked on
/// the aggregate honest over-charge (`cluster-tax-sim decoy-quantile-sweep
/// --ring-size 20 --honest-jitter-bps 1000 --honest-band-offset`): 1.0825 at
/// ring 20, against 1.0855 for the centred band; about 19% of individual
/// honest spends exceed 1.15.
pub const AGE_SIMILARITY_SPREAD_BPS: u64 = 1_000;

/// Half-width `delta = real_age × spread` of the age band, in blocks.
fn age_similarity_delta(real_input_age: u64) -> u64 {
    (real_input_age as u128 * AGE_SIMILARITY_SPREAD_BPS as u128 / 10_000) as u64
}

/// Compute the inclusive `[min_age, max_age]` decoy-age band CENTRED on a real
/// input's age under the [`AGE_SIMILARITY_SPREAD_BPS`] policy.
///
/// This is the window at offset `delta`
/// (`age_similarity_window(real, age_similarity_window_width(real) / 2)`).
/// Since #1692 the pickers do NOT use the centred band: they draw the window
/// position at random with [`draw_age_similarity_window`]. It is kept as the
/// reference band of the shared web/native fixture and of the research sims.
///
/// The lower bound is additionally floored at [`MIN_DECOY_AGE_BLOCKS`] so
/// decoys are always confirmed. A degenerate real age (below the confirmation
/// floor) can produce `min_age > max_age`, in which case no candidate is
/// in-band and callers fall back to nearest-age selection.
pub fn age_similarity_band(real_input_age: u64) -> (u64, u64) {
    let delta = age_similarity_delta(real_input_age);
    let min_age = real_input_age
        .saturating_sub(delta)
        .max(MIN_DECOY_AGE_BLOCKS);
    let max_age = real_input_age.saturating_add(delta);
    (min_age, max_age)
}

/// Total width `W = 2 × delta` of the decoy age window, in blocks. The window
/// spans `W + 1` block ages and contains the real input's age at every offset.
pub fn age_similarity_window_width(real_input_age: u64) -> u64 {
    2 * age_similarity_delta(real_input_age)
}

/// The inclusive `[min_age, max_age]` decoy-age window at `offset` blocks
/// below the real input's age: `[real − offset, real − offset + W]`, lower
/// bound floored at [`MIN_DECOY_AGE_BLOCKS`]. `offset` is clamped to
/// `0..=W`; `offset = 0` puts the real input at the window's young edge,
/// `offset = W` at its old edge, and `offset = W / 2` is
/// [`age_similarity_band`]. Degenerate (`min_age > max_age`) exactly when
/// the real age is below the confirmation floor.
pub fn age_similarity_window(real_input_age: u64, offset: u64) -> (u64, u64) {
    let width = age_similarity_window_width(real_input_age);
    let offset = offset.min(width);
    let start = real_input_age - offset;
    (start.max(MIN_DECOY_AGE_BLOCKS), start + width)
}

/// Draw the random-offset decoy-age window for one ring (#1692): the offset
/// is uniform in `0..=W`, so the real input's age is uniform among the
/// window's `W + 1` ages. One draw (`rng.random_range(0..=W)`) per ring.
pub fn draw_age_similarity_window<R: Rng + ?Sized>(real_input_age: u64, rng: &mut R) -> (u64, u64) {
    let width = age_similarity_window_width(real_input_age);
    let offset = rng.random_range(0..=width);
    age_similarity_window(real_input_age, offset)
}

/// Union of every window [`draw_age_similarity_window`] can return:
/// `[real − W, real + W]` (lower bound floored at [`MIN_DECOY_AGE_BLOCKS`]).
/// A decoy chosen from a window always lies inside it.
pub fn age_similarity_envelope(real_input_age: u64) -> (u64, u64) {
    let width = age_similarity_window_width(real_input_age);
    (
        real_input_age
            .saturating_sub(width)
            .max(MIN_DECOY_AGE_BLOCKS),
        real_input_age.saturating_add(width),
    )
}

/// Fixed-point scale for cluster factors (1000 = 1.0×, 6000 = 6.0×). Matches
/// `bth_cluster_tax::ClusterFactorCurve::FACTOR_SCALE` and the demurrage
/// kernel.
pub const FACTOR_SCALE: u64 = 1_000;

/// Maximum cluster factor in FACTOR_SCALE units (6.0×).
pub const MAX_FACTOR_SCALED: u64 = 6 * FACTOR_SCALE;

/// Factor-similarity policy: maximum relative spread, in basis points, of a
/// selected decoy's cluster factor around the real input's factor
/// (1000 bps = ±10%).
///
/// # Why this exists (issue #925, consensus-economic dependency of the #834 fix)
///
/// #925 prices a demurrage class DOWNGRADE: the consensus fee floor charges the
/// capitalized future demurrage whenever a spend's declared output factor drops
/// **below** the value-weighted ring-implied input floor
/// (`bth_cluster_tax::spend_demurrage_charge`). For a genuine background (1×)
/// spend the floor is exactly 1× ⇒ no downgrade ⇒ zero charge — but ONLY if the
/// ring floor is exact. A single high-value **wealthy decoy** in a background
/// spender's ring inflates the value-weighted floor above 1× and spuriously
/// trips the downgrade charge (the honest-overcharge channel quantified in
/// `docs/research/demurrage-downgrade-overcharge.md` §4.5: 8.3% false positives
/// at just 1% contamination).
///
/// This band closes that channel at the wallet layer, exactly as
/// [`AGE_SIMILARITY_SPREAD_BPS`] closes the age-dilution channel for the
/// elapsed term: a spender draws only decoys whose cluster factor is within
/// ±10% of its real input's factor, so a background spender draws only near-1×
/// decoys and the ring floor stays exact.
///
/// # Why ±10% (docs/research/demurrage-downgrade-overcharge.md §4.6)
///
/// The band-width sweep (`downgrade_overcharge_sweep::factor_band_sweep`) shows
/// the honest false-positive rate stays at **exactly 0%** for every band up to
/// ±90% (the nearest wealthy class, 1.939×, only enters the band at ±100%),
/// while the same-class background pool is retained at 100% at every width
/// (factor is class-quantized, unlike continuous age). ±10% is adopted to match
/// the established [`AGE_SIMILARITY_SPREAD_BPS`] convention with a ~9× safety
/// margin to the first contaminating class, and it caps any in-band decoy's
/// factor at 1.100× — so a worst-case admitted decoy can lift the floor by at
/// most 10% of the 1× minimum. The gamma/OSPEAD age distribution and the
/// [`AGE_SIMILARITY_SPREAD_BPS`] age band still apply WITHIN the factor band.
pub const FACTOR_SIMILARITY_SPREAD: u64 = 1_000;

/// Compute the inclusive `[min_factor, max_factor]` decoy-factor band around a
/// real input's cluster factor under the [`FACTOR_SIMILARITY_SPREAD`] policy.
///
/// Mirrors [`age_similarity_band`]. Both bounds are clamped into the valid
/// factor range `[FACTOR_SCALE, MAX_FACTOR_SCALED]`, so a background (1×) real
/// input yields `[1000, 1100]` — admitting the whole background class while
/// excluding every wealthy class.
pub fn factor_similarity_band(real_input_factor: u64) -> (u64, u64) {
    let f = real_input_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
    let delta = (f as u128 * FACTOR_SIMILARITY_SPREAD as u128 / 10_000) as u64;
    let min_factor = f.saturating_sub(delta).max(FACTOR_SCALE);
    let max_factor = f.saturating_add(delta).min(MAX_FACTOR_SCALED);
    (min_factor, max_factor)
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
    /// The candidate's effective cluster factor in FACTOR_SCALE units
    /// (1000 = 1×, 6000 = 6×), resolved by the wallet from the output's tags
    /// against global per-cluster wealth. Drives the [`factor_similarity_band`]
    /// filter (#925). Defaults to `FACTOR_SCALE` (1× / background) when
    /// unknown.
    pub effective_factor: u64,
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
            effective_factor: FACTOR_SCALE,
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
            effective_factor: FACTOR_SCALE,
        }
    }

    /// Add cluster tags to an existing candidate.
    pub fn with_cluster_tags(mut self, cluster_tags: ClusterTags) -> Self {
        self.cluster_tags = cluster_tags;
        self
    }

    /// Set the candidate's effective cluster factor (FACTOR_SCALE units), used
    /// by the [`factor_similarity_band`] decoy filter (#925). The wallet
    /// resolves this from the output's cluster tags against global per-cluster
    /// wealth via `bth_cluster_tax::ClusterFactorCurve`.
    pub fn with_effective_factor(mut self, effective_factor: u64) -> Self {
        self.effective_factor = effective_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
        self
    }

    /// Age in days at the mainnet target block time ([`BLOCKS_PER_DAY`]).
    pub fn age_days(&self) -> f64 {
        self.age_blocks as f64 / BLOCKS_PER_DAY as f64
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
            gamma_scale_blocks: DEFAULT_GAMMA_SCALE_DAYS * BLOCKS_PER_DAY as f64,
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
            let sample = rng.random::<f64>() * current_total;
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
            let idx = rng.random_range(0..remaining_indices.len());
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

    /// Select decoys age-similar to the real input from a random-offset age
    /// window (#1692 / #1755).
    ///
    /// # Placement rule (shared by every ring producer)
    ///
    /// One window of width `W = 2 × spread × real_age` is drawn per ring with
    /// [`draw_age_similarity_window`], so the real input's age is uniform
    /// inside it, and decoys are drawn uniformly without replacement from the
    /// eligible candidates inside it. The CLI wallet
    /// (`botho-wallet` `select_rpc_decoys_from_history`) and the web/Snap
    /// picker (`web/packages/wasm-signer/src/decoys.ts`) use the same rule.
    /// The gamma spend distribution is NOT applied here: any age weighting
    /// inside the window would make the real input's rank non-uniform.
    ///
    /// # Consensus-economic role (issue #596)
    ///
    /// Under the `ring_elapsed_quantile@max` demurrage kernel (#577-B1 / PR
    /// #595) the charge is driven by the OLDEST ring member's age. A decoy much
    /// older than the real input therefore over-charges an honest spender. The
    /// window bounds every in-window decoy to at most `1 + 2 × spread` (1.20×)
    /// the real age; the aggregate honest over-charge stays inside the #314
    /// tolerance (see [`AGE_SIMILARITY_SPREAD_BPS`]).
    ///
    /// # Liveness fallback
    ///
    /// If the window cannot supply `count` decoys (sparse chain / unusual
    /// real age), the remaining slots are filled with the NEAREST-aged eligible
    /// candidates to the real input. This preserves transaction liveness while
    /// still minimizing the realized max ring age — and hence the honest
    /// over-charge — to the tightest value the available pool allows.
    ///
    /// # Randomness and tie-breaking (#1771)
    ///
    /// The eligible set is shuffled once with `rng`, then the window is drawn.
    /// Phase 1 takes the first `count` in-window candidates in shuffled order
    /// (a uniform sample), and phase 2 stable-sorts the rest by distance, so
    /// equal-distance ties are uniform too. Neither depends on the order of
    /// `candidates` (DB key / `tx_hash` order). Production callers must pass a
    /// CSPRNG (`OsRng`).
    pub fn select_decoys_for_input<R: Rng>(
        &self,
        candidates: &[OutputCandidate],
        count: usize,
        exclude_keys: &[[u8; 32]],
        real_input_age: u64,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        // Filter eligible candidates (confirmed, not excluded).
        let mut eligible: Vec<&OutputCandidate> = candidates
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

        // Randomize the selection order (#1771). Phase 1 takes in-window
        // candidates in this order, and phase 2 uses a stable sort by
        // distance (ties keep this order). Without the shuffle, ties and the
        // in-window choice would follow CANDIDATE order, which callers derive
        // from DB key order (`tx_hash || index`). An attacker who grinds
        // output hashes to sort first would then capture every decoy slot.
        // Shuffling with the caller's CSPRNG makes every choice independent of
        // candidate order and of `tx_hash`/`UtxoId` bytes.
        eligible.shuffle(rng);

        // Random-offset age window (#1692 / #1755): the real input's age is
        // uniform inside it.
        let (min_age, max_age) = draw_age_similarity_window(real_input_age, rng);

        let mut selected = Vec::with_capacity(count);
        let mut used_keys = exclude_keys.to_vec();

        // Phase 1: a uniform sample without replacement from the window (the
        // first `count` in-window candidates of the shuffled set).
        //
        // A real input younger than `MIN_DECOY_AGE_BLOCKS` gives a degenerate
        // window (`min_age > max_age`, issue #611): nothing is in-window and
        // phase 2 fills the ring.
        if min_age <= max_age {
            for candidate in &eligible {
                if selected.len() >= count {
                    break;
                }
                if candidate.age_blocks < min_age
                    || candidate.age_blocks > max_age
                    || used_keys.contains(&candidate.output.target_key)
                {
                    continue;
                }
                selected.push(candidate.output.clone());
                used_keys.push(candidate.output.target_key);
            }
        }

        // Phase 2 (liveness fallback): fill any remaining slots with the
        // nearest-aged eligible candidates so over-charge stays as small as the
        // pool allows even when the window cannot be fully satisfied.
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
                // Earlier fallback entries may have added this target key.
                if used_keys.contains(&candidate.output.target_key) {
                    continue;
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
        // #925 factor-similarity band: only draw decoys whose cluster factor is
        // within ±FACTOR_SIMILARITY_SPREAD of the real input's factor, so a
        // background (1×) spender draws only near-1× decoys and the consensus
        // ring floor stays exact — a wealthy decoy would inflate the
        // value-weighted floor and spuriously trip the #925 downgrade charge
        // (docs/research/demurrage-downgrade-overcharge.md §4.5/§4.6). The band
        // is applied ON TOP of the age + cluster-similarity filters; the gamma
        // age distribution still applies within it.
        let (min_factor, max_factor) = factor_similarity_band(real_input.effective_factor);
        let in_factor_band = |c: &OutputCandidate| {
            c.effective_factor >= min_factor && c.effective_factor <= max_factor
        };

        // Filter candidates by:
        // 1. Minimum age
        // 2. Not excluded
        // 3. Factor-similarity band (#925)
        // 4. Cluster similarity above threshold
        let cluster_compatible: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                    && !exclude_keys.contains(&c.output.target_key)
                    && in_factor_band(c)
                    && real_input.cluster_similarity(c) >= MIN_CLUSTER_SIMILARITY
            })
            .collect();

        // If we have enough cluster-compatible, factor-banded candidates, use them
        if cluster_compatible.len() >= count {
            return self.select_from_pool(&cluster_compatible, count, exclude_keys, rng);
        }

        // Fallback 1: keep the factor band (it is consensus-economically
        // load-bearing) but relax the cluster-similarity requirement.
        let factor_banded: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                    && !exclude_keys.contains(&c.output.target_key)
                    && in_factor_band(c)
            })
            .collect();
        if factor_banded.len() >= count {
            return self.select_with_cluster_scoring(
                &factor_banded,
                count,
                real_input,
                exclude_keys,
                rng,
            );
        }

        // Fallback 2 (liveness): a sparse chain cannot supply `count` in-band
        // decoys. Rather than fail the spend, relax the factor band and select
        // from the full eligible pool. The #925 charge is liveness-safe (it only
        // ever raises the fee floor), so a rare over-charge here is preferable to
        // a stuck transaction — and the wallet can surface the higher fee.
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

            let sample = rng.random::<f64>() * current_total;
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

            let sample = rng.random::<f64>() * current_total;
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
                kem_ciphertext: None,
            },
            created_at: current_height.saturating_sub(age_blocks),
            age_blocks,
            cluster_tags: ClusterTags::empty(),
            effective_factor: FACTOR_SCALE,
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
                kem_ciphertext: None,
            },
            created_at: current_height.saturating_sub(age_blocks),
            age_blocks,
            cluster_tags,
            effective_factor: FACTOR_SCALE,
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
        let mut rng = rand::rng();
        for _ in 0..200 {
            // Simulate spends around 30 days (21600 blocks)
            let age = 15000 + rng.random_range(0..15000);
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

        let mut rng = rand::rng();
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

        let mut rng = rand::rng();

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

        let mut rng = rand::rng();
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

        // With default parameters (mean ~31 days at the policy block time),
        // ages around that range should have higher weight than extremes.
        // Ages are in days × BLOCKS_PER_DAY (#1692: 5 s blocks, not 2 min).
        let weight_young = selector.weight_for_age(100); // minutes
        let weight_medium = selector.weight_for_age(30 * BLOCKS_PER_DAY); // 30 days
        let weight_old = selector.weight_for_age(MAX_DECOY_AGE_BLOCKS); // 2 years

        eprintln!("Weight at 100 blocks: {}", weight_young);
        eprintln!("Weight at 30 days: {}", weight_medium);
        eprintln!("Weight at 2 years: {}", weight_old);

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

        let mut rng = rand::rng();
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

    /// Shared web<->native conformance fixture (#1693). The web wallet / Snap /
    /// claim-link decoy picker (`web/packages/wasm-signer/src/decoys.ts`)
    /// checks the SAME JSON against its `ageSimilarityBand` port, so a
    /// change to the band policy on either side fails one of the two suites
    /// until both agree.
    #[test]
    fn test_age_similarity_band_matches_shared_web_fixture() {
        let raw = include_str!("../../web/packages/wasm-signer/test/fixtures/decoy-age-band.json");
        let fixture: serde_json::Value = serde_json::from_str(raw).expect("fixture parses");
        assert_eq!(
            fixture["minDecoyAgeBlocks"].as_u64().unwrap(),
            MIN_DECOY_AGE_BLOCKS
        );
        assert_eq!(
            fixture["ageSimilaritySpreadBps"].as_u64().unwrap(),
            AGE_SIMILARITY_SPREAD_BPS
        );

        let bands = fixture["bands"].as_array().unwrap();
        assert!(!bands.is_empty());
        for v in bands {
            let real = v["realAge"].as_u64().unwrap();
            let (min_age, max_age) = age_similarity_band(real);
            assert_eq!(min_age, v["minAge"].as_u64().unwrap(), "minAge for {real}");
            assert_eq!(max_age, v["maxAge"].as_u64().unwrap(), "maxAge for {real}");
            assert_eq!(
                min_age > max_age,
                v["degenerate"].as_bool().unwrap(),
                "degenerate flag for {real}"
            );
        }

        // Random-offset windows (#1692): the placement rule both pickers draw.
        let windows = fixture["windows"].as_array().unwrap();
        assert!(!windows.is_empty());
        for v in windows {
            let real = v["realAge"].as_u64().unwrap();
            let offset = v["offset"].as_u64().unwrap();
            assert_eq!(
                age_similarity_window_width(real),
                v["width"].as_u64().unwrap(),
                "width for {real}"
            );
            let (min_age, max_age) = age_similarity_window(real, offset);
            assert_eq!(
                min_age,
                v["minAge"].as_u64().unwrap(),
                "minAge for {real}@{offset}"
            );
            assert_eq!(
                max_age,
                v["maxAge"].as_u64().unwrap(),
                "maxAge for {real}@{offset}"
            );
            assert_eq!(
                min_age > max_age,
                v["degenerate"].as_bool().unwrap(),
                "degenerate flag for {real}@{offset}"
            );
        }

        // Fallback vectors: the selected SET is RNG-independent, so the native
        // selector must produce exactly the ages the web picker is pinned to.
        let current_height = 100_000;
        let selector = GammaDecoySelector::new();
        for case in fixture["fallback"].as_array().unwrap() {
            let real = case["realAge"].as_u64().unwrap();
            let count = case["count"].as_u64().unwrap() as usize;
            let candidates: Vec<OutputCandidate> = case["poolAges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    let age = a.as_u64().unwrap();
                    let mut key = [0u8; 32];
                    key[..8].copy_from_slice(&age.to_le_bytes());
                    make_candidate(key, age, current_height)
                })
                .collect();
            let mut expected: Vec<u64> = case["expectedAges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_u64().unwrap())
                .collect();
            expected.sort_unstable();
            for _ in 0..32 {
                let mut rng = rand::rng();
                let picked = selector
                    .select_decoys_for_input(&candidates, count, &[], real, &mut rng)
                    .expect("fallback case selects");
                let mut ages: Vec<u64> = picked
                    .iter()
                    .map(|o| u64::from_le_bytes(o.target_key[..8].try_into().unwrap()))
                    .collect();
                ages.sort_unstable();
                assert_eq!(ages, expected, "{}", case["name"]);
            }
        }
    }

    /// The #596 guarantee under the random-offset window (#1692): for an
    /// honest spender, the realized demurrage over-charge under the
    /// `ring_elapsed_quantile@max` kernel (PR #595) is capped per ring at
    /// `1 + 2 × spread` (1.20) by construction, even when the candidate pool
    /// contains decoys far older than the real input, and the AGGREGATE
    /// over-charge (Σ realized / Σ true, the metric the #314 tolerance and the
    /// `decoy-quantile-sweep` gate use) stays within 1.15.
    #[test]
    fn test_honest_overcharge_under_max_within_314_bound() {
        use bth_cluster_tax::demurrage::{demurrage_charge, ring_elapsed_quantile};
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        const BLOCKS_PER_YEAR: u64 = 6_307_200; // 5s blocks
        const RATE_BPS: u32 = 200; // 2%/yr
        const VALUE: u64 = 1_000_000;
        // Worst case for demurrage magnitude: full 6x cluster factor.
        const FACTOR: u64 = 6_000;
        // Ring size 20: the consensus minimum.
        const DECOYS: usize = 19;

        let selector = GammaDecoySelector::new();
        let current_height = 10_000_000u64;
        let real_input_age = 1_000_000u64; // ~58 days at 5s blocks

        // Dense pool spanning far younger to ~2.5x the real age — decoys that
        // would over-charge the honest spender under @max if the window were
        // not enforced.
        let mut candidates = Vec::new();
        for i in 0..2_500u64 {
            let mut key = [0u8; 32];
            key[..8].copy_from_slice(&i.to_le_bytes());
            let age = 1_000 + i * 1_000; // 1k .. 2.5M blocks
            candidates.push(make_candidate(key, age, current_height));
        }

        let mut real_key = [0u8; 32];
        real_key[31] = 254;
        let exclude = vec![real_key];

        // Demurrage on the spender's TRUE holding age.
        let true_charge =
            demurrage_charge(VALUE, FACTOR, real_input_age, RATE_BPS, BLOCKS_PER_YEAR);
        assert!(true_charge > 0);

        let mut rng = ChaCha8Rng::seed_from_u64(0x1692);
        let mut worst_overcharge = 1.0f64;
        let (mut realized_sum, mut true_sum) = (0u128, 0u128);

        for _ in 0..2_000 {
            let decoys = selector
                .select_decoys_for_input(&candidates, DECOYS, &exclude, real_input_age, &mut rng)
                .expect("age-similar selection should succeed with a dense pool");

            // Reconstruct ring members as (value, creation_height): the real
            // input plus the selected decoys.
            let mut members: Vec<(u64, u64)> = vec![(VALUE, current_height - real_input_age)];
            for d in &decoys {
                let age = u64::from_le_bytes(d.target_key[..8].try_into().unwrap()) * 1_000 + 1_000;
                members.push((VALUE, current_height - age));
            }

            // @max order-statistic over ages (quantile_bps = 10000).
            let max_elapsed = ring_elapsed_quantile(&members, current_height, 10_000);
            let charge_max =
                demurrage_charge(VALUE, FACTOR, max_elapsed, RATE_BPS, BLOCKS_PER_YEAR);
            let overcharge = charge_max as f64 / true_charge as f64;
            worst_overcharge = worst_overcharge.max(overcharge);
            realized_sum += charge_max as u128;
            true_sum += true_charge as u128;
        }
        let aggregate = realized_sum as f64 / true_sum as f64;

        eprintln!(
            "honest over-charge under ring_elapsed_quantile@max: aggregate {aggregate:.4}, \
             worst ring {worst_overcharge:.4}"
        );

        // #314 tolerance (≤15%) on the aggregate.
        assert!(
            aggregate <= 1.15,
            "aggregate over-charge {aggregate} breaches the #314 tolerance"
        );
        // The window caps a single ring at 1 + 2 × spread = 1.20 by
        // construction (small slack for integer rounding in demurrage_charge).
        let cap = 1.0 + 2.0 * AGE_SIMILARITY_SPREAD_BPS as f64 / 10_000.0;
        assert!(
            worst_overcharge <= cap + 0.001,
            "over-charge {worst_overcharge} exceeds the window ceiling {cap}"
        );
    }

    /// Positions of the real input's age among a ring's ages: the number of
    /// decoys strictly younger, plus a uniformly random share of equal-age
    /// decoys (equal ages are indistinguishable to an observer). 0 = youngest.
    fn real_age_rank<R: Rng>(real_age: u64, decoy_ages: &[u64], rng: &mut R) -> usize {
        let younger = decoy_ages.iter().filter(|&&a| a < real_age).count();
        let equal = decoy_ages.iter().filter(|&&a| a == real_age).count();
        younger + rng.random_range(0..=equal)
    }

    /// #1692 acceptance: over a dense pool, the real input's age rank within
    /// its ring of 20 is uniform (χ² p > 0.01, df 19) at every real age from
    /// 100 to 1M blocks. Before #1692 the edge-clamped gamma draw made the real
    /// input the youngest member at 1k/5k and the oldest at 100k/1M; a band
    /// centred on the real input would make it the median member (#1755).
    #[test]
    fn real_input_age_rank_is_uniform_at_all_ages() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        const DECOYS: usize = 19;
        const RINGS: usize = 4_000;
        // χ² critical value, df 19, p = 0.01.
        const CHI2_CRIT_DF19_P01: f64 = 36.191;

        let selector = GammaDecoySelector::new();
        let current_height = 2_000_000u64;
        for real_age in [100u64, 1_000, 5_000, 30_000, 100_000, 1_000_000] {
            // Dense pool covering the envelope of every possible window plus
            // margin, at locally uniform density: about 1000 candidates per
            // window width, several outputs per age when the window is narrow.
            let width = age_similarity_window_width(real_age);
            let (lo, hi) = age_similarity_envelope(real_age);
            let (lo, hi) = (
                lo.saturating_sub(width).max(MIN_DECOY_AGE_BLOCKS),
                hi + width,
            );
            let step = (width / 1_000).max(1);
            let per_age = (1_000 / (width + 1)).max(1);
            let mut candidates = Vec::new();
            let mut id = 0u64;
            let mut age = lo;
            while age <= hi {
                for _ in 0..per_age {
                    let mut key = [0u8; 32];
                    key[..8].copy_from_slice(&id.to_le_bytes());
                    key[8..16].copy_from_slice(&age.to_le_bytes());
                    candidates.push(make_candidate(key, age, current_height));
                    id += 1;
                }
                age += step;
            }

            let mut rng = ChaCha8Rng::seed_from_u64(0x1692 ^ real_age);
            let mut hist = [0usize; DECOYS + 1];
            for _ in 0..RINGS {
                let decoys = selector
                    .select_decoys_for_input(&candidates, DECOYS, &[], real_age, &mut rng)
                    .expect("dense pool fills the ring");
                let ages: Vec<u64> = decoys
                    .iter()
                    .map(|o| u64::from_le_bytes(o.target_key[8..16].try_into().unwrap()))
                    .collect();
                hist[real_age_rank(real_age, &ages, &mut rng)] += 1;
            }
            let expected = RINGS as f64 / (DECOYS + 1) as f64;
            let chi2: f64 = hist
                .iter()
                .map(|&o| (o as f64 - expected).powi(2) / expected)
                .sum();
            eprintln!("real age {real_age}: rank χ² = {chi2:.1}, histogram {hist:?}");
            assert!(
                chi2 < CHI2_CRIT_DF19_P01,
                "real age {real_age}: rank histogram {hist:?} is not uniform (χ² = {chi2:.1})"
            );
        }
    }

    /// The random-offset window always contains the real age, has width
    /// `2 × spread`, and its offset spans the whole envelope.
    #[test]
    fn test_age_similarity_window_bounds() {
        let real = 21_600u64;
        let width = age_similarity_window_width(real);
        assert_eq!(width, 4_320);
        assert_eq!(age_similarity_window(real, 0), (21_600, 25_920));
        assert_eq!(age_similarity_window(real, width), (17_280, 21_600));
        assert_eq!(
            age_similarity_window(real, width / 2),
            age_similarity_band(real)
        );
        // Offsets past the width clamp to the old-edge window.
        assert_eq!(age_similarity_window(real, width + 9), (17_280, 21_600));
        assert_eq!(age_similarity_envelope(real), (17_280, 25_920));
        // Below the confirmation floor the window is degenerate.
        let (lo, hi) = age_similarity_window(9, 0);
        assert!(lo > hi);
        // Near the floor the lower bound clamps but the window keeps the real age.
        assert_eq!(age_similarity_window(11, 2), (10, 11));

        let mut rng = rand::rng();
        for real in [10u64, 11, 50, 1_000, 1_000_000] {
            let (env_lo, env_hi) = age_similarity_envelope(real);
            for _ in 0..200 {
                let (lo, hi) = draw_age_similarity_window(real, &mut rng);
                assert!(lo <= real && real <= hi, "{real}: [{lo}, {hi}]");
                assert!(env_lo <= lo && hi <= env_hi);
                assert!(hi - lo <= age_similarity_window_width(real));
            }
        }
    }

    /// Selection must never pick a decoy outside a single random-offset window
    /// containing the real age when the window has enough candidates to fill
    /// the ring: every decoy lies in the envelope, and the whole ring
    /// (real input included) spans at most the window width.
    #[test]
    fn test_age_similar_selection_stays_in_band() {
        let selector = GammaDecoySelector::new();
        let current_height = 1_000_000u64;
        let real_input_age = 21_600u64;
        let (env_lo, env_hi) = age_similarity_envelope(real_input_age);
        let width = age_similarity_window_width(real_input_age);

        // Dense pool plus far-out decoys that must never be chosen.
        let mut candidates = Vec::new();
        for i in 0..1_200u64 {
            let mut key = [0u8; 32];
            key[0] = (i % 251) as u8;
            key[1] = (i / 251) as u8;
            let age = 1_000 + i * 50;
            candidates.push(make_candidate(key, age, current_height));
        }

        let mut rng = rand::rng();
        let exclude: Vec<[u8; 32]> = vec![];

        for _ in 0..100 {
            let decoys = selector
                .select_decoys_for_input(&candidates, 19, &exclude, real_input_age, &mut rng)
                .expect("selection should succeed");
            let mut ages = vec![real_input_age];
            for d in &decoys {
                let age = candidates
                    .iter()
                    .find(|c| c.output.target_key == d.target_key)
                    .unwrap()
                    .age_blocks;
                assert!(
                    age >= env_lo && age <= env_hi,
                    "decoy age {age} outside envelope [{env_lo}, {env_hi}]"
                );
                ages.push(age);
            }
            let span = ages.iter().max().unwrap() - ages.iter().min().unwrap();
            assert!(
                span <= width,
                "ring age span {span} exceeds window width {width}"
            );
        }
    }

    #[test]
    fn test_age_fallback_uses_each_target_key_once() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let candidates = vec![
            make_candidate([9; 32], 10, 1000), // Explicitly excluded.
            make_candidate([8; 32], 9, 1000),  // Not mature.
            make_candidate([1; 32], 100, 1000),
            make_candidate([1; 32], 100, 1000),
            make_candidate([2; 32], 120, 1000),
            make_candidate([2; 32], 121, 1000),
            make_candidate([3; 32], 130, 1000),
        ];
        // Exercise both fallback-only and partially filled in-band selection.
        for real_age in [0, 100] {
            let selected = GammaDecoySelector::new()
                .select_decoys_for_input(
                    &candidates,
                    3,
                    &[[9; 32]],
                    real_age,
                    &mut ChaCha8Rng::seed_from_u64(7),
                )
                .unwrap();
            let keys: Vec<_> = selected.iter().map(|output| output.target_key).collect();
            assert_eq!(keys, vec![[1; 32], [2; 32], [3; 32]]);
        }
    }

    #[test]
    fn test_age_fallback_reports_insufficient_unique_targets() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let candidates = vec![
            make_candidate([1; 32], 100, 1000),
            make_candidate([1; 32], 100, 1000),
            make_candidate([2; 32], 120, 1000),
            make_candidate([2; 32], 121, 1000),
            make_candidate([3; 32], 130, 1000),
        ];
        for real_age in [0, 100] {
            let result = GammaDecoySelector::new().select_decoys_for_input(
                &candidates,
                4,
                &[],
                real_age,
                &mut ChaCha8Rng::seed_from_u64(7),
            );
            assert!(matches!(
                result,
                Err(DecoySelectionError::InsufficientCandidates {
                    required: 4,
                    available: 3,
                })
            ));
        }
    }

    /// Regression for issue #611: a real input younger than
    /// `MIN_DECOY_AGE_BLOCKS` produces a degenerate age band (`min_age >
    /// max_age`). The phase-1 loop used to call `u64::clamp(min_age,
    /// max_age)` unconditionally, which panics when `min_age > max_age`.
    /// The fix skips phase 1 for degenerate bands and lets the nearest-age
    /// phase-2 fallback fill the ring.
    #[test]
    fn test_young_real_input_no_panic() {
        let selector = GammaDecoySelector::new();
        let current_height = 1_000_000u64;
        let count = 10usize;

        // Dense pool of confirmed candidates (all aged >= MIN_DECOY_AGE_BLOCKS)
        // so phase 2 can always supply a full ring.
        let mut candidates = Vec::new();
        for i in 0..50u64 {
            let mut key = [0u8; 32];
            key[0] = (i % 251) as u8;
            key[1] = (i / 251) as u8;
            let age = MIN_DECOY_AGE_BLOCKS + i; // 10 .. 59
            candidates.push(make_candidate(key, age, current_height));
        }
        let exclude: Vec<[u8; 32]> = vec![];
        let mut rng = rand::rng();

        // AC-1 / AC-4: age 0 must not panic (degenerate band [10, 0]).
        // AC-2: phase 2 fills the full ring; every decoy is confirmed (age >= 10).
        let decoys_0 = selector
            .select_decoys_for_input(&candidates, count, &exclude, 0, &mut rng)
            .expect("age-0 real input must fall back to phase-2 selection, not panic");
        assert_eq!(
            decoys_0.len(),
            count,
            "phase-2 fallback must fill the full ring for a degenerate band"
        );
        for d in &decoys_0 {
            let age = candidates
                .iter()
                .find(|c| c.output.target_key == d.target_key)
                .expect("decoy must come from the candidate pool")
                .age_blocks;
            assert!(
                age >= MIN_DECOY_AGE_BLOCKS,
                "phase-2 decoy age {age} below confirmation floor"
            );
        }

        // AC-5: age 9 is the last degenerate value (band [10, 9]). Same fallback.
        let decoys_9 = selector
            .select_decoys_for_input(&candidates, count, &exclude, 9, &mut rng)
            .expect("age-9 real input must fall back to phase-2 selection, not panic");
        assert_eq!(decoys_9.len(), count);
        for d in &decoys_9 {
            let age = candidates
                .iter()
                .find(|c| c.output.target_key == d.target_key)
                .expect("decoy must come from the candidate pool")
                .age_blocks;
            assert!(
                age >= MIN_DECOY_AGE_BLOCKS,
                "phase-2 decoy age {age} below confirmation floor"
            );
        }

        // AC-5: age 10 is the first NON-degenerate value. The band is [10, 11]
        // (delta = 10 * 1000 / 10000 = 1), so phase 1 runs. Confirm phase-1
        // in-band selection by giving the pool a dense supply at ages 10 and 11
        // and asserting every decoy lands inside the band.
        let (min10, max10) = age_similarity_band(10);
        assert_eq!((min10, max10), (10, 11), "band for age 10 must be [10, 11]");
        let mut in_band_pool = Vec::new();
        for i in 0..30u64 {
            let mut key = [0u8; 32];
            key[0] = (i % 251) as u8;
            key[1] = (i / 251) as u8;
            let age = 10 + (i % 2); // alternate 10 and 11, both in-band
            in_band_pool.push(make_candidate(key, age, current_height));
        }
        for _ in 0..50 {
            let decoys_10 = selector
                .select_decoys_for_input(&in_band_pool, count, &exclude, 10, &mut rng)
                .expect("age-10 real input must select via phase 1");
            assert_eq!(decoys_10.len(), count);
            for d in &decoys_10 {
                let age = in_band_pool
                    .iter()
                    .find(|c| c.output.target_key == d.target_key)
                    .expect("decoy must come from the candidate pool")
                    .age_blocks;
                assert!(
                    (min10..=max10).contains(&age),
                    "age-10 decoy age {age} outside phase-1 band [{min10}, {max10}]"
                );
            }
        }
    }

    /// AC-6: the fix must not perturb the non-degenerate path's RNG
    /// consumption. Given the same seed and pool, `select_decoys_for_input`
    /// returns the same decoy set (same members, same order) across runs.
    #[test]
    fn test_select_decoys_deterministic_with_seed() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let selector = GammaDecoySelector::new();
        let current_height = 1_000_000u64;
        let real_input_age = 21_600u64; // non-degenerate: phase 1 runs
        let count = 10usize;

        let mut candidates = Vec::new();
        for i in 0..300u64 {
            let mut key = [0u8; 32];
            key[0] = (i % 251) as u8;
            key[1] = (i / 251) as u8;
            let age = 1_000 + i * 200;
            candidates.push(make_candidate(key, age, current_height));
        }
        let exclude: Vec<[u8; 32]> = vec![];

        let mut rng_a = ChaCha8Rng::seed_from_u64(0xB0_1234_5678);
        let decoys_a = selector
            .select_decoys_for_input(&candidates, count, &exclude, real_input_age, &mut rng_a)
            .expect("selection should succeed");

        let mut rng_b = ChaCha8Rng::seed_from_u64(0xB0_1234_5678);
        let decoys_b = selector
            .select_decoys_for_input(&candidates, count, &exclude, real_input_age, &mut rng_b)
            .expect("selection should succeed");

        let keys_a: Vec<[u8; 32]> = decoys_a.iter().map(|d| d.target_key).collect();
        let keys_b: Vec<[u8; 32]> = decoys_b.iter().map(|d| d.target_key).collect();
        assert_eq!(
            keys_a, keys_b,
            "same seed + pool must yield identical decoy selection"
        );
    }

    #[test]
    fn test_effective_anonymity_with_clusters() {
        let selector = GammaDecoySelector::new();
        let current_height = 2_000_000;
        // Near the gamma spend model's mode (~29 days at the policy block
        // time), where age alone leaves the ring members comparably plausible.
        let base_age = 29 * BLOCKS_PER_DAY;

        // Real input with cluster 1
        let real_tags = ClusterTags::single(1);
        let mut real_key = [0u8; 32];
        real_key[0] = 255;
        let real_input =
            make_candidate_with_tags(real_key, base_age, current_height, real_tags.clone());

        // Ring where all members have similar clusters (ring size 20)
        let similar_ring: Vec<OutputCandidate> = (0u64..20)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8;
                let tags = ClusterTags::from_pairs(&[(1, 900_000 + (i as u32) * 5_000)]);
                make_candidate_with_tags(key, base_age + i * 100, current_height, tags)
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

    // =========================================================================
    // Factor-similarity band tests (#925)
    // =========================================================================

    #[test]
    fn test_factor_similarity_band_background() {
        // A background (1×) real input admits [1000, 1100] — the whole
        // background class, excluding every wealthy class.
        let (lo, hi) = factor_similarity_band(FACTOR_SCALE);
        assert_eq!(lo, 1_000);
        assert_eq!(hi, 1_100);
        // The nearest wealthy class (1.939× = 1939) is well outside the band.
        assert!(1_939 > hi);
    }

    #[test]
    fn test_factor_similarity_band_clamps() {
        // Below-1× and above-6× inputs clamp into the valid factor range.
        let (lo, _) = factor_similarity_band(0);
        assert_eq!(lo, FACTOR_SCALE);
        let (_, hi) = factor_similarity_band(u64::MAX);
        assert_eq!(hi, MAX_FACTOR_SCALED);
        // A mid-tier factor gets a symmetric ±10% band.
        let (lo, hi) = factor_similarity_band(3_000);
        assert_eq!(lo, 2_700);
        assert_eq!(hi, 3_300);
    }

    #[test]
    fn test_factor_band_excludes_wealthy_decoys_for_background_spender() {
        // A background spender's ring must exclude a high-value wealthy decoy —
        // exactly the §4.5 contamination the #925 charge would otherwise trip.
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;
        let mut real_key = [0u8; 32];
        real_key[0] = 200;
        // Background real input (factor 1×).
        let real_input = make_candidate(real_key, 5_000, current_height);
        assert_eq!(real_input.effective_factor, FACTOR_SCALE);

        // Candidate pool: plenty of background (1×) decoys + a few wealthy ones.
        let mut candidates = Vec::new();
        for i in 0..15u8 {
            let mut key = [0u8; 32];
            key[0] = i;
            candidates.push(make_candidate(key, 4_800 + i as u64 * 10, current_height));
        }
        for i in 0..5u8 {
            let mut key = [0u8; 32];
            key[0] = 100 + i;
            // Wealthy decoy: factor 5.745× — the §4.5 contaminant.
            candidates.push(
                make_candidate(key, 4_800 + i as u64 * 10, current_height)
                    .with_effective_factor(5_745),
            );
        }

        let mut rng = rand::rng();
        let exclude = vec![real_key];
        let decoys = selector
            .select_decoys_cluster_aware(&candidates, 6, &real_input, &exclude, &mut rng)
            .expect("selection succeeds with ample in-band background decoys");
        assert_eq!(decoys.len(), 6);

        // None of the selected decoys may be one of the wealthy keys (100..105):
        // the factor band filtered them out, so the ring floor stays 1×.
        for d in &decoys {
            assert!(
                d.target_key[0] < 100,
                "a wealthy decoy leaked past the factor band"
            );
        }
    }

    #[test]
    fn test_factor_band_liveness_fallback_when_pool_sparse() {
        // If the in-band pool cannot supply `count`, selection still succeeds
        // (liveness fallback), rather than failing the spend.
        let selector = GammaDecoySelector::new();
        let current_height = 100_000;
        let mut real_key = [0u8; 32];
        real_key[0] = 200;
        let real_input = make_candidate(real_key, 5_000, current_height);

        // Only wealthy candidates available (no in-band background decoys).
        let candidates: Vec<OutputCandidate> = (0..8u8)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i;
                make_candidate(key, 4_800 + i as u64 * 10, current_height)
                    .with_effective_factor(5_745)
            })
            .collect();

        let mut rng = rand::rng();
        let exclude = vec![real_key];
        let decoys = selector
            .select_decoys_cluster_aware(&candidates, 6, &real_input, &exclude, &mut rng)
            .expect("liveness fallback selects out-of-band decoys rather than failing");
        assert_eq!(decoys.len(), 6);
    }
}

/// Issue #1771: tie-breaking in `select_decoys_for_input` must not depend on
/// candidate order (which production callers derive from DB key order,
/// `tx_hash || index`). An attacker who grinds output hashes so its outputs
/// sort first must gain no capture advantage among equal-distance candidates.
#[cfg(test)]
mod tie_break_tests {
    use super::*;
    use bth_transaction_types::ClusterTagVector;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    const HEIGHT: u64 = 1_000_000;
    /// Marker byte identifying attacker-owned outputs in `target_key[31]`.
    const ATTACKER: u8 = 0xAA;

    fn candidate(id: u16, age_blocks: u64, attacker: bool) -> OutputCandidate {
        let mut target_key = [0u8; 32];
        target_key[..2].copy_from_slice(&id.to_le_bytes());
        if attacker {
            target_key[31] = ATTACKER;
        }
        OutputCandidate {
            output: TxOutput {
                amount: 1000,
                target_key,
                public_key: [0u8; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            },
            created_at: HEIGHT - age_blocks,
            age_blocks,
            cluster_tags: ClusterTags::empty(),
            effective_factor: FACTOR_SCALE,
        }
    }

    fn id_of(o: &TxOutput) -> usize {
        u16::from_le_bytes([o.target_key[0], o.target_key[1]]) as usize
    }

    fn is_attacker(o: &TxOutput) -> bool {
        o.target_key[31] == ATTACKER
    }

    /// `n` equal-age candidates; the first `attackers` ids are attacker-owned.
    fn equal_age_pool(n: u16, attackers: u16, age: u64) -> Vec<OutputCandidate> {
        (0..n).map(|i| candidate(i, age, i < attackers)).collect()
    }

    /// The candidate orderings an adversary might induce (or that DB key order
    /// happens to produce): as built, reversed, ground-first (attacker outputs
    /// sort first, the #1771 attack), ground-last, and interleaved.
    fn orderings(pool: &[OutputCandidate]) -> Vec<(&'static str, Vec<OutputCandidate>)> {
        let original = pool.to_vec();
        let mut reversed = pool.to_vec();
        reversed.reverse();
        let mut ground_first = pool.to_vec();
        ground_first.sort_by_key(|c| c.output.target_key[31] != ATTACKER);
        let mut ground_last = pool.to_vec();
        ground_last.sort_by_key(|c| c.output.target_key[31] == ATTACKER);
        let (evens, odds): (Vec<_>, Vec<_>) = pool
            .iter()
            .cloned()
            .enumerate()
            .partition(|(i, _)| i % 2 == 0);
        let interleaved = odds.into_iter().chain(evens).map(|(_, c)| c).collect();
        vec![
            ("original", original),
            ("reversed", reversed),
            ("ground_first", ground_first),
            ("ground_last", ground_last),
            ("interleaved", interleaved),
        ]
    }

    /// Per-candidate selection frequency over `trials` seeded draws.
    fn frequencies(
        pool: &[OutputCandidate],
        n: usize,
        count: usize,
        real_age: u64,
        trials: usize,
        seed: u64,
    ) -> Vec<f64> {
        let selector = GammaDecoySelector::new();
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut hits = vec![0usize; n];
        for _ in 0..trials {
            let picked = selector
                .select_decoys_for_input(pool, count, &[], real_age, &mut rng)
                .expect("selection succeeds");
            assert_eq!(picked.len(), count);
            for o in &picked {
                hits[id_of(o)] += 1;
            }
        }
        hits.into_iter().map(|h| h as f64 / trials as f64).collect()
    }

    /// AC: permuting, reversing, or "grinding" candidate order leaves the
    /// selection distribution unchanged, and every equal-age candidate is
    /// selected with probability `count / n` (phase 1, in-band ties).
    #[test]
    fn phase1_ties_are_uniform_and_order_invariant() {
        let (n, attackers, count, age) = (40u16, 10u16, 19usize, 5_000u64);
        let pool = equal_age_pool(n, attackers, age);
        let expected = count as f64 / n as f64; // 0.475
        let trials = 4_000;
        // sd ~= sqrt(0.475 * 0.525 / 4000) ~= 0.0079; 0.05 ~= 6 sd.
        let tol = 0.05;
        let mut per_order = Vec::new();
        for (k, (name, ordered)) in orderings(&pool).into_iter().enumerate() {
            let f = frequencies(&ordered, n as usize, count, age, trials, 0x1771 + k as u64);
            for (id, p) in f.iter().enumerate() {
                assert!(
                    (p - expected).abs() < tol,
                    "{name}: candidate {id} freq {p:.3} vs expected {expected:.3}"
                );
            }
            per_order.push((name, f));
        }
        // Cross-ordering invariance: the same candidate's frequency agrees
        // across every ordering (difference of two ~N(p, 0.0079) estimates).
        let (_, base) = &per_order[0];
        for (name, f) in &per_order[1..] {
            for id in 0..n as usize {
                assert!(
                    (f[id] - base[id]).abs() < 2.0 * tol,
                    "{name}: candidate {id} freq {:.3} differs from original {:.3}",
                    f[id],
                    base[id]
                );
            }
        }
    }

    /// Exact probability that a uniformly random `k`-subset of `n` items, `m`
    /// of them attacker-owned, is entirely attacker-owned (hypergeometric).
    fn p_all_attacker(n: u64, m: u64, k: u64) -> f64 {
        (0..k).map(|i| (m - i) as f64 / (n - i) as f64).product()
    }

    /// AC: with attacker outputs ground to sort first, P(all decoys
    /// attacker-owned) matches the uniform-subset probability (~f^k), not 1.0
    /// as it would under first-minimum / stable-sort tie-breaking.
    #[test]
    fn ground_first_attacker_gains_no_capture_advantage_phase1() {
        let selector = GammaDecoySelector::new();
        let (n, m, k, age) = (20u16, 16u16, 4usize, 5_000u64);
        let pool = equal_age_pool(n, m, age);
        let mut ground_first = pool.clone();
        ground_first.sort_by_key(|c| c.output.target_key[31] != ATTACKER);

        let expected = p_all_attacker(n as u64, m as u64, k as u64); // ~0.3757
        let trials = 4_000;
        let mut rng = ChaCha8Rng::seed_from_u64(0x1771_0001);
        let mut all_attacker = 0usize;
        let mut attacker_slots = 0usize;
        for _ in 0..trials {
            let picked = selector
                .select_decoys_for_input(&ground_first, k, &[], age, &mut rng)
                .unwrap();
            let a = picked.iter().filter(|o| is_attacker(o)).count();
            attacker_slots += a;
            if a == k {
                all_attacker += 1;
            }
        }
        let observed = all_attacker as f64 / trials as f64;
        // sd ~= sqrt(0.376 * 0.624 / 4000) ~= 0.0077.
        assert!(
            (observed - expected).abs() < 0.04,
            "P(all attacker) observed {observed:.3}, expected {expected:.3} (pre-fix: 1.0)"
        );
        let share = attacker_slots as f64 / (trials * k) as f64;
        let f = m as f64 / n as f64;
        assert!(
            (share - f).abs() < 0.03,
            "attacker slot share {share:.3} vs pool fraction {f:.3}"
        );
    }

    /// Large-pool version of the #1771 grind at ring size 20 (19 decoys):
    /// half the equal-age pool is attacker-owned and ground to sort first.
    /// Uniform tie-breaking makes a fully captured ring ~0.5^19 (≈2e-6), so
    /// none should appear; pre-fix every ring was fully captured.
    #[test]
    fn ground_first_attacker_cannot_fill_ring_size_20() {
        let selector = GammaDecoySelector::new();
        let (n, m, k, age) = (400u16, 200u16, 19usize, 5_000u64);
        let mut pool = equal_age_pool(n, m, age);
        pool.sort_by_key(|c| c.output.target_key[31] != ATTACKER);
        let mut rng = ChaCha8Rng::seed_from_u64(0x1771_0002);
        let trials = 1_000;
        let mut full_capture = 0usize;
        let mut attacker_slots = 0usize;
        for _ in 0..trials {
            let picked = selector
                .select_decoys_for_input(&pool, k, &[], age, &mut rng)
                .unwrap();
            let a = picked.iter().filter(|o| is_attacker(o)).count();
            attacker_slots += a;
            if a == k {
                full_capture += 1;
            }
        }
        assert_eq!(full_capture, 0, "ground-first attacker filled a ring");
        let share = attacker_slots as f64 / (trials * k) as f64;
        assert!((share - 0.5).abs() < 0.03, "attacker slot share {share:.3}");
    }

    /// Phase 2 (liveness fallback) ties: a real input younger than the
    /// confirmation depth makes the band degenerate, so every slot comes from
    /// the nearest-age fallback. Equal-distance candidates must still be a
    /// uniform draw regardless of order — including candidates equidistant on
    /// opposite sides of the real age.
    #[test]
    fn phase2_ties_are_uniform_and_order_invariant() {
        let (n, attackers, count) = (24u16, 8u16, 6usize);
        let real_age = 5u64; // < MIN_DECOY_AGE_BLOCKS: degenerate band
        let (lo, hi) = age_similarity_band(real_age);
        assert!(lo > hi, "test requires a degenerate band");
        // All candidates at distance 15 from the real age.
        let pool: Vec<OutputCandidate> = (0..n).map(|i| candidate(i, 20, i < attackers)).collect();
        let expected = count as f64 / n as f64; // 0.25
        let trials = 4_000;
        let tol = 0.04; // sd ~= 0.0068
        for (k, (name, ordered)) in orderings(&pool).into_iter().enumerate() {
            let f = frequencies(
                &ordered,
                n as usize,
                count,
                real_age,
                trials,
                0x2771 + k as u64,
            );
            for (id, p) in f.iter().enumerate() {
                assert!(
                    (p - expected).abs() < tol,
                    "{name}: candidate {id} freq {p:.3} vs expected {expected:.3}"
                );
            }
        }
    }

    /// Phase 2 when the band is non-degenerate but too sparse: in-band slots
    /// are filled first, and the equal-distance out-of-band remainder is still
    /// drawn uniformly, so a ground-first attacker does not capture it.
    #[test]
    fn phase2_sparse_band_remainder_is_uniform() {
        let selector = GammaDecoySelector::new();
        let real_age = 10_000u64;
        // 2 honest in-band candidates; 30 out-of-band candidates all at
        // distance 5_000 (15 attacker, ground to sort first).
        let mut pool = vec![candidate(0, 10_000, false), candidate(1, 10_050, false)];
        for i in 0..30u16 {
            let age = if i % 2 == 0 { 5_000 } else { 15_000 };
            pool.push(candidate(2 + i, age, i < 15));
        }
        pool.sort_by_key(|c| c.output.target_key[31] != ATTACKER);
        let count = 6usize;
        let mut rng = ChaCha8Rng::seed_from_u64(0x1771_0003);
        let trials = 4_000;
        let mut attacker_slots = 0usize;
        for _ in 0..trials {
            let picked = selector
                .select_decoys_for_input(&pool, count, &[], real_age, &mut rng)
                .unwrap();
            assert!(picked.iter().any(|o| id_of(o) == 0) && picked.iter().any(|o| id_of(o) == 1));
            attacker_slots += picked.iter().filter(|o| is_attacker(o)).count();
        }
        // 4 fallback slots per trial drawn from 30 candidates, 15 attacker.
        let share = attacker_slots as f64 / (trials * (count - 2)) as f64;
        assert!(
            (share - 0.5).abs() < 0.03,
            "fallback attacker share {share:.3} (pre-fix: 1.0)"
        );
    }

    /// Tie randomization never trades away age-similarity: when an exact-age
    /// candidate exists alongside farther ones, the distance ordering still
    /// governs; only equal-distance choices are randomized.
    #[test]
    fn shuffle_does_not_override_distance() {
        let selector = GammaDecoySelector::new();
        let real_age = 5u64; // degenerate band -> pure nearest-age fallback
        let mut pool: Vec<OutputCandidate> = (0..10u16)
            .map(|i| candidate(i, 100 + i as u64 * 10, i % 2 == 0))
            .collect();
        pool.reverse();
        let mut rng = ChaCha8Rng::seed_from_u64(0x1771_0004);
        for _ in 0..200 {
            let picked = selector
                .select_decoys_for_input(&pool, 3, &[], real_age, &mut rng)
                .unwrap();
            let mut ids: Vec<usize> = picked.iter().map(id_of).collect();
            ids.sort_unstable();
            assert_eq!(ids, vec![0, 1, 2]);
        }
    }
}
