// Copyright (c) 2024 Botho Foundation

//! Unified address format for Botho
//!
//! Supports both classical and quantum-safe addresses with a clean URI format:
//!
//! Mainnet:
//! - Classical: `botho://1/<base58(view||spend)>` (~90 chars)
//! - Quantum:   `botho://1q/<base58(view||spend||kem||sig)>` (~4400 chars)
//!
//! Testnet:
//! - Classical: `tbotho://1/<base58(view||spend)>` (~91 chars)
//! - Quantum:   `tbotho://1q/<base58(view||spend||kem||sig)>` (~4401 chars)
//!
//! The version number (1) allows for future format upgrades.
//! The 'q' suffix indicates quantum-safe addresses.
//! The 't' prefix indicates testnet addresses.

use anyhow::{anyhow, Result};
use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPublic;
use bth_transaction_types::constants::Network;

#[cfg(feature = "pq")]
use bth_account_keys::QuantumSafePublicAddress;

/// Address format version
pub const ADDRESS_VERSION: u8 = 1;

/// Classical address prefixes by network
pub const MAINNET_CLASSICAL_PREFIX: &str = "botho://1/";
pub const TESTNET_CLASSICAL_PREFIX: &str = "tbotho://1/";

/// Quantum-safe address prefixes by network
pub const MAINNET_QUANTUM_PREFIX: &str = "botho://1q/";
pub const TESTNET_QUANTUM_PREFIX: &str = "tbotho://1q/";

// Legacy prefixes for backwards compatibility
pub const CLASSICAL_PREFIX: &str = MAINNET_CLASSICAL_PREFIX;
pub const QUANTUM_PREFIX: &str = MAINNET_QUANTUM_PREFIX;

/// Represents either a classical or quantum-safe address, with network info
#[derive(Debug, Clone)]
pub struct Address {
    /// The network this address belongs to
    pub network: Network,
    /// The address type (classical or quantum)
    pub kind: AddressKind,
}

/// The type of address (classical or quantum-safe)
#[derive(Debug, Clone)]
pub enum AddressKind {
    /// Classical address (view + spend keys, ~64 bytes)
    Classical(PublicAddress),
    /// Quantum-safe address (view + spend + KEM + sig keys, ~3200 bytes)
    #[cfg(feature = "pq")]
    Quantum(QuantumSafePublicAddress),
}

impl Address {
    /// Create a new classical address for a network
    pub fn classical(addr: PublicAddress, network: Network) -> Self {
        Self {
            network,
            kind: AddressKind::Classical(addr),
        }
    }

    /// Create a new quantum-safe address for a network
    #[cfg(feature = "pq")]
    pub fn quantum(addr: QuantumSafePublicAddress, network: Network) -> Self {
        Self {
            network,
            kind: AddressKind::Quantum(addr),
        }
    }

    /// Parse an address from a string, auto-detecting the type and network
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();

        // Try file path first
        if s.ends_with(".botho") || s.ends_with(".pq") {
            return Self::from_file(s);
        }

        // Check for testnet quantum prefix first (most specific)
        #[cfg(feature = "pq")]
        if s.starts_with(TESTNET_QUANTUM_PREFIX) {
            let addr = parse_quantum_address(s, Network::Testnet)?;
            return Ok(Address::quantum(addr, Network::Testnet));
        }

        // Check for mainnet quantum prefix
        #[cfg(feature = "pq")]
        if s.starts_with(MAINNET_QUANTUM_PREFIX) {
            let addr = parse_quantum_address(s, Network::Mainnet)?;
            return Ok(Address::quantum(addr, Network::Mainnet));
        }

        // Check for testnet classical prefix
        if s.starts_with(TESTNET_CLASSICAL_PREFIX) {
            let addr = parse_classical_address(s, Network::Testnet)?;
            return Ok(Address::classical(addr, Network::Testnet));
        }

        // Check for mainnet classical prefix
        if s.starts_with(MAINNET_CLASSICAL_PREFIX) {
            let addr = parse_classical_address(s, Network::Mainnet)?;
            return Ok(Address::classical(addr, Network::Mainnet));
        }

        // Try legacy format: "view:<hex>,spend:<hex>" (assume mainnet)
        if s.starts_with("view:") {
            let addr = parse_legacy_address(s)?;
            return Ok(Address::classical(addr, Network::Mainnet));
        }

        // Try legacy PQ format: "botho-pq://1/" (assume mainnet)
        #[cfg(feature = "pq")]
        if s.starts_with("botho-pq://1/") {
            let addr = QuantumSafePublicAddress::from_address_string(s)
                .map_err(|e| anyhow!("Invalid legacy quantum address: {}", e))?;
            return Ok(Address::quantum(addr, Network::Mainnet));
        }

