// Copyright (c) 2024 The Botho Foundation

//! Bridge order types and state machine.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{attestation::MintAuthorization, chains::Chain};

/// Domain-separation tag for deriving the on-chain 32-byte order id from the
/// order UUID. Changing this tag is a bridge-breaking change: in-flight
/// orders would derive different on-chain ids.
const ORDER_ID_DOMAIN_TAG: &[u8] = b"botho-bridge-order-id-v1";

/// Derive the deterministic 32-byte on-chain order id from an order UUID.
///
/// Both destination chains bind mints to this same value: Ethereum passes it
/// as the `bytes32` idempotency key of `WrappedBTH.bridgeMint` and Solana as
/// the `[u8; 32]` argument of the `bridge_mint` instruction. The contract-side
/// duplicate-order guard (#826) plus this derivation make a replayed
/// attestation unable to mint twice.
pub fn derive_order_id(order_uuid: &Uuid) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ORDER_ID_DOMAIN_TAG);
    hasher.update(order_uuid.as_bytes());
    hasher.finalize().into()
}

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

    /// Check whether a transition from `self` to `next` is allowed.
    ///
    /// Allowed transitions:
    /// - The forward happy path (each status to its [`OrderStatus::next`]). In
    ///   particular `MintPending -> Completed` — callers must only take this
    ///   edge once the destination-chain confirmation requirement is met (see
    ///   `confirm_mint` in the bridge service).
    /// - `MintPending -> DepositConfirmed`: reorg unwind. If a submitted mint
    ///   tx is orphaned before finality the order rolls back to
    ///   `DepositConfirmed` and is re-submitted, instead of terminally failing
    ///   (the BTH deposit is still confirmed and owed).
    /// - Any non-terminal status to `Failed`.
    /// - `AwaitingDeposit` / `DepositDetected` to `Expired`.
    ///
    /// Terminal states allow no transitions.
    pub fn can_transition_to(&self, next: &OrderStatus) -> bool {
        if self.is_terminal() {
            return false;
        }

        // Forward happy path.
        if let Some(expected) = self.next() {
            if std::mem::discriminant(&expected) == std::mem::discriminant(next) {
                return true;
            }
        }

        match (self, next) {
            // Reorg unwind: submitted mint orphaned before finality.
            (OrderStatus::MintPending, OrderStatus::DepositConfirmed) => true,
            // Any non-terminal order may fail.
            (_, OrderStatus::Failed { .. }) => true,
            // Only orders still waiting on a deposit can expire.
            (OrderStatus::AwaitingDeposit | OrderStatus::DepositDetected, OrderStatus::Expired) => {
                true
            }
            _ => false,
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

    /// Threshold attestation authorizing the mint (produced by the #824
    /// attestation protocol once the deposit is confirmed). `None` until
    /// the federation has signed.
    #[serde(default)]
    pub mint_authorization: Option<MintAuthorization>,

    /// When the destination-chain transaction (`dest_tx`) reached the
    /// required confirmation depth / finality. `None` while unconfirmed —
    /// confirmation waits are reorg-aware: `dest_tx` may be set and later
    /// cleared again if the tx is orphaned before this is set.
    #[serde(default)]
    pub dest_confirmed_at: Option<DateTime<Utc>>,

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
            mint_authorization: None,
            dest_confirmed_at: None,
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
            mint_authorization: None,
            dest_confirmed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// The deterministic 32-byte on-chain order id for this order.
    ///
    /// See [`derive_order_id`]. Both the Ethereum and Solana mint paths
    /// bind their on-chain mint to this exact value.
    pub fn order_id_bytes(&self) -> [u8; 32] {
        derive_order_id(&self.id)
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

    /// Guarded status transition. Returns an error (and leaves the order
    /// unmodified) if [`OrderStatus::can_transition_to`] rejects the edge.
    ///
    /// Rolling back from `MintPending` to `DepositConfirmed` (reorg unwind)
    /// clears `dest_tx` and `dest_confirmed_at` so the order is re-submitted
    /// cleanly.
    pub fn try_set_status(&mut self, status: OrderStatus) -> Result<(), String> {
        if !self.status.can_transition_to(&status) {
            return Err(format!(
                "invalid order status transition: {} -> {}",
                self.status, status
            ));
        }

        if matches!(
            (&self.status, &status),
            (OrderStatus::MintPending, OrderStatus::DepositConfirmed)
        ) {
            self.dest_tx = None;
            self.dest_confirmed_at = None;
        }

        self.set_status(status);
        Ok(())
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

    #[test]
    fn test_order_id_derivation_is_stable() {
        // Fixed UUID -> fixed on-chain order id. This vector must never
        // change: in-flight orders bind to it on both chains.
        let uuid = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let id1 = derive_order_id(&uuid);
        let id2 = derive_order_id(&uuid);
        assert_eq!(id1, id2, "derivation must be deterministic");

        // Golden vector: sha256("botho-bridge-order-id-v1" || uuid_bytes).
        let expected = {
            let mut hasher = Sha256::new();
            hasher.update(b"botho-bridge-order-id-v1");
            hasher.update(uuid.as_bytes());
            let out: [u8; 32] = hasher.finalize().into();
            out
        };
        assert_eq!(id1, expected);

        // Distinct orders derive distinct ids.
        let other = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        assert_ne!(derive_order_id(&other), id1);
    }

    #[test]
    fn test_order_id_bytes_matches_free_function() {
        // The ETH path calls order.order_id_bytes(); the Solana path may
        // derive from the UUID directly. Both must agree.
        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "addr".to_string(),
            "0x...".to_string(),
        );
        assert_eq!(order.order_id_bytes(), derive_order_id(&order.id));
    }

    #[test]
    fn test_guarded_transitions_happy_path() {
        assert!(OrderStatus::DepositConfirmed.can_transition_to(&OrderStatus::MintPending));
        assert!(OrderStatus::MintPending.can_transition_to(&OrderStatus::Completed));
        assert!(OrderStatus::AwaitingDeposit.can_transition_to(&OrderStatus::DepositDetected));
    }

    #[test]
    fn test_guarded_transitions_reject_skips() {
        // Cannot skip confirmation gating: DepositConfirmed -> Completed
        // must go through MintPending.
        assert!(!OrderStatus::DepositConfirmed.can_transition_to(&OrderStatus::Completed));
        // Cannot complete before submitting.
        assert!(!OrderStatus::DepositDetected.can_transition_to(&OrderStatus::MintPending));
        // Terminal states are frozen.
        assert!(!OrderStatus::Completed.can_transition_to(&OrderStatus::MintPending));
        assert!(!OrderStatus::Failed {
            reason: "x".to_string()
        }
        .can_transition_to(&OrderStatus::DepositConfirmed));
        // Orders past the deposit stage cannot expire.
        assert!(!OrderStatus::MintPending.can_transition_to(&OrderStatus::Expired));
    }

    #[test]
    fn test_reorg_unwind_transition() {
        // MintPending -> DepositConfirmed is the reorg rollback edge.
        assert!(OrderStatus::MintPending.can_transition_to(&OrderStatus::DepositConfirmed));

        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "addr".to_string(),
            "0x...".to_string(),
        );
        order.set_status(OrderStatus::MintPending);
        order.dest_tx = Some("0xdeadbeef".to_string());

        order.try_set_status(OrderStatus::DepositConfirmed).unwrap();
        assert_eq!(order.status, OrderStatus::DepositConfirmed);
        assert!(order.dest_tx.is_none(), "reorg unwind must clear dest_tx");
        assert!(order.dest_confirmed_at.is_none());
    }

    #[test]
    fn test_try_set_status_rejects_invalid() {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "addr".to_string(),
            "0x...".to_string(),
        );
        // AwaitingDeposit -> Completed is not a legal edge.
        assert!(order.try_set_status(OrderStatus::Completed).is_err());
        assert_eq!(order.status, OrderStatus::AwaitingDeposit);
    }
}
