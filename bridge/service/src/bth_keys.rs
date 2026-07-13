// Copyright (c) 2024 The Botho Foundation

//! Reserve wallet key material for the live BTH transports (#856).
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
//! Absent key files disable the relevant transport (watch-only): the deposit
//! watcher stays idle and release submission is disabled, exactly the
//! fail-safe posture the stubs had.

use bth_account_keys::AccountKey;
use bth_crypto_keys::RistrettoPrivate;

/// Reserve wallet keys reconstructed from the configured key files.
#[derive(Clone)]
pub struct ReserveKeys {
    account: AccountKey,
}

impl ReserveKeys {
    /// Load from the hex spend/view private-key files. Returns `Ok(None)`
    /// (transport disabled, not an error) when either path is absent —
    /// watch-only deployments run without reserve keys.
    pub fn load(
        view_key_file: Option<&str>,
        spend_key_file: Option<&str>,
    ) -> Result<Option<Self>, String> {
        let (Some(view_path), Some(spend_path)) = (view_key_file, spend_key_file) else {
            return Ok(None);
        };
        let view_private = read_hex_scalar(view_path, "bth.view_key_file")?;
        let spend_private = read_hex_scalar(spend_path, "bth.spend_key_file")?;
        Ok(Some(Self {
            account: AccountKey::new(&spend_private, &view_private),
        }))
    }

    /// The reserve account (view + spend private keys).
    pub fn account(&self) -> &AccountKey {
        &self.account
    }
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

    #[test]
    fn load_none_when_key_files_absent() {
        assert!(ReserveKeys::load(None, None).unwrap().is_none());
        assert!(ReserveKeys::load(Some("v"), None).unwrap().is_none());
        assert!(ReserveKeys::load(None, Some("s")).unwrap().is_none());
    }

    #[test]
    fn load_reconstructs_account_from_hex_files() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountKey::random(&mut rand::rngs::OsRng);

        let view_path = dir.path().join("view.hex");
        let spend_path = dir.path().join("spend.hex");
        // A trailing newline (as an editor/`echo` would leave) must be tolerated.
        writeln!(
            std::fs::File::create(&view_path).unwrap(),
            "{}",
            hex::encode(account.view_private_key().to_bytes())
        )
        .unwrap();
        writeln!(
            std::fs::File::create(&spend_path).unwrap(),
            "{}",
            hex::encode(account.spend_private_key().to_bytes())
        )
        .unwrap();

        let loaded = ReserveKeys::load(
            Some(view_path.to_str().unwrap()),
            Some(spend_path.to_str().unwrap()),
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

        assert!(
            ReserveKeys::load(Some(bad.to_str().unwrap()), Some(ok.to_str().unwrap())).is_err()
        );
    }
}
