// Copyright (c) 2024 Botho Foundation

//! STUN (Session Traversal Utilities for NAT) client implementation.
//!
//! STUN is used to discover the public address and port of a node behind NAT,
//! and to determine the NAT type for relay capacity reporting.
//!
//! # NAT Types
//!
//! Understanding NAT type is important for relay capacity:
//!
//! - **Open/Full Cone**: Any external host can send to the mapped port
//! - **Restricted Cone**: Only hosts the internal host has contacted can send
//! - **Port Restricted Cone**: Must match both host and port
//! - **Symmetric**: Different mapping for each destination (hardest to traverse)
//!
//! # Protocol (RFC 5389)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |0 0|     STUN Message Type     |         Message Length        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                         Magic Cookie                          |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                                                               |
//! |                     Transaction ID (96 bits)                  |
//! |                                                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, info, trace, warn};

/// STUN magic cookie (RFC 5389)
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;

/// STUN binding request message type
const STUN_BINDING_REQUEST: u16 = 0x0001;

/// STUN binding response message type
const STUN_BINDING_RESPONSE: u16 = 0x0101;

/// STUN XOR-MAPPED-ADDRESS attribute type
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// STUN MAPPED-ADDRESS attribute type (fallback)
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Errors that can occur during STUN operations.
#[derive(Debug, Error)]
pub enum StunError {
    /// Network error
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),

    /// Request timed out
    #[error("STUN request timed out after {0:?}")]
    Timeout(Duration),

    /// Invalid response
    #[error("invalid STUN response: {0}")]
    InvalidResponse(String),

    /// No STUN servers configured
    #[error("no STUN servers configured")]
    NoServers,

    /// All STUN servers failed
    #[error("all STUN servers failed")]
    AllServersFailed,

    /// Failed to parse server address
    #[error("failed to parse STUN server: {0}")]
    InvalidServer(String),
}

/// NAT type as detected by STUN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NatType {
    /// No NAT - public IP address
    Open,
    /// Full cone NAT - easiest to traverse
    FullCone,
    /// Restricted cone NAT
    Restricted,
    /// Port restricted cone NAT
    PortRestricted,
    /// Symmetric NAT - hardest to traverse
    Symmetric,
    /// Unknown (detection failed or incomplete)
    Unknown,
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NatType::Open => write!(f, "open"),
            NatType::FullCone => write!(f, "full-cone"),
            NatType::Restricted => write!(f, "restricted-cone"),
            NatType::PortRestricted => write!(f, "port-restricted-cone"),
            NatType::Symmetric => write!(f, "symmetric"),
            NatType::Unknown => write!(f, "unknown"),
        }
    }
}

impl NatType {
    /// Returns the relay score modifier for this NAT type.
    /// Higher scores indicate better relay capability.
    pub fn relay_score_modifier(&self) -> f64 {
        match self {
            NatType::Open => 1.0,
            NatType::FullCone => 0.8,
            NatType::Restricted => 0.6,
            NatType::PortRestricted => 0.4,
            NatType::Symmetric => 0.1,
            NatType::Unknown => 0.3,
        }
    }

    /// Whether this NAT type supports accepting inbound connections.
    pub fn supports_inbound(&self) -> bool {
        matches!(self, NatType::Open | NatType::FullCone)
    }
}

/// STUN client configuration.
#[derive(Debug, Clone)]
pub struct StunConfig {
    /// List of STUN servers to use
    pub servers: Vec<String>,
    /// Request timeout
    pub request_timeout: Duration,
    /// Number of retries per server
    pub retries: u8,
    /// Delay between retries
    pub retry_delay: Duration,
}

impl Default for StunConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                "stun.l.google.com:19302".to_string(),
                "stun1.l.google.com:19302".to_string(),
                "stun.cloudflare.com:3478".to_string(),
            ],
            request_timeout: Duration::from_secs(3),
            retries: 2,
            retry_delay: Duration::from_millis(500),
        }
    }
}

impl StunConfig {
    /// Create a config with custom servers.
    pub fn with_servers(servers: Vec<String>) -> Self {
        Self {
            servers,
            ..Default::default()
        }
    }
}

/// Result of a STUN binding request.
#[derive(Debug, Clone)]
pub struct StunResult {
    /// The external (mapped) address as seen by the STUN server
    pub mapped_address: SocketAddr,
    /// The STUN server that responded
    pub server: String,
    /// Round-trip time
    pub rtt: Duration,
}

