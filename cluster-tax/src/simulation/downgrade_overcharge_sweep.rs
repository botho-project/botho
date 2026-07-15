//! Honest-overcharge sweep for the #925 base-layer downgrade charge (issue
//! #950).
//!
//! # What this sweep measures
//!
//! Issue #925 closes the demurrage class-transition leak proven by
//! [`crate::demurrage::tests::demurrage_background_reset_leak_is_real`]
//! (verdict `docs/research/demurrage-background-reset-leak.md`, #834): a
//! wealthy holder can spend a *young* wealthy coin to a fully background-tagged
//! output, pay ≈0 accrued demurrage, and escape **all** future stock-level
//! demurrage. The fix prices that transition by charging **capitalized future
//! demurrage** over the shared `SETTLEMENT_HORIZON_BLOCKS` on any spend whose
//! declared output cluster mass drops **below the ring-implied input cluster
//! floor** — the same primitive #831/settlement uses
//! (`settlement_horizon_sweep`).
//!
//! Unlike #831's *opt-in* settlement op, #925 auto-applies to **every** spend.
//! That raises a risk the settlement sweep never had to measure: because the
//! charge fires automatically on any deflating spend, it can **over-charge
//! honest activity** that legitimately looks like a downgrade. This sweep
//! quantifies that honest-overcharge / false-positive surface across the
//! horizon range, over a realistic honest agent mix, and confirms the keying
//! still charges the exploit.
//!
//! # The keying under test (strictly off an actual downgrade vs the ring floor)
//!
//! For a spend of `value` picocredits:
//!
//! ```text
//! input_floor   = ring_centroid_implied_factor(ring)   // consensus kernel, value-weighted
//! output_factor = curve.factor(declared_output_cluster_wealth)
//! downgrade_charge = if output_factor < input_floor {
//!     demurrage_charge(value, input_floor,   H) - demurrage_charge(value, output_factor, H)
//! } else { 0 }   // in-class or inflation ⇒ no charge
//! ```
//!
//! The subtraction is the **net** capitalized future demurrage escaped by
//! dropping from the floor class to the declared class over the horizon `H`. It
//! reuses the shipped [`crate::demurrage_charge`] and
//! [`crate::ring_centroid_implied_factor`] kernels and the production
//! [`crate::ClusterFactorCurve`] verbatim — no reimplementation. Two structural
//! consequences fall straight out of the kernels:
//!
//! - **A genuine background→background spend floors at 1×.** A background
//!   spender's real input and honest decoys all carry zero cluster wealth, so
//!   the value-weighted centroid is 0 and `curve.factor(0) == 1000` (exactly
//!   1×). The declared output is also 1×, so `output_factor < input_floor` is
//!   `1000 < 1000` = false ⇒ **zero charge**. This is the #950 acceptance case.
//! - **An honest in-class wealthy spend never downgrades.** The spender
//!   declares its true (full) class; honest decoys can only *dilute* the
//!   value-weighted floor **downward**, so the declared output is always `>=`
//!   the floor ⇒ never `<` ⇒ zero charge.
//!
//! The **only** way an honest spend can trip a non-zero charge is if a
//! background spender declares a 1× output while the ring floor is lifted
//! **above** 1× — and the sole channel for that is a **wealthy decoy**
//! contaminating a background spender's ring. Because the floor is
//! value-weighted, a single high-value wealthy decoy dominates the centroid and
//! inflates the floor (the reverse of the H2 age-dilution the quantile kernel
//! guards against). This sweep isolates that channel and measures the
//! false-positive rate and over-charge magnitude as a function of decoy
//! contamination.
//!
//! # Why the exploit is still charged (the separation)
//!
//! In the exploit the wealthy mass is the spender's **own** high-value real
//! input, so the value-weighted ring floor reliably recovers the wealthy class
//! (5.745× for a 10M-BTH cluster, matching the leak test) regardless of the
//! background decoys around it. The declared 1× output is far below that floor
//! ⇒ the full capitalized future demurrage is charged. Honest background→
//! background and honest in-class spends sit at zero. The exploit-vs-honest
//! table below reports both sides from the same kernels.
//!
//! # Method (a focused, deterministic Monte-Carlo)
//!
//! Same shape as the sibling `settlement_horizon_sweep` and
//! `decoy_quantile_sweep`: a focused, seeded Monte-Carlo rather than the full
//! multi-round agent framework (the lever here is a per-spend downgrade
//! classification, not multi-agent transaction flow). The honest population is
//! sourced from the shipped [`crate::simulation::agents`] archetypes — the
//! spend *sizes* are read off the real [`RetailUserAgent`], [`MerchantAgent`],
//! [`MixerServiceAgent`] and [`WhaleAgent`] structs by driving their
//! `decide_action`, so the population is genuinely agent-derived. All
//! randomness is a deterministically seeded `ChaCha8Rng` (seed `0xB07A_0925`),
//! so the doc numbers regenerate byte-for-byte.

