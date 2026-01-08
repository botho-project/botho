//! Entropy-Weighted Decay: Defense against patient wash trading attacks.
//!
//! This module implements decay that only applies when cluster entropy actually
//! changes, indicating genuine commerce rather than self-transfers.
//!
//! # Key Insight
//!
//! Patient wash trading exploits age-based decay by waiting between each
//! self-transfer. While each individual transfer is rate-limited, the attacker
//! eventually achieves full decay without any real commerce.
//!
//! Entropy-weighted decay solves this by only allowing decay when the receiver's
//! cluster entropy increases. Self-transfers don't change entropy, so no decay
//! applies regardless of timing.
//!
//! # Properties
//!
//! - **Wash trading (rapid or patient)**: NO decay (entropy unchanged)
//! - **Sybil wash trading**: NO decay (creating fake counterparties doesn't add
//!   entropy)
//! - **Legitimate commerce**: Normal decay (entropy increases from mixing
//!   sources)
//! - **Privacy preserved**: Uses existing cluster_entropy() calculation
//!
//! # Integration with Age-Based Decay
//!
//! Entropy-weighted decay can be used as a gating condition on top of age-based
//! decay: - Age check: UTXO must be >= min_age_blocks old - Entropy check: Must
//!   cause entropy delta >= min_entropy_delta - Decay rate: 5% × scaling_factor
//!   (where scaling_factor depends on entropy delta)

use crate::age_decay::AgeDecayConfig;
use crate::tag::{TagVector, TagWeight, TAG_WEIGHT_SCALE};

/// Configuration for entropy-weighted decay.
#[derive(Clone, Debug)]
pub struct EntropyDecayConfig {
    /// Minimum entropy delta required to trigger decay.
    /// Value in bits (e.g., 0.1 means 0.1 bits of entropy increase).
    pub min_entropy_delta: f64,

    /// Base decay rate (applied when entropy condition is met).
    /// Uses same scale as TagWeight (e.g., 50_000 = 5%).
    pub base_decay_rate: TagWeight,

    /// How entropy delta scales the decay rate.
    pub decay_scaling: EntropyScaling,

    /// Optional: Combine with age-based gating.
    pub age_config: Option<AgeDecayConfig>,
}

impl Default for EntropyDecayConfig {
    fn default() -> Self {
        Self {
            min_entropy_delta: 0.1, // Require at least 0.1 bits of entropy increase
            base_decay_rate: 50_000, // 5% base decay
            decay_scaling: EntropyScaling::Linear,
            age_config: Some(AgeDecayConfig::default()),
        }
    }
}

impl EntropyDecayConfig {
    /// Create a config with custom entropy threshold.
    pub fn with_threshold(min_entropy_delta: f64) -> Self {
        Self {
            min_entropy_delta,
            ..Default::default()
        }
    }

    /// Create a config without age gating (pure entropy-based).
    pub fn entropy_only() -> Self {
        Self {
            age_config: None,
            ..Default::default()
        }
    }

    /// Create a config optimized for wash trading resistance.
    pub fn anti_wash_trading() -> Self {
        Self {
            min_entropy_delta: 0.05, // Low threshold, but still requires real commerce
            base_decay_rate: 50_000,
            decay_scaling: EntropyScaling::Sqrt,
            age_config: Some(AgeDecayConfig::default()),
        }
    }
}

/// How entropy delta scales the decay rate.
#[derive(Clone, Debug, Default, Copy, PartialEq)]
pub enum EntropyScaling {
    /// Full decay when min_entropy_delta is met.
    #[default]
    Linear,

    /// Decay proportional to sqrt(entropy_delta).
    /// More gradual, rewards larger entropy increases less.
    Sqrt,

    /// Decay proportional to log(1 + entropy_delta).
    /// Even more gradual, good for large entropy ranges.
    Log,

    /// Binary: full decay if threshold met, zero otherwise.
    Binary,
}