/// STUN client for discovering public address and NAT type.
pub struct StunClient {
    config: StunConfig,
}

impl StunClient {
    /// Create a new STUN client with the given configuration.
    pub fn new(config: StunConfig) -> Self {
        Self { config }
    }

    /// Create a STUN client with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(StunConfig::default())
    }

    /// Discover the public (mapped) address using STUN.
    pub async fn discover_public_address(&self) -> Result<StunResult, StunError> {
        if self.config.servers.is_empty() {
            return Err(StunError::NoServers);
        }

        // Try each server in order
        let mut last_error = None;
        for server in &self.config.servers {
            match self.query_server(server).await {
                Ok(result) => {
                    info!(
                        "STUN: discovered public address {} via {}",
                        result.mapped_address, server
                    );
                    return Ok(result);
                }
                Err(e) => {
                    warn!("STUN server {} failed: {}", server, e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(StunError::AllServersFailed))
    }

    /// Query a single STUN server.
    async fn query_server(&self, server: &str) -> Result<StunResult, StunError> {
        // Parse server address
        let server_addr = parse_stun_server(server)?;

        // Create UDP socket (bind to any port)
        let socket = UdpSocket::bind("0.0.0.0:0").await?;

        // Try with retries
        for attempt in 0..=self.config.retries {
            if attempt > 0 {
                tokio::time::sleep(self.config.retry_delay).await;
                debug!("STUN retry {} for {}", attempt, server);
            }

            let start = std::time::Instant::now();

            // Send binding request
            let transaction_id = generate_transaction_id();
            let request = build_binding_request(&transaction_id);
            socket.send_to(&request, server_addr).await?;

            // Wait for response
            let mut buf = [0u8; 512];
            match timeout(self.config.request_timeout, socket.recv_from(&mut buf)).await {
                Ok(Ok((len, _))) => {
                    let rtt = start.elapsed();

                    // Parse response
                    if let Some(mapped) = parse_binding_response(&buf[..len], &transaction_id)? {
                        return Ok(StunResult {
                            mapped_address: mapped,
                            server: server.to_string(),
                            rtt,
                        });
                    }
                }
                Ok(Err(e)) => {
                    if attempt == self.config.retries {
                        return Err(StunError::Network(e));
                    }
                }
                Err(_) => {
                    if attempt == self.config.retries {
                        return Err(StunError::Timeout(self.config.request_timeout));
                    }
                }
            }
        }

        Err(StunError::AllServersFailed)
    }

    /// Detect NAT type using multiple STUN queries.
    ///
    /// This performs the classic NAT type detection algorithm:
    /// 1. Query server to get mapped address
    /// 2. Query same server from different port to check consistency
    /// 3. Query different server to check mapping behavior
    pub async fn detect_nat_type(&self) -> Result<NatType, StunError> {
        if self.config.servers.len() < 2 {
            warn!("NAT type detection requires at least 2 STUN servers");
            return Ok(NatType::Unknown);
        }

        // First query: get initial mapped address
        let result1 = self.query_server(&self.config.servers[0]).await?;
        let mapped1 = result1.mapped_address;

        // Check if we have a public IP (no NAT)
        if let Ok(local_addrs) = get_local_addresses().await {
            if local_addrs.contains(&mapped1.ip()) {
                info!("No NAT detected - public IP: {}", mapped1.ip());
                return Ok(NatType::Open);
            }
        }

        // Second query: different server to check for symmetric NAT
        let result2 = self.query_server(&self.config.servers[1]).await?;
        let mapped2 = result2.mapped_address;

        if mapped1.ip() != mapped2.ip() || mapped1.port() != mapped2.port() {
            // Different mappings for different destinations = Symmetric NAT
            info!(
                "Symmetric NAT detected: {} vs {}",
                mapped1, mapped2
            );
            return Ok(NatType::Symmetric);
        }

        // Same mapping for different destinations - could be cone NAT
        // For full detection, we'd need servers that support CHANGE-REQUEST
        // For now, assume Port Restricted Cone (most common)
        info!(
            "Cone NAT detected (mapped address: {}), assuming port-restricted",
            mapped1
        );
        Ok(NatType::PortRestricted)
    }

    /// Get the current configuration.
    pub fn config(&self) -> &StunConfig {
        &self.config
    }
}

/// Parse STUN server URL or address.
fn parse_stun_server(server: &str) -> Result<SocketAddr, StunError> {
    // Handle stun: URL prefix
    let addr_str = server
        .strip_prefix("stun:")
        .unwrap_or(server);

    // Parse as socket address
    if let Ok(addr) = addr_str.parse() {
        return Ok(addr);
    }

    // Try with default port
    if !addr_str.contains(':') {
        if let Ok(addr) = format!("{}:3478", addr_str).parse() {
            return Ok(addr);
        }
    }

    // Try DNS resolution
    use std::net::ToSocketAddrs;
    addr_str
        .to_socket_addrs()
        .map_err(|_| StunError::InvalidServer(server.to_string()))?
        .next()
        .ok_or_else(|| StunError::InvalidServer(server.to_string()))
}

/// Generate a random 96-bit transaction ID.
fn generate_transaction_id() -> [u8; 12] {
    let mut id = [0u8; 12];
    getrandom::getrandom(&mut id).expect("failed to generate random transaction ID");
    id
}

/// Build a STUN binding request message.
fn build_binding_request(transaction_id: &[u8; 12]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(20);

    // Message Type: Binding Request (0x0001)
    msg.extend_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());

    // Message Length: 0 (no attributes)
    msg.extend_from_slice(&0u16.to_be_bytes());

    // Magic Cookie
    msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());

    // Transaction ID
    msg.extend_from_slice(transaction_id);

    msg
}

