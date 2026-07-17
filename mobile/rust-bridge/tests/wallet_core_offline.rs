//! Offline, deterministic tests for the mobile bridge's on-device wallet core:
//! key derivation, ownership scan, key-image derivation, and CLSAG
//! build+sign — exercised end-to-end **without a live node**.
//!
//! The existing `live_testnet_e2e.rs` proves the full faucet -> balance -> send
//! flow against the public testnet, but it is gated behind `BOTHO_LIVE_TESTNET`
//! and needs a producing chain, so it never runs in CI. These tests close that
//! gap: they mint owned outputs locally (the same `TxOutput::new`
//! stealth-output primitive the node uses) and drive the exact node-identical
//! signer core the bridge relies on (`bth_wasm_signer::core`), so
//! derive/scan/spend regressions are caught deterministically on every CI run.
//!
//! Why this is the right boundary: the bridge's `wallet_ops` module
//! orchestrates these core primitives over a `NodeRpc`; the cryptographic
//! correctness lives entirely in derive + scan + key-image + build/sign. By
//! feeding synthetic chain outputs (instead of an HTTP node) we test that
//! correctness in isolation. The transport layer is covered by the live e2e.

use bth_account_keys::PublicAddress;
use bth_transaction_clsag::{TxOutput, DEFAULT_RING_SIZE, DUST_THRESHOLD, MIN_TX_FEE};
use bth_wasm_signer::core::{
    build_and_sign_inner, compute_owned_output_key_images_inner, scan_owned_outputs_inner,
    ChainOutput, DecoyOutput, KeyImageRequest, OwnedOutput, RecipientAddress, ScanRequest,
    SignRequest, SpendInput,
};

/// Hex spend/view private keys for a wallet, derived from a mnemonic. These are
/// exactly what the bridge derives on demand in `MobileWallet::signer_keys` and
/// passes to the signer core; they never cross the FFI boundary in the real
/// app.
struct DerivedKeys {
    spend_private_hex: String,
    view_private_hex: String,
    public_address: PublicAddress,
    /// Hex raw ML-KEM-768 public key derived from the same mnemonic's BIP39
    /// seed (node-identical). Under 6.0.0 every send output is a hybrid
    /// post-quantum output, so the recipient's key is required to encapsulate
    /// the ciphertext and the sender's own key seeds its change (issue #978).
    kem_public_hex: String,
    /// Hex-encoded 64-byte BIP39 seed. The RECEIVE scan needs this to derive
    /// the wallet's ML-KEM SECRET (node-identically, via the
    /// feature-independent `derive_pq_keys_from_seed`) and decapsulate
    /// hybrid outputs — proving the mobile crate (which builds
    /// `botho-wallet` with `pq` OFF) can detect hybrid receives without
    /// `WalletKeys::public_address()` (issue #988/#984).
    seed_hex: String,
}

/// The 64-byte BIP39 seed for a mnemonic (empty passphrase), matching the seed
/// the node/browser derive PQ keys from.
fn mnemonic_to_seed(mnemonic: &str) -> [u8; bth_crypto_pq::BIP39_SEED_SIZE] {
    bip39::Mnemonic::parse_normalized(mnemonic)
        .expect("valid mnemonic")
        .to_seed("")
}

/// Build a deterministic, checksum-valid 24-word mnemonic from a fixed 32-byte
/// entropy seed (avoiding the all-zero entropy that yields the rejected
/// `abandon abandon abandon` test-vector prefix).
fn mnemonic_from_seed(seed: u8) -> String {
    let entropy = [seed.wrapping_add(1); 32];
    bip39::Mnemonic::from_entropy(&entropy)
        .expect("32-byte entropy yields a valid 24-word mnemonic")
        .to_string()
}

