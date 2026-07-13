// Copyright (c) 2024 The Botho Foundation

//! SQLite database for bridge order tracking.

use bth_bridge_core::{BridgeOrder, Chain, MintAuthorization, OrderStatus, OrderType};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqliteResult};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// A row in the `mints` idempotency table.
///
/// One row exists per order that has ever had a destination-chain mint
/// transaction prepared. The `order_id` UNIQUE constraint is the service-side
/// exactly-once guard: a resubmission finds the existing row and reuses the
/// prior transaction instead of double-minting. (The contract-side order-id
/// guard, #826, is the on-chain backstop.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintRecord {
    /// Bridge order UUID (string form).
    pub order_id: Uuid,
    /// Hex encoding of the 32-byte on-chain order id bound to the mint.
    pub order_id_hash: String,
    /// Destination chain the mint was submitted to.
    pub chain: Chain,
    /// Destination-chain transaction id (ETH tx hash / Solana signature).
    pub dest_tx: String,
    /// When the transaction was persisted (always before first broadcast).
    pub submitted_at: DateTime<Utc>,
    /// When the transaction reached the required confirmation depth.
    pub confirmed_at: Option<DateTime<Utc>>,
}

/// Database wrapper for thread-safe access.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

#[allow(dead_code)]
impl Database {
    /// Open or create the database.
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open database: {}", e))?;

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

            CREATE TABLE IF NOT EXISTS mints (
                order_id TEXT PRIMARY KEY REFERENCES bridge_orders(id),
                order_id_hash TEXT NOT NULL,
                chain TEXT NOT NULL,
                dest_tx TEXT NOT NULL,
                submitted_at INTEGER NOT NULL,
                confirmed_at INTEGER
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_mints_order_hash ON mints(order_id_hash);
            "#,
        )
        .map_err(|e| format!("Migration failed: {}", e))?;

        // Columns added to bridge_orders after the initial schema shipped.
        // CREATE TABLE IF NOT EXISTS does not add columns to existing
        // databases, so guard each with a pragma check.
        Self::ensure_column(&conn, "bridge_orders", "mint_authorization", "TEXT")?;
        Self::ensure_column(&conn, "bridge_orders", "dest_confirmed_at", "INTEGER")?;

