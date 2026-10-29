//! Shared wallet-side V2 payout vector (#1667 part 2).
//!
//! `fixtures/lottery-v2-wallet.json` is the ONE vector every V2 payout consumer
//! runs: this crate's shared helpers ([`payout_context`], [`base_target`],
//! [`recover_payout_owner`]), the wasm-signer core, the bridge reserve scan,
//! botho-wallet and the node wallet. Each consumer parses the wire
//! `lotteryContext` with its own syntax rules and supplies its own ordinary
//! ownership callbacks, then asserts the outcomes recorded here, so the four
//! implementations cannot drift apart without a vector failure.
//!
//! Sections:
//! - `contexts`: wire `lotteryContext` objects and the shared verdict (`"ok"`,
//!   `"Version"`, `"ZeroTweak"`, `"NonCanonicalTweak"`, or `"Syntax"` for a
//!   tweak that is not 32 bytes of hex). `expectStrictHex` overrides the
//!   verdict for botho-wallet, which also requires lowercase hex.
//! - `baseTargets`: `P − T·G` for a target and tweak, or the error name.
//! - `recoveries`: outputs (classical and hybrid) under a wire context, with
//!   the expected owning subaddress and key image of `x_base + T`, or `null`
//!   when the wallet must NOT discover the output.
//!
//! The committed file is static; `print_wallet_vector` regenerates a fresh
//! (randomized hybrid) instance only if the format has to change.
use bth_crypto_keys::RistrettoPrivate;
use bth_transaction_clsag::{lottery_v2::*, TxOutput};
use serde_json::Value;

