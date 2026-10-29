//! V2 lottery payout scanning, key recovery and spending (#1645, P8 of #1286).
//!
//! The committed vector `test/fixtures/lottery-v2-payout.json` is a REAL V2
//! payout: a hybrid source output owned by the fixture wallet, plus the payout
//! row that `lottery_v2::derive` produces for it, in the frozen
//! `chain_getOutputs` shape (`cryptoOutputIndex`, `lotteryContext`,
//! `winnerOutpoint`, `clusterTags`, #1641). These native tests prove the
//! signer core's results equal NATIVE recovery (`recover_with_context` driven
//! directly by the `TxOutput` ownership primitives, the path `botho-wallet`
//! uses). The vitest suite `test/lottery-v2-wasm.test.ts` checks the same
//! vector through the actual compiled WASM module, so WASM == native.
//!
//! Regenerate (only if the vector format changes) with:
//! `cargo test -p bth-wasm-signer lottery_v2_tests::print_vector -- --ignored
//! --nocapture`

use super::*;
use bth_crypto_ring_signature::KeyImage;
use bth_transaction_clsag::lottery_v2::{self, Award, Domain, Outpoint, Source};
use rand::{rngs::StdRng, SeedableRng};
use serde_json::{json, Value};

const VECTOR: &str = include_str!("../../test/fixtures/lottery-v2-payout.json");

/// Fixture wallet seed byte: `derive_pq_keys_from_seed([SEED_BYTE; 64])`.
const SEED_BYTE: u8 = 0x5e;
/// The source output's derivation index `j` (its position in its tx).
const BASE_INDEX: u32 = 3;
/// The winning source outpoint (`winnerOutpoint`).
const WINNER_HASH: [u8; 32] = [7; 32];
const SOURCE_AMOUNT: u64 = 1_000_000_000_000;
const PAYOUT_AMOUNT: u64 = 5_000_000_000_000;
const PAYOUT_HEIGHT: u64 = 17;

/// The draw domain the fixture payout was derived under. The chain supplies
/// this to the node; the wallet only ever sees the resulting context.
fn domain() -> Domain {
    Domain {
        genesis: [1; 32],
        parent: [2; 32],
        height: PAYOUT_HEIGHT,
        ordinary_root: [5; 32],
        manifest: lottery_v2::manifest(&[Award {
            winner: Outpoint {
                hash: WINNER_HASH,
                index: BASE_INDEX,
            },
            amount: PAYOUT_AMOUNT,
        }])
        .unwrap(),
        ordinal: 0,
        amount: PAYOUT_AMOUNT,
    }
}

fn h32(s: &str) -> [u8; 32] {
    parse_hex_32("vector", s).unwrap()
}

fn vector() -> Value {
    serde_json::from_str(VECTOR).expect("committed vector is valid JSON")
}

fn row(v: &Value, name: &str) -> ChainOutput {
    serde_json::from_value(v[name].clone()).expect("row deserializes as ChainOutput")
}

fn tx_output(row: &ChainOutput) -> TxOutput {
    TxOutput {
        amount: row.amount,
        target_key: h32(&row.target_key),
        public_key: h32(&row.public_key),
        e_memo: None,
        cluster_tags: Default::default(),
        kem_ciphertext: row.kem_ciphertext.as_ref().map(|c| hex::decode(c).unwrap()),
    }
}

fn account(v: &Value) -> AccountKey {
    AccountKey::new(
        &parse_private("spend", v["spendPrivateKey"].as_str().unwrap()).unwrap(),
        &parse_private("view", v["viewPrivateKey"].as_str().unwrap()).unwrap(),
    )
}

fn kem_keypair() -> bth_crypto_pq::MlKem768KeyPair {
    bth_crypto_pq::derive_pq_keys_from_seed(&[SEED_BYTE; bth_crypto_pq::BIP39_SEED_SIZE])
        .kem_keypair
}

fn keys(v: &Value) -> (String, String, String) {
    (
        v["spendPrivateKey"].as_str().unwrap().to_string(),
        v["viewPrivateKey"].as_str().unwrap().to_string(),
        v["seed"].as_str().unwrap().to_string(),
    )
}

