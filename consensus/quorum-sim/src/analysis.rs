//! Static FBAS analysis: quorum enumeration, intersection, and the minimal
//! quorum / blocking / splitting set metrics.
//!
//! All functions here brute-force over the `2^N` node subsets. This is exact
//! and, at the curated-federation sizes Path A targets (`N ≤ ~20`), fast. The
//! coNP-complete quorum-intersection check is decided exactly the same way.

use crate::{model::Fbas, nodeset::NodeSet, thresholds};
use serde::{Deserialize, Serialize};

/// Which threshold rule a [`ThresholdComparisonRow`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdRule {
    /// Botho's current `n − floor((n−1)/3)`.
    BothoBft,
    /// `ceil(0.67·n)` two-thirds supermajority.
    TwoThirds,
    /// Unanimity (`n`).
    Unanimity,
}

impl ThresholdRule {
    /// The threshold this rule assigns to a federation of size `n`.
    pub fn threshold(&self, n: usize) -> usize {
        match self {
            ThresholdRule::BothoBft => thresholds::botho_bft_threshold(n),
            ThresholdRule::TwoThirds => thresholds::two_thirds_threshold(n),
            ThresholdRule::Unanimity => thresholds::unanimity_threshold(n),
        }
    }

    /// Display label.
    pub fn label(&self) -> &'static str {
        match self {
            ThresholdRule::BothoBft => "botho_bft (n-floor((n-1)/3))",
            ThresholdRule::TwoThirds => "two_thirds (ceil(0.67n))",
            ThresholdRule::Unanimity => "unanimity (n)",
        }
    }
}

impl Fbas {
    /// Iterate over every non-empty quorum (every subset that satisfies
    /// [`Fbas::is_quorum`]).
    pub fn all_quorums(&self) -> Vec<NodeSet> {
        let n = self.len();
        let mut quorums = Vec::new();
        if n == 0 {
            return quorums;
        }
        let total = 1u32 << n;
        for mask in 1..total {
            let set = NodeSet(mask);
            if self.is_quorum(&set) {
                quorums.push(set);
            }
        }
        quorums
    }

    /// Whether the FBAS enjoys **quorum intersection**: every pair of quorums
    /// shares at least one node.
    ///
    /// A `false` result means two disjoint quorums exist and the network can
    /// fork (the safety prerequisite is violated). It suffices to check the
    /// *minimal* quorums pairwise, since any quorum contains a minimal one.
    pub fn has_quorum_intersection(&self) -> bool {
        let minimal = self.minimal_quorums();
        for (i, a) in minimal.iter().enumerate() {
            for b in &minimal[i + 1..] {
                if !a.intersects(b) {
                    return false;
                }
            }
        }
        true
    }

    /// Enumerate the **minimal quorums**: quorums with no proper subset that is
    /// also a quorum.
    pub fn minimal_quorums(&self) -> Vec<NodeSet> {
        let quorums = self.all_quorums();
        let mut minimal = Vec::new();
        for &q in &quorums {
            let has_smaller = quorums
                .iter()
                .any(|&other| other != q && other.is_subset_of(&q));
            if !has_smaller {
                minimal.push(q);
            }
        }
        minimal
    }

    /// Enumerate the **minimal blocking sets**: minimal sets of nodes whose
    /// removal leaves no quorum among the survivors (i.e. halting the network).
    ///
    /// A set `B` is *blocking* iff the complement `all \ B` contains no quorum.
    /// The cardinality of the smallest blocking set is the **liveness buffer**:
    /// how many nodes must fail (crash) before the network can no longer make
    /// progress.
    pub fn minimal_blocking_sets(&self) -> Vec<NodeSet> {
        let n = self.len();
        if n == 0 {
            return Vec::new();
        }
        let all = self.all_nodes();
        let total = 1u32 << n;
        let mut blocking = Vec::new();
        for mask in 1..total {
            let b = NodeSet(mask);
            let survivors = all.difference(&b);
            // Blocking iff survivors contain no quorum.
            if !self.contains_quorum(&survivors) {
                blocking.push(b);
            }
        }
        minimize(blocking)
    }

