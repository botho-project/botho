//! Emission-schedule sweep for the monetary-policy decision (issue #350).
//!
//! This module produces the **data** needed to choose Botho's permanent
//! emission schedule. It does NOT pick a winner: it runs a fixed grid of
//! candidate [`MonetaryPolicy`] schedules through the agent-based simulator
//! plus an analytic monetary model, and emits a neutral comparison table.
//! Schedule selection is the operator's, in a separate decision-gated issue.
//!
//! # Two-track methodology
//!
//! Each candidate is evaluated on two tracks, because the two questions live
//! at different scales:
//!
//! 1. **Analytic monetary track** (real-world scale). The emission curve is a
//!    pure deterministic function of the policy parameters
//!    ([`MonetaryPolicy::halving_reward`] /
//!    [`MonetaryPolicy::calculate_tail_reward`]), so quantities like derived
//!    total supply, time-to-tail, %-issued-by-year, steady-state inflation and
//!    the early-vs-late issuance share are computed *exactly* over the real
//!    Phase-1 block counts (`BLOCKS_PER_YEAR = 6_307_200` at 5s blocks). No
//!    simulation is needed or desirable here — simulating ~60M blocks would add
//!    noise, not signal.
//!
//! 2. **Agent-based distribution track** (sim scale). Wealth-distribution
//!    outcomes (Gini trajectory, top-1%/10% share), the recycled-value
//!    accounting (fees burned vs subsidy emitted) and a velocity proxy depend
//!    on agent behaviour and must be simulated. Because simulating tens of
//!    millions of blocks is impractical, every schedule is **horizon-scaled**:
//!    `halving_interval` is divided by a common factor so each schedule
//!    reaches/approaches its tail within the same simulated horizon, while the
//!    *shape* (initial reward, halving count, tail rate, ratios between
//!    schedules) is preserved exactly. This is the assumption the issue calls
//!    out explicitly, and it is restated in the report.
//!
//! The sweep is reproducible: the agent-based path uses per-agent deterministic
//! RNG seeded from agent IDs (no global/thread RNG), and an identical agent
//! population is used for every schedule.

use crate::{
    simulation::{
        run_simulation, Agent, AgentId, MerchantAgent, Metrics, MinterAgent, RetailUserAgent,
        SimulationConfig, WhaleAgent, WhaleStrategy,
    },
    MonetaryPolicy,
};

/// 1 BTH expressed in nanoBTH (the smallest unit used by the policy).
const NANOBTH_PER_BTH: u128 = 1_000_000_000;

/// Real-world blocks per year at 5-second blocks (issue #350 grid assumption).
pub const BLOCKS_PER_YEAR: u64 = 6_307_200;

/// A candidate emission schedule, defined with **real-world** parameters.
///
/// The same parameters drive both the analytic monetary track (used directly)
/// and the agent-based distribution track (horizon-scaled via
/// [`scaled_policy`]).
#[derive(Clone, Debug)]
pub struct Schedule {
    /// Short identifier, e.g. `"S1"`.
    pub id: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// The real-world monetary policy for this schedule.
    pub policy: MonetaryPolicy,
}

impl Schedule {
    /// Real-world block height at which Phase 2 (tail emission) begins.
    pub fn tail_start_blocks(&self) -> u64 {
        self.policy.tail_emission_start_height()
    }

    /// Years until tail emission begins (real-world).
    pub fn time_to_tail_years(&self) -> f64 {
        self.tail_start_blocks() as f64 / BLOCKS_PER_YEAR as f64
    }
}

/// Build the candidate grid (S1..S5) using the parameters from issue #350.
///
/// Total supply is a **derived** quantity: `S = R0 * H * (2 - 2^-(K-1))`. The
/// real levers are `R0`, `H`, `K`, and the tail rate; the "100M vs 1.22B"
/// framing is just the value of `H`.
pub fn candidate_schedules() -> Vec<Schedule> {
    // 50 BTH initial reward, in nanoBTH.
    let r0: u64 = 50 * NANOBTH_PER_BTH as u64;

    // Shared block-time / difficulty parameters: 5s blocks (mainnet timing).
    let base = MonetaryPolicy {
        initial_reward: r0,
        halving_interval: 0, // set per schedule below
        halving_count: 5,
        tail_inflation_bps: 200,
        target_block_time_secs: 5,
        min_block_time_secs: 3,
        max_block_time_secs: 40,
        difficulty_adjustment_interval: 17_280, // ~1 day at 5s blocks
        max_difficulty_adjustment_bps: 2500,
        expected_fee_burn_rate_bps: 50,
    };

    vec![
        Schedule {
            id: "S1",
            label: "slow / Bitcoin-ish (~1.22B BTH, ~10yr to tail)",
            policy: MonetaryPolicy {
                halving_interval: 12_614_400, // ~2yr
                halving_count: 5,
                ..base.clone()
            },
        },
        Schedule {
            id: "S2",
            label: "medium (~305M BTH, ~2.5yr to tail)",
            policy: MonetaryPolicy {
                halving_interval: 3_150_000, // ~6mo
                halving_count: 5,
                ..base.clone()
            },
        },
        Schedule {
            id: "S3",
            label: "fast / flat (~100M BTH, ~10mo to tail)",
            policy: MonetaryPolicy {
                halving_interval: 1_051_200, // ~2mo
                halving_count: 5,
                ..base.clone()
            },
        },
        Schedule {
            id: "S4",
            label: "very fast / low front (K=3, faster to tail)",
            policy: MonetaryPolicy {
                halving_interval: 1_051_200, // ~2mo
                halving_count: 3,
                ..base.clone()
            },
        },
        Schedule {
            id: "S5",
            label: "fast / flat, 1% tail (tail-rate sensitivity vs S3)",
            policy: MonetaryPolicy {
                halving_interval: 1_051_200, // ~2mo, same shape as S3
                halving_count: 5,
                tail_inflation_bps: 100, // 1% tail instead of 2%
                ..base.clone()
            },
        },
    ]
}

