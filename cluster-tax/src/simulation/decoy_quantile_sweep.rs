//! Decoy-quantile demurrage sweep — empirical gate for issue #577 (H2-B1).
//!
//! # What this gate answers
//!
//! Demurrage charges a holding fee on wealthy-cluster coins at spend time.
//! Under ring signatures the real input is hidden, so the *elapsed age* fed
//! into the charge is estimated from the public creation heights of the whole
//! ring. The shipped estimator, [`crate::ring_elapsed_centroid`], is the
//! **value-weighted mean** of the members' ages. That mean is an attack surface
//! (audit cycle 6 H2): a whale holding an old, wealthy coin can pad the ring
//! with **fresh, high-value** decoys, dragging the value-weighted age toward
//! zero and escaping most of the charge. The factor-side floor (#574 B2) closes
//! the wealth leg; this sweep evaluates the proposed age-side fix — replacing
//! the mean with a value-independent order statistic,
//! [`crate::ring_elapsed_quantile`].
//!
//! The module docs of `demurrage.rs` record the design property the whole
//! mechanism rests on: the emission-fraction sweep passes its Δgini > 0.05
//! criterion at miner-viable emission **only with demurrage active**. This
//! sweep re-confirms that property holds under the NEW kernel against an
//! **adversarial** decoy population, and measures three numbers per candidate
//! kernel:
//!
//! 1. **Δgini vs the no-demurrage baseline** — must stay above 0.05 (the design
//!    criterion the centroid kernel was validated against).
//! 2. **Adversary dilution ratio** = realized charge / charge on TRUE holding
//!    age, summed over the adversaries' spends. `1.0` = no escape; `→0` = full
//!    escape. The H2 vector drives this toward 0 against the mean kernel.
//! 3. **Honest over-charge ratio** = realized charge / true accrual for honest
//!    spenders. Must stay `≈1.0` — the #314 re-check that honest spenders are
//!    not over-charged by the new estimator.
//!
//! A kernel **passes the gate** iff Δgini > 0.05 AND dilution ≈ 1.0 AND
//! over-charge ≈ 1.0.
//!
//! # Why a focused Monte-Carlo (not the agent framework)
//!
//! The agent framework ([`crate::simulation::run_simulation`]) models balances
//! and transactions but does not express ring composition or decoy selection —
//! the exact lever this attack pulls. So this is a focused Monte-Carlo over
//! ring compositions: each spend builds an explicit ring (real input + decoys),
//! feeds it through the candidate kernel into [`crate::demurrage_charge`],
//! accumulates per-agent wealth, redistributes the proceeds per-capita (the
//! lottery's egalitarian payout), and computes Gini with the shared
//! [`super::metrics::calculate_gini`] — no second Gini implementation.
//!
//! Spend-time anchoring matches the node and issue #314: a coin's age anchor
//! resets on spend, and the accrued charge is paid at that spend.
//!
//! # Determinism
//!
//! Reproducible: every ring is built from a `ChaCha8Rng` seeded
//! deterministically from `(base_seed, agent_index, round)`, so the run does
//! not depend on iteration order and is byte-for-byte stable across runs. The
//! kernel itself is consensus-grade pure-integer; the surrounding sim uses
//! `f64` only for ratios and reporting.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::{demurrage_charge, ring_elapsed_centroid, ring_elapsed_quantile};

/// Basis-point scale (10000 = 100%).
const BPS: u64 = 10_000;

/// A candidate age estimator under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    /// The shipped value-weighted mean ([`ring_elapsed_centroid`]) — the
    /// baseline the new kernel must beat on dilution resistance.
    Centroid,
    /// A value-independent order statistic ([`ring_elapsed_quantile`]) at the
    /// given percentile in basis points (7500 = p75, 9000 = p90, 10000 = max).
    Quantile(u32),
}

impl Kernel {
    /// Short label for the report table.
    pub fn label(&self) -> String {
        match self {
            Kernel::Centroid => "centroid (mean)".to_string(),
            Kernel::Quantile(10_000) => "quantile@max".to_string(),
            Kernel::Quantile(bps) => format!("quantile@p{}", bps / 100),
        }
    }

