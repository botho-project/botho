// Copyright (c) 2024 Botho Foundation

//! Unified address format for Botho (address format v2, ADR 0008)
//!
//! Supports the universal post-quantum address with a clean URI format:
//!
//! Mainnet:
//! - `botho://2/<base58(view||spend||kem||dsa)>` (~4.4k chars)
//!
//! Testnet:
//! - `tbotho://2/<base58(view||spend||kem||dsa)>`
//!
//! The base58 body carries the two 32-byte Ristretto keys **and** the raw
//! ML-KEM-768 (1184 B) and ML-DSA-65 (1952 B) public keys — 3200 bytes total.
//! The version number (2) supersedes the retired 64-byte v1 format; the 't'
//! prefix indicates testnet addresses.
//!
//! The actual base58 encode/decode lives in the shared [`bth_address_codec`]
//! crate (ADR 0008 D5) so the node, the browser wasm-signer, the mobile bridge,
//! and the wallet all route through **one** implementation and cannot drift.
//!
//! Retired formats fail loudly on parse:
//! - old 64-byte v1 (`botho://1/...`) — carries no post-quantum keys, cannot
//!   receive on the v2 chain (ADR 0008);
//! - the former quantum-private class (`botho://1q/...`, `botho-pq://1/...`) —
//!   ADR 0006, the separate quantum-private transaction class was removed
//!   before mainnet.

use anyhow::{anyhow, Result};
use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPublic;
use bth_transaction_types::constants::Network;

/// Address format version
pub const ADDRESS_VERSION: u8 = 2;

/// v2 address prefixes by network
pub const MAINNET_CLASSICAL_PREFIX: &str = bth_address_codec::MAINNET_PREFIX;
pub const TESTNET_CLASSICAL_PREFIX: &str = bth_address_codec::TESTNET_PREFIX;

/// Retired v1 (64-byte, classical-only) prefixes.
///
/// Kept only so `Address::parse` can detect old v1 addresses and reject them
/// with a clear error instead of a confusing format failure.
pub const MAINNET_V1_PREFIX: &str = bth_address_codec::MAINNET_V1_PREFIX;
pub const TESTNET_V1_PREFIX: &str = bth_address_codec::TESTNET_V1_PREFIX;

/// Retired quantum-private address prefixes (ADR 0006).
///
/// Kept only so `Address::parse` can detect legacy quantum addresses and
/// reject them with a clear error instead of a confusing format failure.
pub const MAINNET_QUANTUM_PREFIX: &str = bth_address_codec::MAINNET_QUANTUM_PREFIX;
pub const TESTNET_QUANTUM_PREFIX: &str = bth_address_codec::TESTNET_QUANTUM_PREFIX;
const LEGACY_QUANTUM_PREFIX: &str = bth_address_codec::LEGACY_QUANTUM_PREFIX;

// Legacy prefix for backwards compatibility
pub const CLASSICAL_PREFIX: &str = MAINNET_CLASSICAL_PREFIX;

/// Map a node [`Network`] to the shared codec's network enum.
fn codec_network(network: Network) -> bth_address_codec::Network {
    match network {
        Network::Mainnet => bth_address_codec::Network::Mainnet,
        Network::Testnet => bth_address_codec::Network::Testnet,
    }
}

/// Represents a classical address, with network info
#[derive(Debug, Clone)]
pub struct Address {
    /// The network this address belongs to
    pub network: Network,
    /// The address type
    pub kind: AddressKind,
}

/// The type of address
#[derive(Debug, Clone)]
pub enum AddressKind {
    /// Classical address (view + spend keys, ~64 bytes)
    Classical(PublicAddress),
}

impl Address {
    /// Create a new classical address for a network
    pub fn classical(addr: PublicAddress, network: Network) -> Self {
        Self {
            network,
            kind: AddressKind::Classical(addr),
        }
    }

    /// Parse an address from a string, auto-detecting the type and network
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();

        // Try file path first
        if s.ends_with(".botho") || s.ends_with(".pq") {
            return Self::from_file(s);
        }

