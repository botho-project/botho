//! Block-aware tag decay for wash trading resistance.
//!
//! This module provides an alternative decay mechanism where cluster tags
//! decay based on block height rather than transaction count. This prevents
//! wash trading attacks where users accelerate decay through self-transfers.
//!
//! ## Design Rationale
//!
//! In the hop-based decay model:
//! - Tags decay by X% per transfer
//! - Whales can "wash trade" by making many self-transfers to accelerate decay
//! - At 5% decay, break-even after ~278 transactions
//! - At 10% decay, break-even after only ~27 transactions
//!
//! In the block-based decay model:
//! - Tags decay based on time (block height), not transaction count
//! - Self-transfers don't accelerate decay
//! - Wash trading provides no economic benefit
//! - Natural privacy still improves over time
//!
//! ## Half-Life Model
//!
//! We use a half-life decay model for intuitive parameterization:
//! - `half_life_blocks`: Number of blocks for tags to decay to 50%
//! - After 1 half-life: 50% remaining
//! - After 2 half-lives: 25% remaining
//! - After 3 half-lives: 12.5% remaining
//! - etc.
//!
//! Example settings (assuming 10-second blocks):
//! - 1 day half-life: ~8,640 blocks
//! - 1 week half-life: ~60,480 blocks
//! - 1 month half-life: ~262,800 blocks

use crate::{
    cluster::ClusterId,
    tag::{TagVector, TagWeight, TAG_WEIGHT_SCALE},
};
use std::collections::HashMap;

/// Configuration for block-aware decay.
#[derive(Clone, Debug)]
pub struct BlockDecayConfig {
    /// Number of blocks for tags to decay to 50%.
    /// Larger values = slower decay = longer cluster fingerprinting window.
    /// Smaller values = faster decay = better privacy but less taxation.
    pub half_life_blocks: u64,

    /// Minimum decay interval in blocks.
    /// Decay is only applied when this many blocks have passed.
    /// Prevents excessive computation on frequent queries.
    pub min_decay_interval: u64,

    /// Optional: also apply small hop-based decay for mixing incentive.
    /// Set to 0 to disable hop decay entirely.
    pub hop_decay_rate: TagWeight,
}

impl Default for BlockDecayConfig {
    fn default() -> Self {
        Self {
            // ~1 week half-life at 10s blocks
            half_life_blocks: 60_480,
            // Apply decay at least every 100 blocks
            min_decay_interval: 100,
            // No hop decay by default (block-only)
            hop_decay_rate: 0,
        }
    }
}

impl BlockDecayConfig {
    /// Create a config with specified half-life in days (assuming 10s blocks).
    pub fn with_half_life_days(days: u64) -> Self {
        Self {
            half_life_blocks: days * 8_640, // 86400 seconds / 10 seconds per block
            ..Default::default()
        }
    }

    /// Create a config optimized for anti-wash-trading.
    /// Uses long half-life (1 month) with no hop decay.
    pub fn anti_wash_trading() -> Self {
        Self {
            half_life_blocks: 262_800, // ~1 month
            min_decay_interval: 100,
            hop_decay_rate: 0,
        }
    }

    /// Create a hybrid config with both block and hop decay.
    /// Provides wash trading resistance while still incentivizing mixing.
    pub fn hybrid(half_life_blocks: u64, hop_decay_pct: f64) -> Self {
        Self {
            half_life_blocks,
            min_decay_interval: 100,
            hop_decay_rate: (hop_decay_pct * 10_000.0) as TagWeight, // Convert % to ppm/100
        }
    }

    /// Compute the decay factor for a given number of elapsed blocks.
    ///
    /// Returns a value in [0, TAG_WEIGHT_SCALE] representing the fraction remaining.
    /// e.g., 500_000 means 50% remains.
    pub fn decay_factor(&self, blocks_elapsed: u64) -> TagWeight {
        if self.half_life_blocks == 0 || blocks_elapsed == 0 {
            return TAG_WEIGHT_SCALE;
        }

        // Use the formula: remaining = 0.5^(blocks/half_life)
        // We compute this in fixed-point using repeated halving.

        // Number of complete half-lives
        let complete_half_lives = blocks_elapsed / self.half_life_blocks;

        // Remaining blocks after complete half-lives
        let remaining_blocks = blocks_elapsed % self.half_life_blocks;

        // Start with full weight
        let mut factor = TAG_WEIGHT_SCALE as u64;

        // Apply complete half-lives (each halves the factor)
        for _ in 0..complete_half_lives.min(20) {
            // Cap at 20 to prevent underflow
            factor /= 2;
        }

        // Apply partial half-life using linear interpolation
        // This is an approximation: we linearly interpolate between 1.0 and 0.5
        // for the fractional part of the half-life
        if remaining_blocks > 0 && self.half_life_blocks > 0 {
            // fraction of half-life elapsed (in parts per SCALE)
            let frac =
                (remaining_blocks as u128 * TAG_WEIGHT_SCALE as u128 / self.half_life_blocks as u128)
                    as u64;

            // Decay by (1 - 0.5 * frac/SCALE) = 1 - frac/2/SCALE
            // This linearly interpolates from 1.0 to 0.5 over one half-life
            let partial_decay = frac / 2;
            factor = factor * (TAG_WEIGHT_SCALE as u64 - partial_decay) / TAG_WEIGHT_SCALE as u64;
        }

        factor.min(TAG_WEIGHT_SCALE as u64) as TagWeight
    }
}

