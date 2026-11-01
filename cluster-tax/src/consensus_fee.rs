//! One shared consensus fee model for every economic simulation (#1695).
//!
//! Before this module each simulation priced fees its own way — a linear bps
//! [`crate::FeeCurve`] on transfer value (runner, whale, wash-trading,
//! structuring), a flat `base_fee` of 0.25 BTH (m2, lottery), or 1e9 pico per
//! byte (the asymmetric-fees spec). None of them matched what the node
//! actually enforces, and they overstated the cost of an attack by 10³–10⁹.
//!
//! [`consensus_fee_model`] is the **pure** half of the node's consensus fee
//! floor, `Ledger::consensus_fee_floor`
//! (`botho/src/ledger/store/validation.rs`):
//!
//! ```text
//! floor = FeeConfig::default().minimum_fee_dynamic_with_outputs(
//!             Hidden, size, claimed_wealth, n_out, n_memo, CONSENSUS_FEE_BASE)
//!       + spend_demurrage_charge(output_sum, max(claimed_factor, floor_factor),
//!             claimed_factor, elapsed, demurrage_rate_bps(height), blocks_per_year)
//! ```
//!
//! i.e. `1 pico/byte × size × factor × min(n_out, 10)²` plus memo fees, plus
//! demurrage (accrued-to-date, or the capitalized class-downgrade charge,
//! whichever is larger).
//!
//! The ledger resolves the DB-dependent inputs **before** it runs this math:
//! the output-tag effective cluster wealth (`claimed_wealth`), the composed
//! ring-centroid / bridge-import factor floor (`floor_factor`) and the
//! max-quantile ring age (`elapsed`). A simulation does not have a UTXO DB, so
//! it supplies those inputs from its own state; the arithmetic after that is
//! the node's arithmetic, built from the same `cluster-tax` primitives. A
//! golden-vector test in `botho` (`consensus_fee_model_matches_ledger_floor_*`
//! in `botho/src/ledger/store.rs`) pins this function to
//! `Ledger::consensus_fee_floor` so drift fails CI.
//!
//! Two modes ([`FeeModelMode`]):
//! - [`FeeModelMode::ConsensusOnly`]: the block-validity floor. This is what an
//!   attacker who mines (or bribes a miner to include) its own txs pays — the
//!   conservative attack-cost lower bound.
//! - [`FeeModelMode::ConsensusPlusRelayMin`]: additionally applies the default
//!   node's relay/mempool minimum `MIN_TX_FEE` (1e8 pico = 0.0001 BTH):
//!   `max(floor, MIN_TX_FEE)`. This is what an ordinary wallet pays.
//!
//! Nothing here is consensus code: the node does not call it. It exists so the
//! simulators price fees the way the node does.

use crate::{
    demurrage::spend_demurrage_charge,
    fee_curve::{ClusterFactorCurve, FeeConfig, TransactionType, OUTPUT_FEE_EXPONENT_SCALE},
    MonetaryPolicy,
};

/// The node's congestion-free consensus fee base, in picocredits per byte.
///
/// Mirrors `CONSENSUS_FEE_BASE` in `botho/src/ledger/store.rs` (which is
/// private). The golden-vector test in `botho` fails if they diverge.
pub const CONSENSUS_FEE_BASE: u64 = 1;

/// The relay/mempool minimum fee, in picocredits (0.0001 BTH).
///
/// Mirrors `bth_transaction_clsag::MIN_TX_FEE`; `cluster-tax` does not depend
/// on that crate, so a drift-guard test in `botho`
/// (`consensus_fee_model_relay_min_matches_min_tx_fee`) asserts equality.
pub const RELAY_MIN_TX_FEE: u64 = 100_000_000;

/// Fraction of a block's total fees (base + demurrage) routed to the lottery
/// pool, in permille; the remainder is burned.
///
/// Mirrors the consensus
/// `lottery_v2::canonical_config().pool_fraction_permille`
/// and `LotteryFeeConfig::default()` (`botho/src/consensus/lottery.rs`), which
/// `cluster-tax` cannot import. A drift-guard test in `botho`
/// (`consensus_fee_model_pool_split_and_ring_size_match_node`) asserts
/// equality, and that [`split_fee_pool_burn`] matches `split_fees`.
pub const LOTTERY_POOL_FRACTION_PERMILLE: u32 = 800;

