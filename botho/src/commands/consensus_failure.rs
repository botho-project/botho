//! A terminal consensus/application disagreement requires operator recovery.
//! The marker is also a startup latch; even an empty or partial file refuses
//! startup, and an existing first-failure record is never overwritten.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{block::Block, consensus::ConsensusValue, ledger::ChainState};

const MARKER: &str = "consensus-failure.json";

pub(super) fn marker_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(MARKER)
}

pub(super) fn refuse_marked_startup(config_path: &Path) -> Result<()> {
    let marker = marker_path(config_path);
    match fs::symlink_metadata(&marker) {
        Ok(_) => bail!(
            "Consensus failure marker {} exists; node startup refused. Preserve the evidence and reconcile the externalized decision before operator recovery",
            marker.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Cannot check consensus failure marker; refusing startup"),
    }
}

pub(super) fn height_already_applied(block_height: u64, ledger_height: Option<u64>) -> bool {
    matches!(ledger_height, Some(height) if block_height <= height)
}

/// Exit the node runtime before writing evidence: detached RPC handlers must
/// not remain available while a filesystem write or fsync blocks.
pub(super) struct TerminalFailure {
    evidence: FailureEvidence,
    config_path: PathBuf,
    shutdown: Arc<AtomicBool>,
}

impl TerminalFailure {
    pub(super) fn finish(self) -> Result<()> {
        match self.evidence.persist(&self.config_path) {
            Ok(marker) => bail!(
                "Consensus stopped after externalized-block failure; evidence and startup refusal marker: {}. Operator recovery is required",
                marker.display()
            ),
            Err(error) => {
                tracing::error!(error = %error,
                    "CONSENSUS HALTED: failure evidence is not durable. Mining, RPC and networking are stopped. Stop the service before interrupting this process; operator recovery is required");
                let failure = self.evidence.undurable(self.shutdown);
                failure.downcast_ref::<UndurableFailure>().expect("created above").wait_for_operator();
                Ok(())
            }
        }
    }
}

impl std::fmt::Debug for TerminalFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalFailure").finish_non_exhaustive()
    }
}

impl std::fmt::Display for TerminalFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Terminal consensus failure requires evidence persistence after runtime shutdown"
        )
    }
}

impl std::error::Error for TerminalFailure {}

/// Retains evidence after the runtime has stopped when persistence failed.
pub(super) struct UndurableFailure {
    _evidence: FailureEvidence,
    shutdown: Arc<AtomicBool>,
}

impl UndurableFailure {
    pub(super) fn wait_for_operator(&self) {
        while !self.shutdown.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

impl std::fmt::Debug for UndurableFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UndurableFailure").finish_non_exhaustive()
    }
}

impl std::fmt::Display for UndurableFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Consensus failure evidence is not durable; node must remain stopped"
        )
    }
}

impl std::error::Error for UndurableFailure {}

#[derive(Serialize)]
struct CachedValue {
    tx_hash: String,
    /// Missing cache entries are themselves relevant to a build failure.
    is_minting_tx: Option<bool>,
    raw_transaction_hex: Option<String>,
}

#[derive(Serialize)]
pub(super) struct FailureEvidence {
    version: u32,
    recorded_at_unix_seconds: Option<u64>,
    externalized_slot: u64,
    scp_slot_after_externalization: u64,
    ledger_height: Option<u64>,
    ledger_tip_hash: Option<String>,
    reason: String,
    values: Vec<ConsensusValue>,
    cached_values: Vec<CachedValue>,
    rejected_block: Option<Block>,
}

impl FailureEvidence {
    pub(super) fn terminal(self, config_path: &Path, shutdown: Arc<AtomicBool>) -> anyhow::Error {
        TerminalFailure {
            evidence: self,
            config_path: config_path.to_path_buf(),
            shutdown,
        }
        .into()
    }

    pub(super) fn undurable(self, shutdown: Arc<AtomicBool>) -> anyhow::Error {
        UndurableFailure {
            _evidence: self,
            shutdown,
        }
        .into()
    }