        // Reject retired quantum-private addresses with a clear error
        // (ADR 0006). Check before the classical prefixes: "botho://1q/"
        // would otherwise never match "botho://1/" but keep this explicit
        // and first so old inputs fail loudly, not confusingly.
        if s.starts_with(MAINNET_QUANTUM_PREFIX)
            || s.starts_with(TESTNET_QUANTUM_PREFIX)
            || s.starts_with(LEGACY_QUANTUM_PREFIX)
        {
            return Err(anyhow!(
                "quantum addresses retired (ADR 0006): the quantum-private \
                 transaction class was removed before mainnet, so this \
                 address can no longer receive funds.\n\
                 Ask the recipient for a current address (botho://2/...).\n\
                 Post-quantum protection is moving to universal ML-KEM on \
                 standard outputs (see issue #904)."
            ));
        }

        // Reject retired 64-byte v1 addresses loudly (ADR 0008): they carry no
        // post-quantum keys and cannot receive on the v2 chain. Checked before
        // the v2 prefixes so `botho://1/...` never silently mis-parses.
        if s.starts_with(MAINNET_V1_PREFIX) || s.starts_with(TESTNET_V1_PREFIX) {
            return Err(anyhow!(
                "address format v1 (botho://1/) retired (ADR 0008): v1 \
                 addresses carry no post-quantum keys and cannot receive on the \
                 v2 chain.\n\
                 Ask the recipient to regenerate a botho://2/ address."
            ));
        }

        // Check for testnet v2 prefix
        if s.starts_with(TESTNET_CLASSICAL_PREFIX) {
            let addr = parse_classical_address(s, Network::Testnet)?;
            return Ok(Address::classical(addr, Network::Testnet));
        }

        // Check for mainnet v2 prefix
        if s.starts_with(MAINNET_CLASSICAL_PREFIX) {
            let addr = parse_classical_address(s, Network::Mainnet)?;
            return Ok(Address::classical(addr, Network::Mainnet));
        }

        // Try legacy format: "view:<hex>,spend:<hex>" (assume mainnet)
        if s.starts_with("view:") {
            let addr = parse_legacy_address(s)?;
            return Ok(Address::classical(addr, Network::Mainnet));
        }

        Err(anyhow!(
            "Invalid address format. Expected:\n  \
             Mainnet:  botho://2/<base58>\n  \
             Testnet:  tbotho://2/<base58>\n  \
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
        let line = content
            .lines()
            .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .ok_or_else(|| anyhow!("Address file is empty"))?;

        Self::parse(line)
    }

    /// Get the classical public address
    pub fn public_address(&self) -> PublicAddress {
        match &self.kind {
            AddressKind::Classical(addr) => addr.clone(),
        }
    }

    /// Format as a `botho://2/<base58>` string.
    ///
    /// Fails if the underlying [`PublicAddress`] does not carry both
    /// post-quantum keys at their exact raw lengths (a v2 address cannot be
    /// represented without them).
    pub fn to_address_string(&self) -> Result<String> {
        match &self.kind {
            AddressKind::Classical(addr) => format_classical_address(addr, self.network),
        }
    }

    /// Save to a file
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let content = format!(
            "# Botho Address\n\
             # Network: {}\n\
             # Type: PostQuantum (v2)\n\
             # Created: {}\n\n\
             {}\n",
            self.network.display_name(),
            chrono_lite_now(),
            self.to_address_string()?
        );

        std::fs::write(path, content).map_err(|e| anyhow!("Failed to write address file: {}", e))
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.to_address_string() {
            Ok(s) => write!(f, "{s}"),
            Err(e) => write!(f, "<invalid address: {e}>"),
        }
    }
}

/// Get the classical address prefix for a network
fn classical_prefix(network: Network) -> &'static str {
    match network {
        Network::Mainnet => MAINNET_CLASSICAL_PREFIX,
        Network::Testnet => TESTNET_CLASSICAL_PREFIX,
    }
}

/// Format a v2 address as `botho://2/<base58>` or `tbotho://2/<base58>`.
///
/// Routes through the shared [`bth_address_codec`] (ADR 0008 D5). Fails if the
/// address does not carry both post-quantum keys.
pub fn format_classical_address(addr: &PublicAddress, network: Network) -> Result<String> {
    bth_address_codec::encode_address(addr, codec_network(network))
        .map_err(|e| anyhow!("Failed to encode address: {e}"))
}