fn scan(v: &Value, outputs: Vec<ChainOutput>) -> Vec<OwnedOutput> {
    let (spend, view, seed) = keys(v);
    scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: spend,
        view_private_key: view,
        seed,
        outputs,
    })
    .unwrap()
}

/// NATIVE recovery, independent of this crate's helpers: drive the shared
/// `lottery_v2::recover_with_context` directly with the `TxOutput` hybrid
/// ownership primitives, exactly as the clsag lottery tests and botho-wallet
/// do.
fn native_key_image(
    account: &AccountKey,
    output: &TxOutput,
    context: &lottery_v2::Context,
) -> String {
    let kp = kem_keypair();
    let key = lottery_v2::recover_with_context(output, context, |base, index| {
        let sub = base.belongs_to_account(account, &kp, index)?;
        base.recover_spend_key_for(account, &kp, sub, index)
    })
    .expect("native recovery of the fixture payout");
    hex::encode(KeyImage::from(&key).as_bytes())
}

fn decoys(v: &Value) -> Vec<DecoyOutput> {
    v["decoys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| DecoyOutput {
            target_key: d["targetKey"].as_str().unwrap().to_string(),
            public_key: d["publicKey"].as_str().unwrap().to_string(),
            amount: d["amount"].as_u64().unwrap(),
        })
        .collect()
}

fn spend_request(v: &Value, input: SpendInput, amount: u64) -> SignRequest {
    let (spend, view, seed) = keys(v);
    SignRequest {
        spend_private_key: spend,
        view_private_key: view,
        seed,
        inputs: vec![input],
        recipient: serde_json::from_value(v["recipient"].clone()).unwrap(),
        sender_kem_public_key: v["senderKemPublicKey"].as_str().unwrap().to_string(),
        amount,
        fee: MIN_TX_FEE,
        created_at_height: PAYOUT_HEIGHT + 10,
        bridge_deposit_memo: None,
    }
}

fn payout_spend_input(v: &Value, owned: &OwnedOutput) -> SpendInput {
    SpendInput {
        target_key: owned.target_key.clone(),
        public_key: owned.public_key.clone(),
        amount: owned.amount,
        subaddress_index: owned.subaddress_index,
        output_index: owned.output_index,
        kem_ciphertext: owned.kem_ciphertext.clone(),
        lottery_context: owned.lottery_context.clone(),
        decoys: decoys(v),
    }
}

/// The fixture payout is a genuine V2 derivation of the fixture source, and
/// its base target `P − T·G` is the source's key.
#[test]
fn vector_payout_is_a_real_v2_derivation_of_the_source() {
    let v = vector();
    let source = row(&v, "source");
    let payout = row(&v, "payout");
    let derived = lottery_v2::derive(
        &Source {
            outpoint: Outpoint {
                hash: WINNER_HASH,
                index: BASE_INDEX,
            },
            target: h32(&source.target_key),
            context: lottery_v2::Context {
                base_index: BASE_INDEX,
                tweak: [0; 32],
            },
        },
        &domain(),
    )
    .unwrap();
    let context = payout
        .lottery_context
        .as_ref()
        .expect("payout row carries a context");
    assert_eq!(hex::encode(derived.target), payout.target_key);
    assert_eq!(context.to_context().unwrap(), derived.context);
    assert_eq!(context.base_index, BASE_INDEX);
    assert_eq!(v["payout"]["cryptoOutputIndex"], json!(BASE_INDEX));
    assert_eq!(v["expected"]["baseTarget"], json!(source.target_key));
    // The payout reuses the winner's ephemeral key and ciphertext (it is not an
    // ordinary output to the wallet), but has its own one-time key.
    assert_eq!(payout.public_key, source.public_key);
    assert_eq!(payout.kem_ciphertext, source.kem_ciphertext);
    assert_ne!(payout.target_key, source.target_key);
}

