//! Faithful-to-code re-run of the Path C reward-cap sweep (issue #1682).
//!
//! # Why this exists
//!
//! [`super::lottery_reward_cap_sweep`] (research memo §9.1) proved that coin
//! splitting is net-zero under `R = min(fee_pool, ρ·base_fee)`. That proof
//! rests on two modelling premises:
//!
//! 1. `ρ` is a **per-block** output creation rate, and
//! 2. every split output pays `base_fee` = [`LOTTERY_BASE_FEE_PICO`] (0.25
//!    BTH).
//!
//! The implementation (protocol 7.0.0, `ledger::lottery_v2`) honours neither:
//!
//! * `consensus::lottery::reward_cap` uses `ρ` = `count_eligible` over the
//!   seeded-offset candidate set, i.e. every output aged
//!   `min_utxo_age..=circulation_window` (720..=10,000 blocks), truncated at
//!   `MAX_LOTTERY_CANDIDATES` = 10,000. At any non-trivial activity this count
//!   is the full 10,000, so the "Sybil cap" is `10,000 × 0.25 = 2,500` BTH, and
//!   the binding clamp is the anti-grinding `block_reward`.
//! * No fee rule charges `base_fee`. The consensus floor is the size-based
//!   `CONSENSUS_FEE_BASE` (1 pico/byte) × size × cluster factor × `min(n_out,
//!   10)²`, plus demurrage. `MIN_TX_FEE` (1e8 pico) is a relay/mempool rule
//!   only (#1684).
//!
//! This module re-runs the whale-splitting experiment with the implemented
//! rules and reports the whale's optimal net per block, the break-even
//! per-ticket cost, and the same quantities under candidate remedies. It is
//! ANALYSIS ONLY: nothing here is consensus code, and no remedy is
//! implemented.
//!
//! # What is real and what is mirrored
//!
//! Called directly (the shipped kernels):
//! [`crate::consensus_fee::consensus_fee_breakdown`] (the pure half of
//! `Ledger::consensus_fee_floor`, golden-pinned to the ledger by #1695),
//! [`sybil_reward_cap`], [`LotteryDrawConfig::default`] (which is what
//! `ledger::lottery_v2::canonical_config` uses),
//! [`MonetaryPolicy::lottery_emission_share`] and
//! [`MonetaryPolicy::calculate_tail_reward_u128`].
//!
//! Mirrored, because `cluster-tax` cannot depend on `botho`:
//! `consensus::lottery::reward_cap` and `compute_pool_accounting` (both a few
//! lines of integer math — see [`code_reward_cap`] and [`code_pool_payout`]),
//! `MAX_LOTTERY_CANDIDATES`, the 80% pool share, and the mainnet monetary
//! schedule. The shared [`PARITY_VECTORS`] table is asserted against the
//! mirror here and against the real node functions in
//! `botho/src/ledger/lottery_v2.rs` (`faithful_sweep_parity_*`), so drift on
//! either side fails CI.
//!
//! # Model
//!
//! Stationary expected values (the per-block draw averages to the ticket share
//! over a long run). Every eligible output lives `L = circulation_window −
//! min_utxo_age + 1` = 9,281 blocks in the eligible set, for whale and organic
//! outputs alike, so window shares equal creation-rate shares. Per block:
//!
//! * organic economy: `ρ_o` eligible outputs in 2-output txs, each tx paying
//!   the wallet fee `max(floor, MIN_TX_FEE)` (the x1 run observed 1e8–4e8
//!   pico/tx);
//! * free tickets (#1690): 1 coinbase output + 4 lottery payout outputs; the
//!   whale wins its share of the payout tickets, so its stationary share is `s
//!   = k/(ρ_o + k + 1)` (one coinbase ticket per block it never owns);
//! * whale: `k` fresh eligible outputs per block, each worth `min_utxo_value`,
//!   minted by spending an aged-out ticket into an `n`-output tx (`n` in
//!   1..=16, chosen to minimise cost) at its cluster factor;
//! * pool: `avail = backlog + emission_share + 0.8·fees`, `R = min(avail,
//!   cap)`, and the whale nets `s·R − whale_fees` (80% of its own fees return
//!   through `R` when the pool is not capped).
//!
//! The whale picks the `k` that maximises its net, subject to a block
//! capacity: the default proposer slot (`ConsensusConfig::max_txs_per_slot` =
//! 100, minus the minting tx, × 16 outputs = 1,584 outputs/block), or the
//! gossip block-size bound (`MAX_BLOCK_SIZE` = 20 MiB). Neither the slot
//! limit nor the 16-output cap is enforced at block validation (#1684), so the
//! block-size bound is the honest worst case.
//!
//! Conservative choices (they understate the whale): organic demurrage is not
//! added to the pool, and the tail reward is taken from the undiminished
//! emission schedule.

use crate::{
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, ClaimedClass, ConsensusFeeInputs,
        ConsensusFeeParams, FeeModelMode, LOTTERY_POOL_FRACTION_PERMILLE, MAINNET_BLOCKS_PER_YEAR,
        MAINNET_HALVING_INTERVAL, RELAY_MIN_TX_FEE,
    },
    fee_curve::PICO_PER_BTH,
    sybil_reward_cap, LotteryDrawConfig, MonetaryPolicy, LOTTERY_BASE_FEE_PICO,
};

// ===========================================================================
// Mirrored node constants (parity-tested in botho/src/ledger/lottery_v2.rs)
// ===========================================================================

/// `MAX_LOTTERY_CANDIDATES` in `botho/src/ledger/store/validation.rs`
/// (`lottery_candidates`): the candidate walk stops after this many eligible
/// UTXOs, so `count_eligible` — the reward-cap `ρ` — never exceeds it.
pub const MAX_LOTTERY_CANDIDATES: usize = 10_000;

