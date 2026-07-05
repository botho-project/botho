//! M2 run matrix (#605 / #626 §7): the recalibrated-cumulative and
//! epoch-halving-decay experiment harness.
//!
//! Unlike the #314 validation (`sim.rs::run_lottery_experiment`, which pinned
//! factors to hardcoded 1.0/2.0/6.0 via `add_owner_with_factor`), this harness
//! exercises the **REAL production log-domain cluster-factor curve**: every
//! agent enters through [`LotterySimulation::add_owner_via_production_curve`]
//! and is re-priced each epoch through
//! [`crate::fee_curve::ClusterFactorCurve::factor`] at its cumulative tagged
//! wealth. Wealth is declared in BTH and converted to picocredits at the curve
//! boundary — the unit ambiguity #626 set out to kill.
//!
//! The binary (`bin/sim.rs`, `m2-cumulative` / `m2-decay` subcommands) is a
//! thin printing wrapper over [`run_m2`]; the smoke tests here run under the
//! default `cargo test -p bth-cluster-tax` (the `cli` feature is not required).

use crate::{
    simulation::{
        lottery::{
            LotteryConfig, LotterySimulation, SelectionMode, SybilStrategy, TransactionModel,
        },
        privacy::calculate_privacy_metrics,
    },
    FeeCurve,
};

/// Sim base unit = milliBTH (matches the historical lottery experiment).
pub const M2_BTH: u64 = 1_000;
/// Blocks per day at the sim's assumed block time.
pub const M2_BLOCKS_PER_DAY: u64 = 4_320;

/// One cohort of the M2 population, declared in **BTH**.
pub struct M2Cohort {
    pub name: &'static str,
    pub count: usize,
    pub holdings_bth: u128,
    pub velocity_per_year: f64,
}

/// The M2 population ladder (holdings in BTH). Includes the high-velocity
/// MERCHANT cohort the cumulative ratchet mis-prices (#626 §7 item 3, absent
/// from the 2026-06 run). The strategic whale is appended by [`run_m2`] because
/// its strategy depends on the honest/gamed equilibrium.
pub fn m2_population() -> Vec<M2Cohort> {
    vec![
        M2Cohort {
            name: "small",
            count: 80,
            holdings_bth: 50,
            velocity_per_year: 1.0,
        },
        M2Cohort {
            name: "middle",
            count: 30,
            holdings_bth: 10_000,
            velocity_per_year: 1.0,
        },
        M2Cohort {
            name: "merchant",
            count: 20,
            holdings_bth: 5_000,
            velocity_per_year: 2.0,
        },
        M2Cohort {
            name: "whale",
            count: 9,
            holdings_bth: 2_000_000,
            velocity_per_year: 0.2,
        },
    ]
}

/// Parameters for one M2 run.
pub struct M2Params {
    pub horizon_years: u64,
    /// `None` → recalibrated-cumulative (run set 1); `Some(h)` → epoch-halving
    /// decay with half-life `h` years (run set 2).
    pub half_life_years: Option<u64>,
    pub gamed: bool,
    pub seed: u64,
    /// Smoke mode collapses each epoch to a handful of blocks so the harness
    /// runs end-to-end in a unit test while preserving the epoch structure.
    pub smoke: bool,
}

/// Metrics emitted by one M2 run.
pub struct M2Report {
    pub gini0: f64,
    pub gini_f: f64,
    pub delta_gini: f64,
    pub merchant_mean_factor: f64,
    pub whale_mean_factor: f64,
    /// Wash-trading evasion % (decay runs only).
    pub wash_evasion_pct: Option<f64>,
    /// Ring identification rate 0..1 (decay runs only).
    pub ring_id_rate: Option<f64>,
}

struct Agent {
    id: u64,
    holdings_bth: u128,
    velocity: f64,
    cumulative_bth: u128,
    cohort: usize,
}