    /// Estimate elapsed age for a ring under this kernel.
    fn estimate(&self, members: &[(u64, u64)], current_height: u64) -> u64 {
        match self {
            Kernel::Centroid => ring_elapsed_centroid(members, current_height),
            Kernel::Quantile(bps) => ring_elapsed_quantile(members, current_height, *bps),
        }
    }
}

/// Which behavioural class an agent belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Factor-1 background holders: pay zero demurrage, receive redistribution.
    Poor,
    /// Wealthy factor-6 holders who select **age-similar** decoys (the Botho
    /// wallet default). No adversarial intent.
    HonestWhale,
    /// Wealthy factor-6 holders who hold old coins but deliberately select
    /// **fresh, high-value** decoys to dilute the age estimate — the #577 / H2
    /// vector.
    AdversaryWhale,
}

/// One agent in the Monte-Carlo.
#[derive(Clone, Debug)]
struct Agent {
    kind: Kind,
    /// Current wealth (moves under demurrage + redistribution).
    wealth: u64,
    /// Cluster factor in FACTOR_SCALE units (1000 = 1x, 6000 = 6x).
    factor_scaled: u64,
    /// Height at which this agent's coins were last anchored (reset on spend).
    anchor: u64,
}

/// Tunable parameters for the sweep.
#[derive(Clone, Debug)]
pub struct DecoySweepParams {
    /// Number of factor-1 background holders.
    pub poor: usize,
    /// Number of honest factor-6 whales (age-similar decoys).
    pub honest_whales: usize,
    /// Number of adversarial factor-6 whales (fresh high-value decoys).
    pub adversary_whales: usize,
    /// Starting wealth of each poor holder.
    pub poor_wealth: u64,
    /// Starting wealth of each whale (honest and adversary alike).
    pub whale_wealth: u64,
    /// Ring size (1 real input + `ring_size - 1` decoys).
    pub ring_size: usize,
    /// Number of simulated rounds; each round = one spend per agent and one
    /// per-capita redistribution. With `blocks_per_round == blocks_per_year`,
    /// one round represents one year of holding.
    pub rounds: u64,
    /// Blocks advanced per round.
    pub blocks_per_round: u64,
    /// Annual demurrage rate at max factor, in basis points (200 = 2%/yr).
    pub rate_bps: u32,
    /// Blocks per year for the charge formula.
    pub blocks_per_year: u64,
    /// Half-width of honest decoy age jitter, in basis points of the real age
    /// (500 = ±5%). Adversary decoys ignore this (always fresh).
    pub honest_jitter_bps: u32,
    /// Base RNG seed (combined with agent index and round).
    pub seed: u64,
}

impl Default for DecoySweepParams {
    fn default() -> Self {
        // Each round = one year (blocks_per_round == blocks_per_year), so a
        // 25-round run is ~25 years of holding — long enough that 2%/yr
        // demurrage drives a visible Gini compression under full payment.
        let blocks_per_year = 6_307_200; // 5s blocks, matches the node
        Self {
            poor: 100,
            honest_whales: 10,
            adversary_whales: 10,
            poor_wealth: 1_000,
            whale_wealth: 1_000_000,
            ring_size: 11,
            rounds: 25,
            blocks_per_round: blocks_per_year,
            rate_bps: 200,
            blocks_per_year,
            honest_jitter_bps: 500,
            seed: 0xB07A_2025,
        }
    }
}

impl DecoySweepParams {
    fn num_agents(&self) -> usize {
        self.poor + self.honest_whales + self.adversary_whales
    }

