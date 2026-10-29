//! Production V2 ledger schema (`botho.ledger.2`, #1286 P2).
//!
//! A fresh [`Ledger`] is marked `botho.ledger.2` and carries a
//! `derivation_contexts` table (outpoint -> [`StoredContext`]) written in the
//! same LMDB transaction as each output. Unmarked non-empty (V1) stores and
//! every other marker are refused at open (D0: no migration, no dual rule
//! set). Since protocol 7.0.0 (#1640, P3) the [`ProductionView`] adapter is
//! what the live block path validates against (`Ledger::add_block`) and what
//! the V2 producer ([`Ledger::seal_block`]) derives payouts from, through the
//! shared [`lottery_v2`](crate::ledger::lottery_v2) rules.
use super::{
    validation::{self, ReadPolicy, ValidationReads},
    *,
};
use crate::ledger::lottery_v2::{fixed, DerivationReads, StoredContext};
use heed::RoTxn;

/// Native node-wallet discovery/recovery through these contexts (#1286 P7).
pub(in crate::ledger) mod wallet;

/// Authenticated detached native-wallet witness (#1499, #1286 P10). Inactive.
pub(in crate::ledger) mod detached;

/// Metadata key holding the storage-schema marker. Shared with the private
/// experimental stores, whose (different) markers are therefore refused here
/// by exact comparison.
pub(in crate::ledger) const SCHEMA_KEY: &[u8] = b"storage_schema";
/// The production V2 schema marker.
pub(in crate::ledger) const LEDGER_SCHEMA_V2: &[u8] = b"botho.ledger.2";
/// Table name of the per-output derivation contexts.
pub(in crate::ledger) const CONTEXTS_TABLE: &str = "derivation_contexts";

/// Every table a V1 ledger could have written. A store in which any of these
/// holds a row, but which carries no marker, is an unmarked V1 store.
const V1_TABLES: [&str; 8] = [
    "blocks",
    "meta",
    "utxos",
    "address_index",
    "key_images",
    "tx_index",
    "cluster_wealth",
    "bridge_import_clusters",
];

fn db_op(operation: &'static str) -> impl FnOnce(heed::Error) -> LedgerError {
    move |e| LedgerError::Database(format!("{operation}: {e}"))
}

/// How an opened environment must be treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ledger) enum StoreSchema {
    /// No table holds a row: initialize as `botho.ledger.2` in this txn.
    Fresh,
    /// Exactly marked `botho.ledger.2`.
    V2,
}

/// Operator-facing refusal for an unmarked, non-empty (pre-7.0.0) store.
pub(in crate::ledger) fn v1_store_refusal(path: &Path) -> LedgerError {
    LedgerError::UnsupportedSchema(format!(
        "the ledger at {} is an unmarked V1 ledger store (written before the \
         botho.ledger.2 schema). This binary refuses V1 ledgers: there is no \
         migration (protocol 7.0.0 starts fresh chains from genesis, #1286). \
         Stop the node, wipe/reset the ledger by deleting that directory, and \
         restart to resync from genesis",
        path.display()
    ))
}

