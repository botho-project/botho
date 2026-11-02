//! NIST ACVP known-answer tests (FIPS 203 / FIPS 204) for ML-KEM-768 and
//! ML-DSA-65, run against BOTH libcrux (every compiled backend) and the
//! incumbent RustCrypto crates.
//!
//! The vectors are the ACVP-Server `internalProjection.json` files, which are
//! too large to commit; fetch them first with `./fetch-kats.sh` (pinned
//! commit + SHA-256 check) and point `LIBCRUX_EVAL_KAT_DIR` at the directory.

use libcrux_eval::*;
use serde_json::Value;
use std::path::PathBuf;

fn kat_dir() -> PathBuf {
    let dir = std::env::var("LIBCRUX_EVAL_KAT_DIR").unwrap_or_else(|_| {
        panic!("set LIBCRUX_EVAL_KAT_DIR to the directory populated by ./fetch-kats.sh")
    });
    PathBuf::from(dir)
}

fn load(name: &str) -> Value {
    let p = kat_dir().join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap()
}

fn h(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn arr<const N: usize>(v: &Value) -> [u8; N] {
    h(v).try_into().unwrap()
}

fn groups<'a>(doc: &'a Value, pred: impl Fn(&Value) -> bool) -> Vec<&'a Value> {
    doc["testGroups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| pred(g))
        .collect()
}

#[test]
fn mlkem768_keygen_kat() {
    let doc = load("ML-KEM-keyGen-FIPS203");
    let mut n = 0;
    for g in groups(&doc, |g| g["parameterSet"] == "ML-KEM-768") {
        for t in g["tests"].as_array().unwrap() {
            let mut dz = [0u8; 64];
            dz[..32].copy_from_slice(&h(&t["d"]));
            dz[32..].copy_from_slice(&h(&t["z"]));
            let ek = h(&t["ek"]);
            let dk = h(&t["dk"]);
            for b in Backend::all() {
                let (pk, sk) = lc_kem::keygen(b, dz);
                assert_eq!(pk.to_vec(), ek, "libcrux {b:?} ek tc {}", t["tcId"]);
                assert_eq!(sk.to_vec(), dk, "libcrux {b:?} dk tc {}", t["tcId"]);
            }
            let (pk, sk) = rc_kem::keygen(dz);
            assert_eq!(pk.to_vec(), ek, "rustcrypto ek tc {}", t["tcId"]);
            assert_eq!(sk.to_vec(), dk, "rustcrypto dk tc {}", t["tcId"]);
            n += 1;
        }
    }
    assert!(n > 0);
    eprintln!(
        "ML-KEM-768 keyGen KAT: {n}/{n} pass (libcrux backends {:?} + RustCrypto)",
        Backend::all()
    );
}

#[test]
fn mlkem768_encap_decap_kat() {
    let doc = load("ML-KEM-encapDecap-FIPS203");
    let (mut enc, mut dec) = (0, 0);
    for g in groups(&doc, |g| g["parameterSet"] == "ML-KEM-768") {
        let func = g["function"].as_str().unwrap();
        for t in g["tests"].as_array().unwrap() {
            match func {
                "encapsulation" => {
                    let ek: [u8; KEM_PK] = arr(&t["ek"]);
                    let m: [u8; 32] = arr(&t["m"]);
                    let (c, k) = (h(&t["c"]), h(&t["k"]));
                    for b in Backend::all() {
                        let (ct, ss) = lc_kem::encaps(b, &ek, m);
                        assert_eq!(ct.to_vec(), c, "libcrux {b:?} c tc {}", t["tcId"]);
                        assert_eq!(ss.to_vec(), k, "libcrux {b:?} k tc {}", t["tcId"]);
                    }
                    let (ct, ss) = rc_kem::encaps(&ek, m);
                    assert_eq!(ct.to_vec(), c, "rustcrypto c tc {}", t["tcId"]);
                    assert_eq!(ss.to_vec(), k, "rustcrypto k tc {}", t["tcId"]);
                    enc += 1;
                }
                "decapsulation" => {
                    // Includes implicit-rejection cases (modified ciphertexts):
                    // the expected k is the rejection key J(z || c).
                    let dk: [u8; KEM_SK] = arr(&t["dk"]);
                    let c: [u8; KEM_CT] = arr(&t["c"]);
                    let k = h(&t["k"]);
                    for b in Backend::all() {
                        assert_eq!(
                            lc_kem::decaps(b, &dk, &c).to_vec(),
                            k,
                            "libcrux {b:?} tc {}",
                            t["tcId"]
                        );
                    }
                    assert_eq!(
                        rc_kem::decaps(&dk, &c).to_vec(),
                        k,
                        "rustcrypto tc {}",
                        t["tcId"]
                    );
                    dec += 1;
                }
                _ => {}
            }
        }
    }
    assert!(enc > 0 && dec > 0);
    eprintln!("ML-KEM-768 encapsulation KAT: {enc}/{enc}; decapsulation KAT (incl. implicit rejection): {dec}/{dec} (libcrux + RustCrypto)");
}

