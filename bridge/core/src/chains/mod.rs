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
    pub fn validate(&self) -> Result<(), String> {
        match self.chain {
            Chain::Bth => {
                // BTH addresses are base58-encoded public addresses
                if self.address.is_empty() {
                    return Err("BTH address cannot be empty".to_string());
                }
                // Basic validation - actual validation would check base58 and structure
                Ok(())
            }
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