/// Classify the store inside the opening write transaction, BEFORE any table
/// creation or initialization write. Returning an error aborts that
/// transaction, so a refused store is left byte-for-byte untouched.
pub(in crate::ledger) fn classify(
    env: &Env,
    txn: &RwTxn<'_>,
    path: &Path,
) -> Result<StoreSchema, LedgerError> {
    let meta: Option<Database<Bytes, Bytes>> = env
        .open_database(txn, Some("meta"))
        .map_err(db_op("schema open metadata"))?;
    if let Some(meta) = meta {
        if let Some(marker) = meta
            .get(txn, SCHEMA_KEY)
            .map_err(db_op("schema read marker"))?
        {
            if marker != LEDGER_SCHEMA_V2 {
                return Err(LedgerError::UnsupportedSchema(format!(
                    "the ledger at {} carries storage marker {:?} (hex {}), not {:?}; \
                     refusing to open it as a production ledger",
                    path.display(),
                    String::from_utf8_lossy(marker),
                    hex::encode(marker),
                    String::from_utf8_lossy(LEDGER_SCHEMA_V2),
                )));
            }
            let contexts: Option<Database<Bytes, Bytes>> = env
                .open_database(txn, Some(CONTEXTS_TABLE))
                .map_err(db_op("schema open contexts"))?;
            if contexts.is_none() {
                return Err(LedgerError::InconsistentRecord(
                    "botho.ledger.2 store is missing its derivation_contexts table".into(),
                ));
            }
            return Ok(StoreSchema::V2);
        }
    }
    for name in V1_TABLES.iter().copied().chain([CONTEXTS_TABLE]) {
        let table: Option<Database<Bytes, Bytes>> = env
            .open_database(txn, Some(name))
            .map_err(db_op("schema open table"))?;
        if let Some(table) = table {
            if !table
                .is_empty(txn)
                .map_err(db_op("schema table emptiness"))?
            {
                return Err(v1_store_refusal(path));
            }
        }
    }
    Ok(StoreSchema::Fresh)
}

/// Whether `height` lies strictly between genesis and the V2 snapshot this
/// ledger was restored from (blocks the restored ledger does not hold).
fn below_snapshot_base(ledger: &Ledger, txn: &RoTxn<'_>, height: u64) -> Result<bool, LedgerError> {
    Ok(ledger
        .snapshot_base_in(txn)?
        .is_some_and(|base| height > 0 && height < base))
}

/// Strict context read inside `txn`: a missing context is
/// [`LedgerError::MissingContext`], never a default. The context must agree
/// with its persisted output (identity, creation height, index shape).
pub(in crate::ledger) fn read_context(
    ledger: &Ledger,
    txn: &RoTxn<'_>,
    id: &UtxoId,
) -> Result<(Utxo, StoredContext), LedgerError> {
    read_context_with(ledger, txn, id, &mut |height| {
        ledger
            .blocks_db
            .get(txn, &height)
            .map_err(db_op("read payout creating block"))?
            .map(|bytes| crate::ledger::experimental::decode::<Block>(bytes).map(|b| b.hash()))
            .transpose()
    })
}

/// [`read_context`] with the lottery creating-block lookup supplied by the
/// caller: `creating_hash(height)` must return the hash of the block stored
/// at `height` in the SAME `txn`, `None` if no block is stored there, or an
/// error. Lets a caller that already decoded that block in `txn` skip
/// re-decoding it per payout. A `None` is judged HERE, for every caller: it
/// is accepted only strictly below a V2 snapshot base (#1646), otherwise it
/// is [`LedgerError::InconsistentRecord`].
pub(super) fn read_context_with(
    ledger: &Ledger,
    txn: &RoTxn<'_>,
    id: &UtxoId,
    creating_hash: &mut dyn FnMut(u64) -> Result<Option<[u8; 32]>, LedgerError>,
) -> Result<(Utxo, StoredContext), LedgerError> {
    let key = id.to_bytes();
    let context = StoredContext::decode(
        ledger
            .contexts_db
            .get(txn, &key)
            .map_err(db_op("read derivation context"))?
            .ok_or_else(|| LedgerError::MissingContext(hex::encode(key)))?,
    )?;
    let utxo: Utxo = crate::ledger::experimental::decode(
        ledger
            .utxo_db
            .get(txn, &key)
            .map_err(db_op("read context output"))?
            .ok_or_else(|| {
                LedgerError::InconsistentRecord("derivation context without output".into())
            })?,
    )?;
    if utxo.id != *id || utxo.created_at != context.height() {
        return Err(LedgerError::InconsistentRecord(
            "output identity/height disagrees with its derivation context".into(),
        ));
    }
    match &context {
        StoredContext::Direct { base_index, .. } if *base_index != id.output_index => {
            return Err(LedgerError::InconsistentRecord(
                "direct context base index".into(),
            ))
        }
        StoredContext::Lottery { ordinal, .. } => {
            // Payout outpoints are (creating block hash, 1 + ordinal).
            if ordinal.checked_add(1) != Some(id.output_index) {
                return Err(LedgerError::InconsistentRecord(
                    "lottery context ordinal/index".into(),
                ));
            }
            match creating_hash(context.height())? {
                Some(hash) if hash == id.tx_hash => {}
                Some(_) => {
                    return Err(LedgerError::InconsistentRecord(
                        "lottery context creating block".into(),
                    ))
                }
                // A ledger restored from a V2 snapshot (#1646) holds no
                // blocks strictly between genesis and the snapshot tip; a
                // payout created there was checked against the snapshot's
                // verified commitment at load instead. Anything else missing
                // its creating block is inconsistent.
                None if below_snapshot_base(ledger, txn, context.height())? => {}
                None => {
                    return Err(LedgerError::InconsistentRecord(
                        "missing payout creating block".into(),
                    ))
                }
            }
        }
        StoredContext::Direct { .. } => {}
    }
    Ok((utxo, context))
}