// ===========================================================================
// Analytic monetary track (real-world scale, exact)
// ===========================================================================

/// Total Phase-1 supply (nanoBTH) emitted by `policy` over its real-world
/// halving schedule. Exact sum of `R0 >> h` over each halving epoch.
pub fn phase1_supply_nanobth(policy: &MonetaryPolicy) -> u128 {
    let h = policy.halving_interval as u128;
    let r0 = policy.initial_reward as u128;
    let mut total = 0u128;
    for halving in 0..policy.halving_count {
        total += h * (r0 >> halving);
    }
    total
}

/// Cumulative supply (nanoBTH) emitted by the end of block `height` under the
/// real-world schedule, including tail emission after Phase 1.
pub fn cumulative_supply_at(policy: &MonetaryPolicy, height: u64) -> u128 {
    let tail_start = policy.tail_emission_start_height();
    if height <= tail_start {
        // Still in Phase 1: sum rewards block-by-block, but exploit that the
        // reward is constant within a halving epoch.
        let mut total = 0u128;
        let h = policy.halving_interval;
        if h == 0 {
            return 0;
        }
        let full_epochs = (height / h).min(policy.halving_count as u64);
        let r0 = policy.initial_reward as u128;
        for halving in 0..full_epochs {
            total += h as u128 * (r0 >> halving);
        }
        // Partial epoch.
        let consumed = full_epochs * h;
        if consumed < height && full_epochs < policy.halving_count as u64 {
            let rem = (height - consumed) as u128;
            total += rem * (r0 >> full_epochs);
        }
        total
    } else {
        let phase1 = phase1_supply_nanobth(policy);
        // Tail reward is calibrated from supply at transition.
        let tail_reward =
            policy.calculate_tail_reward((phase1.min(u64::MAX as u128)) as u64) as u128;
        let tail_blocks = (height - tail_start) as u128;
        phase1 + tail_blocks * tail_reward
    }
}

/// Fraction of total Phase-1 supply minted within the first `frac` of
/// blocks-to-tail (the early-minter-concentration proxy).
///
/// E.g. `early_issuance_share(p, 0.10)` is the share of all Phase-1 coins
/// minted in the first 10% of the run-up to tail emission.
pub fn early_issuance_share(policy: &MonetaryPolicy, frac: f64) -> f64 {
    let tail_start = policy.tail_emission_start_height();
    if tail_start == 0 {
        return 0.0;
    }
    let cutoff = (tail_start as f64 * frac).round() as u64;
    let early = cumulative_supply_at(policy, cutoff) as f64;
    let total = phase1_supply_nanobth(policy) as f64;
    if total == 0.0 {
        0.0
    } else {
        early / total
    }
}

/// Percentage of *total Phase-1 supply* already issued by the end of
/// simulated/real year `year`.
pub fn pct_issued_by_year(policy: &MonetaryPolicy, year: u64) -> f64 {
    let height = year.saturating_mul(BLOCKS_PER_YEAR);
    let total = phase1_supply_nanobth(policy) as f64;
    if total == 0.0 {
        return 0.0;
    }
    // Cap Phase-1 portion at 100%; tail emission beyond that is reported
    // separately as steady-state inflation.
    let p1_at =
        cumulative_supply_at(policy, height.min(policy.tail_emission_start_height())) as f64;
    (p1_at / total) * 100.0
}

