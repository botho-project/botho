// Copyright (c) 2024 Botho Foundation

//! OSPEAD-Style Decoy Selection for Ring Signatures
//!
//! This module implements Optimal Selection Probability to Evade Analysis of Decoys (OSPEAD),
//! which matches decoy age distribution to actual spending patterns using a gamma distribution.
//!
//! Key concepts:
//! - **Spend Distribution**: Models how quickly outputs are typically spent after creation
//! - **Gamma Distribution**: Real spending follows a gamma distribution - most outputs are
//!   spent relatively quickly, with a long tail of outputs held for longer periods
//! - **Age-Weighted Selection**: Decoys are selected to match the expected spend distribution,
//!   making the real input indistinguishable from decoys
//!
//! Target: Achieve 1-in-4+ effective anonymity with ring size 7, meaning at least 2 ring
//! members should be equally likely to be the real spend based on age patterns.

use rand::Rng;
use rand_distr::{Distribution, Gamma};
use std::collections::VecDeque;

use crate::transaction::{TxOutput, Utxo};

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

/// An output candidate for decoy selection with age information.
#[derive(Debug, Clone)]
pub struct OutputCandidate {
    /// The output itself
    pub output: TxOutput,
    /// Block height when the output was created
    pub created_at: u64,
    /// Age in blocks (current_height - created_at)
    pub age_blocks: u64,
}

impl OutputCandidate {
    /// Create a new output candidate from a UTXO at the given height.
    pub fn from_utxo(utxo: &Utxo, current_height: u64) -> Self {
        let age_blocks = current_height.saturating_sub(utxo.created_at);
        Self {
            output: utxo.output.clone(),
            created_at: utxo.created_at,
            age_blocks,
        }
    }

    /// Age in days (approximate, assuming 2-minute blocks).
    pub fn age_days(&self) -> f64 {
        self.age_blocks as f64 / BLOCKS_PER_DAY
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
        if self.spend_ages.len() >= 100 && self.spend_ages.len() % 100 == 0 {
            self.update_parameters();
        }
    }

    /// Update gamma distribution parameters from observations using method of moments.
    fn update_parameters(&mut self) {
        if self.spend_ages.len() < 100 {
            return;
        }

        // Calculate mean and variance
        let n = self.spend_ages.len() as f64;
        let mean: f64 = self.spend_ages.iter().map(|&x| x as f64).sum::<f64>() / n;
        let variance: f64 = self.spend_ages
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

    /// Calculate the probability density at a given age (for debugging/analysis).
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
    // Using Lanczos approximation would be more accurate, but Stirling is sufficient
    // for our purposes and simpler
    if x <= 0.0 {
        return f64::INFINITY;
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x - 0.5) * x.ln() - x
        + 1.0 / (12.0 * x)
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
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                    && !exclude_keys.contains(&c.output.target_key)
            })
            .collect();

        if eligible.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: eligible.len(),
            });
        }

        // Validate gamma distribution parameters
        let _ = Gamma::new(self.distribution.gamma_shape, self.distribution.gamma_scale_blocks)
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

    /// Select decoys with a target age for the real input.
    ///
    /// This version samples decoy ages that, together with the real input age,
    /// form a plausible ring from the observer's perspective.
    pub fn select_decoys_for_input<R: Rng>(
        &self,
        candidates: &[OutputCandidate],
        count: usize,
        exclude_keys: &[[u8; 32]],
        _real_input_age: u64, // Reserved for future use in age-aware sampling
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, DecoySelectionError> {
        // Filter eligible candidates
        let eligible: Vec<&OutputCandidate> = candidates
            .iter()
            .filter(|c| {
                c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                    && !exclude_keys.contains(&c.output.target_key)
            })
            .collect();

        if eligible.len() < count {
            return Err(DecoySelectionError::InsufficientCandidates {
                required: count,
                available: eligible.len(),
            });
        }

        let gamma = Gamma::new(self.distribution.gamma_shape, self.distribution.gamma_scale_blocks)
            .map_err(|_| DecoySelectionError::InvalidDistribution)?;

        // For each decoy slot, sample a target age and find best matching candidate
        let mut selected = Vec::with_capacity(count);
        let mut used_keys = exclude_keys.to_vec();

        for _ in 0..count {
            // Sample target age from gamma distribution
            let target_age: f64 = gamma.sample(rng);
            let target_age_blocks = (target_age as u64).clamp(MIN_DECOY_AGE_BLOCKS, MAX_DECOY_AGE_BLOCKS);

            // Find best matching candidate not yet used
            let best = eligible
                .iter()
                .filter(|c| !used_keys.contains(&c.output.target_key))
                .min_by_key(|c| {
                    let diff = (c.age_blocks as i64 - target_age_blocks as i64).abs();
                    diff as u64
                });

            if let Some(candidate) = best {
                selected.push(candidate.output.clone());
                used_keys.push(candidate.output.target_key);
            } else {
                break;
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
    /// This estimates how many ring members appear equally likely to be the real spend
    /// based on age distribution. A higher number is better.
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
            Self::InsufficientCandidates { required, available } => {
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

    fn make_candidate(target_key: [u8; 32], age_blocks: u64, current_height: u64) -> OutputCandidate {
        OutputCandidate {
            output: TxOutput {
                amount: 1000,
                target_key,
                public_key: [0u8; 32],
                e_memo: None,
            },
            created_at: current_height.saturating_sub(age_blocks),
            age_blocks,
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
        println!("Similar ages anonymity: {:.2}", similar_anon);
        println!("Diverse ages anonymity: {:.2}", diverse_anon);
    }

    #[test]
    fn test_gamma_weighting_prefers_realistic_ages() {
        let selector = GammaDecoySelector::new();

        // With default parameters (mean ~30 days = ~21600 blocks),
        // ages around that range should have higher weight than extremes
        let weight_young = selector.weight_for_age(100);    // 3 hours
        let weight_medium = selector.weight_for_age(21600); // 30 days
        let weight_old = selector.weight_for_age(525600);   // 2 years

        println!("Weight at 100 blocks: {}", weight_young);
        println!("Weight at 21600 blocks: {}", weight_medium);
        println!("Weight at 525600 blocks: {}", weight_old);

        // Medium-aged outputs should be preferred
        assert!(weight_medium > weight_young);
        assert!(weight_medium > weight_old);
    }
}