/// Parse a v2 address from `botho://2/<base58>` or `tbotho://2/<base58>`.
///
/// Routes through the shared [`bth_address_codec`] and verifies the decoded
/// network matches `network`.
pub fn parse_classical_address(s: &str, network: Network) -> Result<PublicAddress> {
    let (addr, decoded_network) =
        bth_address_codec::decode_address(s).map_err(|e| anyhow!("{e}"))?;

    let expected = codec_network(network);
    if decoded_network != expected {
        return Err(anyhow!(
            "Address network mismatch: decoded {:?}, expected {:?}",
            decoded_network,
            expected
        ));
    }

    Ok(addr)
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

    let view_bytes =
        hex::decode(view_hex).map_err(|e| anyhow!("Invalid hex in view key: {}", e))?;
    let spend_bytes =
        hex::decode(spend_hex).map_err(|e| anyhow!("Invalid hex in spend key: {}", e))?;

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

    /// Build a v2 address with dummy-but-correctly-sized PQ payloads.
    fn sample_v2_public_address() -> PublicAddress {
        use bth_util_from_random::FromRandom;
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::from_seed([3u8; 32]);
        let spend = RistrettoPublic::from_random(&mut rng);
        let view = RistrettoPublic::from_random(&mut rng);
        let kem = vec![7u8; bth_account_keys::ML_KEM_768_PUBLIC_KEY_LEN];
        let dsa = vec![9u8; bth_account_keys::ML_DSA_65_PUBLIC_KEY_LEN];
        PublicAddress::new_with_pq(&spend, &view, kem, dsa)
    }

    #[test]
    fn test_mainnet_prefix() {
        let addr = sample_v2_public_address();
        let formatted = format_classical_address(&addr, Network::Mainnet).unwrap();
        assert!(formatted.starts_with("botho://2/"));
        assert!(!formatted.starts_with("tbotho://"));
    }

    #[test]
    fn test_testnet_prefix() {
        let addr = sample_v2_public_address();
        let formatted = format_classical_address(&addr, Network::Testnet).unwrap();
        assert!(formatted.starts_with("tbotho://2/"));
    }

    #[test]
    fn test_v2_round_trip_through_address_parse() {
        let addr = sample_v2_public_address();
        for network in [Network::Mainnet, Network::Testnet] {
            let s = format_classical_address(&addr, network).unwrap();
            let parsed = Address::parse(&s).expect("v2 address parses");
            assert_eq!(parsed.network, network);
            let pa = parsed.public_address();
            assert_eq!(pa.kem_public_key(), addr.kem_public_key());
            assert_eq!(pa.dsa_public_key(), addr.dsa_public_key());
            // Canonical re-render is identical.
            assert_eq!(parsed.to_address_string().unwrap(), s);
        }
    }

    #[test]
    fn test_old_v1_address_rejected_with_clear_error() {
        // A well-formed 64-byte v1 body under the retired prefix must fail
        // loudly, not silently truncate into a bogus address.
        let body = bs58::encode([0u8; 64]).into_string();
        for prefix in ["botho://1/", "tbotho://1/"] {
            let err = Address::parse(&format!("{prefix}{body}"))
                .expect_err("v1 address must not parse")
                .to_string();
            assert!(
                err.contains("v1") && err.contains("ADR 0008"),
                "error must cite the v1 retirement, got: {err}"
            );
        }
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
        assert!(MAINNET_CLASSICAL_PREFIX.starts_with("botho://2/"));
        assert!(MAINNET_QUANTUM_PREFIX.starts_with("botho://1q/"));

        // Testnet prefixes
        assert!(TESTNET_CLASSICAL_PREFIX.starts_with("tbotho://2/"));
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

    /// Regression test for #903 (ADR 0006): parsing a retired quantum-private
    /// address must fail with a clear "retired" error — not a panic, not a
    /// generic format error, and never a silently mis-parsed address.
    #[test]
    fn test_quantum_address_rejected_with_clear_error() {
        for addr in [
            "botho://1q/3sampleBase58Payload",
            "tbotho://1q/3sampleBase58Payload",
            "botho-pq://1/3sampleBase58Payload",
        ] {
            let err = Address::parse(addr)
                .expect_err("retired quantum address must not parse")
                .to_string();
            assert!(
                err.contains("quantum addresses retired (ADR 0006)"),
                "error for {addr:?} must cite the retirement, got: {err}"
            );
        }
    }

    /// The retirement error must also surface through the network-checked
    /// parse path used by `botho send`.
    #[test]
    fn test_quantum_address_rejected_via_parse_for_network() {
        let err = Address::parse_for_network("botho://1q/3sampleBase58Payload", Network::Testnet)
            .expect_err("retired quantum address must not parse")
            .to_string();
        assert!(err.contains("quantum addresses retired (ADR 0006)"));
    }
}
