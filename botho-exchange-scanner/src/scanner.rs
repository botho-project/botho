//! Exchange scanner with precomputed subaddress lookup.
//!
//! This module provides efficient output scanning by precomputing all
//! subaddress spend public keys at initialization time, enabling O(1) ownership
//! detection.

use std::collections::HashMap;

use bth_account_keys::ViewAccountKey;
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::onetime_keys::recover_public_subaddress_spend_key;

use crate::{config::ScannerConfig, deposit::DetectedDeposit};

/// Exchange scanner with precomputed subaddress lookup table.
///
/// The scanner precomputes spend public keys for all subaddresses in the
/// configured range, enabling O(1) ownership detection for each output.
pub struct ExchangeScanner {
    /// View account key for the exchange
    view_account_key: ViewAccountKey,

    /// Precomputed subaddress spend public keys for fast lookup
    /// Key: spend_public_key bytes, Value: subaddress index
    spend_key_lookup: HashMap<[u8; 32], u64>,

    /// Minimum subaddress index being scanned
    subaddress_min: u64,

    /// Maximum subaddress index being scanned
    subaddress_max: u64,
}

impl ExchangeScanner {
    /// Create a new scanner from configuration.
    ///
    /// This will precompute all subaddress spend keys, which may take a moment
    /// for large ranges.
    pub fn from_config(config: &ScannerConfig) -> anyhow::Result<Self> {
        // Parse view private key
        let view_private_bytes: [u8; 32] = hex::decode(&config.view_private_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("view_private_key must be 32 bytes"))?;

        let view_private_key = RistrettoPrivate::try_from(&view_private_bytes[..])
            .map_err(|e| anyhow::anyhow!("Invalid view private key: {:?}", e))?;

        // Parse spend public key
        let spend_public_bytes: [u8; 32] = hex::decode(&config.spend_public_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("spend_public_key must be 32 bytes"))?;

        let spend_public_key = RistrettoPublic::try_from(&spend_public_bytes[..])
            .map_err(|e| anyhow::anyhow!("Invalid spend public key: {:?}", e))?;

        Self::new(
            view_private_key,
            spend_public_key,
            config.subaddress_min,
            config.subaddress_max,
        )
    }

    /// Create a new scanner with the given view key and subaddress range.
    ///
    /// This will precompute all subaddress spend keys, which may take a moment
    /// for large ranges (approximately 1ms per 1000 subaddresses).
    pub fn new(
        view_private_key: RistrettoPrivate,
        spend_public_key: RistrettoPublic,
        min_index: u64,
        max_index: u64,
    ) -> anyhow::Result<Self> {
        if max_index < min_index {
            anyhow::bail!("max_index must be >= min_index");
        }

        let range_size = max_index - min_index + 1;
        tracing::info!(
            "Precomputing {} subaddress keys (indices {} to {})",
            range_size,
            min_index,
            max_index
        );

        let view_account_key = ViewAccountKey::new(view_private_key, spend_public_key);

        // Precompute all subaddress spend keys for O(1) lookup
        let mut spend_key_lookup = HashMap::with_capacity(range_size as usize);

        let start = std::time::Instant::now();
        for index in min_index..=max_index {
            let subaddress = view_account_key.subaddress(index);
            let spend_bytes = subaddress.spend_public_key().to_bytes();
            spend_key_lookup.insert(spend_bytes, index);

            // Log progress for large ranges
            if range_size > 10_000 && (index - min_index + 1).is_multiple_of(10_000) {
                tracing::debug!(
                    "Precomputed {}/{} subaddress keys",
                    index - min_index + 1,
                    range_size
                );
            }
        }
        let elapsed = start.elapsed();

        tracing::info!(
            "Precomputed {} subaddress keys in {:?} ({:.2} keys/ms)",
            range_size,
            elapsed,
            range_size as f64 / elapsed.as_millis().max(1) as f64
        );

        Ok(Self {
            view_account_key,
            spend_key_lookup,
            subaddress_min: min_index,
            subaddress_max: max_index,
        })
    }

    /// Get the view account key.
    pub fn view_account_key(&self) -> &ViewAccountKey {
        &self.view_account_key
    }

    /// Get the subaddress range being scanned.
    pub fn subaddress_range(&self) -> (u64, u64) {
        (self.subaddress_min, self.subaddress_max)
    }

    /// Get the number of subaddresses being scanned.
    pub fn subaddress_count(&self) -> u64 {
        self.subaddress_max - self.subaddress_min + 1
    }