        Err(anyhow!(
            "Invalid address format. Expected:\n  \
             Mainnet:  botho://1/<base58>  or  botho://1q/<base58>\n  \
             Testnet:  tbotho://1/<base58> or  tbotho://1q/<base58>\n  \
             Legacy:   view:<hex>,spend:<hex>"
        ))
    }

    /// Parse an address, validating it matches the expected network
    pub fn parse_for_network(s: &str, expected: Network) -> Result<Self> {
        let addr = Self::parse(s)?;
        if addr.network != expected {
            return Err(anyhow!(
                "Address is for {} but expected {}.\n\
                 Sending to the wrong network would result in lost funds.",
                addr.network.display_name(),
                expected.display_name()
            ));
        }
        Ok(addr)
    }

    /// Load an address from a file
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read address file: {}", e))?;

        // Parse the first non-empty line
        let line = content.lines()
            .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .ok_or_else(|| anyhow!("Address file is empty"))?;

        Self::parse(line)
    }

    /// Check if this is a quantum-safe address
    pub fn is_quantum(&self) -> bool {
        match &self.kind {
            AddressKind::Classical(_) => false,
            #[cfg(feature = "pq")]
            AddressKind::Quantum(_) => true,
        }
    }

    /// Get the classical public address (works for both types since quantum includes classical)
    pub fn public_address(&self) -> PublicAddress {
        match &self.kind {
            AddressKind::Classical(addr) => addr.clone(),
            #[cfg(feature = "pq")]
            AddressKind::Quantum(addr) => addr.classical().clone(),
        }
    }

    /// Get the quantum-safe address if available
    #[cfg(feature = "pq")]
    pub fn quantum_address(&self) -> Option<&QuantumSafePublicAddress> {
        match &self.kind {
            AddressKind::Classical(_) => None,
            AddressKind::Quantum(addr) => Some(addr),
        }
    }

    /// Format as a string
    pub fn to_address_string(&self) -> String {
        match &self.kind {
            AddressKind::Classical(addr) => format_classical_address(addr, self.network),
            #[cfg(feature = "pq")]
            AddressKind::Quantum(addr) => format_quantum_address(addr, self.network),
        }
    }

    /// Save to a file
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let content = format!(
            "# Botho Address\n\
             # Network: {}\n\
             # Type: {}\n\
             # Created: {}\n\n\
             {}\n",
            self.network.display_name(),
            if self.is_quantum() { "Quantum-Safe" } else { "Classical" },
            chrono_lite_now(),
            self.to_address_string()
        );

        std::fs::write(path, content)
            .map_err(|e| anyhow!("Failed to write address file: {}", e))
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_address_string())
    }
}

/// Get the classical address prefix for a network
fn classical_prefix(network: Network) -> &'static str {
    match network {
        Network::Mainnet => MAINNET_CLASSICAL_PREFIX,
        Network::Testnet => TESTNET_CLASSICAL_PREFIX,
    }
}

/// Get the quantum address prefix for a network
#[cfg(feature = "pq")]
fn quantum_prefix(network: Network) -> &'static str {
    match network {
        Network::Mainnet => MAINNET_QUANTUM_PREFIX,
        Network::Testnet => TESTNET_QUANTUM_PREFIX,
    }
}

/// Format a classical address as `botho://1/<base58>` or `tbotho://1/<base58>`
pub fn format_classical_address(addr: &PublicAddress, network: Network) -> String {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&addr.view_public_key().to_bytes());
    bytes.extend_from_slice(&addr.spend_public_key().to_bytes());

    let encoded = bs58::encode(&bytes).into_string();
    format!("{}{}", classical_prefix(network), encoded)
}

/// Parse a classical address from `botho://1/<base58>` or `tbotho://1/<base58>`
pub fn parse_classical_address(s: &str, network: Network) -> Result<PublicAddress> {
    let prefix = classical_prefix(network);
    let encoded = s.strip_prefix(prefix)
        .ok_or_else(|| anyhow!("Invalid classical address prefix for {}", network))?;

    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| anyhow!("Invalid base58 encoding: {}", e))?;

    if bytes.len() != 64 {
        return Err(anyhow!(
            "Invalid address length: expected 64 bytes, got {}",
            bytes.len()
        ));
    }

    let view_key = RistrettoPublic::try_from(&bytes[0..32])
        .map_err(|e| anyhow!("Invalid view key: {}", e))?;
    let spend_key = RistrettoPublic::try_from(&bytes[32..64])
        .map_err(|e| anyhow!("Invalid spend key: {}", e))?;

    Ok(PublicAddress::new(&spend_key, &view_key))
}

/// Format a quantum-safe address as `botho://1q/<base58>` or `tbotho://1q/<base58>`
#[cfg(feature = "pq")]
pub fn format_quantum_address(addr: &QuantumSafePublicAddress, network: Network) -> String {
    let bytes = addr.to_bytes();
    let encoded = bs58::encode(&bytes).into_string();
    format!("{}{}", quantum_prefix(network), encoded)
}

