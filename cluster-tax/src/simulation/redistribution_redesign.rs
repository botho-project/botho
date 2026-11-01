//! Sim-only redistribution redesign study (#1801).
//!
//! The operator's ruling on #1727 / #1682 asks for candidate redesigns of the
//! redistribution mechanism, each tested on three axes before any constant or
//! consensus rule moves:
//!
//! 1. **Gini**: the #1790 production-fidelity harness
//!    ([`super::m2_production`]), at 1x / 100x / 1000x transaction density.
//! 2. **Splitting**: the #1761 faithful-to-code splitting grid
//!    ([`super::lottery_reward_cap_faithful`]), 192 points.
//! 3. **Privacy**: what the rule needs to read, and what that reveals about
//!    ring inputs, measured where possible on the #1757 production decoy routes
//!    ([`super::privacy_routes`]).
//!
//! # Variants
//!
//! Each variant is a pair of switches on the m2-production harness
//! ([`TicketRule`] x [`TagDecay`]) plus the matching rule in the splitting
//! grid. R0 is the current rule set and reproduces #1790 and #1761 exactly
//! (tested).
//!
//! | id | lottery ticket weight | tag decay | targets |
//! |---|---|---|---|
//! | R0 | uniform per output | 5% per hop | (baseline) |
//! | R1 | value x age | 5% per hop | tickets track hashpower / output count; splitting |
//! | R2 | value x (7 - factor) (`ClusterWeighted`) | 5% per hop | the same, plus a progressive tilt |
//! | R3 | uniform, each ticket priced at its expected payout | 5% per hop | splitting |
//! | R4 | uniform per output | per block (time), not per hop | tags erased by hop churn |
//! | R5 | value x (7 - factor) | per block | R2's tilt with tags that survive churn |
//!
//! # The candidate-set rule matters
//!
//! The node never removes a ring-spent output from `utxo_db`: a CLSAG input
//! only records its key image (`ledger/store/writer.rs`), because the node
//! does not know which ring member is real. So on the node, spent outputs stay
//! lottery candidates until they age out of the 10k window. #1790's harness
//! removes them (its default here, so R0 reproduces it); that rule is only
//! enforceable if the winner reveals which output it spent. The study reports
//! both: `spent_eligible = false` (#1790) and `true` (what the node can do
//! without a new reveal). In the splitting grid the difference decides the
//! value-linear rules: with spent outputs eligible, re-spending the same
//! capital mints fresh value weight every block ("churn").
//!
//! ANALYSIS ONLY: nothing here is node or consensus code.

use std::collections::BTreeMap;

use crate::{
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, ClaimedClass, ConsensusFeeInputs,
        ConsensusFeeParams,
    },
    fee_curve::PICO_PER_BTH,
    lottery::{FACTOR_SCALE, MAX_FACTOR_SCALED},
    LotteryDrawConfig,
};

use super::{
    lottery_reward_cap_faithful::{
        code_pool_payout, code_reward_cap, evaluate, grid, k_grid, optimum, organic_tx_fee,
        reference_scenario, whale_cost_per_output, Rules, Scenario, BLOCKS_PER_DAY,
        COINBASE_TICKETS_PER_BLOCK,
    },
    m2_production::{
        MiningModel, ProdParams, TagDecay, TicketRule, WhaleStrategy, PROD_BLOCKS_PER_YEAR,
    },
    privacy_routes::{
        sample_route_ring_ages, ChainModel, DecoyRoute, RealAgeSpec, RouteRingAges,
        MIN_DECOY_AGE_BLOCKS,
    },
};

// ===========================================================================
// Variants
// ===========================================================================

/// One candidate rule set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// R0: the current rules (#1790 / #1761).
    Baseline,
    /// R1: tickets weighted by value x age.
    ValueAge,
    /// R2: tickets weighted by value x (max factor - factor + 1).
    InverseFactor,
    /// R3: uniform tickets, each priced at its expected lifetime payout.
    TicketPrice,
    /// R4: uniform tickets; tags decay with time, not with hops.
    TimeDecay,
    /// R5: R2's weight with R4's decay.
    InverseFactorTimeDecay,
}

impl Variant {
    /// Every variant, in report order.
    pub const ALL: [Variant; 6] = [
        Variant::Baseline,
        Variant::ValueAge,
        Variant::InverseFactor,
        Variant::TicketPrice,
        Variant::TimeDecay,
        Variant::InverseFactorTimeDecay,
    ];