/// Re-derive the same keys the bridge derives: `WalletKeys::from_mnemonic` ->
/// account key -> hex private spend/view + the default-subaddress public
/// address that received outputs target (mirrors `WalletKeys::public_address`).
fn derive_from_seed(seed: u8) -> DerivedKeys {
    let mnemonic = mnemonic_from_seed(seed);
    let keys = botho_wallet::keys::WalletKeys::from_mnemonic(&mnemonic)
        .expect("mnemonic should derive wallet keys");
    let account = keys.account_key();

    // Derive the account-wide post-quantum keys from the same BIP39 seed
    // (node-identical `derive_pq_keys_from_seed`), so the test's addresses are
    // v2 (post-quantum) and the hybrid send path attaches ML-KEM ciphertexts —
    // exactly what the bridge does. Derived from the seed here because the
    // mobile crate builds `botho-wallet` without its `pq` feature.
    let seed = mnemonic_to_seed(&mnemonic);
    let pq = bth_crypto_pq::derive_pq_keys_from_seed(&seed);
    let kem_bytes = pq.kem_keypair.public_key().as_bytes().to_vec();
    let dsa_bytes = pq.sig_keypair.public_key().as_bytes().to_vec();

    DerivedKeys {
        spend_private_hex: hex::encode(account.spend_private_key().to_bytes()),
        view_private_hex: hex::encode(account.view_private_key().to_bytes()),
        kem_public_hex: hex::encode(&kem_bytes),
        seed_hex: hex::encode(seed),
        // Outputs are sent to the default subaddress; `belongs_to` returns
        // subaddress index 0 for these. The address carries the derived PQ keys
        // so it is a valid v2 (post-quantum) recipient.
        public_address: account
            .default_subaddress()
            .with_pq_keys(kem_bytes, dsa_bytes),
    }
}

/// Mint a stealth output of `amount` picocredits owned by `recipient`, returned
/// as a node-shaped `ChainOutput` (transparent amount, hex target/public keys)
/// — i.e. what `chain_getOutputs` would report for an output the wallet owns.
fn mint_owned_output(amount: u64, recipient: &PublicAddress) -> ChainOutput {
    let out = TxOutput::new(amount, recipient);
    ChainOutput {
        target_key: hex::encode(out.target_key),
        public_key: hex::encode(out.public_key),
        amount,
        output_index: 0,
        kem_ciphertext: None,
    }
}

/// Mint a HYBRID (post-quantum) stealth output of `amount` picocredits owned by
/// `recipient` at `output_index`, shaped as `chain_getOutputs` reports it: with
/// its ML-KEM-768 ciphertext preserved. This is what a 6.0.0 producer emits and
/// what the RECEIVE scan must decapsulate to detect (#988).
fn mint_hybrid_owned_output(
    amount: u64,
    recipient: &PublicAddress,
    output_index: u32,
) -> ChainOutput {
    let out =
        TxOutput::new_hybrid_to_address(amount, recipient, output_index, None, Default::default())
            .expect("recipient publishes an ML-KEM key (v2 address)");
    ChainOutput {
        target_key: hex::encode(out.target_key),
        public_key: hex::encode(out.public_key),
        amount,
        output_index,
        kem_ciphertext: out.kem_ciphertext.as_ref().map(hex::encode),
    }
}

/// Mint a decoy output owned by a throwaway recipient (so it is never one of
/// the wallet's owned outputs), shaped for `chain_getOutputs`.
fn mint_decoy(amount: u64) -> ChainOutput {
    // A distinct mnemonic => distinct keys => the produced output does not belong
    // to the wallet under test, making it a valid ring decoy. Seed 200 is well
    // clear of the seeds the tests use for real wallets.
    let other = derive_from_seed(200);
    mint_owned_output(amount, &other.public_address)
}