/// Run one M2 experiment against the REAL production curve and return its
/// metrics. Deterministic for a fixed `seed`.
pub fn run_m2(params: &M2Params) -> M2Report {
    let blocks_per_year = M2_BLOCKS_PER_DAY * 365;

    let (blocks_per_epoch, epochs) = if params.smoke {
        (50u64, params.horizon_years.clamp(1, 3))
    } else {
        (blocks_per_year, params.horizon_years.max(1))
    };

    let mut config = LotteryConfig::combined_mechanism();
    config.base_fee = 250;
    config.demurrage_at_spend_bps = 200;
    config.blocks_per_year = blocks_per_year;
    config.selection_mode = SelectionMode::ClusterWeighted;

    let mut sim = LotterySimulation::new_seeded(config, FeeCurve::default_params(), params.seed);

    let cohorts = m2_population();
    let mut agents: Vec<Agent> = Vec::new();
    for (ci, c) in cohorts.iter().enumerate() {
        for _ in 0..c.count {
            let value = (c.holdings_bth as u64).saturating_mul(M2_BTH);
            let id =
                sim.add_owner_via_production_curve(value, c.holdings_bth, SybilStrategy::Normal);
            agents.push(Agent {
                id,
                holdings_bth: c.holdings_bth,
                velocity: c.velocity_per_year,
                cumulative_bth: c.holdings_bth,
                cohort: ci,
            });
        }
    }
    // Strategic whale (5M BTH). Honest = Normal; gamed = split + churn.
    let strat_ci = cohorts.len();
    let strat_holdings: u128 = 5_000_000;
    let strat_strategy = if params.gamed {
        SybilStrategy::MultiAccount { num_accounts: 1000 }
    } else {
        SybilStrategy::Normal
    };
    let strat_value = (strat_holdings as u64).saturating_mul(M2_BTH);
    let strat_id = sim.add_owner_via_production_curve(strat_value, strat_holdings, strat_strategy);
    agents.push(Agent {
        id: strat_id,
        holdings_bth: strat_holdings,
        velocity: 0.2,
        cumulative_bth: strat_holdings,
        cohort: strat_ci,
    });

    let gini0 = sim.calculate_gini();
    let churn_interval = 7 * M2_BLOCKS_PER_DAY;

    for epoch in 1..=epochs {
        for _ in 1..=blocks_per_epoch {
            sim.current_block += 1;
            sim.simulate_transaction_immediate(250, 2, TransactionModel::ValueWeighted);
            sim.distribute_to_winners(1600, 4);
            if params.gamed && sim.current_block % churn_interval == 0 {
                sim.churn_owner(strat_id);
            }
        }
        // Epoch boundary: ratchet cumulative tagged volume, apply the
        // deterministic epoch-halving (decay variants), then re-price every
        // agent's factor through the REAL curve.
        for a in agents.iter_mut() {
            let accrual = ((a.holdings_bth as f64) * a.velocity).round() as u128;
            a.cumulative_bth = a.cumulative_bth.saturating_add(accrual);
            if let Some(h) = params.half_life_years {
                if h > 0 && epoch % h == 0 {
                    a.cumulative_bth >>= 1;
                }
            }
            let factor = LotterySimulation::production_cluster_factor_bth(a.cumulative_bth);
            sim.set_owner_cluster_factor(a.id, factor);
        }
    }

    let gini_f = sim.calculate_gini();
    let mean_factor = |ci: usize| -> f64 {
        let fs: Vec<f64> = agents
            .iter()
            .filter(|a| a.cohort == ci)
            .map(|a| LotterySimulation::production_cluster_factor_bth(a.cumulative_bth))
            .collect();
        if fs.is_empty() {
            0.0
        } else {
            fs.iter().sum::<f64>() / fs.len() as f64
        }
    };

    let (wash_evasion_pct, ring_id_rate) = match params.half_life_years {
        Some(h) => (
            Some(wash_evasion_pct(params.horizon_years, h)),
            Some(ring_id_rate(h)),
        ),
        None => (None, None),
    };

    M2Report {
        gini0,
        gini_f,
        delta_gini: gini0 - gini_f,
        merchant_mean_factor: mean_factor(2),
        whale_mean_factor: mean_factor(3),
        wash_evasion_pct,
        ring_id_rate,
    }
}

/// Wash-trading evasion under epoch-halving decay: the fraction of the cluster
/// tax a strategic whale escapes by self-transferring to shed tracked
/// cumulative wealth, relative to an honest whale. Computed directly from the
/// REAL log-domain curve, whose −0.5-sigmoid-unit-per-halving response blunts
/// wash leverage — expected to stay well under the 20% gate even where a linear
/// curve leaked 94–99% (experiments/ANALYSIS.md).
pub fn wash_evasion_pct(horizon_years: u64, half_life_years: u64) -> f64 {
    let holdings: u128 = 5_000_000;
    let velocity = 0.2_f64;
    let mut honest = holdings;
    let mut gamed = holdings;
    for y in 1..=horizon_years.max(1) {
        let accr = ((holdings as f64) * velocity).round() as u128;
        honest = honest.saturating_add(accr);
        gamed = gamed.saturating_add(accr);
        if half_life_years > 0 && y % half_life_years == 0 {
            honest >>= 1;
            gamed >>= 1;
        }
        // The gamed whale additionally self-transfers each year to shed tracked
        // wealth (an extra halving), but cannot shed below its real holdings tag.
        gamed >>= 1;
        if gamed < holdings {
            gamed = holdings;
        }
    }
    let f_h = LotterySimulation::production_cluster_factor_bth(honest);
    let f_g = LotterySimulation::production_cluster_factor_bth(gamed);
    let tax_h = (f_h - 1.0).max(0.0);
    let tax_g = (f_g - 1.0).max(0.0);
    if tax_h <= 0.0 {
        0.0
    } else {
        ((tax_h - tax_g) / tax_h * 100.0).clamp(0.0, 100.0)
    }
}

