// Copyright (c) 2024 Botho Foundation

//! Persistent libp2p node identity key (issue #439).
//!
//! botho previously generated a fresh libp2p [`Keypair`] on every process
//! start, so the node's peer ID changed on every restart. That broke DNS-seed
//! discovery (the `seeds.testnet.botho.io` TXT records pin peer IDs, which went
//! stale on any restart) and reset peer-level reputation/ban state.
//!
//! This module persists the keypair to disk and loads it on startup, so the
//! peer ID is **stable across restarts**. The key lives next to the ledger and
//! config in the per-network data directory (default
//! `~/.botho/<network>/node_key`) and is written with `0600` permissions.
//!
//! ## Serialization
//!
//! The key is stored using libp2p's protobuf keypair encoding
//! ([`Keypair::to_protobuf_encoding`] / [`Keypair::from_protobuf_encoding`]).
//! This is self-describing (it records the key type), round-trips losslessly,
//! and is forward-compatible with non-ed25519 key types should we ever adopt
//! one. The bytes are written raw (no base64/hex wrapper) to keep the format
//! minimal.
//!
//! ## Security
//!
//! - The file is created with mode `0600` (owner read/write only) on Unix.
//! - The private key is **never logged**; only the derived peer ID is logged.

use std::path::Path;

use anyhow::{Context, Result};
use libp2p::{identity::Keypair, PeerId};
use tracing::info;

/// Load the node identity keypair from `path`, generating and persisting a new
/// one on first run.
///
/// Behaviour:
/// - If `path` exists, the keypair is read and decoded from it.
/// - If `path` does not exist, a fresh ed25519 keypair is generated, written to
///   `path` (creating parent directories as needed, mode `0600`), and returned.
///
/// This makes the node's peer ID stable across restarts: the same file yields
/// the same keypair (and therefore the same [`PeerId`]), while distinct data
/// directories yield distinct keys.
///
/// The private key material is never logged; the returned [`PeerId`] is logged
/// at INFO so operators can confirm a stable identity across restarts.
pub fn load_or_create_keypair(path: &Path) -> Result<Keypair> {
    if path.exists() {
        let keypair = read_keypair(path)
            .with_context(|| format!("loading node key from {}", path.display()))?;
        info!(
            "Loaded persistent node identity from {} (peer ID: {})",
            path.display(),
            PeerId::from(keypair.public())
        );
        Ok(keypair)
    } else {
        let keypair = Keypair::generate_ed25519();
        write_keypair(path, &keypair)
            .with_context(|| format!("persisting new node key to {}", path.display()))?;
        info!(
            "Generated new persistent node identity at {} (peer ID: {})",
            path.display(),
            PeerId::from(keypair.public())
        );
        Ok(keypair)
    }
}

/// Decode a keypair from the protobuf bytes stored at `path`.
fn read_keypair(path: &Path) -> Result<Keypair> {
    let bytes = std::fs::read(path).context("reading key file")?;
    let keypair = Keypair::from_protobuf_encoding(&bytes)
        .context("decoding key file (corrupt or unsupported node_key format)")?;
    Ok(keypair)
}

/// Encode `keypair` to protobuf bytes and write them to `path` with `0600`
/// permissions, creating parent directories as needed.
fn write_keypair(path: &Path, keypair: &Keypair) -> Result<()> {
    let bytes = keypair
        .to_protobuf_encoding()
        .context("encoding keypair for persistence")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating key directory {}", parent.display()))?;
    }

    write_private_file(path, &bytes).context("writing key file")?;
    Ok(())
}

/// Write `bytes` to `path` with owner-only (`0600`) permissions on Unix.
///
/// On non-Unix platforms the file is written without explicit permission bits
/// (filesystem ACLs apply); the protobuf format is identical across platforms.
#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    /// Helper: a unique temp path that does not yet exist.
    fn temp_key_path(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "botho_node_key_test_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir
    }

    #[test]
    fn generates_on_first_call_and_persists_file() {
        // Issue #439 acceptance #2: first run with no key file generates +
        // persists one.
        let dir = temp_key_path("generate");
        let path = dir.join("node_key");
        assert!(!path.exists(), "precondition: key file must not exist");

        let keypair = load_or_create_keypair(&path).expect("first load creates key");
        assert!(path.exists(), "key file must be persisted after first call");

        // Sanity: the persisted bytes round-trip back to the same peer ID.
        let reloaded = read_keypair(&path).expect("reload persisted key");
        assert_eq!(
            PeerId::from(keypair.public()),
            PeerId::from(reloaded.public()),
            "persisted key must decode to the same peer ID"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_file_yields_same_peer_id_across_calls() {
        // Issue #439 acceptance #1: a node's peer ID is identical across
        // restarts when the same node_key file is used.
        let dir = temp_key_path("stable");
        let path = dir.join("node_key");

        let first = load_or_create_keypair(&path).expect("first load");
        // Simulate a process restart: a brand-new load from the same path.
        let second = load_or_create_keypair(&path).expect("second load");

        assert_eq!(
            PeerId::from(first.public()),
            PeerId::from(second.public()),
            "same key file must yield the same peer ID across restarts"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn distinct_files_yield_distinct_peer_ids() {
        // Issue #439 acceptance #3 + HARD CONSTRAINT: distinct nodes (distinct
        // data dirs) must get distinct keys / peer IDs. A shared key would make
        // every node the same peer and break the quorum.
        let dir_a = temp_key_path("node_a");
        let dir_b = temp_key_path("node_b");
        let path_a = dir_a.join("node_key");
        let path_b = dir_b.join("node_key");

        let a = load_or_create_keypair(&path_a).expect("node a key");
        let b = load_or_create_keypair(&path_b).expect("node b key");

        assert_ne!(
            PeerId::from(a.public()),
            PeerId::from(b.public()),
            "distinct data dirs must produce distinct peer IDs"
        );

        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn protobuf_serialization_round_trips() {
        // Issue #439 acceptance #4: round-trip the serialization.
        let original = Keypair::generate_ed25519();
        let encoded = original.to_protobuf_encoding().expect("encode");
        let decoded = Keypair::from_protobuf_encoding(&encoded).expect("decode");
        assert_eq!(
            PeerId::from(original.public()),
            PeerId::from(decoded.public()),
            "protobuf encoding must round-trip to the same peer ID"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_key_has_0600_permissions() {
        // Issue #439 acceptance #2 + HARD CONSTRAINT: key file perms 0600.
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_key_path("perms");
        let path = dir.join("node_key");
        load_or_create_keypair(&path).expect("create key");

        let mode = std::fs::metadata(&path)
            .expect("stat key file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "node_key must be owner-read/write only");

        std::fs::remove_dir_all(&dir).ok();
    }
}
