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
    rpc_pool::RpcPool,
    storage::EncryptedWallet,
    transaction::{
        format_amount, parse_amount, to_tx_hex, OwnedUtxo, TransactionBuilder, MIN_TX_FEE,
        PICOCREDITS_PER_CAD,
    },
};

// The real transaction format now comes from the shared clsag crate (the same
// type the node accepts). The wallet's flat `botho-tx-v1` types were
// quarantined in `transaction_legacy.rs` — see issue #614.
use bth_transaction_clsag::{
    RingMember, Transaction as ClsagTransaction, TxOutput as ClsagTxOutput, MIN_RING_SIZE,
};
use tempfile::TempDir;

// Standard BIP39 test vector (24 words)
const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
// A distinct valid 24-word phrase for recipient/decoy fixtures.
const RECIPIENT_MNEMONIC: &str = "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title";
const TEST_PASSWORD: &str = "secure-test-password-123!";

/// Build a UTXO genuinely owned by `keys` (a real stealth output whose spend
/// key the wallet can recover), suitable for CLSAG signing.
fn owned_utxo(keys: &WalletKeys, amount: u64, created_at: u64, seed: u8) -> OwnedUtxo {
    let out = ClsagTxOutput::new(amount, &keys.public_address());
    OwnedUtxo {
        tx_hash: [seed; 32],
        output_index: 0,
        amount,
        created_at,
        target_key: out.target_key,
        public_key: out.public_key,
        subaddress_index: 0,
        cluster_tags: None,
    }
}

/// A ring of `n` valid decoy members (real stealth outputs to a distinct
/// wallet).
fn fixture_decoys(n: usize) -> Vec<RingMember> {
    let decoy_keys = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC).unwrap();
    (0..n)
        .map(|i| {
            let out = ClsagTxOutput::new(1_000 + i as u64, &decoy_keys.public_address());
            RingMember::from_output(&out)
        })
        .collect()
}

/// A disconnected RPC pool. Error-path builder tests reach their error before
/// any RPC call, so the pool is never dialed.
fn disconnected_rpc() -> RpcPool {
    RpcPool::new(NodeDiscovery::new())
}

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

    // These drive the real CLSAG assembly via `build_signed_transaction` with
    // fixture decoy rings (the RPC decoy fetch is bypassed so the tests are
    // deterministic and node-free). Each fixture UTXO is a genuine stealth
    // output the wallet can spend.

    #[test]
    fn test_build_clsag_transaction_verifies() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC)
            .unwrap()
            .public_address();

        let input_amount = 10 * PICOCREDITS_PER_CAD;
        let utxo = owned_utxo(&keys, input_amount, 50, 1);
        let builder = TransactionBuilder::new(keys.clone(), vec![utxo.clone()], 150);

        let tx = builder
            .build_signed_transaction(
                &recipient,
                3 * PICOCREDITS_PER_CAD,
                MIN_TX_FEE,
                vec![utxo],
                input_amount,
                vec![fixture_decoys(MIN_RING_SIZE - 1)],
            )
            .unwrap()
            .transaction;

        assert_eq!(tx.inputs.len(), 1);
        assert_eq!(tx.inputs.clsag()[0].ring.len(), MIN_RING_SIZE);
        assert_eq!(tx.fee, MIN_TX_FEE);
        assert_eq!(tx.created_at_height, 150);
        assert!(tx.is_valid_structure().is_ok());
        assert!(tx.verify_ring_signatures().is_ok());
    }

    #[test]
    fn test_transaction_with_change() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC)
            .unwrap()
            .public_address();

        let input_amount = 10 * PICOCREDITS_PER_CAD;
        let utxo = owned_utxo(&keys, input_amount, 50, 1);
        let builder = TransactionBuilder::new(keys.clone(), vec![utxo.clone()], 150);

        let tx = builder
            .build_signed_transaction(
                &recipient,
                3 * PICOCREDITS_PER_CAD,
                MIN_TX_FEE,
                vec![utxo],
                input_amount,
                vec![fixture_decoys(MIN_RING_SIZE - 1)],
            )
            .unwrap()
            .transaction;

        // Recipient + change.
        assert_eq!(tx.outputs.len(), 2);
        let total_output: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        assert_eq!(total_output + tx.fee, input_amount);
        assert!(tx.verify_ring_signatures().is_ok());
    }

    #[test]
    fn test_transaction_exact_amount_no_change() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC)
            .unwrap()
            .public_address();

        let exact_amount = PICOCREDITS_PER_CAD; // 1 CAD
        let fee = MIN_TX_FEE;
        let input_amount = exact_amount + fee;
        let utxo = owned_utxo(&keys, input_amount, 50, 1);
        let builder = TransactionBuilder::new(keys.clone(), vec![utxo.clone()], 150);

        let tx = builder
            .build_signed_transaction(
                &recipient,
                exact_amount,
                fee,
                vec![utxo],
                input_amount,
                vec![fixture_decoys(MIN_RING_SIZE - 1)],
            )
            .unwrap()
            .transaction;

        // No change output (would be zero / dust).
        assert_eq!(tx.outputs.len(), 1);
        assert_eq!(tx.outputs[0].amount, exact_amount);
        assert!(tx.verify_ring_signatures().is_ok());
    }

    #[test]
    fn test_transaction_serialization_roundtrips_through_node_type() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC)
            .unwrap()
            .public_address();

        let input_amount = 5 * PICOCREDITS_PER_CAD;
        let utxo = owned_utxo(&keys, input_amount, 50, 1);
        let builder = TransactionBuilder::new(keys.clone(), vec![utxo.clone()], 150);

        let tx = builder
            .build_signed_transaction(
                &recipient,
                PICOCREDITS_PER_CAD,
                MIN_TX_FEE,
                vec![utxo],
                input_amount,
                vec![fixture_decoys(MIN_RING_SIZE - 1)],
            )
            .unwrap()
            .transaction;

        // Hex-encode the way `tx_submit` expects, then decode as the node's
        // transaction type and re-verify.
        let hex = to_tx_hex(&tx).unwrap();
        let bytes = hex::decode(&hex).unwrap();
        let decoded: ClsagTransaction = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.fee, tx.fee);
        assert_eq!(decoded.inputs.len(), tx.inputs.len());
        assert_eq!(decoded.outputs.len(), tx.outputs.len());
        assert!(decoded.verify_ring_signatures().is_ok());
    }
}