impl EntropyScaling {
    /// Calculate scaling factor for given entropy delta.
    /// Returns a value between 0.0 and 1.0.
    pub fn factor(&self, entropy_delta: f64, min_delta: f64) -> f64 {
        if entropy_delta < min_delta {
            return 0.0;
        }

        match self {
            EntropyScaling::Binary => 1.0,
            EntropyScaling::Linear => (entropy_delta / min_delta).min(2.0) / 2.0,
            EntropyScaling::Sqrt => ((entropy_delta / min_delta).sqrt()).min(2.0) / 2.0,
            EntropyScaling::Log => ((1.0 + entropy_delta / min_delta).ln()).min(2.0) / 2.0,
        }
    }
}

/// Result of attempting entropy-weighted decay.
#[derive(Clone, Debug)]
pub struct EntropyDecayResult {
    /// Whether decay was applied.
    pub decay_applied: bool,

    /// Entropy before the transfer.
    pub entropy_before: f64,

    /// Entropy after the transfer (if decay was applied).
    pub entropy_after: f64,

    /// The entropy delta that triggered decay.
    pub entropy_delta: f64,

    /// Scaling factor applied to decay rate.
    pub scaling_factor: f64,

    /// Actual decay rate applied (0 if no decay).
    pub effective_decay_rate: TagWeight,

    /// Reason decay was blocked (if applicable).
    pub block_reason: Option<DecayBlockReason>,
}

/// Reasons why decay might be blocked.
#[derive(Clone, Debug, PartialEq)]
pub enum DecayBlockReason {
    /// UTXO too young (age check failed).
    UtxoTooYoung {
        utxo_age_blocks: u64,
        required_age: u64,
    },
    /// Entropy increase too small.
    InsufficientEntropy { delta: f64, required: f64 },
    /// No cluster tags to decay (fully background).
    FullyDecayed,
}

/// Calculate entropy-weighted decay for a transfer.
///
/// This is the core function that determines whether decay should apply
/// based on entropy changes from mixing incoming funds.
pub fn calculate_entropy_decay(
    receiver_tags_before: &TagVector,
    receiver_balance: u64,
    incoming_tags: &TagVector,
    incoming_amount: u64,
    config: &EntropyDecayConfig,
) -> EntropyDecayResult {
    // Calculate entropy before the mix
    let entropy_before = receiver_tags_before.cluster_entropy();

    // Simulate the mix to calculate entropy after
    let mut mixed_tags = receiver_tags_before.clone();
    mixed_tags.mix(receiver_balance, incoming_tags, incoming_amount);
    let entropy_after = mixed_tags.cluster_entropy();

    // Calculate entropy delta
    let entropy_delta = entropy_after - entropy_before;

    // Check if entropy threshold is met
    if entropy_delta < config.min_entropy_delta {
        return EntropyDecayResult {
            decay_applied: false,
            entropy_before,
            entropy_after,
            entropy_delta,
            scaling_factor: 0.0,
            effective_decay_rate: 0,
            block_reason: Some(DecayBlockReason::InsufficientEntropy {
                delta: entropy_delta,
                required: config.min_entropy_delta,
            }),
        };
    }

    // Check if there are any cluster tags to decay
    if receiver_tags_before.total_attributed() == 0 && incoming_tags.total_attributed() == 0 {
        return EntropyDecayResult {
            decay_applied: false,
            entropy_before,
            entropy_after,
            entropy_delta,
            scaling_factor: 0.0,
            effective_decay_rate: 0,
            block_reason: Some(DecayBlockReason::FullyDecayed),
        };
    }

    // Calculate scaling factor based on entropy delta
    let scaling_factor = config
        .decay_scaling
        .factor(entropy_delta, config.min_entropy_delta);

    // Calculate effective decay rate
    let effective_decay_rate =
        (config.base_decay_rate as f64 * scaling_factor).round() as TagWeight;

    EntropyDecayResult {
        decay_applied: true,
        entropy_before,
        entropy_after,
        entropy_delta,
        scaling_factor,
        effective_decay_rate,
        block_reason: None,
    }
}

