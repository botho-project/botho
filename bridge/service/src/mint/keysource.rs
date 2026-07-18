// Copyright (c) 2024 The Botho Foundation

//! At-rest hardening for the relayer / submit signing keys (#1077).
//!
//! Both chain mint submitters need a signing key — a secp256k1 relayer EOA on
//! Ethereum, an Ed25519 submit key on Solana — to pay gas and broadcast the
//! threshold-authorized mint transaction. These are **not** custody keys:
//! custody is the Gnosis Safe / Squads threshold per ADR 0002. But a
//! compromised relayer/submit key still enables gas drain and submission
//! griefing, so its at-rest handling is hardened here:
//!
//! 1. **Load source**. A key may come from a plaintext file on disk (an
//!    explicit testnet opt-in) OR from an environment variable (`*_env`). When
//!    an env-var name is configured it takes PRECEDENCE over the file, so a
//!    mainnet deployment never needs a plaintext key file on disk — the secret
//!    is injected at runtime by a secrets manager, a systemd `LoadCredential`,
//!    or an OS keyring that exports into the process environment. A
//!    configured-but-unset var fails closed rather than silently falling back
//!    to a file.
//! 2. **File-permission preflight**. A group- or world-accessible key file is a
//!    leak surface an auditor will flag. The loader logs the offending octal
//!    mode and, when `enforce_permissions` is set (recommended for mainnet),
//!    refuses to load; otherwise it warns (the testnet-compatible default, so
//!    existing drills with loosely-permissioned files keep working).
//! 3. **Zeroization**. The raw secret buffer is returned inside a
//!    [`zeroize::Zeroizing`] wrapper, so it is wiped from memory on drop once
//!    the caller has parsed it into the chain-specific signer.
//!
//! Key material is NEVER logged (the mainnet key-hygiene posture): only the
//! file path, its permission mode, and the env-var NAME ever appear in a
//! message.

use zeroize::Zeroizing;

use super::MintError;

/// Where a relayer / submit signing key is loaded from, plus the at-rest
/// policy to apply.
pub struct KeySourceConfig<'a> {
    /// Path to a plaintext key file (an explicit testnet opt-in), if any.
    pub file: Option<&'a str>,
    /// Name of an environment variable holding the key (the non-plaintext,
    /// no-key-file-on-disk load path). Takes precedence over `file`.
    pub env_var: Option<&'a str>,
    /// Hard-fail (vs. warn) when a key FILE is group/world accessible.
    pub enforce_permissions: bool,
    /// Human-readable label for messages (e.g. `ethereum.private_key`). Only
    /// the label — never the secret — appears in errors/logs.
    pub label: &'a str,
}

/// Load raw signing-key material from the configured source.
///
/// Returns `Ok(None)` when neither a file nor an env var is configured (a
/// watch-only deployment). Otherwise returns the secret in a [`Zeroizing`]
/// buffer that is wiped on drop; the caller parses it into the chain-specific
/// signer. The secret itself is never logged.
pub fn load_key_material(src: KeySourceConfig<'_>) -> Result<Option<Zeroizing<String>>, MintError> {
    // Env var takes precedence: it is the mainnet load path (no plaintext key
    // file on disk). A configured-but-missing var fails closed rather than
    // silently falling back to a file.
    if let Some(var) = src.env_var {
        return match std::env::var(var) {
            Ok(val) => Ok(Some(Zeroizing::new(val))),
            Err(std::env::VarError::NotPresent) => Err(MintError::Config(format!(
                "{}: env var {} is configured but not set",
                src.label, var
            ))),
            Err(std::env::VarError::NotUnicode(_)) => Err(MintError::Config(format!(
                "{}: env var {} is not valid UTF-8",
                src.label, var
            ))),
        };
    }

    let Some(path) = src.file else {
        return Ok(None);
    };

    preflight_permissions(path, src.enforce_permissions, src.label)?;

    let raw = std::fs::read_to_string(path)
        .map_err(|e| MintError::Config(format!("{}: cannot read {}: {}", src.label, path, e)))?;
    Ok(Some(Zeroizing::new(raw)))
}