/// FIPS 203 §7.2/§7.3 input checks. libcrux implements both; RustCrypto
/// ml-kem 0.2.3 implements neither (there is no API to call), so this test
/// records libcrux's results only and reports that gap.
#[test]
fn mlkem768_key_check_kat() {
    let doc = load("ML-KEM-encapDecap-FIPS203");
    let (mut ek_n, mut dk_n, mut ek_bad, mut dk_bad) = (0, 0, 0, 0);
    for g in groups(&doc, |g| g["parameterSet"] == "ML-KEM-768") {
        let func = g["function"].as_str().unwrap();
        for t in g["tests"].as_array().unwrap() {
            let expect = t["testPassed"].as_bool();
            match func {
                "encapsulationKeyCheck" => {
                    let ek: [u8; KEM_PK] = arr(&t["ek"]);
                    for b in Backend::all() {
                        assert_eq!(
                            Some(lc_kem::validate_pk(b, &ek)),
                            expect,
                            "{b:?} tc {}",
                            t["tcId"]
                        );
                    }
                    if expect == Some(false) {
                        ek_bad += 1;
                        // RustCrypto has no check: it silently encapsulates
                        // to the non-canonical key (no panic, no error).
                        let _ = rc_kem::encaps(&ek, [7u8; 32]);
                        // libcrux's unchecked encapsulate must not panic either.
                        for b in Backend::all() {
                            let _ = lc_kem::encaps(b, &ek, [7u8; 32]);
                        }
                    }
                    ek_n += 1;
                }
                "decapsulationKeyCheck" => {
                    let dk: [u8; KEM_SK] = arr(&t["dk"]);
                    for b in Backend::all() {
                        assert_eq!(
                            Some(lc_kem::validate_sk(b, &dk)),
                            expect,
                            "{b:?} tc {}",
                            t["tcId"]
                        );
                    }
                    if expect == Some(false) {
                        dk_bad += 1;
                    }
                    dk_n += 1;
                }
                _ => {}
            }
        }
    }
    assert!(ek_n > 0 && dk_n > 0);
    eprintln!(
        "ML-KEM-768 key checks (libcrux only; RustCrypto 0.2.3 has no check API): \
         encapsulationKeyCheck {ek_n}/{ek_n} ({ek_bad} invalid keys rejected), \
         decapsulationKeyCheck {dk_n}/{dk_n} ({dk_bad} invalid keys rejected)"
    );
}

#[test]
fn mldsa65_keygen_kat() {
    let doc = load("ML-DSA-keyGen-FIPS204");
    let mut n = 0;
    for g in groups(&doc, |g| g["parameterSet"] == "ML-DSA-65") {
        for t in g["tests"].as_array().unwrap() {
            let xi: [u8; 32] = arr(&t["seed"]);
            let (epk, esk) = (h(&t["pk"]), h(&t["sk"]));
            for b in Backend::all() {
                let (pk, sk) = lc_dsa::keygen(b, xi);
                assert_eq!(pk.to_vec(), epk, "libcrux {b:?} pk tc {}", t["tcId"]);
                assert_eq!(sk.to_vec(), esk, "libcrux {b:?} sk tc {}", t["tcId"]);
            }
            let (pk, sk) = rc_dsa::keygen(xi);
            assert_eq!(pk.to_vec(), epk, "rustcrypto pk tc {}", t["tcId"]);
            assert_eq!(sk.to_vec(), esk, "rustcrypto sk tc {}", t["tcId"]);
            n += 1;
        }
    }
    assert!(n > 0);
    eprintln!("ML-DSA-65 keyGen KAT: {n}/{n} pass (libcrux + RustCrypto)");
}

