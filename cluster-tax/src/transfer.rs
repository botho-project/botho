//! Transfer logic with tag inheritance and progressive fees.
//!
//! This module provides two transfer implementations:
//!
//! 1. **Legacy hop-based decay** (`execute_transfer`): Tags decay by a fixed
//!    percentage per transfer. Vulnerable to wash trading attacks where users
//!    accelerate decay through self-transfers.
//!
//! 2. **AND-based decay** (`execute_transfer_and`): Tags decay only when BOTH a
//!    transfer occurs AND sufficient time has passed since the last decay.
//!    Resistant to wash trading attacks.
//!
//! For new implementations, prefer `execute_transfer_and` with
//! `BlockAwareAccount`.

use crate::{
    block_decay::AndDecayConfig,
    cluster::{ClusterId, ClusterWealth},
    fee_curve::{FeeCurve, FeeRateBps},
    tag::{TagVector, TAG_WEIGHT_SCALE},
};

/// Account state for simulation purposes.
///
/// In a real implementation, this would be derived from UTXOs.
#[derive(Clone, Debug)]
pub struct Account {
    /// Account identifier (for simulation).
    pub id: u64,

    /// Current balance.
    pub balance: u64,

    /// Tag vector representing cluster attribution.
    pub tags: TagVector,
}

impl Account {
    /// Create a new account with zero balance and no tags.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            balance: 0,
            tags: TagVector::new(),
        }
    }

    /// Create an account with initial balance from a specific cluster.
    ///
    /// Used for minting: the newly created coins are fully attributed
    /// to the new cluster.
    pub fn with_minted_balance(id: u64, balance: u64, cluster: ClusterId) -> Self {
        Self {
            id,
            balance,
            tags: TagVector::single(cluster),
        }
    }

    /// Compute effective fee rate based on this account's tags and cluster
    /// wealths.
    pub fn effective_fee_rate(
        &self,
        cluster_wealth: &ClusterWealth,
        fee_curve: &FeeCurve,
    ) -> FeeRateBps {
        let mut weighted_rate: u64 = 0;
        let mut total_weight: u64 = 0;

        // Weighted average of cluster rates by tag weight
        for (cluster, weight) in self.tags.iter() {
            let cluster_w = cluster_wealth.get(cluster);
            let rate = fee_curve.rate_bps(cluster_w) as u64;
            weighted_rate += rate * weight as u64;
            total_weight += weight as u64;
        }

        // Add background contribution
        let bg_weight = self.tags.background() as u64;
        weighted_rate += fee_curve.background_rate_bps as u64 * bg_weight;
        total_weight += bg_weight;

        if total_weight == 0 {
            return fee_curve.background_rate_bps;
        }

        (weighted_rate / total_weight) as FeeRateBps
    }
}

/// Result of a transfer operation.
#[derive(Clone, Debug)]
pub struct TransferResult {
    /// Fee paid (burned).
    pub fee: u64,

    /// Amount received by recipient after fee.
    pub net_amount: u64,

    /// Effective fee rate that was applied (basis points).
    pub fee_rate_bps: FeeRateBps,
}

/// Configuration for the transfer system.
#[derive(Clone, Debug)]
pub struct TransferConfig {
    /// Fee curve parameters.
    pub fee_curve: FeeCurve,

    /// Decay rate per transfer (parts per million).
    /// E.g., 50_000 = 5% decay per hop.
    pub decay_rate: u32,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            fee_curve: FeeCurve::default_params(),
            decay_rate: 50_000, // 5% decay per hop
        }
    }
}

