// Copyright (c) 2024 Botho Foundation

//! Per-IP connection rate limiting to prevent Sybil attacks.
//!
//! This module provides protection against attacks where a single entity
//! attempts to create many connections from the same IP address to
//! overwhelm the network or gain disproportionate influence.

use parking_lot::RwLock;
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tracing::{debug, warn};

/// Default maximum connections allowed per IP address.
pub const DEFAULT_MAX_CONNECTIONS_PER_IP: u32 = 10;

/// Metrics for connection limiting.
#[derive(Debug, Default)]
pub struct ConnectionLimiterMetrics {
    /// Total number of connections rejected due to IP limit.
    pub rejected_connections: AtomicU64,
    /// Total number of connections accepted.
    pub accepted_connections: AtomicU64,
}

impl ConnectionLimiterMetrics {
    /// Get the number of rejected connections.
    pub fn rejected(&self) -> u64 {
        self.rejected_connections.load(Ordering::Relaxed)
    }

    /// Get the number of accepted connections.
    pub fn accepted(&self) -> u64 {
        self.accepted_connections.load(Ordering::Relaxed)
    }

    /// Increment rejected connection count.
    fn increment_rejected(&self) {
        self.rejected_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment accepted connection count.
    fn increment_accepted(&self) {
        self.accepted_connections.fetch_add(1, Ordering::Relaxed);
    }
}

/// Per-IP connection rate limiter.
///
/// Tracks the number of active connections from each IP address and
/// rejects new connections when the limit is exceeded.
#[derive(Debug)]
pub struct ConnectionLimiter {
    /// Maximum connections allowed per IP address.
    max_per_ip: u32,
    /// Whitelisted IP addresses (exempt from limits).
    whitelist: Vec<IpAddr>,
    /// Current connection count per IP.
    connections: RwLock<HashMap<IpAddr, u32>>,
    /// Metrics for monitoring.
    metrics: Arc<ConnectionLimiterMetrics>,
}

impl ConnectionLimiter {
    /// Create a new connection limiter with the specified settings.
    ///
    /// # Arguments
    ///
    /// * `max_per_ip` - Maximum connections allowed per IP (0 = unlimited)
    /// * `whitelist` - IP addresses exempt from rate limiting
    pub fn new(max_per_ip: u32, whitelist: Vec<IpAddr>) -> Self {
        Self {
            max_per_ip,
            whitelist,
            connections: RwLock::new(HashMap::new()),
            metrics: Arc::new(ConnectionLimiterMetrics::default()),
        }
    }

    /// Create a connection limiter with default settings.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_MAX_CONNECTIONS_PER_IP, Vec::new())
    }

    /// Get a reference to the metrics.
    pub fn metrics(&self) -> Arc<ConnectionLimiterMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Check if an IP address is whitelisted.
    pub fn is_whitelisted(&self, ip: &IpAddr) -> bool {
        self.whitelist.contains(ip)
    }

    /// Get the current connection count for an IP address.
    pub fn connection_count(&self, ip: &IpAddr) -> u32 {
        self.connections.read().get(ip).copied().unwrap_or(0)
    }

    /// Check if a new connection from the given IP should be allowed.
    ///
    /// Returns `true` if the connection should be allowed, `false` otherwise.
    pub fn should_allow(&self, ip: &IpAddr) -> bool {
        // Unlimited mode (max_per_ip = 0)
        if self.max_per_ip == 0 {
            return true;
        }

        // Whitelisted IPs are always allowed
        if self.is_whitelisted(ip) {
            return true;
        }

        // Check current connection count
        let count = self.connection_count(ip);
        count < self.max_per_ip
    }