#[test]
fn derive_is_deterministic() {
    // The same mnemonic always derives the same keys and address (a wallet must
    // recover identically across devices / restarts).
    let a = derive_from_seed(1);
    let b = derive_from_seed(1);
    assert_eq!(a.spend_private_hex, b.spend_private_hex);
    assert_eq!(a.view_private_hex, b.view_private_hex);
    assert_eq!(
        a.public_address.spend_public_key().to_bytes(),
        b.public_address.spend_public_key().to_bytes(),
    );
    // Sanity: derived hex keys are 32 bytes each.
    assert_eq!(a.spend_private_hex.len(), 64);
    assert_eq!(a.view_private_hex.len(), 64);
}

#[test]
fn derive_distinct_mnemonics_differ() {
    let a = derive_from_seed(1);
    let b = derive_from_seed(2);
    assert_ne!(a.spend_private_hex, b.spend_private_hex);
    assert_ne!(a.view_private_hex, b.view_private_hex);
}

#[test]
fn scan_recovers_owned_outputs_and_ignores_others() {
    let wallet = derive_from_seed(1);

    // Two owned outputs + two outputs belonging to someone else.
    let owned_a = mint_owned_output(5_000_000_000, &wallet.public_address);
    let owned_b = mint_owned_output(3_000_000_000, &wallet.public_address);
    let foreign_a = mint_decoy(7_000_000_000);
    let foreign_b = mint_decoy(9_000_000_000);

    let scanned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: wallet.spend_private_hex.clone(),
        view_private_key: wallet.view_private_hex.clone(),
        seed: wallet.seed_hex.clone(),
        outputs: vec![foreign_a, owned_a.clone(), foreign_b, owned_b.clone()],
    })
    .expect("scan should succeed");

    // Exactly the two owned outputs are recovered, with correct amounts and
    // subaddress index 0 (the default subaddress they were sent to).
    assert_eq!(scanned.len(), 2, "should recover exactly the owned outputs");
    let total: u64 = scanned.iter().map(|o| o.amount).sum();
    assert_eq!(total, 8_000_000_000);
    for o in &scanned {
        assert_eq!(o.subaddress_index, 0);
    }
    let targets: Vec<&str> = scanned.iter().map(|o| o.target_key.as_str()).collect();
    assert!(targets.contains(&owned_a.target_key.as_str()));
    assert!(targets.contains(&owned_b.target_key.as_str()));
}

#[test]
fn key_image_derivation_is_stable_and_unique_per_output() {
    let wallet = derive_from_seed(1);

    let scanned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: wallet.spend_private_hex.clone(),
        view_private_key: wallet.view_private_hex.clone(),
        seed: wallet.seed_hex.clone(),
        outputs: vec![
            mint_owned_output(5_000_000_000, &wallet.public_address),
            mint_owned_output(3_000_000_000, &wallet.public_address),
        ],
    })
    .expect("scan");

    let req = KeyImageRequest {
        spend_private_key: wallet.spend_private_hex.clone(),
        view_private_key: wallet.view_private_hex.clone(),
        seed: wallet.seed_hex.clone(),
        outputs: scanned,
    };

    let first = compute_owned_output_key_images_inner(&req).expect("key images");
    let second = compute_owned_output_key_images_inner(&req).expect("key images again");

    assert_eq!(first.len(), 2);
    // Key image is the spent-filter identity sent to `chain_areKeyImagesSpent`;
    // it must be deterministic (same output => same key image every time).
    for (a, b) in first.iter().zip(second.iter()) {
        assert_eq!(a.key_image, b.key_image);
        assert_eq!(a.key_image.len(), 64, "key image is a hex 32-byte value");
    }
    // Distinct outputs must produce distinct key images.
    assert_ne!(first[0].key_image, first[1].key_image);
}

