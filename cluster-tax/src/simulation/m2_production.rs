//! M2-production (#1696): the M2 Gini experiment re-run at the node's real
//! parameters.
//!
//! The legacy M2 harness ([`super::m2`]) is kept unchanged so its results stay
//! reproducible. It assumes 4,320 blocks/day, a fixed 1.6-unit lottery pool per
//! block, a flat-fee economy, one cluster per agent with a hand-ratcheted
//! cumulative wealth, and no mining. This module rebuilds the experiment on the
//! rules the node actually runs:
//!
//! 1. **Time and emission.** 6,307,200 blocks/yr (5 s), the
//!    [`crate::mainnet_monetary_policy`] mirror of `botho::monetary::
//!    mainnet_policy` (50 BTH, 1-year halvings, 5 halvings, supply-indexed 2%
//!    tail recomputed from `total_mined` as `calculate_block_reward` does), the
//!    real lottery-emission ramp (`lottery_emission_bps`: 0 in epoch 0, then
//!    +10%/epoch to 50%) and the epoch-0 demurrage holiday, on one clock.
//! 2. **Lottery.** The production V2 rules: candidates are outputs aged
//!    720..=10,000 blocks with value >= 1e6 pico (capped at 10k, a uniform
//!    subset), a uniform draw of 4 winners per block, the per-block payout
//!    `min(carry + emission share + 80% fees, min(rho * 0.25 BTH,
//!    block_reward))` with the remainder carried, and each award created as a
//!    **new output** that copies the winning output's cluster tags (and adds to
//!    those clusters' W), as `writer.rs` does.
//! 3. **Clusters.** Every coinbase starts a fresh cluster whose W is the miner
//!    payout (<= 50 BTH). Spends merge inputs value-weighted
//!    (`merge_weighted`), the stock wallet applies its 5%/hop decay, tags are
//!    pruned below 0.1% (`MIN_STORED_WEIGHT`) and truncated to 16
//!    (`MAX_CLUSTER_TAGS`). Every created output adds `amount * weight` to each
//!    tagged cluster's W, which therefore only grows.
//! 4. **Adversary.** The strategic 5M-BTH whale also mines. In
//!    [`WhaleStrategy::OptimalDecoy`] it fills every ring with the decoys that
//!    minimise the value-weighted ring-centroid floor (#1686) and, per spend,
//!    takes the cheaper of two legal claims: its merged class undecayed
//!    (in-class, so only accrued demurrage), or background output tags (legal:
//!    the inheritance check only rejects *inflated* tag mass, #1687), which
//!    pays the #925 downgrade charge from the diluted floor.
//! 5. **Output.** Gini at each snapshot (10 and 20 years), the share of
//!    top-decile value priced below 1.5x, and the same run with the mechanism
//!    switched off (no lottery share, fees burned, no demurrage) so the
//!    mechanism's own contribution can be separated from the mining
//!    distribution.
//!
//! Fees are priced with the shared [`crate::consensus_fee`] model (#1695);
//! nothing here re-derives the fee floor.
//!
//! # Approximations (stated; see `docs/research/m2-production-verdicts.md`)
//!
//! - **Time aggregation.** Blocks are processed in steps of `step_blocks`
//!   (default 720). Within a step the candidate window is fixed: outputs
//!   created in step `t` are eligible in steps `t + ceil(720/S) ..= t +
//!   floor(10000/S)`. The 4 x S winner slots are drawn as one multinomial over
//!   owners in proportion to their eligible outputs (with replacement across
//!   the step; the node draws without replacement inside one block, which
//!   differs by O(4/rho)). Per-block reward, emission share, pool and cap are
//!   computed block by block. `step_blocks = 1` is the exact per-block mode
//!   used to validate the aggregation.
//! - **Wallets.** Each agent consolidates all of its coins whenever it spends
//!   (one payment output + one change output per transaction, chunked at 10M
//!   BTH per transaction because ledger amounts are `u64`). The demurrage clock
//!   is the oldest input's age (the node's max-quantile ring age, with decoys
//!   assumed no older than the real inputs).
//! - **Honest rings.** Honest decoys are treated as negligible in value, so the
//!   ring-centroid floor is the inputs' own (undecayed) effective wealth. The
//!   stock wallet's 5% decay then shows up as a small capitalized downgrade
//!   charge, as it would on the node.
//! - **Optimal-decoy universe.** The whale's decoy candidates are the other
//!   agents' consolidated positions (one UTXO each). A real chain offers far
//!   more background outputs, so the dilution here is a lower bound.
//! - **Tag-mass bookkeeping.** A position's coins are tracked as the union of
//!   their tag mass. When the union exceeds 4,096 clusters (and again each time
//!   it doubles), mass below 1e-6 of the position is dropped. That mass is too
//!   small to reach any merged output tag, but it could raise the ring-centroid
//!   floor slightly, so dropping it errs toward a lower floor.
//! - **Consolidation charge.** Because the wallet consolidates, the merged
//!   output tags are often pruned or truncated well below the inputs'
//!   value-weighted centroid. The node prices that gap as a #925 downgrade. A
//!   coin-selecting wallet that never moves tainted coins would avoid it (and
//!   spend-time demurrage altogether), so the demurrage collected here is an
//!   upper bound, in the mechanism's favour.
//! - **Payout tags.** When an owner wins `k` slots, they are spread over that
//!   owner's eligible outputs in proportion to count, and each payout copies
//!   the chosen output's tags.
//! - **Endowments.** The M2 ladder is a genesis endowment, each agent's
//!   holdings in one cluster with W = holdings (the legacy M2 assumption; the
//!   real chain has no pre-mine). This maximises the whales' factors, again in
//!   the mechanism's favour.
//! - **Payments.** Cohorts transact at 52 (small), 24 (middle), 365 (merchant)
//!   and 12 (whales) transactions a year, each paying `velocity / tx_per_year`
//!   of the position. The recipient is drawn in proportion to its spending flow
//!   (velocity x value), so each agent's expected inflow equals its expected
//!   outflow: payments by themselves neither concentrate nor equalise wealth.
//!   (Drawing in proportion to wealth alone would drain high-velocity agents
//!   into low-velocity whales.) Payments are lumpy, though, so they add noise
//!   to the Gini path.
//! - **No lottery farming.** No agent splits outputs to farm tickets; that
//!   attack is #1682.
//! - **Spent outputs leave the draw.** A spend consumes every coin of the
//!   position, so those coins drop out of the candidate window and out of the
//!   not-yet-mature buckets, as on the node, where candidates come only from
//!   the UTXO set. Each agent carries a spend generation; window entries and
//!   pending bucket entries from an older generation are spent coins.
//! - **Transaction density.** The population makes about 13,000 transactions a
//!   year against 6.3M blocks, so about 99.6% of window outputs are coinbases
//!   and lottery payouts. `tx_scale` multiplies every cohort's transaction rate
//!   at a fixed velocity (each transaction moves `velocity / tx_per_year` of
//!   the position), to test how the result depends on density.

use std::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Binomial, Distribution};

use crate::{
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, mainnet_monetary_policy, ClaimedClass,
        ConsensusFeeInputs, ConsensusFeeParams, FeeModelMode, LOTTERY_POOL_FRACTION_PERMILLE,
        MAINNET_BLOCKS_PER_YEAR,
    },
    demurrage::{demurrage_charge, ring_centroid_implied_factor},
    fee_curve::{ClusterFactorCurve, PICO_PER_BTH},
    LotteryDrawConfig, MonetaryPolicy, LOTTERY_BASE_FEE_PICO,
};

use super::m2::m2_population;

/// Blocks per year at the node's 5 s monetary block time.
pub const PROD_BLOCKS_PER_YEAR: u64 = MAINNET_BLOCKS_PER_YEAR;
/// `MAX_LOTTERY_CANDIDATES` in `botho/src/ledger/store/validation.rs`, via
/// the faithful sweep's mirror (source-pinned in `lottery_v2.rs`; #1702).
pub const MAX_LOTTERY_CANDIDATES: u64 =
    super::lottery_reward_cap_faithful::MAX_LOTTERY_CANDIDATES as u64;
/// `canonical_config().pool_fraction_permille` (`lottery_v2.rs`), imported
/// from the shared [`LOTTERY_POOL_FRACTION_PERMILLE`] (#1702).
pub const POOL_FRACTION_PERMILLE: u128 = LOTTERY_POOL_FRACTION_PERMILLE as u128;
/// `MAX_CLUSTER_TAGS` (`transaction/types/src/cluster_tags.rs`), imported.
pub const MAX_CLUSTER_TAGS: usize = bth_transaction_types::MAX_CLUSTER_TAGS;
/// `MIN_STORED_WEIGHT` / `TAG_WEIGHT_SCALE` = 1000 ppm = 0.1%, imported.
pub const MIN_TAG_WEIGHT: f64 = bth_transaction_types::MIN_STORED_WEIGHT as f64
    / bth_transaction_types::TAG_WEIGHT_SCALE as f64;
