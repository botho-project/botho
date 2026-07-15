// Copyright (c) 2018-2023 The Botho Foundation

//! Botho account keys.
//!
//! Botho accounts give users fine-grained controls for sharing their
//! address with senders and third-party services. Each account is defined
//! by a pair of private keys (a,b) that are used for identifying owned
//! outputs and spending them, respectively. Instead of sharing the public
//! keys (A,B) directly with senders, users generate and share "subaddresses"
//! (C_i, D_i) that are derived from the private keys (a,b) and an index i.
//! We refer to (C_0, D_0)* as the "default subaddress" for account (a,b).

#![allow(non_snake_case)]

use alloc::vec::Vec;
use bth_core::{
    keys::{
        RootSpendPrivate, RootSpendPublic, RootViewPrivate, SubaddressSpendPublic,
        SubaddressViewPublic,
    },
    slip10::Slip10Key,
    subaddress::Subaddress,
};
use bth_crypto_digestible::{Digestible, MerlinTranscript};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_util_from_random::FromRandom;
use core::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
};
use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
#[cfg(feature = "prost")]
use prost::Message;
use rand_core::{CryptoRng, RngCore};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[cfg(feature = "pq")]
use bth_crypto_pq::{derive_pq_keys_from_seed, BIP39_SEED_SIZE};

pub use bth_core::{
    account::ShortAddressHash,
    consts::{
        CHANGE_SUBADDRESS_INDEX, DEFAULT_SUBADDRESS_INDEX, GIFT_CODE_SUBADDRESS_INDEX,
        INVALID_SUBADDRESS_INDEX,
    },
};

/// Length in bytes of a raw ML-KEM-768 public key (published in a v2 address).
///
/// Kept as a local constant so the base `account-keys` type carries no hard
/// dependency on `bth-crypto-pq` (D1: raw fixed bytes, validate-on-parse).
pub const ML_KEM_768_PUBLIC_KEY_LEN: usize = 1184;

/// Length in bytes of a raw ML-DSA-65 public key (published in a v2 address).
pub const ML_DSA_65_PUBLIC_KEY_LEN: usize = 1952;

/// A Botho user's public subaddress.
///
/// # Address format v2 (universal post-quantum)
///
/// In addition to the two 32-byte Ristretto keys, an address carries two raw
/// post-quantum public keys as fixed-length byte payloads (ADR 0008):
///
/// - `kem_public_key` — ML-KEM-768 (`ML_KEM_768_PUBLIC_KEY_LEN` = 1184 bytes)
/// - `dsa_public_key` — ML-DSA-65 (`ML_DSA_65_PUBLIC_KEY_LEN` = 1952 bytes)
///
/// Both PQ keys are stored as raw bytes (D1: keep the base type free of a hard
/// `bth-crypto-pq` dependency; validate the exact length on parse). Both PQ
/// keys are part of the address's `Digestible` / `ShortAddressHash` identity
/// (D3: an address's PQ keys must not be swappable). A classical-only ("v1")
/// address leaves both PQ fields empty and hashes/serializes exactly as before
/// aside from the two extra (empty) fields.
#[derive(Clone, Digestible, Eq, Hash, Ord, PartialEq, PartialOrd, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PublicAddress {
    /// The user's public subaddress view key 'C'.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "1"))]
    view_public_key: RistrettoPublic,

    /// The user's public subaddress spend key `D`.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "2"))]
    spend_public_key: RistrettoPublic,

    /// Raw ML-KEM-768 public key (1184 bytes) published in the address.
    ///
    /// Empty for classical-only (v1) addresses. For v2 addresses this MUST be
    /// exactly `ML_KEM_768_PUBLIC_KEY_LEN` bytes (validated on parse).
    #[cfg_attr(feature = "prost", prost(bytes, tag = "3"))]
    kem_public_key: Vec<u8>,

    /// Raw ML-DSA-65 public key (1952 bytes) published in the address.
    ///
    /// Empty for classical-only (v1) addresses. For v2 addresses this MUST be
    /// exactly `ML_DSA_65_PUBLIC_KEY_LEN` bytes (validated on parse).
    #[cfg_attr(feature = "prost", prost(bytes, tag = "4"))]
    dsa_public_key: Vec<u8>,
}