/// `canonical_config().pool_fraction_permille` (80% of fees to the pool),
/// imported from the shared [`LOTTERY_POOL_FRACTION_PERMILLE`] (#1702).
pub const POOL_FRACTION_PERMILLE: u64 = LOTTERY_POOL_FRACTION_PERMILLE as u64;

/// `bth_transaction_types::MAX_OUTPUTS` (mempool-only; #1684), imported
/// (#1702).
pub const MAX_OUTPUTS_PER_TX: usize = bth_transaction_types::MAX_OUTPUTS as usize;

/// `ConsensusConfig::default().max_txs_per_slot` — node-local proposer policy,
/// not a validity rule. One slot goes to the minting tx.
pub const PROPOSER_MAX_TXS_PER_SLOT: usize = 100;

/// `bth_transaction_types::MAX_BLOCK_SIZE` (20 MiB gossip transmit bound),
/// imported (#1702).
pub const MAX_BLOCK_SIZE_BYTES: usize = bth_transaction_types::MAX_BLOCK_SIZE;

/// Coinbase outputs per block (the minting tx has one stealth output).
pub const COINBASE_TICKETS_PER_BLOCK: f64 = 1.0;

/// Blocks per day at the 5 s monetary block time, derived from the shared
/// [`MAINNET_BLOCKS_PER_YEAR`] (#1702).
pub const BLOCKS_PER_DAY: f64 = (MAINNET_BLOCKS_PER_YEAR / 365) as f64;

/// Number of halving epochs before tail emission (mainnet).
pub const MAINNET_HALVING_COUNT: u32 = 5;

/// The mainnet monetary policy (`botho::monetary::mainnet_policy`), rebuilt
/// from the shared mainnet constants. Parity: the botho test asserts the
/// per-epoch rewards in [`PARITY_VECTORS`]-adjacent goldens.
pub fn mainnet_policy() -> MonetaryPolicy {
    MonetaryPolicy {
        initial_reward: 50_000_000_000_000,
        halving_interval: MAINNET_HALVING_INTERVAL,
        halving_count: MAINNET_HALVING_COUNT,
        tail_inflation_bps: 200,
        target_block_time_secs: 5,
        min_block_time_secs: 3,
        max_block_time_secs: 60,
        difficulty_adjustment_interval: MAINNET_BLOCKS_PER_YEAR / 365,
        max_difficulty_adjustment_bps: 2500,
        expected_fee_burn_rate_bps: 50,
    }
}

/// Mirror of `consensus::lottery::reward_cap` given the eligible count of the
/// candidate set: `sybil_reward_cap(min(count, MAX_LOTTERY_CANDIDATES))`
/// clamped by `block_reward`. (The node counts the already-truncated candidate
/// vector, so the truncation is implicit there.)
pub fn code_reward_cap(eligible_in_window: u64, block_reward: u64) -> u64 {
    let rho = (eligible_in_window as usize).min(MAX_LOTTERY_CANDIDATES);
    sybil_reward_cap(rho).min(block_reward as u128) as u64
}

/// Mirror of `consensus::lottery::compute_pool_accounting`: returns
/// `(fee_pool, payout)` with `payout = min(stored + emission + fee_pool,
/// cap)`.
pub fn code_pool_payout(
    total_fees: u64,
    emission_share: u64,
    stored_pool: u128,
    payout_cap: u64,
) -> (u64, u64) {
    let fee_pool = (total_fees as u128 * POOL_FRACTION_PERMILLE as u128 / 1000) as u64;
    let available = stored_pool
        .saturating_add(emission_share as u128)
        .saturating_add(fee_pool as u128);
    (fee_pool, available.min(payout_cap as u128) as u64)
}

/// One parity vector: `(eligible_count, block_reward, total_fees,
/// emission_share, stored_pool) → (cap, fee_pool, payout)`. The same table is
/// asserted against the real node functions in botho.
pub type ParityVector = (u64, u64, u64, u64, u128, u64, u64, u64);

/// Golden vectors pinning [`code_reward_cap`] and [`code_pool_payout`] to the
/// node. Columns: count, reward, fees, emission, stored → cap, fee_pool,
/// payout.
pub const PARITY_VECTORS: [ParityVector; 5] = [
    // Tiny window: Sybil cap (3 × 0.25 BTH) binds below the reward.
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
    // Full window, truncated at 10k: cap = block reward (25 BTH).
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
    // 1000-BTH backlog drains at the full reward.
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
    // Epoch-0 quiet chain: fee pool only.
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
    // Exactly at the truncation boundary.
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

// ===========================================================================
// Scenario axes
// ===========================================================================

/// Emission epoch. `0..=4` are halving epochs; `5` is the tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch(pub u32);

impl Epoch {
    /// The swept epochs: 0, 1, 3, tail.
    pub fn swept() -> [Epoch; 4] {
        [Epoch(0), Epoch(1), Epoch(3), Epoch(MAINNET_HALVING_COUNT)]
    }

    /// A height in the middle of the epoch.
    pub fn height(self) -> u64 {
        self.0 as u64 * MAINNET_HALVING_INTERVAL + MAINNET_HALVING_INTERVAL / 2
    }

    /// Block reward in pico. Tail = `calculate_tail_reward_u128` of the
    /// undiminished schedule supply (burns ignored; conservative).
    pub fn block_reward(self) -> u64 {
        let p = mainnet_policy();
        match p.halving_reward(self.height()) {
            Some(r) => r,
            None => {
                let supply: u128 = (0..p.halving_count)
                    .map(|i| (p.initial_reward >> i) as u128 * p.halving_interval as u128)
                    .sum();
                p.calculate_tail_reward_u128(supply)
            }
        }
    }

    /// Lottery share of the block reward (0% in epoch 0, +10%/epoch, ≤ 50%).
    pub fn emission_share(self) -> u64 {
        mainnet_policy().lottery_emission_share(self.height(), self.block_reward())
    }

    /// Short label.
    pub fn label(self) -> String {
        if self.0 >= MAINNET_HALVING_COUNT {
            "tail".to_string()
        } else {
            format!("{}", self.0)
        }
    }
}

/// Block capacity available to the whale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capacity {
    /// Default proposer slot: 99 transfer txs × up to 16 outputs.
    ProposerSlot,
    /// Gossip block-size bound (20 MiB) — the consensus-level worst case.
    BlockSize,
}

