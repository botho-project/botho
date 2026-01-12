//! Rollup and retention management
//!
//! Aggregates fine-grained data into coarser granularities and
//! enforces retention policies.
//!
//! Retention policy:
//! - 5-minute samples: 24 hours
//! - Hourly aggregates: 30 days
//! - Daily aggregates: 1 year

use anyhow::Result;
use chrono::{Utc, Duration, Timelike};
use tracing::{info, debug};

use crate::db::MetricsDb;

/// Retention periods in seconds
const RETENTION_5MIN: i64 = 24 * 3600;           // 24 hours
const RETENTION_HOURLY: i64 = 30 * 24 * 3600;    // 30 days
const RETENTION_DAILY: i64 = 365 * 24 * 3600;    // 1 year

/// Run the complete rollup process
pub fn run_rollup(db: &mut MetricsDb) -> Result<()> {
    let now = Utc::now();

    // Aggregate to hourly
    aggregate_hourly(db, now)?;

    // Aggregate to daily
    aggregate_daily(db, now)?;

    // Cleanup old data
    cleanup_old_data(db, now)?;

    Ok(())
}

/// Aggregate 5min samples from the last 2 hours into hourly buckets
fn aggregate_hourly(db: &mut MetricsDb, now: chrono::DateTime<Utc>) -> Result<()> {
    // Process last 2 hours to ensure we catch any late data
    let current_hour = now
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);

    for hours_ago in 1..=2 {
        let hour_start = (current_hour - Duration::hours(hours_ago)).timestamp();
        db.aggregate_to_hourly(hour_start)?;
        debug!("Aggregated hour starting at {}", hour_start);
    }

    info!("Hourly aggregation complete");
    Ok(())
}

/// Aggregate hourly samples from yesterday into daily buckets
fn aggregate_daily(db: &mut MetricsDb, now: chrono::DateTime<Utc>) -> Result<()> {
    // Process yesterday's data
    let today_start = now
        .with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);

    let yesterday_start = (today_start - Duration::days(1)).timestamp();

    db.aggregate_to_daily(yesterday_start)?;
    debug!("Aggregated day starting at {}", yesterday_start);

    info!("Daily aggregation complete");
    Ok(())
}

/// Clean up data older than retention period
fn cleanup_old_data(db: &mut MetricsDb, now: chrono::DateTime<Utc>) -> Result<()> {
    let now_ts = now.timestamp();

    // Cleanup 5min samples older than 24h
    let cleanup_5min_before = now_ts - RETENTION_5MIN;
    let deleted_5min = db.cleanup_5min(cleanup_5min_before)?;
    if deleted_5min > 0 {
        info!("Deleted {} old 5min samples", deleted_5min);
    }

    // Cleanup hourly samples older than 30d
    let cleanup_hourly_before = now_ts - RETENTION_HOURLY;
    let deleted_hourly = db.cleanup_hourly(cleanup_hourly_before)?;
    if deleted_hourly > 0 {
        info!("Deleted {} old hourly samples", deleted_hourly);
    }

    // Cleanup daily samples older than 1y
    let cleanup_daily_before = now_ts - RETENTION_DAILY;
    let deleted_daily = db.cleanup_daily(cleanup_daily_before)?;
    if deleted_daily > 0 {
        info!("Deleted {} old daily samples", deleted_daily);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use crate::db::MetricsSample;

    #[test]
    fn test_rollup_aggregation() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut db = MetricsDb::open(&db_path).unwrap();

        // Use current time rounded to start of hour
        let now = Utc::now();
        let base_time = now
            .with_minute(0)
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(now)
            .timestamp();

        // Insert 12 samples (one per 5 minutes = 1 hour of data)
        for i in 0..12 {
            let sample = MetricsSample {
                timestamp: base_time + i * 300, // Every 5 minutes
                height: 1000 + i as u64,
                peer_count: 5.0,
                scp_peer_count: 3.0,
                mempool_size: 10.0 + i as f64,
                tx_delta: 10,
                uptime_seconds: 1000,
                minting_active: true,
            };
            db.insert_sample(&sample).unwrap();
        }

        // Aggregate to hourly
        db.aggregate_to_hourly(base_time).unwrap();

        // Query should return data within the last 24h
        let query = crate::db::HistoryQuery {
            metric: "height".to_string(),
            period: "24h".to_string(),
            granularity: "5min".to_string(),
        };
        let data = db.query_history(&query).unwrap();
        assert_eq!(data.len(), 12);
    }
}