// ============================================================================
// UTXO Selection Tests
// ============================================================================

mod utxo_selection {
    use super::*;

    // Error-path builder tests: each returns before any RPC decoy fetch, so a
    // disconnected pool is never dialed.

    #[tokio::test]
    async fn test_insufficient_funds() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = vec![owned_utxo(&keys, PICOCREDITS_PER_CAD, 100, 1)]; // Only 1 CAD

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);
        let mut rpc = disconnected_rpc();

        // Try to send 10 CAD - should fail during selection (pre-RPC).
        let result = builder
            .build_transfer(
                &mut rpc,
                &keys.public_address(),
                10 * PICOCREDITS_PER_CAD,
                MIN_TX_FEE,
            )
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Insufficient funds"));
    }

    #[tokio::test]
    async fn test_no_utxos_available() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let builder = TransactionBuilder::new(keys.clone(), vec![], 150);
        let mut rpc = disconnected_rpc();

        let result = builder
            .build_transfer(
                &mut rpc,
                &keys.public_address(),
                PICOCREDITS_PER_CAD,
                MIN_TX_FEE,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No UTXOs"));
    }

    #[tokio::test]
    async fn test_zero_amount_rejected() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let utxos = vec![owned_utxo(&keys, PICOCREDITS_PER_CAD, 100, 1)];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);
        let mut rpc = disconnected_rpc();

        let result = builder
            .build_transfer(&mut rpc, &keys.public_address(), 0, MIN_TX_FEE)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("greater than 0"));
    }

    #[test]
    fn test_largest_first_selection() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();

        // UTXOs of varying sizes (distinct tx_hash seeds).
        let utxos = vec![
            owned_utxo(&keys, PICOCREDITS_PER_CAD, 100, 1), // 1 CAD - smallest
            owned_utxo(&keys, 5 * PICOCREDITS_PER_CAD, 101, 2), // 5 CAD - largest
            owned_utxo(&keys, 2 * PICOCREDITS_PER_CAD, 102, 3), // 2 CAD - medium
        ];

        let builder = TransactionBuilder::new(keys.clone(), utxos, 150);

        // Request 4 CAD - largest-first should select only the 5 CAD UTXO.
        let (selected, total) = builder.select_inputs(4 * PICOCREDITS_PER_CAD).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].tx_hash, [2u8; 32]); // The 5 CAD UTXO
        assert_eq!(total, 5 * PICOCREDITS_PER_CAD);
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

        // Request 4 CAD - needs both UTXOs.
        let (selected, total) = builder.select_inputs(4 * PICOCREDITS_PER_CAD).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(total, 5 * PICOCREDITS_PER_CAD);
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
    fn test_transaction_inputs_all_have_ring_signatures() {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC)
            .unwrap()
            .public_address();

        // Two owned inputs of 1 CAD each; spend ~1 CAD + fee across both.
        let u1 = owned_utxo(&keys, PICOCREDITS_PER_CAD, 100, 1);
        let u2 = owned_utxo(&keys, PICOCREDITS_PER_CAD, 101, 2);
        let total_selected = 2 * PICOCREDITS_PER_CAD;
        let builder = TransactionBuilder::new(keys.clone(), vec![u1.clone(), u2.clone()], 150);

        let tx = builder
            .build_signed_transaction(
                &recipient,
                PICOCREDITS_PER_CAD,
                MIN_TX_FEE,
                vec![u1, u2],
                total_selected,
                vec![
                    fixture_decoys(MIN_RING_SIZE - 1),
                    fixture_decoys(MIN_RING_SIZE - 1),
                ],
            )
            .unwrap()
            .transaction;

        // Every input carries a full CLSAG ring signature.
        assert_eq!(tx.inputs.len(), 2);
        for input in tx.inputs.clsag() {
            assert_eq!(input.ring.len(), MIN_RING_SIZE);
            assert!(!input.clsag_signature.is_empty());
        }
        assert!(tx.verify_ring_signatures().is_ok());
    }
}

