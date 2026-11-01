//! #1698 drift guards for the shared `bth_cluster_tax::consensus_fee` model.
//!
//! Kept out of `store.rs` so a sim parity test does not churn that file's
//! SHA-256 pin in `scripts/research/fixtures/ct-contract-inventory.json`.

/// The fee split, ring size and wallet decoy band the sims mirror equal the
/// node's values.
#[test]
fn consensus_fee_model_pool_split_and_ring_size_match_node() {
    use bth_cluster_tax::consensus_fee::{
        split_fee_pool_burn, CONSENSUS_RING_SIZE, LOTTERY_POOL_FRACTION_PERMILLE,
    };
    let canonical = crate::ledger::lottery_v2::canonical_config();
    assert_eq!(
        LOTTERY_POOL_FRACTION_PERMILLE,
        canonical.pool_fraction_permille
    );
    assert_eq!(
        LOTTERY_POOL_FRACTION_PERMILLE,
        crate::consensus::lottery::LotteryFeeConfig::default().pool_fraction_permille
    );
    for fee in [0u64, 1, 999, 3_961, 1_000_000_007, u64::MAX / 2] {
        assert_eq!(split_fee_pool_burn(fee), canonical.split_fees(fee));
    }
    assert_eq!(CONSENSUS_RING_SIZE, bth_transaction_clsag::MIN_RING_SIZE);
    assert_eq!(
        CONSENSUS_RING_SIZE,
        bth_transaction_clsag::DEFAULT_RING_SIZE
    );
    // The settlement-horizon sweep's honest ring mirrors the wallet's
    // factor-similarity decoy band.
    assert_eq!(
        bth_cluster_tax::consensus_fee::WALLET_FACTOR_SIMILARITY_SPREAD_BPS,
        crate::decoy_selection::FACTOR_SIMILARITY_SPREAD
    );
}
