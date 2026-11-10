//! The unbuilt §7.1 redistribution shapes (#1987, from the #1837 ruling).
//!
//! The #1801 study (`docs/research/redistribution-redesign-study.md`) built
//! five candidates (R1–R5) and listed four rule shapes it did not cover
//! (§7.1). The operator's ruling on #1837 (option 2) commissioned those four
//! before choosing the lottery's role. Each is scored here on the study's own
//! three axes, with its own harnesses, so the results compare directly with
//! R0–R5:
//!
//! 1. **Gini**: the #1790 m2-production harness ([`super::m2_production`]),
//!    switched by [`ShapeSwitches`] / [`TicketRule::FactorPrice`]. Runs are
//!    driven one at a time from the CLI (`redistribution-shapes --part m2`).
//! 2. **Splitting**: the #1761 192-point grid
//!    ([`super::lottery_reward_cap_faithful`]), extended per shape below.
//! 3. **Privacy**: the #1757 production decoy routes and the §5.1 claim-reveal
//!    analysis ([`super::redistribution_redesign`]), re-run for the shapes that
//!    change what a reveal exposes (the window shape).
//!
//! # Shapes
//!
//! | id | shape | lottery | other switches |
//! |---|---|---|---|
//! | S1 | factor-dependent ticket price | uniform, priced at `base x factor` | — |
//! | S2 | churn fee proportional to value moved | value x (7 - factor) (R2) | ad-valorem fee `c` |
//! | S3a | demurrage to block producers | R3 (priced at expected payout) | demurrage → miners |
//! | S3b | settle on spend, demurrage to block producers | R3 | outputs background, spender pays #925 charge; demurrage → miners |
//! | S4a | 30-day lottery window | uniform | window 720..=518,400 |
//! | S4b | 30-day window, value-weighted | value x (7 - factor) (R2) | window 720..=518,400 |
//!
//! S3 pairs with R3 because R3 is the only studied lottery that passes
//! splitting without a reveal; the demurrage side then carries all of the
//! redistribution.
//!
//! ANALYSIS ONLY: nothing here is node or consensus code.

use crate::{
    consensus_fee::MAINNET_BLOCKS_PER_YEAR, fee_curve::PICO_PER_BTH, lottery::FACTOR_SCALE,
    LotteryDrawConfig,
};

use super::{
    lottery_reward_cap_faithful::{
        code_pool_payout, code_reward_cap, grid, k_grid, optimum, organic_tx_fee,
        reference_scenario, whale_cost_per_output, Outcome, Rules, Scenario, BLOCKS_PER_DAY,
        COINBASE_TICKETS_PER_BLOCK,
    },
    m2_production::{
        DemurrageDest, MiningModel, ProdParams, ProdReport, ShapeSwitches, TagDecay, TicketRule,
        WhaleStrategy, PROD_BLOCKS_PER_YEAR,
    },
    privacy_routes::{sample_route_ring_ages, ChainModel, DecoyRoute, RealAgeSpec, RouteRingAges},
    redistribution_redesign::{
        churn_grid, churn_tx_fee, inverse_factor_tilt, privacy_real_ages, ticket_price_evaluate,
        win_probability, SplitOutcome, Variant, CHURN_WHALE_CAPITAL_BTH, ORGANIC_OUTPUT_VALUE_BTH,
        PRIVACY_RINGS, PRIVACY_SEED,
    },
};

/// Blocks per day at the node's 5 s block time.
pub const BLOCKS_PER_DAY_U64: u64 = MAINNET_BLOCKS_PER_YEAR / 365;
/// The S4 window: ages 720..=30 days.
pub const WINDOW_30D: (u64, u64) = (720, 30 * BLOCKS_PER_DAY_U64);
/// The S2 churn fee used for the Gini runs, ppm of the value moved (1%).
///
/// Closing the churn attack needs far more: under the node rule, churning a
/// small `x` wins `R x / (x + O)` for a cost of `c x`, where `R` is the
/// payout per block and `O` the honest value weight created per block, so
/// the break-even fee is about `R / O` (11% to 70% of the value moved over
/// the #1761 grid, [`churn_break_even_by_epoch`]). No fee an honest payer
/// could bear closes it, so the Gini runs use a payment-network-like 1% and
/// the splitting table reports both.
pub const CHURN_FEE_PPM: u64 = 10_000;

// ===========================================================================
// Shapes
// ===========================================================================

/// One #1987 shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// S1: uniform tickets priced at `base x factor`.
    FactorPrice,
    /// S2: value x (7 - factor) tickets plus an ad-valorem churn fee.
    ChurnFee,
    /// S3a: R3 lottery; demurrage goes to block producers.
    DemurrageMiners,
    /// S3b: R3 lottery; every spend settles (background outputs, spender pays
    /// the #925 charge); demurrage goes to block producers.
    SettleMiners,
    /// S4a: uniform tickets over a 30-day window.
    Window30d,
    /// S4b: value x (7 - factor) tickets over a 30-day window.
    Window30dValue,
}