/// Signer-core scanning and key images equal native recovery for the same
/// payout (and the source stays separately owned under a distinct key image).
#[test]
fn signer_core_matches_native_recovery() {
    let v = vector();
    let account = account(&v);
    let source = row(&v, "source");
    let payout = row(&v, "payout");
    let context = payout
        .lottery_context
        .as_ref()
        .unwrap()
        .to_context()
        .unwrap();

    let native_payout = native_key_image(&account, &tx_output(&payout), &context);
    let native_source = native_key_image(
        &account,
        &tx_output(&source),
        &lottery_v2::Context {
            base_index: BASE_INDEX,
            tweak: [0; 32],
        },
    );
    assert_eq!(
        native_payout,
        v["expected"]["payoutKeyImage"].as_str().unwrap()
    );
    assert_eq!(
        native_source,
        v["expected"]["sourceKeyImage"].as_str().unwrap()
    );
    assert_ne!(
        native_payout, native_source,
        "payout must have its own key image"
    );

    let owned = scan(&v, vec![source.clone(), payout.clone()]);
    assert_eq!(
        owned.len(),
        2,
        "both the source and the V2 payout are owned"
    );
    assert_eq!(owned[0].lottery_context, None);
    assert_eq!(owned[1].target_key, payout.target_key);
    assert_eq!(owned[1].amount, PAYOUT_AMOUNT);
    assert_eq!(
        owned[1].subaddress_index,
        v["expected"]["subaddressIndex"].as_u64().unwrap()
    );
    assert_eq!(owned[1].lottery_context, payout.lottery_context);

    let (spend, view, seed) = keys(&v);
    let images = compute_owned_output_key_images_inner(&KeyImageRequest {
        spend_private_key: spend,
        view_private_key: view,
        seed,
        outputs: owned,
    })
    .unwrap();
    assert_eq!(images[0].key_image, native_source);
    assert_eq!(images[1].key_image, native_payout);
    assert_eq!(images[1].lottery_context, payout.lottery_context);
}

/// A discovered V2 payout can be spent: the signer recovers `x = x_base + T`,
/// the produced transaction verifies, and its key image is the payout's.
#[test]
fn discovered_payout_builds_a_verifying_spend() {
    let v = vector();
    let owned = scan(&v, vec![row(&v, "payout")]);
    assert_eq!(owned.len(), 1);
    let input = payout_spend_input(&v, &owned[0]);
    let req = spend_request(&v, input, 2_000_000_000_000);
    let mut rng = StdRng::from_seed([45u8; 32]);
    let tx = build_and_sign_with_rng(&req, &mut rng).expect("V2 payout spend builds");
    tx.verify_ring_signatures()
        .expect("node verifier accepts the spend");
    assert_eq!(
        hex::encode(tx.inputs.clsag()[0].key_image),
        v["expected"]["payoutKeyImage"].as_str().unwrap()
    );
    let out_total: u64 = tx.outputs.iter().map(|o| o.amount).sum();
    assert_eq!(out_total + tx.fee, PAYOUT_AMOUNT);

    // The hex entry point (what WASM `buildAndSign` returns) round-trips too.
    let hex_tx = build_and_sign_inner(&req).unwrap();
    let decoded: Transaction = bincode::deserialize(&hex::decode(hex_tx).unwrap()).unwrap();
    decoded.verify_ring_signatures().unwrap();
}

/// Mutate the payout row's context and check the payout is NOT discovered.
fn assert_not_owned(edit: impl Fn(&mut ChainOutput)) {
    let v = vector();
    let mut payout = row(&v, "payout");
    edit(&mut payout);
    assert!(scan(&v, vec![payout]).is_empty());
}

#[test]
fn context_less_payout_row_is_not_discovered_by_the_plain_check() {
    // Without its context the payout key P matches no ordinary derivation.
    assert_not_owned(|p| p.lottery_context = None);
}

#[test]
fn forged_tweak_yields_no_ownership() {
    assert_not_owned(|p| {
        let c = p.lottery_context.as_mut().unwrap();
        let mut t = h32(&c.tweak);
        t[0] ^= 1;
        c.tweak = hex::encode(t);
    });
}

#[test]
fn wrong_base_index_yields_no_ownership() {
    assert_not_owned(|p| p.lottery_context.as_mut().unwrap().base_index = BASE_INDEX + 1);
}