    /// Try to register a new connection from the given IP.
    ///
    /// Returns `Ok(())` if the connection was registered successfully,
    /// or `Err(ConnectionLimitExceeded)` if the limit was exceeded.
    pub fn try_connect(&self, ip: IpAddr) -> Result<(), ConnectionLimitExceeded> {
        // Unlimited mode
        if self.max_per_ip == 0 {
            self.metrics.increment_accepted();
            return Ok(());
        }

        // Whitelisted IPs bypass limits
        if self.is_whitelisted(&ip) {
            let mut connections = self.connections.write();
            *connections.entry(ip).or_insert(0) += 1;
            self.metrics.increment_accepted();
            debug!(%ip, "Accepted whitelisted connection");
            return Ok(());
        }

        // Check and increment atomically
        let mut connections = self.connections.write();
        let count = connections.entry(ip).or_insert(0);

        if *count >= self.max_per_ip {
            self.metrics.increment_rejected();
            warn!(
                %ip,
                current = *count,
                max = self.max_per_ip,
                "Connection rejected: IP limit exceeded"
            );
            return Err(ConnectionLimitExceeded {
                ip,
                current: *count,
                max: self.max_per_ip,
            });
        }

        *count += 1;
        self.metrics.increment_accepted();
        debug!(%ip, connections = *count, max = self.max_per_ip, "Connection accepted");
        Ok(())
    }

    /// Register a disconnection from the given IP.
    ///
    /// Decrements the connection count for the IP address.
    pub fn disconnect(&self, ip: &IpAddr) {
        let mut connections = self.connections.write();
        if let Some(count) = connections.get_mut(ip) {
            if *count > 0 {
                *count -= 1;
                debug!(%ip, remaining = *count, "Connection closed");
            }
            if *count == 0 {
                connections.remove(ip);
            }
        }
    }

    /// Get a snapshot of all tracked IPs and their connection counts.
    pub fn connection_snapshot(&self) -> HashMap<IpAddr, u32> {
        self.connections.read().clone()
    }

    /// Get the total number of tracked connections.
    pub fn total_connections(&self) -> usize {
        self.connections.read().values().map(|&c| c as usize).sum()
    }

    /// Get the number of unique IPs with active connections.
    pub fn unique_ips(&self) -> usize {
        self.connections.read().len()
    }
}

/// Error returned when a connection is rejected due to IP limit.
#[derive(Debug, Clone)]
pub struct ConnectionLimitExceeded {
    /// The IP address that was rejected.
    pub ip: IpAddr,
    /// Current number of connections from this IP.
    pub current: u32,
    /// Maximum allowed connections per IP.
    pub max: u32,
}

impl std::fmt::Display for ConnectionLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Connection limit exceeded for {}: {} connections (max: {})",
            self.ip, self.current, self.max
        )
    }
}

impl std::error::Error for ConnectionLimitExceeded {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_default_limiter() {
        let limiter = ConnectionLimiter::with_defaults();
        assert_eq!(limiter.max_per_ip, DEFAULT_MAX_CONNECTIONS_PER_IP);
        assert!(limiter.whitelist.is_empty());
    }

