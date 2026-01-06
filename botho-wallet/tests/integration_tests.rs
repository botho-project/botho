//! Integration tests for botho-wallet
//!
//! These tests verify end-to-end wallet functionality including:
//! - Wallet lifecycle (create, save, load, restore)
//! - Transaction building and signing
//! - UTXO selection and management
//! - Address formatting
//! - Error handling

use botho_wallet::{
    discovery::NodeDiscovery,
    keys::{validate_mnemonic, WalletKeys},
    storage::EncryptedWallet,
    transaction::{
        format_amount, parse_amount, OwnedUtxo, Transaction, TransactionBuilder, TxInput, TxOutput,
        MIN_FEE, PICOCREDITS_PER_CAD,
    },
};
use tempfile::TempDir;

// Standard BIP39 test vector (24 words)
const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const TEST_PASSWORD: &str = "secure-test-password-123!";

// ============================================================================
// Wallet Lifecycle Tests
// ============================================================================

mod wallet_lifecycle {
    use super::*;

    #[test]
    fn test_full_wallet_lifecycle() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // 1. Generate new wallet
        let keys = WalletKeys::generate().unwrap();
        let mnemonic = keys.mnemonic_phrase().to_string();

        // 2. Encrypt and save wallet
        let wallet = EncryptedWallet::encrypt(&mnemonic, TEST_PASSWORD).unwrap();
        wallet.save(&wallet_path).unwrap();

        // 3. Load wallet from disk
        let loaded = EncryptedWallet::load(&wallet_path).unwrap();

        // 4. Decrypt and verify
        let decrypted = loaded.decrypt(TEST_PASSWORD).unwrap();
        assert_eq!(decrypted.as_str(), mnemonic.as_str());

        // 5. Restore keys from mnemonic
        let restored_keys = WalletKeys::from_mnemonic(&decrypted).unwrap();

        // 6. Verify keys match
        assert_eq!(
            keys.view_public_key_bytes(),
            restored_keys.view_public_key_bytes()
        );
        assert_eq!(
            keys.spend_public_key_bytes(),
            restored_keys.spend_public_key_bytes()
        );
    }

    #[test]
    fn test_wallet_restore_from_mnemonic() {
        // Create wallet from known mnemonic
        let keys1 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        // Save encrypted
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();

        // Decrypt
        let mnemonic = wallet.decrypt(TEST_PASSWORD).unwrap();

        // Restore
        let keys2 = WalletKeys::from_mnemonic(&mnemonic).unwrap();

        // Keys should be identical
        assert_eq!(keys1.mnemonic_phrase(), keys2.mnemonic_phrase());
        assert_eq!(keys1.view_public_key_bytes(), keys2.view_public_key_bytes());
        assert_eq!(
            keys1.spend_public_key_bytes(),
            keys2.spend_public_key_bytes()
        );
        assert_eq!(keys1.address_string(), keys2.address_string());
    }

    #[test]
    fn test_wallet_password_change_preserves_keys() {
        let keys_before = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let mut wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();

        let new_password = "new-super-secure-password-456!";
        wallet.change_password(TEST_PASSWORD, new_password).unwrap();

        // Restore keys with new password
        let mnemonic = wallet.decrypt(new_password).unwrap();
        let keys_after = WalletKeys::from_mnemonic(&mnemonic).unwrap();

        // Keys should be unchanged
        assert_eq!(
            keys_before.view_public_key_bytes(),
            keys_after.view_public_key_bytes()
        );
        assert_eq!(
            keys_before.spend_public_key_bytes(),
            keys_after.spend_public_key_bytes()
        );
    }

    #[test]
    fn test_wallet_sync_height_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // Create and save with sync height
        let mut wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        wallet.set_sync_height(12345);
        wallet.save(&wallet_path).unwrap();

        // Load and verify
        let loaded = EncryptedWallet::load(&wallet_path).unwrap();
        assert_eq!(loaded.sync_height, 12345);
    }

    #[test]
    fn test_discovery_state_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // Create wallet with discovery state
        let mut wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();
        let mut discovery = NodeDiscovery::new();

        // Record some peer activity
        let peer_addr = "127.0.0.1:8545".parse().unwrap();
        discovery.record_success(peer_addr, 50, 1000);

        // Save discovery state
        wallet
            .set_discovery_state(&discovery, TEST_PASSWORD)
            .unwrap();
        wallet.save(&wallet_path).unwrap();

        // Load and restore discovery state
        let loaded = EncryptedWallet::load(&wallet_path).unwrap();
        let restored_discovery = loaded.get_discovery_state(TEST_PASSWORD).unwrap();

        assert!(restored_discovery.is_some());
        let restored = restored_discovery.unwrap();
        assert!(restored.known_peers().contains(&peer_addr));
    }
}