/// Steady-state annual inflation (%) once tail emission is in effect.
///
/// Equals `tail_reward * blocks_per_year / supply_at_transition`, which is the
/// gross tail emission rate. The net rate is lower by the fee-burn rate; the
/// policy targets `tail_inflation_bps` net, so we report both.
pub fn steady_state_inflation_pct(policy: &MonetaryPolicy) -> (f64, f64) {
    let supply = phase1_supply_nanobth(policy);
    if supply == 0 {
        return (0.0, 0.0);
    }
    let tail_reward = policy.calculate_tail_reward((supply.min(u64::MAX as u128)) as u64) as f64;
    let gross_annual = tail_reward * BLOCKS_PER_YEAR as f64;
    let gross_pct = gross_annual / supply as f64 * 100.0;
    let net_pct = policy.tail_inflation_bps as f64 / 100.0; // bps -> %
    (gross_pct, net_pct)
}

/// Analytic monetary metrics for one schedule, all at real-world scale.
#[derive(Clone, Debug)]
pub struct MonetaryMetrics {
    pub total_supply_bth: f64,
    pub time_to_tail_years: f64,
    pub early_share_10pct: f64,
    pub early_share_25pct: f64,
    pub pct_issued_y1: f64,
    pub pct_issued_y2: f64,
    pub pct_issued_y5: f64,
    pub steady_state_gross_pct: f64,
    pub steady_state_net_pct: f64,
}

/// Compute the analytic monetary metrics for a schedule.
pub fn monetary_metrics(schedule: &Schedule) -> MonetaryMetrics {
    let policy = &schedule.policy;
    let (gross, net) = steady_state_inflation_pct(policy);
    MonetaryMetrics {
        total_supply_bth: phase1_supply_nanobth(policy) as f64 / NANOBTH_PER_BTH as f64,
        time_to_tail_years: schedule.time_to_tail_years(),
        early_share_10pct: early_issuance_share(policy, 0.10),
        early_share_25pct: early_issuance_share(policy, 0.25),
        pct_issued_y1: pct_issued_by_year(policy, 1),
        pct_issued_y2: pct_issued_by_year(policy, 2),
        pct_issued_y5: pct_issued_by_year(policy, 5),
        steady_state_gross_pct: gross,
        steady_state_net_pct: net,
    }
}

// ===========================================================================
// Agent-based distribution track (sim scale, horizon-scaled)
// ===========================================================================

/// Parameters controlling the agent-based distribution track.
#[derive(Clone, Debug)]
pub struct SweepParams {
    /// Number of simulated rounds (each round = `blocks_per_round` blocks).
    pub rounds: u64,
    /// Blocks processed per round.
    pub blocks_per_round: u64,
    /// Number of retail users.
    pub retail: usize,
    /// Number of merchants.
    pub merchants: usize,
    /// Number of minters.
    pub minters: usize,
    /// Number of whales.
    pub whales: usize,
    /// Snapshot frequency (rounds between metric snapshots).
    pub snapshot_frequency: u64,
}

impl Default for SweepParams {
    fn default() -> Self {
        Self {
            rounds: 4_000,
            blocks_per_round: 4,
            retail: 120,
            merchants: 12,
            minters: 4,
            whales: 4,
            snapshot_frequency: 200,
        }
    }
}

impl SweepParams {
    /// Total simulated blocks across the run.
    pub fn total_blocks(&self) -> u64 {
        self.rounds * self.blocks_per_round
    }
}

/// Compute a single common scale divisor for the whole grid.
///
/// All schedules are scaled by the *same* divisor so that their *relative*
/// time-to-tail is preserved in the sim: a schedule whose real tail is 10x
/// farther out than another's still takes ~10x longer to reach tail in the
/// simulation. The divisor is chosen so the slowest schedule (largest
/// `tail_emission_start_height`) reaches its tail by `target_sim_blocks`.
pub fn common_scale_divisor(schedules: &[Schedule], target_sim_blocks: u64) -> u64 {
    let max_tail_start = schedules
        .iter()
        .map(|s| s.policy.tail_emission_start_height())
        .max()
        .unwrap_or(1)
        .max(1);
    (max_tail_start / target_sim_blocks.max(1)).max(1)
}

/// Produce a horizon-scaled copy of a real-world policy by dividing its block
/// counts by `divisor`, preserving shape and relative ordering.
///
/// `halving_count`, `initial_reward`, `tail_inflation_bps` and all timing
/// parameters are unchanged, so the emission *curve shape* is preserved.
/// Applying the same `divisor` to every schedule preserves their relative
/// time-to-tail, so the sim exercises the actual difference between schedules.
/// `secs_per_block` (via the policy's `target_block_time_secs`) stays at the
/// real target so derived inflation remains comparable.
pub fn scaled_policy(policy: &MonetaryPolicy, divisor: u64) -> MonetaryPolicy {
    let mut scaled = policy.clone();
    let divisor = divisor.max(1);
    let interval = (policy.halving_interval / divisor).max(1);
    scaled.halving_interval = interval;
    // Keep the difficulty epoch shorter than the (now small) halving interval
    // so adjustments still happen, but never zero.
    scaled.difficulty_adjustment_interval = (interval / 4).max(1);
    scaled
}

