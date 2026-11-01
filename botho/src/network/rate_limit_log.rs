// Copyright (c) 2024 Botho Foundation

//! Per-peer summary for gossip rate-limit drops (#1547).
//!
//! A flooding peer can trip the gossip rate limiter hundreds of times a
//! second. Transaction and Consensus drops are never disconnect-eligible
//! (#413), so nothing bounds the flood on the receiving side. In the w12 run a
//! `warn!` per dropped message produced about 65k lines per 10 min per peer
//! and buried the real fault. The per-message line is now `debug!`. This
//! helper decides when to emit one summary `warn!` per peer per interval,
//! carrying the number of drops since the previous summary, broken down by
//! message type.
//!
//! The same helper summarizes gossipsub `SlowPeer` reports (#1572), which
//! otherwise warn once per slow peer per 1 s heartbeat.

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    hash::Hash,
    time::{Duration, Instant},
};

/// Minimum spacing between summary warnings for the same peer.
pub const RATE_LIMIT_SUMMARY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
struct PeerDrops {
    last_warn: Instant,
    dropped_since_warn: BTreeMap<&'static str, u64>,
}

/// Drops from one peer since its previous summary warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropSummary {
    /// Count per message-type label, sorted by label.
    pub by_type: BTreeMap<&'static str, u64>,
}

impl DropSummary {
    /// Total drops across all message types.
    pub fn total(&self) -> u64 {
        self.by_type.values().sum()
    }
}

impl fmt::Display for DropSummary {
    /// `consensus=480 transaction=6000`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (kind, n)) in self.by_type.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{kind}={n}")?;
        }
        Ok(())
    }
}

/// Tracks rate-limit drops per peer and rate-limits the summary warning.
#[derive(Debug)]
pub struct RateLimitLogSummary<K> {
    peers: HashMap<K, PeerDrops>,
    interval: Duration,
}

impl<K: Hash + Eq + Clone> Default for RateLimitLogSummary<K> {
    fn default() -> Self {
        Self::new(RATE_LIMIT_SUMMARY_INTERVAL)
    }
}

impl<K: Hash + Eq + Clone> RateLimitLogSummary<K> {
    /// Summary with the given minimum interval between warnings per peer.
    pub fn new(interval: Duration) -> Self {
        Self {
            peers: HashMap::new(),
            interval,
        }
    }

    /// Record one dropped message of type `kind` from `peer`.
    ///
    /// Returns `Some(summary)` when a summary warning should be emitted now.
    /// The summary counts the drops since the previous summary, including
    /// this one, per message type. The first drop from a peer always warns,
    /// so the onset of a flood is visible right away.
    pub fn record_drop(
        &mut self,
        peer: &K,
        kind: &'static str,
        now: Instant,
    ) -> Option<DropSummary> {
        self.record_drops(peer, &[(kind, 1)], now)
    }