/// Stock wallet decay per hop (`DEFAULT_CLUSTER_DECAY_RATE` = 50,000 ppm).
/// The wallet constant lives in `botho`/`botho-wallet`, which depend on this
/// crate; `botho/tests/sim_code_parity.rs` pins it (#1702).
pub const STOCK_WALLET_DECAY: f64 = 0.05;
/// `RING_SIZE` (`transaction/types/src/constants.rs`), imported.
pub const RING_SIZE: usize = bth_transaction_types::RING_SIZE;
/// Largest amount priced as one transaction (ledger amounts are `u64` pico).
const TX_CHUNK_PICO: u128 = 10_000_000 * PICO_PER_BTH;
/// Union size above which tiny tag mass is dropped from a position.
const MASS_PRUNE_TRIGGER: usize = 4096;
/// Tag mass below this fraction of the position is dropped at that point.
const MASS_PRUNE_FRACTION: f64 = 1e-6;
/// Strategic whale holdings in BTH (as in legacy M2).
pub const STRATEGIC_WHALE_BTH: u128 = 5_000_000;
/// Factor threshold for the "priced below 1.5x" metric.
pub const PRICED_FACTOR_THRESHOLD: u64 = 1_500;

/// How hashpower is distributed across the population.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiningModel {
    /// Hashpower proportional to the genesis endowment (capital buys
    /// hashpower).
    WealthProportional,
    /// Every agent has the same hashpower (the RandomX/CPU-mining ideal).
    Egalitarian,
}

impl MiningModel {
    pub fn label(self) -> &'static str {
        match self {
            MiningModel::WealthProportional => "wealth",
            MiningModel::Egalitarian => "egalitarian",
        }
    }
}

/// The strategic whale's behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhaleStrategy {
    /// Stock wallet: merged tags with 5%/hop decay, honest decoys.
    Honest,
    /// Floor-minimising decoys plus, per spend, the cheaper of an undecayed
    /// in-class claim or a background claim (#1686/#1687).
    OptimalDecoy,
}

impl WhaleStrategy {
    pub fn label(self) -> &'static str {
        match self {
            WhaleStrategy::Honest => "honest",
            WhaleStrategy::OptimalDecoy => "optimal-decoy",
        }
    }
}

/// How a lottery candidate's ticket weight is computed (#1801 redesign
/// study). [`TicketRule::Uniform`] is the node's rule and the #1790 baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketRule {
    /// One ticket per eligible output (`SelectionMode::Uniform`, the node).
    Uniform,
    /// Weight = output value x output age (coin-age). Value-linear, so a
    /// split adds no weight, and it discounts outputs that are recycled
    /// often.
    ValueAge,
    /// Weight = value x (max factor - factor + 1), the existing
    /// `SelectionMode::ClusterWeighted` kernel: a 1x output has 6x the
    /// per-value weight of a 6x output.
    InverseFactor,
    /// Uniform draw, but an output is eligible only if it buys a ticket, at
    /// a price equal to the average payout per ticket (previous step's
    /// payout / tickets bought in that step), burned. Coinbase and payout
    /// outputs must buy too (no free tickets). Wallets buy rationally: only
    /// for outputs worth more than the price, and (under #1790's rule, where
    /// a spent ticket stops counting) only if the owner's median holding time
    /// covers the circulation window.
    TicketPrice,
}

impl TicketRule {
    pub fn label(self) -> &'static str {
        match self {
            TicketRule::Uniform => "uniform",
            TicketRule::ValueAge => "value-age",
            TicketRule::InverseFactor => "inverse-factor",
            TicketRule::TicketPrice => "ticket-price",
        }
    }
}

/// How honest wallets decay their output tags (#1801 redesign study).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagDecay {
    /// The stock wallet: 5% per hop, however often the holder spends.
    PerHop,
    /// Decay keyed to elapsed blocks since the position's last spend:
    /// `(1 - 5%)^(elapsed / TIME_DECAY_REF_BLOCKS)`. The reference interval is
    /// the 1x whale's mean inter-spend time (1/12 yr), so the 1x whale decays
    /// about as before, but churn no longer speeds decay up.
    PerBlock,
}

impl TagDecay {
    pub fn label(self) -> &'static str {
        match self {
            TagDecay::PerHop => "per-hop",
            TagDecay::PerBlock => "per-block",
        }
    }
}

/// Reference interval for [`TagDecay::PerBlock`]: one 5% decay step per
/// 1/12 year (525,600 blocks).
pub const TIME_DECAY_REF_BLOCKS: u64 = PROD_BLOCKS_PER_YEAR / 12;

/// Eligible lifetime of a lottery ticket, `circulation_window - min_utxo_age
/// + 1` = 9,281 blocks.
pub fn ticket_life_blocks() -> u64 {
    let d = LotteryDrawConfig::default();
    d.circulation_window - d.min_utxo_age + 1
}

/// Parameters of one M2-production run.
#[derive(Clone, Debug)]
pub struct ProdParams {
    /// Blocks to simulate.
    pub horizon_blocks: u64,
    /// Block offsets (from `start_height`) at which to snapshot metrics.
    pub snapshots: Vec<u64>,
    /// Height of the first simulated block (0 = genesis).
    pub start_height: u64,
    /// Blocks per aggregation step (1 = exact per-block mode). Must divide
    /// [`PROD_BLOCKS_PER_YEAR`] so reward changes fall on step boundaries.
    pub step_blocks: u64,
    pub mining: MiningModel,
    pub whale: WhaleStrategy,
    pub fee_mode: FeeModelMode,
    /// `false` = counterfactual: no lottery share (miner keeps the whole
    /// reward), fees fully burned, demurrage rate 0.
    pub mechanism: bool,
    pub seed: u64,
    /// Multiplier on every cohort's transactions per year (1.0 = the M2
    /// rates). Velocity is unchanged, so each transaction moves a
    /// proportionally smaller fraction of the position.
    pub tx_scale: f64,
    /// Lottery ticket weighting (#1801). [`TicketRule::Uniform`] = #1790.
    pub ticket_rule: TicketRule,
    /// Honest-wallet tag decay (#1801). [`TagDecay::PerHop`] = #1790.
    pub tag_decay: TagDecay,
    /// `true` = spent outputs stay lottery candidates until they age out of
    /// the window, which is what the node does: ring inputs only record a
    /// key image, so `utxo_db` never drops the real input (`writer.rs`).
    /// `false` = #1790's rule (a spend removes its coins from the draw),
    /// which the node can only enforce by revealing which output was spent.
    pub spent_eligible: bool,
}

impl ProdParams {
    /// A genesis-start run of `years` with snapshots at each listed year.
    pub fn years(years: u64, snapshot_years: &[u64]) -> Self {
        Self {
            horizon_blocks: years * PROD_BLOCKS_PER_YEAR,
            snapshots: snapshot_years
                .iter()
                .map(|y| y * PROD_BLOCKS_PER_YEAR)
                .collect(),
            start_height: 0,
            step_blocks: 720,
            mining: MiningModel::WealthProportional,
            whale: WhaleStrategy::OptimalDecoy,
            fee_mode: FeeModelMode::ConsensusOnly,
            mechanism: true,
            seed: 1_696,
            tx_scale: 1.0,
            ticket_rule: TicketRule::Uniform,
            tag_decay: TagDecay::PerHop,
            spent_eligible: false,
        }
    }
}

/// Per-cohort totals at a snapshot.
#[derive(Clone, Debug)]
pub struct CohortSnapshot {
    pub name: &'static str,
    pub wealth_share: f64,
    /// Share of all lottery payouts won so far.
    pub lottery_share: f64,
    /// Demurrage paid so far, in BTH.
    pub demurrage_bth: f64,
    /// Mean priced (demurrage) factor of the cohort's current positions.
    pub mean_priced_factor: f64,
}

/// Metrics at one snapshot.
#[derive(Clone, Debug)]
pub struct ProdSnapshot {
    pub blocks: u64,
    pub gini: f64,
    /// Share of the top decile's value whose priced factor is below 1.5x.
    pub top_decile_below_1_5x: f64,
    pub cohorts: Vec<CohortSnapshot>,
    pub total_mined_bth: f64,
    pub lottery_paid_bth: f64,
    pub demurrage_bth: f64,
    pub fees_bth: f64,
    pub burned_bth: f64,
    /// Ticket prices paid so far ([`TicketRule::TicketPrice`]; burned and
    /// included in `burned_bth`), in BTH.
    pub ticket_burn_bth: f64,
    /// Part of `demurrage_bth` charged above the time-accrued demurrage: the
    /// #925 downgrade (capitalized reset) charge, in BTH.
    pub downgrade_bth: f64,
    /// Strategic whale's priced factor (FACTOR_SCALE) at this snapshot.
    pub whale_priced_factor: u64,
}

