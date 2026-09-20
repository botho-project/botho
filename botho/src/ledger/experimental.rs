//! Private, inactive durable LotteryV2 infrastructure (#1349).
//!
//! This is NOT a validated Ledger. It shares production accounting/index
//! writes, but receives unvalidated fixture transitions. Only tests can open
//! it. Future consensus integration must validate transitions before any real
//! caller exposure.
use super::{
    store::writer::{Representation, WriteTables},
    EmissionStateUpdate, LedgerError,
};
use crate::{
    block::Block,
    transaction::{Utxo, UtxoId},
};
use bincode::Options;
use bth_crypto_ring_signature::Scalar;
use bth_transaction_clsag::lottery_v2::{Context, Record, CANDIDATE_MAX_AWARDS};
use heed::{
    types::{Bytes, U64},
    Database, Env, RoTxn,
};
use serde::de::DeserializeOwned;

const SCHEMA_KEY: &[u8] = b"storage_schema";
const SCHEMA: &[u8] = b"botho.experimental.lottery-v2.storage.2";
const BLOCK_TAG: &[u8; 8] = b"BLV2\0\0\0\x01";
const CHECKPOINT: &[u8] = b"experimental_checkpoint";
// Local storage allocation bound, NOT a proposed network block-size limit.
const MAX_STORED_BLOCK: usize = 16 * 1024 * 1024;

fn db(e: heed::Error) -> LedgerError {
    LedgerError::Database(e.to_string())
}
fn encoding(s: impl ToString) -> LedgerError {
    LedgerError::StorageEncoding(s.to_string())
}
fn inconsistent(s: impl ToString) -> LedgerError {
    LedgerError::InconsistentRecord(s.to_string())
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, LedgerError> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_STORED_BLOCK as u64)
        .reject_trailing_bytes()
        .deserialize(bytes)
        .map_err(encoding)
}

/// Must run before create_database, genesis or metadata initialization.
pub(super) fn require_v1_schema(env: &Env) -> Result<(), LedgerError> {
    let txn = env.read_txn().map_err(db)?;
    let meta: Option<Database<Bytes, Bytes>> = env.open_database(&txn, Some("meta")).map_err(db)?;
    if let Some(meta) = meta {
        if let Some(marker) = meta.get(&txn, SCHEMA_KEY).map_err(db)? {
            return Err(LedgerError::UnsupportedSchema(hex::encode(marker)));
        }
    }
    txn.commit().map_err(db)
}

