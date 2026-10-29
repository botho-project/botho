//! Versioned UTXO snapshot V2 for `botho.ledger.2` ledgers (#1286 P9, #1646).
//!
//! Export ([`Ledger::create_snapshot_v2`]) reads every table it carries from
//! one LMDB read transaction with strict decoding, and checks each output's
//! derivation context through the same strict [`v2::read_context`] the
//! consensus reads use. Import ([`Ledger::load_snapshot_v2`]) verifies the
//! whole file (commitment, section roots, bijections, each context against
//! its output, the tip block against `block_hash`, the genesis against the
//! target ledger) before its single write transaction, so every refusal
//! leaves the target byte-for-byte untouched.
//!
//! A restored ledger holds genesis and the tip block but not the blocks in
//! between (as with V1 snapshots). Lottery contexts created below the tip
//! therefore cannot be cross-checked against their creating block; the
//! restored ledger records the snapshot height under [`META_SNAPSHOT_BASE`]
//! so [`v2::read_context`] accepts exactly those (their identity is covered
//! by the verified snapshot commitment instead).
use super::{v2, *};
use crate::ledger::{
    experimental::decode,
    lottery_v2::{DerivationReads, StoredContext},
    SnapshotError, SnapshotV2Contents, UtxoSnapshotV2,
};

/// Meta key: height of the V2 snapshot this ledger was restored from
/// (u64 LE). Absent on ledgers synced from genesis.
pub(in crate::ledger) const META_SNAPSHOT_BASE: &[u8] = b"snapshot_base_height";

fn db_op(operation: &'static str) -> impl FnOnce(heed::Error) -> LedgerError {
    move |e| LedgerError::Database(format!("{operation}: {e}"))
}

fn rejected(reason: impl std::fmt::Display) -> LedgerError {
    LedgerError::InvalidBlock(format!("Snapshot rejected: {reason}"))
}

/// Map snapshot-format errors: a V1 (or other-version) snapshot is a schema
/// refusal; everything else is a failed verification.
fn verification_error(e: SnapshotError) -> LedgerError {
    match e {
        SnapshotError::VersionMismatch { expected, got } => {
            LedgerError::UnsupportedSchema(format!(
            "snapshot format V{got} is refused by botho.ledger.2 ledgers (expected V{expected}): \
             V1 snapshots carry no derivation contexts. Create a V2 snapshot with this binary \
             or sync from genesis"
        ))
        }
        other => LedgerError::InvalidBlock(format!("Snapshot verification failed: {other}")),
    }
}

/// Snapshot-side check of one context against its output: the same shape
/// rules as [`v2::read_context`], with the creating block available only
/// when it is the snapshot's tip.
fn check_context(
    utxo: &Utxo,
    context: &StoredContext,
    height: u64,
    tip: &Block,
) -> Result<(), LedgerError> {
    if context.height() != utxo.created_at || utxo.created_at > height {
        return Err(rejected("context/output creation height"));
    }
    match context {
        StoredContext::Direct { base_index, .. } => {
            if *base_index != utxo.id.output_index {
                return Err(rejected("direct context base index"));
            }
        }
        StoredContext::Lottery {
            height: created,
            ordinal,
            ..
        } => {
            if ordinal.checked_add(1) != Some(utxo.id.output_index) || *created == 0 {
                return Err(rejected("lottery context ordinal/index"));
            }
            if *created == height && tip.hash() != utxo.id.tx_hash {
                return Err(rejected("lottery context creating block (tip)"));
            }
        }
    }
    Ok(())
}

