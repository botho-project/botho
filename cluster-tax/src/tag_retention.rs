//! Protocol 10.0.0 (#1687): the import floor read from the inputs, and the
//! consensus floor on retained cluster-tag weight.
//!
//! These are the pure kernels of two consensus rules. The node resolves the
//! ring members and the recorded bridge-import set from its databases and then
//! calls these functions (`ValidationReads::import_floor_factor_from_inputs`
//! and `ValidationReads::verify_cluster_tag_retention`,
//! `botho/src/ledger/store/validation.rs`), so block validation, the SCP value
//! check, the block builder and mempool admission all run this code. The
//! simulations call it too (`fee_floor_inputs::resolve_fee_floor_at`,
//! `simulation::bridge_import_sweep`).
//!
//! # Spend candidates
//!
//! A ring hides which member is spent. With public amounts, though, the real
//! member is bound to the input's `pseudo_output_amount`: C3
//! (`verify_ring_members`) rejects an input whose pseudo-output amount matches
//! no member's amount, and the CLSAG balance proof ties it to the real
//! member's commitment. So the real member is one of the members whose amount
//! equals the pseudo-output amount, the input's **spend candidates**
//! ([`spend_candidates`]). A member with a different amount is provably a
//! decoy. Both rules below read candidates only, which keeps decoys from
//! raising an honest spender's floor and from lowering an attacker's. If no
//! member matches (only reachable when C3 did not run, for example in a unit
//! test), every member is a candidate.
//!
//! # 1. The import floor from the inputs
//!
//! Before 10.0.0 the ADR-0007 floor `F` was a blend over the spender's
//! **output** tags, so an output with no tags removed it (#1687; Lean
//! `Botho.Tags.import_floor_invariant_refuted`). Now each input contributes its
//! value (the pseudo-output amount) at the **largest** import share among its
//! candidates (the maximum, so no decoy can dilute it), and the floor is the
//! value-weighted blend of `F` on the import share and 1x on the rest
//! ([`import_floor_factor_from_inputs`]). The node takes the `max` of this and
//! the old output-tag blend, which it keeps: it still prices a release's
//! freshly tagged import output.
//!
//! # 2. The retained-weight floor
//!
//! Consensus used to reject only tag *inflation*, so an output could carry less
//! weight than its input, or none. Now, for each cluster `c`, every output must
//! keep at least
//!
//! ```text
//! required(c) = floor( Σ_rings pseudo_r · min_{candidates m of r} w_m(c)
//!                      · CONSENSUS_MIN_TAG_RETENTION
//!                      / (Σ_rings pseudo_r · TAG_WEIGHT_SCALE) )  −  slack
//! ```
//!
//! of weight on `c`, where `slack = (number of inputs) + 1` absorbs the
//! wallet's integer rounding. `CONSENSUS_MIN_TAG_RETENTION` is 95% of the
//! weight (5% decay per spend): the per-transfer decay the whitepaper
//! (Appendix "Tag Decay: 0.95 per Transfer") and
//! `docs/design/cluster-tag-decay.md` ("decay_rate_per_hop = 5%") specify, and
//! the decay the stock wallets apply (`DEFAULT_CLUSTER_DECAY_RATE`). No new
//! constant is introduced.
//!
//! The rule never rejects an honest wallet's spend (`merge_weighted(inputs,
//! 5%)` on every output):
//!
//! - **Rounding.** The wallet's merged weight is `Σ_i floor(v_i·w_i/V)`, at
//!   most one unit per input below the exact mean, and the decay floors once
//!   more, so it is at least `required(c)` before the slack is subtracted. The
//!   real input is a candidate, so its weight is at least the candidates'
//!   minimum. The fee does not enter: weights are per unit of value.
//! - **Pruning.** `ClusterTagVector::from_pairs` drops a weight below
//!   `MIN_STORED_WEIGHT` (0.1%). Clusters whose `required` is below that are
//!   not checked.
//! - **Truncation.** A merge of many clusters keeps the largest
//!   `MAX_CLUSTER_TAGS` (16). An output that holds 16 entries, all at least
//!   `required(c)`, is excused for `c`: the honest wallet dropped `c` only if
//!   16 other clusters outweighed it.
//!
//! Limits (stated in the PR and ADR 0007): the floor binds only where every
//! candidate of a ring carries the cluster. A spender who adds a decoy with the
//! same amount and without the cluster lowers the minimum to zero for that
//! ring. That costs one extra output of the exact amount, and it does not
//! touch the import floor, which takes the candidates' maximum. The
//! truncation excuse lets a spender drop a cluster of weight at most 1/16 by
//! filling an output with 16 other ring-present clusters, each bounded by C6.
//! Neither depends on time: the floor limits decay per spend, not per day
//! (the 720-block age gate of `cluster-tag-decay.md` is still not
//! implemented).
//!
//! Determinism: integer arithmetic in `u128`, `BTreeMap` iteration, no node
//! state. A pure function of the transaction and the resolved ring members.

