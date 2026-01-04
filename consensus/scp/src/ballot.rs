// Copyright (c) 2018-2022 The Botho Foundation

//! The ballot contains the value on which to consense.

use bth_common::HasherBuilder;
use bth_consensus_scp_types::Value;
use bth_crypto_digestible::Digestible;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fmt,
    hash::{BuildHasher, Hash},
};

/// The ballot contains the value on which to consense.
///
/// The balloting protocol centers around successively higher ballots
/// which are moving through the phases of the federated voting.
///
/// Ballots are totally ordered, with "counter" more significant than "value."
#[derive(Hash, Eq, PartialEq, Debug, Clone, Serialize, Deserialize, Digestible)]
pub struct Ballot<V: Value> {
    /// Counter.
    pub N: u32,

    /// Values.
    pub X: Vec<V>,
}

impl<V: Value> Ballot<V> {
    /// Create a new Ballot with the given counter and values.
    pub fn new(counter: u32, values: &[V]) -> Self {
        Ballot {
            N: counter,
            X: values.to_vec(),
        }
    }

    /// Check whether the ballot's counter is 0 and values are empty.
    pub fn is_zero(&self) -> bool {
        self.N == 0 && self.X.is_empty()
    }

    /// Check whether the ballot's values are in canonical (sorted) order.
    ///
    /// For consensus safety, all nodes must process ballot values in the same
    /// order. Values must be strictly sorted (no duplicates) in ascending
    /// order.
    pub fn is_values_sorted(&self) -> bool {
        self.X.windows(2).all(|w| w[0] < w[1])
    }
}

// Ballots are totally ordered with N more significant than X.
impl<V: Value> Ord for Ballot<V> {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.N != other.N {
            return self.N.cmp(&other.N);
        }

        self.X.cmp(&other.X)
    }
}

