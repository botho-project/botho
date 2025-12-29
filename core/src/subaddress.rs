// Copyright (c) 2018-2022 The Botho Foundation

//! Botho Subaddress Derivations

#![allow(non_snake_case)]

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};

use bth_core_types::account::{PublicSubaddress, ViewAccount};
use bth_crypto_hashes::{Blake2b512, Digest};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};

use crate::{
    account::{Account, SpendSubaddress},
    consts::SUBADDRESS_DOMAIN_TAG,
    keys::*,
};

/// Generate a subaddress for a given input key set
pub trait Subaddress {
    /// Subaddress type
    type Output: core::fmt::Debug;

    /// Generate the subaddress for the corresponding index
    fn subaddress(&self, index: u64) -> Self::Output;
}

/// Generate subadress private keys from root private keys
impl Subaddress for (&RootViewPrivate, &RootSpendPrivate) {
    type Output = (SubaddressViewPrivate, SubaddressSpendPrivate);

    fn subaddress(&self, index: u64) -> Self::Output {
        let (view_private, spend_private) = (self.0, self.1);

        let a = Scalar::from(view_private);

        // `Hs(a || n)`
        let Hs: Scalar = {
            let n = Scalar::from(index);
            let mut digest = Blake2b512::new();
            digest.update(SUBADDRESS_DOMAIN_TAG);
            digest.update(a.as_bytes());
            digest.update(n.as_bytes());
            Scalar::from_hash(digest)
        };

        // Return private subaddress keys
        let b = Scalar::from(spend_private);
        (
            SubaddressViewPrivate::from(RistrettoPrivate::from(a * (Hs + b))),
            SubaddressSpendPrivate::from(RistrettoPrivate::from(Hs + b)),
        )
    }
}

/// Generate subaddress public keys from root view private and spend public keys
impl Subaddress for (&RootViewPrivate, &RootSpendPublic) {
    type Output = (SubaddressViewPublic, SubaddressSpendPublic);

    fn subaddress(&self, index: u64) -> Self::Output {
        let (view_private, spend_public) = (self.0, self.1);

        // Generate spend public
        let a = Scalar::from(view_private);

        // `Hs(a || n)`
        let Hs: Scalar = {
            let n = Scalar::from(index);
            let mut digest = Blake2b512::new();
            digest.update(SUBADDRESS_DOMAIN_TAG);
            digest.update(a.as_bytes());
            digest.update(n.as_bytes());
            Scalar::from_hash(digest)
        };

        let b = RistrettoPrivate::from(Hs);
        let B = RistrettoPublic::from(&b);

        // Return public subaddress keys
        let C: RistrettoPoint = B.as_ref() + RistrettoPoint::from(spend_public);
        (
            SubaddressViewPublic::from(RistrettoPublic::from(a * C)),
            SubaddressSpendPublic::from(RistrettoPublic::from(C)),
        )
    }
}

/// [Subaddress] implementation for base account
impl Subaddress for Account {
    type Output = SpendSubaddress;

    /// Fetch private keys for the i^th subaddress
    fn subaddress(&self, index: u64) -> Self::Output {
        let (view_private, spend_private) =
            (self.view_private_key(), self.spend_private_key()).subaddress(index);

        SpendSubaddress {
            view_private,
            spend_private,
        }
    }
}

/// [Subaddress] implementation for view-only account
impl Subaddress for ViewAccount {
    type Output = PublicSubaddress;

    /// Fetch private keys for the i^th subaddress
    fn subaddress(&self, index: u64) -> Self::Output {
        let (view_public, spend_public) =
            (self.view_private_key(), self.spend_public_key()).subaddress(index);

        PublicSubaddress {
            view_public,
            spend_public,
        }
    }
}

#[cfg(test)]
mod tests {

    use bth_test_vectors_definitions::account_keys::DefaultSubaddrKeysFromAcctPrivKeys;
    use bth_util_test_vector::TestVector;
    use bth_util_test_with_data::test_with_data;

    use super::*;
    use crate::consts::{
        CHANGE_SUBADDRESS_INDEX, DEFAULT_SUBADDRESS_INDEX, GIFT_CODE_SUBADDRESS_INDEX,
        INVALID_SUBADDRESS_INDEX,
    };

    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn default_subaddr_keys_from_acct_priv_keys(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        // Load in keys from test vector
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();

        // Generate private subaddress keys from root view and spend private
        let (subaddr_view_private, subaddr_spend_private) =
            (&root_view_private, &root_spend_private).subaddress(DEFAULT_SUBADDRESS_INDEX);

        // Test subaddress private keys match expectations
        assert_eq!(
            subaddr_view_private.to_bytes(),
            case.subaddress_view_private_key
        );
        assert_eq!(
            subaddr_spend_private.to_bytes(),
            case.subaddress_spend_private_key
        );

        // Check subaddress public keys match expectations
        assert_eq!(
            SubaddressViewPublic::from(&subaddr_view_private).to_bytes(),
            case.subaddress_view_public_key
        );
        assert_eq!(
            SubaddressSpendPublic::from(&subaddr_spend_private).to_bytes(),
            case.subaddress_spend_public_key
        );
    }

    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn default_subaddr_keys_from_acct_view_keys(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        // Load in keys from test vector
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();
        let root_spend_public = RootSpendPublic::from(&root_spend_private);

        // Generate public subaddress keys from root view private and spend public
        let (subaddr_view_public, subaddr_spend_public) =
            (&root_view_private, &root_spend_public).subaddress(DEFAULT_SUBADDRESS_INDEX);

        // Check expectations match
        assert_eq!(
            subaddr_view_public.to_bytes(),
            case.subaddress_view_public_key
        );
        assert_eq!(
            subaddr_spend_public.to_bytes(),
            case.subaddress_spend_public_key
        );
    }

