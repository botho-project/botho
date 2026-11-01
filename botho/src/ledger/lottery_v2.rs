//! Shared LotteryV2 (independently keyed payout) rule module (#1286, P1).
//!
//! One implementation of the V2 lottery rules: the canonical drawing, award
//! and domain construction, payout record derivation (producer) and
//! validation (validator), the committed body `root()`, and the persisted
//! per-output [`StoredContext`]. It is generic over [`ValidationReads`] plus a
//! [`DerivationReads`] adapter, so the private experimental ledger and (from
//! P2/P3) the production ledger share these rules instead of forking them.
//!
//! ACTIVE from protocol 7.0.0 (#1286 P3, #1640): the production ledger
//! applies every block through [`validate_block`] (validator) and builds every
//! block body through [`derive`] (producer, `Ledger::seal_block`), both
//! against the pinned `store::v2::ProductionView`. The header's `tx_root` is
//! the V2 body [`root`] from genesis on fresh chains; V1 payouts (which copy
//! the winner's one-time target) are invalid. The private experimental
//! ledger keeps calling [`validate`] with its persisted records.
use super::{
    store::validation::{RootRule, ValidationReads},
    ChainState, LedgerError,
};
use crate::{
    block::{Block, BlockHeader, BlockLotterySummary, LotteryOutput},
    consensus::{
        lottery::{
            compute_pool_accounting, draw_lottery_winners_seeded, lottery_seed_v2, reward_cap,
            BlockLotteryResult, LotteryPoolAccounting,
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
    /// The header of the committed tip of this pinned view (#1676: the
    /// parent whose RandomX PoW output seeds the next draw). Strict: a
    /// missing or undecodable tip block, or one that disagrees with the
    /// view's tip height/hash, is an error, never a default.
    fn tip_header(&self) -> Result<BlockHeader, LedgerError>;
    /// Canonical, strictly-read lottery candidate set for `block`, windowed
    /// by the block's PoW-bound lottery `seed` (see [`seed`]).
    fn lottery_candidates(
        &self,
        block: &Block,
        seed: &[u8; 32],
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

/// The PoW-bound lottery seed of `block` (#1676): [`lottery_seed_v2`] over
/// the PoW fields of the committed parent (its RandomX output and its
/// preimage's `prev_block_hash`), read strictly from the pinned view and
/// recomputed from the stored parent header (RandomX light mode; a pure
/// function of the header, memoized by exact input in `crate::pow`). The
/// parent's header hash, which a minter can re-roll without new PoW via the
/// timestamp or coinbase output, is never a seed input.
///
/// The parent is the pinned view's committed tip: `block` must sit at
/// `tip.height + 1` and name `tip.hash()` as `prev_block_hash`, otherwise it
/// is invalid (wrong height/parent) — checked here, before any seed is
/// derived, so a peer block on a wrong parent is classified as invalid rather
/// than as missing storage. Genesis has no drawing; height 1 seeds from the
/// fixed genesis header's (deterministic, unmined) PoW output.
pub(in crate::ledger) fn seed<R: DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
) -> Result<[u8; 32], LedgerError> {
    let parent = reads.tip_header()?;
    if parent.height.checked_add(1) != Some(block.height())
        || parent.hash() != block.header.prev_block_hash
    {
        return Err(invalid("V2 seed parent"));
    }
    Ok(lottery_seed_v2(
        &parent.pow_hash(),
        &parent.prev_block_hash,
        block.height(),
    ))
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
    let seed = seed(reads, block)?;
    let candidates = reads.lottery_candidates(block, &seed, &config.draw_config)?;
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
    let draw = draw_lottery_winners_seeded(
        &candidates,
        fees,
        &accounting,
        block.height(),
        &seed,
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
    Ok(domain_for(
        reads.genesis_hash()?,
        block,
        manifest,
        ordinal,
        amount,
    ))
}

/// The payout derivation [`Domain`] of award `ordinal` in `block` under the
/// chain whose genesis hash is `genesis`. The one construction shared by the
/// producer/validator ([`domain`]) and the detached witness verifier
/// (`store::v2::detached`, #1499), which has no ledger to read genesis from.
pub(in crate::ledger) fn domain_for(
    genesis: [u8; 32],
    block: &Block,
    manifest: [u8; 32],
    ordinal: usize,
    amount: u64,
) -> Domain {
    Domain {
        genesis,
        parent: block.header.prev_block_hash,
        height: block.height(),
        ordinary_root: Block::compute_tx_root(&block.transactions),
        manifest,
        ordinal: ordinal as u32,
        amount,
    }
}

/// One canonical drawing: the winners plus the pool accounting.
type Drawing = (BlockLotteryResult, LotteryPoolAccounting);

/// Full V2 block validation against the pinned pre-state: the shared ordinary
/// checks (with the recomputed V2 body root), then the drawing, summary and
/// every payout record. Returns the new lottery pool carryover.
///
/// Callers must first check the block/record envelope shape (record count,
/// record/output agreement, [`bind_outputs`]); this does not.
pub(in crate::ledger) fn validate<R: ValidationReads + DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
    records: &[Record],
    state: &ChainState,
) -> Result<u128, LedgerError> {
    validate_drawn(reads, block, records, state, None)
}

/// A drawing the production validator already computed over the SAME pinned
/// pre-state, AFTER running the ordinary header prefix
/// (`ValidationReads::validate_ordinary_header`) against the same `state`.
struct Prechecked(Drawing);

/// [`validate`], optionally reusing a drawing already computed over the SAME
/// pinned pre-state (the production validator draws once to reconstruct the
/// records, and must not pay for a second full candidate scan). With
/// `prechecked`, the header prefix already ran (before the draw), so only the
/// ordinary body checks run here; the full ordinary order is unchanged.
fn validate_drawn<R: ValidationReads + DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
    records: &[Record],
    state: &ChainState,
    prechecked: Option<Prechecked>,
) -> Result<u128, LedgerError> {
    // Root comes from independent decoding and recomputation, never from a
    // supplied validation token. All ordinary checks retain their order.
    let root = RootRule::BodyV2(root(block, records)?);
    let (draw, accounting) = match prechecked {
        Some(Prechecked(drawn)) => {
            reads.validate_ordinary_body(block, state, root)?;
            drawn
        }
        None => {
            reads.validate_ordinary_block(block, state, root)?;
            drawing(reads, block)?
        }
    };
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

/// Envelope shape: the block's wire payouts are exactly `records` (count,
/// bound, ordinal order, winner, amount, target, public key, ciphertext).
/// The per-record derivation context is not on the wire; it is committed
/// only through the body root. Returns a static reason on mismatch so each
/// caller can classify it (a peer's invalid block in production, a corrupt
/// envelope in the experimental store).
pub(in crate::ledger) fn bind_outputs(
    block: &Block,
    records: &[Record],
) -> Result<(), &'static str> {
    if records.len() > CANDIDATE_MAX_AWARDS || records.len() != block.lottery_outputs.len() {
        return Err("payout record count");
    }
    for (i, (r, o)) in records.iter().zip(&block.lottery_outputs).enumerate() {
        if r.encode().is_err()
            || r.ordinal as usize != i
            || r.winner.hash != o.winner_tx_hash
            || r.winner.index != o.winner_output_index
            || r.amount != o.payout
            || r.target != o.target_key
            || r.public_key != o.public_key
            || r.ciphertext != o.kem_ciphertext
        {
            return Err("payout envelope/record mismatch");
        }
    }
    Ok(())
}

/// Production V2 validator (#1640): the block carries only the wire payouts,
/// so the validator draws ONCE over the pinned pre-state, re-derives the
/// canonical payout records (target and derivation context) exactly as the
/// producer did, requires the block's payouts to equal them
/// ([`bind_outputs`]), and then runs the full [`validate`] checks (ordinary
/// rules with the recomputed V2 body root, summary and every record) on that
/// same drawing. Returns the records (for the writer's derivation contexts)
/// and the new lottery pool carryover.
///
/// A V1-shaped payout (the winner's target, key and ciphertext copied
/// verbatim) can never equal its V2 record, whose target is tweaked, so it is
/// rejected here.
///
/// The header-only ordinary checks (height, parent hash, minting-tx/header
/// agreement, coinbase ciphertext shape, difficulty, PoW) run FIRST (#1670
/// §1): a gossiped block on the wrong parent or without PoW is rejected with
/// its precise error before the full candidate scan and the per-award
/// derivations, so it cannot amplify into a scan.
pub(in crate::ledger) fn validate_block<R: ValidationReads + DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
    state: &ChainState,
) -> Result<(Vec<Record>, u128), LedgerError> {
    reads.validate_ordinary_header(block, state)?;
    let drawn = drawing(reads, block)?;
    let records = records_for(reads, block, &drawn.0)?;
    bind_outputs(block, &records).map_err(|reason| invalid(format!("V2 payouts: {reason}")))?;
    let pool = validate_drawn(reads, block, &records, state, Some(Prechecked(drawn)))?;
    Ok((records, pool))
}

/// The canonical payout records of `draw` for `block` over the pinned
/// pre-state: one [`v2::derive`] per award, in draw order. Only
/// `block.transactions` (ordinary root) and its height/parent feed the
/// derivation domain; the block's own payouts, body root and hash never do.
fn records_for<R: DerivationReads + ?Sized>(
    reads: &R,
    block: &Block,
    draw: &BlockLotteryResult,
) -> Result<Vec<Record>, LedgerError> {
    let awards = awards(draw)?;
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
            &domain(reads, block, manifest, i, a.amount)?,
        )
        .map_err(crypto)?;
        // Only the one-time target is new (tweaked). The DH public key and
        // the ML-KEM ciphertext are inherited from the original derivation by
        // construction: the owner recovers the base key with them at the
        // original index, then adds the committed tweak (#1286 design §1).
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
    Ok(records)
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
    let records = records_for(reads, &block, &draw)?;
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

/// Parity pins for the faithful reward-cap sweep (#1682).
///
/// `cluster-tax/src/simulation/lottery_reward_cap_faithful.rs` mirrors
/// [`reward_cap`], [`compute_pool_accounting`], `MAX_LOTTERY_CANDIDATES`,
/// [`canonical_config`] and the mainnet schedule because `cluster-tax`
/// cannot depend on `botho`. These tests assert the same golden table
/// (`PARITY_VECTORS`) and constants against the real node code, so a change
/// on either side fails CI. Test-only: no consensus behaviour changes.
#[cfg(test)]
mod faithful_sweep_parity {
    use super::*;
    use bth_cluster_tax::TagVector;

    /// Same rows as `lottery_reward_cap_faithful::PARITY_VECTORS`: count,
    /// reward, fees, emission, stored → cap, fee_pool, payout.
    const PARITY_VECTORS: [(u64, u64, u64, u64, u128, u64, u64, u64); 5] = [
        (
            3,
            50_000_000_000_000,
            0,
            0,
            10_000_000_000_000,
            750_000_000_000,
            0,
            750_000_000_000,
        ),
        (
            1_000_000,
            25_000_000_000_000,
            100_000_000,
            2_500_000_000_000,
            0,
            25_000_000_000_000,
            80_000_000,
            2_500_080_000_000,
        ),
        (
            9_281,
            25_000_000_000_000,
            100_000_000,
            2_500_000_000_000,
            1_000_000_000_000_000,
            25_000_000_000_000,
            80_000_000,
            25_000_000_000_000,
        ),
        (
            46_405,
            50_000_000_000_000,
            1_000_000_000,
            0,
            0,
            50_000_000_000_000,
            800_000_000,
            800_000_000,
        ),
        (
            10_000,
            3_000_000_000_000_000,
            0,
            0,
            u128::MAX,
            2_500_000_000_000_000,
            0,
            2_500_000_000_000_000,
        ),
    ];

    /// Mirrored in the sim as `MAX_LOTTERY_CANDIDATES`.
    const SIM_MAX_LOTTERY_CANDIDATES: usize = 10_000;

    #[test]
    fn faithful_sweep_parity_reward_cap_and_accounting() {
        let config = canonical_config();
        let height = 20_000u64;
        let tags = TagVector::new();
        for (count, reward, fees, emission, stored, cap, fee_pool, payout) in PARITY_VECTORS {
            // The candidate walk stops at MAX_LOTTERY_CANDIDATES eligible
            // UTXOs, so reward_cap only ever sees the truncated set.
            let n = (count as usize).min(SIM_MAX_LOTTERY_CANDIDATES);
            let candidates: Vec<LotteryCandidate> = (0..n)
                .map(|i| {
                    let mut id = [0u8; 36];
                    id[..8].copy_from_slice(&(i as u64).to_le_bytes());
                    LotteryCandidate::new(id, 1_000_000, 1000, &tags, height - 5_000)
                })
                .collect();
            assert_eq!(reward_cap(&candidates, height, reward, &config), cap);
            let acc = compute_pool_accounting(fees, emission, stored, cap, &config);
            assert_eq!((acc.fee_pool, acc.payout), (fee_pool, payout));
        }
    }

    #[test]
    fn faithful_sweep_parity_constants() {
        let config = canonical_config();
        assert_eq!(config.pool_fraction_permille, 800);
        assert_eq!(config.draw_config.winners_per_draw, 4);
        assert_eq!(config.draw_config.min_utxo_age, 720);
        assert_eq!(config.draw_config.circulation_window, 10_000);
        assert_eq!(config.draw_config.min_utxo_value, 1_000_000);
        let src = include_str!("store/validation.rs");
        assert!(
            src.contains(&format!(
                "const MAX_LOTTERY_CANDIDATES: usize = {};",
                "10_000"
            )),
            "MAX_LOTTERY_CANDIDATES changed: update the #1682 faithful sweep"
        );
        assert_eq!(
            crate::consensus::ConsensusConfig::default().max_txs_per_slot,
            100
        );
        let p = crate::monetary::mainnet_policy();
        let interval = p.halving_interval;
        assert_eq!(interval, 6_307_200);
        assert_eq!(p.halving_count, 5);
        assert_eq!(p.halving_reward(interval / 2), Some(50_000_000_000_000));
        assert_eq!(
            p.lottery_emission_share(interval + interval / 2, 25_000_000_000_000),
            2_500_000_000_000
        );
        assert_eq!(p.tail_inflation_bps, 200);
        assert_eq!(p.expected_fee_burn_rate_bps, 50);
        assert_eq!(p.target_block_time_secs, 5);
    }
}