/// Apply entropy-weighted decay with optional age gating.
///
/// Returns the decay result and applies the decay to transferred tags
/// if all conditions are met.
pub fn apply_entropy_decay(
    transferred_tags: &mut TagVector,
    receiver_tags_before: &TagVector,
    receiver_balance: u64,
    incoming_amount: u64,
    utxo_creation_block: Option<u64>,
    current_block: Option<u64>,
    config: &EntropyDecayConfig,
) -> EntropyDecayResult {
    // First check age requirement if configured
    if let (Some(age_config), Some(creation_block), Some(current)) =
        (&config.age_config, utxo_creation_block, current_block)
    {
        let utxo_age = current.saturating_sub(creation_block);
        if utxo_age < age_config.min_age_blocks {
            return EntropyDecayResult {
                decay_applied: false,
                entropy_before: receiver_tags_before.cluster_entropy(),
                entropy_after: receiver_tags_before.cluster_entropy(),
                entropy_delta: 0.0,
                scaling_factor: 0.0,
                effective_decay_rate: 0,
                block_reason: Some(DecayBlockReason::UtxoTooYoung {
                    utxo_age_blocks: utxo_age,
                    required_age: age_config.min_age_blocks,
                }),
            };
        }
    }

    // Calculate entropy-based decay
    let result = calculate_entropy_decay(
        receiver_tags_before,
        receiver_balance,
        transferred_tags,
        incoming_amount,
        config,
    );

    // Apply decay if conditions met
    if result.decay_applied {
        transferred_tags.apply_decay(result.effective_decay_rate);
    }

    result
}

// ============================================================================
// Attack Strategies for Simulation
// ============================================================================

/// Attack strategies for testing decay resistance.
#[derive(Clone, Debug)]
pub enum AttackStrategy {
    /// Rapid self-transfers without waiting.
    RapidWash {
        /// Number of transfers to execute.
        transfers: u32,
    },

    /// Patient wash trading: wait between each transfer.
    PatientWash {
        /// Blocks to wait between transfers.
        interval_blocks: u64,
        /// Total duration in blocks.
        duration_blocks: u64,
    },

    /// Create fake counterparty addresses for wash trading.
    SybilWash {
        /// Number of fake counterparties to create.
        fake_counterparties: u32,
        /// Transfers per counterparty.
        transfers_per_counterparty: u32,
    },

    /// Mix real commerce with wash trading.
    PartialCommerce {
        /// Fraction of legitimate transactions (0.0 to 1.0).
        legit_ratio: f64,
        /// Total transactions.
        total_transactions: u32,
    },
}

/// Result of simulating an attack strategy.
#[derive(Clone, Debug)]
pub struct AttackResult {
    /// Strategy that was executed.
    pub strategy: String,

    /// Initial cluster tag weight.
    pub initial_tag: TagWeight,

    /// Final cluster tag weight.
    pub final_tag: TagWeight,

    /// Tag remaining as fraction (0.0 to 1.0).
    pub tag_remaining_fraction: f64,

    /// Initial cluster entropy.
    pub initial_entropy: f64,

    /// Final cluster entropy.
    pub final_entropy: f64,

    /// Number of decay events that occurred.
    pub decay_events: u32,

    /// Total decay attempts.
    pub total_attempts: u32,

    /// Effective decay rate per block.
    pub effective_decay_rate_per_block: f64,

    /// Time elapsed in blocks.
    pub blocks_elapsed: u64,
}

/// Simulated UTXO for attack testing.
#[derive(Clone, Debug)]
pub struct SimUtxo {
    /// UTXO value.
    pub value: u64,
    /// Tag vector.
    pub tags: TagVector,
    /// Block when UTXO was created.
    pub creation_block: u64,
    /// Entropy history for tracking.
    pub entropy_history: Vec<f64>,
}

impl SimUtxo {
    /// Create a new simulated UTXO.
    pub fn new(value: u64, cluster_id: crate::ClusterId, creation_block: u64) -> Self {
        Self {
            value,
            tags: TagVector::single(cluster_id),
            creation_block,
            entropy_history: vec![0.0], // Fresh mint has 0 entropy
        }
    }

    /// Record current entropy in history.
    pub fn record_entropy(&mut self) {
        self.entropy_history.push(self.tags.cluster_entropy());
    }
}

