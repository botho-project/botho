//! The FBAS model: nodes with threshold-based quorum sets.

use crate::nodeset::NodeSet;
use crate::thresholds::botho_bft_threshold;
use serde::{Deserialize, Serialize};

/// A quorum set: a threshold over a list of validator member indices.
///
/// This is a flat, single-level slice (a threshold over concrete members),
/// which is sufficient for the symmetric top-tier and arbitrary per-node
/// constructions in scope for v1. Nested/inner quorum sets (as in full Stellar
/// `QuorumSet`s) are out of scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumSet {
    /// Number of members that must agree.
    pub threshold: usize,
    /// Member node indices this node trusts.
    pub members: Vec<usize>,
}

impl QuorumSet {
    /// Construct a quorum set, clamping `threshold` into `0..=members.len()`.
    pub fn new(threshold: usize, members: Vec<usize>) -> Self {
        let threshold = threshold.min(members.len());
        QuorumSet { threshold, members }
    }

    /// Whether `set` satisfies this quorum set (contains at least `threshold`
    /// of its members).
    pub fn is_satisfied_by(&self, set: &NodeSet) -> bool {
        let count = self.members.iter().filter(|&&m| set.contains(m)).count();
        count >= self.threshold
    }
}

/// A single node in the FBAS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Human-readable identifier (e.g. "A", "seed", a public-key prefix).
    pub id: String,
    /// This node's quorum set.
    pub quorum_set: QuorumSet,
}

/// A Federated Byzantine Agreement System: an ordered list of nodes.
///
/// Node *index* is the position in [`Fbas::nodes`]; that index is what
/// [`NodeSet`] addresses. The same index appears in other nodes' quorum-set
/// `members` lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fbas {
    /// The nodes, in index order.
    pub nodes: Vec<Node>,
}

impl Fbas {
    /// Empty FBAS.
    pub fn new() -> Self {
        Fbas { nodes: Vec::new() }
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the FBAS has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The set of all node indices.
    pub fn all_nodes(&self) -> NodeSet {
        NodeSet::full(self.len())
    }

    /// Build a **symmetric top-tier** FBAS: `n` nodes that all share the same
    /// quorum set — a `threshold`-of-`n` over every node (including self).
    ///
    /// This mirrors a curated federation where every validator trusts every
    /// other equally (the simplest and most robust Path A construction).
    pub fn symmetric(n: usize, threshold: usize) -> Self {
        let members: Vec<usize> = (0..n).collect();
        let qs = QuorumSet::new(threshold, members);
        let nodes = (0..n)
            .map(|i| Node {
                id: node_label(i),
                quorum_set: qs.clone(),
            })
            .collect();
        Fbas { nodes }
    }

    /// Build a symmetric FBAS of `n` nodes using **Botho's BFT threshold rule**
    /// (`effective_threshold = n − floor((n−1)/3)`), matching
    /// `QuorumConfig::effective_threshold` in `botho/src/config.rs`.
    pub fn symmetric_botho(n: usize) -> Self {
        Self::symmetric(n, botho_bft_threshold(n))
    }

    /// Build an FBAS from explicit, arbitrary per-node quorum sets.
    pub fn from_quorum_sets<I>(quorum_sets: I) -> Self
    where
        I: IntoIterator<Item = QuorumSet>,
    {
        let nodes = quorum_sets
            .into_iter()
            .enumerate()
            .map(|(i, quorum_set)| Node {
                id: node_label(i),
                quorum_set,
            })
            .collect();
        Fbas { nodes }
    }

    /// Whether `set` is a quorum: non-empty, and every member's quorum set is
    /// satisfied by `set` itself.
    ///
    /// This is the standard FBAS quorum definition: a quorum is a set of nodes
    /// that contains a slice for each of its members.
    pub fn is_quorum(&self, set: &NodeSet) -> bool {
        if set.is_empty() {
            return false;
        }
        set.iter().all(|i| {
            // Indices outside the node range cannot be in a valid quorum.
            i < self.len() && self.nodes[i].quorum_set.is_satisfied_by(set)
        })
    }

    // ----- Growth / churn (Path A curated admission + reactive-shun) -----

    /// Admit a new validator with the given quorum set, returning its index.
    ///
    /// The caller is responsible for any updates to *existing* nodes' quorum
    /// sets; [`admit_symmetric`](Fbas::admit_symmetric) handles the common
    /// "everyone trusts everyone" case.
    pub fn admit(&mut self, id: impl Into<String>, quorum_set: QuorumSet) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node {
            id: id.into(),
            quorum_set,
        });
        idx
    }

    /// Admit a validator into a symmetric top-tier federation, re-deriving every
    /// node's quorum set with Botho's BFT threshold for the new `n`.
    ///
    /// Returns the new node's index.
    pub fn admit_symmetric(&mut self) -> usize {
        let n = self.len() + 1;
        let new = Fbas::symmetric_botho(n);
        // Preserve existing ids; adopt the freshly-derived quorum sets.
        let new_idx = self.len();
        for (i, node) in new.nodes.iter().enumerate() {
            if i < self.nodes.len() {
                self.nodes[i].quorum_set = node.quorum_set.clone();
            } else {
                self.nodes.push(node.clone());
            }
        }
        new_idx
    }

    /// Reactively shun (remove) a node by index, re-indexing the remaining
    /// nodes and rewriting every quorum set to drop the removed member and
    /// renumber the survivors.
    ///
    /// In a symmetric federation this also reduces each node's threshold via the
    /// shared rule, because [`shun`](Fbas::shun) does not itself recompute
    /// thresholds — use [`shun_symmetric`](Fbas::shun_symmetric) for symmetric
    /// federations.
    pub fn shun(&mut self, idx: usize) {
        if idx >= self.nodes.len() {
            return;
        }
        self.nodes.remove(idx);
        // Rewrite member indices: drop `idx`, decrement anything above it.
        for node in &mut self.nodes {
            let mut new_members: Vec<usize> = Vec::with_capacity(node.quorum_set.members.len());
            for &m in &node.quorum_set.members {
                if m == idx {
                    continue;
                }
                new_members.push(if m > idx { m - 1 } else { m });
            }
            let new_threshold = node.quorum_set.threshold.min(new_members.len());
            node.quorum_set = QuorumSet::new(new_threshold, new_members);
        }
    }

    /// Reactively shun a node from a symmetric top-tier federation, re-deriving
    /// every survivor's quorum set with Botho's BFT threshold for the new `n`.
    pub fn shun_symmetric(&mut self, idx: usize) {
        if idx >= self.nodes.len() {
            return;
        }
        let ids: Vec<String> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, n)| n.id.clone())
            .collect();
        let n = ids.len();
        let mut new = Fbas::symmetric_botho(n);
        for (node, id) in new.nodes.iter_mut().zip(ids) {
            node.id = id;
        }
        *self = new;
    }
}

