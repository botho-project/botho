// Copyright (c) 2024 Botho Foundation

//! DNS-based seed node discovery for bootstrap.
//!
//! This module queries DNS TXT records to discover bootstrap peers dynamically.
//! Seeds can be updated without releasing new client versions.
//!
//! ## DNS Record Format
//!
//! TXT records are expected in the format:
//! ```text
//! PEER_ID@ADDRESS:PORT
//! ```
//!
//! Examples:
//! - `12D3KooWBrjT...@98.95.2.200:7100` (IP address)
//! - `12D3KooWBrjT...@eu.seed.botho.io:7100` (hostname)
//!
//! ## Caching
//!
//! Results are cached based on DNS TTL to reduce lookup frequency.
//! If DNS fails, falls back to hardcoded seeds.

use bth_transaction_types::constants::Network;
use hickory_resolver::Resolver;
use parking_lot::RwLock;
use std::{
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, info, warn};

/// Default DNS seed domain for mainnet
const MAINNET_DNS_SEED: &str = "seeds.botho.io";

/// Default DNS seed domain for testnet
const TESTNET_DNS_SEED: &str = "seeds.testnet.botho.io";

/// Minimum cache TTL (prevents hammering DNS)
const MIN_CACHE_TTL: Duration = Duration::from_secs(60);

/// Maximum cache TTL (ensures eventual refresh)
const MAX_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default TTL when DNS doesn't provide one
const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Cached seed entries with expiration
#[derive(Debug, Clone)]
struct CachedSeeds {
    /// Bootstrap peer multiaddrs
    peers: Vec<String>,
    /// When this cache entry expires
    expires_at: Instant,
}

/// DNS seed discovery service
pub struct DnsSeedDiscovery {
    /// Cached seeds per network
    cache: Arc<RwLock<Option<CachedSeeds>>>,
    /// Network type
    network: Network,
    /// Custom DNS seed domain (overrides default)
    custom_domain: Option<String>,
}