/// Parse a STUN binding response.
fn parse_binding_response(
    data: &[u8],
    expected_transaction_id: &[u8; 12],
) -> Result<Option<SocketAddr>, StunError> {
    if data.len() < 20 {
        return Err(StunError::InvalidResponse("message too short".to_string()));
    }

    // Check message type
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != STUN_BINDING_RESPONSE {
        return Err(StunError::InvalidResponse(format!(
            "unexpected message type: 0x{:04x}",
            msg_type
        )));
    }

    // Check magic cookie
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if cookie != STUN_MAGIC_COOKIE {
        return Err(StunError::InvalidResponse("invalid magic cookie".to_string()));
    }

    // Check transaction ID
    if &data[8..20] != expected_transaction_id {
        return Err(StunError::InvalidResponse("transaction ID mismatch".to_string()));
    }

    // Get message length
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if data.len() < 20 + msg_len {
        return Err(StunError::InvalidResponse("truncated message".to_string()));
    }

    // Parse attributes
    let mut pos = 20;
    while pos + 4 <= 20 + msg_len {
        let attr_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let attr_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;

        if pos + 4 + attr_len > data.len() {
            break;
        }

        let attr_data = &data[pos + 4..pos + 4 + attr_len];

        match attr_type {
            STUN_ATTR_XOR_MAPPED_ADDRESS => {
                if let Some(addr) = parse_xor_mapped_address(attr_data) {
                    return Ok(Some(addr));
                }
            }
            STUN_ATTR_MAPPED_ADDRESS => {
                if let Some(addr) = parse_mapped_address(attr_data) {
                    return Ok(Some(addr));
                }
            }
            _ => {
                trace!("Ignoring STUN attribute 0x{:04x}", attr_type);
            }
        }

        // Padding to 4-byte boundary
        pos += 4 + ((attr_len + 3) & !3);
    }

    Ok(None)
}

/// Parse XOR-MAPPED-ADDRESS attribute (RFC 5389).
fn parse_xor_mapped_address(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]) ^ (STUN_MAGIC_COOKIE >> 16) as u16;

    match family {
        0x01 => {
            // IPv4
            if data.len() < 8 {
                return None;
            }
            let ip_bytes: [u8; 4] = [
                data[4] ^ ((STUN_MAGIC_COOKIE >> 24) as u8),
                data[5] ^ ((STUN_MAGIC_COOKIE >> 16) as u8),
                data[6] ^ ((STUN_MAGIC_COOKIE >> 8) as u8),
                data[7] ^ (STUN_MAGIC_COOKIE as u8),
            ];
            let ip = IpAddr::from(ip_bytes);
            Some(SocketAddr::new(ip, port))
        }
        0x02 => {
            // IPv6
            if data.len() < 20 {
                return None;
            }
            // XOR with magic cookie and transaction ID
            // For simplicity, just parse IPv4 for now
            None
        }
        _ => None,
    }
}

