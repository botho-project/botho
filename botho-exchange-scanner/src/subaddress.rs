//! Subaddress derivation utilities.
//!
//! This module provides functions for deriving subaddresses from view keys,
//! useful for generating deposit addresses for exchange customers.

use bth_account_keys::{PublicAddress, ViewAccountKey};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};

/// Derive a single subaddress from view key components.
///
/// # Arguments
/// * `view_private_key` - The exchange's view private key
/// * `spend_public_key` - The exchange's spend public key
/// * `index` - The subaddress index
///
/// # Returns
/// The derived public address
pub fn derive_subaddress(
    view_private_key: &RistrettoPrivate,
    spend_public_key: &RistrettoPublic,
    index: u64,
) -> PublicAddress {
    let view_account_key = ViewAccountKey::new(*view_private_key, *spend_public_key);
    view_account_key.subaddress(index)
}

/// Derive a batch of subaddresses.
///
/// # Arguments
/// * `view_private_key` - The exchange's view private key
/// * `spend_public_key` - The exchange's spend public key
/// * `start_index` - Starting subaddress index
/// * `count` - Number of subaddresses to derive
///
/// # Returns
/// Vector of (index, address) pairs
pub fn derive_subaddress_batch(
    view_private_key: &RistrettoPrivate,
    spend_public_key: &RistrettoPublic,
    start_index: u64,
    count: u64,
) -> Vec<(u64, PublicAddress)> {
    let view_account_key = ViewAccountKey::new(*view_private_key, *spend_public_key);

    (start_index..start_index + count)
        .map(|index| (index, view_account_key.subaddress(index)))
        .collect()
}

/// Derive a subaddress and return it in various formats.
#[derive(Debug, Clone)]
pub struct DerivedSubaddress {
    /// The subaddress index
    pub index: u64,
    /// The full public address
    pub address: PublicAddress,
    /// View public key (hex)
    pub view_public_key_hex: String,
    /// Spend public key (hex)
    pub spend_public_key_hex: String,
    /// Address in BTH format
    pub address_string: String,
}

impl DerivedSubaddress {
    /// Create from a public address and index.
    pub fn new(index: u64, address: PublicAddress) -> Self {
        let view_public_key_hex = hex::encode(address.view_public_key().to_bytes());
        let spend_public_key_hex = hex::encode(address.spend_public_key().to_bytes());
        let address_string = address.to_string();

        Self {
            index,
            address,
            view_public_key_hex,
            spend_public_key_hex,
            address_string,
        }
    }

    /// Derive a subaddress and create the full representation.
    pub fn derive(
        view_private_key: &RistrettoPrivate,
        spend_public_key: &RistrettoPublic,
        index: u64,
    ) -> Self {
        let address = derive_subaddress(view_private_key, spend_public_key, index);
        Self::new(index, address)
    }
}

/// Parse keys from hex strings and derive a subaddress.
///
/// # Arguments
/// * `view_private_key_hex` - View private key as 64-character hex string
/// * `spend_public_key_hex` - Spend public key as 64-character hex string
/// * `index` - Subaddress index
///
/// # Returns
/// The derived subaddress with metadata, or an error
pub fn derive_subaddress_from_hex(
    view_private_key_hex: &str,
    spend_public_key_hex: &str,
    index: u64,
) -> anyhow::Result<DerivedSubaddress> {
    let view_private_bytes: [u8; 32] = hex::decode(view_private_key_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("view_private_key must be 32 bytes"))?;

    let view_private_key = RistrettoPrivate::try_from(&view_private_bytes[..])
        .map_err(|e| anyhow::anyhow!("Invalid view private key: {:?}", e))?;

    let spend_public_bytes: [u8; 32] = hex::decode(spend_public_key_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("spend_public_key must be 32 bytes"))?;

    let spend_public_key = RistrettoPublic::try_from(&spend_public_bytes[..])
        .map_err(|e| anyhow::anyhow!("Invalid spend public key: {:?}", e))?;

    Ok(DerivedSubaddress::derive(
        &view_private_key,
        &spend_public_key,
        index,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_util_from_random::FromRandom;
    use rand_core::SeedableRng;

    fn create_test_keys() -> (RistrettoPrivate, RistrettoPublic) {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let view_private = RistrettoPrivate::from_random(&mut rng);
        let spend_private = RistrettoPrivate::from_random(&mut rng);
        let spend_public = RistrettoPublic::from(&spend_private);
        (view_private, spend_public)
    }

    #[test]
    fn test_derive_subaddress() {
        let (view_private, spend_public) = create_test_keys();
        let addr0 = derive_subaddress(&view_private, &spend_public, 0);
        let addr1 = derive_subaddress(&view_private, &spend_public, 1);

        // Different indices should produce different addresses
        assert_ne!(
            addr0.spend_public_key().to_bytes(),
            addr1.spend_public_key().to_bytes()
        );
    }

    #[test]
    fn test_derive_batch() {
        let (view_private, spend_public) = create_test_keys();
        let batch = derive_subaddress_batch(&view_private, &spend_public, 0, 10);

        assert_eq!(batch.len(), 10);
        assert_eq!(batch[0].0, 0);
        assert_eq!(batch[9].0, 9);
    }

    #[test]
    fn test_derived_subaddress() {
        let (view_private, spend_public) = create_test_keys();
        let derived = DerivedSubaddress::derive(&view_private, &spend_public, 42);

        assert_eq!(derived.index, 42);
        assert_eq!(derived.view_public_key_hex.len(), 64);
        assert_eq!(derived.spend_public_key_hex.len(), 64);
        assert!(derived.address_string.starts_with("BTH"));
    }
}