/// Ring size (real input + decoys) the node requires of every CLSAG input.
///
/// Mirrors `bth_transaction_clsag::MIN_RING_SIZE` / `DEFAULT_RING_SIZE`;
/// drift-guarded in `botho` by
/// `consensus_fee_model_pool_split_and_ring_size_match_node`. Sims that model
/// the ring-centroid floor build rings of this size.
pub const CONSENSUS_RING_SIZE: usize = 20;

/// The wallet's factor-similarity decoy band, in bps of the real input's
/// factor (±10%): honest wallets draw decoys only from coins in this band.
///
/// Mirrors `botho::decoy_selection::FACTOR_SIMILARITY_SPREAD` (wallet policy,
/// not consensus); drift-guarded in `botho` alongside [`CONSENSUS_RING_SIZE`]
/// (`consensus_fee_model_pool_split_and_ring_size_match_node`).
pub const WALLET_FACTOR_SIMILARITY_SPREAD_BPS: u64 = 1_000;

/// Split a fee into `(pool, burn)` exactly as the node's
/// `LotteryFeeConfig::split_fees` does: `pool = fee × 800 / 1000` (floored),
/// `burn = fee − pool`.
pub const fn split_fee_pool_burn(fee: u64) -> (u64, u64) {
    let pool = (fee as u128 * LOTTERY_POOL_FRACTION_PERMILLE as u128 / 1000) as u64;
    (pool, fee - pool)
}

/// Quantile (bps) of ring-member ages the node uses for the demurrage clock
/// (`CONSENSUS_RING_AGE_QUANTILE_BPS`: the maximum age). Exposed so a sim that
/// models rings can call [`crate::ring_elapsed_quantile`] the same way.
pub const CONSENSUS_RING_AGE_QUANTILE_BPS: u32 = 10_000;

/// Mainnet blocks per year at the 5 s monetary block-time assumption
/// (`botho::monetary::mainnet_policy`): `365 × 86400 / 5`.
pub const MAINNET_BLOCKS_PER_YEAR: u64 = 6_307_200;

/// Mainnet halving interval (`botho::monetary::mainnet_policy`). Demurrage is
/// zero during epoch 0 (`height < halving_interval`).
pub const MAINNET_HALVING_INTERVAL: u64 = MAINNET_BLOCKS_PER_YEAR;

/// Size-estimate constants, mirroring `BothoTransaction::estimate_size`
/// (`transaction/clsag/src/lib.rs`). Drift-guarded by the `botho` golden test.
const TX_HEADER_SIZE: usize = 50;
const TX_CLSAG_INPUT_SIZE: usize = 700;
const TX_OUTPUT_SIZE: usize = 120;
const TX_OUTPUT_MEMO_SIZE: usize = 66;

/// Estimated size in bytes of a CLSAG transaction, exactly as the node's
/// `BothoTransaction::estimate_size` computes it (the size the consensus floor
/// prices).
pub const fn consensus_tx_size_bytes(n_inputs: usize, n_outputs: usize, n_memos: usize) -> usize {
    TX_HEADER_SIZE
        + n_inputs * TX_CLSAG_INPUT_SIZE
        + n_outputs * TX_OUTPUT_SIZE
        + n_memos * TX_OUTPUT_MEMO_SIZE
}

/// Whether the model applies only the consensus floor or also the default
/// relay minimum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeModelMode {
    /// The block-validity floor only (attack-cost lower bound).
    ConsensusOnly,
    /// `max(consensus floor, RELAY_MIN_TX_FEE)` — what a default node relays.
    ConsensusPlusRelayMin,
}

impl FeeModelMode {
    /// Short, stable label for reports.
    pub fn label(self) -> &'static str {
        match self {
            FeeModelMode::ConsensusOnly => "consensus",
            FeeModelMode::ConsensusPlusRelayMin => "consensus+relay",
        }
    }
}

/// The non-tx parameters of the model: the mode and the demurrage schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsensusFeeParams {
    /// Consensus-only or consensus + relay minimum.
    pub mode: FeeModelMode,
    /// Blocks per year used to convert elapsed blocks to years of demurrage.
    pub blocks_per_year: u64,
    /// Height at which demurrage switches on (the end of epoch 0).
    pub halving_interval: u64,
}