/// A tag vector with block-aware decay tracking.
#[derive(Clone, Debug)]
pub struct BlockAwareTagVector {
    /// The underlying tag weights.
    tags: HashMap<ClusterId, TagWeight>,

    /// Block height when these tags were last decayed.
    last_decay_block: u64,
}

impl Default for BlockAwareTagVector {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockAwareTagVector {
    /// Minimum tag weight to retain (same as TagVector).
    pub const PRUNE_THRESHOLD: TagWeight = 100;

    /// Maximum number of tags to track (same as TagVector).
    pub const MAX_TAGS: usize = 32;

    /// Create an empty tag vector.
    pub fn new() -> Self {
        Self {
            tags: HashMap::new(),
            last_decay_block: 0,
        }
    }

    /// Create a tag vector at a specific block height.
    pub fn at_block(block: u64) -> Self {
        Self {
            tags: HashMap::new(),
            last_decay_block: block,
        }
    }

    /// Create a tag vector fully attributed to one cluster.
    pub fn single(cluster: ClusterId, block: u64) -> Self {
        let mut tags = HashMap::new();
        tags.insert(cluster, TAG_WEIGHT_SCALE);
        Self {
            tags,
            last_decay_block: block,
        }
    }

    /// Convert from a regular TagVector.
    pub fn from_tag_vector(tags: &TagVector, block: u64) -> Self {
        let mut new_tags = HashMap::new();
        for (cluster, weight) in tags.iter() {
            new_tags.insert(cluster, weight);
        }
        Self {
            tags: new_tags,
            last_decay_block: block,
        }
    }

    /// Convert to a regular TagVector (for compatibility).
    pub fn to_tag_vector(&self) -> TagVector {
        let mut tv = TagVector::new();
        for (&cluster, &weight) in &self.tags {
            tv.set(cluster, weight);
        }
        tv
    }

    /// Get the block height when tags were last decayed.
    pub fn last_decay_block(&self) -> u64 {
        self.last_decay_block
    }

    /// Get tag weight for a cluster WITHOUT applying decay.
    /// Use `get_decayed` for the current effective weight.
    pub fn get_raw(&self, cluster: ClusterId) -> TagWeight {
        self.tags.get(&cluster).copied().unwrap_or(0)
    }

