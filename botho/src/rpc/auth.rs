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
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

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
            // Transaction methods ("pq_tx_submit" retired, ADR 0006)
            "tx_submit" | "sendRawTransaction" => self.submit_transactions,
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
        let api_keys = keys.into_iter().map(|k| (k.key_id.clone(), k)).collect();

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
        let key_config = self.api_keys.get(key_id).ok_or(AuthError::InvalidApiKey)?;

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
            let mut limits = self
                .rate_limits
                .write()
                .map_err(|_| AuthError::InternalError)?;
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

// ============================================================================
// Operator read-token (magic link) — #707, P4.2 of the #695 proposal
// ============================================================================
//
// A node-verified, expiring, read-only bearer token that unlocks the
// operator-only read RPCs (`operator_getQuorumInfo`, `operator_getAuditLog`).
// It is the exact shape of the BaaS status link
// (`web/packages/baas-worker/src/status-link.ts`), with the node as verifier:
//
//     op.<expUnixSeconds>.<hmacSha256Hex(secret, "op.<exp>")>
//
// The HMAC reuses the SAME `HmacSha256` primitive and the SAME
// `constant_time_eq` compare as the exchange API-key auth above — there is a
// single audited HMAC path in this module, never a second hand-rolled one.
//
// SECURITY POSTURE (mirrors status-link.ts):
//   - The signature is verified BEFORE the expiry field is trusted. An attacker
//     cannot forge or extend a token by tampering with the (unauthenticated)
//     `exp` field: the HMAC covers `"op.<exp>"`, so any change to `exp` fails
//     the constant-time compare before the expiry check ever runs.
//   - The compare is constant-time.
//   - The token is a bearer credential that grants READS ONLY. No RPC reachable
//     with it mutates any state (the write path is #709, gated on a separate
//     security review — `docs/security/quorum-write-path.md`).

/// Fixed subject/domain tag for operator read tokens. Also acts as a domain
/// separator so an operator token can never be confused with any other
/// dot-delimited HMAC token shape.
pub const OPERATOR_TOKEN_SUBJECT: &str = "op";

/// Default lifetime of a freshly minted operator read token: 7 days. Parity
/// with `DEFAULT_STATUS_TOKEN_TTL_SECONDS` in `status-link.ts`.
pub const DEFAULT_OPERATOR_TOKEN_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Why an operator read token was rejected.
///
/// The RPC layer collapses ALL of these into a single generic client-facing
/// error (it must not leak whether a token was malformed vs forged vs expired).
/// The variants exist so tests can assert the internal verification ORDER —
/// specifically that a bad signature is reported even when the token is also
/// expired (signature is checked first, exactly like `status-link.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorTokenError {
    /// Empty, wrong field count, wrong subject tag, or non-integer expiry.
    Malformed,
    /// The HMAC over `"op.<exp>"` did not match (tampered/forged token, or a
    /// token signed with a different secret).
    BadSignature,
    /// The signature was valid but the token's (now-trusted) expiry has passed.
    Expired,
}

