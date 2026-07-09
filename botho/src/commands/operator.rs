//! `botho operator …` — operator tooling (#707, P4.2 of the #695 proposal).
//!
//! Today this hosts a single subcommand, `mint-read-link`, which mints a
//! node-verified magic-link READ token from the `[rpc.operator]
//! read_token_secret` in the node's config file and prints the dashboard URL
//! the operator opens:
//!
//! ```text
//! https://<dashboard>/operator#token=op.<exp>.<hmac>
//! ```
//!
//! The token is a bearer credential granting READS ONLY (per-peer gate
//! classification, configured quorum contents, audit log) — it can never
//! change node state. The key is never mailed; the operator carries the link.

use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use bth_transaction_types::constants::Network;

use crate::{
    config::Config,
    operator_key::OperatorKeyFile,
    rpc::auth::{mint_operator_read_token, DEFAULT_OPERATOR_TOKEN_TTL_SECONDS},
};

/// Default filename for a generated operator keypair (in the current dir).
pub const DEFAULT_OPERATOR_KEY_FILE: &str = "operator-key.json";

/// Environment variable holding the passphrase for non-interactive
/// `botho operator keygen` (CI/automation). When set, it is used verbatim and
/// no interactive prompt is shown. An empty value is still REFUSED by
/// [`OperatorKeyFile::generate`] (the #474/#475 no-plaintext rule holds on
/// every path).
pub const OPERATOR_KEY_PASSPHRASE_ENV: &str = "BOTHO_OPERATOR_KEY_PASSPHRASE";

/// Default dashboard base URL the minted link points at.
pub const DEFAULT_DASHBOARD_URL: &str = "https://wallet.botho.io";

/// Build the operator read-link URL from a config file, without printing it.
///
/// Split out from [`mint_read_link`] so it is unit-testable: it takes the clock
/// as an argument and returns the URL string instead of writing to stdout.
///
/// Fails closed when `[rpc.operator] read_token_secret` is absent or empty —
/// the same condition under which the node's operator RPCs are OFF, so a link
/// minted here would be useless anyway.
pub fn build_read_link(
    config_path: &Path,
    dashboard: &str,
    ttl_seconds: u64,
    now_unix_seconds: u64,
) -> Result<String> {
    let config = Config::load(config_path)?;

    let secret = config.rpc.operator_read_token_secret().ok_or_else(|| {
        anyhow!(
            "[rpc.operator] read_token_secret is not configured in {} — \
             add a [rpc.operator] section with a read_token_secret to enable \
             the operator read surface",
            config_path.display()
        )
    })?;

    let exp = now_unix_seconds.saturating_add(ttl_seconds);
    let token = mint_operator_read_token(secret, exp);

    // Trim a trailing slash so we don't emit `.../operator` with a double slash.
    let base = dashboard.trim_end_matches('/');
    Ok(format!("{base}/operator#token={token}"))
}

/// `botho operator mint-read-link`: mint a read link and print it to stdout.
pub fn mint_read_link(config_path: &Path, dashboard: &str, ttl_seconds: Option<u64>) -> Result<()> {
    let ttl = ttl_seconds.unwrap_or(DEFAULT_OPERATOR_TOKEN_TTL_SECONDS);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| anyhow!("system clock is before the Unix epoch"))?;

    let url = build_read_link(config_path, dashboard, ttl, now)?;

    // Operator-facing tooling: stdout is the deliverable.
    println!("{url}");
    eprintln!(
        "Operator read link minted (valid for {} days). This is a bearer \
         credential granting READ-ONLY access — anyone with the link can view \
         operator trust internals until it expires; it grants no write \
         capability.",
        ttl / (24 * 60 * 60)
    );
    Ok(())
}

/// Prompt for a passphrase (hidden) with confirmation, refusing an empty one.
///
/// Mirrors the wallet CLI's hidden-entry pattern (`rpassword::read_password`).
/// The confirmation loop guards against a typo silently encrypting the key
/// under an unknown passphrase.
fn prompt_new_passphrase() -> Result<String> {
    loop {
        eprint!("Enter a passphrase to encrypt the operator private key (required): ");
        std::io::stderr().flush()?;
        let pass = rpassword::read_password()?;

        if pass.trim().is_empty() {
            eprintln!("A passphrase is required — the private key is never written unencrypted. Try again.");
            continue;
        }

        eprint!("Confirm passphrase: ");
        std::io::stderr().flush()?;
        let confirm = rpassword::read_password()?;

        if pass != confirm {
            eprintln!("Passphrases did not match. Try again.");
            continue;
        }
        return Ok(pass);
    }
}

/// Generate a keypair file and print the public key + fingerprint, without any
/// interactive prompting. Split out from [`keygen`] so it is unit-testable: it
/// takes the passphrase and output path directly and returns the generated
/// [`OperatorKeyFile`] instead of reading stdin.
///
/// Refuses an empty passphrase (via [`OperatorKeyFile::generate`]) and refuses
/// to overwrite an existing file (via [`OperatorKeyFile::write_to`]).
pub fn generate_key_file(output: &Path, passphrase: &str) -> Result<OperatorKeyFile> {
    let key = OperatorKeyFile::generate(passphrase)?;
    key.write_to(output)?;
    Ok(key)
}