// `get_derivation_context` is consumed by tests until P4 (RPC) reads it.
#[allow(dead_code)]
impl Ledger {
    /// The persisted derivation context of `id` (strict; #1286 P2).
    ///
    /// A missing context is [`LedgerError::MissingContext`] and corruption is
    /// an error; neither is ever replaced by a default (Direct/zero tweak).
    pub(in crate::ledger) fn get_derivation_context(
        &self,
        id: &UtxoId,
    ) -> Result<StoredContext, LedgerError> {
        let txn = self.env.read_txn().map_err(db_op("context read txn"))?;
        read_context(self, &txn, id).map(|(_, c)| c)
    }

    /// Pin one read transaction as the pre-state for the shared V2 rules.
    pub(in crate::ledger) fn production_view<'a, 'env>(
        &'a self,
        txn: &'a RoTxn<'env>,
    ) -> ProductionView<'a, 'env> {
        ProductionView { ledger: self, txn }
    }

    /// The V2 block-body producer (protocol 7.0.0, #1286 P3, #1640).
    ///
    /// Takes a block whose header, minting tx and ordinary transactions are
    /// final, draws the lottery over ONE pinned pre-state, derives every
    /// payout record through the shared rules (`lottery_v2::derive`: a fresh,
    /// independently keyed target per award; the winner's one-time target is
    /// never copied), writes the payouts and the lottery summary, and sets
    /// `header.tx_root` to the V2 body root. The block hash therefore commits
    /// to the payouts; the PoW preimage does not include `tx_root`
    /// (`BlockHeader::pow_hash`), so sealing never invalidates the PoW.
    ///
    /// Deterministic: every node that seals the same externalized block over
    /// the same pre-state produces byte-identical output. The result grants
    /// no authority to write; [`Ledger::add_block`] validates it from
    /// scratch. Errors (wrong parent/height, a strict-read failure) are
    /// returned, never papered over with a lottery-free fallback.
    pub fn seal_block(&self, block: Block) -> Result<Block, LedgerError> {
        let txn = self.env.read_txn().map_err(db_op("seal read txn"))?;
        let view = self.production_view(&txn);
        let (height, tip) = (view.height()?, view.tip_hash()?);
        // Same wording as `add_block`'s pre-checks, so operator evidence
        // (consensus-failure markers, logs) reads the same whether the
        // producer or the validator refused the block.
        let expected = height.checked_add(1);
        let mismatch = if expected != Some(block.height()) {
            Some(format!(
                "Expected height {}, got {}",
                height.saturating_add(1),
                block.height()
            ))
        } else if block.header.prev_block_hash != tip {
            Some("Previous block hash mismatch".to_string())
        } else {
            None
        };
        if let Some(mismatch) = mismatch {
            return Err(LedgerError::InvalidBlock(format!(
                "{mismatch} (V2 producer pre-state: block {} on parent {}, tip {} at height {})",
                block.height(),
                hex::encode(&block.header.prev_block_hash[..8]),
                hex::encode(&tip[..8]),
                height
            )));
        }
        let (block, _records) = crate::ledger::lottery_v2::derive(&view, block)?;
        Ok(block)
    }