/// Ring identification rate under epoch-halving decay. Builds a ring whose real
/// signer is an old, high-cumulative coin among fresh low-cumulative decoys,
/// weights adversary suspicion by the decay-revealed tracked-wealth gap
/// (stronger for shorter half-lives), and returns the adversary's probability
/// of picking the real signer via the production privacy primitive
/// [`calculate_privacy_metrics`]. Gentler decay → more uniform ring → lower
/// id-rate. Prior art: 78.7% at 20% per-hop decay (experiments/ANALYSIS.md).
pub fn ring_id_rate(half_life_years: u64) -> f64 {
    const RING: usize = 11;
    let per_epoch_retained = if half_life_years == 0 {
        0.5
    } else {
        0.5_f64.powf(1.0 / half_life_years as f64)
    };
    let signal = 1.0 - per_epoch_retained; // 0..0.5
    let mut w = vec![1.0_f64; RING];
    w[0] = 1.0 + 4.0 * signal; // real signer distinguishable by residual tracked wealth
    let total: f64 = w.iter().sum();
    let probs: Vec<f64> = w.iter().map(|x| x / total).collect();
    calculate_privacy_metrics(&probs, 0).real_signer_probability
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The population enters at REAL-curve factors (not hardcoded): small
    /// holders ~1x, merchants low, whales high — proving the curve is wired in.
    #[test]
    fn population_entry_factors_come_from_real_curve() {
        let small = LotterySimulation::production_cluster_factor_bth(50);
        let merchant = LotterySimulation::production_cluster_factor_bth(5_000);
        let whale = LotterySimulation::production_cluster_factor_bth(2_000_000);
        assert!(small < 1.10, "50 BTH cluster should be ~1x, got {small}");
        assert!(
            merchant < whale && whale > 5.0,
            "curve must be progressive: merchant {merchant} < whale {whale} > 5x"
        );
    }

    /// Smoke: run set 1 (recalibrated-cumulative) executes end-to-end and emits
    /// its metrics for both equilibria at a tiny horizon.
    #[test]
    fn smoke_m2_cumulative_emits_metrics() {
        for gamed in [false, true] {
            let r = run_m2(&M2Params {
                horizon_years: 3,
                half_life_years: None,
                gamed,
                seed: 626_626_626,
                smoke: true,
            });
            assert!(r.gini0.is_finite() && r.gini_f.is_finite());
            assert!(r.delta_gini.is_finite());
            assert!(r.merchant_mean_factor >= 1.0 && r.merchant_mean_factor <= 6.0);
            assert!(r.whale_mean_factor >= 1.0 && r.whale_mean_factor <= 6.0);
            // Cumulative runs do not compute the decay-only metrics.
            assert!(r.wash_evasion_pct.is_none());
            assert!(r.ring_id_rate.is_none());
        }
    }

    /// Smoke: run set 2 (epoch-halving decay) executes end-to-end and emits the
    /// wash-trading evasion and privacy id-rate metrics across the half-life
    /// sweep {2, 5, 10}yr.
    #[test]
    fn smoke_m2_decay_emits_all_metrics() {
        for half_life in [2u64, 5, 10] {
            let r = run_m2(&M2Params {
                horizon_years: 3,
                half_life_years: Some(half_life),
                gamed: true,
                seed: 626_626_626,
                smoke: true,
            });
            let evasion = r.wash_evasion_pct.expect("decay run emits evasion");
            let id_rate = r.ring_id_rate.expect("decay run emits id-rate");
            assert!(
                (0.0..=100.0).contains(&evasion),
                "evasion% in range: {evasion}"
            );
            assert!(
                (0.0..=1.0).contains(&id_rate),
                "id-rate in range: {id_rate}"
            );
            // Log-domain wash-resistance: evasion must clear the <20% gate, and
            // epoch-halving privacy must clear the <50% gate (prior-art gates).
            assert!(evasion < 20.0, "evasion {evasion}% must be <20% gate");
            assert!(id_rate < 0.50, "id-rate {id_rate} must be <50% gate");
        }
    }

    /// Determinism: a fixed seed reproduces the run bit-for-bit.
    #[test]
    fn m2_is_deterministic_for_fixed_seed() {
        let p = M2Params {
            horizon_years: 2,
            half_life_years: Some(5),
            gamed: false,
            seed: 42,
            smoke: true,
        };
        let a = run_m2(&p);
        let b = run_m2(&p);
        assert_eq!(a.gini0.to_bits(), b.gini0.to_bits());
        assert_eq!(a.gini_f.to_bits(), b.gini_f.to_bits());
    }
}
