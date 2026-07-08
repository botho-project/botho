// Copyright (c) 2018-2022 The Botho Foundation

//! Botho Transaction Constants.

use bth_crypto_ring_signature::Scalar;
use core::fmt;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// =============================================================================
// Network Configuration
// =============================================================================

/// The network type (mainnet or testnet)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Network {
    /// Production network with real value
    Mainnet,
    /// Test network for development and testing (default during beta)
    #[default]
    Testnet,
}

impl Network {
    /// Address prefix for this network
    ///
    /// Different prefixes prevent accidental cross-network sends.
    pub const fn address_prefix(&self) -> &'static str {
        match self {
            Network::Mainnet => "botho://1/",
            Network::Testnet => "tbotho://1/",
        }
    }

    /// Quantum-safe address prefix for this network
    #[cfg(feature = "pq")]
    pub const fn quantum_address_prefix(&self) -> &'static str {
        match self {
            Network::Mainnet => "botho://1q/",
            Network::Testnet => "tbotho://1q/",
        }
    }

    /// Default gossip port for this network
    pub const fn default_gossip_port(&self) -> u16 {
        match self {
            Network::Mainnet => 7100,
            Network::Testnet => 17100,
        }
    }

    /// Default RPC port for this network
    pub const fn default_rpc_port(&self) -> u16 {
        match self {
            Network::Mainnet => 7101,
            Network::Testnet => 17101,
        }
    }

    /// Magic bytes for protocol handshake
    ///
    /// Nodes reject connections from different networks.
    pub const fn magic_bytes(&self) -> [u8; 4] {
        match self {
            Network::Mainnet => [0x42, 0x54, 0x48, 0x4D], // "BTHM"
            Network::Testnet => [0x42, 0x54, 0x48, 0x54], // "BTHT"
        }
    }

    /// Network name as a string
    pub const fn name(&self) -> &'static str {
        match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
        }
    }

    /// Short display name for CLI output
    pub const fn display_name(&self) -> &'static str {
        match self {
            Network::Mainnet => "MAINNET",
            Network::Testnet => "TESTNET",
        }
    }

    /// Directory name suffix for this network (e.g., "testnet" or "mainnet")
    pub const fn dir_name(&self) -> &'static str {
        self.name()
    }

    /// Whether this network is suitable for real value
    pub const fn is_production(&self) -> bool {
        matches!(self, Network::Mainnet)
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mainnet" | "main" => Some(Network::Mainnet),
            "testnet" | "test" => Some(Network::Testnet),
            _ => None,
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Maximum number of transactions that may be included in a Block.
pub const MAX_TRANSACTIONS_PER_BLOCK: usize = 5000;

// =============================================================================
// Message Size Limits (DoS Protection)
// =============================================================================

/// Maximum serialized size of a Standard-Private (CLSAG) transaction in bytes
/// (100 KB).
///
/// Rationale: A maximally-sized CLSAG ring transaction has:
/// - 16 inputs × (~700B CLSAG signature per input) ≈ 11 KB
/// - 16 outputs × ~1.2 KB (ML-KEM stealth + Pedersen) ≈ 19 KB
/// - Bulletproofs aggregated: ~2 KB
/// - Overhead: ~1 KB
/// - Total: ~33 KB typical max, 100 KB limit provides margin.
///
/// Messages exceeding this size are rejected before deserialization to prevent
/// resource exhaustion attacks.
pub const MAX_TRANSACTION_SIZE: usize = 100 * 1024; // 100 KB

/// Maximum serialized size of a single block in bytes (20 MB).
///
/// Rationale: With MAX_TRANSACTIONS_PER_BLOCK (5000) and average tx size of
/// ~2KB, typical full blocks are ~10MB. 20MB limit provides headroom.
pub const MAX_BLOCK_SIZE: usize = 20 * 1024 * 1024; // 20 MB

/// Maximum serialized size of an SCP consensus message in bytes (1 MB).
///
/// SCP messages contain nominations and ballot state, which can reference
/// transaction hashes but not full transaction data.
pub const MAX_SCP_MESSAGE_SIZE: usize = 1024 * 1024; // 1 MB

// =============================================================================
// Ring Signature Parameters
// =============================================================================

/// Ring size for private (CLSAG) transactions.
/// Ring size 20 provides strong anonymity (larger than Monero's 16).
/// CLSAG signatures are ~700 bytes per input, so ring size 20 is efficient.
pub const RING_SIZE: usize = 20;

// =============================================================================
// Transaction Limits
// =============================================================================

/// Maximum inputs for private (CLSAG) transactions.
/// 16 inputs × ~700B = ~11 KB signature data, well within 100 KB limit.
pub const MAX_INPUTS: u64 = 16;

/// Each transaction must contain no more than this many outputs.
/// Bulletproofs aggregation keeps proof sizes efficient for 16 outputs.
pub const MAX_OUTPUTS: u64 = 16;

/// Maximum number of blocks in the future a transaction's tombstone block can
/// be set to.
///
/// This is the limit enforced in the enclave as part of transaction
/// validation rules. However, untrusted may decide to evict pending
/// transactions from the queue before this point, so this is only a maximum on
/// how long a Tx can actually be pending.
///
/// Note that clients are still in charge of setting the actual tombstone value.
/// For normal transactions, clients at time of writing are defaulting to
/// something like current block height + 100, so that they can know quickly if
/// a Tx succeeded or failed.
///
/// Rationale for this number is, at a rate of 2 blocks / minute, this is 7
/// days, which eases operations for minting agents which must perform a
/// multi-sig.
pub const MAX_TOMBSTONE_BLOCKS: u64 = 20160;

// =============================================================================
// BTH Tokenomics
// =============================================================================
//
// The Botho network has NO pre-mine. Initial supply is 0 BTH.
// All BTH is created through minting rewards.
//
// Phase 1 (Years 0-10): Halving schedule distributes ~100M BTH
//   - Initial reward: ~50 BTH per block
//   - 5 halvings every 2 years
//
// Phase 2 (Year 10+): 2% annual net inflation target
//   - Difficulty adjusts to achieve target net inflation
//   - Fee burns reduce effective inflation
//
// For detailed monetary policy, see: cluster-tax/src/monetary.rs

/// Approximate BTH distributed during Phase 1 (10 years of halvings).
/// This is NOT a hard cap - inflation continues in Phase 2.
pub const PHASE1_BTH_DISTRIBUTION: u64 = 100_000_000;

// =============================================================================
// BTH Unit System (single unit: picocredits, decision #649)
// =============================================================================
//
// BTH uses a 12-decimal precision system for maximum accounting accuracy.
// The one and only base unit is the "picocredit" (10^-12 BTH). Every amount
// below the UI edge — transaction amounts, fees, fee curves, cluster wealth,
// emission/monetary policy — is denominated in picocredits. Formatting into
// BTH happens only in display components.
//
// (The former two-tier system, with a separate nanoBTH fee/display tier, was
// retired by #694 per the #649 decision: it was the root of a recurring
// unit-confusion bug class — see #626/#628.)
//
// Unit Hierarchy (all relative to picocredits):
//   1 picocredit    = 1                     (the base unit)
//   1 microBTH      = 1,000,000 picocredits (10^-6 BTH)
//   1 milliBTH      = 1,000,000,000 picocredits (10^-3 BTH)
//   1 BTH           = 1,000,000,000,000 picocredits (10^12)
//
// Overflow Safety:
//   - Individual transaction amounts stay in u64: u64::MAX ≈ 1.84 × 10^19
//     picocredits ≈ 18.4M BTH per amount.
//   - Aggregate supply exceeds u64 (100M BTH = 10^20 picocredits), so supply
//     accounting is u128 throughout the node (issues #333/#626).

// -----------------------------------------------------------------------------
// Picocredit constants (12-decimal precision)
// -----------------------------------------------------------------------------

/// One BTH = 10^12 picocredits (the base unit)
pub const BTH_TO_PICOCREDITS: u64 = 1_000_000_000_000;

/// One milliBTH = 10^9 picocredits
pub const MILLIBTH_TO_PICOCREDITS: u64 = 1_000_000_000;

/// One microBTH = 10^6 picocredits
pub const MICROBTH_TO_PICOCREDITS: u64 = 1_000_000;

/// Blinding for the implicit fee outputs.
pub const FEE_BLINDING: Scalar = Scalar::ZERO;

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Network Tests
    // =========================================================================

    #[test]
    fn test_network_default_is_testnet() {
        assert_eq!(Network::default(), Network::Testnet);
    }

    #[test]
    fn test_network_names() {
        assert_eq!(Network::Mainnet.name(), "mainnet");
        assert_eq!(Network::Testnet.name(), "testnet");
    }

    #[test]
    fn test_network_display_names() {
        assert_eq!(Network::Mainnet.display_name(), "MAINNET");
        assert_eq!(Network::Testnet.display_name(), "TESTNET");
    }

    #[test]
    fn test_network_address_prefixes_are_distinct() {
        assert_ne!(
            Network::Mainnet.address_prefix(),
            Network::Testnet.address_prefix()
        );
        // Testnet prefix should start with 't'
        assert!(Network::Testnet.address_prefix().starts_with('t'));
        // Mainnet prefix should not start with 't'
        assert!(!Network::Mainnet.address_prefix().starts_with('t'));
    }

    #[test]
    fn test_network_ports_are_distinct() {
        assert_ne!(
            Network::Mainnet.default_gossip_port(),
            Network::Testnet.default_gossip_port()
        );
        assert_ne!(
            Network::Mainnet.default_rpc_port(),
            Network::Testnet.default_rpc_port()
        );
        // Testnet ports should be offset by 10000
        assert_eq!(
            Network::Testnet.default_gossip_port(),
            Network::Mainnet.default_gossip_port() + 10000
        );
    }

    #[test]
    fn test_network_magic_bytes_are_distinct() {
        assert_ne!(
            Network::Mainnet.magic_bytes(),
            Network::Testnet.magic_bytes()
        );
        // Both should start with "BTH"
        assert_eq!(&Network::Mainnet.magic_bytes()[..3], b"BTH");
        assert_eq!(&Network::Testnet.magic_bytes()[..3], b"BTH");
        // Mainnet ends with 'M', Testnet with 'T'
        assert_eq!(Network::Mainnet.magic_bytes()[3], b'M');
        assert_eq!(Network::Testnet.magic_bytes()[3], b'T');
    }

    #[test]
    fn test_network_is_production() {
        assert!(Network::Mainnet.is_production());
        assert!(!Network::Testnet.is_production());
    }

    #[test]
    fn test_network_from_str() {
        assert_eq!(Network::from_str("mainnet"), Some(Network::Mainnet));
        assert_eq!(Network::from_str("main"), Some(Network::Mainnet));
        assert_eq!(Network::from_str("MAINNET"), Some(Network::Mainnet));
        assert_eq!(Network::from_str("testnet"), Some(Network::Testnet));
        assert_eq!(Network::from_str("test"), Some(Network::Testnet));
        assert_eq!(Network::from_str("TESTNET"), Some(Network::Testnet));
        assert_eq!(Network::from_str("invalid"), None);
    }

    #[test]
    fn test_network_display() {
        // Use alloc::format for no_std compatibility
        extern crate alloc;
        assert_eq!(alloc::format!("{}", Network::Mainnet), "mainnet");
        assert_eq!(alloc::format!("{}", Network::Testnet), "testnet");
    }

    // =========================================================================
    // Original Tests
    // =========================================================================

    #[test]
    fn test_max_transactions_per_block() {
        // Maximum transactions per block should be 5000
        assert_eq!(MAX_TRANSACTIONS_PER_BLOCK, 5000);
        // Should be reasonable for block processing
        assert!(MAX_TRANSACTIONS_PER_BLOCK > 0);
        assert!(MAX_TRANSACTIONS_PER_BLOCK <= 10_000);
    }

    #[test]
    fn test_ring_size() {
        // CLSAG ring size is 20 for strong anonymity (larger than Monero's 16)
        assert_eq!(RING_SIZE, 20);
        // Ring size should be at least 7 for meaningful privacy
        assert!(RING_SIZE >= 7);
        // Ring size should be reasonable for transaction size
        assert!(RING_SIZE <= 64);
    }

    #[test]
    fn test_max_inputs() {
        // Maximum CLSAG inputs should be 16
        assert_eq!(MAX_INPUTS, 16);
        // Should be reasonable limit
        assert!(MAX_INPUTS > 0);
        assert!(MAX_INPUTS <= 64);
    }

    #[test]
    fn test_max_outputs() {
        // Maximum outputs should be 16
        assert_eq!(MAX_OUTPUTS, 16);
        // Should be reasonable limit
        assert!(MAX_OUTPUTS > 0);
        assert!(MAX_OUTPUTS <= 64);
    }

    #[test]
    fn test_max_tombstone_blocks() {
        // Maximum tombstone is 20160 blocks (~7 days at 2 blocks/minute)
        assert_eq!(MAX_TOMBSTONE_BLOCKS, 20160);

        // Verify the rationale: 2 blocks/min * 60 min * 24 hr * 7 days = 20160
        let blocks_per_minute = 2u64;
        let minutes_per_hour = 60u64;
        let hours_per_day = 24u64;
        let days = 7u64;
        let expected = blocks_per_minute * minutes_per_hour * hours_per_day * days;
        assert_eq!(MAX_TOMBSTONE_BLOCKS, expected);
    }

    #[test]
    fn test_phase1_bth_distribution() {
        // Phase 1 distributes approximately 100 million BTH
        assert_eq!(PHASE1_BTH_DISTRIBUTION, 100_000_000);
    }

    #[test]
    fn test_fee_blinding() {
        // Fee blinding should be zero (fees are public)
        assert_eq!(FEE_BLINDING, Scalar::ZERO);
    }

    #[test]
    fn test_inflation_headroom() {
        // Verify u128 supply accounting has headroom for 2% annual inflation
        // over 250+ years, starting from 100M BTH at end of Phase 1.
        // (Supply-scale quantities are u128 in the node — #333/#626 — because
        // 100M BTH = 10^20 picocredits already exceeds u64::MAX.)
        let phase1_supply_pico = PHASE1_BTH_DISTRIBUTION as u128 * BTH_TO_PICOCREDITS as u128;

        // (1.02)^250 ≈ 144.2 (scaled by 1000)
        let supply_250y = phase1_supply_pico * 144_210 / 1_000;

        // ~1.44e22 picocredits — u128 (max ~3.4e38) has ~16 orders of
        // magnitude of headroom beyond that.
        assert!(
            supply_250y < u128::MAX / 1_000_000_000_000,
            "250-year supply must fit u128 with ample headroom"
        );
    }

    #[test]
    fn test_max_inputs_outputs_relationship() {
        // Inputs and outputs limits should be equal
        assert_eq!(MAX_INPUTS, MAX_OUTPUTS);
    }

    #[test]
    fn test_ring_size_fits_in_block() {
        // A maximally sized CLSAG transaction with all rings should fit
        let clsag_elements = (MAX_INPUTS as usize) * RING_SIZE;
        assert!(
            clsag_elements <= 1000,
            "CLSAG ring elements should be bounded"
        );
    }

    // =========================================================================
    // Message Size Limit Tests
    // =========================================================================

    #[test]
    fn test_max_transaction_size() {
        // 100 KB limit for Standard-Private (CLSAG) transactions
        assert_eq!(MAX_TRANSACTION_SIZE, 100 * 1024);
        // Should be enough for a max CLSAG tx (16 inputs × ~700B + 16 outputs × ~1.2KB
        // ≈ 33KB)
        assert!(
            MAX_TRANSACTION_SIZE > 50_000,
            "Should fit largest CLSAG transactions"
        );
        // But not too large for DoS protection
        assert!(
            MAX_TRANSACTION_SIZE <= 1024 * 1024,
            "Should be under 1MB for DoS protection"
        );
    }

    #[test]
    fn test_max_block_size() {
        // 20 MB limit
        assert_eq!(MAX_BLOCK_SIZE, 20 * 1024 * 1024);
        // Should fit MAX_TRANSACTIONS_PER_BLOCK average-sized transactions
        // Average tx ~2KB, 5000 txs = 10MB, with 2x headroom
        assert!(MAX_BLOCK_SIZE >= MAX_TRANSACTIONS_PER_BLOCK * 2048);
    }

    #[test]
    fn test_max_scp_message_size() {
        // 1 MB limit
        assert_eq!(MAX_SCP_MESSAGE_SIZE, 1024 * 1024);
        // SCP messages reference tx hashes, not full txs
        // 5000 txs × 32 bytes = 160KB, plus ballot state overhead
        assert!(MAX_SCP_MESSAGE_SIZE >= MAX_TRANSACTIONS_PER_BLOCK * 32);
    }

    #[test]
    fn test_size_limits_ordering() {
        // Transaction < SCP < Block
        assert!(MAX_TRANSACTION_SIZE < MAX_SCP_MESSAGE_SIZE);
        assert!(MAX_SCP_MESSAGE_SIZE < MAX_BLOCK_SIZE);
    }

    // =========================================================================
    // Picocredit Unit Tests
    // =========================================================================

    #[test]
    fn test_bth_to_picocredits() {
        // 1 BTH = 10^12 picocredits
        assert_eq!(BTH_TO_PICOCREDITS, 1_000_000_000_000);
    }

    #[test]
    fn test_millibth_to_picocredits() {
        // 1 milliBTH = 10^9 picocredits
        assert_eq!(MILLIBTH_TO_PICOCREDITS, 1_000_000_000);
        // milliBTH should be 1/1000 of BTH
        assert_eq!(MILLIBTH_TO_PICOCREDITS * 1000, BTH_TO_PICOCREDITS);
    }

    #[test]
    fn test_microbth_to_picocredits() {
        // 1 microBTH = 10^6 picocredits
        assert_eq!(MICROBTH_TO_PICOCREDITS, 1_000_000);
        // microBTH should be 1/1000 of milliBTH
        assert_eq!(MICROBTH_TO_PICOCREDITS * 1000, MILLIBTH_TO_PICOCREDITS);
    }

    #[test]
    fn test_picocredit_supply_limits() {
        // Phase 1 distributes 100M BTH.
        // In picocredits: 100M * 10^12 = 10^20, which overflows u64
        // (max ~1.84 * 10^19).
        //
        // This is why aggregate supply tracking is u128 in the node
        // (#333/#626), while individual transaction amounts (much smaller)
        // stay in u64 picocredits.

        // Verify Phase 1 overflows u64 in picocredits (expected behavior)
        let phase1_picocredits = PHASE1_BTH_DISTRIBUTION.checked_mul(BTH_TO_PICOCREDITS);
        assert!(
            phase1_picocredits.is_none(),
            "Phase 1 in picocredits overflows u64 (expected)"
        );

        // And fits comfortably in u128.
        let phase1_pico_u128 = PHASE1_BTH_DISTRIBUTION as u128 * BTH_TO_PICOCREDITS as u128;
        assert_eq!(phase1_pico_u128, 100_000_000_000_000_000_000u128); // 10^20
    }

    #[test]
    fn test_individual_amounts_in_picocredits() {
        // Individual transaction amounts should fit in u64
        // Max realistic single transaction: 1M BTH
        let large_tx = 1_000_000u64.checked_mul(BTH_TO_PICOCREDITS);
        assert!(
            large_tx.is_some(),
            "1M BTH transaction fits in u64 picocredits"
        );

        // Even 10M BTH fits
        let very_large_tx = 10_000_000u64.checked_mul(BTH_TO_PICOCREDITS);
        assert!(
            very_large_tx.is_some(),
            "10M BTH transaction fits in u64 picocredits"
        );

        // 18M BTH is about the max that fits
        // u64::MAX / 10^12 ≈ 18.4 million
        let max_bth = u64::MAX / BTH_TO_PICOCREDITS;
        assert!(
            max_bth >= 18_000_000,
            "At least 18M BTH fits in picocredits"
        );
    }
}
