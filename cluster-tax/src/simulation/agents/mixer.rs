//! Mixer service agent: Accepts deposits, returns coins from pooled reserves.

use crate::{
    simulation::{
        agent::{Action, Agent, AgentId},
        state::SimulationState,
    },
    tag::TagVector,
    Account,
};
use std::collections::VecDeque;

/// Pending withdrawal request.
#[derive(Debug, Clone)]
struct WithdrawalRequest {
    to: AgentId,
    amount: u64,
    delay_rounds: u64,
}

/// Mixer service that pools coins to break tag linkage.
#[derive(Debug)]
pub struct MixerServiceAgent {
    account: Account,
    /// Fee charged by mixer (basis points).
    fee_bps: u32,
    /// Minimum deposit amount.
    min_deposit: u64,
    /// Pending withdrawals.
    pending_withdrawals: VecDeque<WithdrawalRequest>,
    /// Delay before withdrawal (in rounds).
    withdrawal_delay: u64,
    /// Total deposits received.
    total_deposits: u64,
    /// Total withdrawals processed.
    total_withdrawals: u64,
    /// Total fees collected.
    total_fees: u64,
    /// Current round (updated externally).
    current_round: u64,
}

impl MixerServiceAgent {
    /// Create a new mixer service.
    pub fn new(id: AgentId) -> Self {
        Self {
            account: Account::new(id.0),
            fee_bps: 100, // 1% default fee
            min_deposit: 100,
            pending_withdrawals: VecDeque::new(),
            withdrawal_delay: 5,
            total_deposits: 0,
            total_withdrawals: 0,
            total_fees: 0,
            current_round: 0,
        }
    }

    /// Set mixer fee.
    pub fn with_fee_bps(mut self, fee: u32) -> Self {
        self.fee_bps = fee;
        self
    }

    /// Set minimum deposit.
    pub fn with_min_deposit(mut self, amount: u64) -> Self {
        self.min_deposit = amount;
        self
    }

    /// Set withdrawal delay.
    pub fn with_withdrawal_delay(mut self, delay: u64) -> Self {
        self.withdrawal_delay = delay;
        self
    }

    /// Queue a withdrawal request.
    pub fn queue_withdrawal(&mut self, to: AgentId, amount: u64, current_round: u64) {
        self.pending_withdrawals.push_back(WithdrawalRequest {
            to,
            amount,
            delay_rounds: current_round + self.withdrawal_delay,
        });
    }

    /// Update current round.
    pub fn set_round(&mut self, round: u64) {
        self.current_round = round;
    }

    /// Get mutable account reference.
    pub fn account_mut_ref(&mut self) -> &mut Account {
        &mut self.account
    }

    /// Get stats.
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.total_deposits, self.total_withdrawals, self.total_fees)
    }

    /// Get number of pending withdrawals.
    pub fn pending_count(&self) -> usize {
        self.pending_withdrawals.len()
    }
}

impl Agent for MixerServiceAgent {
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
        self.current_round = state.round;

        // Process pending withdrawals that are due
        while let Some(front) = self.pending_withdrawals.front() {
            if front.delay_rounds <= state.round {
                let request = self.pending_withdrawals.pop_front().unwrap();

                // Check if we have enough funds
                if self.account.balance >= request.amount {
                    self.total_withdrawals += request.amount;
                    return Some(Action::Transfer {
                        to: request.to,
                        amount: request.amount,
                    });
                } else {
                    // Not enough funds, re-queue with longer delay
                    self.pending_withdrawals.push_back(WithdrawalRequest {
                        delay_rounds: state.round + 1,
                        ..request
                    });
                }
            } else {
                break;
            }
        }

        Some(Action::Hold)
    }

    fn on_receive_payment(&mut self, amount: u64, from: AgentId) {
        // Calculate fee
        let fee = (amount as u128 * self.fee_bps as u128 / 10_000) as u64;
        let net_amount = amount.saturating_sub(fee);

        self.total_deposits += amount;
        self.total_fees += fee;

        // Queue withdrawal of net amount back to sender
        if net_amount >= self.min_deposit {
            self.queue_withdrawal(from, net_amount, self.current_round);
        }
    }

    fn agent_type(&self) -> &'static str {
        "Mixer"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeeCurve, TransferConfig};

    #[test]
    fn test_mixer_fee_collection() {
        let mut mixer = MixerServiceAgent::new(AgentId(1)).with_fee_bps(100); // 1%

        mixer.on_receive_payment(1000, AgentId(10));

        assert_eq!(mixer.total_deposits, 1000);
        assert_eq!(mixer.total_fees, 10); // 1% of 1000
        assert_eq!(mixer.pending_count(), 1);
    }

    #[test]
    fn test_mixer_withdrawal_delay() {
        let mut mixer = MixerServiceAgent::new(AgentId(1)).with_withdrawal_delay(5);

        mixer.account.balance = 10000;
        mixer.queue_withdrawal(AgentId(10), 500, 0);

        // Round 0: not due yet
        let mut state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        let action = mixer.decide_action(&state);
        assert!(matches!(action, Some(Action::Hold)));

        // Round 5: should process
        state.round = 5;
        let action = mixer.decide_action(&state);
        assert!(matches!(action, Some(Action::Transfer { amount: 500, .. })));
    }
}