impl ConsensusFeeParams {
    /// Mainnet parameters (`botho::monetary::mainnet_policy`).
    pub const fn mainnet(mode: FeeModelMode) -> Self {
        Self {
            mode,
            blocks_per_year: MAINNET_BLOCKS_PER_YEAR,
            halving_interval: MAINNET_HALVING_INTERVAL,
        }
    }

    /// Parameters derived from a monetary policy, the same way the ledger
    /// derives `blocks_per_year` (`365 × 86400 / target_block_time_secs`).
    pub fn from_policy(policy: &MonetaryPolicy, mode: FeeModelMode) -> Self {
        Self {
            mode,
            blocks_per_year: (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1),
            halving_interval: policy.halving_interval,
        }
    }

    /// Override `blocks_per_year` (for sims whose block-time convention differs
    /// from mainnet's; the annual demurrage rate is unchanged).
    pub const fn with_blocks_per_year(mut self, blocks_per_year: u64) -> Self {
        self.blocks_per_year = blocks_per_year;
        self
    }

    /// The consensus demurrage rate at `height`, via
    /// [`MonetaryPolicy::demurrage_rate_bps`] (zero in epoch 0).
    pub fn demurrage_rate_bps(&self, height: u64) -> u32 {
        MonetaryPolicy {
            halving_interval: self.halving_interval,
            ..MonetaryPolicy::default()
        }
        .demurrage_rate_bps(height)
    }

    /// A height in epoch 1 (demurrage active) — the first post-bootstrap block.
    pub const fn first_demurrage_height(&self) -> u64 {
        self.halving_interval
    }
}

/// The spender-declared output class, either as effective cluster wealth (what
/// the ledger resolves from output tags) or directly as a factor (for sims that
/// track factors rather than wealth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimedClass {
    /// Effective cluster wealth of the outputs, in picocredits. The factor is
    /// `ClusterFactorCurve::default().factor(wealth)`, exactly as the ledger.
    Wealth(u128),
    /// Cluster factor in `FACTOR_SCALE` (1000 = 1×).
    Factor(u64),
}

/// The per-transaction inputs to [`consensus_fee_model`]. Field names follow
/// the #1695 signature `(size, n_out, n_memo, claimed, floor, elapsed,
/// height)`, plus the output value the demurrage term is charged on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsensusFeeInputs {
    /// `BothoTransaction::estimate_size` (see [`consensus_tx_size_bytes`]).
    pub size: usize,
    /// Number of outputs.
    pub n_out: usize,
    /// Number of outputs carrying an encrypted memo.
    pub n_memo: usize,
    /// The spender-declared output class.
    pub claimed: ClaimedClass,
    /// The composed input-class floor factor the spender cannot rewrite —
    /// `max(ring_centroid_implied_factor, bridge-import floor)` in
    /// `FACTOR_SCALE`. Use `FACTOR_SCALE` (1×) for "no floor"; the model takes
    /// `max(claimed_factor, floor)` exactly as the ledger does.
    pub floor: u64,
    /// Max-quantile ring age in blocks (`ring_elapsed_quantile` at
    /// [`CONSENSUS_RING_AGE_QUANTILE_BPS`]).
    pub elapsed: u64,
    /// The validating block height (gates the demurrage rate).
    pub height: u64,
    /// Sum of output amounts in picocredits (the demurrage base).
    pub output_sum: u64,
}

/// The decomposed fee: the size/output/memo base and the demurrage charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsensusFeeBreakdown {
    /// `fee_per_byte × size × factor × min(n_out, cap)² + memo fees`.
    pub base: u64,
    /// `spend_demurrage_charge(..)`: accrued or capitalized downgrade.
    pub demurrage: u64,
    /// `base + demurrage` — `Ledger::consensus_fee_floor`.
    pub consensus_floor: u64,
    /// The fee under the selected mode (`consensus_floor`, or
    /// `max(consensus_floor, RELAY_MIN_TX_FEE)`).
    pub fee: u64,
    /// The claimed (output) factor, `FACTOR_SCALE`.
    pub claimed_factor: u64,
    /// The demurrage factor `max(claimed_factor, floor)`, `FACTOR_SCALE`.
    pub demurrage_factor: u64,
}

