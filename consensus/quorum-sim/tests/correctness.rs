//! Mandatory correctness tests encoding the verified #510 research.
//!
//! These are the acceptance bar for issue #511. Each asserts a known FBAS /
//! threshold result that the analyzer must reproduce.

use bth_quorum_sim::{
    model::Fbas,
    nodeset::NodeSet,
    thresholds::{botho_bft_threshold, supermajority_fault_tolerance, two_thirds_threshold},
};

/// #510: Botho's current formula degenerates to unanimity at n ≤ 3, so the
/// minimal blocking-set cardinality is 1 (any single node halts the network →
/// zero crash-fault tolerance).
#[test]
fn current_formula_unanimous_below_4_blocking_one() {
    for n in 1..=3 {
        let fbas = Fbas::symmetric_botho(n);
        let report = fbas.health_report();
        assert_eq!(
            report.min_blocking_set_cardinality,
            Some(1),
            "n={n}: Botho rule should be unanimous → min blocking set = 1"
        );
        // And it really is unanimity:
        assert_eq!(fbas.nodes[0].quorum_set.threshold, n);
    }
    // n=4 is the first non-degenerate case (3-of-4): blocking buffer = 2.
    let fbas4 = Fbas::symmetric_botho(4);
    assert_eq!(
        fbas4.health_report().min_blocking_set_cardinality,
        Some(2),
        "n=4 (3-of-4) should tolerate 1 crash → min blocking set = 2"
    );
}

/// #510: a 2-of-4 threshold admits disjoint quorums {A,B} and {C,D}, so
/// quorum_intersection() must be FALSE (a fork is possible).
#[test]
fn two_of_four_disjoint_quorums_no_intersection() {
    let fbas = Fbas::symmetric(4, 2);
    assert!(
        fbas.is_quorum(&NodeSet::from_indices([0, 1])),
        "{{A,B}} quorum"
    );
    assert!(
        fbas.is_quorum(&NodeSet::from_indices([2, 3])),
        "{{C,D}} quorum"
    );
    assert!(
        !fbas.has_quorum_intersection(),
        "2-of-4 must NOT have quorum intersection (disjoint {{A,B}}/{{C,D}})"
    );
    // The empty intersection also surfaces as a zero-cardinality splitting set.
    assert_eq!(
        fbas.health_report().min_splitting_set_cardinality,
        Some(0),
        "no quorum intersection → empty splitting set (cardinality 0)"
    );
}

/// #510: under ceil(0.67·n), tolerating f=1 requires n ≥ 4, and the liveness
/// margin m − t + 1 matches expectation (m = n for the symmetric top tier).
#[test]
fn two_thirds_f1_requires_n_at_least_4() {
    // f=1 tolerance only from n=4 up.
    assert_eq!(supermajority_fault_tolerance(3, two_thirds_threshold(3)), 0);
    assert_eq!(supermajority_fault_tolerance(4, two_thirds_threshold(4)), 1);

    // Liveness margin m - t + 1 for the symmetric tier (m = n).
    // n=4, t = ceil(8/3) = 3  ->  margin = 4 - 3 + 1 = 2.
    let (n, t) = (4usize, two_thirds_threshold(4));
    assert_eq!(t, 3);
    let margin = n - t + 1;
    assert_eq!(margin, 2, "n=4 two-thirds liveness margin");

    // The margin equals (min blocking cardinality) for a symmetric tier:
    // blocking needs to drop survivors below t, i.e. remove n - t + 1 nodes.
    let fbas = Fbas::symmetric(n, t);
    assert_eq!(
        fbas.health_report().min_blocking_set_cardinality,
        Some(margin),
        "min blocking cardinality should equal the liveness margin"
    );
}

/// #510: with fewer than 4 distinct entities, real fault tolerance is
/// effectively 1 (min blocking-set cardinality 1 → zero genuine tolerance)
/// across every threshold rule (since the strongest non-trivial threshold at
/// n=3 is still unanimity-or-weaker and cannot reach 3f+1 with f≥1).
#[test]
fn fewer_than_four_entities_have_no_real_tolerance() {
    for n in 2..=3 {
        // Botho rule (unanimity here).
        let botho = Fbas::symmetric(n, botho_bft_threshold(n));
        assert_eq!(
            botho.health_report().min_blocking_set_cardinality,
            Some(1),
            "n={n} botho rule: blocking cardinality 1 (no real tolerance)"
        );
        // No threshold choice at n<4 yields f>=1 fault tolerance.
        for t in 1..=n {
            assert_eq!(
                supermajority_fault_tolerance(n, t),
                0,
                "n={n}, t={t}: cannot tolerate a fault below 4 entities"
            );
        }
    }
}

/// Cross-check fbas_analyzer semantics: in a symmetric top-tier federation, the
/// minimal blocking and splitting sets consist exclusively of top-tier nodes.
#[test]
fn minimal_sets_are_top_tier_nodes() {
    let fbas = Fbas::symmetric_botho(7); // 5-of-7
    let report = fbas.health_report();
    for s in &report.minimal_blocking_sets {
        assert!(
            s.iter().all(|&i| i < 7),
            "blocking set should be top-tier nodes"
        );
    }
    for s in &report.minimal_splitting_sets {
        assert!(
            s.iter().all(|&i| i < 7),
            "splitting set should be top-tier nodes"
        );
    }
    // 5-of-7 has quorum intersection and a healthy buffer.
    assert!(report.quorum_intersection);
    assert_eq!(report.min_blocking_set_cardinality, Some(3)); // remove 3 -> 4<5
}