    #[test]
    fn test_connection_limit_enforced() {
        let limiter = ConnectionLimiter::new(2, vec![]);
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));

        // First two connections should succeed
        assert!(limiter.try_connect(ip).is_ok());
        assert!(limiter.try_connect(ip).is_ok());
        assert_eq!(limiter.connection_count(&ip), 2);

        // Third should fail
        let result = limiter.try_connect(ip);
        assert!(result.is_err());

        // Verify error details
        let err = result.unwrap_err();
        assert_eq!(err.ip, ip);
        assert_eq!(err.current, 2);
        assert_eq!(err.max, 2);
    }

    #[test]
    fn test_disconnect_decrements_count() {
        let limiter = ConnectionLimiter::new(3, vec![]);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        limiter.try_connect(ip).unwrap();
        limiter.try_connect(ip).unwrap();
        assert_eq!(limiter.connection_count(&ip), 2);

        limiter.disconnect(&ip);
        assert_eq!(limiter.connection_count(&ip), 1);

        limiter.disconnect(&ip);
        assert_eq!(limiter.connection_count(&ip), 0);
    }

    #[test]
    fn test_whitelist_bypasses_limit() {
        let whitelisted_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let limiter = ConnectionLimiter::new(1, vec![whitelisted_ip]);

        // Whitelisted IP can exceed limit
        assert!(limiter.try_connect(whitelisted_ip).is_ok());
        assert!(limiter.try_connect(whitelisted_ip).is_ok());
        assert!(limiter.try_connect(whitelisted_ip).is_ok());
        assert_eq!(limiter.connection_count(&whitelisted_ip), 3);
    }

    #[test]
    fn test_unlimited_mode() {
        let limiter = ConnectionLimiter::new(0, vec![]); // 0 = unlimited
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));

        // Should accept many connections
        for _ in 0..100 {
            assert!(limiter.try_connect(ip).is_ok());
        }
    }

    #[test]
    fn test_multiple_ips() {
        let limiter = ConnectionLimiter::new(2, vec![]);
        let ip1 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));

        // Each IP has its own limit
        limiter.try_connect(ip1).unwrap();
        limiter.try_connect(ip1).unwrap();
        limiter.try_connect(ip2).unwrap();
        limiter.try_connect(ip2).unwrap();

        assert_eq!(limiter.connection_count(&ip1), 2);
        assert_eq!(limiter.connection_count(&ip2), 2);
        assert_eq!(limiter.total_connections(), 4);
        assert_eq!(limiter.unique_ips(), 2);

        // Both should now be at limit
        assert!(limiter.try_connect(ip1).is_err());
        assert!(limiter.try_connect(ip2).is_err());
    }

    #[test]
    fn test_ipv6_support() {
        let limiter = ConnectionLimiter::new(2, vec![]);
        let ip = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        assert!(limiter.try_connect(ip).is_ok());
        assert!(limiter.try_connect(ip).is_ok());
        assert!(limiter.try_connect(ip).is_err());
    }

    #[test]
    fn test_metrics_tracking() {
        let limiter = ConnectionLimiter::new(1, vec![]);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // One accepted
        limiter.try_connect(ip).unwrap();
        assert_eq!(limiter.metrics().accepted(), 1);
        assert_eq!(limiter.metrics().rejected(), 0);

        // One rejected
        let _ = limiter.try_connect(ip);
        assert_eq!(limiter.metrics().accepted(), 1);
        assert_eq!(limiter.metrics().rejected(), 1);
    }

    #[test]
    fn test_should_allow() {
        let whitelisted = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let limiter = ConnectionLimiter::new(1, vec![whitelisted]);
        let normal_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));

        // Before any connections
        assert!(limiter.should_allow(&normal_ip));
        assert!(limiter.should_allow(&whitelisted));

        // After hitting limit
        limiter.try_connect(normal_ip).unwrap();
        assert!(!limiter.should_allow(&normal_ip));
        assert!(limiter.should_allow(&whitelisted)); // Always allowed
    }

    #[test]
    fn test_connection_snapshot() {
        let limiter = ConnectionLimiter::new(10, vec![]);
        let ip1 = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2));

        limiter.try_connect(ip1).unwrap();
        limiter.try_connect(ip1).unwrap();
        limiter.try_connect(ip2).unwrap();

        let snapshot = limiter.connection_snapshot();
        assert_eq!(snapshot.get(&ip1), Some(&2));
        assert_eq!(snapshot.get(&ip2), Some(&1));
    }

    #[test]
    fn test_error_display() {
        let err = ConnectionLimitExceeded {
            ip: IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
            current: 5,
            max: 5,
        };

        let msg = err.to_string();
        assert!(msg.contains("1.2.3.4"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_cleanup_on_disconnect() {
        let limiter = ConnectionLimiter::new(10, vec![]);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        limiter.try_connect(ip).unwrap();
        assert_eq!(limiter.unique_ips(), 1);

        limiter.disconnect(&ip);
        assert_eq!(limiter.unique_ips(), 0); // Entry removed when count = 0
    }
}