/// Compute the hex HMAC-SHA256 of `payload` under `secret`, using the same
/// `HmacSha256` primitive as the exchange API-key auth.
fn hmac_sha256_hex(secret: &str, payload: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Mint an operator read token valid until `exp_unix_seconds`.
///
/// `op.<exp>.<hmacSha256Hex(secret, "op.<exp>")>`. Minted off-node by the
/// `botho operator mint-read-link` CLI; the node is the verifier.
pub fn mint_operator_read_token(secret: &str, exp_unix_seconds: u64) -> String {
    let payload = format!("{OPERATOR_TOKEN_SUBJECT}.{exp_unix_seconds}");
    let sig = hmac_sha256_hex(secret, &payload);
    format!("{payload}.{sig}")
}

/// Verify an operator read token and return its (trusted) expiry on success.
///
/// Verification order (parity with `status-link.ts`, fail closed):
///   1. structural parse: exactly three dot-delimited fields, subject == `op`,
///      integer expiry > 0 (else [`OperatorTokenError::Malformed`]);
///   2. recompute the HMAC over `"op.<exp>"` and compare in CONSTANT TIME —
///      **before** trusting any field ([`OperatorTokenError::BadSignature`]);
///   3. only after the signature passes, honor the (now-trusted) expiry
///      ([`OperatorTokenError::Expired`]).
///
/// `now_unix_seconds` is injectable so tests can pin the clock.
pub fn verify_operator_read_token(
    token: &str,
    secret: &str,
    now_unix_seconds: u64,
) -> Result<u64, OperatorTokenError> {
    if token.is_empty() {
        return Err(OperatorTokenError::Malformed);
    }

    // Expect exactly three fields: subject . exp . signature.
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(OperatorTokenError::Malformed);
    }
    let (subject, exp_str, sig) = (parts[0], parts[1], parts[2]);

    if subject != OPERATOR_TOKEN_SUBJECT {
        return Err(OperatorTokenError::Malformed);
    }
    let exp: u64 = match exp_str.parse() {
        Ok(v) if v > 0 => v,
        _ => return Err(OperatorTokenError::Malformed),
    };

    // Verify the signature BEFORE trusting the expiry. Constant-time compare.
    let expected = hmac_sha256_hex(secret, &format!("{OPERATOR_TOKEN_SUBJECT}.{exp}"));
    if !constant_time_eq(expected.as_bytes(), sig.as_bytes()) {
        return Err(OperatorTokenError::BadSignature);
    }

    // Only now is `exp` trusted — honor it.
    if now_unix_seconds >= exp {
        return Err(OperatorTokenError::Expired);
    }

    Ok(exp)
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

    // ------------------------------------------------------------------
    // Operator read-token (#707)
    // ------------------------------------------------------------------

    const OP_SECRET: &str = "operator-read-token-secret";
    const NOW: u64 = 1_700_000_000;

    #[test]
    fn operator_token_round_trips() {
        let exp = NOW + DEFAULT_OPERATOR_TOKEN_TTL_SECONDS;
        let token = mint_operator_read_token(OP_SECRET, exp);
        // Shape: op.<exp>.<hex-sig>
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "op");
        assert_eq!(parts[1], exp.to_string());
        assert_eq!(parts[2].len(), 64, "hex sha256 is 64 hex chars");

        let got = verify_operator_read_token(&token, OP_SECRET, NOW + 10);
        assert_eq!(got, Ok(exp));
    }

    #[test]
    fn operator_token_missing_or_malformed_rejected() {
        assert_eq!(
            verify_operator_read_token("", OP_SECRET, NOW),
            Err(OperatorTokenError::Malformed)
        );
        assert_eq!(
            verify_operator_read_token("garbage", OP_SECRET, NOW),
            Err(OperatorTokenError::Malformed)
        );
        // Wrong field count.
        assert_eq!(
            verify_operator_read_token("op.123", OP_SECRET, NOW),
            Err(OperatorTokenError::Malformed)
        );
        // Wrong subject tag.
        let exp = NOW + 1000;
        let sig = hmac_sha256_hex(OP_SECRET, &format!("op.{exp}"));
        assert_eq!(
            verify_operator_read_token(&format!("xx.{exp}.{sig}"), OP_SECRET, NOW),
            Err(OperatorTokenError::Malformed)
        );
        // Non-integer expiry.
        assert_eq!(
            verify_operator_read_token(&format!("op.notanumber.{sig}"), OP_SECRET, NOW),
            Err(OperatorTokenError::Malformed)
        );
    }

    #[test]
    fn operator_token_tampered_signature_rejected() {
        let exp = NOW + 1000;
        let token = mint_operator_read_token(OP_SECRET, exp);
        // Tamper the expiry but keep the original signature: the HMAC covers
        // "op.<exp>", so bumping exp must fail the signature check.
        let parts: Vec<&str> = token.split('.').collect();
        let forged = format!("op.{}.{}", exp + 999_999, parts[2]);
        assert_eq!(
            verify_operator_read_token(&forged, OP_SECRET, NOW),
            Err(OperatorTokenError::BadSignature)
        );
    }

    #[test]
    fn operator_token_wrong_secret_rejected() {
        let exp = NOW + 1000;
        let token = mint_operator_read_token("other-secret", exp);
        assert_eq!(
            verify_operator_read_token(&token, OP_SECRET, NOW),
            Err(OperatorTokenError::BadSignature)
        );
    }

    #[test]
    fn operator_token_expired_rejected_only_after_signature_passes() {
        let exp = NOW + 60;
        let token = mint_operator_read_token(OP_SECRET, exp);
        // Valid signature, but the clock is past expiry.
        assert_eq!(
            verify_operator_read_token(&token, OP_SECRET, exp + 1),
            Err(OperatorTokenError::Expired)
        );
    }

    #[test]
    fn operator_token_signature_checked_before_expiry() {
        // A token that is BOTH expired AND has a bad signature must report the
        // SIGNATURE failure, proving the signature is verified before the
        // expiry is trusted (parity with status-link.ts verification order).
        let exp = NOW - 100; // already expired
        let token = mint_operator_read_token(OP_SECRET, exp);
        let parts: Vec<&str> = token.split('.').collect();
        // Corrupt the signature.
        let mut bad_sig: String = parts[2].to_string();
        bad_sig.replace_range(0..1, if &bad_sig[0..1] == "a" { "b" } else { "a" });
        let both_bad = format!("op.{exp}.{bad_sig}");
        assert_eq!(
            verify_operator_read_token(&both_bad, OP_SECRET, NOW),
            Err(OperatorTokenError::BadSignature),
            "signature must be checked before expiry"
        );
    }
}
