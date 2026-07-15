// Copyright (c) 2024 The Botho Foundation

//! Reserve wallet key material for the live BTH transports (#856, #972).
//!
//! Both the deposit watcher (view-key scanning outputs paid to the bridge
//! deposit/reserve address) and the release path (spending reserve-owned
//! outputs) need the reserve wallet's Ristretto **private** keys. The bridge
//! loads them from `BthConfig::view_key_file` / `spend_key_file`, each a file
//! holding a single hex-encoded 32-byte scalar (the same on-disk shape the
//! node's key files use). The reserved account is reconstructed with the
//! node-identical [`AccountKey`] so ownership detection and one-time-key
//! recovery cannot drift from the chain.
//!
//! # Post-quantum reserve key (issue #972)
//!
//! On the universal-ML-KEM chain (protocol 6.0.0, #958) every output carries an
//! ML-KEM ciphertext, and a hybrid deposit paid to the reserve can only be
//! detected by ML-KEM decapsulation with the reserve's secret. When
//! `BthConfig::pq_seed_file` is configured (a single hex-encoded 64-byte BIP39
//! seed), the reserve derives its ML-KEM-768 + ML-DSA-65 keypairs from that
//! seed using the SAME `derive_pq_keys_from_seed` a normal wallet uses, so its
//! published v2 (`botho://2/…`) address is self-consistent: the ML-KEM public
//! key it advertises matches the secret the scanner decapsulates with. The
//! ML-DSA public key is bundled into the address (v2 carries both) even though
//! reserve spends remain CLSAG (the ML-DSA secret is unused for spending).
//!
//! Absent key files disable the relevant transport (watch-only): the deposit
//! watcher stays idle and release submission is disabled, exactly the
//! fail-safe posture the stubs had. An absent `pq_seed_file` leaves a
//! classical-only (v1) reserve: hybrid deposits are warned about, not detected.

use bth_account_keys::AccountKey;
use bth_address_codec::{encode_address, Network};
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_pq::{derive_pq_keys_from_seed, MlKem768KeyPair, BIP39_SEED_SIZE};

/// Reserve wallet keys reconstructed from the configured key files.
#[derive(Clone)]
pub struct ReserveKeys {
    account: AccountKey,
    /// Present when a reserve PQ seed file is configured: the ML-KEM-768
    /// keypair the scanner decapsulates hybrid deposits with, alongside the
    /// account-wide ML-KEM + ML-DSA public keys bundled into the reserve's
    /// published v2 address. `None` == classical-only (v1) reserve.
    pq: Option<ReservePqKeys>,
}

/// Post-quantum reserve material derived from the reserve PQ seed.
#[derive(Clone)]
struct ReservePqKeys {
    /// The ML-KEM-768 keypair used to decapsulate hybrid deposits (the scanner
    /// needs the secret; the public key is what senders encapsulate to).
    kem_keypair: MlKem768KeyPair,
    /// Raw ML-KEM-768 public key bytes (published in the v2 address).
    kem_public_key: Vec<u8>,
    /// Raw ML-DSA-65 public key bytes (published in the v2 address). The DSA
    /// secret is not retained: reserve spends are CLSAG, so it is never used.
    dsa_public_key: Vec<u8>,
}

impl ReserveKeys {
    /// Load from the hex spend/view private-key files and an optional PQ seed
    /// file. Returns `Ok(None)` (transport disabled, not an error) when either
    /// classical key path is absent — watch-only deployments run without
    /// reserve keys.
    ///
    /// When `pq_seed_file` is present the reserve additionally derives its
    /// ML-KEM-768 + ML-DSA-65 keypairs from the seed (issue #972), enabling
    /// hybrid-deposit detection and a v2 published address. When absent the
    /// reserve stays classical-only (hybrid deposits warned, not detected).
    pub fn load(
        view_key_file: Option<&str>,
        spend_key_file: Option<&str>,
        pq_seed_file: Option<&str>,
    ) -> Result<Option<Self>, String> {
        let (Some(view_path), Some(spend_path)) = (view_key_file, spend_key_file) else {
            return Ok(None);
        };
        let view_private = read_hex_scalar(view_path, "bth.view_key_file")?;
        let spend_private = read_hex_scalar(spend_path, "bth.spend_key_file")?;

        let pq = match pq_seed_file {
            Some(seed_path) => Some(load_pq_keys(seed_path)?),
            None => None,
        };

        Ok(Some(Self {
            account: AccountKey::new(&spend_private, &view_private),
            pq,
        }))
    }