#[test]
fn build_and_sign_produces_node_verifiable_tx() {
    let sender = derive_from_seed(1);
    let recipient = derive_from_seed(2);

    // One owned input big enough to cover amount + fee + change above dust.
    let input_amount = 100_000_000_000u64; // 0.1 BTH
    let owned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: sender.spend_private_hex.clone(),
        view_private_key: sender.view_private_hex.clone(),
        seed: sender.seed_hex.clone(),
        outputs: vec![mint_owned_output(input_amount, &sender.public_address)],
    })
    .expect("scan input");
    assert_eq!(owned.len(), 1);
    let input: &OwnedOutput = &owned[0];

    // Synthetic decoys (DEFAULT_RING_SIZE - 1) — foreign outputs, mirroring the
    // real send path's decoy gathering in `wallet_ops::send`.
    let decoys: Vec<DecoyOutput> = (0..DEFAULT_RING_SIZE - 1)
        .map(|i| {
            let d = mint_decoy(input_amount + i as u64 + 1);
            DecoyOutput {
                target_key: d.target_key,
                public_key: d.public_key,
                amount: d.amount,
            }
        })
        .collect();
    assert_eq!(decoys.len(), DEFAULT_RING_SIZE - 1);

    let amount = 10_000_000_000u64; // 0.01 BTH, comfortably above dust
    assert!(amount >= DUST_THRESHOLD);

    let recipient_addr = RecipientAddress {
        view_public_key: hex::encode(recipient.public_address.view_public_key().to_bytes()),
        spend_public_key: hex::encode(recipient.public_address.spend_public_key().to_bytes()),
        kem_public_key: recipient.kem_public_hex.clone(),
    };

    let tx_hex = build_and_sign_inner(&SignRequest {
        spend_private_key: sender.spend_private_hex.clone(),
        view_private_key: sender.view_private_hex.clone(),
        // Thread the seed so the signer can decapsulate a hybrid input on the
        // unified recovery path (#988); harmless for this classical input.
        seed: sender.seed_hex.clone(),
        inputs: vec![SpendInput {
            target_key: input.target_key.clone(),
            public_key: input.public_key.clone(),
            amount: input.amount,
            subaddress_index: input.subaddress_index,
            // Preserve the scanned output's hybrid metadata (#988).
            output_index: input.output_index,
            kem_ciphertext: input.kem_ciphertext.clone(),
            decoys,
        }],
        recipient: recipient_addr,
        sender_kem_public_key: sender.kem_public_hex.clone(),
        amount,
        fee: MIN_TX_FEE,
        created_at_height: 1,
        bridge_deposit_memo: None,
    })
    // `build_and_sign_inner` self-verifies structure + ring signatures + the
    // balance equation under the node's own verifier before returning, so a
    // successful result means the node would accept this tx.
    .expect("build+sign should produce a node-verifiable transaction");

    assert!(!tx_hex.is_empty());
    // Output is hex (the wire form submitted to `tx_submit`).
    assert!(hex::decode(&tx_hex).is_ok(), "signed tx must be valid hex");
}

