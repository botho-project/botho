//! Whale agent: Large holder that may attempt fee minimization strategies.

use crate::simulation::agent::{Action, Agent, AgentId};
use crate::simulation::state::SimulationState;
use crate::tag::TagVector;
use crate::Account;

/// Fee minimization strategy for whales.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhaleStrategy {
    /// Just hold and make necessary transfers (passive).
    Passive,
    /// Attempt wash trading to reduce tags.
    WashTrading,
    /// Use mixers when available.
    UseMixers,
    /// Split transactions to attempt structuring.
    Structuring,
    /// Combine multiple strategies.
    Aggressive,
}

/// Large holder agent that may attempt to minimize fees.
#[derive(Debug)]
pub struct WhaleAgent {
    account: Account,
    strategy: WhaleStrategy,
    /// Target recipients for regular spending.
    spending_targets: Vec<AgentId>,
    /// Fraction of wealth to spend per round (0.0 to 1.0).
    spending_rate: f64,
    /// Wash trade hops when using that strategy.
    wash_hops: u32,
    /// Track total fees paid.
    total_fees_paid: u64,
    /// Track total amount sent.
    total_sent: u64,
    /// RNG seed for determinism.
    rng_state: u64,
}

impl WhaleAgent {
    /// Create a new whale agent.
    pub fn new(id: AgentId, _initial_balance: u64, strategy: WhaleStrategy) -> Self {
        Self {
            account: Account::new(id.0),
            strategy,
            spending_targets: Vec::new(),
            spending_rate: 0.001, // 0.1% of wealth per round
            wash_hops: 10,
            total_fees_paid: 0,
            total_sent: 0,
            rng_state: id.0,
        }
    }

    /// Set the list of agents this whale sends to.
    pub fn with_spending_targets(mut self, targets: Vec<AgentId>) -> Self {
        self.spending_targets = targets;
        self
    }

    /// Set spending rate (fraction per round).
    pub fn with_spending_rate(mut self, rate: f64) -> Self {
        self.spending_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Set wash trade hops.
    pub fn with_wash_hops(mut self, hops: u32) -> Self {
        self.wash_hops = hops;
        self
    }

    /// Simple pseudo-RNG for deterministic behavior.
    fn next_random(&mut self) -> u64 {
        // xorshift64
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        self.rng_state
    }

    /// Get effective fee rate stats.
    pub fn fee_stats(&self) -> (u64, u64, f64) {
        let avg_rate = if self.total_sent > 0 {
            self.total_fees_paid as f64 / self.total_sent as f64 * 10_000.0
        } else {
            0.0
        };
        (self.total_fees_paid, self.total_sent, avg_rate)
    }

    /// Set the account (for minting initial balance).
    pub fn set_account(&mut self, account: Account) {
        self.account = account;
    }

    /// Get mutable account reference (for external minting).
    pub fn account_mut_ref(&mut self) -> &mut Account {
        &mut self.account
    }
}

impl Agent for WhaleAgent {
    fn id(&self) -> AgentId {
        AgentId(self.account.id)
    }

    fn balance(&self) -> u64 {
        self.account.balance
    }

    fn tags(&self) -> &TagVector {
        &self.account.tags
    }

    fn account_mut(&mut self) -> &mut Account {
        &mut self.account
    }

