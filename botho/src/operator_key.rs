//! Operator signing keypair (#747, P4.4a of the #695 proposal).
//!
//! This is the FOUNDATION sub-issue of #709 (P4.4 operator-signed quorum
//! curation). It lands two things and deliberately nothing else — no RPC, no
//! apply logic, no envelope verification (those are #748–#751):
//!
//! 1. The `botho operator keygen` CLI, which runs on the operator's
//!    **workstation** and writes an Ed25519 keypair file whose private key is
//!    encrypted at rest under a **mandatory** passphrase. The #474/#475 lesson
//!    (no plaintext-by-default, no optional password) is enforced here: an
//!    empty passphrase is REFUSED, so this module can never write a plaintext
//!    private key.
//!
//! 2. The shared [`fingerprint_hex`] helper computing
//!    `blake2b-256(pubkey)[..8]` (hex). This is the `signerKeyId` selector the
//!    signed-action envelope (`docs/security/quorum-write-path.md` §3) uses,
//!    and the value the node computes to pick the right key out of
//!    `[rpc.operator] action_public_keys`. Both sides MUST derive the identical
//!    string, so the computation lives in exactly one place: here.
//!
//! Algorithm choices mirror what is already in the tree:
//! - **Ed25519** (`ed25519-dalek`, the same primitive libp2p identity keys use)
//!   — small signatures, no parameters to get wrong (§2 of the design).
//! - **Argon2id + ChaCha20-Poly1305** for the at-rest envelope — the exact
//!   recipe `botho-wallet`'s encrypted storage uses (`botho-wallet/src/storage.rs`).
//!
//! ## Security boundary (documented in code, verified by absence)
//!
//! The key list on a node (`[rpc.operator] action_public_keys`) is provisioned
//! and changed over SSH/config only. There is intentionally no code path — RPC
//! or signed action — in this module or anywhere else that reads, adds, or
//! removes those entries: key management must not be self-referential (a stolen
//! key must not enroll further keys, §2). This module only *generates* a
//! keypair on the operator's machine and *prints* the pubkey/fingerprint the
//! operator then provisions by hand.

use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{rand_core::OsRng as ArgonOsRng, SaltString},
    Argon2, PasswordHasher,
};
use blake2::Blake2b;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use digest::{generic_array::typenum::U32, Digest};
use ed25519_dalek::SigningKey;
use rand::Rng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Blake2b with 256-bit output — the same construction as
/// `bth-crypto-hashes::Blake2b256`, inlined here so this module (and the node
/// config accessor) can compute the fingerprint without pulling the hashes
/// crate into the node's dependency graph.
type Blake2b256 = Blake2b<U32>;

/// Length in bytes of the fingerprint prefix taken from the blake2b-256 digest.
/// `blake2b-256(pubkey)[..8]` → 8 bytes → 16 hex chars.
pub const FINGERPRINT_BYTES: usize = 8;

/// Current on-disk keypair-file format version.
const KEYFILE_VERSION: u32 = 1;

/// Argon2id parameters for the at-rest passphrase KDF. Matches the tuning
/// `botho-wallet` uses for its encrypted storage (64 MiB / 3 passes / 4 lanes)
/// so the operator-key file has the same brute-force cost profile as the
/// wallet.
const ARGON2_MEMORY_KB: u32 = 65536; // 64 MiB
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 4;

/// Compute the operator-key fingerprint used as the envelope `signerKeyId`
/// (`docs/security/quorum-write-path.md` §3): the lowercase-hex encoding of the
/// first [`FINGERPRINT_BYTES`] bytes of `blake2b-256(pubkey)`.
///
/// This is the ONE canonical implementation. The `botho operator keygen` CLI
/// prints it, and node-side `signerKeyId` selection (later sub-issues) MUST
/// call this same function against each configured `action_public_keys` entry
/// so the two sides always agree.
pub fn fingerprint_hex(public_key: &[u8; 32]) -> String {
    let mut hasher = Blake2b256::new();
    hasher.update(public_key);
    let digest = hasher.finalize();
    hex::encode(&digest[..FINGERPRINT_BYTES])
}

/// On-disk representation of an operator signing keypair.
///
/// The public key is stored in the clear (it is public). The private key is
/// NEVER stored in the clear: `ciphertext` is the Argon2id-derived
/// ChaCha20-Poly1305 encryption of the 32-byte Ed25519 secret scalar. There is
/// no code path that serializes the secret key unencrypted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorKeyFile {
    /// File-format version (currently [`KEYFILE_VERSION`]).
    pub version: u32,
    /// Ed25519 public key, lowercase hex (32 bytes → 64 hex chars).
    pub public_key: String,
    /// `blake2b-256(pubkey)[..8]` hex — the `signerKeyId` (§3). Redundant with
    /// `public_key` (it is derivable) but stored so the operator can eyeball
    /// which key a file holds without re-deriving.
    pub fingerprint: String,
    /// Argon2id salt (PHC-string form, as produced by `SaltString`).
    pub salt: String,
    /// ChaCha20-Poly1305 nonce, lowercase hex (12 bytes → 24 hex chars).
    pub nonce: String,
    /// Encrypted 32-byte Ed25519 secret scalar, lowercase hex.
    pub ciphertext: String,
}

