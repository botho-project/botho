// Copyright (c) 2018-2022 The Botho Foundation

//! Botho Account and Subaddress objects

use zeroize::Zeroize;

use crate::keys::{
    RootSpendPrivate, RootSpendPublic, RootViewPrivate, RootViewPublic, SubaddressSpendPrivate,
    SubaddressSpendPublic, SubaddressViewPrivate, SubaddressViewPublic,
};

/// An object which represents a subaddress, and has RingCT-style
/// view and spend public keys.
pub trait RingCtAddress {
    /// Get the subaddress' view public key
    fn view_public_key(&self) -> SubaddressViewPublic;
    /// Get the subaddress' spend public key
    fn spend_public_key(&self) -> SubaddressSpendPublic;
}

impl<T: RingCtAddress> RingCtAddress for &T {
    fn view_public_key(&self) -> SubaddressViewPublic {
        T::view_public_key(self)
    }

    fn spend_public_key(&self) -> SubaddressSpendPublic {
        T::spend_public_key(self)
    }
}

/// Botho basic account object.
///
/// Typically derived via slip10, and containing root view and spend private
/// keys.
#[derive(Debug, Zeroize)]
#[zeroize(drop)]
pub struct Account {
    /// Root view private key
    view_private: RootViewPrivate,
    /// Root spend private key
    spend_private: RootSpendPrivate,
}

impl Account {
    /// Create an account from existing private keys
    pub fn new(view_private: RootViewPrivate, spend_private: RootSpendPrivate) -> Self {
        Self {
            view_private,
            spend_private,
        }
    }

    /// Fetch account view public key
    pub fn view_public_key(&self) -> RootViewPublic {
        RootViewPublic::from(&self.view_private)
    }

    /// Fetch account spend public key
    pub fn spend_public_key(&self) -> RootSpendPublic {
        RootSpendPublic::from(&self.spend_private)
    }

    /// Fetch account view private key
    pub fn view_private_key(&self) -> &RootViewPrivate {
        &self.view_private
    }

    /// Fetch account spend private key
    pub fn spend_private_key(&self) -> &RootSpendPrivate {
        &self.spend_private
    }
}

/// Botho view only account object.
///
/// Derived from an [Account] object, used where spend key custody is external
/// (offline or via hardware). Protobuf encoding is equivalent to
/// [bth_account_keys::ViewAccountKey]
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct ViewAccount {
    /// Root view private key
    view_private: RootViewPrivate,

    /// Root spend public key
    spend_public: RootSpendPublic,
}

impl ViewAccount {
    /// Create an view-only account from existing private keys
    pub fn new(view_private: RootViewPrivate, spend_public: RootSpendPublic) -> Self {
        Self {
            view_private,
            spend_public,
        }
    }

    /// Fetch account view public key
    pub fn view_public_key(&self) -> RootViewPublic {
        RootViewPublic::from(&self.view_private)
    }

    /// Fetch account spend public key
    pub fn spend_public_key(&self) -> &RootSpendPublic {
        &self.spend_public
    }

    /// Fetch account view private key
    pub fn view_private_key(&self) -> &RootViewPrivate {
        &self.view_private
    }
}

impl From<&Account> for ViewAccount {
    fn from(a: &Account) -> Self {
        Self {
            view_private: a.view_private_key().clone(),
            spend_public: a.spend_public_key(),
        }
    }
}

/// Botho spend subaddress object.
///
/// Contains view and spend private keys.
#[derive(Clone, Debug, PartialEq, Zeroize)]
#[zeroize(drop)]
pub struct SpendSubaddress {
    /// sub-address view private key
    pub view_private: SubaddressViewPrivate,
    /// sub-address spend private key
    pub spend_private: SubaddressSpendPrivate,
}

impl RingCtAddress for SpendSubaddress {
    /// Fetch view public address
    fn view_public_key(&self) -> SubaddressViewPublic {
        SubaddressViewPublic::from(&self.view_private)
    }

    /// Fetch spend public address
    fn spend_public_key(&self) -> SubaddressSpendPublic {
        SubaddressSpendPublic::from(&self.spend_private)
    }
}

impl SpendSubaddress {
    /// Fetch subaddress view private key
    pub fn view_private_key(&self) -> &SubaddressViewPrivate {
        &self.view_private
    }

