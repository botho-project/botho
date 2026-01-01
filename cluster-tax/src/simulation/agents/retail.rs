//! Retail user agent: Small holder with occasional transactions.

use crate::{
    simulation::{
        agent::{Action, Agent, AgentId},
        state::SimulationState,
    },
    tag::TagVector,
    Account,
};

/// Retail user with small balance and occasional spending.
#[derive(Debug)]
pub struct RetailUserAgent {
    account: Account,
    /// Merchants to spend at.
    merchants: Vec<AgentId>,
    /// Probability of spending each round (0.0 to 1.0).
    spending_probability: f64,
    /// Average spend amount.
    avg_spend: u64,
    /// Track spending.
    total_spent: u64,
    /// Track income.
    total_income: u64,
    /// RNG state.
    rng_state: u64,
}

impl RetailUserAgent {
    /// Create a new retail user.
    pub fn new(id: AgentId) -> Self {
        Self {
            account: Account::new(id.0),
            merchants: Vec::new(),
            spending_probability: 0.1, // 10% chance of spending each round
            avg_spend: 100,
            total_spent: 0,
            total_income: 0,
            rng_state: id.0 * 31337, // Different seed per agent
        }
    }

    /// Set merchants to spend at.
    pub fn with_merchants(mut self, merchants: Vec<AgentId>) -> Self {
        self.merchants = merchants;
        self
    }

    /// Set spending probability.
    pub fn with_spending_probability(mut self, prob: f64) -> Self {
        self.spending_probability = prob.clamp(0.0, 1.0);
        self
    }

    /// Set average spend amount.
    pub fn with_avg_spend(mut self, amount: u64) -> Self {
        self.avg_spend = amount;
        self
    }

    /// Simple pseudo-RNG returning 0.0 to 1.0.
    fn random_float(&mut self) -> f64 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        (self.rng_state as f64) / (u64::MAX as f64)
    }

    /// Get random amount around average.
    fn random_amount(&mut self) -> u64 {
        let variance = self.random_float();
        let multiplier = 0.5 + variance; // 0.5x to 1.5x
        (self.avg_spend as f64 * multiplier) as u64
    }

    /// Get mutable account reference.
    pub fn account_mut_ref(&mut self) -> &mut Account {
        &mut self.account
    }

    /// Get stats.
    pub fn stats(&self) -> (u64, u64) {
        (self.total_spent, self.total_income)
    }
}

impl Agent for RetailUserAgent {
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

    fn decide_action(&mut self, _state: &SimulationState) -> Option<Action> {
        // Random chance to spend
        if self.random_float() > self.spending_probability {
            return Some(Action::Hold);
        }

        if self.merchants.is_empty() || self.account.balance < 10 {
            return Some(Action::Hold);
        }

        let amount = self.random_amount().min(self.account.balance);
        if amount < 10 {
            return Some(Action::Hold);
        }

        let merchant_idx = (self.rng_state as usize) % self.merchants.len();
        self.total_spent += amount;

        Some(Action::Transfer {
            to: self.merchants[merchant_idx],
            amount,
        })
    }

    fn on_receive_payment(&mut self, amount: u64, _from: AgentId) {
        self.total_income += amount;
    }

    fn agent_type(&self) -> &'static str {
        "Retail"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeeCurve, TransferConfig};

    #[test]
    fn test_retail_spending() {
        let merchants = vec![AgentId(100), AgentId(101)];
        let mut retail = RetailUserAgent::new(AgentId(1))
            .with_merchants(merchants)
            .with_spending_probability(1.0); // Always spend for test

        retail.account.balance = 1000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        let action = retail.decide_action(&state);

        assert!(matches!(action, Some(Action::Transfer { .. })));
    }
}
