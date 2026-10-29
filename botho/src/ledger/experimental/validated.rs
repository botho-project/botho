//! Inactive, private producer/validator boundary. Only tests open this local
//! chain. Disk schema/rules, not a peer header version, select these rules.
use super::*;
#[cfg(test)]
use crate::ledger::lottery_v2::canonical_config;
use crate::{
    consensus::BlockBuilder,
    ledger::{
        // V2 rules are shared with the production ledger (#1637); this
        // boundary only adapts its pinned store to the rule module's reads.
        lottery_v2::{self as rules, crypto, fixed, invalid, root, DerivationReads},
        store::validation::{self, ReadPolicy, ValidationReads},
        ChainState,
    },
};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig};
use bth_transaction_clsag::lottery_v2::{self as v2, Outpoint};

const VALIDATED_SCHEMA: &[u8] = b"botho.experimental.lottery-v2.validated.1";
const RULES: &[u8] = b"local-v2-candidate4-canonical-lottery-v1-fees-easy-pow-1";
const GENESIS: &[u8] = b"experimental_genesis";
const RULES_KEY: &[u8] = b"experimental_rules";

struct ValidatedStore {
    store: ExperimentalStore,
}
struct PinnedView<'a, 'env> {
    store: &'a ExperimentalStore,
    txn: &'a RoTxn<'env>,
}