/// The size/output/memo part of the floor for a known factor. Identical
/// arithmetic to `FeeConfig::compute_fee_with_dynamic_base_and_outputs` (a
/// unit test pins the two together for every wealth → factor mapping).
fn base_fee_for_factor(
    config: &FeeConfig,
    size: usize,
    factor: u64,
    n_out: usize,
    n_memo: usize,
) -> u64 {
    let size_fee = CONSENSUS_FEE_BASE
        .saturating_mul(size as u64)
        .saturating_mul(factor)
        .saturating_mul(config.output_penalty(n_out))
        / (ClusterFactorCurve::FACTOR_SCALE * OUTPUT_FEE_EXPONENT_SCALE as u64);
    let memo_base = std::cmp::max(config.fee_per_memo, CONSENSUS_FEE_BASE * 100);
    size_fee.saturating_add(memo_base.saturating_mul(n_memo as u64))
}

/// The consensus fee model, decomposed. See the module docs.
pub fn consensus_fee_breakdown(
    inputs: &ConsensusFeeInputs,
    params: &ConsensusFeeParams,
) -> ConsensusFeeBreakdown {
    let config = FeeConfig::default();

    let (base, claimed_factor) = match inputs.claimed {
        // Exactly the ledger's call, so the wealth path cannot drift from it.
        ClaimedClass::Wealth(wealth) => (
            config.minimum_fee_dynamic_with_outputs(
                TransactionType::Hidden,
                inputs.size,
                wealth,
                inputs.n_out,
                inputs.n_memo,
                CONSENSUS_FEE_BASE,
            ),
            config.cluster_factor(wealth),
        ),
        ClaimedClass::Factor(factor) => (
            base_fee_for_factor(&config, inputs.size, factor, inputs.n_out, inputs.n_memo),
            factor,
        ),
    };

    let demurrage_factor = claimed_factor.max(inputs.floor);
    let demurrage = spend_demurrage_charge(
        inputs.output_sum,
        demurrage_factor,
        claimed_factor,
        inputs.elapsed,
        params.demurrage_rate_bps(inputs.height),
        params.blocks_per_year,
    );

    let consensus_floor = base.saturating_add(demurrage);
    let fee = match params.mode {
        FeeModelMode::ConsensusOnly => consensus_floor,
        FeeModelMode::ConsensusPlusRelayMin => consensus_floor.max(RELAY_MIN_TX_FEE),
    };

    ConsensusFeeBreakdown {
        base,
        demurrage,
        consensus_floor,
        fee,
        claimed_factor,
        demurrage_factor,
    }
}

/// `cluster_tax::consensus_fee_model` (#1695): the fee in picocredits that the
/// node's consensus floor (and optionally its relay minimum) requires for a
/// transaction with these ledger-resolved inputs. Pure integer math.
pub fn consensus_fee_model(inputs: &ConsensusFeeInputs, params: &ConsensusFeeParams) -> u64 {
    consensus_fee_breakdown(inputs, params).fee
}

/// Simulator adapter: prices transfers in a sim's own value unit with the
/// shared model.
///
/// Sims keep integer balances in a coarse unit (BTH, milliBTH, ...), while
/// consensus fees are often a few thousand picocredits — far below one sim
/// unit. Rounding every fee down to zero units would silently drop them;
/// rounding up would overstate them by orders of magnitude. The adapter
/// therefore carries the sub-unit remainder across calls ([`Self::charge`]),
/// so the total charged equals the total consensus fee to within one unit.
#[derive(Clone, Debug)]
pub struct SimConsensusFee {
    /// Model parameters (mode + demurrage schedule).
    pub params: ConsensusFeeParams,
    /// Picocredits per sim value unit (e.g. `PICO_PER_BTH` for BTH units).
    pub pico_per_unit: u128,
    /// Height of sim block 0. Sims default to the first epoch-1 block so the
    /// node's demurrage is active, as in the legacy sims.
    pub height_offset: u64,
    /// Sub-unit remainder carried between charges, in picocredits.
    carry_pico: u128,
    /// Total picocredits priced by the model (exact, before unit rounding).
    pub total_fee_pico: u128,
}