/// Parse MAPPED-ADDRESS attribute (RFC 3489 fallback).
fn parse_mapped_address(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 => {
            // IPv4
            let ip = IpAddr::from([data[4], data[5], data[6], data[7]]);
            Some(SocketAddr::new(ip, port))
        }
        0x02 => {
            // IPv6
            if data.len() < 20 {
                return None;
            }
            let mut ip_bytes = [0u8; 16];
            ip_bytes.copy_from_slice(&data[4..20]);
            let ip = IpAddr::from(ip_bytes);
            Some(SocketAddr::new(ip, port))
        }
        _ => None,
    }
}

/// Get local network interface addresses.
async fn get_local_addresses() -> Result<Vec<IpAddr>, std::io::Error> {
    // On real implementation, use netlink/getifaddrs
    // For now, return empty
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_type_display() {
        assert_eq!(NatType::Open.to_string(), "open");
        assert_eq!(NatType::FullCone.to_string(), "full-cone");
        assert_eq!(NatType::Symmetric.to_string(), "symmetric");
    }

    #[test]
    fn test_nat_type_relay_score() {
        assert!(NatType::Open.relay_score_modifier() > NatType::Symmetric.relay_score_modifier());
        assert!(NatType::FullCone.relay_score_modifier() > NatType::PortRestricted.relay_score_modifier());
    }

    #[test]
    fn test_nat_type_inbound_support() {
        assert!(NatType::Open.supports_inbound());
        assert!(NatType::FullCone.supports_inbound());
        assert!(!NatType::Symmetric.supports_inbound());
        assert!(!NatType::PortRestricted.supports_inbound());
    }

    #[test]
    fn test_stun_config_default() {
        let config = StunConfig::default();
        assert_eq!(config.servers.len(), 3);
        assert_eq!(config.retries, 2);
    }

    #[test]
    fn test_parse_stun_server() {
        // With stun: prefix
        let addr = parse_stun_server("stun:192.168.1.1:3478").unwrap();
        assert_eq!(addr.port(), 3478);

        // Without prefix
        let addr = parse_stun_server("192.168.1.1:3478").unwrap();
        assert_eq!(addr.port(), 3478);
    }

    #[test]
    fn test_build_binding_request() {
        let tx_id = [1u8; 12];
        let request = build_binding_request(&tx_id);

        assert_eq!(request.len(), 20);
        assert_eq!(u16::from_be_bytes([request[0], request[1]]), STUN_BINDING_REQUEST);
        assert_eq!(u16::from_be_bytes([request[2], request[3]]), 0); // No attributes
        assert_eq!(
            u32::from_be_bytes([request[4], request[5], request[6], request[7]]),
            STUN_MAGIC_COOKIE
        );
    }

    #[test]
    fn test_generate_transaction_id() {
        let id1 = generate_transaction_id();
        let id2 = generate_transaction_id();
        assert_ne!(id1, id2); // Should be random
    }

    #[test]
    fn test_parse_xor_mapped_address() {
        // XOR-MAPPED-ADDRESS for 192.0.2.1:32853
        // After XOR with magic cookie
        let mut data = vec![0x00, 0x01]; // Reserved + Family (IPv4)

        // Port XOR'd with high 16 bits of magic cookie (0x2112)
        let port: u16 = 32853;
        let xor_port = port ^ 0x2112;
        data.extend_from_slice(&xor_port.to_be_bytes());

        // IP XOR'd with magic cookie
        let ip: [u8; 4] = [192, 0, 2, 1];
        let magic_bytes = STUN_MAGIC_COOKIE.to_be_bytes();
        for i in 0..4 {
            data.push(ip[i] ^ magic_bytes[i]);
        }

        let result = parse_xor_mapped_address(&data).unwrap();
        assert_eq!(result.port(), port);
        assert_eq!(result.ip(), IpAddr::from([192, 0, 2, 1]));
    }

    #[test]
    fn test_parse_mapped_address() {
        // MAPPED-ADDRESS for 192.0.2.1:32853
        let mut data = vec![0x00, 0x01]; // Reserved + Family (IPv4)
        data.extend_from_slice(&32853u16.to_be_bytes()); // Port
        data.extend_from_slice(&[192, 0, 2, 1]); // IP

        let result = parse_mapped_address(&data).unwrap();
        assert_eq!(result.port(), 32853);
        assert_eq!(result.ip(), IpAddr::from([192, 0, 2, 1]));
    }
}