impl fmt::Display for PublicAddress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "BTH")?;
        // Classical keys first (spend||view), preserving the historical v1
        // rendering. The PQ keys are appended only when present, so a
        // classical-only address renders identically to before.
        for byte in self
            .spend_public_key
            .to_bytes()
            .iter()
            .chain(self.view_public_key().to_bytes().iter())
            .chain(self.kem_public_key.iter())
            .chain(self.dsa_public_key.iter())
        {
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl PublicAddress {
    /// Create a new public address from CryptoNote key pair
    ///
    /// # Arguments
    /// `spend_public_key` - The user's public subaddress spend key `D`,
    /// `view_public_key` - The user's public subaddress view key  `C`,
    #[inline]
    pub fn new(spend_public_key: &RistrettoPublic, view_public_key: &RistrettoPublic) -> Self {
        Self {
            view_public_key: *view_public_key,
            spend_public_key: *spend_public_key,
            kem_public_key: Vec::new(),
            dsa_public_key: Vec::new(),
        }
    }

    /// Create a new v2 (universal post-quantum) public address.
    ///
    /// # Arguments
    /// * `spend_public_key` - The user's public subaddress spend key `D`.
    /// * `view_public_key` - The user's public subaddress view key `C`.
    /// * `kem_public_key` - Raw ML-KEM-768 public key bytes (expected length
    ///   `ML_KEM_768_PUBLIC_KEY_LEN`).
    /// * `dsa_public_key` - Raw ML-DSA-65 public key bytes (expected length
    ///   `ML_DSA_65_PUBLIC_KEY_LEN`).
    ///
    /// The PQ keys are stored verbatim; length validation is performed on
    /// parse (D1). Callers deriving from real keypairs always supply
    /// correctly-sized bytes.
    #[inline]
    pub fn new_with_pq(
        spend_public_key: &RistrettoPublic,
        view_public_key: &RistrettoPublic,
        kem_public_key: Vec<u8>,
        dsa_public_key: Vec<u8>,
    ) -> Self {
        Self {
            view_public_key: *view_public_key,
            spend_public_key: *spend_public_key,
            kem_public_key,
            dsa_public_key,
        }
    }

    /// Attach post-quantum public keys to an existing (classical) address.
    ///
    /// Consumes `self` and returns a v2 address carrying the supplied raw
    /// ML-KEM-768 and ML-DSA-65 public keys.
    #[inline]
    pub fn with_pq_keys(mut self, kem_public_key: Vec<u8>, dsa_public_key: Vec<u8>) -> Self {
        self.kem_public_key = kem_public_key;
        self.dsa_public_key = dsa_public_key;
        self
    }

    /// Get the public subaddress view key.
    pub fn view_public_key(&self) -> &RistrettoPublic {
        &self.view_public_key
    }

    /// Get the public subaddress spend key.
    pub fn spend_public_key(&self) -> &RistrettoPublic {
        &self.spend_public_key
    }

    /// Get the raw ML-KEM-768 public key bytes.
    ///
    /// Empty for classical-only (v1) addresses.
    pub fn kem_public_key(&self) -> &[u8] {
        &self.kem_public_key
    }

    /// Get the raw ML-DSA-65 public key bytes.
    ///
    /// Empty for classical-only (v1) addresses.
    pub fn dsa_public_key(&self) -> &[u8] {
        &self.dsa_public_key
    }

    /// Whether this address publishes well-formed post-quantum keys.
    ///
    /// Returns true only when both PQ keys are present and have exactly the
    /// expected raw lengths (`ML_KEM_768_PUBLIC_KEY_LEN` and
    /// `ML_DSA_65_PUBLIC_KEY_LEN`).
    pub fn has_pq_keys(&self) -> bool {
        self.kem_public_key.len() == ML_KEM_768_PUBLIC_KEY_LEN
            && self.dsa_public_key.len() == ML_DSA_65_PUBLIC_KEY_LEN
    }
}

impl bth_account_keys_types::RingCtAddress for PublicAddress {
    fn view_public_key(&self) -> &RistrettoPublic {
        &self.view_public_key
    }

    fn spend_public_key(&self) -> &RistrettoPublic {
        &self.spend_public_key
    }
}

impl bth_core::account::RingCtAddress for PublicAddress {
    fn view_public_key(&self) -> SubaddressViewPublic {
        SubaddressViewPublic::from(self.view_public_key)
    }

    fn spend_public_key(&self) -> SubaddressSpendPublic {
        SubaddressSpendPublic::from(self.spend_public_key)
    }
}

impl From<&PublicAddress> for ShortAddressHash {
    fn from(src: &PublicAddress) -> Self {
        let digest = src.digest32::<MerlinTranscript>(b"mc-address");
        let hash: [u8; 16] = digest[0..16].try_into().expect("arithmetic error");
        Self::from(hash)
    }
}

impl From<&PublicAddress> for bth_core::account::PublicSubaddress {
    fn from(value: &PublicAddress) -> Self {
        Self {
            view_public: value.view_public_key.into(),
            spend_public: value.spend_public_key.into(),
        }
    }
}

impl FromRandom for PublicAddress {
    fn from_random<T: RngCore + CryptoRng>(rng: &mut T) -> Self {
        PublicAddress::new(
            &RistrettoPublic::from_random(rng),
            &RistrettoPublic::from_random(rng),
        )
    }
}

/// Complete AccountKey.
///
/// Containing the pair of secret keys, which can be used
/// for spending. This should only ever be present in client code.
///
/// # Account-wide post-quantum public keys (address format v2)
///
/// In addition to the classical Ristretto secret keys, an account carries the
/// account-wide ML-KEM-768 and ML-DSA-65 *public* keys derived from the same
/// BIP39/SLIP-0010 root seed (whitepaper §4.2). These are **account-wide** —
/// identical across every subaddress index, matching the model in
/// `quantum_safe.rs` — and are attached to each [`PublicAddress`] produced by
/// [`AccountKey::subaddress`]. They are stored as raw fixed-length byte
/// payloads (empty for classical/test-only keys that were built without a
/// seed). Only the *derivation* helper [`AccountKey::attach_pq_from_seed`]
/// requires the `pq` feature; the raw-bytes builder does not, keeping the base
/// type free of a hard `bth-crypto-pq` dependency.
#[derive(Clone, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(not(feature = "prost"), derive(Debug))]
#[zeroize(drop)]
pub struct AccountKey {
    /// Private key 'a' used for view-key matching.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "1"))]
    view_private_key: RistrettoPrivate,

    /// Private key `b` used for spending.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "2"))]
    spend_private_key: RistrettoPrivate,

    /// Account-wide raw ML-KEM-768 public key (1184 bytes). Empty for
    /// classical/test-only account keys derived without a seed.
    #[cfg_attr(feature = "prost", prost(bytes, tag = "3"))]
    kem_public_key: Vec<u8>,

    /// Account-wide raw ML-DSA-65 public key (1952 bytes). Empty for
    /// classical/test-only account keys derived without a seed.
    #[cfg_attr(feature = "prost", prost(bytes, tag = "4"))]
    dsa_public_key: Vec<u8>,
}

// Note: Hash, Ord is implemented in terms of default_subaddress() because
// we don't want comparisons to leak private key details over side-channels.
impl Hash for AccountKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.default_subaddress().hash(state)
    }
}