    fn build_population(&self) -> Vec<Agent> {
        let mut agents = Vec::with_capacity(self.num_agents());
        for _ in 0..self.poor {
            agents.push(Agent {
                kind: Kind::Poor,
                wealth: self.poor_wealth,
                factor_scaled: 1_000, // 1x
                anchor: 0,
            });
        }
        for _ in 0..self.honest_whales {
            agents.push(Agent {
                kind: Kind::HonestWhale,
                wealth: self.whale_wealth,
                factor_scaled: 6_000, // 6x
                anchor: 0,
            });
        }
        for _ in 0..self.adversary_whales {
            agents.push(Agent {
                kind: Kind::AdversaryWhale,
                wealth: self.whale_wealth,
                factor_scaled: 6_000, // 6x
                anchor: 0,
            });
        }
        agents
    }
}

/// Build the ring a given agent presents at a spend.
///
/// Returns `(value, creation_height)` members including the real input. Honest
/// and poor agents pick **age-similar** decoys (jittered around the real age);
/// adversaries pick **fresh, equal-value** decoys (creation_height ==
/// current_height → age 0) — the maximal mean-dilution choice.
fn build_ring(
    rng: &mut ChaCha8Rng,
    kind: Kind,
    real_value: u64,
    real_age: u64,
    current_height: u64,
    ring_size: usize,
    honest_jitter_bps: u32,
) -> Vec<(u64, u64)> {
    let mut members = Vec::with_capacity(ring_size);
    let real_creation = current_height.saturating_sub(real_age);
    members.push((real_value, real_creation));

    for _ in 1..ring_size {
        match kind {
            Kind::AdversaryWhale => {
                // Fresh decoy, equal value: contributes `value × 0` to the
                // value-weighted mean numerator while inflating its denominator.
                members.push((real_value, current_height));
            }
            Kind::HonestWhale | Kind::Poor => {
                // Age-similar decoy: age within ±jitter of the real age, value
                // comparable to the real input (so the mean is well-behaved).
                let span = (real_age.saturating_mul(honest_jitter_bps as u64)) / BPS;
                let age = if span == 0 {
                    real_age
                } else {
                    let delta = rng.gen_range(0..=2 * span) as i64 - span as i64;
                    (real_age as i64 + delta).max(0) as u64
                };
                let creation = current_height.saturating_sub(age);
                // Value jitter ±10% so the value-weighted mean is realistic.
                let vspan = real_value / 10;
                let value = if vspan == 0 {
                    real_value
                } else {
                    let dv = rng.gen_range(0..=2 * vspan) as i64 - vspan as i64;
                    (real_value as i64 + dv).max(1) as u64
                };
                members.push((value, creation));
            }
        }
    }
    members
}

/// Per-kernel outcome of one Monte-Carlo run.
#[derive(Clone, Debug)]
struct RunOutcome {
    final_gini: f64,
    /// Σ realized demurrage charged to adversaries (under the kernel estimate).
    adv_realized: u128,
    /// Σ demurrage adversaries would owe on their TRUE holding age.
    adv_true: u128,
    /// Σ realized demurrage charged to honest whales.
    honest_realized: u128,
    /// Σ demurrage honest whales would owe on their TRUE holding age.
    honest_true: u128,
}