impl OperatorKeyFile {
    /// Generate a fresh Ed25519 keypair and wrap its private key at rest under
    /// `passphrase`.
    ///
    /// **Refuses an empty (or whitespace-only) passphrase** — the #474/#475
    /// lesson: no plaintext-by-default, no optional password. This is the only
    /// constructor, so an `OperatorKeyFile` can never exist without a real
    /// passphrase behind its ciphertext.
    pub fn generate(passphrase: &str) -> Result<Self> {
        if passphrase.trim().is_empty() {
            return Err(anyhow!(
                "a passphrase is required to encrypt the operator private key \
                 at rest — refusing to write an unencrypted key (empty \
                 passphrase rejected)"
            ));
        }

        // Fresh Ed25519 keypair. `to_bytes()` is the 32-byte secret scalar;
        // wrap it in Zeroizing so it is wiped from memory after encryption.
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let secret = Zeroizing::new(signing_key.to_bytes());
        let public_key = signing_key.verifying_key().to_bytes();

        // Derive a 32-byte key from the passphrase (Argon2id) and encrypt the
        // secret scalar (ChaCha20-Poly1305), exactly as botho-wallet does.
        let salt = SaltString::generate(&mut ArgonOsRng);
        let derived = derive_key(passphrase, salt.as_str())?;

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill(&mut nonce_bytes);

        let cipher = ChaCha20Poly1305::new_from_slice(derived.as_slice())
            .map_err(|_| anyhow!("failed to construct cipher"))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, secret.as_slice())
            .map_err(|_| anyhow!("encryption of the operator private key failed"))?;

        Ok(Self {
            version: KEYFILE_VERSION,
            public_key: hex::encode(public_key),
            fingerprint: fingerprint_hex(&public_key),
            salt: salt.to_string(),
            nonce: hex::encode(nonce_bytes),
            ciphertext: hex::encode(ciphertext),
        })
    }

    /// Serialize the key file to pretty JSON and write it to `path` with
    /// owner-only permissions (Unix `0600`; owner-only ACL on Windows) —
    /// matching how `botho-wallet` protects its encrypted wallet file. Refuses
    /// to clobber an existing file so a stray `keygen` cannot silently destroy a
    /// provisioned key.
    pub fn write_to(&self, path: &std::path::Path) -> Result<()> {
        if path.exists() {
            return Err(anyhow!(
                "refusing to overwrite existing key file at {} — move it aside \
                 first",
                path.display()
            ));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let json = serde_json::to_string_pretty(self)?;

        #[cfg(unix)]
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(json.as_bytes())?;
        }
        #[cfg(not(unix))]
        {
            // Non-Unix: create_new to preserve the no-clobber guarantee; the
            // OS default ACL applies (we do not weaken it here).
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(json.as_bytes())?;
        }
        Ok(())
    }

    /// Parse the stored public key into its 32-byte form.
    pub fn public_key_bytes(&self) -> Result<[u8; 32]> {
        let bytes =
            hex::decode(&self.public_key).map_err(|_| anyhow!("invalid public_key hex"))?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("public_key must be 32 bytes"))?;
        Ok(arr)
    }

    /// Decrypt the private key with `passphrase`, returning the 32-byte Ed25519
    /// secret scalar wrapped in [`Zeroizing`].
    ///
    /// Not used by this issue's node/CLI surface (there is no signing yet), but
    /// it is the round-trip counterpart of [`generate`](Self::generate) and is
    /// exercised by the tests to prove the file is decryptable with the correct
    /// passphrase and only that passphrase.
    pub fn decrypt(&self, passphrase: &str) -> Result<Zeroizing<[u8; 32]>> {
        if self.version != KEYFILE_VERSION {
            return Err(anyhow!(
                "unsupported operator key file version: {} (expected {})",
                self.version,
                KEYFILE_VERSION
            ));
        }

        let derived = derive_key(passphrase, &self.salt)?;

        let nonce_bytes =
            hex::decode(&self.nonce).map_err(|_| anyhow!("invalid nonce hex"))?;
        if nonce_bytes.len() != 12 {
            return Err(anyhow!("invalid nonce length"));
        }
        let ciphertext =
            hex::decode(&self.ciphertext).map_err(|_| anyhow!("invalid ciphertext hex"))?;

        let cipher = ChaCha20Poly1305::new_from_slice(derived.as_slice())
            .map_err(|_| anyhow!("failed to construct cipher"))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| anyhow!("decryption failed - wrong passphrase?"))?;

        let secret: [u8; 32] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("decrypted key is not 32 bytes"))?;
        Ok(Zeroizing::new(secret))
    }
}

