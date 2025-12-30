// Copyright (c) 2024 The Botho Foundation

//! Bridge order types and state machine.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chains::Chain;

/// Custom serde for memo field (fixed-size byte array)
mod memo_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(memo: &Option<[u8; 64]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match memo {
            Some(bytes) => serializer.serialize_some(&hex::encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<[u8; 64]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(hex_str) => {
                let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
                if bytes.len() != 64 {
                    return Err(serde::de::Error::custom("memo must be 64 bytes"));
                }
                let mut arr = [0u8; 64];
                arr.copy_from_slice(&bytes);
                Ok(Some(arr))
            }
            None => Ok(None),
        }
    }
}

/// The type of bridge operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    /// Mint wBTH: User deposits BTH, receives wBTH on target chain
    Mint,
    /// Burn wBTH: User burns wBTH, receives BTH
    Burn,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::Mint => write!(f, "mint"),
            OrderType::Burn => write!(f, "burn"),
        }
    }
}

/// The status of a bridge order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    // === Mint flow (BTH -> wBTH) ===
    /// Order created, waiting for BTH deposit
    AwaitingDeposit,

    /// BTH transaction detected, waiting for finality
    DepositDetected,

    /// BTH deposit confirmed (SCP finalized), ready to mint
    DepositConfirmed,

    /// wBTH mint transaction submitted, waiting for confirmation
    MintPending,

    /// wBTH minted and confirmed, order complete
    Completed,

    // === Burn flow (wBTH -> BTH) ===
    /// wBTH burn transaction detected
    BurnDetected,

    /// wBTH burn confirmed, ready to release BTH
    BurnConfirmed,

    /// BTH release transaction submitted
    ReleasePending,

    /// BTH released and confirmed, order complete
    Released,

    // === Error states ===
    /// Order failed with an error
    Failed { reason: String },

    /// Order expired (deposit not received in time)
    Expired,
}

impl OrderStatus {
    /// Check if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrderStatus::Completed
                | OrderStatus::Released
                | OrderStatus::Failed { .. }
                | OrderStatus::Expired
        )
    }

    /// Check if this order is in an actionable state (needs processing).
    pub fn is_actionable(&self) -> bool {
        matches!(
            self,
            OrderStatus::DepositConfirmed
                | OrderStatus::MintPending
                | OrderStatus::BurnConfirmed
                | OrderStatus::ReleasePending
        )
    }

    /// Get the next expected status in the flow.
    pub fn next(&self) -> Option<OrderStatus> {
        match self {
            OrderStatus::AwaitingDeposit => Some(OrderStatus::DepositDetected),
            OrderStatus::DepositDetected => Some(OrderStatus::DepositConfirmed),
            OrderStatus::DepositConfirmed => Some(OrderStatus::MintPending),
            OrderStatus::MintPending => Some(OrderStatus::Completed),
            OrderStatus::BurnDetected => Some(OrderStatus::BurnConfirmed),
            OrderStatus::BurnConfirmed => Some(OrderStatus::ReleasePending),
            OrderStatus::ReleasePending => Some(OrderStatus::Released),
            _ => None,
        }
    }
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderStatus::AwaitingDeposit => write!(f, "awaiting_deposit"),
            OrderStatus::DepositDetected => write!(f, "deposit_detected"),
            OrderStatus::DepositConfirmed => write!(f, "deposit_confirmed"),
            OrderStatus::MintPending => write!(f, "mint_pending"),
            OrderStatus::Completed => write!(f, "completed"),
            OrderStatus::BurnDetected => write!(f, "burn_detected"),
            OrderStatus::BurnConfirmed => write!(f, "burn_confirmed"),
            OrderStatus::ReleasePending => write!(f, "release_pending"),
            OrderStatus::Released => write!(f, "released"),
            OrderStatus::Failed { reason } => write!(f, "failed: {}", reason),
            OrderStatus::Expired => write!(f, "expired"),
        }
    }
}

