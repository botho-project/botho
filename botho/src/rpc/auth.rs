//! API Key + HMAC authentication for exchange endpoints.
//!
//! This module provides HMAC-SHA256 based authentication for securing
//! exchange-specific RPC endpoints. It supports:
//!
//! - API key identification
//! - Request signing with HMAC-SHA256
//! - Timestamp validation (replay protection)
//! - Permission-based access control
//! - Optional IP whitelisting
//! - Rate limiting per API key

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// API key configuration.
#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    /// API key ID (public identifier)
    pub key_id: String,
    /// API key secret (for HMAC signing)
    pub key_secret: String,
    /// Permissions for this key
    pub permissions: ApiPermissions,
    /// Rate limit (requests per minute)
    pub rate_limit: u32,
    /// Optional IP whitelist
    pub ip_whitelist: Option<Vec<IpAddr>>,
}

/// Permissions for an API key.
#[derive(Debug, Clone, Default)]
pub struct ApiPermissions {
    /// Can access exchange-specific endpoints
    pub exchange_api: bool,
    /// Can register view keys for notifications
    pub register_view_keys: bool,
    /// Can submit transactions
    pub submit_transactions: bool,
}

impl ApiPermissions {
    /// Create permissions with all access.
    pub fn all() -> Self {
        Self {
            exchange_api: true,
            register_view_keys: true,
            submit_transactions: true,
        }
    }

    /// Check if the given method is allowed.
    pub fn allows_method(&self, method: &str) -> bool {
        match method {
            // Exchange-specific methods
            "exchange_registerViewKey" | "exchange_unregisterViewKey" | "exchange_listViewKeys" => {
                self.exchange_api && self.register_view_keys
            }
            // Transaction methods
            "tx_submit" | "sendRawTransaction" | "pq_tx_submit" => self.submit_transactions,
            // All other methods are allowed by default
            _ => true,
        }
    }
}

/// Authentication error types.
#[derive(Debug, Clone)]
pub enum AuthError {
    /// API key not found
    InvalidApiKey,
    /// Signature verification failed
    InvalidSignature,
    /// Request timestamp is too old or in the future
    TimestampExpired,
    /// API key doesn't have required permissions
    InsufficientPermissions,
    /// Rate limit exceeded
    RateLimitExceeded,
    /// Client IP not in whitelist
    IpNotAllowed,
    /// Missing required header
    MissingHeader(String),
    /// Internal error
    InternalError,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::InvalidApiKey => write!(f, "Invalid API key"),
            AuthError::InvalidSignature => write!(f, "Invalid signature"),
            AuthError::TimestampExpired => write!(f, "Timestamp expired or invalid"),
            AuthError::InsufficientPermissions => write!(f, "Insufficient permissions"),
            AuthError::RateLimitExceeded => write!(f, "Rate limit exceeded"),
            AuthError::IpNotAllowed => write!(f, "IP address not allowed"),
            AuthError::MissingHeader(h) => write!(f, "Missing header: {}", h),
            AuthError::InternalError => write!(f, "Internal authentication error"),
        }
    }
}

/// Rate limit tracker for a single API key.
struct RateLimitState {
    /// Request timestamps within the current window
    requests: Vec<u64>,
    /// Requests per minute limit
    limit: u32,
}

impl RateLimitState {
    fn new(limit: u32) -> Self {
        Self {
            requests: Vec::new(),
            limit,
        }
    }

    /// Check if a request is allowed and record it if so.
    fn check_and_record(&mut self, now: u64) -> bool {
        // Remove requests older than 60 seconds
        let cutoff = now.saturating_sub(60);
        self.requests.retain(|&t| t > cutoff);

        // Check limit
        if self.requests.len() as u32 >= self.limit {
            return false;
        }

        // Record this request
        self.requests.push(now);
        true
    }
}

/// HMAC authentication validator.
pub struct HmacAuthenticator {
    /// API keys indexed by key_id
    api_keys: HashMap<String, ApiKeyConfig>,
    /// Rate limit state per key
    rate_limits: RwLock<HashMap<String, RateLimitState>>,
    /// Maximum timestamp skew allowed (seconds)
    max_timestamp_skew: u64,
}

impl HmacAuthenticator {
    /// Create a new authenticator with the given API keys.
    pub fn new(keys: Vec<ApiKeyConfig>) -> Self {
        let api_keys = keys
            .into_iter()
            .map(|k| (k.key_id.clone(), k))
            .collect();

        Self {
            api_keys,
            rate_limits: RwLock::new(HashMap::new()),
            max_timestamp_skew: 300, // 5 minutes
        }
    }

    /// Create an authenticator with custom timestamp skew.
    pub fn with_timestamp_skew(keys: Vec<ApiKeyConfig>, max_skew_seconds: u64) -> Self {
        let mut auth = Self::new(keys);
        auth.max_timestamp_skew = max_skew_seconds;
        auth
    }

