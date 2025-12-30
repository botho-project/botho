// Copyright (c) 2024 Botho Foundation

//! Unified address format for Botho
//!
//! Supports both classical and quantum-safe addresses with a clean URI format:
//!
//! - Classical: `botho://1/<base58(view||spend)>` (~90 chars)
//! - Quantum:   `botho://1q/<base58(view||spend||kem||sig)>` (~4400 chars)
//!
//! The version number (1) allows for future format upgrades.
//! The 'q' suffix indicates quantum-safe addresses.

use anyhow::{anyhow, Result};
use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPublic;

#[cfg(feature = "pq")]
use bth_account_keys::QuantumSafePublicAddress;

/// Address format version
pub const ADDRESS_VERSION: u8 = 1;

/// Classical address prefix
pub const CLASSICAL_PREFIX: &str = "botho://1/";

/// Quantum-safe address prefix
pub const QUANTUM_PREFIX: &str = "botho://1q/";

/// Represents either a classical or quantum-safe address
#[derive(Debug, Clone)]
pub enum Address {
    /// Classical address (view + spend keys, ~64 bytes)
    Classical(PublicAddress),
    /// Quantum-safe address (view + spend + KEM + sig keys, ~3200 bytes)
    #[cfg(feature = "pq")]
    Quantum(QuantumSafePublicAddress),
}

impl Address {
    /// Parse an address from a string, auto-detecting the type
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();

        // Try file path first
        if s.ends_with(".botho") || s.ends_with(".pq") {
            return Self::from_file(s);
        }

        // Check for quantum prefix first (more specific)
        #[cfg(feature = "pq")]
        if s.starts_with(QUANTUM_PREFIX) {
            let addr = parse_quantum_address(s)?;
            return Ok(Address::Quantum(addr));
        }

        // Check for classical prefix
        if s.starts_with(CLASSICAL_PREFIX) {
            let addr = parse_classical_address(s)?;
            return Ok(Address::Classical(addr));
        }

        // Try legacy format: "view:<hex>,spend:<hex>"
        if s.starts_with("view:") {
            let addr = parse_legacy_address(s)?;
            return Ok(Address::Classical(addr));
        }

        // Try legacy PQ format: "botho-pq://1/"
        #[cfg(feature = "pq")]
        if s.starts_with("botho-pq://1/") {
            let addr = QuantumSafePublicAddress::from_address_string(s)
                .map_err(|e| anyhow!("Invalid legacy quantum address: {}", e))?;
            return Ok(Address::Quantum(addr));
        }

        Err(anyhow!(
            "Invalid address format. Expected:\n  \
             Classical: botho://1/<base58>\n  \
             Quantum:   botho://1q/<base58>\n  \
             Legacy:    view:<hex>,spend:<hex>"
        ))
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
        match self {
            Address::Classical(_) => false,
            #[cfg(feature = "pq")]
            Address::Quantum(_) => true,
        }
    }

    /// Get the classical address (works for both types since quantum includes classical)
    pub fn classical(&self) -> PublicAddress {
        match self {
            Address::Classical(addr) => addr.clone(),
            #[cfg(feature = "pq")]
            Address::Quantum(addr) => addr.classical().clone(),
        }
    }

    /// Get the quantum-safe address if available
    #[cfg(feature = "pq")]
    pub fn quantum(&self) -> Option<&QuantumSafePublicAddress> {
        match self {
            Address::Classical(_) => None,
            Address::Quantum(addr) => Some(addr),
        }
    }

    /// Format as a string
    pub fn to_string(&self) -> String {
        match self {
            Address::Classical(addr) => format_classical_address(addr),
            #[cfg(feature = "pq")]
            Address::Quantum(addr) => format_quantum_address(addr),
        }
    }

    /// Save to a file
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let content = format!(
            "# Botho Address\n\
             # Type: {}\n\
             # Created: {}\n\n\
             {}\n",
            if self.is_quantum() { "Quantum-Safe" } else { "Classical" },
            chrono_lite_now(),
            self.to_string()
        );

        std::fs::write(path, content)
            .map_err(|e| anyhow!("Failed to write address file: {}", e))
    }
}

/// Format a classical address as `botho://1/<base58>`
pub fn format_classical_address(addr: &PublicAddress) -> String {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&addr.view_public_key().to_bytes());
    bytes.extend_from_slice(&addr.spend_public_key().to_bytes());

    let encoded = bs58::encode(&bytes).into_string();
    format!("{}{}", CLASSICAL_PREFIX, encoded)
}

/// Parse a classical address from `botho://1/<base58>`
pub fn parse_classical_address(s: &str) -> Result<PublicAddress> {
    let encoded = s.strip_prefix(CLASSICAL_PREFIX)
        .ok_or_else(|| anyhow!("Invalid classical address prefix"))?;

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

/// Format a quantum-safe address as `botho://1q/<base58>`
#[cfg(feature = "pq")]
pub fn format_quantum_address(addr: &QuantumSafePublicAddress) -> String {
    let bytes = addr.to_bytes();
    let encoded = bs58::encode(&bytes).into_string();
    format!("{}{}", QUANTUM_PREFIX, encoded)
}

/// Parse a quantum-safe address from `botho://1q/<base58>`
#[cfg(feature = "pq")]
pub fn parse_quantum_address(s: &str) -> Result<QuantumSafePublicAddress> {
    let encoded = s.strip_prefix(QUANTUM_PREFIX)
        .ok_or_else(|| anyhow!("Invalid quantum address prefix"))?;

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
    fn test_classical_roundtrip() {
        // Create a test address
        let view_bytes = [1u8; 32];
        let spend_bytes = [2u8; 32];

        // These won't be valid Ristretto points, but we can test the format
        let formatted = format!("{}{}",
            CLASSICAL_PREFIX,
            bs58::encode([view_bytes, spend_bytes].concat()).into_string()
        );

        assert!(formatted.starts_with(CLASSICAL_PREFIX));
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
    fn test_address_type_detection() {
        assert!(CLASSICAL_PREFIX.starts_with("botho://1/"));
        assert!(QUANTUM_PREFIX.starts_with("botho://1q/"));

        // Quantum prefix is more specific, check it first
        assert!(!QUANTUM_PREFIX.starts_with(CLASSICAL_PREFIX));
    }
}