const VECTOR: &str = include_str!("fixtures/lottery-v2-wallet.json");

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
fn h32(s: &str) -> Option<Hash> {
    unhex(s)?.try_into().ok()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn error_name(e: Error) -> String {
    format!("{e:?}")
}

/// The shared verdict for one wire `lotteryContext`, through
/// [`payout_context`] after a lenient 32-byte hex parse of the tweak.
fn verdict(wire: &Value) -> Result<Context, String> {
    let version = wire["version"].as_u64().unwrap() as u8;
    let base_index = wire["baseIndex"].as_u64().unwrap() as u32;
    // The shared rule checks the version before the consumer's tweak syntax.
    if version != CONTEXT_VERSION {
        return Err("Version".into());
    }
    let tweak = h32(wire["tweak"].as_str().unwrap()).ok_or("Syntax")?;
    payout_context(version, base_index, tweak).map_err(error_name)
}

#[test]
fn shared_context_verdicts() {
    let v: Value = serde_json::from_str(VECTOR).unwrap();
    let cases = v["contexts"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for case in cases {
        let got = match verdict(&case["wire"]) {
            Ok(_) => "ok".to_string(),
            Err(e) => e,
        };
        assert_eq!(got, case["expect"].as_str().unwrap(), "{}", case["name"]);
    }
}

#[test]
fn shared_base_targets() {
    let v: Value = serde_json::from_str(VECTOR).unwrap();
    for case in v["baseTargets"].as_array().unwrap() {
        let context = Context {
            base_index: case["baseIndex"].as_u64().unwrap() as u32,
            tweak: h32(case["tweak"].as_str().unwrap()).unwrap(),
        };
        let target = h32(case["targetKey"].as_str().unwrap()).unwrap();
        let got = match base_target(&target, &context) {
            Ok(base) => hex(&base),
            Err(e) => error_name(e),
        };
        assert_eq!(got, case["expect"].as_str().unwrap(), "{}", case["name"]);
    }
}

/// The base target `base_target` reports is exactly the base output
/// `recover_with_context` hands to the wallet callback, and a rejected base
/// target never reaches the callback.
#[test]
fn base_target_is_the_recovery_callback_target() {
    let v: Value = serde_json::from_str(VECTOR).unwrap();
    for case in v["baseTargets"].as_array().unwrap() {
        let context = Context {
            base_index: case["baseIndex"].as_u64().unwrap() as u32,
            tweak: h32(case["tweak"].as_str().unwrap()).unwrap(),
        };
        let output = TxOutput {
            amount: 1,
            target_key: h32(case["targetKey"].as_str().unwrap()).unwrap(),
            // Any valid point: the base point of the classical record vector.
            public_key: h32("bce83f8ba5dd2fa572864c24ba1810f9522bc6004afe95877ac73241cafdab42")
                .unwrap(),
            kem_ciphertext: None,
            e_memo: None,
            cluster_tags: Default::default(),
        };
        let mut seen = None;
        let result = recover_with_context(&output, &context, |base, index| {
            seen = Some((hex(&base.target_key), index));
            None
        });
        match base_target(&output.target_key, &context) {
            Ok(base) => {
                assert_eq!(result, Err(Error::Ownership), "{}", case["name"]);
                assert_eq!(seen, Some((hex(&base), context.base_index)));
            }
            Err(e) => {
                assert_eq!(result, Err(e), "{}", case["name"]);
                assert_eq!(seen, None, "{}", case["name"]);
            }
        }
    }
}

/// The owner helpers report an owner only together with a key that passes
/// `x·G = P`: a callback that claims ownership with the wrong base key yields
/// `Ownership`, never the owner.
#[test]
fn owner_is_never_reported_without_the_xg_proof() {
    let v: Value = serde_json::from_str(VECTOR).unwrap();
    let case = &v["recoveries"][0];
    let o = &case["output"];
    let output = TxOutput {
        amount: 0,
        target_key: h32(o["targetKey"].as_str().unwrap()).unwrap(),
        public_key: h32(o["publicKey"].as_str().unwrap()).unwrap(),
        e_memo: None,
        cluster_tags: Default::default(),
        kem_ciphertext: None,
    };
    let context = verdict(&case["wire"]).unwrap();
    let wrong = || RistrettoPrivate::from(bth_crypto_ring_signature::Scalar::from(7u64));
    assert_eq!(
        recover_payout_owner(&output, &context, |_, _| Some((0u64, wrong()))).err(),
        Some(Error::Ownership)
    );
    assert_eq!(
        recover_direct_owner(&output, 0, |_, _| Some((0u64, wrong()))).err(),
        Some(Error::Ownership)
    );
    // A rejected context never reaches the callback.
    let zero = Context {
        base_index: 0,
        tweak: [0; 32],
    };
    let never = |_: &TxOutput, _| -> Option<(u64, RistrettoPrivate)> {
        panic!("callback reached for a zero tweak")
    };
    assert_eq!(
        recover_payout_owner(&output, &zero, never).err(),
        Some(Error::ZeroTweak)
    );
}

#[cfg(feature = "pq")]
mod hybrid {
    use super::*;
    use bth_account_keys::AccountKey;
    use bth_crypto_pq::MlKem768KeyPair;
    use bth_crypto_ring_signature::KeyImage;

    pub(super) fn account(v: &Value) -> (AccountKey, MlKem768KeyPair) {
        let a = &v["account"];
        let private = |name: &str| {
            RistrettoPrivate::try_from(&h32(a[name].as_str().unwrap()).unwrap()).unwrap()
        };
        let seed: [u8; 64] = unhex(a["seed"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        (
            AccountKey::new(&private("spendPrivateKey"), &private("viewPrivateKey")),
            bth_crypto_pq::derive_pq_keys_from_seed(&seed).kem_keypair,
        )
    }

    pub(super) fn output(o: &Value) -> TxOutput {
        TxOutput {
            amount: 0,
            target_key: h32(o["targetKey"].as_str().unwrap()).unwrap(),
            public_key: h32(o["publicKey"].as_str().unwrap()).unwrap(),
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext: o["kemCiphertext"].as_str().map(|s| unhex(s).unwrap()),
        }
    }

    /// Every recovery case through the shared owner helper, with the unified
    /// hybrid-when-present ordinary rule as the base callback.
    #[test]
    fn shared_recoveries() {
        let v: Value = serde_json::from_str(VECTOR).unwrap();
        let (account, kp) = account(&v);
        let cases = v["recoveries"].as_array().unwrap();
        assert!(cases.iter().any(|c| !c["expect"].is_null()));
        assert!(cases
            .iter()
            .any(|c| c["output"]["kemCiphertext"].is_string()));
        for case in cases {
            let output = output(&case["output"]);
            let context = verdict(&case["wire"]).unwrap();
            let got = recover_payout_owner(&output, &context, |base, index| {
                let sub = base.belongs_to_account(&account, &kp, index)?;
                Some((sub, base.recover_spend_key_for(&account, &kp, sub, index)?))
            });
            match case["expect"].as_object() {
                None => assert_eq!(got.err(), Some(Error::Ownership), "{}", case["name"]),
                Some(expect) => {
                    let (sub, key) = got.unwrap();
                    assert_eq!(sub, expect["subaddressIndex"].as_u64().unwrap());
                    assert_eq!(
                        hex(KeyImage::from(&key).as_bytes()),
                        expect["keyImage"].as_str().unwrap(),
                        "{}",
                        case["name"]
                    );
                }
            }
        }
    }

    /// `recover_direct_owner` on the untweaked source of each owned payout
    /// reports the same owner (lineage start of the same family).
    #[test]
    fn shared_direct_owner_matches_payout_owner() {
        let v: Value = serde_json::from_str(VECTOR).unwrap();
        let (account, kp) = account(&v);
        for case in v["recoveries"].as_array().unwrap() {
            let (Some(expect), Some(source)) = (case["expect"].as_object(), case.get("source"))
            else {
                continue;
            };
            let base_index = case["wire"]["baseIndex"].as_u64().unwrap() as u32;
            let (sub, _) = recover_direct_owner(&output(source), base_index, |base, index| {
                let sub = base.belongs_to_account(&account, &kp, index)?;
                Some((sub, base.recover_spend_key_for(&account, &kp, sub, index)?))
            })
            .unwrap();
            assert_eq!(sub, expect["subaddressIndex"].as_u64().unwrap());
        }
    }
}

/// Regenerate the vector (randomized hybrid encapsulation and ephemeral keys):
/// `cargo test -p bth-transaction-clsag --features pq --test lottery_v2_wallet
/// print_wallet_vector -- --ignored --nocapture`. Expected values are computed
/// from the ordinary primitives (`x_base + T`, checked against `P`), not from
/// the helpers under test.
#[cfg(feature = "pq")]
#[test]
#[ignore]
fn print_wallet_vector() {
    use bth_account_keys::AccountKey;
    use bth_crypto_keys::RistrettoPublic;
    use bth_crypto_ring_signature::{KeyImage, Scalar};
    use serde_json::json;
    use sha2::{Digest, Sha512};

    let scalar = |label: &str| Scalar::from_bytes_mod_order_wide(&Sha512::digest(label).into());
    let private = |label: &str| RistrettoPrivate::from(scalar(label));
    let g = |s: Scalar| RistrettoPublic::from(&RistrettoPrivate::from(s));
    let seed = [0x6a_u8; 64];
    let spend = private("botho lottery v2 wallet vector: spend");
    let view = private("botho lottery v2 wallet vector: view");
    let account = AccountKey::new(&spend, &view);
    let kp = bth_crypto_pq::derive_pq_keys_from_seed(&seed).kem_keypair;
    let foreign = AccountKey::new(
        &private("botho lottery v2 wallet vector: foreign spend"),
        &private("botho lottery v2 wallet vector: foreign view"),
    );
    let tweak = scalar("botho lottery v2 wallet vector: tweak");
    let t = hex(&tweak.to_bytes());
    let wire = |base_index: u32, tweak: &str| json!({"version": 2, "baseIndex": base_index, "tweak": tweak, "ordinal": 0, "height": 17});
    let row = |o: &TxOutput| {
        json!({"targetKey": hex(&o.target_key), "publicKey": hex(&o.public_key),
               "kemCiphertext": o.kem_ciphertext.as_ref().map(|c| hex(c))})
    };
    let pay = |source: &TxOutput, t: Scalar| {
        let mut payout = source.clone();
        let p = RistrettoPublic::try_from(&source.target_key[..]).unwrap();
        payout.target_key = RistrettoPublic::from(p.as_ref() + g(t).as_ref()).to_bytes();
        payout
    };
    // Independent expectation: x_base from the ordinary primitive, plus T,
    // proven against the payout target.
    let expect = |source: &TxOutput, payout: &TxOutput, index: u32, t: Scalar| {
        let sub = source.belongs_to_account(&account, &kp, index).unwrap();
        let base = source
            .recover_spend_key_for(&account, &kp, sub, index)
            .unwrap();
        let x = RistrettoPrivate::from(*AsRef::<Scalar>::as_ref(&base) + t);
        assert_eq!(RistrettoPublic::from(&x).to_bytes(), payout.target_key);
        json!({"subaddressIndex": sub, "keyImage": hex(KeyImage::from(&x).as_bytes())})
    };

    let classical = TxOutput::new_with_key(
        1,
        &account.default_subaddress(),
        &private("botho lottery v2 wallet vector: r1"),
    );
    // Hybrid change: the hybrid detector reports the actual change subaddress
    // index. (The classical detector reports the label 1, which is not a
    // recoverable subaddress index, so a classical change case would not be a
    // valid ownership vector.)
    let change = TxOutput::new_hybrid(
        1,
        &account.change_subaddress(),
        kp.public_key(),
        0,
        None,
        Default::default(),
    );
    let hybrid = TxOutput::new_hybrid(
        1,
        &account.default_subaddress(),
        kp.public_key(),
        3,
        None,
        Default::default(),
    );
    let foreign_source = TxOutput::new_with_key(
        1,
        &foreign.default_subaddress(),
        &private("botho lottery v2 wallet vector: r3"),
    );
    let (c_pay, ch_pay, h_pay, f_pay) = (
        pay(&classical, tweak),
        pay(&change, tweak),
        pay(&hybrid, tweak),
        pay(&foreign_source, tweak),
    );
    let forged = hex(&(tweak + Scalar::ONE).to_bytes());
    let recoveries = json!([
        {"name": "classical payout, default subaddress", "output": row(&c_pay), "source": row(&classical),
         "wire": wire(0, &t), "expect": expect(&classical, &c_pay, 0, tweak)},
        {"name": "hybrid payout, change subaddress", "output": row(&ch_pay), "source": row(&change),
         "wire": wire(0, &t), "expect": expect(&change, &ch_pay, 0, tweak)},
        {"name": "hybrid payout at baseIndex 3", "output": row(&h_pay), "source": row(&hybrid),
         "wire": wire(3, &t), "expect": expect(&hybrid, &h_pay, 3, tweak)},
        {"name": "hybrid payout, wrong baseIndex", "output": row(&h_pay), "wire": wire(2, &t), "expect": null},
        {"name": "hybrid payout, forged tweak", "output": row(&h_pay), "wire": wire(3, &forged), "expect": null},
        {"name": "classical payout, forged tweak", "output": row(&c_pay), "wire": wire(0, &forged), "expect": null},
        {"name": "foreign payout", "output": row(&f_pay), "wire": wire(0, &t), "expect": null},
        {"name": "untweaked source under a payout context", "output": row(&classical), "wire": wire(0, &t), "expect": null},
    ]);

    // l, l + 1 and 2^256 - 1 little-endian.
    let l = "edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010";
    let l1 = "eed3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010";
    let max = "ff".repeat(32);
    let zero = "00".repeat(32);
    let ctx = |name: &str, wire: Value, expect: &str| json!({"name": name, "wire": wire, "expect": expect});
    let with_version = |version: u8| {
        let mut w = wire(3, &t);
        w["version"] = json!(version);
        w
    };
    let mut upper = ctx("uppercase tweak hex", wire(3, &t.to_uppercase()), "ok");
    upper["expectStrictHex"] = json!("Syntax");
    let contexts = json!([
        ctx("valid", wire(3, &t), "ok"),
        ctx("valid, baseIndex 0", wire(0, &t), "ok"),
        ctx("valid, baseIndex u32::MAX", wire(u32::MAX, &t), "ok"),
        ctx(
            "valid, tweak = 1",
            wire(3, &hex(&Scalar::ONE.to_bytes())),
            "ok"
        ),
        ctx(
            "valid, tweak = l - 1",
            wire(3, &hex(&(-Scalar::ONE).to_bytes())),
            "ok"
        ),
        upper,
        ctx("version 0", with_version(0), "Version"),
        ctx("version 1", with_version(1), "Version"),
        ctx("version 3", with_version(3), "Version"),
        ctx("version 255", with_version(255), "Version"),
        ctx(
            "version 1 with malformed tweak",
            {
                let mut w = wire(3, "zz");
                w["version"] = json!(1);
                w
            },
            "Version"
        ),
        ctx("zero tweak", wire(3, &zero), "ZeroTweak"),
        ctx("tweak = l", wire(3, l), "NonCanonicalTweak"),
        ctx("tweak = l + 1", wire(3, l1), "NonCanonicalTweak"),
        ctx("tweak = 2^256 - 1", wire(3, &max), "NonCanonicalTweak"),
        ctx("tweak 31 bytes", wire(3, &t[..62]), "Syntax"),
        ctx("tweak 33 bytes", wire(3, &format!("{t}00")), "Syntax"),
        ctx("tweak odd length", wire(3, &t[..63]), "Syntax"),
        ctx(
            "tweak not hex",
            wire(3, &format!("zz{}", &t[2..])),
            "Syntax"
        ),
        ctx("tweak empty", wire(3, ""), "Syntax"),
    ]);

    let base = g(scalar("botho lottery v2 wallet vector: base"));
    let target = RistrettoPublic::from(base.as_ref() + g(tweak).as_ref());
    let base_case = |name: &str, target: String, tweak: &str, expect: String| json!({"name": name, "targetKey": target, "baseIndex": 3, "tweak": tweak, "expect": expect});
    let base_targets = json!([
        base_case(
            "P - T·G",
            hex(&target.to_bytes()),
            &t,
            hex(&base.to_bytes())
        ),
        base_case(
            "payout output of the classical case",
            hex(&c_pay.target_key),
            &t,
            hex(&classical.target_key)
        ),
        base_case(
            "payout output of the hybrid case",
            hex(&h_pay.target_key),
            &t,
            hex(&hybrid.target_key)
        ),
        base_case("identity target", zero.clone(), &t, "Point".into()),
        base_case("target not a point", max.clone(), &t, "Point".into()),
        base_case(
            "identity base (P = T·G)",
            hex(&g(tweak).to_bytes()),
            &t,
            "Point".into()
        ),
        base_case(
            "zero tweak",
            hex(&target.to_bytes()),
            &zero,
            "ZeroTweak".into()
        ),
        base_case(
            "non-canonical tweak",
            hex(&target.to_bytes()),
            l,
            "NonCanonicalTweak".into()
        ),
    ]);

    let vector = json!({
        "description": "Shared V2 lottery payout wallet vector (#1667 part 2). Run by bth-transaction-clsag (shared helpers), bth-wasm-signer, bth-bridge-service, botho-wallet and the botho node wallet. See transaction/clsag/tests/lottery_v2_wallet.rs.",
        "version": 1,
        "account": {"spendPrivateKey": hex(&spend.to_bytes()), "viewPrivateKey": hex(&view.to_bytes()), "seed": hex(&seed)},
        "contexts": contexts,
        "baseTargets": base_targets,
        "recoveries": recoveries,
    });
    println!("{}", serde_json::to_string_pretty(&vector).unwrap());
}
