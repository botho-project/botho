//! Faucet for testnet coin distribution.
//!
//! Provides rate-limited coin distribution for testing purposes.
//! The faucet is only available on testnet and must be explicitly enabled.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

use crate::config::FaucetConfig;

/// Faucet error types
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FaucetError {
    /// Faucet is disabled
    Disabled,
    /// Faucet is only available on testnet
    MainnetNotAllowed,
    /// Rate limited - too many requests from this IP
    IpRateLimited {
        retry_after_secs: u64,
        requests_this_hour: u32,
        limit: u32,
    },
    /// Rate limited - too many requests to this address
    AddressRateLimited {
        retry_after_secs: u64,
        requests_today: u32,
        limit: u32,
    },
    /// Rate limited - cooldown between requests
    CooldownActive { retry_after_secs: u64 },
    /// Rate limited - daily limit reached
    DailyLimitReached {
        dispensed_today: u64,
        limit: u64,
        retry_after_secs: u64,
    },
    /// No wallet configured (faucet needs a wallet to send from)
    NoWallet,
    /// Insufficient balance in faucet wallet
    InsufficientBalance { available: u64, requested: u64 },
    /// Invalid address format
    InvalidAddress(String),
    /// Transaction failed
    TransactionFailed(String),
}

impl FaucetError {
    /// Get the retry-after duration in seconds (0 if not rate-limited)
    pub fn retry_after_secs(&self) -> u64 {
        match self {
            Self::IpRateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            Self::AddressRateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            Self::CooldownActive { retry_after_secs } => *retry_after_secs,
            Self::DailyLimitReached {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => 0,
        }
    }

    /// Convert to user-friendly message
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => "Faucet is disabled on this node".to_string(),
            Self::MainnetNotAllowed => "Faucet is only available on testnet".to_string(),
            Self::IpRateLimited {
                retry_after_secs,
                requests_this_hour,
                limit,
            } => format!(
                "Too many requests from your IP ({}/{} this hour). Try again in {} seconds.",
                requests_this_hour, limit, retry_after_secs
            ),
            Self::AddressRateLimited {
                retry_after_secs,
                requests_today,
                limit,
            } => format!(
                "Too many requests for this address ({}/{} today). Try again in {} seconds.",
                requests_today, limit, retry_after_secs
            ),
            Self::CooldownActive { retry_after_secs } => {
                format!(
                    "Please wait {} seconds between requests.",
                    retry_after_secs
                )
            }
            Self::DailyLimitReached {
                retry_after_secs, ..
            } => format!(
                "Daily faucet limit reached. Try again in {} seconds.",
                retry_after_secs
            ),
            Self::NoWallet => "Faucet wallet not configured".to_string(),
            Self::InsufficientBalance {
                available,
                requested,
            } => format!(
                "Faucet has insufficient balance ({} available, {} requested)",
                available, requested
            ),
            Self::InvalidAddress(msg) => format!("Invalid address: {}", msg),
            Self::TransactionFailed(msg) => format!("Transaction failed: {}", msg),
        }
    }
}

/// Request tracking for a single IP or address
#[derive(Debug)]
struct RequestTracker {
    /// Timestamps of recent requests
    requests: Vec<Instant>,
}

impl RequestTracker {
    fn new() -> Self {
        Self {
            requests: Vec::new(),
        }
    }

    /// Add a new request timestamp
    fn record(&mut self) {
        self.requests.push(Instant::now());
    }

    /// Count requests within a time window
    fn count_within(&self, window: Duration) -> u32 {
        let cutoff = Instant::now() - window;
        self.requests.iter().filter(|&&t| t > cutoff).count() as u32
    }

    /// Get the most recent request timestamp
    fn last_request(&self) -> Option<Instant> {
        self.requests.last().copied()
    }

    /// Remove requests older than the given duration
    fn cleanup(&mut self, max_age: Duration) {
        let cutoff = Instant::now() - max_age;
        self.requests.retain(|&t| t > cutoff);
    }
}

