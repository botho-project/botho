//! Core agent trait and action types.

use crate::tag::TagVector;

/// Unique identifier for an agent in the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AgentId(pub u64);

impl AgentId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

impl From<u64> for AgentId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

/// Actions an agent can take during a simulation step.
#[derive(Clone, Debug)]
pub enum Action {
    /// Transfer coins to another agent.
    Transfer {
        to: AgentId,
        amount: u64,
    },

    /// Hold coins (do nothing this round).
    Hold,

    /// Use a mixer service to diffuse tags.
    UseMixer {
        mixer_id: AgentId,
        amount: u64,
    },

    /// Perform a wash trade (circular transfer through Sybils).
    WashTrade {
        amount: u64,
        hops: u32,
    },

    /// Perform multiple transfers (e.g., batch payments).
    BatchTransfer {
        transfers: Vec<(AgentId, u64)>,
    },
}

/// Result of an agent's action execution.
#[derive(Clone, Debug)]
pub struct ActionResult {
    /// Total fees paid.
    pub fees_paid: u64,

    /// Net amount transferred (after fees).
    pub net_transferred: u64,

    /// Whether the action was successful.
    pub success: bool,

    /// Optional description of what happened.
    pub description: Option<String>,
}

impl ActionResult {
    pub fn success(fees: u64, net: u64) -> Self {
        Self {
            fees_paid: fees,
            net_transferred: net,
            success: true,
            description: None,
        }
    }

    pub fn failed(reason: &str) -> Self {
        Self {
            fees_paid: 0,
            net_transferred: 0,
            success: false,
            description: Some(reason.to_string()),
        }
    }

    pub fn hold() -> Self {
        Self {
            fees_paid: 0,
            net_transferred: 0,
            success: true,
            description: None,
        }
    }
}

/// Agent behavior trait.
///
/// Agents decide actions based on their internal strategy and the current
/// simulation state, and can receive payments from other agents.
pub trait Agent: std::fmt::Debug {
    /// Get the agent's unique identifier.
    fn id(&self) -> AgentId;

    /// Get the agent's current balance.
    fn balance(&self) -> u64;

    /// Get the agent's tag vector.
    fn tags(&self) -> &TagVector;

    /// Get mutable access to the agent's account.
    fn account_mut(&mut self) -> &mut crate::Account;

    /// Decide what action to take this round.
    ///
    /// Returns None if the agent chooses to do nothing.
    fn decide_action(&mut self, state: &super::SimulationState) -> Option<Action>;

    /// Called when the agent receives a payment.
    ///
    /// Allows agents to update internal state based on incoming funds.
    fn on_receive_payment(&mut self, amount: u64, from: AgentId);

    /// Get the agent's type name (for reporting).
    fn agent_type(&self) -> &'static str;

    /// Get the agent's wealth quintile (1-5, where 5 is richest).
    fn wealth_quintile(&self, total_wealth: u64, num_agents: usize) -> u8 {
        if total_wealth == 0 || num_agents == 0 {
            return 3; // Middle quintile as default
        }

        // Estimate quintile based on balance relative to average
        let avg_wealth = total_wealth / num_agents as u64;
        let ratio = self.balance() as f64 / avg_wealth.max(1) as f64;

        if ratio < 0.2 {
            1
        } else if ratio < 0.5 {
            2
        } else if ratio < 1.5 {
            3
        } else if ratio < 3.0 {
            4
        } else {
            5
        }
    }
}
