//! Transfer logic with tag inheritance and progressive fees.

use crate::{
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

    /// Compute effective fee rate based on this account's tags and cluster wealths.
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
    receiver.tags.mix(receiver_balance_before, &transferred_tags, net_amount);
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
            TransferError::InsufficientBalance { available, requested } => {
                write!(f, "Insufficient balance: have {available}, need {requested}")
            }
        }
    }
}

impl std::error::Error for TransferError {}

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
        ).unwrap();

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

        assert!(matches!(result, Err(TransferError::InsufficientBalance { .. })));
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
        ).unwrap();

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
        ).unwrap();

        // Fee should be substantial (near max rate)
        assert!(result.fee_rate_bps > 1000, "High wealth should yield high fee rate");
    }
}