impl Capacity {
    /// Max txs per block of `n` outputs the whale can place.
    pub fn max_txs(self, n: usize) -> f64 {
        match self {
            Capacity::ProposerSlot => (PROPOSER_MAX_TXS_PER_SLOT - 1) as f64,
            Capacity::BlockSize => (MAX_BLOCK_SIZE_BYTES / consensus_tx_size_bytes(1, n, 0)) as f64,
        }
    }

    /// Max whale outputs/block (at the best n = 16).
    pub fn max_outputs(self) -> f64 {
        self.max_txs(MAX_OUTPUTS_PER_TX) * MAX_OUTPUTS_PER_TX as f64
    }

    /// Label.
    pub fn label(self) -> &'static str {
        match self {
            Capacity::ProposerSlot => "proposer slot (1,584 out/blk)",
            Capacity::BlockSize => "block size (20 MiB)",
        }
    }
}

/// The rule set under test: the implemented code, or a candidate remedy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rules {
    /// As implemented: window `ρ` truncated at 10k, `R ≤ block_reward`,
    /// size-based fee floor, free tickets eligible.
    CurrentCode,
    /// (a) Per-block cap: `ρ` = eligible outputs created per block (whale,
    /// organic and free tickets), fee floor unchanged.
    PerBlockCap,
    /// (a') Window count / window length with the existing 10k truncation:
    /// `ρ = min(count, 10k) / L` — the truncation pins `ρ` ≈ 1.08.
    WindowOverLength,
    /// (b) Every lottery-eligible new output pays `LOTTERY_BASE_FEE_PICO` on
    /// top of the fee floor; cap unchanged (window `ρ`).
    BaseFeePerOutput,
    /// (a)+(b): per-block cap and base fee per eligible output; free tickets
    /// still eligible and still counted.
    PerBlockAndBaseFee,
    /// (a)+(b)+free-ticket rule: coinbase and payout outputs are neither
    /// eligible nor counted in `ρ`.
    PerBlockBaseFeeNoFree,
    /// Raise `min_utxo_value` to 1 BTH (cost = demurrage on the locked ticket
    /// value via the real fee floor; capital itself is not a cost).
    MinValue1Bth,
}

impl Rules {
    /// All rule sets, in decision-table order.
    pub fn all() -> [Rules; 7] {
        [
            Rules::CurrentCode,
            Rules::PerBlockCap,
            Rules::WindowOverLength,
            Rules::BaseFeePerOutput,
            Rules::PerBlockAndBaseFee,
            Rules::PerBlockBaseFeeNoFree,
            Rules::MinValue1Bth,
        ]
    }

    fn base_fee_per_output(self) -> bool {
        matches!(
            self,
            Rules::BaseFeePerOutput | Rules::PerBlockAndBaseFee | Rules::PerBlockBaseFeeNoFree
        )
    }

    fn free_tickets(self) -> bool {
        !matches!(self, Rules::PerBlockBaseFeeNoFree)
    }

    fn ticket_value_pico(self) -> u64 {
        match self {
            Rules::MinValue1Bth => PICO_PER_BTH as u64,
            _ => LotteryDrawConfig::default().min_utxo_value,
        }
    }

    /// Label.
    pub fn label(self) -> &'static str {
        match self {
            Rules::CurrentCode => "current code",
            Rules::PerBlockCap => "(a) per-block ρ in cap",
            Rules::WindowOverLength => "(a') ρ = window count / L (10k truncation kept)",
            Rules::BaseFeePerOutput => "(b) base_fee per eligible output",
            Rules::PerBlockAndBaseFee => "(a)+(b), free tickets kept",
            Rules::PerBlockBaseFeeNoFree => "(a)+(b)+free-ticket rule",
            Rules::MinValue1Bth => "min_utxo_value = 1 BTH",
        }
    }
}

/// One grid point.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub epoch: Epoch,
    /// Carry-forward backlog in the pool, BTH.
    pub backlog_bth: f64,
    /// Organic eligible outputs per block.
    pub rho_o: f64,
    /// Whale fee mode (consensus floor only, or + relay `MIN_TX_FEE`).
    pub mode: FeeModelMode,
    /// Whale cluster factor, `FACTOR_SCALE` (1000 = 1×).
    pub factor: u64,
    pub capacity: Capacity,
}

/// Eligible lifetime `L = circulation_window − min_utxo_age + 1` (9,281).
pub fn ticket_life_blocks() -> f64 {
    let c = LotteryDrawConfig::default();
    (c.circulation_window - c.min_utxo_age + 1) as f64
}

fn base_fee() -> f64 {
    LOTTERY_BASE_FEE_PICO as f64
}