/// sigGen: pure external interface (what Botho uses) and the internal
/// interface, both deterministic and hedged. preHash (HashML-DSA) and
/// external-mu groups are out of Botho's scope and skipped.
#[test]
fn mldsa65_siggen_kat() {
    let doc = load("ML-DSA-sigGen-FIPS204");
    let (mut ext, mut int) = (0, 0);
    for g in groups(&doc, |g| {
        g["parameterSet"] == "ML-DSA-65" && g["externalMu"] == false && g["preHash"] != "preHash"
    }) {
        let deterministic = g["deterministic"].as_bool().unwrap();
        let external = g["signatureInterface"] == "external";
        for t in g["tests"].as_array().unwrap() {
            let sk: [u8; DSA_SK] = arr(&t["sk"]);
            let msg = h(&t["message"]);
            let rnd: [u8; 32] = if deterministic {
                [0u8; 32]
            } else {
                arr(&t["rnd"])
            };
            let want = h(&t["signature"]);
            if external {
                let ctx = h(&t["context"]);
                for b in Backend::all() {
                    let s = lc_dsa::sign(b, &sk, &msg, &ctx, rnd).expect("libcrux sign");
                    assert_eq!(s.to_vec(), want, "libcrux {b:?} tc {}", t["tcId"]);
                }
                let s = if deterministic {
                    rc_dsa::sign_deterministic(&sk, &msg, &ctx).unwrap()
                } else {
                    rc_dsa::sign(&sk, &msg, &ctx, rnd).unwrap()
                };
                assert_eq!(s.to_vec(), want, "rustcrypto tc {}", t["tcId"]);
                ext += 1;
            } else {
                for b in Backend::all() {
                    let s =
                        lc_dsa::sign_internal(b, &sk, &msg, rnd).expect("libcrux sign_internal");
                    assert_eq!(s.to_vec(), want, "libcrux {b:?} internal tc {}", t["tcId"]);
                }
                assert_eq!(
                    rc_dsa::sign_internal(&sk, &msg, rnd).to_vec(),
                    want,
                    "rustcrypto internal tc {}",
                    t["tcId"]
                );
                int += 1;
            }
        }
    }
    assert!(ext > 0 && int > 0);
    eprintln!("ML-DSA-65 sigGen KAT: external/pure {ext}/{ext}, internal {int}/{int} (det + hedged; libcrux + RustCrypto)");
}

#[test]
fn mldsa65_sigver_kat() {
    let doc = load("ML-DSA-sigVer-FIPS204");
    let (mut n, mut neg) = (0, 0);
    for g in groups(&doc, |g| {
        g["parameterSet"] == "ML-DSA-65" && g["externalMu"] == false && g["preHash"] != "preHash"
    }) {
        let external = g["signatureInterface"] == "external";
        for t in g["tests"].as_array().unwrap() {
            let pk: [u8; DSA_PK] = arr(&t["pk"]);
            let msg = h(&t["message"]);
            let sig: [u8; DSA_SIG] = arr(&t["signature"]);
            let want = t["testPassed"].as_bool().unwrap();
            let reason = t["reason"].as_str().unwrap_or("");
            if external {
                let ctx = h(&t["context"]);
                for b in Backend::all() {
                    assert_eq!(
                        lc_dsa::verify(b, &pk, &msg, &ctx, &sig),
                        want,
                        "libcrux {b:?} tc {} ({reason})",
                        t["tcId"]
                    );
                }
                assert_eq!(
                    rc_dsa::verify(&pk, &msg, &ctx, &sig),
                    want,
                    "rustcrypto tc {} ({reason})",
                    t["tcId"]
                );
            } else {
                for b in Backend::all() {
                    assert_eq!(
                        lc_dsa::verify_internal(b, &pk, &msg, &sig),
                        want,
                        "libcrux {b:?} internal tc {} ({reason})",
                        t["tcId"]
                    );
                }
                assert_eq!(
                    rc_dsa::verify_internal(&pk, &msg, &sig),
                    want,
                    "rustcrypto internal tc {} ({reason})",
                    t["tcId"]
                );
            }
            if !want {
                neg += 1;
            }
            n += 1;
        }
    }
    assert!(n > 0);
    eprintln!("ML-DSA-65 sigVer KAT: {n}/{n} agree with expected ({neg} negative cases: modified sig/msg/hint/z) (libcrux + RustCrypto)");
}
