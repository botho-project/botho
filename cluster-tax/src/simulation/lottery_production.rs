//! Conditional production lottery parity, in picocredits.
//!
//! Call [`LotterySimulation::new_production`] once, then
//! [`LotterySimulation::apply_production_block`] for each block. The ordered
//! snapshot is a subset of registered immutable outputs, including earlier
//! awards; omitting an output does not delete its owner's balance. External
//! window sampling, spent-record retention, fee-floor resolution, emission
//! scheduling and V2 seed derivation are outside this transition. Supply
//! recorded fees or #1695's shared fee model and state its fee mode. Fees are
//! already collected externally: this entry records their split, not owner
//! debits. Legacy transaction generation and draw methods remain research and
//! must not be mixed into a production instance.
//!
//! Integer records are authoritative; the legacy float factor is only a
//! display projection. Synthetic award identities are deterministic simulator
//! identifiers, NOT ledger cryptographic outpoints. Representability and
//! mapping errors below are simulator restrictions, not node validity rules.

use std::collections::{BTreeMap, BTreeSet};

use super::{LotteryConfig, LotteryOwner, LotterySimulation, LotteryUtxo, SybilStrategy};
use crate::{
    consensus_fee::split_fee_pool_burn,
    lottery::{
        count_eligible, draw_winners_with_seed, sybil_reward_cap, LotteryCandidate,
        LotteryDrawConfig, FACTOR_SCALE,
    },
    FeeCurve,
};

/// Mirror of the node's bounded, externally selected candidate window.
/// The integration parity guard pins this to the node source constant.
pub const MAX_PRODUCTION_CANDIDATES: usize = 10_000;

/// An immutable registered output and its unambiguous owner mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionOutput {
    pub id: [u8; 36],
    pub simulation_id: u64,
    pub owner_id: u64,
    pub value: u64,
    pub cluster_factor: u64,
    pub creation_block: u64,
}

impl ProductionOutput {
    /// Uniform selection ignores entropy; no float-derived monetary inputs.
    pub fn candidate(&self) -> LotteryCandidate {
        LotteryCandidate {
            id: self.id,
            value: self.value,
            cluster_factor: self.cluster_factor,
            creation_block: self.creation_block,
            tag_entropy: 0.0,
        }
    }
}

/// Already-selected candidate order and already-derived V2 seed.
#[derive(Clone, Debug)]
pub struct ProductionBlock {
    pub height: u64,
    pub total_fees: u64,
    pub emission_share: u64,
    pub block_reward: u64,
    pub v2_seed: [u8; 32],
    pub candidates: Vec<ProductionOutput>,
}

/// Both the selected source and the fresh output actually credited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionAward {
    pub winner_id: [u8; 36],
    pub output: ProductionOutput,
    /// Quintiles use pre-block owner balances, with equal wealth sharing the
    /// first equal rank (the legacy convention), before any awards are added.
    pub quintile: usize,
}