/// Fee in pico for one whale split tx: 1 input (an aged-out ticket, so the
/// demurrage clock is the full window), `n` outputs of `ticket_value` each.
pub fn whale_tx_fee(n: usize, s: &Scenario, ticket_value: u64) -> u64 {
    let window = LotteryDrawConfig::default().circulation_window;
    let inputs = ConsensusFeeInputs {
        size: consensus_tx_size_bytes(1, n, 0),
        n_out: n,
        n_memo: 0,
        claimed: ClaimedClass::Factor(s.factor),
        floor: s.factor,
        elapsed: window,
        height: s.epoch.height(),
        output_sum: ticket_value.saturating_mul(n as u64),
    };
    consensus_fee_breakdown(&inputs, &ConsensusFeeParams::mainnet(s.mode)).fee
}

/// Organic tx fee (a 2-output wallet tx at 1×, relay minimum applied — what
/// the x1 run observed).
pub fn organic_tx_fee(epoch: Epoch) -> u64 {
    let inputs = ConsensusFeeInputs {
        size: consensus_tx_size_bytes(1, 2, 0),
        n_out: 2,
        n_memo: 0,
        claimed: ClaimedClass::Factor(1000),
        floor: 1000,
        elapsed: 0,
        height: epoch.height(),
        output_sum: PICO_PER_BTH as u64,
    };
    consensus_fee_breakdown(
        &inputs,
        &ConsensusFeeParams::mainnet(FeeModelMode::ConsensusPlusRelayMin),
    )
    .fee
}

/// Cheapest feasible per-output whale cost (pico) for `k` outputs/block, and
/// the outputs-per-tx that achieves it. `None` if `k` exceeds capacity.
pub fn whale_cost_per_output(k: f64, s: &Scenario, rules: Rules) -> Option<(f64, usize)> {
    let tv = rules.ticket_value_pico();
    let extra = if rules.base_fee_per_output() {
        base_fee()
    } else {
        0.0
    };
    (1..=MAX_OUTPUTS_PER_TX)
        .filter(|&n| k / n as f64 <= s.capacity.max_txs(n) + 1e-9)
        .map(|n| (whale_tx_fee(n, s, tv) as f64 / n as f64 + extra, n))
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
}

/// The outcome for the whale at one `k`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    pub k: f64,
    pub n_per_tx: usize,
    pub share: f64,
    /// Payout cap (pico).
    pub cap: f64,
    /// Payout per block `R` (pico).
    pub payout: f64,
    pub winnings: f64,
    pub cost: f64,
    pub net: f64,
    /// Per-output whale cost (pico).
    pub cost_per_output: f64,
    /// Pool inflow this block, `emission_share + 0.8·fees` (pico).
    pub inflow: f64,
}

/// Evaluate the whale at `k` outputs/block. `cost_override` replaces the
/// per-output cost (for the break-even search).
pub fn evaluate(k: f64, s: &Scenario, rules: Rules, cost_override: Option<f64>) -> Option<Outcome> {
    let (c, n) = match cost_override {
        Some(c) => (c, MAX_OUTPUTS_PER_TX),
        None => whale_cost_per_output(k, s, rules)?,
    };
    let free_cb = if rules.free_tickets() {
        COINBASE_TICKETS_PER_BLOCK
    } else {
        0.0
    };
    let free_po = if rules.free_tickets() {
        LotteryDrawConfig::default().winners_per_draw as f64
    } else {
        0.0
    };
    // Whale owns k + s·free_po of T tickets/block ⇒ s = k / (T − free_po).
    let created = s.rho_o + k + free_cb + free_po;
    let share = if k > 0.0 {
        k / (s.rho_o + k + free_cb)
    } else {
        0.0
    };
    let life = ticket_life_blocks();
    let eligible = created * life;
    let reward = s.epoch.block_reward();
    let cap = match rules {
        Rules::CurrentCode | Rules::BaseFeePerOutput | Rules::MinValue1Bth => {
            code_reward_cap(eligible.round() as u64, reward) as f64
        }
        Rules::PerBlockCap | Rules::PerBlockAndBaseFee | Rules::PerBlockBaseFeeNoFree => {
            (created * base_fee()).min(reward as f64)
        }
        Rules::WindowOverLength => {
            (eligible.min(MAX_LOTTERY_CANDIDATES as f64) / life * base_fee()).min(reward as f64)
        }
    };
    let mut organic_fees = s.rho_o / 2.0 * organic_tx_fee(s.epoch) as f64;
    if rules.base_fee_per_output() {
        organic_fees += s.rho_o * base_fee();
    }
    let cost = k * c;
    let total_fees = (organic_fees + cost).min(u64::MAX as f64) as u64;
    let stored = (s.backlog_bth * PICO_PER_BTH as f64) as u128;
    let cap_u = cap.min(u64::MAX as f64) as u64;
    let (fee_pool, payout) = code_pool_payout(total_fees, s.epoch.emission_share(), stored, cap_u);
    let inflow = (s.epoch.emission_share() + fee_pool) as f64;
    let payout = payout as f64;
    let winnings = share * payout;
    Some(Outcome {
        k,
        n_per_tx: n,
        share,
        cap,
        payout,
        winnings,
        cost,
        net: winnings - cost,
        cost_per_output: c,
        inflow,
    })
}

/// The `k` grid searched: geometric from 1 to the capacity bound, plus the
/// bound itself.
pub fn k_grid(capacity: Capacity) -> Vec<f64> {
    let kmax = capacity.max_outputs();
    let mut ks = Vec::new();
    let mut k = 1.0f64;
    while k < kmax {
        ks.push(k.round());
        k *= 1.06;
    }
    ks.push(kmax);
    ks.dedup();
    ks
}

/// The whale's optimum over `k` (net ≤ 0 everywhere ⇒ abstain, k = 0).
pub fn optimum(s: &Scenario, rules: Rules, cost_override: Option<f64>) -> Outcome {
    let mut best = Outcome::default();
    for k in k_grid(s.capacity) {
        if let Some(o) = evaluate(k, s, rules, cost_override) {
            if o.net > best.net {
                best = o;
            }
        }
    }
    best
}