/// #988 regression (mobile spend leg): a 6.0.0 HYBRID owned output must be
/// spendable via the exact mobile-shaped path (`wallet_ops::send` threads the
/// scanned `seed` + `output_index` + `kem_ciphertext` into the signer). Without
/// that threading the signer would classically recover the one-time key of a
/// hybrid input, derive the WRONG key, and self-verification of the CLSAG ring
/// signature would fail — the same bug this PR fixes for the web wallet. Minting
/// a hybrid input (`new_hybrid_to_address`), scanning it (which decapsulates and
/// preserves the metadata), and spending it end-to-end proves the mobile leg is
/// fixed too, not just left latent.
#[test]
fn build_and_sign_spends_hybrid_output() {
    let sender = derive_from_seed(1);
    let recipient = derive_from_seed(2);

    // A HYBRID (ciphertext-bearing) input bound to a non-trivial output index —
    // exactly the shape a 6.0.0 producer emits and `chain_getOutputs` reports.
    let input_amount = 100_000_000_000u64; // 0.1 BTH
    let hybrid_output_index = 3u32;
    let owned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: sender.spend_private_hex.clone(),
        view_private_key: sender.view_private_hex.clone(),
        // The seed-derived ML-KEM secret is what decapsulates the hybrid input.
        seed: sender.seed_hex.clone(),
        outputs: vec![mint_hybrid_owned_output(
            input_amount,
            &sender.public_address,
            hybrid_output_index,
        )],
    })
    .expect("scan hybrid input");
    assert_eq!(owned.len(), 1, "the hybrid output must be detected as owned");
    let input: &OwnedOutput = &owned[0];
    // The scan preserved the hybrid metadata the spend needs (#988).
    assert_eq!(input.output_index, hybrid_output_index);
    assert!(
        input.kem_ciphertext.is_some(),
        "scanned hybrid input must carry its ML-KEM ciphertext"
    );

    let decoys: Vec<DecoyOutput> = (0..DEFAULT_RING_SIZE - 1)
        .map(|i| {
            let d = mint_decoy(input_amount + i as u64 + 1);
            DecoyOutput {
                target_key: d.target_key,
                public_key: d.public_key,
                amount: d.amount,
            }
        })
        .collect();

    let amount = 10_000_000_000u64;
    assert!(amount >= DUST_THRESHOLD);

    let recipient_addr = RecipientAddress {
        view_public_key: hex::encode(recipient.public_address.view_public_key().to_bytes()),
        spend_public_key: hex::encode(recipient.public_address.spend_public_key().to_bytes()),
        kem_public_key: recipient.kem_public_hex.clone(),
    };

    // Build the SignRequest exactly as `wallet_ops::send` does: thread the seed
    // and the scanned output's hybrid metadata so the signer recovers the true
    // one-time key on the unified path.
    let tx_hex = build_and_sign_inner(&SignRequest {
        spend_private_key: sender.spend_private_hex.clone(),
        view_private_key: sender.view_private_hex.clone(),
        seed: sender.seed_hex.clone(),
        inputs: vec![SpendInput {
            target_key: input.target_key.clone(),
            public_key: input.public_key.clone(),
            amount: input.amount,
            subaddress_index: input.subaddress_index,
            output_index: input.output_index,
            kem_ciphertext: input.kem_ciphertext.clone(),
            decoys,
        }],
        recipient: recipient_addr,
        sender_kem_public_key: sender.kem_public_hex.clone(),
        amount,
        fee: MIN_TX_FEE,
        created_at_height: 1,
        bridge_deposit_memo: None,
    })
    // Self-verification (structure + CLSAG ring signatures + balance) runs under
    // the node's own verifier before returning; success means the hybrid input
    // was spent with the correct one-time key.
    .expect("hybrid input must be spendable with threaded seed + ciphertext");

    assert!(!tx_hex.is_empty());
    assert!(hex::decode(&tx_hex).is_ok(), "signed tx must be valid hex");
}

#[test]
fn build_and_sign_rejects_insufficient_inputs() {
    let sender = derive_from_seed(1);
    let recipient = derive_from_seed(2);

    // Input cannot cover amount + fee => the signer core must reject it (the
    // bridge maps this to the InsufficientFunds error variant).
    let input_amount = MIN_TX_FEE; // far below the amount we try to send
    let owned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: sender.spend_private_hex.clone(),
        view_private_key: sender.view_private_hex.clone(),
        seed: sender.seed_hex.clone(),
        outputs: vec![mint_owned_output(input_amount, &sender.public_address)],
    })
    .expect("scan");

    let decoys: Vec<DecoyOutput> = (0..DEFAULT_RING_SIZE - 1)
        .map(|i| {
            let d = mint_decoy(i as u64 + 1);
            DecoyOutput {
                target_key: d.target_key,
                public_key: d.public_key,
                amount: d.amount,
            }
        })
        .collect();

    let result = build_and_sign_inner(&SignRequest {
        spend_private_key: sender.spend_private_hex.clone(),
        view_private_key: sender.view_private_hex.clone(),
        seed: sender.seed_hex.clone(),
        inputs: vec![SpendInput {
            target_key: owned[0].target_key.clone(),
            public_key: owned[0].public_key.clone(),
            amount: owned[0].amount,
            subaddress_index: owned[0].subaddress_index,
            output_index: owned[0].output_index,
            kem_ciphertext: owned[0].kem_ciphertext.clone(),
            decoys,
        }],
        recipient: RecipientAddress {
            view_public_key: hex::encode(recipient.public_address.view_public_key().to_bytes()),
            spend_public_key: hex::encode(recipient.public_address.spend_public_key().to_bytes()),
            kem_public_key: recipient.kem_public_hex.clone(),
        },
        sender_kem_public_key: sender.kem_public_hex.clone(),
        amount: 50_000_000_000,
        fee: MIN_TX_FEE,
        created_at_height: 1,
        bridge_deposit_memo: None,
    });

    assert!(
        result.is_err(),
        "build+sign must reject inputs that cannot cover amount + fee"
    );
}