/// Execute a transfer between accounts.
///
/// This is the core operation that:
/// 1. Computes the progressive fee based on sender's cluster tags
/// 2. Updates sender's balance
/// 3. Applies tag decay to the transferred coins
/// 4. Mixes the incoming tags into the receiver's tag vector
/// 5. Updates global cluster wealth state
///
/// Returns an error if the sender has insufficient balance.
pub fn execute_transfer(
    sender: &mut Account,
    receiver: &mut Account,
    amount: u64,
    config: &TransferConfig,
    cluster_wealth: &mut ClusterWealth,
) -> Result<TransferResult, TransferError> {
    if sender.balance < amount {
        return Err(TransferError::InsufficientBalance {
            available: sender.balance,
            requested: amount,
        });
    }

    // 1. Compute fee based on sender's effective rate
    let fee_rate = sender.effective_fee_rate(cluster_wealth, &config.fee_curve);
    let fee = (amount as u128 * fee_rate as u128 / 10_000) as u64;
    let net_amount = amount.saturating_sub(fee);

    // 2. Update sender balance
    sender.balance -= amount;

    // 3. Compute tags for the transferred coins (with decay)
    let mut transferred_tags = sender.tags.clone();
    transferred_tags.apply_decay(config.decay_rate);

    // 4. Update cluster wealth (before mixing, to reflect decay)
    // The sender's contribution to each cluster decreases
    for (cluster, weight) in sender.tags.iter() {
        // Mass leaving sender for this cluster
        let mass_leaving = amount as i64 * weight as i64 / TAG_WEIGHT_SCALE as i64;
        cluster_wealth.apply_delta(cluster, -mass_leaving);
    }

    // Mass arriving at receiver (after decay)
    for (cluster, weight) in transferred_tags.iter() {
        let mass_arriving = net_amount as i64 * weight as i64 / TAG_WEIGHT_SCALE as i64;
        cluster_wealth.apply_delta(cluster, mass_arriving);
    }

    // 5. Mix into receiver's tags
    let receiver_balance_before = receiver.balance;
    receiver
        .tags
        .mix(receiver_balance_before, &transferred_tags, net_amount);
    receiver.balance += net_amount;

    Ok(TransferResult {
        fee,
        net_amount,
        fee_rate_bps: fee_rate,
    })
}

/// Errors that can occur during a transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// Sender doesn't have enough balance.
    InsufficientBalance { available: u64, requested: u64 },
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::InsufficientBalance {
                available,
                requested,
            } => {
                write!(
                    f,
                    "Insufficient balance: have {available}, need {requested}"
                )
            }
        }
    }
}

impl std::error::Error for TransferError {}

// ============================================================================
// Block-Aware Transfer System (AND-Based Decay)
// ============================================================================
//
// The following types and functions implement wash trading resistant transfers
// using AND-based decay: decay only occurs when BOTH a transfer happens AND
// sufficient time has passed since the last decay.

/// Account state with block-aware decay tracking.
///
/// This account type tracks the metadata needed for AND-based decay:
/// - `last_decay_block`: When decay was last applied
/// - `decays_this_epoch`: How many decays have occurred in current epoch
/// - `epoch_start_block`: When the current epoch started
///
/// In production, this metadata would be stored in UTXO outputs.
#[derive(Clone, Debug)]
pub struct BlockAwareAccount {
    /// Account identifier (for simulation).
    pub id: u64,

    /// Current balance.
    pub balance: u64,

    /// Tag vector representing cluster attribution.
    pub tags: TagVector,

    /// Block height when decay was last applied.
    pub last_decay_block: u64,

    /// Number of decays applied in current epoch.
    pub decays_this_epoch: u32,

    /// Block height when current epoch started.
    pub epoch_start_block: u64,
}

