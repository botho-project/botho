//! Decoy-flood cost model at the real fee floor, with lottery recapture
//! (#1701, part of the design-confidence epic #1683).
//!
//! # The attack
//!
//! An attacker floods the chain with its own outputs so that decoy pickers
//! fill rings with them. If all 19 decoys in a victim's ring are the
//! attacker's, the 20th member is the real input: the ring is fully
//! de-anonymised. The question is what that costs on the rules the node
//! actually enforces.
//!
//! The CT1 research checkpoints (`docs/research/ct1-*.md`) price every
//! attacker output at the CT1 **candidate** base fee, 0.25 BTH = 2.5e11 pico.
//! Nothing in the shipped code charges that. The node charges
//! `max(consensus floor, MIN_TX_FEE)` per *transaction*, where the floor is
//! `1 pico/byte × size × factor × min(n_out, 10)²` (+ demurrage) and
//! `MIN_TX_FEE` is 1e8 pico. Since #1780 the mempool rejects a transaction
//! with more than `MAX_OUTPUTS` = 16 outputs (relay policy); block validation
//! still accepts more (#1772 Part B, pending #1684).
//!
//! A flood output aged 720..=10,000 blocks is also lottery-eligible, so the
//! flooder wins back its ticket share of every lottery payout: 80% of all fees
//! (its own and honest users'), plus the lottery's share of emission from
//! epoch 1 on.
//!
//! # Model
//!
//! Steady state, per block: `h` honest outputs (in 2-output transactions that
//! pay the relay minimum) and `a` attacker outputs (in 16-output
//! transactions). The attacker's share of new outputs is `f = a / (a + h)`.
//! Every block of the chain looks the same, so the attacker's share of any age
//! band — and of the lottery window — is `f`.
//!
//! ## Costs (all from the shared consensus fee model)
//!
//! Per-transaction fees come from [`crate::consensus_fee_breakdown`], which a
//! golden-vector test in `botho` pins to `Ledger::consensus_fee_floor`. Two
//! attacker fee modes are reported:
//!
//! - `relay`: `max(floor, MIN_TX_FEE)`, what a default node relays;
//! - `consensus`: the bare block-validity floor, what an attacker that mines
//!   (or bribes a miner to include) its own transactions pays.
//!
//! The attacker uses background coins (factor 1×, its cheapest class; the
//! demurrage term is then zero) and gives each flood output exactly the
//! lottery dust floor `min_utxo_value` = 1e6 pico. The factor and the
//! input-class floor are resolved by the shared ledger mirror
//! [`crate::resolve_fee_floor`] (#1697) over untagged outputs and an untagged
//! ring, not assumed.
//!
//! Both modes use 16-output transactions. That is exact for `relay` (the
//! mempool cap). For `consensus` it is an upper bound on cost: a self-mining
//! attacker may put more outputs in one block-valid transaction.
//!
//! ## Lottery recapture
//!
//! Per block the pool receives `0.8 × fees + emission_share` and pays
//! `min(available, ρ_window × LOTTERY_BASE_FEE_PICO, block_reward)`
//! (`consensus::lottery::reward_cap` + `compute_pool_accounting`). As #1682
//! notes, `ρ` is the whole-window eligible count (capped at
//! `MAX_LOTTERY_CANDIDATES` = 10,000), so the Path C cap is thousands of BTH
//! and never binds here; the report shows the binding term per row. In steady
//! state the payout equals the inflow unless the inflow exceeds the block
//! reward (then the excess accumulates and the payout is the reward).
//!
//! The winners are drawn uniformly from up to 10,000 eligible UTXOs taken in
//! key order from a seed-derived offset (`lottery_candidates`). The
//! attacker's expected ticket share is therefore `f` when its keys are
//! indistinguishable from honest ones, and lower when it grinds its keys into
//! one clump (see below; [`ground_ticket_share`]).
//!
//! ## Privacy: `P(all decoys are the attacker's)`
//!
//! All three production pickers, via the #1700 ports in
//! [`super::privacy_routes`] (pinned to production by
//! `botho/tests/privacy_sim_route_parity.rs`):
//!
//! Since #1692 every picker draws one random-offset age window per ring (the
//! real input's age uniform inside it, `W + 1` blocks wide with
//! `W = 2 × 10%` of the real age) and takes a uniform sample of the window:
//!
//! - **CLI** (`select_rpc_decoy_pool`): a full Fisher–Yates shuffle of the
//!   window, truncated to 19 — a uniform sample without replacement.
//! - **Web** (`selectDecoysForInput`, #1735; web wallet, Snap, claim links):
//!   when the window holds at least 19 candidates, phase 1 is a partial
//!   Fisher–Yates over it — also a uniform sample without replacement.
//! - **Node** (`select_decoys_for_input`): the first 19 window members of a
//!   per-ring shuffle of the eligible set (#1786) — again a uniform sample.
//!
//! Every window has the same composition (`W + 1` full blocks, the real
//! input's block among them), so `P` is exactly hypergeometric,
//! `C(A, 19) / C(N, 19)`, for all three pickers, whatever the attacker's keys
//! are; the report uses it. Unit tests check each port against the formula by
//! Monte Carlo. (Before #1692 the node clamped a gamma target into a centred
//! band and took the nearest candidate, which needed its own Monte Carlo.)
//!
//! Two attacker key strategies are still distinguished:
//!
//! - **uniform keys**: the attacker's transaction hashes are as random as
//!   anyone's.
//! - **ground keys**: the attacker re-signs each flood transaction until its
//!   hash sorts below every honest transaction hash in the same block. Before
//!   #1786 this made the node picker take the attacker's outputs first, and 19
//!   outputs per block gave `P = 1`. The node picker no longer reads key order,
//!   so grinding does not change `P`; [`node_ground_full_width_prob`] re-runs
//!   the case on the port at full chain width to show it. Grinding still
//!   changes the attacker's lottery ticket share (the lottery candidate scan is
//!   in key order; [`ground_ticket_share`]), downward.
//!
//! # What is *not* modelled
//!
//! Real honest traffic is not uniform in time; coinbase and lottery payout
//! outputs are folded into `h`; the thin-window fallback (fewer than 19
//! window candidates, never reached at these rates) is not modelled;
//! adversaries beyond "all 19 decoys known" are in the #1700 report. Nothing
//! here is consensus code.

use crate::{
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, ClaimedClass, ConsensusFeeInputs,
        ConsensusFeeParams, FeeModelMode, MAINNET_BLOCKS_PER_YEAR,
    },
    fee_curve::{FeeConfig, PICO_PER_BTH},
    fee_floor_inputs::{resolve_fee_floor, FloorOutput, TagPair},
    monetary::{mainnet_policy_mirror, node_constants::FEE_BURN_SHARE_PERMILLE},
    LotteryDrawConfig, MonetaryPolicy, LOTTERY_BASE_FEE_PICO,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;

use super::{
    privacy::DECOY_COUNT,
    privacy_routes::{
        age_similarity_envelope, age_similarity_window_width, draw_age_similarity_window,
        node_select_decoys_banded, node_select_decoys_in_window, window_slice, ChainModel,
        RouteCandidate, MIN_DECOY_AGE_BLOCKS,
    },
};

// ============================================================================
// Production constants (mirrored; pinned by botho/tests/decoy_flood_parity.rs)
// ============================================================================

/// `bth_transaction_types::constants::MAX_OUTPUTS`: outputs per transaction.
/// Enforced by the mempool since #1780 (`Transaction::is_valid_structure`), not
/// by block validation (#1772 Part B).
pub const MAX_OUTPUTS_PER_TX: u64 = 16;