impl Ledger {
    /// Strict V2 export from one read transaction.
    pub(super) fn create_snapshot_v2(&self) -> Result<UtxoSnapshotV2, LedgerError> {
        let rtxn = self.env.read_txn().map_err(db_op("snapshot read txn"))?;
        if self
            .meta_db
            .get(&rtxn, v2::SCHEMA_KEY)
            .map_err(db_op("snapshot read marker"))?
            != Some(v2::LEDGER_SCHEMA_V2)
        {
            return Err(LedgerError::InconsistentRecord(
                "snapshot source is not marked botho.ledger.2".into(),
            ));
        }
        let state = self.chain_state_in(&rtxn)?;
        let view = self.production_view(&rtxn);
        let genesis_hash = view.genesis_hash()?;
        let lottery_pool = view.lottery_pool()?;
        let tip_bytes = self
            .blocks_db
            .get(&rtxn, &state.height)
            .map_err(db_op("snapshot read tip"))?
            .ok_or_else(|| LedgerError::InconsistentRecord("missing tip block".into()))?
            .to_vec();
        let tip: Block = decode(&tip_bytes)?;
        if tip.height() != state.height || tip.hash() != state.tip_hash {
            return Err(LedgerError::InconsistentRecord(
                "tip block disagrees with tip metadata".into(),
            ));
        }

        let mut contents = SnapshotV2Contents::default();
        for row in self
            .utxo_db
            .iter(&rtxn)
            .map_err(db_op("snapshot iterate utxos"))?
        {
            let (key, value) = row.map_err(db_op("snapshot read utxo"))?;
            let utxo: Utxo = decode(value)?;
            if key != utxo.id.to_bytes() {
                return Err(LedgerError::InconsistentRecord("utxo key/id".into()));
            }
            let (_, context) = v2::read_context(self, &rtxn, &utxo.id)?;
            contents.contexts.push((key.to_vec(), context.encode()?));
            contents.utxos.push(utxo);
        }
        if self
            .contexts_db
            .len(&rtxn)
            .map_err(db_op("snapshot count contexts"))?
            != contents.utxos.len() as u64
        {
            return Err(LedgerError::InconsistentRecord(
                "derivation contexts without outputs".into(),
            ));
        }
        for row in self
            .address_index_db
            .iter(&rtxn)
            .map_err(db_op("snapshot iterate address index"))?
        {
            let (key, value) = row.map_err(db_op("snapshot read address index"))?;
            contents.address_index.push((
                key.try_into()
                    .map_err(|_| LedgerError::StorageEncoding("address index key".into()))?,
                value.to_vec(),
            ));
        }
        for row in self
            .key_images_db
            .iter(&rtxn)
            .map_err(db_op("snapshot iterate key images"))?
        {
            let (key, value) = row.map_err(db_op("snapshot read key image"))?;
            contents.key_images.push((
                key.try_into()
                    .map_err(|_| LedgerError::StorageEncoding("key image key".into()))?,
                u64::from_le_bytes(crate::ledger::lottery_v2::fixed(value)?),
            ));
        }
        for row in self
            .cluster_wealth_db
            .iter(&rtxn)
            .map_err(db_op("snapshot iterate cluster wealth"))?
        {
            let (key, value) = row.map_err(db_op("snapshot read cluster wealth"))?;
            contents.cluster_wealth.push((
                u64::from_le_bytes(crate::ledger::lottery_v2::fixed(key)?),
                decode_cluster_wealth(value)?,
            ));
        }
        for row in self
            .bridge_import_clusters_db
            .iter(&rtxn)
            .map_err(db_op("snapshot iterate import clusters"))?
        {
            let (key, value) = row.map_err(db_op("snapshot read import cluster"))?;
            if !value.is_empty() {
                return Err(LedgerError::StorageEncoding("import value".into()));
            }
            contents
                .import_clusters
                .push(u64::from_le_bytes(crate::ledger::lottery_v2::fixed(key)?));
        }
        drop(rtxn);

        info!(
            height = state.height,
            utxo_count = contents.utxos.len(),
            key_image_count = contents.key_images.len(),
            cluster_count = contents.cluster_wealth.len(),
            "Creating V2 UTXO snapshot"
        );
        UtxoSnapshotV2::new(
            genesis_hash,
            state.height,
            state.tip_hash,
            state,
            lottery_pool,
            &tip_bytes,
            &contents,
        )
        .map_err(|e| LedgerError::Serialization(e.to_string()))
    }