    /// Validate an HMAC-signed request.
    ///
    /// # Headers required:
    /// - `X-API-Key`: The API key ID
    /// - `X-Timestamp`: Unix timestamp in milliseconds
    /// - `X-Signature`: HMAC-SHA256 signature (hex)
    ///
    /// # Signature computation:
    /// The signature is computed over: `timestamp + method + body`
    pub fn validate(
        &self,
        key_id: &str,
        timestamp: u64,
        signature: &str,
        method: &str,
        body: &[u8],
        client_ip: Option<IpAddr>,
    ) -> Result<&ApiKeyConfig, AuthError> {
        // Get API key config
        let key_config = self
            .api_keys
            .get(key_id)
            .ok_or(AuthError::InvalidApiKey)?;

        // Check IP whitelist if configured
        if let Some(ref whitelist) = key_config.ip_whitelist {
            if let Some(ip) = client_ip {
                if !whitelist.contains(&ip) {
                    return Err(AuthError::IpNotAllowed);
                }
            }
        }

        // Validate timestamp (prevent replay attacks)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let skew = now.abs_diff(timestamp);

        // Convert max skew to milliseconds for comparison
        if skew > self.max_timestamp_skew * 1000 {
            return Err(AuthError::TimestampExpired);
        }

        // Compute expected signature
        let mut mac = HmacSha256::new_from_slice(key_config.key_secret.as_bytes())
            .map_err(|_| AuthError::InternalError)?;

        mac.update(timestamp.to_string().as_bytes());
        mac.update(method.as_bytes());
        mac.update(body);

        let expected = hex::encode(mac.finalize().into_bytes());

        // Constant-time comparison
        if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
            return Err(AuthError::InvalidSignature);
        }

        // Check rate limit
        let now_secs = now / 1000;
        {
            let mut limits = self.rate_limits.write().map_err(|_| AuthError::InternalError)?;
            let state = limits
                .entry(key_id.to_string())
                .or_insert_with(|| RateLimitState::new(key_config.rate_limit));

            if !state.check_and_record(now_secs) {
                return Err(AuthError::RateLimitExceeded);
            }
        }

        Ok(key_config)
    }

    /// Check if a method requires authentication.
    pub fn requires_auth(&self, method: &str) -> bool {
        matches!(
            method,
            "exchange_registerViewKey" | "exchange_unregisterViewKey" | "exchange_listViewKeys"
        )
    }

    /// Get an API key config by ID.
    pub fn get_key(&self, key_id: &str) -> Option<&ApiKeyConfig> {
        self.api_keys.get(key_id)
    }

    /// Check if any API keys are configured.
    pub fn is_enabled(&self) -> bool {
        !self.api_keys.is_empty()
    }
}

/// Constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// Extract authentication headers from an HTTP request.
pub struct AuthHeaders {
    pub api_key: String,
    pub timestamp: u64,
    pub signature: String,
}

impl AuthHeaders {
    /// Parse authentication headers from header map.
    pub fn from_headers(headers: &hyper::HeaderMap) -> Result<Self, AuthError> {
        let api_key = headers
            .get("X-API-Key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| AuthError::MissingHeader("X-API-Key".to_string()))?;

        let timestamp = headers
            .get("X-Timestamp")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| AuthError::MissingHeader("X-Timestamp".to_string()))?;

        let signature = headers
            .get("X-Signature")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| AuthError::MissingHeader("X-Signature".to_string()))?;

        Ok(Self {
            api_key,
            timestamp,
            signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_authenticator() -> HmacAuthenticator {
        let key = ApiKeyConfig {
            key_id: "test-key".to_string(),
            key_secret: "test-secret".to_string(),
            permissions: ApiPermissions::all(),
            rate_limit: 100,
            ip_whitelist: None,
        };
        HmacAuthenticator::new(vec![key])
    }

    fn compute_signature(secret: &str, timestamp: u64, method: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.to_string().as_bytes());
        mac.update(method.as_bytes());
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn test_valid_signature() {
        let auth = create_test_authenticator();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let method = "exchange_registerViewKey";
        let body = b"{}";
        let signature = compute_signature("test-secret", now, method, body);

        let result = auth.validate("test-key", now, &signature, method, body, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_invalid_api_key() {
        let auth = create_test_authenticator();
        let result = auth.validate("invalid-key", 0, "sig", "method", b"", None);
        assert!(matches!(result, Err(AuthError::InvalidApiKey)));
    }

    #[test]
    fn test_invalid_signature() {
        let auth = create_test_authenticator();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let result = auth.validate("test-key", now, "invalid", "method", b"", None);
        assert!(matches!(result, Err(AuthError::InvalidSignature)));
    }

    #[test]
    fn test_expired_timestamp() {
        let auth = create_test_authenticator();

        let old_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - (10 * 60 * 1000); // 10 minutes ago

        let signature = compute_signature("test-secret", old_timestamp, "method", b"");

        let result = auth.validate("test-key", old_timestamp, &signature, "method", b"", None);
        assert!(matches!(result, Err(AuthError::TimestampExpired)));
    }

    #[test]
    fn test_permissions() {
        let perms = ApiPermissions::all();
        assert!(perms.allows_method("exchange_registerViewKey"));
        assert!(perms.allows_method("getChainInfo"));

        let limited = ApiPermissions {
            exchange_api: false,
            ..Default::default()
        };
        assert!(!limited.allows_method("exchange_registerViewKey"));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"hell"));
    }
}