impl Default for Fbas {
    fn default() -> Self {
        Fbas::new()
    }
}

/// Generate a stable label for node index `i`: A..Z, then n26, n27, ...
fn node_label(i: usize) -> String {
    if i < 26 {
        ((b'A' + i as u8) as char).to_string()
    } else {
        format!("n{i}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quorum_set_satisfaction() {
        let qs = QuorumSet::new(2, vec![0, 1, 2]);
        assert!(qs.is_satisfied_by(&NodeSet::from_indices([0, 1])));
        assert!(qs.is_satisfied_by(&NodeSet::from_indices([0, 1, 2])));
        assert!(!qs.is_satisfied_by(&NodeSet::from_indices([0])));
    }

    #[test]
    fn quorum_set_threshold_clamped() {
        let qs = QuorumSet::new(5, vec![0, 1]);
        assert_eq!(qs.threshold, 2);
    }

    #[test]
    fn symmetric_is_quorum() {
        // 3-of-4 symmetric: any 3 nodes form a quorum, any 2 do not.
        let fbas = Fbas::symmetric(4, 3);
        assert!(fbas.is_quorum(&NodeSet::from_indices([0, 1, 2])));
        assert!(fbas.is_quorum(&NodeSet::full(4)));
        assert!(!fbas.is_quorum(&NodeSet::from_indices([0, 1])));
        assert!(!fbas.is_quorum(&NodeSet::EMPTY));
    }

    #[test]
    fn node_labels() {
        let fbas = Fbas::symmetric(3, 2);
        assert_eq!(fbas.nodes[0].id, "A");
        assert_eq!(fbas.nodes[1].id, "B");
        assert_eq!(fbas.nodes[2].id, "C");
    }

    #[test]
    fn admit_symmetric_grows_threshold() {
        // Start 3-of-3, admit one -> 3-of-4.
        let mut fbas = Fbas::symmetric_botho(3);
        assert_eq!(fbas.nodes[0].quorum_set.threshold, 3);
        fbas.admit_symmetric();
        assert_eq!(fbas.len(), 4);
        assert_eq!(fbas.nodes[0].quorum_set.threshold, 3); // 4 - floor(3/3) = 3
        assert_eq!(fbas.nodes[3].quorum_set.threshold, 3);
    }

    #[test]
    fn shun_reindexes_members() {
        // 4 nodes, arbitrary slices referencing each other.
        let mut fbas = Fbas::from_quorum_sets([
            QuorumSet::new(2, vec![0, 1, 2]),
            QuorumSet::new(2, vec![1, 2, 3]),
            QuorumSet::new(2, vec![0, 2, 3]),
            QuorumSet::new(2, vec![0, 1, 3]),
        ]);
        fbas.shun(1); // remove node index 1
        assert_eq!(fbas.len(), 3);
        // Old index 2 -> 1, old index 3 -> 2; index 1 dropped.
        assert_eq!(fbas.nodes[0].quorum_set.members, vec![0, 1]); // was [0,1,2]->[0,_,1]
    }

    #[test]
    fn shun_symmetric_shrinks() {
        let mut fbas = Fbas::symmetric_botho(4); // 3-of-4
        fbas.shun_symmetric(0);
        assert_eq!(fbas.len(), 3);
        assert_eq!(fbas.nodes[0].quorum_set.threshold, 3); // 3-of-3
    }
}