use std::collections::BTreeMap;

use crate::{
    fee_curve::ClusterFactorCurve, fee_floor_inputs::TagPair, BRIDGE_IMPORT_FACTOR_FLOOR,
    TAG_WEIGHT_SCALE,
};

/// The least fraction of each cluster's weight an output must keep per spend,
/// in `TAG_WEIGHT_SCALE` units: 950,000 = 95%, the documented 5% decay per
/// transfer. Not a new parameter: it is `TAG_WEIGHT_SCALE −
/// WALLET_CLUSTER_DECAY_RATE`, asserted below.
pub const CONSENSUS_MIN_TAG_RETENTION: u32 = 950_000;

const _: () = assert!(
    CONSENSUS_MIN_TAG_RETENTION
        == TAG_WEIGHT_SCALE - crate::fee_floor_inputs::WALLET_CLUSTER_DECAY_RATE,
    "the consensus retention floor is the documented 5% per-transfer decay"
);

/// `bth_transaction_types::MIN_STORED_WEIGHT`: `from_pairs` prunes weights
/// below this. Restated because `bth-cluster-tax` does not depend on
/// `bth-transaction-types` by default; the node asserts the two are equal.
pub const RETENTION_MIN_STORED_WEIGHT: u32 = 1_000;

/// `bth_transaction_types::MAX_CLUSTER_TAGS`: `from_pairs` keeps at most this
/// many entries. Restated like [`RETENTION_MIN_STORED_WEIGHT`].
pub const RETENTION_MAX_CLUSTER_TAGS: usize = 16;

/// One transaction input as these rules see it: the pseudo-output amount (the
/// real member's value) and every resolved ring member's `(amount, tags)`.
#[derive(Clone, Copy, Debug)]
pub struct SpendRing<'a> {
    /// The input's `pseudo_output_amount`.
    pub pseudo_amount: u64,
    /// Every resolved ring member.
    pub members: &'a [(u64, &'a [TagPair])],
}

/// The members that can be the spent one: those whose amount equals the
/// pseudo-output amount, or every member if none does.
pub fn spend_candidates<'a>(ring: &SpendRing<'a>) -> Vec<&'a [TagPair]> {
    let matching: Vec<&'a [TagPair]> = ring
        .members
        .iter()
        .filter(|(amount, _)| *amount == ring.pseudo_amount)
        .map(|(_, tags)| *tags)
        .collect();
    if matching.is_empty() {
        ring.members.iter().map(|(_, tags)| *tags).collect()
    } else {
        matching
    }
}

/// A vector's total weight on `cluster` (duplicate entries summed), capped at
/// `TAG_WEIGHT_SCALE`.
fn weight_on(tags: &[TagPair], cluster: u64) -> u64 {
    tags.iter()
        .filter(|(c, _)| *c == cluster)
        .fold(0u64, |acc, (_, w)| acc.saturating_add(*w as u64))
        .min(TAG_WEIGHT_SCALE as u64)
}

/// A vector's total weight on recorded import clusters, capped at
/// `TAG_WEIGHT_SCALE`.
fn import_share(tags: &[TagPair], is_import: &dyn Fn(u64) -> bool) -> u64 {
    tags.iter()
        .filter(|(c, _)| is_import(*c))
        .fold(0u64, |acc, (_, w)| acc.saturating_add(*w as u64))
        .min(TAG_WEIGHT_SCALE as u64)
}

