//! Cluster demurrage: a holding charge on wealthy-cluster coins, paid at
//! spend time.
//!
//! Demurrage is the stock-level component of the cluster-tilted
//! redistribution design (docs/design/cluster-tilted-redistribution.md).
//! Transaction fees are a consumption tax and cannot touch idle wealth; the
//! emission-fraction sweep showed the mechanism passes its Δgini > 0.05
//! criterion at miner-viable emission fractions only with demurrage active.
//!
//! ## Mechanism
//!
//! ```text
//! charge = value × rate × (factor − 1)/(max_factor − 1) × elapsed / blocks_per_year
//! ```
//!
//! - `factor` is the cluster factor already used for progressive fees
//!   (1.0x..6.0x in FACTOR_SCALE units). Factor-1 (background/commerce) coins
//!   pay exactly zero — demurrage only binds wealthy clusters.
//! - `elapsed` is how long the spent coins sat idle. Under ring signatures the
//!   real input is hidden, so the elapsed value is the value-weighted centroid
//!   of the PUBLIC creation heights of all ring members: a large real input
//!   dominates its own ring's centroid, while the factor term protects small
//!   spenders from old decoys entirely.
//! - The charge is added to the transaction's minimum fee and flows through the
//!   standard fee split into the redistribution lottery pool.
//!
//! ## Why churning doesn't escape
//!
//! Spending resets a coin's creation height — but the spend pays the
//! accrued charge first. Total demurrage paid over any holding period is
//! invariant to churn frequency; churning just adds transaction fees on
//! top (which also feed the lottery pool).
//!
//! Since protocol 9.0.0 (#1875) this holds to the picocredit: the charge is
//! ONE floor of the full rational (no intermediate time quantization), so
//! splitting a hold into n same-class spends pays the unsplit charge less
//! fewer than n pico. Before 9.0.0 the elapsed time was first floored to
//! `1e6` units of a year, which zeroed every hold of 1..=6 blocks at the 5 s
//! year and let six-block churn escape all demurrage
//! (`docs/research/demurrage-quantization-study.md`, PR #1908).
//!
//! ### Scope of the churn-invariance claim: it holds WITHIN a wealth class
//!
//! Churn-invariance is invariance of *accrued* demurrage under re-spending at
//! the *same* cluster factor. It does NOT price a **class transition**. The
//! charge is proportional to `elapsed` (age-since-creation), so a coin spent
//! while young (`elapsed ≈ 0`) pays ≈0 *regardless of factor* — the
//! ring-centroid factor floor ([`ring_centroid_implied_factor`]) raises the
//! factor a background-tagged output claims, but multiplies a near-zero
//! elapsed. A holder can therefore spend a freshly-created wealthy coin to a
//! fully background-tagged output (legal deflation under
//! `check_cluster_tag_inheritance`, which only rejects inflation), pay ≈0, and
//! land a genuinely-background coin whose future ring floor is 1× — escaping
//! all FUTURE stock-level demurrage. This was a real, unpriced future-demurrage
//! leak (issue #834, write-up in
//! `docs/research/demurrage-background-reset-leak.md`).
//!
//! ### The fix (#925): price the class transition
//!
//! [`spend_demurrage_charge`] closes the leak by charging
//! `max(accrued_to_date, capitalized_reset_charge)` on every spend. The
//! capitalized term ([`capitalized_reset_charge`]) prices the permanent exit
//! from a wealth class at *capitalized future* demurrage over the shared
//! [`SETTLEMENT_HORIZON_BLOCKS`], mirroring exactly the escape #831's
//! settlement op closes for the wrapping on-ramp (ADR 0003). Because it keys
//! strictly off an actual downgrade relative to the ring floor, an honest hold
//! spent at its true class pays only its accrued demurrage, and a genuine
//! background→ background spend (floor 1× on both sides) pays zero — but the
//! young-coin exploit, whose `elapsed ≈ 0` zeroed the accrued term, now pays
//! the full capitalized reset.
//! [`tests::demurrage_background_reset_leak_is_real`] asserts the priced
//! transition.
//!
//! ### The opt-in sibling (#831): the demurrage-settlement operation
//!
//! [`demurrage_settlement_charge`] is the **explicit opt-in counterpart** of
//! the same charge. Where #925 prices the *implicit* class exit that any
//! deflating spend performs, a settlement transaction is an *explicit*,
//! consensus-recognized reclassification of a wealthy coin down to
//! factor-1/background in exchange for **wrap eligibility** (the bridge
//! on-ramp, #822/#825). It is the ONE sanctioned place cluster mass
//! legitimately DROPS to background: the drop is deflation (already legal under
//! `check_cluster_tag_inheritance`, which only rejects inflation), but it is
//! only *wrap-eligible* when it rides an accepted settlement transaction that
//! paid the full capitalized charge. Both doors read the SAME
//! [`SETTLEMENT_HORIZON_BLOCKS`] and reuse the SAME
//! [`capitalized_reset_charge`], so settling is never cheaper than holding over
//! the horizon (churn-invariance across the class transition), and neither door
//! undercuts the other.
//!
//! ## Determinism
//!
//! CONSENSUS-CRITICAL: pure integer arithmetic throughout.

/// Fixed-point scale for cluster factors (1000 = 1.0x, 6000 = 6.0x).
/// Matches `ClusterFactorCurve::FACTOR_SCALE` and the lottery.
pub const FACTOR_SCALE: u64 = 1000;

/// Maximum cluster factor in FACTOR_SCALE units.
pub const MAX_FACTOR_SCALED: u64 = 6 * FACTOR_SCALE;

/// Compute the demurrage charge for a spend.
///
/// # Arguments
/// * `transfer_value` - Total value moved (sum of output amounts)
/// * `cluster_factor` - Cluster factor in FACTOR_SCALE units (1000..=6000),
///   from the same computation the progressive fee uses
/// * `elapsed_blocks` - Holding duration in blocks (value-weighted ring
///   centroid of public UTXO creation heights)
/// * `rate_bps` - Annual demurrage rate at maximum factor, in basis points (200
///   = 2%/year); use `MonetaryPolicy::demurrage_rate_bps(height)`
/// * `blocks_per_year` - Blocks per year at the policy's assumed block time
///
/// # Returns
/// The charge in base units:
///
/// ```text
/// floor(value × rate_bps × (factor − 1000) × elapsed / (10_000 × 5_000 × blocks_per_year))
/// ```
///
/// exactly (one floor, no intermediate rounding; #1875, protocol 9.0.0),
/// clamped to `u64::MAX`. Zero for factor-1 coins, zero rate, zero elapsed
/// time or zero `blocks_per_year`. Deterministic and panic-free for every
/// input (u128 arithmetic only, overflow checks on).
pub fn demurrage_charge(
    transfer_value: u64,
    cluster_factor: u64,
    elapsed_blocks: u64,
    rate_bps: u32,
    blocks_per_year: u64,
) -> u64 {
    if rate_bps == 0 || elapsed_blocks == 0 || blocks_per_year == 0 {
        return 0;
    }

    let factor = cluster_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
    // Progressivity in FACTOR_SCALE units: 0 at factor 1.0, 5000 at factor 6.0
    let progressivity = factor - FACTOR_SCALE;
    if progressivity == 0 {
        return 0;
    }

    // EXACT (#1875): one floor of the full rational
    //
    //     charge = floor(value × rate_bps × progressivity × elapsed
    //                    / (10_000 × 5_000 × blocks_per_year))
    //
    // clamped to u64::MAX. There is NO intermediate truncation: the pre-9.0.0
    // kernel floored `elapsed × 1e6 / blocks_per_year` first, which zeroed
    // every hold of 1..=6 blocks at the 5 s year and charged every 7..=12
    // block hold as one 1e6 unit (the short-hold churn escape, #1875; the
    // legacy formula is kept in the tests as `legacy_quantized_charge`).
    //
    // u128-only, panic-free with overflow checks on. With
    //   num = value × rate × p       < 2^64 · 2^32 · 2^13 = 2^109
    //   den = 10_000 × 5_000 × bpy   < 2^26 · 2^64       = 2^90
    //   q = num / den, r = num % den   (r < den)
    // the charge is q·e + floor(r·e / den). `r·e` can exceed u128 when bpy is
    // large, so e is split as e1·2^32 + e0 (e1, e0 < 2^32):
    //   r·e1 < 2^122;  q1 = r·e1 / den < 2^32;  r1 = r·e1 % den < 2^90
    //   floor(r·e / den) = q1·2^32 + floor((r1·2^32 + r·e0) / den)
    // with r1·2^32 + r·e0 < 2^123. Every product below is therefore in range
    // EXCEPT q·e, whose overflow (checked) means the true charge exceeds
    // 2^128 > u64::MAX, so saturating it is the exact clamp. The result
    // equals the 256-bit exact floor (clamped) for EVERY u64/u32 input,
    // including absurd blocks_per_year (tests: `exact_kernel_matches_u256`).
    //
    // `checked_*(..).unwrap_or(u128::MAX)` is `saturating_*` for unsigned
    // integers, spelled with operations Aeneas models (#1820).
    const BPS: u128 = 10_000;
    const PROG_SPAN: u128 = (MAX_FACTOR_SCALED - FACTOR_SCALE) as u128;
    const SPLIT: u128 = 0x1_0000_0000; // 2^32
    let num = transfer_value as u128 * rate_bps as u128 * progressivity as u128;
    let den = BPS * PROG_SPAN * blocks_per_year as u128;
    let elapsed = elapsed_blocks as u128;
    let whole = num / den;
    let rem = num % den;
    let e_hi = elapsed / SPLIT;
    let e_lo = elapsed % SPLIT;
    let hi = rem * e_hi;
    let part = (hi / den) * SPLIT + ((hi % den) * SPLIT + rem * e_lo) / den;
    #[allow(
        clippy::manual_saturating_arithmetic,
        reason = "saturating_mul/add have no Aeneas model; this spelling extracts to Lean (#1820)"
    )]
    let charge = whole
        .checked_mul(elapsed)
        .unwrap_or(u128::MAX)
        .checked_add(part)
        .unwrap_or(u128::MAX);

    // Clamp to u64::MAX. Same result as `u64::try_from(charge)
    // .unwrap_or(u64::MAX)`, but Aeneas has no model of the u128 -> u64
    // `TryFrom` and does model `min` and the (here lossless) cast (#1820).
    charge.min(u64::MAX as u128) as u64
}

/// Blocks per year at the 5s reference block time (`31_536_000 s / 5 s`).
/// Matches `mainnet_policy().target_block_time_secs = 5` and the simulation
/// sweeps (`settlement_horizon_sweep`, `downgrade_overcharge_sweep`).
pub const BLOCKS_PER_YEAR_5S: u64 = 6_307_200;

