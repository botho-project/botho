#![no_main]

//! Fuzzing target for subaddress derivation and view key scanning.
//!
//! Security rationale: Subaddress derivation must be deterministic and consistent.
//! The same account key must always produce the same subaddresses. View key scanning
//! must correctly identify owned outputs without false positives or negatives.
//!
//! This target tests:
//! - Subaddress derivation consistency
//! - Public/private key correspondence
//! - View key calculations
//! - Edge cases (index 0, max index, special indices)

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use bth_account_keys::{
    AccountKey, ViewAccountKey, PublicAddress,
    DEFAULT_SUBADDRESS_INDEX, CHANGE_SUBADDRESS_INDEX, GIFT_CODE_SUBADDRESS_INDEX,
};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_util_from_random::FromRandom;

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode for subaddress operations
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Test subaddress derivation consistency
    DerivationConsistency(DerivationFuzz),
    /// Test view account key consistency
    ViewAccountKey(ViewKeyFuzz),
    /// Test special indices
    SpecialIndices(SpecialIndexFuzz),
    /// Test index range
    IndexRange(IndexRangeFuzz),
    /// Test key correspondence
    KeyCorrespondence(KeyCorrespondenceFuzz),
}

/// Derivation consistency test
#[derive(Debug, Arbitrary)]
struct DerivationFuzz {
    /// Seed for account key
    seed: [u8; 32],
    /// Indices to test
    indices: Vec<u64>,
}

/// View account key test
#[derive(Debug, Arbitrary)]
struct ViewKeyFuzz {
    /// Seed for account key
    seed: [u8; 32],
    /// Indices to compare
    indices: Vec<u64>,
}

/// Special index test
#[derive(Debug, Arbitrary)]
struct SpecialIndexFuzz {
    /// Seed for account key
    seed: [u8; 32],
}

/// Index range test
#[derive(Debug, Arbitrary)]
struct IndexRangeFuzz {
    /// Seed for account key
    seed: [u8; 32],
    /// Start index
    start: u64,
    /// Number of indices to test
    count: u8,
}

/// Key correspondence test
#[derive(Debug, Arbitrary)]
struct KeyCorrespondenceFuzz {
    /// Seed for account key
    seed: [u8; 32],
    /// Indices to verify
    indices: Vec<u64>,
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::DerivationConsistency(deriv) => {
            fuzz_derivation_consistency(&deriv);
        }
        FuzzMode::ViewAccountKey(view) => {
            fuzz_view_account_key(&view);
        }
        FuzzMode::SpecialIndices(special) => {
            fuzz_special_indices(&special);
        }
        FuzzMode::IndexRange(range) => {
            fuzz_index_range(&range);
        }
        FuzzMode::KeyCorrespondence(corr) => {
            fuzz_key_correspondence(&corr);
        }
    }
});

/// Test subaddress derivation consistency
fn fuzz_derivation_consistency(deriv: &DerivationFuzz) {
    let mut rng = ChaCha20Rng::from_seed(deriv.seed);
    let spend_private = RistrettoPrivate::from_random(&mut rng);
    let view_private = RistrettoPrivate::from_random(&mut rng);
    let account_key = AccountKey::new(&spend_private, &view_private);

    // Test each index (limit to prevent OOM)
    for &index in deriv.indices.iter().take(20) {
        // Derive subaddress twice - must be identical
        let subaddress1 = account_key.subaddress(index);
        let subaddress2 = account_key.subaddress(index);

        assert_eq!(
            subaddress1, subaddress2,
            "Subaddress derivation must be deterministic for index {}",
            index
        );

        // Public address components should be valid
        let _ = subaddress1.view_public_key();
        let _ = subaddress1.spend_public_key();

        // Test Display trait doesn't panic
        let _display = format!("{}", subaddress1);
    }

    // Default subaddress should be consistent
    let default1 = account_key.default_subaddress();
    let default2 = account_key.subaddress(DEFAULT_SUBADDRESS_INDEX);
    assert_eq!(
        default1, default2,
        "default_subaddress() must equal subaddress(DEFAULT_SUBADDRESS_INDEX)"
    );

    // Change subaddress should be consistent
    let change1 = account_key.change_subaddress();
    let change2 = account_key.subaddress(CHANGE_SUBADDRESS_INDEX);
    assert_eq!(
        change1, change2,
        "change_subaddress() must equal subaddress(CHANGE_SUBADDRESS_INDEX)"
    );

    // Gift code subaddress should be consistent
    let gift1 = account_key.gift_code_subaddress();
    let gift2 = account_key.subaddress(GIFT_CODE_SUBADDRESS_INDEX);
    assert_eq!(
        gift1, gift2,
        "gift_code_subaddress() must equal subaddress(GIFT_CODE_SUBADDRESS_INDEX)"
    );
}

/// Test view account key consistency with full account key
fn fuzz_view_account_key(view: &ViewKeyFuzz) {
    let mut rng = ChaCha20Rng::from_seed(view.seed);
    let spend_private = RistrettoPrivate::from_random(&mut rng);
    let view_private = RistrettoPrivate::from_random(&mut rng);
    let account_key = AccountKey::new(&spend_private, &view_private);

    // Create view account key from full account key
    let view_account_key = ViewAccountKey::from(&account_key);

    // All subaddresses should match
    for &index in view.indices.iter().take(20) {
        let full_subaddress = account_key.subaddress(index);
        let view_subaddress = view_account_key.subaddress(index);

        assert_eq!(
            full_subaddress, view_subaddress,
            "ViewAccountKey must produce same subaddresses as AccountKey for index {}",
            index
        );
    }

    // Special subaddresses should match
    assert_eq!(
        account_key.default_subaddress(),
        view_account_key.default_subaddress()
    );
    assert_eq!(
        account_key.change_subaddress(),
        view_account_key.change_subaddress()
    );
    assert_eq!(
        account_key.gift_code_subaddress(),
        view_account_key.gift_code_subaddress()
    );
}

