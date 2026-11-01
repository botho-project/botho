//! The DRT legs: one per mirrored function. Each leg is one generator
//! (`strategy`), one Lean entry point (the `NAME` arm of `dispatch` in
//! `formal-drt/lean/Drt/Codec.lean`, fed by `args`) and one comparison (the
//! Rust result encoded by `rust` in the same wire format).

use crate::{
    fixtures::{Fixtures, TIP_POOL},
    gen::{self, Cand, Cfg, FeeCfg},
    wire::{self, b, list, n},
};
use botho::{
    block::{Block, BlockHeader},
    consensus::{
        draw_lottery_winners_seeded,
        lottery::{compute_pool_accounting, reward_cap, LotteryPoolAccounting},
        lottery_seed_v2, BlockLotteryResult,
    },
    ledger::{formal_drt, LedgerError},
    transaction::Transaction,
};
use bth_cluster_tax::{count_eligible, draw_winners_with_seed, sybil_reward_cap, LotteryResult};
use proptest::{collection::vec, prelude::*};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fmt::Debug};

/// One mirrored function under DRT.
pub trait Leg {
    /// The Lean `fn` name, the fixture file name and the CLI name.
    const NAME: &'static str;
    /// The Rust function(s) this leg calls.
    const RUST: &'static str;
    type Input: Clone + Debug + Serialize + DeserializeOwned;
    fn strategy() -> BoxedStrategy<Self::Input>;
    fn args(fx: &Fixtures, input: &Self::Input) -> Value;
    fn rust(fx: &Fixtures, input: &Self::Input) -> Value;
    /// A coarse outcome class of a (Rust) result, for the coverage histogram
    /// in the report: evidence that the cases reach the interesting branches.
    fn classify(_input: &Self::Input, _result: &Value) -> String {
        String::new()
    }
}

fn winners_class(r: &Value) -> String {
    match r.get("winners").and_then(Value::as_array) {
        Some(w) => format!("{} winners", w.len()),
        None => "none".into(),
    }
}

// ---- Shared encoders --------------------------------------------------------

fn acc_wire(a: &LotteryPoolAccounting) -> Value {
    json!({
        "fee_pool": n(a.fee_pool),
        "fee_burn": n(a.fee_burn),
        "emission_share": n(a.emission_share),
        "available": n(a.available),
        "payout": n(a.payout),
    })
}

fn winners_wire(ws: &[bth_cluster_tax::LotteryWinner]) -> Value {
    list(
        ws,
        |w| json!({ "utxo_id": b(&w.utxo_id), "payout": n(w.payout) }),
    )
}

fn lottery_result_wire(r: &LotteryResult) -> Value {
    json!({
        "block_height": n(r.block_height),
        "pool_amount": n(r.pool_amount),
        "winners": winners_wire(&r.winners),
        "seed": b(&r.seed),
    })
}

fn block_result_wire(r: &BlockLotteryResult) -> Value {
    json!({
        "block_height": n(r.block_height),
        "total_fees": n(r.total_fees),
        "pool_amount": n(r.pool_amount),
        "burn_amount": n(r.burn_amount),
        "winners": winners_wire(&r.winners),
        "seed": b(&r.seed),
    })
}

fn cands_wire(cs: &[Cand]) -> Value {
    list(cs, Cand::wire)
}

/// A pool accounting given directly (for the legs that take one as input).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AccIn {
    pub fee_pool: u64,
    pub fee_burn: u64,
    pub emission_share: u64,
    #[serde(with = "wire::dec_u128")]
    pub available: u128,
    pub payout: u64,
}

impl AccIn {
    fn rust(&self) -> LotteryPoolAccounting {
        LotteryPoolAccounting {
            fee_pool: self.fee_pool,
            fee_burn: self.fee_burn,
            emission_share: self.emission_share,
            available: self.available,
            payout: self.payout,
        }
    }
}

fn acc_in() -> BoxedStrategy<AccIn> {
    (
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u128_edge(),
        prop_oneof![2 => Just(0u64), 2 => 1u64..1_000, 6 => gen::u64_edge()],
    )
        .prop_map(
            |(fee_pool, fee_burn, emission_share, available, payout)| AccIn {
                fee_pool,
                fee_burn,
                emission_share,
                available,
                payout,
            },
        )
        .boxed()
}