impl DnsSeedDiscovery {
    /// Create a new DNS seed discovery service
    pub fn new(network: Network) -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
            network,
            custom_domain: None,
        }
    }

    /// Create with a custom DNS seed domain
    pub fn with_domain(network: Network, domain: String) -> Self {
        let mut discovery = Self::new(network);
        discovery.custom_domain = Some(domain);
        discovery
    }

    /// Get the DNS seed domain for the current network
    fn seed_domain(&self) -> &str {
        self.custom_domain.as_deref().unwrap_or(match self.network {
            Network::Mainnet => MAINNET_DNS_SEED,
            Network::Testnet => TESTNET_DNS_SEED,
        })
    }

    /// Discover seeds via DNS, with caching and fallback
    ///
    /// Returns bootstrap peer addresses in multiaddr format.
    /// Falls back to hardcoded seeds if DNS fails.
    pub async fn discover_seeds(&self) -> Vec<String> {
        // Check cache first
        {
            let cache = self.cache.read();
            if let Some(cached) = cache.as_ref() {
                if Instant::now() < cached.expires_at {
                    debug!(
                        "Using {} cached DNS seeds (expires in {:?})",
                        cached.peers.len(),
                        cached.expires_at.saturating_duration_since(Instant::now())
                    );
                    return cached.peers.clone();
                }
            }
        }

        // Cache miss or expired - query DNS
        match self.query_dns_seeds().await {
            Ok((peers, ttl)) => {
                if peers.is_empty() {
                    warn!("DNS seed query returned no records, using hardcoded seeds");
                    return self.hardcoded_seeds();
                }

                info!("Discovered {} seeds via DNS (TTL: {:?})", peers.len(), ttl);

                // Update cache
                let mut cache = self.cache.write();
                *cache = Some(CachedSeeds {
                    peers: peers.clone(),
                    expires_at: Instant::now() + ttl,
                });

                peers
            }
            Err(e) => {
                warn!("DNS seed discovery failed: {}, using hardcoded seeds", e);
                self.hardcoded_seeds()
            }
        }
    }

    /// Query DNS TXT records for seeds
    async fn query_dns_seeds(&self) -> Result<(Vec<String>, Duration), DnsSeedError> {
        use hickory_resolver::{config::ResolverConfig, name_server::TokioConnectionProvider};

        let domain = self.seed_domain();
        debug!("Querying DNS TXT records for {}", domain);

        // Create resolver using default configuration with tokio provider
        let resolver = Resolver::builder_with_config(
            ResolverConfig::default(),
            TokioConnectionProvider::default(),
        )
        .build();

        let response = resolver
            .txt_lookup(domain)
            .await
            .map_err(|e| DnsSeedError::DnsQuery(format!("DNS lookup failed: {}", e)))?;

        let mut peers = Vec::new();

        // Calculate TTL from valid_until
        let valid_until = response.valid_until();
        let ttl = if valid_until > Instant::now() {
            valid_until.duration_since(Instant::now())
        } else {
            DEFAULT_TTL
        }
        .max(MIN_CACHE_TTL)
        .min(MAX_CACHE_TTL);

        // Parse TXT records
        for txt in response.iter() {
            for txt_data in txt.txt_data() {
                let txt_str = String::from_utf8_lossy(txt_data);
                match self.parse_seed_record(&txt_str) {
                    Ok(multiaddr) => {
                        debug!("Parsed DNS seed: {}", multiaddr);
                        peers.push(multiaddr);
                    }
                    Err(e) => {
                        warn!("Failed to parse DNS seed record '{}': {}", txt_str, e);
                    }
                }
            }
        }

        Ok((peers, ttl))
    }

    /// Parse a seed record in format `PEER_ID@ADDRESS:PORT`
    fn parse_seed_record(&self, record: &str) -> Result<String, DnsSeedError> {
        let record = record.trim();

        // Split on @ to get peer_id and address:port
        let parts: Vec<&str> = record.splitn(2, '@').collect();
        if parts.len() != 2 {
            return Err(DnsSeedError::InvalidFormat(
                "Expected format: PEER_ID@ADDRESS:PORT".to_string(),
            ));
        }

        let peer_id = parts[0].trim();
        let addr_port = parts[1].trim();

        // Validate peer ID (should be base58 encoded)
        if !peer_id.starts_with("12D3Koo") && !peer_id.starts_with("Qm") {
            return Err(DnsSeedError::InvalidFormat(
                "Invalid peer ID format".to_string(),
            ));
        }

        // Split address:port
        let (address, port) = if let Some(idx) = addr_port.rfind(':') {
            let (addr, port_str) = addr_port.split_at(idx);
            let port: u16 = port_str[1..]
                .parse()
                .map_err(|_| DnsSeedError::InvalidFormat("Invalid port number".to_string()))?;
            (addr, port)
        } else {
            return Err(DnsSeedError::InvalidFormat(
                "Expected ADDRESS:PORT format".to_string(),
            ));
        };

        // Build multiaddr based on whether address is IP or hostname
        let multiaddr = if let Ok(ip) = address.parse::<IpAddr>() {
            match ip {
                IpAddr::V4(_) => format!("/ip4/{}/tcp/{}/p2p/{}", address, port, peer_id),
                IpAddr::V6(_) => format!("/ip6/{}/tcp/{}/p2p/{}", address, port, peer_id),
            }
        } else {
            // Assume hostname - use dns4
            format!("/dns4/{}/tcp/{}/p2p/{}", address, port, peer_id)
        };

        Ok(multiaddr)
    }

    /// Get hardcoded fallback seeds
    fn hardcoded_seeds(&self) -> Vec<String> {
        match self.network {
            Network::Mainnet => vec![
                "/dns4/seed.botho.io/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ".to_string(),
            ],
            Network::Testnet => vec![
                "/dns4/seed.botho.io/tcp/17100".to_string(),
            ],
        }
    }

    /// Invalidate the cache to force a fresh DNS lookup
    pub fn invalidate_cache(&self) {
        let mut cache = self.cache.write();
        *cache = None;
    }

    /// Check if the cache is valid (not expired)
    pub fn is_cache_valid(&self) -> bool {
        let cache = self.cache.read();
        cache
            .as_ref()
            .map(|c| Instant::now() < c.expires_at)
            .unwrap_or(false)
    }
}

