//! Per-API-key rate limiting for RPC endpoints.
//!
//! This module provides sliding window rate limiting with:
//! - Configurable limits per API key tier
//! - Standard HTTP rate limit headers (X-RateLimit-*)
//! - Proper 429 Too Many Requests responses with Retry-After

use std::{
    collections::HashMap,
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

/// Default rate limit: 100 requests per minute
pub const DEFAULT_RATE_LIMIT: u32 = 100;

/// Rate limit window in seconds
const WINDOW_SECONDS: u64 = 60;

/// API key tier with associated rate limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTier {
    /// Free tier: 100 requests/minute (default)
    Free,
    /// Basic tier: 500 requests/minute
    Basic,
    /// Pro tier: 2000 requests/minute
    Pro,
    /// Enterprise tier: 10000 requests/minute
    Enterprise,
    /// Custom tier with specific limit
    Custom(u32),
}

impl KeyTier {
    /// Get the rate limit for this tier (requests per minute).
    pub fn rate_limit(&self) -> u32 {
        match self {
            KeyTier::Free => 100,
            KeyTier::Basic => 500,
            KeyTier::Pro => 2000,
            KeyTier::Enterprise => 10000,
            KeyTier::Custom(limit) => *limit,
        }
    }

    /// Parse tier from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "free" => KeyTier::Free,
            "basic" => KeyTier::Basic,
            "pro" => KeyTier::Pro,
            "enterprise" => KeyTier::Enterprise,
            _ => {
                // Try to parse as number for custom limit
                if let Ok(limit) = s.parse::<u32>() {
                    KeyTier::Custom(limit)
                } else {
                    KeyTier::Free
                }
            }
        }
    }
}

impl Default for KeyTier {
    fn default() -> Self {
        KeyTier::Free
    }
}

/// Rate limit information for a request.
#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    /// Maximum requests allowed in the window
    pub limit: u32,
    /// Remaining requests in the current window
    pub remaining: u32,
    /// Unix timestamp when the window resets
    pub reset: u64,
    /// Whether the request is allowed
    pub allowed: bool,
    /// Seconds until rate limit resets (for Retry-After header)
    pub retry_after: Option<u64>,
}

impl RateLimitInfo {
    /// Create info for an allowed request.
    fn allowed(limit: u32, remaining: u32, reset: u64) -> Self {
        Self {
            limit,
            remaining,
            reset,
            allowed: true,
            retry_after: None,
        }
    }

    /// Create info for a rate-limited request.
    fn limited(limit: u32, reset: u64, retry_after: u64) -> Self {
        Self {
            limit,
            remaining: 0,
            reset,
            allowed: false,
            retry_after: Some(retry_after),
        }
    }
}

/// Per-key rate limit state using sliding window.
struct KeyRateLimitState {
    /// Request timestamps within the current window
    requests: Vec<u64>,
    /// Rate limit for this key
    limit: u32,
}

impl KeyRateLimitState {
    fn new(limit: u32) -> Self {
        Self {
            requests: Vec::with_capacity(limit as usize),
            limit,
        }
    }

    /// Check rate limit and record request if allowed.
    /// Returns (allowed, remaining, oldest_request_in_window).
    fn check_and_record(&mut self, now: u64) -> (bool, u32, Option<u64>) {
        // Remove requests older than the window
        let cutoff = now.saturating_sub(WINDOW_SECONDS);
        self.requests.retain(|&t| t > cutoff);

        // Get oldest request in window for reset calculation
        let oldest = self.requests.first().copied();

        // Check if we're at the limit
        let current_count = self.requests.len() as u32;
        if current_count >= self.limit {
            return (false, 0, oldest);
        }

        // Record this request
        self.requests.push(now);
        let remaining = self.limit.saturating_sub(current_count + 1);

        (true, remaining, oldest)
    }

    /// Get current state without recording a request.
    fn peek(&self, now: u64) -> (u32, Option<u64>) {
        let cutoff = now.saturating_sub(WINDOW_SECONDS);
        let valid_requests: Vec<_> = self.requests.iter().filter(|&&t| t > cutoff).collect();
        let oldest = valid_requests.first().copied().copied();
        let remaining = self.limit.saturating_sub(valid_requests.len() as u32);
        (remaining, oldest)
    }
}

