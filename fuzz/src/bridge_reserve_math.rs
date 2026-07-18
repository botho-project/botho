// Copyright (c) 2024 The Botho Foundation

//! Shared harness body for the proof-of-reserves peg-drift math (fuzz target
//! `fuzz_bridge_reserve_math`; #1078, Phase 3 of bridge epic #816).
//!
//! Security rationale: the reserve reconciler is the peg's safety valve. It
//! computes the drift between the locked BTH reserve and the outstanding wBTH
//! supply across chains, applies a tolerance, flips `peg_healthy`, and trips
//! the circuit breaker on a violation. The arithmetic crosses `u64`/`u128`
//! boundaries and folds an adversary-influenced input — the on-chain wBTH
//! `totalSupply`, narrowed from a `uint256` to a `u128` — into a signed drift.
//! A **false-healthy** peg (the drift math under-reports a real shortfall) is
//! the highest-severity bridge failure, so this math warrants continuous
//! coverage-guided fuzzing on top of the existing proptest
//! (`reserve.rs::prop_invariant_holds_across_mint_burn_sequences`).
//!
//! This target drives the **same** [`reserve_verdict`] function the live
//! [`Reconciler`](bth_bridge_service) runs (`bridge/service/src/reserve.rs`) —
//! there is no test-only reimplementation that could drift from production.
//! `reconcile_once` gathers the figures (DB + RPC) and calls straight into
//! `reserve_verdict`; here we synthesize adversarial figures and call the same
//! entry point, then assert its documented post-conditions against an
//! independent reference model:
//!
//!   * **Total function.** `reserve_verdict` never panics or overflow-traps for
//!     any input across the full `u64`/`u128` range (all boundary arithmetic
//!     saturates). Simply reaching the assertions is that invariant.
//!   * **No false-healthy.** A real shortfall — unbacked supply beyond
//!     tolerance on any verified chain, or an actual reserve balance short of
//!     the ledger beyond tolerance — is *never* reported `peg_healthy`. Checked
//!     against a shortfall reference computed independently in `u128`.
//!   * **Exact peg is healthy.** With every verified chain at `supply ==
//!     locked` and the custody balance exactly covering the ledger, the peg is
//!     healthy.
//!   * **Monotone in drift.** Worsening coverage (raising the ledger-locked
//!     total, lowering the reserve balance) never flips unhealthy → healthy;
//!     and growing an already-unbacked supply never flips unhealthy → healthy.
//!
//! The harness body lives here (not in the `fuzz_targets/` binary) so the
//! coverage-guided libFuzzer target and the macOS native-smoke driver run the
//! exact same assertions over one source of truth (#920).

use arbitrary::Arbitrary;

use bth_bridge_core::Chain;
use bth_bridge_service::{reserve_verdict, ChainFigure, ReserveVerdict};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// One chain's fuzzer-chosen reserve figures.
#[derive(Debug, Arbitrary)]
pub struct ChainInput {
    /// Selects the chain identity (`% 3` → Bth / Ethereum / Solana).
    pub chain_sel: u8,
    /// Whether this chain's supply was verified (read) this pass. Unverified
    /// chains are excluded from the drift math.
    pub verified: bool,
    /// On-chain wrapped supply in picocredits (the adversary-influenced value,
    /// narrowed from a `uint256`; used only when `verified`).
    pub supply: u128,
    /// Ledger-locked backing attributed to this chain.
    pub locked: u64,
    /// In-flight allowance (pending mints net + pending burns gross).
    pub in_flight: u64,
}

impl ChainInput {
    fn chain(&self) -> Chain {
        match self.chain_sel % 3 {
            0 => Chain::Bth,
            1 => Chain::Ethereum,
            _ => Chain::Solana,
        }
    }

    fn figure(&self) -> ChainFigure {
        ChainFigure {
            chain: self.chain(),
            supply: self.verified.then_some(self.supply),
            locked: self.locked,
            in_flight: self.in_flight,
        }
    }
}

/// One reserve-reconciliation exercise per fuzz input.
#[derive(Debug, Arbitrary)]
pub struct Input {
    /// Per-chain figures (kept small; libFuzzer bounds this by input length).
    pub chains: Vec<ChainInput>,
    /// The ledger's total locked reserve across all chains.
    pub locked_reserve_total: u64,
    /// The actual on-chain reserve balance, or `None` when the custody leg was
    /// not checkable this pass.
    pub reserve_balance: Option<u128>,
    /// `reserve.tolerance_picocredits` (default 0 = the exact peg).
    pub tolerance: u64,
    /// Monotonicity lever: how much to grow every supply in the "worse" run.
    pub supply_bump: u128,
    /// Monotonicity lever: how much to lower the reserve balance.
    pub balance_drop: u128,
    /// Monotonicity lever: how much to raise the ledger-locked total.
    pub locked_bump: u64,
}

// ============================================================================
// Entry point (shared by the libFuzzer target and the native-smoke driver)
// ============================================================================

/// Decode an [`Input`] from raw bytes the same way libFuzzer would and run the
/// harness. Bytes that cannot form an `Input` are skipped.
pub fn run_from_bytes(data: &[u8]) {
    if let Some(input) = crate::decode_take_rest::<Input>(data) {
        run(input);
    }
}