/// The import floor read from the inputs, with an explicit `F` (FACTOR_SCALE
/// units) so a calibration sweep can vary it. Each input's value is its
/// pseudo-output amount, priced at the largest import share among its spend
/// candidates; the result is the value-weighted blend of `F` on the import
/// share and 1x on the rest. 1x when the inputs carry no value.
pub fn import_floor_factor_from_inputs_at(
    rings: &[SpendRing<'_>],
    is_import: &dyn Fn(u64) -> bool,
    floor: u64,
) -> u64 {
    let background = ClusterFactorCurve::FACTOR_SCALE as u128;
    let floor = floor as u128;
    let scale = TAG_WEIGHT_SCALE as u128;
    let mut total: u128 = 0;
    let mut import_value: u128 = 0;
    for ring in rings {
        let share = spend_candidates(ring)
            .iter()
            .map(|tags| import_share(tags, is_import))
            .max()
            .unwrap_or(0) as u128;
        let value = ring.pseudo_amount as u128;
        total = total.saturating_add(value);
        import_value = import_value.saturating_add(value * share / scale);
    }
    if total == 0 {
        return ClusterFactorCurve::FACTOR_SCALE;
    }
    let import_value = import_value.min(total);
    ((floor
        .saturating_mul(import_value)
        .saturating_add(background.saturating_mul(total - import_value)))
        / total) as u64
}

/// [`import_floor_factor_from_inputs_at`] at the ratified `F`.
pub fn import_floor_factor_from_inputs(
    rings: &[SpendRing<'_>],
    is_import: &dyn Fn(u64) -> bool,
) -> u64 {
    import_floor_factor_from_inputs_at(rings, is_import, BRIDGE_IMPORT_FACTOR_FLOOR)
}

/// The weight each output must keep on each cluster, after the rounding slack,
/// for the clusters where it is at least `MIN_STORED_WEIGHT` (the others may be
/// pruned by an honest wallet and are not checked). Ascending cluster order.
pub fn required_retained_weights(rings: &[SpendRing<'_>]) -> BTreeMap<u64, u64> {
    let scale = TAG_WEIGHT_SCALE as u128;
    let total: u128 = rings.iter().map(|r| r.pseudo_amount as u128).sum();
    let mut out = BTreeMap::new();
    if total == 0 {
        return out;
    }
    // Σ_rings pseudo_r · (min over candidates of the weight on c).
    let mut lower: BTreeMap<u64, u128> = BTreeMap::new();
    for ring in rings {
        let candidates = spend_candidates(ring);
        let Some(first) = candidates.first() else {
            continue;
        };
        // A cluster every candidate carries is named by the first one.
        let mut clusters: Vec<u64> = first.iter().map(|(c, _)| *c).collect();
        clusters.sort_unstable();
        clusters.dedup();
        for c in clusters {
            let min = candidates
                .iter()
                .map(|tags| weight_on(tags, c))
                .min()
                .unwrap_or(0);
            if min > 0 {
                *lower.entry(c).or_insert(0) += ring.pseudo_amount as u128 * min as u128;
            }
        }
    }
    let slack = rings.len() as u128 + 1;
    for (c, l) in lower {
        // l ≤ total · SCALE and RETENTION < SCALE, so the product fits u128
        // (total ≤ 2^64 · inputs).
        let exact = l * CONSENSUS_MIN_TAG_RETENTION as u128 / (total * scale);
        let required = exact.saturating_sub(slack);
        if required >= RETENTION_MIN_STORED_WEIGHT as u128 {
            out.insert(c, required as u64);
        }
    }
    out
}

/// The first output that keeps too little weight on a cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionViolation {
    /// Output index.
    pub output: usize,
    /// The cluster.
    pub cluster: u64,
    /// The weight the output had to keep (after the slack).
    pub required: u64,
    /// The weight it keeps.
    pub actual: u64,
}

/// The retained-weight rule: the first `(output, cluster)` whose weight is
/// below [`required_retained_weights`], scanning outputs in order and clusters
/// in ascending order, or `None`. An output with `MAX_CLUSTER_TAGS` entries,
/// each at least the required weight, is excused for that cluster (the honest
/// wallet's truncation).
pub fn first_tag_retention_violation(
    rings: &[SpendRing<'_>],
    outputs: &[&[TagPair]],
) -> Option<RetentionViolation> {
    let required = required_retained_weights(rings);
    if required.is_empty() {
        return None;
    }
    for (i, tags) in outputs.iter().enumerate() {
        let full = tags.len() >= RETENTION_MAX_CLUSTER_TAGS;
        let min_entry = tags.iter().map(|(_, w)| *w as u64).min().unwrap_or(0);
        for (&c, &req) in &required {
            let actual = weight_on(tags, c);
            if actual >= req || (full && min_entry >= req) {
                continue;
            }
            return Some(RetentionViolation {
                output: i,
                cluster: c,
                required: req,
                actual,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMP: u64 = 7;

    fn ring<'a>(pseudo: u64, members: &'a [(u64, &'a [TagPair])]) -> SpendRing<'a> {
        SpendRing {
            pseudo_amount: pseudo,
            members,
        }
    }

    #[test]
    fn candidates_are_the_members_of_the_pseudo_amount() {
        let a: &[TagPair] = &[(1, 1_000_000)];
        let b: &[TagPair] = &[];
        let members = [(10, a), (20, b), (10, b)];
        assert_eq!(spend_candidates(&ring(10, &members)), vec![a, b]);
        assert_eq!(spend_candidates(&ring(20, &members)), vec![b]);
        // No match: every member.
        assert_eq!(spend_candidates(&ring(30, &members)).len(), 3);
    }

    #[test]
    fn import_floor_reads_the_inputs_not_the_outputs() {
        let imp: &[TagPair] = &[(IMP, TAG_WEIGHT_SCALE)];
        let bg: &[TagPair] = &[];
        // A fully import-tagged coin among background decoys of other amounts.
        let members = [(1_000, imp), (999, bg), (1_001, bg)];
        let is_imp = |c: u64| c == IMP;
        assert_eq!(
            import_floor_factor_from_inputs(&[ring(1_000, &members)], &is_imp),
            BRIDGE_IMPORT_FACTOR_FLOOR
        );
        // A same-amount background decoy cannot dilute it (max, not mean).
        let members = [(1_000, imp), (1_000, bg)];
        assert_eq!(
            import_floor_factor_from_inputs(&[ring(1_000, &members)], &is_imp),
            BRIDGE_IMPORT_FACTOR_FLOOR
        );
        // An import-tagged decoy of another amount does not charge F.
        let members = [(1_000, bg), (5_000, imp)];
        assert_eq!(
            import_floor_factor_from_inputs(&[ring(1_000, &members)], &is_imp),
            ClusterFactorCurve::FACTOR_SCALE
        );
        // Two inputs: value-weighted blend.
        let m1 = [(1_000, imp)];
        let m2 = [(3_000, bg)];
        assert_eq!(
            import_floor_factor_from_inputs(&[ring(1_000, &m1), ring(3_000, &m2)], &is_imp),
            (1_500 * 1_000 + 1_000 * 3_000) / 4_000
        );
        // No value: 1x.
        assert_eq!(
            import_floor_factor_from_inputs(&[ring(0, &m1)], &is_imp),
            ClusterFactorCurve::FACTOR_SCALE
        );
    }

    fn decayed(tags: &[TagPair]) -> Vec<TagPair> {
        tags.iter()
            .map(|&(c, w)| {
                (
                    c,
                    (w as u64 * CONSENSUS_MIN_TAG_RETENTION as u64 / TAG_WEIGHT_SCALE as u64)
                        as u32,
                )
            })
            .collect()
    }

    #[test]
    fn retention_floor_boundary() {
        let real: &[TagPair] = &[(3, 600_000), (IMP, 400_000)];
        let members = [(1_000_000, real), (7, &[][..])];
        let rings = [ring(1_000_000, &members)];
        let req = required_retained_weights(&rings);
        // 95% of each weight, minus the slack of 2.
        assert_eq!(req.get(&3), Some(&(570_000 - 2)));
        assert_eq!(req.get(&IMP), Some(&(380_000 - 2)));

        // The wallet's 5% decay passes.
        let kept = decayed(real);
        assert_eq!(first_tag_retention_violation(&rings, &[&kept]), None);
        // At the required weight: accepted.
        let at: Vec<TagPair> = vec![(3, 569_998), (IMP, 379_998)];
        assert_eq!(first_tag_retention_violation(&rings, &[&at]), None);
        // One unit below on one cluster: rejected.
        let below: Vec<TagPair> = vec![(3, 569_998), (IMP, 379_997)];
        assert_eq!(
            first_tag_retention_violation(&rings, &[&below]),
            Some(RetentionViolation {
                output: 0,
                cluster: IMP,
                required: 379_998,
                actual: 379_997
            })
        );
        // Dropped tags: rejected at the first cluster.
        assert_eq!(
            first_tag_retention_violation(&rings, &[&kept, &[]]).map(|v| (v.output, v.cluster)),
            Some((1, 3))
        );
    }

    #[test]
    fn retention_floor_takes_the_minimum_over_candidates() {
        let real: &[TagPair] = &[(IMP, TAG_WEIGHT_SCALE)];
        let bg: &[TagPair] = &[];
        // A same-amount background decoy: the minimum is zero, nothing binds.
        let members = [(500, real), (500, bg)];
        assert!(required_retained_weights(&[ring(500, &members)]).is_empty());
        // A background decoy of another amount is not a candidate.
        let members = [(500, real), (501, bg)];
        assert_eq!(
            required_retained_weights(&[ring(500, &members)]).get(&IMP),
            Some(&(950_000 - 2))
        );
    }

    #[test]
    fn retention_floor_exempts_prunable_weight() {
        // 95% of 1,054 = 1,001.3, minus the slack of 2 = 999 < 1,000: not
        // checked (an honest wallet may prune it).
        let real: &[TagPair] = &[(3, 1_054)];
        let members = [(10, real)];
        assert!(required_retained_weights(&[ring(10, &members)]).is_empty());
        // 95% of 1,056 = 1,003.2, minus 2 = 1,001: checked.
        let real: &[TagPair] = &[(3, 1_056)];
        let members = [(10, real)];
        assert_eq!(
            required_retained_weights(&[ring(10, &members)]).get(&3),
            Some(&1_001)
        );
    }

    #[test]
    fn retention_floor_excuses_honest_truncation() {
        // 17 clusters of equal weight: the honest merge keeps 16.
        let real: Vec<TagPair> = (0..17u64).map(|c| (c, 58_823)).collect();
        let members = [(1_000, real.as_slice())];
        let rings = [ring(1_000, &members)];
        let kept: Vec<TagPair> = decayed(&real).into_iter().take(16).collect();
        assert_eq!(first_tag_retention_violation(&rings, &[&kept]), None);
        // Fifteen entries do not excuse the missing ones.
        let short: Vec<TagPair> = kept[..15].to_vec();
        assert!(first_tag_retention_violation(&rings, &[&short]).is_some());
    }

    #[test]
    fn multi_input_merge_passes() {
        // Two inputs of different clusters, merged by value as the wallet does.
        let a: &[TagPair] = &[(1, TAG_WEIGHT_SCALE)];
        let b: &[TagPair] = &[(2, 500_000), (IMP, 500_000)];
        let m1 = [(700, a)];
        let m2 = [(300, b)];
        let rings = [ring(700, &m1), ring(300, &m2)];
        let merged: Vec<TagPair> = vec![(1, 700_000), (2, 150_000), (IMP, 150_000)];
        let kept = decayed(&merged);
        assert_eq!(first_tag_retention_violation(&rings, &[&kept, &kept]), None);
        let req = required_retained_weights(&rings);
        assert_eq!(req.get(&1), Some(&(665_000 - 3)));
        assert_eq!(req.get(&2), Some(&(142_500 - 3)));
    }
}