/// Build a deterministic, fixed agent population. Identical across schedules.
///
/// Agent IDs are assigned in a fixed order, and the agents themselves seed
/// their RNG from those IDs, so the population is byte-for-byte reproducible.
fn build_population(params: &SweepParams) -> Vec<Box<dyn Agent>> {
    let retail_balance = 1_000u64;
    let merchant_balance = 5_000u64;
    let minter_balance = 10_000u64;
    let whale_balance = 500_000u64;

    let mut agents: Vec<Box<dyn Agent>> = Vec::new();
    let mut next_id = 0u64;

    // Merchants first (retail references them).
    let merchant_ids: Vec<AgentId> = (0..params.merchants)
        .map(|_| {
            let id = AgentId(next_id);
            next_id += 1;
            id
        })
        .collect();
    for &id in &merchant_ids {
        let mut m = MerchantAgent::new(id);
        m.account_mut_ref().balance = merchant_balance;
        agents.push(Box::new(m));
    }

    // Retail users.
    for _ in 0..params.retail {
        let id = AgentId(next_id);
        next_id += 1;
        let mut r = RetailUserAgent::new(id)
            .with_merchants(merchant_ids.clone())
            .with_spending_probability(0.15)
            .with_avg_spend(50);
        r.account_mut_ref().balance = retail_balance;
        agents.push(Box::new(r));
    }

    // Whales (passive: they hold, mild spending).
    for _ in 0..params.whales {
        let id = AgentId(next_id);
        next_id += 1;
        let mut w = WhaleAgent::new(id, whale_balance, WhaleStrategy::Passive)
            .with_spending_targets(merchant_ids.clone())
            .with_spending_rate(0.001);
        w.account_mut_ref().balance = whale_balance;
        agents.push(Box::new(w));
    }

    // Minters (receive block rewards; sell a fraction into the economy).
    for _ in 0..params.minters {
        let id = AgentId(next_id);
        next_id += 1;
        let mut mi = MinterAgent::new(id)
            .with_buyers(merchant_ids.clone())
            .with_minting_interval(1)
            .with_sell_fraction(0.5);
        mi.account_mut_ref().balance = minter_balance;
        agents.push(Box::new(mi));
    }

    agents
}

/// Distribution + recycling metrics from the agent-based track.
#[derive(Clone, Debug)]
pub struct DistributionMetrics {
    pub initial_gini: f64,
    pub final_gini: f64,
    /// Gini at ~25%, ~50%, ~75% of the run (trajectory samples).
    pub gini_trajectory: Vec<(u64, f64)>,
    pub final_top_1_pct: f64,
    pub final_top_10_pct: f64,
    /// Total block subsidy emitted over the run (nanoBTH-scale sim units).
    pub subsidy_emitted: u128,
    /// Total fees burned/recycled over the run.
    pub fees_recycled: u128,
    /// subsidy / (subsidy + recycled); 1.0 = all minter incentive from
    /// emission, 0.0 = all from recycled value.
    pub subsidy_fraction: f64,
    /// Transactions per simulated year (velocity/activity proxy).
    pub velocity_tx_per_year: f64,
    /// Transactions per 1M BTH of final supply (turnover proxy).
    pub turnover_per_supply: f64,
    pub final_phase: String,
    pub final_block_reward: u64,
    pub sim_total_blocks: u64,
}