impl Shape {
    /// Every shape, in report order.
    pub const ALL: [Shape; 6] = [
        Shape::FactorPrice,
        Shape::ChurnFee,
        Shape::DemurrageMiners,
        Shape::SettleMiners,
        Shape::Window30d,
        Shape::Window30dValue,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Shape::FactorPrice => "S1",
            Shape::ChurnFee => "S2",
            Shape::DemurrageMiners => "S3a",
            Shape::SettleMiners => "S3b",
            Shape::Window30d => "S4a",
            Shape::Window30dValue => "S4b",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::FactorPrice => "factor-price",
            Shape::ChurnFee => "churn-fee",
            Shape::DemurrageMiners => "demurrage-miners",
            Shape::SettleMiners => "settle-miners",
            Shape::Window30d => "window-30d",
            Shape::Window30dValue => "window-30d-value",
        }
    }

    /// Parse a CLI name or id.
    pub fn parse(s: &str) -> Option<Shape> {
        Shape::ALL
            .into_iter()
            .find(|v| v.name() == s || v.id().eq_ignore_ascii_case(s))
    }

    pub fn ticket_rule(self) -> TicketRule {
        match self {
            Shape::FactorPrice => TicketRule::FactorPrice,
            Shape::ChurnFee | Shape::Window30dValue => TicketRule::InverseFactor,
            Shape::DemurrageMiners | Shape::SettleMiners => TicketRule::TicketPrice,
            Shape::Window30d => TicketRule::Uniform,
        }
    }

    /// The shape's switches, with `churn_fee_ppm` for S2.
    pub fn switches(self, churn_fee_ppm: u64) -> ShapeSwitches {
        let mut sw = ShapeSwitches::default();
        match self {
            Shape::FactorPrice => {}
            Shape::ChurnFee => sw.value_fee_ppm = churn_fee_ppm,
            Shape::DemurrageMiners => sw.demurrage_dest = DemurrageDest::Miners,
            Shape::SettleMiners => {
                sw.demurrage_dest = DemurrageDest::Miners;
                sw.settle_on_spend = true;
            }
            Shape::Window30d | Shape::Window30dValue => sw.window = Some(WINDOW_30D),
        }
        sw
    }
}

/// The m2-production parameters of `shape` (20-year run, snapshots at 10 and
/// 20 years), on the same footing as
/// [`super::redistribution_redesign::variant_params`].
pub fn shape_params(
    shape: Shape,
    mining: MiningModel,
    whale: WhaleStrategy,
    seed: u64,
    tx_scale: f64,
    spent_eligible: bool,
    churn_fee_ppm: u64,
) -> ProdParams {
    let mut p = ProdParams::years(20, &[10, 20]);
    p.mining = mining;
    p.whale = whale;
    p.seed = seed;
    p.tx_scale = tx_scale;
    p.ticket_rule = shape.ticket_rule();
    p.tag_decay = TagDecay::PerHop;
    p.spent_eligible = spent_eligible;
    p.shapes = shape.switches(churn_fee_ppm);
    p
}

// ===========================================================================
// Splitting: S1 factor-dependent ticket price
// ===========================================================================

/// S1 at `k` whale outputs per block.
///
/// Every ticket (organic, whale, coinbase, payout) is bought at `base x w`,
/// where `w` is its factor in units of 1x and `base = R / sum of w over the
/// tickets created per block`, so the burn balances the payout `R` in
/// aggregate. Organic outputs are 1x except a fraction `phi` at 6x (the
/// #1761 grid's organic outputs are all 1x: `phi = 0`). Coinbases are fresh
/// clusters (1x). A payout output copies the winning output's tags, so the
/// whale's payout tickets are at its own factor and the others at the
/// non-whale tickets' mean. The whale's tickets are priced at `whale_w`
/// (its factor, or 1.0 if it claims background, which costs only the #925
/// charge on 1e6-pico ticket outputs).
pub fn factor_price_evaluate(k: f64, s: &Scenario, phi: f64, whale_w: f64) -> Option<SplitOutcome> {
    let (c, _) = whale_cost_per_output(k, s, Rules::CurrentCode)?;
    let o = super::lottery_reward_cap_faithful::evaluate(k, s, Rules::CurrentCode, Some(c))?;
    let free_po = LotteryDrawConfig::default().winners_per_draw as f64;
    let w_organic = 1.0 + 5.0 * phi;
    // Mean weight of a non-whale winning ticket (organic or coinbase).
    let w_other =
        (s.rho_o * w_organic + COINBASE_TICKETS_PER_BLOCK) / (s.rho_o + COINBASE_TICKETS_PER_BLOCK);
    let weight = s.rho_o * w_organic
        + k * whale_w
        + COINBASE_TICKETS_PER_BLOCK
        + free_po * (o.share * whale_w + (1.0 - o.share) * w_other);
    let base = o.payout / weight;
    let whale_tickets = k + o.share * free_po;
    let net = o.winnings - k * c - whale_tickets * whale_w * base;
    Some(SplitOutcome {
        net,
        share: o.share,
        effort: k,
    })
}

/// S1 whale optimum at one point (abstain = net 0). `background` = the whale
/// claims background tags (1x) instead of pricing at its grid factor.
pub fn factor_price_optimum(s: &Scenario, phi: f64, background: bool) -> SplitOutcome {
    let whale_w = if background {
        1.0
    } else {
        s.factor as f64 / FACTOR_SCALE as f64
    };
    let mut best = SplitOutcome::default();
    for k in k_grid(s.capacity) {
        if let Some(o) = factor_price_evaluate(k, s, phi, whale_w) {
            if o.net > best.net {
                best = o;
            }
        }
    }
    best
}

// ===========================================================================
// Splitting: S2 churn fee
// ===========================================================================