impl Eq for AccountKey {}

impl PartialEq for AccountKey {
    fn eq(&self, other: &Self) -> bool {
        self.default_subaddress().eq(&other.default_subaddress())
    }
}

impl PartialOrd for AccountKey {
    fn partial_cmp(&self, other: &AccountKey) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AccountKey {
    fn cmp(&self, other: &AccountKey) -> Ordering {
        self.default_subaddress().cmp(&other.default_subaddress())
    }
}

/// Create an AccountKey from a SLIP-0010 key
///
/// The resulting account key is **classical-only**: a [`Slip10Key`] is the
/// 32-byte per-account SLIP-0010 node, not the 512-bit BIP39 seed that the
/// canonical PQ derivation (whitepaper §4.2) requires, so no PQ keys are
/// attached here. The real wallet paths (`botho-wallet` / `botho`) hold the
/// full BIP39 seed and attach the account-wide PQ public keys via
/// [`AccountKey::attach_pq_from_seed`] after this conversion.
impl From<Slip10Key> for AccountKey {
    fn from(slip10key: Slip10Key) -> Self {
        let spend_private_key = RootSpendPrivate::from(&slip10key);
        let view_private_key = RootViewPrivate::from(&slip10key);

        Self::new(spend_private_key.as_ref(), view_private_key.as_ref())
    }
}

impl AccountKey {
    /// A user's AccountKey.
    ///
    /// # Arguments
    /// * `spend_private_key` - The user's private spend key `b`.
    /// * `view_private_key` - The user's private view key `a`.
    #[inline]
    pub fn new(spend_private_key: &RistrettoPrivate, view_private_key: &RistrettoPrivate) -> Self {
        Self {
            spend_private_key: *spend_private_key,
            view_private_key: *view_private_key,
            // Classical-only: no seed available at this constructor, so the
            // account publishes no PQ keys. Seed-derived routes attach them via
            // `attach_pq_from_seed` / `with_pq_public_keys`.
            kem_public_key: Vec::new(),
            dsa_public_key: Vec::new(),
        }
    }

    /// Attach account-wide raw post-quantum public keys to this account key.
    ///
    /// Consumes `self` and returns an account key whose [`subaddress`] outputs
    /// publish the supplied ML-KEM-768 and ML-DSA-65 public keys. The bytes are
    /// stored verbatim (length validation happens on address-string parse).
    ///
    /// [`subaddress`]: AccountKey::subaddress
    #[inline]
    pub fn with_pq_public_keys(mut self, kem_public_key: Vec<u8>, dsa_public_key: Vec<u8>) -> Self {
        self.kem_public_key = kem_public_key;
        self.dsa_public_key = dsa_public_key;
        self
    }

    /// Derive and attach account-wide post-quantum public keys from a BIP39
    /// seed (§4.2 seed-derived).
    ///
    /// Reuses the canonical derivation in `bth-crypto-pq`
    /// ([`derive_pq_keys_from_seed`]): the 512-bit BIP39 seed is fed through
    /// HKDF-SHA256 (`"botho-pq-v1"` / `"kem-seed"` and `"sig-seed"`) to
    /// deterministically produce the ML-KEM-768 and ML-DSA-65 keypairs. Only
    /// the derived *public* keys are retained here; the secret keys live on
    /// [`QuantumSafeAccountKey`](crate::QuantumSafeAccountKey). This is the
    /// same derivation used by `QuantumSafeAccountKey`, so the two agree on
    /// the same seed.
    #[cfg(feature = "pq")]
    #[inline]
    pub fn attach_pq_from_seed(self, seed: &[u8; BIP39_SEED_SIZE]) -> Self {
        let pq_keys = derive_pq_keys_from_seed(seed);
        self.with_pq_public_keys(
            pq_keys.kem_keypair.public_key().as_bytes().to_vec(),
            pq_keys.sig_keypair.public_key().as_bytes().to_vec(),
        )
    }

    /// Get the view private key.
    pub fn view_private_key(&self) -> &RistrettoPrivate {
        &self.view_private_key
    }

    /// Get the spend private key.
    pub fn spend_private_key(&self) -> &RistrettoPrivate {
        &self.spend_private_key
    }