/// `pool_fraction_permille` of `LotteryFeeConfig::default()` and of the
/// consensus `ledger::lottery_v2::canonical_config()`: 80% of fees go to the
/// lottery pool, 20% are burned. Derived from the shared
/// [`crate::monetary::node_constants::FEE_BURN_SHARE_PERMILLE`] (#1751).
pub const LOTTERY_POOL_PERMILLE: u64 = 1_000 - FEE_BURN_SHARE_PERMILLE as u64;

/// `MAX_LOTTERY_CANDIDATES` in `lottery_candidates`
/// (`botho/src/ledger/store/validation.rs`, a function-local constant; the
/// parity test reads the source).
pub const MAX_LOTTERY_CANDIDATES: u64 = 10_000;

/// Halving interval of `botho::monetary::mainnet_policy()` (one 5 s-block
/// year), as a constant for [`epoch_height`]. A unit test checks it against
/// [`mainnet_policy_mirror`].
pub const MAINNET_HALVING_INTERVAL: u64 = MAINNET_BLOCKS_PER_YEAR;

/// The mainnet monetary policy: the shared, drift-guarded
/// [`mainnet_policy_mirror`] (#1751; `ledger::emission_parity_tests`).
/// `cluster-tax`'s `MonetaryPolicy::default()` is a generic 60 s library
/// default, not mainnet.
pub fn mainnet_monetary_policy() -> MonetaryPolicy {
    mainnet_policy_mirror()
}

/// The lottery eligibility rule, straight from the production default
/// (`LotteryDrawConfig::default()`, used by `canonical_config`).
pub fn lottery_draw_config() -> LotteryDrawConfig {
    LotteryDrawConfig::default()
}

/// Number of block ages that are lottery-eligible:
/// `min_utxo_age..=circulation_window`.
pub fn lottery_window_blocks() -> u64 {
    let c = lottery_draw_config();
    c.circulation_window - c.min_utxo_age + 1
}

// ============================================================================
// Fees
// ============================================================================

/// How the attacker's transactions are priced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttackerFeeMode {
    /// `max(consensus floor, MIN_TX_FEE)`: relayed by a default node.
    Relay,
    /// The bare consensus floor: the attacker mines its own transactions.
    Consensus,
}

impl AttackerFeeMode {
    /// Both modes.
    pub const ALL: [AttackerFeeMode; 2] = [AttackerFeeMode::Relay, AttackerFeeMode::Consensus];

    fn model_mode(self) -> FeeModelMode {
        match self {
            AttackerFeeMode::Relay => FeeModelMode::ConsensusPlusRelayMin,
            AttackerFeeMode::Consensus => FeeModelMode::ConsensusOnly,
        }
    }

    /// Report label.
    pub fn label(self) -> &'static str {
        match self {
            AttackerFeeMode::Relay => "relay",
            AttackerFeeMode::Consensus => "consensus",
        }
    }
}

/// Height of the first block of halving epoch `epoch` (1 for epoch 0, so the
/// height is never genesis).
pub fn epoch_height(epoch: u64) -> u64 {
    (epoch * MAINNET_HALVING_INTERVAL).max(1)
}

/// Fee in pico for a 1-input, `n_out`-output transaction of background
/// coins whose outputs total `output_sum`, at `height`.
///
/// The claimed class and the input-class floor come from the shared ledger
/// mirror [`resolve_fee_floor`] (#1697) over untagged outputs (value split
/// evenly) and an untagged ring: no cluster wealth, no recorded import, so
/// both resolve to 1×.
pub fn tx_fee_pico(n_out: u64, output_sum: u64, mode: FeeModelMode, height: u64) -> u64 {
    const NO_TAGS: &[TagPair] = &[];
    let per_output = output_sum / n_out.max(1);
    let outputs: Vec<FloorOutput<'_>> = (0..n_out)
        .map(|_| FloorOutput {
            amount: per_output,
            tags: NO_TAGS,
        })
        .collect();
    let ring: Vec<(u64, &[TagPair])> = vec![(per_output, NO_TAGS); DECOY_COUNT + 1];
    let resolved = resolve_fee_floor(&outputs, &ring, &|_| 0, &|_| false);
    let inputs = ConsensusFeeInputs {
        size: consensus_tx_size_bytes(1, n_out as usize, 0),
        n_out: n_out as usize,
        n_memo: 0,
        claimed: ClaimedClass::Wealth(resolved.claimed_wealth),
        floor: resolved.floor,
        // Ring age only feeds demurrage, which is zero at factor 1×.
        elapsed: 0,
        height,
        output_sum,
    };
    consensus_fee_breakdown(&inputs, &ConsensusFeeParams::mainnet(mode)).fee
}

/// Attacker fees per block for `a` flood outputs: full 16-output transactions
/// plus one partial transaction, each output at the lottery dust floor.
pub fn attacker_fees_per_block(a: u64, mode: AttackerFeeMode, height: u64) -> u64 {
    let dust = lottery_draw_config().min_utxo_value;
    let full = a / MAX_OUTPUTS_PER_TX;
    let rest = a % MAX_OUTPUTS_PER_TX;
    let m = mode.model_mode();
    let mut fees = full * tx_fee_pico(MAX_OUTPUTS_PER_TX, MAX_OUTPUTS_PER_TX * dust, m, height);
    if rest > 0 {
        fees += tx_fee_pico(rest, rest * dust, m, height);
    }
    fees
}

/// Attacker transactions per block for `a` flood outputs.
pub fn attacker_txs_per_block(a: u64) -> u64 {
    a.div_ceil(MAX_OUTPUTS_PER_TX)
}

/// Honest fees per block for `h` outputs in 2-output payments (one 1-output
/// transaction if `h` is odd), paying what a default wallet pays (relay mode).
/// Output value does not change the factor-1× fee.
pub fn honest_fees_per_block(h: u64, height: u64) -> u64 {
    let m = FeeModelMode::ConsensusPlusRelayMin;
    let value = PICO_PER_BTH as u64;
    (h / 2) * tx_fee_pico(2, value, m, height) + (h % 2) * tx_fee_pico(1, value, m, height)
}

// ============================================================================
// Lottery
// ============================================================================

/// Which term of `min(inflow, ρ·base_fee, block_reward)` sets the payout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayoutBinding {
    /// Steady state: the payout is the per-block inflow.
    Inflow,
    /// The Path C cap `ρ_window × LOTTERY_BASE_FEE_PICO`.
    SybilCap,
    /// The anti-grinding block-reward clamp.
    BlockReward,
}

impl PayoutBinding {
    fn label(self) -> &'static str {
        match self {
            PayoutBinding::Inflow => "inflow",
            PayoutBinding::SybilCap => "ρ·base",
            PayoutBinding::BlockReward => "reward",
        }
    }
}

/// Per-block lottery flows in steady state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LotteryFlow {
    /// Block reward at this height.
    pub block_reward: u64,
    /// Lottery share of the block reward.
    pub emission_share: u64,
    /// `0.8 × (honest + attacker fees)`.
    pub fee_pool: u64,
    /// Eligible outputs the consensus cap counts (`min(10k, window × n)`).
    pub rho: u64,
    /// `ρ × LOTTERY_BASE_FEE_PICO`.
    pub sybil_cap: u128,
    /// Steady-state payout per block.
    pub payout: u64,
    /// Which term set the payout.
    pub binding: PayoutBinding,
}