/// Payout per block with no whale (pico) — `R0`.
pub fn baseline_payout(s: &Scenario, rules: Rules) -> f64 {
    evaluate(0.0, s, rules, Some(0.0))
        .map(|o| o.payout)
        .unwrap_or(0.0)
}

/// Break-even per-ticket cost (pico): the smallest per-output cost at which
/// the whale's best net is ≤ 0. Bisection in log space.
pub fn break_even_cost(s: &Scenario, rules: Rules) -> f64 {
    let profitable = |c: f64| optimum(s, rules, Some(c)).net > 0.0;
    let (mut lo, mut hi) = (1.0f64, 1.0e16f64);
    if !profitable(lo) {
        return lo;
    }
    for _ in 0..80 {
        let mid = (lo * hi).sqrt();
        if profitable(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

// ===========================================================================
// Grid
// ===========================================================================

/// Swept backlogs (BTH).
pub fn backlogs() -> [f64; 2] {
    [0.0, 1000.0]
}

/// Swept organic eligible-output rates (per block).
pub fn organic_rates() -> [f64; 3] {
    [2.0, 20.0, 200.0]
}

/// Swept whale cluster factors (1× and the 6× curve maximum).
pub fn factors() -> [u64; 2] {
    [1000, 6000]
}

/// Every grid point (epoch × backlog × ρ_o × mode × factor × capacity).
pub fn grid() -> Vec<Scenario> {
    let mut v = Vec::new();
    for epoch in Epoch::swept() {
        for backlog_bth in backlogs() {
            for rho_o in organic_rates() {
                for mode in [
                    FeeModelMode::ConsensusOnly,
                    FeeModelMode::ConsensusPlusRelayMin,
                ] {
                    for factor in factors() {
                        for capacity in [Capacity::ProposerSlot, Capacity::BlockSize] {
                            v.push(Scenario {
                                epoch,
                                backlog_bth,
                                rho_o,
                                mode,
                                factor,
                                capacity,
                            });
                        }
                    }
                }
            }
        }
    }
    v
}

/// Per-remedy summary over the whole grid.
#[derive(Clone, Debug)]
pub struct RemedyRow {
    pub rules: Rules,
    pub points: usize,
    pub profitable_points: usize,
    pub max_net: f64,
    pub worst: Option<(Scenario, Outcome)>,
    /// Max net at the reference point (epoch 1, no backlog, ρ_o = 20,
    /// relay, 6×, proposer slot).
    pub reference_net: f64,
    /// Honest regimes (epoch × ρ_o, no whale, no backlog) where the pool
    /// inflow exceeds the cap, so the carry-forward backlog grows without
    /// bound and redistribution is throttled to the cap (§9.2 stability).
    pub unstable_regimes: usize,
    /// Number of honest regimes checked.
    pub regimes: usize,
    /// Honest payout/blk with no whale at the reference point (pico).
    pub reference_honest_payout: f64,
}

/// The reference scenario used for the net-vs-k table.
pub fn reference_scenario() -> Scenario {
    Scenario {
        epoch: Epoch(1),
        backlog_bth: 0.0,
        rho_o: 20.0,
        mode: FeeModelMode::ConsensusPlusRelayMin,
        factor: 6000,
        capacity: Capacity::ProposerSlot,
    }
}

/// Summarise every remedy over the grid.
pub fn remedy_table() -> Vec<RemedyRow> {
    let g = grid();
    Rules::all()
        .into_iter()
        .map(|rules| {
            let mut row = RemedyRow {
                rules,
                points: g.len(),
                profitable_points: 0,
                max_net: 0.0,
                worst: None,
                reference_net: optimum(&reference_scenario(), rules, None).net,
                unstable_regimes: 0,
                regimes: 0,
                reference_honest_payout: baseline_payout(&reference_scenario(), rules),
            };
            for epoch in Epoch::swept() {
                for rho_o in organic_rates() {
                    let h = Scenario {
                        epoch,
                        rho_o,
                        backlog_bth: 0.0,
                        ..reference_scenario()
                    };
                    let o = evaluate(0.0, &h, rules, Some(0.0)).expect("k = 0 is feasible");
                    row.regimes += 1;
                    if o.inflow > o.cap {
                        row.unstable_regimes += 1;
                    }
                }
            }
            for s in &g {
                let o = optimum(s, rules, None);
                if o.net > 0.0 {
                    row.profitable_points += 1;
                }
                if o.net > row.max_net {
                    row.max_net = o.net;
                    row.worst = Some((*s, o));
                }
            }
            row
        })
        .collect()
}

// ===========================================================================
// Candidate-window Monte Carlo (MAX_LOTTERY_CANDIDATES seeded-offset walk)
// ===========================================================================

/// Deterministic seed for the windowing Monte Carlo.
pub const SEED: u64 = 0x1682_FA17_0902;

struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Result of the seeded-offset windowing Monte Carlo.
#[derive(Clone, Debug)]
pub struct WindowRow {
    pub rho_o: f64,
    pub k: f64,
    pub n_per_tx: usize,
    pub eligible_total: usize,
    /// `count_eligible` of the truncated candidate set (always ≤ 10k).
    pub window_count: usize,
    /// Global whale share of the eligible set.
    pub global_share: f64,
    /// Mean whale share inside the 10k window over the trials.
    pub mean_window_share: f64,
    /// Std-dev of the window share across trials.
    pub sd_window_share: f64,
    /// Binomial std-dev for an unclumped 10k sample (for comparison).
    pub sd_binomial: f64,
}

/// Simulate the node's candidate walk: UTXO ids are `tx_hash ‖ index`, so a
/// tx's outputs are contiguous in key order. Start at a uniform offset, take
/// the first 10k eligible keys (wrapping), and measure the whale's share.
pub fn window_mc(rho_o: f64, k: f64, n_whale: usize, trials: usize) -> WindowRow {
    let life = ticket_life_blocks();
    let mut rng = SplitMix64(SEED ^ (rho_o as u64) << 32 ^ k as u64);
    // (tx_hash, index, is_whale)
    let mut keys: Vec<(u64, u8, bool)> = Vec::new();
    let organic_txs = (rho_o * life / 2.0).round() as usize;
    for _ in 0..organic_txs {
        let h = rng.next();
        keys.push((h, 0, false));
        keys.push((h, 1, false));
    }
    let whale_txs = (k * life / n_whale as f64).round() as usize;
    for _ in 0..whale_txs {
        let h = rng.next();
        for i in 0..n_whale {
            keys.push((h, i as u8, true));
        }
    }
    keys.sort_unstable();
    let total = keys.len();
    let whale_total = keys.iter().filter(|x| x.2).count();
    let take = total.min(MAX_LOTTERY_CANDIDATES);
    let mut shares = Vec::with_capacity(trials);
    for _ in 0..trials {
        let off = rng.next();
        let start = keys.partition_point(|x| x.0 < off);
        let mut w = 0usize;
        for j in 0..take {
            if keys[(start + j) % total].2 {
                w += 1;
            }
        }
        shares.push(w as f64 / take as f64);
    }
    let mean = shares.iter().sum::<f64>() / trials as f64;
    let var = shares.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / trials as f64;
    let g = whale_total as f64 / total as f64;
    WindowRow {
        rho_o,
        k,
        n_per_tx: n_whale,
        eligible_total: total,
        window_count: take,
        global_share: g,
        mean_window_share: mean,
        sd_window_share: var.sqrt(),
        sd_binomial: (g * (1.0 - g) / take as f64).sqrt(),
    }
}

/// The windowing configurations reported.
pub fn window_rows() -> Vec<WindowRow> {
    vec![
        window_mc(20.0, 20.0, 16, 400),
        window_mc(20.0, 200.0, 16, 400),
        window_mc(20.0, 200.0, 1, 400),
        window_mc(200.0, 200.0, 16, 400),
    ]
}

// ===========================================================================
// Report
// ===========================================================================

/// The full faithful report.
#[derive(Clone, Debug)]
pub struct FaithfulReport {
    pub remedies: Vec<RemedyRow>,
    pub windows: Vec<WindowRow>,
}

/// Run the faithful sweep.
pub fn run_faithful_sweep() -> FaithfulReport {
    FaithfulReport {
        remedies: remedy_table(),
        windows: window_rows(),
    }
}

fn bth(p: f64) -> f64 {
    p / PICO_PER_BTH as f64
}

fn mode_label(m: FeeModelMode) -> &'static str {
    match m {
        FeeModelMode::ConsensusOnly => "floor",
        FeeModelMode::ConsensusPlusRelayMin => "floor+MIN_TX_FEE",
    }
}

fn scenario_label(s: &Scenario) -> String {
    format!(
        "epoch {}, backlog {:.0}, ρ_o {:.0}, {}, {}×, {}",
        s.epoch.label(),
        s.backlog_bth,
        s.rho_o,
        mode_label(s.mode),
        s.factor / 1000,
        s.capacity.label()
    )
}

/// Render the report as Markdown (every §9.1 number is generated here).
pub fn to_markdown(report: &FaithfulReport) -> String {
    let mut s = String::new();
    let cfg = LotteryDrawConfig::default();

    s.push_str(&format!(
        "### Setup (faithful to protocol 7.0.0)\n\n\
         Candidate window: ages {}..={} blocks (ticket life L = {:.0}), truncated at \
         MAX_LOTTERY_CANDIDATES = {}; {} winners, uniform draw; pool share {}‰; \
         base_fee = {:.2} BTH; min_utxo_value = {} pico; MIN_TX_FEE = {} pico (relay only). \
         Cap = min(sybil_reward_cap(min(count, 10k)), block_reward). Per-ticket cost from the \
         real consensus fee floor (`consensus_fee_breakdown`), 1 input, n = 1..=16 outputs. \
         Organic outputs come in 2-output txs paying {} pico each. Free tickets: 1 coinbase + \
         {} payouts per block. 1 day = {:.0} blocks.\n\n",
        cfg.min_utxo_age,
        cfg.circulation_window,
        ticket_life_blocks(),
        MAX_LOTTERY_CANDIDATES,
        cfg.winners_per_draw,
        POOL_FRACTION_PERMILLE,
        bth(base_fee()),
        cfg.min_utxo_value,
        RELAY_MIN_TX_FEE,
        organic_tx_fee(Epoch(1)),
        cfg.winners_per_draw,
        BLOCKS_PER_DAY,
    ));

    // --- Epoch parameters.
    s.push_str("| epoch | block reward (BTH) | lottery emission share (BTH/blk) |\n");
    s.push_str("|---|---:|---:|\n");
    for e in Epoch::swept() {
        s.push_str(&format!(
            "| {} | {:.4} | {:.4} |\n",
            e.label(),
            bth(e.block_reward() as f64),
            bth(e.emission_share() as f64)
        ));
    }
    s.push('\n');

    // --- Per-ticket cost.
    s.push_str("### Per-ticket cost under the implemented fee floor\n\n");
    s.push_str(
        "Fee of one split tx (1 input, n outputs of min_utxo_value) and the cost per new \
         eligible output, epoch 1, vs the modelled base_fee of 2.5e11 pico.\n\n",
    );
    s.push_str("| n outputs | factor | floor fee/tx (pico) | floor per ticket | floor+MIN_TX_FEE per ticket | base_fee ÷ cheapest |\n");
    s.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for n in [1usize, 2, 4, 8, 10, 16] {
        for factor in factors() {
            let mut sc = reference_scenario();
            sc.factor = factor;
            sc.mode = FeeModelMode::ConsensusOnly;
            let f = whale_tx_fee(n, &sc, cfg.min_utxo_value);
            sc.mode = FeeModelMode::ConsensusPlusRelayMin;
            let fr = whale_tx_fee(n, &sc, cfg.min_utxo_value);
            s.push_str(&format!(
                "| {} | {}× | {} | {:.0} | {:.3e} | {:.1e} |\n",
                n,
                factor / 1000,
                f,
                f as f64 / n as f64,
                fr as f64 / n as f64,
                base_fee() / (f as f64 / n as f64),
            ));
        }
    }
    s.push('\n');

    // --- Net vs k at the reference point.
    let rs = reference_scenario();
    s.push_str(&format!(
        "### Whale net vs k — current code ({})\n\n",
        scenario_label(&rs)
    ));
    s.push_str("| k (out/blk) | n/tx | share | cap (BTH) | R (BTH) | winnings/blk | cost/blk | **net/blk** | net/day |\n");
    s.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for k in [1.0, 10.0, 100.0, 200.0, 1000.0, 1584.0] {
        if let Some(o) = evaluate(k, &rs, Rules::CurrentCode, None) {
            s.push_str(&format!(
                "| {:.0} | {} | {:.2}% | {:.1} | {:.4} | {:.4} | {:.6} | {:+.4} | {:+.0} |\n",
                k,
                o.n_per_tx,
                o.share * 100.0,
                bth(o.cap),
                bth(o.payout),
                bth(o.winnings),
                bth(o.cost),
                bth(o.net),
                bth(o.net) * BLOCKS_PER_DAY,
            ));
        }
    }
    s.push('\n');

    // --- Current-code grid.
    s.push_str("### Current code — whale optimum over the grid\n\n");
    s.push_str(
        "R0 = payout/blk with no whale. c* = break-even per-ticket cost (splitting is \
         profitable iff the per-ticket cost is below c*). c = the whale's actual cheapest \
         per-ticket cost at k*. Columns at the realistic-expensive end (floor+MIN_TX_FEE, \
         6×, proposer slot); the last column is the cheapest end (floor only, 1×, \
         block-size bound).\n\n",
    );
    s.push_str("| epoch | backlog | ρ_o | R0 (BTH) | c* (BTH/ticket) | c (BTH/ticket) | k* | share* | **net*/blk** | net*/day | net*/blk (floor, 1×, block-size) |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for epoch in Epoch::swept() {
        for backlog_bth in backlogs() {
            for rho_o in organic_rates() {
                let hi = Scenario {
                    epoch,
                    backlog_bth,
                    rho_o,
                    mode: FeeModelMode::ConsensusPlusRelayMin,
                    factor: 6000,
                    capacity: Capacity::ProposerSlot,
                };
                let lo = Scenario {
                    mode: FeeModelMode::ConsensusOnly,
                    factor: 1000,
                    capacity: Capacity::BlockSize,
                    ..hi
                };
                let o = optimum(&hi, Rules::CurrentCode, None);
                let olo = optimum(&lo, Rules::CurrentCode, None);
                s.push_str(&format!(
                    "| {} | {:.0} | {:.0} | {:.4} | {:.3e} | {:.2e} | {:.0} | {:.1}% | {:+.4} | {:+.0} | {:+.4} |\n",
                    epoch.label(),
                    backlog_bth,
                    rho_o,
                    bth(baseline_payout(&hi, Rules::CurrentCode)),
                    bth(break_even_cost(&hi, Rules::CurrentCode)),
                    bth(o.cost_per_output),
                    o.k,
                    o.share * 100.0,
                    bth(o.net),
                    bth(o.net) * BLOCKS_PER_DAY,
                    bth(olo.net),
                ));
            }
        }
    }
    s.push('\n');

    // --- Window MC.
    s.push_str("### Candidate window (MAX_LOTTERY_CANDIDATES) — seeded-offset walk\n\n");
    s.push_str(&format!(
        "Monte Carlo of the node's walk (seed {:#x}, 400 offsets each): ids are \
         `tx_hash ‖ index`, so a split tx's outputs sit together in key order.\n\n",
        SEED
    ));
    s.push_str("| ρ_o | k | n/tx | eligible set | window count | global share | mean window share | sd (window) | sd (binomial) |\n");
    s.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for w in &report.windows {
        s.push_str(&format!(
            "| {:.0} | {:.0} | {} | {} | {} | {:.3}% | {:.3}% | {:.3}% | {:.3}% |\n",
            w.rho_o,
            w.k,
            w.n_per_tx,
            w.eligible_total,
            w.window_count,
            w.global_share * 100.0,
            w.mean_window_share * 100.0,
            w.sd_window_share * 100.0,
            w.sd_binomial * 100.0,
        ));
    }
    s.push('\n');

    // --- Remedy decision table.
    s.push_str("### Remedy decision table (sim-measured; none implemented)\n\n");
    s.push_str(&format!(
        "Whale optimum over all {} grid points (4 epochs × 2 backlogs × 3 ρ_o × 2 fee modes × \
         2 factors × 2 capacities). Reference = {}.\n\n",
        report.remedies.first().map(|r| r.points).unwrap_or(0),
        scenario_label(&rs)
    ));
    s.push_str("| rule set | profitable points | max net*/blk (BTH) | max net*/day | reference net*/blk | honest R0 at reference (BTH/blk) | honest regimes with inflow > cap | worst point |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---|\n");
    for r in &report.remedies {
        let worst = r
            .worst
            .as_ref()
            .map(|(sc, o)| format!("{} (k*={:.0})", scenario_label(sc), o.k))
            .unwrap_or_else(|| "—".to_string());
        s.push_str(&format!(
            "| {} | {}/{} | {:+.4} | {:+.0} | {:+.4} | {:.4} | {}/{} | {} |\n",
            r.rules.label(),
            r.profitable_points,
            r.points,
            bth(r.max_net),
            bth(r.max_net) * BLOCKS_PER_DAY,
            bth(r.reference_net),
            bth(r.reference_honest_payout),
            r.unstable_regimes,
            r.regimes,
            worst,
        ));
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_vectors_hold_for_the_mirror() {
        for (count, reward, fees, emission, stored, cap, fee_pool, payout) in PARITY_VECTORS {
            assert_eq!(code_reward_cap(count, reward), cap, "cap for count {count}");
            assert_eq!(
                code_pool_payout(fees, emission, stored, cap),
                (fee_pool, payout),
                "accounting for count {count}"
            );
        }
    }

    #[test]
    fn mirrored_constants_match_shared_crates() {
        assert_eq!(
            MAX_OUTPUTS_PER_TX as u64,
            bth_transaction_types::constants::MAX_OUTPUTS
        );
        assert_eq!(
            MAX_BLOCK_SIZE_BYTES,
            bth_transaction_types::constants::MAX_BLOCK_SIZE
        );
        let c = LotteryDrawConfig::default();
        assert_eq!(c.winners_per_draw, 4);
        assert_eq!(c.selection_mode, crate::SelectionMode::Uniform);
        assert_eq!(ticket_life_blocks(), 9_281.0);
        assert_eq!(Epoch(0).block_reward(), 50_000_000_000_000);
        assert_eq!(Epoch(1).emission_share(), 2_500_000_000_000);
    }

    #[test]
    fn uniform_weight_is_value_free() {
        // The share model assumes every eligible ticket weighs the same.
        let t = crate::TagVector::new();
        let a = crate::LotteryCandidate::new([0; 36], 1_000_000, 1000, &t, 0);
        let b = crate::LotteryCandidate::new([1; 36], 1u64 << 50, 6000, &t, 0);
        assert_eq!(
            a.weight(crate::SelectionMode::Uniform, 1u64 << 50),
            b.weight(crate::SelectionMode::Uniform, 1u64 << 50)
        );
    }

    #[test]
    fn sybil_cap_never_binds_in_code() {
        // Any realistic window saturates the 10k truncation: cap = reward.
        for e in Epoch::swept() {
            let s = Scenario {
                epoch: e,
                backlog_bth: 1000.0,
                rho_o: 2.0,
                mode: FeeModelMode::ConsensusOnly,
                factor: 1000,
                capacity: Capacity::ProposerSlot,
            };
            let o = evaluate(1.0, &s, Rules::CurrentCode, None).unwrap();
            assert_eq!(o.cap, e.block_reward() as f64);
        }
    }

    #[test]
    fn current_code_splitting_is_profitable() {
        // The #1682 finding at the reference point (epoch 1, ρ_o=20, relay
        // minimum, 6×, proposer slot): the whale captures most of the payout.
        let o = optimum(&reference_scenario(), Rules::CurrentCode, None);
        assert!(o.share > 0.9, "share {}", o.share);
        assert!(bth(o.net) > 2.0, "net {} BTH/blk", bth(o.net));
    }

    #[test]
    fn single_remedies_do_not_close_the_hole() {
        let g = grid();
        for rules in [
            Rules::PerBlockCap,
            Rules::WindowOverLength,
            Rules::BaseFeePerOutput,
            Rules::PerBlockAndBaseFee,
            Rules::MinValue1Bth,
        ] {
            assert!(
                g.iter().any(|s| optimum(s, rules, None).net > 0.0),
                "{:?} should leave a profitable point",
                rules
            );
        }
    }

    #[test]
    fn combined_remedy_is_non_positive_everywhere() {
        for s in grid() {
            let o = optimum(&s, Rules::PerBlockBaseFeeNoFree, None);
            assert!(
                o.net <= 1e-6 * PICO_PER_BTH as f64,
                "{}: net {}",
                scenario_label(&s),
                bth(o.net)
            );
        }
    }

    #[test]
    fn break_even_matches_closed_form() {
        // With R independent of k (backlog drain), the best split is the
        // smallest one (k = 1, share 1/(ρ_o + 2)), so c* = R0 / (ρ_o + 2).
        let s = Scenario {
            epoch: Epoch(1),
            backlog_bth: 1000.0,
            rho_o: 20.0,
            mode: FeeModelMode::ConsensusOnly,
            factor: 1000,
            capacity: Capacity::BlockSize,
        };
        let r0 = baseline_payout(&s, Rules::CurrentCode);
        let c = break_even_cost(&s, Rules::CurrentCode);
        let closed = r0 / (s.rho_o + 2.0);
        assert!((c / closed - 1.0).abs() < 0.02, "c*={c}, closed={closed}");
    }

    #[test]
    fn window_share_matches_global_share() {
        let w = window_mc(20.0, 200.0, 16, 200);
        assert_eq!(w.window_count, MAX_LOTTERY_CANDIDATES);
        assert!((w.mean_window_share - w.global_share).abs() < 0.01);
    }

    #[test]
    fn report_is_reproducible() {
        let r = FaithfulReport {
            remedies: Vec::new(),
            windows: vec![window_mc(20.0, 20.0, 16, 20)],
        };
        assert_eq!(to_markdown(&r), to_markdown(&r));
        let a = window_mc(20.0, 20.0, 16, 20);
        let b = window_mc(20.0, 20.0, 16, 20);
        assert_eq!(a.mean_window_share, b.mean_window_share);
    }
}