    /// The reserve account (view + spend private keys).
    pub fn account(&self) -> &AccountKey {
        &self.account
    }

    /// The reserve's ML-KEM-768 keypair, when a PQ seed file is configured.
    ///
    /// This is the secret the deposit scanner / release path pass into
    /// [`crate::bth_scan::scan_deposit_output`] /
    /// [`crate::bth_scan::build_release_tx`] to detect and spend hybrid
    /// deposits. `None` for a classical-only reserve (hybrid deposits are
    /// then warned about, not detected).
    pub fn kem_keypair(&self) -> Option<&MlKem768KeyPair> {
        self.pq.as_ref().map(|pq| &pq.kem_keypair)
    }

    /// Whether this reserve holds a post-quantum key (can detect hybrid
    /// deposits and publish a v2 address).
    pub fn has_pq_keys(&self) -> bool {
        self.pq.is_some()
    }

    /// The reserve's published receive address: a v2 [`PublicAddress`] carrying
    /// the reserve's ML-KEM + ML-DSA public keys when a PQ seed is configured,
    /// else the classical (v1) default subaddress. Senders encapsulate to the
    /// ML-KEM key in this address; the scanner decapsulates with the matching
    /// secret, so the pair is self-consistent by construction.
    pub fn public_address(&self) -> bth_account_keys::PublicAddress {
        let base = self.account.default_subaddress();
        match &self.pq {
            Some(pq) => base.with_pq_keys(pq.kem_public_key.clone(), pq.dsa_public_key.clone()),
            None => base,
        }
    }

    /// The reserve's published address as a `botho://2/…` (or `tbotho://2/…`)
    /// URI. Errors for a classical-only reserve (v2 requires both PQ keys).
    pub fn public_address_uri(&self, network: Network) -> Result<String, String> {
        encode_address(&self.public_address(), network)
            .map_err(|e| format!("reserve address is not v2-encodable: {e}"))
    }
}

/// Load the reserve PQ keypairs from a hex-encoded 64-byte BIP39 seed file,
/// deriving them with the same `derive_pq_keys_from_seed` a normal wallet uses
/// so the published v2 address is self-consistent (issue #972).
fn load_pq_keys(path: &str) -> Result<ReservePqKeys, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("bth.pq_seed_file: cannot read {path}: {e}"))?;
    let bytes = hex::decode(raw.trim())
        .map_err(|e| format!("bth.pq_seed_file: invalid hex in {path}: {e}"))?;
    let seed: [u8; BIP39_SEED_SIZE] = bytes.try_into().map_err(|_| {
        format!("bth.pq_seed_file: {path} must decode to exactly {BIP39_SEED_SIZE} bytes")
    })?;
    let material = derive_pq_keys_from_seed(&seed);
    let kem_public_key = material.kem_keypair.public_key().as_bytes().to_vec();
    let dsa_public_key = material.sig_keypair.public_key().as_bytes().to_vec();
    Ok(ReservePqKeys {
        kem_keypair: material.kem_keypair,
        kem_public_key,
        dsa_public_key,
    })
}

