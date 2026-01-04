//! View key registry for server-side deposit scanning.
//!
//! This module provides storage and scanning for registered exchange view keys.
//! Exchanges can register their view keys to receive real-time deposit
//! notifications via WebSocket when outputs matching their keys are detected in
//! new blocks.

use bth_account_keys::ViewAccountKey;
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::onetime_keys::recover_public_subaddress_spend_key;
use std::{collections::HashMap, sync::RwLock};

/// A registered view key for deposit scanning.
#[derive(Clone)]
pub struct RegisteredViewKey {
    /// Unique identifier for this registration
    pub id: String,
    /// The view account key for scanning
    view_account_key: ViewAccountKey,
    /// Minimum subaddress index to scan
    pub subaddress_min: u64,
    /// Maximum subaddress index to scan
    pub subaddress_max: u64,
    /// Precomputed spend key lookup table for O(1) detection
    spend_key_lookup: HashMap<[u8; 32], u64>,
    /// API key ID that registered this view key
    pub api_key_id: String,
    /// Registration timestamp
    pub registered_at: u64,
}

impl RegisteredViewKey {
    /// Check if an output belongs to this view key's subaddresses.
    ///
    /// Returns `Some(subaddress_index)` if owned, `None` otherwise.
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

    /// Get the subaddress range size.
    pub fn subaddress_count(&self) -> u64 {
        self.subaddress_max - self.subaddress_min + 1
    }
}

/// View key registry for server-side scanning.
///
/// This registry stores view keys registered by exchanges for deposit
/// detection. When new blocks arrive, all registered view keys are scanned
/// against the outputs.
pub struct ViewKeyRegistry {
    /// Registered view keys indexed by ID
    view_keys: RwLock<HashMap<String, RegisteredViewKey>>,
    /// Maximum number of registered view keys (for resource limits)
    max_registrations: usize,
    /// Maximum subaddress range per registration
    max_subaddress_range: u64,
}

impl ViewKeyRegistry {
    /// Create a new view key registry.
    pub fn new() -> Self {
        Self {
            view_keys: RwLock::new(HashMap::new()),
            max_registrations: 100,
            max_subaddress_range: 100_000,
        }
    }

    /// Create a registry with custom limits.
    pub fn with_limits(max_registrations: usize, max_subaddress_range: u64) -> Self {
        Self {
            view_keys: RwLock::new(HashMap::new()),
            max_registrations,
            max_subaddress_range,
        }
    }

    /// Register a view key for deposit scanning.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this registration
    /// * `view_private_key` - The exchange's view private key
    /// * `spend_public_key` - The exchange's spend public key
    /// * `subaddress_min` - Minimum subaddress index to scan
    /// * `subaddress_max` - Maximum subaddress index to scan
    /// * `api_key_id` - API key that is registering this view key
    pub fn register(
        &self,
        id: String,
        view_private_key: RistrettoPrivate,
        spend_public_key: RistrettoPublic,
        subaddress_min: u64,
        subaddress_max: u64,
        api_key_id: String,
    ) -> Result<(), RegistryError> {
        // Validate range
        if subaddress_max < subaddress_min {
            return Err(RegistryError::InvalidRange);
        }

        let range_size = subaddress_max - subaddress_min + 1;
        if range_size > self.max_subaddress_range {
            return Err(RegistryError::RangeTooLarge(self.max_subaddress_range));
        }

        let mut keys = self
            .view_keys
            .write()
            .map_err(|_| RegistryError::LockPoisoned)?;

        // Check registration limit
        if keys.len() >= self.max_registrations && !keys.contains_key(&id) {
            return Err(RegistryError::TooManyRegistrations(self.max_registrations));
        }

        // Check for duplicate ID from different API key
        if let Some(existing) = keys.get(&id) {
            if existing.api_key_id != api_key_id {
                return Err(RegistryError::IdAlreadyExists);
            }
        }

        tracing::info!(
            "Registering view key '{}' with {} subaddresses ({}-{})",
            id,
            range_size,
            subaddress_min,
            subaddress_max
        );

        let view_account_key = ViewAccountKey::new(view_private_key, spend_public_key);

        // Precompute all subaddress spend keys for O(1) lookup
        let mut spend_key_lookup = HashMap::with_capacity(range_size as usize);

        let start = std::time::Instant::now();
        for index in subaddress_min..=subaddress_max {
            let subaddress = view_account_key.subaddress(index);
            let spend_bytes = subaddress.spend_public_key().to_bytes();
            spend_key_lookup.insert(spend_bytes, index);
        }
        let elapsed = start.elapsed();

        tracing::debug!(
            "Precomputed {} subaddress keys in {:?}",
            range_size,
            elapsed
        );

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let registered = RegisteredViewKey {
            id: id.clone(),
            view_account_key,
            subaddress_min,
            subaddress_max,
            spend_key_lookup,
            api_key_id,
            registered_at: now,
        };

        keys.insert(id, registered);

        Ok(())
    }

    /// Unregister a view key.
    ///
    /// Only the API key that registered the view key can unregister it.
    pub fn unregister(&self, id: &str, api_key_id: &str) -> Result<(), RegistryError> {
        let mut keys = self
            .view_keys
            .write()
            .map_err(|_| RegistryError::LockPoisoned)?;

        if let Some(registered) = keys.get(id) {
            if registered.api_key_id != api_key_id {
                return Err(RegistryError::NotAuthorized);
            }
            keys.remove(id);
            tracing::info!("Unregistered view key '{}'", id);
            Ok(())
        } else {
            Err(RegistryError::NotFound)
        }
    }

