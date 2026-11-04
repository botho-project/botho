//! TEST/DRT ONLY: observations around the actual production block commit.
//!
//! Unlike `formal_drt::drawing`, this uses real LMDB, the production V2
//! validator, transaction verifier and writer. An observation is evidence,
//! not a validation token or a proof of cryptographic soundness. Callers must
//! own an otherwise idle fixture ledger while taking before/after snapshots.
//! Local storage/encoding failures are returned as errors, never rejections.

use super::{lottery_v2, ChainState, DerivationContext, Ledger, LedgerError};
use crate::{
    block::Block,
    transaction::{Utxo, UtxoId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Instant};

/// Serializable copy of the strict persisted context read, not a derivation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Context {
    Direct {
        height: u64,
        base_index: u32,
    },
    Lottery {
        height: u64,
        ordinal: u32,
        base_index: u32,
        tweak: [u8; 32],
    },
}

impl From<DerivationContext> for Context {
    fn from(value: DerivationContext) -> Self {
        match value {
            DerivationContext::Direct { height, base_index } => Self::Direct { height, base_index },
            DerivationContext::Lottery {
                height,
                ordinal,
                base_index,
                tweak,
            } => Self::Lottery {
                height,
                ordinal,
                base_index,
                tweak,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Output {
    /// Full persisted row; amount is read from this row, never reconstructed.
    pub row: Utxo,
    pub context: Context,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub state: ChainState,
    pub pool: u128,
    /// All stored rows, including spent decoys. This is NOT economic U.
    pub outputs: Vec<Output>,
    pub key_images: Vec<([u8; 32], Option<u64>)>,
}

fn encoding(error: impl std::fmt::Display) -> LedgerError {
    LedgerError::Serialization(error.to_string())
}

impl Snapshot {
    /// Binds every observed counter, row, context and watched key image.
    pub fn fingerprint(&self) -> Result<String, LedgerError> {
        Ok(hex::encode(Sha256::digest(
            bincode::serialize(self).map_err(encoding)?,
        )))
    }
}

/// Read-only snapshot. A failed strict context/database read is infrastructure
/// failure, even when the attempted block would otherwise be invalid.
pub fn snapshot(ledger: &Ledger, watched: &[[u8; 32]]) -> Result<Snapshot, LedgerError> {
    let state = ledger.get_chain_state()?;
    let pool = ledger.get_lottery_pool()?;
    let mut outputs = Vec::new();
    for u in ledger.fixture_utxos()? {
        let (row, context) = ledger.get_output_derivation_context(&u.id)?;
        outputs.push(Output {
            row,
            context: context.into(),
        });
    }
    outputs.sort_by_key(|o| o.row.id.to_bytes());
    let mut keys = watched.to_vec();
    keys.sort();
    keys.dedup();
    let key_images = keys
        .into_iter()
        .map(|key| ledger.is_key_image_spent(&key).map(|height| (key, height)))
        .collect::<Result<_, _>>()?;
    Ok(Snapshot {
        state,
        pool,
        outputs,
        key_images,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputKind {
    Coinbase,
    Ordinary { transaction: [u8; 32] },
    Lottery { ordinal: u32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatedOutput {
    pub kind: OutputKind,
    pub output: Output,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Accepted,
    Rejected { error: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub block: Block,
    pub before: Snapshot,
    pub after: Snapshot,
    pub new_outputs: Vec<CreatedOutput>,
    pub outcome: Outcome,
    /// Independent production V2 precheck, including actual header/PoW.
    /// `true` plus a rejection demonstrates the later add-block/writer gate.
    pub structure_accepted: bool,
    pub structure_error: Option<String>,
    pub precheck_us: u128,
    /// Timed actual `Ledger::add_block`, excluding observation/precheck cost.
    pub add_block_us: u128,
    pub observation_us: u128,
}

/// Call the actual V2 pre-write checks over the production LMDB read view.
/// This grants no write authority. `attempt` still calls `Ledger::add_block`,
/// which repeats validation over its own writer-pinned pre-state.
pub fn structure_check(ledger: &Ledger, block: &Block) -> Result<(), LedgerError> {
    ledger.fixture_validate_v2_structure(block)
}

pub fn lottery_maturity() -> u64 {
    lottery_v2::canonical_config().draw_config.min_utxo_age
}

/// Observe a sealed (or deliberately corrupted) candidate through the real
/// commit path. Only `is_block_fault` errors count as consensus rejections.
pub fn attempt(
    ledger: &Ledger,
    block: Block,
    watched: &[[u8; 32]],
) -> Result<Observation, LedgerError> {
    let start = Instant::now();
    let before = snapshot(ledger, watched)?;
    let before_us = start.elapsed().as_micros();
    let start = Instant::now();
    let structure_error = match structure_check(ledger, &block) {
        Ok(()) => None,
        Err(e) if e.is_block_fault() => Some(e.to_string()),
        Err(e) => return Err(e),
    };
    let precheck_us = start.elapsed().as_micros();
    let start = Instant::now();
    let outcome = match ledger.add_block(&block) {
        Ok(()) => Outcome::Accepted,
        Err(e) if e.is_block_fault() => Outcome::Rejected {
            error: e.to_string(),
        },
        Err(e) => return Err(e),
    };
    let add_block_us = start.elapsed().as_micros();
    let start = Instant::now();
    let after = snapshot(ledger, watched)?;
    let existing: BTreeMap<_, _> = before
        .outputs
        .iter()
        .map(|o| (o.row.id.to_bytes(), o))
        .collect();
    let mut kinds = BTreeMap::new();
    kinds.insert(
        UtxoId::new(block.hash(), 0).to_bytes(),
        OutputKind::Coinbase,
    );
    for (i, _) in block.lottery_outputs.iter().enumerate() {
        kinds.insert(
            UtxoId::new(block.hash(), i as u32 + 1).to_bytes(),
            OutputKind::Lottery { ordinal: i as u32 },
        );
    }
    for tx in &block.transactions {
        for (i, _) in tx.outputs.iter().enumerate() {
            kinds.insert(
                UtxoId::new(tx.hash(), i as u32).to_bytes(),
                OutputKind::Ordinary {
                    transaction: tx.hash(),
                },
            );
        }
    }
    let mut new_outputs = Vec::new();
    for output in &after.outputs {
        if !existing.contains_key(&output.row.id.to_bytes()) {
            let kind = kinds.get(&output.row.id.to_bytes()).ok_or_else(|| {
                LedgerError::InconsistentRecord("unexpected new oracle outpoint".into())
            })?;
            new_outputs.push(CreatedOutput {
                kind: kind.clone(),
                output: output.clone(),
            });
        }
    }
    Ok(Observation {
        block,
        before,
        after,
        new_outputs,
        outcome,
        structure_accepted: structure_error.is_none(),
        structure_error,
        precheck_us,
        add_block_us,
        observation_us: before_us + start.elapsed().as_micros(),
    })
}

/// Known real source outpoints supplied by the fixture signer. The node's
/// ring records cannot identify these; do not infer them from ring members.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RealInput {
    pub outpoint: UtxoId,
    pub amount: u64,
}

/// Independent economic unspent map. Spent LMDB rows remain as decoys;
/// consuming known real sources is the fixture's explicit ghost premise.
#[derive(Clone, Default)]
pub struct Ghost {
    unspent: BTreeMap<[u8; 36], u64>,
}

impl Ghost {
    pub fn value(&self) -> Result<u128, String> {
        checked_sum(self.unspent.values().map(|v| *v as u128))
    }

    /// Used for setup blocks too; only actual persisted output rows enter U.
    pub fn add_output(&mut self, row: &Utxo) -> Result<(), String> {
        if self.unspent.contains_key(&row.id.to_bytes()) {
            return Err("duplicate ghost output".into());
        }
        self.unspent.insert(row.id.to_bytes(), row.output.amount);
        Ok(())
    }

    pub fn check(&self, observation: &Observation, inputs: &[RealInput]) -> Result<(), String> {
        if observation.outcome != Outcome::Accepted {
            return Err("value flow requires an accepted production commit".into());
        }
        let mut distinct = BTreeMap::new();
        for input in inputs {
            if distinct
                .insert(input.outpoint.to_bytes(), input.amount)
                .is_some()
                || self.unspent.get(&input.outpoint.to_bytes()) != Some(&input.amount)
            {
                return Err("real input missing, already spent, duplicated or wrong amount".into());
            }
        }
        let before = &observation.before;
        let after = &observation.after;
        let reward = observation.block.minting_tx.reward as u128;
        let burn = after
            .state
            .total_fees_burned
            .checked_sub(before.state.total_fees_burned)
            .ok_or("burn counter decreased")?;
        if after
            .state
            .total_mined
            .checked_sub(before.state.total_mined)
            != Some(reward)
        {
            return Err("mined delta differs from committed reward".into());
        }
        let spent = checked_sum(inputs.iter().map(|i| i.amount as u128))?;
        let created = checked_sum(
            observation
                .new_outputs
                .iter()
                .map(|o| o.output.row.output.amount as u128),
        )?;
        if checked_sum([created, after.pool, burn])? != checked_sum([spent, reward, before.pool])? {
            return Err("observed cross-addition value flow mismatch".into());
        }
        if checked_sum([self.value()?, before.pool, before.state.total_fees_burned])?
            != before.state.total_mined
        {
            return Err("ghost pre-state accounting mismatch".into());
        }
        Ok(())
    }

    pub fn apply(&mut self, observation: &Observation, inputs: &[RealInput]) -> Result<(), String> {
        self.check(observation, inputs)?;
        let mut next = self.clone();
        for input in inputs {
            next.unspent.remove(&input.outpoint.to_bytes());
        }
        for output in &observation.new_outputs {
            next.add_output(&output.output.row)?;
        }
        let after = &observation.after;
        if checked_sum([next.value()?, after.pool, after.state.total_fees_burned])?
            != after.state.total_mined
        {
            return Err("ghost post-state accounting mismatch".into());
        }
        *self = next;
        Ok(())
    }
}

fn checked_sum(values: impl IntoIterator<Item = u128>) -> Result<u128, String> {
    values.into_iter().try_fold(0u128, |sum, value| {
        sum.checked_add(value)
            .ok_or_else(|| "oracle accounting arithmetic overflow".into())
    })
}

/// Exact compiled source fingerprint for replay receipts, including the
/// production validation/writer/signature implementations and this oracle.
pub fn source_hash() -> String {
    let mut hash = Sha256::new();
    for source in [
        include_bytes!("formal_drt_supply.rs").as_slice(),
        include_bytes!("store.rs"),
        include_bytes!("store/validation.rs"),
        include_bytes!("store/writer.rs"),
        include_bytes!("store/v2.rs"),
        include_bytes!("lottery_v2.rs"),
        include_bytes!("../block.rs"),
        include_bytes!("../../../transaction/clsag/src/lib.rs"),
    ] {
        hash.update((source.len() as u64).to_le_bytes());
        hash.update(source);
    }
    hex::encode(hash.finalize())
}