impl BlockAwareAccount {
    /// Create a new account with zero balance and no tags.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            balance: 0,
            tags: TagVector::new(),
            last_decay_block: 0,
            decays_this_epoch: 0,
            epoch_start_block: 0,
        }
    }

    /// Create a new account at a specific block height.
    pub fn at_block(id: u64, block: u64) -> Self {
        Self {
            id,
            balance: 0,
            tags: TagVector::new(),
            last_decay_block: block,
            decays_this_epoch: 0,
            epoch_start_block: block,
        }
    }

    /// Create an account with initial balance from a specific cluster.
    ///
    /// Used for minting: the newly created coins are fully attributed
    /// to the new cluster.
    pub fn with_minted_balance(id: u64, balance: u64, cluster: ClusterId, block: u64) -> Self {
        Self {
            id,
            balance,
            tags: TagVector::single(cluster),
            last_decay_block: block,
            decays_this_epoch: 0,
            epoch_start_block: block,
        }
    }

    /// Convert from a legacy Account (assumes block 0 for decay tracking).
    pub fn from_legacy(account: Account) -> Self {
        Self {
            id: account.id,
            balance: account.balance,
            tags: account.tags,
            last_decay_block: 0,
            decays_this_epoch: 0,
            epoch_start_block: 0,
        }
    }

    /// Compute effective fee rate based on this account's tags and cluster
    /// wealths.
    pub fn effective_fee_rate(
        &self,
        cluster_wealth: &ClusterWealth,
        fee_curve: &FeeCurve,
    ) -> FeeRateBps {
        let mut weighted_rate: u64 = 0;
        let mut total_weight: u64 = 0;

        // Weighted average of cluster rates by tag weight
        for (cluster, weight) in self.tags.iter() {
            let cluster_w = cluster_wealth.get(cluster);
            let rate = fee_curve.rate_bps(cluster_w) as u64;
            weighted_rate += rate * weight as u64;
            total_weight += weight as u64;
        }

        // Add background contribution
        let bg_weight = self.tags.background() as u64;
        weighted_rate += fee_curve.background_rate_bps as u64 * bg_weight;
        total_weight += bg_weight;

        if total_weight == 0 {
            return fee_curve.background_rate_bps;
        }

        (weighted_rate / total_weight) as FeeRateBps
    }

    /// Check if a transfer at the given block would trigger decay.
    pub fn would_decay(&self, current_block: u64, config: &AndDecayConfig) -> bool {
        // First decay ever is always eligible
        let never_decayed = self.last_decay_block == 0 && self.decays_this_epoch == 0;

        // Check time condition
        let time_eligible = never_decayed
            || current_block.saturating_sub(self.last_decay_block)
                >= config.min_blocks_between_decays;

        if !time_eligible {
            return false;
        }

        // Check epoch cap (accounting for potential epoch reset)
        if config.max_decays_per_epoch > 0 {
            let in_new_epoch =
                current_block.saturating_sub(self.epoch_start_block) >= config.epoch_blocks;
            let effective_decays = if in_new_epoch {
                0
            } else {
                self.decays_this_epoch
            };
            // Epoch cap reached - no more decays allowed this epoch
            return effective_decays < config.max_decays_per_epoch;
        }

        true
    }
}

/// Configuration for the AND-based transfer system.
#[derive(Clone, Debug, Default)]
pub struct AndTransferConfig {
    /// Fee curve parameters.
    pub fee_curve: FeeCurve,

    /// AND-based decay configuration.
    pub decay_config: AndDecayConfig,
}

impl AndTransferConfig {
    /// Create a config optimized for wash trading resistance.
    pub fn anti_wash_trading() -> Self {
        Self {
            fee_curve: FeeCurve::default_params(),
            decay_config: AndDecayConfig::anti_wash_trading(),
        }
    }
}

/// Result of an AND-based transfer operation.
#[derive(Clone, Debug)]
pub struct AndTransferResult {
    /// Fee paid (burned).
    pub fee: u64,

    /// Amount received by recipient after fee.
    pub net_amount: u64,

    /// Effective fee rate that was applied (basis points).
    pub fee_rate_bps: FeeRateBps,

    /// Whether decay was applied on this transfer.
    pub decay_applied: bool,
}