/// Faucet state with rate limiting
pub struct FaucetState {
    /// Configuration
    config: FaucetConfig,
    /// Per-IP request tracking
    ip_requests: DashMap<IpAddr, RequestTracker>,
    /// Per-address request tracking (normalized address string)
    address_requests: DashMap<String, RequestTracker>,
    /// Total amount dispensed today (in picocredits)
    daily_dispensed: AtomicU64,
    /// Unix timestamp of the start of the current day (UTC)
    day_start: AtomicU64,
    /// Mutex to prevent concurrent UTXO selection.
    /// This prevents race conditions where two faucet requests select
    /// the same UTXO and one fails with a double-spend error.
    tx_build_mutex: Mutex<()>,
}

impl FaucetState {
    /// Create a new faucet state
    pub fn new(config: FaucetConfig) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let day_start = now - (now % 86400); // Start of current UTC day

        Self {
            config,
            ip_requests: DashMap::new(),
            address_requests: DashMap::new(),
            daily_dispensed: AtomicU64::new(0),
            day_start: AtomicU64::new(day_start),
            tx_build_mutex: Mutex::new(()),
        }
    }

    /// Acquire the transaction build lock.
    ///
    /// This must be held during UTXO selection and transaction submission
    /// to prevent race conditions where concurrent requests select the same UTXO.
    pub async fn acquire_tx_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.tx_build_mutex.lock().await
    }

    /// Check if the faucet is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the amount to dispense per request
    pub fn amount(&self) -> u64 {
        self.config.amount
    }

    /// Check rate limits for a request
    ///
    /// Returns Ok(()) if the request is allowed, Err with the specific limit hit.
    pub fn check_rate_limit(&self, ip: IpAddr, address: &str) -> Result<(), FaucetError> {
        // Reset daily counter if we've crossed into a new day
        self.maybe_reset_daily_counter();

        // 1. Check cooldown (minimum time between requests from same IP)
        if let Some(tracker) = self.ip_requests.get(&ip) {
            if let Some(last) = tracker.last_request() {
                let elapsed = last.elapsed().as_secs();
                if elapsed < self.config.cooldown_secs {
                    return Err(FaucetError::CooldownActive {
                        retry_after_secs: self.config.cooldown_secs - elapsed,
                    });
                }
            }
        }

        // 2. Check per-IP hourly limit
        let ip_count = self
            .ip_requests
            .get(&ip)
            .map(|t| t.count_within(Duration::from_secs(3600)))
            .unwrap_or(0);

        if ip_count >= self.config.per_ip_hourly_limit {
            // Calculate time until oldest request expires
            let retry_after = self
                .ip_requests
                .get(&ip)
                .and_then(|t| {
                    t.requests.first().map(|oldest| {
                        let age = oldest.elapsed().as_secs();
                        3600_u64.saturating_sub(age)
                    })
                })
                .unwrap_or(3600);

            return Err(FaucetError::IpRateLimited {
                retry_after_secs: retry_after,
                requests_this_hour: ip_count,
                limit: self.config.per_ip_hourly_limit,
            });
        }

        // 3. Check per-address daily limit
        let normalized_address = normalize_address(address);
        let addr_count = self
            .address_requests
            .get(&normalized_address)
            .map(|t| t.count_within(Duration::from_secs(86400)))
            .unwrap_or(0);

        if addr_count >= self.config.per_address_daily_limit {
            // Calculate time until oldest request expires
            let retry_after = self
                .address_requests
                .get(&normalized_address)
                .and_then(|t| {
                    t.requests.first().map(|oldest| {
                        let age = oldest.elapsed().as_secs();
                        86400_u64.saturating_sub(age)
                    })
                })
                .unwrap_or(86400);

            return Err(FaucetError::AddressRateLimited {
                retry_after_secs: retry_after,
                requests_today: addr_count,
                limit: self.config.per_address_daily_limit,
            });
        }

        // 4. Check global daily limit
        let dispensed = self.daily_dispensed.load(Ordering::Relaxed);
        if dispensed + self.config.amount > self.config.daily_limit {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let day_start = self.day_start.load(Ordering::Relaxed);
            let retry_after = (day_start + 86400).saturating_sub(now);

            return Err(FaucetError::DailyLimitReached {
                dispensed_today: dispensed,
                limit: self.config.daily_limit,
                retry_after_secs: retry_after,
            });
        }

        Ok(())
    }

    /// Record a successful faucet request
    pub fn record_request(&self, ip: IpAddr, address: &str, amount: u64) {
        // Record IP request
        self.ip_requests
            .entry(ip)
            .or_insert_with(RequestTracker::new)
            .record();

        // Record address request
        let normalized_address = normalize_address(address);
        self.address_requests
            .entry(normalized_address)
            .or_insert_with(RequestTracker::new)
            .record();

        // Update daily total
        self.daily_dispensed.fetch_add(amount, Ordering::Relaxed);
    }

    /// Reset the daily counter if we've crossed into a new UTC day
    fn maybe_reset_daily_counter(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let current_day_start = now - (now % 86400);
        let stored_day_start = self.day_start.load(Ordering::Relaxed);

        if current_day_start > stored_day_start {
            // New day - reset counter
            // Use compare_exchange to avoid race conditions
            if self
                .day_start
                .compare_exchange(
                    stored_day_start,
                    current_day_start,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                self.daily_dispensed.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Clean up old request tracking data to prevent memory growth
    pub fn cleanup(&self) {
        // Remove entries older than 24 hours
        let max_age = Duration::from_secs(86400);

        self.ip_requests.retain(|_, tracker| {
            tracker.cleanup(max_age);
            !tracker.requests.is_empty()
        });

        self.address_requests.retain(|_, tracker| {
            tracker.cleanup(max_age);
            !tracker.requests.is_empty()
        });
    }

    /// Get current stats for monitoring
    pub fn stats(&self) -> FaucetStats {
        FaucetStats {
            enabled: self.config.enabled,
            amount_per_request: self.config.amount,
            daily_dispensed: self.daily_dispensed.load(Ordering::Relaxed),
            daily_limit: self.config.daily_limit,
            tracked_ips: self.ip_requests.len(),
            tracked_addresses: self.address_requests.len(),
        }
    }
}

/// Faucet statistics for monitoring
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaucetStats {
    pub enabled: bool,
    pub amount_per_request: u64,
    pub daily_dispensed: u64,
    pub daily_limit: u64,
    pub tracked_ips: usize,
    pub tracked_addresses: usize,
}

/// Normalize an address string for consistent rate limiting
fn normalize_address(address: &str) -> String {
    // Remove whitespace and convert to lowercase for consistency
    address
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// Faucet request parameters
#[derive(Debug, Deserialize)]
pub struct FaucetRequest {
    /// Destination address (view:hex\nspend:hex format)
    pub address: String,
}

/// Faucet response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaucetResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_formatted: Option<String>,
}

impl FaucetResponse {
    pub fn success(tx_hash: String, amount: u64) -> Self {
        let bth = amount as f64 / 1_000_000_000_000.0;
        Self {
            success: true,
            tx_hash: Some(tx_hash),
            amount: Some(amount.to_string()),
            amount_formatted: Some(format!("{:.6} BTH", bth)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> FaucetConfig {
        FaucetConfig {
            enabled: true,
            amount: 10_000_000_000_000, // 10 BTH
            per_ip_hourly_limit: 3,
            per_address_daily_limit: 2,
            daily_limit: 100_000_000_000_000, // 100 BTH
            cooldown_secs: 5,
        }
    }

    #[test]
    fn test_faucet_allows_first_request() {
        let state = FaucetState::new(test_config());
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        assert!(state.check_rate_limit(ip, "view:abc\nspend:def").is_ok());
    }

    #[test]
    fn test_faucet_cooldown() {
        let state = FaucetState::new(test_config());
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        let address = "view:abc\nspend:def";

        // First request OK
        assert!(state.check_rate_limit(ip, address).is_ok());
        state.record_request(ip, address, 10_000_000_000_000);

        // Immediate second request should be rate limited (cooldown)
        let result = state.check_rate_limit(ip, address);
        assert!(matches!(result, Err(FaucetError::CooldownActive { .. })));
    }

    #[test]
    fn test_faucet_per_ip_limit() {
        let mut config = test_config();
        config.cooldown_secs = 0; // Disable cooldown for this test
        config.per_ip_hourly_limit = 2;

        let state = FaucetState::new(config);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        // Two requests OK
        assert!(state.check_rate_limit(ip, "view:a\nspend:a").is_ok());
        state.record_request(ip, "view:a\nspend:a", 10_000_000_000_000);

        assert!(state.check_rate_limit(ip, "view:b\nspend:b").is_ok());
        state.record_request(ip, "view:b\nspend:b", 10_000_000_000_000);

        // Third request should be rate limited
        let result = state.check_rate_limit(ip, "view:c\nspend:c");
        assert!(matches!(result, Err(FaucetError::IpRateLimited { .. })));
    }

    #[test]
    fn test_faucet_per_address_limit() {
        let mut config = test_config();
        config.cooldown_secs = 0; // Disable cooldown for this test
        config.per_address_daily_limit = 2;

        let state = FaucetState::new(config);
        let address = "view:abc\nspend:def";

        // Two requests to same address OK (different IPs)
        let ip1: IpAddr = "192.168.1.1".parse().unwrap();
        let ip2: IpAddr = "192.168.1.2".parse().unwrap();
        let ip3: IpAddr = "192.168.1.3".parse().unwrap();

        assert!(state.check_rate_limit(ip1, address).is_ok());
        state.record_request(ip1, address, 10_000_000_000_000);

        assert!(state.check_rate_limit(ip2, address).is_ok());
        state.record_request(ip2, address, 10_000_000_000_000);

        // Third request to same address should be rate limited
        let result = state.check_rate_limit(ip3, address);
        assert!(matches!(result, Err(FaucetError::AddressRateLimited { .. })));
    }

    #[test]
    fn test_faucet_daily_limit() {
        let mut config = test_config();
        config.cooldown_secs = 0;
        config.per_ip_hourly_limit = 100;
        config.per_address_daily_limit = 100;
        config.daily_limit = 25_000_000_000_000; // 25 BTH (2.5 requests worth)

        let state = FaucetState::new(config);

        // Two requests OK (20 BTH total)
        let ip1: IpAddr = "192.168.1.1".parse().unwrap();
        let ip2: IpAddr = "192.168.1.2".parse().unwrap();
        let ip3: IpAddr = "192.168.1.3".parse().unwrap();

        assert!(state.check_rate_limit(ip1, "view:a\nspend:a").is_ok());
        state.record_request(ip1, "view:a\nspend:a", 10_000_000_000_000);

        assert!(state.check_rate_limit(ip2, "view:b\nspend:b").is_ok());
        state.record_request(ip2, "view:b\nspend:b", 10_000_000_000_000);

        // Third request would exceed daily limit
        let result = state.check_rate_limit(ip3, "view:c\nspend:c");
        assert!(matches!(result, Err(FaucetError::DailyLimitReached { .. })));
    }

    #[test]
    fn test_address_normalization() {
        let state = FaucetState::new(test_config());
        let ip1: IpAddr = "192.168.1.1".parse().unwrap();
        let ip2: IpAddr = "192.168.1.2".parse().unwrap();

        // These should be treated as the same address
        let addr1 = "view:ABC\nspend:DEF";
        let addr2 = "view:abc\nspend:def";

        assert!(state.check_rate_limit(ip1, addr1).is_ok());
        state.record_request(ip1, addr1, 10_000_000_000_000);

        assert!(state.check_rate_limit(ip2, addr2).is_ok());
        state.record_request(ip2, addr2, 10_000_000_000_000);

        // Third request should be limited (same normalized address)
        let ip3: IpAddr = "192.168.1.3".parse().unwrap();
        let result = state.check_rate_limit(ip3, addr1);
        assert!(matches!(result, Err(FaucetError::AddressRateLimited { .. })));
    }

    #[test]
    fn test_faucet_stats() {
        let state = FaucetState::new(test_config());
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        state.record_request(ip, "view:abc\nspend:def", 10_000_000_000_000);

        let stats = state.stats();
        assert!(stats.enabled);
        assert_eq!(stats.daily_dispensed, 10_000_000_000_000);
        assert_eq!(stats.tracked_ips, 1);
        assert_eq!(stats.tracked_addresses, 1);
    }
}