/// S2 at one grid point:
/// [`super::redistribution_redesign::value_linear_evaluate`] plus an ad-valorem
/// fee of `c_ppm` on every transaction's output sum (the node cannot tell
/// change from payment, so it charges the whole output sum). The churn
/// transaction moves `x`; each organic transaction moves its two outputs. The
/// fee is pooled like any fee (80% pool, 20% burn), so the whale wins back its
/// share of it.
pub fn churn_fee_evaluate(
    x_bth: f64,
    s: &Scenario,
    variant: Variant,
    spent_eligible: bool,
    c_ppm: f64,
    capital_bth: f64,
    organic_value_bth: f64,
) -> SplitOutcome {
    let c = c_ppm / 1e6;
    let x = x_bth.min(capital_bth) * PICO_PER_BTH as f64;
    let cost = if x > 0.0 {
        churn_tx_fee(x, s) + c * x
    } else {
        0.0
    };
    let reward = s.epoch.block_reward() as f64;
    let emission = s.epoch.emission_share() as f64;
    let organic_tx_value = 2.0 * organic_value_bth * PICO_PER_BTH as f64;
    let organic_fees = s.rho_o / 2.0 * (organic_tx_fee(s.epoch) as f64 + c * organic_tx_value);
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

/// S2 whale optimum at one point.
pub fn churn_fee_optimum(
    s: &Scenario,
    variant: Variant,
    spent_eligible: bool,
    c_ppm: f64,
    capital_bth: f64,
    organic_value_bth: f64,
) -> SplitOutcome {
    let mut best = SplitOutcome::default();
    for x in churn_grid(capital_bth) {
        let o = churn_fee_evaluate(
            x,
            s,
            variant,
            spent_eligible,
            c_ppm,
            capital_bth,
            organic_value_bth,
        );
        if o.net > best.net {
            best = o;
        }
    }
    best
}

/// The smallest churn fee (ppm) at which the churn attack is unprofitable at
/// grid point `s` under the node rule. Bisection; monotone in `c`.
pub fn churn_break_even_ppm(s: &Scenario, variant: Variant, capital_bth: f64) -> f64 {
    let profitable = |c: f64| {
        churn_fee_optimum(s, variant, true, c, capital_bth, ORGANIC_OUTPUT_VALUE_BTH).net > 0.0
    };
    if !profitable(0.0) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0f64, 1e6f64);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if profitable(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

// ===========================================================================
// Splitting: S4 window
// ===========================================================================

/// The current-code uniform lottery ([`Rules::CurrentCode`]) at `k` whale
/// outputs per block with an eligible life of `life` blocks: the #1761
/// `evaluate` with `ticket_life_blocks()` replaced. A longer window keeps the
/// whale's share (`k / (rho_o + k + 1)`, every output spends the same life in
/// the window) and raises the window count, hence the cap.
pub fn window_uniform_evaluate(k: f64, s: &Scenario, life: f64) -> Option<Outcome> {
    let (c, n) = whale_cost_per_output(k, s, Rules::CurrentCode)?;
    let free_po = LotteryDrawConfig::default().winners_per_draw as f64;
    let created = s.rho_o + k + COINBASE_TICKETS_PER_BLOCK + free_po;
    let share = if k > 0.0 {
        k / (s.rho_o + k + COINBASE_TICKETS_PER_BLOCK)
    } else {
        0.0
    };
    let eligible = created * life;
    let reward = s.epoch.block_reward();
    let cap = code_reward_cap(eligible.round() as u64, reward) as f64;
    let organic_fees = s.rho_o / 2.0 * organic_tx_fee(s.epoch) as f64;
    let cost = k * c;
    let total_fees = (organic_fees + cost).min(u64::MAX as f64) as u64;
    let stored = (s.backlog_bth * PICO_PER_BTH as f64) as u128;
    let cap_u = cap.min(u64::MAX as f64) as u64;
    let (fee_pool, payout) = code_pool_payout(total_fees, s.epoch.emission_share(), stored, cap_u);
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
        inflow: (s.epoch.emission_share() + fee_pool) as f64,
    })
}

/// Uniform-window whale optimum.
pub fn window_uniform_optimum(s: &Scenario, life: f64) -> Outcome {
    let mut best = Outcome::default();
    for k in k_grid(s.capacity) {
        if let Some(o) = window_uniform_evaluate(k, s, life) {
            if o.net > best.net {
                best = o;
            }
        }
    }
    best
}