    /// Short id (R0..R5).
    pub fn id(self) -> &'static str {
        match self {
            Variant::Baseline => "R0",
            Variant::ValueAge => "R1",
            Variant::InverseFactor => "R2",
            Variant::TicketPrice => "R3",
            Variant::TimeDecay => "R4",
            Variant::InverseFactorTimeDecay => "R5",
        }
    }

    /// CLI name.
    pub fn name(self) -> &'static str {
        match self {
            Variant::Baseline => "baseline",
            Variant::ValueAge => "value-age",
            Variant::InverseFactor => "inverse-factor",
            Variant::TicketPrice => "ticket-price",
            Variant::TimeDecay => "time-decay",
            Variant::InverseFactorTimeDecay => "inverse-factor+time-decay",
        }
    }

    /// Parse a CLI name or id.
    pub fn parse(s: &str) -> Option<Variant> {
        Variant::ALL
            .into_iter()
            .find(|v| v.name() == s || v.id().eq_ignore_ascii_case(s))
    }

    /// Lottery ticket rule in the m2-production harness.
    pub fn ticket_rule(self) -> TicketRule {
        match self {
            Variant::Baseline | Variant::TimeDecay => TicketRule::Uniform,
            Variant::ValueAge => TicketRule::ValueAge,
            Variant::InverseFactor | Variant::InverseFactorTimeDecay => TicketRule::InverseFactor,
            Variant::TicketPrice => TicketRule::TicketPrice,
        }
    }

    /// Honest-wallet tag decay in the m2-production harness.
    pub fn tag_decay(self) -> TagDecay {
        match self {
            Variant::TimeDecay | Variant::InverseFactorTimeDecay => TagDecay::PerBlock,
            _ => TagDecay::PerHop,
        }
    }

    /// Whether the ticket weight is linear in output value.
    pub fn value_linear(self) -> bool {
        matches!(
            self.ticket_rule(),
            TicketRule::ValueAge | TicketRule::InverseFactor
        )
    }
}

/// The m2-production parameters of `variant` (20-year run, snapshots at 10
/// and 20 years unless overridden by the caller).
pub fn variant_params(
    variant: Variant,
    mining: MiningModel,
    whale: WhaleStrategy,
    seed: u64,
    tx_scale: f64,
    spent_eligible: bool,
) -> ProdParams {
    let mut p = ProdParams::years(20, &[10, 20]);
    p.mining = mining;
    p.whale = whale;
    p.seed = seed;
    p.tx_scale = tx_scale;
    p.ticket_rule = variant.ticket_rule();
    p.tag_decay = variant.tag_decay();
    p.spent_eligible = spent_eligible;
    p
}

// ===========================================================================
// Splitting grid (#1761 grid, 192 points)
// ===========================================================================

/// Strategic whale capital for the value-linear churn attack, BTH (the
/// m2-production strategic whale).
pub const CHURN_WHALE_CAPITAL_BTH: f64 = 5_000_000.0;
/// Mean value of an organic eligible output, BTH (value-linear rules only).
pub const ORGANIC_OUTPUT_VALUE_BTH: f64 = 10.0;

/// Per-value tilt of an output at `factor` under `ClusterWeighted`
/// (`max_factor - factor + 1`, in units of 1x).
pub fn inverse_factor_tilt(factor: u64) -> f64 {
    let f = factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
    (MAX_FACTOR_SCALED - f + FACTOR_SCALE) as f64 / FACTOR_SCALE as f64
}

/// The whale's outcome at one grid point under a variant.
#[derive(Clone, Copy, Debug, Default)]
pub struct SplitOutcome {
    /// Net per block (pico): winnings from manufactured tickets minus their
    /// cost. The whale abstains (net 0) when nothing is profitable.
    pub net: f64,
    /// Whale share of the draw at the optimum.
    pub share: f64,
    /// Manufactured outputs per block (uniform rules) or BTH re-spent per
    /// block (value-linear churn) at the optimum.
    pub effort: f64,
}

/// R3 at `k` whale outputs per block: uniform draw, every ticket (organic,
/// whale, coinbase and payout outputs) pays `p = R / T`, the expected payout
/// per ticket, burned. Fees and the cap are the current code's.
pub fn ticket_price_evaluate(k: f64, s: &Scenario) -> Option<SplitOutcome> {
    let (c, _) = whale_cost_per_output(k, s, Rules::CurrentCode)?;
    // Same pool as the current code (ticket prices are burned, not pooled).
    let o = evaluate(k, s, Rules::CurrentCode, Some(c))?;
    let free_po = LotteryDrawConfig::default().winners_per_draw as f64;
    let tickets = s.rho_o + k + COINBASE_TICKETS_PER_BLOCK + free_po;
    let price = o.payout / tickets;
    // The whale also owns its share of the payout tickets, and pays for them.
    let whale_tickets = k + o.share * free_po;
    let net = o.winnings - k * c - whale_tickets * price;
    Some(SplitOutcome {
        net,
        share: o.share,
        effort: k,
    })
}

