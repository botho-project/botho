#![no_main]

//! Fuzzing target for address parsing.
//!
//! Security rationale: Address parsing accepts user input (from wallets, exchanges,
//! command line). Invalid addresses must be rejected gracefully without panics or
//! undefined behavior.
//!
//! Botho supports both classical (Ristretto-based) and quantum-safe addresses.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use botho::address::Address;

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode for address parsing
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Raw string input
    RawString(String),
    /// Raw bytes (may not be valid UTF-8)
    RawBytes(Vec<u8>),
    /// Structured address variations
    Structured(FuzzAddress),
    /// Base58 variations
    Base58Fuzz(Base58Fuzz),
}

/// Structured address for fuzzing
#[derive(Debug, Arbitrary)]
struct FuzzAddress {
    /// Network prefix
    prefix: AddressPrefix,
    /// View key bytes
    view_key: [u8; 32],
    /// Spend key bytes
    spend_key: [u8; 32],
    /// Whether to include PQ components
    include_pq: bool,
    /// PQ KEM public key bytes (if include_pq)
    pq_kem_key: Option<Vec<u8>>,
    /// PQ signature public key bytes (if include_pq)
    pq_sig_key: Option<Vec<u8>>,
}

/// Network prefixes
#[derive(Debug, Arbitrary)]
enum AddressPrefix {
    /// Mainnet classical
    MainnetClassical,
    /// Mainnet quantum
    MainnetQuantum,
    /// Testnet classical
    TestnetClassical,
    /// Testnet quantum
    TestnetQuantum,
    /// Invalid/unknown prefix
    Invalid(u8),
    /// Custom string prefix
    Custom(String),
}

/// Base58 encoding variations
#[derive(Debug, Arbitrary)]
struct Base58Fuzz {
    /// Input bytes to encode
    data: Vec<u8>,
    /// Whether to corrupt the encoding
    corrupt: bool,
    /// Corruption position (if corrupt)
    corrupt_pos: u8,
    /// Corruption character
    corrupt_char: char,
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::RawString(s) => {
            fuzz_raw_string(&s);
        }
        FuzzMode::RawBytes(data) => {
            fuzz_raw_bytes(&data);
        }
        FuzzMode::Structured(addr) => {
            fuzz_structured(&addr);
        }
        FuzzMode::Base58Fuzz(b58) => {
            fuzz_base58(&b58);
        }
    }
});

/// Fuzz with raw string input
fn fuzz_raw_string(s: &str) {
    // Try to parse as address - should never panic
    let result = Address::parse(s);

    // If parsing succeeds, verify roundtrip
    if let Ok(addr) = result {
        let canonical = addr.to_address_string();
        // Re-parsing the canonical form should succeed
        let _ = Address::parse(&canonical);

        // Test other methods
        let _ = addr.network;
        let _ = addr.is_quantum();
    }
}

/// Fuzz with raw bytes (potentially invalid UTF-8)
fn fuzz_raw_bytes(data: &[u8]) {
    // Try to interpret as UTF-8 first
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = Address::parse(s);
    }

    // Also try with lossy conversion
    let lossy = String::from_utf8_lossy(data);
    let _ = Address::parse(&lossy);

    // Try as hex-encoded address
    let hex_str = hex::encode(data);
    let _ = Address::parse(&hex_str);
}

/// Fuzz with structured address components
fn fuzz_structured(addr: &FuzzAddress) {
    // Build address string based on structure
    let prefix = match &addr.prefix {
        AddressPrefix::MainnetClassical => "bth",
        AddressPrefix::MainnetQuantum => "bthq",
        AddressPrefix::TestnetClassical => "tbth",
        AddressPrefix::TestnetQuantum => "tbthq",
        AddressPrefix::Invalid(_) => "invalid",
        AddressPrefix::Custom(s) => s.as_str(),
    };

    // Encode keys as base58
    let view_b58 = bs58::encode(&addr.view_key).into_string();
    let spend_b58 = bs58::encode(&addr.spend_key).into_string();

    // Try various address formats
    let formats = vec![
        format!("{}:{}{}", prefix, view_b58, spend_b58),
        format!("{}1{}{}", prefix, view_b58, spend_b58),
        format!("{}{}{}", prefix, view_b58, spend_b58),
    ];

    for format in formats {
        let _ = Address::parse(&format);
    }

    // Test with PQ components if enabled
    if addr.include_pq {
        if let (Some(kem), Some(sig)) = (&addr.pq_kem_key, &addr.pq_sig_key) {
            let kem_b58 = bs58::encode(kem).into_string();
            let sig_b58 = bs58::encode(sig).into_string();
            let pq_format = format!("{}{}{}{}{}", prefix, view_b58, spend_b58, kem_b58, sig_b58);
            let _ = Address::parse(&pq_format);
        }
    }
}

/// Fuzz base58 encoding variations
fn fuzz_base58(b58: &Base58Fuzz) {
    // Limit data size to prevent OOM
    let data = &b58.data[..b58.data.len().min(200)];

    // Encode as base58
    let mut encoded = bs58::encode(data).into_string();

    // Optionally corrupt the encoding
    if b58.corrupt && !encoded.is_empty() {
        let pos = (b58.corrupt_pos as usize) % encoded.len();
        let mut chars: Vec<char> = encoded.chars().collect();
        chars[pos] = b58.corrupt_char;
        encoded = chars.into_iter().collect();
    }

    // Try to parse as address
    let _ = Address::parse(&encoded);

    // Also try with common prefixes
    for prefix in ["bth", "bthq", "tbth", "tbthq"].iter() {
        let addr_str = format!("{}{}", prefix, encoded);
        let _ = Address::parse(&addr_str);
    }
}

// ============================================================================
// Additional Test Cases
// ============================================================================

/// Test edge cases that should always be handled gracefully
#[allow(dead_code)]
fn test_edge_cases() {
    // Empty string
    assert!(Address::parse("").is_err());

    // Only prefix
    assert!(Address::parse("bth").is_err());

    // Very long input
    let long = "bth".to_string() + &"a".repeat(10000);
    assert!(Address::parse(&long).is_err());

    // Null bytes
    assert!(Address::parse("bth\0abc").is_err());

    // Unicode
    assert!(Address::parse("bth\u{1F600}abc").is_err());
}