    /// Get tag weight with decay applied for current block.
    pub fn get_decayed(
        &self,
        cluster: ClusterId,
        current_block: u64,
        config: &BlockDecayConfig,
    ) -> TagWeight {
        let raw = self.get_raw(cluster);
        if raw == 0 {
            return 0;
        }

        let blocks_elapsed = current_block.saturating_sub(self.last_decay_block);
        let factor = config.decay_factor(blocks_elapsed);

        (raw as u64 * factor as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight
    }

    /// Apply decay up to the current block.
    ///
    /// This mutates the tag vector to reflect decay, updating `last_decay_block`.
    pub fn apply_block_decay(&mut self, current_block: u64, config: &BlockDecayConfig) {
        let blocks_elapsed = current_block.saturating_sub(self.last_decay_block);

        // Skip if not enough blocks have passed
        if blocks_elapsed < config.min_decay_interval {
            return;
        }

        let factor = config.decay_factor(blocks_elapsed);

        // Apply decay to all tags
        for weight in self.tags.values_mut() {
            *weight = (*weight as u64 * factor as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
        }

        // Prune small tags
        self.prune();

        // Update last decay block
        self.last_decay_block = current_block;
    }

    /// Apply hop-based decay (for hybrid mode).
    pub fn apply_hop_decay(&mut self, decay_rate: TagWeight) {
        if decay_rate == 0 {
            return;
        }

        for weight in self.tags.values_mut() {
            let decay =
                (*weight as u64 * decay_rate as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
            *weight = weight.saturating_sub(decay);
        }

        self.prune();
    }

    /// Total attributed weight (before decay).
    pub fn total_attributed_raw(&self) -> TagWeight {
        self.tags.values().sum::<TagWeight>().min(TAG_WEIGHT_SCALE)
    }

    /// Total attributed weight with decay applied.
    pub fn total_attributed(&self, current_block: u64, config: &BlockDecayConfig) -> TagWeight {
        let blocks_elapsed = current_block.saturating_sub(self.last_decay_block);
        let factor = config.decay_factor(blocks_elapsed);

        let raw_total = self.total_attributed_raw();
        (raw_total as u64 * factor as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight
    }

    /// Background weight with decay applied.
    pub fn background(&self, current_block: u64, config: &BlockDecayConfig) -> TagWeight {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_attributed(current_block, config))
    }

    /// Iterate over (cluster, raw_weight) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, TagWeight)> + '_ {
        self.tags.iter().map(|(&k, &v)| (k, v))
    }

    /// Number of tracked clusters.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Returns true if no cluster attribution.
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Mix incoming tags (after applying decay to both).
    pub fn mix(
        &mut self,
        self_value: u64,
        incoming: &BlockAwareTagVector,
        incoming_value: u64,
        current_block: u64,
        config: &BlockDecayConfig,
    ) {
        // Apply decay to both before mixing
        self.apply_block_decay(current_block, config);

        // Get decayed incoming weights
        let incoming_blocks_elapsed = current_block.saturating_sub(incoming.last_decay_block);
        let incoming_factor = config.decay_factor(incoming_blocks_elapsed);

        let total_value = self_value + incoming_value;
        if total_value == 0 {
            return;
        }

        // Collect all cluster IDs
        let mut all_clusters: Vec<ClusterId> = self.tags.keys().copied().collect();
        for &cluster in incoming.tags.keys() {
            if !self.tags.contains_key(&cluster) {
                all_clusters.push(cluster);
            }
        }

        // Compute weighted average
        for cluster in all_clusters {
            let self_weight = self.tags.get(&cluster).copied().unwrap_or(0) as u64;

            let incoming_raw = incoming.tags.get(&cluster).copied().unwrap_or(0) as u64;
            let incoming_weight = incoming_raw * incoming_factor as u64 / TAG_WEIGHT_SCALE as u64;

            let numerator = self_value * self_weight + incoming_value * incoming_weight;
            let new_weight = (numerator / total_value) as TagWeight;

            if new_weight >= Self::PRUNE_THRESHOLD {
                self.tags.insert(cluster, new_weight);
            } else {
                self.tags.remove(&cluster);
            }
        }

        // Apply hop decay if configured
        if config.hop_decay_rate > 0 {
            self.apply_hop_decay(config.hop_decay_rate);
        }

        self.prune();
    }

    /// Remove tags below threshold and enforce MAX_TAGS limit.
    fn prune(&mut self) {
        self.tags
            .retain(|_, &mut w| w >= Self::PRUNE_THRESHOLD);

        if self.tags.len() > Self::MAX_TAGS {
            let mut entries: Vec<_> = self.tags.drain().collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            self.tags = entries.into_iter().take(Self::MAX_TAGS).collect();
        }
    }

    /// Set tag weight directly (for minting).
    pub fn set(&mut self, cluster: ClusterId, weight: TagWeight) {
        if weight >= Self::PRUNE_THRESHOLD {
            self.tags.insert(cluster, weight);
        } else {
            self.tags.remove(&cluster);
        }
    }
}

// ============================================================================
// Rate-Limited Hop Decay (Hybrid Model)
// ============================================================================
//
// This model keeps hop-based decay semantics but rate-limits how frequently
// decay can occur. Each transfer can trigger decay, but only if enough blocks
// have passed since the last decay event.
//
// Key insight: Wash trading requires many rapid transactions. By requiring
// a minimum time between decays, we prevent the attack while keeping the
// intuitive "decay per hop" model.

/// Configuration for rate-limited hop decay.
#[derive(Clone, Debug)]
pub struct RateLimitedDecayConfig {
    /// Decay rate per eligible hop (parts per million).
    /// E.g., 50_000 = 5% decay per hop.
    pub decay_rate_per_hop: TagWeight,

    /// Minimum blocks that must pass between decay events.
    /// Hops within this window don't trigger decay.
    pub min_blocks_between_decays: u64,

    /// Optional: also apply time-based decay for very inactive accounts.
    /// If set, accounts that don't transact for a long time still decay.
    pub passive_half_life_blocks: Option<u64>,
}

impl Default for RateLimitedDecayConfig {
    fn default() -> Self {
        Self {
            decay_rate_per_hop: 50_000, // 5% per eligible hop
            min_blocks_between_decays: 360, // ~1 hour at 10s blocks
            passive_half_life_blocks: None, // No passive decay by default
        }
    }
}

impl RateLimitedDecayConfig {
    /// Create a config optimized for wash trading resistance.
    /// Requires 1 hour between decays, with 5% decay per eligible hop.
    pub fn anti_wash_trading() -> Self {
        Self {
            decay_rate_per_hop: 50_000, // 5%
            min_blocks_between_decays: 360, // 1 hour
            passive_half_life_blocks: Some(262_800), // 1 month passive decay
        }
    }

    /// Create a config with custom parameters.
    pub fn new(decay_pct: f64, min_hours_between: f64) -> Self {
        Self {
            decay_rate_per_hop: (decay_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as TagWeight,
            min_blocks_between_decays: (min_hours_between * 360.0) as u64,
            passive_half_life_blocks: None,
        }
    }

    /// Check if a hop is eligible for decay given the last decay block.
    pub fn is_hop_eligible(&self, last_decay_block: u64, current_block: u64) -> bool {
        current_block.saturating_sub(last_decay_block) >= self.min_blocks_between_decays
    }

    /// Compute the number of eligible decay events between two blocks.
    /// This counts how many rate-limit windows have passed.
    pub fn eligible_decays(&self, last_decay_block: u64, current_block: u64) -> u64 {
        if self.min_blocks_between_decays == 0 {
            return 1; // Every hop counts if no rate limit
        }
        let elapsed = current_block.saturating_sub(last_decay_block);
        elapsed / self.min_blocks_between_decays
    }
}

/// A tag vector with rate-limited hop decay.
#[derive(Clone, Debug)]
pub struct RateLimitedTagVector {
    /// The underlying tag weights.
    tags: HashMap<ClusterId, TagWeight>,

    /// Block height when decay was last applied.
    last_decay_block: u64,
}

impl Default for RateLimitedTagVector {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitedTagVector {
    /// Minimum tag weight to retain.
    pub const PRUNE_THRESHOLD: TagWeight = 100;

    /// Maximum number of tags to track.
    pub const MAX_TAGS: usize = 32;

    /// Create an empty tag vector.
    pub fn new() -> Self {
        Self {
            tags: HashMap::new(),
            last_decay_block: 0,
        }
    }

    /// Create a tag vector at a specific block height.
    pub fn at_block(block: u64) -> Self {
        Self {
            tags: HashMap::new(),
            last_decay_block: block,
        }
    }

    /// Create a tag vector fully attributed to one cluster.
    pub fn single(cluster: ClusterId, block: u64) -> Self {
        let mut tags = HashMap::new();
        tags.insert(cluster, TAG_WEIGHT_SCALE);
        Self {
            tags,
            last_decay_block: block,
        }
    }

    /// Get the last decay block.
    pub fn last_decay_block(&self) -> u64 {
        self.last_decay_block
    }

    /// Get tag weight for a cluster.
    pub fn get(&self, cluster: ClusterId) -> TagWeight {
        self.tags.get(&cluster).copied().unwrap_or(0)
    }

    /// Apply hop decay if eligible (rate-limited).
    ///
    /// Returns true if decay was applied, false if rate-limited.
    pub fn try_apply_hop_decay(
        &mut self,
        current_block: u64,
        config: &RateLimitedDecayConfig,
    ) -> bool {
        if !config.is_hop_eligible(self.last_decay_block, current_block) {
            return false; // Rate limited - no decay
        }

        // Apply hop decay
        let decay_rate = config.decay_rate_per_hop;
        for weight in self.tags.values_mut() {
            let decay =
                (*weight as u64 * decay_rate as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
            *weight = weight.saturating_sub(decay);
        }

        self.prune();
        self.last_decay_block = current_block;
        true
    }

    /// Apply any pending passive decay (for accounts that haven't transacted).
    pub fn apply_passive_decay(&mut self, current_block: u64, config: &RateLimitedDecayConfig) {
        if let Some(half_life) = config.passive_half_life_blocks {
            let block_config = BlockDecayConfig {
                half_life_blocks: half_life,
                min_decay_interval: 1,
                hop_decay_rate: 0,
            };
            let factor = block_config.decay_factor(
                current_block.saturating_sub(self.last_decay_block)
            );

            for weight in self.tags.values_mut() {
                *weight = (*weight as u64 * factor as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
            }
            self.prune();
        }
    }

    /// Total attributed weight.
    pub fn total_attributed(&self) -> TagWeight {
        self.tags.values().sum::<TagWeight>().min(TAG_WEIGHT_SCALE)
    }

    /// Background weight.
    pub fn background(&self) -> TagWeight {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_attributed())
    }

    /// Iterate over (cluster, weight) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, TagWeight)> + '_ {
        self.tags.iter().map(|(&k, &v)| (k, v))
    }

    /// Number of tracked clusters.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Returns true if no cluster attribution.
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Set tag weight directly.
    pub fn set(&mut self, cluster: ClusterId, weight: TagWeight) {
        if weight >= Self::PRUNE_THRESHOLD {
            self.tags.insert(cluster, weight);
        } else {
            self.tags.remove(&cluster);
        }
    }

    /// Remove tags below threshold and enforce MAX_TAGS limit.
    fn prune(&mut self) {
        self.tags.retain(|_, &mut w| w >= Self::PRUNE_THRESHOLD);

        if self.tags.len() > Self::MAX_TAGS {
            let mut entries: Vec<_> = self.tags.drain().collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            self.tags = entries.into_iter().take(Self::MAX_TAGS).collect();
        }
    }
}

// ============================================================================
// AND-Based Decay (Time AND Hop Required)
// ============================================================================
//
// This model requires BOTH conditions to be met for decay:
// 1. A transfer (hop) must occur
// 2. AND sufficient time must have passed since the last decay
//
// Key properties:
// - Holding without trading: NO decay (preserves cluster tag)
// - Rapid wash trading: NO decay (rate-limited)
// - Patient wash trading: YES decay (but requires time investment)
// - Legitimate trading over time: YES decay (natural diffusion)
//
// This ensures wealthy clusters cannot passively reduce their tax burden -
// they must actively engage in wash trading, which is rate-limited.

/// Configuration for AND-based decay (requires both time AND hop).
#[derive(Clone, Debug)]
pub struct AndDecayConfig {
    /// Decay rate per eligible hop (parts per million).
    /// E.g., 50_000 = 5% decay per eligible hop.
    pub decay_rate_per_hop: TagWeight,

    /// Minimum blocks that must pass between decay events.
    /// Hops within this window don't trigger decay.
    pub min_blocks_between_decays: u64,

    /// Maximum number of decays allowed per epoch.
    /// This caps total decay achievable in any time period.
    /// Set to 0 for unlimited.
    pub max_decays_per_epoch: u32,

    /// Epoch length in blocks (for max_decays_per_epoch).
    pub epoch_blocks: u64,
}

impl Default for AndDecayConfig {
    fn default() -> Self {
        Self {
            decay_rate_per_hop: 50_000, // 5% per eligible hop
            min_blocks_between_decays: 360, // ~1 hour at 10s blocks
            max_decays_per_epoch: 24,   // Max 24 decays per epoch
            epoch_blocks: 8_640,        // ~1 day epoch
        }
    }
}

impl AndDecayConfig {
    /// Create a config optimized for wash trading resistance.
    pub fn anti_wash_trading() -> Self {
        Self {
            decay_rate_per_hop: 50_000,     // 5%
            min_blocks_between_decays: 720, // 2 hours
            max_decays_per_epoch: 12,       // Max 12 decays per day
            epoch_blocks: 8_640,            // 1 day
        }
    }

    /// Create a config with custom parameters.
    pub fn new(decay_pct: f64, min_hours_between: f64, max_per_day: u32) -> Self {
        Self {
            decay_rate_per_hop: (decay_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as TagWeight,
            min_blocks_between_decays: (min_hours_between * 360.0) as u64,
            max_decays_per_epoch: max_per_day,
            epoch_blocks: 8_640,
        }
    }

    /// Maximum decay achievable per epoch.
    /// Returns the fraction remaining after max_decays_per_epoch decays.
    pub fn max_decay_per_epoch(&self) -> f64 {
        if self.max_decays_per_epoch == 0 {
            return 0.0; // Unlimited
        }
        let decay_fraction = self.decay_rate_per_hop as f64 / TAG_WEIGHT_SCALE as f64;
        (1.0 - decay_fraction).powi(self.max_decays_per_epoch as i32)
    }
}

/// A tag vector with AND-based decay (requires both time AND hop).
#[derive(Clone, Debug)]
pub struct AndTagVector {
    /// The underlying tag weights.
    tags: HashMap<ClusterId, TagWeight>,

    /// Block height when decay was last applied.
    last_decay_block: u64,

    /// Number of decays applied in current epoch.
    decays_this_epoch: u32,

    /// Start block of current epoch.
    epoch_start_block: u64,
}

impl Default for AndTagVector {
    fn default() -> Self {
        Self::new()
    }
}

impl AndTagVector {
    /// Minimum tag weight to retain.
    pub const PRUNE_THRESHOLD: TagWeight = 100;

    /// Maximum number of tags to track.
    pub const MAX_TAGS: usize = 32;

    /// Create an empty tag vector.
    pub fn new() -> Self {
        Self {
            tags: HashMap::new(),
            last_decay_block: 0,
            decays_this_epoch: 0,
            epoch_start_block: 0,
        }
    }

    /// Create a tag vector at a specific block height.
    pub fn at_block(block: u64) -> Self {
        Self {
            tags: HashMap::new(),
            last_decay_block: block,
            decays_this_epoch: 0,
            epoch_start_block: block,
        }
    }

    /// Create a tag vector fully attributed to one cluster.
    pub fn single(cluster: ClusterId, block: u64) -> Self {
        let mut tags = HashMap::new();
        tags.insert(cluster, TAG_WEIGHT_SCALE);
        Self {
            tags,
            last_decay_block: block,
            decays_this_epoch: 0,
            epoch_start_block: block,
        }
    }

    /// Get the last decay block.
    pub fn last_decay_block(&self) -> u64 {
        self.last_decay_block
    }

    /// Get decays applied in current epoch.
    pub fn decays_this_epoch(&self) -> u32 {
        self.decays_this_epoch
    }

    /// Get tag weight for a cluster.
    pub fn get(&self, cluster: ClusterId) -> TagWeight {
        self.tags.get(&cluster).copied().unwrap_or(0)
    }

    /// Check if a hop at the given block would trigger decay.
    ///
    /// Note: This doesn't account for epoch resets - call after `try_apply_decay_on_transfer`
    /// has handled epoch transitions, or use for informational purposes only.
    pub fn would_decay(&self, current_block: u64, config: &AndDecayConfig) -> bool {
        // First decay ever is always eligible (never_decayed is true when last_decay_block == 0
        // and we haven't applied any decays yet)
        let never_decayed = self.last_decay_block == 0 && self.decays_this_epoch == 0;

        // Check time condition (first decay bypasses this)
        let time_eligible = never_decayed
            || current_block.saturating_sub(self.last_decay_block)
                >= config.min_blocks_between_decays;

        if !time_eligible {
            return false;
        }

        // Check epoch cap
        if config.max_decays_per_epoch > 0 {
            if self.decays_this_epoch >= config.max_decays_per_epoch {
                return false; // Epoch cap reached
            }
        }

        true
    }

    /// Apply decay on transfer if eligible (AND condition: time AND hop).
    ///
    /// Call this when a transfer occurs. Returns true if decay was applied.
    /// Decay only happens if:
    /// 1. Enough blocks have passed since last decay (time condition)
    /// 2. This is being called during a transfer (hop condition - implicit)
    /// 3. Epoch decay cap not reached (if configured)
    pub fn try_apply_decay_on_transfer(
        &mut self,
        current_block: u64,
        config: &AndDecayConfig,
    ) -> bool {
        // Check if we're in a new epoch
        if current_block.saturating_sub(self.epoch_start_block) >= config.epoch_blocks {
            self.epoch_start_block = current_block;
            self.decays_this_epoch = 0;
        }

        // Check if decay would occur
        if !self.would_decay(current_block, config) {
            return false;
        }

        // Apply hop decay
        let decay_rate = config.decay_rate_per_hop;
        for weight in self.tags.values_mut() {
            let decay =
                (*weight as u64 * decay_rate as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
            *weight = weight.saturating_sub(decay);
        }

        self.prune();
        self.last_decay_block = current_block;
        self.decays_this_epoch += 1;
        true
    }

    /// Total attributed weight.
    pub fn total_attributed(&self) -> TagWeight {
        self.tags.values().sum::<TagWeight>().min(TAG_WEIGHT_SCALE)
    }

    /// Background weight.
    pub fn background(&self) -> TagWeight {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_attributed())
    }

    /// Iterate over (cluster, weight) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, TagWeight)> + '_ {
        self.tags.iter().map(|(&k, &v)| (k, v))
    }

    /// Number of tracked clusters.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Returns true if no cluster attribution.
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Set tag weight directly.
    pub fn set(&mut self, cluster: ClusterId, weight: TagWeight) {
        if weight >= Self::PRUNE_THRESHOLD {
            self.tags.insert(cluster, weight);
        } else {
            self.tags.remove(&cluster);
        }
    }

    /// Remove tags below threshold and enforce MAX_TAGS limit.
    fn prune(&mut self) {
        self.tags.retain(|_, &mut w| w >= Self::PRUNE_THRESHOLD);

        if self.tags.len() > Self::MAX_TAGS {
            let mut entries: Vec<_> = self.tags.drain().collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            self.tags = entries.into_iter().take(Self::MAX_TAGS).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decay_factor_half_life() {
        let config = BlockDecayConfig {
            half_life_blocks: 1000,
            min_decay_interval: 1,
            hop_decay_rate: 0,
        };

        // At 0 blocks, factor should be 100%
        assert_eq!(config.decay_factor(0), TAG_WEIGHT_SCALE);

        // At half-life, factor should be ~50%
        let at_half = config.decay_factor(1000);
        assert!(
            (450_000..=550_000).contains(&at_half),
            "At half-life: {at_half}"
        );

        // At 2 half-lives, factor should be ~25%
        let at_two = config.decay_factor(2000);
        assert!(
            (200_000..=300_000).contains(&at_two),
            "At 2 half-lives: {at_two}"
        );

        // At 3 half-lives, factor should be ~12.5%
        let at_three = config.decay_factor(3000);
        assert!(
            (100_000..=175_000).contains(&at_three),
            "At 3 half-lives: {at_three}"
        );
    }

    #[test]
    fn test_block_decay_vs_hop_decay() {
        let config = BlockDecayConfig {
            half_life_blocks: 1000,
            min_decay_interval: 1,
            hop_decay_rate: 0,
        };

        let cluster = ClusterId::new(1);

        // Create tag vector at block 0
        let mut tags = BlockAwareTagVector::single(cluster, 0);

        // Simulate 100 transactions within 10 blocks (wash trading attempt)
        for _ in 0..100 {
            // Each transaction is at block 0 (no time passes)
            tags.apply_block_decay(10, &config);
        }

        // With block decay, all 100 txs only result in 10 blocks of decay
        let weight = tags.get_raw(cluster);
        assert!(
            weight > 900_000,
            "Block decay should resist wash trading: {weight}"
        );

        // Compare to hop-based decay at 5%
        let mut hop_tags = TagVector::single(cluster);
        for _ in 0..100 {
            hop_tags.apply_decay(50_000); // 5% per hop
        }

        let hop_weight = hop_tags.get(cluster);
        // 0.95^100 ≈ 0.006 = ~6000
        assert!(
            hop_weight < 50_000,
            "Hop decay should be much faster: {hop_weight}"
        );

        // Block decay is 18x more resistant to wash trading in this example
        assert!(weight > hop_weight * 10);
    }

    #[test]
    fn test_hybrid_decay() {
        let config = BlockDecayConfig {
            half_life_blocks: 1000,
            min_decay_interval: 1,
            hop_decay_rate: 10_000, // 1% per hop
        };

        let cluster = ClusterId::new(1);
        let mut tags = BlockAwareTagVector::single(cluster, 0);

        // One transfer at block 100
        tags.apply_block_decay(100, &config);
        tags.apply_hop_decay(config.hop_decay_rate);

        let weight = tags.get_raw(cluster);

        // Should have ~95% from block decay (100/1000 half-life ≈ 7% decay)
        // times ~99% from hop decay
        // ≈ 92-93%
        assert!(
            (850_000..=950_000).contains(&weight),
            "Hybrid decay: {weight}"
        );
    }

    // ========================================================================
    // AND-Based Decay Tests
    // ========================================================================

    #[test]
    fn test_and_decay_rapid_wash_trading_resistance() {
        // Attack: 100 rapid self-transfers in 100 blocks
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000, // 5%
            min_blocks_between_decays: 360, // ~1 hour
            max_decays_per_epoch: 24,
            epoch_blocks: 8_640,
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // Attempt 100 wash trades in 100 blocks
        let mut decays = 0;
        for i in 0..100 {
            if tags.try_apply_decay_on_transfer(i, &config) {
                decays += 1;
            }
        }

        // Should have exactly 1 decay (first transfer is always eligible, rest blocked)
        // 100 blocks < 360 min_blocks_between, so only the first one succeeds
        assert_eq!(decays, 1, "Only first transfer should decay, rest blocked");

        // Tag should be 95% (one 5% decay)
        let expected = TAG_WEIGHT_SCALE - 50_000; // 950,000
        assert_eq!(tags.get(cluster), expected, "Tag should be 95%");

        // Compare to hop-based: 100 transfers would give 0.95^100 ≈ 0.6%
        // AND-based gives 95% - that's 158x more resistant!
    }

    #[test]
    fn test_and_decay_rate_limiting() {
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000, // 5%
            min_blocks_between_decays: 100,
            max_decays_per_epoch: 1000, // High cap to test rate limiting
            epoch_blocks: 100_000,
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // Attempt 50 transactions over 1000 blocks (spaced 20 blocks apart)
        let mut decays = 0;
        for i in 0..50 {
            let block = i * 20; // Every 20 blocks
            if tags.try_apply_decay_on_transfer(block, &config) {
                decays += 1;
            }
        }

        // With 100 block rate limit and 20 block spacing, should get ~10 decays
        // (blocks 0, 100, 200, 300, ... up to 980)
        assert!(
            decays <= 10,
            "Rate limiting should cap decays: got {decays}"
        );
    }

    #[test]
    fn test_and_decay_epoch_cap() {
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000, // 5%
            min_blocks_between_decays: 10, // Low rate limit
            max_decays_per_epoch: 5, // Strict epoch cap
            epoch_blocks: 1000,
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // Attempt many transactions within ONE epoch (blocks 0-500)
        let mut decays = 0;
        for i in 0..50 {
            let block = i * 10; // Exactly at rate limit, stays within epoch
            if tags.try_apply_decay_on_transfer(block, &config) {
                decays += 1;
            }
        }

        // Should be capped at 5 (max_decays_per_epoch) even though 50 txs attempted
        assert_eq!(decays, 5, "Epoch cap should limit decays to 5");

        // Tag should be 0.95^5 ≈ 77.4%
        let expected = (0.95_f64.powi(5) * TAG_WEIGHT_SCALE as f64) as TagWeight;
        let actual = tags.get(cluster);
        assert!(
            (actual as i64 - expected as i64).abs() < 5000,
            "Tag should be ~77.4%: got {actual}, expected ~{expected}"
        );
    }

    #[test]
    fn test_and_decay_epoch_reset() {
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000,
            min_blocks_between_decays: 10,
            max_decays_per_epoch: 3,
            epoch_blocks: 100,
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // First epoch (blocks 0-99): Try to get 3 decays
        // Transactions at blocks 0, 15, 30 should all succeed (rate limit = 10)
        // Transaction at block 45 should be blocked (epoch cap = 3)
        let mut total_decays = 0;

        // Block 0: First decay (first ever, always eligible)
        assert!(tags.try_apply_decay_on_transfer(0, &config));
        total_decays += 1;

        // Block 15: Second decay (15 - 0 = 15 >= 10)
        assert!(tags.try_apply_decay_on_transfer(15, &config));
        total_decays += 1;

        // Block 30: Third decay (30 - 15 = 15 >= 10)
        assert!(tags.try_apply_decay_on_transfer(30, &config));
        total_decays += 1;

        // Block 45: Should be blocked by epoch cap (3 decays already)
        assert!(!tags.try_apply_decay_on_transfer(45, &config));

        assert_eq!(total_decays, 3, "First epoch should allow exactly 3 decays");
        assert_eq!(tags.decays_this_epoch(), 3);

        // New epoch starts at block 100
        // Block 100: First decay of new epoch
        assert!(tags.try_apply_decay_on_transfer(100, &config));
        total_decays += 1;

        // Block 115: Second decay of new epoch
        assert!(tags.try_apply_decay_on_transfer(115, &config));
        total_decays += 1;

        // Block 130: Third decay of new epoch
        assert!(tags.try_apply_decay_on_transfer(130, &config));
        total_decays += 1;

        // Block 145: Should be blocked by epoch cap
        assert!(!tags.try_apply_decay_on_transfer(145, &config));

        assert_eq!(total_decays, 6, "Two epochs should allow 6 total decays");
    }

    #[test]
    fn test_and_decay_no_passive_decay() {
        let config = AndDecayConfig::default();
        let cluster = ClusterId::new(1);

        // Create tags at block 0
        let tags = AndTagVector::single(cluster, 0);

        // No transactions, just time passing - tag should remain 100%
        // (Unlike block-based decay, AND-based requires a hop)
        assert_eq!(
            tags.get(cluster),
            TAG_WEIGHT_SCALE,
            "No passive decay without transactions"
        );
    }

    #[test]
    fn test_and_decay_weekly_bound() {
        // Verify the mathematical bound: max 84 decays per week with 12/day cap
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000, // 5%
            min_blocks_between_decays: 360, // 1 hour
            max_decays_per_epoch: 12, // 12 per day
            epoch_blocks: 8_640, // 1 day
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // Patient attacker: 1 tx every rate limit period for 1 week
        let week_blocks = 8_640 * 7;
        let mut decays = 0;
        let mut block = 0u64;

        while block < week_blocks {
            if tags.try_apply_decay_on_transfer(block, &config) {
                decays += 1;
            }
            block += 360; // Every hour
        }

        // Should be capped at 7 days × 12/day = 84
        assert!(
            decays <= 84,
            "Weekly decay should be bounded at 84: got {decays}"
        );

        // Tag should be approximately 0.95^84 ≈ 1.35%
        let expected = (0.95_f64.powi(decays as i32) * TAG_WEIGHT_SCALE as f64) as TagWeight;
        let actual = tags.get(cluster);
        assert!(
            (actual as i64 - expected as i64).abs() < 5000,
            "Tag should be ~{:.2}%: got {actual}",
            expected as f64 / TAG_WEIGHT_SCALE as f64 * 100.0
        );
    }

    #[test]
    fn test_and_decay_legitimate_trading() {
        // Verify legitimate traders (1 tx/day over 30 days) get reasonable decay
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000,
            min_blocks_between_decays: 360,
            max_decays_per_epoch: 12,
            epoch_blocks: 8_640,
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // 1 transaction per day for 30 days
        let mut decays = 0;
        for day in 0..30 {
            let block = day * 8_640 + 1000; // Mid-day transaction
            if tags.try_apply_decay_on_transfer(block, &config) {
                decays += 1;
            }
        }

        // Should get ~30 decays (one per day, well under rate limit)
        assert_eq!(decays, 30, "Legitimate trader should get 1 decay per day");

        // Tag should be 0.95^30 ≈ 21.5%
        let expected = (0.95_f64.powi(30) * TAG_WEIGHT_SCALE as f64) as TagWeight;
        let actual = tags.get(cluster);
        assert!(
            (actual as i64 - expected as i64).abs() < 10000,
            "Tag should be ~21.5%: got {actual}, expected ~{expected}"
        );
    }

    #[test]
    fn test_and_decay_config_max_decay_per_epoch() {
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000, // 5%
            min_blocks_between_decays: 100,
            max_decays_per_epoch: 12,
            epoch_blocks: 8_640,
        };

        // Max decay per epoch: 0.95^12 ≈ 54%
        let expected = 0.95_f64.powi(12);
        let actual = config.max_decay_per_epoch();
        assert!(
            (actual - expected).abs() < 0.01,
            "Max decay per epoch should be ~54%: got {actual}"
        );
    }

    #[test]
    fn test_and_decay_would_decay_predicate() {
        let config = AndDecayConfig {
            decay_rate_per_hop: 50_000,
            min_blocks_between_decays: 100,
            max_decays_per_epoch: 5,
            epoch_blocks: 1000,
        };

        let cluster = ClusterId::new(1);
        let mut tags = AndTagVector::single(cluster, 0);

        // Initially should decay (time = 0, but no prior decay)
        assert!(
            tags.would_decay(0, &config),
            "First transfer should be eligible"
        );

        // After first decay, immediate should not
        tags.try_apply_decay_on_transfer(0, &config);
        assert!(
            !tags.would_decay(50, &config),
            "Should not decay within rate limit"
        );

        // After rate limit, should decay
        assert!(
            tags.would_decay(100, &config),
            "Should decay after rate limit"
        );
    }
}