/// Shared capitalization horizon for a **permanent exit from the demurrage
/// regime** (ADR 0003 / issue #833).
///
/// Both escape doors read this ONE constant so neither is a cheaper route than
/// the other:
///
/// - #831's opt-in **settlement op** (wrap wealthy → factor-1, then bridge
///   out).
/// - #925's automatic **downgrade charge** (spend wealthy → background output),
///   priced by [`spend_demurrage_charge`] / [`capitalized_reset_charge`].
///
/// = **5 years** at the 5s reference block time (`5 × BLOCKS_PER_YEAR_5S` =
/// `31_536_000`). Calibrated in
/// `docs/research/settlement-horizon-calibration.md`: 5yr is the only swept
/// horizon that keeps the residual redistribution gain above the 0.05 design
/// floor even if the entire top decile exits at once, while keeping the
/// wrap/downgrade cost a 1.9%–9.5% friction rather than a wall. A single shared
/// horizon prevents the cheaper door from dominating (calibration doc §6); if a
/// future need to diverge the two arises it must be its own ADR.
pub const SETTLEMENT_HORIZON_BLOCKS: u64 = 5 * BLOCKS_PER_YEAR_5S; // 31_536_000

/// Capitalized future-demurrage charge for a demurrage-class **downgrade** —
/// the base-layer analog of #831's settlement charge (issue #925).
///
/// Prices the permanent exit from a wealth class on a deflating spend: the
/// **net** capitalized future demurrage escaped by declaring an output at
/// `output_factor` when the input's true class floor — the ring-implied /
/// import-composed factor the spender cannot rewrite — is `input_floor_factor`:
///
/// ```text
/// if output_factor < input_floor_factor {
///     demurrage_charge(value, input_floor_factor, horizon)
///         - demurrage_charge(value, output_factor, horizon)
/// } else { 0 }   // in-class or inflation ⇒ nothing to price
/// ```
///
/// Reuses [`demurrage_charge`] verbatim (no second charge model). A genuine
/// background→background spend floors at 1× on both sides ⇒ `1000 < 1000` is
/// false ⇒ zero charge, so honest commerce is never touched. Keyed strictly off
/// an actual downgrade relative to the ring floor, so it fires only on the
/// young-coin exploit the accrued-to-date term misses.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic (two [`demurrage_charge`] calls
/// and a saturating subtraction). Liveness-safe: only ever raises the fee
/// floor.
pub fn capitalized_reset_charge(
    transfer_value: u64,
    input_floor_factor: u64,
    output_factor: u64,
    horizon_blocks: u64,
    rate_bps: u32,
    blocks_per_year: u64,
) -> u64 {
    if output_factor >= input_floor_factor {
        return 0;
    }
    let at_floor = demurrage_charge(
        transfer_value,
        input_floor_factor,
        horizon_blocks,
        rate_bps,
        blocks_per_year,
    );
    let at_output = demurrage_charge(
        transfer_value,
        output_factor,
        horizon_blocks,
        rate_bps,
        blocks_per_year,
    );
    at_floor.saturating_sub(at_output)
}

/// The opt-in **settlement charge** (#831): the one-time price a holder pays to
/// permanently reclassify a wealthy-cluster coin down to factor-1 / background
/// so it becomes wrap-eligible (the bridge on-ramp, #822/#825).
///
/// This is the **explicit opt-in counterpart** to #925's **implicit** downgrade
/// charge ([`spend_demurrage_charge`] / [`capitalized_reset_charge`]): both
/// price a permanent exit from a wealth class at the capitalized future
/// demurrage the shed cluster mass would otherwise owe, over the *same* shared
/// [`SETTLEMENT_HORIZON_BLOCKS`]. Sharing the horizon is what keeps neither
/// door a cheaper route than the other (calibration doc §6).
///
/// A settlement transaction declares **all-background (factor-1) outputs**, so
/// the exit target is exactly [`FACTOR_SCALE`] (1×) and the charge is
/// [`capitalized_reset_charge`] evaluated from the ring-floored input class
/// down to background — **reused verbatim, no second charge model**.
///
/// `input_floor_factor` MUST be the ring-centroid / import-composed factor
/// floor the spender cannot rewrite ([`ring_centroid_implied_factor`] etc.),
/// never a spender-declared factor — otherwise a wealthy holder could
/// under-declare the input's class to pay a smaller charge (the anti-dilution
/// machinery is the same one #925 relies on). A genuinely background (factor-1)
/// coin returns 0 (nothing to shed), so honest commerce coins settle for free.
///
/// # Relationship to the consensus fee floor
///
/// The consensus fee floor already charges `spend_demurrage_charge =
/// max(accrued, capitalized)` on every spend. For a settlement, whose outputs
/// are background, `claimed_factor == FACTOR_SCALE` so the capitalized term
/// equals **exactly** this settlement charge — hence a settlement is priced by
/// the existing floor with no separate additive term (double-charging is
/// avoided by construction), and the floor is always `>= base +
/// demurrage_settlement_charge`.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic (one
/// [`capitalized_reset_charge`] call). Liveness-safe: only ever raises the fee
/// floor.
pub fn demurrage_settlement_charge(
    settled_value: u64,
    input_floor_factor: u64,
    rate_bps: u32,
    blocks_per_year: u64,
) -> u64 {
    capitalized_reset_charge(
        settled_value,
        input_floor_factor,
        FACTOR_SCALE, // exit target: factor-1 / background
        SETTLEMENT_HORIZON_BLOCKS,
        rate_bps,
        blocks_per_year,
    )
}

/// Total demurrage owed on a spend under #925: the greater of accrued-to-date
/// demurrage and the capitalized charge for a class downgrade.
///
/// ```text
/// charge = max(
///     demurrage_charge(value, input_floor_factor, elapsed, ...),        // accrued-to-date
///     capitalized_reset_charge(value, input_floor_factor, output_factor,
///                              SETTLEMENT_HORIZON_BLOCKS, ...),          // priced class exit
/// )
/// ```
///
/// This is the **load-bearing formula** from the #925 verdict
/// (`docs/research/demurrage-background-reset-leak.md`): an honest hold spent
/// at its true class pays its real accrued demurrage (the capitalized term is 0
/// because there is no downgrade), while the young-coin exploit — which drives
/// `elapsed ≈ 0` so accrued ≈ 0 — is caught by the capitalized term. The `max`
/// (not a sum) guarantees the charge never exceeds the larger of the two
/// obligations, so it never over-charges an honest spend and stays symmetric
/// with #831's opt-in settlement price (capitalized future demurrage over the
/// same [`SETTLEMENT_HORIZON_BLOCKS`]).
///
/// `input_floor_factor` is the composed ring floor the spender cannot rewrite
/// (`max(claimed, amount_matched_ring_floor_factor, import_floor)`; the ring
/// term is the amount-matched per-input max of #1686),
/// `output_factor` is the spender-declared output factor. A downgrade is
/// exactly `output_factor < input_floor_factor`; a background→background spend
/// floors at 1× on both sides and pays zero.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic. Liveness-safe: only ever raises
/// the fee floor, never false-rejects a valid spend.
///
/// This single-rate form charges both terms at one rate. Consensus calls
/// [`spend_demurrage_charge_with_exit_rate`], which prices the two terms at
/// their own rates (#1685); the two agree whenever the rates are equal (from
/// epoch 1 on), which is what this function computes.
pub fn spend_demurrage_charge(
    transfer_value: u64,
    input_floor_factor: u64,
    output_factor: u64,
    elapsed_blocks: u64,
    rate_bps: u32,
    blocks_per_year: u64,
) -> u64 {
    let accrued = demurrage_charge(
        transfer_value,
        input_floor_factor,
        elapsed_blocks,
        rate_bps,
        blocks_per_year,
    );
    let capitalized = capitalized_reset_charge(
        transfer_value,
        input_floor_factor,
        output_factor,
        SETTLEMENT_HORIZON_BLOCKS,
        rate_bps,
        blocks_per_year,
    );
    accrued.max(capitalized)
}

/// The consensus spend-demurrage charge (#925 + #1685): the greater of
/// accrued-to-date demurrage at the **in-class** rate and the capitalized
/// class-exit charge at the **exit-pricing** rate.
///
/// ```text
/// charge = max(
///     demurrage_charge(value, input_floor_factor, elapsed, accrual_rate_bps, ..),
///     capitalized_reset_charge(value, input_floor_factor, output_factor,
///                              SETTLEMENT_HORIZON_BLOCKS, exit_rate_bps, ..),
/// )
/// ```
///
/// Callers pass `accrual_rate_bps = MonetaryPolicy::demurrage_rate_bps(h)`
/// (0 in the epoch-0 bootstrap holiday) and `exit_rate_bps =
/// MonetaryPolicy::exit_pricing_rate_bps(h)` (200 bps from block 1). The
/// capitalized term is the #925 downgrade charge, and for the two special
/// cases of a downgrade it is also the #831 settlement charge (all-background
/// outputs, see [`demurrage_settlement_charge`]) and the ADR-0007 bridge-import
/// toll (the import floor `F` composed into `input_floor_factor`). Splitting
/// the rates keeps the holiday for ordinary holding while pricing every exit
/// from a wealth class from block 1, which closes the epoch-0 leak (#1685).
///
/// When `accrual_rate_bps == exit_rate_bps` this is exactly
/// [`spend_demurrage_charge`] (unit-tested), so the DRT-checked single-rate
/// model covers every epoch-1+ spend.
///
/// The ledger's consensus floor (`Ledger::consensus_fee_floor`), mempool relay
/// admission (`relay_demurrage`) and the sims' shared
/// [`crate::consensus_fee_model`] all call this ONE function, so block
/// validation, relay and the models cannot diverge.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic. Liveness-safe: the exit term
/// only ever raises the floor relative to [`spend_demurrage_charge`] at the
/// in-class rate.
pub fn spend_demurrage_charge_with_exit_rate(
    transfer_value: u64,
    input_floor_factor: u64,
    output_factor: u64,
    elapsed_blocks: u64,
    accrual_rate_bps: u32,
    exit_rate_bps: u32,
    blocks_per_year: u64,
) -> u64 {
    let accrued = demurrage_charge(
        transfer_value,
        input_floor_factor,
        elapsed_blocks,
        accrual_rate_bps,
        blocks_per_year,
    );
    let capitalized = capitalized_reset_charge(
        transfer_value,
        input_floor_factor,
        output_factor,
        SETTLEMENT_HORIZON_BLOCKS,
        exit_rate_bps,
        blocks_per_year,
    );
    accrued.max(capitalized)
}

/// Compute the value-weighted elapsed-blocks centroid for a set of ring
/// members.
///
/// # Arguments
/// * `members` - (value, creation_height) for each ring member (public data)
/// * `current_height` - The validating block height
///
/// # Returns
/// Exact value-weighted average of saturated `current_height −
/// creation_height`, floored once after summing, or 0 for
/// empty/zero-total-value input.
pub fn ring_elapsed_centroid(members: &[(u64, u64)], current_height: u64) -> u64 {
    // On targets with at most 64-bit usize, n < 2^64: the numerator is
    // < 2^192 and denominator < 2^128. Neither accumulator can saturate.
    // A positive-weight mean is <= max(age) <= u64::MAX, so the final clamp
    // and conversion are exact too. This bound does not require a ring cap.
    let mut mean = crate::wide::WeightedMean::new();
    for &(value, creation_height) in members {
        let elapsed = current_height.saturating_sub(creation_height);
        mean.add_product(value as u128, elapsed as u128);
        mean.add_weight(value as u128);
    }
    mean.floor() as u64
}