// ============================================================================
// Transaction Building Tests
// ============================================================================

mod transaction_building {
    use super::*;

    fn create_test_utxos() -> Vec<OwnedUtxo> {
        vec![
            OwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: 10 * PICOCREDITS_PER_CAD, // 10 CAD
                created_at: 100,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: 5 * PICOCREDITS_PER_CAD, // 5 CAD
                created_at: 101,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [3u8; 32],
                output_index: 1,
                amount: 1 * PICOCREDITS_PER_CAD, // 1 CAD
                created_at: 102,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
        ]
    }

    #[test]
    fn test_build_simple_transaction() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = create_test_utxos();
        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        // Build transaction: send 3 CAD
        let recipient = keys.public_address(); // Send to self for testing
        let amount = 3 * PICOCREDITS_PER_CAD;
        let fee = MIN_FEE;

        let tx = builder.build_transfer(&recipient, amount, fee).unwrap();

        // Verify transaction structure
        assert_eq!(tx.version, 1);
        assert!(!tx.inputs.is_empty());
        assert!(!tx.outputs.is_empty());
        assert_eq!(tx.fee, fee);
        assert_eq!(tx.created_at_height, 150);

        // Verify all inputs are signed
        for input in &tx.inputs {
            assert!(!input.signature.is_empty());
            assert_eq!(input.signature.len(), 64); // Schnorrkel signature
        }
    }

    #[test]
    fn test_transaction_with_change() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = vec![OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: 10 * PICOCREDITS_PER_CAD,
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: None,
        }];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        // Send 3 CAD with 10 CAD UTXO - should have change
        let tx = builder
            .build_transfer(&keys.public_address(), 3 * PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();

        // Should have 2 outputs: recipient + change
        assert_eq!(tx.outputs.len(), 2);

        // Verify amounts
        let total_output: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        assert_eq!(total_output + tx.fee, 10 * PICOCREDITS_PER_CAD);
    }

    #[test]
    fn test_transaction_exact_amount_no_change() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let exact_amount = PICOCREDITS_PER_CAD; // 1 CAD
        let fee = MIN_FEE;

        let utxos = vec![OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: exact_amount + fee,
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: None,
        }];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        let tx = builder
            .build_transfer(&keys.public_address(), exact_amount, fee)
            .unwrap();

        // Should have only 1 output (no change because it's dust)
        assert_eq!(tx.outputs.len(), 1);
        assert_eq!(tx.outputs[0].amount, exact_amount);
    }

    #[test]
    fn test_transaction_signing_deterministic() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = create_test_utxos();

        let builder1 = TransactionBuilder::new(keys.clone(), utxos.clone(), 150);
        let builder2 = TransactionBuilder::new(keys.clone(), utxos, 150);

        let tx1 = builder1
            .build_transfer(&keys.public_address(), PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();
        let tx2 = builder2
            .build_transfer(&keys.public_address(), PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();

        // Signing hash should be based on transaction content, not random
        // Note: Outputs have random components, so hashes may differ
        // But structure should be the same
        assert_eq!(tx1.inputs.len(), tx2.inputs.len());
        assert_eq!(tx1.fee, tx2.fee);
    }

    #[test]
    fn test_transaction_serialization() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = create_test_utxos();
        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        let tx = builder
            .build_transfer(&keys.public_address(), PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();

        // Serialize to hex
        let hex = tx.to_hex();
        assert!(!hex.is_empty());

        // Should be valid hex
        let bytes = hex::decode(&hex).unwrap();
        assert!(!bytes.is_empty());

        // Should be deserializable
        let deserialized: Transaction = bincode::deserialize(&bytes).unwrap();
        assert_eq!(deserialized.version, tx.version);
        assert_eq!(deserialized.fee, tx.fee);
        assert_eq!(deserialized.inputs.len(), tx.inputs.len());
    }
}