    /// Get the account-wide raw ML-KEM-768 public key bytes.
    ///
    /// Empty for classical/test-only account keys derived without a seed.
    pub fn kem_public_key(&self) -> &[u8] {
        &self.kem_public_key
    }

    /// Get the account-wide raw ML-DSA-65 public key bytes.
    ///
    /// Empty for classical/test-only account keys derived without a seed.
    pub fn dsa_public_key(&self) -> &[u8] {
        &self.dsa_public_key
    }

    /// Whether this account carries well-formed account-wide PQ public keys.
    pub fn has_pq_keys(&self) -> bool {
        self.kem_public_key.len() == ML_KEM_768_PUBLIC_KEY_LEN
            && self.dsa_public_key.len() == ML_DSA_65_PUBLIC_KEY_LEN
    }

    /// Create an account key with random secret keys (intended for tests).
    pub fn random<T: RngCore + CryptoRng>(rng: &mut T) -> Self {
        Self::new(
            &RistrettoPrivate::from_random(rng),
            &RistrettoPrivate::from_random(rng),
        )
    }

    /// Get the account's default subaddress.
    #[inline]
    pub fn default_subaddress(&self) -> PublicAddress {
        self.subaddress(DEFAULT_SUBADDRESS_INDEX)
    }

    /// Get the account's change subaddress.
    #[inline]
    pub fn change_subaddress(&self) -> PublicAddress {
        self.subaddress(CHANGE_SUBADDRESS_INDEX)
    }

    /// Get the account's gift code subaddress.
    #[inline]
    pub fn gift_code_subaddress(&self) -> PublicAddress {
        self.subaddress(GIFT_CODE_SUBADDRESS_INDEX)
    }

    /// Get the account's i^th subaddress.
    pub fn subaddress(&self, index: u64) -> PublicAddress {
        let view_public_key = {
            let subaddress_view_private = self.subaddress_view_private(index);
            RistrettoPublic::from(&subaddress_view_private)
        };

        let spend_public_key = {
            let subaddress_spend_private = self.subaddress_spend_private(index);
            RistrettoPublic::from(&subaddress_spend_private)
        };

        // Attach the account-wide PQ public keys (§4.2). They are identical
        // across all subaddress indices; only the classical keys vary. When the
        // account was built without a seed the PQ vecs are empty and the
        // address is classical-only.
        PublicAddress::new(&spend_public_key, &view_public_key)
            .with_pq_keys(self.kem_public_key.clone(), self.dsa_public_key.clone())
    }

    /// The private spend key for the default subaddress.
    pub fn default_subaddress_spend_private(&self) -> RistrettoPrivate {
        self.subaddress_spend_private(DEFAULT_SUBADDRESS_INDEX)
    }

    /// The private spend key for the change subaddress.
    pub fn change_subaddress_spend_private(&self) -> RistrettoPrivate {
        self.subaddress_spend_private(CHANGE_SUBADDRESS_INDEX)
    }

    /// The private spend key for the gift code subaddress
    pub fn gift_code_subaddress_spend_private(&self) -> RistrettoPrivate {
        self.subaddress_spend_private(GIFT_CODE_SUBADDRESS_INDEX)
    }

    /// The private spend key for the i^th subaddress.
    pub fn subaddress_spend_private(&self, index: u64) -> RistrettoPrivate {
        let (_view_private, spend_private) = (
            &RootViewPrivate::from(self.view_private_key),
            &RootSpendPrivate::from(self.spend_private_key),
        )
            .subaddress(index);

        spend_private.inner()
    }

    /// The private view key for the default subaddress.
    pub fn default_subaddress_view_private(&self) -> RistrettoPrivate {
        self.subaddress_view_private(DEFAULT_SUBADDRESS_INDEX)
    }

    /// The private view key for the change subaddress.
    pub fn change_subaddress_view_private(&self) -> RistrettoPrivate {
        self.subaddress_view_private(CHANGE_SUBADDRESS_INDEX)
    }

    /// The private view key for the gift code subaddress.
    pub fn gift_code_subaddress_view_private(&self) -> RistrettoPrivate {
        self.subaddress_view_private(GIFT_CODE_SUBADDRESS_INDEX)
    }

    /// The private view key for the i^th subaddress.
    pub fn subaddress_view_private(&self, index: u64) -> RistrettoPrivate {
        let (view_private, _spend_private) = (
            &RootViewPrivate::from(self.view_private_key),
            &RootSpendPrivate::from(self.spend_private_key),
        )
            .subaddress(index);

        view_private.inner()
    }
}

/// View AccountKey, containing the view private key and the spend public key.
///
/// Like [`AccountKey`], a view account key carries the account-wide raw
/// ML-KEM-768 and ML-DSA-65 public keys so the subaddresses it produces publish
/// the same PQ keys as the full account key it was derived from (empty for
/// classical/test-only view keys).
#[derive(Clone, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[zeroize(drop)]
pub struct ViewAccountKey {
    /// Private key 'a' used for view-key matching.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "1"))]
    view_private_key: RistrettoPrivate,

    /// Public key `B` used for generating Public Addresses.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "2"))]
    spend_public_key: RistrettoPublic,

