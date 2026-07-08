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
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};

use crate::{
    config::Config,
    rpc::auth::{mint_operator_read_token, DEFAULT_OPERATOR_TOKEN_TTL_SECONDS},
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::auth::{verify_operator_read_token, OperatorTokenError};
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
}