#[test]
fn malformed_contexts_yield_no_ownership() {
    assert_not_owned(|p| p.lottery_context.as_mut().unwrap().version = 1);
    assert_not_owned(|p| p.lottery_context.as_mut().unwrap().tweak = "00".repeat(32));
    // Non-canonical scalar (>= group order).
    assert_not_owned(|p| p.lottery_context.as_mut().unwrap().tweak = "ff".repeat(32));
    assert_not_owned(|p| p.lottery_context.as_mut().unwrap().tweak = "zz".into());
}

/// A wrong context at key-image or signing time is a hard error (the `x·G = P`
/// check), never a silently wrong key image or an invalid signature.
#[test]
fn forged_context_at_key_image_and_spend_is_a_hard_error() {
    let v = vector();
    let mut owned = scan(&v, vec![row(&v, "payout")]).remove(0);
    let c = owned.lottery_context.as_mut().unwrap();
    c.base_index += 1;

    let (spend, view, seed) = keys(&v);
    let err = compute_owned_output_key_images_inner(&KeyImageRequest {
        spend_private_key: spend,
        view_private_key: view,
        seed,
        outputs: vec![owned.clone()],
    })
    .unwrap_err();
    assert!(err.contains("V2 lottery payout"), "got: {err}");

    let req = spend_request(&v, payout_spend_input(&v, &owned), 2_000_000_000_000);
    let err = build_and_sign_with_rng(&req, &mut StdRng::from_seed([46u8; 32])).unwrap_err();
    assert!(err.contains("V2 lottery payout"), "got: {err}");

    // Dropping the context entirely also fails recovery rather than signing
    // with the base key.
    owned.lottery_context = None;
    let req = spend_request(&v, payout_spend_input(&v, &owned), 2_000_000_000_000);
    assert!(build_and_sign_with_rng(&req, &mut StdRng::from_seed([47u8; 32])).is_err());
}

/// The frozen #1641 row shape deserializes; unknown/extra fields are accepted
/// and an ordinary row without `lotteryContext` stays context-less.
#[test]
fn rpc_contract_rows_deserialize() {
    let v = vector();
    let payout: ChainOutput = serde_json::from_value(v["payout"].clone()).unwrap();
    assert!(v["payout"]["winnerOutpoint"]
        .as_str()
        .unwrap()
        .ends_with(":3"));
    assert_eq!(payout.lottery_context.as_ref().unwrap().ordinal, 0);
    assert_eq!(
        payout.lottery_context.as_ref().unwrap().height,
        PAYOUT_HEIGHT
    );
    let ordinary: ChainOutput = serde_json::from_value(json!({
        "targetKey": "00".repeat(32), "publicKey": "00".repeat(32), "amount": 1,
        "outputIndex": 0, "cryptoOutputIndex": 0, "clusterTags": [[1, 2]], "kemCiphertext": null,
    }))
    .unwrap();
    assert_eq!(ordinary.lottery_context, None);
    // OwnedOutput serializes without a `lotteryContext` key for ordinary outputs.
    let owned = OwnedOutput {
        target_key: String::new(),
        public_key: String::new(),
        amount: 1,
        subaddress_index: 0,
        output_index: 0,
        kem_ciphertext: None,
        lottery_context: None,
    };
    assert!(serde_json::to_value(owned)
        .unwrap()
        .get("lotteryContext")
        .is_none());
}