    /// Crate-internal spend-recovery read (#1640): the persisted output and
    /// its committed derivation context as the crypto-layer `Context`
    /// (`base_index` = ORIGINAL derivation index, `tweak` = cumulative V2
    /// payout tweak, zero for a direct output), read strictly in one txn.
    /// Only the test/fixture accessor `Ledger::fixture_derivation_context`
    /// uses it now: the node wallet's spend path (`WalletRead for Ledger`)
    /// recovers through `v2::wallet::recover_wallet_input` (#1286 P7), the
    /// same `recover_owned` rule wallet discovery uses, so there is one
    /// recovery path. The public, RPC-facing accessor is P4's (#1641)
    /// `get_output_derivation_context`.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn spend_recovery_context(
        &self,
        id: &UtxoId,
    ) -> Result<(Utxo, bth_transaction_clsag::lottery_v2::Context), LedgerError> {
        let txn = self.env.read_txn().map_err(db_op("context read txn"))?;
        read_context(self, &txn, id).map(|(utxo, c)| (utxo, c.derivation()))
    }
}

#[cfg(test)]
impl Ledger {
    /// UNIT-TEST ONLY: apply `block`'s storage effects through the shared
    /// production writer with NO consensus validation (supplied pool and
    /// emission). Pins writer byte effects for fixtures whose block predates
    /// the V2 rules (e.g. the V1 writer golden), which `add_block` now
    /// rightly rejects. Also seeds the RPC lottery-row goldens (#1641) with
    /// fixed records, whose tweak must be byte-stable across runs (a sealed
    /// V2 block's tweak depends on its timestamp).
    pub(crate) fn apply_writer_effects_for_test(
        &self,
        block: &Block,
        records: &[bth_transaction_clsag::lottery_v2::Record],
        new_lottery_pool: u128,
        emission: Option<EmissionStateUpdate>,
    ) -> Result<(), LedgerError> {
        let state = self.get_chain_state()?;
        let bytes =
            bincode::serialize(block).map_err(|e| LedgerError::Serialization(e.to_string()))?;
        let mut txn = self.env.write_txn().map_err(db_op("test write txn"))?;
        self.write_tables().block_effects(
            &mut txn,
            block,
            &state,
            new_lottery_pool,
            emission,
            &bytes,
            super::writer::Representation::Production {
                contexts: self.contexts_db,
                records,
            },
            &mut |_| Ok(()),
            &mut || Ok(()),
        )?;
        txn.commit().map_err(db_op("test commit"))
    }
}

/// Lottery-rules generation of this binary's ledger (#1286): every
/// `botho.ledger.2` store accepts only V2 (independently keyed) payouts, so
/// the RPC reports this as `lotteryRules` (P4, #1641).
pub const LOTTERY_RULES_VERSION: u32 = 2;

/// Public, read-only copy of an output's persisted derivation context
/// (#1286 P4). A value snapshot of the crate-internal [`StoredContext`]:
/// holding one grants no write path, and it is only ever produced by the
/// strict [`Ledger::get_output_derivation_context`] and
/// [`Ledger::get_block_with_payout_contexts`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DerivationContext {
    /// Coinbase / transaction output: zero tweak, base index = output index.
    Direct { height: u64, base_index: u32 },
    /// V2 lottery payout `(creating block hash, 1 + ordinal)`. The one-time
    /// key is `P = P_base + tweak·G`, where `P_base` was derived at
    /// `base_index` (the original derivation index j, NOT the payout index).
    Lottery {
        height: u64,
        ordinal: u32,
        base_index: u32,
        /// Canonical 32-byte scalar `T`.
        tweak: [u8; 32],
    },
}

impl From<&StoredContext> for DerivationContext {
    fn from(context: &StoredContext) -> Self {
        match context {
            StoredContext::Direct { height, base_index } => Self::Direct {
                height: *height,
                base_index: *base_index,
            },
            StoredContext::Lottery {
                height,
                ordinal,
                context,
            } => Self::Lottery {
                height: *height,
                ordinal: *ordinal,
                base_index: context.base_index,
                tweak: context.tweak,
            },
        }
    }
}