/// Refuse (or warn) when a key file is accessible beyond its owner.
///
/// A key file should be `0600` (or `0400`). Any group/world permission bit
/// (`0o077`) is a leak surface. On non-Unix platforms file-mode bits are not
/// meaningful, so the preflight is a no-op there.
#[cfg(unix)]
fn preflight_permissions(path: &str, enforce: bool, label: &str) -> Result<(), MintError> {
    use std::os::unix::fs::PermissionsExt;

    let meta = std::fs::metadata(path)
        .map_err(|e| MintError::Config(format!("{}: cannot stat {}: {}", label, path, e)))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        let msg = format!(
            "{}: key file {} is group/world accessible (mode {:#o}); \
             tighten it to 0600 (`chmod 600 {}`)",
            label, path, mode, path
        );
        if enforce {
            return Err(MintError::Config(format!(
                "{} — refusing to load (enforce_key_permissions=true)",
                msg
            )));
        }
        tracing::warn!(
            "{} — loading anyway (enforce_key_permissions=false; set it true for mainnet)",
            msg
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn preflight_permissions(_path: &str, _enforce: bool, _label: &str) -> Result<(), MintError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configured-but-unset env var fails closed (does NOT silently fall back
    /// to a file): the operator asked for env loading, so a missing var is a
    /// misconfiguration, not a reason to read a plaintext file.
    #[test]
    fn missing_env_var_fails_closed() {
        let var = "BTH_TEST_KEYSOURCE_UNSET_1077";
        std::env::remove_var(var);
        let err = load_key_material(KeySourceConfig {
            file: Some("/should/not/be/read"),
            env_var: Some(var),
            enforce_permissions: false,
            label: "test.key",
        })
        .unwrap_err();
        match err {
            MintError::Config(m) => assert!(m.contains("not set"), "got: {m}"),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    /// The env-var load path returns the secret (no key file on disk) and the
    /// buffer is a `Zeroizing<String>` (wiped on drop).
    #[test]
    fn env_var_load_path_returns_secret() {
        let var = "BTH_TEST_KEYSOURCE_SET_1077";
        std::env::set_var(var, "  deadbeef\n");
        let loaded = load_key_material(KeySourceConfig {
            file: None,
            env_var: Some(var),
            enforce_permissions: true, // no file, so perms never checked
            label: "test.key",
        })
        .unwrap()
        .expect("env var is set");
        assert_eq!(loaded.trim(), "deadbeef");
        std::env::remove_var(var);
    }

    /// Neither source configured => watch-only (Ok(None), not an error).
    #[test]
    fn no_source_is_watch_only() {
        assert!(load_key_material(KeySourceConfig {
            file: None,
            env_var: None,
            enforce_permissions: true,
            label: "test.key",
        })
        .unwrap()
        .is_none());
    }

    /// A `0600` file loads cleanly under either policy.
    #[cfg(unix)]
    #[test]
    fn secure_file_loads() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.hex");
        std::fs::write(&path, "cafebabe\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        for enforce in [false, true] {
            let loaded = load_key_material(KeySourceConfig {
                file: Some(path.to_str().unwrap()),
                env_var: None,
                enforce_permissions: enforce,
                label: "test.key",
            })
            .unwrap()
            .expect("0600 file loads");
            assert_eq!(loaded.trim(), "cafebabe");
        }
    }

    /// A group/world-readable file is REFUSED when enforcement is on and
    /// merely WARNED about (still loads) when it is off — the two halves of the
    /// AC. The error/warn message names the offending octal mode.
    #[cfg(unix)]
    #[test]
    fn group_world_readable_file_rejected_when_enforced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.hex");
        std::fs::write(&path, "feedface\n").unwrap();
        // 0644: world-readable — the classic `echo > file` / editor default.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        // enforce=true => hard refusal, message carries the octal mode.
        let err = load_key_material(KeySourceConfig {
            file: Some(path.to_str().unwrap()),
            env_var: None,
            enforce_permissions: true,
            label: "test.key",
        })
        .unwrap_err();
        match err {
            MintError::Config(m) => {
                assert!(m.contains("group/world accessible"), "got: {m}");
                assert!(m.contains("0o644"), "message names the mode: {m}");
                assert!(m.contains("refusing to load"), "got: {m}");
            }
            other => panic!("expected Config error, got {other:?}"),
        }

        // enforce=false (testnet default) => warns but still loads, so existing
        // drills are not broken.
        let loaded = load_key_material(KeySourceConfig {
            file: Some(path.to_str().unwrap()),
            env_var: None,
            enforce_permissions: false,
            label: "test.key",
        })
        .unwrap()
        .expect("testnet default warns but loads");
        assert_eq!(loaded.trim(), "feedface");
    }

    /// A group-readable (but not world) file is also caught (`0o040` bit).
    #[cfg(unix)]
    #[test]
    fn group_readable_file_also_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.hex");
        std::fs::write(&path, "abcd\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        let err = load_key_material(KeySourceConfig {
            file: Some(path.to_str().unwrap()),
            env_var: None,
            enforce_permissions: true,
            label: "test.key",
        })
        .unwrap_err();
        match err {
            MintError::Config(m) => assert!(m.contains("0o640"), "got: {m}"),
            other => panic!("expected Config error, got {other:?}"),
        }
    }
}
