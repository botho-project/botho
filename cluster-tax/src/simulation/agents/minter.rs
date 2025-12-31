//! Minter agent: Receives block rewards, sells coins for goods/services.

use crate::{
    simulation::{
        agent::{Action, Agent, AgentId},
        state::SimulationState,
    },
    tag::TagVector,
    Account,
};

/// Minter that receives fresh coin rewards and sells them.
#[derive(Debug)]
pub struct MinterAgent {
    account: Account,
    /// Agents to sell coins to (merchants, exchanges, etc.).
    buyers: Vec<AgentId>,
    /// Block reward per "minting" round.
    block_reward: u64,
    /// Rounds between minting rewards.
    minting_interval: u64,
    /// Fraction of balance to sell each round.
    sell_fraction: f64,
    /// Total coins mined.
    total_mined: u64,
    /// Total coins sold.
    total_sold: u64,
    /// RNG state.
    rng_state: u64,
}

impl MinterAgent {
    /// Create a new minter.
    pub fn new(id: AgentId) -> Self {
        Self {
            account: Account::new(id.0),
            buyers: Vec::new(),
            block_reward: 1000,
            minting_interval: 10,
            sell_fraction: 0.2,
            total_mined: 0,
            total_sold: 0,
            rng_state: id.0 * 54321,
        }
    }

    /// Set buyers.
    pub fn with_buyers(mut self, buyers: Vec<AgentId>) -> Self {
        self.buyers = buyers;
        self
    }

    /// Set block reward.
    pub fn with_block_reward(mut self, reward: u64) -> Self {
        self.block_reward = reward;
        self
    }

    /// Set minting interval.
    pub fn with_minting_interval(mut self, interval: u64) -> Self {
        self.minting_interval = interval.max(1);
        self
    }

    /// Set sell fraction.
    pub fn with_sell_fraction(mut self, fraction: f64) -> Self {
        self.sell_fraction = fraction.clamp(0.0, 1.0);
        self
    }

    /// Record minting reward (called externally).
    pub fn record_minting(&mut self, amount: u64) {
        self.total_mined += amount;
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
        (self.total_mined, self.total_sold)
    }

    /// Check if this round is a minting round.
    pub fn is_minting_round(&self, round: u64) -> bool {
        round % self.minting_interval == 0
    }

    /// Get block reward amount.
    pub fn reward_amount(&self) -> u64 {
        self.block_reward
    }
}

impl Agent for MinterAgent {
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
        // Sell coins to buyers
        if self.buyers.is_empty() || self.account.balance < 100 {
            return Some(Action::Hold);
        }

        let sell_amount = (self.account.balance as f64 * self.sell_fraction) as u64;
        if sell_amount < 50 {
            return Some(Action::Hold);
        }

        let buyer_idx = (self.next_random() as usize) % self.buyers.len();
        self.total_sold += sell_amount;

        Some(Action::Transfer {
            to: self.buyers[buyer_idx],
            amount: sell_amount,
        })
    }

    fn on_receive_payment(&mut self, _amount: u64, _from: AgentId) {
        // Minters typically don't receive payments (only block rewards)
    }

    fn agent_type(&self) -> &'static str {
        "Minter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FeeCurve, TransferConfig};

    #[test]
    fn test_minter_sells() {
        let buyers = vec![AgentId(100), AgentId(101)];
        let mut minter = MinterAgent::new(AgentId(1))
            .with_buyers(buyers)
            .with_sell_fraction(0.5);

        minter.account.balance = 1000;

        let state = SimulationState::new(1000, FeeCurve::default(), TransferConfig::default());
        let action = minter.decide_action(&state);

        match action {
            Some(Action::Transfer { amount, .. }) => {
                assert_eq!(amount, 500); // 50% of 1000
            }
            _ => panic!("Expected transfer"),
        }
    }

    #[test]
    fn test_minting_interval() {
        let minter = MinterAgent::new(AgentId(1)).with_minting_interval(10);

        assert!(minter.is_minting_round(0));
        assert!(!minter.is_minting_round(5));
        assert!(minter.is_minting_round(10));
        assert!(minter.is_minting_round(20));
    }
}
