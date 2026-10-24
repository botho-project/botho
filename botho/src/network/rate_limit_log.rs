// Copyright (c) 2024 Botho Foundation

//! Per-peer summary for gossip rate-limit drops (#1547).
//!
//! A flooding peer can trip the gossip rate limiter hundreds of times a
//! second. Transaction and Consensus drops are never disconnect-eligible
//! (#413), so nothing bounds the flood on the receiving side. In the w12 run a
//! `warn!` per dropped message produced about 65k lines per 10 min per peer
//! and buried the real fault. The per-message line is now `debug!`. This
//! helper decides when to emit one summary `warn!` per peer per interval,
//! carrying the number of drops since the previous summary.

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

/// Minimum spacing between summary warnings for the same peer.
pub const RATE_LIMIT_SUMMARY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
struct PeerDrops {
    last_warn: Instant,
    dropped_since_warn: u64,
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

    /// Record one dropped message from `peer`.
    ///
    /// Returns `Some(count)` when a summary warning should be emitted now.
    /// `count` is the number of drops since the previous summary, including
    /// this one. The first drop from a peer always warns, so the onset of a
    /// flood is visible right away.
    pub fn record_drop(&mut self, peer: &K, now: Instant) -> Option<u64> {
        match self.peers.get_mut(peer) {
            None => {
                self.peers.insert(
                    peer.clone(),
                    PeerDrops {
                        last_warn: now,
                        dropped_since_warn: 0,
                    },
                );
                Some(1)
            }
            Some(p) => {
                p.dropped_since_warn += 1;
                if now.saturating_duration_since(p.last_warn) >= self.interval {
                    let count = p.dropped_since_warn;
                    p.last_warn = now;
                    p.dropped_since_warn = 0;
                    Some(count)
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

    #[test]
    fn first_drop_warns_then_summarizes_once_per_interval() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert_eq!(s.record_drop(&1, t0), Some(1), "onset is visible");

        // A 108 msg/s flood for just under a minute stays quiet.
        let mut warned = 0;
        for i in 1..6_480u64 {
            if s.record_drop(&1, t0 + Duration::from_millis(i * 9))
                .is_some()
            {
                warned += 1;
            }
        }
        assert_eq!(warned, 0);

        // At the interval boundary one summary carries every drop since the
        // last warning.
        assert_eq!(s.record_drop(&1, t0 + Duration::from_secs(60)), Some(6_480));
        assert_eq!(s.record_drop(&1, t0 + Duration::from_secs(61)), None);
        assert_eq!(s.record_drop(&1, t0 + Duration::from_secs(120)), Some(2));
    }

    #[test]
    fn peers_are_tracked_independently() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert_eq!(s.record_drop(&1, t0), Some(1));
        assert_eq!(s.record_drop(&2, t0), Some(1));
        assert_eq!(s.record_drop(&1, t0 + Duration::from_secs(1)), None);
        assert_eq!(s.tracked_peers(), 2);
    }

    #[test]
    fn removed_peer_starts_fresh() {
        let mut s = RateLimitLogSummary::<u32>::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert_eq!(s.record_drop(&1, t0), Some(1));
        s.remove_peer(&1);
        assert_eq!(s.tracked_peers(), 0);
        assert_eq!(s.record_drop(&1, t0 + Duration::from_secs(1)), Some(1));
    }
}
