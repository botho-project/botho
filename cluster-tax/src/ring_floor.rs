//! Protocol 10.0.0 (#1686): the ring floor on the demurrage factor.
//!
//! A spend's demurrage factor is `max(claimed, ring floor, import floor)`. The
//! claimed factor comes from the spender's own output tags, so the floor is
//! what stops a wealthy spender declaring background outputs. Before 10.0.0
//! the floor was [`crate::ring_centroid_implied_factor`]: the value-weighted
//! mean wealth over every member of every ring, pooled. The spender picks the
//! decoys, so high-value background decoys diluted it. A 1 BTH coin from a
//! 10M BTH cluster with 19 decoys of 100k BTH paid 0% of the undiluted charge
//! (#1686, `docs/research/ring-floor-dilution-sweep.md`).
//!
//! # The rule (operator ruling, 2026-11-10)
//!
//! - **Per input, the max over amount-matched members.** With public amounts
//!   the spent member is one of the ring members whose amount equals the
//!   input's `pseudo_output_amount` (C3, `verify_ring_members`, binds it). The
//!   input's floor wealth is the **largest** effective cluster wealth among
//!   those members. A decoy of a different amount is provably not the spent
//!   coin and is ignored; a decoy of the same amount can only raise the floor.
//!   The candidate selection is [`crate::tag_retention::amount_matched`], the
//!   same code the #1687 rules use, so both rules agree on which members can be
//!   the spent coin.
//! - **Across inputs, value-weighted by `pseudo_output_amount`.** The per-input
//!   floor wealths are averaged with the pseudo-output amounts as weights, and
//!   the curve is applied once, the same way the claimed side is computed
//!   (`effective_cluster_wealth_from_outputs` averages output wealth by output
//!   value, then applies the curve). Averaging in the wealth domain keeps the
//!   two sides comparable: an honest two-input spend's claimed wealth is
//!   (before tag decay) the same weighted mean of its real inputs' wealths, and
//!   the floor is that mean with each real wealth replaced by a value at least
//!   as large.
//!
//! # Guarantees
//!
//! - **Dilution-proof.** The real member is a candidate of its input, so each
//!   input's floor wealth is at least the real member's wealth, whatever the
//!   decoys are. Adding decoys of any amount and wealth never lowers it.
//! - **No honest over-charge from decoys of other amounts.** Only a decoy with
//!   exactly the input's amount and a higher wealth raises an honest spend's
//!   floor above its real class. In the #1686 sweep that happened in 0% of
//!   honest spends (1.1% with 25% round amounts).
//! - **Inputs priced by their own value.** An input that carries no value
//!   (pseudo amount 0) contributes nothing; no input's ring can price another
//!   input's value.
//!
//! The rule needs public amounts. ADR 0015
//! (`docs/decisions/0015-ct-era-ring-floor.md`) records its successor once
//! amounts are confidential.
//!
//! # Callers
//!
//! The node (`ValidationReads::ring_floored_factor_for_tx`,
//! `botho/src/ledger/store/validation.rs`) resolves each ring member's
//! effective wealth from the ledger and calls
//! [`amount_matched_ring_floor_factor`]. Block validation, the SCP value
//! check, the block builder and mempool admission all go through
//! `consensus_fee_floor` or that ledger method, so they run this code. The
//! simulations call it through `fee_floor_inputs::resolve_fee_floor_at`. A
//! wallet estimating the consensus fee floor (#2001) calls it with the ring it
//! built: each member's amount and effective wealth
//! (`fee_floor_inputs::ring_member_wealth` over its tags and the published
//! per-cluster wealth).
//!
//! Determinism: integer arithmetic (a 256-bit weighted mean,
//! [`crate::wide::WeightedMean`]), slice order only, no node state.

use crate::{fee_curve::ClusterFactorCurve, tag_retention::amount_matched, wide::WeightedMean};

/// One transaction input as the ring floor sees it.
#[derive(Clone, Copy, Debug)]
pub struct RingFloorInput<'a> {
    /// The input's `pseudo_output_amount` (the spent member's amount).
    pub pseudo_amount: u64,
    /// Every resolved ring member as `(amount, effective cluster wealth)`.
    /// The effective wealth is `Σ_tag floor(weight · W[c] / TAG_WEIGHT_SCALE)`
    /// in picocredits ([`crate::fee_floor_inputs::ring_member_wealth`]).
    pub members: &'a [(u64, u128)],
}

/// One input's floor wealth: the largest effective wealth among the members
/// whose amount equals the pseudo-output amount (every member if none does,
/// which C3 makes unreachable on a valid transaction). 0 for an empty ring.
pub fn input_floor_wealth(input: &RingFloorInput<'_>) -> u128 {
    amount_matched(input.pseudo_amount, input.members)
        .into_iter()
        .copied()
        .max()
        .unwrap_or(0)
}