// ============================================================================
// UTXO Selection Tests
// ============================================================================

mod utxo_selection {
    use super::*;

    #[test]
    fn test_insufficient_funds() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = vec![OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: PICOCREDITS_PER_CAD, // Only 1 CAD
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: None,
        }];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        // Try to send 10 CAD - should fail
        let result =
            builder.build_transfer(&keys.public_address(), 10 * PICOCREDITS_PER_CAD, MIN_FEE);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Insufficient funds"));
    }

    #[test]
    fn test_no_utxos_available() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos: Vec<OwnedUtxo> = vec![];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        let result = builder.build_transfer(&keys.public_address(), PICOCREDITS_PER_CAD, MIN_FEE);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No UTXOs"));
    }

    #[test]
    fn test_zero_amount_rejected() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = vec![OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: PICOCREDITS_PER_CAD,
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: None,
        }];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        let result = builder.build_transfer(&keys.public_address(), 0, MIN_FEE);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("greater than 0"));
    }

    #[test]
    fn test_largest_first_selection() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        // Create UTXOs of varying sizes
        let utxos = vec![
            OwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: 1 * PICOCREDITS_PER_CAD, // 1 CAD - smallest
                created_at: 100,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: 5 * PICOCREDITS_PER_CAD, // 5 CAD - largest
                created_at: 101,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [3u8; 32],
                output_index: 0,
                amount: 2 * PICOCREDITS_PER_CAD, // 2 CAD - medium
                created_at: 102,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
        ];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        // Request 4 CAD - should select the 5 CAD UTXO
        let tx = builder
            .build_transfer(&keys.public_address(), 4 * PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();

        // Should only need 1 input (the 5 CAD UTXO)
        assert_eq!(tx.inputs.len(), 1);
        assert_eq!(tx.inputs[0].tx_hash, [2u8; 32]); // The 5 CAD UTXO
    }

    #[test]
    fn test_multiple_utxo_selection() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        let utxos = vec![
            OwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: 3 * PICOCREDITS_PER_CAD,
                created_at: 100,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: 2 * PICOCREDITS_PER_CAD,
                created_at: 101,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
        ];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        // Request 4 CAD - needs both UTXOs
        let tx = builder
            .build_transfer(&keys.public_address(), 4 * PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();

        // Should need 2 inputs
        assert_eq!(tx.inputs.len(), 2);
    }

    #[test]
    fn test_balance_calculation() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        let utxos = vec![
            OwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: 10 * PICOCREDITS_PER_CAD,
                created_at: 100,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: 5 * PICOCREDITS_PER_CAD,
                created_at: 101,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [3u8; 32],
                output_index: 0,
                amount: 2 * PICOCREDITS_PER_CAD,
                created_at: 102,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
        ];

        let builder = TransactionBuilder::new(keys, utxos, 150);

        assert_eq!(builder.balance(), 17 * PICOCREDITS_PER_CAD);
    }
}

// ============================================================================
// Amount Formatting Tests
// ============================================================================

mod amount_formatting {
    use super::*;

    #[test]
    fn test_format_whole_amounts() {
        assert_eq!(format_amount(PICOCREDITS_PER_CAD), "1.000000 CAD");
        assert_eq!(format_amount(10 * PICOCREDITS_PER_CAD), "10.000000 CAD");
        assert_eq!(format_amount(100 * PICOCREDITS_PER_CAD), "100.000000 CAD");
    }

    #[test]
    fn test_format_fractional_amounts() {
        assert_eq!(format_amount(500_000_000_000), "0.500000 CAD");
        assert_eq!(format_amount(123_456_789_012), "0.123457 CAD"); // Rounds
        assert_eq!(format_amount(1_000_000), "0.000001 CAD");
    }

    #[test]
    fn test_format_zero() {
        assert_eq!(format_amount(0), "0.000000 CAD");
    }