/// Steady-state lottery flow for `total_fees` per block and `outputs_per_block`
/// new eligible outputs per block, at `height`.
pub fn lottery_flow(total_fees: u64, outputs_per_block: u64, height: u64) -> LotteryFlow {
    let policy = mainnet_monetary_policy();
    // Phase 1 only (the sweep's epochs are < halving_count).
    let block_reward = policy.halving_reward(height).unwrap_or(0);
    let emission_share = policy.lottery_emission_share(height, block_reward);
    let fee_pool = (total_fees as u128 * LOTTERY_POOL_PERMILLE as u128 / 1_000) as u64;
    let inflow = fee_pool as u128 + emission_share as u128;
    let rho = (lottery_window_blocks() * outputs_per_block).min(MAX_LOTTERY_CANDIDATES);
    let sybil_cap = rho as u128 * LOTTERY_BASE_FEE_PICO as u128;
    let mut payout = inflow;
    let mut binding = PayoutBinding::Inflow;
    if sybil_cap < payout {
        payout = sybil_cap;
        binding = PayoutBinding::SybilCap;
    }
    if (block_reward as u128) < payout {
        payout = block_reward as u128;
        binding = PayoutBinding::BlockReward;
    }
    LotteryFlow {
        block_reward,
        emission_share,
        fee_pool,
        rho,
        sybil_cap,
        payout: payout as u64,
        binding,
    }
}

/// How the attacker's transaction hashes (and so its UTXO keys) are chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyStrategy {
    /// As random as everyone else's.
    Uniform,
    /// Ground to sort before every honest key in the same block.
    Ground,
}

impl KeyStrategy {
    /// Both strategies.
    pub const ALL: [KeyStrategy; 2] = [KeyStrategy::Uniform, KeyStrategy::Ground];

    /// Report label.
    pub fn label(self) -> &'static str {
        match self {
            KeyStrategy::Uniform => "uniform keys",
            KeyStrategy::Ground => "ground keys",
        }
    }
}

/// The attacker's expected share of lottery tickets.
///
/// `lottery_candidates` collects up to [`MAX_LOTTERY_CANDIDATES`] eligible
/// UTXOs in key order from a seed-derived offset, wrapping. With uniform keys
/// every eligible output is equally likely to be collected, so the share is
/// `a / (a + h)`. With ground keys see [`ground_ticket_share`].
pub fn ticket_share(a: u64, h: u64, keys: KeyStrategy) -> f64 {
    if a + h == 0 {
        return 0.0;
    }
    match keys {
        KeyStrategy::Uniform => a as f64 / (a + h) as f64,
        KeyStrategy::Ground => {
            let w = lottery_window_blocks();
            ground_ticket_share(a * w, h * w, MAX_LOTTERY_CANDIDATES)
        }
    }
}

/// Expected attacker ticket share when its `n_a` eligible outputs form one
/// clump at the bottom of the keyspace and the `n_h` honest outputs are
/// uniform. The collection window takes the next `k` outputs after a uniform
/// offset `u`; with `m = n_h (1 − u)` honest outputs before the wrap (uniform
/// on `[0, n_h]`), the clump contributes `min(n_a, k − m)` tickets when
/// `m < k`. Closed form of `E[tickets] / k`.
pub fn ground_ticket_share(n_a: u64, n_h: u64, k: u64) -> f64 {
    let (na, nh, k) = (n_a as f64, n_h as f64, k as f64);
    if na + nh <= k {
        return if na + nh == 0.0 { 0.0 } else { na / (na + nh) };
    }
    if nh == 0.0 {
        return 1.0;
    }
    let m_max = k.min(nh);
    let m_star = (k - na).max(0.0);
    let part1 = na * m_star.min(m_max);
    let part2 = if m_max > m_star {
        k * (m_max - m_star) - (m_max * m_max - m_star * m_star) / 2.0
    } else {
        0.0
    };
    (part1 + part2) / nh / k
}

// ============================================================================
// Privacy: P(all 19 decoys are the attacker's)
// ============================================================================

/// `C(a, k) / C(n, k)`: the probability that `k` distinct members drawn
/// uniformly from `n` (of which `a` are the attacker's) are all the attacker's.
pub fn all_attacker_prob(a: u64, n: u64, k: u64) -> f64 {
    if k > a || k > n {
        return if k == 0 { 1.0 } else { 0.0 };
    }
    (0..k).fold(1.0, |p, i| p * (a - i) as f64 / (n - i) as f64)
}

/// Number of block ages in a random-offset window around the real input:
/// `W + 1` (every offset gives the same count unless the window is clamped at
/// the confirmation floor, i.e. for real ages below ~13 blocks; 0 for a
/// degenerate window).
pub fn band_blocks(real_age: u64) -> u64 {
    if real_age < MIN_DECOY_AGE_BLOCKS {
        0
    } else {
        age_similarity_window_width(real_age) + 1
    }
}

/// Exact `P(all decoys are the attacker's)` for every picker (#1692: each
/// takes a uniform sample of a `B`-block window that contains the real
/// input's block). The window holds `a × B` attacker and `h × B − 1` honest
/// candidates (the real input is honest and excluded).
pub fn cli_all_attacker_prob(a: u64, h: u64, real_age: u64) -> f64 {
    let b = band_blocks(real_age);
    let attacker = a * b;
    let total = (a + h) * b;
    all_attacker_prob(attacker, total.saturating_sub(1), DECOY_COUNT as u64)
}

/// One node-picker ring: for each block the picker drew from, its age and the
/// in-block positions (key order) it took.
pub type NodeSelection = Vec<(u64, Vec<u64>)>;

/// Node-picker selections for `trials` rings around a real input of
/// `real_age`, on a chain with `outputs_per_block` outputs per block. The real
/// input is the last output of its block (honest under both key strategies).
/// Each ring draws its random-offset window (#1692) and samples it with the
/// distribution-equivalent fast path (or the literal port with the same
/// window when the window is thin).
pub fn node_selections(
    outputs_per_block: u64,
    real_age: u64,
    trials: usize,
    seed: u64,
) -> Vec<NodeSelection> {
    let (lo, hi) = age_similarity_envelope(real_age);
    let chain = ChainModel {
        tip_height: (hi + 2_000).max(real_age + 2_000),
        outputs_per_block,
    };
    let real_id = chain.id_at(real_age, outputs_per_block - 1);
    let real_key = ChainModel::key(real_id);
    let envelope: Vec<RouteCandidate> = chain
        .candidates_by_age(lo, hi)
        .into_iter()
        .filter(|c| c.key != real_key)
        .collect();
    let real_eligible = usize::from(real_age >= MIN_DECOY_AGE_BLOCKS);
    let eligible_total = chain.eligible_count() - real_eligible;
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ real_age.rotate_left(17) ^ outputs_per_block);
    let mut out = Vec::with_capacity(trials);
    for _ in 0..trials {
        let window = draw_age_similarity_window(real_age, &mut rng);
        let band = window_slice(&envelope, window);
        let keys = match node_select_decoys_banded(band, eligible_total, DECOY_COUNT, &mut rng) {
            Some(r) => r.expect("node picker succeeds on a full chain"),
            None => {
                let all = chain.candidates_by_age(0, chain.max_age());
                node_select_decoys_in_window(
                    &all,
                    DECOY_COUNT,
                    &[real_key],
                    real_age,
                    window,
                    &mut rng,
                )
                .expect("node picker succeeds on a full chain")
            }
        };
        let mut by_block: HashMap<u64, Vec<u64>> = HashMap::new();
        for k in &keys {
            let id = ChainModel::id_of(k);
            by_block
                .entry(chain.age_of(id))
                .or_default()
                .push(id % outputs_per_block);
        }
        let mut sel: NodeSelection = by_block.into_iter().collect();
        sel.sort_by_key(|(age, _)| *age);
        out.push(sel);
    }
    out
}

