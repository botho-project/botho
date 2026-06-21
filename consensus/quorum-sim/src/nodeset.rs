//! A compact set of node indices backed by a `u32` bitmask.
//!
//! Botho's curated federation is small (`N ≤ ~20`), so a single `u32` is more
//! than enough to address every node and lets us iterate over the `2^N` subsets
//! by simply counting from `0` to `(1 << n) - 1`.

use core::fmt;

/// Maximum number of nodes addressable by a [`NodeSet`].
///
/// Bounded by the `u32` backing store. Brute-force enumeration over `2^N`
/// subsets is only practical well below this limit; callers should keep
/// `N ≤ ~20` (the realistic curated-federation size).
pub const MAX_NODES: usize = 32;

/// A set of node indices, represented as a bitmask over node positions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeSet(pub u32);

impl NodeSet {
    /// The empty set.
    pub const EMPTY: NodeSet = NodeSet(0);

    /// Create an empty set.
    #[inline]
    pub const fn new() -> Self {
        NodeSet(0)
    }

    /// The full set of `n` nodes (indices `0..n`).
    #[inline]
    pub fn full(n: usize) -> Self {
        debug_assert!(n <= MAX_NODES);
        if n == 0 {
            NodeSet(0)
        } else if n >= 32 {
            NodeSet(u32::MAX)
        } else {
            NodeSet((1u32 << n) - 1)
        }
    }

    /// Build a set from an iterator of node indices.
    pub fn from_indices<I: IntoIterator<Item = usize>>(indices: I) -> Self {
        let mut s = NodeSet::new();
        for i in indices {
            s.insert(i);
        }
        s
    }

    /// Insert a node index.
    #[inline]
    pub fn insert(&mut self, idx: usize) {
        debug_assert!(idx < MAX_NODES);
        self.0 |= 1u32 << idx;
    }

    /// Remove a node index.
    #[inline]
    pub fn remove(&mut self, idx: usize) {
        debug_assert!(idx < MAX_NODES);
        self.0 &= !(1u32 << idx);
    }

    /// Whether `idx` is a member.
    #[inline]
    pub fn contains(&self, idx: usize) -> bool {
        debug_assert!(idx < MAX_NODES);
        (self.0 >> idx) & 1 == 1
    }

    /// Number of members.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.count_ones() as usize
    }

    /// Whether the set is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Set intersection.
    #[inline]
    pub fn intersect(&self, other: &NodeSet) -> NodeSet {
        NodeSet(self.0 & other.0)
    }

    /// Set union.
    #[inline]
    pub fn union(&self, other: &NodeSet) -> NodeSet {
        NodeSet(self.0 | other.0)
    }

    /// Set difference (`self \ other`).
    #[inline]
    pub fn difference(&self, other: &NodeSet) -> NodeSet {
        NodeSet(self.0 & !other.0)
    }

    /// Whether `self` and `other` share any member.
    #[inline]
    pub fn intersects(&self, other: &NodeSet) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether every member of `self` is also in `other`.
    #[inline]
    pub fn is_subset_of(&self, other: &NodeSet) -> bool {
        self.0 & other.0 == self.0
    }

    /// Iterate over the member indices in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..MAX_NODES).filter(move |&i| self.contains(i))
    }

    /// Collect members into a `Vec<usize>`.
    pub fn to_vec(&self) -> Vec<usize> {
        self.iter().collect()
    }
}

impl Default for NodeSet {
    fn default() -> Self {
        NodeSet::new()
    }
}

impl fmt::Debug for NodeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (n, i) in self.iter().enumerate() {
            if n > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{i}")?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ops() {
        let mut s = NodeSet::new();
        assert!(s.is_empty());
        s.insert(0);
        s.insert(3);
        assert_eq!(s.len(), 2);
        assert!(s.contains(0));
        assert!(s.contains(3));
        assert!(!s.contains(1));
        s.remove(0);
        assert!(!s.contains(0));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn full_set() {
        assert_eq!(NodeSet::full(0).len(), 0);
        assert_eq!(NodeSet::full(4).len(), 4);
        assert!(NodeSet::full(4).contains(3));
        assert!(!NodeSet::full(4).contains(4));
    }

    #[test]
    fn set_algebra() {
        let a = NodeSet::from_indices([0, 1]);
        let b = NodeSet::from_indices([1, 2]);
        assert_eq!(a.intersect(&b), NodeSet::from_indices([1]));
        assert_eq!(a.union(&b), NodeSet::from_indices([0, 1, 2]));
        assert_eq!(a.difference(&b), NodeSet::from_indices([0]));
        assert!(a.intersects(&b));
        assert!(!NodeSet::from_indices([0]).intersects(&NodeSet::from_indices([1])));
        assert!(NodeSet::from_indices([1]).is_subset_of(&a));
        assert!(!b.is_subset_of(&a));
    }

    #[test]
    fn iteration_order() {
        let s = NodeSet::from_indices([3, 0, 5]);
        assert_eq!(s.to_vec(), vec![0, 3, 5]);
    }
}