impl Ledger {
    /// The persisted output `id` and its derivation context, read in ONE
    /// transaction (public read-only accessor, #1286 P4).
    ///
    /// Strict exactly like `get_derivation_context`: a missing context is
    /// [`LedgerError::MissingContext`] (never defaulted), and an
    /// output/context disagreement or corruption is an error.
    pub fn get_output_derivation_context(
        &self,
        id: &UtxoId,
    ) -> Result<(Utxo, DerivationContext), LedgerError> {
        let txn = self.env.read_txn().map_err(db_op("context read txn"))?;
        read_context(self, &txn, id).map(|(utxo, c)| (utxo, DerivationContext::from(&c)))
    }

    /// The block at `height` and, for each of its lottery payouts in order
    /// (outpoint `(block hash, 1 + i)`), the payout UTXO and its derivation
    /// context, all read in ONE transaction (#1286 P4, the
    /// `chain_getOutputs` path).
    ///
    /// Strict: the block is decoded once and every payout read applies the
    /// full `get_derivation_context` checks, with the creating-block check
    /// made against that decoded block. A payout without a context is
    /// [`LedgerError::MissingContext`] (on `botho.ledger.2` that can only be
    /// corruption). A missing block is [`LedgerError::BelowSnapshotBase`]
    /// strictly between genesis and the snapshot base of a ledger restored
    /// from a V2 snapshot (#1646: the block was never held), otherwise
    /// [`LedgerError::BlockNotFound`] (past the tip).
    pub fn get_block_with_payout_contexts(
        &self,
        height: u64,
    ) -> Result<(Block, Vec<(Utxo, DerivationContext)>), LedgerError> {
        let txn = self.env.read_txn().map_err(db_op("payout read txn"))?;
        let Some(bytes) = self
            .blocks_db
            .get(&txn, &height)
            .map_err(db_op("read block"))?
        else {
            return Err(self.missing_block_error(&txn, height)?);
        };
        let block: Block = crate::ledger::experimental::decode(bytes)?;
        let hash = block.hash();
        let payouts = (0..block.lottery_outputs.len())
            .map(|i| {
                let id = UtxoId::new(hash, (i as u32) + 1);
                read_context_with(self, &txn, &id, &mut |h| {
                    if h == height {
                        Ok(Some(hash))
                    } else {
                        Err(LedgerError::InconsistentRecord(
                            "lottery context creating block".into(),
                        ))
                    }
                })
                .map(|(utxo, c)| (utxo, DerivationContext::from(&c)))
            })
            .collect::<Result<_, _>>()?;
        Ok((block, payouts))
    }

    /// TEST/FIXTURE ONLY: delete the derivation context of `id`, simulating
    /// a corrupted `botho.ledger.2` store. Gated like
    /// [`Ledger::seed_fixture_snapshot_unvalidated`].
    #[cfg(any(test, feature = "test-fixtures"))]
    #[doc(hidden)]
    pub fn fixture_delete_derivation_context(&self, id: &UtxoId) -> Result<bool, LedgerError> {
        let mut txn = self.env.write_txn().map_err(db_op("fixture write txn"))?;
        let deleted = self
            .contexts_db
            .delete(&mut txn, &id.to_bytes())
            .map_err(db_op("fixture delete context"))?;
        txn.commit().map_err(db_op("fixture commit"))?;
        Ok(deleted)
    }
}

/// Production adapter for the shared V2 rule module: every read comes from
/// one pinned transaction, and every read is strict.
pub(in crate::ledger) struct ProductionView<'a, 'env> {
    ledger: &'a Ledger,
    txn: &'a RoTxn<'env>,
}