/// Node draw projection plus exact accounting and applied credits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionBlockResult {
    pub block_height: u64,
    pub total_fees: u64,
    pub fee_pool: u64,
    pub burn_amount: u64,
    pub emission_share: u64,
    pub available: u128,
    pub payout_cap: u64,
    pub payout: u64,
    /// Node BlockLotteryResult projection: zero for an empty drawing.
    pub pool_amount: u64,
    /// Node projection: all zeroes for an empty drawing.
    pub seed: [u8; 32],
    pub paid: u64,
    pub carryover: u128,
    /// In draw order, including awards with value zero.
    pub awards: Vec<ProductionAward>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProductionError {
    NotProduction,
    InvalidSnapshot(&'static str),
    InconsistentState(&'static str),
    Overflow(&'static str),
    IdentityCollision,
}

#[derive(Debug)]
pub(super) struct ProductionState {
    outputs: BTreeMap<[u8; 36], ProductionOutput>,
    carryover: u128,
    last_height: Option<u64>,
}

fn add(a: u64, b: u64, field: &'static str) -> Result<u64, ProductionError> {
    a.checked_add(b).ok_or(ProductionError::Overflow(field))
}

fn synthetic_id(serial: u64) -> [u8; 36] {
    let mut id = [0; 36];
    id[..26].copy_from_slice(b"BOTHO_SIM_LOTTERY_AWARD_V1");
    id[28..].copy_from_slice(&serial.to_be_bytes());
    id
}

impl LotterySimulation {
    /// Register the initial integer inventory once. Owners are inferred from
    /// these outputs; duplicate candidate/simulator IDs and unrepresentable
    /// owner balances are rejected. Use a separate instance for legacy modes.
    /// Neither snapshots nor later blocks can reinitialize this inventory.
    pub fn new_production(
        outputs: Vec<ProductionOutput>,
        carryover: u128,
    ) -> Result<Self, ProductionError> {
        let mut sim = Self::new(LotteryConfig::default(), FeeCurve::default());
        let mut registered = BTreeMap::new();
        for output in outputs {
            if registered.contains_key(&output.id) || sim.utxos.contains_key(&output.simulation_id)
            {
                return Err(ProductionError::IdentityCollision);
            }
            sim.next_utxo_id = sim
                .next_utxo_id
                .max(add(output.simulation_id, 1, "identity")?);
            let owner = sim
                .owners
                .entry(output.owner_id)
                .or_insert_with(|| LotteryOwner::new(output.owner_id, SybilStrategy::Normal));
            owner.utxo_ids.push(output.simulation_id);
            sim.utxos.insert(
                output.simulation_id,
                LotteryUtxo::new(
                    output.simulation_id,
                    output.owner_id,
                    output.value,
                    output.cluster_factor as f64 / FACTOR_SCALE as f64,
                    output.creation_block,
                ),
            );
            registered.insert(output.id, output);
        }
        sim.production = Some(ProductionState {
            outputs: registered,
            carryover,
            last_height: None,
        });
        sim.validate_production_state()?;
        Ok(sim)
    }

    /// Registered integer records, including all prior awards. A caller may
    /// select/reorder these for the next externally constructed snapshot.
    pub fn production_outputs(&self) -> Option<&BTreeMap<[u8; 36], ProductionOutput>> {
        self.production.as_ref().map(|state| &state.outputs)
    }

    /// Separate from the legacy u64 lottery pool; never narrowed to it.
    pub fn production_carryover(&self) -> Option<u128> {
        self.production.as_ref().map(|state| state.carryover)
    }

    fn validate_production_state(&self) -> Result<BTreeMap<u64, u64>, ProductionError> {
        use ProductionError::InconsistentState as Bad;
        let state = self
            .production
            .as_ref()
            .ok_or(ProductionError::NotProduction)?;
        if self.lottery_pool != 0
            || state.outputs.len() != self.utxos.len()
            || state
                .last_height
                .is_some_and(|height| height != self.current_block)
        {
            return Err(Bad("production/legacy state"));
        }
        let mut balances = BTreeMap::new();
        let mut membership = BTreeSet::new();
        for (&owner_id, owner) in &self.owners {
            if owner.id != owner_id {
                return Err(Bad("owner identity"));
            }
            let mut balance = 0;
            for id in &owner.utxo_ids {
                let output = self.utxos.get(id).ok_or(Bad("missing owner output"))?;
                if output.owner_id != owner_id || !membership.insert(*id) {
                    return Err(Bad("owner membership"));
                }
                balance = add(balance, output.value, "owner balance")?;
            }
            balances.insert(owner_id, balance);
        }
        if membership.len() != self.utxos.len() {
            return Err(Bad("unowned output"));
        }
        let mut mapped = BTreeSet::new();
        for output in state.outputs.values() {
            let stored = self
                .utxos
                .get(&output.simulation_id)
                .ok_or(Bad("missing output"))?;
            if !mapped.insert(output.simulation_id)
                || stored.id != output.simulation_id
                || stored.owner_id != output.owner_id
                || stored.value != output.value
                || stored.creation_block != output.creation_block
            {
                return Err(Bad("integer output mapping"));
            }
        }
        Ok(balances)
    }

    /// Apply one production block atomically. Heights must increase after the
    /// first block; gaps are allowed. Every snapshot record must exactly match
    /// its registered output, so values/ages/owners cannot reset between calls.
    /// The shared canonical draw ignores legacy config and RNG. Fee splitting
    /// reuses #1695; only the node-only cap/pool composition is mirrored here.
    /// All fallible validation and bookkeeping precedes the commit section.
    pub fn apply_production_block(
        &mut self,
        block: &ProductionBlock,
    ) -> Result<ProductionBlockResult, ProductionError> {
        use ProductionError::InvalidSnapshot as Bad;
        let mut balances = self.validate_production_state()?;
        let state = self
            .production
            .as_ref()
            .ok_or(ProductionError::NotProduction)?;
        if state
            .last_height
            .is_some_and(|height| block.height <= height)
        {
            return Err(Bad("height must increase"));
        }
        if block.candidates.len() > MAX_PRODUCTION_CANDIDATES {
            return Err(Bad("candidate bound"));
        }
        let mut seen = BTreeSet::new();
        for output in &block.candidates {
            if !seen.insert(output.id) {
                return Err(Bad("duplicate candidate"));
            }
            if state.outputs.get(&output.id) != Some(output) {
                return Err(Bad("unregistered or mismatched output"));
            }
        }
        let config = LotteryDrawConfig::default();
        let candidates: Vec<_> = block
            .candidates
            .iter()
            .map(ProductionOutput::candidate)
            .collect();
        let payout_cap = sybil_reward_cap(count_eligible(&candidates, block.height, &config))
            .min(block.block_reward as u128) as u64;
        let (fee_pool, burn_amount) = split_fee_pool_burn(block.total_fees);
        let available = state
            .carryover
            .saturating_add(block.emission_share as u128)
            .saturating_add(fee_pool as u128);
        let payout = available.min(payout_cap as u128) as u64;
        let drawing =
            draw_winners_with_seed(&candidates, payout, block.height, &block.v2_seed, &config);
        let mut result = ProductionBlockResult {
            block_height: block.height,
            total_fees: block.total_fees,
            fee_pool,
            burn_amount,
            emission_share: block.emission_share,
            available,
            payout_cap,
            payout,
            pool_amount: 0,
            seed: [0; 32],
            paid: 0,
            carryover: available,
            awards: Vec::new(),
        };
        let mut wealths: Vec<_> = balances.values().copied().collect();
        wealths.sort_unstable();
        let quintiles: BTreeMap<_, _> = balances
            .iter()
            .map(|(&id, &wealth)| {
                let rank = wealths.partition_point(|&w| w < wealth);
                (id, ((rank as u128 * 5) / wealths.len() as u128) as usize)
            })
            .collect();
        let mut winnings: BTreeMap<_, _> = self
            .owners
            .iter()
            .map(|(&id, o)| (id, o.total_winnings))
            .collect();
        let mut by_quintile = self.metrics.winnings_by_quintile;
        let mut next_id = self.next_utxo_id;
        if let Some(draw) = drawing {
            result.pool_amount = draw.pool_amount;
            result.seed = draw.seed;
            for winner in draw.winners {
                let source = &state.outputs[&winner.utxo_id];
                let id = synthetic_id(next_id);
                if self.utxos.contains_key(&next_id) || state.outputs.contains_key(&id) {
                    return Err(ProductionError::IdentityCollision);
                }
                let output = ProductionOutput {
                    id,
                    simulation_id: next_id,
                    owner_id: source.owner_id,
                    value: winner.payout,
                    cluster_factor: FACTOR_SCALE,
                    creation_block: block.height,
                };
                next_id = add(next_id, 1, "identity")?;
                let owner = source.owner_id;
                balances.insert(
                    owner,
                    add(balances[&owner], winner.payout, "owner balance")?,
                );
                winnings.insert(
                    owner,
                    add(winnings[&owner], winner.payout, "owner winnings")?,
                );
                let quintile = quintiles[&owner];
                by_quintile[quintile] =
                    add(by_quintile[quintile], winner.payout, "quintile winnings")?;
                result.paid = add(result.paid, winner.payout, "block paid")?;
                result.awards.push(ProductionAward {
                    winner_id: winner.utxo_id,
                    output,
                    quintile,
                });
            }
        }
        result.carryover = available - result.paid as u128;
        let total_distributed = add(
            self.metrics.total_distributed,
            result.paid,
            "total distributed",
        )?;
        let total_fees = add(
            self.metrics.total_fees_collected,
            block.total_fees,
            "total fees",
        )?;
        let total_burned = add(self.total_burned, burn_amount, "total burned")?;
        let drawings = add(
            self.metrics.drawings_held,
            u64::from(!result.awards.is_empty()),
            "drawings",
        )?;

        // Commit: every identity, owner and affected u64 sum has been checked.
        for award in &result.awards {
            let output = &award.output;
            self.utxos.insert(
                output.simulation_id,
                LotteryUtxo::new(
                    output.simulation_id,
                    output.owner_id,
                    output.value,
                    1.0,
                    output.creation_block,
                ),
            );
            self.owners
                .get_mut(&output.owner_id)
                .expect("validated owner")
                .utxo_ids
                .push(output.simulation_id);
        }
        for (id, amount) in winnings {
            self.owners
                .get_mut(&id)
                .expect("validated owner")
                .total_winnings = amount;
        }
        self.next_utxo_id = next_id;
        self.current_block = block.height;
        self.total_burned = total_burned;
        self.metrics.total_distributed = total_distributed;
        self.metrics.total_fees_collected = total_fees;
        self.metrics.drawings_held = drawings;
        self.metrics.winnings_by_quintile = by_quintile;
        let state = self
            .production
            .as_mut()
            .expect("validated production state");
        state.carryover = result.carryover;
        state.last_height = Some(block.height);
        for award in &result.awards {
            state.outputs.insert(award.output.id, award.output.clone());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lottery::{CIRCULATION_WINDOW_BLOCKS, LOTTERY_BASE_FEE_PICO};

    fn output(id: u64, owner_id: u64) -> ProductionOutput {
        let mut identity = [0; 36];
        identity[..8].copy_from_slice(&id.to_le_bytes());
        ProductionOutput {
            id: identity,
            simulation_id: id,
            owner_id,
            value: 1_000_000,
            cluster_factor: FACTOR_SCALE,
            creation_block: 0,
        }
    }

    fn fixture(n: u64, pool: u128) -> (LotterySimulation, ProductionBlock) {
        // Several winning outputs deliberately belong to the same owner.
        let outputs: Vec<_> = (1..=n).map(|id| output(id, id % 2)).collect();
        let sim = LotterySimulation::new_production(outputs.clone(), pool).unwrap();
        (
            sim,
            ProductionBlock {
                height: 720,
                total_fees: 0,
                emission_share: 0,
                block_reward: u64::MAX,
                v2_seed: [9; 32],
                candidates: outputs,
            },
        )
    }

    fn snapshot(sim: &LotterySimulation) -> String {
        format!(
            "{:?}",
            (
                &sim.production,
                &sim.utxos,
                &sim.owners,
                sim.current_block,
                sim.next_utxo_id,
                sim.lottery_pool,
                sim.total_burned,
                &sim.metrics
            )
        )
    }

    fn rejected(sim: &mut LotterySimulation, block: &ProductionBlock, expected: ProductionError) {
        let before = snapshot(sim);
        assert_eq!(sim.apply_production_block(block), Err(expected));
        assert_eq!(
            snapshot(sim),
            before,
            "rejection must precede every mutation"
        );
    }

    #[test]
    fn production_draw_matrix_credits_exactly_and_preserves_sources() {
        for n in [0, 1, 3, 4, 7] {
            for pool in [0, 1, 3, 11, 10_000_000_000_000u128] {
                let (mut sim, block) = fixture(n, pool);
                let originals = format!("{:?}", sim.utxos);
                let before: u64 = sim.owners.keys().map(|&id| sim.owner_value(id)).sum();
                let result = sim.apply_production_block(&block).unwrap();
                assert_eq!(result.payout_cap, n * LOTTERY_BASE_FEE_PICO);
                assert_eq!(result.paid, pool.min(result.payout_cap as u128) as u64);
                assert_eq!(
                    result.awards.len(),
                    if pool == 0 { 0 } else { n.min(4) as usize }
                );
                assert_eq!(
                    result.awards.iter().map(|a| a.output.value).sum::<u64>(),
                    result.paid
                );
                assert_eq!(sim.production_carryover(), Some(pool - result.paid as u128));
                assert_eq!(sim.metrics.total_distributed, result.paid);
                assert_eq!(
                    sim.metrics.winnings_by_quintile.iter().sum::<u64>(),
                    result.paid
                );
                assert_eq!(
                    sim.owners.values().map(|o| o.total_winnings).sum::<u64>(),
                    result.paid
                );
                assert_eq!(
                    sim.owners
                        .keys()
                        .map(|&id| sim.owner_value(id))
                        .sum::<u64>(),
                    before + result.paid
                );
                let sources: BTreeMap<_, _> =
                    sim.utxos.iter().filter(|(id, _)| **id <= n).collect();
                assert_eq!(format!("{sources:?}"), originals);
                if result.awards.is_empty() {
                    assert_eq!((result.pool_amount, result.seed), (0, [0; 32]));
                } else {
                    assert_eq!(result.seed, block.v2_seed);
                    let count = result.awards.len() as u64;
                    for (i, award) in result.awards.iter().enumerate() {
                        let expected = result.paid / count
                            + if i + 1 == count as usize {
                                result.paid % count
                            } else {
                                0
                            };
                        assert_eq!(award.output.value, expected);
                        let source = block
                            .candidates
                            .iter()
                            .find(|o| o.id == award.winner_id)
                            .unwrap();
                        assert_eq!(award.output.owner_id, source.owner_id);
                        assert_eq!(award.output.creation_block, 720);
                        assert_eq!(sim.utxos[&award.output.simulation_id].value, expected);
                    }
                }
                assert_eq!(sim.lottery_pool, 0);
            }
        }
    }

    #[test]
    fn production_carryover_and_fresh_output_maturity_across_blocks() {
        let (mut sim, mut block) = fixture(4, 10_000_000);
        block.block_reward = 4_000_000;
        let first = sim.apply_production_block(&block).unwrap();
        assert_eq!((first.paid, first.carryover), (4_000_000, 6_000_000));
        block.height += 1;
        block.candidates = sim
            .production_outputs()
            .unwrap()
            .values()
            .cloned()
            .collect();
        let second = sim.apply_production_block(&block).unwrap();
        assert_eq!((second.paid, second.carryover), (4_000_000, 2_000_000));
        assert!(second
            .awards
            .iter()
            .all(|a| first.awards.iter().all(|old| old.output.id != a.winner_id)));
        // Only the first fresh awards, one block short of maturity.
        block.candidates = first.awards.iter().map(|a| a.output.clone()).collect();
        block.height = 1439;
        let immature = sim.apply_production_block(&block).unwrap();
        assert!(immature.awards.is_empty());
        assert_eq!(immature.carryover, 2_000_000);
        block.height = 1440;
        let mature = sim.apply_production_block(&block).unwrap();
        assert_eq!(
            (mature.awards.len(), mature.paid, mature.carryover),
            (4, 2_000_000, 0)
        );
        assert_eq!(sim.metrics.total_distributed, 10_000_000);
        assert_eq!(sim.production_outputs().unwrap().len(), 16);
    }

    #[test]
    fn production_zero_awards_are_created_and_fail_later_value_gate() {
        let (mut sim, mut block) = fixture(4, 1);
        let first = sim.apply_production_block(&block).unwrap();
        assert_eq!(
            first
                .awards
                .iter()
                .map(|a| a.output.value)
                .collect::<Vec<_>>(),
            [0, 0, 0, 1]
        );
        assert_eq!(sim.utxos.len(), 8);
        block.height += 720;
        block.emission_share = 100;
        block.candidates = first.awards.into_iter().map(|a| a.output).collect();
        let later = sim.apply_production_block(&block).unwrap();
        assert_eq!(
            (later.payout_cap, later.pool_amount, later.carryover),
            (0, 0, 100)
        );
    }

    #[test]
    fn production_age_window_and_zero_cap_boundaries() {
        for (height, eligible) in [
            (0, false),
            (719, false),
            (720, true),
            (CIRCULATION_WINDOW_BLOCKS, true),
            (CIRCULATION_WINDOW_BLOCKS + 1, false),
        ] {
            let (mut sim, mut block) = fixture(1, 10);
            block.height = height;
            assert_eq!(
                sim.apply_production_block(&block).unwrap().paid,
                if eligible { 10 } else { 0 }
            );
        }
        let (mut sim, mut block) = fixture(1, 10);
        block.block_reward = 0;
        block.total_fees = 7;
        let result = sim.apply_production_block(&block).unwrap();
        assert_eq!(
            (
                result.pool_amount,
                result.seed,
                result.burn_amount,
                result.carryover
            ),
            (0, [0; 32], 2, 15)
        );
    }

    #[test]
    fn production_u128_saturation_is_supported_u64_counter_overflow_is_not() {
        let (mut sim, mut block) = fixture(4, u128::MAX - 1);
        block.total_fees = u64::MAX;
        block.emission_share = u64::MAX;
        block.block_reward = 7;
        let result = sim.apply_production_block(&block).unwrap();
        assert_eq!(result.available, u128::MAX);
        assert_eq!(result.carryover, u128::MAX - 7);
        assert_eq!(
            result.fee_pool as u128 + result.burn_amount as u128,
            u64::MAX as u128
        );
        block.height += 1;
        block.total_fees = 1;
        rejected(&mut sim, &block, ProductionError::Overflow("total fees"));
    }

    #[test]
    fn production_invalid_snapshots_roll_back() {
        let (mut sim, block) = fixture(4, 100);
        for mutate in 0..5 {
            let mut bad = block.clone();
            match mutate {
                0 => bad.candidates[0].owner_id = 99,
                1 => bad.candidates[0].value += 1,
                2 => bad.candidates[0].creation_block += 1,
                3 => bad.candidates[0].simulation_id = 999,
                _ => bad.candidates[0].id = [99; 36],
            }
            rejected(
                &mut sim,
                &bad,
                ProductionError::InvalidSnapshot("unregistered or mismatched output"),
            );
        }
        let mut bad = block.clone();
        bad.candidates.push(block.candidates[0].clone());
        rejected(
            &mut sim,
            &bad,
            ProductionError::InvalidSnapshot("duplicate candidate"),
        );
        bad.candidates = vec![block.candidates[0].clone(); MAX_PRODUCTION_CANDIDATES + 1];
        rejected(
            &mut sim,
            &bad,
            ProductionError::InvalidSnapshot("candidate bound"),
        );
        sim.apply_production_block(&block).unwrap();
        rejected(
            &mut sim,
            &block,
            ProductionError::InvalidSnapshot("height must increase"),
        );
    }

    #[test]
    fn production_mapping_and_allocator_failures_roll_back() {
        for case in 0..7 {
            let (mut sim, block) = fixture(4, 100);
            let expected = match case {
                0 => {
                    sim.owners.get_mut(&1).unwrap().utxo_ids.push(1);
                    ProductionError::InconsistentState("owner membership")
                }
                1 => {
                    sim.owners.remove(&1);
                    ProductionError::InconsistentState("unowned output")
                }
                2 => {
                    sim.utxos.get_mut(&1).unwrap().value += 1;
                    ProductionError::InconsistentState("integer output mapping")
                }
                3 => {
                    sim.next_utxo_id = 1;
                    ProductionError::IdentityCollision
                }
                4 => {
                    sim.next_utxo_id = u64::MAX - 1;
                    ProductionError::Overflow("identity")
                }
                5 => {
                    sim.lottery_pool = 1;
                    ProductionError::InconsistentState("production/legacy state")
                }
                _ => {
                    // A legitimate initial external identity can collide with the synthetic
                    // namespace.
                    let mut outputs = block.candidates.clone();
                    outputs[0].id = synthetic_id(5);
                    sim = LotterySimulation::new_production(outputs, 100).unwrap();
                    let mut updated = block.clone();
                    updated.candidates = sim
                        .production_outputs()
                        .unwrap()
                        .values()
                        .cloned()
                        .collect();
                    rejected(&mut sim, &updated, ProductionError::IdentityCollision);
                    continue;
                }
            };
            rejected(&mut sim, &block, expected);
        }
    }

    #[test]
    fn production_each_affected_counter_is_checked_before_commit() {
        for field in [
            "owner balance",
            "owner winnings",
            "quintile winnings",
            "total distributed",
            "total fees",
            "total burned",
            "drawings",
        ] {
            let (mut sim, mut block) = fixture(1, 100);
            block.total_fees = 10;
            match field {
                "owner balance" => {
                    let mut original = block.candidates[0].clone();
                    original.value = u64::MAX;
                    sim = LotterySimulation::new_production(vec![original.clone()], 100).unwrap();
                    block.candidates = vec![original];
                }
                "owner winnings" => sim.owners.get_mut(&1).unwrap().total_winnings = u64::MAX,
                "quintile winnings" => sim.metrics.winnings_by_quintile[0] = u64::MAX,
                "total distributed" => sim.metrics.total_distributed = u64::MAX,
                "total fees" => sim.metrics.total_fees_collected = u64::MAX,
                "total burned" => sim.total_burned = u64::MAX,
                "drawings" => sim.metrics.drawings_held = u64::MAX,
                _ => unreachable!(),
            }
            rejected(&mut sim, &block, ProductionError::Overflow(field));
        }
        let mut a = output(1, 1);
        a.value = u64::MAX;
        assert!(matches!(
            LotterySimulation::new_production(vec![a, output(2, 1)], 0),
            Err(ProductionError::Overflow("owner balance"))
        ));
        assert!(matches!(
            LotterySimulation::new_production(vec![output(1, 1), output(1, 2)], 0),
            Err(ProductionError::IdentityCollision)
        ));
        let (mut sim, block) = fixture(1, 0);
        sim.production = None;
        rejected(&mut sim, &block, ProductionError::NotProduction);
    }

    #[test]
    fn production_quintiles_use_one_pre_block_state() {
        let outputs: Vec<_> = (1..=5)
            .map(|id| {
                let mut o = output(id, id);
                o.value *= id;
                o
            })
            .collect();
        let mut sim = LotterySimulation::new_production(outputs.clone(), 100_000_000).unwrap();
        let block = ProductionBlock {
            candidates: outputs,
            ..fixture(0, 0).1
        };
        let result = sim.apply_production_block(&block).unwrap();
        for award in result.awards {
            assert_eq!(award.quintile, award.output.owner_id as usize - 1);
        }
    }

    #[test]
    fn production_entry_does_not_change_seeded_legacy_execution() {
        fn run() -> String {
            let mut sim =
                LotterySimulation::new_seeded(LotteryConfig::default(), FeeCurve::default(), 1968);
            for _ in 0..4 {
                sim.add_owner(1_000_000, SybilStrategy::Normal);
            }
            sim.current_block = 720;
            sim.lottery_pool = 101;
            sim.run_drawing();
            // Exercise the RNG stream as well as the old proportional path.
            sim.simulate_transaction_immediate(1000, 2, super::super::TransactionModel::Uniform);
            assert_eq!(sim.metrics.drawings_held, 1);
            assert_eq!(sim.metrics.total_distributed, 15_301);
            assert!(sim.production.is_none());
            snapshot(&sim)
        }
        assert_eq!(run(), run());
    }
}