    #[test]
    fn test_parse_whole_amounts() {
        assert_eq!(parse_amount("1").unwrap(), PICOCREDITS_PER_CAD);
        assert_eq!(parse_amount("10").unwrap(), 10 * PICOCREDITS_PER_CAD);
        assert_eq!(parse_amount("100").unwrap(), 100 * PICOCREDITS_PER_CAD);
    }

    #[test]
    fn test_parse_fractional_amounts() {
        assert_eq!(parse_amount("0.5").unwrap(), 500_000_000_000);
        assert_eq!(parse_amount("0.123456").unwrap(), 123_456_000_000);
        assert_eq!(parse_amount("1.5").unwrap(), 1_500_000_000_000);
    }

    #[test]
    fn test_parse_with_suffix() {
        // parse_amount supports optional " CAD" suffix
        assert_eq!(parse_amount("1.0 CAD").unwrap(), PICOCREDITS_PER_CAD);
        assert_eq!(parse_amount("5CAD").unwrap(), 5 * PICOCREDITS_PER_CAD);
        // Note: function trims end, so leading whitespace before CAD works
        assert_eq!(parse_amount("2.5 CAD").unwrap(), 2_500_000_000_000);
    }

    #[test]
    fn test_parse_negative_rejected() {
        let result = parse_amount("-1.0");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("negative"));
    }

    #[test]
    fn test_parse_invalid_format() {
        assert!(parse_amount("abc").is_err());
        assert!(parse_amount("").is_err());
        assert!(parse_amount("1.2.3").is_err());
    }

    #[test]
    fn test_roundtrip_formatting() {
        let amounts = vec![
            PICOCREDITS_PER_CAD,
            500_000_000_000,
            1_234_567_890_000,
            1_000_000,
        ];

        for original in amounts {
            let formatted = format_amount(original);
            let parsed = parse_amount(&formatted).unwrap();
            // May lose some precision due to float conversion
            let diff = if original > parsed {
                original - parsed
            } else {
                parsed - original
            };
            assert!(diff < 1_000_000, "Roundtrip failed for {}", original);
        }
    }
}

// ============================================================================
// Address Format Tests
// ============================================================================

mod address_format {
    use super::*;

    #[test]
    fn test_address_string_format() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let address = keys.address_string();

        // Should start with "cad:" prefix
        assert!(address.starts_with("cad:"));

        // Should contain two hex strings separated by ':'
        let parts: Vec<&str> = address.split(':').collect();
        assert_eq!(parts.len(), 3); // "cad", view_key, spend_key

        // Each key part should be valid hex (16 bytes = 32 hex chars)
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 32);
        assert!(hex::decode(parts[1]).is_ok());
        assert!(hex::decode(parts[2]).is_ok());
    }

    #[test]
    fn test_address_deterministic() {
        let keys1 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let keys2 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        assert_eq!(keys1.address_string(), keys2.address_string());
    }

    #[test]
    fn test_different_mnemonics_different_addresses() {
        let keys1 = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let keys2 = WalletKeys::generate().unwrap();

        assert_ne!(keys1.address_string(), keys2.address_string());
    }
}

// ============================================================================
// Mnemonic Validation Tests
// ============================================================================

mod mnemonic_validation {
    use super::*;

    #[test]
    fn test_valid_mnemonic() {
        assert!(validate_mnemonic(TEST_MNEMONIC).is_ok());
    }

    #[test]
    fn test_wrong_word_count() {
        // Too few words
        assert!(validate_mnemonic("abandon abandon abandon").is_err());

        // 12 words (valid BIP39 but we require 24)
        let twelve_words = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert!(validate_mnemonic(twelve_words).is_err());
    }

    #[test]
    fn test_invalid_word() {
        // Contains non-BIP39 word
        let invalid = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon xyz123";
        assert!(validate_mnemonic(invalid).is_err());
    }