    /// Fetch subaddress spend private key
    pub fn spend_private_key(&self) -> &SubaddressSpendPrivate {
        &self.spend_private
    }
}

/// Botho view-only subaddress object.
///
/// Contains view private and spend public key.
#[derive(Clone, Debug, PartialEq, Zeroize)]
#[zeroize(drop)]
pub struct ViewSubaddress {
    /// sub-address view private key
    pub view_private: SubaddressViewPrivate,

    /// sub-address spend private key
    pub spend_public: SubaddressSpendPublic,
}

impl RingCtAddress for ViewSubaddress {
    /// Fetch view public address
    fn view_public_key(&self) -> SubaddressViewPublic {
        SubaddressViewPublic::from(&self.view_private)
    }

    /// Fetch spend public address
    fn spend_public_key(&self) -> SubaddressSpendPublic {
        self.spend_public.clone()
    }
}

impl ViewSubaddress {
    /// Fetch subaddress view private key
    pub fn view_private_key(&self) -> &SubaddressViewPrivate {
        &self.view_private
    }
}

/// Botho public subaddress object
///
/// Contains view and spend public keys
#[derive(Clone, Debug, PartialEq)]
pub struct PublicSubaddress {
    /// Subaddress view public key
    pub view_public: SubaddressViewPublic,
    /// Subaddress spend public key
    pub spend_public: SubaddressSpendPublic,
}

impl RingCtAddress for PublicSubaddress {
    /// Fetch view public address
    fn view_public_key(&self) -> SubaddressViewPublic {
        self.view_public.clone()
    }

    /// Fetch spend public address
    fn spend_public_key(&self) -> SubaddressSpendPublic {
        self.spend_public.clone()
    }
}

/// Create a [`PublicSubaddress`] object from a [`SpendSubaddress`]
impl From<&SpendSubaddress> for PublicSubaddress {
    fn from(addr: &SpendSubaddress) -> Self {
        Self {
            view_public: addr.view_public_key(),
            spend_public: addr.spend_public_key(),
        }
    }
}

/// Create a [`PublicSubaddress`] object from a [`ViewSubaddress`]
impl From<&ViewSubaddress> for PublicSubaddress {
    fn from(addr: &ViewSubaddress) -> Self {
        Self {
            view_public: addr.view_public_key(),
            spend_public: addr.spend_public_key(),
        }
    }
}

/// Represents a "standard" public address hash created using merlin,
/// used in memos as a compact representation of a Botho public address.
/// This hash is collision resistant.
#[derive(Copy, Clone, Default, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ShortAddressHash([u8; 16]);

impl From<[u8; 16]> for ShortAddressHash {
    fn from(src: [u8; 16]) -> Self {
        Self(src)
    }
}

impl From<ShortAddressHash> for [u8; 16] {
    fn from(src: ShortAddressHash) -> [u8; 16] {
        src.0
    }
}

impl AsRef<[u8; 16]> for ShortAddressHash {
    fn as_ref(&self) -> &[u8; 16] {
        &self.0
    }
}

impl subtle::ConstantTimeEq for ShortAddressHash {
    fn ct_eq(&self, other: &Self) -> subtle::Choice {
        self.0.ct_eq(&other.0)
    }
}

impl core::fmt::Display for ShortAddressHash {
    fn fmt(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
        for b in self.0 {
            write!(formatter, "{b:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::string::ToString;
    use bth_crypto_keys::RistrettoPrivate;
    use bth_util_from_random::FromRandom;

    #[test]
    fn test_account_creation() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private = RistrettoPrivate::from_random(&mut rng);
            let spend_private = RistrettoPrivate::from_random(&mut rng);
            let account = Account::new(view_private.into(), spend_private.into());
            // Just verify we can create and access keys
            let _ = account.view_public_key();
            let _ = account.spend_public_key();
        });
    }