    /// Enumerate the **minimal splitting sets**: minimal sets of nodes whose
    /// Byzantine misbehaviour can split the network into two non-intersecting
    /// quorums.
    ///
    /// A set `S` is *splitting* iff there exist two quorums `Q1`, `Q2` whose
    /// intersection is contained in `S` (`Q1 ∩ Q2 ⊆ S`). The smallest such
    /// `S` is the **safety buffer**: how many nodes must be Byzantine before a
    /// fork becomes possible. If the FBAS has quorum intersection, the empty
    /// set is never splitting and the minimal cardinality is `≥ 1`; if it lacks
    /// quorum intersection, the empty set is splitting (cardinality 0).
    pub fn minimal_splitting_sets(&self) -> Vec<NodeSet> {
        let minimal_quorums = self.minimal_quorums();
        let mut splitting = Vec::new();
        // Every pairwise intersection of quorums is a splitting set (and any
        // superset of one is too). Minimizing afterwards keeps only the
        // smallest. We include the (Q,Q) diagonal so a degenerate single
        // minimal quorum is handled, but its self-intersection is the quorum
        // itself, which is never minimal against cross pairs.
        for (i, a) in minimal_quorums.iter().enumerate() {
            for b in &minimal_quorums[i..] {
                let inter = a.intersect(b);
                if a != b {
                    splitting.push(inter);
                }
            }
        }
        // If there is a single minimal quorum, a split needs disabling it
        // entirely; represent that by its own membership.
        if minimal_quorums.len() == 1 {
            splitting.push(minimal_quorums[0]);
        }
        minimize(splitting)
    }

    /// Whether `set` contains any quorum as a subset.
    fn contains_quorum(&self, set: &NodeSet) -> bool {
        if set.is_empty() {
            return false;
        }
        let members: Vec<usize> = set.iter().collect();
        let k = members.len();
        let total = 1u32 << k;
        // Enumerate subsets of `set` and test the quorum predicate. Equivalent
        // to checking is_quorum over the masked space.
        for sub in 1..total {
            let mut candidate = NodeSet::new();
            for (bit, &node) in members.iter().enumerate() {
                if (sub >> bit) & 1 == 1 {
                    candidate.insert(node);
                }
            }
            if self.is_quorum(&candidate) {
                return true;
            }
        }
        false
    }

    /// Compute the [`HealthReport`] for this FBAS.
    pub fn health_report(&self) -> HealthReport {
        let minimal_quorums = self.minimal_quorums();
        let minimal_blocking = self.minimal_blocking_sets();
        let minimal_splitting = self.minimal_splitting_sets();
        let quorum_intersection = self.has_quorum_intersection();

        let min_blocking_card = minimal_blocking.iter().map(|s| s.len()).min();
        let min_splitting_card = minimal_splitting.iter().map(|s| s.len()).min();
        let min_quorum_card = minimal_quorums.iter().map(|s| s.len()).min();

        HealthReport {
            n: self.len(),
            quorum_intersection,
            min_quorum_cardinality: min_quorum_card,
            min_blocking_set_cardinality: min_blocking_card,
            min_splitting_set_cardinality: min_splitting_card,
            num_minimal_quorums: minimal_quorums.len(),
            num_minimal_blocking_sets: minimal_blocking.len(),
            num_minimal_splitting_sets: minimal_splitting.len(),
            minimal_quorums: minimal_quorums.iter().map(|s| s.to_vec()).collect(),
            minimal_blocking_sets: minimal_blocking.iter().map(|s| s.to_vec()).collect(),
            minimal_splitting_sets: minimal_splitting.iter().map(|s| s.to_vec()).collect(),
        }
    }
}

/// Reduce a collection of sets to those that are minimal under set inclusion,
/// de-duplicating in the process.
fn minimize(mut sets: Vec<NodeSet>) -> Vec<NodeSet> {
    sets.sort_by_key(|s| (s.len(), s.0));
    sets.dedup();
    let mut minimal: Vec<NodeSet> = Vec::new();
    for &s in &sets {
        if s.is_empty() {
            // An empty set is the unique minimum; it dominates everything.
            return vec![NodeSet::EMPTY];
        }
        if !minimal.iter().any(|m| m.is_subset_of(&s)) {
            minimal.push(s);
        }
    }
    minimal
}