impl SimConsensusFee {
    /// A new adapter with zero carry.
    pub fn new(params: ConsensusFeeParams, pico_per_unit: u128) -> Self {
        Self {
            params,
            pico_per_unit: pico_per_unit.max(1),
            height_offset: params.first_demurrage_height(),
            carry_pico: 0,
            total_fee_pico: 0,
        }
    }

    /// Set the height of sim block 0.
    pub fn with_height_offset(mut self, height_offset: u64) -> Self {
        self.height_offset = height_offset;
        self
    }

    /// Convert a sim amount to picocredits (saturating at `u64::MAX`, the
    /// ledger's amount width).
    pub fn units_to_pico(&self, units: u64) -> u64 {
        (units as u128)
            .saturating_mul(self.pico_per_unit)
            .min(u64::MAX as u128) as u64
    }

    /// Convert sim-unit wealth to picocredits.
    pub fn wealth_to_pico(&self, units: u128) -> u128 {
        units.saturating_mul(self.pico_per_unit)
    }

    /// The model fee in picocredits for these inputs (no unit conversion).
    pub fn fee_pico(&self, inputs: &ConsensusFeeInputs) -> u64 {
        consensus_fee_model(inputs, &self.params)
    }

    /// Charge a fee of `fee_pico` picocredits in sim units, carrying the
    /// sub-unit remainder to the next charge.
    pub fn charge(&mut self, fee_pico: u64) -> u64 {
        self.total_fee_pico = self.total_fee_pico.saturating_add(fee_pico as u128);
        let owed = self.carry_pico.saturating_add(fee_pico as u128);
        let units = owed / self.pico_per_unit;
        self.carry_pico = owed % self.pico_per_unit;
        units.min(u64::MAX as u128) as u64
    }

    /// Model fee for a simple transfer, charged in sim units. `claimed` and
    /// `floor` are the output class and input-class floor; `elapsed` and
    /// `sim_block` are in sim blocks; `value_units` is the value moved.
    #[allow(clippy::too_many_arguments)]
    pub fn charge_transfer(
        &mut self,
        n_inputs: usize,
        n_out: usize,
        claimed: ClaimedClass,
        floor: u64,
        elapsed: u64,
        sim_block: u64,
        value_units: u64,
    ) -> u64 {
        let inputs = ConsensusFeeInputs {
            size: consensus_tx_size_bytes(n_inputs, n_out, 0),
            n_out,
            n_memo: 0,
            claimed,
            floor,
            elapsed,
            height: self.height_offset.saturating_add(sim_block),
            output_sum: self.units_to_pico(value_units),
        };
        let fee = self.fee_pico(&inputs);
        self.charge(fee)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{demurrage::FACTOR_SCALE, fee_curve::PICO_PER_BTH};

    fn inputs(claimed: ClaimedClass, floor: u64, elapsed: u64, height: u64) -> ConsensusFeeInputs {
        ConsensusFeeInputs {
            size: consensus_tx_size_bytes(1, 2, 0),
            n_out: 2,
            n_memo: 0,
            claimed,
            floor,
            elapsed,
            height,
            output_sum: 1_000 * PICO_PER_BTH as u64,
        }
    }

    #[test]
    fn size_estimate_matches_node_constants() {
        // 50 header + 700/input + 120/output + 66/memo.
        assert_eq!(consensus_tx_size_bytes(1, 2, 0), 990);
        assert_eq!(consensus_tx_size_bytes(2, 3, 1), 50 + 1400 + 360 + 66);
    }

    #[test]
    fn background_transfer_is_size_times_quadratic_outputs() {
        let p = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let b = consensus_fee_breakdown(&inputs(ClaimedClass::Wealth(0), FACTOR_SCALE, 0, 1), &p);
        // 1 pico/byte × 990 B × 1× × 2² = 3_960 pico; no demurrage.
        assert_eq!(b.base, 3_960);
        assert_eq!(b.demurrage, 0);
        assert_eq!(b.fee, 3_960);
    }

    #[test]
    fn relay_mode_applies_min_tx_fee() {
        let only = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let relay = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusPlusRelayMin);
        let i = inputs(ClaimedClass::Wealth(0), FACTOR_SCALE, 0, 1);
        assert_eq!(consensus_fee_model(&i, &only), 3_960);
        assert_eq!(consensus_fee_model(&i, &relay), RELAY_MIN_TX_FEE);
    }