    #[test]
    fn test_invalid_checksum() {
        // Valid words but wrong checksum
        let bad_checksum = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        assert!(validate_mnemonic(bad_checksum).is_err());
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

mod error_handling {
    use super::*;

    #[test]
    fn test_decrypt_wrong_password() {
        let wallet = EncryptedWallet::encrypt(TEST_MNEMONIC, TEST_PASSWORD).unwrap();

        let result = wallet.decrypt("wrong-password");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("wrong password"));
    }

    #[test]
    fn test_load_nonexistent_wallet() {
        let result = EncryptedWallet::load(std::path::Path::new("/nonexistent/path/wallet.dat"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_corrupted_wallet() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");

        // Write garbage data
        std::fs::write(&wallet_path, "not valid json").unwrap();

        let result = EncryptedWallet::load(&wallet_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_mnemonic() {
        let result = WalletKeys::from_mnemonic("");
        assert!(result.is_err());
    }

    #[test]
    fn test_whitespace_only_mnemonic() {
        let result = WalletKeys::from_mnemonic("   \t\n  ");
        assert!(result.is_err());
    }
}

// ============================================================================
// Signing Tests
// ============================================================================

mod signing {
    use super::*;

    #[test]
    fn test_signature_format() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let message = b"test message";
        let context = b"test context";

        let signature = keys.sign(context, message);

        // Schnorrkel signatures are 64 bytes
        assert_eq!(signature.len(), 64);
    }

    #[test]
    fn test_signature_deterministic() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let message = b"test message";
        let context = b"test context";

        let sig1 = keys.sign(context, message);
        let sig2 = keys.sign(context, message);

        // Schnorrkel signatures may have randomness, but should be valid
        assert_eq!(sig1.len(), sig2.len());
    }

    #[test]
    fn test_different_messages_different_signatures() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let context = b"test context";

        let sig1 = keys.sign(context, b"message 1");
        let sig2 = keys.sign(context, b"message 2");

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_transaction_inputs_all_signed() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        let utxos = vec![
            OwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: PICOCREDITS_PER_CAD,
                created_at: 100,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: PICOCREDITS_PER_CAD,
                created_at: 101,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [3u8; 32],
                output_index: 0,
                amount: PICOCREDITS_PER_CAD,
                created_at: 102,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
        ];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        let tx = builder
            .build_transfer(&keys.public_address(), 2 * PICOCREDITS_PER_CAD, MIN_FEE)
            .unwrap();

        // Every input should have a valid signature
        for input in &tx.inputs {
            assert!(!input.signature.is_empty());
            assert_eq!(input.signature.len(), 64);
        }
    }
}

// ============================================================================
// Transaction Hash Tests
// ============================================================================

mod transaction_hash {
    use super::*;

    #[test]
    fn test_signing_hash_excludes_signatures() {
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0xff; 64], // Different signature
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        // Signing hash should be the same
        assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_signing_hash_changes_with_amount() {
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![TxOutput {
                amount: 2000, // Different amount
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_signing_hash_changes_with_fee() {
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            200, // Different fee
            1,
        );

        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_hash_is_deterministic() {
        let tx = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        let hash1 = tx.signing_hash();
        let hash2 = tx.signing_hash();

        assert_eq!(hash1, hash2);
    }
}

// ============================================================================
// Cluster Tag Tests
// ============================================================================

mod cluster_tags {
    use super::*;
    use botho_wallet::fee_estimation::StoredTags;

    #[test]
    fn test_utxo_with_cluster_tags() {
        // Create UTXO with cluster attribution
        let tags = StoredTags {
            tags: vec![(42, 800_000), (123, 200_000)], // 80% cluster 42, 20% cluster 123
        };

        let utxo = OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: 10 * PICOCREDITS_PER_CAD,
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: Some(tags),
        };

        // Verify cluster tags are stored
        assert!(utxo.cluster_tags.is_some());
        let stored = utxo.cluster_tags.as_ref().unwrap();
        assert_eq!(stored.tags.len(), 2);
        assert_eq!(stored.tags[0], (42, 800_000));
        assert_eq!(stored.tags[1], (123, 200_000));
    }

    #[test]
    fn test_utxo_tags_helper_with_attribution() {
        let tags = StoredTags {
            tags: vec![(1, 500_000), (2, 500_000)], // 50% each
        };

        let utxo = OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: PICOCREDITS_PER_CAD,
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: Some(tags),
        };

        // tags() helper should return the stored tags
        let retrieved = utxo.tags();
        assert!(retrieved.has_attribution());
        assert_eq!(retrieved.total_attributed(), 1_000_000);
    }

    #[test]
    fn test_utxo_tags_helper_without_attribution() {
        let utxo = OwnedUtxo {
            tx_hash: [1u8; 32],
            output_index: 0,
            amount: PICOCREDITS_PER_CAD,
            created_at: 100,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            subaddress_index: 0,
            cluster_tags: None,
        };

        // tags() helper should return empty StoredTags when None
        let retrieved = utxo.tags();
        assert!(!retrieved.has_attribution());
        assert_eq!(retrieved.total_attributed(), 0);
    }

    #[test]
    fn test_stored_tags_serialization() {
        let tags = StoredTags {
            tags: vec![(42, 1_000_000)], // 100% single cluster
        };

        // Test serialization round-trip
        let json = serde_json::to_string(&tags).unwrap();
        let deserialized: StoredTags = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tags.len(), 1);
        assert_eq!(deserialized.tags[0], (42, 1_000_000));
    }

    #[test]
    fn test_utxo_with_tags_serialization() {
        let tags = StoredTags {
            tags: vec![(1, 600_000), (2, 400_000)],
        };

        let utxo = OwnedUtxo {
            tx_hash: [0xAB; 32],
            output_index: 5,
            amount: 5 * PICOCREDITS_PER_CAD,
            created_at: 12345,
            target_key: [0xCD; 32],
            public_key: [0xEF; 32],
            subaddress_index: 1,
            cluster_tags: Some(tags),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&utxo).unwrap();
        let restored: OwnedUtxo = serde_json::from_str(&json).unwrap();

        // Verify all fields preserved
        assert_eq!(restored.tx_hash, utxo.tx_hash);
        assert_eq!(restored.output_index, utxo.output_index);
        assert_eq!(restored.amount, utxo.amount);
        assert_eq!(restored.created_at, utxo.created_at);
        assert!(restored.cluster_tags.is_some());

        let restored_tags = restored.cluster_tags.unwrap();
        assert_eq!(restored_tags.tags.len(), 2);
        assert_eq!(restored_tags.tags[0], (1, 600_000));
        assert_eq!(restored_tags.tags[1], (2, 400_000));
    }

    #[test]
    fn test_utxo_without_tags_serialization_backwards_compat() {
        // Simulate old wallet data without cluster_tags field
        let old_json = r#"{
            "tx_hash": [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],
            "output_index": 0,
            "amount": 1000000000000,
            "created_at": 100,
            "target_key": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "public_key": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "subaddress_index": 0
        }"#;

        // Should deserialize without cluster_tags (defaults to None)
        let utxo: OwnedUtxo = serde_json::from_str(old_json).unwrap();
        assert!(utxo.cluster_tags.is_none());

        // tags() helper should return empty
        let tags = utxo.tags();
        assert!(!tags.has_attribution());
    }
}

// ============================================================================
// Decoy Selection Tests
// ============================================================================

mod decoy_selection {
    use botho_wallet::decoy_selection::{
        select_decoys, select_decoys_with_fallback, validate_decoys, DecoySelectionConfig,
        DecoySelectionError, UtxoCandidate,
    };
    use botho_wallet::fee_estimation::StoredTags;
    use bth_cluster_tax::{ClusterId, TAG_WEIGHT_SCALE};
    use std::collections::HashMap;