    /// List all view keys registered by a specific API key.
    pub fn list_by_api_key(&self, api_key_id: &str) -> Result<Vec<ViewKeyInfo>, RegistryError> {
        let keys = self
            .view_keys
            .read()
            .map_err(|_| RegistryError::LockPoisoned)?;

        let infos: Vec<ViewKeyInfo> = keys
            .values()
            .filter(|k| k.api_key_id == api_key_id)
            .map(|k| ViewKeyInfo {
                id: k.id.clone(),
                subaddress_min: k.subaddress_min,
                subaddress_max: k.subaddress_max,
                subaddress_count: k.subaddress_count(),
                registered_at: k.registered_at,
            })
            .collect();

        Ok(infos)
    }

    /// Scan an output against all registered view keys.
    ///
    /// Returns a list of (view_key_id, subaddress_index) for all matches.
    pub fn scan_output(&self, target_key: &[u8; 32], public_key: &[u8; 32]) -> Vec<(String, u64)> {
        let keys = match self.view_keys.read() {
            Ok(k) => k,
            Err(_) => return Vec::new(),
        };

        let mut matches = Vec::new();

        for registered in keys.values() {
            if let Some(subaddress_index) = registered.check_ownership(target_key, public_key) {
                matches.push((registered.id.clone(), subaddress_index));
            }
        }

        matches
    }

    /// Get the number of registered view keys.
    pub fn count(&self) -> usize {
        self.view_keys.read().map(|k| k.len()).unwrap_or(0)
    }

    /// Check if a view key is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.view_keys
            .read()
            .map(|k| k.contains_key(id))
            .unwrap_or(false)
    }
}

impl Default for ViewKeyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Public information about a registered view key.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ViewKeyInfo {
    /// Registration ID
    pub id: String,
    /// Minimum subaddress index
    pub subaddress_min: u64,
    /// Maximum subaddress index
    pub subaddress_max: u64,
    /// Number of subaddresses being scanned
    pub subaddress_count: u64,
    /// Unix timestamp of registration
    pub registered_at: u64,
}

/// Errors that can occur with the view key registry.
#[derive(Debug, Clone)]
pub enum RegistryError {
    /// Invalid subaddress range (max < min)
    InvalidRange,
    /// Subaddress range exceeds maximum allowed
    RangeTooLarge(u64),
    /// Maximum number of registrations reached
    TooManyRegistrations(usize),
    /// View key ID already exists (registered by different API key)
    IdAlreadyExists,
    /// View key not found
    NotFound,
    /// Caller not authorized to modify this view key
    NotAuthorized,
    /// Internal lock error
    LockPoisoned,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::InvalidRange => write!(f, "Invalid subaddress range"),
            RegistryError::RangeTooLarge(max) => {
                write!(f, "Subaddress range too large (max {})", max)
            }
            RegistryError::TooManyRegistrations(max) => {
                write!(f, "Maximum registrations reached ({})", max)
            }
            RegistryError::IdAlreadyExists => write!(f, "View key ID already exists"),
            RegistryError::NotFound => write!(f, "View key not found"),
            RegistryError::NotAuthorized => write!(f, "Not authorized"),
            RegistryError::LockPoisoned => write!(f, "Internal error"),
        }
    }
}

impl std::error::Error for RegistryError {}

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
    fn test_register_and_unregister() {
        let registry = ViewKeyRegistry::new();
        let (view_private, spend_public) = create_test_keys();

        // Register
        let result = registry.register(
            "test-key".to_string(),
            view_private,
            spend_public,
            0,
            100,
            "api-key-1".to_string(),
        );
        assert!(result.is_ok());
        assert_eq!(registry.count(), 1);

        // Unregister with wrong API key should fail
        let result = registry.unregister("test-key", "wrong-api-key");
        assert!(matches!(result, Err(RegistryError::NotAuthorized)));

        // Unregister with correct API key
        let result = registry.unregister("test-key", "api-key-1");
        assert!(result.is_ok());
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_list_by_api_key() {
        let registry = ViewKeyRegistry::new();
        let (view_private, spend_public) = create_test_keys();

        // Register multiple keys
        registry
            .register(
                "key-1".to_string(),
                view_private,
                spend_public,
                0,
                100,
                "api-1".to_string(),
            )
            .unwrap();

        let (view_private2, spend_public2) = create_test_keys();
        registry
            .register(
                "key-2".to_string(),
                view_private2,
                spend_public2,
                0,
                50,
                "api-1".to_string(),
            )
            .unwrap();

        let (view_private3, spend_public3) = create_test_keys();
        registry
            .register(
                "key-3".to_string(),
                view_private3,
                spend_public3,
                0,
                200,
                "api-2".to_string(),
            )
            .unwrap();

        // List for api-1
        let list = registry.list_by_api_key("api-1").unwrap();
        assert_eq!(list.len(), 2);

        // List for api-2
        let list = registry.list_by_api_key("api-2").unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_range_limits() {
        let registry = ViewKeyRegistry::with_limits(10, 1000);
        let (view_private, spend_public) = create_test_keys();

        // Too large range
        let result = registry.register(
            "test".to_string(),
            view_private,
            spend_public,
            0,
            2000,
            "api".to_string(),
        );
        assert!(matches!(result, Err(RegistryError::RangeTooLarge(_))));
    }

    #[test]
    fn test_invalid_range() {
        let registry = ViewKeyRegistry::new();
        let (view_private, spend_public) = create_test_keys();

        // max < min
        let result = registry.register(
            "test".to_string(),
            view_private,
            spend_public,
            100,
            50,
            "api".to_string(),
        );
        assert!(matches!(result, Err(RegistryError::InvalidRange)));
    }
}