/// The ring floor's wealth: each input's [`input_floor_wealth`], averaged with
/// the pseudo-output amounts as weights (exact 256-bit numerator, floored).
/// 0 when the inputs carry no value.
pub fn amount_matched_ring_floor_wealth(inputs: &[RingFloorInput<'_>]) -> u128 {
    let mut mean = WeightedMean::new();
    for input in inputs {
        let value = input.pseudo_amount as u128;
        mean.add_weight(value);
        mean.add_product(value, input_floor_wealth(input));
    }
    mean.floor()
}

/// The ring floor on the demurrage factor (FACTOR_SCALE units, 1000 = 1x):
/// `curve(amount_matched_ring_floor_wealth(inputs))`. The consensus fee floor
/// charges at `max(claimed, this, import floor)`. 1x for background rings and
/// for inputs that carry no value.
pub fn amount_matched_ring_floor_factor(
    inputs: &[RingFloorInput<'_>],
    curve: &ClusterFactorCurve,
) -> u64 {
    curve.factor(amount_matched_ring_floor_wealth(inputs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring_centroid_implied_factor;

    const BTH: u64 = 1_000_000_000_000;
    /// A 10M BTH cluster: the issue's wealthy class, floor 5.745x.
    const W: u128 = 10_000_000 * BTH as u128;

    fn curve() -> ClusterFactorCurve {
        ClusterFactorCurve::default_params()
    }

    fn input(pseudo: u64, members: &[(u64, u128)]) -> RingFloorInput<'_> {
        RingFloorInput {
            pseudo_amount: pseudo,
            members,
        }
    }

    /// The #1686 table: ring 20, the real coin from the 10M BTH cluster, 19
    /// background decoys. The old pooled mean is diluted; the new floor is
    /// the undiluted 5.745x in every row.
    #[test]
    fn issue_table_is_not_diluted() {
        let c = curve();
        let undiluted = c.factor(W);
        assert_eq!(undiluted, 5745);
        for (coin, decoy, old) in [
            (1_000 * BTH, 2_000 * BTH, 4162),
            (10 * BTH, 2_000 * BTH, 1441),
            (BTH, 100_000 * BTH, 1000),
        ] {
            let mut ring = vec![(coin, W)];
            ring.extend(std::iter::repeat_n((decoy, 0u128), 19));
            assert_eq!(ring_centroid_implied_factor(&ring, &c), old);
            assert_eq!(
                amount_matched_ring_floor_factor(&[input(coin, &ring)], &c),
                undiluted
            );
        }
    }

    /// Same-amount background decoys are candidates too, but the max ignores
    /// them; a same-amount wealthier decoy can only raise the floor.
    #[test]
    fn same_amount_decoys_cannot_lower_the_floor() {
        let c = curve();
        let coin = 7 * BTH;
        let mut ring = vec![(coin, W)];
        ring.extend(std::iter::repeat_n((coin, 0u128), 19));
        assert_eq!(
            amount_matched_ring_floor_factor(&[input(coin, &ring)], &c),
            c.factor(W)
        );
        ring[5] = (coin, 10 * W);
        assert_eq!(
            amount_matched_ring_floor_factor(&[input(coin, &ring)], &c),
            c.factor(10 * W)
        );
    }

    /// A wealthier decoy of a different amount is provably not the spent coin
    /// and does not raise an honest background spend's floor.
    #[test]
    fn wealthy_decoy_of_another_amount_is_ignored() {
        let c = curve();
        let ring = [(3 * BTH, 0u128), (5 * BTH, W), (9 * BTH, W)];
        assert_eq!(
            amount_matched_ring_floor_factor(&[input(3 * BTH, &ring)], &c),
            1000
        );
    }

    /// Inputs combine by value, in the wealth domain: two equal inputs, one
    /// wealthy and one background, give the floor of the mean wealth (the
    /// honest claimed class before decay), not the mean of the two factors.
    #[test]
    fn inputs_blend_by_pseudo_amount_in_wealth() {
        let c = curve();
        let v = 100 * BTH;
        let r1 = [(v, W), (v + 1, 0)];
        let r2 = [(v, 0u128), (v + 2, W)];
        let floor = amount_matched_ring_floor_factor(&[input(v, &r1), input(v, &r2)], &c);
        assert_eq!(floor, c.factor(W / 2));
        // Weighting follows the pseudo amounts, not the members' amounts.
        let small = [(BTH, W)];
        let big = [(99 * BTH, 0u128)];
        assert_eq!(
            amount_matched_ring_floor_wealth(&[input(BTH, &small), input(99 * BTH, &big)]),
            W / 100
        );
    }

    #[test]
    fn empty_and_valueless_inputs_are_background() {
        let c = curve();
        assert_eq!(amount_matched_ring_floor_factor(&[], &c), 1000);
        assert_eq!(
            amount_matched_ring_floor_factor(&[input(0, &[(0, W)])], &c),
            1000
        );
        assert_eq!(input_floor_wealth(&input(5, &[])), 0);
    }

    /// No member matches (C3 rejects this first): every member is a
    /// candidate, so the floor falls back to the max over the ring.
    #[test]
    fn no_match_falls_back_to_the_whole_ring() {
        let ring = [(1, 0u128), (2, W)];
        assert_eq!(input_floor_wealth(&input(3, &ring)), W);
    }

    /// Extreme wealth: the exact mean never exceeds the largest floor wealth,
    /// so it maps to the curve's maximum without overflow.
    #[test]
    fn extreme_values_do_not_overflow() {
        let c = curve();
        let ring = [(u64::MAX, u128::MAX)];
        let inputs = [input(u64::MAX, &ring), input(u64::MAX, &ring)];
        assert_eq!(amount_matched_ring_floor_wealth(&inputs), u128::MAX);
        assert_eq!(
            amount_matched_ring_floor_factor(&inputs, &c),
            c.factor(u128::MAX)
        );
    }
}