#[test]
fn scan_detects_hybrid_receive_and_recovers_key_image() {
    // #988: prove the mobile crate's RECEIVE scan (the same
    // `bth_wasm_signer::core::scan_owned_outputs_inner` the bridge's
    // `wallet_ops::sync` calls) detects a 6.0.0 HYBRID output by decapsulating
    // its ML-KEM ciphertext with the wallet's seed-derived secret — and then
    // recovers its key image so it is spendable. This is the exact gap that left
    // mobile wallets unable to see incoming funds / their own change.
    let wallet = derive_from_seed(1);
    let foreign = mint_decoy(9_000_000_000);

    // A hybrid payment to us (output index 0) plus a foreign hybrid output.
    let received = mint_hybrid_owned_output(5_000_000_000, &wallet.public_address, 0);
    assert!(
        received.kem_ciphertext.is_some(),
        "a 6.0.0 output must carry an ML-KEM ciphertext"
    );

    let scanned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: wallet.spend_private_hex.clone(),
        view_private_key: wallet.view_private_hex.clone(),
        seed: wallet.seed_hex.clone(),
        outputs: vec![foreign, received.clone()],
    })
    .expect("scan should succeed");

    assert_eq!(scanned.len(), 1, "only our hybrid output is detected");
    assert_eq!(scanned[0].amount, 5_000_000_000);
    assert_eq!(scanned[0].subaddress_index, 0);
    assert_eq!(scanned[0].target_key, received.target_key);
    assert!(
        scanned[0].kem_ciphertext.is_some(),
        "the detected output carries its ciphertext for spend recovery"
    );

    // The hybrid one-time spend key must recover (seed-aware key-image path), so
    // the received funds are spendable.
    let images = compute_owned_output_key_images_inner(&KeyImageRequest {
        spend_private_key: wallet.spend_private_hex.clone(),
        view_private_key: wallet.view_private_hex.clone(),
        seed: wallet.seed_hex.clone(),
        outputs: scanned,
    })
    .expect("key-image derivation must succeed for the detected hybrid output");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].key_image.len(), 64, "hex 32-byte key image");
}

#[test]
fn scan_ignores_hybrid_output_addressed_to_another_wallet() {
    // #988 negative: a hybrid output to someone else must NOT decapsulate to our
    // keys — the wrong ML-KEM secret yields a useless shared secret and the
    // stealth check rejects it.
    let me = derive_from_seed(1);
    let other = derive_from_seed(2);

    let theirs = mint_hybrid_owned_output(5_000_000_000, &other.public_address, 0);

    let scanned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: me.spend_private_hex.clone(),
        view_private_key: me.view_private_hex.clone(),
        seed: me.seed_hex.clone(),
        outputs: vec![theirs],
    })
    .expect("scan should succeed");

    assert!(
        scanned.is_empty(),
        "a hybrid output addressed to another wallet must not be detected"
    );
}