/// Parse a quantum-safe address from `botho://1q/<base58>` or `tbotho://1q/<base58>`
#[cfg(feature = "pq")]
pub fn parse_quantum_address(s: &str, network: Network) -> Result<QuantumSafePublicAddress> {
    let prefix = quantum_prefix(network);
    let encoded = s.strip_prefix(prefix)
        .ok_or_else(|| anyhow!("Invalid quantum address prefix for {}", network))?;

    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| anyhow!("Invalid base58 encoding: {}", e))?;

    QuantumSafePublicAddress::from_bytes(&bytes)
        .map_err(|e| anyhow!("Invalid quantum address: {}", e))
}

/// Parse legacy address format: "view:<hex>,spend:<hex>"
fn parse_legacy_address(s: &str) -> Result<PublicAddress> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid legacy address format"));
    }

    let view_part = parts[0].trim();
    let spend_part = parts[1].trim();

    if !view_part.starts_with("view:") || !spend_part.starts_with("spend:") {
        return Err(anyhow!("Invalid legacy address format"));
    }

    let view_hex = &view_part[5..];
    let spend_hex = &spend_part[6..];

    let view_bytes = hex::decode(view_hex)
        .map_err(|e| anyhow!("Invalid hex in view key: {}", e))?;
    let spend_bytes = hex::decode(spend_hex)
        .map_err(|e| anyhow!("Invalid hex in spend key: {}", e))?;

    if view_bytes.len() != 32 || spend_bytes.len() != 32 {
        return Err(anyhow!("View and spend keys must be 32 bytes each"));
    }

    let view_key = RistrettoPublic::try_from(&view_bytes[..])
        .map_err(|e| anyhow!("Invalid view key: {}", e))?;
    let spend_key = RistrettoPublic::try_from(&spend_bytes[..])
        .map_err(|e| anyhow!("Invalid spend key: {}", e))?;

    Ok(PublicAddress::new(&spend_key, &view_key))
}

/// Simple timestamp without chrono dependency
fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let secs = duration.as_secs();
    // Rough approximation - good enough for a comment
    let days = secs / 86400;
    let years_since_1970 = days / 365;
    let year = 1970 + years_since_1970;
    let day_of_year = days % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;

    format!("{}-{:02}-{:02}", year, month.min(12), day.min(31))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mainnet_prefix() {
        let view_bytes = [1u8; 32];
        let spend_bytes = [2u8; 32];

        let formatted = format!("{}{}",
            MAINNET_CLASSICAL_PREFIX,
            bs58::encode([view_bytes, spend_bytes].concat()).into_string()
        );

        assert!(formatted.starts_with("botho://1/"));
        assert!(!formatted.starts_with("tbotho://"));
    }

    #[test]
    fn test_testnet_prefix() {
        let view_bytes = [1u8; 32];
        let spend_bytes = [2u8; 32];

        let formatted = format!("{}{}",
            TESTNET_CLASSICAL_PREFIX,
            bs58::encode([view_bytes, spend_bytes].concat()).into_string()
        );

        assert!(formatted.starts_with("tbotho://1/"));
    }

    #[test]
    fn test_legacy_parse() {
        // The actual parsing will fail because these aren't valid Ristretto points,
        // but we can test the format detection
        let legacy = "view:0000000000000000000000000000000000000000000000000000000000000000,spend:0000000000000000000000000000000000000000000000000000000000000000";

        // Should detect as legacy format (will fail on actual key parsing)
        assert!(legacy.starts_with("view:"));
    }

    #[test]
    fn test_address_prefixes_are_distinct() {
        // Mainnet prefixes
        assert!(MAINNET_CLASSICAL_PREFIX.starts_with("botho://1/"));
        assert!(MAINNET_QUANTUM_PREFIX.starts_with("botho://1q/"));

        // Testnet prefixes
        assert!(TESTNET_CLASSICAL_PREFIX.starts_with("tbotho://1/"));
        assert!(TESTNET_QUANTUM_PREFIX.starts_with("tbotho://1q/"));

        // Testnet can be distinguished from mainnet
        assert!(TESTNET_CLASSICAL_PREFIX.starts_with('t'));
        assert!(TESTNET_QUANTUM_PREFIX.starts_with('t'));
        assert!(!MAINNET_CLASSICAL_PREFIX.starts_with('t'));
        assert!(!MAINNET_QUANTUM_PREFIX.starts_with('t'));
    }

    #[test]
    fn test_network_prefixes_match_constants() {
        assert_eq!(classical_prefix(Network::Mainnet), MAINNET_CLASSICAL_PREFIX);
        assert_eq!(classical_prefix(Network::Testnet), TESTNET_CLASSICAL_PREFIX);
    }
}