/// Regenerate the committed vector. The hybrid encapsulation is randomized, so
/// the output differs per run; commit a new vector only when the format
/// changes.
#[test]
#[ignore]
fn print_vector() {
    let mut rng = StdRng::from_seed([0x45; 32]);
    let wallet = AccountKey::random(&mut rng);
    let pq = bth_crypto_pq::derive_pq_keys_from_seed(&[SEED_BYTE; bth_crypto_pq::BIP39_SEED_SIZE]);
    let kem_public = pq.kem_keypair.public_key().as_bytes().to_vec();
    let sub = wallet.default_subaddress();
    let address = PublicAddress::new_with_pq(
        sub.spend_public_key(),
        sub.view_public_key(),
        kem_public.clone(),
        Vec::new(),
    );
    let source = TxOutput::new_hybrid_to_address(
        SOURCE_AMOUNT,
        &address,
        BASE_INDEX,
        None,
        Default::default(),
    )
    .unwrap();
    let derived = lottery_v2::derive(
        &Source {
            outpoint: Outpoint {
                hash: WINNER_HASH,
                index: BASE_INDEX,
            },
            target: source.target_key,
            context: lottery_v2::Context {
                base_index: BASE_INDEX,
                tweak: [0; 32],
            },
        },
        &domain(),
    )
    .unwrap();
    let mut payout = source.clone();
    payout.amount = PAYOUT_AMOUNT;
    payout.target_key = derived.target;

    let kp = &pq.kem_keypair;
    let key_image = |out: &TxOutput, ctx: &lottery_v2::Context| {
        let key = lottery_v2::recover_with_context(out, ctx, |base, index| {
            let s = base.belongs_to_account(&wallet, kp, index)?;
            base.recover_spend_key_for(&wallet, kp, s, index)
        })
        .unwrap();
        hex::encode(KeyImage::from(&key).as_bytes())
    };
    let zero = lottery_v2::Context {
        base_index: BASE_INDEX,
        tweak: [0; 32],
    };

    let recipient = AccountKey::random(&mut rng);
    let recipient_pq = bth_crypto_pq::derive_pq_keys_from_seed(&[0x7a; 64]);
    let decoys: Vec<Value> = (0..DEFAULT_RING_SIZE - 1)
        .map(|_| {
            let acct = AccountKey::random(&mut rng);
            let out = TxOutput::new(SOURCE_AMOUNT, &acct.default_subaddress());
            json!({"targetKey": hex::encode(out.target_key), "publicKey": hex::encode(out.public_key),
                   "amount": SOURCE_AMOUNT, "outputIndex": 0, "kemCiphertext": null})
        })
        .collect();
    let ct = source.kem_ciphertext.as_ref().map(hex::encode);
    let vector = json!({
        "version": 1,
        "description": "V2 lottery payout vector (#1645): a hybrid source output owned by the wallet at index 3 and the payout lottery_v2::derive produces for it, as frozen chain_getOutputs rows (#1641). Native recovery (recover_with_context) and the compiled WASM signer must both yield expected.*",
        "spendPrivateKey": hex::encode(wallet.spend_private_key().to_bytes()),
        "viewPrivateKey": hex::encode(wallet.view_private_key().to_bytes()),
        "seed": hex::encode([SEED_BYTE; 64]),
        "senderKemPublicKey": hex::encode(&kem_public),
        "recipient": {
            "spend_public_key": hex::encode(recipient.default_subaddress().spend_public_key().to_bytes()),
            "view_public_key": hex::encode(recipient.default_subaddress().view_public_key().to_bytes()),
            "kem_public_key": hex::encode(recipient_pq.kem_keypair.public_key().as_bytes()),
        },
        "source": {
            "txHash": hex::encode(WINNER_HASH), "outputIndex": BASE_INDEX, "cryptoOutputIndex": BASE_INDEX,
            "targetKey": hex::encode(source.target_key), "publicKey": hex::encode(source.public_key),
            "amount": SOURCE_AMOUNT, "clusterTags": [], "kemCiphertext": ct,
        },
        "payout": {
            "txHash": "0b".repeat(32), "outputIndex": 1, "cryptoOutputIndex": BASE_INDEX,
            "targetKey": hex::encode(payout.target_key), "publicKey": hex::encode(payout.public_key),
            "amount": PAYOUT_AMOUNT, "clusterTags": [], "kemCiphertext": ct, "lottery": true,
            "lotteryContext": {"version": 2, "baseIndex": BASE_INDEX, "tweak": hex::encode(derived.context.tweak),
                               "ordinal": 0, "height": PAYOUT_HEIGHT},
            "winnerOutpoint": format!("{}:{}", hex::encode(WINNER_HASH), BASE_INDEX),
        },
        "decoys": decoys,
        "expected": {
            "baseTarget": hex::encode(source.target_key),
            "subaddressIndex": 0,
            "sourceKeyImage": key_image(&source, &zero),
            "payoutKeyImage": key_image(&payout, &derived.context),
        },
    });
    println!("{}", serde_json::to_string_pretty(&vector).unwrap());
}