/// Run one Monte-Carlo pass. `kernel == None` is the no-demurrage baseline
/// (charges forced to zero, no redistribution) used to anchor Δgini.
fn run_one(params: &DecoySweepParams, kernel: Option<Kernel>) -> RunOutcome {
    let mut agents = params.build_population();
    let n = agents.len();

    let mut adv_realized: u128 = 0;
    let mut adv_true: u128 = 0;
    let mut honest_realized: u128 = 0;
    let mut honest_true: u128 = 0;

    // Integer redistribution carry so per-capita division loses nothing.
    let mut pool_carry: u64 = 0;

    for round in 1..=params.rounds {
        let current_height = round * params.blocks_per_round;
        let mut pool: u64 = pool_carry;

        for (idx, agent) in agents.iter_mut().enumerate() {
            let true_age = current_height.saturating_sub(agent.anchor);
            // Spend moves the agent's whole stack; the charge binds to that.
            let value = agent.wealth;

            if let Some(kernel) = kernel {
                // Deterministic per-(agent, round) RNG: independent of iteration
                // order, byte-for-byte reproducible.
                let mut rng = ChaCha8Rng::seed_from_u64(
                    params
                        .seed
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        .wrapping_add((idx as u64).wrapping_mul(0x1000_0001))
                        .wrapping_add(round.wrapping_mul(0x0100_0193)),
                );
                let ring = build_ring(
                    &mut rng,
                    agent.kind,
                    value,
                    true_age,
                    current_height,
                    params.ring_size,
                    params.honest_jitter_bps,
                );
                let est_age = kernel.estimate(&ring, current_height);

                let realized = demurrage_charge(
                    value,
                    agent.factor_scaled,
                    est_age,
                    params.rate_bps,
                    params.blocks_per_year,
                )
                .min(agent.wealth);
                let truth = demurrage_charge(
                    value,
                    agent.factor_scaled,
                    true_age,
                    params.rate_bps,
                    params.blocks_per_year,
                );

                match agent.kind {
                    Kind::AdversaryWhale => {
                        adv_realized += realized as u128;
                        adv_true += truth as u128;
                    }
                    Kind::HonestWhale => {
                        honest_realized += realized as u128;
                        honest_true += truth as u128;
                    }
                    Kind::Poor => {}
                }

                agent.wealth -= realized;
                pool += realized;
            }

            // Spend-time anchoring (#314): the anchor resets at every spend.
            agent.anchor = current_height;
        }

        // Per-capita redistribution (egalitarian lottery payout).
        if n > 0 && pool > 0 {
            let share = pool / n as u64;
            pool_carry = pool % n as u64;
            if share > 0 {
                for agent in agents.iter_mut() {
                    agent.wealth += share;
                }
            }
        } else {
            pool_carry = pool;
        }
    }

    let wealths: Vec<u64> = agents.iter().map(|a| a.wealth).collect();
    let final_gini = super::metrics::calculate_gini(&wealths);

    RunOutcome {
        final_gini,
        adv_realized,
        adv_true,
        honest_realized,
        honest_true,
    }
}

/// Result row for one candidate kernel.
#[derive(Clone, Debug)]
pub struct KernelResult {
    pub kernel: Kernel,
    pub final_gini: f64,
    /// Δgini vs the no-demurrage baseline (baseline_gini − kernel_gini);
    /// positive = inequality reduced.
    pub delta_gini: f64,
    /// Adversary realized charge / true-age charge. 1.0 = no escape, →0 = full
    /// escape.
    pub adversary_dilution_ratio: f64,
    /// Honest realized charge / true accrual. Must stay ≈1.0.
    pub honest_overcharge_ratio: f64,
}

impl KernelResult {
    /// The gate: Δgini > 0.05 AND dilution ≈ 1.0 AND over-charge ≈ 1.0.
    pub fn passes(&self) -> bool {
        self.delta_gini > 0.05
            && (self.adversary_dilution_ratio - 1.0).abs() <= 0.10
            && (self.honest_overcharge_ratio - 1.0).abs() <= 0.15
    }
}

/// Full sweep across the baseline (no demurrage) and every candidate kernel.
#[derive(Clone, Debug)]
pub struct DecoySweepReport {
    /// Final Gini with demurrage disabled (the Δgini anchor).
    pub baseline_gini: f64,
    /// Initial Gini of the population (sanity reference).
    pub initial_gini: f64,
    pub results: Vec<KernelResult>,
    pub params: DecoySweepParams,
}

/// The candidate kernels evaluated by the sweep, in report order.
pub fn candidate_kernels() -> Vec<Kernel> {
    vec![
        Kernel::Centroid,
        Kernel::Quantile(7_500),  // p75
        Kernel::Quantile(9_000),  // p90
        Kernel::Quantile(10_000), // max
    ]
}