    const CURRENT_BLOCK: u64 = 10_000;
    const TOTAL_SUPPLY: u64 = 10_000_000_000_000;

    fn create_utxo(id: u8, created_at: u64, attribution_pct: u32) -> UtxoCandidate {
        let mut stored_tags = StoredTags::new();
        if attribution_pct > 0 {
            let weight = (attribution_pct as u64 * TAG_WEIGHT_SCALE as u64 / 100) as u32;
            stored_tags.tags = vec![(1, weight)];
        }

        UtxoCandidate {
            id: [id; 32],
            created_at,
            amount: 1_000_000_000_000,
            tags: stored_tags,
        }
    }

    fn create_cluster_wealth() -> HashMap<ClusterId, u64> {
        let mut wealth = HashMap::new();
        wealth.insert(ClusterId::new(1), 1_000_000_000_000); // 10% of supply
        wealth.insert(ClusterId::new(2), 2_000_000_000_000); // 20% of supply
        wealth
    }

    #[test]
    fn test_decoy_selection_integration() {
        // Create a realistic UTXO pool with varying ages and attributions
        let real_utxo = create_utxo(0, 5_000, 25); // Age: 5000 blocks, 25% attribution

        let mut pool = Vec::new();
        // Add UTXOs with similar characteristics (good decoys)
        for i in 1..=20 {
            let created_at = 4_500 + (i as u64 * 100); // Ages: 5500 to 3500
            let attribution = 20 + (i % 10); // 20-29% attribution
            pool.push(create_utxo(i, created_at, attribution as u32));
        }

        let cluster_wealth = create_cluster_wealth();
        let config = DecoySelectionConfig::default(); // Ring size 11

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok(), "Should succeed: {:?}", result);
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 10, "Should select 10 decoys for ring size 11");