/// Run one schedule through the agent-based simulator (horizon-scaled by the
/// shared `divisor`) and collect distribution metrics.
pub fn run_schedule(
    schedule: &Schedule,
    params: &SweepParams,
    divisor: u64,
) -> DistributionMetrics {
    let mut agents = build_population(params);

    // Horizon-scale by the grid-wide common divisor so relative time-to-tail
    // between schedules is preserved within the sim.
    let policy = scaled_policy(&schedule.policy, divisor);

    let config = SimulationConfig {
        rounds: params.rounds,
        snapshot_frequency: params.snapshot_frequency,
        blocks_per_round: params.blocks_per_round,
        verbose: false,
        initial_difficulty: 1_000,
        ..Default::default()
    }
    .with_monetary_policy(policy);

    let result = run_simulation(&mut agents, &config);
    let snapshots: &[Metrics] = &result.metrics.snapshots;

    let initial_gini = snapshots.first().map(|m| m.gini_coefficient).unwrap_or(0.0);
    let final_gini = snapshots.last().map(|m| m.gini_coefficient).unwrap_or(0.0);
    let (final_top_1, final_top_10) = snapshots
        .last()
        .map(|m| (m.top_1_pct_wealth_share, m.top_10_pct_wealth_share))
        .unwrap_or((0.0, 0.0));

    // Trajectory samples at ~25/50/75% of the run.
    let mut gini_trajectory = Vec::new();
    if !snapshots.is_empty() {
        for frac in [0.25, 0.50, 0.75] {
            let idx = ((snapshots.len() as f64 - 1.0) * frac).round() as usize;
            let m = &snapshots[idx];
            gini_trajectory.push((m.round, m.gini_coefficient));
        }
    }

    let mstats = result.monetary_stats;
    let subsidy_emitted = mstats.as_ref().map(|s| s.total_emitted).unwrap_or(0);
    let fees_recycled = mstats.as_ref().map(|s| s.total_fees_burned).unwrap_or(0);
    let subsidy_fraction = {
        let denom = subsidy_emitted as f64 + fees_recycled as f64;
        if denom > 0.0 {
            subsidy_emitted as f64 / denom
        } else {
            0.0
        }
    };

    let tx_count = snapshots.last().map(|m| m.transaction_count).unwrap_or(0);
    let final_supply = snapshots.last().map(|m| m.total_wealth).unwrap_or(0);
    // Simulated years = sim blocks / blocks-per-year-at-target-block-time.
    let secs_per_block = schedule.policy.target_block_time_secs.max(1);
    let blocks_per_year = (365 * 24 * 3600) / secs_per_block;
    let sim_years = params.total_blocks() as f64 / blocks_per_year as f64;
    let velocity_tx_per_year = if sim_years > 0.0 {
        tx_count as f64 / sim_years
    } else {
        0.0
    };
    // Turnover expressed as transactions per 1M BTH of final supply, so the
    // figure is readable at nanoBTH scale (raw tx/nanoBTH is ~1e-12).
    let supply_in_million_bth = final_supply as f64 / NANOBTH_PER_BTH as f64 / 1.0e6;
    let turnover_per_supply = if supply_in_million_bth > 0.0 {
        tx_count as f64 / supply_in_million_bth
    } else {
        0.0
    };

    DistributionMetrics {
        initial_gini,
        final_gini,
        gini_trajectory,
        final_top_1_pct: final_top_1,
        final_top_10_pct: final_top_10,
        subsidy_emitted,
        fees_recycled,
        subsidy_fraction,
        velocity_tx_per_year,
        turnover_per_supply,
        final_phase: mstats
            .as_ref()
            .map(|s| s.phase.to_string())
            .unwrap_or_else(|| "None".to_string()),
        final_block_reward: mstats.as_ref().map(|s| s.block_reward).unwrap_or(0),
        sim_total_blocks: params.total_blocks(),
    }
}

/// Combined per-schedule result.
#[derive(Clone, Debug)]
pub struct ScheduleResult {
    pub id: &'static str,
    pub label: &'static str,
    pub monetary: MonetaryMetrics,
    pub distribution: DistributionMetrics,
}

/// Run the full sweep across all candidate schedules.
///
/// A single grid-wide horizon-scale divisor is computed so the slowest schedule
/// reaches its tail near the end of the simulated horizon and faster schedules
/// reach theirs proportionally sooner — preserving relative time-to-tail.
pub fn run_sweep(params: &SweepParams) -> Vec<ScheduleResult> {
    let schedules = candidate_schedules();
    // Target the slowest schedule reaching tail at ~85% of the sim horizon so
    // the tail phase is exercised for every schedule.
    let target_sim_blocks = ((params.total_blocks() as f64) * 0.85) as u64;
    let divisor = common_scale_divisor(&schedules, target_sim_blocks);
    schedules
        .into_iter()
        .map(|schedule| {
            let monetary = monetary_metrics(&schedule);
            let distribution = run_schedule(&schedule, params, divisor);
            ScheduleResult {
                id: schedule.id,
                label: schedule.label,
                monetary,
                distribution,
            }
        })
        .collect()
}

// ===========================================================================
// Report emitters (CSV + Markdown). Data + neutral observations only.
// ===========================================================================

/// Render the results as CSV (one row per schedule).
pub fn to_csv(results: &[ScheduleResult]) -> String {
    let mut csv = String::new();
    csv.push_str(
        "schedule,label,total_supply_bth,time_to_tail_years,early_share_10pct,early_share_25pct,\
pct_issued_y1,pct_issued_y2,pct_issued_y5,steady_state_gross_pct,steady_state_net_pct,\
initial_gini,final_gini,final_top_1_pct,final_top_10_pct,subsidy_emitted,fees_recycled,\
subsidy_fraction,velocity_tx_per_year,turnover_per_supply,sim_total_blocks,final_phase\n",
    );
    for r in results {
        let m = &r.monetary;
        let d = &r.distribution;
        csv.push_str(&format!(
            "{},\"{}\",{:.0},{:.3},{:.4},{:.4},{:.2},{:.2},{:.2},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{:.4},{:.1},{:.2},{},{}\n",
            r.id,
            r.label,
            m.total_supply_bth,
            m.time_to_tail_years,
            m.early_share_10pct,
            m.early_share_25pct,
            m.pct_issued_y1,
            m.pct_issued_y2,
            m.pct_issued_y5,
            m.steady_state_gross_pct,
            m.steady_state_net_pct,
            d.initial_gini,
            d.final_gini,
            d.final_top_1_pct,
            d.final_top_10_pct,
            d.subsidy_emitted,
            d.fees_recycled,
            d.subsidy_fraction,
            d.velocity_tx_per_year,
            d.turnover_per_supply,
            d.sim_total_blocks,
            d.final_phase,
        ));
    }
    csv
}