/// Fee of one churn transaction re-spending `value_pico` (1 input, 2
/// outputs, the input one block old: the ring's oldest member is then about
/// [`MIN_DECOY_AGE_BLOCKS`] old, which sets the demurrage clock).
pub fn churn_tx_fee(value_pico: f64, s: &Scenario) -> f64 {
    let inputs = ConsensusFeeInputs {
        size: consensus_tx_size_bytes(1, 2, 0),
        n_out: 2,
        n_memo: 0,
        claimed: ClaimedClass::Factor(s.factor),
        floor: s.factor,
        elapsed: MIN_DECOY_AGE_BLOCKS,
        height: s.epoch.height(),
        output_sum: value_pico.min(u64::MAX as f64) as u64,
    };
    consensus_fee_breakdown(&inputs, &ConsensusFeeParams::mainnet(s.mode)).fee as f64
}

/// Value-linear rules (R1, R2, R5) at one grid point.
///
/// `spent_eligible = true` (the node): the whale re-spends `x` BTH of its
/// capital every block. The spent outputs stay candidates, so each block
/// adds `x` of fresh weight while the capital never leaves the whale. The age
/// term cancels: whale, organic and free outputs all spend the same
/// `L`-block life in the window. Shares are by value x tilt.
///
/// `spent_eligible = false` (#1790's rule, which needs a spend reveal):
/// splitting or churning adds no weight (the whale's weight is the value it
/// holds unspent, as for any holder), so manufactured outputs only cost.
pub fn value_linear_evaluate(
    x_bth: f64,
    s: &Scenario,
    variant: Variant,
    spent_eligible: bool,
    capital_bth: f64,
    organic_value_bth: f64,
) -> SplitOutcome {
    let x = x_bth.min(capital_bth) * PICO_PER_BTH as f64;
    let cost = if x > 0.0 { churn_tx_fee(x, s) } else { 0.0 };
    let reward = s.epoch.block_reward() as f64;
    let emission = s.epoch.emission_share() as f64;
    let organic_fees = s.rho_o / 2.0 * organic_tx_fee(s.epoch) as f64;
    // Window count for the cap: 2 churn outputs per block plus organic and
    // free outputs (same truncation as the code).
    let churn_outputs = if x > 0.0 { 2.0 } else { 0.0 };
    let free = COINBASE_TICKETS_PER_BLOCK + LotteryDrawConfig::default().winners_per_draw as f64;
    let eligible =
        (s.rho_o + churn_outputs + free) * super::lottery_reward_cap_faithful::ticket_life_blocks();
    let cap = code_reward_cap(eligible.round() as u64, s.epoch.block_reward()) as f64;
    let total_fees = (organic_fees + cost).min(u64::MAX as f64) as u64;
    let stored = (s.backlog_bth * PICO_PER_BTH as f64) as u128;
    let (_, payout) = code_pool_payout(total_fees, s.epoch.emission_share(), stored, cap as u64);
    let payout = payout as f64;

    let (g_whale, g_other) = match variant.ticket_rule() {
        TicketRule::InverseFactor => (
            inverse_factor_tilt(s.factor),
            inverse_factor_tilt(FACTOR_SCALE),
        ),
        _ => (1.0, 1.0),
    };
    let organic = s.rho_o * organic_value_bth * PICO_PER_BTH as f64 * g_other;
    let coinbase = (reward - emission) * g_other;
    let whale_w = if spent_eligible { x * g_whale } else { 0.0 };
    // Payout outputs are owned in proportion to the share, as in #1761.
    let share = if whale_w > 0.0 {
        whale_w / (whale_w + organic + coinbase)
    } else {
        0.0
    };
    SplitOutcome {
        net: share * payout - cost,
        share,
        effort: x_bth.min(capital_bth),
    }
}

/// Re-spend amounts searched (BTH): geometric from 1 BTH to the capital.
pub fn churn_grid(capital_bth: f64) -> Vec<f64> {
    let mut v = Vec::new();
    let mut x = 1.0f64;
    while x < capital_bth {
        v.push(x);
        x *= 1.25;
    }
    v.push(capital_bth);
    v
}

/// The whale's optimum at one grid point (abstain = net 0).
pub fn split_optimum(
    s: &Scenario,
    variant: Variant,
    spent_eligible: bool,
    capital_bth: f64,
    organic_value_bth: f64,
) -> SplitOutcome {
    match variant {
        // The current lottery (R4 changes only the tag decay).
        Variant::Baseline | Variant::TimeDecay => {
            let o = optimum(s, Rules::CurrentCode, None);
            SplitOutcome {
                net: o.net,
                share: o.share,
                effort: o.k,
            }
        }
        Variant::TicketPrice => {
            let mut best = SplitOutcome::default();
            for k in k_grid(s.capacity) {
                if let Some(o) = ticket_price_evaluate(k, s) {
                    if o.net > best.net {
                        best = o;
                    }
                }
            }
            best
        }
        Variant::ValueAge | Variant::InverseFactor | Variant::InverseFactorTimeDecay => {
            let mut best = SplitOutcome::default();
            for x in churn_grid(capital_bth) {
                let o = value_linear_evaluate(
                    x,
                    s,
                    variant,
                    spent_eligible,
                    capital_bth,
                    organic_value_bth,
                );
                if o.net > best.net {
                    best = o;
                }
            }
            best
        }
    }
}

