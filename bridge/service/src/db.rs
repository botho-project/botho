// Copyright (c) 2024 The Botho Foundation

//! SQLite database for bridge order tracking.

use bth_bridge_core::{BridgeOrder, Chain, OrderStatus, OrderType};
use chrono::{TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqliteResult};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Database wrapper for thread-safe access.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

#[allow(dead_code)]
impl Database {
    /// Open or create the database.
    pub fn open(path: &str) -> Result<Self, String> {
        let conn =
            Connection::open(path).map_err(|e| format!("Failed to open database: {}", e))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("Failed to open in-memory database: {}", e))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run database migrations.
    pub fn migrate(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS bridge_orders (
                id TEXT PRIMARY KEY,
                order_type TEXT NOT NULL,
                source_chain TEXT NOT NULL,
                dest_chain TEXT NOT NULL,
                amount INTEGER NOT NULL,
                fee INTEGER NOT NULL DEFAULT 0,
                source_tx TEXT,
                dest_tx TEXT,
                source_address TEXT NOT NULL,
                dest_address TEXT NOT NULL,
                status TEXT NOT NULL,
                error_message TEXT,
                memo BLOB,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_orders_status ON bridge_orders(status);
            CREATE INDEX IF NOT EXISTS idx_orders_source_addr ON bridge_orders(source_address);
            CREATE INDEX IF NOT EXISTS idx_orders_dest_addr ON bridge_orders(dest_address);
            CREATE INDEX IF NOT EXISTS idx_orders_created ON bridge_orders(created_at);

            CREATE TABLE IF NOT EXISTS processed_deposits (
                tx_hash TEXT PRIMARY KEY,
                order_id TEXT REFERENCES bridge_orders(id),
                processed_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS rate_limits (
                address TEXT PRIMARY KEY,
                daily_volume INTEGER NOT NULL DEFAULT 0,
                last_reset INTEGER NOT NULL,
                request_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                order_id TEXT REFERENCES bridge_orders(id),
                action TEXT NOT NULL,
                details TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_audit_order ON audit_log(order_id);
            CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_log(created_at);
            "#,
        )
        .map_err(|e| format!("Migration failed: {}", e))?;

        Ok(())
    }

    /// Insert a new order.
    pub fn insert_order(&self, order: &BridgeOrder) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO bridge_orders (
                id, order_type, source_chain, dest_chain, amount, fee,
                source_tx, dest_tx, source_address, dest_address,
                status, error_message, memo, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            "#,
            params![
                order.id.to_string(),
                order.order_type.to_string(),
                order.source_chain.to_string(),
                order.dest_chain.to_string(),
                order.amount as i64,
                order.fee as i64,
                order.source_tx,
                order.dest_tx,
                order.source_address,
                order.dest_address,
                order.status.to_string(),
                order.error_message,
                order.memo.as_ref().map(|m| m.to_vec()),
                order.created_at.timestamp(),
                order.updated_at.timestamp(),
            ],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(())
    }

    /// Get an order by ID.
    pub fn get_order(&self, id: &Uuid) -> Result<Option<BridgeOrder>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, order_type, source_chain, dest_chain, amount, fee,
                       source_tx, dest_tx, source_address, dest_address,
                       status, error_message, memo, created_at, updated_at
                FROM bridge_orders WHERE id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let result = stmt
            .query_row(params![id.to_string()], |row| {
                Self::row_to_order(row)
            })
            .optional()
            .map_err(|e| format!("Query failed: {}", e))?;

        Ok(result)
    }

    /// Get orders by status.
    pub fn get_orders_by_status(&self, status: &str) -> Result<Vec<BridgeOrder>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, order_type, source_chain, dest_chain, amount, fee,
                       source_tx, dest_tx, source_address, dest_address,
                       status, error_message, memo, created_at, updated_at
                FROM bridge_orders WHERE status LIKE ?1
                ORDER BY created_at ASC
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let orders = stmt
            .query_map(params![format!("{}%", status)], |row| {
                Self::row_to_order(row)
            })
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;

        Ok(orders)
    }

    /// Update order status.
    pub fn update_order_status(
        &self,
        id: &Uuid,
        status: &OrderStatus,
        tx_hash: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let now = Utc::now().timestamp();
        let status_str = status.to_string();
        let error_msg = match status {
            OrderStatus::Failed { reason } => Some(reason.clone()),
            _ => None,
        };

        if let Some(hash) = tx_hash {
            // Update with transaction hash
            conn.execute(
                r#"
                UPDATE bridge_orders
                SET status = ?1, dest_tx = ?2, error_message = ?3, updated_at = ?4
                WHERE id = ?5
                "#,
                params![status_str, hash, error_msg, now, id.to_string()],
            )
            .map_err(|e| format!("Update failed: {}", e))?;
        } else {
            conn.execute(
                r#"
                UPDATE bridge_orders
                SET status = ?1, error_message = ?2, updated_at = ?3
                WHERE id = ?4
                "#,
                params![status_str, error_msg, now, id.to_string()],
            )
            .map_err(|e| format!("Update failed: {}", e))?;
        }

        Ok(())
    }

