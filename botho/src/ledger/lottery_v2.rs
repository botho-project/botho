//! Shared LotteryV2 (independently keyed payout) rule module (#1286, P1).
//!
//! One implementation of the V2 lottery rules: the canonical drawing, award
//! and domain construction, payout record derivation (producer) and
//! validation (validator), the committed body `root()`, and the persisted
//! per-output [`StoredContext`]. It is generic over [`ValidationReads`] plus a
//! [`DerivationReads`] adapter, so the private experimental ledger and (from
//! P2/P3) the production ledger share these rules instead of forking them.
//!
//! INACTIVE: nothing on the production path selects these rules yet. The
//! production ledger stores `botho.ledger.2` derivation contexts and exposes a
//! pinned [`DerivationReads`] adapter (P2, `store::v2::ProductionView`), but
//! keeps V1 block-application rules until protocol 7.0.0 activation (P3).
use super::{
    store::validation::{RootRule, ValidationReads},
    ChainState, LedgerError,
};
use crate::{
    block::{Block, BlockLotterySummary, LotteryOutput},
    consensus::{
        lottery::{
            compute_pool_accounting, draw_lottery_winners, reward_cap, BlockLotteryResult,
            LotteryPoolAccounting,
        },
        LotteryFeeConfig,
    },
    transaction::{Utxo, UtxoId},
};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig};
use bth_crypto_ring_signature::Scalar;
use bth_transaction_clsag::lottery_v2::{
    self as v2, Award, Context, Domain, Outpoint, Record, Source, Summary, CANDIDATE_MAX_AWARDS,
};

pub(in crate::ledger) fn invalid(s: impl ToString) -> LedgerError {
    LedgerError::InvalidBlock(s.to_string())
}
pub(in crate::ledger) fn crypto(e: v2::Error) -> LedgerError {
    invalid(format!("V2 binding: {e:?}"))
}
fn encoding(s: impl ToString) -> LedgerError {
    LedgerError::StorageEncoding(s.to_string())
}
fn inconsistent(s: impl ToString) -> LedgerError {
    LedgerError::InconsistentRecord(s.to_string())
}
pub(in crate::ledger) fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], LedgerError> {
    bytes.try_into().map_err(|_| encoding("state width"))
}

/// Bounded little-endian byte reader for persisted V2 encodings.
pub(in crate::ledger) struct Reader<'a>(pub(in crate::ledger) &'a [u8]);
impl<'a> Reader<'a> {
    pub(in crate::ledger) fn take(&mut self, n: usize) -> Result<&'a [u8], LedgerError> {
        let (a, b) = self
            .0
            .split_at_checked(n)
            .ok_or_else(|| encoding("truncated"))?;
        self.0 = b;
        Ok(a)
    }
    pub(in crate::ledger) fn u32(&mut self) -> Result<u32, LedgerError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub(in crate::ledger) fn u64(&mut self) -> Result<u64, LedgerError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub(in crate::ledger) fn end(self) -> Result<(), LedgerError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(encoding("trailing bytes"))
        }
    }
}