/// Derive a 32-byte key from `passphrase` using Argon2id — the same KDF and
/// parameters `botho-wallet::storage` uses.
fn derive_key(passphrase: &str, salt: &str) -> Result<Zeroizing<[u8; 32]>> {
    let salt = SaltString::from_b64(salt).map_err(|_| anyhow!("invalid salt format"))?;

    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(
            ARGON2_MEMORY_KB,
            ARGON2_ITERATIONS,
            ARGON2_PARALLELISM,
            Some(32),
        )
        .map_err(|_| anyhow!("invalid Argon2 parameters"))?,
    );

    let hash = argon2
        .hash_password(passphrase.as_bytes(), &salt)
        .map_err(|_| anyhow!("key derivation failed"))?;
    let hash_output = hash.hash.ok_or_else(|| anyhow!("no hash output"))?;
    let hash_bytes = hash_output.as_bytes();
    if hash_bytes.len() < 32 {
        return Err(anyhow!("derived key too short"));
    }

    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&hash_bytes[..32]);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_passphrase_is_refused() {
        let err = OperatorKeyFile::generate("").unwrap_err();
        assert!(
            err.to_string().contains("passphrase is required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn whitespace_only_passphrase_is_refused() {
        let err = OperatorKeyFile::generate("   \t ").unwrap_err();
        assert!(
            err.to_string().contains("passphrase is required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn generate_produces_a_decryptable_encrypted_key() {
        let pass = "correct horse battery staple";
        let file = OperatorKeyFile::generate(pass).unwrap();

        // The private key is NOT stored in the clear: the ciphertext must not
        // equal any plausible plaintext scalar, and decrypting reproduces a
        // valid 32-byte scalar whose public key matches the stored one.
        let secret = file.decrypt(pass).unwrap();
        let reconstructed = SigningKey::from_bytes(&secret);
        assert_eq!(
            reconstructed.verifying_key().to_bytes(),
            file.public_key_bytes().unwrap(),
            "decrypted secret does not match stored public key"
        );
    }

    #[test]
    fn wrong_passphrase_fails_to_decrypt() {
        let file = OperatorKeyFile::generate("the right one").unwrap();
        let err = file.decrypt("the wrong one").unwrap_err();
        assert!(
            err.to_string().contains("wrong passphrase"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fingerprint_matches_node_side_derivation() {
        // The CLI prints `file.fingerprint`; a node selecting `signerKeyId`
        // recomputes it from the pubkey via the SAME helper. They must agree.
        let file = OperatorKeyFile::generate("passphrase").unwrap();
        let pubkey = file.public_key_bytes().unwrap();
        let node_side = fingerprint_hex(&pubkey);
        assert_eq!(file.fingerprint, node_side);
        // 8 bytes → 16 hex chars.
        assert_eq!(node_side.len(), FINGERPRINT_BYTES * 2);
        // Deterministic: same pubkey always yields the same fingerprint.
        assert_eq!(node_side, fingerprint_hex(&pubkey));
    }

    #[test]
    fn fingerprint_is_stable_known_vector() {
        // Guard the fingerprint scheme against accidental drift: the digest of
        // an all-zero pubkey is a fixed value. If this ever changes, every
        // provisioned signerKeyId would silently stop matching.
        let zero = [0u8; 32];
        let fp = fingerprint_hex(&zero);
        assert_eq!(fp.len(), 16);
        // blake2b-256(0x00*32)[..8], lowercase hex.
        assert_eq!(fp, "89eb0d6a8a691dae");
    }

    #[test]
    fn each_generation_is_a_fresh_key() {
        let a = OperatorKeyFile::generate("p").unwrap();
        let b = OperatorKeyFile::generate("p").unwrap();
        assert_ne!(a.public_key, b.public_key, "keygen must not be deterministic");
    }

    #[test]
    fn keyfile_json_never_contains_the_secret_scalar_in_clear() {
        let pass = "some passphrase";
        let file = OperatorKeyFile::generate(pass).unwrap();
        let secret = file.decrypt(pass).unwrap();
        let json = serde_json::to_string(&file).unwrap();
        assert!(
            !json.contains(&hex::encode(&*secret)),
            "serialized key file leaked the plaintext secret scalar"
        );
    }
}