/// Largest `a + h` for which [`node_ground_full_width_prob`] runs (the chain
/// is built at full width, so its band grows with `a + h`).
pub const GROUND_FULL_WIDTH_MAX: u64 = 256;

/// The #1771 grind case, re-run on the node port at full chain width: every
/// block holds `a + h` outputs, the attacker's `a` outputs have the lowest
/// keys of the block (what grinding the transaction hash buys), and the real
/// input is the block's last output. Returns the fraction of `trials` rings
/// whose 19 decoys are all the attacker's.
///
/// Before #1786 the port took each block's lowest unused key first, and this
/// returned 1.0 for `a ≥ 19`. Since #1786 it matches the uniform-key value.
pub fn node_ground_full_width_prob(a: u64, h: u64, real_age: u64, trials: usize, seed: u64) -> f64 {
    let n = a + h;
    assert!(h >= 1, "the real input is honest");
    assert!(
        n <= GROUND_FULL_WIDTH_MAX,
        "full-width chain capped at {GROUND_FULL_WIDTH_MAX}"
    );
    let sels = node_selections(n, real_age, trials, seed ^ 0x1786);
    let hits = sels
        .iter()
        .filter(|sel| sel.iter().all(|(_, pos)| pos.iter().all(|&j| j < a)))
        .count();
    hits as f64 / sels.len() as f64
}

/// The production pickers this sweep covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Picker {
    /// `select_rpc_decoy_pool` (botho-wallet thin CLI). The web picker
    /// (`selectDecoysForInput`, #1735) has the same `P` (a uniform window
    /// sample), so its rows are shared.
    Cli,
    /// `select_decoys_for_input` (node wallet, detached signer). Same `P` as
    /// the CLI since #1692 (a uniform window sample); kept as its own rows.
    Node,
}

impl Picker {
    /// Both pickers.
    pub const ALL: [Picker; 2] = [Picker::Cli, Picker::Node];

    /// Report label.
    pub fn label(self) -> &'static str {
        match self {
            Picker::Cli => "CLI / web",
            Picker::Node => "node",
        }
    }
}

/// `P(all decoys are the attacker's)` for a picker. Key grinding affects no
/// picker: CLI and web shuffle the window, and the node picker shuffles the
/// eligible set (#1786). Since #1692 all three take a uniform sample of the
/// random-offset window, so `P` is the same hypergeometric value for each.
pub fn all_decoys_prob(picker: Picker, a: u64, h: u64, real_age: u64) -> f64 {
    match picker {
        Picker::Cli | Picker::Node => cli_all_attacker_prob(a, h, real_age),
    }
}

/// Smallest attacker rate `a` (outputs/block) with `P ≥ target` at every
/// age in `ages`, searched up to `a_max`. `P` is monotone in `a`.
pub fn min_flood_rate(
    picker: Picker,
    h: u64,
    ages: &[u64],
    target: f64,
    a_max: u64,
) -> Option<u64> {
    let ok = |a: u64| {
        ages.iter()
            .all(|&age| all_decoys_prob(picker, a, h, age) >= target)
    };
    if !ok(a_max) {
        return None;
    }
    let (mut lo, mut hi) = (0u64, a_max); // ok(lo) false (a=0 => P=0), ok(hi) true
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

// ============================================================================
// Sweep
// ============================================================================

/// Sweep configuration.
#[derive(Clone, Debug)]
pub struct SweepConfig {
    /// Honest outputs per block.
    pub honest_rates: Vec<u64>,
    /// Target attacker shares `f`; `a = round(h f / (1 − f))`.
    pub attacker_shares: Vec<f64>,
    /// Block times in seconds (only per-day figures depend on these).
    pub block_times_secs: Vec<u64>,
    /// Halving epochs (0 = no lottery emission, 1 = 10% of a 25 BTH reward).
    pub epochs: Vec<u64>,
    /// Real-input ages (blocks) at which `P` is reported.
    pub real_ages: Vec<u64>,
    /// Node-picker rings per (chain width, age) for the §5 ground-key Monte
    /// Carlo.
    pub trials: usize,
    /// RNG seed.
    pub seed: u64,
    /// `P` target for the pass criterion.
    pub p_target: f64,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            honest_rates: vec![1, 4, 16],
            attacker_shares: vec![0.5, 0.9, 0.95, 0.9642, 0.99],
            block_times_secs: vec![5, 20, 60],
            epochs: vec![0, 1],
            // The pre-#1692 node picker's edge-clamp, mid-band and upper ages,
            // kept so runs stay comparable.
            real_ages: vec![1_000, 22_000, 60_000],
            trials: 4_000,
            seed: 1701,
            p_target: 0.5,
        }
    }
}

impl SweepConfig {
    /// A small configuration for tests and `--smoke`.
    pub fn smoke() -> Self {
        Self {
            honest_rates: vec![1, 4],
            attacker_shares: vec![0.5, 0.9642],
            block_times_secs: vec![5, 60],
            epochs: vec![0, 1],
            real_ages: vec![1_000, 22_000],
            trials: 300,
            ..Self::default()
        }
    }
}

/// Attacker rate for honest rate `h` and target share `f`.
pub fn attacker_rate(h: u64, f: f64) -> u64 {
    ((h as f64) * f / (1.0 - f)).round().max(1.0) as u64
}

/// Per-block economics of one (h, a, epoch, fee mode, key strategy) cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockEconomics {
    /// Attacker fees per block (pico).
    pub gross: u64,
    /// Honest fees per block (pico).
    pub honest_fees: u64,
    /// Lottery flow.
    pub flow: LotteryFlow,
    /// Attacker ticket share.
    pub share: f64,
    /// Expected attacker lottery winnings per block (pico).
    pub recapture: f64,
}

impl BlockEconomics {
    /// `gross − recapture` (negative = the flood is profitable).
    pub fn net(&self) -> f64 {
        self.gross as f64 - self.recapture
    }
}

/// Per-block economics.
pub fn block_economics(
    a: u64,
    h: u64,
    epoch: u64,
    mode: AttackerFeeMode,
    keys: KeyStrategy,
) -> BlockEconomics {
    let height = epoch_height(epoch);
    let gross = attacker_fees_per_block(a, mode, height);
    let honest_fees = honest_fees_per_block(h, height);
    let flow = lottery_flow(gross + honest_fees, a + h, height);
    let share = ticket_share(a, h, keys);
    BlockEconomics {
        gross,
        honest_fees,
        flow,
        share,
        recapture: share * flow.payout as f64,
    }
}

/// Blocks per day at a block time.
pub fn blocks_per_day(block_time_secs: u64) -> f64 {
    86_400.0 / block_time_secs as f64
}

/// The PROPOSED pass threshold, for human ratification (not ratified): the
/// net cost per day of sustaining `P ≥ 0.5` must be at least this fraction of
/// one day's block-reward emission at that block time and epoch (the chain's
/// own security budget as the yardstick), and must be positive.
pub const PROPOSED_MIN_NET_COST_FRACTION_OF_DAILY_EMISSION: f64 = 0.01;

/// One row of the privacy table.
#[derive(Clone, Debug)]
pub struct PrivacyRow {
    pub h: u64,
    pub a: u64,
    pub real_age: u64,
    /// CLI and web picker (exact).
    pub cli: f64,
    /// Node picker (any attacker keys since #1786; exact since #1692).
    pub node: f64,
}

