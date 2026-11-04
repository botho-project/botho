//! Test-only closed-store replay contract; no DRT leg is registered here.
use botho::{
    block::Block,
    ledger::{
        formal_drt_supply::{
            self as oracle, Ghost, KeyHistory, Observation, Outcome, RealInput, Snapshot,
        },
        Ledger,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::Instant};

pub const PROFILE: &str = "genesis_origin_easy_difficulty_u64_max_real_pow_maturity_720";
pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn fixture_source_hash() -> String {
    let mut h = Sha256::new();
    for source in [
        include_bytes!("supply.rs").as_slice(),
        include_bytes!("supply_oracle_tests.rs"),
        include_bytes!("../Cargo.toml"),
        include_bytes!("../Cargo.lock"),
        include_bytes!("../../rust-toolchain"),
    ] {
        h.update((source.len() as u64).to_le_bytes());
        h.update(source);
    }
    hex::encode(h.finalize())
}
pub fn profile_hash() -> String {
    hash(format!("{PROFILE};network=Ledger::open default;release opt3 overflow_checks=true;schema=botho.ledger.2;no snapshot seeding").as_bytes())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub schema: u32,
    pub profile: String,
    pub profile_hash: String,
    pub source_hash: String,
    pub fixture_source_hash: String,
    pub network: String,
    pub data_sha256: String,
    pub data_bytes: u64,
    pub pre: Snapshot,
    pub ghost: Vec<RealInput>,
    pub history: KeyHistory,
    pub candidate: Block,
    pub inputs: Vec<RealInput>,
    pub rejection: Option<(bool, String)>,
}

/// The entire CLOSED data.mdb image binds all tables, full block history,
/// MTP/difficulty/snapshot-base metadata and LMDB metadata. lock.mdb contains
/// process synchronization, not validator/writer state; it is never copied.
/// Caller must have dropped every source handle. Never hard-link the image.
pub fn copy_closed(source: &Path, target: &Path) -> Result<(String, u64), String> {
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || !matches!(entry.file_name().to_str(), Some("data.mdb" | "lock.mdb"))
        {
            return Err("unexpected closed-store entry; review identity scope".into());
        }
    }
    fs::create_dir(target).map_err(|e| e.to_string())?;
    let bytes = fs::read(source.join("data.mdb")).map_err(|e| e.to_string())?;
    fs::write(target.join("data.mdb"), &bytes).map_err(|e| e.to_string())?;
    Ok((hash(&bytes), bytes.len() as u64))
}

impl Saved {
    pub fn identity(&self) -> Result<String, String> {
        Ok(hash(
            &serde_json::to_vec(&json!({"data":self.data_sha256,"bytes":self.data_bytes,
            "network":self.network,"profile":self.profile_hash,"pre":self.pre,
            "ghost":self.ghost,"known_key_history":self.history}))
            .map_err(|e| e.to_string())?,
        ))
    }