// ---- lottery_seed_v2 --------------------------------------------------------

pub struct SeedV2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeedIn {
    #[serde(with = "wire::hex_bytes")]
    pub pow_hash: Vec<u8>,
    #[serde(with = "wire::hex_bytes")]
    pub prev_hash: Vec<u8>,
    pub height: u64,
}

impl Leg for SeedV2 {
    const NAME: &'static str = "lottery_seed_v2";
    const RUST: &'static str = "botho::consensus::lottery::lottery_seed_v2";
    type Input = SeedIn;
    fn strategy() -> BoxedStrategy<SeedIn> {
        (gen::bytes32(), gen::bytes32(), gen::height())
            .prop_map(|(pow_hash, prev_hash, height)| SeedIn {
                pow_hash,
                prev_hash,
                height,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &SeedIn) -> Value {
        json!({ "pow_hash": b(&i.pow_hash), "prev_hash": b(&i.prev_hash), "height": n(i.height) })
    }
    fn rust(_: &Fixtures, i: &SeedIn) -> Value {
        b(&lottery_seed_v2(
            &wire::arr(&i.pow_hash),
            &wire::arr(&i.prev_hash),
            i.height,
        ))
    }
}

// ---- is_eligible ------------------------------------------------------------

pub struct IsEligible;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EligIn {
    pub candidate: Cand,
    pub height: u64,
    pub config: Cfg,
}

impl Leg for IsEligible {
    const NAME: &'static str = "is_eligible";
    const RUST: &'static str = "bth_cluster_tax::LotteryCandidate::is_eligible";
    type Input = EligIn;
    fn strategy() -> BoxedStrategy<EligIn> {
        gen::height()
            .prop_flat_map(|h| (Just(h), gen::cand_at(h), gen::cfg()))
            .prop_map(|(height, candidate, config)| EligIn {
                candidate,
                height,
                config,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &EligIn) -> Value {
        json!({ "candidate": i.candidate.wire(), "height": n(i.height), "config": i.config.wire() })
    }
    fn rust(_: &Fixtures, i: &EligIn) -> Value {
        Value::Bool(i.candidate.rust().is_eligible(i.height, &i.config.rust()))
    }
    fn classify(_: &EligIn, r: &Value) -> String {
        format!("eligible={r}")
    }
}

// ---- count_eligible ---------------------------------------------------------

pub struct CountEligible;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandsIn {
    pub candidates: Vec<Cand>,
    pub height: u64,
    pub config: Cfg,
}

fn cands_in() -> BoxedStrategy<CandsIn> {
    gen::height()
        .prop_flat_map(|h| (Just(h), gen::cands_at(h), gen::cfg()))
        .prop_map(|(height, candidates, config)| CandsIn {
            candidates,
            height,
            config,
        })
        .boxed()
}

impl Leg for CountEligible {
    const NAME: &'static str = "count_eligible";
    const RUST: &'static str = "bth_cluster_tax::count_eligible";
    type Input = CandsIn;
    fn strategy() -> BoxedStrategy<CandsIn> {
        cands_in()
    }
    fn args(_: &Fixtures, i: &CandsIn) -> Value {
        json!({ "candidates": cands_wire(&i.candidates), "height": n(i.height), "config": i.config.wire() })
    }
    fn rust(_: &Fixtures, i: &CandsIn) -> Value {
        n(count_eligible(
            &gen::rust_cands(&i.candidates),
            i.height,
            &i.config.rust(),
        ))
    }
    fn classify(i: &CandsIn, r: &Value) -> String {
        let k = r
            .as_str()
            .and_then(|x| x.parse::<usize>().ok())
            .unwrap_or(0);
        let bucket = match k {
            0 => "0",
            1 => "1",
            2..=12 => "2-12",
            13..=9_998 => "13-9998",
            _ => ">=9999",
        };
        format!(
            "rho {bucket} of {}",
            if i.candidates.len() >= 9_999 {
                ">=9999"
            } else {
                "<9999"
            }
        )
    }
}

// ---- sybil_reward_cap -------------------------------------------------------

pub struct SybilCap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CountIn {
    pub count: u64,
}

impl Leg for SybilCap {
    const NAME: &'static str = "sybil_reward_cap";
    const RUST: &'static str = "bth_cluster_tax::sybil_reward_cap";
    type Input = CountIn;
    fn strategy() -> BoxedStrategy<CountIn> {
        prop_oneof![
            3 => gen::u64_edge(),
            1 => 9_998u64..=10_002,
            // ρ · base_fee crosses u64::MAX at ρ ≈ 7.4e7.
            1 => 73_786_970u64..=73_786_980,
        ]
        .prop_map(|count| CountIn { count })
        .boxed()
    }
    fn args(_: &Fixtures, i: &CountIn) -> Value {
        json!({ "count": n(i.count) })
    }
    fn rust(_: &Fixtures, i: &CountIn) -> Value {
        n(sybil_reward_cap(i.count as usize))
    }
    fn classify(_: &CountIn, r: &Value) -> String {
        let v = r.as_str().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
        if v == u128::MAX {
            "saturated".into()
        } else if v > u64::MAX as u128 {
            "above u64".into()
        } else {
            "fits u64".into()
        }
    }
}

// ---- split_fees -------------------------------------------------------------

pub struct SplitFees;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SplitIn {
    pub pool_fraction_permille: u32,
    pub total_fees: u64,
}

impl Leg for SplitFees {
    const NAME: &'static str = "split_fees";
    const RUST: &'static str = "botho::consensus::lottery::LotteryFeeConfig::split_fees";
    type Input = SplitIn;
    fn strategy() -> BoxedStrategy<SplitIn> {
        (gen::permille(), gen::u64_edge())
            .prop_map(|(pool_fraction_permille, total_fees)| SplitIn {
                pool_fraction_permille,
                total_fees,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &SplitIn) -> Value {
        json!({ "pool_fraction_permille": n(i.pool_fraction_permille), "total_fees": n(i.total_fees) })
    }
    fn rust(_: &Fixtures, i: &SplitIn) -> Value {
        let cfg = FeeCfg {
            pool_fraction_permille: i.pool_fraction_permille,
            draw: Cfg::canonical(),
        }
        .rust();
        let (pool, burn) = cfg.split_fees(i.total_fees);
        json!([n(pool), n(burn)])
    }
    fn classify(i: &SplitIn, _: &Value) -> String {
        let wide = i.total_fees as u128 * i.pool_fraction_permille as u128 / 1000;
        if wide > u64::MAX as u128 {
            "as-u64 truncates".into()
        } else {
            format!(
                "permille {}",
                if i.pool_fraction_permille == 800 {
                    "800"
                } else {
                    "other"
                }
            )
        }
    }
}

// ---- reward_cap -------------------------------------------------------------

pub struct RewardCap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapIn {
    pub candidates: Vec<Cand>,
    pub height: u64,
    pub block_reward: u64,
    pub config: FeeCfg,
}

impl Leg for RewardCap {
    const NAME: &'static str = "reward_cap";
    const RUST: &'static str = "botho::consensus::lottery::reward_cap";
    type Input = CapIn;
    fn strategy() -> BoxedStrategy<CapIn> {
        gen::height()
            .prop_flat_map(|h| {
                (
                    Just(h),
                    gen::cands_at(h),
                    prop_oneof![
                        Just(50_000_000_000_000u64),
                        // Around k · base_fee, the cap's crossover points.
                        (0u64..12, 0u64..3)
                            .prop_map(|(k, d)| (k * 250_000_000_000).saturating_sub(1) + d),
                        gen::u64_edge(),
                    ],
                    gen::fee_cfg(),
                )
            })
            .prop_map(|(height, candidates, block_reward, config)| CapIn {
                candidates,
                height,
                block_reward,
                config,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &CapIn) -> Value {
        json!({
            "candidates": cands_wire(&i.candidates),
            "height": n(i.height),
            "block_reward": n(i.block_reward),
            "config": i.config.wire(),
        })
    }
    fn rust(_: &Fixtures, i: &CapIn) -> Value {
        n(reward_cap(
            &gen::rust_cands(&i.candidates),
            i.height,
            i.block_reward,
            &i.config.rust(),
        ))
    }
    fn classify(i: &CapIn, r: &Value) -> String {
        if r.as_str() == Some(&i.block_reward.to_string()) {
            "cap = block reward".into()
        } else {
            "cap = rho * base fee".into()
        }
    }
}

// ---- compute_pool_accounting ------------------------------------------------

pub struct PoolAccounting;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolIn {
    pub total_fees: u64,
    pub emission_share: u64,
    #[serde(with = "wire::dec_u128")]
    pub stored_pool: u128,
    pub payout_cap: u64,
    pub config: FeeCfg,
}

impl Leg for PoolAccounting {
    const NAME: &'static str = "compute_pool_accounting";
    const RUST: &'static str = "botho::consensus::lottery::compute_pool_accounting";
    type Input = PoolIn;
    fn strategy() -> BoxedStrategy<PoolIn> {
        (
            gen::u64_edge(),
            gen::u64_edge(),
            gen::u128_edge(),
            gen::u64_edge(),
            gen::fee_cfg(),
        )
            .prop_map(
                |(total_fees, emission_share, stored_pool, payout_cap, config)| PoolIn {
                    total_fees,
                    emission_share,
                    stored_pool,
                    payout_cap,
                    config,
                },
            )
            .boxed()
    }
    fn args(_: &Fixtures, i: &PoolIn) -> Value {
        json!({
            "total_fees": n(i.total_fees),
            "emission_share": n(i.emission_share),
            "stored_pool": n(i.stored_pool),
            "payout_cap": n(i.payout_cap),
            "config": i.config.wire(),
        })
    }
    fn rust(_: &Fixtures, i: &PoolIn) -> Value {
        acc_wire(&compute_pool_accounting(
            i.total_fees,
            i.emission_share,
            i.stored_pool,
            i.payout_cap,
            &i.config.rust(),
        ))
    }
    fn classify(i: &PoolIn, r: &Value) -> String {
        let avail = r["available"].as_str().unwrap_or("");
        let sat = if avail == u128::MAX.to_string() {
            "available saturated"
        } else {
            "available exact"
        };
        let capped = if r["payout"].as_str() == Some(&i.payout_cap.to_string()) {
            "payout = cap"
        } else {
            "payout = available"
        };
        format!("{sat}, {capped}")
    }
}

// ---- carryover_after --------------------------------------------------------

pub struct Carryover;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryIn {
    pub accounting: AccIn,
    pub distributed: u64,
}

impl Leg for Carryover {
    const NAME: &'static str = "carryover_after";
    const RUST: &'static str = "botho::consensus::lottery::LotteryPoolAccounting::carryover_after";
    type Input = CarryIn;
    fn strategy() -> BoxedStrategy<CarryIn> {
        (acc_in(), gen::u64_edge())
            .prop_map(|(accounting, distributed)| CarryIn {
                accounting,
                distributed,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &CarryIn) -> Value {
        json!({ "accounting": acc_wire(&i.accounting.rust()), "distributed": n(i.distributed) })
    }
    fn rust(_: &Fixtures, i: &CarryIn) -> Value {
        n(i.accounting.rust().carryover_after(i.distributed))
    }
    fn classify(i: &CarryIn, _: &Value) -> String {
        if (i.distributed as u128) > i.accounting.available {
            "saturates at 0".into()
        } else {
            "exact".into()
        }
    }
}

// ---- draw_winners_with_seed -------------------------------------------------

pub struct DrawWithSeed;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrawIn {
    pub candidates: Vec<Cand>,
    pub pool_amount: u64,
    pub height: u64,
    #[serde(with = "wire::hex_bytes")]
    pub seed: Vec<u8>,
    pub config: Cfg,
}

impl Leg for DrawWithSeed {
    const NAME: &'static str = "draw_winners_with_seed";
    const RUST: &'static str = "bth_cluster_tax::draw_winners_with_seed (draw_eligible)";
    type Input = DrawIn;
    fn strategy() -> BoxedStrategy<DrawIn> {
        (cands_in(), gen::u64_edge(), gen::bytes32())
            .prop_map(|(c, pool_amount, seed)| DrawIn {
                candidates: c.candidates,
                pool_amount,
                height: c.height,
                seed,
                config: c.config,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &DrawIn) -> Value {
        json!({
            "candidates": cands_wire(&i.candidates),
            "pool_amount": n(i.pool_amount),
            "height": n(i.height),
            "seed": b(&i.seed),
            "config": i.config.wire(),
        })
    }
    fn rust(_: &Fixtures, i: &DrawIn) -> Value {
        match draw_winners_with_seed(
            &gen::rust_cands(&i.candidates),
            i.pool_amount,
            i.height,
            &wire::arr(&i.seed),
            &i.config.rust(),
        ) {
            Some(r) => lottery_result_wire(&r),
            None => Value::Null,
        }
    }
    fn classify(i: &DrawIn, r: &Value) -> String {
        let mode = match i.config.mode {
            gen::Mode::Hybrid(_) => "Hybrid".to_string(),
            m => format!("{m:?}"),
        };
        format!("{mode}: {}", winners_class(r))
    }
}

// ---- draw_lottery_winners_seeded --------------------------------------------

pub struct DrawSeeded;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeededIn {
    pub candidates: Vec<Cand>,
    pub total_fees: u64,
    pub accounting: AccIn,
    pub height: u64,
    #[serde(with = "wire::hex_bytes")]
    pub seed: Vec<u8>,
    pub config: FeeCfg,
}

impl Leg for DrawSeeded {
    const NAME: &'static str = "draw_lottery_winners_seeded";
    const RUST: &'static str =
        "botho::consensus::lottery::draw_lottery_winners_seeded (finish_draw)";
    type Input = SeededIn;
    fn strategy() -> BoxedStrategy<SeededIn> {
        (
            cands_in(),
            gen::u64_edge(),
            acc_in(),
            gen::bytes32(),
            gen::permille(),
        )
            .prop_map(|(c, total_fees, accounting, seed, permille)| SeededIn {
                candidates: c.candidates,
                total_fees,
                accounting,
                height: c.height,
                seed,
                config: FeeCfg {
                    pool_fraction_permille: permille,
                    draw: c.config,
                },
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &SeededIn) -> Value {
        json!({
            "candidates": cands_wire(&i.candidates),
            "total_fees": n(i.total_fees),
            "accounting": acc_wire(&i.accounting.rust()),
            "height": n(i.height),
            "seed": b(&i.seed),
            "config": i.config.wire(),
        })
    }
    fn rust(_: &Fixtures, i: &SeededIn) -> Value {
        block_result_wire(&draw_lottery_winners_seeded(
            &gen::rust_cands(&i.candidates),
            i.total_fees,
            &i.accounting.rust(),
            i.height,
            &wire::arr(&i.seed),
            &i.config.rust(),
        ))
    }
    fn classify(_: &SeededIn, r: &Value) -> String {
        winners_class(r)
    }
}

// ---- lottery_candidates (the seeded window over a real LMDB ledger) ---------

pub struct Window;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowIn {
    /// Index into `fixtures::LEDGERS`.
    pub ledger: usize,
    #[serde(with = "wire::hex_bytes")]
    pub seed: Vec<u8>,
    pub height: u64,
    pub config: Cfg,
}

impl Leg for Window {
    const NAME: &'static str = "lottery_candidates";
    const RUST: &'static str =
        "botho::ledger::Ledger::get_lottery_validation_candidates (store::validation::lottery_candidates)";
    type Input = WindowIn;
    fn strategy() -> BoxedStrategy<WindowIn> {
        (
            crate::fixtures::ledger_index(),
            gen::bytes32(),
            prop_oneof![
                4 => Just(crate::fixtures::WINDOW_H0),
                6 => (crate::fixtures::WINDOW_H0 - 2_000)..(crate::fixtures::WINDOW_H0 + 12_000),
                1 => gen::height(),
            ],
            prop_oneof![4 => Just(Cfg::canonical()), 1 => gen::cfg()],
        )
            .prop_map(|(ledger, seed, height, config)| WindowIn {
                ledger,
                seed,
                height,
                config,
            })
            .boxed()
    }
    fn args(fx: &Fixtures, i: &WindowIn) -> Value {
        json!({
            "rows": fx.ledger_rows_wire(i.ledger),
            "height": n(i.height),
            "seed": b(&i.seed),
            "config": i.config.wire(),
        })
    }
    fn rust(fx: &Fixtures, i: &WindowIn) -> Value {
        let cands = fx
            .ledger(i.ledger)
            .get_lottery_validation_candidates(i.height, &wire::arr(&i.seed), &i.config.rust())
            .expect("candidate read");
        list(
            &cands,
            |c| json!({ "id": b(&c.id), "value": n(c.value), "creation_block": n(c.creation_block) }),
        )
    }
    fn classify(i: &WindowIn, r: &Value) -> String {
        let k = r.as_array().map_or(0, Vec::len);
        let rows = crate::fixtures::LEDGERS[i.ledger].rows;
        let class = if k == 0 {
            "empty"
        } else if k == formal_drt::MAX_LOTTERY_CANDIDATES {
            "capped at 10000"
        } else {
            "under cap"
        };
        format!("{class} (ledger {rows} rows)")
    }
}

// ---- drawing (end to end, in-memory DerivationReads double) -----------------

pub struct Drawing;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RowIn {
    #[serde(with = "wire::hex_bytes")]
    pub id: Vec<u8>,
    pub amount: u64,
    pub created_at: u64,
    pub cluster_factor: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrawingIn {
    /// Index into `fixtures::TIP_POOL` (the PoW fields of the parent).
    pub tip: usize,
    pub version: u32,
    #[serde(with = "wire::hex_bytes")]
    pub tx_root: Vec<u8>,
    pub timestamp: u64,
    pub difficulty: u64,
    /// `None`: the block sits at `tip.height + 1` (wrapping). `Some(h)`: at
    /// `h`.
    pub block_height: Option<u64>,
    /// `None`: the block names `tip.hash()` as its parent. `Some(x)`: `x`.
    #[serde(default)]
    pub wrong_parent: Option<Vec<u8>>,
    pub fees: Vec<u64>,
    pub reward: u64,
    #[serde(with = "wire::dec_u128")]
    pub lottery_pool: u128,
    pub rows: Vec<RowIn>,
}

impl DrawingIn {
    fn tip_header(&self) -> BlockHeader {
        let t = &TIP_POOL[self.tip];
        BlockHeader {
            version: self.version,
            prev_block_hash: t.prev,
            tx_root: wire::arr(&self.tx_root),
            timestamp: self.timestamp,
            height: t.height,
            difficulty: self.difficulty,
            nonce: t.nonce,
            minter_view_key: t.view,
            minter_spend_key: t.spend,
        }
    }

    fn block(&self, fx: &Fixtures, tip: &BlockHeader) -> Block {
        let height = self.block_height.unwrap_or(tip.height.wrapping_add(1));
        let mut block = fx.genesis().clone();
        block.header.height = height;
        block.header.prev_block_hash = match &self.wrong_parent {
            Some(p) => wire::arr(p),
            None => tip.hash(),
        };
        block.minting_tx.block_height = height;
        block.minting_tx.reward = self.reward;
        block.transactions = self
            .fees
            .iter()
            .map(|&fee| Transaction::new(vec![], vec![], fee, 0))
            .collect();
        block
    }

    /// The rows in key order, with duplicate ids removed (the table is a map).
    fn table(&self) -> BTreeMap<[u8; 36], formal_drt::Row> {
        self.rows
            .iter()
            .map(|r| {
                (
                    wire::arr(&r.id),
                    formal_drt::Row {
                        amount: r.amount,
                        created_at: r.created_at,
                        cluster_factor: r.cluster_factor,
                    },
                )
            })
            .collect()
    }
}

fn header_wire(h: &BlockHeader) -> Value {
    json!({
        "version": n(h.version),
        "prev_block_hash": b(&h.prev_block_hash),
        "tx_root": b(&h.tx_root),
        "timestamp": n(h.timestamp),
        "height": n(h.height),
        "difficulty": n(h.difficulty),
        "nonce": n(h.nonce),
        "minter_view_key": b(&h.minter_view_key),
        "minter_spend_key": b(&h.minter_spend_key),
    })
}

fn drawing_error(e: &LedgerError) -> String {
    match e {
        LedgerError::InvalidBlock(s) if s == "V2 seed parent" => "seed_parent".into(),
        LedgerError::InvalidBlock(s) if s == "experimental award bound" => "award_bound".into(),
        LedgerError::FeeOverflow => "fee_overflow".into(),
        LedgerError::InconsistentRecord(s) if s.contains("returned no winners") => {
            "no_winners".into()
        }
        other => format!("unexpected: {other}"),
    }
}

impl Leg for Drawing {
    const NAME: &'static str = "drawing";
    const RUST: &'static str = "botho::ledger::lottery_v2::drawing (via ledger::formal_drt)";
    type Input = DrawingIn;
    fn strategy() -> BoxedStrategy<DrawingIn> {
        let row = |h: u64| {
            (
                gen::id36(),
                gen::value(),
                prop_oneof![8 => gen::age().prop_map(move |a| h.saturating_sub(a)), 1 => gen::u64_edge()],
                gen::factor(),
            )
                .prop_map(|(id, amount, created_at, cluster_factor)| RowIn {
                    id,
                    amount,
                    created_at,
                    cluster_factor,
                })
        };
        (0..TIP_POOL.len())
            .prop_flat_map(move |tip| {
                let h = TIP_POOL[tip].height.wrapping_add(1);
                (
                    (
                        Just(tip),
                        prop_oneof![Just(1u32), any::<u32>()],
                        gen::bytes32(),
                        gen::u64_edge(),
                        gen::u64_edge(),
                    ),
                    prop_oneof![
                        12 => Just(None),
                        1 => gen::height().prop_map(Some),
                        1 => (0u64..3).prop_map(move |d| Some(h.wrapping_add(d).wrapping_sub(1))),
                    ],
                    prop_oneof![12 => Just(None), 1 => gen::bytes32().prop_map(Some)],
                    vec(
                        prop_oneof![
                            4 => 0u64..1_000_000_000_000,
                            1 => gen::u64_edge(),
                        ],
                        0..6,
                    ),
                    prop_oneof![
                        3 => Just(50_000_000_000_000u64),
                        1 => Just(0u64),
                        2 => gen::u64_edge(),
                    ],
                    gen::u128_edge(),
                    gen::sized_vec(row(h).boxed()),
                )
            })
            .prop_map(
                |(
                    (tip, version, tx_root, timestamp, difficulty),
                    block_height,
                    wrong_parent,
                    fees,
                    reward,
                    lottery_pool,
                    rows,
                )| DrawingIn {
                    tip,
                    version,
                    tx_root,
                    timestamp,
                    difficulty,
                    block_height,
                    wrong_parent,
                    fees,
                    reward,
                    lottery_pool,
                    rows,
                },
            )
            .boxed()
    }
    fn args(fx: &Fixtures, i: &DrawingIn) -> Value {
        let tip = i.tip_header();
        let block = i.block(fx, &tip);
        let rows: Vec<Value> = i
            .table()
            .iter()
            .map(|(id, r)| {
                json!({
                    "id": b(id),
                    "amount": n(r.amount),
                    "created_at": n(r.created_at),
                    "cluster_factor": n(r.cluster_factor),
                })
            })
            .collect();
        json!({
            "lottery_pool": n(i.lottery_pool),
            "tip": header_wire(&tip),
            "tip_pow_hash": b(&fx.pow_hash(i.tip, &tip)),
            "rows": rows,
            "block": {
                "height": n(block.height()),
                "prev_block_hash": b(&block.header.prev_block_hash),
                "fees": list(&i.fees, |f| n(*f)),
                "reward": n(i.reward),
                "emission_share": n(block.minting_tx.lottery_emission_share()),
            },
        })
    }
    fn rust(fx: &Fixtures, i: &DrawingIn) -> Value {
        let tip = i.tip_header();
        let block = i.block(fx, &tip);
        let reads = formal_drt::Reads {
            lottery_pool: i.lottery_pool,
            tip,
            rows: i.table(),
        };
        let (result, windows) = formal_drt::drawing(&reads, &block);
        let windows = list(&windows, |w| b(w));
        match result {
            Ok((draw, acc)) => json!({
                "result": { "draw": block_result_wire(&draw), "accounting": acc_wire(&acc) },
                "windows": windows,
            }),
            Err(e) => json!({ "error": drawing_error(&e), "windows": windows }),
        }
    }
    fn classify(_: &DrawingIn, r: &Value) -> String {
        match r.get("error").and_then(Value::as_str) {
            Some(e) => format!("error {e}"),
            None => format!("ok, {}", winners_class(&r["result"]["draw"])),
        }
    }
}