/// Decay mode for comparison simulations.
#[derive(Clone, Debug, Copy, PartialEq)]
pub enum DecayMode {
    /// Age-based decay (current implementation).
    AgeBased,
    /// Entropy-weighted decay (proposed).
    EntropyWeighted,
    /// No decay (control group).
    None,
}

/// Compare decay modes under a given attack strategy.
pub fn compare_decay_modes(
    strategy: &AttackStrategy,
    initial_wealth: u64,
    initial_factor: f64,
    duration_blocks: u64,
) -> Vec<(DecayMode, AttackResult)> {
    let cluster_id = crate::ClusterId::new(1);
    let initial_tag = (initial_factor / 6.0 * TAG_WEIGHT_SCALE as f64) as TagWeight;

    vec![
        (
            DecayMode::AgeBased,
            simulate_attack_age_based(
                strategy,
                cluster_id,
                initial_wealth,
                initial_tag,
                duration_blocks,
            ),
        ),
        (
            DecayMode::EntropyWeighted,
            simulate_attack_entropy_weighted(
                strategy,
                cluster_id,
                initial_wealth,
                initial_tag,
                duration_blocks,
            ),
        ),
    ]
}

/// Simulate attack with age-based decay.
fn simulate_attack_age_based(
    strategy: &AttackStrategy,
    cluster_id: crate::ClusterId,
    _initial_wealth: u64,
    initial_tag: TagWeight,
    duration_blocks: u64,
) -> AttackResult {
    let age_config = AgeDecayConfig::default();
    let mut tags = TagVector::new();
    tags.set(cluster_id, initial_tag);
    let initial_entropy = tags.cluster_entropy();

    let mut decay_events = 0u32;
    let mut total_attempts = 0u32;
    let mut current_block = 0u64;
    let mut last_creation_block = 0u64;

    match strategy {
        AttackStrategy::RapidWash { transfers } => {
            for _ in 0..*transfers {
                total_attempts += 1;
                // Rapid transfers: each output is created 1 block ago
                current_block += 1;
                if age_config.is_eligible(last_creation_block, current_block) {
                    tags.apply_decay(age_config.decay_rate);
                    decay_events += 1;
                    last_creation_block = current_block;
                }
            }
        }
        AttackStrategy::PatientWash {
            interval_blocks,
            duration_blocks: attack_duration,
        } => {
            let max_transfers = attack_duration / interval_blocks;
            for _ in 0..max_transfers {
                total_attempts += 1;
                current_block += interval_blocks;
                if current_block > duration_blocks {
                    break;
                }
                if age_config.is_eligible(last_creation_block, current_block) {
                    tags.apply_decay(age_config.decay_rate);
                    decay_events += 1;
                    last_creation_block = current_block;
                }
            }
        }
        AttackStrategy::SybilWash {
            fake_counterparties,
            transfers_per_counterparty,
        } => {
            // Sybil attack: create fake addresses, but it's still self-transfer
            // from the chain's perspective (same cluster tags)
            let total_transfers = fake_counterparties * transfers_per_counterparty;
            let interval = duration_blocks / total_transfers as u64;
            for _ in 0..total_transfers {
                total_attempts += 1;
                current_block += interval;
                if age_config.is_eligible(last_creation_block, current_block) {
                    tags.apply_decay(age_config.decay_rate);
                    decay_events += 1;
                    last_creation_block = current_block;
                }
            }
        }
        AttackStrategy::PartialCommerce {
            legit_ratio,
            total_transactions,
        } => {
            let interval = duration_blocks / *total_transactions as u64;
            let legit_count = (*total_transactions as f64 * legit_ratio) as u32;
            for i in 0..*total_transactions {
                total_attempts += 1;
                current_block += interval;
                let is_legit = i < legit_count;

                if is_legit {
                    // Legitimate commerce always triggers decay
                    tags.apply_decay(age_config.decay_rate);
                    decay_events += 1;
                    last_creation_block = current_block;
                } else if age_config.is_eligible(last_creation_block, current_block) {
                    // Wash trading only if age allows
                    tags.apply_decay(age_config.decay_rate);
                    decay_events += 1;
                    last_creation_block = current_block;
                }
            }
        }
    }

    let final_tag = tags.get(cluster_id);
    let final_entropy = tags.cluster_entropy();
    let blocks_elapsed = current_block.max(duration_blocks);

    AttackResult {
        strategy: format!("{strategy:?}"),
        initial_tag,
        final_tag,
        tag_remaining_fraction: final_tag as f64 / initial_tag as f64,
        initial_entropy,
        final_entropy,
        decay_events,
        total_attempts,
        effective_decay_rate_per_block: if blocks_elapsed > 0 {
            1.0 - (final_tag as f64 / initial_tag as f64).powf(1.0 / blocks_elapsed as f64)
        } else {
            0.0
        },
        blocks_elapsed,
    }
}