    /// Test that different subaddress indices produce different keys
    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn different_indices_produce_different_keys(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();

        let (view_0, spend_0) = (&root_view_private, &root_spend_private).subaddress(0);
        let (view_1, spend_1) = (&root_view_private, &root_spend_private).subaddress(1);
        let (view_100, spend_100) = (&root_view_private, &root_spend_private).subaddress(100);

        // All subaddresses should be different
        assert_ne!(view_0.to_bytes(), view_1.to_bytes());
        assert_ne!(view_0.to_bytes(), view_100.to_bytes());
        assert_ne!(view_1.to_bytes(), view_100.to_bytes());

        assert_ne!(spend_0.to_bytes(), spend_1.to_bytes());
        assert_ne!(spend_0.to_bytes(), spend_100.to_bytes());
        assert_ne!(spend_1.to_bytes(), spend_100.to_bytes());
    }

    /// Test subaddress derivation is deterministic
    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn subaddress_derivation_is_deterministic(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();

        // Derive the same subaddress twice
        let (view_1, spend_1) = (&root_view_private, &root_spend_private).subaddress(42);
        let (view_2, spend_2) = (&root_view_private, &root_spend_private).subaddress(42);

        assert_eq!(view_1.to_bytes(), view_2.to_bytes());
        assert_eq!(spend_1.to_bytes(), spend_2.to_bytes());
    }

    /// Test that private and public subaddress derivations match
    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn private_and_public_derivations_match(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();
        let root_spend_public = RootSpendPublic::from(&root_spend_private);

        for index in [0, 1, 42, 1000] {
            // Private derivation
            let (priv_view, priv_spend) =
                (&root_view_private, &root_spend_private).subaddress(index);

            // Public derivation
            let (pub_view, pub_spend) =
                (&root_view_private, &root_spend_public).subaddress(index);

            // Public keys from private should match public derivation
            assert_eq!(
                SubaddressViewPublic::from(&priv_view).to_bytes(),
                pub_view.to_bytes()
            );
            assert_eq!(
                SubaddressSpendPublic::from(&priv_spend).to_bytes(),
                pub_spend.to_bytes()
            );
        }
    }

    /// Test reserved subaddress indices (change, gift code)
    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn reserved_subaddress_indices(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();

        // Change subaddress (u64::MAX - 1)
        let (change_view, change_spend) =
            (&root_view_private, &root_spend_private).subaddress(CHANGE_SUBADDRESS_INDEX);

        // Gift code subaddress (u64::MAX - 2)
        let (gift_view, gift_spend) =
            (&root_view_private, &root_spend_private).subaddress(GIFT_CODE_SUBADDRESS_INDEX);

        // Invalid subaddress (u64::MAX)
        let (invalid_view, invalid_spend) =
            (&root_view_private, &root_spend_private).subaddress(INVALID_SUBADDRESS_INDEX);

        // All reserved indices should produce different keys
        assert_ne!(change_view.to_bytes(), gift_view.to_bytes());
        assert_ne!(change_view.to_bytes(), invalid_view.to_bytes());
        assert_ne!(gift_view.to_bytes(), invalid_view.to_bytes());

        assert_ne!(change_spend.to_bytes(), gift_spend.to_bytes());
        assert_ne!(change_spend.to_bytes(), invalid_spend.to_bytes());
        assert_ne!(gift_spend.to_bytes(), invalid_spend.to_bytes());
    }

    /// Test view and spend keys are different for same subaddress
    #[test_with_data(DefaultSubaddrKeysFromAcctPrivKeys::from_jsonl("../test-vectors/vectors"))]
    fn view_and_spend_keys_are_different(case: DefaultSubaddrKeysFromAcctPrivKeys) {
        let root_spend_private = RootSpendPrivate::try_from(&case.spend_private_key).unwrap();
        let root_view_private = RootViewPrivate::try_from(&case.view_private_key).unwrap();

        let (view, spend) = (&root_view_private, &root_spend_private).subaddress(0);

        // View and spend private keys should be different
        assert_ne!(view.to_bytes(), spend.to_bytes());

        // View and spend public keys should be different
        let view_public = SubaddressViewPublic::from(&view);
        let spend_public = SubaddressSpendPublic::from(&spend);
        assert_ne!(view_public.to_bytes(), spend_public.to_bytes());
    }

    /// Test const values for subaddress indices
    #[test]
    fn test_subaddress_constants() {
        assert_eq!(DEFAULT_SUBADDRESS_INDEX, 0);
        assert_eq!(INVALID_SUBADDRESS_INDEX, u64::MAX);
        assert_eq!(CHANGE_SUBADDRESS_INDEX, u64::MAX - 1);
        assert_eq!(GIFT_CODE_SUBADDRESS_INDEX, u64::MAX - 2);

        // Ensure proper ordering
        assert!(DEFAULT_SUBADDRESS_INDEX < GIFT_CODE_SUBADDRESS_INDEX);
        assert!(GIFT_CODE_SUBADDRESS_INDEX < CHANGE_SUBADDRESS_INDEX);
        assert!(CHANGE_SUBADDRESS_INDEX < INVALID_SUBADDRESS_INDEX);
    }
}
