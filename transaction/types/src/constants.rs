// Copyright (c) 2018-2022 The Botho Foundation

//! Botho Transaction Constants.

use bth_crypto_ring_signature::Scalar;

/// Maximum number of transactions that may be included in a Block.
pub const MAX_TRANSACTIONS_PER_BLOCK: usize = 5000;

/// Each input ring must contain this many elements.
pub const RING_SIZE: usize = 11;

/// Each transaction must contain no more than this many inputs (rings).
pub const MAX_INPUTS: u64 = 16;

/// Each transaction must contain no more than this many outputs.
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
// All BTH is created through mining rewards.
//
// Phase 1 (Years 0-10): Halving schedule distributes ~100M BTH
//   - Initial reward: ~50 BTH per block
//   - 5 halvings every 2 years
//
// Phase 2 (Year 10+): 2% annual net inflation target
//   - Difficulty adjusts to achieve target net inflation
//   - Fee burns reduce effective inflation
//
// Overflow Safety:
//   - Using nanoBTH (1e9) as smallest unit
//   - 100M BTH at year 10 = 10^17 nanoBTH
//   - u64::MAX / 10^17 = 184x max growth capacity
//   - At 2% annual inflation: (1.02)^263 ≈ 184x is the limit
//   - Safe for ~260 years after Phase 1 (~270 years from genesis)
//
// For detailed monetary policy, see: cluster-tax/src/monetary.rs

/// Approximate BTH distributed during Phase 1 (10 years of halvings).
/// This is NOT a hard cap - inflation continues in Phase 2.
pub const PHASE1_BTH_DISTRIBUTION: u64 = 100_000_000;

/// one microBTH = 1e3 nanoBTH
pub const MICROBTH_TO_NANOBTH: u64 = 1_000;

/// one milliBTH = 1e6 nanoBTH
pub const MILLIBTH_TO_NANOBTH: u64 = 1_000_000;

/// one BTH = 1e9 nanoBTH
pub const BTH_TO_NANOBTH: u64 = 1_000_000_000;

/// Blinding for the implicit fee outputs.
pub const FEE_BLINDING: Scalar = Scalar::ZERO;

#[cfg(test)]
mod tests {
    use super::*;

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
        // Ring size must be exactly 11 for privacy guarantees
        assert_eq!(RING_SIZE, 11);
        // Ring size should be odd for anonymity set properties
        assert!(RING_SIZE % 2 == 1);
        // Ring size should be at least 3 for meaningful privacy
        assert!(RING_SIZE >= 3);
    }

    #[test]
    fn test_max_inputs() {
        // Maximum inputs should be 16
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
    fn test_microbth_to_nanobth() {
        // 1 microBTH = 1e3 nanoBTH
        assert_eq!(MICROBTH_TO_NANOBTH, 1_000);
    }

    #[test]
    fn test_millibth_to_nanobth() {
        // 1 milliBTH = 1e6 nanoBTH
        assert_eq!(MILLIBTH_TO_NANOBTH, 1_000_000);
        // milliBTH should be 1000x microBTH
        assert_eq!(MILLIBTH_TO_NANOBTH, MICROBTH_TO_NANOBTH * 1000);
    }

    #[test]
    fn test_bth_to_nanobth() {
        // 1 BTH = 1e9 nanoBTH
        assert_eq!(BTH_TO_NANOBTH, 1_000_000_000);
        // BTH should be 1000x milliBTH
        assert_eq!(BTH_TO_NANOBTH, MILLIBTH_TO_NANOBTH * 1000);
    }

    #[test]
    fn test_fee_blinding() {
        // Fee blinding should be zero (fees are public)
        assert_eq!(FEE_BLINDING, Scalar::ZERO);
    }

    #[test]
    fn test_unit_conversions_consistency() {
        // Verify unit conversion relationships
        // 1 BTH = 1e9 nanoBTH
        assert_eq!(BTH_TO_NANOBTH, 1_000_000_000u64);

        // Phase 1 distribution in nanoBTH should NOT overflow u64
        let phase1_nanobth = PHASE1_BTH_DISTRIBUTION.checked_mul(BTH_TO_NANOBTH);
        assert!(phase1_nanobth.is_some(), "Phase 1 distribution in nanoBTH fits in u64");
        assert_eq!(phase1_nanobth.unwrap(), 100_000_000_000_000_000u64); // 10^17

        // With 2% annual inflation over 100 years from year 10 (~7.24x), still fits
        // (1.02)^100 ≈ 7.244
        let max_inflated_supply = (phase1_nanobth.unwrap() as f64 * 7.244) as u64;
        assert!(max_inflated_supply < u64::MAX, "100-year inflated supply fits in u64");
    }

    #[test]
    fn test_inflation_headroom() {
        // Verify we have headroom for 2% annual inflation over 250+ years
        // Starting from 100M BTH at end of Phase 1
        let phase1_supply_nanobth = PHASE1_BTH_DISTRIBUTION as u128 * BTH_TO_NANOBTH as u128;

        // (1.02)^100 ≈ 7.244
        let inflation_factor_100y = 7244u128; // scaled by 1000
        let supply_100y = phase1_supply_nanobth * inflation_factor_100y / 1000;

        // (1.02)^200 ≈ 52.5
        let inflation_factor_200y = 52485u128; // scaled by 1000
        let supply_200y = phase1_supply_nanobth * inflation_factor_200y / 1000;

        // (1.02)^250 ≈ 144.2
        let inflation_factor_250y = 144210u128; // scaled by 1000
        let supply_250y = phase1_supply_nanobth * inflation_factor_250y / 1000;

        assert!(supply_100y < u64::MAX as u128, "100-year supply fits in u64");
        assert!(supply_200y < u64::MAX as u128, "200-year supply fits in u64");
        assert!(supply_250y < u64::MAX as u128, "250-year supply fits in u64");

        // Calculate the theoretical maximum years before overflow
        // max_multiplier = u64::MAX / phase1_supply_nanobth
        //                = 1.84e19 / 1e17 = 184
        // (1.02)^x = 184 → x = ln(184) / ln(1.02) ≈ 263 years
        //
        // So we're safe for ~260 years after Phase 1 (year 10)
        // That means safe until approximately year 270 from genesis
        let max_safe_multiplier = u64::MAX as u128 / phase1_supply_nanobth;
        assert!(max_safe_multiplier > 100, "At least 100x growth capacity (>230 years)");
        assert!(max_safe_multiplier > 180, "At least 180x growth capacity (>260 years)");
    }

    #[test]
    fn test_max_inputs_outputs_relationship() {
        // Inputs and outputs limits should be equal
        assert_eq!(MAX_INPUTS, MAX_OUTPUTS);
    }

    #[test]
    fn test_ring_size_fits_in_block() {
        // A maximally sized transaction with all rings should fit
        // MAX_INPUTS rings * RING_SIZE elements should be reasonable
        let total_ring_elements = (MAX_INPUTS as usize) * RING_SIZE;
        assert!(total_ring_elements <= 1000, "Total ring elements should be bounded");
    }
}