    pub(super) fn capture(
        slot: u64,
        scp_slot: u64,
        checkpoint: Option<&ChainState>,
        reason: String,
        values: &[ConsensusValue],
        rejected_block: Option<Block>,
        get_cached: impl Fn(&[u8; 32]) -> Option<(Vec<u8>, bool)>,
    ) -> Self {
        Self {
            version: 1,
            recorded_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs()),
            externalized_slot: slot,
            scp_slot_after_externalization: scp_slot,
            ledger_height: checkpoint.map(|state| state.height),
            ledger_tip_hash: checkpoint.map(|state| hex::encode(state.tip_hash)),
            reason,
            values: values.to_vec(),
            cached_values: values
                .iter()
                .map(|value| {
                    let cached = get_cached(&value.tx_hash);
                    CachedValue {
                        tx_hash: hex::encode(value.tx_hash),
                        is_minting_tx: cached.as_ref().map(|(_, minting)| *minting),
                        raw_transaction_hex: cached.map(|(bytes, _)| hex::encode(bytes)),
                    }
                })
                .collect(),
            rejected_block,
        }
    }

    pub(super) fn persist(&self, config_path: &Path) -> Result<PathBuf> {
        let marker = marker_path(config_path);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Create the latch first. A write/flush failure leaves it in place;
        // startup never treats a partial diagnostic as permission to proceed.
        let mut file = options
            .open(&marker)
            .context("Cannot create consensus failure marker")?;
        serde_json::to_writer(&mut file, self)
            .context("Cannot write consensus failure evidence")?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()
            .context("Cannot sync consensus failure evidence")?;
        File::open(marker.parent().expect("marker has a parent"))?
            .sync_all()
            .context("Cannot sync consensus failure marker directory")?;
        Ok(marker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(reason: &str) -> FailureEvidence {
        FailureEvidence::capture(
            7,
            8,
            Some(&ChainState {
                height: 6,
                tip_hash: [3; 32],
                ..Default::default()
            }),
            reason.into(),
            &[
                ConsensusValue::from_transaction([1; 32], 100),
                ConsensusValue::from_transaction([2; 32], 200),
            ],
            None,
            |hash| (*hash == [1; 32]).then(|| (vec![0, 1, 255], false)),
        )
    }

    #[test]
    fn persisted_failure_refuses_restart_and_preserves_first_diagnosis() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        assert!(refuse_marked_startup(&config).is_ok());
        let marker = evidence("ledger rejected externalized block")
            .persist(&config)
            .unwrap();
        assert!(refuse_marked_startup(&config).is_err());
        // Config filenames cannot bypass the latch for the same ledger.
        let alternate = dir.path().join("alternate.toml");
        assert_eq!(
            crate::config::ledger_db_path_from_config(&config),
            crate::config::ledger_db_path_from_config(&alternate)
        );
        assert!(refuse_marked_startup(&alternate).is_err());
        let before = fs::read(&marker).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(saved["externalized_slot"], 7);
        assert_eq!(saved["scp_slot_after_externalization"], 8);
        assert_eq!(saved["ledger_height"], 6);
        assert_eq!(saved["ledger_tip_hash"], hex::encode([3; 32]));
        assert_eq!(saved["cached_values"][0]["raw_transaction_hex"], "0001ff");
        assert!(saved["cached_values"][1]["raw_transaction_hex"].is_null());
        assert!(evidence("later failure").persist(&config).is_err());
        assert_eq!(fs::read(&marker).unwrap(), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(marker).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn partial_empty_and_non_file_markers_all_refuse_startup() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let marker = marker_path(&config);
        for bytes in [b"".as_slice(), b"{\"externalized_slot\":".as_slice()] {
            fs::write(&marker, bytes).unwrap();
            assert!(refuse_marked_startup(&config).is_err());
        }
        fs::remove_file(&marker).unwrap();
        fs::create_dir(&marker).unwrap();
        assert!(refuse_marked_startup(&config).is_err());
        assert!(evidence("failure").persist(&config).is_err());
        #[cfg(unix)]
        {
            fs::remove_dir(&marker).unwrap();
            std::os::unix::fs::symlink(dir.path().join("absent"), &marker).unwrap();
            assert!(refuse_marked_startup(&config).is_err());
            assert!(evidence("failure").persist(&config).is_err());
        }
    }

    #[test]
    fn run_refuses_partial_marker_before_loading_missing_or_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        fs::write(marker_path(&config), b"{partial").unwrap();
        for contents in [None, Some("this is not a valid config = [")] {
            if let Some(contents) = contents {
                fs::write(&config, contents).unwrap();
            }
            let error = super::super::run::run(&config, false, None, None).unwrap_err();
            assert!(error.to_string().contains("Consensus failure marker"));
            assert!(!error.to_string().contains("Config not found"));
        }
    }

    #[test]
    fn only_verified_filled_heights_are_benign() {
        assert!(height_already_applied(7, Some(7)));
        assert!(height_already_applied(6, Some(7)));
        assert!(!height_already_applied(8, Some(7)));
        assert!(!height_already_applied(7, None));
    }

    #[test]
    fn failed_persistence_does_not_return_for_automatic_restart() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("absent/config.toml");
        assert!(evidence("application failed").persist(&config).is_err());
        let shutdown = Arc::new(AtomicBool::new(false));
        let failure = evidence("application failed").undurable(shutdown.clone());
        let (finished, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            failure
                .downcast_ref::<UndurableFailure>()
                .unwrap()
                .wait_for_operator();
            finished.send(()).unwrap();
        });
        assert!(result.recv_timeout(Duration::from_millis(20)).is_err());
        shutdown.store(true, Ordering::SeqCst);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
