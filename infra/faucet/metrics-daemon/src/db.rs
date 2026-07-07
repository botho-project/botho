//! SQLite database for metrics storage
//!
//! Schema (all tables keyed per node):
//! - metrics_5min: Raw 5-minute samples (24h retention)
//! - metrics_hourly: Hourly aggregates (30d retention)
//! - metrics_daily: Daily aggregates (1y retention)
//!
//! The testnet database is disposable: the schema is created fresh with
//! `CREATE TABLE IF NOT EXISTS` and there is no migration path from the
//! old single-node schema (delete the old .db file when deploying).

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Number of most-recent samples whose heights must all match for a node
/// to be flagged height-stale.
pub const STALE_SAMPLE_WINDOW: usize = 3;

/// A single metrics sample for one node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSample {
    pub node: String,
    pub timestamp: i64,
    pub height: u64,
    pub peer_count: f64,
    pub scp_peer_count: f64,
    pub mempool_size: f64,
    pub tx_delta: i64,
    pub uptime_seconds: u64,
    pub minting_active: bool,
}

/// Latest sample for one node, plus the derived height-staleness flag.
///
/// Serialized shape is the /api/metrics/latest contract:
/// `{node, timestamp, height, peerCount, scpPeerCount, mempoolSize,
///   mintingActive, uptimeSeconds, heightStale}`
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeLatest {
    pub node: String,
    pub timestamp: i64,
    pub height: u64,
    pub peer_count: f64,
    pub scp_peer_count: f64,
    pub mempool_size: f64,
    pub minting_active: bool,
    pub uptime_seconds: u64,
    pub height_stale: bool,
}

/// Table resolution for history queries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    FiveMin,
    Hourly,
    Daily,
}

impl Resolution {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "5min" => Some(Self::FiveMin),
            "hourly" => Some(Self::Hourly),
            "daily" => Some(Self::Daily),
            _ => None,
        }
    }
}

/// A normalized history point (works across the 5min/hourly/daily tables).
///
/// For the 5min table `tx_total` is the per-sample tx delta; for hourly and
/// daily it is the summed tx count over the bucket.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub timestamp: i64,
    pub height: u64,
    pub peer_count: f64,
    pub scp_peer_count: f64,
    pub mempool_size: f64,
    pub tx_total: i64,
}

/// Metrics database wrapper
pub struct MetricsDb {
    conn: Connection,
}

impl MetricsDb {
    /// Open or create the metrics database
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (tests)
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            -- 5-minute samples (raw data, 24h retention)
            CREATE TABLE IF NOT EXISTS metrics_5min (
                node TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                height INTEGER NOT NULL,
                peer_count REAL NOT NULL,
                scp_peer_count REAL NOT NULL,
                mempool_size REAL NOT NULL,
                tx_delta INTEGER NOT NULL,
                uptime_seconds INTEGER NOT NULL,
                minting_active INTEGER NOT NULL,
                PRIMARY KEY (node, timestamp)
            );

            -- Hourly aggregates (30d retention)
            CREATE TABLE IF NOT EXISTS metrics_hourly (
                node TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                height INTEGER NOT NULL,
                peer_count_avg REAL NOT NULL,
                scp_peer_count_avg REAL NOT NULL,
                mempool_size_avg REAL NOT NULL,
                tx_total INTEGER NOT NULL,
                samples INTEGER NOT NULL,
                PRIMARY KEY (node, timestamp)
            );

            -- Daily aggregates (1y retention)
            CREATE TABLE IF NOT EXISTS metrics_daily (
                node TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                height INTEGER NOT NULL,
                peer_count_avg REAL NOT NULL,
                scp_peer_count_avg REAL NOT NULL,
                mempool_size_avg REAL NOT NULL,
                tx_total INTEGER NOT NULL,
                samples INTEGER NOT NULL,
                PRIMARY KEY (node, timestamp)
            );

            -- Indexes for efficient time-range queries
            CREATE INDEX IF NOT EXISTS idx_5min_ts ON metrics_5min(timestamp);
            CREATE INDEX IF NOT EXISTS idx_hourly_ts ON metrics_hourly(timestamp);
            CREATE INDEX IF NOT EXISTS idx_daily_ts ON metrics_daily(timestamp);