    fn decide_action(&mut self, state: &SimulationState) -> Option<Action> {
        if self.account.balance == 0 {
            return None;
        }

        match self.strategy {
            WhaleStrategy::Passive => {
                // Just make occasional transfers to spending targets
                if self.spending_targets.is_empty() {
                    return Some(Action::Hold);
                }

                let amount = (self.account.balance as f64 * self.spending_rate) as u64;
                if amount < 100 {
                    return Some(Action::Hold);
                }

                let target_idx = (self.next_random() as usize) % self.spending_targets.len();
                Some(Action::Transfer {
                    to: self.spending_targets[target_idx],
                    amount,
                })
            }

            WhaleStrategy::WashTrading => {
                // Every 10 rounds, attempt wash trading
                if state.round % 10 == 0 {
                    let amount = (self.account.balance as f64 * 0.1) as u64;
                    if amount > 1000 {
                        return Some(Action::WashTrade {
                            amount,
                            hops: self.wash_hops,
                        });
                    }
                }

                // Otherwise, normal spending
                if !self.spending_targets.is_empty() {
                    let amount = (self.account.balance as f64 * self.spending_rate) as u64;
                    if amount >= 100 {
                        let target_idx = (self.next_random() as usize) % self.spending_targets.len();
                        return Some(Action::Transfer {
                            to: self.spending_targets[target_idx],
                            amount,
                        });
                    }
                }

                Some(Action::Hold)
            }

            WhaleStrategy::UseMixers => {
                // Use mixer if available, every 5 rounds
                if state.round % 5 == 0 {
                    if let Some(mixer_id) = state.mixer_ids.first() {
                        let amount = (self.account.balance as f64 * 0.05) as u64;
                        if amount > 1000 {
                            return Some(Action::UseMixer {
                                mixer_id: *mixer_id,
                                amount,
                            });
                        }
                    }
                }

                // Normal spending
                if !self.spending_targets.is_empty() {
                    let amount = (self.account.balance as f64 * self.spending_rate) as u64;
                    if amount >= 100 {
                        let target_idx = (self.next_random() as usize) % self.spending_targets.len();
                        return Some(Action::Transfer {
                            to: self.spending_targets[target_idx],
                            amount,
                        });
                    }
                }

                Some(Action::Hold)
            }

            WhaleStrategy::Structuring => {
                // Split transfers into smaller pieces
                if !self.spending_targets.is_empty() {
                    let total_amount = (self.account.balance as f64 * self.spending_rate) as u64;
                    if total_amount >= 1000 {
                        // Split into 10 smaller transfers
                        let num_splits = 10.min(self.spending_targets.len());
                        let per_transfer = total_amount / num_splits as u64;

                        let transfers: Vec<_> = self
                            .spending_targets
                            .iter()
                            .take(num_splits)
                            .map(|&to| (to, per_transfer))
                            .collect();

                        return Some(Action::BatchTransfer { transfers });
                    }
                }

                Some(Action::Hold)
            }

            WhaleStrategy::Aggressive => {
                // Combine strategies based on round
                match state.round % 20 {
                    0..=4 => {
                        // Use mixer
                        if let Some(mixer_id) = state.mixer_ids.first() {
                            let amount = (self.account.balance as f64 * 0.05) as u64;
                            if amount > 1000 {
                                return Some(Action::UseMixer {
                                    mixer_id: *mixer_id,
                                    amount,
                                });
                            }
                        }
                        Some(Action::Hold)
                    }
                    5..=9 => {
                        // Wash trade
                        let amount = (self.account.balance as f64 * 0.1) as u64;
                        if amount > 1000 {
                            Some(Action::WashTrade {
                                amount,
                                hops: self.wash_hops,
                            })
                        } else {
                            Some(Action::Hold)
                        }
                    }
                    _ => {
                        // Normal structuring
                        if !self.spending_targets.is_empty() {
                            let total = (self.account.balance as f64 * self.spending_rate) as u64;
                            if total >= 500 {
                                let num_splits = 5.min(self.spending_targets.len());
                                let per_transfer = total / num_splits as u64;
                                let transfers: Vec<_> = self
                                    .spending_targets
                                    .iter()
                                    .take(num_splits)
                                    .map(|&to| (to, per_transfer))
                                    .collect();
                                return Some(Action::BatchTransfer { transfers });
                            }
                        }
                        Some(Action::Hold)
                    }
                }
            }
        }
    }

    fn on_receive_payment(&mut self, _amount: u64, _from: AgentId) {
        // Whales just accumulate
    }

    fn agent_type(&self) -> &'static str {
        "Whale"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeeCurve, TransferConfig};

    #[test]
    fn test_whale_passive_strategy() {
        let targets = vec![AgentId(100), AgentId(101)];
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::Passive)
            .with_spending_targets(targets)
            .with_spending_rate(0.01);

