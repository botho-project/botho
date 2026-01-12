//! SQLite database for metrics storage
//!
//! Schema:
//! - metrics_5min: Raw 5-minute samples (24h retention)
//! - metrics_hourly: Hourly aggregates (30d retention)
//! - metrics_daily: Daily aggregates (1y retention)

use std::path::Path;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// A single metrics sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSample {
    pub timestamp: i64,
    pub height: u64,
    pub peer_count: f64,
    pub scp_peer_count: f64,
    pub mempool_size: f64,
    pub tx_delta: i64,
    pub uptime_seconds: u64,
    pub minting_active: bool,
}

/// Query parameters for historical data
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryQuery {
    pub metric: String,
    pub period: String,      // "1h", "24h", "7d", "30d"
    pub granularity: String, // "5min", "hourly", "daily"
}

/// A data point for API response
#[derive(Debug, Clone, Serialize)]
pub struct DataPoint {
    pub timestamp: i64,
    pub value: f64,
}

/// Metrics database wrapper
pub struct MetricsDb {
    conn: Connection,
}

impl MetricsDb {
    /// Open or create the metrics database
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Create tables
        conn.execute_batch(r#"
            -- 5-minute samples (raw data, 24h retention)
            CREATE TABLE IF NOT EXISTS metrics_5min (
                timestamp INTEGER PRIMARY KEY,
                height INTEGER NOT NULL,
                peer_count REAL NOT NULL,
                scp_peer_count REAL NOT NULL,
                mempool_size REAL NOT NULL,
                tx_delta INTEGER NOT NULL,
                uptime_seconds INTEGER NOT NULL,
                minting_active INTEGER NOT NULL
            );

            -- Hourly aggregates (30d retention)
            CREATE TABLE IF NOT EXISTS metrics_hourly (
                timestamp INTEGER PRIMARY KEY,
                height INTEGER NOT NULL,
                peer_count_avg REAL NOT NULL,
                scp_peer_count_avg REAL NOT NULL,
                mempool_size_avg REAL NOT NULL,
                tx_total INTEGER NOT NULL,
                samples INTEGER NOT NULL
            );

            -- Daily aggregates (1y retention)
            CREATE TABLE IF NOT EXISTS metrics_daily (
                timestamp INTEGER PRIMARY KEY,
                height INTEGER NOT NULL,
                peer_count_avg REAL NOT NULL,
                scp_peer_count_avg REAL NOT NULL,
                mempool_size_avg REAL NOT NULL,
                tx_total INTEGER NOT NULL,
                samples INTEGER NOT NULL
            );

            -- Indexes for efficient queries
            CREATE INDEX IF NOT EXISTS idx_5min_ts ON metrics_5min(timestamp);
            CREATE INDEX IF NOT EXISTS idx_hourly_ts ON metrics_hourly(timestamp);
            CREATE INDEX IF NOT EXISTS idx_daily_ts ON metrics_daily(timestamp);

            -- Track last known height for delta calculation
            CREATE TABLE IF NOT EXISTS state (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            );
        "#)?;

        Ok(Self { conn })
    }

    /// Insert a 5-minute sample
    pub fn insert_sample(&mut self, sample: &MetricsSample) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO metrics_5min
             (timestamp, height, peer_count, scp_peer_count, mempool_size, tx_delta, uptime_seconds, minting_active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
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

    /// Get the last recorded total_tx for delta calculation
    pub fn get_last_tx_count(&self) -> Result<Option<u64>> {
        let result: Option<i64> = self.conn.query_row(
            "SELECT value FROM state WHERE key = 'last_tx_count'",
            [],
            |row| row.get(0),
        ).ok();
        Ok(result.map(|v| v as u64))
    }

    /// Update the last recorded total_tx
    pub fn set_last_tx_count(&mut self, count: u64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES ('last_tx_count', ?1)",
            params![count as i64],
        )?;
        Ok(())
    }

