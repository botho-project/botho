// Copyright (c) 2024 The Botho Foundation

//! Chain-specific types and utilities.

use serde::{Deserialize, Serialize};

/// Supported blockchain networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    /// Botho (BTH) native chain
    Bth,
    /// Ethereum mainnet or testnet
    Ethereum,
    /// Solana mainnet or devnet
    Solana,
}

impl std::fmt::Display for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chain::Bth => write!(f, "bth"),
            Chain::Ethereum => write!(f, "ethereum"),
            Chain::Solana => write!(f, "solana"),
        }
    }
}

impl std::str::FromStr for Chain {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "bth" | "botho" => Ok(Chain::Bth),
            "eth" | "ethereum" => Ok(Chain::Ethereum),
            "sol" | "solana" => Ok(Chain::Solana),
            _ => Err(format!("Unknown chain: {}", s)),
        }
    }
}

/// A chain-specific address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAddress {
    pub chain: Chain,
    pub address: String,
}

impl ChainAddress {
    pub fn new(chain: Chain, address: impl Into<String>) -> Self {
        Self {
            chain,
            address: address.into(),
        }
    }

    /// Validate the address format for the chain.
    ///
    /// This is a STRUCTURAL check (prefix, base58 alphabet, payload size)
    /// meant to reject junk before it is persisted — e.g. at public
    /// order-create (#1042). It does not verify key material; the service's
    /// address codec does full validation when an address is actually used.
    pub fn validate(&self) -> Result<(), String> {
        match self.chain {
            Chain::Bth => validate_bth_address(&self.address),
            Chain::Ethereum => {
                // Ethereum addresses are 0x-prefixed 40-char hex strings
                if !self.address.starts_with("0x") {
                    return Err("Ethereum address must start with 0x".to_string());
                }
                if self.address.len() != 42 {
                    return Err(format!(
                        "Ethereum address must be 42 characters, got {}",
                        self.address.len()
                    ));
                }
                // Validate hex
                if !self.address[2..].chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("Ethereum address must be valid hex".to_string());
                }
                Ok(())
            }
            Chain::Solana => {
                // Solana addresses are base58-encoded 32-byte public keys
                if self.address.is_empty() {
                    return Err("Solana address cannot be empty".to_string());
                }
                // Typical Solana address is 32-44 characters in base58
                if self.address.len() < 32 || self.address.len() > 44 {
                    return Err(format!(
                        "Solana address must be 32-44 characters, got {}",
                        self.address.len()
                    ));
                }
                Ok(())
            }
        }
    }
}

impl std::fmt::Display for ChainAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.chain, self.address)
    }
}

/// Hard cap on the accepted BTH address string length. A v2 hybrid address
/// body (base58 of an ML-KEM/ML-DSA public-key payload, ~3.2 KB) encodes to
/// roughly 4,400 characters; 8 KiB leaves ample headroom while still bounding
/// abusive input.
const MAX_BTH_ADDRESS_LEN: usize = 8 * 1024;

/// Retired BTH address URI prefixes (ADR 0008): rejected outright so a stale
/// wallet gets a clear error instead of a never-matching order.
const RETIRED_BTH_PREFIXES: &[&str] = &[
    "botho://1/",
    "tbotho://1/",
    "botho://1q/",
    "tbotho://1q/",
    "botho-pq://",
];

/// Structural validation for a BTH address (#1042).
///
/// Accepts the two shapes the bridge's recipient decoder
/// (`decode_recipient_address` in the service) accepts:
///
///   * a v2 hybrid address URI — `botho://2/<base58>` (mainnet) or
///     `tbotho://2/<base58>` (testnet) — whose base58 body must decode to a
///     plausibly-sized key payload, and
///   * a legacy bare base58 body that decodes to exactly 64 bytes (`view32 ||
///     spend32`, the classical layout).
///
/// Everything else — empty strings, non-base58 junk, retired v1/quantum
/// prefixes, unrecognized URI schemes, oversized input — is rejected, so an
/// invalid BTH destination fails at order-create instead of surfacing later
/// as an order that can never match.
fn validate_bth_address(address: &str) -> Result<(), String> {
    if address.is_empty() {
        return Err("BTH address cannot be empty".to_string());
    }
    if address.len() > MAX_BTH_ADDRESS_LEN {
        return Err(format!(
            "BTH address too long ({} chars, max {})",
            address.len(),
            MAX_BTH_ADDRESS_LEN
        ));
    }
    if !address.is_ascii()
        || address
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err("BTH address contains illegal characters".to_string());
    }
    for retired in RETIRED_BTH_PREFIXES {
        if address.starts_with(retired) {
            return Err(format!(
                "BTH address uses the retired {} prefix (ADR 0008); \
                 ask the recipient for a botho://2/ address",
                retired
            ));
        }
    }

    let (body, is_v2_uri) = if let Some(body) = address.strip_prefix("botho://2/") {
        (body, true)
    } else if let Some(body) = address.strip_prefix("tbotho://2/") {
        (body, true)
    } else if address.contains("://") {
        return Err(
            "unrecognized BTH address prefix (expected botho://2/ or tbotho://2/)".to_string(),
        );
    } else {
        (address, false)
    };

    if body.is_empty() {
        return Err("BTH address has an empty base58 body".to_string());
    }
    let payload = bs58::decode(body)
        .into_vec()
        .map_err(|e| format!("BTH address is not valid base58: {}", e))?;

    if is_v2_uri {
        // A v2 payload carries hybrid public keys; require at least the
        // classical 64 bytes so trivially short junk cannot pass. Exact key
        // validation is the address codec's job, not this structural check.
        if payload.len() < 64 {
            return Err(format!(
                "BTH v2 address payload too short ({} bytes)",
                payload.len()
            ));
        }
    } else if payload.len() != 64 {
        // Bare bodies are the legacy classical layout: view32 || spend32.
        return Err(format!(
            "bare BTH address must decode to 64 bytes (view||spend), got {}",
            payload.len()
        ));
    }
    Ok(())
}