/// A bridge order representing a transfer between BTH and a wrapped token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeOrder {
    /// Unique order identifier
    pub id: Uuid,

    /// Type of bridge operation (mint or burn)
    pub order_type: OrderType,

    /// Source chain for the transfer
    pub source_chain: Chain,

    /// Destination chain for the transfer
    pub dest_chain: Chain,

    /// Amount in picocredits (BTH base unit, 12 decimals)
    pub amount: u64,

    /// Bridge fee in picocredits
    pub fee: u64,

    /// Source transaction hash (if known)
    pub source_tx: Option<String>,

    /// Destination transaction hash (if known)
    pub dest_tx: Option<String>,

    /// Source address (sender's address on source chain)
    pub source_address: String,

    /// Destination address (recipient's address on destination chain)
    pub dest_address: String,

    /// Current order status
    pub status: OrderStatus,

    /// Error message if status is Failed
    pub error_message: Option<String>,

    /// BTH memo bytes (for order identification in deposits)
    #[serde(with = "memo_serde")]
    pub memo: Option<[u8; 64]>,

    /// Order creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

impl BridgeOrder {
    /// Create a new mint order (BTH -> wBTH).
    pub fn new_mint(
        dest_chain: Chain,
        amount: u64,
        fee: u64,
        bth_deposit_address: String,
        dest_address: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            order_type: OrderType::Mint,
            source_chain: Chain::Bth,
            dest_chain,
            amount,
            fee,
            source_tx: None,
            dest_tx: None,
            source_address: bth_deposit_address,
            dest_address,
            status: OrderStatus::AwaitingDeposit,
            error_message: None,
            memo: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new burn order (wBTH -> BTH).
    pub fn new_burn(
        source_chain: Chain,
        amount: u64,
        fee: u64,
        source_address: String,
        bth_address: String,
        source_tx: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            order_type: OrderType::Burn,
            source_chain,
            dest_chain: Chain::Bth,
            amount,
            fee,
            source_tx: Some(source_tx),
            dest_tx: None,
            source_address,
            dest_address: bth_address,
            status: OrderStatus::BurnDetected,
            error_message: None,
            memo: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Generate a memo containing the order ID for deposit identification.
    pub fn generate_memo(&mut self) -> [u8; 64] {
        let mut memo = [0u8; 64];
        // First 16 bytes: order UUID
        memo[..16].copy_from_slice(self.id.as_bytes());
        // Remaining bytes can be used for additional metadata
        self.memo = Some(memo);
        memo
    }

    /// Extract order ID from a memo.
    pub fn order_id_from_memo(memo: &[u8; 64]) -> Option<Uuid> {
        Uuid::from_slice(&memo[..16]).ok()
    }

    /// Calculate the net amount after fee.
    pub fn net_amount(&self) -> u64 {
        self.amount.saturating_sub(self.fee)
    }

    /// Update the order status.
    pub fn set_status(&mut self, status: OrderStatus) {
        self.status = status;
        self.updated_at = Utc::now();
    }

    /// Mark the order as failed.
    pub fn fail(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.error_message = Some(reason.clone());
        self.status = OrderStatus::Failed { reason };
        self.updated_at = Utc::now();
    }

    /// Check if the order has expired.
    pub fn is_expired(&self, max_age_minutes: i64) -> bool {
        if self.status.is_terminal() {
            return false;
        }

        let age = Utc::now() - self.created_at;
        age.num_minutes() > max_age_minutes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_mint_order() {
        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000, // 1 BTH
            1_000_000_000,     // 0.001 BTH fee
            "bth_deposit_addr".to_string(),
            "0x1234...".to_string(),
        );

        assert_eq!(order.order_type, OrderType::Mint);
        assert_eq!(order.source_chain, Chain::Bth);
        assert_eq!(order.dest_chain, Chain::Ethereum);
        assert_eq!(order.status, OrderStatus::AwaitingDeposit);
        assert_eq!(order.net_amount(), 999_000_000_000);
    }

    #[test]
    fn test_order_memo() {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "addr".to_string(),
            "0x...".to_string(),
        );

        let memo = order.generate_memo();
        let recovered_id = BridgeOrder::order_id_from_memo(&memo).unwrap();
        assert_eq!(recovered_id, order.id);
    }

    #[test]
    fn test_status_transitions() {
        assert!(!OrderStatus::AwaitingDeposit.is_terminal());
        assert!(OrderStatus::Completed.is_terminal());
        assert!(OrderStatus::Released.is_terminal());
        assert!(OrderStatus::Failed {
            reason: "test".to_string()
        }
        .is_terminal());

        assert!(OrderStatus::DepositConfirmed.is_actionable());
        assert!(OrderStatus::BurnConfirmed.is_actionable());
        assert!(!OrderStatus::AwaitingDeposit.is_actionable());
    }
}
