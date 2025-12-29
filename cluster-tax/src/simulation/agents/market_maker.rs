//! Market maker agent: High velocity, many small trades, seeks low-tag coins.

use crate::simulation::agent::{Action, Agent, AgentId};
use crate::simulation::state::SimulationState;
use crate::tag::TagVector;
use crate::Account;

/// Market maker with high transaction velocity.
#[derive(Debug)]
pub struct MarketMakerAgent {
    account: Account,
    /// Counterparties to trade with.
    counterparties: Vec<AgentId>,
    /// Number of trades per round.
    trades_per_round: usize,
    /// Average trade size.
    avg_trade_size: u64,
    /// Total volume traded.
    total_volume: u64,
    /// Number of trades executed.
    trade_count: u64,
    /// RNG state.
    rng_state: u64,
    /// Current trade index within round.
    current_trade_index: usize,
}

impl MarketMakerAgent {
    /// Create a new market maker.
    pub fn new(id: AgentId) -> Self {
        Self {
            account: Account::new(id.0),
            counterparties: Vec::new(),
            trades_per_round: 5,
            avg_trade_size: 500,
            total_volume: 0,
            trade_count: 0,
            rng_state: id.0 * 12345,
            current_trade_index: 0,
        }
    }

    /// Set counterparties.
    pub fn with_counterparties(mut self, counterparties: Vec<AgentId>) -> Self {
        self.counterparties = counterparties;
        self
    }

    /// Set trades per round.
    pub fn with_trades_per_round(mut self, n: usize) -> Self {
        self.trades_per_round = n;
        self
    }

    /// Set average trade size.
    pub fn with_avg_trade_size(mut self, size: u64) -> Self {
        self.avg_trade_size = size;
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

    /// Get stats.
    pub fn stats(&self) -> (u64, u64) {
        (self.total_volume, self.trade_count)
    }

    /// Reset trade index for new round.
    pub fn new_round(&mut self) {
        self.current_trade_index = 0;
    }
}

impl Agent for MarketMakerAgent {
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
        if self.counterparties.is_empty() || self.account.balance < self.avg_trade_size {
            return Some(Action::Hold);
        }

        // Execute multiple trades per round
        if self.current_trade_index >= self.trades_per_round {
            return Some(Action::Hold);
        }

        let rand = self.next_random();
        let counterparty_idx = (rand as usize) % self.counterparties.len();

        // Random trade size around average
        let size_variance = (rand % 100) as f64 / 100.0;
        let trade_size = ((self.avg_trade_size as f64) * (0.5 + size_variance)) as u64;
        let amount = trade_size.min(self.account.balance / 2);

        if amount < 10 {
            return Some(Action::Hold);
        }

        self.current_trade_index += 1;
        self.total_volume += amount;
        self.trade_count += 1;

        Some(Action::Transfer {
            to: self.counterparties[counterparty_idx],
            amount,
        })
    }

    fn on_receive_payment(&mut self, amount: u64, _from: AgentId) {
        self.total_volume += amount;
    }

    fn agent_type(&self) -> &'static str {
        "MarketMaker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeeCurve, TransferConfig};

    #[test]
    fn test_market_maker_trades() {
        let counterparties = vec![AgentId(100), AgentId(101)];
        let mut mm = MarketMakerAgent::new(AgentId(1))
            .with_counterparties(counterparties)
            .with_trades_per_round(3);

        mm.account.balance = 10000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());

        // Should execute multiple trades
        let action1 = mm.decide_action(&state);
        assert!(matches!(action1, Some(Action::Transfer { .. })));

        let action2 = mm.decide_action(&state);
        assert!(matches!(action2, Some(Action::Transfer { .. })));

        let action3 = mm.decide_action(&state);
        assert!(matches!(action3, Some(Action::Transfer { .. })));

        // Fourth should hold (exceeded trades per round)
        let action4 = mm.decide_action(&state);
        assert!(matches!(action4, Some(Action::Hold)));
    }
}