/// Result of one run.
#[derive(Clone, Debug)]
pub struct ProdReport {
    pub gini0: f64,
    pub snapshots: Vec<ProdSnapshot>,
    /// Coinbase outputs plus transaction and payout outputs created.
    pub outputs_created: u64,
    pub transactions: u64,
}

/// A cluster tag vector: (cluster index, weight in 0..=1), sorted by weight
/// descending, at most [`MAX_CLUSTER_TAGS`] entries, none below
/// [`MIN_TAG_WEIGHT`]. The unlisted remainder is background.
type Tags = Rc<Vec<(u32, f64)>>;

struct Agent {
    cohort: usize,
    hashpower: f64,
    velocity: f64,
    tx_per_year: f64,
    strategic: bool,
    /// Total position, pico.
    value: u128,
    /// Pre-merge tag mass: cluster -> pico of value attributed to it.
    mass: BTreeMap<u32, f64>,
    /// Creation height of the oldest coin in the position.
    oldest: u64,
    /// Coins (UTXOs) currently in the position.
    coins: u32,
    /// Union size that triggers the next tiny-mass prune.
    prune_at: usize,
    lottery_won: u128,
    demurrage_paid: u128,
    /// Spend generation: bumped on every spend, which consumes all coins.
    /// Outputs tagged with an older generation are spent.
    generation: u32,
}

/// The tags of a group of identical-tag outputs, or of `count` coinbase
/// outputs whose fresh clusters are `first..first + count`.
#[derive(Clone)]
enum OutTags {
    Shared(Tags),
    Coinbase { first: u32 },
}

struct WinEntry {
    owner: u32,
    /// Owner's spend generation when the outputs were created.
    generation: u32,
    count: u64,
    tags: OutTags,
    /// Value of each output in the group, pico (ticket weighting only).
    value: f64,
}

struct OwnerEntry {
    step: u64,
    count: u64,
    tags: OutTags,
    /// Value of each output, pico.
    value: f64,
    /// Per-value ticket tilt fixed when the output enters the window
    /// ([`TicketRule::InverseFactor`]; 1.0 otherwise).
    tilt: f64,
}

struct World {
    p: ProdParams,
    policy: MonetaryPolicy,
    fee_params: ConsensusFeeParams,
    curve: ClusterFactorCurve,
    draw: LotteryDrawConfig,
    agents: Vec<Agent>,
    names: Vec<&'static str>,
    /// Per-cluster cumulative wealth W, pico (monotone).
    w: Vec<f64>,
    /// Buckets of created outputs, one per step, from `bucket_base`.
    buckets: VecDeque<Vec<WinEntry>>,
    bucket_base: u64,
    owner_entries: Vec<VecDeque<OwnerEntry>>,
    owner_counts: Vec<u64>,
    lo: u64,
    hi: u64,
    total_mined: u128,
    carry: u128,
    lottery_paid: u128,
    demurrage: u128,
    fees: u128,
    burned: u128,
    outputs_created: u64,
    transactions: u64,
    /// Current ticket price, pico ([`TicketRule::TicketPrice`]).
    ticket_price: u128,
    /// Tickets bought since the last price update.
    tickets_bought: u64,
    /// Demurrage above the time-accrued charge (#925 downgrade part), pico.
    downgrade: u128,
    /// Ticket prices paid (burned), pico.
    ticket_burn: u128,
    rng_mine: ChaCha8Rng,
    rng_tx: ChaCha8Rng,
    rng_lot: ChaCha8Rng,
}

fn bth(pico: u128) -> f64 {
    pico as f64 / PICO_PER_BTH as f64
}