        Ok(())
    }

    /// Add a column to `table` if it does not already exist.
    fn ensure_column(conn: &Connection, table: &str, column: &str, ty: &str) -> Result<(), String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .map_err(|e| format!("Pragma failed: {}", e))?;

        let existing: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Pragma query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Pragma collect failed: {}", e))?;

        if !existing.iter().any(|c| c == column) {
            conn.execute(
                &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, ty),
                [],
            )
            .map_err(|e| format!("Add column failed: {}", e))?;
        }

        Ok(())
    }

    /// Insert a new order.
    pub fn insert_order(&self, order: &BridgeOrder) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mint_auth_json = order
            .mint_authorization
            .as_ref()
            .map(|a| serde_json::to_string(a).map_err(|e| format!("Serialize failed: {}", e)))
            .transpose()?;

        conn.execute(
            r#"
            INSERT INTO bridge_orders (
                id, order_type, source_chain, dest_chain, amount, fee,
                source_tx, dest_tx, source_address, dest_address,
                status, error_message, memo, mint_authorization,
                dest_confirmed_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
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
                mint_auth_json,
                order.dest_confirmed_at.map(|t| t.timestamp()),
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
                       status, error_message, memo, mint_authorization,
                       dest_confirmed_at, created_at, updated_at
                FROM bridge_orders WHERE id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let result = stmt
            .query_row(params![id.to_string()], Self::row_to_order)
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
                       status, error_message, memo, mint_authorization,
                       dest_confirmed_at, created_at, updated_at
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

    // === Mint idempotency table ===

    /// Record a prepared mint transaction for an order, exactly once.
    ///
    /// Must be called BEFORE the transaction is first broadcast so that a
    /// crash between broadcast and persistence cannot lose the tx id.
    ///
    /// If a row already exists for this order (a resubmission after a crash
    /// or retry), the EXISTING record is returned unchanged and the caller
    /// must reuse its `dest_tx` instead of broadcasting a new transaction —
    /// this is the service-side exactly-once guard.
    pub fn record_mint_submitted(
        &self,
        order_id: &Uuid,
        order_id_hash: &str,
        chain: Chain,
        dest_tx: &str,
    ) -> Result<MintRecord, String> {
        {
            let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
            conn.execute(
                r#"
                INSERT OR IGNORE INTO mints (
                    order_id, order_id_hash, chain, dest_tx, submitted_at, confirmed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                "#,
                params![
                    order_id.to_string(),
                    order_id_hash,
                    chain.to_string(),
                    dest_tx,
                    Utc::now().timestamp(),
                ],
            )
            .map_err(|e| format!("Insert failed: {}", e))?;
        }

        self.get_mint_by_order(order_id)?
            .ok_or_else(|| "Mint record missing after insert".to_string())
    }

    /// Get the mint record for an order, if a mint was ever submitted.
    pub fn get_mint_by_order(&self, order_id: &Uuid) -> Result<Option<MintRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT order_id, order_id_hash, chain, dest_tx, submitted_at, confirmed_at
                FROM mints WHERE order_id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row(params![order_id.to_string()], Self::row_to_mint)
            .optional()
            .map_err(|e| format!("Query failed: {}", e))
    }

    /// Mark an order's mint as confirmed at the required depth.
    ///
    /// Atomically stamps `mints.confirmed_at` and moves the order to
    /// `Completed` with `dest_confirmed_at` set. The caller must have
    /// validated the `MintPending -> Completed` transition (confirmation
    /// gating) before calling.
    pub fn mark_mint_confirmed(&self, order_id: &Uuid) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let now = Utc::now().timestamp();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction failed: {}", e))?;

        tx.execute(
            "UPDATE mints SET confirmed_at = ?1 WHERE order_id = ?2 AND confirmed_at IS NULL",
            params![now, order_id.to_string()],
        )
        .map_err(|e| format!("Update mints failed: {}", e))?;

        tx.execute(
            r#"
            UPDATE bridge_orders
            SET status = 'completed', dest_confirmed_at = ?1, updated_at = ?1
            WHERE id = ?2 AND status = 'mint_pending'
            "#,
            params![now, order_id.to_string()],
        )
        .map_err(|e| format!("Update order failed: {}", e))?;

        tx.commit().map_err(|e| format!("Commit failed: {}", e))
    }

    /// Reorg unwind: roll a `MintPending` order back to `DepositConfirmed`.
    ///
    /// Atomically deletes the UNCONFIRMED mint record and clears the order's
    /// `dest_tx`, so `submit_mint` re-runs cleanly against the same on-chain
    /// order id. Confirmed mints are never rolled back.
    pub fn rollback_mint(&self, order_id: &Uuid) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let now = Utc::now().timestamp();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction failed: {}", e))?;

        tx.execute(
            "DELETE FROM mints WHERE order_id = ?1 AND confirmed_at IS NULL",
            params![order_id.to_string()],
        )
        .map_err(|e| format!("Delete mint failed: {}", e))?;

        tx.execute(
            r#"
            UPDATE bridge_orders
            SET status = 'deposit_confirmed', dest_tx = NULL,
                dest_confirmed_at = NULL, updated_at = ?1
            WHERE id = ?2 AND status = 'mint_pending'
            "#,
            params![now, order_id.to_string()],
        )
        .map_err(|e| format!("Update order failed: {}", e))?;

        tx.commit().map_err(|e| format!("Commit failed: {}", e))
    }

    /// Convert a database row to a MintRecord.
    fn row_to_mint(row: &rusqlite::Row<'_>) -> SqliteResult<MintRecord> {
        let order_id_str: String = row.get(0)?;
        let order_id_hash: String = row.get(1)?;
        let chain_str: String = row.get(2)?;
        let dest_tx: String = row.get(3)?;
        let submitted_at: i64 = row.get(4)?;
        let confirmed_at: Option<i64> = row.get(5)?;

        Ok(MintRecord {
            order_id: Uuid::parse_str(&order_id_str).unwrap_or_default(),
            order_id_hash,
            chain: chain_str.parse().unwrap_or(Chain::Ethereum),
            dest_tx,
            submitted_at: Utc.timestamp_opt(submitted_at, 0).unwrap(),
            confirmed_at: confirmed_at.and_then(|t| Utc.timestamp_opt(t, 0).single()),
        })
    }

    /// Log an audit event.
    pub fn log_audit(
        &self,
        order_id: Option<&Uuid>,
        action: &str,
        details: &str,
    ) -> Result<(), String> {
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
        let mint_auth_json: Option<String> = row.get(13)?;
        let dest_confirmed_at: Option<i64> = row.get(14)?;
        let created_at: i64 = row.get(15)?;
        let updated_at: i64 = row.get(16)?;

        let memo = memo_bytes.and_then(|b| {
            if b.len() == 64 {
                let mut arr = [0u8; 64];
                arr.copy_from_slice(&b);
                Some(arr)
            } else {
                None
            }
        });

        let mint_authorization: Option<MintAuthorization> =
            mint_auth_json.and_then(|j| serde_json::from_str(&j).ok());

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
            mint_authorization,
            dest_confirmed_at: dest_confirmed_at.and_then(|t| Utc.timestamp_opt(t, 0).single()),
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

    fn setup_mint_order(db: &Database) -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&order).unwrap();
        order
    }

    #[test]
    fn test_mint_idempotency_duplicate_order_id_exactly_once() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        // First submission records the tx.
        let first = db
            .record_mint_submitted(&order.id, &hash, Chain::Ethereum, "0xtx_one")
            .unwrap();
        assert_eq!(first.dest_tx, "0xtx_one");
        assert!(first.confirmed_at.is_none());

        // A duplicate submission with a DIFFERENT tx must return the
        // original record unchanged: exactly-once.
        let second = db
            .record_mint_submitted(&order.id, &hash, Chain::Ethereum, "0xtx_two")
            .unwrap();
        assert_eq!(
            second.dest_tx, "0xtx_one",
            "duplicate order_id must reuse the prior tx, never a new one"
        );
        assert_eq!(first.order_id, second.order_id);
    }

    #[test]
    fn test_mint_confirm_flow() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        db.record_mint_submitted(&order.id, &hash, Chain::Ethereum, "0xtx")
            .unwrap();
        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xtx"))
            .unwrap();

        db.mark_mint_confirmed(&order.id).unwrap();

        let mint = db.get_mint_by_order(&order.id).unwrap().unwrap();
        assert!(mint.confirmed_at.is_some());

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::Completed);
        assert!(stored.dest_confirmed_at.is_some());
    }

    #[test]
    fn test_mint_rollback_clears_unconfirmed() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        db.record_mint_submitted(&order.id, &hash, Chain::Ethereum, "0xreorged")
            .unwrap();
        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xreorged"))
            .unwrap();

        // Reorg unwind: back to DepositConfirmed, mint record gone.
        db.rollback_mint(&order.id).unwrap();

        assert!(db.get_mint_by_order(&order.id).unwrap().is_none());
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositConfirmed);
        assert!(stored.dest_tx.is_none());

        // Re-submission after rollback records the NEW tx (same order id).
        let resub = db
            .record_mint_submitted(&order.id, &hash, Chain::Ethereum, "0xretry")
            .unwrap();
        assert_eq!(resub.dest_tx, "0xretry");
    }

    #[test]
    fn test_mint_rollback_never_touches_confirmed() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        db.record_mint_submitted(&order.id, &hash, Chain::Ethereum, "0xtx")
            .unwrap();
        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xtx"))
            .unwrap();
        db.mark_mint_confirmed(&order.id).unwrap();

        // A late rollback attempt must be a no-op on a confirmed mint.
        db.rollback_mint(&order.id).unwrap();

        let mint = db.get_mint_by_order(&order.id).unwrap();
        assert!(mint.is_some(), "confirmed mint record must survive");
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::Completed);
    }

    #[test]
    fn test_order_new_fields_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.mint_authorization = Some(bth_bridge_core::MintAuthorization {
            order_id: order.order_id_bytes(),
            scheme: bth_bridge_core::SignatureScheme::Secp256k1,
            threshold: 2,
            signatures: vec![],
        });
        db.insert_order(&order).unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.mint_authorization, order.mint_authorization);
        assert!(stored.dest_confirmed_at.is_none());
    }
}