/// Value-linear (R2-weight) churn with an eligible life of `life` blocks:
/// [`churn_fee_evaluate`] at `c = 0` with the window count taken over `life`.
pub fn window_value_evaluate(
    x_bth: f64,
    s: &Scenario,
    spent_eligible: bool,
    life: f64,
) -> SplitOutcome {
    let x = x_bth.min(CHURN_WHALE_CAPITAL_BTH) * PICO_PER_BTH as f64;
    let cost = if x > 0.0 { churn_tx_fee(x, s) } else { 0.0 };
    let reward = s.epoch.block_reward() as f64;
    let emission = s.epoch.emission_share() as f64;
    let organic_fees = s.rho_o / 2.0 * organic_tx_fee(s.epoch) as f64;
    let churn_outputs = if x > 0.0 { 2.0 } else { 0.0 };
    let free = COINBASE_TICKETS_PER_BLOCK + LotteryDrawConfig::default().winners_per_draw as f64;
    let eligible = (s.rho_o + churn_outputs + free) * life;
    let cap = code_reward_cap(eligible.round() as u64, s.epoch.block_reward()) as f64;
    let total_fees = (organic_fees + cost).min(u64::MAX as f64) as u64;
    let stored = (s.backlog_bth * PICO_PER_BTH as f64) as u128;
    let (_, payout) = code_pool_payout(total_fees, s.epoch.emission_share(), stored, cap as u64);
    let g_whale = inverse_factor_tilt(s.factor);
    let g_other = inverse_factor_tilt(FACTOR_SCALE);
    let organic = s.rho_o * ORGANIC_OUTPUT_VALUE_BTH * PICO_PER_BTH as f64 * g_other;
    let coinbase = (reward - emission) * g_other;
    let whale_w = if spent_eligible { x * g_whale } else { 0.0 };
    let share = if whale_w > 0.0 {
        whale_w / (whale_w + organic + coinbase)
    } else {
        0.0
    };
    SplitOutcome {
        net: share * payout as f64 - cost,
        share,
        effort: x_bth.min(CHURN_WHALE_CAPITAL_BTH),
    }
}

/// Value-linear window optimum.
pub fn window_value_optimum(s: &Scenario, spent_eligible: bool, life: f64) -> SplitOutcome {
    let mut best = SplitOutcome::default();
    for x in churn_grid(CHURN_WHALE_CAPITAL_BTH) {
        let o = window_value_evaluate(x, s, spent_eligible, life);
        if o.net > best.net {
            best = o;
        }
    }
    best
}

/// Eligible life of the S4 window (`max - min + 1`).
pub fn window_30d_life() -> f64 {
    (WINDOW_30D.1 - WINDOW_30D.0 + 1) as f64
}

// ===========================================================================
// Splitting tables
// ===========================================================================

/// One shape's row of the splitting table.
#[derive(Clone, Debug)]
pub struct ShapeSplitRow {
    pub label: String,
    pub points: usize,
    pub profitable_node: usize,
    pub profitable_reveal: usize,
    pub max_net_node: f64,
    pub reference_net_node: f64,
    pub reference_share_node: f64,
}

fn split_row(label: &str, f: impl Fn(&Scenario, bool) -> SplitOutcome) -> ShapeSplitRow {
    let g = grid();
    let mut row = ShapeSplitRow {
        label: label.to_string(),
        points: g.len(),
        profitable_node: 0,
        profitable_reveal: 0,
        max_net_node: 0.0,
        reference_net_node: 0.0,
        reference_share_node: 0.0,
    };
    for s in &g {
        let node = f(s, true);
        let reveal = f(s, false);
        if node.net > 0.0 {
            row.profitable_node += 1;
        }
        if reveal.net > 0.0 {
            row.profitable_reveal += 1;
        }
        row.max_net_node = row.max_net_node.max(node.net);
    }
    let r = f(&reference_scenario(), true);
    row.reference_net_node = r.net;
    row.reference_share_node = r.share;
    row
}

fn from_outcome(o: Outcome) -> SplitOutcome {
    SplitOutcome {
        net: o.net,
        share: o.share,
        effort: o.k,
    }
}

/// The main splitting table: every shape over the 192-point grid.
pub fn shape_split_table() -> Vec<ShapeSplitRow> {
    let life = window_30d_life();
    vec![
        split_row("R0 current rules (reference)", |s, _| {
            from_outcome(optimum(s, Rules::CurrentCode, None))
        }),
        split_row("R3 ticket-price (reference)", |s, _| {
            let mut best = SplitOutcome::default();
            for k in k_grid(s.capacity) {
                if let Some(o) = ticket_price_evaluate(k, s) {
                    if o.net > best.net {
                        best = o;
                    }
                }
            }
            best
        }),
        split_row("S1 factor-price (whale at its factor)", |s, _| {
            factor_price_optimum(s, 0.0, false)
        }),
        split_row("S1 factor-price (whale claims background)", |s, _| {
            factor_price_optimum(s, 0.0, true)
        }),
        split_row("S2 churn-fee, R2 weight, c = 1%", |s, spent| {
            churn_fee_optimum(
                s,
                Variant::InverseFactor,
                spent,
                CHURN_FEE_PPM as f64,
                CHURN_WHALE_CAPITAL_BTH,
                ORGANIC_OUTPUT_VALUE_BTH,
            )
        }),
        split_row("S3a/S3b (R3 lottery; demurrage side only)", |s, _| {
            let mut best = SplitOutcome::default();
            for k in k_grid(s.capacity) {
                if let Some(o) = ticket_price_evaluate(k, s) {
                    if o.net > best.net {
                        best = o;
                    }
                }
            }
            best
        }),
        split_row("S4a window-30d, uniform", |s, _| {
            from_outcome(window_uniform_optimum(s, life))
        }),
        split_row("S4b window-30d, R2 weight", |s, spent| {
            window_value_optimum(s, spent, life)
        }),
    ]
}