/// Persisted derivation context of one output: how its one-time key was
/// derived. Direct outputs use the zero tweak; V2 lottery payouts carry the
/// committed record context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ledger) enum StoredContext {
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
    pub(in crate::ledger) fn height(&self) -> u64 {
        match self {
            Self::Direct { height, .. } | Self::Lottery { height, .. } => *height,
        }
    }
    pub(in crate::ledger) fn derivation(&self) -> Context {
        match self {
            Self::Direct { base_index, .. } => Context {
                base_index: *base_index,
                tweak: [0; 32],
            },
            Self::Lottery { context, .. } => context.clone(),
        }
    }
    pub(in crate::ledger) fn encode(&self) -> Result<Vec<u8>, LedgerError> {
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
    pub(in crate::ledger) fn decode(bytes: &[u8]) -> Result<Self, LedgerError> {
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

/// Derivation-context reader: the pinned pre-state inputs the V2 rules need
/// beyond [`ValidationReads`]. Implementations must read one consistent
/// snapshot (the same pre-state the ordinary validation sees).
pub(in crate::ledger) trait DerivationReads {
    /// Genesis identity bound into every payout [`Domain`].
    fn genesis_hash(&self) -> Result<[u8; 32], LedgerError>;
    /// Lottery pool carryover before this block.
    fn lottery_pool(&self) -> Result<u128, LedgerError>;
    /// A persisted output and its derivation context. Strict: a missing
    /// context MUST be an explicit error ([`LedgerError::MissingContext`]),
    /// never a default (e.g. a Direct/zero-tweak context), and corruption or
    /// an output/context disagreement is an error too.
    fn output_context(&self, id: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError>;
    /// Canonical, strictly-read lottery candidate set for `block`.
    fn lottery_candidates(
        &self,
        block: &Block,
        config: &LotteryDrawConfig,
    ) -> Result<Vec<LotteryCandidate>, LedgerError>;
}

pub(in crate::ledger) fn canonical_config() -> LotteryFeeConfig {
    // Deliberately does not read test environment overrides.
    LotteryFeeConfig {
        pool_fraction_permille: 800,
        draw_config: bth_cluster_tax::LotteryDrawConfig::default(),
    }
}

/// The V2 body root committed in `header.tx_root`.
pub(in crate::ledger) fn root(block: &Block, records: &[Record]) -> Result<[u8; 32], LedgerError> {
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

fn source<R: DerivationReads + ?Sized>(
    reads: &R,
    id: &Outpoint,
) -> Result<(Utxo, StoredContext), LedgerError> {
    reads.output_context(&UtxoId::new(id.hash, id.index))
}

/// Canonical V2 drawing over the pinned pre-state.
pub(in crate::ledger) fn drawing<R: DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
) -> Result<(BlockLotteryResult, LotteryPoolAccounting), LedgerError> {
    let config = canonical_config();
    if config.draw_config.winners_per_draw > CANDIDATE_MAX_AWARDS {
        return Err(invalid("experimental award bound"));
    }
    let candidates = reads.lottery_candidates(block, &config.draw_config)?;
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
        reads.lottery_pool()?,
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

pub(in crate::ledger) fn awards(draw: &BlockLotteryResult) -> Result<Vec<Award>, LedgerError> {
    draw.winners
        .iter()
        .map(|w| {
            let id = UtxoId::from_bytes(&w.utxo_id).ok_or_else(|| encoding("winner outpoint"))?;
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

pub(in crate::ledger) fn domain<R: DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
    manifest: [u8; 32],
    ordinal: usize,
    amount: u64,
) -> Result<Domain, LedgerError> {
    Ok(Domain {
        genesis: reads.genesis_hash()?,
        parent: block.header.prev_block_hash,
        height: block.height(),
        ordinary_root: Block::compute_tx_root(&block.transactions),
        manifest,
        ordinal: ordinal as u32,
        amount,
    })
}

/// Full V2 block validation against the pinned pre-state: the shared ordinary
/// checks (with the recomputed V2 body root), then the drawing, summary and
/// every payout record. Returns the new lottery pool carryover.
///
/// Callers must first check the block/record envelope shape (record count,
/// record/output agreement); this does not.
pub(in crate::ledger) fn validate<R: ValidationReads + DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
    records: &[Record],
    state: &ChainState,
) -> Result<u128, LedgerError> {
    // Root comes from independent decoding and recomputation, never from a
    // supplied validation token. All ordinary checks retain their order.
    reads.validate_ordinary_block(block, state, RootRule::Experimental(root(block, records)?))?;
    let (draw, accounting) = drawing(reads, block)?;
    let awards = awards(&draw)?;
    let manifest = v2::manifest(&awards).map_err(crypto)?;
    let s = &block.lottery_summary;
    if s.total_fees != draw.total_fees
        || s.pool_distributed != draw.pool_amount
        || s.amount_burned != draw.burn_amount
        || s.lottery_seed != draw.seed
        || awards.len() != records.len()
    {
        return Err(invalid("V2 draw/summary/count"));
    }
    for (i, (a, r)) in awards.iter().zip(records).enumerate() {
        let (source, context) = source(reads, &a.winner)?;
        if source.created_at >= block.height() {
            return Err(inconsistent("source height"));
        }
        let source_key = Source {
            outpoint: a.winner.clone(),
            target: source.output.target_key,
            context: context.derivation(),
        };
        let d = domain(reads, block, manifest, i, a.amount)?;
        r.validate(&source_key, &source.output, &d)
            .map_err(crypto)?;
    }
    Ok(accounting.carryover_after(draw.pool_amount))
}

/// Producer: draw, derive every payout record, and fill the block's lottery
/// outputs, summary and V2 body root. The result grants no authority to
/// write; it must still pass [`validate`]. Callers must first check the
/// producer pre-state (height/parent).
pub(in crate::ledger) fn derive<R: DerivationReads + ?Sized>(
    reads: &R,
    mut block: Block,
) -> Result<(Block, Vec<Record>), LedgerError> {
    let (draw, _) = drawing(reads, &block)?;
    let awards = awards(&draw)?;
    let manifest = v2::manifest(&awards).map_err(crypto)?;
    let mut records = Vec::with_capacity(awards.len());
    for (i, a) in awards.iter().enumerate() {
        let (source, context) = source(reads, &a.winner)?;
        let derived = v2::derive(
            &Source {
                outpoint: a.winner.clone(),
                target: source.output.target_key,
                context: context.derivation(),
            },
            &domain(reads, &block, manifest, i, a.amount)?,
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
    Ok((block, records))
}