/// Compute a value-independent quantile (order statistic) of the elapsed ages
/// of a ring's members.
///
/// Where [`ring_elapsed_centroid`] takes the **value-weighted mean** of the
/// ring members' ages, this takes an **unweighted order statistic** (a
/// percentile) over the ages. `quantile_bps` selects the percentile in basis
/// points of the sorted ages: `10000` = the maximum age, `9000` = p90, `7500` =
/// p75, `5000` = the median, `0` = the minimum.
///
/// # Why value-independent (audit cycle 6 H2, design #574 item B1, issue #577)
///
/// The value-weighted mean is exactly the H2 *age*-dilution vector. A spender
/// who holds an old, wealthy real input can drag the centroid age toward zero
/// by padding the ring with **fresh, high-value** background-tagged decoys:
/// every such decoy contributes `value × 0` to the weighted numerator while
/// inflating the denominator, so the mean collapses toward the decoys' (zero)
/// age even though the real input is old. The factor-side floor (item B2,
/// [`ring_centroid_implied_factor`]) closes the wealth/factor leg of this
/// attack, but the age leg remains open against the mean kernel.
///
/// An order statistic over ages defeats this because **value is not in the
/// computation at all**: a fresh decoy is just one more `0` in the sorted age
/// list, and no amount of value attached to it moves a percentile. The real
/// input is a single old member; the maximum (`quantile_bps = 10000`) is the
/// one statistic guaranteed to surface a lone old member regardless of how many
/// fresh decoys surround it. Lower percentiles (p75/p90) only surface the real
/// input when the old members make up more than `(1 − quantile)` of the ring —
/// for a single real input in a ring of size `n`, that requires
/// `quantile_bps > (n − 1)/n × 10000`. This tradeoff is the subject of the
/// empirical decoy-quantile sweep (`simulation::decoy_quantile_sweep`).
///
/// # Arguments
/// * `members` - `(value, creation_height)` for each ring member (public data).
///   The `value` field is **accepted but deliberately ignored** so this
///   function is drop-in comparable with [`ring_elapsed_centroid`]; only the
///   per-member elapsed age `current_height − creation_height` is used.
/// * `current_height` - The validating block height.
/// * `quantile_bps` - The percentile to return, in basis points (`0..=10000`).
///   Values above `10000` are clamped to `10000`.
///
/// # Returns
/// The `quantile_bps`-th percentile of the members' elapsed ages, by the
/// nearest-rank method (`rank = ceil(quantile_bps/10000 × n)`, `index =
/// rank − 1` clamped into `0..n`). Empty set → 0. A single member → that
/// member's elapsed age for every quantile.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic, operates on a sorted *copy* of
/// the elapsed ages (no in-place mutation of caller data, no float, no
/// HashMap/HashSet iteration order). The sort is total over `u64`, so the
/// output is a pure function of the multiset of ages and `quantile_bps`. Safe
/// for the consensus age-floor enforcement (#577) to reuse unchanged once
/// wired.
pub fn ring_elapsed_quantile(
    members: &[(u64, u64)],
    current_height: u64,
    quantile_bps: u32,
) -> u64 {
    if members.is_empty() {
        return 0;
    }

    // Order statistic over AGES ONLY — value is intentionally not read. A
    // high-value fresh decoy is just another `0` here and cannot move the result.
    let mut ages: Vec<u64> = members
        .iter()
        .map(|&(_value, creation_height)| current_height.saturating_sub(creation_height))
        .collect();
    ages.sort_unstable();

    let n = ages.len() as u64;
    let q = quantile_bps.min(10_000) as u64;

    // Nearest-rank: rank = ceil(q/10000 × n), index = rank - 1, clamped to
    // [0, n-1]. Pure integer ceil via (a + b - 1) / b. q = 0 → index 0 (min);
    // q = 10000 → index n-1 (max).
    let rank = (q * n).div_ceil(10_000);
    let index = rank.saturating_sub(1).min(n - 1) as usize;
    ages[index]
}

