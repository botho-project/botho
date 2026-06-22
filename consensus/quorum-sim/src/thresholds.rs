//! Threshold rules under comparison.
//!
//! These three rules map a federation size `n` to a quorum threshold `t`
//! (the number of nodes, including self, required to form a quorum in a
//! symmetric top-tier construction).

/// Botho's current BFT quorum threshold: `t = n − floor((n−1)/3)`.
///
/// This is exactly `QuorumConfig::effective_threshold` in
/// `botho/src/config.rs` under `FaultModel::Bft`, expressed in terms of the
/// total node count `n` (the config takes `connected_count = n − 1`).
///
/// Notable values: n=1→1, n=2→2, n=3→3 (unanimity below 4), n=4→3, n=5→4,
/// n=6→5, n=7→5.
pub fn botho_bft_threshold(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let f = (n - 1) / 3;
    n - f
}

/// The classic `ceil(0.67·n)` two-thirds supermajority threshold.
///
/// Computed in integer arithmetic as `ceil(2n/3) = (2n + 2) / 3` to avoid
/// floating point. Notable values: n=1→1, n=2→2, n=3→2, n=4→3, n=5→4, n=6→4,
/// n=7→5.
pub fn two_thirds_threshold(n: usize) -> usize {
    // ceil(2n/3), in integer arithmetic.
    (2 * n).div_ceil(3)
}

/// Unanimity: every node must agree (`t = n`).
pub fn unanimity_threshold(n: usize) -> usize {
    n
}

/// Maximum number of faulty nodes a `t`-of-`n` symmetric quorum can tolerate
/// while keeping safety, under the standard `n ≥ 3f + 1` / `t ≥ 2f + 1`
/// reading: the largest `f` with `t ≥ 2f + 1` **and** `n ≥ 3f + 1`.
///
/// This is a convenience for the threshold comparison report; the authoritative
/// fault tolerance for an arbitrary FBAS comes from the minimal blocking /
/// splitting set cardinalities in [`crate::analysis`].
pub fn supermajority_fault_tolerance(n: usize, t: usize) -> usize {
    let mut f = 0;
    loop {
        let next = f + 1;
        // Safety needs t >= 2f+1; the 3f+1 reading needs n >= 3f+1.
        // (Written with `>` to satisfy clippy::int_plus_one.)
        if t > 2 * next && n > 3 * next {
            f = next;
        } else {
            break;
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn botho_matches_config_table() {
        // Mirror of test_quorum_effective_threshold_bft in botho/src/config.rs,
        // expressed in total-node terms (config uses connected_count = n - 1).
        assert_eq!(botho_bft_threshold(1), 1); // n=1: 1-of-1
        assert_eq!(botho_bft_threshold(2), 2); // n=2: 2-of-2
        assert_eq!(botho_bft_threshold(3), 3); // n=3: 3-of-3
        assert_eq!(botho_bft_threshold(4), 3); // n=4: 3-of-4
        assert_eq!(botho_bft_threshold(5), 4); // n=5: 4-of-5
        assert_eq!(botho_bft_threshold(6), 5); // n=6: 5-of-6
    }

    #[test]
    fn botho_degenerates_to_unanimity_below_4() {
        for n in 1..=3 {
            assert_eq!(
                botho_bft_threshold(n),
                unanimity_threshold(n),
                "n={n} should be unanimous"
            );
        }
        assert_ne!(botho_bft_threshold(4), unanimity_threshold(4));
    }

    #[test]
    fn two_thirds_ceiling() {
        assert_eq!(two_thirds_threshold(1), 1);
        assert_eq!(two_thirds_threshold(2), 2); // ceil(4/3) = 2
        assert_eq!(two_thirds_threshold(3), 2); // ceil(6/3) = 2
        assert_eq!(two_thirds_threshold(4), 3); // ceil(8/3) = 3
        assert_eq!(two_thirds_threshold(6), 4); // ceil(12/3) = 4
        assert_eq!(two_thirds_threshold(9), 6); // ceil(18/3) = 6
    }

    #[test]
    fn fault_tolerance_needs_four_nodes() {
        // ceil(0.67n): f=1 tolerance requires n>=4.
        assert_eq!(supermajority_fault_tolerance(3, two_thirds_threshold(3)), 0);
        assert_eq!(supermajority_fault_tolerance(4, two_thirds_threshold(4)), 1);
        assert_eq!(supermajority_fault_tolerance(7, two_thirds_threshold(7)), 2);
    }
}