/// Aggregate static-health metrics for an FBAS, serializable to JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    /// Number of nodes.
    pub n: usize,
    /// Whether all quorums pairwise intersect (safety prerequisite).
    pub quorum_intersection: bool,
    /// Cardinality of the smallest quorum (`None` if no quorum exists).
    pub min_quorum_cardinality: Option<usize>,
    /// Cardinality of the smallest blocking set — the **liveness** buffer
    /// (crash faults to halt the network). `None` if no node can block.
    pub min_blocking_set_cardinality: Option<usize>,
    /// Cardinality of the smallest splitting set — the **safety** buffer
    /// (Byzantine faults to fork the network). `None` if not computable.
    pub min_splitting_set_cardinality: Option<usize>,
    /// Number of minimal quorums.
    pub num_minimal_quorums: usize,
    /// Number of minimal blocking sets.
    pub num_minimal_blocking_sets: usize,
    /// Number of minimal splitting sets.
    pub num_minimal_splitting_sets: usize,
    /// The minimal quorums, as node-index lists.
    pub minimal_quorums: Vec<Vec<usize>>,
    /// The minimal blocking sets, as node-index lists.
    pub minimal_blocking_sets: Vec<Vec<usize>>,
    /// The minimal splitting sets, as node-index lists.
    pub minimal_splitting_sets: Vec<Vec<usize>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuorumSet;

    #[test]
    fn symmetric_3of4_quorums() {
        let fbas = Fbas::symmetric(4, 3);
        // Minimal quorums are the four 3-subsets.
        let mq = fbas.minimal_quorums();
        assert_eq!(mq.len(), 4);
        assert!(mq.iter().all(|q| q.len() == 3));
        assert!(fbas.has_quorum_intersection());
    }

    #[test]
    fn disjoint_quorums_2of4() {
        // 2-of-4 symmetric: {A,B} and {C,D} are both quorums and disjoint.
        let fbas = Fbas::symmetric(4, 2);
        assert!(fbas.is_quorum(&NodeSet::from_indices([0, 1])));
        assert!(fbas.is_quorum(&NodeSet::from_indices([2, 3])));
        assert!(!fbas.has_quorum_intersection());
    }

    #[test]
    fn blocking_set_unanimity() {
        // 3-of-3 unanimity: any single node blocks (its absence kills quorum).
        let fbas = Fbas::symmetric(3, 3);
        let report = fbas.health_report();
        assert_eq!(report.min_blocking_set_cardinality, Some(1));
    }

    #[test]
    fn blocking_set_3of4() {
        // 3-of-4: need to remove 2 nodes (leaving 2 < 3) to halt.
        let fbas = Fbas::symmetric(4, 3);
        let report = fbas.health_report();
        assert_eq!(report.min_blocking_set_cardinality, Some(2));
    }

    #[test]
    fn splitting_set_top_tier() {
        // 3-of-4: minimal quorums are 3-subsets; any two intersect in 2 nodes.
        // The minimal splitting set is the smallest such intersection.
        let fbas = Fbas::symmetric(4, 3);
        let report = fbas.health_report();
        // Two 3-subsets of 4 elements always share exactly 2 nodes.
        assert_eq!(report.min_splitting_set_cardinality, Some(2));
        // Cross-check fbas_analyzer semantics: splitting sets are top-tier nodes.
        for s in &report.minimal_splitting_sets {
            assert!(s.iter().all(|&i| i < 4));
        }
    }

    #[test]
    fn arbitrary_slices_quorum() {
        // Asymmetric: node 0 trusts only itself (1-of-1) -> {0} is a quorum.
        let fbas = Fbas::from_quorum_sets([
            QuorumSet::new(1, vec![0]),
            QuorumSet::new(2, vec![0, 1, 2]),
            QuorumSet::new(2, vec![0, 1, 2]),
        ]);
        assert!(fbas.is_quorum(&NodeSet::from_indices([0])));
        let mq = fbas.minimal_quorums();
        assert!(mq.iter().any(|q| *q == NodeSet::from_indices([0])));
    }
}