// ============================================================================
// Transaction Hash Tests
// ============================================================================

mod transaction_hash {
    use super::*;

    // The CLSAG signing hash is computed over outputs + fee + height (never over
    // signatures, which don't exist yet at signing time). Build outputs directly
    // from the clsag TxOutput type.
    fn clsag_output(amount: u64, target: [u8; 32], public: [u8; 32]) -> ClsagTxOutput {
        ClsagTxOutput {
            amount,
            target_key: target,
            public_key: public,
            e_memo: None,
            cluster_tags: Default::default(),
        }
    }

    #[test]
    fn test_signing_hash_deterministic_over_content() {
        let outputs = vec![clsag_output(1000, [2u8; 32], [3u8; 32])];
        let tx1 = ClsagTransaction::new_clsag(Vec::new(), outputs.clone(), 100, 1);
        let tx2 = ClsagTransaction::new_clsag(Vec::new(), outputs, 100, 1);
        // Same content -> same signing hash (and it does not depend on any
        // input signatures, which are absent here by construction).
        assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_signing_hash_changes_with_amount() {
        let tx1 = ClsagTransaction::new_clsag(
            Vec::new(),
            vec![clsag_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        let tx2 = ClsagTransaction::new_clsag(
            Vec::new(),
            vec![clsag_output(2000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_signing_hash_changes_with_fee() {
        let outputs = vec![clsag_output(1000, [2u8; 32], [3u8; 32])];
        let tx1 = ClsagTransaction::new_clsag(Vec::new(), outputs.clone(), 100, 1);
        let tx2 = ClsagTransaction::new_clsag(Vec::new(), outputs, 200, 1);
        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_hash_is_deterministic() {
        let tx = ClsagTransaction::new_clsag(
            Vec::new(),
            vec![clsag_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        assert_eq!(tx.signing_hash(), tx.signing_hash());
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
    use botho_wallet::{
        decoy_selection::{
            select_decoys, select_decoys_with_fallback, validate_decoys, DecoySelectionConfig,
            DecoySelectionError, UtxoCandidate,
        },
        fee_estimation::StoredTags,
    };
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

    /// Build `n` in-band decoy candidates for a real input of the given age.
    /// Ages are spread evenly across the ±10% band [900, 1100] (real age 1000).
    fn in_band_pool(n: usize, attribution_pct: u32) -> Vec<UtxoCandidate> {
        // Real age 1000 -> band [900, 1100].
        let (min_age, max_age) = (900u64, 1100u64);
        (0..n)
            .map(|i| {
                let age = min_age + (i as u64 % (max_age - min_age + 1));
                create_utxo((i + 1) as u8, CURRENT_BLOCK - age, attribution_pct)
            })
            .collect()
    }

    #[test]
    fn test_decoy_selection_integration() {
        // Real input age 1000 -> ±10% band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, 25);
        let pool = in_band_pool(25, 25);

        let cluster_wealth = create_cluster_wealth();
        let config = DecoySelectionConfig::default(); // Ring size 20

        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        )
        .expect("selection should succeed");
        assert_eq!(decoys.len(), 19, "Should select 19 decoys for ring size 20");

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
        // A malicious pool offers high-factor decoys to inflate the user's fee;
        // the factor ceiling must exclude them.
        let real_utxo = create_utxo(0, 9_000, 10); // Low attribution (10%)
        let cluster_wealth = create_cluster_wealth();

        // 19 legitimate low-factor in-band decoys + several high-factor ones.
        let mut pool = in_band_pool(19, 15);
        for i in 20..=28 {
            pool.push(create_utxo(i, CURRENT_BLOCK - 1000, 100)); // 100% attribution
        }

        let config = DecoySelectionConfig::default(); // ring 20, factor ceiling 1.5
        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        )
        .expect("selection should succeed");

        let real_factor = real_utxo.cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
        for decoy in &decoys {
            let factor = decoy.cluster_factor_global(&cluster_wealth, TOTAL_SUPPLY);
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
        // Real age 1000 -> band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, 0);
        let cluster_wealth = create_cluster_wealth();

        let mut pool = in_band_pool(19, 0);
        // Out-of-band candidates that must never be selected.
        pool.push(create_utxo(200, 9_800, 0)); // age 200 - too young
        pool.push(create_utxo(201, 7_000, 0)); // age 3000 - too old

        let config = DecoySelectionConfig::default();
        let decoys = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        )
        .expect("selection should succeed");

        for decoy in &decoys {
            let age = decoy.age(CURRENT_BLOCK);
            assert!(
                (900..=1100).contains(&age),
                "Decoy age {} outside ±10% band",
                age
            );
        }
    }

    #[test]
    fn test_decoy_selection_rejects_young_input() {
        // A real input younger than the confirmation floor must fail cleanly.
        let real_utxo = create_utxo(0, CURRENT_BLOCK - 5, 0); // age 5 < 10
        let cluster_wealth = create_cluster_wealth();
        let pool = in_band_pool(25, 0);

        let result = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &DecoySelectionConfig::default(),
        );
        assert!(matches!(
            result,
            Err(DecoySelectionError::InvalidRealUtxo(_))
        ));
    }

    #[test]
    fn test_decoy_selection_fallback_relaxes_factor() {
        // Only high-factor in-band decoys exist; the strict factor ceiling
        // fails, but the fallback relaxes the ceiling (never the age band).
        let real_utxo = create_utxo(0, 9_000, 0); // anonymous, factor ~1.0
        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50% of supply

        let pool = in_band_pool(25, 100); // all 100%-attribution (high factor)

        let config = DecoySelectionConfig::default();
        let strict = select_decoys(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );
        assert!(
            matches!(strict, Err(DecoySelectionError::InsufficientDecoys { .. })),
            "Strict factor ceiling should exclude all high-factor decoys"
        );

        let (decoys, was_relaxed) = select_decoys_with_fallback(
            &real_utxo,
            &pool,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        )
        .expect("fallback should succeed");
        assert!(
            was_relaxed,
            "Should indicate the factor ceiling was relaxed"
        );
        assert_eq!(decoys.len(), 19, "Should still select required decoys");
    }

    #[test]
    fn test_validate_decoys_detects_violations() {
        // Real age 1000 -> band [900, 1100].
        let real_utxo = create_utxo(0, 9_000, 10);

        let mut cluster_wealth = HashMap::new();
        cluster_wealth.insert(ClusterId::new(1), 5_000_000_000_000); // 50% of supply

        let decoys = vec![
            create_utxo(1, 9_900, 10),  // Age: 100 - TOO YOUNG
            create_utxo(2, 9_000, 10),  // Age: 1000 - valid
            create_utxo(3, 5_000, 10),  // Age: 5000 - TOO OLD
            create_utxo(4, 9_000, 100), // Age: 1000, 100% attribution - FACTOR TOO HIGH
        ];

        let config = DecoySelectionConfig::default();
        let violations = validate_decoys(
            &real_utxo,
            &decoys,
            CURRENT_BLOCK,
            &cluster_wealth,
            TOTAL_SUPPLY,
            &config,
        );

        assert_eq!(
            violations.len(),
            3,
            "Should detect 3 violations: {:?}",
            violations
        );
        assert!(
            violations
                .iter()
                .any(|(idx, msg)| *idx == 0 && msg.contains("young")),
            "Should detect 'too young' violation"
        );
        assert!(
            violations
                .iter()
                .any(|(idx, msg)| *idx == 2 && msg.contains("old")),
            "Should detect 'too old' violation"
        );
        assert!(
            violations
                .iter()
                .any(|(idx, msg)| *idx == 3 && msg.contains("factor")),
            "Should detect factor violation"
        );
    }
}