impl ProductionView<'_, '_> {
    fn tip_hash(&self) -> Result<[u8; 32], LedgerError> {
        fixed(
            self.ledger
                .meta_db
                .get(self.txn, META_TIP_HASH)
                .map_err(db_op("view tip"))?
                .ok_or_else(|| LedgerError::InconsistentRecord("missing tip hash".into()))?,
        )
    }
    fn height(&self) -> Result<u64, LedgerError> {
        let bytes = self
            .ledger
            .meta_db
            .get(self.txn, META_HEIGHT)
            .map_err(db_op("view height"))?
            .ok_or_else(|| LedgerError::InconsistentRecord("missing height".into()))?;
        Ok(u64::from_le_bytes(fixed(bytes)?))
    }
}

impl DerivationReads for ProductionView<'_, '_> {
    fn genesis_hash(&self) -> Result<[u8; 32], LedgerError> {
        let bytes = self
            .ledger
            .blocks_db
            .get(self.txn, &0)
            .map_err(db_op("view genesis"))?
            .ok_or_else(|| LedgerError::InconsistentRecord("missing genesis block".into()))?;
        let genesis: Block = crate::ledger::experimental::decode(bytes)?;
        if genesis.height() != 0 {
            return Err(LedgerError::InconsistentRecord("genesis key/height".into()));
        }
        Ok(genesis.hash())
    }
    fn lottery_pool(&self) -> Result<u128, LedgerError> {
        match self
            .ledger
            .meta_db
            .get(self.txn, META_LOTTERY_POOL)
            .map_err(db_op("view lottery pool"))?
        {
            Some(bytes) => Ok(u128::from_le_bytes(fixed(bytes)?)),
            // Genesis initialization does not write the pool; every applied
            // block does. Absence is only legitimate before the first block.
            None if self.height()? == 0 => Ok(0),
            None => Err(LedgerError::InconsistentRecord(
                "missing lottery pool after genesis".into(),
            )),
        }
    }
    fn output_context(&self, id: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError> {
        read_context(self.ledger, self.txn, id)
    }
    fn lottery_candidates(
        &self,
        block: &Block,
        config: &LotteryDrawConfig,
    ) -> Result<Vec<LotteryCandidate>, LedgerError> {
        validation::lottery_candidates(
            &self.ledger.write_tables(),
            self.txn,
            block.height(),
            &block.header.prev_block_hash,
            config,
            ReadPolicy::Strict,
            &mut |u| read_context(self.ledger, self.txn, &u.id).map(|_| ()),
        )
    }
}

impl ValidationReads for ProductionView<'_, '_> {
    fn get_utxo_by_target_key(&self, key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError> {
        let Some(bytes) = self
            .ledger
            .address_index_db
            .get(self.txn, key)
            .map_err(db_op("view target index"))?
        else {
            return Ok(None);
        };
        if bytes.is_empty() || bytes.len() % 36 != 0 {
            return Err(LedgerError::StorageEncoding("target index width".into()));
        }
        let id = UtxoId::from_bytes(&bytes[..36])
            .ok_or_else(|| LedgerError::StorageEncoding("target index".into()))?;
        let (utxo, _) = read_context(self.ledger, self.txn, &id)?;
        if &utxo.output.target_key != key {
            return Err(LedgerError::InconsistentRecord("target index key".into()));
        }
        Ok(Some(utxo))
    }
    fn get_cluster_wealth(&self, id: u64) -> Result<u128, LedgerError> {
        self.ledger
            .cluster_wealth_db
            .get(self.txn, &id.to_le_bytes())
            .map_err(db_op("view cluster wealth"))?
            .map(decode_cluster_wealth)
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
    fn is_bridge_import_cluster(&self, id: u64) -> Result<bool, LedgerError> {
        match self
            .ledger
            .bridge_import_clusters_db
            .get(self.txn, &id.to_le_bytes())
            .map_err(db_op("view import cluster"))?
        {
            None => Ok(false),
            Some([]) => Ok(true),
            Some(_) => Err(LedgerError::StorageEncoding("import value".into())),
        }
    }
    fn is_key_image_spent(&self, key: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        self.ledger
            .key_images_db
            .get(self.txn, key)
            .map_err(db_op("view key image"))?
            .map(|b| fixed(b).map(u64::from_le_bytes))
            .transpose()
    }
}