        whale.account.balance = 1_000_000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::Transfer { .. })));
    }

    #[test]
    fn test_whale_wash_trading() {
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::WashTrading);
        whale.account.balance = 1_000_000;

        let mut state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        state.round = 10; // Trigger wash trade

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::WashTrade { .. })));
    }

    #[test]
    fn test_whale_zero_balance() {
        let mut whale = WhaleAgent::new(AgentId(1), 0, WhaleStrategy::Passive);
        whale.account.balance = 0;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());

        let action = whale.decide_action(&state);
        assert!(action.is_none(), "Zero balance should return None");
    }

    #[test]
    fn test_whale_no_spending_targets() {
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::Passive);
        whale.account.balance = 1_000_000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::Hold)));
    }

    #[test]
    fn test_whale_fee_stats() {
        let whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::Passive);

        let (fees, sent, rate) = whale.fee_stats();
        assert_eq!(fees, 0);
        assert_eq!(sent, 0);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_whale_builder_methods() {
        let whale = WhaleAgent::new(AgentId(1), 100, WhaleStrategy::Passive)
            .with_spending_targets(vec![AgentId(2), AgentId(3)])
            .with_spending_rate(0.5)
            .with_wash_hops(20);

        assert_eq!(whale.spending_targets.len(), 2);
        assert_eq!(whale.spending_rate, 0.5);
        assert_eq!(whale.wash_hops, 20);
    }

    #[test]
    fn test_whale_spending_rate_clamped() {
        let whale1 = WhaleAgent::new(AgentId(1), 100, WhaleStrategy::Passive)
            .with_spending_rate(2.0); // Above 1.0
        assert_eq!(whale1.spending_rate, 1.0);

        let whale2 = WhaleAgent::new(AgentId(1), 100, WhaleStrategy::Passive)
            .with_spending_rate(-0.5); // Below 0.0
        assert_eq!(whale2.spending_rate, 0.0);
    }

    #[test]
    fn test_whale_structuring_strategy() {
        let targets = vec![AgentId(100), AgentId(101), AgentId(102)];
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::Structuring)
            .with_spending_targets(targets)
            .with_spending_rate(0.01);

        whale.account.balance = 1_000_000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::BatchTransfer { .. })));
    }

    #[test]
    fn test_whale_use_mixers_no_mixer() {
        let targets = vec![AgentId(100)];
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::UseMixers)
            .with_spending_targets(targets)
            .with_spending_rate(0.01);

        whale.account.balance = 1_000_000;

        let mut state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        state.round = 5; // Trigger mixer check

        // No mixers registered, should fall back to transfer
        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::Transfer { .. })));
    }

    #[test]
    fn test_whale_use_mixers_with_mixer() {
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::UseMixers);
        whale.account.balance = 1_000_000;

        let mut state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        state.round = 5;
        state.mixer_ids.push(AgentId(999)); // Register a mixer

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::UseMixer { .. })));
    }

    #[test]
    fn test_whale_aggressive_wash_trade_round() {
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::Aggressive);
        whale.account.balance = 1_000_000;

        let mut state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        state.round = 5; // Rounds 5-9 trigger wash trade

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::WashTrade { .. })));
    }

    #[test]
    fn test_whale_aggressive_structuring_round() {
        let targets = vec![AgentId(100), AgentId(101)];
        let mut whale = WhaleAgent::new(AgentId(1), 1_000_000, WhaleStrategy::Aggressive)
            .with_spending_targets(targets)
            .with_spending_rate(0.01);

        whale.account.balance = 1_000_000;

        let mut state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        state.round = 15; // Rounds 10+ trigger structuring

        let action = whale.decide_action(&state);
        assert!(matches!(action, Some(Action::BatchTransfer { .. })));
    }

    #[test]
    fn test_whale_agent_type() {
        let whale = WhaleAgent::new(AgentId(1), 1000, WhaleStrategy::Passive);
        assert_eq!(whale.agent_type(), "Whale");
    }

    #[test]
    fn test_whale_account_access() {
        let mut whale = WhaleAgent::new(AgentId(5), 1000, WhaleStrategy::Passive);
        whale.account.balance = 500;

        assert_eq!(whale.id(), AgentId(5));
        assert_eq!(whale.balance(), 500);

        // Test mutable access
        whale.account_mut().balance = 1000;
        assert_eq!(whale.balance(), 1000);
    }

    #[test]
    fn test_whale_rng_determinism() {
        let mut whale1 = WhaleAgent::new(AgentId(42), 1000, WhaleStrategy::Passive);
        let mut whale2 = WhaleAgent::new(AgentId(42), 1000, WhaleStrategy::Passive);

        // Same seed should produce same sequence
        let r1 = whale1.next_random();
        let r2 = whale2.next_random();
        assert_eq!(r1, r2);

        // Sequence continues to match
        let r1_2 = whale1.next_random();
        let r2_2 = whale2.next_random();
        assert_eq!(r1_2, r2_2);
    }

    #[test]
    fn test_whale_strategy_equality() {
        assert_eq!(WhaleStrategy::Passive, WhaleStrategy::Passive);
        assert_ne!(WhaleStrategy::Passive, WhaleStrategy::WashTrading);
        assert_ne!(WhaleStrategy::UseMixers, WhaleStrategy::Structuring);
    }

    #[test]
    fn test_whale_small_balance_holds() {
        let targets = vec![AgentId(100)];
        let mut whale = WhaleAgent::new(AgentId(1), 100, WhaleStrategy::Passive)
            .with_spending_targets(targets)
            .with_spending_rate(0.001); // 0.1% of 100 = 0.1, less than threshold

        whale.account.balance = 100;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());

        let action = whale.decide_action(&state);
        // Amount < 100, so should hold
        assert!(matches!(action, Some(Action::Hold)));
    }
}