/// Execute a transfer with AND-based decay (wash trading resistant).
///
/// This function implements the AND-based decay model where decay only occurs
/// when BOTH conditions are met:
/// 1. A transfer (hop) occurs
/// 2. Sufficient time has passed since the last decay (rate limit)
/// 3. Epoch decay cap not reached
///
/// Key properties:
/// - **Holding without trading**: NO decay (preserves cluster tag)
/// - **Rapid wash trading**: NO decay (rate-limited)
/// - **Patient wash trading**: Bounded decay (epoch cap)
/// - **Legitimate trading**: Normal decay over time
///
/// Returns an error if the sender has insufficient balance.
pub fn execute_transfer_and(
    sender: &mut BlockAwareAccount,
    receiver: &mut BlockAwareAccount,
    amount: u64,
    current_block: u64,
    config: &AndTransferConfig,
    cluster_wealth: &mut ClusterWealth,
) -> Result<AndTransferResult, TransferError> {
    if sender.balance < amount {
        return Err(TransferError::InsufficientBalance {
            available: sender.balance,
            requested: amount,
        });
    }

    // 1. Compute fee based on sender's effective rate
    let fee_rate = sender.effective_fee_rate(cluster_wealth, &config.fee_curve);
    let fee = (amount as u128 * fee_rate as u128 / 10_000) as u64;
    let net_amount = amount.saturating_sub(fee);

    // 2. Update sender balance
    sender.balance -= amount;

    // 3. Compute tags for the transferred coins (with AND-based decay)
    let mut transferred_tags = sender.tags.clone();
    let decay_applied = try_apply_and_decay(sender, current_block, &config.decay_config);

    if decay_applied {
        // Apply the decay to the transferred tags
        let decay_rate = config.decay_config.decay_rate_per_hop;
        transferred_tags.apply_decay(decay_rate);
    }

    // 4. Update cluster wealth (before mixing, to reflect decay)
    for (cluster, weight) in sender.tags.iter() {
        let mass_leaving = amount as i64 * weight as i64 / TAG_WEIGHT_SCALE as i64;
        cluster_wealth.apply_delta(cluster, -mass_leaving);
    }

    for (cluster, weight) in transferred_tags.iter() {
        let mass_arriving = net_amount as i64 * weight as i64 / TAG_WEIGHT_SCALE as i64;
        cluster_wealth.apply_delta(cluster, mass_arriving);
    }

    // 5. Mix into receiver's tags
    let receiver_balance_before = receiver.balance;
    receiver
        .tags
        .mix(receiver_balance_before, &transferred_tags, net_amount);
    receiver.balance += net_amount;

    // 6. Update receiver's decay metadata if this is their first receipt
    if receiver.last_decay_block == 0 {
        receiver.last_decay_block = current_block;
        receiver.epoch_start_block = current_block;
    }

    Ok(AndTransferResult {
        fee,
        net_amount,
        fee_rate_bps: fee_rate,
        decay_applied,
    })
}

/// Try to apply AND-based decay to an account.
///
/// Returns true if decay was applied, false if rate-limited or epoch-capped.
fn try_apply_and_decay(
    account: &mut BlockAwareAccount,
    current_block: u64,
    config: &AndDecayConfig,
) -> bool {
    // Check if we're in a new epoch
    if current_block.saturating_sub(account.epoch_start_block) >= config.epoch_blocks {
        account.epoch_start_block = current_block;
        account.decays_this_epoch = 0;
    }

    // Check if decay would occur
    if !account.would_decay(current_block, config) {
        return false;
    }

    // Update decay tracking (actual tag decay happens in caller)
    account.last_decay_block = current_block;
    account.decays_this_epoch += 1;
    true
}

/// Mint new coins into a block-aware account, creating a new cluster.
///
/// Returns the newly created cluster ID.
pub fn mint_and(
    account: &mut BlockAwareAccount,
    amount: u64,
    cluster_id: ClusterId,
    current_block: u64,
    cluster_wealth: &mut ClusterWealth,
) -> ClusterId {
    // Mix the new cluster into the account's tags
    let new_tags = TagVector::single(cluster_id);
    account.tags.mix(account.balance, &new_tags, amount);
    account.balance += amount;

    // Initialize decay tracking if not set
    if account.last_decay_block == 0 {
        account.last_decay_block = current_block;
        account.epoch_start_block = current_block;
    }

    // Update cluster wealth
    cluster_wealth.apply_delta(cluster_id, amount as i64);

    cluster_id
}