impl<V: Value> PartialOrd for Ballot<V> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// This makes debugging easier when looking at large ballots.
impl<V: Value> fmt::Display for Ballot<V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let hasher = HasherBuilder::default();
        let hashed_X_values = hasher.hash_one(&self.X);
        write!(f, "<{}, {}:{:x}>", self.N, self.X.len(), hashed_X_values)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn total_ordering() {
        // Ballots are ordered first by counter `N`.
        {
            let high_ballot: Ballot<u32> = Ballot { N: 13, X: vec![] };
            let low_ballot: Ballot<u32> = Ballot {
                N: 4,
                X: vec![100, 200, 88],
            };
            assert!(high_ballot > low_ballot);
        }

        // Ballots are then ordered lexicographically by `X`.
        {
            let high_ballot: Ballot<u32> = Ballot {
                N: 13,
                X: vec![2000, 1000],
            };
            let low_ballot: Ballot<u32> = Ballot {
                N: 13,
                X: vec![1000, 2001],
            };
            assert!(high_ballot > low_ballot);
        }
    }

    #[test]
    fn test_ballot_new() {
        let ballot: Ballot<u32> = Ballot::new(5, &[100, 200, 300]);
        assert_eq!(ballot.N, 5);
        assert_eq!(ballot.X, vec![100, 200, 300]);
    }

    #[test]
    fn test_ballot_new_empty() {
        let ballot: Ballot<String> = Ballot::new(0, &[]);
        assert_eq!(ballot.N, 0);
        assert!(ballot.X.is_empty());
    }

    #[test]
    fn test_is_zero() {
        // Zero ballot: counter 0 and empty values
        let zero_ballot: Ballot<u32> = Ballot::new(0, &[]);
        assert!(zero_ballot.is_zero());

        // Not zero: has counter > 0
        let non_zero_1: Ballot<u32> = Ballot::new(1, &[]);
        assert!(!non_zero_1.is_zero());

        // Not zero: has values
        let non_zero_2: Ballot<u32> = Ballot::new(0, &[42]);
        assert!(!non_zero_2.is_zero());

        // Not zero: has both
        let non_zero_3: Ballot<u32> = Ballot::new(5, &[1, 2, 3]);
        assert!(!non_zero_3.is_zero());
    }

    #[test]
    fn test_ballot_equality() {
        let ballot1: Ballot<u32> = Ballot::new(5, &[100, 200]);
        let ballot2: Ballot<u32> = Ballot::new(5, &[100, 200]);
        let ballot3: Ballot<u32> = Ballot::new(5, &[100, 201]);
        let ballot4: Ballot<u32> = Ballot::new(6, &[100, 200]);

        assert_eq!(ballot1, ballot2);
        assert_ne!(ballot1, ballot3);
        assert_ne!(ballot1, ballot4);
    }

    #[test]
    fn test_ballot_clone() {
        let ballot: Ballot<u32> = Ballot::new(10, &[1, 2, 3]);
        let cloned = ballot.clone();
        assert_eq!(ballot, cloned);
    }

    #[test]
    fn test_ballot_display() {
        let ballot: Ballot<u32> = Ballot::new(5, &[100, 200, 300]);
        let display = format!("{}", ballot);
        // Format is <N, len:hash>
        assert!(display.starts_with("<5, 3:"));
        assert!(display.ends_with(">"));
    }

    #[test]
    fn test_ballot_ordering_comprehensive() {
        let b1: Ballot<u32> = Ballot::new(1, &[1]);
        let b2: Ballot<u32> = Ballot::new(1, &[2]);
        let b3: Ballot<u32> = Ballot::new(2, &[1]);
        let b4: Ballot<u32> = Ballot::new(2, &[2]);

        // b1 < b2 < b3 < b4
        assert!(b1 < b2);
        assert!(b2 < b3);
        assert!(b3 < b4);
        assert!(b1 < b3);
        assert!(b1 < b4);
        assert!(b2 < b4);

        // Verify partial_cmp returns consistent results
        assert_eq!(b1.partial_cmp(&b2), Some(Ordering::Less));
        assert_eq!(b2.partial_cmp(&b1), Some(Ordering::Greater));
        assert_eq!(b1.partial_cmp(&b1.clone()), Some(Ordering::Equal));
    }

    #[test]
    fn test_ballot_hash() {
        use std::collections::HashSet;

        let ballot1: Ballot<u32> = Ballot::new(5, &[100, 200]);
        let ballot2: Ballot<u32> = Ballot::new(5, &[100, 200]);
        let ballot3: Ballot<u32> = Ballot::new(5, &[100, 201]);

        let mut set = HashSet::new();
        set.insert(ballot1.clone());

        // Same ballot should be found
        assert!(set.contains(&ballot2));
        // Different ballot should not be found
        assert!(!set.contains(&ballot3));
    }

    #[test]
    fn test_ballot_with_string_values() {
        let ballot: Ballot<String> = Ballot::new(3, &["hello".to_string(), "world".to_string()]);
        assert_eq!(ballot.N, 3);
        assert_eq!(ballot.X.len(), 2);
        assert!(!ballot.is_zero());
    }

    #[test]
    fn test_ballot_ordering_same_counter_different_lengths() {
        // With same counter, longer value list comes after shorter
        let short: Ballot<u32> = Ballot::new(5, &[1]);
        let long: Ballot<u32> = Ballot::new(5, &[1, 2]);
        assert!(short < long);
    }

    #[test]
    fn test_ballot_ordering_empty_values() {
        let empty: Ballot<u32> = Ballot::new(5, &[]);
        let non_empty: Ballot<u32> = Ballot::new(5, &[1]);
        assert!(empty < non_empty);
    }

    #[test]
    fn test_is_values_sorted() {
        // Empty ballot is considered sorted
        let empty: Ballot<u32> = Ballot::new(0, &[]);
        assert!(empty.is_values_sorted());

        // Single value is sorted
        let single: Ballot<u32> = Ballot::new(1, &[42]);
        assert!(single.is_values_sorted());

        // Correctly sorted values
        let sorted: Ballot<u32> = Ballot::new(1, &[1, 2, 3, 4, 5]);
        assert!(sorted.is_values_sorted());

        // Unsorted values should fail
        let unsorted: Ballot<u32> = Ballot::new(1, &[3, 1, 2]);
        assert!(!unsorted.is_values_sorted());

        // Duplicate values should fail (not strictly sorted)
        let duplicates: Ballot<u32> = Ballot::new(1, &[1, 2, 2, 3]);
        assert!(!duplicates.is_values_sorted());

        // Reverse sorted should fail
        let reverse: Ballot<u32> = Ballot::new(1, &[5, 4, 3, 2, 1]);
        assert!(!reverse.is_values_sorted());
    }
}