/// Test special subaddress indices
fn fuzz_special_indices(special: &SpecialIndexFuzz) {
    let mut rng = ChaCha20Rng::from_seed(special.seed);
    let spend_private = RistrettoPrivate::from_random(&mut rng);
    let view_private = RistrettoPrivate::from_random(&mut rng);
    let account_key = AccountKey::new(&spend_private, &view_private);

    // Test default subaddress
    let default = account_key.default_subaddress();
    assert_eq!(
        default,
        account_key.subaddress(DEFAULT_SUBADDRESS_INDEX),
        "Default should be at DEFAULT_SUBADDRESS_INDEX"
    );

    // Test change subaddress
    let change = account_key.change_subaddress();
    assert_eq!(
        change,
        account_key.subaddress(CHANGE_SUBADDRESS_INDEX),
        "Change should be at CHANGE_SUBADDRESS_INDEX"
    );

    // Test gift code subaddress
    let gift = account_key.gift_code_subaddress();
    assert_eq!(
        gift,
        account_key.subaddress(GIFT_CODE_SUBADDRESS_INDEX),
        "Gift code should be at GIFT_CODE_SUBADDRESS_INDEX"
    );

    // All special subaddresses should be distinct
    assert_ne!(default, change, "Default and change must be different");
    assert_ne!(default, gift, "Default and gift must be different");
    assert_ne!(change, gift, "Change and gift must be different");

    // Test max u64 index (should not panic)
    let max_index = account_key.subaddress(u64::MAX);
    let _ = max_index.view_public_key();
    let _ = max_index.spend_public_key();

    // Test some large indices
    for index in [1000, 10000, 100000, u64::MAX / 2, u64::MAX - 1] {
        let subaddress = account_key.subaddress(index);
        // Should produce valid addresses
        let _ = format!("{}", subaddress);
    }
}

/// Test a range of indices
fn fuzz_index_range(range: &IndexRangeFuzz) {
    let mut rng = ChaCha20Rng::from_seed(range.seed);
    let spend_private = RistrettoPrivate::from_random(&mut rng);
    let view_private = RistrettoPrivate::from_random(&mut rng);
    let account_key = AccountKey::new(&spend_private, &view_private);

    let requested_count = (range.count as usize).min(50);

    // Limit count to avoid overflow - can only test up to (u64::MAX - start + 1) unique indices
    let available_indices = u64::MAX.saturating_sub(range.start).saturating_add(1);
    let count = requested_count.min(available_indices as usize);

    if count < 2 {
        // Not enough unique indices to test, skip
        return;
    }

    // All subaddresses in range should be unique
    let mut subaddresses: Vec<PublicAddress> = Vec::with_capacity(count);

    for i in 0..count {
        let index = range.start.checked_add(i as u64).unwrap();
        let subaddress = account_key.subaddress(index);

        // Check uniqueness
        for (j, prev) in subaddresses.iter().enumerate() {
            assert_ne!(
                &subaddress, prev,
                "Subaddresses at indices {} and {} must be unique",
                range.start + j as u64,
                index
            );
        }

        subaddresses.push(subaddress);
    }
}

/// Test that private keys correspond to public keys
fn fuzz_key_correspondence(corr: &KeyCorrespondenceFuzz) {
    let mut rng = ChaCha20Rng::from_seed(corr.seed);
    let spend_private = RistrettoPrivate::from_random(&mut rng);
    let view_private = RistrettoPrivate::from_random(&mut rng);
    let account_key = AccountKey::new(&spend_private, &view_private);

    for &index in corr.indices.iter().take(10) {
        // Get public subaddress
        let public_address = account_key.subaddress(index);

        // Get private keys for this subaddress
        let subaddress_spend_private = account_key.subaddress_spend_private(index);
        let subaddress_view_private = account_key.subaddress_view_private(index);

        // Compute public keys from private keys
        let computed_spend_public = RistrettoPublic::from(&subaddress_spend_private);
        let computed_view_public = RistrettoPublic::from(&subaddress_view_private);

        // They must match the public address
        assert_eq!(
            &computed_spend_public,
            public_address.spend_public_key(),
            "Spend public key mismatch at index {}",
            index
        );
        assert_eq!(
            &computed_view_public,
            public_address.view_public_key(),
            "View public key mismatch at index {}",
            index
        );
    }

    // Test the convenience methods for special indices
    let default_spend = account_key.default_subaddress_spend_private();
    let computed_default_spend = RistrettoPublic::from(&default_spend);
    assert_eq!(
        &computed_default_spend,
        account_key.default_subaddress().spend_public_key()
    );

    let change_spend = account_key.change_subaddress_spend_private();
    let computed_change_spend = RistrettoPublic::from(&change_spend);
    assert_eq!(
        &computed_change_spend,
        account_key.change_subaddress().spend_public_key()
    );

    let gift_spend = account_key.gift_code_subaddress_spend_private();
    let computed_gift_spend = RistrettoPublic::from(&gift_spend);
    assert_eq!(
        &computed_gift_spend,
        account_key.gift_code_subaddress().spend_public_key()
    );
}
