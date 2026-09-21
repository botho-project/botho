//! Inactive, private producer/validator boundary. Only tests open this local
//! chain. Disk schema/rules, not a peer header version, select these rules.
use super::*;
use crate::{
    block::{BlockLotterySummary, LotteryOutput},
    consensus::{
        lottery::{
            compute_pool_accounting, draw_lottery_winners, reward_cap, BlockLotteryResult,
            LotteryPoolAccounting,
        },
        BlockBuilder, LotteryFeeConfig,
    },
    ledger::{
        store::validation::{self, ReadPolicy, RootRule, ValidationReads},
        ChainState,
    },
};
use bth_transaction_clsag::lottery_v2::{self as v2, Award, Domain, Outpoint, Source, Summary};

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

fn invalid(s: impl ToString) -> LedgerError {
    LedgerError::InvalidBlock(s.to_string())
}
fn crypto(e: v2::Error) -> LedgerError {
    invalid(format!("V2 binding: {e:?}"))
}
fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], LedgerError> {
    bytes.try_into().map_err(|_| encoding("state width"))
}
fn canonical_config() -> LotteryFeeConfig {
    // Deliberately does not read test environment overrides.
    LotteryFeeConfig {
        pool_fraction_permille: 800,
        draw_config: bth_cluster_tax::LotteryDrawConfig::default(),
    }
}
fn root(block: &Block, records: &[Record]) -> Result<[u8; 32], LedgerError> {
    Ok(v2::body_root(
        block.minting_tx.hash(),
        Block::compute_tx_root(&block.transactions),
        v2::payout_root(records).map_err(crypto)?,
        v2::summary_root(&Summary {
            fees: block.lottery_summary.total_fees,
            distributed: block.lottery_summary.pool_distributed,
            burned: block.lottery_summary.amount_burned,
            seed: block.lottery_summary.lottery_seed,
        }),
    ))
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
    fn drawing(
        &self,
        block: &Block,
    ) -> Result<(BlockLotteryResult, LotteryPoolAccounting), LedgerError> {
        let config = canonical_config();
        if config.draw_config.winners_per_draw > CANDIDATE_MAX_AWARDS {
            return Err(invalid("experimental award bound"));
        }
        let candidates = validation::lottery_candidates(
            &self.store.tables,
            self.txn,
            block.height(),
            &block.header.prev_block_hash,
            &config.draw_config,
            ReadPolicy::Strict,
            &mut |u| self.output(&u.id).map(|_| ()),
        )?;
        let fees = block.transactions.iter().try_fold(0u64, |sum, t| {
            sum.checked_add(t.fee).ok_or(LedgerError::FeeOverflow)
        })?;
        let cap = reward_cap(
            &candidates,
            block.height(),
            block.minting_tx.reward,
            &config,
        );
        let accounting = compute_pool_accounting(
            fees,
            block.minting_tx.lottery_emission_share(),
            self.u128(b"lottery_pool")?,
            cap,
            &config,
        );
        let draw = draw_lottery_winners(
            &candidates,
            fees,
            &accounting,
            block.height(),
            &block.header.prev_block_hash,
            &config,
        );
        if accounting.payout > 0 && !candidates.is_empty() && draw.winners.is_empty() {
            return Err(inconsistent("positive eligible draw returned no winners"));
        }
        Ok((draw, accounting))
    }
    fn awards(draw: &BlockLotteryResult) -> Result<Vec<Award>, LedgerError> {
        draw.winners
            .iter()
            .map(|w| {
                let id =
                    UtxoId::from_bytes(&w.utxo_id).ok_or_else(|| encoding("winner outpoint"))?;
                Ok(Award {
                    winner: Outpoint {
                        hash: id.tx_hash,
                        index: id.output_index,
                    },
                    amount: w.payout,
                })
            })
            .collect()
    }
    fn domain(
        &self,
        block: &Block,
        manifest: [u8; 32],
        ordinal: usize,
        amount: u64,
    ) -> Result<Domain, LedgerError> {
        Ok(Domain {
            genesis: fixed(self.metadata(GENESIS)?)?,
            parent: block.header.prev_block_hash,
            height: block.height(),
            ordinary_root: Block::compute_tx_root(&block.transactions),
            manifest,
            ordinal: ordinal as u32,
            amount,
        })
    }
    fn validate(&self, e: &Envelope, state: &ChainState) -> Result<u128, LedgerError> {
        e.check()?;
        // Root comes from independent decoding and recomputation, never from a
        // supplied validation token. All ordinary checks retain their order.
        self.validate_ordinary_block(
            &e.block,
            state,
            RootRule::Experimental(root(&e.block, &e.records)?),
        )?;
        let (draw, accounting) = self.drawing(&e.block)?;
        let awards = Self::awards(&draw)?;
        let manifest = v2::manifest(&awards).map_err(crypto)?;
        let s = &e.block.lottery_summary;
        if s.total_fees != draw.total_fees
            || s.pool_distributed != draw.pool_amount
            || s.amount_burned != draw.burn_amount
            || s.lottery_seed != draw.seed
            || awards.len() != e.records.len()
        {
            return Err(invalid("V2 draw/summary/count"));
        }
        for (i, (a, r)) in awards.iter().zip(&e.records).enumerate() {
            let (source, context) = self.source(&a.winner)?;
            if source.created_at >= e.block.height() {
                return Err(inconsistent("source height"));
            }
            let source_key = Source {
                outpoint: a.winner.clone(),
                target: source.output.target_key,
                context: context.derivation(),
            };
            let d = self.domain(&e.block, manifest, i, a.amount)?;
            r.validate(&source_key, &source.output, &d)
                .map_err(crypto)?;
        }
        Ok(accounting.carryover_after(draw.pool_amount))
    }
}
impl BlockBuilder {
    // Actual fallible producer, private to this inactive boundary. Producer
    // output grants no authority to write: apply independently validates it.
    fn apply_lottery_v2(
        mut block: Block,
        view: &PinnedView<'_, '_>,
    ) -> Result<Envelope, LedgerError> {
        let state = view.state()?;
        if block.height() != state.height + 1 || block.header.prev_block_hash != state.tip_hash {
            return Err(invalid("producer pre-state"));
        }
        let (draw, _) = view.drawing(&block)?;
        let awards = PinnedView::awards(&draw)?;
        let manifest = v2::manifest(&awards).map_err(crypto)?;
        let mut records = Vec::with_capacity(awards.len());
        for (i, a) in awards.iter().enumerate() {
            let (source, context) = view.source(&a.winner)?;
            let derived = v2::derive(
                &Source {
                    outpoint: a.winner.clone(),
                    target: source.output.target_key,
                    context: context.derivation(),
                },
                &view.domain(&block, manifest, i, a.amount)?,
            )
            .map_err(crypto)?;
            records.push(Record {
                ordinal: i as u32,
                winner: a.winner.clone(),
                amount: a.amount,
                target: derived.target,
                public_key: source.output.public_key,
                ciphertext: source.output.kem_ciphertext,
                context: derived.context,
            });
        }
        block.lottery_outputs = records
            .iter()
            .map(|r| LotteryOutput {
                winner_tx_hash: r.winner.hash,
                winner_output_index: r.winner.index,
                payout: r.amount,
                target_key: r.target,
                public_key: r.public_key,
                kem_ciphertext: r.ciphertext.clone(),
            })
            .collect();
        block.lottery_summary = BlockLotterySummary {
            total_fees: draw.total_fees,
            pool_distributed: draw.pool_amount,
            amount_burned: draw.burn_amount,
            lottery_seed: draw.seed,
        };
        block.header.tx_root = root(&block, &records)?;
        Ok(Envelope { block, records })
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