            -- Per-node collector state (e.g. last known total_tx for deltas)
            CREATE TABLE IF NOT EXISTS state (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            );
        "#,
        )?;
        Ok(())
    }

    /// Insert a 5-minute sample
    pub fn insert_sample(&mut self, sample: &MetricsSample) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO metrics_5min
             (node, timestamp, height, peer_count, scp_peer_count, mempool_size, tx_delta, uptime_seconds, minting_active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                sample.node,
                sample.timestamp,
                sample.height as i64,
                sample.peer_count,
                sample.scp_peer_count,
                sample.mempool_size,
                sample.tx_delta,
                sample.uptime_seconds as i64,
                sample.minting_active as i32,
            ],
        )?;
        Ok(())
    }

    /// Get the last recorded total_tx for a node (for delta calculation)
    pub fn get_last_tx_count(&self, node: &str) -> Result<Option<u64>> {
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                params![format!("last_tx_count:{node}")],
                |row| row.get(0),
            )
            .ok();
        Ok(result.map(|v| v as u64))
    }

    /// Update the last recorded total_tx for a node
    pub fn set_last_tx_count(&mut self, node: &str, count: u64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES (?1, ?2)",
            params![format!("last_tx_count:{node}"), count as i64],
        )?;
        Ok(())
    }

    /// Aggregate 5min data into hourly buckets (for a specific hour), per node
    pub fn aggregate_to_hourly(&mut self, hour_start: i64) -> Result<()> {
        let hour_end = hour_start + 3600;

        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO metrics_hourly
            (node, timestamp, height, peer_count_avg, scp_peer_count_avg, mempool_size_avg, tx_total, samples)
            SELECT
                node,
                ?1 as timestamp,
                MAX(height) as height,
                AVG(peer_count) as peer_count_avg,
                AVG(scp_peer_count) as scp_peer_count_avg,
                AVG(mempool_size) as mempool_size_avg,
                SUM(tx_delta) as tx_total,
                COUNT(*) as samples
            FROM metrics_5min
            WHERE timestamp >= ?1 AND timestamp < ?2
            GROUP BY node
            "#,
            params![hour_start, hour_end],
        )?;
        Ok(())
    }

    /// Aggregate hourly data into daily buckets (for a specific day), per node
    pub fn aggregate_to_daily(&mut self, day_start: i64) -> Result<()> {
        let day_end = day_start + 86400;

        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO metrics_daily
            (node, timestamp, height, peer_count_avg, scp_peer_count_avg, mempool_size_avg, tx_total, samples)
            SELECT
                node,
                ?1 as timestamp,
                MAX(height) as height,
                AVG(peer_count_avg) as peer_count_avg,
                AVG(scp_peer_count_avg) as scp_peer_count_avg,
                AVG(mempool_size_avg) as mempool_size_avg,
                SUM(tx_total) as tx_total,
                SUM(samples) as samples
            FROM metrics_hourly
            WHERE timestamp >= ?1 AND timestamp < ?2
            GROUP BY node
            "#,
            params![day_start, day_end],
        )?;
        Ok(())
    }

    /// Delete old 5min samples (older than 24h)
    pub fn cleanup_5min(&mut self, before: i64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM metrics_5min WHERE timestamp < ?1",
            params![before],
        )?;
        Ok(count)
    }

    /// Delete old hourly samples (older than 30d)
    pub fn cleanup_hourly(&mut self, before: i64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM metrics_hourly WHERE timestamp < ?1",
            params![before],
        )?;
        Ok(count)
    }

    /// Delete old daily samples (older than 1y)
    pub fn cleanup_daily(&mut self, before: i64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM metrics_daily WHERE timestamp < ?1",
            params![before],
        )?;
        Ok(count)
    }

    /// Query history for one node at a given resolution, from `since` onward.
    pub fn query_node_history(
        &self,
        node: &str,
        resolution: Resolution,
        since: i64,
    ) -> Result<Vec<HistoryPoint>> {
        let sql = match resolution {
            Resolution::FiveMin => {
                "SELECT timestamp, height, peer_count, scp_peer_count, mempool_size, tx_delta
                 FROM metrics_5min WHERE node = ?1 AND timestamp >= ?2 ORDER BY timestamp ASC"
            }
            Resolution::Hourly => {
                "SELECT timestamp, height, peer_count_avg, scp_peer_count_avg, mempool_size_avg, tx_total
                 FROM metrics_hourly WHERE node = ?1 AND timestamp >= ?2 ORDER BY timestamp ASC"
            }
            Resolution::Daily => {
                "SELECT timestamp, height, peer_count_avg, scp_peer_count_avg, mempool_size_avg, tx_total
                 FROM metrics_daily WHERE node = ?1 AND timestamp >= ?2 ORDER BY timestamp ASC"
            }
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![node, since], |row| {
            Ok(HistoryPoint {
                timestamp: row.get(0)?,
                height: row.get::<_, i64>(1)? as u64,
                peer_count: row.get(2)?,
                scp_peer_count: row.get(3)?,
                mempool_size: row.get(4)?,
                tx_total: row.get(5)?,
            })
        })?;

        let mut data = Vec::new();
        for row in rows {
            data.push(row?);
        }
        Ok(data)
    }

    /// True when the node's height is unchanged across the last
    /// `window` samples. Nodes with fewer than `window` samples are
    /// not considered stale (not enough evidence).
    pub fn is_height_stale(&self, node: &str, window: usize) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT height FROM metrics_5min WHERE node = ?1 ORDER BY timestamp DESC LIMIT ?2",
        )?;
        let heights: Vec<i64> = stmt
            .query_map(params![node, window as i64], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;

        Ok(heights.len() == window && heights.iter().all(|h| *h == heights[0]))
    }

    /// Latest sample per node (with staleness flag), ordered by node name.
    pub fn get_latest_per_node(&self) -> Result<Vec<NodeLatest>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT m.node, m.timestamp, m.height, m.peer_count, m.scp_peer_count,
                   m.mempool_size, m.minting_active, m.uptime_seconds
            FROM metrics_5min m
            JOIN (SELECT node, MAX(timestamp) AS ts FROM metrics_5min GROUP BY node) latest
              ON m.node = latest.node AND m.timestamp = latest.ts
            ORDER BY m.node ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(NodeLatest {
                node: row.get(0)?,
                timestamp: row.get(1)?,
                height: row.get::<_, i64>(2)? as u64,
                peer_count: row.get(3)?,
                scp_peer_count: row.get(4)?,
                mempool_size: row.get(5)?,
                minting_active: row.get::<_, i32>(6)? != 0,
                uptime_seconds: row.get::<_, i64>(7)? as u64,
                height_stale: false, // filled in below
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        for entry in &mut result {
            entry.height_stale = self.is_height_stale(&entry.node, STALE_SAMPLE_WINDOW)?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(node: &str, timestamp: i64, height: u64) -> MetricsSample {
        MetricsSample {
            node: node.to_string(),
            timestamp,
            height,
            peer_count: 4.0,
            scp_peer_count: 3.0,
            mempool_size: 2.0,
            tx_delta: 7,
            uptime_seconds: 1000,
            minting_active: node == "faucet",
        }
    }

    #[test]
    fn test_per_node_insert_and_latest() {
        let mut db = MetricsDb::open_in_memory().unwrap();

        db.insert_sample(&sample("seed", 1000, 50)).unwrap();
        db.insert_sample(&sample("seed", 1300, 51)).unwrap();
        db.insert_sample(&sample("faucet", 1000, 49)).unwrap();

        let latest = db.get_latest_per_node().unwrap();
        assert_eq!(latest.len(), 2);

        // Ordered by node name
        assert_eq!(latest[0].node, "faucet");
        assert_eq!(latest[0].timestamp, 1000);
        assert_eq!(latest[0].height, 49);
        assert!(latest[0].minting_active);

        assert_eq!(latest[1].node, "seed");
        assert_eq!(latest[1].timestamp, 1300);
        assert_eq!(latest[1].height, 51);
        assert!(!latest[1].minting_active);
    }

    #[test]
    fn test_same_timestamp_different_nodes_do_not_collide() {
        let mut db = MetricsDb::open_in_memory().unwrap();

        // Same timestamp for every node: the (node, timestamp) primary key
        // must keep them as distinct rows.
        for node in ["seed", "seed2", "faucet", "eu", "ap"] {
            db.insert_sample(&sample(node, 1000, 42)).unwrap();
        }

        let latest = db.get_latest_per_node().unwrap();
        assert_eq!(latest.len(), 5);
    }

    #[test]
    fn test_height_staleness() {
        let mut db = MetricsDb::open_in_memory().unwrap();

        // "stuck": 3 samples, same height -> stale
        for i in 0..3 {
            db.insert_sample(&sample("stuck", 1000 + i * 300, 100))
                .unwrap();
        }
        // "moving": 3 samples, advancing height -> not stale
        for i in 0..3 {
            db.insert_sample(&sample("moving", 1000 + i * 300, 100 + i as u64))
                .unwrap();
        }
        // "young": only 2 samples -> not enough evidence, not stale
        for i in 0..2 {
            db.insert_sample(&sample("young", 1000 + i * 300, 100))
                .unwrap();
        }
        // "recovered": stalled earlier but last sample advanced -> not stale
        db.insert_sample(&sample("recovered", 1000, 100)).unwrap();
        db.insert_sample(&sample("recovered", 1300, 100)).unwrap();
        db.insert_sample(&sample("recovered", 1600, 101)).unwrap();

        assert!(db.is_height_stale("stuck", STALE_SAMPLE_WINDOW).unwrap());
        assert!(!db.is_height_stale("moving", STALE_SAMPLE_WINDOW).unwrap());
        assert!(!db.is_height_stale("young", STALE_SAMPLE_WINDOW).unwrap());
        assert!(!db
            .is_height_stale("recovered", STALE_SAMPLE_WINDOW)
            .unwrap());

        // /latest surfaces the same flags
        let latest = db.get_latest_per_node().unwrap();
        let stale_map: std::collections::HashMap<_, _> = latest
            .iter()
            .map(|e| (e.node.as_str(), e.height_stale))
            .collect();
        assert_eq!(stale_map["stuck"], true);
        assert_eq!(stale_map["moving"], false);
        assert_eq!(stale_map["young"], false);
        assert_eq!(stale_map["recovered"], false);
    }

    #[test]
    fn test_rollup_groups_by_node() {
        let mut db = MetricsDb::open_in_memory().unwrap();

        let hour_start = 1_700_000_000i64 / 3600 * 3600;

        // 12 samples per node within the hour, different heights per node
        for i in 0..12i64 {
            db.insert_sample(&sample("seed", hour_start + i * 300, 2000 + i as u64))
                .unwrap();
            db.insert_sample(&sample("eu", hour_start + i * 300, 1000 + i as u64))
                .unwrap();
        }

        db.aggregate_to_hourly(hour_start).unwrap();

        let seed = db
            .query_node_history("seed", Resolution::Hourly, hour_start)
            .unwrap();
        let eu = db
            .query_node_history("eu", Resolution::Hourly, hour_start)
            .unwrap();

        assert_eq!(seed.len(), 1);
        assert_eq!(eu.len(), 1);
        assert_eq!(seed[0].height, 2011); // MAX(height) for seed
        assert_eq!(eu[0].height, 1011); // MAX(height) for eu
        assert_eq!(seed[0].tx_total, 12 * 7); // SUM(tx_delta)

        // Daily rollup also groups by node
        let day_start = hour_start / 86400 * 86400;
        db.aggregate_to_daily(day_start).unwrap();
        let seed_daily = db
            .query_node_history("seed", Resolution::Daily, day_start)
            .unwrap();
        assert_eq!(seed_daily.len(), 1);
        assert_eq!(seed_daily[0].height, 2011);
    }

    #[test]
    fn test_history_since_filter_and_unknown_node() {
        let mut db = MetricsDb::open_in_memory().unwrap();

        db.insert_sample(&sample("seed", 1000, 1)).unwrap();
        db.insert_sample(&sample("seed", 1300, 2)).unwrap();
        db.insert_sample(&sample("seed", 1600, 3)).unwrap();

        let all = db
            .query_node_history("seed", Resolution::FiveMin, 0)
            .unwrap();
        assert_eq!(all.len(), 3);

        let recent = db
            .query_node_history("seed", Resolution::FiveMin, 1300)
            .unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].timestamp, 1300);

        // Unknown node -> empty array, not an error
        let none = db
            .query_node_history("nope", Resolution::FiveMin, 0)
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn test_per_node_tx_count_state() {
        let mut db = MetricsDb::open_in_memory().unwrap();

        assert_eq!(db.get_last_tx_count("seed").unwrap(), None);
        db.set_last_tx_count("seed", 100).unwrap();
        db.set_last_tx_count("eu", 55).unwrap();
        assert_eq!(db.get_last_tx_count("seed").unwrap(), Some(100));
        assert_eq!(db.get_last_tx_count("eu").unwrap(), Some(55));
    }
}