    /// Read a snapshot file as V2 (V1 is a schema refusal).
    pub(super) fn read_snapshot_file(path: &Path) -> Result<UtxoSnapshotV2, LedgerError> {
        let file = std::fs::File::open(path)
            .map_err(|e| LedgerError::Database(format!("Failed to open file: {}", e)))?;
        UtxoSnapshotV2::read_from(std::io::BufReader::new(file)).map_err(|e| match e {
            e @ SnapshotError::VersionMismatch { .. } => verification_error(e),
            other => LedgerError::Serialization(other.to_string()),
        })
    }

    /// Verify-then-write V2 import into a genesis-only ledger.
    pub(super) fn load_snapshot_v2(
        &self,
        snapshot: &UtxoSnapshotV2,
        expected_block_hash: Option<&[u8; 32]>,
    ) -> Result<usize, LedgerError> {
        // 1. Self-contained integrity (no ledger access yet).
        let (contents, tip_bytes) = snapshot.verify().map_err(verification_error)?;
        if let Some(expected) = expected_block_hash {
            if &snapshot.block_hash != expected {
                return Err(LedgerError::InvalidBlock("Block hash mismatch".to_string()));
            }
        }
        let height = snapshot.height;
        if snapshot.chain_state.height != height
            || snapshot.chain_state.tip_hash != snapshot.block_hash
        {
            return Err(rejected("chain state does not describe the snapshot tip"));
        }
        let tip: Block = decode(&tip_bytes).map_err(|e| rejected(format!("tip block: {e}")))?;
        if tip.height() != height || tip.hash() != snapshot.block_hash {
            return Err(rejected(
                "tip block does not hash to the snapshot block hash",
            ));
        }
        if height == 0 && snapshot.lottery_pool != 0 {
            return Err(rejected("non-zero lottery pool at genesis"));
        }
        let mut contexts = Vec::with_capacity(contents.contexts.len());
        for (utxo, (_, encoded)) in contents.utxos.iter().zip(&contents.contexts) {
            let context = StoredContext::decode(encoded)
                .map_err(|e| rejected(format!("derivation context encoding: {e}")))?;
            check_context(utxo, &context, height, &tip)?;
            contexts.push(context);
        }

        info!(
            height,
            utxo_count = snapshot.utxo_count,
            key_image_count = snapshot.key_image_count,
            "Loading ledger from V2 snapshot"
        );

        // 2. Target checks and the write, under the exclusive writer.
        let mut wtxn = self.env.write_txn().map_err(db_op("snapshot write txn"))?;
        let genesis: Block = decode(
            self.blocks_db
                .get(&wtxn, &0)
                .map_err(db_op("snapshot read target genesis"))?
                .ok_or_else(|| LedgerError::InconsistentRecord("missing genesis block".into()))?,
        )?;
        if genesis.hash() != snapshot.genesis_hash {
            return Err(rejected(format!(
                "snapshot genesis {} does not match this ledger's genesis {} (wrong network or chain)",
                hex::encode(snapshot.genesis_hash),
                hex::encode(genesis.hash())
            )));
        }
        if height == 0 && tip.hash() != genesis.hash() {
            return Err(rejected(
                "height-0 snapshot tip is not this ledger's genesis",
            ));
        }
        let target_height = self
            .meta_db
            .get(&wtxn, META_HEIGHT)
            .map_err(db_op("snapshot read target height"))?
            .map(crate::ledger::lottery_v2::fixed)
            .transpose()?
            .map(u64::from_le_bytes)
            .unwrap_or(0);
        let mut nonempty = target_height != 0
            || self
                .blocks_db
                .len(&wtxn)
                .map_err(db_op("snapshot target"))?
                != 1;
        for table in [
            self.utxo_db,
            self.address_index_db,
            self.key_images_db,
            self.tx_index_db,
            self.cluster_wealth_db,
            self.bridge_import_clusters_db,
            self.contexts_db,
        ] {
            nonempty |= !table.is_empty(&wtxn).map_err(db_op("snapshot target"))?;
        }
        if nonempty {
            return Err(rejected(format!(
                "the target ledger is not genesis-only (height {target_height}); a V2 \
                 snapshot only loads into a freshly initialized ledger. Wipe the ledger \
                 directory first"
            )));
        }

        let utxo_count = contents.utxos.len();
        for (utxo, context) in contents.utxos.iter().zip(&contexts) {
            let key = utxo.id.to_bytes();
            let bytes =
                bincode::serialize(utxo).map_err(|e| LedgerError::Serialization(e.to_string()))?;
            self.utxo_db
                .put(&mut wtxn, &key, &bytes)
                .map_err(db_op("snapshot put utxo"))?;
            self.contexts_db
                .put(&mut wtxn, &key, &context.encode()?)
                .map_err(db_op("snapshot put context"))?;
        }
        for (target, ids) in &contents.address_index {
            self.address_index_db
                .put(&mut wtxn, target, ids)
                .map_err(db_op("snapshot put address index"))?;
        }
        for (ki, spent) in &contents.key_images {
            self.key_images_db
                .put(&mut wtxn, ki, &spent.to_le_bytes())
                .map_err(db_op("snapshot put key image"))?;
        }
        for (cluster, wealth) in &contents.cluster_wealth {
            self.cluster_wealth_db
                .put(&mut wtxn, &cluster.to_le_bytes(), &wealth.to_le_bytes())
                .map_err(db_op("snapshot put cluster wealth"))?;
        }
        for cluster in &contents.import_clusters {
            self.bridge_import_clusters_db
                .put(&mut wtxn, &cluster.to_le_bytes(), &[])
                .map_err(db_op("snapshot put import cluster"))?;
        }

        let cs = &snapshot.chain_state;
        let mut meta: Vec<(&[u8], Vec<u8>)> = vec![
            (META_HEIGHT, cs.height.to_le_bytes().to_vec()),
            (META_TIP_HASH, snapshot.block_hash.to_vec()),
            (META_TOTAL_MINED, cs.total_mined.to_le_bytes().to_vec()),
            (
                META_FEES_BURNED,
                cs.total_fees_burned.to_le_bytes().to_vec(),
            ),
            (META_DIFFICULTY, cs.difficulty.to_le_bytes().to_vec()),
            (META_TOTAL_TX, cs.total_tx.to_le_bytes().to_vec()),
            (META_EPOCH_TX, cs.epoch_tx.to_le_bytes().to_vec()),
            (
                META_EPOCH_EMISSION,
                cs.epoch_emission.to_le_bytes().to_vec(),
            ),
            (META_EPOCH_BURNS, cs.epoch_burns.to_le_bytes().to_vec()),
            (
                META_CURRENT_REWARD,
                cs.current_reward.to_le_bytes().to_vec(),
            ),
        ];
        if height > 0 {
            let tip_canonical =
                bincode::serialize(&tip).map_err(|e| LedgerError::Serialization(e.to_string()))?;
            self.blocks_db
                .put(&mut wtxn, &height, &tip_canonical)
                .map_err(db_op("snapshot put tip block"))?;
            meta.push((
                META_LOTTERY_POOL,
                snapshot.lottery_pool.to_le_bytes().to_vec(),
            ));
            meta.push((META_SNAPSHOT_BASE, height.to_le_bytes().to_vec()));
        }
        for (key, value) in meta {
            self.meta_db
                .put(&mut wtxn, key, &value)
                .map_err(db_op("snapshot put metadata"))?;
        }
        wtxn.commit().map_err(db_op("snapshot commit"))?;

        info!(utxo_count, "V2 snapshot loaded successfully");
        Ok(utxo_count)
    }
}