/// Compute the cluster factor implied by the value-weighted centroid of a
/// ring's own cluster wealth.
///
/// This is the wealth/factor analog of [`ring_elapsed_centroid`]: where that
/// function takes the value-weighted centroid of ring members' public creation
/// AGES, this takes the value-weighted centroid of their effective cluster
/// WEALTH and maps it through the progressive fee curve.
///
/// # Why this exists (audit cycle 6 H2, design #574 item B2)
///
/// The cluster factor that drives the demurrage charge (and the progressive
/// fee) is otherwise derived from the transaction's spender-authored OUTPUT
/// tags. A wealthy spender can tag every output as "background" (factor 1x) and
/// pay ~zero demurrage, even while spending coins that inherited a wealthy
/// cluster's tags — picking fresh background-tagged decoys to drag the implied
/// factor toward 1.
///
/// The ring members' tags, by contrast, are public chain state the spender
/// cannot rewrite. The real input is one of the ring members and carries its
/// inherited (wealthy) tags, so the factor the ring composition implies is a
/// floor the spender cannot claim below. There is no free or empirical
/// parameter: the floor is a pure function of public ring data and the chain's
/// per-cluster wealth.
///
/// # Arguments
/// * `members` - `(value, member_effective_cluster_wealth)` for each ring
///   member. `member_effective_cluster_wealth` is the member's value-normalized
///   cluster wealth (`Σ_tag weight × W_global / TAG_WEIGHT_SCALE`), resolved by
///   the caller from public per-cluster wealth state.
/// * `curve` - The progressive cluster factor curve.
///
/// # Returns
/// The implied cluster factor in FACTOR_SCALE units (1000 = 1x .. 6000 = 6x).
/// Background-only rings (zero centroid wealth) imply exactly the 1x floor.
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic, ordered-slice input, no
/// HashMap/HashSet iteration order, no node-local state. Safe for the consensus
/// fee-floor enforcement (item B4) to reuse unchanged.
pub fn ring_centroid_implied_factor(
    members: &[(u64, u128)],
    curve: &crate::fee_curve::ClusterFactorCurve,
) -> u64 {
    // Exact value-weighted centroid (#1876, protocol 9.0.0):
    // floor(Σ value_i·wealth_i / Σ value_i) with a 256-bit numerator.
    //
    // `value` is a u64 coin amount and `member_wealth` full-u128 cumulative
    // cluster wealth (#626), so a single product reaches ~2^192 and the sum
    // over a ring routinely exceeds u128::MAX for whale-scale rings. Before
    // 9.0.0 the numerator saturated at u128::MAX and was then divided by the
    // unsaturated total value. That did NOT pin the centroid to u128::MAX (the
    // old comment's "conservative" claim was false): it LOWERED the centroid,
    // so scaling every value in a homogeneous ring by 10 lowered its floor from
    // 5949 to 5841. The exact mean is scale invariant.
    //
    // A weighted mean never exceeds its largest term, so the result is at most
    // max(member_wealth) <= u128::MAX: the final clamp in `WeightedMean::floor`
    // is unreachable here. On every ring whose old u128 numerator did not
    // overflow, the result is bit-identical to the pre-9.0.0 one.
    let mut mean = crate::wide::WeightedMean::new();
    for &(value, member_wealth) in members {
        mean.add_product(value as u128, member_wealth);
        mean.add_weight(value as u128);
    }

    let centroid_wealth = mean.floor();

    curve.factor(centroid_wealth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fee_curve::ClusterFactorCurve;

    const BLOCKS_PER_YEAR: u64 = 6_307_200; // 5s blocks

    #[test]
    fn test_factor_one_pays_zero() {
        // Background/commerce coins are exempt regardless of age or value
        assert_eq!(
            demurrage_charge(
                u64::MAX,
                FACTOR_SCALE,
                BLOCKS_PER_YEAR * 10,
                200,
                BLOCKS_PER_YEAR
            ),
            0
        );
    }

    #[test]
    fn test_max_factor_full_rate() {
        // Factor 6.0, one year: exactly rate_bps of value
        // 1M × 2% = 20_000
        assert_eq!(
            demurrage_charge(
                1_000_000,
                MAX_FACTOR_SCALED,
                BLOCKS_PER_YEAR,
                200,
                BLOCKS_PER_YEAR
            ),
            20_000
        );
    }

    #[test]
    fn test_linear_in_factor_time_and_value() {
        let full = demurrage_charge(
            1_000_000,
            MAX_FACTOR_SCALED,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        );

        // Half the progressivity range: factor 3.5 → (3500-1000)/5000 = 1/2
        let half_factor = demurrage_charge(1_000_000, 3_500, BLOCKS_PER_YEAR, 200, BLOCKS_PER_YEAR);
        assert_eq!(half_factor, full / 2);

        // Half the time
        let half_time = demurrage_charge(
            1_000_000,
            MAX_FACTOR_SCALED,
            BLOCKS_PER_YEAR / 2,
            200,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(half_time, full / 2);

        // Double the value
        let double_value = demurrage_charge(
            2_000_000,
            MAX_FACTOR_SCALED,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(double_value, full * 2);
    }

    #[test]
    fn test_zero_inputs() {
        assert_eq!(
            demurrage_charge(1_000_000, 6_000, 0, 200, BLOCKS_PER_YEAR),
            0
        );
        assert_eq!(
            demurrage_charge(1_000_000, 6_000, 1000, 0, BLOCKS_PER_YEAR),
            0
        );
        assert_eq!(demurrage_charge(1_000_000, 6_000, 1000, 200, 0), 0);
        assert_eq!(demurrage_charge(0, 6_000, 1000, 200, BLOCKS_PER_YEAR), 0);
    }

    #[test]
    fn test_no_overflow_at_extremes() {
        // Must not panic; saturates at u64::MAX
        let charge = demurrage_charge(u64::MAX, u64::MAX, u64::MAX, u32::MAX, 1);
        assert!(charge > 0);
    }

    #[test]
    fn test_ring_centroid_value_weighted() {
        // A whale's own large input dominates its ring centroid: small fresh
        // decoys barely reduce the elapsed value.
        let current = 1_000_000u64;
        let members = [
            (100_000_000, 0), // real input: 100M, 1M blocks old
            (1_000, current), // fresh small decoy
            (1_000, current), // fresh small decoy
        ];
        let elapsed = ring_elapsed_centroid(&members, current);
        // 100M×1M / 100.002M ≈ 999_980
        assert!(elapsed > 999_900, "elapsed = {elapsed}");

        // Conversely, a small spender with an old whale decoy gets a large
        // centroid — but pays zero anyway via the factor term (see
        // test_factor_one_pays_zero).
    }

    #[test]
    fn test_ring_centroid_empty_and_zero_value() {
        assert_eq!(ring_elapsed_centroid(&[], 1000), 0);
        assert_eq!(ring_elapsed_centroid(&[(0, 0)], 1000), 0);
    }

    #[test]
    fn test_ring_quantile_known_set() {
        // current = 100, creation heights chosen so elapsed = {10,20,30,40,50}.
        // Values are varied (and large on fresh members) to prove the result is
        // value-INDEPENDENT.
        let current = 100u64;
        let members = [
            (1u64, current - 10),        // elapsed 10
            (999_999_999, current - 20), // elapsed 20 (huge value: must not matter)
            (7, current - 30),           // elapsed 30
            (3, current - 40),           // elapsed 40
            (1, current - 50),           // elapsed 50
        ];
        // n = 5, nearest-rank.
        assert_eq!(ring_elapsed_quantile(&members, current, 10_000), 50); // max
        assert_eq!(ring_elapsed_quantile(&members, current, 9_000), 50); // p90: rank=ceil(4.5)=5
        assert_eq!(ring_elapsed_quantile(&members, current, 7_500), 40); // p75: rank=ceil(3.75)=4
        assert_eq!(ring_elapsed_quantile(&members, current, 5_000), 30); // p50: rank=ceil(2.5)=3
        assert_eq!(ring_elapsed_quantile(&members, current, 2_500), 20); // p25: rank=ceil(1.25)=2
        assert_eq!(ring_elapsed_quantile(&members, current, 0), 10); // p0 = min
    }

    #[test]
    fn test_ring_quantile_value_independent() {
        // Same ages, wildly different values -> identical quantiles. This is the
        // structural property the H2 age-dilution attack cannot bypass.
        let current = 1_000u64;
        let cheap = [(1u64, 900), (1, 800), (1, 700)]; // elapsed 100,200,300
        let pricey = [(u64::MAX, 900), (u64::MAX, 800), (u64::MAX, 700)];
        for q in [0u32, 2_500, 5_000, 7_500, 9_000, 10_000] {
            assert_eq!(
                ring_elapsed_quantile(&cheap, current, q),
                ring_elapsed_quantile(&pricey, current, q),
                "quantile {q} must not depend on value"
            );
        }
    }

    #[test]
    fn test_ring_quantile_resists_h2_dilution_that_mean_succumbs_to() {
        // The #577 / H2 age-dilution attack: an OLD, wealthy real input padded
        // with FRESH, EQUAL-VALUE decoys. The value-weighted mean collapses
        // toward the decoys' zero age; the max-quantile recovers the real age.
        let current = 1_000_000u64;
        let real_age = current; // real input created at height 0
        let mut members = vec![(100_000_000u64, 0u64)]; // real: 100M, age 1_000_000
        for _ in 0..10 {
            members.push((100_000_000, current)); // fresh decoy, equal value,
                                                  // age 0
        }

        // Mean kernel SUCCUMBS: 100M×T / (11×100M) = T/11 — dragged ~91% down.
        let mean = ring_elapsed_centroid(&members, current);
        assert!(
            mean < real_age / 8,
            "mean should be diluted far below the real age: mean={mean}, real={real_age}"
        );

        // Max-quantile RESISTS: the lone old member is the maximum age, fully
        // recovered regardless of how many fresh decoys surround it.
        let qmax = ring_elapsed_quantile(&members, current, 10_000);
        assert_eq!(
            qmax, real_age,
            "max-quantile must recover the real age exactly"
        );

        // The charge a factor-6 spender owes is restored by the quantile kernel.
        let charge_mean = demurrage_charge(100_000_000, 6_000, mean, 200, BLOCKS_PER_YEAR);
        let charge_qmax = demurrage_charge(100_000_000, 6_000, qmax, 200, BLOCKS_PER_YEAR);
        assert!(
            charge_qmax > charge_mean * 8,
            "quantile charge {charge_qmax} must dwarf the diluted mean charge {charge_mean}"
        );
    }

    #[test]
    fn test_ring_quantile_lone_real_input_needs_max() {
        // With a single old real input in a ring of 11 fresh decoys, only the
        // maximum surfaces it; p90 and p75 still return a fresh (zero) age.
        // This is the order-statistic tradeoff the sweep quantifies.
        let current = 500_000u64;
        let mut members = vec![(1u64, 0u64)]; // real input, age 500_000
        for _ in 0..10 {
            members.push((1, current)); // fresh decoys, age 0
        }
        assert_eq!(ring_elapsed_quantile(&members, current, 10_000), 500_000); // max recovers
        assert_eq!(ring_elapsed_quantile(&members, current, 9_000), 0); // p90 misses
        assert_eq!(ring_elapsed_quantile(&members, current, 7_500), 0); // p75 misses
    }

    #[test]
    fn test_ring_quantile_empty_and_single() {
        // Empty -> 0 for every quantile.
        for q in [0u32, 5_000, 10_000] {
            assert_eq!(ring_elapsed_quantile(&[], 1000, q), 0);
        }
        // Single member -> its elapsed age for every quantile.
        for q in [0u32, 2_500, 5_000, 7_500, 10_000] {
            assert_eq!(ring_elapsed_quantile(&[(42, 300)], 1000, q), 700);
        }
    }

    #[test]
    fn test_ring_quantile_clamps_out_of_range_bps() {
        // quantile_bps above 10_000 clamps to the max.
        let members = [(1u64, 90), (1, 80), (1, 70)]; // current 100 -> 10,20,30
        assert_eq!(
            ring_elapsed_quantile(&members, 100, 50_000),
            ring_elapsed_quantile(&members, 100, 10_000)
        );
    }

    #[test]
    fn test_ring_centroid_implied_factor_wealthy_ring() {
        // Every member carries a wealthy cluster's wealth -> high implied factor.
        let curve = ClusterFactorCurve::default_params();
        // (value, member_effective_wealth) — wealth in picocredits: 10M BTH.
        const W: u128 = 10_000_000_000_000_000_000; // 10M BTH in pico
        let members = [(1_000_000u64, W), (1_000_000, W)];
        let implied = ring_centroid_implied_factor(&members, &curve);
        // Wealthy centroid maps near the curve maximum, well above 1x.
        assert!(implied >= 5_000, "wealthy ring implied factor = {implied}");
        assert_eq!(implied, curve.factor(W));
    }

    #[test]
    fn test_ring_centroid_implied_factor_background_ring() {
        // Zero member wealth (background ring) -> exactly the 1x floor.
        let curve = ClusterFactorCurve::default_params();
        let members = [(1_000_000u64, 0u128), (2_000_000, 0)];
        let implied = ring_centroid_implied_factor(&members, &curve);
        assert_eq!(implied, curve.factor(0));
        assert_eq!(implied, FACTOR_SCALE); // 1x
    }

    #[test]
    fn test_ring_centroid_implied_factor_value_weighted() {
        // A large-value wealthy member dominates the centroid; a small fresh
        // background decoy barely moves it.
        let curve = ClusterFactorCurve::default_params();
        let members = [
            (100_000_000u64, 10_000_000_000_000_000_000u128), // big wealthy real input (10M BTH)
            (1_000, 0),                                       // tiny fresh background decoy
        ];
        let implied = ring_centroid_implied_factor(&members, &curve);
        // Centroid wealth ≈ 10M BTH (decoy barely moves it) -> still high.
        assert!(
            implied >= 5_000,
            "value-weighted implied factor = {implied}"
        );
    }

    #[test]
    fn test_ring_centroid_implied_factor_empty() {
        let curve = ClusterFactorCurve::default_params();
        assert_eq!(ring_centroid_implied_factor(&[], &curve), curve.factor(0));
    }

    #[test]
    fn test_ring_centroid_floor_changes_demurrage_outcome() {
        // End-to-end: a background-claimed factor pays zero demurrage, but the
        // ring-floored factor produces a real charge. The spender can no longer
        // escape demurrage by understating the output tags.
        let curve = ClusterFactorCurve::default_params();
        // Member wealth in picocredits: 10M BTH -> high implied factor.
        let members = [
            (1_000_000u64, 10_000_000_000_000_000_000u128),
            (1_000_000, 10_000_000_000_000_000_000),
        ];

        let claimed_factor = FACTOR_SCALE; // 1x background claim
        let charge_claimed = demurrage_charge(
            1_000_000,
            claimed_factor,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(charge_claimed, 0, "background claim pays no demurrage");

        let implied = ring_centroid_implied_factor(&members, &curve);
        let floored = claimed_factor.max(implied);
        let charge_floored =
            demurrage_charge(1_000_000, floored, BLOCKS_PER_YEAR, 200, BLOCKS_PER_YEAR);
        assert!(
            charge_floored > 0,
            "ring-floored factor must produce a real demurrage charge: {charge_floored}"
        );
    }

    #[test]
    fn test_churn_invariance() {
        // Paying demurrage at every churn sums to the same total as paying
        // once at the end: charge(T) = charge(T/2) + charge(T/2).
        let one_year = demurrage_charge(1_000_000, 6_000, BLOCKS_PER_YEAR, 200, BLOCKS_PER_YEAR);
        let half = demurrage_charge(1_000_000, 6_000, BLOCKS_PER_YEAR / 2, 200, BLOCKS_PER_YEAR);
        assert_eq!(one_year, half * 2);
    }

    /// #1685: the two-rate consensus kernel equals the single-rate one when
    /// the rates agree, and in the epoch-0 shape (accrual 0, exit 200) it
    /// still prices the downgrade while an in-class spend stays free.
    #[test]
    fn exit_rate_kernel_matches_single_rate_and_prices_epoch_zero_exits() {
        let v = 1_000_000_000_000_000u64; // 1000 BTH
        for (fi, fo) in [(5745u64, 1000u64), (5745, 5745), (1500, 1254), (1000, 1000)] {
            for elapsed in [0u64, 1, BLOCKS_PER_YEAR, 10 * BLOCKS_PER_YEAR] {
                for rate in [0u32, 200, 10_000] {
                    assert_eq!(
                        spend_demurrage_charge_with_exit_rate(
                            v,
                            fi,
                            fo,
                            elapsed,
                            rate,
                            rate,
                            BLOCKS_PER_YEAR
                        ),
                        spend_demurrage_charge(v, fi, fo, elapsed, rate, BLOCKS_PER_YEAR),
                        "fi {fi} fo {fo} elapsed {elapsed} rate {rate}"
                    );
                }
            }
        }

        // Epoch-0 rates: in-class accrual 0, exit pricing 200 bps.
        let old = 10 * BLOCKS_PER_YEAR;
        let in_class =
            spend_demurrage_charge_with_exit_rate(v, 5745, 5745, old, 0, 200, BLOCKS_PER_YEAR);
        assert_eq!(in_class, 0, "in-class holding stays free in epoch 0");
        let downgrade =
            spend_demurrage_charge_with_exit_rate(v, 5745, 1000, 0, 0, 200, BLOCKS_PER_YEAR);
        assert_eq!(
            downgrade,
            capitalized_reset_charge(
                v,
                5745,
                1000,
                SETTLEMENT_HORIZON_BLOCKS,
                200,
                BLOCKS_PER_YEAR
            )
        );
        assert_eq!(
            downgrade, 94_900_000_000_000,
            "5.745x -> 1x on 1000 BTH = 94.9 BTH"
        );
        // Settlement and import toll are the same capitalized term.
        assert_eq!(
            downgrade,
            demurrage_settlement_charge(v, 5745, 200, BLOCKS_PER_YEAR)
        );
        let toll = spend_demurrage_charge_with_exit_rate(v, 1475, 1254, 0, 0, 200, BLOCKS_PER_YEAR);
        assert!(toll > 0, "import toll priced in epoch 0");
    }

    /// REGRESSION (issue #834 → fix #925): the wealthy-holder class-reset
    /// escape is now **priced**. A holder who spends a young wealthy coin
    /// to a background output no longer pays ≈0 —
    /// [`spend_demurrage_charge`] charges the capitalized reset (the same
    /// capitalized future demurrage #831's settlement op charges), so the
    /// class transition costs the full price.
    ///
    /// This test walks the exact former-leak sequence and ASSERTS the new
    /// priced charge at the deflating step. It is the executable proof that
    /// the fix closes the #834 leak. The full write-up is in
    /// `docs/research/demurrage-background-reset-leak.md`.
    ///
    /// The mechanism reproduced here is the consensus fee-floor demurrage term
    /// (`Ledger::consensus_fee_floor` in `botho/src/ledger/store.rs`):
    /// `spend_demurrage_charge(output_sum, floor_factor, output_factor,
    /// elapsed@max, rate, bpy)`, which composes
    /// [`ring_centroid_implied_factor`] (the factor floor the spender
    /// cannot rewrite), [`demurrage_charge`] (the accrued-time charge) and
    /// [`capitalized_reset_charge`] (the priced class exit) — so the
    /// numbers are the real consensus numbers, not a re-derivation.
    #[test]
    fn demurrage_background_reset_leak_is_real() {
        use crate::ClusterFactorCurve;
        const PICO_PER_BTH: u128 = 1_000_000_000_000;
        let curve = ClusterFactorCurve::default_params();
        let rate_bps = 200u32; // 2%/yr at max factor

        // A wealthy cluster: 10M BTH of tagged wealth.
        let wealthy_cluster_wealth: u128 = 10_000_000 * PICO_PER_BTH;
        // The coin being manipulated: 1000 BTH (in picocredits, fits u64).
        let output_sum: u64 = 1_000 * PICO_PER_BTH as u64;

        // --- Baseline: what an HONEST wealthy holder pays if they simply hold
        //     the coin for a year and then spend it IN-CLASS (no downgrade). The
        //     ring members carry the wealthy cluster's public tags, so the
        //     implied factor floor is high; the coin is one year old, so
        //     elapsed = BLOCKS_PER_YEAR. Spent in-class, the capitalized term is
        //     0 and the holder pays only the accrued-to-date demurrage.
        let wealthy_ring = [(output_sum, wealthy_cluster_wealth)];
        let wealthy_factor = ring_centroid_implied_factor(&wealthy_ring, &curve);
        assert_eq!(
            wealthy_factor, 5745,
            "10M-BTH cluster maps to factor 5.745x (observed floor)"
        );
        let honest_annual_charge = spend_demurrage_charge(
            output_sum,
            wealthy_factor, // input floor
            wealthy_factor, // output declared in-class => no downgrade
            BLOCKS_PER_YEAR,
            rate_bps,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(
            honest_annual_charge, 18_980_000_000_000,
            "honest in-class wealthy holder pays ~18.98 BTH/yr on a 1000-BTH coin (accrued only)"
        );

        // --- Step 1 of the former attack: the whale re-spends the wealthy coin
        //     wealthy->wealthy to RESET its creation height (standard UTXO
        //     behavior). It times Step 2 to follow immediately, so the age it
        //     must pay for at Step 2 is ~0. We model the post-reset coin as
        //     elapsed = 0.

        // --- Step 2 (formerly THE LEAK, now PRICED): spend the now-YOUNG wealthy
        //     coin to a fully background-tagged output. The ring floor raises the
        //     input factor to the wealthy 5745 while the declared output factor
        //     is 1x — a genuine class DOWNGRADE. `spend_demurrage_charge` now
        //     charges the capitalized reset for the shed mass even though the
        //     accrued-to-date term is ~0 (elapsed ≈ 0).
        let young_elapsed = 0u64;
        let output_declared_factor = FACTOR_SCALE; // background 1x claim
        let reset_spend_charge = spend_demurrage_charge(
            output_sum,
            wealthy_factor,         // input floor (ring-implied, unrewritable)
            output_declared_factor, // declared background => downgrade
            young_elapsed,          // accrued term ≈ 0 ...
            rate_bps,
            BLOCKS_PER_YEAR,
        );
        // The charge equals the capitalized reset over the 5-year settlement
        // horizon: by linearity that is exactly 5 × one honest year at the floor.
        let expected_capitalized = capitalized_reset_charge(
            output_sum,
            wealthy_factor,
            output_declared_factor,
            SETTLEMENT_HORIZON_BLOCKS,
            rate_bps,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(
            reset_spend_charge, expected_capitalized,
            "the priced downgrade charge must equal the capitalized reset"
        );
        assert_eq!(
            reset_spend_charge, 94_900_000_000_000,
            "THE FIX: spending a young wealthy coin to background now costs the capitalized \
             reset = 5 × the honest annual ({honest_annual_charge}), i.e. ~94.9 BTH"
        );
        assert_eq!(
            reset_spend_charge,
            honest_annual_charge * 5,
            "capitalized over the 5yr horizon = 5 years of accrued demurrage, up front"
        );
        assert!(
            reset_spend_charge > 0,
            "the class transition is no longer free (leak closed)"
        );

        // --- After the reset: the output genuinely carries background tags, so
        //     future rings composed of background members imply factor 1x. A
        //     genuine background→background spend floors at 1x on BOTH sides,
        //     so there is no downgrade to price and the charge is zero — honest
        //     commerce is never touched.
        let background_ring = [(output_sum, 0u128)];
        let background_factor = ring_centroid_implied_factor(&background_ring, &curve);
        assert_eq!(
            background_factor, FACTOR_SCALE,
            "the reset coin's ring floor is now 1x (background)"
        );
        let future_charge_one_year = spend_demurrage_charge(
            output_sum,
            background_factor, // input floor 1x
            background_factor, // output 1x => no downgrade
            BLOCKS_PER_YEAR,
            rate_bps,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(
            future_charge_one_year, 0,
            "an honest background->background spend pays zero: floor 1x on both sides"
        );

        // --- The bottom line: shedding the wealthy class now costs the full
        //     capitalized reset up front (symmetric with #831's settlement op),
        //     which strictly exceeds the honest single-year accrued charge. The
        //     free exit is closed.
        assert!(
            reset_spend_charge > honest_annual_charge,
            "the priced class transition now costs MORE than one honest year: \
             {reset_spend_charge} > {honest_annual_charge}"
        );
    }

    // ---- #831: demurrage-settlement charge (the explicit opt-in door) ----

    /// The settlement charge REUSES #925's `capitalized_reset_charge` primitive
    /// verbatim (no reimplementation): `demurrage_settlement_charge` is exactly
    /// that primitive with the exit target pinned to factor-1/background and
    /// the shared `SETTLEMENT_HORIZON_BLOCKS`.
    #[test]
    fn settlement_charge_is_capitalized_reset_to_background() {
        let rate_bps = 200u32;
        let value = 1_000_000u64;
        for floor in [FACTOR_SCALE, 2_000, 3_500, 5_745, MAX_FACTOR_SCALED] {
            let settlement = demurrage_settlement_charge(value, floor, rate_bps, BLOCKS_PER_YEAR);
            let capitalized = capitalized_reset_charge(
                value,
                floor,
                FACTOR_SCALE, // background exit target
                SETTLEMENT_HORIZON_BLOCKS,
                rate_bps,
                BLOCKS_PER_YEAR,
            );
            assert_eq!(
                settlement, capitalized,
                "settlement charge must equal capitalized_reset_charge to background at floor {floor}"
            );
        }
    }

    /// A genuinely background (factor-1) coin settles for FREE — there is no
    /// wealthy class to shed, so honest commerce is never charged to wrap.
    #[test]
    fn settlement_charge_factor_one_is_free() {
        assert_eq!(
            demurrage_settlement_charge(1_000_000, FACTOR_SCALE, 200, BLOCKS_PER_YEAR),
            0,
            "background/factor-1 coin pays zero to settle"
        );
    }

    /// A wealthy coin pays a strictly positive settlement charge, and it is the
    /// full capitalized future demurrage over the horizon — i.e. exactly
    /// `H`-years of demurrage at the ring-floored class (linearity of
    /// `demurrage_charge` in elapsed). This is the "never a cheaper escape than
    /// holding" property the calibration relies on.
    #[test]
    fn settlement_charge_wealthy_equals_horizon_years_of_demurrage() {
        let rate_bps = 200u32;
        let value = 1_000_000u64;
        let floor = MAX_FACTOR_SCALED; // 6x
        let settle = demurrage_settlement_charge(value, floor, rate_bps, BLOCKS_PER_YEAR);
        assert!(
            settle > 0,
            "wealthy coin must pay a positive settlement charge"
        );

        // One year of ordinary in-class demurrage at the same floor.
        let one_year = demurrage_charge(value, floor, BLOCKS_PER_YEAR, rate_bps, BLOCKS_PER_YEAR);
        let horizon_years = SETTLEMENT_HORIZON_BLOCKS / BLOCKS_PER_YEAR; // 5
        assert_eq!(
            settle,
            one_year * horizon_years,
            "settling = paying {horizon_years} years of demurrage up front (churn-invariant \
             across the class transition: never cheaper than holding over the horizon)"
        );
    }

    /// The settlement charge is **elapsed-independent** (anti-instant-flip): a
    /// freshly-minted wealthy coin pays the SAME settlement charge as a
    /// year-old one, because the charge capitalizes a FIXED horizon rather than
    /// the accrued age. So a holder cannot mint→settle→wrap a young coin
    /// cheaply.
    #[test]
    fn settlement_charge_is_elapsed_independent() {
        let rate_bps = 200u32;
        let value = 1_000_000u64;
        let floor = 5_745u64;
        // demurrage_settlement_charge takes no elapsed at all — it is a pure
        // function of value/floor/rate/bpy — so any two "ages" give one number.
        let young = demurrage_settlement_charge(value, floor, rate_bps, BLOCKS_PER_YEAR);
        let old = demurrage_settlement_charge(value, floor, rate_bps, BLOCKS_PER_YEAR);
        assert_eq!(young, old, "settlement charge does not depend on coin age");
        assert!(young > 0);
    }

    /// Determinism: the settlement charge is a pure integer function —
    /// identical across repeated calls (no float, no map iteration, no
    /// node-local state).
    #[test]
    fn settlement_charge_is_deterministic() {
        let a = demurrage_settlement_charge(1_234_567, 4_321, 200, BLOCKS_PER_YEAR);
        let b = demurrage_settlement_charge(1_234_567, 4_321, 200, BLOCKS_PER_YEAR);
        assert_eq!(a, b);
    }

    /// The opt-in settlement charge and the implicit #925 downgrade charge are
    /// two doors on the SAME horizon: settling a wealthy coin to background
    /// costs exactly what a #925 spend-to-background of a young coin costs — so
    /// neither door is a cheaper route than the other.
    #[test]
    fn settlement_and_implicit_downgrade_charge_match() {
        let rate_bps = 200u32;
        let value = 2_000_000u64;
        let floor = 5_000u64;
        let settle = demurrage_settlement_charge(value, floor, rate_bps, BLOCKS_PER_YEAR);
        // #925 implicit charge for a YOUNG (elapsed=0) wealthy->background spend.
        let implicit = spend_demurrage_charge(
            value,
            floor,        // ring-floored input class
            FACTOR_SCALE, // declared background output
            0,            // young: accrued ≈ 0, capitalized dominates
            rate_bps,
            BLOCKS_PER_YEAR,
        );
        assert_eq!(
            settle, implicit,
            "the opt-in settlement price equals the implicit downgrade price on the shared horizon"
        );
    }

    // ========================================================================
    // Independent Rust reproductions of codex's Lean findings #1875 / #1876.
    // #1875 is FIXED (exact arithmetic, protocol 9.0.0): its reproduction now
    // asserts the fix. #1876 pins current behaviour until its own fix lands.
    // ========================================================================

    /// FIXED #1875 (codex; Lean `six_block_dead_zone`, `churn_counterexample`
    /// are the pre-9.0.0 witnesses, kept as the legacy model
    /// `Botho.Demurrage.charge`). Before 9.0.0 `demurrage_charge` folded
    /// elapsed time into `TIME_SCALE = 1e6` units of a 6,307,200-block year
    /// BEFORE applying the value, so `elapsed * 1e6 / 6_307_200 == 0` for
    /// every `elapsed <= 6`: any value accrued zero demurrage for a hold of up
    /// to six blocks, and same-class churn every six blocks paid zero for a
    /// whole year.
    ///
    /// The kernel now takes one floor of the full rational charge, so every
    /// positive-rate, positive-progressivity hold of a large value pays, and
    /// splitting a hold into n spends loses less than n pico in total.
    #[test]
    fn bug_1875_short_hold_demurrage_charged_since_9_0_0() {
        let bpy = BLOCKS_PER_YEAR_5S;
        let rate = 200u32;

        // No dead zone: elapsed 1..=6 now charges, in-class too.
        for elapsed in 1..=6u64 {
            for value in [1_000_000_000_000u64, 1_000_000_000_000_000_000, u64::MAX] {
                let charge = demurrage_charge(value, MAX_FACTOR_SCALED, elapsed, rate, bpy);
                assert!(charge > 0, "elapsed={elapsed} value={value}");
                // The consensus composition (`spend_demurrage_charge`) for an
                // in-class spend (no downgrade) is the accrued charge.
                assert_eq!(
                    spend_demurrage_charge(
                        value,
                        MAX_FACTOR_SCALED,
                        MAX_FACTOR_SCALED,
                        elapsed,
                        rate,
                        bpy
                    ),
                    charge,
                    "in-class spend elapsed={elapsed} value={value}"
                );
            }
        }
        // Pinned exact values (floor(v·200·5000·e / (5e7·6_307_200))).
        // u64::MAX at 6x: 1 block = ~0.0585 BTH; 7 blocks (the old first
        // non-zero block, 368_934_881_474 pre-fix) now 409_459_692_148.
        assert_eq!(
            demurrage_charge(u64::MAX, MAX_FACTOR_SCALED, 1, rate, bpy),
            58_494_241_735
        );
        assert_eq!(
            demurrage_charge(u64::MAX, MAX_FACTOR_SCALED, 6, rate, bpy),
            350_965_450_413
        );
        assert_eq!(
            demurrage_charge(u64::MAX, MAX_FACTOR_SCALED, 7, rate, bpy),
            409_459_692_148
        );

        // Churn counterexample (issue's table): 1M BTH at 6x.
        let value = 1_000_000_000_000_000_000u64; // 1M BTH in picocredits
        let one = demurrage_charge(value, MAX_FACTOR_SCALED, 1, rate, bpy);
        let six = demurrage_charge(value, MAX_FACTOR_SCALED, 6, rate, bpy);
        let seven = demurrage_charge(value, MAX_FACTOR_SCALED, 7, rate, bpy);
        let twelve = demurrage_charge(value, MAX_FACTOR_SCALED, 12, rate, bpy);
        assert_eq!(one, 3_170_979_198);
        assert_eq!(six, 19_025_875_190, "six blocks: ~0.019 BTH (was 0)");
        assert_eq!(seven, 22_196_854_388);
        assert_eq!(
            twelve, 38_051_750_380,
            "twelve blocks: ~0.038 BTH (was 0.02)"
        );
        // Two six-block spends equal one twelve-block spend within the split
        // bound: an n-way split loses less than n pico (here 0 <= loss < 2).
        assert!(six + six <= twelve && twelve - (six + six) < 2);
        // No longer one TIME_SCALE unit for every 7..=12 block hold.
        assert!(seven < twelve);

        // A year of six-block churn (1,051,200 hops) now pays the one-year
        // hold's 20,000 BTH less under 1,051,200 pico (~1.05e-6 BTH).
        let hops = bpy / 6;
        assert_eq!(hops, 1_051_200);
        let churned: u64 = (0..hops)
            .map(|_| demurrage_charge(value, MAX_FACTOR_SCALED, 6, rate, bpy))
            .sum();
        let held = demurrage_charge(value, MAX_FACTOR_SCALED, bpy, rate, bpy);
        assert_eq!(held, 20_000_000_000_000_000, "20,000 BTH/year at 6x, 2%");
        assert!(churned <= held && held - churned < hops, "{held} {churned}");
        assert_eq!(churned, 19_999_999_999_728_000);
    }

    /// #1875 split bound, swept: for any split of a hold into n consecutive
    /// spends at the same class, the n charges sum to at most the unsplit
    /// charge and lose less than n pico (each floor drops < 1 pico). Also
    /// monotone in elapsed.
    #[test]
    fn fix_1875_split_loss_below_n_pico() {
        let mut state = 0x1875_0009_u64;
        let mut next = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        for i in 0..20_000u64 {
            let value = next() >> (next() % 40);
            let factor = 1_000 + next() % 5_001;
            let rate = [1u32, 200, 10_000][(i % 3) as usize];
            let bpy = if i % 2 == 0 {
                BLOCKS_PER_YEAR_5S
            } else {
                1 + next() % 10_000_000
            };
            let n = 1 + next() % 16;
            let parts: Vec<u64> = (0..n).map(|_| next() % 100_000).collect();
            let total: u64 = parts.iter().sum();
            let whole = demurrage_charge(value, factor, total, rate, bpy);
            if whole == u64::MAX {
                continue; // clamped: the split bound is about the exact range
            }
            let split: u128 = parts
                .iter()
                .map(|&e| demurrage_charge(value, factor, e, rate, bpy) as u128)
                .sum();
            let whole = whole as u128;
            assert!(split <= whole, "v={value} f={factor} r={rate} b={bpy}");
            assert!(
                whole - split < n as u128,
                "v={value} f={factor} r={rate} b={bpy} n={n}"
            );
            assert!(
                demurrage_charge(value, factor, total + 1, rate, bpy) as u128 >= whole,
                "monotone in elapsed"
            );
        }
    }

    #[test]
    fn test_ring_elapsed_centroid_full_width_and_single_floor() {
        let m = u64::MAX;
        assert_eq!(ring_elapsed_centroid(&[(m, 0), (m, 0)], m), m);
        assert_eq!(ring_elapsed_centroid(&[(m, 0); 4], m), m);
        assert_eq!(ring_elapsed_centroid(&[(1, 0), (3, 8)], 10), 4);
        assert_eq!(ring_elapsed_centroid(&[(1, 0), (3, 7)], 10), 4);
        assert_eq!(ring_elapsed_centroid(&[(1, 9), (1, 9)], 10), 1);
        assert_eq!(ring_elapsed_centroid(&[(m, 0), (1, 10)], 10), 9);
        assert_eq!(ring_elapsed_centroid(&[(0, 0), (7, 8), (0, m)], 10), 2);
        assert_eq!(ring_elapsed_centroid(&[(m, m)], 10), 0);
        assert_eq!(ring_elapsed_centroid(&[(0, 0), (0, m)], m), 0);
        assert_eq!(ring_elapsed_centroid(&[], m), 0);
    }

    /// Pre-9.0.0 `ring_centroid_implied_factor` centroid, kept verbatim as the
    /// reference for the #1876 fix: the numerator saturates at `u128::MAX`
    /// and is then divided by the unsaturated total value.
    fn centroid_pre_1876(members: &[(u64, u128)]) -> u128 {
        let mut weighted: u128 = 0;
        let mut total_value: u128 = 0;
        for &(value, member_wealth) in members {
            weighted = weighted.saturating_add((value as u128).saturating_mul(member_wealth));
            total_value = total_value.saturating_add(value as u128);
        }
        weighted.checked_div(total_value).unwrap_or(0)
    }

    /// Reference exact centroid over the 256-bit accumulator.
    fn centroid_exact(members: &[(u64, u128)]) -> u128 {
        let mut m = crate::wide::WeightedMean::new();
        for &(v, w) in members {
            m.add_product(v as u128, w);
            m.add_weight(v as u128);
        }
        m.floor()
    }

    /// FIXED #1876 (codex; Lean `homogeneous_ring_scale_counterexample`,
    /// `homogeneous_centroid_counterexample`, `saturation_lowers_charge`;
    /// reproduced in #1899). Before protocol 9.0.0
    /// `ring_centroid_implied_factor` saturated `Σ value·wealth` at
    /// `u128::MAX` and THEN divided by the exact `Σ value`, so on overflow the
    /// centroid was `u128::MAX / Σ value`, LOWER than the true (homogeneous)
    /// wealth: 20 × (1M BTH, W=1e20) gave centroid 17,014,118,346,046,923,173
    /// and factor 5841 instead of 5949, and a 10M BTH whale coin in a 1B BTH
    /// cluster was charged a 5919 floor instead of 6000 (16,200 BTH less on
    /// the #925 downgrade charge).
    ///
    /// The centroid is now the exact rational floor, so it is scale invariant:
    /// a homogeneous ring's centroid is its common wealth at every value scale.
    #[test]
    fn bug_1876_ring_saturation_lowers_fee_floor() {
        let curve = ClusterFactorCurve::default();
        let w: u128 = 100_000_000_000_000_000_000; // 100M BTH

        // Issue's homogeneous rings: twenty members with identical wealth.
        let small = vec![(100_000_000_000_000_000u64, w); 20]; // 100k BTH each
        let large = vec![(1_000_000_000_000_000_000u64, w); 20]; // 1M BTH each
        assert_eq!(curve.factor(w), 5949, "true factor of the common wealth");
        assert_eq!(ring_centroid_implied_factor(&small, &curve), 5949);
        assert_eq!(
            ring_centroid_implied_factor(&large, &curve),
            5949,
            "scale invariance: 10x every value keeps the homogeneous floor"
        );
        assert_eq!(centroid_exact(&small), w);
        assert_eq!(centroid_exact(&large), w, "exact centroid = common wealth");

        // The large ring's numerator (2e39) does overflow u128: the old code
        // returned u128::MAX / Σv here (the pre-fix Lean witness).
        assert!((20u128 * 1_000_000_000_000_000_000u128)
            .checked_mul(w)
            .is_none());
        assert_eq!(centroid_pre_1876(&large), 17_014_118_346_046_923_173);
        assert_eq!(curve.factor(centroid_pre_1876(&large)), 5841);

        // Whale-scale single-coin rings: a 1M / 10M BTH coin plus nineteen
        // 1-BTH decoys, all in a cluster with 1B BTH cumulative wealth (true
        // factor 6000; the accumulator is monotonic, so this is cumulative
        // tagged throughput, not holdings). Pre-fix floors were 5985 / 5919.
        let w_big: u128 = 1_000_000_000_000_000_000_000;
        assert_eq!(curve.factor(w_big), 6000);
        let ring_with = |coin: u64| {
            let mut r = vec![(coin, w_big)];
            r.extend(std::iter::repeat_n((1_000_000_000_000u64, w_big), 19));
            r
        };
        let one_m = 1_000_000_000_000_000_000u64;
        let ten_m = 10_000_000_000_000_000_000u64;
        assert_eq!(curve.factor(centroid_pre_1876(&ring_with(one_m))), 5985);
        assert_eq!(curve.factor(centroid_pre_1876(&ring_with(ten_m))), 5919);
        let f1 = ring_centroid_implied_factor(&ring_with(one_m), &curve);
        let f10 = ring_centroid_implied_factor(&ring_with(ten_m), &curve);
        assert_eq!(f1, 6000);
        assert_eq!(f10, 6000);

        // Consensus-relevant charge: the #925 capitalized downgrade charge
        // (young wealthy coin to background) is computed from this floor and
        // is now the full 6x charge.
        let charge =
            |v: u64, f: u64| spend_demurrage_charge(v, f, FACTOR_SCALE, 0, 200, BLOCKS_PER_YEAR);
        assert_eq!(charge(one_m, f1), 100_000_000_000_000_000); // 100,000 BTH
        assert_eq!(charge(ten_m, f10), 1_000_000_000_000_000_000); // 1,000,000 BTH

        // Worst cases the old code allowed are gone too: a ring of 20
        // u64::MAX coins in a 1B BTH cluster now floors at 6000, not 5029.
        let maxed = vec![(u64::MAX, w_big); 20];
        assert_eq!(curve.factor(centroid_pre_1876(&maxed)), 5029);
        assert_eq!(ring_centroid_implied_factor(&maxed, &curve), 6000);
    }

    /// #1876 boundary: the exact centroid equals the old one whenever the old
    /// u128 numerator did not overflow, and differs exactly from the first
    /// overflowing numerator on.
    #[test]
    fn bug_1876_exact_versus_old_boundary() {
        // Numerator exactly u128::MAX: no overflow, identical.
        let at = [(1u64, u128::MAX)];
        assert_eq!(centroid_pre_1876(&at), u128::MAX);
        assert_eq!(centroid_exact(&at), u128::MAX);

        // Numerator u128::MAX + 1 = 2^128: the old code saturated and gave
        // floor((2^128 - 1) / 2) = 2^127 - 1; the exact mean is 2^127.
        let over = [(1u64, u128::MAX), (1u64, 1)];
        assert_eq!(centroid_pre_1876(&over), (1u128 << 127) - 1);
        assert_eq!(centroid_exact(&over), 1u128 << 127);

        // Largest homogeneous ring the old code handled exactly: Σv·w <= MAX.
        let w: u128 = 100_000_000_000_000_000_000;
        let v_ok = (u128::MAX / w / 20) as u64;
        let ok = vec![(v_ok, w); 20];
        let bad = vec![(v_ok + 1, w); 20];
        assert!((20 * v_ok as u128).checked_mul(w).is_some());
        assert!((20 * (v_ok as u128 + 1)).checked_mul(w).is_none());
        assert_eq!(centroid_pre_1876(&ok), w);
        assert_eq!(centroid_exact(&ok), w);
        assert!(centroid_pre_1876(&bad) < w, "old: saturation lowers it");
        assert_eq!(centroid_exact(&bad), w, "new: exact");

        // Deterministic sweep: wherever the old numerator fits, new == old.
        // Wherever it overflows, new >= old (the old result was always low).
        let mut x: u64 = 0x1876_1876_1876_1876;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..2_000 {
            let n = (next() % 24) as usize;
            let members: Vec<(u64, u128)> = (0..n)
                .map(|_| {
                    let v = next() >> (next() % 64);
                    let w = (((next() as u128) << 64) | next() as u128) >> (next() % 128);
                    (v, w)
                })
                .collect();
            let fits = members.iter().try_fold(0u128, |s, &(v, w)| {
                (v as u128).checked_mul(w).and_then(|p| s.checked_add(p))
            });
            let (old, new) = (centroid_pre_1876(&members), centroid_exact(&members));
            if fits.is_some() {
                assert_eq!(old, new, "{members:?}");
            } else {
                assert!(new >= old, "{members:?}");
            }
            // The mean never exceeds the largest member wealth.
            let max_w = members.iter().map(|m| m.1).max().unwrap_or(0);
            assert!(new <= max_w);
            assert_eq!(
                ring_centroid_implied_factor(&members, &ClusterFactorCurve::default()),
                ClusterFactorCurve::default().factor(new)
            );
        }
    }

    /// #1876: the true ring centroid can never exceed u128::MAX. A weighted
    /// mean is at most its largest term and every member wealth is a u128, so
    /// the final clamp in `WeightedMean::floor` is unreachable from this
    /// kernel. The extreme ring (every value and wealth at its maximum) is
    /// exact and maps to the maximum factor.
    #[test]
    fn bug_1876_true_mean_above_u128_unreachable() {
        let curve = ClusterFactorCurve::default();
        let extreme = vec![(u64::MAX, u128::MAX); 40];
        assert_eq!(centroid_exact(&extreme), u128::MAX);
        assert_eq!(ring_centroid_implied_factor(&extreme, &curve), 6000);
        let mixed = vec![(u64::MAX, u128::MAX), (1, 0), (u64::MAX, u128::MAX - 1)];
        // floor(((2^64-1)(2^128-1) + (2^64-1)(2^128-2)) / (2^65-1)), computed
        // independently with arbitrary-precision integers.
        assert_eq!(
            centroid_exact(&mixed),
            340_282_366_920_938_463_454_151_235_394_913_435_646
        );
    }
}

/// Differential tests for the exact #1875 kernel (protocol 9.0.0).
///
/// `demurrage_charge` is compared against an independent 256-bit oracle of
/// the exact rational charge, `floor(value × rate × p × elapsed / (10_000 ×
/// 5_000 × bpy))` clamped to `u64::MAX`, over the whole u64/u32 input domain.
/// Kept for the record (CLAUDE.md: preserve, don't delete):
///
/// - `legacy_quantized_charge`: the pre-9.0.0 kernel (time fraction floored at
///   `1e6` units first, then `saturating_mul`, then `try_from`), which was the
///   #1820 Aeneas-rewrite reference. The exact kernel never charges less.
/// - `study_exact_charge`: the u128 candidate from the #1875 simulation study
///   (PR #1908, `exact_charge`), exact only for `bpy <= STUDY_EXACT_BPY_LIMIT`
///   and saturating upward beyond. The production kernel splits `elapsed` so it
///   is exact everywhere; the tests below confirm both claims.
#[cfg(test)]
mod exact_kernel_drt {
    use super::*;

    /// Pre-9.0.0 (pre-#1875-fix) `demurrage_charge`, verbatim from before
    /// #1820 (reference copy, the historical quantized semantics).
    fn legacy_quantized_charge(
        transfer_value: u64,
        cluster_factor: u64,
        elapsed_blocks: u64,
        rate_bps: u32,
        blocks_per_year: u64,
    ) -> u64 {
        if rate_bps == 0 || elapsed_blocks == 0 || blocks_per_year == 0 {
            return 0;
        }
        let factor = cluster_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
        let progressivity = factor - FACTOR_SCALE;
        if progressivity == 0 {
            return 0;
        }
        const TIME_SCALE: u128 = 1_000_000;
        let time_fraction = (elapsed_blocks as u128 * TIME_SCALE) / blocks_per_year as u128;
        let charge = transfer_value as u128 * rate_bps as u128 * progressivity as u128
            / 10_000
            / (MAX_FACTOR_SCALED - FACTOR_SCALE) as u128;
        let charge = charge.saturating_mul(time_fraction) / TIME_SCALE;
        u64::try_from(charge).unwrap_or(u64::MAX)
    }

    const DEN_PER_BPY: u128 = 10_000 * (MAX_FACTOR_SCALED - FACTOR_SCALE) as u128;

    /// The study's bound: `rem × elapsed` fits u128 for all u64 inputs iff
    /// `10_000 × 5_000 × bpy <= 2^64 + 2`.
    const STUDY_EXACT_BPY_LIMIT: u64 = (((1u128 << 64) + 2) / DEN_PER_BPY) as u64;

    /// The #1908 study's `exact_charge` (round_up = false), verbatim logic.
    fn study_exact_charge(value: u64, factor: u64, elapsed: u64, rate_bps: u32, bpy: u64) -> u64 {
        if rate_bps == 0 || elapsed == 0 || bpy == 0 {
            return 0;
        }
        let p = factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED) - FACTOR_SCALE;
        if p == 0 {
            return 0;
        }
        let num = value as u128 * rate_bps as u128 * p as u128;
        let den = DEN_PER_BPY * bpy as u128;
        let elapsed = elapsed as u128;
        let Some(whole) = (num / den).checked_mul(elapsed) else {
            return u64::MAX;
        };
        let Some(rem_scaled) = (num % den).checked_mul(elapsed) else {
            return u64::MAX;
        };
        whole.saturating_add(rem_scaled / den).min(u64::MAX as u128) as u64
    }

    /// `a × b` as a 256-bit `(hi, lo)` pair.
    fn mul_wide(a: u128, b: u128) -> (u128, u128) {
        const M: u128 = u64::MAX as u128;
        let (a1, a0) = (a >> 64, a & M);
        let (b1, b0) = (b >> 64, b & M);
        let p00 = a0 * b0;
        let p01 = a0 * b1;
        let p10 = a1 * b0;
        let p11 = a1 * b1;
        let mid = (p00 >> 64) + (p01 & M) + (p10 & M);
        let lo = (p00 & M) | (mid << 64);
        let hi = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
        (hi, lo)
    }

    /// `(hi, lo) / d` by binary long division: `(q_hi, q_lo)`.
    fn div_wide(hi: u128, lo: u128, d: u128) -> (u128, u128) {
        let (mut q_hi, mut q_lo, mut rem) = (0u128, 0u128, 0u128);
        for i in (0..256u32).rev() {
            let bit = if i >= 128 {
                (hi >> (i - 128)) & 1
            } else {
                (lo >> i) & 1
            };
            let carry = rem >> 127;
            rem = (rem << 1) | bit;
            if carry == 1 || rem >= d {
                rem = rem.wrapping_sub(d);
                if i >= 128 {
                    q_hi |= 1 << (i - 128);
                } else {
                    q_lo |= 1 << i;
                }
            }
        }
        (q_hi, q_lo)
    }

    /// The exact rational charge by 256-bit arithmetic, clamped to u64::MAX.
    fn oracle(value: u64, factor: u64, elapsed: u64, rate_bps: u32, bpy: u64) -> u64 {
        let p = factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED) - FACTOR_SCALE;
        if rate_bps == 0 || elapsed == 0 || bpy == 0 || p == 0 {
            return 0;
        }
        let num = value as u128 * rate_bps as u128 * p as u128; // < 2^109
        let den = DEN_PER_BPY * bpy as u128; // < 2^90
        let (hi, lo) = mul_wide(num, elapsed as u128);
        let (q_hi, q_lo) = div_wide(hi, lo, den);
        if q_hi != 0 || q_lo > u64::MAX as u128 {
            u64::MAX
        } else {
            q_lo as u64
        }
    }

    /// Exact composition reference for `capitalized_reset_charge`.
    fn capitalized_reset_oracle(
        v: u64,
        fi: u64,
        fo: u64,
        horizon: u64,
        rate_bps: u32,
        bpy: u64,
    ) -> u64 {
        if fo >= fi {
            return 0;
        }
        oracle(v, fi, horizon, rate_bps, bpy).saturating_sub(oracle(v, fo, horizon, rate_bps, bpy))
    }

    struct Gen(u64);

    impl Gen {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn edge_u64(&mut self) -> u64 {
            match self.next() % 6 {
                0 => [0, 1, 2, u64::MAX, u64::MAX - 1, u64::MAX / 2][(self.next() % 6) as usize],
                1 => {
                    let p = 1u64 << (self.next() % 64);
                    match self.next() % 3 {
                        0 => p - 1,
                        1 => p,
                        _ => p.wrapping_add(1),
                    }
                }
                2 => self.next() % 32,
                _ => self.next(),
            }
        }

        fn factor(&mut self) -> u64 {
            match self.next() % 3 {
                0 => [0, 999, 1_000, 1_001, 5_999, 6_000, 6_001, u64::MAX]
                    [(self.next() % 8) as usize],
                1 => 1_000 + self.next() % 5_001,
                _ => self.edge_u64(),
            }
        }

        fn rate(&mut self) -> u32 {
            match self.next() % 3 {
                0 => [0, 1, 200, 10_000, u32::MAX - 1, u32::MAX][(self.next() % 6) as usize],
                _ => self.edge_u64() as u32,
            }
        }
    }

    const VALUES: [u64; 7] = [
        0,
        1,
        1_000_000,
        1_000_000_000_000,
        u64::MAX / 2,
        u64::MAX - 1,
        u64::MAX,
    ];
    const FACTORS: [u64; 7] = [0, 1_000, 1_001, 3_500, 6_000, 6_001, u64::MAX];
    const ELAPSED: [u64; 9] = [
        0,
        1,
        6,
        7,
        6_307_200,
        SETTLEMENT_HORIZON_BLOCKS,
        1 << 32,
        u64::MAX - 1,
        u64::MAX,
    ];
    const RATES: [u32; 5] = [0, 1, 200, 10_000, u32::MAX];
    // Includes the zero divisor (blocks_per_year == 0 returns early) and both
    // sides of the study's exactness limit.
    const BPY: [u64; 8] = [
        0,
        1,
        2,
        BLOCKS_PER_YEAR_5S,
        STUDY_EXACT_BPY_LIMIT,
        STUDY_EXACT_BPY_LIMIT + 1,
        u64::MAX / 2,
        u64::MAX,
    ];

    #[test]
    fn exact_kernel_matches_u256_on_grid() {
        for &v in &VALUES {
            for &f in &FACTORS {
                for &e in &ELAPSED {
                    for &r in &RATES {
                        for &b in &BPY {
                            let exact = oracle(v, f, e, r, b);
                            assert_eq!(
                                demurrage_charge(v, f, e, r, b),
                                exact,
                                "v={v} f={f} e={e} r={r} b={b}"
                            );
                            assert!(legacy_quantized_charge(v, f, e, r, b) <= exact);
                        }
                    }
                }
            }
        }
    }

    /// The u128 overflow branch (`whole × elapsed`) is reachable and is the
    /// exact clamp: the true charge exceeds 2^128.
    #[test]
    fn exact_kernel_overflow_saturates_to_u64_max() {
        let (v, f, e, r, b) = (u64::MAX, 6_000, u64::MAX, u32::MAX, 1);
        let num = v as u128 * r as u128 * 5_000;
        assert!((num / DEN_PER_BPY).checked_mul(e as u128).is_none());
        assert_eq!(demurrage_charge(v, f, e, r, b), u64::MAX);
        assert_eq!(oracle(v, f, e, r, b), u64::MAX);
        assert_eq!(legacy_quantized_charge(v, f, e, r, b), u64::MAX);
    }

    /// The study's claim, verified: its u128-only `exact_charge` equals the
    /// 256-bit oracle for every `bpy <= STUDY_EXACT_BPY_LIMIT`
    /// (368_934_881_474, any block time above ~86 µs) and beyond that only
    /// ever saturates UPWARD. The production kernel's elapsed split removes
    /// that region: it is exact there too.
    #[test]
    fn study_exact_charge_limit_claim_holds() {
        assert_eq!(STUDY_EXACT_BPY_LIMIT, 368_934_881_474);
        // At the limit the remainder product cannot overflow.
        let den = DEN_PER_BPY * STUDY_EXACT_BPY_LIMIT as u128;
        assert!((den - 1).checked_mul(u64::MAX as u128).is_some());
        let den = DEN_PER_BPY * (STUDY_EXACT_BPY_LIMIT as u128 + 1);
        assert!((den - 1).checked_mul(u64::MAX as u128).is_none());

        let mut g = Gen(0x1875_1908);
        let mut beyond_saturated = 0u32;
        for i in 0..100_000u32 {
            let (v, f, e, r) = (g.edge_u64(), g.factor(), g.edge_u64(), g.rate());
            let b = match i % 3 {
                0 => g.next() % (STUDY_EXACT_BPY_LIMIT + 1),
                1 => STUDY_EXACT_BPY_LIMIT + 1 + g.next() % (u64::MAX - STUDY_EXACT_BPY_LIMIT),
                _ => g.edge_u64(),
            };
            let exact = oracle(v, f, e, r, b);
            let study = study_exact_charge(v, f, e, r, b);
            assert_eq!(
                demurrage_charge(v, f, e, r, b),
                exact,
                "v={v} f={f} e={e} r={r} b={b}"
            );
            if b <= STUDY_EXACT_BPY_LIMIT {
                assert_eq!(study, exact, "v={v} f={f} e={e} r={r} b={b}");
            } else {
                assert!(
                    study == exact || study == u64::MAX,
                    "v={v} f={f} e={e} r={r} b={b}"
                );
                beyond_saturated += u32::from(study != exact);
            }
        }
        assert!(
            beyond_saturated > 0,
            "the study kernel's upward saturation is reachable"
        );
    }

    /// The clamp rewrite, checked directly at the u64 boundary.
    #[test]
    fn min_cast_is_try_from_unwrap_or_max() {
        let edges = [
            0u128,
            1,
            u64::MAX as u128 - 1,
            u64::MAX as u128,
            u64::MAX as u128 + 1,
            1u128 << 100,
            u128::MAX - 1,
            u128::MAX,
        ];
        for &c in &edges {
            assert_eq!(
                c.min(u64::MAX as u128) as u64,
                u64::try_from(c).unwrap_or(u64::MAX),
                "{c}"
            );
        }
    }

    #[test]
    fn capitalized_reset_charge_matches_exact_on_grid() {
        for &v in &VALUES {
            for &fi in &FACTORS {
                for &fo in &FACTORS {
                    for &e in &ELAPSED {
                        for &r in &RATES {
                            for &b in &BPY {
                                assert_eq!(
                                    capitalized_reset_charge(v, fi, fo, e, r, b),
                                    capitalized_reset_oracle(v, fi, fo, e, r, b),
                                    "v={v} in={fi} out={fo} e={e} r={r} b={b}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn demurrage_kernels_match_exact_sweep() {
        let mut g = Gen(0x1820_0020);
        for _ in 0..200_000 {
            let (v, f, fo, e, r, b) = (
                g.edge_u64(),
                g.factor(),
                g.factor(),
                g.edge_u64(),
                g.rate(),
                g.edge_u64(),
            );
            let exact = oracle(v, f, e, r, b);
            assert_eq!(
                demurrage_charge(v, f, e, r, b),
                exact,
                "v={v} f={f} e={e} r={r} b={b}"
            );
            assert!(legacy_quantized_charge(v, f, e, r, b) <= exact);
            assert_eq!(
                capitalized_reset_charge(v, f, fo, e, r, b),
                capitalized_reset_oracle(v, f, fo, e, r, b),
                "v={v} in={f} out={fo} e={e} r={r} b={b}"
            );
        }
    }

    /// Pinned short-elapsed and large-value fixtures (#1875): exact values,
    /// with the legacy (pre-9.0.0) value alongside.
    #[test]
    fn fix_1875_pinned_fixtures() {
        const Y: u64 = BLOCKS_PER_YEAR_5S;
        const BTH: u64 = 1_000_000_000_000;
        // (value, factor, elapsed, rate, exact, legacy)
        let cases: [(u64, u64, u64, u32, u64, u64); 10] = [
            (1_000_000 * BTH, 6_000, 1, 200, 3_170_979_198, 0),
            (1_000_000 * BTH, 6_000, 6, 200, 19_025_875_190, 0),
            (
                1_000_000 * BTH,
                6_000,
                7,
                200,
                22_196_854_388,
                20_000_000_000,
            ),
            (
                1_000_000 * BTH,
                6_000,
                12,
                200,
                38_051_750_380,
                20_000_000_000,
            ),
            (
                1_000_000 * BTH,
                6_000,
                Y,
                200,
                20_000_000_000_000_000,
                20_000_000_000_000_000,
            ),
            (u64::MAX, 6_000, 1, 200, 58_494_241_735, 0),
            (u64::MAX, 6_000, 6, 200, 350_965_450_413, 0),
            (u64::MAX, 6_000, 7, 200, 409_459_692_148, 368_934_881_474),
            (900_000 * BTH, 5_745, 6, 200, 16_250_000_000, 0),
            (1, 6_000, 1, 200, 0, 0),
        ];
        for (v, f, e, r, exact, legacy) in cases {
            assert_eq!(demurrage_charge(v, f, e, r, Y), exact, "v={v} f={f} e={e}");
            assert_eq!(oracle(v, f, e, r, Y), exact, "oracle v={v} f={f} e={e}");
            assert_eq!(
                legacy_quantized_charge(v, f, e, r, Y),
                legacy,
                "legacy v={v} f={f} e={e}"
            );
        }
    }
}