/// One row of the #1771 grind re-run (§5): the node port at full chain
/// width with the attacker's outputs sorted first in every block, against
/// the uniform-key formula at the same rates.
#[derive(Clone, Debug)]
pub struct GrindRow {
    pub h: u64,
    pub a: u64,
    pub real_age: u64,
    /// [`node_ground_full_width_prob`].
    pub ground: f64,
    /// The exact uniform-key value ([`all_decoys_prob`]).
    pub uniform: f64,
}

/// Minimal flood for `P ≥ p_target` at one age, ground keys (full-width
/// port) vs uniform keys (formula).
#[derive(Clone, Debug)]
pub struct GrindMinRow {
    pub h: u64,
    pub real_age: u64,
    /// `None`: not reached with `a + h ≤ GROUND_FULL_WIDTH_MAX`.
    pub ground_a_min: Option<u64>,
    pub uniform_a_min: Option<u64>,
}

/// One row of the per-block economics table.
#[derive(Clone, Debug)]
pub struct EconRow {
    pub h: u64,
    pub a: u64,
    pub epoch: u64,
    pub mode: AttackerFeeMode,
    pub econ: BlockEconomics,
}

/// One row of the pass-criterion table: the cheapest flood reaching
/// `P ≥ p_target` at every reported age.
#[derive(Clone, Debug)]
pub struct CriterionRow {
    pub picker: Picker,
    pub keys: KeyStrategy,
    pub h: u64,
    pub epoch: u64,
    pub mode: AttackerFeeMode,
    /// Minimal attacker outputs/block (None = not reached by `a_max`).
    pub a_min: Option<u64>,
    pub econ: Option<BlockEconomics>,
    /// Per block time: (block time, net cost/day in pico, threshold in pico).
    pub per_day: Vec<(u64, f64, f64)>,
}

impl CriterionRow {
    /// Whether every block time meets the proposed threshold.
    pub fn passes(&self) -> bool {
        self.per_day
            .iter()
            .all(|(_, net, thr)| *net > 0.0 && net >= thr)
    }
}

/// Per-transaction fee reference rows: (label, n_out, consensus floor, relay).
#[derive(Clone, Debug)]
pub struct FeeRow {
    pub label: &'static str,
    pub n_out: u64,
    pub consensus: u64,
    pub relay: u64,
}

/// The full report.
#[derive(Clone, Debug)]
pub struct SweepReport {
    pub config: SweepConfig,
    pub fees: Vec<FeeRow>,
    pub privacy: Vec<PrivacyRow>,
    pub econ: Vec<EconRow>,
    pub criterion: Vec<CriterionRow>,
    /// #1771 grind re-run at h = the smallest honest rate.
    pub grind: Vec<GrindRow>,
    /// #1771 grind re-run: minimal flood per real age.
    pub grind_min: Vec<GrindMinRow>,
}

/// Attacker rates at which §5 re-runs the grind case: 8 and 19 were the
/// pre-#1786 ground-key minima (mid band and band edge).
pub const GRIND_RATES: [u64; 4] = [8, 19, 27, 38];

/// Search ceiling for the minimal flood rate.
pub const A_MAX: u64 = 1 << 16;