#[derive(Clone)]
struct Envelope {
    block: Block,
    records: Vec<Record>,
}
impl Envelope {
    fn check(&self) -> Result<(), LedgerError> {
        if self.records.len() > CANDIDATE_MAX_AWARDS
            || self.records.len() != self.block.lottery_outputs.len()
        {
            return Err(inconsistent("payout record count"));
        }
        for (i, (r, o)) in self
            .records
            .iter()
            .zip(&self.block.lottery_outputs)
            .enumerate()
        {
            r.encode().map_err(|e| encoding(format!("record: {e:?}")))?;
            if r.ordinal as usize != i
                || r.winner.hash != o.winner_tx_hash
                || r.winner.index != o.winner_output_index
                || r.amount != o.payout
                || r.target != o.target_key
                || r.public_key != o.public_key
                || r.ciphertext != o.kem_ciphertext
            {
                return Err(inconsistent("payout envelope/record mismatch"));
            }
        }
        Ok(())
    }
    fn encode(&self) -> Result<Vec<u8>, LedgerError> {
        self.check()?;
        let block = bincode::serialize(&self.block).map_err(encoding)?;
        if block.len() > MAX_STORED_BLOCK {
            return Err(encoding("block too large"));
        }
        let mut out = BLOCK_TAG.to_vec();
        out.extend((block.len() as u32).to_le_bytes());
        out.extend(block);
        out.extend((self.records.len() as u32).to_le_bytes());
        for r in &self.records {
            let bytes = r.encode().map_err(|e| encoding(format!("record: {e:?}")))?;
            out.extend((bytes.len() as u32).to_le_bytes());
            out.extend(bytes);
        }
        Ok(out)
    }
    fn decode(bytes: &[u8]) -> Result<Self, LedgerError> {
        let mut r = Reader(bytes);
        if r.take(8)? != BLOCK_TAG {
            return Err(encoding("block tag"));
        }
        let n = r.u32()? as usize;
        if n > MAX_STORED_BLOCK {
            return Err(encoding("block too large"));
        }
        let block = decode(r.take(n)?)?;
        let count = r.u32()? as usize;
        if count > CANDIDATE_MAX_AWARDS {
            return Err(encoding("award bound"));
        }
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            let n = r.u32()? as usize;
            if n > 2048 {
                return Err(encoding("record length"));
            }
            records
                .push(Record::decode(r.take(n)?).map_err(|e| encoding(format!("record: {e:?}")))?);
        }
        r.end()?;
        let result = Self { block, records };
        result.check()?;
        Ok(result)
    }
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LedgerError> {
        let (a, b) = self
            .0
            .split_at_checked(n)
            .ok_or_else(|| encoding("truncated"))?;
        self.0 = b;
        Ok(a)
    }
    fn u32(&mut self) -> Result<u32, LedgerError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, LedgerError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn end(self) -> Result<(), LedgerError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(encoding("trailing bytes"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StoredContext {
    Direct {
        height: u64,
        base_index: u32,
    },
    Lottery {
        height: u64,
        ordinal: u32,
        context: Context,
    },
}
impl StoredContext {
    fn height(&self) -> u64 {
        match self {
            Self::Direct { height, .. } | Self::Lottery { height, .. } => *height,
        }
    }
    fn derivation(&self) -> Context {
        match self {
            Self::Direct { base_index, .. } => Context {
                base_index: *base_index,
                tweak: [0; 32],
            },
            Self::Lottery { context, .. } => context.clone(),
        }
    }
    pub(super) fn encode(&self) -> Result<Vec<u8>, LedgerError> {
        let mut out = vec![match self {
            Self::Direct { .. } => 0,
            Self::Lottery { .. } => 2,
        }];
        out.extend(self.height().to_le_bytes());
        let c = self.derivation();
        if Option::<Scalar>::from(Scalar::from_canonical_bytes(c.tweak)).is_none() {
            return Err(encoding("noncanonical tweak"));
        }
        out.extend(c.base_index.to_le_bytes());
        out.extend(c.tweak);
        if let Self::Lottery { ordinal, .. } = self {
            out.extend(ordinal.to_le_bytes());
        }
        Ok(out)
    }
    fn decode(bytes: &[u8]) -> Result<Self, LedgerError> {
        let mut r = Reader(bytes);
        let tag = r.take(1)?[0];
        let height = r.u64()?;
        let base_index = r.u32()?;
        let tweak = r.take(32)?.try_into().unwrap();
        let result = match tag {
            0 if tweak == [0; 32] => Self::Direct { height, base_index },
            2 => Self::Lottery {
                height,
                ordinal: r.u32()?,
                context: Context { base_index, tweak },
            },
            _ => return Err(encoding("context tag or direct tweak")),
        };
        r.end()?;
        result.encode()?;
        Ok(result)
    }
}

/// Intentionally distinct from Ledger. No conversion or Deref to Ledger.
struct ExperimentalStore {
    env: Env,
    blocks: Database<U64<heed::byteorder::LE>, Bytes>,
    meta: Database<Bytes, Bytes>,
    utxos: Database<Bytes, Bytes>,
    contexts: Database<Bytes, Bytes>,
    tables: WriteTables,
}
impl ExperimentalStore {
    #[cfg(test)]
    fn open_fixture(path: &std::path::Path, fresh: bool) -> Result<Self, LedgerError> {
        if fresh {
            if path.exists() && std::fs::read_dir(path).map_err(encoding)?.next().is_some() {
                return Err(LedgerError::UnsupportedSchema(
                    "fresh store requires empty directory".into(),
                ));
            }
            std::fs::create_dir_all(path).map_err(encoding)?;
        } else if !path.join("data.mdb").is_file() {
            return Err(LedgerError::UnsupportedSchema(
                "reopen requires existing store".into(),
            ));
        }
        // SAFETY: test owns the directory for the Env lifetime; no overlapping opens.
        let env = unsafe {
            heed::EnvOpenOptions::new()
                .max_dbs(9)
                .map_size(1024 * 1024 * 1024)
                .open(path)
        }
        .map_err(db)?;
        if fresh {
            let mut txn = env.write_txn().map_err(db)?;
            let meta: Database<Bytes, Bytes> =
                env.create_database(&mut txn, Some("meta")).map_err(db)?;
            // Check again under the exclusive writer before any schema creation commits.
            if meta.len(&txn).map_err(db)? != 0 {
                return Err(inconsistent("fresh metadata not empty"));
            }
            meta.put(&mut txn, SCHEMA_KEY, SCHEMA).map_err(db)?;
            env.create_database::<U64<heed::byteorder::LE>, Bytes>(&mut txn, Some("blocks"))
                .map_err(db)?;
            env.create_database::<Bytes, Bytes>(&mut txn, Some("utxos"))
                .map_err(db)?;
            env.create_database::<Bytes, Bytes>(&mut txn, Some("derivation_contexts"))
                .map_err(db)?;
            for name in [
                "address_index",
                "key_images",
                "tx_index",
                "cluster_wealth",
                "bridge_import_clusters",
            ] {
                env.create_database::<Bytes, Bytes>(&mut txn, Some(name))
                    .map_err(db)?;
            }
            WriteTables::initialize_fixture_metadata(meta, &mut txn)?;
            txn.commit().map_err(db)?;
        }
        let txn = env.read_txn().map_err(db)?;
        let meta: Database<Bytes, Bytes> = env
            .open_database(&txn, Some("meta"))
            .map_err(db)?
            .ok_or_else(|| LedgerError::UnsupportedSchema("missing marker".into()))?;
        if meta.get(&txn, SCHEMA_KEY).map_err(db)? != Some(SCHEMA) {
            return Err(LedgerError::UnsupportedSchema(
                "exact experimental marker required".into(),
            ));
        }
        let blocks = env
            .open_database(&txn, Some("blocks"))
            .map_err(db)?
            .ok_or_else(|| inconsistent("missing blocks table"))?;
        let utxos = env
            .open_database(&txn, Some("utxos"))
            .map_err(db)?
            .ok_or_else(|| inconsistent("missing utxos table"))?;
        let contexts = env
            .open_database(&txn, Some("derivation_contexts"))
            .map_err(db)?
            .ok_or_else(|| inconsistent("missing contexts table"))?;
        let open_bytes = |name| {
            env.open_database(&txn, Some(name))
                .map_err(db)?
                .ok_or_else(|| inconsistent(format!("missing {name} table")))
        };
        let tables = WriteTables {
            blocks_db: blocks,
            meta_db: meta,
            utxo_db: utxos,
            address_index_db: open_bytes("address_index")?,
            key_images_db: open_bytes("key_images")?,
            tx_index_db: open_bytes("tx_index")?,
            cluster_wealth_db: open_bytes("cluster_wealth")?,
            bridge_import_clusters_db: open_bytes("bridge_import_clusters")?,
        };
        // LMDB database handles opened in this read transaction survive only
        // when it commits (dropping/aborting invalidates newly opened handles).
        txn.commit().map_err(db)?;
        Ok(Self {
            env,
            blocks,
            meta,
            utxos,
            contexts,
            tables,
        })
    }
    fn envelope(&self, txn: &RoTxn<'_>, height: u64) -> Result<Envelope, LedgerError> {
        let envelope = Envelope::decode(
            self.blocks
                .get(txn, &height)
                .map_err(db)?
                .ok_or_else(|| inconsistent("missing creating block"))?,
        )?;
        if envelope.block.height() != height {
            return Err(inconsistent("block key/height"));
        }
        Ok(envelope)
    }
    fn read(&self, txn: &RoTxn<'_>, id: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError> {
        let key = id.to_bytes();
        let u: Utxo = decode(
            self.utxos
                .get(txn, &key)
                .map_err(db)?
                .ok_or_else(|| inconsistent("missing output"))?,
        )?;
        let c = StoredContext::decode(
            self.contexts
                .get(txn, &key)
                .map_err(db)?
                .ok_or_else(|| LedgerError::MissingContext(hex::encode(key)))?,
        )?;
        if u.id != *id || u.created_at != c.height() {
            return Err(inconsistent("output identity/height"));
        }
        let e = self.envelope(txn, c.height())?;
        let expected = match &c {
            StoredContext::Direct { base_index, .. } => {
                if *base_index != id.output_index {
                    return Err(inconsistent("direct index"));
                }
                if id.tx_hash == e.block.hash() && id.output_index == 0 {
                    e.block.minting_tx.to_tx_output()
                } else {
                    e.block
                        .transactions
                        .iter()
                        .find(|t| t.hash() == id.tx_hash)
                        .and_then(|t| t.outputs.get(id.output_index as usize))
                        .cloned()
                        .ok_or_else(|| inconsistent("direct block reference"))?
                }
            }
            StoredContext::Lottery {
                ordinal, context, ..
            } => {
                let r = e
                    .records
                    .get(*ordinal as usize)
                    .ok_or_else(|| inconsistent("payout reference"))?;
                if id.tx_hash != e.block.hash()
                    || id.output_index
                        != ordinal
                            .checked_add(1)
                            .ok_or_else(|| inconsistent("ordinal overflow"))?
                    || &r.context != context
                {
                    return Err(inconsistent("payout context/reference"));
                }
                // Tags are inherited from the persisted immediate source; this checks storage
                // consistency only, not eligibility, derivation or consensus provenance.
                let source: Utxo = decode(
                    self.utxos
                        .get(txn, &UtxoId::new(r.winner.hash, r.winner.index).to_bytes())
                        .map_err(db)?
                        .ok_or_else(|| inconsistent("missing payout source"))?,
                )?;
                let source_id = UtxoId::new(r.winner.hash, r.winner.index);
                let source_context = StoredContext::decode(
                    self.contexts
                        .get(txn, &source_id.to_bytes())
                        .map_err(db)?
                        .ok_or_else(|| {
                            LedgerError::MissingContext(hex::encode(source_id.to_bytes()))
                        })?,
                )?;
                if source.id != source_id
                    || source.created_at != source_context.height()
                    || source.created_at >= c.height()
                    || source_context.derivation().base_index != context.base_index
                {
                    return Err(inconsistent("payout source context/reference"));
                }
                e.block.lottery_outputs[*ordinal as usize].to_tx_output(source.output.cluster_tags)
            }
        };
        if bincode::serialize(&u.output).map_err(encoding)?
            != bincode::serialize(&expected).map_err(encoding)?
        {
            return Err(inconsistent("stored output/creating record"));
        }
        Ok((u, c))
    }
    /// Reads every row; corruption is an error, never a skipped balance.
    fn all(&self) -> Result<Vec<(Utxo, StoredContext)>, LedgerError> {
        let txn = self.env.read_txn().map_err(db)?;
        if self.utxos.len(&txn).map_err(db)? != self.contexts.len(&txn).map_err(db)? {
            return Err(inconsistent("output/context row counts"));
        }
        let result = self
            .utxos
            .iter(&txn)
            .map_err(db)?
            .map(|row| {
                let (key, _) = row.map_err(db)?;
                if key.len() != 36 {
                    return Err(encoding("outpoint length"));
                }
                self.read(
                    &txn,
                    &UtxoId::from_bytes(key).ok_or_else(|| encoding("outpoint"))?,
                )
            })
            .collect();
        result
    }
    /// Private storage operation. Hook enables abort testing after every write;
    /// neither caller data nor successful persistence constitutes validation.
    fn persist(
        &self,
        e: &Envelope,
        after_write: impl FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        self.persist_fixture_effects(e, 0, None, after_write)
    }
    // Explicitly fixture-only supplied accounting, not consensus-derived state.
    fn persist_fixture_effects(
        &self,
        e: &Envelope,
        new_pool: u128,
        emission: Option<EmissionStateUpdate>,
        mut after_write: impl FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        let bytes = e.encode()?;
        let mut txn = self.env.write_txn().map_err(db)?;
        let height = e.block.height();
        if self.blocks.get(&txn, &height).map_err(db)?.is_some() {
            return Err(inconsistent("existing block"));
        }
        if let Some(checkpoint) = self.meta.get(&txn, CHECKPOINT).map_err(db)? {
            let mut r = Reader(checkpoint);
            let previous = r.u64()?;
            let hash = r.take(32)?;
            r.end()?;
            if previous.checked_add(1) != Some(height) || hash != e.block.header.prev_block_hash {
                return Err(inconsistent("checkpoint continuity"));
            }
        } else if height != 0 {
            return Err(inconsistent("first fixture height must be zero"));
        }
        // Source consistency remains an experimental precondition, not validation.
        for r in &e.records {
            let (source, source_context) =
                self.read(&txn, &UtxoId::new(r.winner.hash, r.winner.index))?;
            if source_context.derivation().base_index != r.context.base_index {
                return Err(inconsistent("inherited base index"));
            }
            if source.created_at >= height {
                return Err(inconsistent("source must precede payout"));
            }
        }
        let state = self.tables.fixture_accounting_state(&txn)?;
        self.tables.block_effects(
            &mut txn,
            &e.block,
            &state,
            new_pool,
            emission,
            &bytes,
            Representation::Experimental {
                contexts: self.contexts,
                records: &e.records,
            },
            // This private fixture store does NOT authenticate transfer signatures.
            // Production V1 always supplies Ledger::verify_transaction instead.
            &mut |_| Ok(()),
            &mut after_write,
        )?;
        let mut checkpoint = height.to_le_bytes().to_vec();
        checkpoint.extend(e.block.hash());
        self.meta
            .put(&mut txn, CHECKPOINT, &checkpoint)
            .map_err(db)?;
        after_write()?;
        txn.commit().map_err(db)
    }
}

#[cfg(test)]
mod tests;
