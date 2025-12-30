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
    use crate::consts::{
        CHANGE_SUBADDRESS_INDEX, DEFAULT_SUBADDRESS_INDEX, GIFT_CODE_SUBADDRESS_INDEX,
        INVALID_SUBADDRESS_INDEX,
    };

    // NOTE: test_with_data tests removed - test vector crates were deleted

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
