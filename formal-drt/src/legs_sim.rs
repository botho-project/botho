//! Stateful production simulation / node / Lean correspondence (#1972).
//!
//! Candidate order, ownership, already-collected fees, emission, and V2 seeds
//! are external inputs, not accepted-chain/reachability claims. The independent
//! reference inventory resolves prior-award references using NODE results only.
//! Lean receives these resolved snapshots and advances its OWN arithmetic pool.
//! Its one model result occupies both named comparison slots; the third slot
//! is a Rust-only public-state invariant, included in ordinary runner equality.

use crate::{
    fixtures::Fixtures,
    gen,
    legs::Leg,
    wire::{self, n},
};
use botho::consensus::lottery::{
    compute_pool_accounting, draw_lottery_winners_seeded, reward_cap, BlockLotteryResult,
    LotteryFeeConfig, LotteryPoolAccounting,
};
use bth_cluster_tax::{
    simulation::{
        lottery::{LotterySimulation, SybilStrategy},
        lottery_production::{
            ProductionAward, ProductionBlock, ProductionBlockResult, ProductionOutput,
        },
    },
    LotteryDrawConfig,
};
use proptest::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

// Array width is part of the fixture schema, not a later panicking conversion.
mod fixed_hex {
    use super::*;
    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        wire::hex_bytes::serialize(v, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[u8; N], D::Error> {
        wire::hex_bytes::deserialize(d)?
            .try_into()
            .map_err(|v: Vec<u8>| {
                serde::de::Error::custom(format!("expected {N} bytes, got {}", v.len()))
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    #[serde(with = "fixed_hex")]
    id: [u8; 36],
    #[serde(with = "wire::dec_u64")]
    simulation_id: u64,
    #[serde(with = "wire::dec_u64")]
    owner_id: u64,
    #[serde(with = "wire::dec_u64")]
    value: u64,
    #[serde(with = "wire::dec_u64")]
    cluster_factor: u64,
    #[serde(with = "wire::dec_u64")]
    creation_block: u64,
}

impl Output {
    fn production(&self) -> ProductionOutput {
        ProductionOutput {
            id: self.id,
            simulation_id: self.simulation_id,
            owner_id: self.owner_id,
            value: self.value,
            cluster_factor: self.cluster_factor,
            creation_block: self.creation_block,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Ref {
    Initial {
        #[serde(with = "wire::dec_u64")]
        index: u64,
    },
    Award {
        #[serde(with = "wire::dec_u64")]
        step: u64,
        #[serde(with = "wire::dec_u64")]
        index: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Block {
    #[serde(with = "wire::dec_u64")]
    height: u64,
    #[serde(with = "wire::dec_u64")]
    total_fees: u64,
    #[serde(with = "wire::dec_u64")]
    emission_share: u64,
    #[serde(with = "wire::dec_u64")]
    block_reward: u64,
    #[serde(with = "fixed_hex")]
    v2_seed: [u8; 32],
    candidates: Vec<Ref>,
}

fn strict_carry<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
    let s = String::deserialize(d)?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(serde::de::Error::custom("expected decimal u128 string"));
    }
    wire::dec_u128::deserialize(serde::de::value::StrDeserializer::<D::Error>::new(&s))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    initial: Vec<Output>,
    #[serde(
        serialize_with = "wire::dec_u128::serialize",
        deserialize_with = "strict_carry"
    )]
    carryover: u128,
    blocks: Vec<Block>,
}

fn config() -> LotteryFeeConfig {
    // Default fee config is environment-sensitive; the consensus split is 800.
    LotteryFeeConfig {
        pool_fraction_permille: 800,
        draw_config: LotteryDrawConfig::default(),
    }
}

fn accounting(a: &LotteryPoolAccounting) -> Value {
    json!({"fee_pool": n(a.fee_pool), "fee_burn": n(a.fee_burn),
        "emission_share": n(a.emission_share), "available": n(a.available), "payout": n(a.payout)})
}

fn node_projection(cap: u64, a: &LotteryPoolAccounting, r: &BlockLotteryResult) -> Value {
    let paid: u64 = r.winners.iter().map(|w| w.payout).sum();
    json!({"payout_cap": n(cap), "accounting": accounting(a), "paid": n(paid),
        "carryover": n(a.carryover_after(paid)), "draw": {
            "block_height": n(r.block_height), "total_fees": n(r.total_fees),
            "pool_amount": n(r.pool_amount), "burn_amount": n(r.burn_amount),
            "seed": wire::b(&r.seed), "winners": r.winners.iter().map(|w|
                json!({"utxo_id": wire::b(&w.utxo_id), "payout": n(w.payout)})).collect::<Vec<_>>()}})
}

fn sim_projection(r: &ProductionBlockResult) -> Value {
    json!({"payout_cap": n(r.payout_cap), "accounting": {
        "fee_pool": n(r.fee_pool), "fee_burn": n(r.burn_amount),
        "emission_share": n(r.emission_share), "available": n(r.available), "payout": n(r.payout)},
        "paid": n(r.paid), "carryover": n(r.carryover), "draw": {
            "block_height": n(r.block_height), "total_fees": n(r.total_fees),
            "pool_amount": n(r.pool_amount), "burn_amount": n(r.burn_amount),
            "seed": wire::b(&r.seed), "winners": r.awards.iter().map(|w|
                json!({"utxo_id": wire::b(&w.winner_id), "payout": n(w.output.value)})).collect::<Vec<_>>()}})
}

#[derive(Default)]
struct Owner {
    ids: Vec<u64>,
    balance: u64,
    winnings: u64,
}

/// Independent bookkeeping, never populated from simulator return values or
/// simulator getters. Synthetic IDs are a simulation invariant, not outpoints.
struct Reference {
    outputs: BTreeMap<[u8; 36], ProductionOutput>,
    owners: BTreeMap<u64, Owner>,
    awards: Vec<Vec<ProductionAward>>,
    next_id: u64,
    height: Option<u64>,
    carry: u128,
    fees: u64,
    burned: u64,
    distributed: u64,
    drawings: u64,
    quintiles: [u64; 5],
}

fn add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| "unrepresentable reference counter".into())
}

impl Reference {
    fn new(i: &Input) -> Result<Self, String> {
        if i.blocks.is_empty() || i.blocks.len() > 4 || i.initial.len() > 10_000 {
            return Err("sequence/inventory bound".into());
        }
        let mut r = Self {
            outputs: BTreeMap::new(),
            owners: BTreeMap::new(),
            awards: vec![],
            next_id: 1,
            height: None,
            carry: i.carryover,
            fees: 0,
            burned: 0,
            distributed: 0,
            drawings: 0,
            quintiles: [0; 5],
        };
        let mut ids = BTreeSet::new();
        for o in &i.initial {
            if r.outputs.insert(o.id, o.production()).is_some() || !ids.insert(o.simulation_id) {
                return Err("duplicate identity".into());
            }
            r.next_id = r.next_id.max(add(o.simulation_id, 1)?);
            let owner = r.owners.entry(o.owner_id).or_default();
            owner.ids.push(o.simulation_id);
            owner.balance = add(owner.balance, o.value)?;
        }
        Ok(r)
    }

    fn resolve(&self, i: &Input, b: &Block) -> Result<ProductionBlock, String> {
        if b.candidates.len() > 10_000 || self.height.is_some_and(|h| b.height <= h) {
            return Err("snapshot/height bound".into());
        }
        let mut seen = BTreeSet::new();
        let candidates = b
            .candidates
            .iter()
            .map(|r| {
                let o = match r {
                    Ref::Initial { index } => i
                        .initial
                        .get(usize::try_from(*index).map_err(|_| "index")?)
                        .map(Output::production),
                    Ref::Award { step, index } => self
                        .awards
                        .get(usize::try_from(*step).map_err(|_| "step")?)
                        .and_then(|a| a.get(usize::try_from(*index).ok()?))
                        .map(|a| a.output.clone()),
                }
                .ok_or("unresolved snapshot reference")?;
                if !seen.insert(o.id) {
                    return Err("duplicate snapshot reference".into());
                }
                Ok(o)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(ProductionBlock {
            height: b.height,
            total_fees: b.total_fees,
            emission_share: b.emission_share,
            block_reward: b.block_reward,
            v2_seed: b.v2_seed,
            candidates,
        })
    }

    fn advance(
        &mut self,
        b: &ProductionBlock,
        a: &LotteryPoolAccounting,
        draw: &BlockLotteryResult,
    ) -> Result<(), String> {
        let mut wealths: Vec<_> = self.owners.values().map(|o| o.balance).collect();
        wealths.sort_unstable();
        let ranks: BTreeMap<_, _> = self
            .owners
            .iter()
            .map(|(&id, o)| {
                (
                    id,
                    wealths.partition_point(|w| *w < o.balance) * 5 / wealths.len(),
                )
            })
            .collect();
        let mut awards = vec![];
        for w in &draw.winners {
            let owner_id = self
                .outputs
                .get(&w.utxo_id)
                .ok_or("unknown node winner")?
                .owner_id;
            let mut id = [0; 36];
            id[..26].copy_from_slice(b"BOTHO_SIM_LOTTERY_AWARD_V1");
            id[28..].copy_from_slice(&self.next_id.to_be_bytes());
            let output = ProductionOutput {
                id,
                simulation_id: self.next_id,
                owner_id,
                value: w.payout,
                cluster_factor: 1000,
                creation_block: b.height,
            };
            self.next_id = add(self.next_id, 1)?;
            if self.outputs.insert(id, output.clone()).is_some() {
                return Err("synthetic collision".into());
            }
            let owner = self.owners.get_mut(&owner_id).ok_or("unknown owner")?;
            owner.ids.push(output.simulation_id);
            owner.balance = add(owner.balance, w.payout)?;
            owner.winnings = add(owner.winnings, w.payout)?;
            let quintile = ranks[&owner_id];
            self.quintiles[quintile] = add(self.quintiles[quintile], w.payout)?;
            awards.push(ProductionAward {
                winner_id: w.utxo_id,
                output,
                quintile,
            });
        }
        let paid = draw
            .winners
            .iter()
            .try_fold(0, |sum, w| add(sum, w.payout))?;
        self.carry = a.carryover_after(paid);
        self.height = Some(b.height);
        self.fees = add(self.fees, b.total_fees)?;
        self.burned = add(self.burned, a.fee_burn)?;
        self.distributed = add(self.distributed, paid)?;
        self.drawings += u64::from(!awards.is_empty());
        self.awards.push(awards);
        Ok(())
    }

    fn observe(&self, sim: &LotterySimulation, step: &str, failed: &mut Vec<String>) {
        let mut check = |name: &str, ok: bool| {
            if !ok {
                failed.push(format!("{step}/{name}"));
            }
        };
        check(
            "integer_inventory",
            sim.production_outputs() == Some(&self.outputs),
        );
        check("carryover", sim.production_carryover() == Some(self.carry));
        check(
            "current_block",
            sim.current_block == self.height.unwrap_or(0),
        );
        check("legacy_pool_untouched", sim.lottery_pool == 0);
        check("burned", sim.total_burned == self.burned);
        check("owner_count", sim.owners.len() == self.owners.len());
        check("utxo_count", sim.utxos.len() == self.outputs.len());
        for (id, expected) in &self.owners {
            let ok = sim.owners.get(id).is_some_and(|o| {
                o.id == *id
                    && o.utxo_ids == expected.ids
                    && sim.owner_value(*id) == expected.balance
                    && o.total_winnings == expected.winnings
                    && o.total_fees_paid == 0
                    && o.tx_count == 0
                    && o.strategy == SybilStrategy::Normal
            });
            check(&format!("owner/{id}"), ok);
        }
        for expected in self.outputs.values() {
            let ok = sim.utxos.get(&expected.simulation_id).is_some_and(|o| {
                o.id == expected.simulation_id
                    && o.owner_id == expected.owner_id
                    && o.value == expected.value
                    && o.creation_block == expected.creation_block
                    && o.last_activity_block == expected.creation_block
                    && o.demurrage_anchor == expected.creation_block
                    && o.cluster_factor == expected.cluster_factor as f64 / 1000.0
                    && o.activity_contribution == 0.0
                    && o.selection_count == 0
                    && o.tickets_from_fees == 0.0
                    && o.tag_entropy == 0.0
            });
            check(&format!("utxo/{}", expected.simulation_id), ok);
        }
        let m = &sim.metrics;
        check("total_distributed", m.total_distributed == self.distributed);
        check("fees_collected", m.total_fees_collected == self.fees);
        check("drawings", m.drawings_held == self.drawings);
        check("quintile_credits", m.winnings_by_quintile == self.quintiles);
        check(
            "unrelated_metrics_untouched",
            m.gini_snapshots.is_empty()
                && m.tickets_by_quintile == [0.0; 5]
                && m.sybil_results.is_empty(),
        );
    }
}

fn node(b: &ProductionBlock, carry: u128) -> (u64, LotteryPoolAccounting, BlockLotteryResult) {
    let cs: Vec<_> = b
        .candidates
        .iter()
        .map(ProductionOutput::candidate)
        .collect();
    let cfg = config();
    let cap = reward_cap(&cs, b.height, b.block_reward, &cfg);
    let acc = compute_pool_accounting(b.total_fees, b.emission_share, carry, cap, &cfg);
    let draw = draw_lottery_winners_seeded(&cs, b.total_fees, &acc, b.height, &b.v2_seed, &cfg);
    (cap, acc, draw)
}

fn model_args(i: &Input) -> Result<Value, String> {
    let mut reference = Reference::new(i)?;
    let mut steps = vec![];
    for block in &i.blocks {
        let b = reference.resolve(i, block)?;
        steps.push(json!({"height": n(b.height), "total_fees": n(b.total_fees),
            "emission_share": n(b.emission_share), "block_reward": n(b.block_reward),
            "v2_seed": wire::b(&b.v2_seed), "candidates": b.candidates.iter().map(|o|
                json!({"id": wire::b(&o.id), "value": n(o.value),
                    "cluster_factor": n(o.cluster_factor), "creation_block": n(o.creation_block)})).collect::<Vec<_>>()}));
        let (_, a, draw) = node(&b, reference.carry);
        reference.advance(&b, &a, &draw)?;
    }
    Ok(json!({"carryover": n(i.carryover), "blocks": steps}))
}

fn evaluate(i: &Input) -> Result<Value, String> {
    let mut reference = Reference::new(i)?;
    let mut sim = LotterySimulation::new_production(
        i.initial.iter().map(Output::production).collect(),
        i.carryover,
    )
    .map_err(|e| format!("constructor: {e:?}"))?;
    let mut failed = vec![];
    reference.observe(&sim, "initial", &mut failed);
    let mut nodes = vec![];
    let mut simulations = vec![];
    for (step, block) in i.blocks.iter().enumerate() {
        let b = reference.resolve(i, block)?;
        let (cap, a, draw) = node(&b, reference.carry);
        nodes.push(node_projection(cap, &a, &draw));
        reference.advance(&b, &a, &draw)?;
        match sim.apply_production_block(&b) {
            Ok(result) => {
                simulations.push(sim_projection(&result));
                if Some(&result.awards) != reference.awards.last() {
                    failed.push(format!("{step}/ordered_awards"));
                }
                reference.observe(&sim, &step.to_string(), &mut failed);
            }
            Err(e) => {
                simulations.push(json!({"unexpected_rejection": format!("{e:?}")}));
                failed.push(format!("{step}/accepted_valid_transition"));
                break;
            }
        }
    }
    Ok(json!({"node": nodes, "simulator": simulations,
        "simulator_state": {"ok": failed.is_empty(), "failed_checks": failed}}))
}

fn initial(index: u64, owner: u64, value: u64, factor: u64, creation: u64) -> Output {
    let mut id = [0; 36];
    id[..8].copy_from_slice(&index.to_le_bytes());
    Output {
        id,
        simulation_id: index + 1,
        owner_id: owner,
        value,
        cluster_factor: factor,
        creation_block: creation,
    }
}

fn initial_refs(count: usize) -> Vec<Ref> {
    (0..count as u64)
        .map(|index| Ref::Initial { index })
        .collect()
}

/// A separate full-width one-block family preserves arbitrary values/owners/
/// factors/heights/fees/rewards/emission/carry without overflowing owner
/// credits: its single output is ineligible (created at MAX), or its reward cap
/// is zero. Payout-cap boundaries and successful credits are exercised by the
/// small multi-block family; no branch filters on simulator success or clamps
/// inputs.
fn wide_strategy() -> BoxedStrategy<Input> {
    (
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        prop_oneof![
            (Just(u64::MAX), gen::u64_edge()),
            (gen::u64_edge(), Just(0u64))
        ],
        gen::u128_edge(),
        gen::bytes32(),
        prop::collection::vec(any::<u8>(), 36),
        0u64..u64::MAX,
    )
        .prop_map(
            |(
                value,
                factor,
                owner,
                height,
                fees,
                emission,
                (creation, reward),
                carryover,
                seed,
                id,
                serial,
            )| Input {
                initial: vec![Output {
                    id: wire::arr(&id),
                    simulation_id: serial,
                    owner_id: owner,
                    value,
                    cluster_factor: factor,
                    creation_block: creation,
                }],
                carryover,
                blocks: vec![Block {
                    height,
                    total_fees: fees,
                    emission_share: emission,
                    block_reward: reward,
                    v2_seed: wire::arr(&seed),
                    candidates: initial_refs(1),
                }],
            },
        )
        .boxed()
}

// Full-width fees/emission/reward with successful draws and ample owner/counter
// headroom. Kept to one block so u64 cumulative fees are representable without
// filtering; each initial value is bounded before construction, never clamped.
fn paying_strategy() -> BoxedStrategy<Input> {
    (
        prop::collection::vec(
            (
                0u64..u64::MAX / 16,
                gen::factor(),
                prop::sample::select(vec![719u64, 720, 721, 10_000, 10_001]),
            ),
            1..9,
        ),
        gen::u128_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        gen::u64_edge(),
        gen::bytes32(),
        any::<bool>(),
    )
        .prop_map(
            |(rows, carryover, fees, emission, reward, seed, same_owner)| {
                let initial: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .map(|(idx, (value, factor, age))| {
                        initial(
                            idx as u64,
                            if same_owner { 7 } else { idx as u64 },
                            *value,
                            *factor,
                            20_000 - age,
                        )
                    })
                    .collect();
                let candidates = initial_refs(initial.len());
                Input {
                    initial,
                    carryover,
                    blocks: vec![Block {
                        height: 20_000,
                        total_fees: fees,
                        emission_share: emission,
                        block_reward: reward,
                        v2_seed: wire::arr(&seed),
                        candidates,
                    }],
                }
            },
        )
        .boxed()
}

fn sequence_strategy() -> BoxedStrategy<Input> {
    (
        prop::collection::vec(
            (
                gen::factor(),
                prop::sample::select(vec![0u64, 719, 720, 721, 10_000, 10_001]),
                prop::sample::select(vec![0u64, 999_999, 1_000_000, 1_000_001, 10_000_000]),
            ),
            0..9,
        ),
        gen::u128_edge(),
        gen::bytes32(),
        0u64..10_000_000,
        0u64..10_000_000,
        0u64..10_000_000,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(rows, carryover, seed, fees, emission, reward, same_owner, reverse)| {
                let initial: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .map(|(idx, (factor, age, value))| {
                        initial(
                            idx as u64,
                            if same_owner { 7 } else { idx as u64 },
                            *value,
                            *factor,
                            20_000 - age,
                        )
                    })
                    .collect();
                let mut refs = initial_refs(initial.len());
                if reverse {
                    refs.reverse();
                }
                let blocks = [20_000, 20_001, 20_720]
                    .into_iter()
                    .enumerate()
                    .map(|(idx, height)| Block {
                        height,
                        total_fees: fees,
                        emission_share: emission,
                        block_reward: reward,
                        v2_seed: wire::arr(&seed),
                        candidates: if idx == 1 { vec![] } else { refs.clone() },
                    })
                    .collect();
                Input {
                    initial,
                    carryover,
                    blocks,
                }
            },
        )
        .boxed()
}

/// Guaranteed first award, subsequently omitted, reordered, then eligible at
/// maturity. Shrinking the values/seed cannot invalidate any award reference.
fn award_strategy() -> BoxedStrategy<Input> {
    (
        gen::bytes32(),
        1_000_000u64..10_000_001,
        gen::factor(),
        any::<bool>(),
    )
        .prop_map(|(seed, value, factor, reverse)| {
            let mut mature = vec![Ref::Initial { index: 0 }, Ref::Award { step: 0, index: 0 }];
            if reverse {
                mature.reverse();
            }
            Input {
                initial: vec![initial(0, 9, value, factor, 100)],
                carryover: 10_000_000,
                blocks: vec![
                    Block {
                        height: 820,
                        total_fees: 0,
                        emission_share: 0,
                        block_reward: 1_000_000,
                        v2_seed: wire::arr(&seed),
                        candidates: initial_refs(1),
                    },
                    Block {
                        height: 1539,
                        total_fees: 1,
                        emission_share: 0,
                        block_reward: 1_000_000,
                        v2_seed: wire::arr(&seed),
                        candidates: vec![Ref::Award { step: 0, index: 0 }],
                    },
                    Block {
                        height: 1540,
                        total_fees: 0,
                        emission_share: 0,
                        block_reward: 1_000_001,
                        v2_seed: wire::arr(&seed),
                        candidates: mature,
                    },
                ],
            }
        })
        .boxed()
}

pub struct LotterySim;
impl Leg for LotterySim {
    const NAME: &'static str = "lottery_sim";
    const RUST: &'static str = "LotterySimulation::{new_production,apply_production_block}; botho::consensus::lottery::{reward_cap,compute_pool_accounting,draw_lottery_winners_seeded}; independent public-state invariants";
    type Input = Input;
    fn strategy() -> BoxedStrategy<Input> {
        prop_oneof![2 => wide_strategy(), 3 => paying_strategy(), 5 => sequence_strategy(), 2 => award_strategy()].boxed()
    }
    fn args(_: &Fixtures, i: &Input) -> Value {
        model_args(i).expect("constructively valid lottery_sim input")
    }
    fn rust(_: &Fixtures, i: &Input) -> Value {
        evaluate(i).unwrap_or_else(|e| json!({"invalid_reference_input": e}))
    }
    fn classify(i: &Input, r: &Value) -> String {
        let count = i
            .blocks
            .iter()
            .map(|b| b.candidates.len())
            .max()
            .unwrap_or(0);
        let size = match count {
            0 => "empty",
            1 => "one",
            2..=3 => "two_three",
            4..=9999 => "four_plus",
            _ => "max",
        };
        let kind = if i
            .blocks
            .iter()
            .any(|b| b.candidates.iter().any(|r| matches!(r, Ref::Award { .. })))
        {
            "award_reentry"
        } else if i.blocks.len() > 1 {
            "sequence"
        } else {
            "wide"
        };
        let state = if r["simulator_state"]["ok"] == true {
            "state_ok"
        } else {
            "state_failed"
        };
        format!("{kind}/{size}/{state}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{
        strategy::ValueTree,
        test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
    };

    // Documentation validation only: no runtime comparator, registration,
    // generator, shrinker or replay path reads this inventory.
    const INVENTORY: &str = include_str!("../sim-divergences.json");

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum Classification {
        Production,
        Research,
        ExternalAssumption,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Inventory {
        schema_version: u32,
        surfaces: Vec<Surface>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Surface {
        id: String,
        classification: Classification,
        sources: Vec<String>,
        members: Vec<String>,
        reason: String,
        effect_on_production_claims: String,
        #[serde(default)]
        command: Option<String>,
    }

    struct SurfaceSpec {
        id: &'static str,
        classification: Classification,
        sources: Vec<String>,
        members: Vec<String>,
        command: Option<&'static str>,
    }

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    fn surface_specs() -> Vec<SurfaceSpec> {
        use bth_cluster_tax::{
            consensus_fee::FeeModelMode,
            simulation::{
                lottery::{DistributionMode, SelectionMode, TicketModel},
                lottery_reward_cap_faithful::{Capacity, Rules},
                lottery_reward_cap_sweep::{ExcessPolicy, GiniPolicy},
                lottery_selection_sweep::Rule,
                lottery_sybil_brake_sweep::HonestPolicy,
            },
        };
        use Classification::{ExternalAssumption as External, Production, Research};

        // Exhaustive matches fail to compile when a source enum grows. The
        // expected members are independent of the document being validated.
        macro_rules! variants {
            ($ty:ident: $($variant:ident),+ $(,)?) => {
                [$($ty::$variant),+].map(|v| match v {
                    $($ty::$variant => concat!(stringify!($ty), "::", stringify!($variant)).to_owned()),+
                }).to_vec()
            };
        }
        let tickets = variants!(TicketModel: ActivityBased, FeeProportional, PureValueWeighted, UniformPerUtxo);
        let distribution = [
            DistributionMode::Pooled,
            DistributionMode::Immediate { winners_per_tx: 1 },
        ]
        .map(|v| match v {
            DistributionMode::Pooled => "DistributionMode::Pooled".into(),
            DistributionMode::Immediate { .. } => "DistributionMode::Immediate".into(),
        })
        .to_vec();
        let selection = [
            SelectionMode::Uniform,
            SelectionMode::ValueWeighted,
            SelectionMode::SqrtWeighted,
            SelectionMode::LogWeighted,
            SelectionMode::CappedUniform { max_per_owner: 1 },
            SelectionMode::Hybrid { alpha: 0.5 },
            SelectionMode::AgeWeighted {
                max_age_blocks: 1,
                age_bonus: 1.0,
            },
            SelectionMode::ClusterWeighted,
            SelectionMode::EntropyWeighted { entropy_bonus: 1.0 },
            SelectionMode::ValueWeightedWithFloor {
                ticket_threshold: 1,
                decay_rate_per_day: 0.1,
                min_eligibility: 0.1,
                blocks_per_day: 1,
            },
        ]
        .map(|v| {
            match v {
                SelectionMode::Uniform => "SelectionMode::Uniform",
                SelectionMode::ValueWeighted => "SelectionMode::ValueWeighted",
                SelectionMode::SqrtWeighted => "SelectionMode::SqrtWeighted",
                SelectionMode::LogWeighted => "SelectionMode::LogWeighted",
                SelectionMode::CappedUniform { .. } => "SelectionMode::CappedUniform",
                SelectionMode::Hybrid { .. } => "SelectionMode::Hybrid",
                SelectionMode::AgeWeighted { .. } => "SelectionMode::AgeWeighted",
                SelectionMode::ClusterWeighted => "SelectionMode::ClusterWeighted",
                SelectionMode::EntropyWeighted { .. } => "SelectionMode::EntropyWeighted",
                SelectionMode::ValueWeightedWithFloor { .. } => {
                    "SelectionMode::ValueWeightedWithFloor"
                }
            }
            .to_owned()
        })
        .to_vec();
        let rules = Rule::all()
            .into_iter()
            .map(|v| {
                match v {
                    Rule::Uniform => "Rule::Uniform",
                    Rule::ClusterWeighted => "Rule::ClusterWeighted",
                    Rule::FeeInverse => "Rule::FeeInverse",
                    Rule::CirculationGated => "Rule::CirculationGated",
                    Rule::Hybrid => "Rule::Hybrid",
                }
                .to_owned()
            })
            .collect();
        let honest = variants!(HonestPolicy: Burn, UniformAll, WindowedCirculated);
        let mut cap = variants!(ExcessPolicy: Burn, CarryForward);
        cap.extend(variants!(GiniPolicy: Burn, UncappedWindowed, CappedBurn, CappedCarryForward, ValueWeightedAll));
        let mut faithful = Rules::all()
            .map(|v| {
                match v {
                    Rules::CurrentCode => "Rules::CurrentCode",
                    Rules::PerBlockCap => "Rules::PerBlockCap",
                    Rules::WindowOverLength => "Rules::WindowOverLength",
                    Rules::BaseFeePerOutput => "Rules::BaseFeePerOutput",
                    Rules::PerBlockAndBaseFee => "Rules::PerBlockAndBaseFee",
                    Rules::PerBlockBaseFeeNoFree => "Rules::PerBlockBaseFeeNoFree",
                    Rules::MinValue1Bth => "Rules::MinValue1Bth",
                }
                .to_owned()
            })
            .to_vec();
        faithful.extend(variants!(Capacity: ProposerSlot, BlockSize));
        let mut fees = [
            FeeModelMode::ConsensusOnly,
            FeeModelMode::ConsensusPlusRelayMin,
        ]
        .map(|v| {
            let variant = match v {
                FeeModelMode::ConsensusOnly => "ConsensusOnly",
                FeeModelMode::ConsensusPlusRelayMin => "ConsensusPlusRelayMin",
            };
            format!("FeeModelMode::{variant}={}", v.label())
        })
        .to_vec();
        fees.push("collected_fees_and_fee_floor".into());

        // Concrete paths/symbols identify the intended surfaces, not arbitrary
        // nonempty strings. These are reviewed source references, not a parser
        // or a registry of scalar parameter combinations/constants.
        let legacy = "cluster-tax/src/simulation/lottery.rs";
        let production = "cluster-tax/src/simulation/lottery_production.rs";
        let cli = "cluster-tax/src/bin/sim.rs";
        let leg = "formal-drt/src/legs_sim.rs";
        let selection_sweep = "cluster-tax/src/simulation/lottery_selection_sweep.rs";
        let sybil = "cluster-tax/src/simulation/lottery_sybil_brake_sweep.rs";
        let reward = "cluster-tax/src/simulation/lottery_reward_cap_sweep.rs";
        let faithful_sweep = "cluster-tax/src/simulation/lottery_reward_cap_faithful.rs";
        macro_rules! spec {
            ($id:literal, $class:ident, [$($path:expr => $symbol:literal),+], $members:expr, $command:expr) => {
                SurfaceSpec { id: $id, classification: $class,
                    sources: vec![$(format!("{}#{}", $path, $symbol)),+], members: $members, command: $command }
            };
        }
        vec![
            spec!("production.transition", Production, [production => "LotterySimulation::new_production", production => "LotterySimulation::apply_production_block"], names(&["new_production", "apply_production_block"]), None),
            spec!("legacy.ticket_model", Research, [legacy => "TicketModel", legacy => "LotteryUtxo::tickets_for_model"], tickets, None),
            spec!("legacy.distribution_mode", Research, [legacy => "DistributionMode", legacy => "LotterySimulation::run_drawing", legacy => "LotterySimulation::distribute_to_winners"], distribution, None),
            spec!("legacy.selection_mode", Research, [legacy => "SelectionMode", legacy => "LotterySimulation::select_winner_by_mode"], selection, None),
            spec!("legacy.application", Research, [legacy => "LotteryConfig::structure_factor", legacy => "LotterySimulation::run_drawing", legacy => "LotterySimulation::distribute_to_winners"], names(&["float_tickets_and_rounding", "activity_and_entropy", "structure_fees", "pooled_proportional_credit", "immediate_in_place_credit"]), None),
            spec!("legacy.fee_time_units", Research, [legacy => "LotteryConfig", legacy => "LotteryConfig::combined_mechanism", legacy => "LotterySimulation::with_consensus_fee"], names(&["legacy_fee_formula", "twenty_second_clock", "caller_selected_units"]), None),
            spec!("sweep.selection", Research, [selection_sweep => "Rule", selection_sweep => "run_lottery_selection_sweep", cli => "run_lottery_selection_sweep_cli"], rules, Some("lottery-selection-sweep")),
            spec!("sweep.sybil_brake", Research, [sybil => "HonestPolicy", sybil => "run_sybil_brake_sweep", cli => "run_lottery_sybil_brake_sweep_cli"], honest, Some("lottery-sybil-brake-sweep")),
            spec!("sweep.reward_cap", Research, [reward => "ExcessPolicy", reward => "GiniPolicy", reward => "run_reward_cap_sweep", cli => "run_lottery_reward_cap_sweep_cli"], cap, Some("lottery-reward-cap-sweep")),
            spec!("sweep.reward_cap.faithful", Research, [faithful_sweep => "Rules", faithful_sweep => "Capacity", faithful_sweep => "run_faithful_sweep", faithful_sweep => "window_mc", cli => "run_lottery_reward_cap_faithful_cli"], faithful, Some("lottery-reward-cap-sweep --faithful")),
            spec!("assumption.fees", External, ["cluster-tax/src/consensus_fee.rs" => "FeeModelMode", "cluster-tax/src/consensus_fee.rs" => "SimConsensusFee", production => "ProductionBlock"], fees, None),
            spec!("assumption.candidates", External, [production => "ProductionBlock", leg => "Reference::resolve"], names(&["candidate_discovery", "window_and_order", "spent_record_policy"]), None),
            spec!("assumption.owners", External, [production => "ProductionOutput", production => "LotterySimulation::new_production", leg => "Reference::observe"], names(&["immutable_registration", "external_owner_mapping", "fresh_output_and_owner_credits"]), None),
            spec!("assumption.emission", External, [production => "ProductionBlock"], names(&["emission_share", "block_reward_schedule"]), None),
            spec!("assumption.seed", External, [production => "ProductionBlock", "formal-drt/lean/Drt/Codec.lean" => "lotterySim"], names(&["already_derived_v2_seed"]), None),
            spec!("assumption.representation", External, [production => "LotterySimulation::new_production", leg => "Reference::new", leg => "Input"], names(&["synthetic_ids", "representable_counters_and_owners", "bounded_sequences_and_snapshots"]), None),
        ]
    }

    fn validate_inventory(text: &str) -> Result<Inventory, String> {
        let inventory: Inventory = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if inventory.schema_version != 1 {
            return Err("unknown schema version".into());
        }
        let specs = surface_specs();
        if inventory.surfaces.len() != specs.len() {
            return Err("missing/extra surface".into());
        }
        let mut ids = BTreeSet::new();
        for surface in &inventory.surfaces {
            if !ids.insert(&surface.id) {
                return Err(format!("duplicate surface: {}", surface.id));
            }
            let spec = specs
                .iter()
                .find(|s| s.id == surface.id)
                .ok_or_else(|| format!("unknown surface: {}", surface.id))?;
            if surface.classification != spec.classification
                || surface.command.as_deref() != spec.command
            {
                return Err(format!("wrong classification/command: {}", surface.id));
            }
            if surface.reason.trim().is_empty()
                || surface.effect_on_production_claims.trim().is_empty()
            {
                return Err(format!("empty reason/effect: {}", surface.id));
            }
            for (field, actual, expected) in [
                ("sources", &surface.sources, &spec.sources),
                ("members", &surface.members, &spec.members),
            ] {
                let set: BTreeSet<_> = actual.iter().collect();
                if actual.iter().any(|s| s.trim().is_empty())
                    || set.len() != actual.len()
                    || set != expected.iter().collect()
                {
                    return Err(format!("invalid {field}: {}", surface.id));
                }
            }
        }
        Ok(inventory)
    }

    #[test]
    fn lottery_sim_inventory_complete_and_registered() {
        let inventory = validate_inventory(INVENTORY).unwrap();
        assert_eq!(
            inventory
                .surfaces
                .iter()
                .filter(|s| s.command.is_some())
                .count(),
            4
        );
        assert!(crate::LEGS
            .iter()
            .any(|(name, _)| *name == LotterySim::NAME));
    }

    #[test]
    fn lottery_sim_inventory_rejects_malformed_and_incomplete_declarations() {
        let good: Value = serde_json::from_str(INVENTORY).unwrap();
        let mut bad = vec![
            ("malformed JSON".to_owned(), "{".to_owned()),
            (
                "duplicate JSON field".into(),
                INVENTORY.replacen(
                    "\"schema_version\": 1",
                    "\"schema_version\": 1, \"schema_version\": 1",
                    1,
                ),
            ),
        ];
        let mut add = |label: &str, value: Value| bad.push((label.into(), value.to_string()));
        for (pointer, value) in [
            ("/schema_version", json!(2)),
            ("/schema_version", json!("1")),
            ("/surfaces", json!({})),
            ("/surfaces/0", json!(null)),
            ("/surfaces/0/id", json!(17)),
            ("/surfaces/0/id", json!("unknown")),
            ("/surfaces/0/classification", json!("unknown")),
            ("/surfaces/0/classification", json!("research")),
            ("/surfaces/0/reason", json!([])),
            ("/surfaces/0/reason", json!(" \t\n")),
            ("/surfaces/0/effect_on_production_claims", json!(null)),
            ("/surfaces/0/effect_on_production_claims", json!(" ")),
            ("/surfaces/0/sources", json!("path#symbol")),
            ("/surfaces/0/sources", json!([])),
            ("/surfaces/0/sources/0", json!(" ")),
            (
                "/surfaces/0/sources/0",
                json!("wrong.rs#apply_production_block"),
            ),
            (
                "/surfaces/0/sources/0",
                json!("cluster-tax/src/simulation/lottery_production.rs#wrong"),
            ),
            ("/surfaces/0/members", json!({})),
            ("/surfaces/0/members", json!([])),
            ("/surfaces/0/members/0", json!("unknown")),
            ("/surfaces/3/members/0", json!("SelectionMode::Unknown")),
            ("/surfaces/10/members/0", json!("FeeModelMode::Legacy")),
            ("/surfaces/0/members/0", json!(" ")),
            ("/surfaces/6/command", json!("lottery-reward-cap-sweep")),
            ("/surfaces/6/command", json!(null)),
            ("/surfaces/6/command", json!(4)),
        ] {
            let mut v = good.clone();
            *v.pointer_mut(pointer).unwrap() = value;
            add(pointer, v);
        }
        for field in ["schema_version", "surfaces"] {
            let mut v = good.clone();
            v.as_object_mut().unwrap().remove(field);
            add(field, v);
        }
        // Each record and each policy member is mandatory, including every
        // sweep policy; missing-member coverage is not limited to production.
        for (i, surface) in good["surfaces"].as_array().unwrap().iter().enumerate() {
            let mut v = good.clone();
            v["surfaces"].as_array_mut().unwrap().remove(i);
            add("missing surface", v);
            for field in [
                "id",
                "classification",
                "sources",
                "members",
                "reason",
                "effect_on_production_claims",
            ] {
                let mut v = good.clone();
                v["surfaces"][i].as_object_mut().unwrap().remove(field);
                add(field, v);
            }
            for field in ["members", "sources"] {
                for j in 0..surface[field].as_array().unwrap().len() {
                    let mut v = good.clone();
                    v["surfaces"][i][field].as_array_mut().unwrap().remove(j);
                    add("missing member/source", v);
                }
                let mut v = good.clone();
                v["surfaces"][i][field]
                    .as_array_mut()
                    .unwrap()
                    .push(surface[field][0].clone());
                add("duplicate member/source", v);
            }
        }
        let mut v = good.clone();
        v["surfaces"][1] = v["surfaces"][0].clone();
        add("duplicate ID", v);
        let mut v = good.clone();
        v["unknown"] = json!(true);
        add("unknown root key", v);
        for field in [
            "unknown",
            "mismatch_exemption",
            "allow_mismatch",
            "exemptions",
        ] {
            for value in [json!(true), json!(false), json!([]), json!(null)] {
                let mut v = good.clone();
                v["surfaces"][0][field] = value;
                add(field, v);
            }
        }
        for (label, text) in bad {
            assert!(
                validate_inventory(&text).is_err(),
                "accepted {label}: {text}"
            );
        }
    }

    fn runner() -> TestRunner {
        TestRunner::new_with_rng(
            Config {
                failure_persistence: None,
                ..Config::default()
            },
            TestRng::from_seed(RngAlgorithm::ChaCha, &[17; 32]),
        )
    }

    fn assert_valid(i: &Input) {
        model_args(i).expect("valid independently resolved input");
        let r = evaluate(i).expect("valid independent arithmetic");
        assert_eq!(r["node"], r["simulator"], "{i:?}");
        assert_eq!(
            r["simulator_state"],
            json!({"ok": true, "failed_checks": []}),
            "{i:?}"
        );
        assert!(!LotterySim::classify(i, &r).is_empty());
    }

    fn basic() -> Input {
        Input {
            initial: vec![initial(0, 7, 1_000_000, 1000, 1)],
            carryover: 10_000_000,
            blocks: vec![Block {
                height: 721,
                total_fees: 7,
                emission_share: 13,
                block_reward: 3,
                v2_seed: [0; 32],
                candidates: initial_refs(1),
            }],
        }
    }

    #[test]
    fn lottery_sim_constructive_generation_and_shrink_validity() {
        let mut rng = runner();
        for _ in 0..96 {
            let mut tree = LotterySim::strategy().new_tree(&mut rng).unwrap();
            assert_valid(&tree.current());
            // Both simplification and backtracking stay in the valid domain.
            for _ in 0..24 {
                if !tree.simplify() {
                    break;
                }
                assert_valid(&tree.current());
            }
            for _ in 0..8 {
                if !tree.complicate() {
                    break;
                }
                assert_valid(&tree.current());
            }
        }
    }

    #[test]
    fn lottery_sim_fixture_wire_precision_and_rejections() {
        let mut i = basic();
        i.carryover = u128::MAX;
        i.initial[0].owner_id = u64::MAX;
        i.initial[0].cluster_factor = u64::MAX;
        i.blocks[0].total_fees = u64::MAX;
        i.blocks[0].emission_share = u64::MAX;
        let value = serde_json::to_value(&i).unwrap();
        assert_eq!(serde_json::from_value::<Input>(value.clone()).unwrap(), i);
        assert_valid(&i);
        for bad in [
            json!(0),
            json!("-1"),
            json!("+1"),
            json!(""),
            json!("340282366920938463463374607431768211456"),
        ] {
            let mut v = value.clone();
            v["carryover"] = bad;
            assert!(serde_json::from_value::<Input>(v).is_err());
        }
        for bad in [
            json!(1),
            json!("-1"),
            json!("+1"),
            json!("18446744073709551616"),
        ] {
            let mut v = value.clone();
            v["blocks"][0]["height"] = bad;
            assert!(serde_json::from_value::<Input>(v).is_err());
        }
        for bytes in [0, 31, 33, 35, 37] {
            let mut v = value.clone();
            v["initial"][0]["id"] = json!("00".repeat(bytes));
            assert!(serde_json::from_value::<Input>(v).is_err());
            let mut v = value.clone();
            v["blocks"][0]["v2_seed"] = json!("00".repeat(bytes));
            assert!(serde_json::from_value::<Input>(v).is_err());
        }
        for path in [
            vec![],
            vec!["initial", "0"],
            vec!["blocks", "0"],
            vec!["blocks", "0", "candidates", "0"],
        ] {
            let mut v = value.clone();
            let mut object = &mut v;
            for key in path {
                object = if key == "0" {
                    &mut object[0]
                } else {
                    &mut object[key]
                };
            }
            object["unknown"] = json!(true);
            assert!(serde_json::from_value::<Input>(v).is_err());
        }
    }

    #[test]
    fn lottery_sim_committed_matrix_and_sparse_maximum() {
        let mut count = 0;
        let mut maximum = 0;
        for line in include_str!("../regressions/lottery_sim.jsonl")
            .lines()
            .filter(|s| !s.trim().is_empty() && !s.starts_with('#'))
        {
            let fixture: Value = serde_json::from_str(line).unwrap();
            let i: Input = serde_json::from_value(fixture["input"].clone()).unwrap();
            maximum = maximum.max(i.initial.len());
            assert_valid(&i);
            count += 1;
        }
        assert!(count >= 16);
        assert_eq!(maximum, 10_000);
    }

    #[test]
    fn lottery_sim_state_only_failure_is_in_equality_channel() {
        let i = basic();
        let mut reference = Reference::new(&i).unwrap();
        let mut sim = LotterySimulation::new_production(
            i.initial.iter().map(Output::production).collect(),
            i.carryover,
        )
        .unwrap();
        let b = reference.resolve(&i, &i.blocks[0]).unwrap();
        let (cap, a, draw) = node(&b, reference.carry);
        let result = sim.apply_production_block(&b).unwrap();
        reference.advance(&b, &a, &draw).unwrap();
        assert_eq!(node_projection(cap, &a, &draw), sim_projection(&result));
        let mut failed = vec![];
        reference.observe(&sim, "0", &mut failed);
        assert!(failed.is_empty());
        sim.metrics.total_distributed += 1;
        reference.observe(&sim, "0", &mut failed);
        assert_eq!(failed, ["0/total_distributed"]);
        // This diagnostic unit control is separate from the required actual
        // production-commit mutation + runner generation/shrink/replay in CI.
        let clean = evaluate(&i).unwrap();
        let mut changed = clean.clone();
        changed["simulator_state"] = json!({"ok": false, "failed_checks": failed});
        assert_ne!(clean, changed);

        // Valid research documentation (including an extended explanation)
        // cannot change observation of the actual corrupted simulator state.
        let mut extended: Value = serde_json::from_str(INVENTORY).unwrap();
        extended["surfaces"][1]["reason"] =
            json!("Additional research observation: legacy ticket weights differ.");
        extended["surfaces"][1]["effect_on_production_claims"] =
            json!("This research declaration grants no production waiver.");
        for document in [INVENTORY.to_owned(), extended.to_string()] {
            validate_inventory(&document).unwrap();
            assert_eq!(node_projection(cap, &a, &draw), sim_projection(&result));
            let mut observed = vec![];
            reference.observe(&sim, "0", &mut observed);
            assert_eq!(observed, ["0/total_distributed"]);
            let mut actual = clean.clone();
            actual["simulator_state"] = json!({"ok": false, "failed_checks": observed});
            assert_eq!(actual, changed);
            assert_ne!(clean, actual); // the runner's ordinary JSON equality
        }
        extended["surfaces"][0]["mismatch_exemption"] = json!("0/total_distributed");
        assert!(validate_inventory(&extended.to_string()).is_err());
        let mut still_failed = vec![];
        reference.observe(&sim, "0", &mut still_failed);
        assert_eq!(still_failed, ["0/total_distributed"]);
    }

    // Public snapshot covers rollback observables. The existing production
    // module tests additionally inspect the private allocator and last-height.
    fn snapshot(sim: &LotterySimulation) -> String {
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}/{}/{}/{}",
            sim.production_outputs(),
            sim.production_carryover(),
            sim.utxos,
            sim.owners,
            sim.metrics,
            sim.current_block,
            sim.total_burned,
            sim.lottery_pool
        )
    }

    #[test]
    fn lottery_sim_invalid_inputs_rejected_atomically() {
        let i = basic();
        let outputs: Vec<_> = i.initial.iter().map(Output::production).collect();
        let mut duplicate = outputs.clone();
        duplicate.push(outputs[0].clone());
        assert!(LotterySimulation::new_production(duplicate, 0).is_err());
        let mut max_id = outputs.clone();
        max_id[0].simulation_id = u64::MAX;
        assert!(LotterySimulation::new_production(max_id, 0).is_err());
        let mut overflow = outputs.clone();
        overflow[0].value = u64::MAX;
        overflow.push(initial(1, 7, 1, 1000, 1).production());
        assert!(LotterySimulation::new_production(overflow, 0).is_err());
        let r = Reference::new(&i).unwrap();
        let good = r.resolve(&i, &i.blocks[0]).unwrap();
        let mut bads = vec![];
        let mut b = good.clone();
        b.candidates[0].value += 1;
        bads.push(b);
        let mut b = good.clone();
        b.candidates[0].owner_id += 1;
        bads.push(b);
        let mut b = good.clone();
        b.candidates.push(b.candidates[0].clone());
        bads.push(b);
        let mut b = good.clone();
        b.candidates = vec![b.candidates[0].clone(); 10_001];
        bads.push(b);
        for b in bads {
            let mut sim = LotterySimulation::new_production(outputs.clone(), i.carryover).unwrap();
            let before = snapshot(&sim);
            assert!(sim.apply_production_block(&b).is_err());
            assert_eq!(snapshot(&sim), before);
            sim.apply_production_block(&good).unwrap(); // rejected attempt did
                                                        // not consume
                                                        // height/IDs
        }
        let mut sim = LotterySimulation::new_production(outputs.clone(), i.carryover).unwrap();
        sim.apply_production_block(&good).unwrap();
        let before = snapshot(&sim);
        assert!(sim.apply_production_block(&good).is_err());
        assert_eq!(snapshot(&sim), before);
        for corrupt in 0..3 {
            let mut sim = LotterySimulation::new_production(outputs.clone(), i.carryover).unwrap();
            match corrupt {
                0 => sim.owners.get_mut(&7).unwrap().utxo_ids.clear(),
                1 => sim.metrics.total_fees_collected = u64::MAX,
                _ => sim.metrics.total_distributed = u64::MAX,
            }
            let before = snapshot(&sim);
            assert!(sim.apply_production_block(&good).is_err());
            assert_eq!(snapshot(&sim), before);
        }
    }
}
