// Copyright (c) 2024 The Botho Foundation

//! Golden determinism vectors for `bth-crypto-secp256k1`.
//!
//! ECDSA signing here is RFC 6979 deterministic: a fixed private key and a
//! fixed message hash always produce the same 65-byte `r || s || v` output.
//! That makes the exact bytes a checkable invariant across dependency
//! upgrades, which "the unit tests still pass" alone does not give us —
//! `test_sign_and_recover` in `src/lib.rs` is self-consistent (sign, then
//! recover with the same library), so it would stay green even if the
//! produced signature bytes changed.
//!
//! Every vector below was captured under **k256 0.13.4** (pre-migration) and
//! re-verified byte-for-byte under **k256 0.14** (see #1202). Do not
//! regenerate these values to make a failing test pass: a diff here means the
//! signing, key-derivation, or SEC1 encoding behavior actually changed, which
//! for a wallet/bridge key crate is a breaking change requiring explicit
//! sign-off.

use bth_crypto_secp256k1::{recover_address, recover_public_key, Secp256k1Keypair};
use sha3::{Digest, Keccak256};

/// Standard BIP-39 test mnemonic (DO NOT USE IN PRODUCTION).
const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// Fixed raw private key (test-only, never used in production).
const FIXED_KEY: [u8; 32] = [
    0x4c, 0x0f, 0x9c, 0x1b, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
    0xcc, 0xdd, 0xee, 0xff, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
];

/// Fixed 32-byte prehash to sign.
const FIXED_HASH: [u8; 32] = [
    0xde, 0xad, 0xbe, 0xef, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
    0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
];

const FIXED_MESSAGE: &[u8] = b"Hello, Ethereum!";

fn eip191_hash(message: &[u8]) -> [u8; 32] {
    let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
    let mut hasher = Keccak256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(message);
    hasher.finalize().into()
}

/// Raw-private-key path: SEC1 public-key encodings and both signing entry
/// points.
#[test]
fn golden_fixed_key_vectors() {
    let kp = Secp256k1Keypair::from_bytes(&FIXED_KEY).unwrap();

    assert_eq!(
        kp.eth_address(),
        "0x6046B8036964cDD4Bf2116ce18E4AB105083E6dE"
    );
    assert_eq!(
        hex::encode(kp.public_key_uncompressed()),
        "044cfb8b825672ac0a0b846dee69a6e3aff6f68031f10d97542b81bf83b7ae6521de9a86c9c88cd5019cd8be7b8b46e559dac39c6c08101f58d755e96b7260a8de"
    );
    assert_eq!(
        hex::encode(kp.public_key_compressed()),
        "024cfb8b825672ac0a0b846dee69a6e3aff6f68031f10d97542b81bf83b7ae6521"
    );
    assert_eq!(
        hex::encode(kp.sign_hash(&FIXED_HASH)),
        "1a87503a6e15f1d5f0272bb7b6f186857f753527f965e50de8238e459d5ec95f3cc4f5ca475f0ae6efbc55145a7106b7dc76a6b715e62dab60acbeabd6e2803b1c"
    );
    assert_eq!(
        hex::encode(kp.sign_message(FIXED_MESSAGE)),
        "8a95fb31500dad19baa0ff214189188285ed64db738ec3e044a989842cf9e9a30645e667c809c2491ea69e5eaf0a1860fc2401da60df419ef18cbce35334e0331c"
    );

    // sign_transaction_hash is an alias for sign_hash; pin that too.
    assert_eq!(
        kp.sign_transaction_hash(&FIXED_HASH),
        kp.sign_hash(&FIXED_HASH)
    );
}

/// BIP-32/39/44 derivation path (m/44'/60'/0'/0/0) plus signing.
#[test]
fn golden_mnemonic_vectors() {
    let kp = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();

    assert_eq!(
        kp.eth_address(),
        "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
    );
    assert_eq!(
        hex::encode(kp.public_key_uncompressed()),
        "0437b0bb7a8288d38ed49a524b5dc98cff3eb5ca824c9f9dc0dfdb3d9cd600f299a6179912b7451c09896c4098eca7ce6b2e58330672795e847c4d6af44e024230"
    );
    assert_eq!(
        hex::encode(kp.public_key_compressed()),
        "0237b0bb7a8288d38ed49a524b5dc98cff3eb5ca824c9f9dc0dfdb3d9cd600f299"
    );
    assert_eq!(
        hex::encode(kp.sign_hash(&FIXED_HASH)),
        "61516203835c890232227b66fc3d4da372fe03630546fde64b8f7f70b30d1c42440166dab99e5db3975c2a3a0e06429659b90b3f980f46b0d25fb70eb83d7af61b"
    );
    assert_eq!(
        hex::encode(kp.sign_message(FIXED_MESSAGE)),
        "e5cdfe97b07c25dad10ffd9167ea3a6daccb2919d1ac9ddf0cfedba2d7e3667b70e0445fc5f1c82405769ceb4846d45cdae6cbc5041dcc7f6dd08c59e38a53c51c"
    );
}

/// Non-zero account indices and a BIP-39 passphrase exercise the non-hardened
/// derivation branch (compressed-pubkey HMAC input) and the mod-n key addition.
#[test]
fn golden_derivation_index_and_passphrase_vectors() {
    let kp3 = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 3).unwrap();
    assert_eq!(
        kp3.eth_address(),
        "0xF3f50213C1d2e255e4B2bAD430F8A38EEF8D718E"
    );
    assert_eq!(
        hex::encode(kp3.sign_hash(&FIXED_HASH)),
        "625eab00306371d0174dd0014d2776ebf955a8fd8b41b04c64282fd551ef475579ea99e67cdb40472b4d8f1c6e8f0b2e23b561360eb934ddffd349fe1a207e8a1b"
    );

    let kp7 = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "botho", 7).unwrap();
    assert_eq!(
        kp7.eth_address(),
        "0x887AEEeedc7C4BA3EeB04372b5A06983C1934d4e"
    );
    assert_eq!(
        hex::encode(kp7.sign_hash(&FIXED_HASH)),
        "af4b29ec535c1036c11ddf97999048fb9cdb358281da99b567b4104db3486538007717b5e915842e8a83b1cb7df03d0704a043b277fd50ce2cd85c58d78b042b1c"
    );
}

/// Public-key / address recovery from the pinned signature above. A wrong
/// recovery-id mapping or corrupted r||s encoding would break these.
#[test]
fn golden_recovery_vectors() {
    let kp = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();
    let signature = kp.sign_message(FIXED_MESSAGE);

    let pubkey = recover_public_key(&eip191_hash(FIXED_MESSAGE), &signature).unwrap();
    assert_eq!(
        hex::encode(pubkey),
        "0437b0bb7a8288d38ed49a524b5dc98cff3eb5ca824c9f9dc0dfdb3d9cd600f299a6179912b7451c09896c4098eca7ce6b2e58330672795e847c4d6af44e024230"
    );

    let addr = recover_address(FIXED_MESSAGE, &signature).unwrap();
    assert_eq!(
        hex::encode(addr),
        "9858effd232b4033e47d90003d41ec34ecaeda94"
    );
    assert_eq!(addr, kp.eth_address_bytes());

    // v is encoded as recovery_id + 27, so only 27/28 are valid here.
    assert!(signature[64] == 27 || signature[64] == 28);
}