/// Gini of a set of non-negative values.
pub fn gini(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 2 {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = v.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let weighted: f64 = v
        .iter()
        .enumerate()
        .map(|(i, x)| (i as f64 + 1.0) * x)
        .sum();
    (2.0 * weighted - (n as f64 + 1.0) * total) / (n as f64 * total)
}

/// Prune below 0.1% and keep the 16 heaviest (ties by cluster index), as
/// `ClusterTagVector::from_pairs` does.
fn finish_tags(mut pairs: Vec<(u32, f64)>) -> Vec<(u32, f64)> {
    pairs.retain(|(_, w)| *w >= MIN_TAG_WEIGHT);
    pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    pairs.truncate(MAX_CLUSTER_TAGS);
    pairs
}

/// Knuth Poisson sampler (the per-step rates here are far below 1).
fn poisson(rng: &mut ChaCha8Rng, lambda: f64) -> u64 {
    if lambda <= 0.0 {
        return 0;
    }
    let l = (-lambda).exp();
    let mut k = 0u64;
    let mut p = 1.0;
    loop {
        p *= rng.random::<f64>();
        if p <= l {
            return k;
        }
        k += 1;
    }
}

/// Multinomial draw of `n` over `weights` by sequential binomials, or by
/// repeated categorical draws when `n` is small.
fn multinomial(rng: &mut ChaCha8Rng, n: u64, weights: &[f64]) -> Vec<u64> {
    let mut out = vec![0u64; weights.len()];
    let total: f64 = weights.iter().sum();
    if n == 0 || total <= 0.0 {
        return out;
    }
    if n <= 8 {
        for _ in 0..n {
            let mut r = rng.random::<f64>() * total;
            let mut pick = weights.len() - 1;
            for (i, w) in weights.iter().enumerate() {
                if *w <= 0.0 {
                    continue;
                }
                if r < *w {
                    pick = i;
                    break;
                }
                r -= w;
                pick = i;
            }
            out[pick] += 1;
        }
        return out;
    }
    let mut left = n;
    let mut rest = total;
    for (i, w) in weights.iter().enumerate() {
        if left == 0 {
            break;
        }
        if *w <= 0.0 {
            continue;
        }
        let p = (w / rest).clamp(0.0, 1.0);
        let k = if p >= 1.0 {
            left
        } else {
            Binomial::new(left, p).unwrap().sample(rng)
        };
        out[i] = k;
        left -= k;
        rest -= w;
        if rest <= 0.0 {
            out[i] += left;
            left = 0;
        }
    }
    if left > 0 {
        // Floating-point residue: give it to the last positive weight.
        if let Some(i) = weights.iter().rposition(|w| *w > 0.0) {
            out[i] += left;
        }
    }
    out
}

impl World {
    fn new(p: ProdParams) -> Self {
        assert!(p.step_blocks >= 1 && PROD_BLOCKS_PER_YEAR % p.step_blocks == 0);
        assert!(p.tx_scale > 0.0, "tx_scale must be positive");
        let policy = mainnet_monetary_policy();
        let mut fee_params = ConsensusFeeParams::from_policy(&policy, p.fee_mode);
        if !p.mechanism {
            // Demurrage never activates.
            fee_params.halving_interval = u64::MAX;
        }
        let draw = LotteryDrawConfig::default();
        let s = p.step_blocks;
        let lo = draw.min_utxo_age.div_ceil(s);
        let hi = draw.circulation_window / s;

        let mut agents = Vec::new();
        let mut names = Vec::new();
        let mut w = Vec::new();
        let cohorts = m2_population();
        let tx_rates = [52.0, 24.0, 365.0, 12.0];
        for (ci, c) in cohorts.iter().enumerate() {
            names.push(c.name);
            for _ in 0..c.count {
                agents.push((
                    ci,
                    c.holdings_bth,
                    c.velocity_per_year,
                    tx_rates[ci] * p.tx_scale,
                    false,
                ));
            }
        }
        names.push("strategic");
        agents.push((
            cohorts.len(),
            STRATEGIC_WHALE_BTH,
            0.2,
            12.0 * p.tx_scale,
            true,
        ));

        let n = agents.len() as f64;
        let total_bth: f64 = agents.iter().map(|a| a.1 as f64).sum();
        let agents: Vec<Agent> = agents
            .into_iter()
            .map(|(cohort, holdings, velocity, tx_per_year, strategic)| {
                let value = holdings * PICO_PER_BTH;
                let id = w.len() as u32;
                w.push(value as f64);
                let mut mass = BTreeMap::new();
                mass.insert(id, value as f64);
                Agent {
                    cohort,
                    hashpower: match p.mining {
                        MiningModel::WealthProportional => holdings as f64 / total_bth,
                        MiningModel::Egalitarian => 1.0 / n,
                    },
                    velocity,
                    tx_per_year,
                    strategic,
                    value,
                    mass,
                    oldest: p.start_height,
                    coins: 1,
                    prune_at: MASS_PRUNE_TRIGGER,
                    lottery_won: 0,
                    demurrage_paid: 0,
                    generation: 0,
                }
            })
            .collect();
        let na = agents.len();
        let seed = p.seed;
        let mut world = World {
            policy,
            fee_params,
            curve: ClusterFactorCurve::default_params(),
            draw,
            agents,
            names,
            w,
            buckets: VecDeque::new(),
            bucket_base: 0,
            owner_entries: (0..na).map(|_| VecDeque::new()).collect(),
            owner_counts: vec![0; na],
            lo,
            hi,
            total_mined: 0,
            carry: 0,
            lottery_paid: 0,
            demurrage: 0,
            fees: 0,
            burned: 0,
            outputs_created: 0,
            transactions: 0,
            ticket_price: 0,
            tickets_bought: 0,
            downgrade: 0,
            ticket_burn: 0,
            rng_mine: ChaCha8Rng::seed_from_u64(seed ^ 0x6d69_6e65),
            rng_tx: ChaCha8Rng::seed_from_u64(seed ^ 0x7478_7478),
            rng_lot: ChaCha8Rng::seed_from_u64(seed ^ 0x6c6f_7474),
            p,
        };
        // Genesis endowments are outputs created at the start height.
        world.buckets.push_back(Vec::new());
        for i in 0..na {
            let tags = OutTags::Shared(Rc::new(vec![(i as u32, 1.0)]));
            let value = world.agents[i].value as f64;
            world.buckets[0].push(WinEntry {
                owner: i as u32,
                generation: 0,
                count: 1,
                tags,
                value,
            });
        }
        world
    }

    fn reward_at(&self, height: u64) -> u64 {
        match self.policy.halving_reward(height) {
            Some(r) => r,
            None => self.policy.calculate_tail_reward_u128(self.total_mined),
        }
    }

    fn lottery_share(&self, height: u64, reward: u64) -> u64 {
        if self.p.mechanism {
            self.policy.lottery_emission_share(height, reward)
        } else {
            0
        }
    }

    fn eff_wealth(&self, tags: &[(u32, f64)]) -> f64 {
        tags.iter().map(|(c, wt)| wt * self.w[*c as usize]).sum()
    }

    fn factor(&self, wealth_pico: f64) -> u64 {
        self.curve.factor(wealth_pico.max(0.0) as u128)
    }

    /// The position's merged input tags (pruned/truncated, no decay).
    fn input_tags(&self, a: usize) -> Vec<(u32, f64)> {
        let ag = &self.agents[a];
        if ag.value == 0 {
            return Vec::new();
        }
        let v = ag.value as f64;
        finish_tags(ag.mass.iter().map(|(c, m)| (*c, m / v)).collect())
    }

    /// Value-weighted effective wealth of the inputs (every input carries its
    /// own tags; the mass map is their union, pruned only where provably
    /// below 0.1% of the final merge).
    fn input_eff_wealth(&self, a: usize) -> f64 {
        let ag = &self.agents[a];
        if ag.value == 0 {
            return 0.0;
        }
        ag.mass
            .iter()
            .map(|(c, m)| m * self.w[*c as usize])
            .sum::<f64>()
            / ag.value as f64
    }

    /// The factor a spend of agent `a`'s position is priced at: honest = the
    /// ring floor = inputs' own effective wealth; optimal whale = diluted
    /// centroid with the best decoys.
    fn priced_factor(&self, a: usize) -> u64 {
        if self.agents[a].strategic && self.p.whale == WhaleStrategy::OptimalDecoy {
            self.optimal_floor(a)
        } else {
            self.factor(self.input_eff_wealth(a))
        }
    }

    /// Ring-centroid floor with greedily chosen decoys from the other agents'
    /// positions: each of the `RING_SIZE - 1` slots takes the candidate that
    /// lowers the value-weighted centroid the most.
    fn optimal_floor(&self, a: usize) -> u64 {
        let v = self.agents[a].value;
        let real_w = self.input_eff_wealth(a);
        let mut members: Vec<(u64, u128)> = Vec::new();
        let chunk = v.min(u64::MAX as u128) as u64;
        members.push((chunk, real_w as u128));
        let mut cands: Vec<(u64, f64)> = (0..self.agents.len())
            .filter(|&j| j != a && self.agents[j].value > 0)
            .map(|j| {
                (
                    self.agents[j].value.min(u64::MAX as u128) as u64,
                    self.input_eff_wealth(j),
                )
            })
            .collect();
        let mut num = chunk as f64 * real_w;
        let mut den = chunk as f64;
        for _ in 0..RING_SIZE - 1 {
            let centroid = num / den;
            let mut best: Option<(usize, f64)> = None;
            for (i, (cv, cw)) in cands.iter().enumerate() {
                let c2 = (num + *cv as f64 * cw) / (den + *cv as f64);
                if c2 < centroid && best.is_none_or(|(_, b)| c2 < b) {
                    best = Some((i, c2));
                }
            }
            let Some((i, _)) = best else { break };
            let (cv, cw) = cands.swap_remove(i);
            num += cv as f64 * cw;
            den += cv as f64;
            members.push((cv, cw as u128));
        }
        ring_centroid_implied_factor(&members, &self.curve)
    }

    fn receive(&mut self, a: usize, value: u128, tags: &[(u32, f64)], height: u64) {
        let ag = &mut self.agents[a];
        let had = ag.value > 0;
        ag.value += value;
        for (c, wt) in tags {
            *ag.mass.entry(*c).or_insert(0.0) += value as f64 * wt;
        }
        if had {
            ag.oldest = ag.oldest.min(height);
        } else {
            ag.oldest = height;
        }
        ag.coins += 1;
        // Bound the union. Entries below 0.1% of the position never reach a
        // merged output tag (the position only grows until the next spend),
        // but they still count in the ring-centroid floor, because on the
        // node each input coin keeps its own tags. Only mass below
        // `MASS_PRUNE_FRACTION` of the position is dropped, and only when the
        // union is large.
        if ag.mass.len() > ag.prune_at {
            let v = ag.value as f64;
            ag.mass.retain(|_, m| *m / v >= MASS_PRUNE_FRACTION);
            // Amortise: next prune only after the union doubles again.
            ag.prune_at = (2 * ag.mass.len()).max(MASS_PRUNE_TRIGGER);
        }
    }

    fn credit_w(&mut self, tags: &[(u32, f64)], amount: u128) {
        for (c, wt) in tags {
            self.w[*c as usize] += amount as f64 * wt;
        }
    }

    /// Record `count` new outputs of `owner` (already credited to its
    /// position). `per_output` decides eligibility exactly as in #1790;
    /// `value_each` is each output's own value, used for ticket weights and
    /// the ticket price.
    fn add_output(
        &mut self,
        step: u64,
        owner: usize,
        count: u64,
        per_output: u128,
        value_each: u128,
        tags: OutTags,
    ) {
        self.outputs_created += count;
        if count == 0 || per_output < self.draw.min_utxo_value as u128 {
            return;
        }
        if self.p.ticket_rule == TicketRule::TicketPrice && self.p.mechanism {
            // An output is eligible only if it buys its ticket. The wallet
            // skips outputs worth no more than the price and, when a spent
            // ticket stops counting, outputs it expects to spend before the
            // ticket's window ends (median holding time below the window).
            let price = self.ticket_price;
            let median_hold = std::f64::consts::LN_2 * PROD_BLOCKS_PER_YEAR as f64
                / self.agents[owner].tx_per_year;
            let holds = self.p.spent_eligible || median_hold >= self.draw.circulation_window as f64;
            if !holds || value_each <= price {
                return;
            }
            self.charge(owner, price * count as u128);
            self.tickets_bought += count;
        }
        let idx = (step - self.bucket_base) as usize;
        while self.buckets.len() <= idx {
            self.buckets.push_back(Vec::new());
        }
        let generation = self.agents[owner].generation;
        self.buckets[idx].push(WinEntry {
            owner: owner as u32,
            generation,
            count,
            tags,
            value: value_each as f64,
        });
    }

    /// Burn `amount` from agent `a`'s position (ticket purchases), scaling
    /// its tag mass so mass and value stay consistent.
    fn charge(&mut self, a: usize, amount: u128) {
        let ag = &mut self.agents[a];
        let amount = amount.min(ag.value);
        if amount == 0 {
            return;
        }
        let keep = (ag.value - amount) as f64 / ag.value as f64;
        ag.value -= amount;
        for m in ag.mass.values_mut() {
            *m *= keep;
        }
        self.ticket_burn += amount;
        self.burned += amount;
    }

    /// Per-value ticket tilt of an output entering the window.
    fn entry_tilt(&self, tags: &OutTags, count: u64) -> f64 {
        if self.p.ticket_rule != TicketRule::InverseFactor {
            return 1.0;
        }
        let tilt = |f: u64| {
            let f = f.clamp(
                ClusterFactorCurve::FACTOR_SCALE,
                crate::lottery::MAX_FACTOR_SCALED,
            );
            (crate::lottery::MAX_FACTOR_SCALED - f + ClusterFactorCurve::FACTOR_SCALE) as f64
                / ClusterFactorCurve::FACTOR_SCALE as f64
        };
        match tags {
            OutTags::Shared(t) => tilt(self.factor(self.eff_wealth(t))),
            OutTags::Coinbase { first } => {
                let n = count.max(1);
                (0..n)
                    .map(|k| tilt(self.factor(self.w[(*first + k as u32) as usize])))
                    .sum::<f64>()
                    / n as f64
            }
        }
    }

    /// Ticket weight of one window entry at `step`.
    fn entry_weight(&self, e: &OwnerEntry, step: u64) -> f64 {
        match self.p.ticket_rule {
            TicketRule::Uniform | TicketRule::TicketPrice => e.count as f64,
            TicketRule::InverseFactor => e.count as f64 * e.value * e.tilt,
            TicketRule::ValueAge => {
                let age = (step - e.step) * self.p.step_blocks;
                e.count as f64 * e.value * age as f64
            }
        }
    }

    /// Move bucket `step - lo` into the eligible window and expire bucket
    /// `step - hi - 1`.
    fn advance_window(&mut self, step: u64) {
        if step >= self.lo {
            let enter = step - self.lo;
            if enter >= self.bucket_base {
                let idx = (enter - self.bucket_base) as usize;
                if idx < self.buckets.len() {
                    let entries = std::mem::take(&mut self.buckets[idx]);
                    for e in entries {
                        let o = e.owner as usize;
                        if e.generation != self.agents[o].generation {
                            // Spent before it matured.
                            continue;
                        }
                        let tilt = self.entry_tilt(&e.tags, e.count);
                        self.owner_counts[o] += e.count;
                        self.owner_entries[o].push_back(OwnerEntry {
                            step: enter,
                            count: e.count,
                            tags: e.tags,
                            value: e.value,
                            tilt,
                        });
                    }
                }
            }
        }
        // Drop buckets that have been moved into the window.
        while self.bucket_base + self.lo <= step && !self.buckets.is_empty() {
            self.buckets.pop_front();
            self.bucket_base += 1;
        }
        if step > self.hi {
            let expire_before = step - self.hi;
            for o in 0..self.owner_entries.len() {
                while let Some(front) = self.owner_entries[o].front() {
                    if front.step >= expire_before {
                        break;
                    }
                    self.owner_counts[o] -= front.count;
                    self.owner_entries[o].pop_front();
                }
            }
        }
    }

    fn mine(&mut self, step: u64, h0: u64) {
        let s = self.p.step_blocks;
        let reward = self.reward_at(h0);
        let share = self.lottery_share(h0, reward);
        let miner_payout = (reward - share) as u128;
        let weights: Vec<f64> = self.agents.iter().map(|a| a.hashpower).collect();
        let blocks = multinomial(&mut self.rng_mine, s, &weights);
        for (i, m) in blocks.into_iter().enumerate() {
            if m == 0 {
                continue;
            }
            // Each coinbase output starts its own fresh cluster with W = the
            // output amount (block.rs `to_tx_output`, writer.rs).
            let first = self.w.len() as u32;
            for k in 0..m as u32 {
                self.w.push(miner_payout as f64);
                self.receive(i, miner_payout, &[(first + k, 1.0)], h0);
            }
            self.add_output(
                step,
                i,
                m,
                miner_payout,
                miner_payout,
                OutTags::Coinbase { first },
            );
        }
    }

    /// Consensus fee (and its demurrage part) for spending `v` pico, chunked
    /// into transactions of at most [`TX_CHUNK_PICO`]. Returns (fee,
    /// demurrage, transactions, downgrade part): the downgrade part is the
    /// demurrage above the time-accrued charge, i.e. the #925 capitalized
    /// reset charge when it is the larger of the two (diagnostic only).
    fn price(
        &self,
        v: u128,
        claimed: u64,
        floor: u64,
        elapsed: u64,
        height: u64,
        n_in: usize,
    ) -> (u128, u128, u64, u128) {
        let (mut fee, mut dem, mut n_tx, mut reset) = (0u128, 0u128, 0u64, 0u128);
        let mut left = v;
        while left > 0 {
            let chunk = left.min(TX_CHUNK_PICO);
            left -= chunk;
            n_tx += 1;
            let b = consensus_fee_breakdown(
                &ConsensusFeeInputs {
                    size: consensus_tx_size_bytes(if n_tx == 1 { n_in } else { 1 }, 2, 0),
                    n_out: 2,
                    n_memo: 0,
                    claimed: ClaimedClass::Factor(claimed),
                    floor,
                    elapsed,
                    height,
                    output_sum: chunk as u64,
                },
                &self.fee_params,
            );
            fee += b.fee as u128;
            dem += b.demurrage as u128;
            let accrued = demurrage_charge(
                chunk as u64,
                b.demurrage_factor,
                elapsed,
                self.fee_params.demurrage_rate_bps(height),
                self.fee_params.blocks_per_year,
            );
            reset += b.demurrage.saturating_sub(accrued) as u128;
        }
        (fee, dem, n_tx, reset)
    }

    /// One spend by agent `a` at `height`: consolidate, price, pay.
    fn spend(&mut self, step: u64, a: usize, height: u64) -> u128 {
        let v = self.agents[a].value;
        if v == 0 {
            return 0;
        }
        let strategic_opt = self.agents[a].strategic && self.p.whale == WhaleStrategy::OptimalDecoy;
        let in_tags = self.input_tags(a);
        let elapsed = height.saturating_sub(self.agents[a].oldest);
        let n_in = (self.agents[a].coins as usize).clamp(1, 16);
        let (out_tags, fee, dem, n_tx, reset) = if strategic_opt {
            // Cheapest of: (a) claim the merged class undecayed (in-class,
            // accrued only) or (b) claim background and pay the downgrade
            // charge from the decoy-diluted floor. Decoys only ever lower the
            // centroid, so both use the floor-minimising ring.
            let decoy_floor = self.optimal_floor(a);
            let claimed_a = self.factor(self.eff_wealth(&in_tags));
            let pa = self.price(
                v,
                claimed_a,
                decoy_floor.max(claimed_a),
                elapsed,
                height,
                n_in,
            );
            let claimed_b = ClusterFactorCurve::FACTOR_SCALE;
            let pb = self.price(
                v,
                claimed_b,
                decoy_floor.max(claimed_b),
                elapsed,
                height,
                n_in,
            );
            if pb.0 <= pa.0 {
                (Vec::new(), pb.0, pb.1, pb.2, pb.3)
            } else {
                (in_tags, pa.0, pa.1, pa.2, pa.3)
            }
        } else {
            let keep = match self.p.tag_decay {
                TagDecay::PerHop => 1.0 - STOCK_WALLET_DECAY,
                // Keyed to time since the position's last spend, not to the
                // number of hops (the same 5% per reference interval).
                TagDecay::PerBlock => {
                    (1.0 - STOCK_WALLET_DECAY).powf(elapsed as f64 / TIME_DECAY_REF_BLOCKS as f64)
                }
            };
            let out = finish_tags(in_tags.iter().map(|(c, wt)| (*c, wt * keep)).collect());
            let claimed = self.factor(self.eff_wealth(&out));
            // Honest decoys are negligible in value: the ring floor is the
            // inputs' own effective wealth.
            let floor = self.factor(self.input_eff_wealth(a)).max(claimed);
            let (f, d, n, r) = self.price(v, claimed, floor, elapsed, height, n_in);
            (out, f, d, n, r)
        };
        let fee = fee.min(v);
        let net = v - fee;
        let frac = (self.agents[a].velocity / self.agents[a].tx_per_year).min(1.0);
        let pay = ((net as f64) * frac) as u128;
        let change = net - pay;

        // Recipient in proportion to spending flow (velocity x value), so
        // every agent's expected inflow equals its expected outflow and
        // payments by themselves move no wealth between agents.
        let weights: Vec<f64> = self
            .agents
            .iter()
            .enumerate()
            .map(|(j, x)| {
                if j == a {
                    0.0
                } else {
                    x.value as f64 * x.velocity
                }
            })
            .collect();
        let r = multinomial(&mut self.rng_tx, 1, &weights)
            .iter()
            .position(|k| *k > 0)
            .unwrap_or((a + 1) % self.agents.len());

        let ag = &mut self.agents[a];
        ag.value = 0;
        ag.mass.clear();
        ag.coins = 0;
        ag.prune_at = MASS_PRUNE_TRIGGER;
        // #1790 rule: the spend consumes every coin, and none of them can
        // win the lottery any more. Eligible ones leave the window now;
        // pending ones are skipped when their bucket matures because their
        // generation is stale. With `spent_eligible` (what the node does:
        // a ring spend only records a key image) the spent coins stay
        // candidates until they age out, and a win still pays their owner.
        if !self.p.spent_eligible {
            ag.generation = ag.generation.wrapping_add(1);
            self.owner_counts[a] = 0;
            self.owner_entries[a].clear();
        }
        let ag = &mut self.agents[a];
        ag.demurrage_paid += dem;
        self.demurrage += dem;
        self.downgrade += reset;
        self.transactions += n_tx;

        self.credit_w(&out_tags, net);
        let tags: Tags = Rc::new(out_tags);
        self.receive(a, change, &tags, height);
        self.agents[a].oldest = height;
        self.receive(r, pay, &tags, height);
        let per = (net / (2 * n_tx as u128)).max(1);
        let n = n_tx as u128;
        self.add_output(
            step,
            a,
            n_tx,
            per,
            change / n,
            OutTags::Shared(tags.clone()),
        );
        self.add_output(step, r, n_tx, per, pay / n, OutTags::Shared(tags));
        fee
    }

    fn lottery(&mut self, step: u64, h0: u64, step_fees: u128) {
        let s = self.p.step_blocks;
        let rho_total: u64 = self.owner_counts.iter().sum();
        let rho = rho_total.min(MAX_LOTTERY_CANDIDATES);
        let mut payout_total: u128 = 0;
        let mut slots: u64 = 0;
        let per_block_fee = step_fees / s as u128;
        for j in 0..s {
            let h = h0 + j;
            let reward = self.reward_at(h);
            let share = self.lottery_share(h, reward) as u128;
            let fees = if j == s - 1 {
                step_fees - per_block_fee * (s as u128 - 1)
            } else {
                per_block_fee
            };
            self.total_mined += reward as u128;
            if !self.p.mechanism {
                self.burned += fees;
                continue;
            }
            let pool = fees * POOL_FRACTION_PERMILLE / 1000;
            self.burned += fees - pool;
            let available = self.carry + share + pool;
            let cap = (rho as u128 * LOTTERY_BASE_FEE_PICO as u128).min(reward as u128);
            let payout = if rho > 0 { available.min(cap) } else { 0 };
            self.carry = available - payout;
            if payout > 0 {
                payout_total += payout;
                slots += (self.draw.winners_per_draw as u64).min(rho);
            }
        }
        if self.p.ticket_rule == TicketRule::TicketPrice {
            // Next step's price: the average payout per ticket, so tickets
            // bought and payouts balance in aggregate (p = R / T).
            if self.tickets_bought > 0 {
                self.ticket_price = payout_total / self.tickets_bought as u128;
            }
            self.tickets_bought = 0;
        }
        if slots == 0 || payout_total == 0 {
            return;
        }
        let per_slot = payout_total / slots as u128;
        let dust = payout_total - per_slot * slots as u128;
        self.carry += dust;
        self.lottery_paid += per_slot * slots as u128;

        let uniform = matches!(
            self.p.ticket_rule,
            TicketRule::Uniform | TicketRule::TicketPrice
        );
        let weights: Vec<f64> = if uniform {
            self.owner_counts.iter().map(|c| *c as f64).collect()
        } else {
            self.owner_entries
                .iter()
                .map(|es| es.iter().map(|e| self.entry_weight(e, step)).sum())
                .collect()
        };
        let wins = multinomial(&mut self.rng_lot, slots, &weights);
        for (o, k) in wins.into_iter().enumerate() {
            if k == 0 {
                continue;
            }
            // Spread the owner's k wins over its eligible outputs by ticket
            // weight (by count under the uniform rule).
            let mut picks: BTreeMap<usize, u64> = BTreeMap::new();
            let ew: Vec<f64> = self.owner_entries[o]
                .iter()
                .map(|e| self.entry_weight(e, step))
                .collect();
            for (i, n) in multinomial(&mut self.rng_lot, k, &ew)
                .into_iter()
                .enumerate()
            {
                if n > 0 {
                    picks.insert(i, n);
                }
            }
            for (i, n) in picks {
                let entry_count = self.owner_entries[o][i].count;
                match self.owner_entries[o][i].tags.clone() {
                    OutTags::Shared(tags) => {
                        let value = per_slot * n as u128;
                        self.credit_w(&tags, value);
                        self.receive(o, value, &tags, h0);
                        self.agents[o].lottery_won += value;
                        self.add_output(step, o, n, per_slot, per_slot, OutTags::Shared(tags));
                    }
                    OutTags::Coinbase { first } => {
                        // Each win copies the tags of one coinbase output.
                        for _ in 0..n {
                            let c = first + self.rng_lot.random_range(0..entry_count) as u32;
                            let tags: Tags = Rc::new(vec![(c, 1.0)]);
                            self.credit_w(&tags, per_slot);
                            self.receive(o, per_slot, &tags, h0);
                            self.agents[o].lottery_won += per_slot;
                            self.add_output(step, o, 1, per_slot, per_slot, OutTags::Shared(tags));
                        }
                    }
                }
            }
        }
    }

    fn snapshot(&self, blocks: u64) -> ProdSnapshot {
        let values: Vec<f64> = self.agents.iter().map(|a| a.value as f64).collect();
        let mut order: Vec<usize> = (0..self.agents.len()).collect();
        order.sort_by(|&x, &y| values[y].partial_cmp(&values[x]).unwrap().then(x.cmp(&y)));
        let top = (self.agents.len() / 10).max(1);
        let (mut below, mut tot) = (0.0, 0.0);
        for &i in order.iter().take(top) {
            tot += values[i];
            if self.priced_factor(i) < PRICED_FACTOR_THRESHOLD {
                below += values[i];
            }
        }
        let total: f64 = values.iter().sum();
        let lot_total: f64 = self.agents.iter().map(|a| a.lottery_won as f64).sum();
        let cohorts = (0..self.names.len())
            .map(|ci| {
                let members: Vec<usize> = (0..self.agents.len())
                    .filter(|&i| self.agents[i].cohort == ci)
                    .collect();
                let wsum: f64 = members.iter().map(|&i| values[i]).sum();
                let lsum: f64 = members
                    .iter()
                    .map(|&i| self.agents[i].lottery_won as f64)
                    .sum();
                let dsum: u128 = members.iter().map(|&i| self.agents[i].demurrage_paid).sum();
                let fsum: f64 = members
                    .iter()
                    .map(|&i| self.priced_factor(i) as f64 / 1000.0)
                    .sum();
                CohortSnapshot {
                    name: self.names[ci],
                    wealth_share: if total > 0.0 { wsum / total } else { 0.0 },
                    lottery_share: if lot_total > 0.0 {
                        lsum / lot_total
                    } else {
                        0.0
                    },
                    demurrage_bth: bth(dsum),
                    mean_priced_factor: fsum / members.len().max(1) as f64,
                }
            })
            .collect();
        let whale = self.agents.iter().position(|a| a.strategic).unwrap();
        ProdSnapshot {
            blocks,
            gini: gini(&values),
            top_decile_below_1_5x: if tot > 0.0 { below / tot } else { 0.0 },
            cohorts,
            total_mined_bth: bth(self.total_mined),
            lottery_paid_bth: bth(self.lottery_paid),
            demurrage_bth: bth(self.demurrage),
            fees_bth: bth(self.fees),
            burned_bth: bth(self.burned),
            ticket_burn_bth: bth(self.ticket_burn),
            downgrade_bth: bth(self.downgrade),
            whale_priced_factor: self.priced_factor(whale),
        }
    }
}

/// Run one M2-production simulation. Deterministic for a fixed seed.
pub fn run_m2_production(params: &ProdParams) -> ProdReport {
    let mut world = World::new(params.clone());
    let values: Vec<f64> = world.agents.iter().map(|a| a.value as f64).collect();
    let gini0 = gini(&values);
    let s = params.step_blocks;
    let steps = params.horizon_blocks.div_ceil(s);
    let mut snaps: Vec<u64> = params.snapshots.clone();
    snaps.sort_unstable();
    let mut snapshots = Vec::new();
    let mut next = 0;
    let n = world.agents.len();
    for step in 0..steps {
        let h0 = params.start_height + step * s;
        world.advance_window(step);
        world.mine(step, h0);
        let mut step_fees: u128 = 0;
        for a in 0..n {
            let lambda = world.agents[a].tx_per_year * s as f64 / PROD_BLOCKS_PER_YEAR as f64;
            let k = poisson(&mut world.rng_tx, lambda);
            for _ in 0..k {
                step_fees += world.spend(step, a, h0);
            }
        }
        world.fees += step_fees;
        world.lottery(step, h0, step_fees);
        let done = (step + 1) * s;
        while next < snaps.len() && snaps[next] <= done {
            snapshots.push(world.snapshot(snaps[next]));
            next += 1;
        }
    }
    ProdReport {
        gini0,
        snapshots,
        outputs_created: world.outputs_created,
        transactions: world.transactions,
    }
}

/// A mechanism-on run and its mechanism-off counterfactual (same seed, same
/// mining and transaction RNG streams).
pub struct ProdPair {
    pub on: ProdReport,
    pub off: ProdReport,
}

impl ProdPair {
    /// Legacy M2 definition: `gini0 - gini_f` with the mechanism on.
    pub fn delta_gini_raw(&self, i: usize) -> f64 {
        self.on.gini0 - self.on.snapshots[i].gini
    }
    /// The mechanism's own effect: `gini_f(off) - gini_f(on)`.
    pub fn delta_gini_mechanism(&self, i: usize) -> f64 {
        self.off.snapshots[i].gini - self.on.snapshots[i].gini
    }
}

/// Run the mechanism-on and mechanism-off variants of `params`.
pub fn run_m2_production_pair(params: &ProdParams) -> ProdPair {
    let on = run_m2_production(&ProdParams {
        mechanism: true,
        ..params.clone()
    });
    let off = run_m2_production(&ProdParams {
        mechanism: false,
        ..params.clone()
    });
    ProdPair { on, off }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(step: u64, mechanism: bool) -> ProdParams {
        ProdParams {
            horizon_blocks: 14_400,
            snapshots: vec![14_400],
            start_height: PROD_BLOCKS_PER_YEAR,
            step_blocks: step,
            mining: MiningModel::WealthProportional,
            whale: WhaleStrategy::OptimalDecoy,
            fee_mode: FeeModelMode::ConsensusOnly,
            mechanism,
            seed: 7,
            tx_scale: 1.0,
            ticket_rule: TicketRule::Uniform,
            tag_decay: TagDecay::PerHop,
            spent_eligible: false,
        }
    }

    /// Drift guards against the node and transaction-type constants.
    #[test]
    fn constants_match_node() {
        assert_eq!(PROD_BLOCKS_PER_YEAR, 365 * 24 * 3600 / 5);
        assert_eq!(PROD_BLOCKS_PER_YEAR, 6_307_200);
        assert_eq!(MAX_CLUSTER_TAGS, bth_transaction_types::MAX_CLUSTER_TAGS);
        assert_eq!(
            MIN_TAG_WEIGHT,
            bth_transaction_types::MIN_STORED_WEIGHT as f64
                / bth_transaction_types::TAG_WEIGHT_SCALE as f64
        );
        assert_eq!(RING_SIZE, bth_transaction_types::constants::RING_SIZE);
        let d = LotteryDrawConfig::default();
        assert_eq!(d.min_utxo_age, 720);
        assert_eq!(d.circulation_window, 10_000);
        assert_eq!(d.winners_per_draw, 4);
        let policy = mainnet_monetary_policy();
        assert_eq!(policy.halving_interval, PROD_BLOCKS_PER_YEAR);
        assert_eq!(policy.target_blocks_per_year(), PROD_BLOCKS_PER_YEAR);
    }

    /// The emission ramp the sim uses is the real one: 0 in epoch 0, +10% per
    /// epoch, capped at 50%, with demurrage off in epoch 0.
    #[test]
    fn emission_ramp_and_epoch0() {
        let w = World::new(short(720, true));
        let y = PROD_BLOCKS_PER_YEAR;
        let r0 = w.reward_at(0);
        assert_eq!(r0, 50 * PICO_PER_BTH as u64);
        assert_eq!(w.lottery_share(0, r0), 0);
        assert_eq!(w.lottery_share(y, w.reward_at(y)), w.reward_at(y) / 10);
        assert_eq!(w.reward_at(y), r0 / 2);
        assert_eq!(
            w.lottery_share(3 * y, w.reward_at(3 * y)),
            w.reward_at(3 * y) * 3 / 10
        );
        assert_eq!(w.policy.lottery_emission_bps(9 * y), 5_000);
        assert_eq!(w.fee_params.demurrage_rate_bps(y - 1), 0);
        assert_eq!(w.fee_params.demurrage_rate_bps(y), 200);
        let off = World::new(short(720, false));
        assert_eq!(off.lottery_share(3 * y, off.reward_at(3 * y)), 0);
        assert_eq!(off.fee_params.demurrage_rate_bps(3 * y), 0);
    }

    /// Lottery payouts are new outputs: they add to the winner's value and to
    /// the output count, never edit an existing coin in place, and the pool is
    /// conserved (paid + carried = emission share + 80% of fees).
    #[test]
    fn payouts_are_new_outputs_and_conserved() {
        let p = short(720, true);
        let mut w = World::new(p.clone());
        let before: u128 = w.agents.iter().map(|a| a.value).sum();
        let coins_before: u32 = w.agents.iter().map(|a| a.coins).sum();
        let mut share_total: u128 = 0;
        let mut pool_total: u128 = 0;
        let mut mined_to_miners: u128 = 0;
        for step in 0..20 {
            let h0 = p.start_height + step * p.step_blocks;
            w.advance_window(step);
            let reward = w.reward_at(h0) as u128;
            let share = w.lottery_share(h0, reward as u64) as u128;
            share_total += share * p.step_blocks as u128;
            mined_to_miners += (reward - share) * p.step_blocks as u128;
            w.mine(step, h0);
            let mut fees = 0;
            for a in 0..w.agents.len() {
                if step % 5 == 0 && a % 7 == 0 {
                    fees += w.spend(step, a, h0);
                }
            }
            // Per-block 80/20 split, exactly as `lottery()` and the node do.
            let per_block = fees / p.step_blocks as u128;
            for j in 0..p.step_blocks {
                let f = if j == p.step_blocks - 1 {
                    fees - per_block * (p.step_blocks as u128 - 1)
                } else {
                    per_block
                };
                pool_total += f * POOL_FRACTION_PERMILLE / 1000;
            }
            w.fees += fees;
            w.lottery(step, h0, fees);
        }
        assert!(w.lottery_paid > 0, "lottery paid out in epoch 1");
        assert_eq!(w.lottery_paid + w.carry, share_total + pool_total);
        let after: u128 = w.agents.iter().map(|a| a.value).sum();
        assert_eq!(after, before + mined_to_miners + w.lottery_paid - w.fees);
        let coins_after: u32 = w.agents.iter().map(|a| a.coins).sum();
        assert!(coins_after > coins_before);
        let won: u128 = w.agents.iter().map(|a| a.lottery_won).sum();
        assert_eq!(won, w.lottery_paid);
    }

    /// W only grows, coinbases start fresh clusters at the miner payout, and
    /// spend tags respect the 16-tag cap and the 0.1% prune.
    #[test]
    fn cluster_rules() {
        let p = short(720, true);
        let mut w = World::new(p.clone());
        let n0 = w.w.len();
        w.advance_window(0);
        w.mine(0, p.start_height);
        let payout = (w.reward_at(p.start_height)
            - w.lottery_share(p.start_height, w.reward_at(p.start_height)))
            as f64;
        assert!(w.w.len() > n0);
        assert!(w.w[n0..].iter().all(|x| *x == payout));
        assert!(payout <= 50.0 * PICO_PER_BTH as f64);
        let snapshot_w = w.w.clone();
        for a in 0..w.agents.len() {
            w.spend(0, a, p.start_height + 10);
        }
        for (old, new) in snapshot_w.iter().zip(&w.w) {
            assert!(new >= old, "W must be monotone");
        }
        let many: Vec<(u32, f64)> = (0..40).map(|i| (i, 1.0 / 40.0)).collect();
        let t = finish_tags(many);
        assert_eq!(t.len(), 16);
        let tiny: Vec<(u32, f64)> = (0..2000).map(|i| (i, 1.0 / 2000.0)).collect();
        assert!(finish_tags(tiny).is_empty(), "0.05% weights are pruned");
    }

    /// Spent outputs cannot win: a full-consolidation spend drops the
    /// agent's eligible count to zero (eligible coins and the coinbases still
    /// maturing in the pending bucket), no lottery slot goes to it while it
    /// holds nothing eligible, and its count comes back only from outputs
    /// created by or after the spend.
    #[test]
    fn spent_outputs_leave_the_lottery() {
        let p = ProdParams {
            mining: MiningModel::WealthProportional,
            ..short(720, true)
        };
        let mut w = World::new(p.clone());
        let whale = w.agents.iter().position(|a| a.strategic).unwrap();
        let h1 = p.start_height + p.step_blocks;
        // Step 0: genesis outputs are pending. Step 1: they mature.
        w.advance_window(0);
        w.advance_window(1);
        assert_eq!(w.owner_counts[whale], 1, "genesis output is eligible");
        // Coinbases mined in step 1 are pending (eligible from step 2).
        w.mine(1, h1);
        let pending_cb: u64 = w.buckets[(1 - w.bucket_base) as usize]
            .iter()
            .filter(|e| e.owner as usize == whale)
            .map(|e| e.count)
            .sum();
        assert!(
            pending_cb > 0,
            "the 21%-hashpower whale mined in 720 blocks"
        );

        let tx_before = w.transactions;
        w.spend(1, whale, h1);
        let n_tx = w.transactions - tx_before;
        assert_eq!(w.owner_counts[whale], 0, "spent coins left the window");
        assert!(w.owner_entries[whale].is_empty());

        // Many draws in epoch 1 (thousands of slots, others eligible): the
        // whale wins nothing while it holds no unspent eligible output.
        let won_before = w.agents[whale].lottery_won;
        for _ in 0..5 {
            w.lottery(1, h1, 0);
        }
        assert!(w.lottery_paid > 0);
        assert_eq!(w.agents[whale].lottery_won, won_before, "spent outputs won");

        // Step 2: the step-1 bucket matures. Only the spend's own change
        // outputs come back; the pre-spend coinbases were consumed.
        w.advance_window(2);
        assert_eq!(w.owner_counts[whale], n_tx);
        assert!(w.owner_entries[whale].iter().all(|e| e.step >= 1));
    }

    /// The optimal-decoy whale's floor is never above the honest floor and
    /// its outputs are background.
    #[test]
    fn optimal_decoy_dilutes_floor() {
        let w = World::new(short(720, true));
        let whale = w.agents.iter().position(|a| a.strategic).unwrap();
        let honest = w.factor(w.input_eff_wealth(whale));
        let opt = w.optimal_floor(whale);
        assert!(opt <= honest, "opt {opt} <= honest {honest}");
        assert!(honest > 5_000);
    }

    /// #1801: the baseline (uniform tickets, per-hop decay, #1790 candidate
    /// rule) is bit-for-bit the #1790 harness. The goldens were captured from
    /// `m2_production.rs` at 5bb7969e, before the redesign switches existed:
    /// 30 days from the start of epoch 1, seed 1696, honest whale, mechanism
    /// on and off, both mining models, 1x and 100x density.
    #[test]
    fn baseline_reproduces_1790_bit_for_bit() {
        // (mining, tx_scale, mechanism, gini bits, outputs, txs, lottery
        // BTH bits, demurrage BTH bits)
        type Golden = (MiningModel, f64, bool, u64, u64, u64, u64, u64);
        let goldens: [Golden; 8] = [
            (
                MiningModel::WealthProportional,
                1.0,
                true,
                0x3fedcfcf94f35788,
                2_591_292,
                1_086,
                4698378751907483686,
                4670662219687873389,
            ),
            (
                MiningModel::WealthProportional,
                1.0,
                false,
                0x3fedcfc892c81767,
                520_572,
                1_086,
                0,
                0,
            ),
            (
                MiningModel::WealthProportional,
                100.0,
                true,
                0x3fedc948f3731c60,
                2_792_294,
                101_587,
                4702284146422142341,
                4698367302383781520,
            ),
            (
                MiningModel::WealthProportional,
                100.0,
                false,
                0x3fedc46e45ad7916,
                721_574,
                101_587,
                0,
                0,
            ),
            (
                MiningModel::Egalitarian,
                1.0,
                true,
                0x3fe37befffc100bb,
                2_591_292,
                1_086,
                4698421317636198927,
                4674067477979002697,
            ),
            (
                MiningModel::Egalitarian,
                1.0,
                false,
                0x3fe36c204c776a50,
                520_572,
                1_086,
                0,
                0,
            ),
            (
                MiningModel::Egalitarian,
                100.0,
                true,
                0x3fe456c2a719ce56,
                2_792_294,
                101_587,
                4702626505183157684,
                4699223199281203697,
            ),
            (
                MiningModel::Egalitarian,
                100.0,
                false,
                0x3fe33276164b2b0e,
                721_574,
                101_587,
                0,
                0,
            ),
        ];
        for (mining, tx_scale, mechanism, gini, outputs, txs, lottery, demurrage) in goldens {
            let p = ProdParams {
                horizon_blocks: 30 * 17_280,
                snapshots: vec![30 * 17_280],
                start_height: PROD_BLOCKS_PER_YEAR,
                mining,
                whale: WhaleStrategy::Honest,
                mechanism,
                tx_scale,
                ..ProdParams::years(0, &[])
            };
            assert_eq!(p.ticket_rule, TicketRule::Uniform);
            assert_eq!(p.tag_decay, TagDecay::PerHop);
            assert!(!p.spent_eligible);
            let r = run_m2_production(&p);
            let s = &r.snapshots[0];
            let tag = format!("{mining:?} x{tx_scale} mechanism={mechanism}");
            assert_eq!(s.gini.to_bits(), gini, "{tag}: gini");
            assert_eq!(r.outputs_created, outputs, "{tag}: outputs");
            assert_eq!(r.transactions, txs, "{tag}: transactions");
            assert_eq!(s.lottery_paid_bth.to_bits(), lottery, "{tag}: lottery paid");
            assert_eq!(s.demurrage_bth.to_bits(), demurrage, "{tag}: demurrage");
            assert_eq!(s.ticket_burn_bth, 0.0);
        }
    }

    /// #1801: the full published #1790 numbers (seed 1696, consensus fees,
    /// 20 years). Slow (about 4 minutes per pair), so ignored by default:
    /// `cargo test --release -p bth-cluster-tax --features cli --lib
    /// published_1790 -- --ignored`.
    #[test]
    #[ignore]
    fn published_1790_numbers_reproduce() {
        // (mining, gini on 10y, gini off 10y, gini on 20y, gini off 20y),
        // from m2-production-verdicts.md "Main runs", honest whale.
        let rows = [
            (
                MiningModel::WealthProportional,
                0.9371,
                0.9369,
                0.9396,
                0.9386,
            ),
            (MiningModel::Egalitarian, 0.3334, 0.3234, 0.4359, 0.3890),
        ];
        for (mining, on10, off10, on20, off20) in rows {
            let mut p = ProdParams::years(20, &[10, 20]);
            p.mining = mining;
            p.whale = WhaleStrategy::Honest;
            let pair = run_m2_production_pair(&p);
            let r4 = |x: f64| (x * 1e4).round() / 1e4;
            assert_eq!(r4(pair.on.snapshots[0].gini), on10, "{mining:?} on 10y");
            assert_eq!(r4(pair.off.snapshots[0].gini), off10, "{mining:?} off 10y");
            assert_eq!(r4(pair.on.snapshots[1].gini), on20, "{mining:?} on 20y");
            assert_eq!(r4(pair.off.snapshots[1].gini), off20, "{mining:?} off 20y");
        }
    }

    /// Fixed seed reproduces the run bit for bit.
    #[test]
    fn deterministic_for_fixed_seed() {
        let a = run_m2_production(&short(720, true));
        let b = run_m2_production(&short(720, true));
        assert_eq!(a.snapshots[0].gini.to_bits(), b.snapshots[0].gini.to_bits());
        assert_eq!(a.outputs_created, b.outputs_created);
    }

    /// The exact per-block mode and the aggregated mode agree on a short
    /// horizon (loose bound; the CLI's `--validate` reports the full table).
    #[test]
    fn aggregation_tracks_exact_mode() {
        let exact = run_m2_production(&short(1, true));
        let agg = run_m2_production(&short(720, true));
        let (e, g) = (&exact.snapshots[0], &agg.snapshots[0]);
        assert!((e.gini - g.gini).abs() < 0.01, "{} vs {}", e.gini, g.gini);
        let rel = (e.lottery_paid_bth - g.lottery_paid_bth).abs() / e.lottery_paid_bth;
        assert!(
            rel < 0.02,
            "lottery paid {} vs {}",
            e.lottery_paid_bth,
            g.lottery_paid_bth
        );
    }
}