/// Run the sweep.
pub fn run_decoy_flood_sweep(config: &SweepConfig) -> SweepReport {
    let h0 = epoch_height(0);
    let dust = lottery_draw_config().min_utxo_value;
    let fee = |n: u64, m: FeeModelMode| tx_fee_pico(n, n * dust, m, h0);
    let fees = vec![
        FeeRow {
            label: "attacker 1-in/16-out, dust outputs",
            n_out: 16,
            consensus: fee(16, FeeModelMode::ConsensusOnly),
            relay: fee(16, FeeModelMode::ConsensusPlusRelayMin),
        },
        FeeRow {
            label: "honest 1-in/2-out payment",
            n_out: 2,
            consensus: tx_fee_pico(2, PICO_PER_BTH as u64, FeeModelMode::ConsensusOnly, h0),
            relay: honest_fees_per_block(2, h0),
        },
    ];

    let mut privacy = Vec::new();
    let mut econ = Vec::new();
    for &h in &config.honest_rates {
        for &f in &config.attacker_shares {
            let a = attacker_rate(h, f);
            for &age in &config.real_ages {
                privacy.push(PrivacyRow {
                    h,
                    a,
                    real_age: age,
                    cli: cli_all_attacker_prob(a, h, age),
                    node: all_decoys_prob(Picker::Node, a, h, age),
                });
            }
            for &epoch in &config.epochs {
                for mode in AttackerFeeMode::ALL {
                    econ.push(EconRow {
                        h,
                        a,
                        epoch,
                        mode,
                        econ: block_economics(a, h, epoch, mode, KeyStrategy::Uniform),
                    });
                }
            }
        }
    }

    let mut criterion = Vec::new();
    // Since #1786 ground keys no longer change `P` for any picker and only
    // lower the attacker's ticket share, so they are dominated by uniform
    // keys; the criterion uses uniform keys (§5 shows the grind re-run).
    let keys = KeyStrategy::Uniform;
    for picker in Picker::ALL {
        for &h in &config.honest_rates {
            let a_min = min_flood_rate(picker, h, &config.real_ages, config.p_target, A_MAX);
            for &epoch in &config.epochs {
                for mode in AttackerFeeMode::ALL {
                    let e = a_min.map(|a| block_economics(a, h, epoch, mode, keys));
                    let per_day = config
                        .block_times_secs
                        .iter()
                        .map(|&bt| {
                            let bpd = blocks_per_day(bt);
                            let net = e.map(|e| e.net() * bpd).unwrap_or(f64::INFINITY);
                            let reward = mainnet_monetary_policy()
                                .halving_reward(epoch_height(epoch))
                                .unwrap_or(0) as f64;
                            let thr =
                                PROPOSED_MIN_NET_COST_FRACTION_OF_DAILY_EMISSION * reward * bpd;
                            (bt, net, thr)
                        })
                        .collect();
                    criterion.push(CriterionRow {
                        picker,
                        keys,
                        h,
                        epoch,
                        mode,
                        a_min,
                        econ: e,
                        per_day,
                    });
                }
            }
        }
    }

    let h_min = config.honest_rates.iter().copied().min().unwrap_or(1);
    let mut grind = Vec::new();
    let mut grind_min = Vec::new();
    for &age in &config.real_ages {
        for &a in &GRIND_RATES {
            grind.push(GrindRow {
                h: h_min,
                a,
                real_age: age,
                ground: node_ground_full_width_prob(a, h_min, age, config.trials, config.seed),
                uniform: all_decoys_prob(Picker::Node, a, h_min, age),
            });
        }
        let ground_ok = |a: u64| {
            node_ground_full_width_prob(a, h_min, age, config.trials, config.seed)
                >= config.p_target
        };
        let a_cap = GROUND_FULL_WIDTH_MAX - h_min;
        let ground_a_min = if ground_ok(a_cap) {
            let (mut lo, mut hi) = (0u64, a_cap);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if ground_ok(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            Some(hi)
        } else {
            None
        };
        grind_min.push(GrindMinRow {
            h: h_min,
            real_age: age,
            ground_a_min,
            uniform_a_min: min_flood_rate(Picker::Node, h_min, &[age], config.p_target, A_MAX),
        });
    }

    SweepReport {
        config: config.clone(),
        fees,
        privacy,
        econ,
        criterion,
        grind,
        grind_min,
    }
}

// ============================================================================
// Markdown
// ============================================================================

fn bth(pico: f64) -> String {
    let v = pico / PICO_PER_BTH as f64;
    if !v.is_finite() {
        return "—".to_string();
    }
    let abs = v.abs();
    if abs == 0.0 {
        "0".to_string()
    } else if abs >= 100.0 {
        format!("{v:.0}")
    } else if abs >= 0.01 {
        format!("{v:.3}")
    } else {
        format!("{v:.2e}")
    }
}

fn prob(p: f64) -> String {
    if p == 0.0 {
        "0".to_string()
    } else if p >= 0.001 {
        format!("{p:.3}")
    } else {
        format!("{p:.1e}")
    }
}

/// Render the report as Markdown.
pub fn to_markdown(r: &SweepReport) -> String {
    use std::fmt::Write;
    let c = &r.config;
    let lc = lottery_draw_config();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Code constants: MIN_TX_FEE = {} pico; CONSENSUS_FEE_BASE = {} pico/byte; output \
         penalty cap = {} outputs; MAX_OUTPUTS = {} (mempool-enforced since #1780, not by block \
         validation); ring size = {} ({} decoys); ±{}% age band; \
         lottery pool = {}‰ of fees; lottery window = ages {}..={} ({} blocks), dust floor {} \
         pico; MAX_LOTTERY_CANDIDATES = {}; LOTTERY_BASE_FEE_PICO = {}; reward epoch 0 = {} \
         BTH, halving every {} blocks; lottery emission share +10%/epoch from epoch 1.\n",
        crate::consensus_fee::RELAY_MIN_TX_FEE,
        crate::consensus_fee::CONSENSUS_FEE_BASE,
        FeeConfig::default().output_count_cap,
        MAX_OUTPUTS_PER_TX,
        DECOY_COUNT + 1,
        DECOY_COUNT,
        super::privacy_routes::AGE_SIMILARITY_SPREAD_BPS / 100,
        LOTTERY_POOL_PERMILLE,
        lc.min_utxo_age,
        lc.circulation_window,
        lottery_window_blocks(),
        lc.min_utxo_value,
        MAX_LOTTERY_CANDIDATES,
        LOTTERY_BASE_FEE_PICO,
        mainnet_monetary_policy().initial_reward / PICO_PER_BTH as u64,
        MAINNET_HALVING_INTERVAL,
    );
    let _ = writeln!(
        s,
        "Node-picker rings per (chain width, age) for the §5 ground-key Monte Carlo: {}; \
         seed {}.\n",
        c.trials, c.seed
    );

    let _ = writeln!(s, "## 1. Fee per transaction (epoch 0, factor 1×)\n");
    let _ = writeln!(
        s,
        "| transaction | consensus floor (pico) | relay fee (pico) | relay pico/output | CT1 candidate pico/output |"
    );
    let _ = writeln!(s, "|---|---:|---:|---:|---:|");
    for f in &r.fees {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {:.3e} | {:.1e} |",
            f.label,
            f.consensus,
            f.relay,
            f.relay as f64 / f.n_out as f64,
            LOTTERY_BASE_FEE_PICO as f64
        );
    }
    if let Some(att) = r.fees.first() {
        let _ = writeln!(
            s,
            "\nThe CT1 docs' 2.5e11 pico per attacker output is {:.0}× the relay cost of a \
             16-output flood output and {:.0}× its consensus floor.\n",
            LOTTERY_BASE_FEE_PICO as f64 / (att.relay as f64 / att.n_out as f64),
            LOTTERY_BASE_FEE_PICO as f64 / (att.consensus as f64 / att.n_out as f64),
        );
    }

    let _ = writeln!(
        s,
        "## 2. P(all {} decoys are the attacker's)\n",
        DECOY_COUNT
    );
    let _ = writeln!(
        s,
        "`a` = attacker outputs/block, `h` = honest outputs/block, `f = a/(a+h)`. Exact \
         (hypergeometric over the window). Since #1692 all three pickers take a uniform sample \
         of a random-offset age window, so the CLI / web and node columns are the same value \
         (each port is checked against the formula by a unit test). Attacker keys do not \
         matter to any picker (§5). `f^19` is the independent-draw reference.\n"
    );
    let _ = writeln!(s, "| h | a | f | f^19 | real age | CLI / web | node |");
    let _ = writeln!(s, "|---:|---:|---:|---:|---:|---:|---:|");
    for p in &r.privacy {
        let f = p.a as f64 / (p.a + p.h) as f64;
        let _ = writeln!(
            s,
            "| {} | {} | {:.4} | {} | {} | {} | {} |",
            p.h,
            p.a,
            f,
            prob(f.powi(DECOY_COUNT as i32)),
            p.real_age,
            prob(p.cli),
            prob(p.node)
        );
    }

    let _ = writeln!(
        s,
        "\n## 3. Per-block economics (uniform keys; BTH per block)\n"
    );
    let _ = writeln!(
        s,
        "Recapture = ticket share × steady-state payout; payout = min(inflow, ρ·base, block \
         reward). Net < 0 means the flood pays for itself and then some.\n"
    );
    let _ = writeln!(
        s,
        "| h | a | f | epoch | attacker fee | gross | honest fees | payout | binds | ρ·base cap | recapture | net | recapture/gross |"
    );
    let _ = writeln!(
        s,
        "|---:|---:|---:|---:|---|---:|---:|---:|---|---:|---:|---:|---:|"
    );
    for e in &r.econ {
        let x = &e.econ;
        let f = e.a as f64 / (e.a + e.h) as f64;
        let _ = writeln!(
            s,
            "| {} | {} | {:.4} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            e.h,
            e.a,
            f,
            e.epoch,
            e.mode.label(),
            bth(x.gross as f64),
            bth(x.honest_fees as f64),
            bth(x.flow.payout as f64),
            x.flow.binding.label(),
            bth(x.flow.sybil_cap as f64),
            bth(x.recapture),
            bth(x.net()),
            if x.gross == 0 {
                "—".to_string()
            } else {
                let ratio = x.recapture / x.gross as f64;
                if ratio >= 1_000.0 {
                    format!("{ratio:.1e}")
                } else {
                    format!("{ratio:.2}")
                }
            }
        );
    }

    let _ = writeln!(s, "\n## 4. Pass criterion (PROPOSED, not ratified)\n");
    let _ = writeln!(
        s,
        "Proposed: sustaining P ≥ {} at every reported age ({:?}) must cost a **positive** net \
         amount per day of at least {:.0}% of that day's block-reward emission. Minimal flood \
         `a_min` found by search; costs per day in BTH; threshold in brackets.\n",
        c.p_target,
        c.real_ages,
        PROPOSED_MIN_NET_COST_FRACTION_OF_DAILY_EMISSION * 100.0
    );
    let bt0 = c.block_times_secs.first().copied().unwrap_or(5);
    let mut header = format!(
        "| picker | keys | h | a_min | attacker tx/block | epoch | attacker fee | gross/day @{bt0}s | recapture/day @{bt0}s |"
    );
    let mut rule = String::from("|---|---|---:|---:|---:|---:|---|---:|---:|");
    for bt in &c.block_times_secs {
        let _ = write!(header, " net/day @{bt}s [thr] |");
        rule.push_str("---:|");
    }
    header.push_str(" verdict |");
    rule.push_str("---|");
    let _ = writeln!(s, "{header}\n{rule}");
    for row in &r.criterion {
        let mut line = format!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            row.picker.label(),
            row.keys.label(),
            row.h,
            row.a_min
                .map(|a| a.to_string())
                .unwrap_or_else(|| format!("> {A_MAX}")),
            row.a_min
                .map(|a| attacker_txs_per_block(a).to_string())
                .unwrap_or_else(|| "—".into()),
            row.epoch,
            row.mode.label()
        );
        let bpd0 = blocks_per_day(bt0);
        let _ = write!(
            line,
            " {} | {} |",
            row.econ
                .map(|e| bth(e.gross as f64 * bpd0))
                .unwrap_or_else(|| "—".into()),
            row.econ
                .map(|e| bth(e.recapture * bpd0))
                .unwrap_or_else(|| "—".into())
        );
        for (_, net, thr) in &row.per_day {
            let _ = write!(line, " {} [{}] |", bth(*net), bth(*thr));
        }
        let _ = write!(line, " {} |", if row.passes() { "PASS" } else { "FAIL" });
        let _ = writeln!(s, "{line}");
    }

    let _ = writeln!(
        s,
        "\n## 5. Node picker with ground keys: the #1771 grind case after #1786\n"
    );
    let _ = writeln!(
        s,
        "The node port at full chain width (a + h outputs per block), with the attacker's \
         outputs sorted first in every block, against the uniform-key value at the same rates. \
         Before #1786 the port took each block's lowest key first and the ground column was \
         1.000 for a ≥ 19 at every age. Monte Carlo, {} rings per cell.\n",
        c.trials
    );
    let _ = writeln!(
        s,
        "| h | a | real age | node, ground keys | node, uniform keys |"
    );
    let _ = writeln!(s, "|---:|---:|---:|---:|---:|");
    for g in &r.grind {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} |",
            g.h,
            g.a,
            g.real_age,
            prob(g.ground),
            prob(g.uniform)
        );
    }
    let _ = writeln!(
        s,
        "\nMinimal attacker outputs per block for P ≥ {} (ground keys searched up to a + h = \
         {GROUND_FULL_WIDTH_MAX}; pre-#1786 ground-key minima were 19 / 8 / 19 at ages 1000 / \
         22000 / 60000).\n",
        c.p_target
    );
    let _ = writeln!(
        s,
        "| h | real age | a_min, ground keys | a_min, uniform keys |"
    );
    let _ = writeln!(s, "|---:|---:|---:|---:|");
    for g in &r.grind_min {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} |",
            g.h,
            g.real_age,
            g.ground_a_min
                .map(|a| a.to_string())
                .unwrap_or_else(|| format!("> {}", GROUND_FULL_WIDTH_MAX - g.h)),
            g.uniform_a_min
                .map(|a| a.to_string())
                .unwrap_or_else(|| format!("> {A_MAX}"))
        );
    }
    s
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::privacy_routes::{
        cli_select_rpc_decoys_for_input, node_select_decoys_for_input as node_literal,
        web_select_decoys_for_input, Mulberry32,
    };
    use rand::SeedableRng;

    #[test]
    fn fee_floor_matches_hand_arithmetic() {
        let h = epoch_height(0);
        // 50 + 700 + 16*120 = 2670 bytes; penalty min(16,10)^2 = 100; factor 1x.
        let floor16 = tx_fee_pico(16, 16_000_000, FeeModelMode::ConsensusOnly, h);
        assert_eq!(floor16, 2_670 * 100);
        assert_eq!(
            tx_fee_pico(16, 16_000_000, FeeModelMode::ConsensusPlusRelayMin, h),
            100_000_000
        );
        // 1 honest 2-output payment pays the relay minimum.
        assert_eq!(honest_fees_per_block(2, h), 100_000_000);
        assert_eq!(
            attacker_fees_per_block(17, AttackerFeeMode::Relay, h),
            200_000_000
        );
        assert_eq!(attacker_txs_per_block(17), 2);
    }

    #[test]
    fn epoch_one_routes_ten_percent_of_reward_to_lottery() {
        let flow = lottery_flow(0, 10, epoch_height(1));
        assert_eq!(flow.block_reward, 25 * PICO_PER_BTH as u64);
        assert_eq!(flow.emission_share, 25 * PICO_PER_BTH as u64 / 10);
        assert_eq!(flow.binding, PayoutBinding::Inflow);
        let flow0 = lottery_flow(1_000, 10, epoch_height(0));
        assert_eq!(flow0.emission_share, 0);
        assert_eq!(flow0.fee_pool, 800);
        assert_eq!(flow0.rho, MAX_LOTTERY_CANDIDATES);
    }

    #[test]
    fn payout_clamps_at_block_reward() {
        let huge = 200 * PICO_PER_BTH as u64;
        let flow = lottery_flow(huge, 10, epoch_height(0));
        assert_eq!(flow.binding, PayoutBinding::BlockReward);
        assert_eq!(flow.payout, 50 * PICO_PER_BTH as u64);
    }

    #[test]
    fn ground_share_closed_form_matches_monte_carlo() {
        // Small keyspace so a direct simulation is cheap.
        let (na, nh, k) = (300u64, 2_000u64, 500u64);
        let analytic = ground_ticket_share(na, nh, k);
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        use rand::RngExt;
        let mut honest: Vec<f64> = (0..nh).map(|_| rng.random::<f64>()).collect();
        honest.sort_by(|x, y| x.partial_cmp(y).unwrap());
        // Attacker clump at keys just above 0 (below every honest key).
        let mut keys: Vec<(f64, bool)> = (0..na).map(|i| (i as f64 * 1e-12, true)).collect();
        keys.extend(honest.iter().map(|&x| (x + 1e-9, false)));
        let total = keys.len();
        let trials = 20_000;
        let mut sum = 0.0;
        for _ in 0..trials {
            let u: f64 = rng.random();
            let start = keys.partition_point(|(x, _)| *x < u);
            let got = (0..k as usize)
                .filter(|i| keys[(start + i) % total].1)
                .count();
            sum += got as f64 / k as f64;
        }
        let mc = sum / trials as f64;
        assert!(
            (mc - analytic).abs() < 0.01,
            "mc {mc} vs analytic {analytic}"
        );
        // Uniform when everything fits under the cap.
        assert!((ground_ticket_share(10, 30, 100) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn cli_port_matches_hypergeometric() {
        // 9 attacker + 1 honest outputs per block, small band.
        let (a, h) = (9u64, 1u64);
        let n = a + h;
        let real_age = 100;
        let chain = ChainModel {
            tip_height: 400,
            outputs_per_block: n,
        };
        let real_key = ChainModel::key(chain.id_at(real_age, n - 1));
        let all = chain.candidates_by_age(0, chain.max_age());
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let trials = 20_000;
        let mut hits = 0usize;
        for _ in 0..trials {
            let keys =
                cli_select_rpc_decoys_for_input(&all, real_age, &[real_key], DECOY_COUNT, &mut rng)
                    .unwrap();
            if keys.iter().all(|k| ChainModel::id_of(k) % n < a) {
                hits += 1;
            }
        }
        let mc = hits as f64 / trials as f64;
        let exact = cli_all_attacker_prob(a, h, real_age);
        assert!(
            exact > 0.005,
            "test needs a measurable probability: {exact}"
        );
        let se = (exact * (1.0 - exact) / trials as f64).sqrt();
        assert!(
            (mc - exact).abs() < 5.0 * se + 1e-3,
            "mc {mc} vs exact {exact}"
        );
    }

    #[test]
    fn node_port_matches_hypergeometric() {
        // Since #1692 the node picker takes a uniform sample of the
        // random-offset window, so the CLI formula applies to it too. Literal
        // port and the sweep's banded fast path (`node_selections`) both.
        let (a, h) = (9u64, 1u64);
        let n = a + h;
        let real_age = 100;
        let chain = ChainModel {
            tip_height: 400,
            outputs_per_block: n,
        };
        let real_key = ChainModel::key(chain.id_at(real_age, n - 1));
        let all = chain.candidates_by_age(0, chain.max_age());
        let mut rng = ChaCha8Rng::seed_from_u64(12);
        let trials = 6_000;
        let mut hits = 0usize;
        for _ in 0..trials {
            let keys = node_literal(&all, DECOY_COUNT, &[real_key], real_age, &mut rng).unwrap();
            hits += usize::from(keys.iter().all(|k| ChainModel::id_of(k) % n < a));
        }
        let exact = cli_all_attacker_prob(a, h, real_age);
        assert!(
            exact > 0.005,
            "test needs a measurable probability: {exact}"
        );
        let se = (exact * (1.0 - exact) / trials as f64).sqrt();
        let mc = hits as f64 / trials as f64;
        assert!(
            (mc - exact).abs() < 5.0 * se + 1e-3,
            "literal {mc} vs exact {exact}"
        );

        let sels = node_selections(n, real_age, trials, 13);
        let fast = sels
            .iter()
            .filter(|s| s.iter().all(|(_, pos)| pos.iter().all(|&j| j < a)))
            .count() as f64
            / trials as f64;
        assert!(
            (fast - exact).abs() < 5.0 * se + 1e-3,
            "banded {fast} vs exact {exact}"
        );
    }

    #[test]
    fn node_ground_keys_no_longer_help_after_1786() {
        // The #1771 grind case. Before #1786, 19 ground outputs per block gave
        // P = 1 at the band edge and 8 gave P > 0.5 mid band. With the
        // shuffled eligible set the ground-key value tracks the uniform one.
        let ground = node_ground_full_width_prob(19, 1, 1_000, 1_000, 5);
        let uniform = all_decoys_prob(Picker::Node, 19, 1, 1_000);
        let se = (uniform * (1.0 - uniform) / 1_000.0).sqrt();
        assert!(
            (ground - uniform).abs() < 5.0 * se + 0.01,
            "ground {ground} vs uniform {uniform}"
        );
        // Mid band: 8 ground outputs among 24 per block do not suffice.
        assert!(node_ground_full_width_prob(8, 16, 22_000, 1_000, 5) < 0.01);
    }

    #[test]
    fn node_uniform_tracks_independent_reference() {
        let (a, h) = (27u64, 1u64);
        let f = a as f64 / (a + h) as f64;
        let p = all_decoys_prob(Picker::Node, a, h, 22_000);
        assert!(
            (p - f.powi(19)).abs() < 0.05,
            "p {p} vs f^19 {}",
            f.powi(19)
        );
    }

    #[test]
    fn node_literal_port_rerun_of_grind_case() {
        // The #1771 grind case on the literal port (#1786 shuffle included):
        // 19 attacker outputs with the lowest keys plus 1 honest output per
        // block. Key order does not matter, so P is the uniform-key
        // hypergeometric value (it was 1 before #1786). The banded fast path
        // the sweep uses must agree.
        let (a, h) = (19u64, 1u64);
        let n = a + h;
        let chain = ChainModel {
            tip_height: 400,
            outputs_per_block: n,
        };
        let age = 200;
        let (lo, hi) = age_similarity_envelope(age);
        let real_key = ChainModel::key(chain.id_at(age, n - 1));
        let envelope: Vec<RouteCandidate> = chain
            .candidates_by_age(lo, hi)
            .into_iter()
            .filter(|c| c.key != real_key)
            .collect();
        let all = chain.candidates_by_age(0, chain.max_age());
        let ground = |k: &[[u8; 32]]| k.iter().all(|k| ChainModel::id_of(k) % n < a);
        let trials = 800;
        let mut r1 = ChaCha8Rng::seed_from_u64(4);
        let mut r2 = ChaCha8Rng::seed_from_u64(5);
        let (mut fast_hits, mut slow_hits) = (0usize, 0usize);
        for _ in 0..trials {
            let window = draw_age_similarity_window(age, &mut r1);
            let fast = node_select_decoys_banded(
                window_slice(&envelope, window),
                chain.eligible_count() - 1,
                DECOY_COUNT,
                &mut r1,
            )
            .unwrap()
            .unwrap();
            let slow = node_literal(&all, DECOY_COUNT, &[real_key], age, &mut r2).unwrap();
            fast_hits += usize::from(ground(&fast));
            slow_hits += usize::from(ground(&slow));
        }
        let expected = cli_all_attacker_prob(a, h, age);
        let se = (expected * (1.0 - expected) / trials as f64).sqrt();
        for (name, hits) in [("banded", fast_hits), ("literal", slow_hits)] {
            let p = hits as f64 / trials as f64;
            assert!((p - expected).abs() < 5.0 * se, "{name}: {p} vs {expected}");
        }
    }

    #[test]
    fn web_port_matches_hypergeometric() {
        // The web picker (#1735) takes a uniform window sample, so the CLI
        // formula applies to it too.
        let (a, h) = (9u64, 1u64);
        let n = a + h;
        let real_age = 100;
        let chain = ChainModel {
            tip_height: 400,
            outputs_per_block: n,
        };
        let real_key = ChainModel::key(chain.id_at(real_age, n - 1));
        let pool: Vec<RouteCandidate> = chain
            .candidates_by_age(0, chain.max_age())
            .into_iter()
            .filter(|c| c.key != real_key)
            .collect();
        let mut rng = Mulberry32::new(1735);
        let mut next = || rng.next_u32();
        let trials = 20_000;
        let mut hits = 0usize;
        for _ in 0..trials {
            let keys =
                web_select_decoys_for_input(&pool, DECOY_COUNT, real_age, &mut next).unwrap();
            if keys.iter().all(|k| ChainModel::id_of(k) % n < a) {
                hits += 1;
            }
        }
        let mc = hits as f64 / trials as f64;
        let exact = cli_all_attacker_prob(a, h, real_age);
        let se = (exact * (1.0 - exact) / trials as f64).sqrt();
        assert!(
            (mc - exact).abs() < 5.0 * se + 1e-3,
            "mc {mc} vs exact {exact}"
        );
    }

    #[test]
    fn shared_mirrors_match_local_constants() {
        let policy = mainnet_policy_mirror();
        assert_eq!(MAINNET_HALVING_INTERVAL, policy.halving_interval);
        assert_eq!(policy.target_block_time_secs, 5);
        assert_eq!(LOTTERY_POOL_PERMILLE, 800);
        // The shared fee-floor mirror resolves untagged flood traffic to 1x.
        let o = [FloorOutput {
            amount: 1_000_000,
            tags: &[],
        }];
        let r = resolve_fee_floor(&o, &[(1_000_000, &[])], &|_| 0, &|_| false);
        assert_eq!((r.claimed_factor, r.floor), (1_000, 1_000));
    }

    #[test]
    fn smoke_report_renders_and_flags_cheap_floods() {
        let report = run_decoy_flood_sweep(&SweepConfig::smoke());
        let md = to_markdown(&report);
        assert!(md.contains("Pass criterion (PROPOSED, not ratified)"));
        // Since #1786 ground keys no longer beat the node picker with <= 19
        // outputs per block (the pre-#1786 minima were 19 and 8).
        for g in &report.grind_min {
            assert!(g.ground_a_min.is_none_or(|a| a > 19), "{g:?}");
        }
        // Uniform flood: CLI needs f >= ~0.964.
        let cli = report
            .criterion
            .iter()
            .find(|r| r.picker == Picker::Cli && r.h == 1)
            .unwrap();
        assert!((25..=30).contains(&cli.a_min.unwrap()));
    }
}