fn local_genesis() -> Envelope {
    let mut block = Block::genesis();
    block.header.tx_root = root(&block, &[]).expect("empty local genesis root");
    Envelope {
        block,
        records: vec![],
    }
}
impl ValidatedStore {
    #[cfg(test)]
    fn open(path: &std::path::Path, fresh: bool) -> Result<Self, LedgerError> {
        let genesis = local_genesis();
        let store =
            ExperimentalStore::open_local(path, fresh, VALIDATED_SCHEMA, |env, txn, meta| {
                let blocks = env
                    .open_database::<U64<heed::byteorder::LE>, Bytes>(txn, Some("blocks"))
                    .map_err(db)?
                    .ok_or_else(|| inconsistent("blocks"))?;
                blocks.put(txn, &0, &genesis.encode()?).map_err(db)?;
                meta.put(txn, GENESIS, &genesis.block.hash()).map_err(db)?;
                meta.put(txn, RULES_KEY, RULES).map_err(db)?;
                meta.put(txn, b"tip_hash", &genesis.block.hash())
                    .map_err(db)?;
                meta.put(txn, b"difficulty", &u64::MAX.to_le_bytes())
                    .map_err(db)?;
                let mut checkpoint = 0u64.to_le_bytes().to_vec();
                checkpoint.extend(genesis.block.hash());
                meta.put(txn, CHECKPOINT, &checkpoint).map_err(db)?;
                Ok(())
            })?;
        let result = Self { store };
        let txn = result.store.env.read_txn().map_err(db)?;
        result.view(&txn).state()?;
        drop(txn);
        Ok(result)
    }
    fn view<'a, 'env>(&'a self, txn: &'a RoTxn<'env>) -> PinnedView<'a, 'env> {
        PinnedView {
            store: &self.store,
            txn,
        }
    }
    fn produce(&self, block: Block) -> Result<Envelope, LedgerError> {
        let txn = self.store.env.read_txn().map_err(db)?;
        BlockBuilder::apply_lottery_v2(block, &self.view(&txn))
    }
    /// Same trusted local emission-update contract as V1; not peer-supplied
    /// metadata. None preserves the existing six counters unchanged.
    fn apply(
        &self,
        e: &Envelope,
        emission: Option<EmissionStateUpdate>,
        mut after: impl FnMut() -> Result<(), LedgerError>,
    ) -> Result<(std::time::Duration, std::time::Duration), LedgerError> {
        let started = std::time::Instant::now();
        let mut write = self.store.env.write_txn().map_err(db)?;
        // Pin only AFTER acquiring the writer: no intervening commit can change
        // the validated pre-state. The signature callback also sees this view.
        let read = self.store.env.read_txn().map_err(db)?;
        let view = self.view(&read);
        let state = view.state()?;
        let new_pool = view.validate(e, &state)?;
        let validated = started.elapsed();
        let persistence = std::time::Instant::now();
        let bytes = e.encode()?;
        self.store.tables.block_effects(
            &mut write,
            &e.block,
            &state,
            new_pool,
            emission,
            &bytes,
            Representation::Experimental {
                contexts: self.store.contexts,
                records: &e.records,
            },
            &mut |tx| view.verify_transaction(tx),
            &mut after,
        )?;
        let mut checkpoint = e.block.height().to_le_bytes().to_vec();
        checkpoint.extend(e.block.hash());
        self.store
            .meta
            .put(&mut write, CHECKPOINT, &checkpoint)
            .map_err(db)?;
        after()?;
        drop(read);
        write.commit().map_err(db)?;
        Ok((validated, persistence.elapsed()))
    }
}
impl PinnedView<'_, '_> {
    fn metadata(&self, key: &[u8]) -> Result<&[u8], LedgerError> {
        self.store
            .meta
            .get(self.txn, key)
            .map_err(db)?
            .ok_or_else(|| {
                inconsistent(format!("missing metadata {}", String::from_utf8_lossy(key)))
            })
    }
    fn u64(&self, key: &[u8]) -> Result<u64, LedgerError> {
        Ok(u64::from_le_bytes(fixed(self.metadata(key)?)?))
    }
    fn u128(&self, key: &[u8]) -> Result<u128, LedgerError> {
        Ok(u128::from_le_bytes(fixed(self.metadata(key)?)?))
    }
    fn state(&self) -> Result<ChainState, LedgerError> {
        if self.metadata(SCHEMA_KEY)? != VALIDATED_SCHEMA || self.metadata(RULES_KEY)? != RULES {
            return Err(inconsistent("validated schema/rules"));
        }
        let genesis = local_genesis();
        if self.metadata(GENESIS)? != genesis.block.hash()
            || self.store.envelope(self.txn, 0)?.encode()? != genesis.encode()?
        {
            return Err(inconsistent("genesis identity"));
        }
        let height = self.u64(b"height")?;
        // Shared V1 height addition is safe in this admitted experimental state.
        if height == u64::MAX {
            return Err(inconsistent("height exhausted"));
        }
        let tip = self.store.envelope(self.txn, height)?;
        let tip_hash = fixed(self.metadata(b"tip_hash")?)?;
        if tip_hash != tip.block.hash()
            || root(&tip.block, &tip.records)? != tip.block.header.tx_root
        {
            return Err(inconsistent("tip identity/root"));
        }
        let mut checkpoint = height.to_le_bytes().to_vec();
        checkpoint.extend(tip_hash);
        if self.metadata(CHECKPOINT)? != checkpoint {
            return Err(inconsistent("checkpoint"));
        }
        let _pool = self.u128(b"lottery_pool")?;
        Ok(ChainState {
            height,
            tip_hash,
            tip_timestamp: tip.block.header.timestamp,
            total_mined: self.u128(b"total_mined")?,
            total_fees_burned: self.u128(b"fees_burned")?,
            difficulty: self.u64(b"difficulty")?,
            total_tx: self.u64(b"total_tx")?,
            epoch_tx: self.u64(b"epoch_tx")?,
            epoch_emission: self.u64(b"epoch_emission")?,
            epoch_burns: self.u64(b"epoch_burns")?,
            current_reward: self.u64(b"current_reward")?,
        })
    }
    fn output(&self, id: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError> {
        let result = self.store.read(self.txn, id)?;
        // The shared writer materializes each strictly positive floored tag
        // contribution. Absence is legitimate for zero/round-to-zero tags,
        // but cannot stand for zero global wealth for this persisted output.
        for tag in &result.0.output.cluster_tags.entries {
            let contribution = result.0.output.amount as u128 * tag.weight as u128
                / bth_transaction_types::TAG_WEIGHT_SCALE as u128;
            if contribution > 0 {
                let bytes = self
                    .store
                    .tables
                    .cluster_wealth_db
                    .get(self.txn, &tag.cluster_id.0.to_le_bytes())
                    .map_err(db)?
                    .ok_or_else(|| inconsistent("missing positive cluster wealth"))?;
                let _: [u8; 16] = fixed(bytes)?;
            }
        }
        let creating = self.store.envelope(self.txn, result.0.created_at)?;
        if root(&creating.block, &creating.records)? != creating.block.header.tx_root {
            return Err(inconsistent("creating body commitment"));
        }
        Ok(result)
    }
    fn source(&self, id: &Outpoint) -> Result<(Utxo, StoredContext), LedgerError> {
        self.output(&UtxoId::new(id.hash, id.index))
    }
    fn validate(&self, e: &Envelope, state: &ChainState) -> Result<u128, LedgerError> {
        e.check()?;
        rules::validate(self, &e.block, &e.records, state)
    }
}
impl BlockBuilder {
    // Actual fallible producer, private to this inactive boundary. Producer
    // output grants no authority to write: apply independently validates it.
    fn apply_lottery_v2(block: Block, view: &PinnedView<'_, '_>) -> Result<Envelope, LedgerError> {
        let state = view.state()?;
        if block.height() != state.height + 1 || block.header.prev_block_hash != state.tip_hash {
            return Err(invalid("producer pre-state"));
        }
        let (block, records) = rules::derive(view, block)?;
        Ok(Envelope { block, records })
    }
}
impl DerivationReads for PinnedView<'_, '_> {
    fn genesis_hash(&self) -> Result<[u8; 32], LedgerError> {
        fixed(self.metadata(GENESIS)?)
    }
    fn lottery_pool(&self) -> Result<u128, LedgerError> {
        self.u128(b"lottery_pool")
    }
    fn output_context(&self, id: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError> {
        self.output(id)
    }
    fn tip_header(&self) -> Result<crate::block::BlockHeader, LedgerError> {
        // `state` checks the stored tip envelope against the tip metadata.
        let state = self.state()?;
        Ok(self.store.envelope(self.txn, state.height)?.block.header)
    }
    fn lottery_candidates(
        &self,
        block: &Block,
        seed: &[u8; 32],
        config: &LotteryDrawConfig,
    ) -> Result<Vec<LotteryCandidate>, LedgerError> {
        validation::lottery_candidates(
            &self.store.tables,
            self.txn,
            block.height(),
            seed,
            config,
            ReadPolicy::Strict,
            &mut |u| self.output(&u.id).map(|_| ()),
        )
    }
}
impl ValidationReads for PinnedView<'_, '_> {
    fn get_utxo_by_target_key(&self, key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError> {
        let Some(bytes) = self
            .store
            .tables
            .address_index_db
            .get(self.txn, key)
            .map_err(db)?
        else {
            return Ok(None);
        };
        if bytes.is_empty() || bytes.len() % 36 != 0 {
            return Err(encoding("target index width"));
        }
        let id = UtxoId::from_bytes(&bytes[..36]).ok_or_else(|| encoding("target index"))?;
        let (u, _) = self.output(&id)?;
        if &u.output.target_key != key {
            return Err(inconsistent("target index key"));
        }
        Ok(Some(u))
    }
    fn get_cluster_wealth(&self, id: u64) -> Result<u128, LedgerError> {
        self.store
            .tables
            .cluster_wealth_db
            .get(self.txn, &id.to_le_bytes())
            .map_err(db)?
            .map(|b| fixed(b).map(u128::from_le_bytes))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
    fn is_bridge_import_cluster(&self, id: u64) -> Result<bool, LedgerError> {
        match self
            .store
            .tables
            .bridge_import_clusters_db
            .get(self.txn, &id.to_le_bytes())
            .map_err(db)?
        {
            None => Ok(false),
            Some([]) => Ok(true),
            Some(_) => Err(encoding("import value")),
        }
    }
    fn is_key_image_spent(&self, key: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        self.store
            .tables
            .key_images_db
            .get(self.txn, key)
            .map_err(db)?
            .map(|b| fixed(b).map(u64::from_le_bytes))
            .transpose()
    }
}

#[cfg(test)]
mod tests;

mod wallet;