/// Ethereum-specific types
pub mod ethereum {
    /// ERC-20 wBTH contract interface events
    #[derive(Debug, Clone)]
    pub struct BridgeMintEvent {
        pub to: [u8; 20],
        pub amount: u64,
        pub bth_tx_hash: [u8; 32],
        pub block_number: u64,
        pub tx_hash: [u8; 32],
    }

    #[derive(Debug, Clone)]
    pub struct BridgeBurnEvent {
        pub from: [u8; 20],
        pub amount: u64,
        pub bth_address: String,
        pub block_number: u64,
        pub tx_hash: [u8; 32],
    }
}

/// Solana-specific types
pub mod solana {
    /// SPL wBTH program events
    #[derive(Debug, Clone)]
    pub struct BridgeMintEvent {
        pub user: [u8; 32],
        pub amount: u64,
        pub bth_tx_hash: [u8; 32],
        pub slot: u64,
        pub signature: String,
    }

    #[derive(Debug, Clone)]
    pub struct BridgeBurnEvent {
        pub user: [u8; 32],
        pub amount: u64,
        pub bth_address: String,
        pub slot: u64,
        pub signature: String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_parsing() {
        assert_eq!("bth".parse::<Chain>().unwrap(), Chain::Bth);
        assert_eq!("ethereum".parse::<Chain>().unwrap(), Chain::Ethereum);
        assert_eq!("eth".parse::<Chain>().unwrap(), Chain::Ethereum);
        assert_eq!("solana".parse::<Chain>().unwrap(), Chain::Solana);
        assert_eq!("sol".parse::<Chain>().unwrap(), Chain::Solana);
    }

    #[test]
    fn test_bth_address_validation_accepts_valid_shapes() {
        // Legacy bare base58 of exactly 64 bytes (view32 || spend32).
        let bare = bs58::encode([7u8; 64]).into_string();
        assert!(ChainAddress::new(Chain::Bth, bare).validate().is_ok());

        // v2 URI forms with a plausible hybrid-key payload.
        let body = bs58::encode(vec![9u8; 3200]).into_string();
        for prefix in ["botho://2/", "tbotho://2/"] {
            let addr = format!("{}{}", prefix, body);
            assert!(
                ChainAddress::new(Chain::Bth, addr).validate().is_ok(),
                "{} address must validate",
                prefix
            );
        }
    }

    #[test]
    fn test_bth_address_validation_rejects_junk() {
        // Empty.
        assert!(ChainAddress::new(Chain::Bth, "")
            .validate()
            .unwrap_err()
            .contains("empty"));

        // Underscores are not base58 (the old validator accepted this).
        assert!(ChainAddress::new(Chain::Bth, "bth_user_receive_addr")
            .validate()
            .is_err());

        // Base58 but the wrong payload size for a bare body.
        let short = bs58::encode([1u8; 10]).into_string();
        assert!(ChainAddress::new(Chain::Bth, short)
            .validate()
            .unwrap_err()
            .contains("64 bytes"));

        // Retired prefixes (ADR 0008).
        let v1 = format!("botho://1/{}", bs58::encode([2u8; 64]).into_string());
        assert!(ChainAddress::new(Chain::Bth, v1)
            .validate()
            .unwrap_err()
            .contains("retired"));

        // Unknown scheme.
        assert!(ChainAddress::new(Chain::Bth, "http://evil.example/x")
            .validate()
            .unwrap_err()
            .contains("unrecognized"));

        // v2 URI with a too-short payload.
        let stub = format!("botho://2/{}", bs58::encode([3u8; 8]).into_string());
        assert!(ChainAddress::new(Chain::Bth, stub)
            .validate()
            .unwrap_err()
            .contains("too short"));

        // Whitespace / control characters.
        assert!(ChainAddress::new(Chain::Bth, "abc def").validate().is_err());

        // Oversized input.
        let huge = "1".repeat(MAX_BTH_ADDRESS_LEN + 1);
        assert!(ChainAddress::new(Chain::Bth, huge)
            .validate()
            .unwrap_err()
            .contains("too long"));
    }

    #[test]
    fn test_eth_address_validation() {
        let valid = ChainAddress::new(
            Chain::Ethereum,
            "0x1234567890abcdef1234567890abcdef12345678",
        );
        assert!(valid.validate().is_ok());

        let no_prefix =
            ChainAddress::new(Chain::Ethereum, "1234567890abcdef1234567890abcdef12345678");
        assert!(no_prefix.validate().is_err());

        let too_short = ChainAddress::new(Chain::Ethereum, "0x1234");
        assert!(too_short.validate().is_err());
    }
}