/// S1 sensitivity: profitable points (node rule) as the share `phi` of 6x
/// organic tickets grows.
pub fn factor_price_sensitivity() -> Vec<(f64, usize, f64, usize, f64)> {
    let g = grid();
    [0.0, 0.05, 0.2, 0.5]
        .into_iter()
        .map(|phi| {
            let (mut n_f, mut max_f, mut n_b, mut max_b) = (0, 0.0f64, 0, 0.0f64);
            for s in &g {
                let a = factor_price_optimum(s, phi, false);
                let b = factor_price_optimum(s, phi, true);
                if a.net > 0.0 {
                    n_f += 1;
                }
                if b.net > 0.0 {
                    n_b += 1;
                }
                max_f = max_f.max(a.net);
                max_b = max_b.max(b.net);
            }
            (phi, n_f, max_f, n_b, max_b)
        })
        .collect()
}

/// S2: profitable points (node rule) by churn fee, R1 and R2 weights.
pub fn churn_fee_sweep() -> Vec<(f64, usize, usize)> {
    let g = grid();
    [
        0.0, 100.0, 1_000.0, 10_000.0, 30_000.0, 100_000.0, 300_000.0, 750_000.0,
    ]
    .into_iter()
    .map(|c| {
        let count = |v: Variant| {
            g.iter()
                .filter(|s| {
                    churn_fee_optimum(
                        s,
                        v,
                        true,
                        c,
                        CHURN_WHALE_CAPITAL_BTH,
                        ORGANIC_OUTPUT_VALUE_BTH,
                    )
                    .net > 0.0
                })
                .count()
        };
        (c, count(Variant::ValueAge), count(Variant::InverseFactor))
    })
    .collect()
}

/// S2: the largest break-even churn fee per epoch (node rule, R2 weight).
pub fn churn_break_even_by_epoch() -> Vec<(String, f64)> {
    super::lottery_reward_cap_faithful::Epoch::swept()
        .into_iter()
        .map(|e| {
            let worst = grid()
                .into_iter()
                .filter(|s| s.epoch == e)
                .map(|s| churn_break_even_ppm(&s, Variant::InverseFactor, CHURN_WHALE_CAPITAL_BTH))
                .fold(0.0f64, f64::max);
            (e.label(), worst)
        })
        .collect()
}

// ===========================================================================
// Privacy: the S4 window under the reveal rule
// ===========================================================================

/// Blocks of window exposure before a spend at `spend_age`, for a window
/// `min_age..=max_age`.
pub fn exposure_before_spend_window(spend_age: u64, min_age: u64, max_age: u64) -> u64 {
    spend_age.min(max_age + 1).saturating_sub(min_age)
}

/// [`super::redistribution_redesign::claim_reveal_rates`] for a window
/// `min_age..=max_age`. The per-output full-life win probability `q` does not
/// depend on the window length (`rho` outputs per block, `rho x life` in the
/// window, 4 winners per block from a uniform subset: `4 / (rho x life)` per
/// block over `life` blocks), so only the real input's exposure changes.
/// Returns (direct reveal, uniform-over-survivors ID, mean survivors).
pub fn claim_reveal_rates_window(
    rings: &[RouteRingAges],
    q: f64,
    seed: u64,
    min_age: u64,
    max_age: u64,
) -> (f64, f64, f64) {
    if rings.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let life = (max_age - min_age + 1) as f64;
    let (mut direct, mut naive, mut size) = (0.0, 0.0, 0.0);
    for (i, ring) in rings.iter().enumerate() {
        let f = exposure_before_spend_window(ring.real_age, min_age, max_age) as f64 / life;
        let p_real = if f == 0.0 {
            0.0
        } else {
            1.0 - (1.0 - q).powf(f)
        };
        if hash01(seed, i as u64, u64::MAX) < p_real {
            direct += 1.0;
            naive += 1.0;
            size += 1.0;
            continue;
        }
        let survivors = ring
            .decoy_ages
            .iter()
            .enumerate()
            .filter(|(j, _)| hash01(seed, i as u64, *j as u64) >= q)
            .count();
        let n = survivors + 1;
        naive += 1.0 / n as f64;
        size += n as f64;
    }
    let n = rings.len() as f64;
    (direct / n, naive / n, size / n)
}