    /// Check if a deposit has been processed.
    pub fn is_deposit_processed(&self, tx_hash: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM processed_deposits WHERE tx_hash = ?1",
                params![tx_hash],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;

        Ok(count > 0)
    }

    /// Mark a deposit as processed.
    pub fn mark_deposit_processed(&self, tx_hash: &str, order_id: &Uuid) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            INSERT OR IGNORE INTO processed_deposits (tx_hash, order_id, processed_at)
            VALUES (?1, ?2, ?3)
            "#,
            params![tx_hash, order_id.to_string(), Utc::now().timestamp()],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(())
    }

    /// Log an audit event.
    pub fn log_audit(&self, order_id: Option<&Uuid>, action: &str, details: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO audit_log (order_id, action, details, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                order_id.map(|id| id.to_string()),
                action,
                details,
                Utc::now().timestamp()
            ],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(())
    }

    /// Convert a database row to a BridgeOrder.
    fn row_to_order(row: &rusqlite::Row<'_>) -> SqliteResult<BridgeOrder> {
        let id_str: String = row.get(0)?;
        let order_type_str: String = row.get(1)?;
        let source_chain_str: String = row.get(2)?;
        let dest_chain_str: String = row.get(3)?;
        let amount: i64 = row.get(4)?;
        let fee: i64 = row.get(5)?;
        let source_tx: Option<String> = row.get(6)?;
        let dest_tx: Option<String> = row.get(7)?;
        let source_address: String = row.get(8)?;
        let dest_address: String = row.get(9)?;
        let status_str: String = row.get(10)?;
        let error_message: Option<String> = row.get(11)?;
        let memo_bytes: Option<Vec<u8>> = row.get(12)?;
        let created_at: i64 = row.get(13)?;
        let updated_at: i64 = row.get(14)?;

        let memo = memo_bytes.and_then(|b| {
            if b.len() == 64 {
                let mut arr = [0u8; 64];
                arr.copy_from_slice(&b);
                Some(arr)
            } else {
                None
            }
        });

        Ok(BridgeOrder {
            id: Uuid::parse_str(&id_str).unwrap_or_default(),
            order_type: match order_type_str.as_str() {
                "mint" => OrderType::Mint,
                _ => OrderType::Burn,
            },
            source_chain: source_chain_str.parse().unwrap_or(Chain::Bth),
            dest_chain: dest_chain_str.parse().unwrap_or(Chain::Bth),
            amount: amount as u64,
            fee: fee as u64,
            source_tx,
            dest_tx,
            source_address,
            dest_address,
            status: parse_status(&status_str, error_message.clone()),
            error_message,
            memo,
            created_at: Utc.timestamp_opt(created_at, 0).unwrap(),
            updated_at: Utc.timestamp_opt(updated_at, 0).unwrap(),
        })
    }
}

fn parse_status(s: &str, error_msg: Option<String>) -> OrderStatus {
    match s {
        "awaiting_deposit" => OrderStatus::AwaitingDeposit,
        "deposit_detected" => OrderStatus::DepositDetected,
        "deposit_confirmed" => OrderStatus::DepositConfirmed,
        "mint_pending" => OrderStatus::MintPending,
        "completed" => OrderStatus::Completed,
        "burn_detected" => OrderStatus::BurnDetected,
        "burn_confirmed" => OrderStatus::BurnConfirmed,
        "release_pending" => OrderStatus::ReleasePending,
        "released" => OrderStatus::Released,
        "expired" => OrderStatus::Expired,
        s if s.starts_with("failed") => OrderStatus::Failed {
            reason: error_msg.unwrap_or_else(|| "Unknown error".to_string()),
        },
        _ => OrderStatus::AwaitingDeposit,
    }
}

// Extension trait for rusqlite optional queries
#[allow(dead_code)]
trait OptionalExt<T> {
    fn optional(self) -> SqliteResult<Option<T>>;
}

#[allow(dead_code)]
impl<T> OptionalExt<T> for SqliteResult<T> {
    fn optional(self) -> SqliteResult<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_operations() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        // Create and insert an order
        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_addr".to_string(),
            "0x1234...".to_string(),
        );

        db.insert_order(&order).unwrap();

        // Retrieve the order
        let retrieved = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(retrieved.id, order.id);
        assert_eq!(retrieved.amount, order.amount);

        // Update status
        db.update_order_status(&order.id, &OrderStatus::DepositConfirmed, None)
            .unwrap();

        let updated = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(updated.status, OrderStatus::DepositConfirmed);
    }

    #[test]
    fn test_processed_deposits() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        // Create an order first (required by foreign key constraint)
        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_addr".to_string(),
            "0xabc123".to_string(),
        );
        db.insert_order(&order).unwrap();

        let tx_hash = "0xabc123";

        assert!(!db.is_deposit_processed(tx_hash).unwrap());

        db.mark_deposit_processed(tx_hash, &order.id).unwrap();

        assert!(db.is_deposit_processed(tx_hash).unwrap());
    }
}