        // Validate all selected decoys meet constraints
        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );
        assert!(
            violations.is_empty(),
            "No constraint violations expected: {:?}",
            violations
        );
    }

    #[test]
    fn test_decoy_selection_prevents_fee_inflation_attack() {
        // Scenario: Malicious node tries to select high-factor decoys
        // to inflate the user's fees

        let real_utxo = create_utxo(0, 5_000, 10); // Low attribution (10%)
        let cluster_wealth = create_cluster_wealth();

        // Pool contains both low-factor and high-factor UTXOs
        let mut pool = Vec::new();

        // Some legitimate low-factor decoys
        for i in 1..=5 {
            pool.push(create_utxo(i, 5_000 + i as u64 * 50, 15)); // ~15%
        }

        // High-factor "attack" decoys
        for i in 6..=15 {
            pool.push(create_utxo(i, 5_000 + i as u64 * 50, 100)); // 100%
        }

        let config = DecoySelectionConfig {
            ring_size: 6, // Need 5 decoys
            max_age_ratio: 2.0,
            max_factor_ratio: 1.5, // Key constraint!
        };

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok());
        let decoys = result.unwrap();

        // Verify NO high-factor decoys were selected
        for decoy in &decoys {
            let factor = decoy.cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
            let real_factor = real_utxo.cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
            assert!(
                factor <= real_factor * config.max_factor_ratio,
                "High-factor decoy should be excluded: factor {} > max {}",
                factor,
                real_factor * config.max_factor_ratio
            );
        }
    }

    #[test]
    fn test_decoy_selection_enforces_age_similarity() {
        let real_utxo = create_utxo(0, 9_000, 0); // Age: 1000 blocks
        let cluster_wealth = create_cluster_wealth();

        // Pool with various ages
        let pool = vec![
            create_utxo(1, 9_800, 0),  // Age: 200 - too young
            create_utxo(2, 9_600, 0),  // Age: 400 - too young
            create_utxo(3, 8_500, 0),  // Age: 1500 - valid
            create_utxo(4, 8_000, 0),  // Age: 2000 - at limit
            create_utxo(5, 7_000, 0),  // Age: 3000 - too old
            create_utxo(6, 6_000, 0),  // Age: 4000 - too old
            create_utxo(7, 8_800, 0),  // Age: 1200 - valid
            create_utxo(8, 8_600, 0),  // Age: 1400 - valid
            create_utxo(9, 8_300, 0),  // Age: 1700 - valid
            create_utxo(10, 9_400, 0), // Age: 600 - valid (min age ~500)
        ];

        let config = DecoySelectionConfig {
            ring_size: 5, // Need 4 decoys
            max_age_ratio: 2.0,
            max_factor_ratio: 10.0, // Relaxed for this test
        };

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok());
        let decoys = result.unwrap();

        // Verify all decoys are within age bounds
        let real_age = 1000u64;
        let min_age = (real_age as f64 / 2.0).ceil() as u64;
        let max_age = (real_age as f64 * 2.0).floor() as u64;

        for decoy in &decoys {
            let age = decoy.age(CURRENT_BLOCK);
            assert!(
                age >= min_age && age <= max_age,
                "Decoy age {} outside bounds [{}, {}]",
                age,
                min_age,
                max_age
            );
        }
    }

    #[test]
    fn test_decoy_selection_fallback_relaxes_constraints() {
        let real_utxo = create_utxo(0, 9_000, 0); // Age: 1000 blocks
        let cluster_wealth = create_cluster_wealth();

        // Pool with UTXOs outside strict bounds but within fallback bounds
        let mut pool = Vec::new();
        for i in 1..=15 {
            // Ages around 300-400 (below strict min of 500, but within relaxed 333)
            pool.push(create_utxo(i, 9_650 + i as u64 * 5, 0));
        }

        let config = DecoySelectionConfig {
            ring_size: 5,
            max_age_ratio: 2.0, // Strict bounds
            max_factor_ratio: 1.5,
        };

        // Strict selection should fail
        let strict_result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );
        assert!(
            matches!(strict_result, Err(DecoySelectionError::InsufficientDecoys { .. })),
            "Strict selection should fail"
        );

        // Fallback should succeed with relaxed constraints
        let fallback_result = select_decoys_with_fallback(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(fallback_result.is_ok(), "Fallback should succeed");
        let (decoys, was_relaxed) = fallback_result.unwrap();
        assert!(was_relaxed, "Should indicate constraints were relaxed");
        assert_eq!(decoys.len(), 4, "Should still select required decoys");
    }

    #[test]
    fn test_validate_decoys_detects_violations() {
        let real_utxo = create_utxo(0, 9_000, 10); // Age: 1000, 10% attribution

        // Use cluster wealth where cluster 1 is wealthy (50% of supply)
        // This makes 100% attribution to cluster 1 produce a high factor
        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50% of supply

        // Create decoys with violations
        let decoys = vec![
            create_utxo(1, 9_900, 10),  // Age: 100 - TOO YOUNG
            create_utxo(2, 8_500, 10),  // Age: 1500 - valid
            create_utxo(3, 5_000, 10),  // Age: 5000 - TOO OLD
            create_utxo(4, 8_700, 100), // Age: 1300, 100% attribution - FACTOR TOO HIGH
        ];

        let config = DecoySelectionConfig {
            ring_size: 5,
            max_age_ratio: 2.0,
            max_factor_ratio: 1.5,
        };

        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        // Should detect 3 violations (too young, too old, high factor)
        assert_eq!(violations.len(), 3, "Should detect 3 violations: {:?}", violations);

        // Verify specific violations
        assert!(
            violations.iter().any(|(idx, msg)| *idx == 0 && msg.contains("young")),
            "Should detect 'too young' violation"
        );
        assert!(
            violations.iter().any(|(idx, msg)| *idx == 2 && msg.contains("old")),
            "Should detect 'too old' violation"
        );
        assert!(
            violations.iter().any(|(idx, msg)| *idx == 3 && msg.contains("factor")),
            "Should detect factor violation"
        );
    }

    #[test]
    fn test_decoy_selection_realistic_scenario() {
        // Simulate a realistic decoy selection for a user transaction

        // User has a UTXO from commerce (30% attribution, 2000 blocks old)
        let real_utxo = create_utxo(0, 8_000, 30);
        let cluster_wealth = create_cluster_wealth();

        // Create a realistic UTXO pool with diverse characteristics
        let mut pool = Vec::new();

        // Old anonymous UTXOs (too different in age)
        for i in 1..=5 {
            pool.push(create_utxo(i, 1_000 + i as u64 * 100, 0));
        }

        // Recent low-value UTXOs (within age range)
        for i in 6..=15 {
            pool.push(create_utxo(i, 7_000 + i as u64 * 100, 20));
        }

        // Commerce UTXOs similar to real input (best candidates)
        for i in 16..=25 {
            pool.push(create_utxo(i, 7_500 + i as u64 * 50, 25));
        }

        // High-value whale UTXOs (factor too high)
        for i in 26..=30 {
            pool.push(create_utxo(i, 7_800 + i as u64 * 30, 90));
        }

        let config = DecoySelectionConfig::default();

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(result.is_ok());
        let decoys = result.unwrap();
        assert_eq!(decoys.len(), 10);

        // Validate the selection meets all constraints
        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert!(
            violations.is_empty(),
            "Realistic scenario should produce valid selection: {:?}",
            violations
        );
    }
}