    #[test]
    fn test_account_view_public_deterministic() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private = RistrettoPrivate::from_random(&mut rng);
            let spend_private = RistrettoPrivate::from_random(&mut rng);
            let account = Account::new(view_private.into(), spend_private.into());
            let pub1 = account.view_public_key();
            let pub2 = account.view_public_key();
            assert_eq!(pub1, pub2);
        });
    }

    #[test]
    fn test_account_spend_public_deterministic() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private = RistrettoPrivate::from_random(&mut rng);
            let spend_private = RistrettoPrivate::from_random(&mut rng);
            let account = Account::new(view_private.into(), spend_private.into());
            let pub1 = account.spend_public_key();
            let pub2 = account.spend_public_key();
            assert_eq!(pub1, pub2);
        });
    }

    #[test]
    fn test_different_accounts_different_keys() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let account1 = Account::new(
                RistrettoPrivate::from_random(&mut rng).into(),
                RistrettoPrivate::from_random(&mut rng).into(),
            );
            let account2 = Account::new(
                RistrettoPrivate::from_random(&mut rng).into(),
                RistrettoPrivate::from_random(&mut rng).into(),
            );
            assert_ne!(account1.view_public_key(), account2.view_public_key());
            assert_ne!(account1.spend_public_key(), account2.spend_public_key());
        });
    }

    #[test]
    fn test_view_account_from_account() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private = RistrettoPrivate::from_random(&mut rng);
            let spend_private = RistrettoPrivate::from_random(&mut rng);
            let account = Account::new(view_private.into(), spend_private.into());
            let view_account = ViewAccount::from(&account);
            assert_eq!(account.view_public_key(), view_account.view_public_key());
            assert_eq!(account.spend_public_key(), *view_account.spend_public_key());
        });
    }

    #[test]
    fn test_view_account_creation() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private = RistrettoPrivate::from_random(&mut rng);
            let spend_private = RistrettoPrivate::from_random(&mut rng);
            let spend_public: RootSpendPublic = RootSpendPublic::from(&RootSpendPrivate::from(spend_private));

            let view_account = ViewAccount::new(view_private.into(), spend_public.clone());
            assert_eq!(*view_account.spend_public_key(), spend_public);
        });
    }

    #[test]
    fn test_spend_subaddress_ring_ct_address() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private: SubaddressViewPrivate = RistrettoPrivate::from_random(&mut rng).into();
            let spend_private: SubaddressSpendPrivate = RistrettoPrivate::from_random(&mut rng).into();

            let subaddr = SpendSubaddress {
                view_private: view_private.clone(),
                spend_private: spend_private.clone(),
            };

            let view_pub = subaddr.view_public_key();
            let spend_pub = subaddr.spend_public_key();

            // Verify determinism
            assert_eq!(view_pub, SubaddressViewPublic::from(&view_private));
            assert_eq!(spend_pub, SubaddressSpendPublic::from(&spend_private));
        });
    }

    #[test]
    fn test_spend_subaddress_accessors() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private: SubaddressViewPrivate = RistrettoPrivate::from_random(&mut rng).into();
            let spend_private: SubaddressSpendPrivate = RistrettoPrivate::from_random(&mut rng).into();

            let subaddr = SpendSubaddress {
                view_private: view_private.clone(),
                spend_private: spend_private.clone(),
            };

            assert_eq!(*subaddr.view_private_key(), view_private);
            assert_eq!(*subaddr.spend_private_key(), spend_private);
        });
    }

    #[test]
    fn test_view_subaddress_ring_ct_address() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private: SubaddressViewPrivate = RistrettoPrivate::from_random(&mut rng).into();
            let spend_public: SubaddressSpendPublic = SubaddressSpendPublic::from(
                &SubaddressSpendPrivate::from(RistrettoPrivate::from_random(&mut rng)),
            );

            let subaddr = ViewSubaddress {
                view_private: view_private.clone(),
                spend_public: spend_public.clone(),
            };

            assert_eq!(subaddr.view_public_key(), SubaddressViewPublic::from(&view_private));
            assert_eq!(subaddr.spend_public_key(), spend_public);
        });
    }

    #[test]
    fn test_public_subaddress_from_spend() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private: SubaddressViewPrivate = RistrettoPrivate::from_random(&mut rng).into();
            let spend_private: SubaddressSpendPrivate = RistrettoPrivate::from_random(&mut rng).into();

            let spend_subaddr = SpendSubaddress {
                view_private,
                spend_private,
            };

            let public_subaddr = PublicSubaddress::from(&spend_subaddr);
            assert_eq!(public_subaddr.view_public_key(), spend_subaddr.view_public_key());
            assert_eq!(public_subaddr.spend_public_key(), spend_subaddr.spend_public_key());
        });
    }

    #[test]
    fn test_public_subaddress_from_view() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private: SubaddressViewPrivate = RistrettoPrivate::from_random(&mut rng).into();
            let spend_public: SubaddressSpendPublic = SubaddressSpendPublic::from(
                &SubaddressSpendPrivate::from(RistrettoPrivate::from_random(&mut rng)),
            );

            let view_subaddr = ViewSubaddress {
                view_private,
                spend_public,
            };

            let public_subaddr = PublicSubaddress::from(&view_subaddr);
            assert_eq!(public_subaddr.view_public_key(), view_subaddr.view_public_key());
            assert_eq!(public_subaddr.spend_public_key(), view_subaddr.spend_public_key());
        });
    }

    #[test]
    fn test_ring_ct_address_ref_impl() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let view_private: SubaddressViewPrivate = RistrettoPrivate::from_random(&mut rng).into();
            let spend_private: SubaddressSpendPrivate = RistrettoPrivate::from_random(&mut rng).into();

            let subaddr = SpendSubaddress {
                view_private,
                spend_private,
            };

            // Test that &T implements RingCtAddress when T does
            let subaddr_ref = &subaddr;
            assert_eq!(subaddr_ref.view_public_key(), subaddr.view_public_key());
            assert_eq!(subaddr_ref.spend_public_key(), subaddr.spend_public_key());
        });
    }

    #[test]
    fn test_short_address_hash_from_bytes() {
        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hash = ShortAddressHash::from(bytes);
        let recovered: [u8; 16] = hash.into();
        assert_eq!(bytes, recovered);
    }

    #[test]
    fn test_short_address_hash_as_ref() {
        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hash = ShortAddressHash::from(bytes);
        assert_eq!(hash.as_ref(), &bytes);
    }

    #[test]
    fn test_short_address_hash_default() {
        let default = ShortAddressHash::default();
        assert_eq!(default.as_ref(), &[0u8; 16]);
    }

    #[test]
    fn test_short_address_hash_display() {
        let bytes: [u8; 16] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
                               0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10];
        let hash = ShortAddressHash::from(bytes);
        let display = hash.to_string();
        assert_eq!(display, "0123456789abcdeffedcba9876543210");
    }

    #[test]
    fn test_short_address_hash_eq() {
        let bytes1: [u8; 16] = [1; 16];
        let bytes2: [u8; 16] = [1; 16];
        let bytes3: [u8; 16] = [2; 16];

        let hash1 = ShortAddressHash::from(bytes1);
        let hash2 = ShortAddressHash::from(bytes2);
        let hash3 = ShortAddressHash::from(bytes3);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_short_address_hash_constant_time_eq() {
        use subtle::ConstantTimeEq;

        let bytes1: [u8; 16] = [1; 16];
        let bytes2: [u8; 16] = [1; 16];
        let bytes3: [u8; 16] = [2; 16];

        let hash1 = ShortAddressHash::from(bytes1);
        let hash2 = ShortAddressHash::from(bytes2);
        let hash3 = ShortAddressHash::from(bytes3);

        assert!(bool::from(hash1.ct_eq(&hash2)));
        assert!(!bool::from(hash1.ct_eq(&hash3)));
    }

    #[test]
    fn test_short_address_hash_ord() {
        let bytes1: [u8; 16] = [0; 16];
        let bytes2: [u8; 16] = [1; 16];

        let hash1 = ShortAddressHash::from(bytes1);
        let hash2 = ShortAddressHash::from(bytes2);

        assert!(hash1 < hash2);
    }

    #[test]
    fn test_short_address_hash_hash() {
        use core::hash::{Hash, Hasher};

        struct SimpleHasher(u64);
        impl Hasher for SimpleHasher {
            fn finish(&self) -> u64 { self.0 }
            fn write(&mut self, bytes: &[u8]) {
                for b in bytes {
                    self.0 = self.0.wrapping_add(*b as u64);
                }
            }
        }

        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hash = ShortAddressHash::from(bytes);

        let mut hasher = SimpleHasher(0);
        hash.hash(&mut hasher);
        let hash_value = hasher.finish();

        // Just verify it produces a hash
        assert!(hash_value > 0);
    }
}