/// `botho operator keygen`: generate an Ed25519 operator signing keypair on the
/// operator's workstation, encrypt the private key at rest under a mandatory
/// passphrase, and print the public key + `signerKeyId` fingerprint to
/// provision into each node's `[rpc.operator] action_public_keys`.
///
/// The passphrase comes from `$BOTHO_OPERATOR_KEY_PASSPHRASE` if set
/// (non-interactive), otherwise from a hidden, confirmed prompt. An empty
/// passphrase is refused on every path.
pub fn keygen(output: Option<&str>, network: Network) -> Result<()> {
    let output = output
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OPERATOR_KEY_FILE));

    let passphrase = match std::env::var(OPERATOR_KEY_PASSPHRASE_ENV) {
        Ok(p) => p,
        Err(_) => prompt_new_passphrase()?,
    };

    let key = generate_key_file(&output, &passphrase)?;

    // Operator-facing tooling: stdout carries the values the operator copies
    // into node config; stderr carries guidance.
    println!("public_key   {}", key.public_key);
    println!("fingerprint  {}", key.fingerprint);
    eprintln!();
    eprintln!(
        "Operator signing keypair written to {} (private key encrypted at rest).",
        output.display()
    );
    eprintln!(
        "Provision the PUBLIC key on every {} node's config.toml:",
        if network.is_production() {
            "mainnet"
        } else {
            "testnet"
        }
    );
    eprintln!("    [rpc.operator]");
    eprintln!("    action_public_keys = [\"{}\"]", key.public_key);
    eprintln!();
    eprintln!(
        "Keep the key file and its passphrase safe: the private key never \
         leaves this machine, and node key-list changes are an SSH/config \
         operation only (a stolen key cannot enroll further keys)."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        operator_key::fingerprint_hex,
        rpc::auth::{verify_operator_read_token, OperatorTokenError},
    };
    use std::io::Write;

    /// Write a minimal config.toml carrying an `[rpc.operator]
    /// read_token_secret` (or not) and return its path (kept alive by the
    /// returned TempDir).
    fn write_config(operator_secret: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "network_type = \"testnet\"").unwrap();
        writeln!(f, "[network]").unwrap();
        writeln!(f, "[minting]").unwrap();
        if let Some(secret) = operator_secret {
            writeln!(f, "[rpc.operator]").unwrap();
            writeln!(f, "read_token_secret = \"{secret}\"").unwrap();
        }
        (dir, path)
    }

    #[test]
    fn mint_round_trips_against_a_config_file() {
        let secret = "operator-cli-round-trip-secret";
        let (_dir, path) = write_config(Some(secret));
        let now = 1_700_000_000;
        let ttl = 3600;

        let url = build_read_link(&path, "https://dash.example", ttl, now).unwrap();

        // URL shape.
        let prefix = "https://dash.example/operator#token=";
        assert!(url.starts_with(prefix), "unexpected url: {url}");
        let token = &url[prefix.len()..];

        // The node would verify this token with the SAME secret — round trip.
        let verified = verify_operator_read_token(token, secret, now + 10);
        assert_eq!(verified, Ok(now + ttl));

        // And it is rejected once expired (proving the exp is real).
        assert_eq!(
            verify_operator_read_token(token, secret, now + ttl + 1),
            Err(OperatorTokenError::Expired)
        );
    }

    #[test]
    fn trailing_slash_on_dashboard_is_normalized() {
        let (_dir, path) = write_config(Some("s3cr3t"));
        let url = build_read_link(&path, "https://dash.example/", 3600, 1_700_000_000).unwrap();
        assert!(url.starts_with("https://dash.example/operator#token="));
        assert!(!url.contains("//operator"));
    }

    #[test]
    fn refuses_to_mint_without_configured_secret() {
        let (_dir, path) = write_config(None);
        let err = build_read_link(&path, "https://dash.example", 3600, 1_700_000_000).unwrap_err();
        assert!(
            err.to_string()
                .contains("read_token_secret is not configured"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn refuses_to_mint_with_empty_secret() {
        let (_dir, path) = write_config(Some(""));
        let err = build_read_link(&path, "https://dash.example", 3600, 1_700_000_000).unwrap_err();
        assert!(err
            .to_string()
            .contains("read_token_secret is not configured"));
    }

    #[test]
    fn keygen_refuses_empty_passphrase_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("operator-key.json");
        let err = generate_key_file(&out, "").unwrap_err();
        assert!(
            err.to_string().contains("passphrase is required"),
            "unexpected error: {err}"
        );
        assert!(!out.exists(), "no key file should be written on refusal");
    }

    #[test]
    fn keygen_writes_a_key_whose_fingerprint_matches_node_side_derivation() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("operator-key.json");
        let key = generate_key_file(&out, "a real passphrase").unwrap();
        assert!(out.exists());

        // The fingerprint the CLI prints matches what a node recomputes from
        // the pubkey via the shared helper — the signerKeyId round trip.
        let pubkey = key.public_key_bytes().unwrap();
        assert_eq!(key.fingerprint, fingerprint_hex(&pubkey));
    }

    #[test]
    fn keygen_refuses_to_overwrite_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("operator-key.json");
        generate_key_file(&out, "first passphrase").unwrap();
        let err = generate_key_file(&out, "second passphrase").unwrap_err();
        assert!(
            err.to_string().contains("refusing to overwrite"),
            "unexpected error: {err}"
        );
    }
}