/// One variant's row of the splitting table.
#[derive(Clone, Debug)]
pub struct SplitRow {
    pub variant: Variant,
    pub points: usize,
    /// Profitable points with spent outputs eligible (the node).
    pub profitable_node: usize,
    /// Profitable points under #1790's rule (spent outputs leave the draw).
    pub profitable_reveal: usize,
    /// Largest net per block over the grid, node candidate rule (pico).
    pub max_net_node: f64,
    /// Net per block at the #1761 reference point, node candidate rule.
    pub reference_net_node: f64,
    /// Whale share of the draw at the reference point, node candidate rule.
    pub reference_share_node: f64,
}

/// Evaluate every variant over the 192-point grid.
pub fn split_table(capital_bth: f64, organic_value_bth: f64) -> Vec<SplitRow> {
    let g = grid();
    Variant::ALL
        .into_iter()
        .map(|variant| {
            let mut row = SplitRow {
                variant,
                points: g.len(),
                profitable_node: 0,
                profitable_reveal: 0,
                max_net_node: 0.0,
                reference_net_node: 0.0,
                reference_share_node: 0.0,
            };
            for s in &g {
                let node = split_optimum(s, variant, true, capital_bth, organic_value_bth);
                let reveal = split_optimum(s, variant, false, capital_bth, organic_value_bth);
                if node.net > 0.0 {
                    row.profitable_node += 1;
                }
                if reveal.net > 0.0 {
                    row.profitable_reveal += 1;
                }
                row.max_net_node = row.max_net_node.max(node.net);
            }
            let r = split_optimum(
                &reference_scenario(),
                variant,
                true,
                capital_bth,
                organic_value_bth,
            );
            row.reference_net_node = r.net;
            row.reference_share_node = r.share;
            row
        })
        .collect()
}

/// Sensitivity of the value-linear churn result to the whale's capital and
/// the organic output value: profitable points out of 192 (R1).
pub fn churn_sensitivity() -> Vec<(f64, f64, usize, usize)> {
    let g = grid();
    let mut out = Vec::new();
    for capital in [1_000.0, 100_000.0, CHURN_WHALE_CAPITAL_BTH] {
        for organic in [ORGANIC_OUTPUT_VALUE_BTH, 1_000.0] {
            let r1 = g
                .iter()
                .filter(|s| split_optimum(s, Variant::ValueAge, true, capital, organic).net > 0.0)
                .count();
            let r2 = g
                .iter()
                .filter(|s| {
                    split_optimum(s, Variant::InverseFactor, true, capital, organic).net > 0.0
                })
                .count();
            out.push((capital, organic, r1, r2));
        }
    }
    out
}

// ===========================================================================
// Privacy measurements on the #1757 production routes
// ===========================================================================

/// Seed for the privacy measurements.
pub const PRIVACY_SEED: u64 = 0x1801_9817;
/// Rings per (route, real age) row.
pub const PRIVACY_RINGS: usize = 4_000;