/// Simulate attack with entropy-weighted decay.
fn simulate_attack_entropy_weighted(
    strategy: &AttackStrategy,
    cluster_id: crate::ClusterId,
    initial_wealth: u64,
    initial_tag: TagWeight,
    duration_blocks: u64,
) -> AttackResult {
    let config = EntropyDecayConfig::anti_wash_trading();
    let mut tags = TagVector::new();
    tags.set(cluster_id, initial_tag);
    let initial_entropy = tags.cluster_entropy();

    let mut decay_events = 0u32;
    let mut total_attempts = 0u32;
    let mut current_block = 0u64;

    match strategy {
        AttackStrategy::RapidWash { transfers } => {
            // Rapid wash: no entropy change on self-transfers
            for _ in 0..*transfers {
                total_attempts += 1;
                current_block += 1;

                // Self-transfer: incoming tags are the same as existing
                let result = calculate_entropy_decay(
                    &tags,
                    initial_wealth,
                    &tags.clone(),
                    initial_wealth / *transfers as u64,
                    &config,
                );

                if result.decay_applied {
                    tags.apply_decay(result.effective_decay_rate);
                    decay_events += 1;
                }
            }
        }
        AttackStrategy::PatientWash {
            interval_blocks,
            duration_blocks: attack_duration,
        } => {
            let max_transfers = attack_duration / interval_blocks;
            for _ in 0..max_transfers {
                total_attempts += 1;
                current_block += interval_blocks;
                if current_block > duration_blocks {
                    break;
                }

                // Patient wash: still self-transfer, no entropy change
                let result = calculate_entropy_decay(
                    &tags,
                    initial_wealth,
                    &tags.clone(),
                    initial_wealth,
                    &config,
                );

                if result.decay_applied {
                    tags.apply_decay(result.effective_decay_rate);
                    decay_events += 1;
                }
            }
        }
        AttackStrategy::SybilWash {
            fake_counterparties,
            transfers_per_counterparty,
        } => {
            // Sybil attack: fake counterparties still have same-origin tags
            // Creating a new address doesn't create new cluster entropy
            let total_transfers = fake_counterparties * transfers_per_counterparty;
            let interval = duration_blocks / total_transfers as u64;

            for _ in 0..total_transfers {
                total_attempts += 1;
                current_block += interval;

                // Fake counterparty has same tags (they received from attacker)
                let result = calculate_entropy_decay(
                    &tags,
                    initial_wealth,
                    &tags.clone(),
                    initial_wealth / total_transfers as u64,
                    &config,
                );

                if result.decay_applied {
                    tags.apply_decay(result.effective_decay_rate);
                    decay_events += 1;
                }
            }
        }
        AttackStrategy::PartialCommerce {
            legit_ratio,
            total_transactions,
        } => {
            let interval = duration_blocks / *total_transactions as u64;
            let legit_count = (*total_transactions as f64 * legit_ratio) as u32;

            // For legitimate commerce, we need a different cluster
            let other_cluster = crate::ClusterId::new(2);

            for i in 0..*total_transactions {
                total_attempts += 1;
                current_block += interval;
                let is_legit = i < legit_count;

                let incoming = if is_legit {
                    // Legitimate: incoming from different cluster
                    TagVector::single(other_cluster)
                } else {
                    // Wash: same tags
                    tags.clone()
                };

                let result = calculate_entropy_decay(
                    &tags,
                    initial_wealth,
                    &incoming,
                    initial_wealth / *total_transactions as u64,
                    &config,
                );

                if result.decay_applied {
                    tags.apply_decay(result.effective_decay_rate);
                    decay_events += 1;
                }
            }
        }
    }

    let final_tag = tags.get(cluster_id);
    let final_entropy = tags.cluster_entropy();
    let blocks_elapsed = current_block.max(duration_blocks);

    AttackResult {
        strategy: format!("{strategy:?}"),
        initial_tag,
        final_tag,
        tag_remaining_fraction: final_tag as f64 / initial_tag as f64,
        initial_entropy,
        final_entropy,
        decay_events,
        total_attempts,
        effective_decay_rate_per_block: if blocks_elapsed > 0 {
            1.0 - (final_tag as f64 / initial_tag as f64).powf(1.0 / blocks_elapsed as f64)
        } else {
            0.0
        },
        blocks_elapsed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;

    #[test]
    fn test_self_transfer_no_entropy_change() {
        let c1 = ClusterId::new(1);
        let tags = TagVector::single(c1);
        let config = EntropyDecayConfig::default();

        // Self-transfer: incoming tags same as receiver
        let result =
            calculate_entropy_decay(&tags, 1000, &tags, 1000, // Same tags
                                    &config);

        // No entropy change = no decay
        assert!(
            !result.decay_applied,
            "Self-transfer should not trigger decay"
        );
        assert_eq!(result.entropy_delta, 0.0, "Self-transfer has zero entropy delta");
    }

    #[test]
    fn test_commerce_increases_entropy() {
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let receiver_tags = TagVector::single(c1);
        let incoming_tags = TagVector::single(c2);
        let config = EntropyDecayConfig::default();

        // Commerce: incoming from different cluster
        let result = calculate_entropy_decay(
            &receiver_tags,
            1000,
            &incoming_tags,
            1000, // Different cluster
            &config,
        );

        // Entropy increases = decay applies
        assert!(result.decay_applied, "Commerce should trigger decay");
        assert!(
            result.entropy_delta > 0.0,
            "Commerce increases entropy: {}",
            result.entropy_delta
        );
        assert!(
            result.entropy_after > result.entropy_before,
            "Entropy should increase: {} -> {}",
            result.entropy_before,
            result.entropy_after
        );
    }

    #[test]
    fn test_patient_wash_blocked() {
        let c1 = ClusterId::new(1);
        let strategy = AttackStrategy::PatientWash {
            interval_blocks: 720, // Maximum patience
            duration_blocks: 60480, // 1 week
        };

        // With age-based decay: patient attacker succeeds
        let age_result = simulate_attack_age_based(
            &strategy,
            c1,
            100_000_000,
            TAG_WEIGHT_SCALE, // 100% initial tag
            60480,
        );

        // With entropy-weighted decay: patient attacker blocked
        let entropy_result = simulate_attack_entropy_weighted(
            &strategy,
            c1,
            100_000_000,
            TAG_WEIGHT_SCALE,
            60480,
        );

        // Age-based allows significant decay
        assert!(
            age_result.decay_events > 0,
            "Age-based should allow some decays"
        );
        assert!(
            age_result.tag_remaining_fraction < 0.5,
            "Age-based: significant decay ({}%)",
            age_result.tag_remaining_fraction * 100.0
        );

        // Entropy-weighted blocks all decay
        assert_eq!(
            entropy_result.decay_events, 0,
            "Entropy-weighted should block patient wash"
        );
        assert_eq!(
            entropy_result.tag_remaining_fraction, 1.0,
            "Entropy-weighted: no decay"
        );
    }

    #[test]
    fn test_sybil_wash_blocked() {
        let c1 = ClusterId::new(1);
        let strategy = AttackStrategy::SybilWash {
            fake_counterparties: 100,
            transfers_per_counterparty: 10,
        };

        let entropy_result = simulate_attack_entropy_weighted(
            &strategy,
            c1,
            100_000_000,
            TAG_WEIGHT_SCALE,
            60480,
        );

        // Sybil attack blocked: fake counterparties don't add entropy
        assert_eq!(entropy_result.decay_events, 0, "Sybil attack should be blocked");
        assert_eq!(
            entropy_result.tag_remaining_fraction, 1.0,
            "No decay for sybil attack"
        );
    }

    #[test]
    fn test_legitimate_commerce_allows_decay() {
        let c1 = ClusterId::new(1);
        let strategy = AttackStrategy::PartialCommerce {
            legit_ratio: 1.0, // 100% legitimate
            total_transactions: 100,
        };

        let entropy_result = simulate_attack_entropy_weighted(
            &strategy,
            c1,
            100_000_000,
            TAG_WEIGHT_SCALE,
            60480,
        );

        // Legitimate commerce allows decay
        assert!(
            entropy_result.decay_events > 0,
            "Legitimate commerce should trigger decay: {} events",
            entropy_result.decay_events
        );
        assert!(
            entropy_result.tag_remaining_fraction < 1.0,
            "Should have some decay: {}%",
            entropy_result.tag_remaining_fraction * 100.0
        );
    }

    #[test]
    fn test_partial_commerce_proportional_decay() {
        let c1 = ClusterId::new(1);

        // 50% legitimate, 50% wash
        let partial_strategy = AttackStrategy::PartialCommerce {
            legit_ratio: 0.5,
            total_transactions: 100,
        };

        // 100% legitimate
        let full_strategy = AttackStrategy::PartialCommerce {
            legit_ratio: 1.0,
            total_transactions: 100,
        };

        let partial_result = simulate_attack_entropy_weighted(
            &partial_strategy,
            c1,
            100_000_000,
            TAG_WEIGHT_SCALE,
            60480,
        );

        let full_result = simulate_attack_entropy_weighted(
            &full_strategy,
            c1,
            100_000_000,
            TAG_WEIGHT_SCALE,
            60480,
        );

        // More legitimate commerce = more decay
        assert!(
            partial_result.decay_events < full_result.decay_events,
            "Partial commerce should have fewer decay events: {} vs {}",
            partial_result.decay_events,
            full_result.decay_events
        );
    }

    #[test]
    fn test_scaling_modes() {
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let receiver_tags = TagVector::single(c1);
        let incoming_tags = TagVector::single(c2);

        // Test all scaling modes
        for scaling in [
            EntropyScaling::Binary,
            EntropyScaling::Linear,
            EntropyScaling::Sqrt,
            EntropyScaling::Log,
        ] {
            let config = EntropyDecayConfig {
                decay_scaling: scaling,
                ..Default::default()
            };

            let result = calculate_entropy_decay(&receiver_tags, 1000, &incoming_tags, 1000, &config);

            assert!(
                result.decay_applied,
                "Commerce should trigger decay with {:?} scaling",
                scaling
            );
            assert!(
                result.scaling_factor > 0.0,
                "Scaling factor should be positive for {:?}",
                scaling
            );
        }
    }

    #[test]
    fn test_age_gating_with_entropy() {
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let mut transferred_tags = TagVector::single(c1);
        let receiver_tags = TagVector::new();

        let config = EntropyDecayConfig::default(); // Has age_config with 720 block min

        // Young UTXO should be blocked even with entropy change
        let result = apply_entropy_decay(
            &mut transferred_tags,
            &receiver_tags,
            0,
            1000,
            Some(100),  // Created at block 100
            Some(500),  // Current block 500 (only 400 blocks old)
            &config,
        );

        assert!(
            !result.decay_applied,
            "Young UTXO should block decay even with entropy"
        );
        assert!(
            matches!(result.block_reason, Some(DecayBlockReason::UtxoTooYoung { .. })),
            "Should report UTXO too young"
        );

        // With commerce (different cluster), should allow decay
        let mut transferred_tags3 = TagVector::single(c1);
        let result3 = apply_entropy_decay(
            &mut transferred_tags3,
            &TagVector::single(c2), // Receiver has different cluster
            1000,
            1000,
            Some(100),
            Some(1000),
            &config,
        );

        assert!(
            result3.decay_applied,
            "Old UTXO with commerce should decay"
        );
    }
}