/// Read a file containing a single hex-encoded 32-byte scalar into a
/// [`RistrettoPrivate`]. Trims surrounding whitespace/newlines.
fn read_hex_scalar(path: &str, label: &str) -> Result<RistrettoPrivate, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("{label}: cannot read {path}: {e}"))?;
    let trimmed = raw.trim();
    let bytes = hex::decode(trimmed).map_err(|e| format!("{label}: invalid hex in {path}: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{label}: {path} must decode to exactly 32 bytes"))?;
    RistrettoPrivate::try_from(&arr)
        .map_err(|e| format!("{label}: {path} is not a valid scalar: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a hex-encoded classical scalar key file (with a trailing newline
    /// an editor/`echo` would leave, which loading must tolerate).
    fn write_key(path: &std::path::Path, bytes: &[u8]) {
        writeln!(
            std::fs::File::create(path).unwrap(),
            "{}",
            hex::encode(bytes)
        )
        .unwrap();
    }

    #[test]
    fn load_none_when_key_files_absent() {
        assert!(ReserveKeys::load(None, None, None).unwrap().is_none());
        assert!(ReserveKeys::load(Some("v"), None, None).unwrap().is_none());
        assert!(ReserveKeys::load(None, Some("s"), None).unwrap().is_none());
    }

    #[test]
    fn load_reconstructs_account_from_hex_files() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountKey::random(&mut rand::rngs::OsRng);

        let view_path = dir.path().join("view.hex");
        let spend_path = dir.path().join("spend.hex");
        write_key(&view_path, &account.view_private_key().to_bytes());
        write_key(&spend_path, &account.spend_private_key().to_bytes());

        let loaded = ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
            None,
        )
        .unwrap()
        .expect("both key files present");

        // Same address the reserve advertises.
        assert_eq!(
            loaded
                .account()
                .default_subaddress()
                .spend_public_key()
                .to_bytes(),
            account.default_subaddress().spend_public_key().to_bytes()
        );
        // No PQ seed => classical-only reserve (hybrid deposits warned, not
        // detected): no ML-KEM secret and no v2 address.
        assert!(!loaded.has_pq_keys());
        assert!(loaded.kem_keypair().is_none());
        assert!(!loaded.public_address().has_pq_keys());
        assert!(loaded.public_address_uri(Network::Testnet).is_err());
    }

    #[test]
    fn load_rejects_malformed_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.hex");
        std::fs::write(&bad, "not-hex").unwrap();
        let ok = dir.path().join("ok.hex");
        std::fs::write(
            &ok,
            hex::encode(
                AccountKey::random(&mut rand::rngs::OsRng)
                    .view_private_key()
                    .to_bytes(),
            ),
        )
        .unwrap();

        assert!(ReserveKeys::load(
            Some(bad.to_str().unwrap()),
            Some(ok.to_str().unwrap()),
            None
        )
        .is_err());
    }

    /// With a PQ seed file the reserve derives ML-KEM + ML-DSA keys and
    /// publishes a v2 address whose ML-KEM public key matches the secret the
    /// scanner decapsulates with — self-consistent by construction (#972).
    #[test]
    fn load_derives_pq_keys_and_publishes_v2_address() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountKey::random(&mut rand::rngs::OsRng);
        let view_path = dir.path().join("view.hex");
        let spend_path = dir.path().join("spend.hex");
        write_key(&view_path, &account.view_private_key().to_bytes());
        write_key(&spend_path, &account.spend_private_key().to_bytes());

        // A 64-byte BIP39 seed, exactly what a normal wallet feeds
        // `derive_pq_keys_from_seed`.
        let seed_path = dir.path().join("pq_seed.hex");
        write_key(&seed_path, &[0x42u8; BIP39_SEED_SIZE]);

        let loaded = ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
            Some(seed_path.to_str().unwrap()),
        )
        .unwrap()
        .expect("classical + PQ material present");

        // The reserve now holds an ML-KEM secret.
        assert!(loaded.has_pq_keys());
        let kem = loaded.kem_keypair().expect("reserve has an ML-KEM keypair");

        // The published address is v2, and its ML-KEM public key is the one
        // senders will encapsulate to — the exact key whose secret the scanner
        // decapsulates with.
        let addr = loaded.public_address();
        assert!(addr.has_pq_keys(), "reserve address is v2");
        assert_eq!(
            addr.kem_public_key(),
            kem.public_key().as_bytes(),
            "published ML-KEM key matches the reserve's decapsulation secret",
        );

        // The v2 URI round-trips: decoding it recovers the same PQ keys.
        let uri = loaded.public_address_uri(Network::Testnet).unwrap();
        assert!(uri.starts_with("tbotho://2/"), "got: {uri}");
        let (decoded, network) = bth_address_codec::decode_address(&uri).unwrap();
        assert_eq!(network, Network::Testnet);
        assert!(decoded.has_pq_keys());
        assert_eq!(decoded.kem_public_key(), addr.kem_public_key());
        assert_eq!(decoded.dsa_public_key(), addr.dsa_public_key());
        assert_eq!(
            decoded.spend_public_key().to_bytes(),
            addr.spend_public_key().to_bytes()
        );

        // Deterministic: the same seed derives the same published PQ keys.
        let loaded2 = ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
            Some(seed_path.to_str().unwrap()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            loaded2.public_address().kem_public_key(),
            addr.kem_public_key()
        );
    }

    #[test]
    fn load_rejects_malformed_pq_seed_file() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountKey::random(&mut rand::rngs::OsRng);
        let view_path = dir.path().join("view.hex");
        let spend_path = dir.path().join("spend.hex");
        write_key(&view_path, &account.view_private_key().to_bytes());
        write_key(&spend_path, &account.spend_private_key().to_bytes());

        // Wrong length (32 bytes, not the 64-byte BIP39 seed) is rejected.
        let short = dir.path().join("short.hex");
        std::fs::write(&short, hex::encode([0x11u8; 32])).unwrap();
        assert!(ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
            Some(short.to_str().unwrap()),
        )
        .is_err());

        // Non-hex is rejected.
        let bad = dir.path().join("bad.hex");
        std::fs::write(&bad, "not-hex").unwrap();
        assert!(ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
            Some(bad.to_str().unwrap()),
        )
        .is_err());
    }

    /// End-to-end: a hybrid deposit encapsulated to the reserve's PUBLISHED v2
    /// address is DETECTED by the reserve scanner. This is the #970 warn-path
    /// becoming a detect-path once the reserve holds its ML-KEM secret (#972):
    /// the sender encapsulates to the exact address the reserve advertises, and
    /// the scanner decapsulates with the matching secret.
    #[test]
    fn hybrid_deposit_to_published_v2_address_is_detected() {
        use crate::{bth_rpc::RpcOutput, bth_scan::scan_deposit_output};
        use bth_transaction_clsag::TxOutput;
        use bth_transaction_types::ClusterTagVector;

        let dir = tempfile::tempdir().unwrap();
        let account = AccountKey::random(&mut rand::rngs::OsRng);
        let view_path = dir.path().join("view.hex");
        let spend_path = dir.path().join("spend.hex");
        write_key(&view_path, &account.view_private_key().to_bytes());
        write_key(&spend_path, &account.spend_private_key().to_bytes());
        let seed_path = dir.path().join("pq_seed.hex");
        write_key(&seed_path, &[0x24u8; BIP39_SEED_SIZE]);

        let reserve = ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
            Some(seed_path.to_str().unwrap()),
        )
        .unwrap()
        .unwrap();

        // A sender encapsulates a hybrid deposit to the reserve's published v2
        // address (exactly what `decode_address` would hand a sender).
        let published = reserve.public_address();
        let output_index = 0u32;
        let out = TxOutput::new_hybrid_to_address(
            7_000_000_000_000,
            &published,
            output_index,
            None,
            ClusterTagVector::empty(),
        )
        .expect("published address is v2 (carries an ML-KEM key)");
        assert!(
            out.kem_ciphertext.is_some(),
            "a hybrid deposit carries a KEM ciphertext"
        );

        let rpc = RpcOutput {
            tx_hash: "0xpublished".to_string(),
            output_index,
            target_key: hex::encode(out.target_key),
            public_key: hex::encode(out.public_key),
            amount: out.amount,
            cluster_tags: vec![],
            e_memo: None,
            kem_ciphertext: out.kem_ciphertext.as_ref().map(hex::encode),
        };

        // Without the ML-KEM secret the reserve cannot see it (the #970 warn
        // path)...
        assert!(
            scan_deposit_output(&rpc, reserve.account(), None)
                .unwrap()
                .is_none(),
            "classical-only reserve cannot detect a hybrid deposit",
        );
        // ...but wired with the reserve's real secret, it is DETECTED (#972).
        let scanned = scan_deposit_output(&rpc, reserve.account(), reserve.kem_keypair())
            .unwrap()
            .expect("reserve detects a hybrid deposit to its published v2 address");
        assert_eq!(scanned.owned.amount, 7_000_000_000_000);
        assert!(
            scanned.owned.kem_ciphertext.is_some(),
            "detected output retains its KEM ciphertext for the release path",
        );
    }
}