/// Probability that an output wins at least once during its window life when
/// `rho_per_block` eligible outputs are created per block and 4 winners are
/// drawn per block: `1 - exp(-4 / rho)`.
pub fn win_probability(rho_per_block: f64) -> f64 {
    let w = LotteryDrawConfig::default().winners_per_draw as f64;
    1.0 - (-w / rho_per_block).exp()
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic uniform in [0, 1) for (seed, ring, member).
fn hash01(seed: u64, a: u64, b: u64) -> f64 {
    let h = splitmix64(seed ^ splitmix64(a ^ splitmix64(b)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Claim-reveal measurement: every lottery winner reveals its output's key
/// image to claim (the only way to keep spent outputs out of the draw without
/// a zero-knowledge non-membership proof). A fraction `q` of all outputs is
/// then known: if a ring's real input is one, the ring is deanonymized; a
/// revealed decoy is known not to be this ring's real input.
///
/// Returns (ID rate of a uniform guess over the surviving members, ID rate
/// of the Band-Center guess over the survivors, mean surviving members).
pub fn claim_reveal_rates(rings: &[RouteRingAges], q: f64, seed: u64) -> (f64, f64, f64) {
    if rings.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let (mut naive, mut band, mut size) = (0.0, 0.0, 0.0);
    for (i, ring) in rings.iter().enumerate() {
        if hash01(seed, i as u64, u64::MAX) < q {
            naive += 1.0;
            band += 1.0;
            size += 1.0;
            continue;
        }
        let survivors: Vec<u64> = ring
            .decoy_ages
            .iter()
            .enumerate()
            .filter(|(j, _)| hash01(seed, i as u64, *j as u64) >= q)
            .map(|(_, a)| *a)
            .collect();
        let n = survivors.len() + 1;
        naive += 1.0 / n as f64;
        size += n as f64;
        // Band-Center over the public ring ages, restricted to survivors.
        let all = ring
            .decoy_ages
            .iter()
            .chain(std::iter::once(&ring.real_age));
        let (lo, hi) = all.fold((u64::MAX, 0u64), |(lo, hi), &a| (lo.min(a), hi.max(a)));
        let mid = (lo as f64 + hi as f64) / 2.0;
        let d_real = (ring.real_age as f64 - mid).abs();
        let best = survivors
            .iter()
            .map(|&a| (a as f64 - mid).abs())
            .fold(f64::INFINITY, f64::min);
        if d_real < best - 1e-9 {
            band += 1.0;
        } else if (d_real - best).abs() <= 1e-9 {
            let ties = 1 + survivors
                .iter()
                .filter(|&&a| ((a as f64 - mid).abs() - best).abs() <= 1e-9)
                .count();
            band += 1.0 / ties as f64;
        }
    }
    let n = rings.len() as f64;
    (naive / n, band / n, size / n)
}

/// Decay-oracle measurement (R4/R5): if the time-decay allowance is keyed to
/// the real input's age, published output tags reveal that age to within
/// `resolution` blocks. Assume the cluster-fingerprint adversary is
/// neutralized (tag-matched decoys, the #1756 direction): the oracle guesses
/// uniformly among ring members whose age is within `resolution` of the
/// revealed one. Returns its ID rate. Keyed to the ring's youngest member
/// instead, the allowance is a function of public ring ages and reveals
/// nothing new.
pub fn decay_oracle_rate(rings: &[RouteRingAges], resolution: u64) -> f64 {
    if rings.is_empty() {
        return 0.0;
    }
    rings
        .iter()
        .map(|r| {
            let matches = r
                .decoy_ages
                .iter()
                .filter(|&&a| a.abs_diff(r.real_age) <= resolution)
                .count();
            1.0 / (matches + 1) as f64
        })
        .sum::<f64>()
        / rings.len() as f64
}

/// Fraction of rings whose real input is the strictly youngest member (what a
/// youngest-member-keyed allowance exposes, which the ring ages already do).
pub fn youngest_rate(rings: &[RouteRingAges]) -> f64 {
    if rings.is_empty() {
        return 0.0;
    }
    rings
        .iter()
        .filter(|r| r.decoy_ages.iter().all(|&a| a > r.real_age))
        .count() as f64
        / rings.len() as f64
}

/// Age resolution of the decay oracle, in blocks: one part per million of a
/// full-weight tag (the `TAG_WEIGHT_SCALE` quantum) at the
/// [`super::m2_production::TIME_DECAY_REF_BLOCKS`] decay rate.
pub fn decay_oracle_resolution_blocks(tag_weight: f64) -> u64 {
    let lambda = -(1.0 - super::m2_production::STOCK_WALLET_DECAY).ln()
        / super::m2_production::TIME_DECAY_REF_BLOCKS as f64;
    let quantum = 1.0 / bth_transaction_types::TAG_WEIGHT_SCALE as f64;
    (quantum / (tag_weight * lambda)).ceil() as u64
}

/// Real ages measured.
pub fn privacy_real_ages() -> [RealAgeSpec; 4] {
    [
        RealAgeSpec::Fixed(1_000),
        RealAgeSpec::Fixed(30_000),
        RealAgeSpec::Fixed(100_000),
        RealAgeSpec::GammaSpend,
    ]
}

/// One row of the privacy measurement.
#[derive(Clone, Debug)]
pub struct PrivacyRow {
    pub route: DecoyRoute,
    pub real_age: RealAgeSpec,
    pub rings: usize,
    /// (rho per block, q, naive ID, band-center ID, mean survivors).
    pub claim_reveal: Vec<(f64, f64, f64, f64, f64)>,
    /// Band-Center ID with nothing revealed (q = 0).
    pub band_center_base: f64,
    /// Decay oracle ID at full-weight and 1%-weight tag resolution.
    pub decay_oracle_full: f64,
    pub decay_oracle_weak: f64,
    pub youngest: f64,
}

/// Run the privacy measurements.
pub fn privacy_table() -> Vec<PrivacyRow> {
    let chain = ChainModel::default();
    let res_full = decay_oracle_resolution_blocks(1.0);
    let res_weak = decay_oracle_resolution_blocks(0.01);
    let mut out = Vec::new();
    for route in DecoyRoute::PRODUCTION {
        for spec in privacy_real_ages() {
            let rings = sample_route_ring_ages(&chain, route, spec, PRIVACY_RINGS, PRIVACY_SEED);
            let base = claim_reveal_rates(&rings, 0.0, PRIVACY_SEED);
            let claim_reveal = [5.0, 20.0, 200.0]
                .into_iter()
                .map(|rho| {
                    let q = win_probability(rho);
                    let (n, b, size) = claim_reveal_rates(&rings, q, PRIVACY_SEED);
                    (rho, q, n, b, size)
                })
                .collect();
            out.push(PrivacyRow {
                route,
                real_age: spec,
                rings: rings.len(),
                claim_reveal,
                band_center_base: base.1,
                decay_oracle_full: decay_oracle_rate(&rings, res_full),
                decay_oracle_weak: decay_oracle_rate(&rings, res_weak),
                youngest: youngest_rate(&rings),
            });
        }
    }
    out
}

// ===========================================================================
// Report
// ===========================================================================

fn bth(p: f64) -> f64 {
    p / PICO_PER_BTH as f64
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// Render the splitting-grid tables as Markdown.
pub fn splitting_markdown() -> String {
    let mut s = String::new();
    let rs = reference_scenario();
    s.push_str(&format!(
        "### Splitting grid (#1761 grid, {} points)\n\n\
         Whale optimum per grid point (4 epochs x 2 backlogs x 3 organic rates x 2 fee modes x \
         2 factors x 2 capacities). \"Node\" = spent outputs stay candidates (what the node \
         does). \"Reveal\" = spent outputs leave the draw (#1790's rule; needs the winner to \
         reveal its key image). Value-linear rules: the whale re-spends up to {:.0} BTH per \
         block; organic outputs are worth {:.0} BTH each. Reference point = epoch {}, \
         backlog {:.0}, rho_o {:.0}, floor+MIN_TX_FEE, {}x, proposer slot.\n\n",
        grid().len(),
        CHURN_WHALE_CAPITAL_BTH,
        ORGANIC_OUTPUT_VALUE_BTH,
        rs.epoch.label(),
        rs.backlog_bth,
        rs.rho_o,
        rs.factor / 1000,
    ));
    s.push_str("| variant | profitable (node) | profitable (reveal) | max net*/blk, node (BTH) | max net*/day, node (BTH) | reference net*/blk, node (BTH) | reference whale share, node |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for r in split_table(CHURN_WHALE_CAPITAL_BTH, ORGANIC_OUTPUT_VALUE_BTH) {
        s.push_str(&format!(
            "| {} {} | {}/{} | {}/{} | {:+.4} | {:+.0} | {:+.4} | {} |\n",
            r.variant.id(),
            r.variant.name(),
            r.profitable_node,
            r.points,
            r.profitable_reveal,
            r.points,
            bth(r.max_net_node),
            bth(r.max_net_node) * BLOCKS_PER_DAY,
            bth(r.reference_net_node),
            pct(r.reference_share_node),
        ));
    }
    s.push('\n');

    s.push_str("### Value-linear churn: sensitivity (node candidate rule)\n\n");
    s.push_str(
        "Profitable points out of 192 when the whale re-spends at most its capital per block.\n\n",
    );
    s.push_str(
        "| whale capital (BTH) | organic output value (BTH) | R1 value-age | R2 inverse-factor |\n",
    );
    s.push_str("|---:|---:|---:|---:|\n");
    for (capital, organic, r1, r2) in churn_sensitivity() {
        s.push_str(&format!(
            "| {:.0} | {:.0} | {}/192 | {}/192 |\n",
            capital, organic, r1, r2
        ));
    }
    s.push('\n');

    s.push_str("### R1 churn by epoch (node candidate rule, reference fee mode and capacity)\n\n");
    s.push_str("| epoch | rho_o | factor | re-spent/blk (BTH) | whale share | net*/blk (BTH) | net*/day (BTH) |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for epoch in super::lottery_reward_cap_faithful::Epoch::swept() {
        for rho_o in [20.0, 200.0] {
            for factor in [1000, 6000] {
                let sc = Scenario {
                    epoch,
                    rho_o,
                    factor,
                    ..rs
                };
                let o = split_optimum(
                    &sc,
                    Variant::ValueAge,
                    true,
                    CHURN_WHALE_CAPITAL_BTH,
                    ORGANIC_OUTPUT_VALUE_BTH,
                );
                s.push_str(&format!(
                    "| {} | {:.0} | {}x | {:.0} | {} | {:+.4} | {:+.0} |\n",
                    epoch.label(),
                    rho_o,
                    factor / 1000,
                    o.effort,
                    pct(o.share),
                    bth(o.net),
                    bth(o.net) * BLOCKS_PER_DAY,
                ));
            }
        }
    }
    s.push('\n');
    s
}

/// Render the privacy tables as Markdown.
pub fn privacy_markdown() -> String {
    let rows = privacy_table();
    let mut s = String::new();
    let chain = ChainModel::default();
    s.push_str(&format!(
        "### Claim-reveal: the cost of keeping spent outputs out of the draw\n\n\
         Production routes (#1757 ports), chain of 1 output/block to height {}, {} rings per \
         row, seed {:#x}. Every winner reveals its output's key image to claim. q = share of \
         outputs that ever win = 1 - exp(-4/rho), rho = eligible outputs created per block. \
         ID = how often the adversary's single guess is the real input (uniform = 5.0%).\n\n",
        chain.tip_height, PRIVACY_RINGS, PRIVACY_SEED
    ));
    s.push_str("| route | real age | Band-Center ID, no reveal | rho | q | survivors (of 20) | ID uniform-over-survivors | ID Band-Center-over-survivors |\n");
    s.push_str("|---|---|---:|---:|---:|---:|---:|---:|\n");
    for r in &rows {
        for (rho, q, naive, band, size) in &r.claim_reveal {
            s.push_str(&format!(
                "| {} | {} | {} | {:.0} | {} | {:.1} | {} | {} |\n",
                r.route.name(),
                r.real_age.label(),
                pct(r.band_center_base),
                rho,
                pct(*q),
                size,
                pct(*naive),
                pct(*band),
            ));
        }
    }
    s.push('\n');

    s.push_str(&format!(
        "### Decay oracle (R4/R5): allowance keyed to the real input's age\n\n\
         The published tag weight reveals the real input's age to within {} blocks for a \
         full-weight tag and {} blocks for a 1% tag (one ppm quantum at the R4 decay rate). \
         ID = uniform guess among members whose age matches. Keyed to the ring's youngest \
         member, the allowance reveals only that member's age, which is public; the last \
         column is how often the real input is that member.\n\n",
        decay_oracle_resolution_blocks(1.0),
        decay_oracle_resolution_blocks(0.01)
    ));
    s.push_str("| route | real age | rings | ID, full-weight tag | ID, 1% tag | real input is youngest |\n");
    s.push_str("|---|---|---:|---:|---:|---:|\n");
    for r in &rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            r.route.name(),
            r.real_age.label(),
            r.rings,
            pct(r.decay_oracle_full),
            pct(r.decay_oracle_weak),
            pct(r.youngest),
        ));
    }
    s.push('\n');
    s
}

/// Cohort lottery shares of one snapshot, keyed by cohort name.
pub type CohortShares = BTreeMap<&'static str, f64>;

/// Blocks per year, re-exported for the CLI.
pub const BLOCKS_PER_YEAR: u64 = PROD_BLOCKS_PER_YEAR;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::{
        lottery_reward_cap_faithful::remedy_table,
        m2_production::{run_m2_production, ProdParams},
    };

    /// R0's splitting row is #1761's current-code row: 192/192 profitable,
    /// and the same optimum at every point.
    #[test]
    fn baseline_reproduces_1761() {
        let table = split_table(CHURN_WHALE_CAPITAL_BTH, ORGANIC_OUTPUT_VALUE_BTH);
        let r0 = &table[0];
        assert_eq!(r0.variant, Variant::Baseline);
        let code = remedy_table()
            .into_iter()
            .find(|r| r.rules == Rules::CurrentCode)
            .unwrap();
        assert_eq!(r0.profitable_node, 192);
        assert_eq!(r0.profitable_reveal, 192);
        assert_eq!(code.profitable_points, 192);
        assert_eq!(r0.max_net_node.to_bits(), code.max_net.to_bits());
        assert_eq!(
            r0.reference_net_node.to_bits(),
            code.reference_net.to_bits()
        );
        // The published #1761 row: max +49.9975 BTH/blk, reference +2.4660.
        assert_eq!(format!("{:+.4}", bth(r0.max_net_node)), "+49.9975");
        assert_eq!(format!("{:+.4}", bth(r0.reference_net_node)), "+2.4660");
        for s in grid() {
            let a = split_optimum(&s, Variant::Baseline, true, 1.0, 1.0);
            assert_eq!(
                a.net.to_bits(),
                optimum(&s, Rules::CurrentCode, None).net.to_bits()
            );
        }
    }

    /// R3 prices every ticket at its expected payout, so the whale's
    /// winnings never exceed its ticket cost: net <= -fees at every point.
    #[test]
    fn ticket_price_is_split_neutral() {
        for s in grid() {
            for k in [1.0, 10.0, 200.0] {
                let o = ticket_price_evaluate(k, &s).unwrap();
                assert!(o.net <= 1e-3, "net {} at k {k}", o.net);
            }
            assert_eq!(
                split_optimum(&s, Variant::TicketPrice, true, 0.0, 0.0).net,
                0.0
            );
        }
    }

    /// Value-linear weights: with spent outputs out of the draw a whale
    /// cannot mint weight; with them in (the node), re-spending mints it.
    #[test]
    fn value_linear_needs_the_spend_reveal() {
        let s = reference_scenario();
        for v in [Variant::ValueAge, Variant::InverseFactor] {
            let reveal = split_optimum(&s, v, false, 5e6, 10.0);
            assert_eq!(reveal.net, 0.0);
            let node = split_optimum(&s, v, true, 5e6, 10.0);
            assert!(node.net > 0.0, "{v:?}");
        }
    }

    #[test]
    fn tilt_matches_cluster_weighted_kernel() {
        let t = crate::TagVector::new();
        for f in [1000u64, 2500, 6000] {
            let c = crate::LotteryCandidate::new([0; 36], 1_000_000, f, &t, 0);
            let w = c.weight(crate::SelectionMode::ClusterWeighted, 0) as f64 / 1e6;
            assert_eq!(w, inverse_factor_tilt(f) * FACTOR_SCALE as f64);
        }
    }

    #[test]
    fn win_probability_limits() {
        assert!((win_probability(4.0) - (1.0 - (-1.0f64).exp())).abs() < 1e-12);
        assert!(win_probability(200.0) < 0.02);
    }

    /// With nothing revealed the claim-reveal measurement is the uniform
    /// 1/20; with everything revealed every ring is deanonymized.
    #[test]
    fn claim_reveal_bounds() {
        let rings: Vec<RouteRingAges> = (0..50)
            .map(|i| RouteRingAges {
                real_age: 1000 + i,
                decoy_ages: (0..19).map(|j| 900 + 10 * j).collect(),
            })
            .collect();
        let (n0, _, s0) = claim_reveal_rates(&rings, 0.0, 1);
        assert!((n0 - 0.05).abs() < 1e-12);
        assert!((s0 - 20.0).abs() < 1e-12);
        let (n1, b1, _) = claim_reveal_rates(&rings, 1.0, 1);
        assert_eq!((n1, b1), (1.0, 1.0));
    }

    #[test]
    fn decay_oracle_resolution() {
        // One ppm of a full tag at 5% per 525,600 blocks is about 10 blocks.
        assert_eq!(decay_oracle_resolution_blocks(1.0), 11);
        let rings = vec![RouteRingAges {
            real_age: 30_000,
            decoy_ages: vec![29_990, 31_000, 28_000],
        }];
        assert_eq!(decay_oracle_rate(&rings, 11), 0.5);
        assert_eq!(decay_oracle_rate(&rings, 5), 1.0);
    }

    /// Each variant runs in the harness, and the non-baseline switches
    /// actually change the run (short horizon, epoch 1).
    #[test]
    fn variants_change_the_harness_run() {
        let base = |v: Variant| ProdParams {
            horizon_blocks: 20 * 17_280,
            snapshots: vec![20 * 17_280],
            start_height: PROD_BLOCKS_PER_YEAR,
            ..variant_params(
                v,
                MiningModel::Egalitarian,
                WhaleStrategy::Honest,
                1_696,
                100.0,
                false,
            )
        };
        let r0 = run_m2_production(&base(Variant::Baseline));
        for v in Variant::ALL.into_iter().skip(1) {
            let r = run_m2_production(&base(v));
            assert_ne!(
                r.snapshots[0].gini.to_bits(),
                r0.snapshots[0].gini.to_bits(),
                "{v:?} should differ from the baseline"
            );
        }
    }

    /// R3 wallets buy tickets only for outputs they expect to hold through
    /// the window under #1790's rule: at 1x every cohort does, at 100x none
    /// does. With spent outputs eligible (the node) everyone buys.
    #[test]
    fn ticket_price_buying_follows_holding_time() {
        let run = |scale: f64, spent: bool| {
            let p = ProdParams {
                horizon_blocks: 20 * 17_280,
                snapshots: vec![20 * 17_280],
                start_height: PROD_BLOCKS_PER_YEAR,
                ..variant_params(
                    Variant::TicketPrice,
                    MiningModel::Egalitarian,
                    WhaleStrategy::Honest,
                    1_696,
                    scale,
                    spent,
                )
            };
            run_m2_production(&p).snapshots[0].clone()
        };
        let one = run(1.0, false);
        assert!(one.ticket_burn_bth > 0.0);
        assert!(one.lottery_paid_bth > 0.0);
        let dense = run(100.0, false);
        assert_eq!(dense.ticket_burn_bth, 0.0);
        // Only the genesis outputs (free, in the first window) can still win.
        assert!(dense.lottery_paid_bth < 0.1 * one.lottery_paid_bth);
        assert!(run(100.0, true).ticket_burn_bth > 0.0);
    }
}