    /// Account-wide raw ML-KEM-768 public key (1184 bytes). Empty for
    /// classical/test-only view keys.
    #[cfg_attr(feature = "prost", prost(bytes, tag = "3"))]
    kem_public_key: Vec<u8>,

    /// Account-wide raw ML-DSA-65 public key (1952 bytes). Empty for
    /// classical/test-only view keys.
    #[cfg_attr(feature = "prost", prost(bytes, tag = "4"))]
    dsa_public_key: Vec<u8>,
}

// Note: Hash, Ord is implemented in terms of default_subaddress() because
// we don't want comparisons to leak private key details over side-channels.
impl Hash for ViewAccountKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.default_subaddress().hash(state)
    }
}

impl Eq for ViewAccountKey {}

impl PartialEq for ViewAccountKey {
    fn eq(&self, other: &Self) -> bool {
        self.default_subaddress().eq(&other.default_subaddress())
    }
}

impl PartialOrd for ViewAccountKey {
    fn partial_cmp(&self, other: &ViewAccountKey) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ViewAccountKey {
    fn cmp(&self, other: &ViewAccountKey) -> Ordering {
        self.default_subaddress().cmp(&other.default_subaddress())
    }
}

impl From<&AccountKey> for ViewAccountKey {
    fn from(account_key: &AccountKey) -> Self {
        ViewAccountKey {
            view_private_key: *account_key.view_private_key(),
            spend_public_key: account_key.spend_private_key().into(),
            // Carry over the account-wide PQ public keys so the view key's
            // subaddresses publish the same PQ keys as the full account key.
            kem_public_key: account_key.kem_public_key().to_vec(),
            dsa_public_key: account_key.dsa_public_key().to_vec(),
        }
    }
}

impl ViewAccountKey {
    /// A user's ViewAccountKey.
    ///
    /// # Arguments
    /// * `view_private_key` - The user's private view key `a`.
    /// * `spend_public_key` - The user's public spend key `B`.
    #[inline]
    pub fn new(view_private_key: RistrettoPrivate, spend_public_key: RistrettoPublic) -> Self {
        Self {
            view_private_key,
            spend_public_key,
            kem_public_key: Vec::new(),
            dsa_public_key: Vec::new(),
        }
    }

    /// Attach account-wide raw post-quantum public keys to this view key.
    ///
    /// Consumes `self` and returns a view key whose [`subaddress`] outputs
    /// publish the supplied ML-KEM-768 and ML-DSA-65 public keys.
    ///
    /// [`subaddress`]: ViewAccountKey::subaddress
    #[inline]
    pub fn with_pq_public_keys(mut self, kem_public_key: Vec<u8>, dsa_public_key: Vec<u8>) -> Self {
        self.kem_public_key = kem_public_key;
        self.dsa_public_key = dsa_public_key;
        self
    }

    /// Get the view private key.
    pub fn view_private_key(&self) -> &RistrettoPrivate {
        &self.view_private_key
    }

    /// Get the spend public key.
    pub fn spend_public_key(&self) -> &RistrettoPublic {
        &self.spend_public_key
    }

    /// Get the account-wide raw ML-KEM-768 public key bytes.
    pub fn kem_public_key(&self) -> &[u8] {
        &self.kem_public_key
    }

    /// Get the account-wide raw ML-DSA-65 public key bytes.
    pub fn dsa_public_key(&self) -> &[u8] {
        &self.dsa_public_key
    }

    /// Create a view account key with random keys
    pub fn random<T: RngCore + CryptoRng>(rng: &mut T) -> Self {
        Self::new(
            RistrettoPrivate::from_random(rng),
            RistrettoPublic::from_random(rng),
        )
    }

    /// Get the account's default subaddress.
    #[inline]
    pub fn default_subaddress(&self) -> PublicAddress {
        self.subaddress(DEFAULT_SUBADDRESS_INDEX)
    }

    /// Get the account's change subaddress.
    #[inline]
    pub fn change_subaddress(&self) -> PublicAddress {
        self.subaddress(CHANGE_SUBADDRESS_INDEX)
    }

    /// Get the account's gift code subaddress.
    #[inline]
    pub fn gift_code_subaddress(&self) -> PublicAddress {
        self.subaddress(GIFT_CODE_SUBADDRESS_INDEX)
    }

    /// Get the account's i^th subaddress.
    pub fn subaddress(&self, index: u64) -> PublicAddress {
        let (view_public, spend_public) = (
            &RootViewPrivate::from(self.view_private_key),
            &RootSpendPublic::from(*self.spend_public_key()),
        )
            .subaddress(index);

        // Attach the account-wide PQ public keys (§4.2), identical across all
        // subaddress indices. Empty vecs yield a classical-only address.
        PublicAddress::new(&spend_public.inner(), &view_public.inner())
            .with_pq_keys(self.kem_public_key.clone(), self.dsa_public_key.clone())
    }

    /// The public spend key for the default subaddress.
    pub fn default_subaddress_spend_public(&self) -> RistrettoPublic {
        self.subaddress_spend_public(DEFAULT_SUBADDRESS_INDEX)
    }

    /// The public spend key for the change subaddress.
    pub fn change_subaddress_spend_public(&self) -> RistrettoPublic {
        self.subaddress_spend_public(CHANGE_SUBADDRESS_INDEX)
    }

