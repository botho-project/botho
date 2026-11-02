//! Cross-implementation tests: libcrux vs RustCrypto, and libcrux vs Botho's
//! own `bth-crypto-pq` facade (the exact code that produced every key,
//! ciphertext and signature on the chain today).
//!
//! Iteration counts can be scaled with `LIBCRUX_EVAL_ITERS` (default 1000).

use bth_crypto_pq::{
    derive_onetime_sig_keypair, derive_pq_keys_from_seed, MlDsa65KeyPair, MlDsa65PublicKey,
    MlDsa65Signature, MlKem768Ciphertext, MlKem768KeyPair, MlKem768PublicKey,
};
use libcrux_eval::*;

fn iters() -> usize {
    std::env::var("LIBCRUX_EVAL_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000)
}

// ----------------------------------------------------------------- ML-KEM ---

/// Every existing wallet's ML-KEM key is `MlKem768KeyPair::from_seed(seed)`
/// (ChaCha20-seeded RustCrypto keygen). libcrux must reproduce it byte for
/// byte from the same seed, or every v2 address changes.
#[test]
fn kem_botho_from_seed_keys_reproduced_by_libcrux() {
    let mut r = TestRng::new(1);
    let n = iters();
    for _ in 0..n {
        let seed: [u8; 32] = r.array();
        let bth = MlKem768KeyPair::from_seed(&seed);
        for b in Backend::all() {
            let (pk, sk) = lc_kem::keygen(b, botho::kem_seed_to_dz(&seed));
            assert_eq!(&pk, bth.public_key().as_bytes(), "{b:?}");
            assert_eq!(&sk, bth.secret_key().as_bytes(), "{b:?}");
            assert!(lc_kem::validate_pk(b, &pk) && lc_kem::validate_sk(b, &sk));
        }
    }
    eprintln!("bth from_seed -> libcrux keygen: {n}/{n} byte-identical (pk 1184 B, sk 2400 B), backends {:?}", Backend::all());
}

/// The full BIP39-seed path (`derive_pq_keys_from_seed`) used for v2 addresses.
#[test]
fn kem_and_dsa_bip39_derivation_reproduced_by_libcrux() {
    let mut r = TestRng::new(2);
    let n = iters() / 4;
    for _ in 0..n {
        let seed: [u8; 64] = r.array();
        let keys = derive_pq_keys_from_seed(&seed);
        // We cannot see the HKDF-derived sub-seeds, but we can check that
        // libcrux accepts and round-trips with the derived keys.
        let kem_pk = keys.kem_public_key_bytes();
        let m: [u8; 32] = r.array();
        for b in Backend::all() {
            let (ct, ss) = lc_kem::encaps(b, kem_pk, m);
            let back = keys
                .kem_keypair
                .decapsulate(&MlKem768Ciphertext::from_bytes(&ct).unwrap())
                .unwrap();
            assert_eq!(&ss, back.as_bytes());
            let msg = r.array::<48>();
            let sig = lc_dsa::sign(
                b,
                keys.sig_keypair.secret_key().as_bytes(),
                &msg,
                b"",
                [0u8; 32],
            )
            .unwrap();
            assert_eq!(
                &sig,
                keys.sig_keypair.sign(&msg).as_bytes(),
                "{b:?} det sig"
            );
        }
    }
    eprintln!("derive_pq_keys_from_seed: {n}/{n} interoperate (libcrux encaps -> bth decaps; libcrux det sign == bth sign)");
}

/// Existing on-chain ciphertexts were produced by
/// `MlKem768PublicKey::encapsulate` (RustCrypto + OsRng). libcrux must
/// decapsulate every one of them to the same shared secret using the stored
/// 2400-byte secret key.
#[test]
fn kem_botho_ciphertexts_decapsulate_under_libcrux() {
    let mut r = TestRng::new(3);
    let n = iters();
    for _ in 0..n {
        let kp = MlKem768KeyPair::from_seed(&r.array());
        let (ct, ss) = kp.public_key().encapsulate();
        assert_eq!(ct.as_bytes().len(), 1088);
        for b in Backend::all() {
            let got = lc_kem::decaps(b, kp.secret_key().as_bytes(), ct.as_bytes());
            assert_eq!(&got, ss.as_bytes(), "{b:?}");
        }
    }
    eprintln!("bth encapsulate (OsRng) -> libcrux decaps: {n}/{n} same shared secret");
}

/// The reverse direction: libcrux encapsulates to a Botho public key, the
/// current node/wallet code decapsulates.
#[test]
fn kem_libcrux_ciphertexts_decapsulate_under_botho() {
    let mut r = TestRng::new(4);
    let n = iters();
    for _ in 0..n {
        let kp = MlKem768KeyPair::from_seed(&r.array());
        let m: [u8; 32] = r.array();
        for b in Backend::all() {
            let (ct, ss) = lc_kem::encaps(b, kp.public_key().as_bytes(), m);
            let got = kp
                .decapsulate(&MlKem768Ciphertext::from_bytes(&ct).unwrap())
                .unwrap();
            assert_eq!(got.as_bytes(), &ss, "{b:?}");
        }
    }
    eprintln!("libcrux encaps -> bth decapsulate: {n}/{n} same shared secret");
}

/// Same (ek, m) => byte-identical 1088-byte ciphertext and shared secret;
/// libcrux keys work under RustCrypto and vice versa.
#[test]
fn kem_deterministic_byte_identity_both_directions() {
    let mut r = TestRng::new(5);
    let n = iters();
    for _ in 0..n {
        let dz: [u8; 64] = r.array();
        let m: [u8; 32] = r.array();
        let (rpk, rsk) = rc_kem::keygen(dz);
        let (rct, rss) = rc_kem::encaps(&rpk, m);
        for b in Backend::all() {
            let (lpk, lsk) = lc_kem::keygen(b, dz);
            assert_eq!((lpk, lsk), (rpk, rsk));
            let (lct, lss) = lc_kem::encaps(b, &lpk, m);
            assert_eq!(lct, rct);
            assert_eq!(lss, rss);
            // cross decaps
            assert_eq!(rc_kem::decaps(&lsk, &lct), lss);
            assert_eq!(lc_kem::decaps(b, &rsk, &rct), rss);
        }
    }
    eprintln!("deterministic keygen/encaps: {n}/{n} byte-identical ek/dk/ct/ss and cross-decaps both ways");
}

/// Implicit rejection: for malformed ciphertexts both implementations must
/// return the same pseudo-random key J(z || c), never an error or panic.
#[test]
fn kem_implicit_rejection_agrees() {
    let mut r = TestRng::new(6);
    let n = iters();
    let (mut garbage, mut flips) = (0, 0);
    for i in 0..n {
        let (pk, sk) = rc_kem::keygen(r.array());
        let ct: [u8; KEM_CT] = if i % 2 == 0 {
            garbage += 1;
            r.array()
        } else {
            flips += 1;
            let (mut ct, _) = rc_kem::encaps(&pk, r.array());
            let bit = (r.next_u64() as usize) % (KEM_CT * 8);
            ct[bit / 8] ^= 1 << (bit % 8);
            ct
        };
        let want = rc_kem::decaps(&sk, &ct);
        for b in Backend::all() {
            assert_eq!(lc_kem::decaps(b, &sk, &ct), want, "{b:?}");
        }
    }
    eprintln!("implicit rejection: {n}/{n} identical rejection keys ({garbage} random ciphertexts, {flips} single-bit flips)");
}

/// FIPS 203 §7.2 modulus check. A Botho v2 address carries a 1184-byte
/// ML-KEM key that today is accepted unchecked. Record how each library
/// behaves on a non-canonical key (coefficient >= q).
#[test]
fn kem_noncanonical_public_key_behaviour() {
    let mut r = TestRng::new(7);
    let (mut pk, _) = rc_kem::keygen(r.array());
    // First 12-bit coefficient := 0xFFF (4095 >= q = 3329).
    pk[0] = 0xFF;
    pk[1] |= 0x0F;
    let bth_accepts = MlKem768PublicKey::from_bytes(&pk).is_ok();
    let m = [9u8; 32];
    let (rct, rss) = rc_kem::encaps(&pk, m);
    for b in Backend::all() {
        assert!(!lc_kem::validate_pk(b, &pk), "{b:?} must reject");
        let (lct, lss) = lc_kem::encaps(b, &pk, m);
        eprintln!(
            "non-canonical ek: bth from_bytes accepts={bth_accepts}; libcrux {b:?} validate=false; \
             unchecked encaps ct identical to RustCrypto={} ss identical={}",
            lct == rct,
            lss == rss
        );
    }
}

// ----------------------------------------------------------------- ML-DSA ---

/// Every ML-DSA key in Botho is `MlDsa65KeyPair::from_seed(xi)` (incl. the
/// per-output one-time keys from `derive_onetime_sig_keypair`). libcrux must
/// reproduce pk (1952 B) and the expanded sk (4032 B) from the same xi.
#[test]
fn dsa_botho_from_seed_keys_reproduced_by_libcrux() {
    let mut r = TestRng::new(8);
    let n = iters();
    for i in 0..n {
        let xi: [u8; 32] = r.array();
        let bth = if i % 2 == 0 {
            MlDsa65KeyPair::from_seed(&xi)
        } else {
            derive_onetime_sig_keypair(&xi, i as u32)
        };
        let xi_used = if i % 2 == 0 { Some(xi) } else { None };
        if let Some(xi) = xi_used {
            for b in Backend::all() {
                let (pk, sk) = lc_dsa::keygen(b, xi);
                assert_eq!(&pk, bth.public_key().as_bytes(), "{b:?}");
                assert_eq!(&sk, bth.secret_key().as_bytes(), "{b:?}");
            }
        }
        // For one-time keys (seed is internal) check signing interop instead.
        let msg = r.array::<64>();
        let bsig = bth.sign(&msg);
        for b in Backend::all() {
            let lsig = lc_dsa::sign(b, bth.secret_key().as_bytes(), &msg, b"", [0u8; 32]).unwrap();
            assert_eq!(
                &lsig,
                bsig.as_bytes(),
                "{b:?} deterministic signature bytes"
            );
            assert!(lc_dsa::verify(
                b,
                bth.public_key().as_bytes(),
                &msg,
                b"",
                bsig.as_bytes()
            ));
        }
    }
    eprintln!("bth ML-DSA from_seed/onetime -> libcrux: {n}/{n} keys byte-identical (where seed visible) and deterministic sigs byte-identical (3309 B)");
}

/// Hedged libcrux signatures verify under Botho; Botho signatures verify under
/// libcrux; tampering is rejected identically.
#[test]
fn dsa_cross_verify_and_tamper() {
    let mut r = TestRng::new(9);
    let n = iters();
    let mut rejected = 0;
    for _ in 0..n {
        let kp = MlDsa65KeyPair::from_seed(&r.array());
        let bpk = MlDsa65PublicKey::from_bytes(kp.public_key().as_bytes()).unwrap();
        let msg = r.array::<100>();
        for b in Backend::all() {
            let hedged = lc_dsa::sign(b, kp.secret_key().as_bytes(), &msg, b"", r.array()).unwrap();
            assert!(bpk
                .verify(&msg, &MlDsa65Signature::from_bytes(&hedged).unwrap())
                .is_ok());
            // tamper: single bit flip anywhere in the signature
            let mut bad = hedged;
            let bit = (r.next_u64() as usize) % (DSA_SIG * 8);
            bad[bit / 8] ^= 1 << (bit % 8);
            let lc_ok = lc_dsa::verify(b, kp.public_key().as_bytes(), &msg, b"", &bad);
            let rc_ok = bpk
                .verify(&msg, &MlDsa65Signature::from_bytes(&bad).unwrap())
                .is_ok();
            assert_eq!(lc_ok, rc_ok, "{b:?} tamper verdicts disagree");
            assert!(!lc_ok);
            rejected += 1;
            // wrong message
            let mut m2 = msg;
            m2[0] ^= 1;
            assert!(!lc_dsa::verify(
                b,
                kp.public_key().as_bytes(),
                &m2,
                b"",
                &hedged
            ));
        }
        let bsig = kp.sign(&msg);
        for b in Backend::all() {
            assert!(lc_dsa::verify(
                b,
                kp.public_key().as_bytes(),
                &msg,
                b"",
                bsig.as_bytes()
            ));
        }
    }
    eprintln!("ML-DSA cross-verify: {n}/{n} both directions; {rejected} tampered signatures rejected by both");
}

/// Random 3309-byte blobs: both verifiers must reject without panicking, and
/// agree.
#[test]
fn dsa_garbage_signatures_rejected_identically() {
    let mut r = TestRng::new(10);
    let n = iters();
    let kp = MlDsa65KeyPair::from_seed(&[1u8; 32]);
    for _ in 0..n {
        let sig: [u8; DSA_SIG] = r.array();
        let rc = rc_dsa::verify(kp.public_key().as_bytes(), b"m", b"", &sig);
        for b in Backend::all() {
            assert_eq!(
                lc_dsa::verify(b, kp.public_key().as_bytes(), b"m", b"", &sig),
                rc
            );
        }
        assert!(!rc);
    }
    eprintln!("ML-DSA garbage signatures: {n}/{n} rejected by both, no panics");
}
