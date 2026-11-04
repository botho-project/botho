//! Frozen copy of the **pre-9.0.0** quantized demurrage kernel (#1914).
//!
//! The circuit in this crate encodes the time fraction as
//! `elapsed * 1_000_000 / blocks_per_year` (a separate `TIME_SCALE` floor
//! before the charge floor). That was production
//! `bth_cluster_tax::demurrage_charge` through protocol 8.x. Protocol 9.0.0
//! (#1875 / PR #1913) removes the time quantization from production, so the
//! circuit's differential tests compare against this frozen reference instead
//! of the live production functions.
//!
//! These bodies are copied verbatim (modulo the Aeneas-only spellings, which
//! are arithmetically identical) from `cluster-tax/src/demurrage.rs` at main
//! commit `326a87c8` (last kernel change `58be9e9e`, #1838). They are
//! deliberately NOT asserted equal to production: after 9.0.0 they differ by
//! design. Re-deriving the circuit for the exact 9.0.0 single-floor charge, and
//! re-recording its resource measurements, is the open follow-up in #1914.
//!
//! Test-only research reference; not consensus code.

/// Fixed-point scale for cluster factors (1000 = 1.0x).
pub const FACTOR_SCALE: u64 = 1000;
/// Maximum cluster factor in `FACTOR_SCALE` units.
pub const MAX_FACTOR_SCALED: u64 = 6 * FACTOR_SCALE;
/// Blocks per year at the 5s reference block time.
pub const BLOCKS_PER_YEAR_5S: u64 = 6_307_200;
/// Shared five-year capitalization horizon (ADR 0003 / #833).
pub const SETTLEMENT_HORIZON_BLOCKS: u64 = 5 * BLOCKS_PER_YEAR_5S;

/// Pre-9.0.0 quantized `demurrage_charge`.
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
    charge.min(u64::MAX as u128) as u64
}

/// Pre-9.0.0 `capitalized_reset_charge`, built on the quantized kernel.
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

/// Pre-9.0.0 `spend_demurrage_charge`: `max(accrued, capitalized)`.
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