    /// Aggregate 5min data into hourly (for a specific hour)
    pub fn aggregate_to_hourly(&mut self, hour_start: i64) -> Result<()> {
        let hour_end = hour_start + 3600;

        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO metrics_hourly
            (timestamp, height, peer_count_avg, scp_peer_count_avg, mempool_size_avg, tx_total, samples)
            SELECT
                ?1 as timestamp,
                MAX(height) as height,
                AVG(peer_count) as peer_count_avg,
                AVG(scp_peer_count) as scp_peer_count_avg,
                AVG(mempool_size) as mempool_size_avg,
                SUM(tx_delta) as tx_total,
                COUNT(*) as samples
            FROM metrics_5min
            WHERE timestamp >= ?1 AND timestamp < ?2
            "#,
            params![hour_start, hour_end],
        )?;
        Ok(())
    }

    /// Aggregate hourly data into daily (for a specific day)
    pub fn aggregate_to_daily(&mut self, day_start: i64) -> Result<()> {
        let day_end = day_start + 86400;

        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO metrics_daily
            (timestamp, height, peer_count_avg, scp_peer_count_avg, mempool_size_avg, tx_total, samples)
            SELECT
                ?1 as timestamp,
                MAX(height) as height,
                AVG(peer_count_avg) as peer_count_avg,
                AVG(scp_peer_count_avg) as scp_peer_count_avg,
                AVG(mempool_size_avg) as mempool_size_avg,
                SUM(tx_total) as tx_total,
                SUM(samples) as samples
            FROM metrics_hourly
            WHERE timestamp >= ?1 AND timestamp < ?2
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

    /// Query historical data for a specific metric
    pub fn query_history(&self, query: &HistoryQuery) -> Result<Vec<DataPoint>> {
        let now = chrono::Utc::now().timestamp();

        // Parse period to seconds
        let period_secs = match query.period.as_str() {
            "1h" => 3600,
            "6h" => 21600,
            "24h" => 86400,
            "7d" => 604800,
            "30d" => 2592000,
            _ => 86400, // Default to 24h
        };

        let since = now - period_secs;

        // Select table based on granularity
        let (table, column) = match (query.granularity.as_str(), query.metric.as_str()) {
            ("5min", "height") => ("metrics_5min", "height"),
            ("5min", "peerCount") => ("metrics_5min", "peer_count"),
            ("5min", "scpPeerCount") => ("metrics_5min", "scp_peer_count"),
            ("5min", "mempoolSize") => ("metrics_5min", "mempool_size"),
            ("5min", "txDelta") => ("metrics_5min", "tx_delta"),

            ("hourly", "height") => ("metrics_hourly", "height"),
            ("hourly", "peerCount") => ("metrics_hourly", "peer_count_avg"),
            ("hourly", "scpPeerCount") => ("metrics_hourly", "scp_peer_count_avg"),
            ("hourly", "mempoolSize") => ("metrics_hourly", "mempool_size_avg"),
            ("hourly", "txTotal") => ("metrics_hourly", "tx_total"),

            ("daily", "height") => ("metrics_daily", "height"),
            ("daily", "peerCount") => ("metrics_daily", "peer_count_avg"),
            ("daily", "scpPeerCount") => ("metrics_daily", "scp_peer_count_avg"),
            ("daily", "mempoolSize") => ("metrics_daily", "mempool_size_avg"),
            ("daily", "txTotal") => ("metrics_daily", "tx_total"),

            // Default to 5min height
            _ => ("metrics_5min", "height"),
        };

        let sql = format!(
            "SELECT timestamp, {} as value FROM {} WHERE timestamp >= ?1 ORDER BY timestamp ASC",
            column, table
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![since], |row| {
            Ok(DataPoint {
                timestamp: row.get(0)?,
                value: row.get(1)?,
            })
        })?;

        let mut data = Vec::new();
        for row in rows {
            data.push(row?);
        }

        Ok(data)
    }

    /// Get latest sample for current status
    pub fn get_latest(&self) -> Result<Option<MetricsSample>> {
        let result = self.conn.query_row(
            "SELECT timestamp, height, peer_count, scp_peer_count, mempool_size, tx_delta, uptime_seconds, minting_active
             FROM metrics_5min ORDER BY timestamp DESC LIMIT 1",
            [],
            |row| {
                Ok(MetricsSample {
                    timestamp: row.get(0)?,
                    height: row.get::<_, i64>(1)? as u64,
                    peer_count: row.get(2)?,
                    scp_peer_count: row.get(3)?,
                    mempool_size: row.get(4)?,
                    tx_delta: row.get(5)?,
                    uptime_seconds: row.get::<_, i64>(6)? as u64,
                    minting_active: row.get::<_, i32>(7)? != 0,
                })
            },
        );

        match result {
            Ok(sample) => Ok(Some(sample)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