    /// The public spend key for the gift code subaddress.
    pub fn gift_code_subaddress_spend_public(&self) -> RistrettoPublic {
        self.subaddress_spend_public(GIFT_CODE_SUBADDRESS_INDEX)
    }

    /// The private spend key for the i^th subaddress.
    pub fn subaddress_spend_public(&self, index: u64) -> RistrettoPublic {
        let (_view_public, spend_public) = (
            &RootViewPrivate::from(self.view_private_key),
            &RootSpendPublic::from(*self.spend_public_key()),
        )
            .subaddress(index);

        spend_public.inner()
    }

    /// The private view key for the default subaddress.
    pub fn default_subaddress_view_public(&self) -> RistrettoPublic {
        self.subaddress_view_public(DEFAULT_SUBADDRESS_INDEX)
    }

    /// The private view key for the change subaddress.
    pub fn change_subaddress_view_public(&self) -> RistrettoPublic {
        self.subaddress_view_public(CHANGE_SUBADDRESS_INDEX)
    }

    /// The private view key for the change subaddress.
    pub fn gift_code_subaddress_view_public(&self) -> RistrettoPublic {
        self.subaddress_view_public(GIFT_CODE_SUBADDRESS_INDEX)
    }

    /// The private view key for the i^th subaddress.
    pub fn subaddress_view_public(&self, index: u64) -> RistrettoPublic {
        let a: &Scalar = self.view_private_key.as_ref();
        let b: RistrettoPoint = a * self.subaddress_spend_public(index).as_ref();

        RistrettoPublic::from(b)
    }
}

#[cfg(test)]
mod account_key_tests {
    use super::*;
    use rand::prelude::StdRng;
    use rand_core::SeedableRng;