/// Render the results as a Markdown report.
///
/// Presents the numbers and **neutral factual observations only**. It must not
/// recommend or select a schedule — selection is the operator's, in a
/// follow-up decision-gated issue.
pub fn to_markdown(results: &[ScheduleResult], params: &SweepParams) -> String {
    let mut s = String::new();
    s.push_str("# Emission-Schedule Sweep (issue #350)\n\n");
    s.push_str(
        "Data to inform the permanent emission-schedule decision (#321). \
This report presents numbers and **neutral observations only**. It does NOT \
recommend or select a schedule; selection is the operator's, in a separate \
decision-gated issue (#351).\n\n",
    );

    s.push_str("## Method\n\n");
    s.push_str(&format!(
        "Two tracks per schedule (see module docs for detail):\n\n\
1. **Analytic monetary track** — exact, at real-world scale \
(`BLOCKS_PER_YEAR = {bpy}`, 5s blocks). Derived total supply, time-to-tail, \
%-issued-by-year, steady-state inflation and early-vs-late issuance share are \
computed directly from the policy parameters (emission is a deterministic \
function of height), not simulated.\n\
2. **Agent-based distribution track** — simulated, at sim scale. Each schedule \
is **horizon-scaled**: `halving_interval` is divided by a common factor so the \
tail is reached within {blocks} simulated blocks, while the curve *shape* and \
the *relative ordering* of schedules are preserved. Gini trajectory, \
top-1%/10% share, the subsidy-vs-recycled accounting and a velocity proxy come \
from this track.\n\n",
        bpy = BLOCKS_PER_YEAR,
        blocks = params.total_blocks(),
    ));
    s.push_str(&format!(
        "**Horizon-scaling assumption (explicit):** the simulated horizon \
({blocks} blocks across {rounds} rounds) is far shorter than 10 real years \
(~{real}M blocks). Distribution outcomes are therefore comparable *between \
schedules under the same scaling*, not as absolute predictions of real-world \
year-N Gini. All schedules share one fixed, deterministic agent population \
({retail} retail, {merch} merchants, {whales} whales, {minters} minters); the \
agent RNG is seeded from agent IDs, so the run is reproducible.\n\n",
        blocks = params.total_blocks(),
        rounds = params.rounds,
        real = (10 * BLOCKS_PER_YEAR) / 1_000_000,
        retail = params.retail,
        merch = params.merchants,
        whales = params.whales,
        minters = params.minters,
    ));

    // --- Monetary table ---
    s.push_str("## Monetary metrics (analytic, real-world scale)\n\n");
    s.push_str(
        "| Schedule | Derived supply (BTH) | Time-to-tail (yr) | Early share (first 10%) | Early share (first 25%) | % issued by Y1 | % issued by Y2 | % issued by Y5 | Steady-state gross/yr | Steady-state net/yr |\n",
    );
    s.push_str(
        "|----------|----------------------|-------------------|-------------------------|-------------------------|----------------|----------------|----------------|-----------------------|---------------------|\n",
    );
    for r in results {
        let m = &r.monetary;
        s.push_str(&format!(
            "| {} | {:.3e} | {:.2} | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.2}% | {:.2}% |\n",
            r.id,
            m.total_supply_bth,
            m.time_to_tail_years,
            m.early_share_10pct * 100.0,
            m.early_share_25pct * 100.0,
            m.pct_issued_y1,
            m.pct_issued_y2,
            m.pct_issued_y5,
            m.steady_state_gross_pct,
            m.steady_state_net_pct,
        ));
    }
    s.push('\n');

    // --- Distribution table ---
    s.push_str("## Distribution & MoE metrics (agent-based, horizon-scaled)\n\n");
    s.push_str(
        "| Schedule | Gini init->final | Gini @25/50/75% | Top 1% | Top 10% | Subsidy emitted | Fees recycled | Subsidy fraction | Velocity (tx/yr) | Turnover (tx/1M BTH) | Final phase |\n",
    );
    s.push_str(
        "|----------|------------------|-----------------|--------|---------|-----------------|---------------|------------------|------------------|----------------------|-------------|\n",
    );
    for r in results {
        let d = &r.distribution;
        let traj = d
            .gini_trajectory
            .iter()
            .map(|(_, g)| format!("{:.3}", g))
            .collect::<Vec<_>>()
            .join("/");
        s.push_str(&format!(
            "| {} | {:.3}->{:.3} | {} | {:.1}% | {:.1}% | {} | {} | {:.3} | {:.0} | {:.2} | {} |\n",
            r.id,
            d.initial_gini,
            d.final_gini,
            traj,
            d.final_top_1_pct * 100.0,
            d.final_top_10_pct * 100.0,
            d.subsidy_emitted,
            d.fees_recycled,
            d.subsidy_fraction,
            d.velocity_tx_per_year,
            d.turnover_per_supply,
            d.final_phase,
        ));
    }
    s.push('\n');

    // --- Schedule legend ---
    s.push_str("## Schedule definitions\n\n");
    for sch in candidate_schedules() {
        s.push_str(&format!(
            "- **{}** ({}): R0={} BTH, H={} blocks (~{:.2} yr), K={}, tail={}bps.\n",
            sch.id,
            sch.label,
            sch.policy.initial_reward / NANOBTH_PER_BTH as u64,
            sch.policy.halving_interval,
            sch.policy.halving_interval as f64 / BLOCKS_PER_YEAR as f64,
            sch.policy.halving_count,
            sch.policy.tail_inflation_bps,
        ));
    }
    s.push('\n');

    // --- Neutral observations (factual, no recommendation) ---
    s.push_str("## Neutral observations\n\n");
    s.push_str(&neutral_observations(results));

    s.push_str(
        "\n_No schedule is recommended here. The numbers above are inputs to the \
operator's decision in the follow-up issue._\n",
    );
    s
}