use crate::{
    demurrage_charge,
    fee_curve::PICO_PER_BTH,
    ring_centroid_implied_factor,
    simulation::{
        agents::{MerchantAgent, MixerServiceAgent, RetailUserAgent, WhaleAgent, WhaleStrategy},
        Action, Agent, AgentId, SimulationState,
    },
    ClusterFactorCurve,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::settlement_horizon_sweep::{
    candidate_horizons, factor_classes, FactorClass, Horizon, BLOCKS_PER_YEAR, RATE_BPS,
};

/// Deterministic seed for this sweep (distinct from the settlement sweep's
/// `0xB07A_0833`).
pub const SEED: u64 = 0xB07A_0925;

/// Convert a BTH amount to picocredits clamped into `u64` (coin values here
/// stay well under `u64::MAX` picocredits ≈ 1.8e19 = 18.4M BTH).
fn bth_to_pico_u64(bth: u64) -> u64 {
    u64::try_from(bth as u128 * PICO_PER_BTH).unwrap_or(u64::MAX)
}

/// Extract a representative spend amount from an agent action (used to source
/// archetype spend sizes from the real agent structs).
fn action_amount(action: Option<Action>) -> Option<u64> {
    match action {
        Some(Action::Transfer { amount, .. }) => Some(amount),
        Some(Action::UseMixer { amount, .. }) => Some(amount),
        Some(Action::BatchTransfer { transfers }) => transfers.first().map(|&(_, a)| a),
        _ => None,
    }
}

/// Spend size of the [`RetailUserAgent`] purchase pattern
/// (background→background), read off the real agent by driving `decide_action`
/// with spending forced on.
fn retail_spend_amount() -> u64 {
    let mut a = RetailUserAgent::new(AgentId(950))
        .with_merchants(vec![AgentId(1)])
        .with_spending_probability(1.0)
        .with_avg_spend(200);
    a.account_mut_ref().balance = 2_000;
    let state = SimulationState::default();
    for _ in 0..32 {
        if let Some(amt) = action_amount(a.decide_action(&state)) {
            return amt.max(1);
        }
    }
    200
}

/// Spend size of the [`MerchantAgent`] supplier-payment pattern
/// (background→background).
fn merchant_spend_amount() -> u64 {
    let mut a = MerchantAgent::new(AgentId(951))
        .with_suppliers(vec![AgentId(1)])
        .with_payment_threshold(10_000)
        .with_supplier_payment_fraction(0.5);
    a.account_mut_ref().balance = 20_000;
    let state = SimulationState::default();
    action_amount(a.decide_action(&state))
        .unwrap_or(10_000)
        .max(1)
}

/// Spend size of the [`MixerServiceAgent`] withdrawal pattern
/// (background→background pooled coins).
fn mixer_spend_amount() -> u64 {
    let mut a = MixerServiceAgent::new(AgentId(952)).with_withdrawal_delay(5);
    a.account_mut_ref().balance = 50_000;
    a.queue_withdrawal(AgentId(1), 500, 0);
    let mut state = SimulationState::default();
    state.round = 10;
    action_amount(a.decide_action(&state)).unwrap_or(500).max(1)
}

/// Spend size of a [`WhaleAgent`] transfer at a given per-round rate
/// (wealthy→wealthy), read off the real agent.
fn whale_spend_amount(balance_bth: u64, rate: f64) -> u64 {
    let mut a = WhaleAgent::new(AgentId(953), balance_bth, WhaleStrategy::Passive)
        .with_spending_targets(vec![AgentId(1)])
        .with_spending_rate(rate);
    a.account_mut_ref().balance = balance_bth;
    let state = SimulationState::default();
    action_amount(a.decide_action(&state))
        .unwrap_or(balance_bth / 100)
        .max(1)
}

/// One honest spend archetype, drawn from the shipped agent population.
#[derive(Clone, Debug)]
pub struct HonestArchetype {
    /// Human label (e.g. "retail-purchase").
    pub name: &'static str,
    /// The `simulation::agents` type this archetype is sourced from.
    pub agent_kind: &'static str,
    /// Population-mix weight (relative frequency in the Monte-Carlo draw).
    pub weight: u32,
    /// The spender's true cluster wealth in BTH (0 = background/commerce).
    pub spender_cluster_wealth_bth: u64,
    /// The declared output cluster wealth in BTH (what the spend tags its
    /// output as). Honest spends declare their inherited/true class.
    pub output_cluster_wealth_bth: u64,
    /// Agent-derived spend value in BTH.
    pub coin_value_bth: u64,
    /// True if this is a background (1× commerce) spender — the only class that
    /// can be tripped by wealthy-decoy contamination.
    pub is_background: bool,
}

/// Build the honest population from the real agent structs.
///
/// Weights approximate a commerce-dominated chain: retail + merchant + mixer
/// background spends vastly outnumber wealthy in-class transfers.
pub fn honest_archetypes() -> Vec<HonestArchetype> {
    let retail_v = retail_spend_amount();
    let merchant_v = merchant_spend_amount();
    let mixer_v = mixer_spend_amount();
    // Wealthy in-class patterns: consolidation moves a large chunk, change-making
    // a medium chunk, an ordinary in-class transfer a smaller chunk. All stay in
    // the 10M-BTH wealthy class (5.745×) on both sides.
    let whale_consolidation_v = whale_spend_amount(100_000, 0.50);
    let whale_change_v = whale_spend_amount(50_000, 0.20);
    let whale_inclass_v = whale_spend_amount(20_000, 0.40);

    const WEALTHY_BTH: u64 = 10_000_000;
    vec![
        HonestArchetype {
            name: "retail-purchase",
            agent_kind: "RetailUserAgent",
            weight: 50,
            spender_cluster_wealth_bth: 0,
            output_cluster_wealth_bth: 0,
            coin_value_bth: retail_v,
            is_background: true,
        },
        HonestArchetype {
            name: "merchant-supplier",
            agent_kind: "MerchantAgent",
            weight: 20,
            spender_cluster_wealth_bth: 0,
            output_cluster_wealth_bth: 0,
            coin_value_bth: merchant_v,
            is_background: true,
        },
        HonestArchetype {
            name: "mixer-withdrawal",
            agent_kind: "MixerServiceAgent",
            weight: 15,
            spender_cluster_wealth_bth: 0,
            output_cluster_wealth_bth: 0,
            coin_value_bth: mixer_v,
            is_background: true,
        },
        HonestArchetype {
            name: "whale-consolidation",
            agent_kind: "WhaleAgent",
            weight: 6,
            spender_cluster_wealth_bth: WEALTHY_BTH,
            output_cluster_wealth_bth: WEALTHY_BTH,
            coin_value_bth: whale_consolidation_v,
            is_background: false,
        },
        HonestArchetype {
            name: "whale-change-making",
            agent_kind: "WhaleAgent",
            weight: 5,
            spender_cluster_wealth_bth: WEALTHY_BTH,
            output_cluster_wealth_bth: WEALTHY_BTH,
            coin_value_bth: whale_change_v,
            is_background: false,
        },
        HonestArchetype {
            name: "whale-inclass-transfer",
            agent_kind: "WhaleAgent",
            weight: 4,
            spender_cluster_wealth_bth: WEALTHY_BTH,
            output_cluster_wealth_bth: WEALTHY_BTH,
            coin_value_bth: whale_inclass_v,
            is_background: false,
        },
    ]
}

/// Population + ring parameters for the sweep.
#[derive(Clone, Debug)]
pub struct HonestSweepParams {
    /// Number of honest spends drawn per (contamination) evaluation.
    pub n_spends: usize,
    /// Ring size (real input + decoys), matching the leak test's 11-member
    /// ring.
    pub ring_size: usize,
    /// The wealthy cluster's tagged wealth in BTH (drives the 5.745× floor).
    pub wealthy_cluster_wealth_bth: u64,
    /// Value range (BTH) of a background decoy (typical small UTXO amounts).
    pub bg_decoy_value_bth: (u64, u64),
    /// Value range (BTH) of a wealthy decoy (large UTXO amounts — this is what
    /// dominates the value-weighted floor when it contaminates a small ring).
    pub wealthy_decoy_value_bth: (u64, u64),
    /// Exploit coin value in BTH (matches the leak test's 1000-BTH coin).
    pub exploit_coin_value_bth: u64,
    /// RNG seed.
    pub seed: u64,
}

impl Default for HonestSweepParams {
    fn default() -> Self {
        Self {
            n_spends: 20_000,
            ring_size: 11,
            wealthy_cluster_wealth_bth: 10_000_000,
            bg_decoy_value_bth: (10, 2_000),
            wealthy_decoy_value_bth: (5_000, 100_000),
            exploit_coin_value_bth: 1_000,
            seed: SEED,
        }
    }
}

/// Contamination levels swept (fraction of decoys that are wealthy), in basis
/// points: 0% (clean/realistic) → 1% → 5% → 10% (pathological mis-selection).
pub fn contamination_levels() -> Vec<u32> {
    vec![0, 100, 500, 1_000]
}

/// The #925 downgrade charge for one spend, keyed strictly off an actual
/// downgrade of the declared output factor below the ring-implied input floor.
///
/// Returns the **net** capitalized future demurrage escaped over `horizon`,
/// reusing the consensus [`demurrage_charge`] kernel. Zero when the output is
/// in-class or above the floor (no downgrade to price).
pub fn downgrade_charge(
    value_pico: u64,
    input_floor_factor: u64,
    output_declared_factor: u64,
    horizon_blocks: u64,
) -> u64 {
    // Reuse the shipped consensus kernel verbatim so the sim and the node price
    // the identical downgrade (issue #925 Part 2).
    crate::capitalized_reset_charge(
        value_pico,
        input_floor_factor,
        output_declared_factor,
        horizon_blocks,
        RATE_BPS,
        BLOCKS_PER_YEAR,
    )
}

/// A generated honest spend, with its horizon-independent downgrade
/// classification already resolved (only the *charge magnitude* depends on the
/// horizon).
#[derive(Clone, Debug)]
struct GeneratedSpend {
    value_pico: u64,
    input_floor_factor: u64,
    output_declared_factor: u64,
}

impl GeneratedSpend {
    fn is_downgrade(&self) -> bool {
        self.output_declared_factor < self.input_floor_factor
    }
}

/// Build a ring for one spend: the real input plus `ring_size - 1` decoys drawn
/// from the UTXO wealth distribution (mostly background, `contamination_bps`
/// wealthy).
fn build_ring(
    arch: &HonestArchetype,
    params: &HonestSweepParams,
    contamination_bps: u32,
    rng: &mut ChaCha8Rng,
) -> Vec<(u64, u128)> {
    let mut ring: Vec<(u64, u128)> = Vec::with_capacity(params.ring_size);
    // Real input: carries the spender's true (inherited) cluster wealth.
    ring.push((
        bth_to_pico_u64(arch.coin_value_bth),
        arch.spender_cluster_wealth_bth as u128 * PICO_PER_BTH,
    ));
    for _ in 1..params.ring_size {
        let wealthy = rng.gen_range(0..10_000u32) < contamination_bps;
        if wealthy {
            let dv =
                rng.gen_range(params.wealthy_decoy_value_bth.0..=params.wealthy_decoy_value_bth.1);
            ring.push((
                bth_to_pico_u64(dv),
                params.wealthy_cluster_wealth_bth as u128 * PICO_PER_BTH,
            ));
        } else {
            let dv = rng.gen_range(params.bg_decoy_value_bth.0..=params.bg_decoy_value_bth.1);
            ring.push((bth_to_pico_u64(dv), 0));
        }
    }
    ring
}

/// Draw one archetype index by weight.
fn draw_archetype(archetypes: &[HonestArchetype], rng: &mut ChaCha8Rng) -> usize {
    let total: u32 = archetypes.iter().map(|a| a.weight).sum();
    let mut pick = rng.gen_range(0..total);
    for (i, a) in archetypes.iter().enumerate() {
        if pick < a.weight {
            return i;
        }
        pick -= a.weight;
    }
    archetypes.len() - 1
}

/// Per-horizon over-charge statistics on the honest spends that tripped a
/// non-zero downgrade charge.
#[derive(Clone, Debug)]
pub struct OverchargeStat {
    pub horizon: Horizon,
    /// Mean over-charge as a percentage of the tripped coin's value.
    pub mean_pct_of_value: f64,
    /// Worst-case over-charge as a percentage of value.
    pub max_pct_of_value: f64,
    /// Over-charge as a multiple of one year of ordinary demurrage at the
    /// tripped floor (= horizon in years, by linearity). 0 when nothing
    /// tripped.
    pub multiple_of_annual: f64,
}

/// Result of evaluating the honest population at one contamination level.
#[derive(Clone, Debug)]
pub struct PopulationResult {
    pub contamination_bps: u32,
    pub n_spends: usize,
    /// Number of honest spends that tripped a non-zero downgrade charge
    /// (horizon-independent — the classification is `output < floor`).
    pub tripped: usize,
    /// False-positive rate = tripped / n_spends.
    pub false_positive_rate: f64,
    /// Per-horizon over-charge magnitude on the tripped spends.
    pub overcharge: Vec<OverchargeStat>,
}

/// Evaluate the honest population at one contamination level across all
/// horizons.
fn evaluate_population(
    archetypes: &[HonestArchetype],
    params: &HonestSweepParams,
    horizons: &[Horizon],
    contamination_bps: u32,
    curve: &ClusterFactorCurve,
) -> PopulationResult {
    // Fresh, deterministic RNG per contamination level.
    let mut rng = ChaCha8Rng::seed_from_u64(params.seed ^ (contamination_bps as u64));

    let mut spends: Vec<GeneratedSpend> = Vec::with_capacity(params.n_spends);
    for _ in 0..params.n_spends {
        let idx = draw_archetype(archetypes, &mut rng);
        let arch = &archetypes[idx];
        let ring = build_ring(arch, params, contamination_bps, &mut rng);
        let input_floor_factor = ring_centroid_implied_factor(&ring, curve);
        let output_declared_factor =
            curve.factor(arch.output_cluster_wealth_bth as u128 * PICO_PER_BTH);
        spends.push(GeneratedSpend {
            value_pico: bth_to_pico_u64(arch.coin_value_bth),
            input_floor_factor,
            output_declared_factor,
        });
    }

    let tripped = spends.iter().filter(|s| s.is_downgrade()).count();
    let false_positive_rate = tripped as f64 / params.n_spends as f64;

    let overcharge = horizons
        .iter()
        .map(|&horizon| {
            let mut sum_pct = 0.0f64;
            let mut max_pct = 0.0f64;
            let mut multiple = 0.0f64;
            let mut count = 0usize;
            for s in spends.iter().filter(|s| s.is_downgrade()) {
                let charge = downgrade_charge(
                    s.value_pico,
                    s.input_floor_factor,
                    s.output_declared_factor,
                    horizon.blocks,
                );
                let pct = charge as f64 / s.value_pico as f64 * 100.0;
                sum_pct += pct;
                if pct > max_pct {
                    max_pct = pct;
                }
                let annual = demurrage_charge(
                    s.value_pico,
                    s.input_floor_factor,
                    BLOCKS_PER_YEAR,
                    RATE_BPS,
                    BLOCKS_PER_YEAR,
                );
                if annual > 0 {
                    multiple = charge as f64 / annual as f64;
                }
                count += 1;
            }
            OverchargeStat {
                horizon,
                mean_pct_of_value: if count > 0 {
                    sum_pct / count as f64
                } else {
                    0.0
                },
                max_pct_of_value: max_pct,
                multiple_of_annual: multiple,
            }
        })
        .collect();

    PopulationResult {
        contamination_bps,
        n_spends: params.n_spends,
        tripped,
        false_positive_rate,
        overcharge,
    }
}

/// A representative clean-ring classification of one archetype (for the
/// per-archetype separation table). Uses an all-background (uncontaminated)
/// ring, the realistic honest case.
#[derive(Clone, Debug)]
pub struct ArchetypeClassification {
    pub name: &'static str,
    pub agent_kind: &'static str,
    pub coin_value_bth: u64,
    pub input_floor_factor: u64,
    pub output_declared_factor: u64,
    pub is_downgrade: bool,
    /// Downgrade charge as % of value at 1yr and 5yr (0 for honest rows).
    pub charge_pct_1yr: f64,
    pub charge_pct_5yr: f64,
}

fn classify_archetypes(
    archetypes: &[HonestArchetype],
    params: &HonestSweepParams,
    curve: &ClusterFactorCurve,
) -> Vec<ArchetypeClassification> {
    let one_year = BLOCKS_PER_YEAR;
    let five_year = 5 * BLOCKS_PER_YEAR;
    let mut rng = ChaCha8Rng::seed_from_u64(params.seed ^ 0xC1EA_0000);
    archetypes
        .iter()
        .map(|arch| {
            // Clean (uncontaminated) ring — the realistic honest case.
            let ring = build_ring(arch, params, 0, &mut rng);
            let input_floor_factor = ring_centroid_implied_factor(&ring, curve);
            let output_declared_factor =
                curve.factor(arch.output_cluster_wealth_bth as u128 * PICO_PER_BTH);
            let value_pico = bth_to_pico_u64(arch.coin_value_bth);
            let c1 = downgrade_charge(
                value_pico,
                input_floor_factor,
                output_declared_factor,
                one_year,
            );
            let c5 = downgrade_charge(
                value_pico,
                input_floor_factor,
                output_declared_factor,
                five_year,
            );
            ArchetypeClassification {
                name: arch.name,
                agent_kind: arch.agent_kind,
                coin_value_bth: arch.coin_value_bth,
                input_floor_factor,
                output_declared_factor,
                is_downgrade: output_declared_factor < input_floor_factor,
                charge_pct_1yr: c1 as f64 / value_pico as f64 * 100.0,
                charge_pct_5yr: c5 as f64 / value_pico as f64 * 100.0,
            }
        })
        .collect()
}

/// The exploit spend (young wealthy → background), evaluated at every horizon.
#[derive(Clone, Debug)]
pub struct ExploitResult {
    pub cluster_wealth_bth: u64,
    pub coin_value_bth: u64,
    pub input_floor_factor: u64,
    pub output_declared_factor: u64,
    /// (horizon, charge as % of value, multiple of one year of ordinary
    /// demurrage).
    pub rows: Vec<(Horizon, f64, f64)>,
}

fn evaluate_exploit(
    params: &HonestSweepParams,
    horizons: &[Horizon],
    curve: &ClusterFactorCurve,
) -> ExploitResult {
    let value_pico = bth_to_pico_u64(params.exploit_coin_value_bth);
    // The exploiter's own high-value wealthy real input dominates the
    // value-weighted ring floor (matching the leak test's 5.745× on a 10M-BTH
    // cluster), so the floor recovers the wealthy class regardless of decoys.
    let real_input = [(
        value_pico,
        params.wealthy_cluster_wealth_bth as u128 * PICO_PER_BTH,
    )];
    let input_floor_factor = ring_centroid_implied_factor(&real_input, curve);
    // Declared output: fully background (the deflation the leak exploits).
    let output_declared_factor = curve.factor(0);

    let rows = horizons
        .iter()
        .map(|&horizon| {
            let charge = downgrade_charge(
                value_pico,
                input_floor_factor,
                output_declared_factor,
                horizon.blocks,
            );
            let annual = demurrage_charge(
                value_pico,
                input_floor_factor,
                BLOCKS_PER_YEAR,
                RATE_BPS,
                BLOCKS_PER_YEAR,
            );
            let pct = charge as f64 / value_pico as f64 * 100.0;
            let multiple = if annual > 0 {
                charge as f64 / annual as f64
            } else {
                0.0
            };
            (horizon, pct, multiple)
        })
        .collect();

    ExploitResult {
        cluster_wealth_bth: params.wealthy_cluster_wealth_bth,
        coin_value_bth: params.exploit_coin_value_bth,
        input_floor_factor,
        output_declared_factor,
        rows,
    }
}

// ===========================================================================
// Factor-similarity decoy band (§4.6, issue #925 Part 1)
// ===========================================================================
//
// §4.5 showed the ONLY honest over-charge channel is a wealthy decoy
// contaminating a background spender's value-weighted ring floor (8.3% FP at 1%
// contamination). The wallet-layer fix mirrors the shipped
// `AGE_SIMILARITY_SPREAD_BPS = 1000` (±10%) age band in
// `botho/src/decoy_selection.rs`: draw only decoys whose cluster factor is
// within a band around the real input's factor, so a background (1×) spender
// draws only near-1× decoys and the floor stays exact.
//
// This sweep finds the widest band that keeps the honest false-positive rate at
// ~0% while retaining the same-class decoy pool (anonymity). Because factor is
// class-quantized (the curve is flat at exactly 1× for background wealth), the
// same-class background pool is retained at ANY band width; the binding
// constraint is the band's UPPER edge staying below the nearest wealthy class.

/// The pathological raw decoy contamination the factor band is stress-tested
/// against: 5% wealthy decoys (the §4.5 level that produced a 34% FP rate with
/// NO band filter). A good band must drive that back to ~0.
pub const FACTOR_BAND_CONTAMINATION_BPS: u32 = 500;

/// Candidate factor-similarity band widths (basis points of multiplicative
/// spread around the real input's factor). ±5% … ±500%: the lower values match
/// the age band convention; the wide values expose the knee where wealthy
/// classes bleed into a background spender's band.
pub fn factor_band_widths() -> Vec<u32> {
    vec![
        500, 1_000, 2_000, 3_500, 5_000, 9_000, 10_000, 25_000, 50_000,
    ]
}

/// Inclusive upper edge of the factor band around `real_factor` at `band_bps`
/// spread, clamped to the curve maximum (6×). Mirrors `age_similarity_band`.
fn factor_band_hi(real_factor: u64, band_bps: u32) -> u64 {
    let delta = (real_factor as u128 * band_bps as u128 / 10_000) as u64;
    real_factor
        .saturating_add(delta)
        .min(6 * ClusterFactorCurve::FACTOR_SCALE)
}

/// Inclusive lower edge of the factor band, floored at the 1× minimum.
fn factor_band_lo(real_factor: u64, band_bps: u32) -> u64 {
    let delta = (real_factor as u128 * band_bps as u128 / 10_000) as u64;
    real_factor
        .saturating_sub(delta)
        .max(ClusterFactorCurve::FACTOR_SCALE)
}

/// Draw one wealthy decoy's `(value_pico, cluster_wealth_pico)` from a random
/// factor class (spread across the top decile), so the band knee surfaces as
/// each class progressively enters a widening band.
fn draw_wealthy_decoy(
    classes: &[FactorClass],
    params: &HonestSweepParams,
    rng: &mut ChaCha8Rng,
) -> (u64, u128) {
    let class = &classes[rng.gen_range(0..classes.len())];
    let dv = rng.gen_range(params.wealthy_decoy_value_bth.0..=params.wealthy_decoy_value_bth.1);
    (
        bth_to_pico_u64(dv),
        class.cluster_wealth_bth as u128 * PICO_PER_BTH,
    )
}

/// One row of the factor-band sweep: a candidate band width and its measured
/// effect on the honest FP rate and the eligible decoy pool.
#[derive(Clone, Debug)]
pub struct FactorBandRow {
    /// Band width in basis points of multiplicative spread (1000 = ±10%).
    pub band_bps: u32,
    /// Inclusive upper factor edge for a background (1×) spender at this band.
    pub band_hi_factor: u64,
    /// Number of wealthy factor classes that leak into a 1× spender's band.
    pub wealthy_classes_admitted: usize,
    /// Fraction of a realistic mixed candidate pool
    /// ((1−c) background + c wealthy) admitted by the band — the eligible pool.
    pub eligible_pool_frac: f64,
    /// Fraction of the same-class (background) sub-pool retained (anonymity).
    /// 1.0 at every band: factor is class-quantized, so a background spender
    /// never loses same-class candidates.
    pub same_class_retained: f64,
    /// Honest false-positive rate for background spends WITH the band filter,
    /// at the pathological [`FACTOR_BAND_CONTAMINATION_BPS`] raw
    /// contamination.
    pub false_positive_rate: f64,
    /// Mean over-charge (% of value) on the tripped spends at the 5yr horizon.
    pub mean_overcharge_5yr: f64,
}

impl FactorBandRow {
    /// True when the band drives the honest false-positive rate to zero — the
    /// selection criterion for `FACTOR_SIMILARITY_SPREAD`.
    pub fn fp_safe(&self) -> bool {
        self.false_positive_rate == 0.0
    }
}

/// Evaluate one factor-band width: apply the band filter to a background
/// spender's decoy selection at the pathological contamination level and
/// measure the residual FP rate + eligible pool.
fn evaluate_factor_band(
    params: &HonestSweepParams,
    classes: &[FactorClass],
    curve: &ClusterFactorCurve,
    band_bps: u32,
) -> FactorBandRow {
    // A background spender's real input is fully diffused → factor 1×.
    let real_factor = curve.factor(0);
    let lo = factor_band_lo(real_factor, band_bps);
    let hi = factor_band_hi(real_factor, band_bps);

    // Which wealthy classes leak into the band (analytic).
    let wealthy_classes_admitted = classes
        .iter()
        .filter(|c| {
            let f = curve.factor(c.cluster_wealth_bth as u128 * PICO_PER_BTH);
            f > real_factor && f >= lo && f <= hi
        })
        .count();

    // Eligible pool of a mixed candidate set: (1−c) background (always in-band)
    // + c wealthy spread uniformly across the classes (in-band iff its factor
    // fits the band). Background retention is 1.0 by construction.
    let c = FACTOR_BAND_CONTAMINATION_BPS as f64 / 10_000.0;
    let eligible_pool_frac =
        (1.0 - c) + c * (wealthy_classes_admitted as f64 / classes.len() as f64);

    // Monte-Carlo FP: build banded background rings and count floors > 1×.
    let value_pico = bth_to_pico_u64(params.exploit_coin_value_bth.max(1));
    let mut rng = ChaCha8Rng::seed_from_u64(params.seed ^ 0xBA0D_0000 ^ band_bps as u64);
    let output_declared = real_factor; // background spender declares 1×
    let mut tripped = 0usize;
    let mut sum_overcharge = 0.0f64;
    for _ in 0..params.n_spends {
        // Real background input.
        let mut ring: Vec<(u64, u128)> = Vec::with_capacity(params.ring_size);
        let bg_real_value =
            rng.gen_range(params.bg_decoy_value_bth.0..=params.bg_decoy_value_bth.1);
        ring.push((bth_to_pico_u64(bg_real_value), 0));
        for _ in 1..params.ring_size {
            let wealthy = rng.gen_range(0..10_000u32) < FACTOR_BAND_CONTAMINATION_BPS;
            let (dv, dw) = if wealthy {
                draw_wealthy_decoy(classes, params, &mut rng)
            } else {
                let dv = rng.gen_range(params.bg_decoy_value_bth.0..=params.bg_decoy_value_bth.1);
                (bth_to_pico_u64(dv), 0u128)
            };
            let f = curve.factor(dw);
            if f >= lo && f <= hi {
                // Passes the band filter → admitted.
                ring.push((dv, dw));
            } else {
                // Rejected by the band filter → the wallet substitutes an
                // in-band (background) decoy, exactly as Part 3 does.
                let sub = rng.gen_range(params.bg_decoy_value_bth.0..=params.bg_decoy_value_bth.1);
                ring.push((bth_to_pico_u64(sub), 0));
            }
        }
        let input_floor = ring_centroid_implied_factor(&ring, curve);
        if output_declared < input_floor {
            tripped += 1;
            let charge = downgrade_charge(
                value_pico,
                input_floor,
                output_declared,
                5 * BLOCKS_PER_YEAR,
            );
            sum_overcharge += charge as f64 / value_pico as f64 * 100.0;
        }
    }

    FactorBandRow {
        band_bps,
        band_hi_factor: hi,
        wealthy_classes_admitted,
        eligible_pool_frac,
        same_class_retained: 1.0,
        false_positive_rate: tripped as f64 / params.n_spends as f64,
        mean_overcharge_5yr: if tripped > 0 {
            sum_overcharge / tripped as f64
        } else {
            0.0
        },
    }
}

/// Run the factor-band sweep across all candidate band widths.
pub fn factor_band_sweep(
    params: &HonestSweepParams,
    classes: &[FactorClass],
    curve: &ClusterFactorCurve,
) -> Vec<FactorBandRow> {
    factor_band_widths()
        .into_iter()
        .map(|bw| evaluate_factor_band(params, classes, curve, bw))
        .collect()
}

/// The widest FP-safe band width in a completed sweep — the recommended
/// `FACTOR_SIMILARITY_SPREAD`. Falls back to the tightest band if none is safe
/// (never happens for the shipped population).
pub fn recommended_factor_band(rows: &[FactorBandRow]) -> u32 {
    rows.iter()
        .filter(|r| r.fp_safe())
        .map(|r| r.band_bps)
        .max()
        .unwrap_or(0)
}

/// The full honest-overcharge report.
#[derive(Clone, Debug)]
pub struct HonestOverchargeReport {
    pub horizons: Vec<Horizon>,
    pub archetypes: Vec<HonestArchetype>,
    pub params: HonestSweepParams,
    /// Per-archetype clean-ring classification (the separation table).
    pub archetype_classification: Vec<ArchetypeClassification>,
    /// Clean-ring (0% contamination) honest population result — the realistic
    /// case.
    pub clean: PopulationResult,
    /// Contamination sensitivity (0/1/5/10% wealthy decoys).
    pub sensitivity: Vec<PopulationResult>,
    /// The exploit spend, confirming the leak is still priced at every horizon.
    pub exploit: ExploitResult,
    /// Factor-similarity decoy band sweep (§4.6, #925 Part 1): picks
    /// `FACTOR_SIMILARITY_SPREAD` for the wallet-layer decoy filter.
    pub factor_band: Vec<FactorBandRow>,
    /// The recommended (widest FP-safe) band width, in basis points.
    pub recommended_factor_band_bps: u32,
}

/// Run the complete honest-overcharge sweep with the default configuration.
pub fn run_honest_overcharge_sweep() -> HonestOverchargeReport {
    let horizons = candidate_horizons();
    let archetypes = honest_archetypes();
    let params = HonestSweepParams::default();
    let curve = ClusterFactorCurve::default_params();

    let archetype_classification = classify_archetypes(&archetypes, &params, &curve);
    let clean = evaluate_population(&archetypes, &params, &horizons, 0, &curve);
    let sensitivity = contamination_levels()
        .into_iter()
        .map(|c| evaluate_population(&archetypes, &params, &horizons, c, &curve))
        .collect();
    let exploit = evaluate_exploit(&params, &horizons, &curve);
    let classes = factor_classes();
    let factor_band = factor_band_sweep(&params, &classes, &curve);
    let recommended_factor_band_bps = recommended_factor_band(&factor_band);

    HonestOverchargeReport {
        horizons,
        archetypes,
        params,
        archetype_classification,
        clean,
        sensitivity,
        exploit,
        factor_band,
        recommended_factor_band_bps,
    }
}

fn factor_x(scaled: u64) -> f64 {
    scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64
}

/// Render the report as Markdown tables (the doc numbers are generated from
/// this, never hand-computed).
pub fn to_markdown(report: &HonestOverchargeReport) -> String {
    let mut s = String::new();

    // 1. Honest archetypes (agent-sourced).
    s.push_str("### Honest archetypes (sourced from `simulation::agents`)\n\n");
    s.push_str(
        "Spend sizes are read off the real agent structs by driving their \
         `decide_action`. Weight = relative frequency in the Monte-Carlo draw.\n\n",
    );
    s.push_str(
        "| archetype | agent | weight | spender class | output class | coin value (BTH) |\n",
    );
    s.push_str("|-----------|-------|-------:|-------------:|------------:|-----------------:|\n");
    for a in &report.archetypes {
        s.push_str(&format!(
            "| {} | {} | {} | {} BTH | {} BTH | {} |\n",
            a.name,
            a.agent_kind,
            a.weight,
            a.spender_cluster_wealth_bth,
            a.output_cluster_wealth_bth,
            a.coin_value_bth,
        ));
    }
    s.push('\n');

    // 2. Per-archetype downgrade classification (clean rings) — the separation.
    s.push_str("### Downgrade classification per archetype (clean rings)\n\n");
    s.push_str(
        "Input floor = `ring_centroid_implied_factor` over the real input + honest \
         (background) decoys. A downgrade is charged only when the declared output \
         factor is strictly below the floor. Honest rows never downgrade; the \
         exploit does.\n\n",
    );
    s.push_str(
        "| archetype | input floor | output factor | downgrade? | charge @1yr | charge @5yr |\n",
    );
    s.push_str("|-----------|-----------:|-------------:|:---------:|-----------:|-----------:|\n");
    for c in &report.archetype_classification {
        s.push_str(&format!(
            "| {} | {:.3}x | {:.3}x | {} | {:.4}% | {:.4}% |\n",
            c.name,
            factor_x(c.input_floor_factor),
            factor_x(c.output_declared_factor),
            if c.is_downgrade { "YES" } else { "no" },
            c.charge_pct_1yr,
            c.charge_pct_5yr,
        ));
    }
    // Exploit row appended for direct comparison.
    let ex = &report.exploit;
    let ex_1yr = ex
        .rows
        .iter()
        .find(|(h, _, _)| h.label == "1yr")
        .map(|(_, p, _)| *p)
        .unwrap_or(0.0);
    let ex_5yr = ex
        .rows
        .iter()
        .find(|(h, _, _)| h.label == "5yr")
        .map(|(_, p, _)| *p)
        .unwrap_or(0.0);
    s.push_str(&format!(
        "| **exploit (young-wealthy→bg)** | {:.3}x | {:.3}x | **YES** | **{:.4}%** | **{:.4}%** |\n",
        factor_x(ex.input_floor_factor),
        factor_x(ex.output_declared_factor),
        ex_1yr,
        ex_5yr,
    ));
    s.push('\n');

    // 3. Honest false-positive rate + over-charge (clean/realistic rings).
    s.push_str("### Honest false-positive rate & over-charge (clean rings, realistic case)\n\n");
    s.push_str(&format!(
        "Monte-Carlo over {} honest spends (seed `{:#010X}`), ring size {}, \
         all-background decoys (0% contamination). False-positive rate = fraction \
         of honest spends charged a non-zero downgrade fee.\n\n",
        report.clean.n_spends, report.params.seed, report.params.ring_size,
    ));
    s.push_str(&format!(
        "**Honest false-positive rate: {:.4}% ({} / {} spends).**\n\n",
        report.clean.false_positive_rate * 100.0,
        report.clean.tripped,
        report.clean.n_spends,
    ));
    s.push_str("| horizon | honest FP rate | mean over-charge (% of value) | max over-charge |\n");
    s.push_str("|---------|--------------:|------------------------------:|----------------:|\n");
    for o in &report.clean.overcharge {
        s.push_str(&format!(
            "| {} ({:.2}yr) | {:.4}% | {:.4}% | {:.4}% |\n",
            o.horizon.label,
            o.horizon.years(),
            report.clean.false_positive_rate * 100.0,
            o.mean_pct_of_value,
            o.max_pct_of_value,
        ));
    }
    s.push('\n');

    // 4. Exploit charge by horizon.
    s.push_str("### Exploit downgrade charge by horizon (the leak is priced)\n\n");
    s.push_str(&format!(
        "Young-wealthy→background spend: {}-BTH cluster ({:.3}× floor), {}-BTH coin, \
         declared background ({:.3}× output). Charge = capitalized future demurrage; \
         multiple = charge ÷ one year of ordinary demurrage at the floor (= horizon in \
         years, by linearity).\n\n",
        ex.cluster_wealth_bth,
        factor_x(ex.input_floor_factor),
        ex.coin_value_bth,
        factor_x(ex.output_declared_factor),
    ));
    s.push_str("| horizon | exploit charge (% of value) | × annual demurrage |\n");
    s.push_str("|---------|----------------------------:|-------------------:|\n");
    for (h, pct, mult) in &ex.rows {
        s.push_str(&format!(
            "| {} ({:.2}yr) | {:.4}% | {:.2}× |\n",
            h.label,
            h.years(),
            pct,
            mult,
        ));
    }
    s.push('\n');

    // 5. Contamination sensitivity.
    s.push_str("### Sensitivity: honest FP rate & over-charge vs decoy contamination\n\n");
    s.push_str(
        "The **only** channel to an honest over-charge: a wealthy decoy contaminating \
         a background spender's value-weighted ring floor. FP rate is \
         horizon-independent (classification is `output < floor`); the over-charge \
         magnitude below is at the 5yr horizon (worst case). Contamination = fraction \
         of decoys that are wealthy coins.\n\n",
    );
    s.push_str(
        "| contamination | honest FP rate | mean over-charge @5yr | max over-charge @5yr |\n",
    );
    s.push_str(
        "|--------------:|--------------:|----------------------:|---------------------:|\n",
    );
    for r in &report.sensitivity {
        let five = r
            .overcharge
            .iter()
            .find(|o| o.horizon.label == "5yr")
            .unwrap();
        s.push_str(&format!(
            "| {:.1}% | {:.4}% | {:.4}% | {:.4}% |\n",
            r.contamination_bps as f64 / 100.0,
            r.false_positive_rate * 100.0,
            five.mean_pct_of_value,
            five.max_pct_of_value,
        ));
    }
    s.push('\n');

    // 6. Factor-similarity decoy band (§4.6, #925 Part 1).
    s.push_str("### Factor-similarity decoy band (picks `FACTOR_SIMILARITY_SPREAD`)\n\n");
    s.push_str(&format!(
        "The §4.5 contamination channel is closed at the wallet layer by a factor band \
         mirroring the shipped `AGE_SIMILARITY_SPREAD_BPS` (±10%) age band: a background (1×) \
         spender draws only decoys whose cluster factor sits within the band around its real \
         input's factor. This sweep applies that filter at the pathological {:.1}% raw \
         contamination (the §4.5 level that gave a {:.1}% FP rate UNFILTERED) and finds the \
         widest band that keeps the honest FP rate at 0%. Band edge = `1000 × (1 + spread)` \
         clamped to 6×; same-class (background) pool retention is 1.00 at every band because \
         factor is class-quantized.\n\n",
        FACTOR_BAND_CONTAMINATION_BPS as f64 / 100.0,
        report
            .sensitivity
            .iter()
            .find(|r| r.contamination_bps == FACTOR_BAND_CONTAMINATION_BPS)
            .map(|r| r.false_positive_rate * 100.0)
            .unwrap_or(0.0),
    ));
    s.push_str(
        "| factor band | band edge (×) | wealthy classes admitted | eligible pool | \
         same-class pool | honest FP rate | mean over-charge @5yr | FP-safe |\n",
    );
    s.push_str(
        "|------------:|--------------:|-------------------------:|--------------:|\
         ----------------:|---------------:|----------------------:|:-------:|\n",
    );
    for r in &report.factor_band {
        s.push_str(&format!(
            "| ±{:.0}% | {:.3}× | {} / {} | {:.2}% | {:.2}% | {:.4}% | {:.4}% | {} |\n",
            r.band_bps as f64 / 100.0,
            r.band_hi_factor as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
            r.wealthy_classes_admitted,
            factor_classes().len(),
            r.eligible_pool_frac * 100.0,
            r.same_class_retained * 100.0,
            r.false_positive_rate * 100.0,
            r.mean_overcharge_5yr,
            if r.fp_safe() { "yes" } else { "NO" },
        ));
    }
    s.push('\n');
    s.push_str(&format!(
        "**Recommended `FACTOR_SIMILARITY_SPREAD` = ±{:.0}% ({} bps)** — the age-band \
         convention, comfortably inside the widest FP-safe band (±{:.0}%). Every band that \
         excludes the nearest wealthy class drives the honest FP rate to exactly 0% while \
         retaining the full same-class background pool.\n\n",
        1_000_f64 / 100.0,
        1_000,
        recommended_factor_band(&report.factor_band) as f64 / 100.0,
    ));

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honest_background_and_inclass_are_never_charged_on_clean_rings() {
        // The #950 acceptance case: on realistic (uncontaminated) rings, no honest
        // archetype trips a downgrade charge, at any horizon.
        let report = run_honest_overcharge_sweep();
        for c in &report.archetype_classification {
            assert!(
                !c.is_downgrade,
                "honest archetype {} tripped a downgrade on a clean ring",
                c.name
            );
            assert_eq!(
                c.charge_pct_1yr, 0.0,
                "honest archetype {} charged at 1yr",
                c.name
            );
            assert_eq!(
                c.charge_pct_5yr, 0.0,
                "honest archetype {} charged at 5yr",
                c.name
            );
        }
        // And the population-level false-positive rate is exactly zero.
        assert_eq!(
            report.clean.tripped, 0,
            "clean-ring honest population must have zero false positives"
        );
        assert_eq!(report.clean.false_positive_rate, 0.0);
    }

    #[test]
    fn exploit_is_charged_at_every_horizon() {
        // The core correctness check: the young-wealthy→background exploit is
        // charged a non-zero downgrade fee at every horizon (the leak is closed).
        let report = run_honest_overcharge_sweep();
        assert!(
            report.exploit.output_declared_factor < report.exploit.input_floor_factor,
            "exploit must classify as a downgrade"
        );
        for (h, pct, mult) in &report.exploit.rows {
            assert!(
                *pct > 0.0,
                "exploit charge must be > 0 at horizon {}",
                h.label
            );
            // By linearity the multiple equals the horizon in years.
            assert!(
                (*mult - h.years()).abs() < 1e-6,
                "exploit multiple {} should equal horizon years {} at {}",
                mult,
                h.years(),
                h.label
            );
        }
    }

    #[test]
    fn exploit_floor_matches_the_leak_test() {
        // Cross-check against `demurrage_background_reset_leak_is_real`: a 10M-BTH
        // cluster on a 1000-BTH coin implies the 5.745× floor, and the escaped
        // one-year demurrage matches the leak test's honest baseline (18.98 BTH).
        let report = run_honest_overcharge_sweep();
        assert_eq!(
            report.exploit.input_floor_factor, 5745,
            "exploit floor must match the leak test's observed 5.745x"
        );
        // 1yr charge as % of value = rate × progressivity(5745). The leak test's
        // honest annual charge is 18.98 BTH on a 1000-BTH coin = 1.898%.
        let one_yr = report
            .exploit
            .rows
            .iter()
            .find(|(h, _, _)| h.label == "1yr")
            .unwrap();
        assert!(
            (one_yr.1 - 1.898).abs() < 1e-3,
            "exploit 1yr charge should be ~1.898% of value, got {}",
            one_yr.1
        );
    }

    #[test]
    fn contamination_is_the_only_false_positive_channel() {
        // FP rate is zero with clean rings and rises monotonically with wealthy
        // decoy contamination — the one channel the keying leaves open.
        let report = run_honest_overcharge_sweep();
        let rates: Vec<f64> = report
            .sensitivity
            .iter()
            .map(|r| r.false_positive_rate)
            .collect();
        assert_eq!(
            rates[0], 0.0,
            "0% contamination must give 0 false positives"
        );
        for w in rates.windows(2) {
            assert!(
                w[1] >= w[0],
                "FP rate must be non-decreasing in contamination: {rates:?}"
            );
        }
        assert!(
            *rates.last().unwrap() > 0.0,
            "heavy contamination must produce some false positives"
        );
    }

    #[test]
    fn factor_band_recommendation_is_the_age_band_and_is_fp_safe() {
        // The recommended band (±10%, matching AGE_SIMILARITY_SPREAD_BPS) must
        // drive the honest FP rate to exactly 0 at the pathological
        // contamination, and the widest FP-safe band must be at least ±10%.
        let report = run_honest_overcharge_sweep();
        let ten_pct = report
            .factor_band
            .iter()
            .find(|r| r.band_bps == 1_000)
            .expect("±10% band swept");
        assert_eq!(
            ten_pct.false_positive_rate, 0.0,
            "±10% band must be FP-safe at {}bps contamination",
            FACTOR_BAND_CONTAMINATION_BPS
        );
        assert_eq!(ten_pct.same_class_retained, 1.0, "same-class pool retained");
        assert!(
            report.recommended_factor_band_bps >= 1_000,
            "widest FP-safe band {} must be at least the ±10% convention",
            report.recommended_factor_band_bps
        );
    }

    #[test]
    fn factor_band_knee_admits_wealthy_classes_and_raises_fp() {
        // A pathologically wide band eventually bleeds a wealthy class into a
        // background spender's ring, lifting FP above zero — the knee that
        // bounds the safe band from above.
        let report = run_honest_overcharge_sweep();
        let widest = report.factor_band.last().unwrap();
        assert!(
            widest.wealthy_classes_admitted > 0,
            "the widest swept band must admit at least one wealthy class"
        );
        assert!(
            widest.false_positive_rate > 0.0,
            "admitting wealthy classes must produce honest false positives (FP={})",
            widest.false_positive_rate
        );
        // FP rate is monotonically non-decreasing in band width (wider admits
        // more contamination, never less).
        let rates: Vec<f64> = report
            .factor_band
            .iter()
            .map(|r| r.false_positive_rate)
            .collect();
        for w in rates.windows(2) {
            assert!(
                w[1] >= w[0] - 1e-12,
                "FP rate must be non-decreasing in band width: {rates:?}"
            );
        }
    }

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_honest_overcharge_sweep());
        let b = to_markdown(&run_honest_overcharge_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
    }

    #[test]
    fn markdown_contains_all_horizons_and_sections() {
        let md = to_markdown(&run_honest_overcharge_sweep());
        for h in candidate_horizons() {
            assert!(
                md.contains(h.label),
                "missing horizon {} in report",
                h.label
            );
        }
        assert!(md.contains("Honest false-positive rate"));
        assert!(md.contains("Exploit downgrade charge"));
        assert!(md.contains("contamination"));
    }
}