    /// Record several drops for `peer` at once, `counts` giving the number
    /// per label (zero counts are ignored). Same warning cadence as
    /// [`Self::record_drop`]: the first call for a peer warns, later calls
    /// warn at most once per interval with everything since the last warning.
    /// Used for gossipsub `SlowPeer` reports, which arrive once per heartbeat
    /// with per-queue counts (#1572).
    pub fn record_drops(
        &mut self,
        peer: &K,
        counts: &[(&'static str, u64)],
        now: Instant,
    ) -> Option<DropSummary> {
        let add = |into: &mut BTreeMap<&'static str, u64>| {
            for &(kind, n) in counts {
                if n > 0 {
                    *into.entry(kind).or_insert(0) += n;
                }
            }
        };
        match self.peers.get_mut(peer) {
            None => {
                self.peers.insert(
                    peer.clone(),
                    PeerDrops {
                        last_warn: now,
                        dropped_since_warn: BTreeMap::new(),
                    },
                );
                let mut by_type = BTreeMap::new();
                add(&mut by_type);
                Some(DropSummary { by_type })
            }
            Some(p) => {
                add(&mut p.dropped_since_warn);
                if now.saturating_duration_since(p.last_warn) >= self.interval {
                    p.last_warn = now;
                    Some(DropSummary {
                        by_type: std::mem::take(&mut p.dropped_since_warn),
                    })
                } else {
                    None
                }
            }
        }
    }

    /// Forget a peer, e.g. on disconnect, so the map stays bounded.
    pub fn remove_peer(&mut self, peer: &K) {
        self.peers.remove(peer);
    }

    /// Number of peers currently tracked.
    #[cfg(test)]
    pub fn tracked_peers(&self) -> usize {
        self.peers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TX: &str = "transaction";
    const CONS: &str = "consensus";

    #[test]
    fn first_drop_warns_then_summarizes_once_per_interval() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let total = |r: Option<DropSummary>| r.map(|d| d.total());
        assert_eq!(
            total(s.record_drop(&1, TX, t0)),
            Some(1),
            "onset is visible"
        );

        // A 108 msg/s flood for just under a minute stays quiet.
        let mut warned = 0;
        for i in 1..6_480u64 {
            if s.record_drop(&1, TX, t0 + Duration::from_millis(i * 9))
                .is_some()
            {
                warned += 1;
            }
        }
        assert_eq!(warned, 0);

        // At the interval boundary one summary carries every drop since the
        // last warning.
        assert_eq!(
            total(s.record_drop(&1, TX, t0 + Duration::from_secs(60))),
            Some(6_480)
        );
        assert_eq!(s.record_drop(&1, TX, t0 + Duration::from_secs(61)), None);
        assert_eq!(
            total(s.record_drop(&1, TX, t0 + Duration::from_secs(120))),
            Some(2)
        );
    }

    /// The summary counts each message type separately, not under the type
    /// of the drop that happened to trigger it.
    #[test]
    fn summary_counts_drops_per_message_type() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(s.record_drop(&1, TX, t0).is_some());
        for i in 1..=5u64 {
            assert!(s.record_drop(&1, TX, t0 + Duration::from_secs(i)).is_none());
        }
        for i in 6..=7u64 {
            assert!(s
                .record_drop(&1, CONS, t0 + Duration::from_secs(i))
                .is_none());
        }
        // A consensus drop triggers the summary, but the five transaction
        // drops are still counted as transaction.
        let summary = s
            .record_drop(&1, CONS, t0 + Duration::from_secs(60))
            .expect("interval elapsed");
        assert_eq!(summary.by_type.get(TX), Some(&5));
        assert_eq!(summary.by_type.get(CONS), Some(&3));
        assert_eq!(summary.total(), 8);
        assert_eq!(summary.to_string(), "consensus=3 transaction=5");
        // The counters reset after a summary.
        let next = s
            .record_drop(&1, TX, t0 + Duration::from_secs(120))
            .expect("interval elapsed");
        assert_eq!(next.to_string(), "transaction=1");
    }

    #[test]
    fn peers_are_tracked_independently() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(s.record_drop(&1, TX, t0).is_some());
        assert!(s.record_drop(&2, TX, t0).is_some());
        assert_eq!(s.record_drop(&1, TX, t0 + Duration::from_secs(1)), None);
        assert_eq!(s.tracked_peers(), 2);
    }

    /// #1572: a slow peer reported every heartbeat warns once at onset, then
    /// once per interval with the per-queue totals since the last warning.
    #[test]
    fn batched_counts_summarize_per_interval() {
        const PRI: &str = "priority";
        const NON: &str = "non_priority";
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let first = s
            .record_drops(&1, &[(PRI, 0), (NON, 7)], t0)
            .expect("onset is visible");
        assert_eq!(first.to_string(), "non_priority=7");
        for i in 1..60u64 {
            assert_eq!(
                s.record_drops(&1, &[(PRI, 1), (NON, 2)], t0 + Duration::from_secs(i)),
                None,
                "one report per 1 s heartbeat stays quiet within the interval"
            );
        }
        let summary = s
            .record_drops(&1, &[(PRI, 1), (NON, 2)], t0 + Duration::from_secs(60))
            .expect("interval elapsed");
        assert_eq!(summary.by_type.get(PRI), Some(&60));
        assert_eq!(summary.by_type.get(NON), Some(&120));
        assert_eq!(summary.total(), 180);
    }

    #[test]
    fn removed_peer_starts_fresh() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(s.record_drop(&1, TX, t0).is_some());
        s.remove_peer(&1);
        assert_eq!(s.tracked_peers(), 0);
        assert!(
            s.record_drop(&1, TX, t0 + Duration::from_secs(1)).is_some(),
            "a re-connected peer's first drop warns again"
        );
    }
}