/// Thread-safe rate limiter for API keys.
pub struct RateLimiter {
    /// Per-key rate limit states
    states: RwLock<HashMap<String, KeyRateLimitState>>,
    /// Per-key tier configuration
    key_tiers: RwLock<HashMap<String, KeyTier>>,
    /// Default tier for unknown keys
    default_tier: KeyTier,
}

impl RateLimiter {
    /// Create a new rate limiter with default settings.
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
            key_tiers: RwLock::new(HashMap::new()),
            default_tier: KeyTier::Free,
        }
    }

    /// Create a rate limiter with a custom default tier.
    pub fn with_default_tier(default_tier: KeyTier) -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
            key_tiers: RwLock::new(HashMap::new()),
            default_tier,
        }
    }

    /// Configure the tier for a specific API key.
    pub fn set_key_tier(&self, key_id: &str, tier: KeyTier) {
        if let Ok(mut tiers) = self.key_tiers.write() {
            tiers.insert(key_id.to_string(), tier);
        }
    }

    /// Get the tier for an API key.
    pub fn get_key_tier(&self, key_id: &str) -> KeyTier {
        self.key_tiers
            .read()
            .ok()
            .and_then(|tiers| tiers.get(key_id).copied())
            .unwrap_or(self.default_tier)
    }

    /// Check rate limit for an API key and record the request if allowed.
    ///
    /// Returns rate limit info including whether the request is allowed,
    /// remaining quota, and reset time.
    pub fn check(&self, key_id: &str) -> RateLimitInfo {
        let now = current_timestamp();
        let tier = self.get_key_tier(key_id);
        let limit = tier.rate_limit();

        let mut states = match self.states.write() {
            Ok(s) => s,
            Err(_) => {
                // Lock poisoned, allow request but don't track
                return RateLimitInfo::allowed(limit, limit, now + WINDOW_SECONDS);
            }
        };

        let state = states
            .entry(key_id.to_string())
            .or_insert_with(|| KeyRateLimitState::new(limit));

        // Update limit if tier changed
        if state.limit != limit {
            state.limit = limit;
        }

        let (allowed, remaining, oldest) = state.check_and_record(now);

        // Calculate reset time (when oldest request in window expires)
        let reset = oldest
            .map(|t| t + WINDOW_SECONDS)
            .unwrap_or(now + WINDOW_SECONDS);

        if allowed {
            RateLimitInfo::allowed(limit, remaining, reset)
        } else {
            let retry_after = reset.saturating_sub(now);
            RateLimitInfo::limited(limit, reset, retry_after)
        }
    }

    /// Get current rate limit status without recording a request.
    pub fn status(&self, key_id: &str) -> RateLimitInfo {
        let now = current_timestamp();
        let tier = self.get_key_tier(key_id);
        let limit = tier.rate_limit();

        let states = match self.states.read() {
            Ok(s) => s,
            Err(_) => {
                return RateLimitInfo::allowed(limit, limit, now + WINDOW_SECONDS);
            }
        };

        if let Some(state) = states.get(key_id) {
            let (remaining, oldest) = state.peek(now);
            let reset = oldest
                .map(|t| t + WINDOW_SECONDS)
                .unwrap_or(now + WINDOW_SECONDS);
            RateLimitInfo::allowed(limit, remaining, reset)
        } else {
            RateLimitInfo::allowed(limit, limit, now + WINDOW_SECONDS)
        }
    }

    /// Clear rate limit state for a key (for testing).
    #[cfg(test)]
    pub fn clear(&self, key_id: &str) {
        if let Ok(mut states) = self.states.write() {
            states.remove(key_id);
        }
    }

    /// Clean up expired entries to prevent memory growth.
    pub fn cleanup(&self) {
        let now = current_timestamp();
        let cutoff = now.saturating_sub(WINDOW_SECONDS * 2);

        if let Ok(mut states) = self.states.write() {
            states.retain(|_, state| {
                // Keep if any request is recent enough
                state.requests.iter().any(|&t| t > cutoff)
            });
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current Unix timestamp in seconds.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_tier_rate_limits() {
        assert_eq!(KeyTier::Free.rate_limit(), 100);
        assert_eq!(KeyTier::Basic.rate_limit(), 500);
        assert_eq!(KeyTier::Pro.rate_limit(), 2000);
        assert_eq!(KeyTier::Enterprise.rate_limit(), 10000);
        assert_eq!(KeyTier::Custom(42).rate_limit(), 42);
    }

    #[test]
    fn test_key_tier_from_str() {
        assert_eq!(KeyTier::from_str("free"), KeyTier::Free);
        assert_eq!(KeyTier::from_str("FREE"), KeyTier::Free);
        assert_eq!(KeyTier::from_str("basic"), KeyTier::Basic);
        assert_eq!(KeyTier::from_str("pro"), KeyTier::Pro);
        assert_eq!(KeyTier::from_str("enterprise"), KeyTier::Enterprise);
        assert_eq!(KeyTier::from_str("500"), KeyTier::Custom(500));
        assert_eq!(KeyTier::from_str("unknown"), KeyTier::Free);
    }

    #[test]
    fn test_rate_limiter_allows_requests_within_limit() {
        let limiter = RateLimiter::new();
        limiter.set_key_tier("test-key", KeyTier::Custom(5));

        for i in 0..5 {
            let info = limiter.check("test-key");
            assert!(info.allowed, "Request {} should be allowed", i);
            assert_eq!(info.remaining, 4 - i as u32);
        }

        // 6th request should be denied
        let info = limiter.check("test-key");
        assert!(!info.allowed, "Request 6 should be denied");
        assert_eq!(info.remaining, 0);
        assert!(info.retry_after.is_some());
    }

    #[test]
    fn test_rate_limiter_default_tier() {
        let limiter = RateLimiter::new();
        let tier = limiter.get_key_tier("unknown-key");
        assert_eq!(tier, KeyTier::Free);
    }

    #[test]
    fn test_rate_limiter_custom_default_tier() {
        let limiter = RateLimiter::with_default_tier(KeyTier::Basic);
        let tier = limiter.get_key_tier("unknown-key");
        assert_eq!(tier, KeyTier::Basic);
    }

    #[test]
    fn test_status_does_not_consume_quota() {
        let limiter = RateLimiter::new();
        limiter.set_key_tier("test-key", KeyTier::Custom(5));

        // Check status multiple times
        for _ in 0..10 {
            let info = limiter.status("test-key");
            assert!(info.allowed);
            assert_eq!(info.remaining, 5);
        }

        // All requests should still be available
        let info = limiter.check("test-key");
        assert!(info.allowed);
        assert_eq!(info.remaining, 4);
    }

    #[test]
    fn test_rate_limit_info_headers() {
        let info = RateLimitInfo::allowed(100, 50, 1234567890);
        assert_eq!(info.limit, 100);
        assert_eq!(info.remaining, 50);
        assert_eq!(info.reset, 1234567890);
        assert!(info.allowed);
        assert!(info.retry_after.is_none());

        let info = RateLimitInfo::limited(100, 1234567890, 30);
        assert_eq!(info.limit, 100);
        assert_eq!(info.remaining, 0);
        assert!(!info.allowed);
        assert_eq!(info.retry_after, Some(30));
    }

    #[test]
    fn test_different_keys_have_separate_limits() {
        let limiter = RateLimiter::new();
        limiter.set_key_tier("key-a", KeyTier::Custom(2));
        limiter.set_key_tier("key-b", KeyTier::Custom(2));

        // Exhaust key-a
        limiter.check("key-a");
        limiter.check("key-a");
        let info = limiter.check("key-a");
        assert!(!info.allowed);

        // key-b should still have full quota
        let info = limiter.check("key-b");
        assert!(info.allowed);
        assert_eq!(info.remaining, 1);
    }
}