/// Run the harness against an already-decoded input.
pub fn run(input: Input) {
    let tol = input.tolerance as u128;
    let figures: Vec<ChainFigure> = input.chains.iter().map(ChainInput::figure).collect();

    // --- Primary call. Reaching past it is the no-panic / no-overflow
    //     invariant; every sub-check below feeds the SAME `reserve_verdict`. ---
    let verdict = reserve_verdict(
        &figures,
        input.locked_reserve_total,
        input.reserve_balance,
        input.tolerance,
    );

    check_no_false_healthy(&input, &figures, &verdict, tol);
    check_exact_peg_is_healthy(&input, &figures);
    check_supply_monotonicity(&input, &figures, tol);
    check_custody_monotonicity(&input, &figures, &verdict);
}

/// The load-bearing invariant: the peg is never reported healthy while a real
/// shortfall exists. `shortfall` is computed independently of `reserve_verdict`
/// (pure `u128` comparisons), so a production off-by-one, wrap, or narrowing
/// bug that under-reports a shortfall is caught here.
fn check_no_false_healthy(
    input: &Input,
    figures: &[ChainFigure],
    verdict: &ReserveVerdict,
    tol: u128,
) {
    // Custody shortfall: an actual balance short of the ledger beyond tolerance.
    let custody_short = matches!(
        input.reserve_balance,
        Some(balance) if balance.saturating_add(tol) < input.locked_reserve_total as u128
    );

    // Any verified chain unbacked (supply above locked+tol) or missing supply
    // (locked above supply+in_flight+tol) is a shortfall.
    let chain_short = figures.iter().any(|f| match f.supply {
        Some(supply) => {
            let locked = f.locked as u128;
            let unbacked = supply > locked.saturating_add(tol);
            let missing = locked
                > supply
                    .saturating_add(f.in_flight as u128)
                    .saturating_add(tol);
            unbacked || missing
        }
        None => false,
    });

    let shortfall = custody_short || chain_short;
    assert!(
        !(verdict.peg_healthy && shortfall),
        "FALSE-HEALTHY peg: reserve_verdict reported peg_healthy=true despite a real shortfall \
         (custody_short={custody_short}, chain_short={chain_short}); \
         locked_reserve_total={}, reserve_balance={:?}, tolerance={}, figures={:?}",
        input.locked_reserve_total,
        input.reserve_balance,
        input.tolerance,
        figures,
    );

    // The verdict's own composition must be internally consistent.
    assert_eq!(
        verdict.peg_healthy,
        verdict.in_tolerance && verdict.reserve_covered,
        "peg_healthy is not exactly in_tolerance && reserve_covered"
    );
}

/// An exact peg — every verified chain at `supply == locked`, custody balance
/// exactly covering the ledger — is always healthy.
fn check_exact_peg_is_healthy(input: &Input, figures: &[ChainFigure]) {
    let exact: Vec<ChainFigure> = figures
        .iter()
        .map(|f| ChainFigure {
            chain: f.chain,
            // Verified chains sit exactly on their backing; unverified stay
            // unverified (excluded from the math).
            supply: f.supply.map(|_| f.locked as u128),
            locked: f.locked,
            in_flight: f.in_flight,
        })
        .collect();

    // Custody balance exactly equal to the ledger covers it (balance + tol >=
    // locked for any tol >= 0).
    let balance = Some(input.locked_reserve_total as u128);
    let verdict = reserve_verdict(&exact, input.locked_reserve_total, balance, input.tolerance);
    assert!(
        verdict.peg_healthy,
        "exact peg (supply==locked on every verified chain, balance==ledger) reported unhealthy: \
         in_tolerance={}, reserve_covered={}",
        verdict.in_tolerance, verdict.reserve_covered
    );
}

/// Growing an already-unbacked supply must never cure the peg: if any verified
/// chain is unbacked in the base figures, raising every supply keeps it
/// unhealthy (an unbacked chain only becomes more unbacked).
fn check_supply_monotonicity(input: &Input, figures: &[ChainFigure], tol: u128) {
    let base_has_unbacked = figures
        .iter()
        .any(|f| matches!(f.supply, Some(s) if s > (f.locked as u128).saturating_add(tol)));
    if !base_has_unbacked {
        return;
    }

    let worse: Vec<ChainFigure> = figures
        .iter()
        .map(|f| ChainFigure {
            chain: f.chain,
            supply: f.supply.map(|s| s.saturating_add(input.supply_bump)),
            locked: f.locked,
            in_flight: f.in_flight,
        })
        .collect();
    let verdict = reserve_verdict(
        &worse,
        input.locked_reserve_total,
        input.reserve_balance,
        input.tolerance,
    );
    assert!(
        !verdict.peg_healthy,
        "MONOTONICITY: raising an already-unbacked supply (bump={}) flipped the peg healthy",
        input.supply_bump
    );
}

/// Worsening the custody coverage — a higher ledger-locked total and/or a lower
/// reserve balance, with the per-chain supplies unchanged — can only move the
/// peg healthy → unhealthy, never the reverse. So a healthy "worse" run implies
/// the base was healthy too.
fn check_custody_monotonicity(input: &Input, figures: &[ChainFigure], base: &ReserveVerdict) {
    let worse_locked = input.locked_reserve_total.saturating_add(input.locked_bump);
    let worse_balance = input
        .reserve_balance
        .map(|b| b.saturating_sub(input.balance_drop));
    let worse = reserve_verdict(figures, worse_locked, worse_balance, input.tolerance);
    assert!(
        !worse.peg_healthy || base.peg_healthy,
        "MONOTONICITY: worsening coverage (locked {}->{}, balance {:?}->{:?}) made an unhealthy \
         peg healthy",
        input.locked_reserve_total,
        worse_locked,
        input.reserve_balance,
        worse_balance
    );
}