/// Errors that can occur during DNS seed discovery
#[derive(Debug, thiserror::Error)]
pub enum DnsSeedError {
    #[error("DNS query failed: {0}")]
    DnsQuery(String),
    #[error("Invalid record format: {0}")]
    InvalidFormat(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_seed_record_ipv4() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ@98.95.2.200:7100";
        let result = discovery.parse_seed_record(record).unwrap();

        assert_eq!(
            result,
            "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ"
        );
    }

    #[test]
    fn test_parse_seed_record_ipv6() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ@::1:7100";
        let result = discovery.parse_seed_record(record).unwrap();

        assert_eq!(
            result,
            "/ip6/::1/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ"
        );
    }

    #[test]
    fn test_parse_seed_record_hostname() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ@eu.seed.botho.io:7100";
        let result = discovery.parse_seed_record(record).unwrap();

        assert_eq!(
            result,
            "/dns4/eu.seed.botho.io/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ"
        );
    }

    #[test]
    fn test_parse_seed_record_with_whitespace() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "  12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ @ 98.95.2.200:7100  ";
        let result = discovery.parse_seed_record(record).unwrap();

        assert!(result.contains("/ip4/98.95.2.200/tcp/7100/p2p/"));
    }

    #[test]
    fn test_parse_seed_record_invalid_no_at() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ:98.95.2.200:7100";
        let result = discovery.parse_seed_record(record);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_seed_record_invalid_no_port() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ@98.95.2.200";
        let result = discovery.parse_seed_record(record);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_seed_record_invalid_port() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        let record = "12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ@98.95.2.200:abc";
        let result = discovery.parse_seed_record(record);

        assert!(result.is_err());
    }

    #[test]
    fn test_hardcoded_seeds_mainnet() {
        let discovery = DnsSeedDiscovery::new(Network::Mainnet);
        let seeds = discovery.hardcoded_seeds();

        assert!(!seeds.is_empty());
        assert!(seeds[0].contains("seed.botho.io"));
        assert!(seeds[0].contains("/tcp/7100/"));
    }

    #[test]
    fn test_hardcoded_seeds_testnet() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);
        let seeds = discovery.hardcoded_seeds();

        assert!(!seeds.is_empty());
        assert!(seeds[0].contains("seed.botho.io"));
        assert!(seeds[0].contains("/tcp/17100"));
    }

    #[test]
    fn test_seed_domain_default() {
        let mainnet = DnsSeedDiscovery::new(Network::Mainnet);
        assert_eq!(mainnet.seed_domain(), MAINNET_DNS_SEED);

        let testnet = DnsSeedDiscovery::new(Network::Testnet);
        assert_eq!(testnet.seed_domain(), TESTNET_DNS_SEED);
    }

    #[test]
    fn test_seed_domain_custom() {
        let discovery =
            DnsSeedDiscovery::with_domain(Network::Mainnet, "custom.seeds.example.com".to_string());
        assert_eq!(discovery.seed_domain(), "custom.seeds.example.com");
    }

    #[test]
    fn test_cache_initially_invalid() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);
        assert!(!discovery.is_cache_valid());
    }

    #[test]
    fn test_invalidate_cache() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        // Manually set cache
        {
            let mut cache = discovery.cache.write();
            *cache = Some(CachedSeeds {
                peers: vec!["test".to_string()],
                expires_at: Instant::now() + Duration::from_secs(3600),
            });
        }

        assert!(discovery.is_cache_valid());

        discovery.invalidate_cache();

        assert!(!discovery.is_cache_valid());
    }

    #[test]
    fn test_cache_expiration() {
        let discovery = DnsSeedDiscovery::new(Network::Testnet);

        // Set expired cache
        {
            let mut cache = discovery.cache.write();
            *cache = Some(CachedSeeds {
                peers: vec!["test".to_string()],
                expires_at: Instant::now() - Duration::from_secs(1),
            });
        }

        assert!(!discovery.is_cache_valid());
    }
}