    #[test]
    // Deserializing should recover a serialized a PublicAddress.
    fn bth_util_serial_prost_roundtrip_public_address() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let acct = AccountKey::random(&mut rng);
            let ser = bth_util_serial::encode(&acct.default_subaddress());
            let result: PublicAddress = bth_util_serial::decode(&ser).unwrap();
            assert_eq!(acct.default_subaddress(), result);
        });
    }

    // Build a v2 address with deterministic (index-seeded) PQ payloads of the
    // correct raw lengths. The bytes are not required to be valid PQ keys for
    // these struct/serde/digest tests (validation happens on address-string
    // parse, sub-issue 3).
    fn sample_pq_address(rng: &mut StdRng) -> PublicAddress {
        let spend = RistrettoPublic::from_random(rng);
        let view = RistrettoPublic::from_random(rng);
        let mut kem = alloc::vec![0u8; ML_KEM_768_PUBLIC_KEY_LEN];
        rng.fill_bytes(&mut kem);
        let mut dsa = alloc::vec![0u8; ML_DSA_65_PUBLIC_KEY_LEN];
        rng.fill_bytes(&mut dsa);
        PublicAddress::new_with_pq(&spend, &view, kem, dsa)
    }

    #[test]
    // A v2 address (with both PQ keys) round-trips through prost/bth_util_serial.
    fn bth_util_serial_prost_roundtrip_public_address_v2() {
        let mut rng: StdRng = SeedableRng::from_seed([7u8; 32]);
        let addr = sample_pq_address(&mut rng);

        // Byte-length invariants for a well-formed v2 address.
        assert_eq!(addr.kem_public_key().len(), ML_KEM_768_PUBLIC_KEY_LEN);
        assert_eq!(addr.dsa_public_key().len(), ML_DSA_65_PUBLIC_KEY_LEN);
        assert!(addr.has_pq_keys());

        let ser = bth_util_serial::encode(&addr);
        let result: PublicAddress = bth_util_serial::decode(&ser).unwrap();
        assert_eq!(addr, result);
        assert_eq!(result.kem_public_key(), addr.kem_public_key());
        assert_eq!(result.dsa_public_key(), addr.dsa_public_key());
    }

    #[test]
    // A classical-only (v1) address has empty PQ fields and is not `has_pq_keys`.
    fn classical_address_has_no_pq_keys() {
        let mut rng: StdRng = SeedableRng::from_seed([9u8; 32]);
        let addr = PublicAddress::new(
            &RistrettoPublic::from_random(&mut rng),
            &RistrettoPublic::from_random(&mut rng),
        );
        assert!(addr.kem_public_key().is_empty());
        assert!(addr.dsa_public_key().is_empty());
        assert!(!addr.has_pq_keys());
    }

    #[test]
    // Both PQ keys are part of the address identity (Digestible /
    // ShortAddressHash). Swapping either PQ key MUST change the
    // ShortAddressHash (D3).
    fn pq_keys_are_part_of_short_address_hash() {
        let mut rng: StdRng = SeedableRng::from_seed([11u8; 32]);
        let spend = RistrettoPublic::from_random(&mut rng);
        let view = RistrettoPublic::from_random(&mut rng);
        let mut kem = alloc::vec![0u8; ML_KEM_768_PUBLIC_KEY_LEN];
        rng.fill_bytes(&mut kem);
        let mut dsa = alloc::vec![0u8; ML_DSA_65_PUBLIC_KEY_LEN];
        rng.fill_bytes(&mut dsa);

        let base = PublicAddress::new_with_pq(&spend, &view, kem.clone(), dsa.clone());

        // Classical-only address with the SAME Ristretto keys must hash differently.
        let classical = PublicAddress::new(&spend, &view);
        assert_ne!(
            ShortAddressHash::from(&base),
            ShortAddressHash::from(&classical),
            "PQ keys must contribute to the address hash"
        );

        // Flip one byte of the KEM key -> different hash.
        let mut kem2 = kem.clone();
        kem2[0] ^= 0xff;
        let kem_swapped = PublicAddress::new_with_pq(&spend, &view, kem2, dsa.clone());
        assert_ne!(
            ShortAddressHash::from(&base),
            ShortAddressHash::from(&kem_swapped),
            "swapping the KEM key must change the address hash"
        );

        // Flip one byte of the DSA key -> different hash.
        let mut dsa2 = dsa.clone();
        dsa2[0] ^= 0xff;
        let dsa_swapped = PublicAddress::new_with_pq(&spend, &view, kem, dsa2);
        assert_ne!(
            ShortAddressHash::from(&base),
            ShortAddressHash::from(&dsa_swapped),
            "swapping the DSA key must change the address hash"
        );

        // Determinism: identical inputs -> identical hash.
        let base_again = PublicAddress::new_with_pq(
            &spend,
            &view,
            base.kem_public_key().to_vec(),
            base.dsa_public_key().to_vec(),
        );
        assert_eq!(
            ShortAddressHash::from(&base),
            ShortAddressHash::from(&base_again)
        );
    }

    #[test]
    // Subaddress private keys should agree with subaddress public keys.
    fn test_subadress_private_keys_agree_with_subaddress_public_keys() {
        let mut rng: StdRng = SeedableRng::from_seed([91u8; 32]);
        let view_private = RistrettoPrivate::from_random(&mut rng);
        let spend_private = RistrettoPrivate::from_random(&mut rng);

        let account_key = AccountKey::new(&spend_private, &view_private);

        let index = rng.next_u64();
        let subaddress = account_key.subaddress(index);

        let subaddress_view_private = account_key.subaddress_view_private(index);
        let subaddress_spend_private = account_key.subaddress_spend_private(index);

        let expected_subaddress_view_public = RistrettoPublic::from(&subaddress_view_private);
        let expected_subaddress_spend_public = RistrettoPublic::from(&subaddress_spend_private);

        assert_eq!(expected_subaddress_view_public, subaddress.view_public_key);
        assert_eq!(
            expected_subaddress_spend_public,
            subaddress.spend_public_key
        );
    }

    // NOTE: test_with_data tests removed - test vector crates were deleted

    #[test]
    // Account Key and View Account Key derived from same keys should generate the
    // same subaddresses
    fn test_view_account_keys_subaddresses() {
        let mut rng: StdRng = SeedableRng::from_seed([42u8; 32]);
        let view_private = RistrettoPrivate::from_random(&mut rng);
        let spend_private = RistrettoPrivate::from_random(&mut rng);
        let account_key = AccountKey::new(&spend_private, &view_private);
        let view_account_key = ViewAccountKey::from(&account_key);

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

        assert_eq!(
            account_key.subaddress(500),
            view_account_key.subaddress(500)
        );
    }

    // The account key derived from a BIP39 seed publishes the account-wide PQ
    // public keys on every subaddress, and those keys are the ones the canonical
    // `derive_pq_keys_from_seed` derivation (§4.2) produces. Round-trip:
    // encapsulating against the published KEM key decapsulates with the derived
    // KEM secret, and the published DSA key verifies a signature from the
    // derived DSA secret.
    #[test]
    #[cfg(feature = "pq")]
    fn account_key_publishes_derived_pq_keys_that_round_trip() {
        use bth_crypto_pq::{
            derive_pq_keys_from_seed, MlDsa65PublicKey, MlKem768PublicKey,
            ML_DSA_65_PUBLIC_KEY_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
        };

        let mut rng: StdRng = SeedableRng::from_seed([5u8; 32]);
        let seed = [7u8; 64];

        let account_key = AccountKey::new(
            &RistrettoPrivate::from_random(&mut rng),
            &RistrettoPrivate::from_random(&mut rng),
        )
        .attach_pq_from_seed(&seed);

        // Account-wide keys are present at their raw lengths.
        assert!(account_key.has_pq_keys());
        assert_eq!(
            account_key.kem_public_key().len(),
            ML_KEM_768_PUBLIC_KEY_BYTES
        );
        assert_eq!(
            account_key.dsa_public_key().len(),
            ML_DSA_65_PUBLIC_KEY_BYTES
        );

        // Every subaddress publishes the same account-wide PQ keys.
        let addr0 = account_key.subaddress(0);
        let addr7 = account_key.subaddress(7);
        assert!(addr0.has_pq_keys());
        assert_eq!(addr0.kem_public_key(), addr7.kem_public_key());
        assert_eq!(addr0.dsa_public_key(), addr7.dsa_public_key());

        // The published keys equal the canonical seed-derived keypairs.
        let pq = derive_pq_keys_from_seed(&seed);
        assert_eq!(
            addr0.kem_public_key(),
            pq.kem_keypair.public_key().as_bytes()
        );
        assert_eq!(
            addr0.dsa_public_key(),
            pq.sig_keypair.public_key().as_bytes()
        );

        // KEM round-trip: encapsulate to the PUBLISHED key, decapsulate with the
        // DERIVED secret — shared secrets must match.
        let published_kem = MlKem768PublicKey::from_bytes(addr0.kem_public_key())
            .expect("published KEM key is well formed");
        let (ciphertext, sender_secret) = published_kem.encapsulate();
        let recipient_secret = pq
            .kem_keypair
            .decapsulate(&ciphertext)
            .expect("decapsulation with derived secret succeeds");
        assert_eq!(sender_secret.as_bytes(), recipient_secret.as_bytes());

        // DSA round-trip: sign with the DERIVED secret, verify with the
        // PUBLISHED key.
        let message = b"botho universal-pq round trip";
        let signature = pq.sig_keypair.sign(message);
        let published_dsa = MlDsa65PublicKey::from_bytes(addr0.dsa_public_key())
            .expect("published DSA key is well formed");
        assert!(published_dsa.verify(message, &signature).is_ok());
    }

    // AccountKey and the ViewAccountKey derived from it agree on the published
    // PQ keys across subaddress indices (extends `test_view_account_keys_
    // subaddresses` to the post-quantum fields).
    #[test]
    #[cfg(feature = "pq")]
    fn view_account_keys_subaddresses_agree_on_pq_keys() {
        let mut rng: StdRng = SeedableRng::from_seed([13u8; 32]);
        let account_key = AccountKey::new(
            &RistrettoPrivate::from_random(&mut rng),
            &RistrettoPrivate::from_random(&mut rng),
        )
        .attach_pq_from_seed(&[21u8; 64]);
        let view_account_key = ViewAccountKey::from(&account_key);

        // Full-address equality already covers the PQ fields (they are part of
        // the address identity), for several indices.
        for index in [DEFAULT_SUBADDRESS_INDEX, CHANGE_SUBADDRESS_INDEX, 500] {
            assert_eq!(
                account_key.subaddress(index),
                view_account_key.subaddress(index)
            );
            assert_eq!(
                account_key.subaddress(index).kem_public_key(),
                view_account_key.subaddress(index).kem_public_key()
            );
            assert_eq!(
                account_key.subaddress(index).dsa_public_key(),
                view_account_key.subaddress(index).dsa_public_key()
            );
        }
        assert!(view_account_key.default_subaddress().has_pq_keys());
    }

    // Determinism: the same seed yields the same published PQ keys regardless of
    // the classical keys, and the `AccountKey` derivation agrees with the
    // `QuantumSafeAccountKey` (from_mnemonic / from_parts) derivation on the
    // same seed.
    #[test]
    #[cfg(feature = "pq")]
    fn pq_keys_are_deterministic_across_routes() {
        use crate::QuantumSafeAccountKey;
        use bth_crypto_pq::derive_pq_keys_from_seed;

        let seed = [42u8; 64];
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);

        // Same seed, different classical keys -> identical published PQ keys.
        let ak1 = AccountKey::new(
            &RistrettoPrivate::from_random(&mut rng),
            &RistrettoPrivate::from_random(&mut rng),
        )
        .attach_pq_from_seed(&seed);
        let ak2 = AccountKey::new(
            &RistrettoPrivate::from_random(&mut rng),
            &RistrettoPrivate::from_random(&mut rng),
        )
        .attach_pq_from_seed(&seed);
        assert_eq!(ak1.kem_public_key(), ak2.kem_public_key());
        assert_eq!(ak1.dsa_public_key(), ak2.dsa_public_key());

        // Agreement with the canonical keypair derivation.
        let pq = derive_pq_keys_from_seed(&seed);
        assert_eq!(ak1.kem_public_key(), pq.kem_keypair.public_key().as_bytes());
        assert_eq!(ak1.dsa_public_key(), pq.sig_keypair.public_key().as_bytes());

        // Agreement with QuantumSafeAccountKey: from_mnemonic and from_parts must
        // publish the same PQ keys (both route through
        // `derive_pq_keys_from_seed`).
        const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let qsak_from_mnemonic = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let mnemonic_seed = {
            use bth_crypto_pq::BIP39_SEED_SIZE;
            let mnemonic = bip39::Mnemonic::from_phrase(TEST_MNEMONIC, bip39::Language::English)
                .expect("valid test mnemonic");
            let seed = bip39::Seed::new(&mnemonic, "");
            let bytes: [u8; BIP39_SEED_SIZE] =
                seed.as_bytes().try_into().expect("BIP39 seed is 64 bytes");
            bytes
        };
        let qsak_from_parts = QuantumSafeAccountKey::from_parts(
            AccountKey::new(
                &RistrettoPrivate::from_random(&mut rng),
                &RistrettoPrivate::from_random(&mut rng),
            ),
            derive_pq_keys_from_seed(&mnemonic_seed),
        );
        assert_eq!(
            qsak_from_mnemonic.default_subaddress().kem_public_key(),
            qsak_from_parts.default_subaddress().kem_public_key(),
            "from_mnemonic and from_parts must agree on the KEM key"
        );
        assert_eq!(
            qsak_from_mnemonic.default_subaddress().dsa_public_key(),
            qsak_from_parts.default_subaddress().dsa_public_key(),
            "from_mnemonic and from_parts must agree on the DSA key"
        );
    }
}
