#![no_main]

//! `fuzz/fuzz_targets/fuzz_mlkem_decapsulation.rs` ported to the libcrux
//! candidate (issue #1715).
//!
//! Same input grammar (`FuzzMode`) and the same safety properties as the
//! original target (no panics on malformed ciphertexts/keys, deterministic
//! keygen, encaps/decaps round trip), driven through libcrux on every compiled
//! backend. In addition, every result is checked *differentially* against
//! Botho's current `bth-crypto-pq` (RustCrypto ml-kem 0.2.3), so the fuzzer
//! also hunts for any input where the two implementations disagree.

use arbitrary::Arbitrary;
use bth_crypto_pq::{MlKem768Ciphertext, MlKem768KeyPair, ML_KEM_768_CIPHERTEXT_BYTES};
use libcrux_eval::{botho, lc_kem, rc_kem, Backend, KEM_CT, KEM_PK, KEM_SK};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
enum FuzzMode {
    RawCiphertext(Vec<u8>),
    Decapsulate(FuzzDecapsulation),
    KeyGen(FuzzKeyGen),
    PublicKeyParse(Vec<u8>),
    /// New vs. the original target: an arbitrary (possibly invalid) 2400-byte
    /// decapsulation key. libcrux must not panic; FIPS 203 §7.3 says the key
    /// should be validated first, so record the validation verdict too.
    GarbageSecretKey(Vec<u8>, Vec<u8>),
}

#[derive(Debug, Arbitrary)]
struct FuzzDecapsulation {
    key_seed: [u8; 32],
    ciphertext: Vec<u8>,
    exact_size: bool,
}

#[derive(Debug, Arbitrary)]
struct FuzzKeyGen {
    seed: [u8; 32],
    alt_seeds: Vec<[u8; 32]>,
    m: [u8; 32],
}

fn fixed<const N: usize>(data: &[u8]) -> Option<[u8; N]> {
    data.try_into().ok()
}

/// libcrux keypair reproducing `MlKem768KeyPair::from_seed`.
fn lc_keypair(b: Backend, seed: &[u8; 32]) -> ([u8; KEM_PK], [u8; KEM_SK]) {
    lc_kem::keygen(b, botho::kem_seed_to_dz(seed))
}

fuzz_target!(|mode: FuzzMode| {
    let backends = Backend::all();
    match mode {
        FuzzMode::RawCiphertext(data) => {
            let seed = [0u8; 32];
            let bth = MlKem768KeyPair::from_seed(&seed);
            let mut cands: Vec<&[u8]> = vec![&data];
            for size in [0usize, 1, 32, 64, 128, 256, 512, 1024, 2048] {
                if size < data.len() {
                    cands.push(&data[..size]);
                }
            }
            for c in cands {
                // Size gate is ours (libcrux's API is fixed-size arrays).
                if let Some(ct) = fixed::<KEM_CT>(c) {
                    let want = bth
                        .decapsulate(&MlKem768Ciphertext::from_bytes(&ct).unwrap())
                        .unwrap();
                    for &b in &backends {
                        let (_, sk) = lc_keypair(b, &seed);
                        assert_eq!(&lc_kem::decaps(b, &sk, &ct), want.as_bytes(), "{b:?}");
                    }
                } else {
                    assert!(MlKem768Ciphertext::from_bytes(c).is_err());
                }
            }
        }
        FuzzMode::Decapsulate(d) => {
            let bth = MlKem768KeyPair::from_seed(&d.key_seed);
            if d.exact_size && d.ciphertext.len() == ML_KEM_768_CIPHERTEXT_BYTES {
                let ct: [u8; KEM_CT] = fixed(&d.ciphertext).unwrap();
                let want = bth
                    .decapsulate(&MlKem768Ciphertext::from_bytes(&ct).unwrap())
                    .unwrap();
                for &b in &backends {
                    let (pk, sk) = lc_keypair(b, &d.key_seed);
                    assert_eq!(&pk, bth.public_key().as_bytes());
                    assert_eq!(&lc_kem::decaps(b, &sk, &ct), want.as_bytes(), "{b:?}");
                }
            }
            for t in [1usize, 10, 100, KEM_CT / 2] {
                if t < d.ciphertext.len() {
                    assert!(fixed::<KEM_CT>(&d.ciphertext[..t]).is_none());
                }
            }
        }
        FuzzMode::KeyGen(k) => {
            let bth = MlKem768KeyPair::from_seed(&k.seed);
            for &b in &backends {
                let (pk1, sk1) = lc_keypair(b, &k.seed);
                let (pk2, _) = lc_keypair(b, &k.seed);
                assert_eq!(pk1, pk2, "determinism");
                assert_eq!(&pk1, bth.public_key().as_bytes());
                assert_eq!(&sk1, bth.secret_key().as_bytes());
                for alt in k.alt_seeds.iter().take(3) {
                    if alt != &k.seed {
                        assert_ne!(lc_keypair(b, alt).0, pk1, "different seeds, same key");
                    }
                }
                // Round trip, and cross-implementation round trip.
                let (ct, ss) = lc_kem::encaps(b, &pk1, k.m);
                assert_eq!(lc_kem::decaps(b, &sk1, &ct), ss);
                assert_eq!((ct, ss), rc_kem::encaps(&pk1, k.m));
                let back = bth
                    .decapsulate(&MlKem768Ciphertext::from_bytes(&ct).unwrap())
                    .unwrap();
                assert_eq!(back.as_bytes(), &ss);
            }
        }
        FuzzMode::PublicKeyParse(data) => {
            if let Some(pk) = fixed::<KEM_PK>(&data) {
                let m = [0x5Au8; 32];
                let (rct, rss) = rc_kem::encaps(&pk, m);
                for &b in &backends {
                    let valid = lc_kem::validate_pk(b, &pk);
                    let (ct, ss) = lc_kem::encaps(b, &pk, m);
                    assert_eq!(ct.len(), KEM_CT);
                    // On FIPS-valid keys the two must agree byte for byte.
                    // (On non-canonical keys they legitimately differ; see
                    // tests/cross.rs::kem_noncanonical_public_key_behaviour.)
                    if valid {
                        assert_eq!((ct, ss), (rct, rss), "{b:?}");
                    }
                }
            }
        }
        FuzzMode::GarbageSecretKey(sk, ct) => {
            if let (Some(sk), Some(ct)) = (fixed::<KEM_SK>(&sk), fixed::<KEM_CT>(&ct)) {
                let want = rc_kem::decaps(&sk, &ct);
                for &b in &backends {
                    let _valid = lc_kem::validate_sk(b, &sk);
                    // Unvalidated garbage key: must not panic. The result
                    // is a deterministic function of (sk, ct) in both
                    // implementations; check they agree.
                    assert_eq!(lc_kem::decaps(b, &sk, &ct), want, "{b:?}");
                }
            }
        }
    }
});