    /// Check if an output belongs to any of our subaddresses.
    ///
    /// Returns `Some(subaddress_index)` if owned, `None` otherwise.
    ///
    /// This uses the precomputed lookup table for O(1) detection.
    pub fn check_ownership(&self, target_key: &[u8; 32], public_key: &[u8; 32]) -> Option<u64> {
        // Parse keys
        let public_key_point = RistrettoPublic::try_from(&public_key[..]).ok()?;
        let target_key_point = RistrettoPublic::try_from(&target_key[..]).ok()?;

        // Recover the spend public key that would correspond to this output
        let recovered_spend_key = recover_public_subaddress_spend_key(
            self.view_account_key.view_private_key(),
            &target_key_point,
            &public_key_point,
        );

        // Look up in precomputed table
        let recovered_bytes = recovered_spend_key.to_bytes();
        self.spend_key_lookup.get(&recovered_bytes).copied()
    }

    /// Scan a batch of outputs and return detected deposits.
    ///
    /// # Arguments
    /// * `outputs` - List of outputs to scan
    /// * `chain_height` - Current chain height (for calculating confirmations)
    pub fn scan_outputs(&self, outputs: &[RpcOutput], chain_height: u64) -> Vec<DetectedDeposit> {
        let mut deposits = Vec::new();

        for output in outputs {
            // Parse target_key
            let target_key = match parse_key_32(&output.target_key) {
                Some(k) => k,
                None => continue,
            };

            // Parse public_key
            let public_key = match parse_key_32(&output.public_key) {
                Some(k) => k,
                None => continue,
            };

            // Check ownership
            if let Some(subaddress_index) = self.check_ownership(&target_key, &public_key) {
                let tx_hash = match parse_key_32(&output.tx_hash) {
                    Some(h) => h,
                    None => continue,
                };

                let confirmations = chain_height.saturating_sub(output.block_height) + 1;

                deposits.push(DetectedDeposit::new(
                    tx_hash,
                    output.output_index,
                    subaddress_index,
                    output.amount,
                    output.block_height,
                    confirmations,
                    target_key,
                    public_key,
                ));
            }
        }

        deposits
    }

    /// Expand the subaddress range (adds new entries to lookup table).
    ///
    /// This is useful if you need to add more customer subaddresses without
    /// restarting the scanner.
    pub fn expand_range(&mut self, new_max: u64) {
        if new_max <= self.subaddress_max {
            return;
        }

        let new_count = new_max - self.subaddress_max;
        tracing::info!(
            "Expanding subaddress range: {} -> {} (+{} addresses)",
            self.subaddress_max,
            new_max,
            new_count
        );

        let start = std::time::Instant::now();
        for index in (self.subaddress_max + 1)..=new_max {
            let subaddress = self.view_account_key.subaddress(index);
            let spend_bytes = subaddress.spend_public_key().to_bytes();
            self.spend_key_lookup.insert(spend_bytes, index);
        }
        let elapsed = start.elapsed();

        tracing::info!("Added {} subaddress keys in {:?}", new_count, elapsed);

        self.subaddress_max = new_max;
    }

    /// Get the public address for a specific subaddress index.
    ///
    /// Returns `None` if the index is outside the configured range.
    pub fn get_subaddress(&self, index: u64) -> Option<bth_account_keys::PublicAddress> {
        if index < self.subaddress_min || index > self.subaddress_max {
            return None;
        }
        Some(self.view_account_key.subaddress(index))
    }
}

/// Output data from RPC response.
#[derive(Debug, Clone)]
pub struct RpcOutput {
    /// Transaction hash (hex)
    pub tx_hash: String,
    /// Output index within transaction
    pub output_index: u32,
    /// One-time target key (hex)
    pub target_key: String,
    /// Ephemeral public key (hex)
    pub public_key: String,
    /// Amount in picocredits
    pub amount: u64,
    /// Block height containing this output
    pub block_height: u64,
}

/// Parse a 32-byte key from hex string.
fn parse_key_32(hex_str: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() >= 32 {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[..32]);
        Some(key)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_crypto_keys::RistrettoPrivate;
    use bth_util_from_random::FromRandom;
    use rand_core::SeedableRng;

    fn create_test_scanner() -> ExchangeScanner {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let view_private = RistrettoPrivate::from_random(&mut rng);
        let spend_private = RistrettoPrivate::from_random(&mut rng);
        let spend_public = RistrettoPublic::from(&spend_private);

        ExchangeScanner::new(view_private, spend_public, 0, 100).unwrap()
    }

    #[test]
    fn test_scanner_creation() {
        let scanner = create_test_scanner();
        assert_eq!(scanner.subaddress_count(), 101);
        assert_eq!(scanner.subaddress_range(), (0, 100));
    }

    #[test]
    fn test_expand_range() {
        let mut scanner = create_test_scanner();
        assert_eq!(scanner.subaddress_count(), 101);

        scanner.expand_range(200);
        assert_eq!(scanner.subaddress_count(), 201);
        assert_eq!(scanner.subaddress_range(), (0, 200));
    }

    #[test]
    fn test_get_subaddress() {
        let scanner = create_test_scanner();

        assert!(scanner.get_subaddress(0).is_some());
        assert!(scanner.get_subaddress(100).is_some());
        assert!(scanner.get_subaddress(101).is_none());
    }
}