/// Mint new coins into an account, creating a new cluster.
///
/// Returns the newly created cluster ID.
pub fn mint(
    account: &mut Account,
    amount: u64,
    cluster_id: ClusterId,
    cluster_wealth: &mut ClusterWealth,
) -> ClusterId {
    // Mix the new cluster into the account's tags
    let new_tags = TagVector::single(cluster_id);
    account.tags.mix(account.balance, &new_tags, amount);
    account.balance += amount;

    // Update cluster wealth
    cluster_wealth.apply_delta(cluster_id, amount as i64);

    cluster_id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test() -> (Account, Account, ClusterWealth, TransferConfig) {
        let cluster = ClusterId::new(1);
        let mut cluster_wealth = ClusterWealth::new();

        let sender = Account::with_minted_balance(1, 10_000, cluster);
        cluster_wealth.set(cluster, 10_000);

        let receiver = Account::new(2);
        let config = TransferConfig::default();

        (sender, receiver, cluster_wealth, config)
    }

    #[test]
    fn test_basic_transfer() {
        let (mut sender, mut receiver, mut cluster_wealth, config) = setup_test();

        let result = execute_transfer(
            &mut sender,
            &mut receiver,
            1000,
            &config,
            &mut cluster_wealth,
        )
        .unwrap();

        // Sender should have less
        assert!(sender.balance < 10_000);
        assert_eq!(sender.balance, 10_000 - 1000);

        // Receiver should have amount minus fee
        assert_eq!(receiver.balance, result.net_amount);
        // Note: with small cluster wealth (10k), fee rate is near minimum (~5 bps)
        // so fee on 1000 is ~0.5 which truncates to 0. This is expected behavior
        // for small clusters. See test_high_wealth_high_fee for high-fee scenario.
        assert!(result.net_amount <= 1000);
        assert_eq!(result.fee + result.net_amount, 1000);
    }

    #[test]
    fn test_insufficient_balance() {
        let (mut sender, mut receiver, mut cluster_wealth, config) = setup_test();

        let result = execute_transfer(
            &mut sender,
            &mut receiver,
            20_000, // More than sender has
            &config,
            &mut cluster_wealth,
        );

        assert!(matches!(
            result,
            Err(TransferError::InsufficientBalance { .. })
        ));
    }

    #[test]
    fn test_tag_inheritance() {
        let (mut sender, mut receiver, mut cluster_wealth, config) = setup_test();
        let cluster = ClusterId::new(1);

        // Transfer half
        execute_transfer(
            &mut sender,
            &mut receiver,
            5000,
            &config,
            &mut cluster_wealth,
        )
        .unwrap();

        // Receiver should now have attribution to cluster 1
        assert!(receiver.tags.get(cluster) > 0);

        // But it should be decayed from 100%
        assert!(receiver.tags.get(cluster) < TAG_WEIGHT_SCALE);
    }

    #[test]
    fn test_mint() {
        let mut account = Account::new(1);
        let mut cluster_wealth = ClusterWealth::new();
        let cluster = ClusterId::new(42);

        mint(&mut account, 1000, cluster, &mut cluster_wealth);

        assert_eq!(account.balance, 1000);
        assert_eq!(account.tags.get(cluster), TAG_WEIGHT_SCALE);
        assert_eq!(cluster_wealth.get(cluster), 1000);
    }

    #[test]
    fn test_high_wealth_high_fee() {
        let cluster = ClusterId::new(1);
        let mut cluster_wealth = ClusterWealth::new();

        // Create a very wealthy cluster
        cluster_wealth.set(cluster, 1_000_000_000); // 1 billion

        let sender = Account::with_minted_balance(1, 10_000, cluster);
        let mut receiver = Account::new(2);
        let config = TransferConfig::default();

        let mut sender_clone = sender.clone();
        let result = execute_transfer(
            &mut sender_clone,
            &mut receiver,
            1000,
            &config,
            &mut cluster_wealth,
        )
        .unwrap();

        // Fee should be substantial (near max rate)
        assert!(
            result.fee_rate_bps > 1000,
            "High wealth should yield high fee rate"
        );
    }

    // ========================================================================
    // AND-Based Transfer Tests
    // ========================================================================

    fn setup_and_test() -> (
        BlockAwareAccount,
        BlockAwareAccount,
        ClusterWealth,
        AndTransferConfig,
    ) {
        let cluster = ClusterId::new(1);
        let mut cluster_wealth = ClusterWealth::new();

        let sender = BlockAwareAccount::with_minted_balance(1, 10_000, cluster, 0);
        cluster_wealth.set(cluster, 10_000);

        let receiver = BlockAwareAccount::new(2);
        let config = AndTransferConfig::default();

        (sender, receiver, cluster_wealth, config)
    }

    #[test]
    fn test_and_basic_transfer() {
        let (mut sender, mut receiver, mut cluster_wealth, config) = setup_and_test();

        let result = execute_transfer_and(
            &mut sender,
            &mut receiver,
            1000,
            100, // current_block
            &config,
            &mut cluster_wealth,
        )
        .unwrap();

        // Sender should have less
        assert_eq!(sender.balance, 10_000 - 1000);

        // Receiver should have amount minus fee
        assert_eq!(receiver.balance, result.net_amount);
        assert_eq!(result.fee + result.net_amount, 1000);

        // First transfer should trigger decay
        assert!(result.decay_applied, "First transfer should trigger decay");
    }

    #[test]
    fn test_and_rapid_wash_trading_resistance() {
        let cluster = ClusterId::new(1);
        let mut cluster_wealth = ClusterWealth::new();
        cluster_wealth.set(cluster, 100_000);

        // Create config with 100-block rate limit
        let config = AndTransferConfig {
            fee_curve: FeeCurve::default_params(),
            decay_config: AndDecayConfig {
                decay_rate_per_hop: 50_000, // 5%
                min_blocks_between_decays: 100,
                max_decays_per_epoch: 24,
                epoch_blocks: 8_640,
            },
        };

        // Single sender attempting rapid sends to drain their own tags
        let mut sender = BlockAwareAccount::with_minted_balance(1, 1_000_000, cluster, 0);
        let mut receiver = BlockAwareAccount::new(2);

        // Attempt 10 rapid transfers within 50 blocks (all within rate limit)
        let mut decays_applied = 0;
        for i in 0..10 {
            let block = i * 5; // Every 5 blocks (much faster than 100-block rate limit)
            let result = execute_transfer_and(
                &mut sender,
                &mut receiver,
                1000,
                block,
                &config,
                &mut cluster_wealth,
            )
            .unwrap();

            if result.decay_applied {
                decays_applied += 1;
            }
        }

        // Only the first transfer should have triggered decay for this sender
        // (rate limit blocks subsequent decays within 100 blocks)
        assert_eq!(
            decays_applied, 1,
            "Rapid transfers from same sender should be blocked by rate limit"
        );
    }

    #[test]
    fn test_and_epoch_cap() {
        let cluster = ClusterId::new(1);
        let mut cluster_wealth = ClusterWealth::new();
        cluster_wealth.set(cluster, 100_000);

        // Create config with low rate limit but strict epoch cap
        let config = AndTransferConfig {
            fee_curve: FeeCurve::default_params(),
            decay_config: AndDecayConfig {
                decay_rate_per_hop: 50_000, // 5%
                min_blocks_between_decays: 10,
                max_decays_per_epoch: 3, // Only 3 decays per epoch
                epoch_blocks: 1000,
            },
        };

        // Single sender testing epoch cap
        let mut sender = BlockAwareAccount::with_minted_balance(1, 1_000_000, cluster, 0);
        let mut receiver = BlockAwareAccount::new(2);

        // Attempt transfers every 10 blocks (at the rate limit) within a single epoch
        let mut decays_applied = 0;
        for i in 0..10 {
            let block = i * 10; // All within epoch_blocks=1000
            let result = execute_transfer_and(
                &mut sender,
                &mut receiver,
                1000,
                block,
                &config,
                &mut cluster_wealth,
            )
            .unwrap();

            if result.decay_applied {
                decays_applied += 1;
            }
        }

        // Should be capped at 3 by epoch cap for this sender
        assert_eq!(
            decays_applied, 3,
            "Single sender's epoch cap should limit decays to 3"
        );
    }

    #[test]
    fn test_and_tag_inheritance() {
        let (mut sender, mut receiver, mut cluster_wealth, config) = setup_and_test();
        let cluster = ClusterId::new(1);

        // Transfer half
        let result = execute_transfer_and(
            &mut sender,
            &mut receiver,
            5000,
            100,
            &config,
            &mut cluster_wealth,
        )
        .unwrap();

        // Receiver should now have attribution to cluster 1
        assert!(receiver.tags.get(cluster) > 0);

        // If decay was applied, tag should be less than 100%
        if result.decay_applied {
            assert!(receiver.tags.get(cluster) < TAG_WEIGHT_SCALE);
        }
    }

    #[test]
    fn test_and_mint() {
        let mut account = BlockAwareAccount::new(1);
        let mut cluster_wealth = ClusterWealth::new();
        let cluster = ClusterId::new(42);

        mint_and(&mut account, 1000, cluster, 100, &mut cluster_wealth);

        assert_eq!(account.balance, 1000);
        assert_eq!(account.tags.get(cluster), TAG_WEIGHT_SCALE);
        assert_eq!(cluster_wealth.get(cluster), 1000);

        // Decay metadata should be initialized
        assert_eq!(account.last_decay_block, 100);
        assert_eq!(account.epoch_start_block, 100);
    }

    #[test]
    fn test_and_no_decay_when_holding() {
        let cluster = ClusterId::new(1);
        let config = AndTransferConfig::default();

        // Create account at block 0
        let account = BlockAwareAccount::with_minted_balance(1, 10_000, cluster, 0);

        // Check tag weight - should be 100% without any transfers
        assert_eq!(account.tags.get(cluster), TAG_WEIGHT_SCALE);

        // would_decay should return true for first transfer even after time passes
        // because we haven't done any transfers yet (first transfer is always eligible)
        assert!(
            account.would_decay(10000, &config.decay_config),
            "First transfer should be eligible even after long time"
        );
    }

    #[test]
    fn test_and_legitimate_trading_over_time() {
        let cluster = ClusterId::new(1);
        let mut cluster_wealth = ClusterWealth::new();
        cluster_wealth.set(cluster, 100_000);

        // Use anti-wash-trading config
        let config = AndTransferConfig::anti_wash_trading();

        // Single sender making daily transactions
        let mut sender = BlockAwareAccount::with_minted_balance(1, 1_000_000, cluster, 0);
        let mut receiver = BlockAwareAccount::new(2);

        // Simulate legitimate trading: 1 transaction per day for 7 days
        // At 10s blocks, 1 day ≈ 8640 blocks
        // Note: anti_wash_trading uses 720-block rate limit (~2 hours) and 12
        // decays/day cap
        let mut decays_applied = 0;
        for day in 0..7 {
            let block = day * 8_640 + 1000; // Mid-day transaction
            let result = execute_transfer_and(
                &mut sender,
                &mut receiver,
                1000,
                block,
                &config,
                &mut cluster_wealth,
            )
            .unwrap();

            if result.decay_applied {
                decays_applied += 1;
            }
        }

        // Should get 7 decays (one per day, well under rate limit and epoch cap)
        assert_eq!(
            decays_applied, 7,
            "Legitimate daily trading should get 1 decay per day"
        );
    }

    #[test]
    fn test_and_from_legacy_conversion() {
        let cluster = ClusterId::new(1);
        let legacy = Account::with_minted_balance(1, 10_000, cluster);

        let block_aware = BlockAwareAccount::from_legacy(legacy);

        assert_eq!(block_aware.id, 1);
        assert_eq!(block_aware.balance, 10_000);
        assert_eq!(block_aware.tags.get(cluster), TAG_WEIGHT_SCALE);
        assert_eq!(block_aware.last_decay_block, 0);
        assert_eq!(block_aware.decays_this_epoch, 0);
    }
}