/// Generate factual, comparative observations from the data. These describe
/// what the numbers show; they do not rank or recommend.
fn neutral_observations(results: &[ScheduleResult]) -> String {
    let mut s = String::new();
    if results.is_empty() {
        return s;
    }

    // Spread of derived supply.
    let min_supply = results
        .iter()
        .map(|r| r.monetary.total_supply_bth)
        .fold(f64::INFINITY, f64::min);
    let max_supply = results
        .iter()
        .map(|r| r.monetary.total_supply_bth)
        .fold(0.0, f64::max);
    s.push_str(&format!(
        "- Derived total supply spans {:.3e} to {:.3e} BTH across the grid (~{:.0}x), \
driven entirely by the halving interval H.\n",
        min_supply,
        max_supply,
        if min_supply > 0.0 {
            max_supply / min_supply
        } else {
            0.0
        },
    ));

    // Time-to-tail spread.
    let min_ttt = results
        .iter()
        .map(|r| r.monetary.time_to_tail_years)
        .fold(f64::INFINITY, f64::min);
    let max_ttt = results
        .iter()
        .map(|r| r.monetary.time_to_tail_years)
        .fold(0.0, f64::max);
    s.push_str(&format!(
        "- Time-to-tail ranges from {:.2} to {:.2} real years; faster schedules \
front-load issuance into fewer years.\n",
        min_ttt, max_ttt,
    ));

    // Early-issuance comparison (concentration proxy): contrast the schedules
    // with the highest and lowest early-10% share.
    let high_early = results.iter().max_by(|a, b| {
        a.monetary
            .early_share_10pct
            .partial_cmp(&b.monetary.early_share_10pct)
            .unwrap()
    });
    let low_early = results.iter().min_by(|a, b| {
        a.monetary
            .early_share_10pct
            .partial_cmp(&b.monetary.early_share_10pct)
            .unwrap()
    });
    if let (Some(first), Some(last)) = (high_early, low_early) {
        s.push_str(&format!(
            "- Early-issuance share (first 10% of blocks-to-tail) ranges across the grid; \
e.g. {} mints {:.1}% of Phase-1 supply in that window vs {} at {:.1}%.\n",
            first.id,
            first.monetary.early_share_10pct * 100.0,
            last.id,
            last.monetary.early_share_10pct * 100.0,
        ));
    }

    // Subsidy fraction observation.
    let avg_subsidy_frac = results
        .iter()
        .map(|r| r.distribution.subsidy_fraction)
        .sum::<f64>()
        / results.len() as f64;
    s.push_str(&format!(
        "- In the simulated economy, the subsidy fraction (emission / (emission + recycled \
fees)) averages {:.2} across schedules; the remainder of minter-facing value comes from \
recycled fees. This is a sim-scale accounting at the horizon-scaled emission rate, not a \
real-world security-budget claim.\n",
        avg_subsidy_frac,
    ));

    // Gini direction note (factual).
    s.push_str(
        "- Final Gini and top-share figures are reported per schedule above; compare them \
*between* schedules under the shared scaling rather than as absolute year-N predictions.\n",
    );

    // Tail-rate sensitivity (S3 vs S5 if both present).
    let s3 = results.iter().find(|r| r.id == "S3");
    let s5 = results.iter().find(|r| r.id == "S5");
    if let (Some(s3), Some(s5)) = (s3, s5) {
        s.push_str(&format!(
            "- Tail-rate sensitivity (same shape, different tail): S3 (2% tail) steady-state \
net inflation {:.2}%/yr vs S5 (1% tail) {:.2}%/yr; their final Gini under this run is \
{:.3} and {:.3} respectively.\n",
            s3.monetary.steady_state_net_pct,
            s5.monetary.steady_state_net_pct,
            s3.distribution.final_gini,
            s5.distribution.final_gini,
        ));
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_has_five_schedules() {
        let g = candidate_schedules();
        assert_eq!(g.len(), 5);
        assert_eq!(g[0].id, "S1");
        assert_eq!(g[4].id, "S5");
    }

    #[test]
    fn s1_supply_is_about_1_22b() {
        let g = candidate_schedules();
        let m = monetary_metrics(&g[0]);
        // ~1.22B BTH expected for S1 per the issue.
        assert!(
            (m.total_supply_bth - 1.22e9).abs() < 1.0e8,
            "S1 supply {} not ~1.22B",
            m.total_supply_bth
        );
    }

    #[test]
    fn s3_supply_is_about_100m() {
        let g = candidate_schedules();
        let s3 = g.iter().find(|s| s.id == "S3").unwrap();
        let m = monetary_metrics(s3);
        assert!(
            (m.total_supply_bth - 1.0e8).abs() < 1.0e7,
            "S3 supply {} not ~100M",
            m.total_supply_bth
        );
    }

    #[test]
    fn early_share_is_monotonic_increasing() {
        // The first 25% window must contain at least as much as the first 10%.
        for s in candidate_schedules() {
            let e10 = early_issuance_share(&s.policy, 0.10);
            let e25 = early_issuance_share(&s.policy, 0.25);
            assert!(e25 >= e10, "{}: e25 {} < e10 {}", s.id, e25, e10);
            assert!(e10 > 0.0 && e25 <= 1.0);
        }
    }

    #[test]
    fn scaled_policy_preserves_shape() {
        let g = candidate_schedules();
        let divisor = 1_000u64;
        let scaled = scaled_policy(&g[0].policy, divisor);
        assert_eq!(scaled.halving_count, g[0].policy.halving_count);
        assert_eq!(scaled.initial_reward, g[0].policy.initial_reward);
        assert_eq!(scaled.tail_inflation_bps, g[0].policy.tail_inflation_bps);
        assert!(scaled.halving_interval >= 1);
        // Interval scaled down by the divisor.
        assert_eq!(
            scaled.halving_interval,
            (g[0].policy.halving_interval / divisor).max(1)
        );
    }

    #[test]
    fn common_divisor_preserves_relative_time_to_tail() {
        let g = candidate_schedules();
        let divisor = common_scale_divisor(&g, 10_000);
        let s1 = g.iter().find(|s| s.id == "S1").unwrap();
        let s3 = g.iter().find(|s| s.id == "S3").unwrap();
        let s1_scaled = scaled_policy(&s1.policy, divisor);
        let s3_scaled = scaled_policy(&s3.policy, divisor);
        // S1's real tail is ~12x farther out than S3's; the ratio survives
        // scaling (within rounding).
        let real_ratio = s1.policy.tail_emission_start_height() as f64
            / s3.policy.tail_emission_start_height() as f64;
        let scaled_ratio = s1_scaled.tail_emission_start_height() as f64
            / s3_scaled.tail_emission_start_height() as f64;
        assert!(
            (real_ratio - scaled_ratio).abs() / real_ratio < 0.05,
            "ratio drift too large: real {real_ratio} scaled {scaled_ratio}"
        );
    }

    #[test]
    fn s5_has_lower_tail_than_s3() {
        let g = candidate_schedules();
        let s3 = g.iter().find(|s| s.id == "S3").unwrap();
        let s5 = g.iter().find(|s| s.id == "S5").unwrap();
        assert!(s5.policy.tail_inflation_bps < s3.policy.tail_inflation_bps);
    }

    #[test]
    fn small_sweep_runs_and_reports() {
        let params = SweepParams {
            rounds: 80,
            blocks_per_round: 4,
            retail: 20,
            merchants: 4,
            minters: 2,
            whales: 2,
            snapshot_frequency: 20,
        };
        let results = run_sweep(&params);
        assert_eq!(results.len(), 5);
        let csv = to_csv(&results);
        assert!(csv.lines().count() >= 6); // header + 5 rows
        let md = to_markdown(&results, &params);
        // Must NOT recommend a schedule.
        let lower = md.to_lowercase();
        assert!(lower.contains("neutral observations"));
        assert!(!lower.contains("we recommend"));
        assert!(!lower.contains("is best"));
        assert!(!lower.contains("winner"));
    }

    #[test]
    fn sweep_is_reproducible() {
        let params = SweepParams {
            rounds: 60,
            blocks_per_round: 4,
            retail: 16,
            merchants: 3,
            minters: 2,
            whales: 2,
            snapshot_frequency: 20,
        };
        let a = to_csv(&run_sweep(&params));
        let b = to_csv(&run_sweep(&params));
        assert_eq!(a, b, "sweep output must be deterministic");
    }
}