/// Run the full decoy-quantile sweep.
pub fn run_decoy_sweep(params: &DecoySweepParams) -> DecoySweepReport {
    let initial = params.build_population();
    let initial_wealths: Vec<u64> = initial.iter().map(|a| a.wealth).collect();
    let initial_gini = super::metrics::calculate_gini(&initial_wealths);

    let baseline = run_one(params, None);
    let baseline_gini = baseline.final_gini;

    let results = candidate_kernels()
        .into_iter()
        .map(|kernel| {
            let out = run_one(params, Some(kernel));
            let adversary_dilution_ratio = if out.adv_true > 0 {
                out.adv_realized as f64 / out.adv_true as f64
            } else {
                // No true charge to dilute -> treat as no escape.
                1.0
            };
            let honest_overcharge_ratio = if out.honest_true > 0 {
                out.honest_realized as f64 / out.honest_true as f64
            } else {
                1.0
            };
            KernelResult {
                kernel,
                final_gini: out.final_gini,
                delta_gini: baseline_gini - out.final_gini,
                adversary_dilution_ratio,
                honest_overcharge_ratio,
            }
        })
        .collect();

    DecoySweepReport {
        baseline_gini,
        initial_gini,
        results,
        params: params.clone(),
    }
}

/// Render the report as a plain-text comparison table for the CLI.
pub fn to_table(report: &DecoySweepReport) -> String {
    let p = &report.params;
    let mut s = String::new();
    s.push_str("Decoy-Quantile Demurrage Sweep (empirical gate for #577 / H2-B1)\n");
    s.push_str("================================================================\n");
    s.push_str(&format!(
        "Population: {} poor (1x) / {} honest whales (6x, age-similar decoys) / {} adversary whales (6x, FRESH high-value decoys)\n",
        p.poor, p.honest_whales, p.adversary_whales,
    ));
    s.push_str(&format!(
        "Ring size {} (1 real input + {} decoys) | {} rounds × {} blocks (~{} yr) | rate {}bps/yr at factor 6 | honest jitter ±{}%\n",
        p.ring_size,
        p.ring_size - 1,
        p.rounds,
        p.blocks_per_round,
        p.rounds * p.blocks_per_round / p.blocks_per_year,
        p.rate_bps,
        p.honest_jitter_bps / 100,
    ));
    s.push_str(&format!(
        "Initial Gini {:.4} | no-demurrage baseline Gini {:.4} (Δgini anchor)\n\n",
        report.initial_gini, report.baseline_gini,
    ));

    s.push_str(
        "| kernel          | final_gini | Δgini   | adv_dilution | honest_overcharge | passes |\n",
    );
    s.push_str(
        "|-----------------|------------|---------|--------------|-------------------|--------|\n",
    );
    for r in &report.results {
        s.push_str(&format!(
            "| {:<15} | {:>10.4} | {:>+7.4} | {:>12.4} | {:>17.4} | {:^6} |\n",
            r.kernel.label(),
            r.final_gini,
            r.delta_gini,
            r.adversary_dilution_ratio,
            r.honest_overcharge_ratio,
            if r.passes() { "YES" } else { "no" },
        ));
    }
    s.push('\n');

    // Interpretation footer (factual; mirrors the K/J/M scenario style).
    s.push_str("Reading the table:\n");
    s.push_str("- Δgini is vs the no-demurrage baseline; the design criterion is Δgini > 0.05.\n");
    s.push_str(
        "- adv_dilution = adversary realized charge / charge on their TRUE holding age.\n  1.0 = no escape; →0 = the fresh-high-value-decoy attack succeeds.\n",
    );
    s.push_str(
        "- honest_overcharge = honest realized charge / true accrual (#314 re-check). Must stay ≈1.0.\n",
    );
    s.push_str(
        "- passes = Δgini > 0.05 AND |adv_dilution−1| ≤ 0.10 AND |honest_overcharge−1| ≤ 0.15.\n",
    );
    s.push_str(&format!(
        "- The mean kernel succumbs to the age-dilution vector: with a single old real input\n  among {} fresh decoys, only the MAXIMUM order statistic surfaces it. A percentile p\n  recovers a lone real input only when p > (ring_size−1)/ring_size × 100% = {:.1}%, so\n  p75/p90 still return a fresh decoy age here. This ring-size dependence is the core\n  tradeoff: max resists the attack fully but relies on age-similar honest decoy selection\n  to avoid over-charging honest spenders.\n",
        p.ring_size - 1,
        (p.ring_size as f64 - 1.0) / p.ring_size as f64 * 100.0,
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick_params() -> DecoySweepParams {
        DecoySweepParams {
            poor: 30,
            honest_whales: 4,
            adversary_whales: 4,
            rounds: 10,
            ..Default::default()
        }
    }

    #[test]
    fn sweep_runs_and_reports_all_kernels() {
        let report = run_decoy_sweep(&quick_params());
        assert_eq!(report.results.len(), 4);
        let table = to_table(&report);
        assert!(table.contains("centroid (mean)"));
        assert!(table.contains("quantile@max"));
        assert!(table.contains("passes"));
    }

    #[test]
    fn sweep_is_reproducible() {
        let p = quick_params();
        let a = to_table(&run_decoy_sweep(&p));
        let b = to_table(&run_decoy_sweep(&p));
        assert_eq!(a, b, "sweep output must be deterministic");
    }

    #[test]
    fn max_kernel_resists_dilution_centroid_does_not() {
        let report = run_decoy_sweep(&DecoySweepParams::default());
        let centroid = report
            .results
            .iter()
            .find(|r| r.kernel == Kernel::Centroid)
            .unwrap();
        let qmax = report
            .results
            .iter()
            .find(|r| r.kernel == Kernel::Quantile(10_000))
            .unwrap();

        // The mean kernel lets the adversary escape most of the charge.
        assert!(
            centroid.adversary_dilution_ratio < 0.5,
            "centroid dilution {} should show heavy escape",
            centroid.adversary_dilution_ratio
        );
        // The max-quantile recovers the true age: ~no escape.
        assert!(
            (qmax.adversary_dilution_ratio - 1.0).abs() < 0.01,
            "max dilution {} should be ~1.0",
            qmax.adversary_dilution_ratio
        );
    }

    #[test]
    fn honest_spenders_not_overcharged_under_any_kernel() {
        let report = run_decoy_sweep(&DecoySweepParams::default());
        for r in &report.results {
            assert!(
                (r.honest_overcharge_ratio - 1.0).abs() <= 0.15,
                "{} over-charges honest spenders: ratio {}",
                r.kernel.label(),
                r.honest_overcharge_ratio
            );
        }
    }

    #[test]
    fn lone_real_input_only_max_passes_dilution() {
        // With ring_size 11 and a single old real input, only the max statistic
        // surfaces it; p75/p90 return a fresh decoy age (heavy dilution).
        let report = run_decoy_sweep(&DecoySweepParams::default());
        let p75 = report
            .results
            .iter()
            .find(|r| r.kernel == Kernel::Quantile(7_500))
            .unwrap();
        let p90 = report
            .results
            .iter()
            .find(|r| r.kernel == Kernel::Quantile(9_000))
            .unwrap();
        assert!(
            p75.adversary_dilution_ratio < 0.5,
            "p75 dilution {} should show escape for a lone real input",
            p75.adversary_dilution_ratio
        );
        assert!(
            p90.adversary_dilution_ratio < 0.5,
            "p90 dilution {} should show escape for a lone real input",
            p90.adversary_dilution_ratio
        );
    }

    #[test]
    fn demurrage_reduces_gini_under_max_kernel() {
        // The design property: with demurrage active (and not escaped), Δgini
        // exceeds the 0.05 criterion.
        let report = run_decoy_sweep(&DecoySweepParams::default());
        let qmax = report
            .results
            .iter()
            .find(|r| r.kernel == Kernel::Quantile(10_000))
            .unwrap();
        assert!(
            qmax.delta_gini > 0.05,
            "max kernel Δgini {} should exceed 0.05",
            qmax.delta_gini
        );
    }
}