    pub fn write(&self, directory: &Path) -> Result<String, String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        fs::write(directory.join("manifest.json"), &bytes).map_err(|e| e.to_string())?;
        Ok(hash(&bytes))
    }

    /// expected_hash comes from the retained receipt, not the loaded manifest.
    pub fn load(directory: &Path, expected_hash: &str) -> Result<Self, String> {
        let bytes = fs::read(directory.join("manifest.json")).map_err(|e| e.to_string())?;
        if hash(&bytes) != expected_hash {
            return Err("saved manifest hash mismatch".into());
        }
        let saved: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if saved.schema != 1
            || saved.profile != PROFILE
            || saved.profile_hash != profile_hash()
            || saved.source_hash != oracle::source_hash()
            || saved.fixture_source_hash != fixture_source_hash()
            || saved.pre.state.difficulty != u64::MAX
            || oracle::lottery_maturity() != 720
        {
            return Err("saved schema/source/profile mismatch".into());
        }
        let data = fs::read(directory.join("store/data.mdb")).map_err(|e| e.to_string())?;
        if data.len() as u64 != saved.data_bytes || hash(&data) != saved.data_sha256 {
            return Err("saved closed-store hash mismatch".into());
        }
        Ghost::from_sources(&saved.ghost)?;
        Ok(saved)
    }

    pub fn reset(&self, directory: &Path) -> Result<Reset, String> {
        let start = Instant::now();
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let path = dir.path().join("ledger");
        let (sha, len) = copy_closed(&directory.join("store"), &path)?;
        if sha != self.data_sha256 || len != self.data_bytes {
            return Err("reset image mismatch".into());
        }
        let ledger = Ledger::open(&path).map_err(|e| e.to_string())?;
        if format!("{:?}", ledger.network()) != self.network
            || hash(&fs::read(path.join("data.mdb")).map_err(|e| e.to_string())?)
                != self.data_sha256
        {
            return Err("open changed exact persisted identity/network".into());
        }
        let watched: Vec<_> = self.history.iter().map(|(key, _)| *key).collect();
        let actual = oracle::snapshot(&ledger, &watched).map_err(|e| e.to_string())?;
        if actual.fingerprint().map_err(|e| e.to_string())?
            != self.pre.fingerprint().map_err(|e| e.to_string())?
        {
            return Err("reset observed pre-state mismatch".into());
        }
        let ghost = Ghost::from_sources(&self.ghost)?;
        let reset_us = start.elapsed().as_micros();
        Ok(Reset {
            ledger,
            _dir: dir,
            ghost,
            reset_us,
        })
    }
}

pub struct Reset {
    // Drop the ledger BEFORE deleting the directory (declaration order).
    pub ledger: Ledger,
    _dir: tempfile::TempDir,
    pub ghost: Ghost,
    pub reset_us: u128,
}

/// Always executes the real production path. `candidate` may be freshly
/// signed or exact saved bytes; callers must label that provenance.
pub fn execute(
    saved: &Saved,
    reset: &mut Reset,
    candidate: Block,
) -> Result<(Observation, u128), String> {
    let watched: Vec<_> = saved.history.iter().map(|(key, _)| *key).collect();
    let observation = oracle::attempt(&reset.ledger, candidate, &watched)
        .map_err(|e| format!("infrastructure: {e}"))?;
    if observation
        .before
        .fingerprint()
        .map_err(|e| e.to_string())?
        != saved.pre.fingerprint().map_err(|e| e.to_string())?
    {
        return Err("attempt did not start at saved observed pre-state".into());
    }
    let start = Instant::now();
    if let Some((stage, message)) = &saved.rejection {
        if observation.structure_accepted != *stage
            || !matches!(&observation.outcome, Outcome::Rejected { error } if error.contains(message))
            || observation
                .before
                .fingerprint()
                .map_err(|e| e.to_string())?
                != observation.after.fingerprint().map_err(|e| e.to_string())?
            || !observation.new_outputs.is_empty()
        {
            return Err("replay rejection/rollback mismatch".into());
        }
    } else {
        if !observation.structure_accepted || observation.structure_error.is_some() {
            return Err("accepted replay failed its production precheck".into());
        }
        reset
            .ghost
            .apply(&observation, &saved.inputs, &saved.history)?;
    }
    Ok((observation, start.elapsed().as_micros()))
}

pub fn replay(directory: &Path, expected_hash: &str) -> Result<Value, String> {
    let start = Instant::now();
    let saved = Saved::load(directory, expected_hash)?;
    let load_us = start.elapsed().as_micros();
    let mut reset = saved.reset(directory)?;
    let (observation, frame_us) = execute(&saved, &mut reset, saved.candidate.clone())?;
    Ok(
        json!({"identity":saved.identity()?,"saved_manifest_sha256":expected_hash,"signed_bytes_reused":true,
        "fresh_signatures":0,"load_us":load_us,"reset_us":reset.reset_us,"frame_us":frame_us,
        "precheck_us":observation.precheck_us,"commit_us":observation.add_block_us,
        "observation_us":observation.observation_us,"outcome":observation.outcome,
        "elapsed_us":start.elapsed().as_micros()}),
    )
}
