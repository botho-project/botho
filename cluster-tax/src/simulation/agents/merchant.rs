//! Merchant agent: Receives many small payments, makes occasional large payments.

use crate::simulation::agent::{Action, Agent, AgentId};
use crate::simulation::state::SimulationState;
use crate::tag::TagVector;
use crate::Account;

/// Merchant agent that receives frequent small payments and pays suppliers.
#[derive(Debug)]
pub struct MerchantAgent {
    account: Account,
    /// Supplier agent IDs to pay.
    suppliers: Vec<AgentId>,
    /// Payment threshold: pay suppliers when balance exceeds this.
    payment_threshold: u64,
    /// Fraction of balance to pay suppliers.
    supplier_payment_fraction: f64,
    /// Track revenue received.
    total_revenue: u64,
    /// Track payments made.
    total_payments: u64,
    /// Number of payments received.
    payment_count: u64,
    /// RNG state.
    rng_state: u64,
}

impl MerchantAgent {
    /// Create a new merchant agent.
    pub fn new(id: AgentId) -> Self {
        Self {
            account: Account::new(id.0),
            suppliers: Vec::new(),
            payment_threshold: 10_000,
            supplier_payment_fraction: 0.5,
            total_revenue: 0,
            total_payments: 0,
            payment_count: 0,
            rng_state: id.0,
        }
    }

    /// Set suppliers.
    pub fn with_suppliers(mut self, suppliers: Vec<AgentId>) -> Self {
        self.suppliers = suppliers;
        self
    }

    /// Set payment threshold.
    pub fn with_payment_threshold(mut self, threshold: u64) -> Self {
        self.payment_threshold = threshold;
        self
    }

    /// Set supplier payment fraction.
    pub fn with_supplier_payment_fraction(mut self, fraction: f64) -> Self {
        self.supplier_payment_fraction = fraction.clamp(0.0, 1.0);
        self
    }

    /// Simple pseudo-RNG.
    fn next_random(&mut self) -> u64 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        self.rng_state
    }

    /// Get mutable account reference.
    pub fn account_mut_ref(&mut self) -> &mut Account {
        &mut self.account
    }

    /// Get merchant stats.
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.total_revenue, self.total_payments, self.payment_count)
    }
}

impl Agent for MerchantAgent {
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
        // Pay suppliers when balance exceeds threshold
        if self.account.balance > self.payment_threshold && !self.suppliers.is_empty() {
            let payment_amount =
                (self.account.balance as f64 * self.supplier_payment_fraction) as u64;

            if payment_amount > 100 {
                // Pick a supplier
                let supplier_idx = (self.next_random() as usize) % self.suppliers.len();
                return Some(Action::Transfer {
                    to: self.suppliers[supplier_idx],
                    amount: payment_amount,
                });
            }
        }

        Some(Action::Hold)
    }

    fn on_receive_payment(&mut self, amount: u64, _from: AgentId) {
        self.total_revenue += amount;
        self.payment_count += 1;
    }

    fn agent_type(&self) -> &'static str {
        "Merchant"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeeCurve, TransferConfig};

    #[test]
    fn test_merchant_pays_supplier() {
        let suppliers = vec![AgentId(100)];
        let mut merchant = MerchantAgent::new(AgentId(1))
            .with_suppliers(suppliers)
            .with_payment_threshold(1000);

        merchant.account.balance = 5000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        let action = merchant.decide_action(&state);

        assert!(matches!(action, Some(Action::Transfer { .. })));
    }

    #[test]
    fn test_merchant_holds_below_threshold() {
        let suppliers = vec![AgentId(100)];
        let mut merchant = MerchantAgent::new(AgentId(1))
            .with_suppliers(suppliers)
            .with_payment_threshold(10000);

        merchant.account.balance = 500;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        let action = merchant.decide_action(&state);

        assert!(matches!(action, Some(Action::Hold)));
    }
}
