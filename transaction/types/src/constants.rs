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

/// The Botho network will contain an initial supply of 250 million BTH.
/// Note: With 2% annual inflation, supply will grow ~7.24x over 100 years.
/// Using nanoBTH (1e9) as the smallest unit ensures no u64 overflow:
/// 250M * 1e9 * 7.24 ≈ 1.81e18 < u64::MAX (1.84e19)
pub const TOTAL_BTH: u64 = 250_000_000;

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
    fn test_total_bth_supply() {
        // Total BTH initial supply should be 250 million
        assert_eq!(TOTAL_BTH, 250_000_000);
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

        // Total supply in nanoBTH should NOT overflow u64
        let total_nanobth = TOTAL_BTH.checked_mul(BTH_TO_NANOBTH);
        assert!(total_nanobth.is_some(), "Total supply in nanoBTH fits in u64");
        assert_eq!(total_nanobth.unwrap(), 250_000_000_000_000_000u64); // 2.5e17

        // With 2% annual inflation over 100 years (~7.24x), still fits
        // (1.02)^100 ≈ 7.244
        let max_inflated_supply = (total_nanobth.unwrap() as f64 * 7.244) as u64;
        assert!(max_inflated_supply < u64::MAX, "100-year inflated supply fits in u64");
    }

    #[test]
    fn test_inflation_headroom() {
        // Verify we have headroom for 2% annual inflation over 100+ years
        let initial_supply_nanobth = TOTAL_BTH as u128 * BTH_TO_NANOBTH as u128;

        // (1.02)^100 ≈ 7.244
        let inflation_factor_100y = 7244u128; // scaled by 1000
        let supply_100y = initial_supply_nanobth * inflation_factor_100y / 1000;

        // (1.02)^150 ≈ 19.22
        let inflation_factor_150y = 19220u128; // scaled by 1000
        let supply_150y = initial_supply_nanobth * inflation_factor_150y / 1000;

        assert!(supply_100y < u64::MAX as u128, "100-year supply fits in u64");
        assert!(supply_150y < u64::MAX as u128, "150-year supply fits in u64");
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