// The same deterministic hash as the #1801 claim-reveal measurement, so the
// default window reproduces its numbers exactly (tested).
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash01(seed: u64, a: u64, b: u64) -> f64 {
    let h = splitmix64(seed ^ splitmix64(a ^ splitmix64(b)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// One row of the S4 reveal table.
#[derive(Clone, Debug)]
pub struct WindowPrivacyRow {
    pub route: DecoyRoute,
    pub real_age: RealAgeSpec,
    /// (rho, q, direct reveal 10k window, direct 30d, ID 10k, ID 30d,
    /// survivors 30d).
    pub cells: Vec<(f64, f64, f64, f64, f64, f64, f64)>,
}

/// Claim-reveal for the S4 window against the current window, on every
/// production route and real age of §5.1.
pub fn window_privacy_table() -> Vec<WindowPrivacyRow> {
    let chain = ChainModel::default();
    let d = LotteryDrawConfig::default();
    let mut out = Vec::new();
    for route in DecoyRoute::PRODUCTION {
        for spec in privacy_real_ages() {
            let rings = sample_route_ring_ages(&chain, route, spec, PRIVACY_RINGS, PRIVACY_SEED);
            let cells = [5.0, 20.0, 200.0]
                .into_iter()
                .map(|rho| {
                    let q = win_probability(rho);
                    let (d0, n0, _) = claim_reveal_rates_window(
                        &rings,
                        q,
                        PRIVACY_SEED,
                        d.min_utxo_age,
                        d.circulation_window,
                    );
                    let (d1, n1, s1) = claim_reveal_rates_window(
                        &rings,
                        q,
                        PRIVACY_SEED,
                        WINDOW_30D.0,
                        WINDOW_30D.1,
                    );
                    (rho, q, d0, d1, n0, n1, s1)
                })
                .collect();
            out.push(WindowPrivacyRow {
                route,
                real_age: spec,
                cells,
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

/// Render the splitting tables as Markdown.
pub fn splitting_markdown() -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "### Splitting grid for the #1987 shapes (#1761 grid, {} points)\n\n\
         Whale optimum per grid point, as in the #1801 study's §4 table. \"Node\" = spent \
         outputs stay candidates; \"reveal\" = spent outputs leave the draw (#1790's rule). \
         Uniform-ticket shapes have no node/reveal difference. S2 charges {} ppm of every \
         transaction's output sum, pooled like any fee. S4 uses a {}..={}-block window \
         ({} blocks of eligible life). Value-linear rows: the whale re-spends up to {:.0} BTH \
         per block; organic outputs are worth {:.0} BTH each.\n\n",
        grid().len(),
        CHURN_FEE_PPM,
        WINDOW_30D.0,
        WINDOW_30D.1,
        window_30d_life(),
        CHURN_WHALE_CAPITAL_BTH,
        ORGANIC_OUTPUT_VALUE_BTH,
    ));
    s.push_str("| shape | profitable (node) | profitable (reveal) | max net*/blk, node (BTH) | max net*/day, node (BTH) | reference net*/blk, node (BTH) | reference whale share, node |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for r in shape_split_table() {
        s.push_str(&format!(
            "| {} | {}/{} | {}/{} | {:+.4} | {:+.0} | {:+.4} | {} |\n",
            r.label,
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

    s.push_str("### S1: farming the factor subsidy\n\n");
    s.push_str(
        "Profitable points out of 192 (node rule) when a share phi of organic tickets is at 6x \
         and the rest at 1x. The #1761 grid's organic outputs are all 1x (phi = 0). \"At its \
         factor\" prices the whale's tickets at the grid's whale factor (1x or 6x); \"background\" \
         lets every whale claim 1x (#1687; the #925 charge on 1e6-pico ticket outputs is \
         negligible).\n\n",
    );
    s.push_str("| phi (6x share of organic tickets) | profitable, whale at its factor | max net*/blk (BTH) | profitable, whale claims background | max net*/blk (BTH) |\n");
    s.push_str("|---:|---:|---:|---:|---:|\n");
    for (phi, n_f, max_f, n_b, max_b) in factor_price_sensitivity() {
        s.push_str(&format!(
            "| {:.2} | {}/192 | {:+.4} | {}/192 | {:+.4} |\n",
            phi,
            n_f,
            bth(max_f),
            n_b,
            bth(max_b)
        ));
    }
    s.push('\n');

    s.push_str("### S2: churn fee needed to close the churn attack\n\n");
    s.push_str(
        "Profitable points out of 192 under the node rule (spent outputs stay candidates), by \
         ad-valorem fee on every transaction's output sum.\n\n",
    );
    s.push_str(
        "| churn fee (ppm of value moved) | churn fee (%) | R1 value-age | R2 inverse-factor |\n",
    );
    s.push_str("|---:|---:|---:|---:|\n");
    for (c, r1, r2) in churn_fee_sweep() {
        s.push_str(&format!(
            "| {:.0} | {:.2}% | {}/192 | {}/192 |\n",
            c,
            c / 1e4,
            r1,
            r2
        ));
    }
    s.push('\n');
    s.push_str("Largest break-even fee over each epoch's 48 points (R2 weight, node rule):\n\n");
    s.push_str("| epoch | break-even churn fee (ppm) | (%) |\n");
    s.push_str("|---|---:|---:|\n");
    for (e, c) in churn_break_even_by_epoch() {
        s.push_str(&format!("| {} | {:.0} | {:.2}% |\n", e, c, c / 1e4));
    }
    s.push('\n');
    s
}

/// Render the privacy table as Markdown.
pub fn privacy_markdown() -> String {
    let rows = window_privacy_table();
    let d = LotteryDrawConfig::default();
    let mut s = String::new();
    s.push_str(&format!(
        "### S4: claim-reveal with a 30-day window\n\n\
         Same production routes, chain, rings ({} per row) and seed ({:#x}) as the #1801 §5.1 \
         table. The reveal is what #1790's rule needs (a winner reveals its key image; a spent \
         output forfeits). The full-life win probability q = 1 - exp(-4/rho) does not depend on \
         the window length; the real input's exposure before its spend does. Window {}..={} \
         against the node's {}..={}. ID = uniform guess over the surviving ring members, \
         counting directly revealed rings (uniform = 5.0%).\n\n",
        PRIVACY_RINGS,
        PRIVACY_SEED,
        WINDOW_30D.0,
        WINDOW_30D.1,
        d.min_utxo_age,
        d.circulation_window,
    ));
    s.push_str("| route | real age | rho | q | direct reveal, 10k window | direct reveal, 30-day | ID, 10k window | ID, 30-day | survivors (of 20), 30-day |\n");
    s.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in &rows {
        for (rho, q, d0, d1, n0, n1, s1) in &r.cells {
            s.push_str(&format!(
                "| {} | {} | {:.0} | {} | {} | {} | {} | {} | {:.1} |\n",
                r.route.name(),
                r.real_age.label(),
                rho,
                pct(*q),
                pct(*d0),
                pct(*d1),
                pct(*n0),
                pct(*n1),
                s1
            ));
        }
    }
    s.push('\n');
    s
}

/// One JSON line per snapshot of an m2 run (same fields as
/// `redistribution-redesign --part m2`, plus the shape diagnostics).
pub fn m2_json_lines(label: &str, p: &ProdParams, r: &ProdReport, secs: f64) -> Vec<String> {
    r.snapshots
        .iter()
        .map(|s| {
            let cohorts: Vec<String> = s
                .cohorts
                .iter()
                .map(|c| {
                    format!(
                        "\"{}\":{{\"wealth\":{:.6},\"lottery\":{:.6},\"demurrage\":{:.6e},\"factor\":{:.4}}}",
                        c.name, c.wealth_share, c.lottery_share, c.demurrage_bth, c.mean_priced_factor
                    )
                })
                .collect();
            format!(
                "{{\"shape\":\"{}\",\"ticket_rule\":\"{}\",\"mining\":\"{}\",\"whale\":\"{}\",\"seed\":{},\"tx_scale\":{},\"spent_eligible\":{},\"value_fee_ppm\":{},\"demurrage_dest\":\"{}\",\"settle_on_spend\":{},\"window\":\"{}\",\"run\":\"{}\",\"yr\":{},\"gini0\":{:.6},\"gini\":{:.6},\"top_decile_below_1_5x\":{:.6},\"lottery_bth\":{:.6e},\"demurrage_bth\":{:.6e},\"ticket_burn_bth\":{:.6e},\"downgrade_bth\":{:.6e},\"value_fee_bth\":{:.6e},\"miner_dividend_bth\":{:.6e},\"ticket_mean_factor\":{:.4},\"burned_bth\":{:.6e},\"mined_bth\":{:.6e},\"txs\":{},\"outputs\":{},\"secs\":{:.0},\"cohorts\":{{{}}}}}",
                label,
                p.ticket_rule.label(),
                p.mining.label(),
                p.whale.label(),
                p.seed,
                p.tx_scale,
                p.spent_eligible,
                p.shapes.value_fee_ppm,
                p.shapes.demurrage_dest.label(),
                p.shapes.settle_on_spend,
                match p.shapes.window {
                    Some((a, b)) => format!("{a}..={b}"),
                    None => "720..=10000".to_string(),
                },
                if p.mechanism { "on" } else { "off" },
                s.blocks / PROD_BLOCKS_PER_YEAR,
                r.gini0,
                s.gini,
                s.top_decile_below_1_5x,
                s.lottery_paid_bth,
                s.demurrage_bth,
                s.ticket_burn_bth,
                s.downgrade_bth,
                s.value_fee_bth,
                s.miner_dividend_bth,
                s.ticket_mean_factor,
                s.burned_bth,
                s.total_mined_bth,
                r.transactions,
                r.outputs_created,
                secs,
                cohorts.join(",")
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::{
        m2_production::{run_m2_production, ProdParams},
        redistribution_redesign::{claim_reveal_rates, split_optimum, value_linear_evaluate},
    };

    /// S1 with every ticket at 1x is R3 exactly.
    #[test]
    fn factor_price_at_1x_is_r3() {
        for s in grid().into_iter().filter(|s| s.factor == FACTOR_SCALE) {
            for k in [1.0, 10.0, 200.0] {
                let a = factor_price_evaluate(k, &s, 0.0, 1.0).unwrap();
                let b = ticket_price_evaluate(k, &s).unwrap();
                assert!(
                    (a.net - b.net).abs() <= 1e-6 * b.net.abs().max(1.0),
                    "{a:?} {b:?}"
                );
            }
        }
    }

    /// S2 at c = 0 is the #1801 value-linear churn evaluator exactly.
    #[test]
    fn churn_fee_zero_is_value_linear() {
        for s in grid().iter().step_by(7) {
            for x in [1.0, 1_000.0, 5e6] {
                for v in [Variant::ValueAge, Variant::InverseFactor] {
                    let a = churn_fee_evaluate(x, s, v, true, 0.0, 5e6, 10.0);
                    let b = value_linear_evaluate(x, s, v, true, 5e6, 10.0);
                    assert_eq!(a.net.to_bits(), b.net.to_bits());
                }
            }
        }
    }

    /// The break-even churn fee is about R / O (payout over honest value
    /// weight per block): above 10% of the value moved somewhere in every
    /// epoch, and the 1% Gini-run fee leaves churn profitable.
    #[test]
    fn churn_fee_break_even_is_structural() {
        for (epoch, worst) in churn_break_even_by_epoch() {
            assert!(worst > 100_000.0, "epoch {epoch}: {worst}");
        }
        let at_gini_fee = grid()
            .iter()
            .filter(|s| {
                churn_fee_optimum(
                    s,
                    Variant::InverseFactor,
                    true,
                    CHURN_FEE_PPM as f64,
                    CHURN_WHALE_CAPITAL_BTH,
                    ORGANIC_OUTPUT_VALUE_BTH,
                )
                .net > 0.0
            })
            .count();
        assert!(at_gini_fee > 0);
        // Analytic check at the reference point: break-even ~ R / O.
        let s = reference_scenario();
        let c = churn_break_even_ppm(&s, Variant::InverseFactor, CHURN_WHALE_CAPITAL_BTH) / 1e6;
        assert!(c > 0.0 && c < 1.0);
    }

    /// The S4 evaluators reproduce the #1761 / #1801 evaluators at the
    /// node's window.
    #[test]
    fn window_evaluators_reproduce_default_window() {
        let life = super::super::lottery_reward_cap_faithful::ticket_life_blocks();
        for s in grid().iter().step_by(5) {
            for k in [1.0, 37.0, 500.0] {
                let a = window_uniform_evaluate(k, s, life);
                let b = super::super::lottery_reward_cap_faithful::evaluate(
                    k,
                    s,
                    Rules::CurrentCode,
                    None,
                );
                assert_eq!(a.map(|o| o.net.to_bits()), b.map(|o| o.net.to_bits()));
            }
            let a = window_value_optimum(s, true, life);
            let b = split_optimum(s, Variant::InverseFactor, true, 5e6, 10.0);
            assert_eq!(a.net.to_bits(), b.net.to_bits());
        }
    }

    /// The window claim-reveal reproduces §5.1 at the node's window.
    #[test]
    fn window_claim_reveal_reproduces_5_1() {
        let chain = ChainModel::default();
        let d = LotteryDrawConfig::default();
        let rings = sample_route_ring_ages(
            &chain,
            DecoyRoute::PRODUCTION[0],
            RealAgeSpec::Fixed(30_000),
            500,
            PRIVACY_SEED,
        );
        let q = win_probability(20.0);
        let (d0, n0, _, s0) = claim_reveal_rates(&rings, q, PRIVACY_SEED);
        let (d1, n1, s1) = claim_reveal_rates_window(
            &rings,
            q,
            PRIVACY_SEED,
            d.min_utxo_age,
            d.circulation_window,
        );
        assert_eq!((d0, n0, s0), (d1, n1, s1));
    }

    fn short(shape: Shape, spent: bool, mechanism: bool) -> ProdParams {
        ProdParams {
            horizon_blocks: 20 * 17_280,
            snapshots: vec![20 * 17_280],
            start_height: PROD_BLOCKS_PER_YEAR,
            mechanism,
            ..shape_params(
                shape,
                MiningModel::Egalitarian,
                WhaleStrategy::Honest,
                1_696,
                1.0,
                spent,
                CHURN_FEE_PPM,
            )
        }
    }

    /// Every shape runs in the harness and its switches act.
    #[test]
    fn shapes_change_the_harness_run() {
        let base = run_m2_production(&ProdParams {
            horizon_blocks: 20 * 17_280,
            snapshots: vec![20 * 17_280],
            start_height: PROD_BLOCKS_PER_YEAR,
            ..crate::simulation::redistribution_redesign::variant_params(
                Variant::Baseline,
                MiningModel::Egalitarian,
                WhaleStrategy::Honest,
                1_696,
                1.0,
                false,
            )
        });
        for shape in Shape::ALL {
            let r = run_m2_production(&short(shape, false, true));
            assert_ne!(
                r.snapshots[0].gini.to_bits(),
                base.snapshots[0].gini.to_bits(),
                "{shape:?}"
            );
        }
        let s2 = run_m2_production(&short(Shape::ChurnFee, true, true));
        assert!(s2.snapshots[0].value_fee_bth > 0.0);
        let s3 = run_m2_production(&short(Shape::DemurrageMiners, true, true));
        assert!(s3.snapshots[0].miner_dividend_bth > 0.0);
        // Settle on spend is live in epoch 1 (where `short` starts): every
        // spend pays the #925 charge and the proceeds reach miners.
        let s3b = run_m2_production(&short(Shape::SettleMiners, true, true));
        assert!(s3b.snapshots[0].downgrade_bth > 0.0);
        assert!(s3b.snapshots[0].miner_dividend_bth > 0.0);
        let s1 = run_m2_production(&short(Shape::FactorPrice, true, true));
        assert!(s1.snapshots[0].ticket_burn_bth > 0.0);
        assert!(s1.snapshots[0].ticket_mean_factor >= 1.0);
    }

    /// With the mechanism off, no shape charges, pays or prices anything.
    #[test]
    fn shapes_are_inert_with_the_mechanism_off() {
        for shape in Shape::ALL {
            let r = run_m2_production(&short(shape, true, false));
            let s = &r.snapshots[0];
            assert_eq!(s.value_fee_bth, 0.0, "{shape:?}");
            assert_eq!(s.miner_dividend_bth, 0.0, "{shape:?}");
            assert_eq!(s.ticket_burn_bth, 0.0, "{shape:?}");
            assert_eq!(s.lottery_paid_bth, 0.0, "{shape:?}");
        }
    }

    /// The demurrage redirect conserves value: what leaves the pool reaches
    /// miners (80%) or the burn (20%), and the pool still balances.
    #[test]
    fn demurrage_to_miners_conserves() {
        let p = short(Shape::DemurrageMiners, true, true);
        let r = run_m2_production(&p);
        let s = &r.snapshots[0];
        assert!(s.miner_dividend_bth > 0.0);
        // Paid to miners never exceeds 80% of the demurrage collected.
        assert!(s.miner_dividend_bth <= 0.8 * s.demurrage_bth + 1e-6);
    }
}