    #[test]
    fn factor_path_matches_wealth_path() {
        let p = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let curve = ClusterFactorCurve::default();
        let h = p.first_demurrage_height();
        for wealth_bth in [0u128, 1, 1_000, 100_000, 1_000_000, 10_000_000, 1 << 40] {
            let w = wealth_bth * PICO_PER_BTH;
            let f = curve.factor(w);
            for (n_out, n_memo) in [(1usize, 0usize), (2, 1), (12, 3)] {
                for elapsed in [0u64, 1_000_000] {
                    let mut a = inputs(ClaimedClass::Wealth(w), 6_000, elapsed, h);
                    a.n_out = n_out;
                    a.n_memo = n_memo;
                    let mut b = a;
                    b.claimed = ClaimedClass::Factor(f);
                    assert_eq!(
                        consensus_fee_breakdown(&a, &p),
                        consensus_fee_breakdown(&b, &p),
                        "wealth {wealth_bth} BTH n_out {n_out} n_memo {n_memo}"
                    );
                }
            }
        }
    }

    #[test]
    fn demurrage_is_zero_in_epoch_zero_and_active_after() {
        let p = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let old = 2 * MAINNET_BLOCKS_PER_YEAR;
        let wealthy = ClaimedClass::Factor(6_000);
        let pre = consensus_fee_breakdown(&inputs(wealthy, 6_000, old, 1), &p);
        let post =
            consensus_fee_breakdown(&inputs(wealthy, 6_000, old, MAINNET_HALVING_INTERVAL), &p);
        assert_eq!(pre.demurrage, 0);
        assert!(post.demurrage > 0);
    }

    #[test]
    fn downgrade_hits_capitalized_charge_even_when_young() {
        let p = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let h = MAINNET_HALVING_INTERVAL;
        // Young (elapsed 0) wealthy input spent to a background claim.
        let down = consensus_fee_breakdown(&inputs(ClaimedClass::Factor(1_000), 5_745, 0, h), &p);
        let in_class =
            consensus_fee_breakdown(&inputs(ClaimedClass::Factor(5_745), 5_745, 0, h), &p);
        assert!(down.demurrage > 0, "young downgrade must be priced");
        assert_eq!(
            in_class.demurrage, 0,
            "young in-class spend accrues nothing"
        );
    }

    #[test]
    fn output_penalty_caps_at_ten_outputs() {
        let p = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
        let mut a = inputs(ClaimedClass::Factor(1_000), 1_000, 0, 1);
        a.n_out = 12;
        a.size = 1_000;
        // 1_000 B × 1× × min(12,10)² = 100_000.
        assert_eq!(consensus_fee_model(&a, &p), 100_000);
    }

    #[test]
    fn sim_adapter_carries_sub_unit_remainder() {
        let mut s = SimConsensusFee::new(
            ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly),
            10_000,
        );
        // 3 × 3_960 pico = 11_880 → one unit charged on the third call.
        assert_eq!(s.charge(3_960), 0);
        assert_eq!(s.charge(3_960), 0);
        assert_eq!(s.charge(3_960), 1);
        assert_eq!(s.total_fee_pico, 11_880);
    }

    #[test]
    fn fee_split_is_eighty_twenty_floored() {
        assert_eq!(split_fee_pool_burn(1_000), (800, 200));
        assert_eq!(split_fee_pool_burn(3_961), (3_168, 793));
        assert_eq!(split_fee_pool_burn(0), (0, 0));
        assert_eq!(split_fee_pool_burn(1), (0, 1));
    }

    #[test]
    fn from_policy_derives_blocks_per_year() {
        let policy = MonetaryPolicy {
            target_block_time_secs: 5,
            halving_interval: MAINNET_HALVING_INTERVAL,
            ..MonetaryPolicy::default()
        };
        assert_eq!(
            ConsensusFeeParams::from_policy(&policy, FeeModelMode::ConsensusOnly),
            ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly)
        );
    }
}
