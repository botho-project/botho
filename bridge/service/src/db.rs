// Copyright (c) 2024 The Botho Foundation

//! SQLite database for bridge order tracking.

use bth_bridge_core::{BridgeOrder, Chain, MintAuthorization, OrderStatus, OrderType};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqliteResult, TransactionBehavior};
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

/// A row in the `release_claims` exactly-once table.
///
/// One row exists per burn order that has ever entered the release path.
/// The claim is taken (row inserted with a NULL tx hash) BEFORE any signing
/// or submission; the signed transaction's hash and raw bytes are recorded
/// BEFORE the first broadcast. The `order_id` PRIMARY KEY is the
/// service-side exactly-once guard: a concurrent tick or a post-restart
/// re-entry finds the existing claim and either resumes with the recorded
/// transaction (never re-signing with new inputs — a reserve double-spend
/// risk) or, if nothing was recorded, knows nothing was ever broadcast and
/// may safely sign fresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseClaim {
    /// Bridge order UUID (string form).
    pub order_id: Uuid,
    /// Hex encoding of the 32-byte deterministic order id bound to the
    /// release attestation.
    pub order_id_hash: String,
    /// BTH transaction hash of the signed release, `None` until a
    /// transaction has been signed and durably recorded.
    pub release_tx_hash: Option<String>,
    /// The exact signed transaction bytes, persisted with the hash so a
    /// resume after restart re-broadcasts the SAME transaction.
    pub release_tx_raw: Option<Vec<u8>>,
    /// When the claim was taken (always before signing).
    pub claimed_at: DateTime<Utc>,
    /// When the signed transaction was recorded (always before first
    /// broadcast).
    pub submitted_at: Option<DateTime<Utc>>,
    /// When the transaction reached the required confirmation depth
    /// (SCP finality by default).
    pub confirmed_at: Option<DateTime<Utc>>,
}

/// Persisted scan progress for a chain watcher.
///
/// One row per source chain. `last_height` is the last FULLY processed
/// block (all its events handled and idempotency rows written), so a
/// restart resumes at `last_height + 1` without missing events.
/// `last_block_hash` is the canonical hash observed for that height —
/// watchers on reorg-capable chains compare it against the node's current
/// canonical hash to detect a reorg at/behind the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherCursor {
    /// Source chain this cursor tracks.
    pub chain: Chain,
    /// Last fully processed block height (slot on Solana).
    pub last_height: u64,
    /// Canonical block hash observed at `last_height` (None where the
    /// chain has no reorgs to guard against, e.g. SCP-final BTH).
    pub last_block_hash: Option<String>,
}

/// A row in the `processed_burns` idempotency table.
///
/// One row exists per source-chain burn event that has ever created a
/// burn order. The `source_key` UNIQUE constraint makes event→order
/// creation exactly-once: a rescan (cursor rewind) or a reorg re-add of
/// the same burn finds the existing row and reuses its order instead of
/// creating a duplicate release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnRecord {
    /// Stable identity of the burn event: `"<source_tx>#<ordinal>"` where
    /// ordinal is the event's position among burn events of the SAME
    /// transaction (stable across reorgs, unlike the absolute log index).
    pub source_key: String,
    /// Bridge order created for this burn.
    pub order_id: Uuid,
    /// Source chain of the burn.
    pub chain: Chain,
    /// Block height (slot) the burn was last observed in.
    pub block_number: u64,
    /// Canonical block hash the burn was last observed in.
    pub block_hash: Option<String>,
    /// True while the burn's block has been observed reorged out and the
    /// event has not yet been re-observed in a canonical block. An
    /// orphaned burn must never advance toward release.
    pub orphaned: bool,
}

/// A registered release-tracking intent (#1036), a row in `release_intents`.
///
/// Purely a UX correlation aid for the wallet Unwrap flow: the burn happens
/// in the user's OWN counterparty wallet (`bridgeBurn(amount, bthAddress)`),
/// the watcher detects it and creates the real `BridgeOrder` (keyed by the
/// on-chain `bthAddress` + amount), and the release is submitted regardless.
/// This record just lets the public API hand the wallet a UUID to poll and
/// then correlate that UUID back to the watcher-created burn order. It is
/// NEVER read by the release pipeline and carries no custody authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseIntent {
    /// Client-facing tracking UUID.
    pub id: Uuid,
    /// Chain the user will burn wBTH on.
    pub source_chain: Chain,
    /// Botho address the released BTH is destined for (the burn's embedded
    /// `bthAddress`; the correlation key to the watcher-created burn order).
    pub bth_address: String,
    /// Gross wBTH the user intends to burn, in picocredits.
    pub amount: u64,
    /// Bridge fee quoted at registration, in picocredits.
    pub fee: u64,
    /// wBTH token/mint address the user must burn on `source_chain`.
    pub token_address: String,
    /// When the intent was registered (unix seconds).
    pub created_at: i64,
    /// When the intent expires if no matching burn is ever seen (unix
    /// seconds).
    pub expires_at: i64,
}

/// Count + gross volume (picocredits) for one activity bucket (#1054).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityAggregate {
    /// Number of orders in the bucket.
    pub count: u64,
    /// Sum of gross order amounts, picocredits. `u128` so cumulative all-time
    /// volume cannot overflow at mainnet scale (#1059): a `u64` sum saturates
    /// at ~18.4M BTH and a SQLite `SUM(amount)` errors at ~9.22M BTH, both
    /// below plausible cumulative bridge volume. `u128` covers ~3.4e26 BTH.
    pub volume: u128,
}

/// Wrap/unwrap activity aggregates for one order type over one window
/// (#1054). Produced by [`Database::aggregate_order_activity`]; buckets are
/// disjoint and cover every order of the type, so the bucket sums equal the
/// window total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityBreakdown {
    /// Settled orders (`completed` mints / `released` burns).
    pub completed: ActivityAggregate,
    /// In-flight orders (every non-terminal status).
    pub pending: ActivityAggregate,
    /// Orders that timed out.
    pub expired: ActivityAggregate,
    /// Terminal failures.
    pub failed: ActivityAggregate,
}

/// A row in the `reserve_ledger` table (#825).
///
/// The locked BTH reserve is derived from bridge-controlled outputs, never
/// from a mutable counter: each confirmed deposit records a locked output
/// (amount = the wBTH backing it mints, i.e. the order's net amount), and
/// each confirmed release spends locked outputs FIFO — returning any
/// remainder to the ledger as a change output, exactly like the real
/// reserve wallet returns change to the reserve address. Per ADR 0003 the
/// reserve holds only factor-1 (zero-demurrage) coins, so
/// `SUM(amount) WHERE locked = 1` is authoritative with no accrual term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveOutput {
    /// Stable identity of the output (`"dep:<order>"` for deposit-backed
    /// outputs, `"chg:<order>"` for release change).
    pub bridge_output_id: String,
    /// Wrapped chain this output backs (ADR 0005 per-chain invariant).
    pub chain: Chain,
    /// Output amount in picocredits.
    pub amount: u64,
    /// Whether the output is still part of the locked reserve.
    pub locked: bool,
    /// Order that created the output.
    pub order_id: Uuid,
    /// Order whose release (or failure) spent/unlocked the output.
    pub spent_order_id: Option<Uuid>,
}

/// A persisted reconciliation result (#825): one row per reconciler pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveSnapshot {
    /// When the reconciliation ran (unix seconds).
    pub taken_at: i64,
    /// DB-derived locked reserve total in picocredits.
    pub locked_reserve: u64,
    /// On-chain wBTH totalSupply on Ethereum (picocredits), `None` when
    /// the supply could not be verified this pass.
    pub eth_supply: Option<u64>,
    /// On-chain wBTH supply on Solana (picocredits), `None` when the
    /// supply could not be verified this pass (transport pending #853).
    pub sol_supply: Option<u64>,
    /// Σ(verified wrapped supply) − Σ(locked backing of verified chains).
    pub drift: i64,
    /// Whether every verified chain was within tolerance + in-flight
    /// allowance.
    pub in_tolerance: bool,
    /// `in_tolerance` AND the on-Botho reserve balance covered the ledger
    /// (when checkable). The dashboard's red/green peg state.
    pub peg_healthy: bool,
    /// Whether the on-Botho reserve balance custody leg was actually
    /// checked this pass (#846: consumers must be able to distinguish
    /// "custody checked OK" from "custody never checked").
    pub reserve_balance_checked: bool,
}

/// Outcome of an atomic rate-limit reservation
/// ([`Database::check_and_reserve_limits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitCheck {
    /// The order's volume was reserved against the daily windows.
    Reserved,
    /// A reservation for this order already exists (crash/tick replay) —
    /// the volume was counted exactly once.
    AlreadyReserved,
    /// The order violates a limit and must not proceed this window.
    Rejected(LimitViolation),
}

/// Which limit a rejected order violated.
// The shared `Cap` postfix is the point: each variant names WHICH cap.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitViolation {
    /// `amount > max_order_amount`: can never pass — permanent for this
    /// order.
    PerOrderCap,
    /// The counterparty address's daily volume window is exhausted;
    /// retryable next window.
    AddressDailyCap,
    /// The bridge-wide daily volume window is exhausted; retryable next
    /// window (and an anomaly signal — callers trip the circuit breaker).
    GlobalDailyCap,
}

impl std::fmt::Display for LimitViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LimitViolation::PerOrderCap => write!(f, "per-order amount cap"),
            LimitViolation::AddressDailyCap => write!(f, "per-address daily volume cap"),
            LimitViolation::GlobalDailyCap => write!(f, "global daily volume cap"),
        }
    }
}

/// Database wrapper for thread-safe access.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

#[allow(dead_code)]
impl Database {
    /// Open or create the database.
    ///
    /// The connection is opened in WAL mode with a busy timeout so that
    /// SEVERAL bridge-service processes can share one database file — the
    /// single-host federation topology the #868 testnet drill runs (N
    /// instances, each with its own attestation key, coordinating through
    /// the shared order store while exchanging envelopes over the wire).
    /// The exactly-once submission guards (`record_mint_submitted` /
    /// `record_release_tx`) already arbitrate multi-writer races at the
    /// row level; WAL + a busy timeout make the underlying file access
    /// safe across processes instead of failing fast with SQLITE_BUSY.
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open database: {}", e))?;

        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| format!("Failed to set busy timeout: {}", e))?;
        // `PRAGMA journal_mode=WAL` returns the resulting mode as a row.
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(|e| format!("Failed to enable WAL mode: {}", e))?;
        if !mode.eq_ignore_ascii_case("wal") {
            // Non-fatal: some filesystems (e.g. network mounts) refuse WAL.
            // Single-process deployments work fine in rollback mode.
            tracing::warn!(
                "database journal_mode is {mode}, not WAL; multi-process sharing unsafe"
            );
        }

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

            CREATE TABLE IF NOT EXISTS release_claims (
                order_id TEXT PRIMARY KEY REFERENCES bridge_orders(id),
                order_id_hash TEXT NOT NULL,
                release_tx_hash TEXT,
                release_tx_raw BLOB,
                claimed_at INTEGER NOT NULL,
                submitted_at INTEGER,
                confirmed_at INTEGER
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_release_claims_order_hash
                ON release_claims(order_id_hash);

            CREATE TABLE IF NOT EXISTS watcher_cursors (
                chain TEXT PRIMARY KEY,
                last_height INTEGER NOT NULL,
                last_block_hash TEXT,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS processed_burns (
                source_key TEXT PRIMARY KEY,
                order_id TEXT NOT NULL REFERENCES bridge_orders(id),
                chain TEXT NOT NULL,
                block_number INTEGER NOT NULL,
                block_hash TEXT,
                orphaned INTEGER NOT NULL DEFAULT 0,
                processed_at INTEGER NOT NULL
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_burns_order ON processed_burns(order_id);

            CREATE TABLE IF NOT EXISTS reserve_ledger (
                bridge_output_id TEXT PRIMARY KEY,
                chain TEXT NOT NULL,
                amount_picocredits INTEGER NOT NULL,
                locked INTEGER NOT NULL DEFAULT 1,
                order_id TEXT NOT NULL,
                spent_order_id TEXT,
                created_at INTEGER NOT NULL,
                spent_at INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_reserve_chain_locked
                ON reserve_ledger(chain, locked);
            CREATE INDEX IF NOT EXISTS idx_reserve_spent_order
                ON reserve_ledger(spent_order_id);

            CREATE TABLE IF NOT EXISTS reserve_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                taken_at INTEGER NOT NULL,
                locked_reserve INTEGER NOT NULL,
                eth_supply INTEGER,
                sol_supply INTEGER,
                drift INTEGER NOT NULL,
                in_tolerance INTEGER NOT NULL,
                peg_healthy INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_reserve_snapshots_taken
                ON reserve_snapshots(taken_at);

            CREATE TABLE IF NOT EXISTS bridge_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                paused INTEGER NOT NULL DEFAULT 0,
                paused_reason TEXT,
                paused_at INTEGER
            );

            INSERT OR IGNORE INTO bridge_state (id, paused) VALUES (1, 0);

            CREATE TABLE IF NOT EXISTS limit_reservations (
                order_id TEXT PRIMARY KEY REFERENCES bridge_orders(id),
                address TEXT NOT NULL,
                amount INTEGER NOT NULL,
                day_bucket INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_limit_res_addr
                ON limit_reservations(address, day_bucket);
            CREATE INDEX IF NOT EXISTS idx_limit_res_day
                ON limit_reservations(day_bucket);

            CREATE TABLE IF NOT EXISTS component_health (
                component TEXT PRIMARY KEY,
                healthy INTEGER NOT NULL,
                detail TEXT,
                updated_at INTEGER NOT NULL
            );

            -- Release-order tracking intents (#1036). Purely a UX correlation
            -- aid for the wallet Unwrap flow: the user registers the Botho
            -- release address + amount they intend to burn wBTH for, so the
            -- public API can later correlate the watcher-created burn order
            -- (keyed by the on-chain bthAddress + amount) and report its
            -- status. This table is NON-CUSTODIAL and is NEVER read by the
            -- release pipeline — burns are self-describing and released
            -- regardless of whether an intent was ever registered.
            CREATE TABLE IF NOT EXISTS release_intents (
                id TEXT PRIMARY KEY,
                source_chain TEXT NOT NULL,
                bth_address TEXT NOT NULL,
                amount INTEGER NOT NULL,
                fee INTEGER NOT NULL,
                token_address TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_release_intents_addr
                ON release_intents(bth_address, amount);
            "#,
        )
        .map_err(|e| format!("Migration failed: {}", e))?;

        // Columns added to bridge_orders after the initial schema shipped.
        // CREATE TABLE IF NOT EXISTS does not add columns to existing
        // databases, so guard each with a pragma check.
        Self::ensure_column(&conn, "bridge_orders", "mint_authorization", "TEXT")?;
        Self::ensure_column(&conn, "bridge_orders", "dest_confirmed_at", "INTEGER")?;
        // #846: persist whether the custody leg was checked, so the proof
        // API can distinguish "checked OK" from "never checked".
        Self::ensure_column(
            &conn,
            "reserve_snapshots",
            "reserve_balance_checked",
            "INTEGER",
        )?;

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
        Self::insert_order_stmt(&conn, order)
    }

    /// Execute the order INSERT against `conn` (also used inside
    /// transactions, e.g. [`Database::insert_burn_order`]).
    fn insert_order_stmt(conn: &Connection, order: &BridgeOrder) -> Result<(), String> {
        Self::insert_order_stmt_impl(conn, order, false)
    }

    /// Like [`Database::insert_order_stmt`] but tolerates a pre-existing id
    /// (`INSERT OR IGNORE`). Used by the burn path, whose ids are deterministic
    /// over the source tuple (#1050) and therefore idempotent across replays
    /// and concurrent watchers observing the same burn.
    fn insert_order_stmt_or_ignore(conn: &Connection, order: &BridgeOrder) -> Result<(), String> {
        Self::insert_order_stmt_impl(conn, order, true)
    }

    fn insert_order_stmt_impl(
        conn: &Connection,
        order: &BridgeOrder,
        or_ignore: bool,
    ) -> Result<(), String> {
        let mint_auth_json = order
            .mint_authorization
            .as_ref()
            .map(|a| serde_json::to_string(a).map_err(|e| format!("Serialize failed: {}", e)))
            .transpose()?;

        let verb = if or_ignore {
            "INSERT OR IGNORE"
        } else {
            "INSERT"
        };
        conn.execute(
            &format!(
                r#"
            {verb} INTO bridge_orders (
                id, order_type, source_chain, dest_chain, amount, fee,
                source_tx, dest_tx, source_address, dest_address,
                status, error_message, memo, mint_authorization,
                dest_confirmed_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#
            ),
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

    /// Register a release-tracking intent (#1036).
    ///
    /// Non-custodial: this only records the wallet's stated intent so the
    /// public API can hand back a pollable UUID and later correlate it to the
    /// watcher-created burn order. See [`ReleaseIntent`].
    pub fn insert_release_intent(&self, intent: &ReleaseIntent) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            r#"
            INSERT INTO release_intents (
                id, source_chain, bth_address, amount, fee, token_address,
                created_at, expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                intent.id.to_string(),
                intent.source_chain.to_string(),
                intent.bth_address,
                intent.amount as i64,
                intent.fee as i64,
                intent.token_address,
                intent.created_at,
                intent.expires_at,
            ],
        )
        .map_err(|e| format!("Insert release intent failed: {}", e))?;
        Ok(())
    }

    /// Fetch a registered release intent by its tracking UUID (#1036).
    pub fn get_release_intent(&self, id: &Uuid) -> Result<Option<ReleaseIntent>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, source_chain, bth_address, amount, fee, token_address,
                       created_at, expires_at
                FROM release_intents WHERE id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;
        let result = stmt
            .query_row(params![id.to_string()], |row| {
                let id_str: String = row.get(0)?;
                let chain_str: String = row.get(1)?;
                Ok(ReleaseIntent {
                    id: Uuid::parse_str(&id_str).unwrap_or_default(),
                    source_chain: chain_str.parse().unwrap_or(Chain::Bth),
                    bth_address: row.get(2)?,
                    amount: row.get::<_, i64>(3)? as u64,
                    fee: row.get::<_, i64>(4)? as u64,
                    token_address: row.get(5)?,
                    created_at: row.get(6)?,
                    expires_at: row.get(7)?,
                })
            })
            .optional()
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(result)
    }

    /// Number of mint orders still awaiting their BTH deposit.
    ///
    /// Backs the public API's GLOBAL order-create ceiling (#1042): the
    /// per-IP rate limiter bounds per-client rate, but only this bounds the
    /// total outstanding created orders under distributed spam.
    pub fn count_awaiting_deposit_mint_orders(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let count: i64 = conn
            .query_row(
                r#"
                SELECT COUNT(*) FROM bridge_orders
                WHERE order_type = 'mint' AND status = 'awaiting_deposit'
                "#,
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Count failed: {}", e))?;
        Ok(count.max(0) as u64)
    }

    /// Number of release-tracking intents that have not yet expired.
    ///
    /// Backs the public API's GLOBAL order-create ceiling (#1042) for the
    /// release-intent surface, mirroring
    /// [`Database::count_awaiting_deposit_mint_orders`].
    pub fn count_active_release_intents(&self, now: i64) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM release_intents WHERE expires_at > ?1",
                params![now],
                |row| row.get(0),
            )
            .map_err(|e| format!("Count failed: {}", e))?;
        Ok(count.max(0) as u64)
    }

    /// Aggregate wrap/unwrap activity for one order type (#1054).
    ///
    /// Backs the public `GET /api/bridge/stats` endpoint: counts and gross
    /// BTH volumes (picocredits) over `bridge_orders` rows of `order_type`,
    /// bucketed by outcome:
    ///
    /// * `completed` — settled orders (`completed` for mints, `released` for
    ///   burns; both strings map here so either type aggregates correctly).
    /// * `expired`   — orders that timed out (`expired`).
    /// * `failed`    — terminal failures (stored as `failed: <reason>`).
    /// * `pending`   — every other (in-flight) status.
    ///
    /// `since` bounds the window: only rows with `created_at >= since` count
    /// (INCLUSIVE edge — an order created exactly at the cutoff is in the
    /// window). Pass `None` for all-time.
    ///
    /// AGGREGATES ONLY: no per-order field leaves this query, so the result
    /// is safe for the unauthenticated public surface (#1042 scope rules).
    ///
    /// Volume is summed in Rust as `u128` rather than via SQLite integer
    /// `SUM(amount)` (#1059): SQLite's integer SUM hard-errors on i64 overflow
    /// (~9.22M BTH/bucket) which would 500 the endpoint permanently, and a
    /// `u64` accumulator would silently saturate at ~18.4M BTH. Streaming the
    /// raw `status, amount` per row and accumulating in u128 is exact (no
    /// float loss) and cannot overflow at any realistic supply.
    pub fn aggregate_order_activity(
        &self,
        order_type: OrderType,
        since: Option<i64>,
    ) -> Result<ActivityBreakdown, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        // Stream raw per-row (status, amount) instead of a SQLite
        // `SUM(amount)`: integer SUM errors on i64 overflow (#1059). Volume is
        // accumulated in Rust as u128 so it cannot overflow at mainnet scale.
        let mut stmt = conn
            .prepare(
                r#"
                SELECT status, amount
                FROM bridge_orders
                WHERE order_type = ?1 AND created_at >= ?2
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let rows = stmt
            .query_map(
                params![order_type.to_string(), since.unwrap_or(i64::MIN)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut breakdown = ActivityBreakdown::default();
        for row in rows {
            let (status, amount) = row.map_err(|e| format!("Row failed: {}", e))?;
            let bucket = match status.as_str() {
                "completed" | "released" => &mut breakdown.completed,
                "expired" => &mut breakdown.expired,
                // Stored as `failed: <reason>` (OrderStatus::Display).
                s if s.starts_with("failed") => &mut breakdown.failed,
                _ => &mut breakdown.pending,
            };
            bucket.count = bucket.count.saturating_add(1);
            // Clamp defensively (`.max(0)`); `saturating_add` never triggers at
            // u128 width but is kept as defense-in-depth.
            bucket.volume = bucket.volume.saturating_add(amount.max(0) as u128);
        }
        Ok(breakdown)
    }

    /// Delete EXPIRED mint orders that never saw a deposit, once they are
    /// older than `retain_secs` (#1042).
    ///
    /// Only rows where `status = 'expired'`, `order_type = 'mint'` and
    /// `source_tx IS NULL` qualify — i.e. abandoned order-create residue
    /// where no funds ever moved. Orders that saw a deposit (or any burn /
    /// settled order) are NEVER pruned; they are the bridge's accounting
    /// record. Audit-log rows are kept (append-only history). Returns the
    /// number of pruned rows.
    pub fn prune_expired_mint_orders(&self, retain_secs: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let cutoff = Utc::now().timestamp() - retain_secs;
        conn.execute(
            r#"
            DELETE FROM bridge_orders
            WHERE status = 'expired'
              AND order_type = 'mint'
              AND source_tx IS NULL
              AND updated_at < ?1
            "#,
            params![cutoff],
        )
        .map_err(|e| format!("Prune failed: {}", e))
    }

    /// Delete release-tracking intents that expired more than `retain_secs`
    /// ago (#1042). Intents are non-custodial UX correlation records (never
    /// read by the release pipeline), so pruning them can never affect a
    /// release. Returns the number of pruned rows.
    pub fn prune_expired_release_intents(&self, retain_secs: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let cutoff = Utc::now().timestamp() - retain_secs;
        conn.execute(
            "DELETE FROM release_intents WHERE expires_at < ?1",
            params![cutoff],
        )
        .map_err(|e| format!("Prune failed: {}", e))
    }

    /// Correlate a release intent to the watcher-created burn order (#1036).
    ///
    /// Burn orders are created by the counterparty-chain watchers keyed by the
    /// on-chain `bthAddress` (stored as the order's `dest_address`) and the
    /// revealed burn amount. Returns the most recent matching burn order, or
    /// `None` if no burn has been detected yet. Correlation is best-effort
    /// tracking only — two identical (address, amount) burns are
    /// indistinguishable here, which is acceptable for a status hint (the
    /// release itself is driven by the self-describing burn, not this lookup).
    pub fn find_burn_order_for_release(
        &self,
        bth_address: &str,
        amount: u64,
    ) -> Result<Option<BridgeOrder>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, order_type, source_chain, dest_chain, amount, fee,
                       source_tx, dest_tx, source_address, dest_address,
                       status, error_message, memo, mint_authorization,
                       dest_confirmed_at, created_at, updated_at
                FROM bridge_orders
                WHERE order_type = 'burn' AND dest_address = ?1 AND amount = ?2
                ORDER BY created_at DESC
                LIMIT 1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;
        let result = stmt
            .query_row(params![bth_address, amount as i64], Self::row_to_order)
            .optional()
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(result)
    }

    /// Update order status, enforcing the state machine at the DB layer
    /// (#839).
    ///
    /// The current status is read and validated inside the same
    /// transaction: the write is rejected unless
    /// [`OrderStatus::can_transition_to`] allows the edge. A same-status
    /// write is treated as an idempotent refresh (replayed ticks may
    /// re-assert a status, optionally updating the tx hash), never as a
    /// transition. No writer can bypass the state machine or clobber a
    /// terminal state through this path.
    pub fn update_order_status(
        &self,
        id: &Uuid,
        status: &OrderStatus,
        tx_hash: Option<&str>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("Transaction failed: {}", e))?;

        let (current_str, current_err): (String, Option<String>) = tx
            .query_row(
                "SELECT status, error_message FROM bridge_orders WHERE id = ?1",
                params![id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("Order {} not found for status update: {}", id, e))?;
        let current = parse_status(&current_str, current_err);

        let same_status = std::mem::discriminant(&current) == std::mem::discriminant(status);
        if !same_status && !current.can_transition_to(status) {
            return Err(format!(
                "illegal order status transition for {}: {} -> {}",
                id, current, status
            ));
        }

        let now = Utc::now().timestamp();
        let status_str = status.to_string();
        let error_msg = match status {
            OrderStatus::Failed { reason } => Some(reason.clone()),
            _ => None,
        };

        // Defense-in-depth: a same-status write on a terminal order
        // (`Completed -> Completed`, etc.) must not clobber the recorded
        // `dest_tx`. `same_status` already passed the transition guard above,
        // so without this a second call with a different `tx_hash` would
        // overwrite the settled destination tx. Only refresh the tx hash while
        // the order is still non-terminal.
        if let (Some(hash), false) = (tx_hash, current.is_terminal()) {
            // Update with transaction hash
            tx.execute(
                r#"
                UPDATE bridge_orders
                SET status = ?1, dest_tx = ?2, error_message = ?3, updated_at = ?4
                WHERE id = ?5
                "#,
                params![status_str, hash, error_msg, now, id.to_string()],
            )
            .map_err(|e| format!("Update failed: {}", e))?;
        } else {
            tx.execute(
                r#"
                UPDATE bridge_orders
                SET status = ?1, error_message = ?2, updated_at = ?3
                WHERE id = ?4
                "#,
                params![status_str, error_msg, now, id.to_string()],
            )
            .map_err(|e| format!("Update failed: {}", e))?;
        }

        tx.commit().map_err(|e| format!("Commit failed: {}", e))
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

    /// Record a detected deposit on an `AwaitingDeposit` order: stamps the
    /// REVEALED amount (ADR 0004) and the deposit tx as `source_tx`, and
    /// advances the order to `DepositDetected`.
    ///
    /// The SQL `status = 'awaiting_deposit'` guard makes this both a state
    /// transition check and an idempotency layer: replaying the same
    /// deposit (cursor rewind) is a no-op. Returns whether the order was
    /// updated.
    pub fn record_deposit_detected(
        &self,
        order_id: &Uuid,
        source_tx: &str,
        amount: u64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let changed = conn
            .execute(
                r#"
                UPDATE bridge_orders
                SET status = 'deposit_detected', source_tx = ?1, amount = ?2, updated_at = ?3
                WHERE id = ?4 AND status = 'awaiting_deposit'
                "#,
                params![
                    source_tx,
                    amount as i64,
                    Utc::now().timestamp(),
                    order_id.to_string()
                ],
            )
            .map_err(|e| format!("Update failed: {}", e))?;

        Ok(changed > 0)
    }

    // === Watcher cursors (restart/resume durability) ===

    /// Load the persisted scan cursor for a chain, if any.
    pub fn get_cursor(&self, chain: Chain) -> Result<Option<WatcherCursor>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare("SELECT last_height, last_block_hash FROM watcher_cursors WHERE chain = ?1")
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row(params![chain.to_string()], |row| {
            let last_height: i64 = row.get(0)?;
            let last_block_hash: Option<String> = row.get(1)?;
            Ok(WatcherCursor {
                chain,
                last_height: last_height as u64,
                last_block_hash,
            })
        })
        .optional()
        .map_err(|e| format!("Query failed: {}", e))
    }

    /// Persist the scan cursor for a chain. Watchers call this only AFTER
    /// a block is fully processed, so a crash replays (never skips) the
    /// in-flight block; the idempotency tables deduplicate the replay.
    pub fn set_cursor(
        &self,
        chain: Chain,
        last_height: u64,
        last_block_hash: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO watcher_cursors (chain, last_height, last_block_hash, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                chain.to_string(),
                last_height as i64,
                last_block_hash,
                Utc::now().timestamp()
            ],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(())
    }

    // === Burn idempotency table ===

    /// Atomically create a burn order together with its `processed_burns`
    /// idempotency row.
    ///
    /// Exactly-once by `source_key`: if the burn was already recorded (a
    /// cursor replay or reorg re-add), NOTHING is written — the whole
    /// transaction rolls back and `Ok(false)` is returned so the caller
    /// reuses the existing order. The single transaction closes the
    /// crash window between "order inserted" and "idempotency row written".
    pub fn insert_burn_order(
        &self,
        order: &BridgeOrder,
        source_key: &str,
        block_number: u64,
        block_hash: Option<&str>,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction failed: {}", e))?;

        // Insert the order row idempotently. Burn order ids are deterministic
        // over the source tuple (#1050 Phase 1) — the same finalized burn
        // always derives the same id — so a replay or a concurrent watcher
        // observing the same burn presents the SAME id. `INSERT OR IGNORE`
        // makes that a no-op instead of hard-erroring on `bridge_orders.id`;
        // the `processed_burns` insert below (gated on the stable, unique
        // `source_key`) remains the exactly-once arbiter that decides the
        // return value. The FK from `processed_burns.order_id` requires the
        // order row to exist first, so this ordering is preserved.
        Self::insert_order_stmt_or_ignore(&tx, order)?;

        let changed = tx
            .execute(
                r#"
                INSERT OR IGNORE INTO processed_burns (
                    source_key, order_id, chain, block_number, block_hash, orphaned, processed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
                "#,
                params![
                    source_key,
                    order.id.to_string(),
                    order.source_chain.to_string(),
                    block_number as i64,
                    block_hash,
                    Utc::now().timestamp()
                ],
            )
            .map_err(|e| format!("Insert failed: {}", e))?;

        if changed == 0 {
            // Duplicate source_key: drop the transaction (rolls back the order
            // insert, if any) — the existing record wins.
            return Ok(false);
        }

        tx.commit().map_err(|e| format!("Commit failed: {}", e))?;
        Ok(true)
    }

    /// Look up a burn record by its stable source key.
    pub fn get_burn_by_source(&self, source_key: &str) -> Result<Option<BurnRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT source_key, order_id, chain, block_number, block_hash, orphaned
                FROM processed_burns WHERE source_key = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row(params![source_key], Self::row_to_burn)
            .optional()
            .map_err(|e| format!("Query failed: {}", e))
    }

    /// Look up the burn record backing an order.
    pub fn get_burn_by_order(&self, order_id: &Uuid) -> Result<Option<BurnRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT source_key, order_id, chain, block_number, block_hash, orphaned
                FROM processed_burns WHERE order_id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row(params![order_id.to_string()], Self::row_to_burn)
            .optional()
            .map_err(|e| format!("Query failed: {}", e))
    }

    /// Update where a burn was (re-)observed on the source chain and clear
    /// its orphaned flag — used when a reorged-out burn is re-included in a
    /// new canonical block (idempotent by order id: same record, same
    /// order, new location).
    pub fn update_burn_location(
        &self,
        source_key: &str,
        block_number: u64,
        block_hash: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            UPDATE processed_burns
            SET block_number = ?1, block_hash = ?2, orphaned = 0
            WHERE source_key = ?3
            "#,
            params![block_number as i64, block_hash, source_key],
        )
        .map_err(|e| format!("Update failed: {}", e))?;

        Ok(())
    }

    /// Flag a burn whose block was reorged out. Returns true if the flag
    /// was newly set (so callers can audit-log the orphaning exactly once).
    pub fn mark_burn_orphaned(&self, source_key: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let changed = conn
            .execute(
                "UPDATE processed_burns SET orphaned = 1 WHERE source_key = ?1 AND orphaned = 0",
                params![source_key],
            )
            .map_err(|e| format!("Update failed: {}", e))?;

        Ok(changed > 0)
    }

    /// Count audit-log entries with the given action (test/ops helper).
    pub fn count_audit_action(&self, action: &str) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.query_row(
            "SELECT COUNT(*) FROM audit_log WHERE action = ?1",
            params![action],
            |row| row.get(0),
        )
        .map_err(|e| format!("Query failed: {}", e))
    }

    /// Convert a database row to a BurnRecord.
    fn row_to_burn(row: &rusqlite::Row<'_>) -> SqliteResult<BurnRecord> {
        let source_key: String = row.get(0)?;
        let order_id_str: String = row.get(1)?;
        let chain_str: String = row.get(2)?;
        let block_number: i64 = row.get(3)?;
        let block_hash: Option<String> = row.get(4)?;
        let orphaned: i64 = row.get(5)?;

        Ok(BurnRecord {
            source_key,
            order_id: Uuid::parse_str(&order_id_str).unwrap_or_default(),
            chain: chain_str.parse().unwrap_or(Chain::Ethereum),
            block_number: block_number as u64,
            block_hash,
            orphaned: orphaned != 0,
        })
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

    // === Release exactly-once claims ===

    /// Take (or find) the durable release claim for a burn order.
    ///
    /// Runs as a `BEGIN IMMEDIATE` transaction so the claim is durably
    /// serialized against any concurrent writer BEFORE the caller does any
    /// signing or submission. Returns the claim row:
    ///
    /// - `release_tx_hash == None`: this caller holds a fresh (or not yet
    ///   signed) claim — nothing was ever recorded, so nothing was ever
    ///   broadcast; it is safe to build and sign a transaction.
    /// - `release_tx_hash == Some(tx)`: a transaction was already signed and
    ///   recorded (possibly before a crash). The caller MUST reuse it and never
    ///   sign a second transaction with different inputs.
    pub fn try_claim_release(
        &self,
        order_id: &Uuid,
        order_id_hash: &str,
    ) -> Result<ReleaseClaim, String> {
        {
            let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| format!("Transaction failed: {}", e))?;

            tx.execute(
                r#"
                INSERT OR IGNORE INTO release_claims (
                    order_id, order_id_hash, release_tx_hash, release_tx_raw,
                    claimed_at, submitted_at, confirmed_at
                ) VALUES (?1, ?2, NULL, NULL, ?3, NULL, NULL)
                "#,
                params![order_id.to_string(), order_id_hash, Utc::now().timestamp(),],
            )
            .map_err(|e| format!("Insert failed: {}", e))?;

            tx.commit().map_err(|e| format!("Commit failed: {}", e))?;
        }

        self.get_release_by_order(order_id)?
            .ok_or_else(|| "Release claim missing after insert".to_string())
    }

    /// Record the signed release transaction for a claimed order, exactly
    /// once.
    ///
    /// Must be called BEFORE the transaction is first broadcast so a crash
    /// between broadcast and persistence cannot lose the tx. Both the hash
    /// and the raw signed bytes are stored so a post-restart resume
    /// re-broadcasts the SAME transaction instead of re-signing with new
    /// inputs (which could double-spend the reserve).
    ///
    /// If a transaction was already recorded (a lost race), the EXISTING
    /// record is returned unchanged and the caller must discard its own
    /// transaction without broadcasting it.
    pub fn record_release_tx(
        &self,
        order_id: &Uuid,
        tx_hash: &str,
        tx_raw: &[u8],
    ) -> Result<ReleaseClaim, String> {
        {
            let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
            conn.execute(
                r#"
                UPDATE release_claims
                SET release_tx_hash = ?1, release_tx_raw = ?2, submitted_at = ?3
                WHERE order_id = ?4 AND release_tx_hash IS NULL
                "#,
                params![
                    tx_hash,
                    tx_raw,
                    Utc::now().timestamp(),
                    order_id.to_string()
                ],
            )
            .map_err(|e| format!("Update failed: {}", e))?;
        }

        self.get_release_by_order(order_id)?
            .ok_or_else(|| "Release claim missing while recording tx".to_string())
    }

    /// Get the release claim for an order, if the release path was ever
    /// entered.
    pub fn get_release_by_order(&self, order_id: &Uuid) -> Result<Option<ReleaseClaim>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT order_id, order_id_hash, release_tx_hash, release_tx_raw,
                       claimed_at, submitted_at, confirmed_at
                FROM release_claims WHERE order_id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row(params![order_id.to_string()], Self::row_to_release)
            .optional()
            .map_err(|e| format!("Query failed: {}", e))
    }

    /// Mark an order's release as confirmed at the required depth.
    ///
    /// Atomically stamps `release_claims.confirmed_at` and moves the order
    /// to `Released` with `dest_confirmed_at` set. The caller must have
    /// validated the `ReleasePending -> Released` transition (finality
    /// gating) before calling.
    pub fn mark_release_confirmed(&self, order_id: &Uuid) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let now = Utc::now().timestamp();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction failed: {}", e))?;

        tx.execute(
            r#"
            UPDATE release_claims SET confirmed_at = ?1
            WHERE order_id = ?2 AND confirmed_at IS NULL
            "#,
            params![now, order_id.to_string()],
        )
        .map_err(|e| format!("Update release_claims failed: {}", e))?;

        tx.execute(
            r#"
            UPDATE bridge_orders
            SET status = 'released', dest_confirmed_at = ?1, updated_at = ?1
            WHERE id = ?2 AND status = 'release_pending'
            "#,
            params![now, order_id.to_string()],
        )
        .map_err(|e| format!("Update order failed: {}", e))?;

        tx.commit().map_err(|e| format!("Commit failed: {}", e))
    }

    /// Release unwind: roll a `ReleasePending` order back to
    /// `BurnConfirmed`.
    ///
    /// Atomically deletes the UNCONFIRMED claim and clears the order's
    /// `dest_tx`, so `submit_release` re-runs cleanly. Confirmed releases
    /// are never rolled back.
    ///
    /// The caller must only unwind when the recorded transaction PROVABLY
    /// cannot land (see `ReleaseConfirmation::Dropped`): BTH has no
    /// on-chain order-id guard, so re-signing with different inputs while
    /// the old transaction could still land would risk a double release.
    pub fn rollback_release(&self, order_id: &Uuid) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let now = Utc::now().timestamp();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction failed: {}", e))?;

        tx.execute(
            "DELETE FROM release_claims WHERE order_id = ?1 AND confirmed_at IS NULL",
            params![order_id.to_string()],
        )
        .map_err(|e| format!("Delete release claim failed: {}", e))?;

        tx.execute(
            r#"
            UPDATE bridge_orders
            SET status = 'burn_confirmed', dest_tx = NULL,
                dest_confirmed_at = NULL, updated_at = ?1
            WHERE id = ?2 AND status = 'release_pending'
            "#,
            params![now, order_id.to_string()],
        )
        .map_err(|e| format!("Update order failed: {}", e))?;

        tx.commit().map_err(|e| format!("Commit failed: {}", e))
    }

    /// Convert a database row to a ReleaseClaim.
    fn row_to_release(row: &rusqlite::Row<'_>) -> SqliteResult<ReleaseClaim> {
        let order_id_str: String = row.get(0)?;
        let order_id_hash: String = row.get(1)?;
        let release_tx_hash: Option<String> = row.get(2)?;
        let release_tx_raw: Option<Vec<u8>> = row.get(3)?;
        let claimed_at: i64 = row.get(4)?;
        let submitted_at: Option<i64> = row.get(5)?;
        let confirmed_at: Option<i64> = row.get(6)?;

        Ok(ReleaseClaim {
            order_id: Uuid::parse_str(&order_id_str).unwrap_or_default(),
            order_id_hash,
            release_tx_hash,
            release_tx_raw,
            claimed_at: Utc.timestamp_opt(claimed_at, 0).unwrap(),
            submitted_at: submitted_at.and_then(|t| Utc.timestamp_opt(t, 0).single()),
            confirmed_at: confirmed_at.and_then(|t| Utc.timestamp_opt(t, 0).single()),
        })
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

    // === Reserve ledger (#825, ADR 0003/0005) ===

    /// Record a locked reserve output backing a mint, exactly once.
    ///
    /// Idempotent by `bridge_output_id` (callers use `"dep:<order_id>"`):
    /// a re-poll of the same confirmed deposit is a no-op. Returns whether
    /// a new output was recorded.
    pub fn record_locked_output(
        &self,
        bridge_output_id: &str,
        chain: Chain,
        amount: u64,
        order_id: &Uuid,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let changed = conn
            .execute(
                r#"
                INSERT OR IGNORE INTO reserve_ledger (
                    bridge_output_id, chain, amount_picocredits, locked,
                    order_id, spent_order_id, created_at, spent_at
                ) VALUES (?1, ?2, ?3, 1, ?4, NULL, ?5, NULL)
                "#,
                params![
                    bridge_output_id,
                    chain.to_string(),
                    amount as i64,
                    order_id.to_string(),
                    Utc::now().timestamp(),
                ],
            )
            .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(changed > 0)
    }

    /// Mark a single reserve output spent (attributed to `spent_order_id`).
    /// Returns whether the output was newly unlocked.
    pub fn mark_output_spent(
        &self,
        bridge_output_id: &str,
        spent_order_id: &Uuid,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        Ok(Self::mark_output_spent_stmt(&conn, bridge_output_id, spent_order_id)? > 0)
    }

    fn mark_output_spent_stmt(
        conn: &Connection,
        bridge_output_id: &str,
        spent_order_id: &Uuid,
    ) -> Result<usize, String> {
        conn.execute(
            r#"
            UPDATE reserve_ledger
            SET locked = 0, spent_order_id = ?1, spent_at = ?2
            WHERE bridge_output_id = ?3 AND locked = 1
            "#,
            params![
                spent_order_id.to_string(),
                Utc::now().timestamp(),
                bridge_output_id
            ],
        )
        .map_err(|e| format!("Update failed: {}", e))
    }

    /// Unlock `net_amount` of backing for a mint that failed after its
    /// deposit was locked: the funds sit in the bridge address but no
    /// longer back wrapped supply — they are owed back to the depositor.
    ///
    /// Value-based (#846): the order's own locked `dep:` output is
    /// unlocked first, but if a release's FIFO spend already consumed it
    /// (its residual value now lives in a `chg:` output attributed to the
    /// release), the remainder is unlocked by VALUE from the chain's
    /// locked outputs FIFO — mirroring [`Database::apply_release_spend`],
    /// change semantics included — so the ledger never permanently
    /// overcounts by the failed mint's amount.
    ///
    /// Exactly-once by attribution: a replay finds outputs already
    /// unlocked with `spent_order_id == order_id` and returns `Ok(false)`.
    /// Fails (rolling back) if the locked reserve cannot cover the
    /// amount — an invariant violation the caller must surface.
    pub fn unlock_backing_for_order(
        &self,
        order_id: &Uuid,
        chain: Chain,
        net_amount: u64,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("Transaction failed: {}", e))?;

        // Idempotency: any output already attributed to this failed mint
        // means the unlock was already applied.
        let already: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM reserve_ledger WHERE spent_order_id = ?1",
                params![order_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        if already > 0 {
            return Ok(false);
        }

        let now = Utc::now().timestamp();
        let mut remaining = net_amount as u128;

        // Pass 1: the order's own locked outputs (the common case — the
        // `dep:` output is untouched and matches the net amount exactly).
        let own: Vec<(String, u64)> = Self::locked_rows(
            &tx,
            r#"
            SELECT bridge_output_id, amount_picocredits FROM reserve_ledger
            WHERE order_id = ?1 AND locked = 1
            ORDER BY created_at ASC, rowid ASC
            "#,
            params![order_id.to_string()],
        )?;
        for (output_id, output_amount) in own {
            tx.execute(
                r#"
                UPDATE reserve_ledger
                SET locked = 0, spent_order_id = ?1, spent_at = ?2
                WHERE bridge_output_id = ?3 AND locked = 1
                "#,
                params![order_id.to_string(), now, output_id],
            )
            .map_err(|e| format!("Update failed: {}", e))?;
            remaining = remaining.saturating_sub(output_amount as u128);
        }

        // Pass 2: FIFO by value over the chain's other locked outputs
        // (the failed mint's own output was consumed by a release spend
        // and its value carried into change).
        if remaining > 0 {
            let rows: Vec<(String, u64)> = Self::locked_rows(
                &tx,
                r#"
                SELECT bridge_output_id, amount_picocredits FROM reserve_ledger
                WHERE chain = ?1 AND locked = 1
                ORDER BY created_at ASC, rowid ASC
                "#,
                params![chain.to_string()],
            )?;

            for (output_id, output_amount) in rows {
                if remaining == 0 {
                    break;
                }
                tx.execute(
                    r#"
                    UPDATE reserve_ledger
                    SET locked = 0, spent_order_id = ?1, spent_at = ?2
                    WHERE bridge_output_id = ?3 AND locked = 1
                    "#,
                    params![order_id.to_string(), now, output_id],
                )
                .map_err(|e| format!("Update failed: {}", e))?;

                let output_amount = output_amount as u128;
                if output_amount >= remaining {
                    let change = output_amount - remaining;
                    remaining = 0;
                    if change > 0 {
                        tx.execute(
                            r#"
                            INSERT INTO reserve_ledger (
                                bridge_output_id, chain, amount_picocredits, locked,
                                order_id, spent_order_id, created_at, spent_at
                            ) VALUES (?1, ?2, ?3, 1, ?4, NULL, ?5, NULL)
                            "#,
                            params![
                                format!("chg:{}", order_id),
                                chain.to_string(),
                                change as i64,
                                order_id.to_string(),
                                now,
                            ],
                        )
                        .map_err(|e| format!("Insert change failed: {}", e))?;
                    }
                } else {
                    remaining -= output_amount;
                }
            }
        }

        if remaining > 0 {
            // Dropping the transaction rolls everything back.
            return Err(format!(
                "insufficient locked reserve on {}: short {} picocredits unlocking failed mint {}",
                chain, remaining, order_id
            ));
        }

        tx.commit().map_err(|e| format!("Commit failed: {}", e))?;
        Ok(true)
    }

    /// Collect `(bridge_output_id, amount)` rows for a locked-output query.
    fn locked_rows(
        conn: &Connection,
        sql: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<(String, u64)>, String> {
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Prepare failed: {}", e))?;
        let rows = stmt
            .query_map(args, |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;
        Ok(rows)
    }

    /// Apply a confirmed release to the reserve ledger: spend locked
    /// outputs of `chain` FIFO until `amount` (the GROSS burn amount — the
    /// on-chain supply dropped by the full burn; the release fee stays in
    /// bridge custody as revenue, not peg backing) is covered, returning
    /// any remainder as a new locked change output.
    ///
    /// Exactly-once by `release_order_id`: a replay finds the prior spend
    /// attribution and returns `Ok(false)` without touching the ledger.
    /// Fails (rolling back) if the locked reserve of `chain` cannot cover
    /// the amount — an invariant violation the caller must surface.
    pub fn apply_release_spend(
        &self,
        release_order_id: &Uuid,
        chain: Chain,
        amount: u64,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("Transaction failed: {}", e))?;

        // Idempotency: any output spent by (or change created for) this
        // release means the spend was already applied.
        let already: i64 = tx
            .query_row(
                r#"
                SELECT COUNT(*) FROM reserve_ledger
                WHERE spent_order_id = ?1 OR order_id = ?1
                "#,
                params![release_order_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        if already > 0 {
            return Ok(false);
        }

        let rows: Vec<(String, u64)> = {
            let mut stmt = tx
                .prepare(
                    r#"
                    SELECT bridge_output_id, amount_picocredits FROM reserve_ledger
                    WHERE chain = ?1 AND locked = 1
                    ORDER BY created_at ASC, rowid ASC
                    "#,
                )
                .map_err(|e| format!("Prepare failed: {}", e))?;
            let mapped = stmt
                .query_map(params![chain.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
                })
                .map_err(|e| format!("Query failed: {}", e))?
                .collect::<SqliteResult<Vec<_>>>()
                .map_err(|e| format!("Collect failed: {}", e))?;
            mapped
        };

        let now = Utc::now().timestamp();
        let mut remaining = amount as u128;
        for (output_id, output_amount) in rows {
            if remaining == 0 {
                break;
            }
            tx.execute(
                r#"
                UPDATE reserve_ledger
                SET locked = 0, spent_order_id = ?1, spent_at = ?2
                WHERE bridge_output_id = ?3 AND locked = 1
                "#,
                params![release_order_id.to_string(), now, output_id],
            )
            .map_err(|e| format!("Update failed: {}", e))?;

            let output_amount = output_amount as u128;
            if output_amount >= remaining {
                let change = output_amount - remaining;
                remaining = 0;
                if change > 0 {
                    tx.execute(
                        r#"
                        INSERT INTO reserve_ledger (
                            bridge_output_id, chain, amount_picocredits, locked,
                            order_id, spent_order_id, created_at, spent_at
                        ) VALUES (?1, ?2, ?3, 1, ?4, NULL, ?5, NULL)
                        "#,
                        params![
                            format!("chg:{}", release_order_id),
                            chain.to_string(),
                            change as i64,
                            release_order_id.to_string(),
                            now,
                        ],
                    )
                    .map_err(|e| format!("Insert change failed: {}", e))?;
                }
            } else {
                remaining -= output_amount;
            }
        }

        if remaining > 0 {
            // Dropping the transaction rolls everything back.
            return Err(format!(
                "insufficient locked reserve on {}: short {} picocredits for release {}",
                chain, remaining, release_order_id
            ));
        }

        tx.commit().map_err(|e| format!("Commit failed: {}", e))?;
        Ok(true)
    }

    /// Total locked reserve in picocredits: `SUM(amount) WHERE locked = 1`.
    /// Summed in Rust as u128 to be overflow-safe, saturated to u64.
    pub fn locked_reserve_total(&self) -> Result<u64, String> {
        self.locked_sum(
            "SELECT amount_picocredits FROM reserve_ledger WHERE locked = 1",
            &[],
        )
    }

    /// Locked reserve backing a single wrapped chain, in picocredits.
    pub fn locked_reserve_by_chain(&self, chain: Chain) -> Result<u64, String> {
        self.locked_sum(
            "SELECT amount_picocredits FROM reserve_ledger WHERE locked = 1 AND chain = ?1",
            &[&chain.to_string()],
        )
    }

    fn locked_sum(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Prepare failed: {}", e))?;
        let amounts = stmt
            .query_map(args, |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;
        let total: u128 = amounts.into_iter().map(|a| a as u64 as u128).sum();
        Ok(u64::try_from(total).unwrap_or(u64::MAX))
    }

    /// Look up a reserve output by id (test/ops helper).
    pub fn get_reserve_output(
        &self,
        bridge_output_id: &str,
    ) -> Result<Option<ReserveOutput>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT bridge_output_id, chain, amount_picocredits, locked,
                       order_id, spent_order_id
                FROM reserve_ledger WHERE bridge_output_id = ?1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row(params![bridge_output_id], |row| {
            let chain_str: String = row.get(1)?;
            let order_id_str: String = row.get(4)?;
            let spent_order_id_str: Option<String> = row.get(5)?;
            Ok(ReserveOutput {
                bridge_output_id: row.get(0)?,
                chain: chain_str.parse().unwrap_or(Chain::Ethereum),
                amount: row.get::<_, i64>(2)? as u64,
                locked: row.get::<_, i64>(3)? != 0,
                order_id: Uuid::parse_str(&order_id_str).unwrap_or_default(),
                spent_order_id: spent_order_id_str.and_then(|s| Uuid::parse_str(&s).ok()),
            })
        })
        .optional()
        .map_err(|e| format!("Query failed: {}", e))
    }

    /// Backing of mints still in flight toward `chain` (deposit locked,
    /// wBTH not yet minted): `SUM(amount - fee)` over `deposit_confirmed`
    /// and `mint_pending` orders. Part of the reconciler's drift allowance.
    pub fn pending_mint_backing(&self, chain: Chain) -> Result<u64, String> {
        self.pending_sum(
            r#"
            SELECT amount - fee FROM bridge_orders
            WHERE order_type = 'mint' AND dest_chain = ?1
              AND status IN ('deposit_confirmed', 'mint_pending')
            "#,
            chain,
        )
    }

    /// Gross burn amounts in flight from `chain` (supply already reduced
    /// on-chain, reserve spend not yet applied): `SUM(amount)` over
    /// `burn_detected`, `burn_confirmed` and `release_pending` orders.
    /// Part of the reconciler's drift allowance.
    pub fn pending_burn_amount(&self, chain: Chain) -> Result<u64, String> {
        self.pending_sum(
            r#"
            SELECT amount FROM bridge_orders
            WHERE order_type = 'burn' AND source_chain = ?1
              AND status IN ('burn_detected', 'burn_confirmed', 'release_pending')
            "#,
            chain,
        )
    }

    fn pending_sum(&self, sql: &str, chain: Chain) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Prepare failed: {}", e))?;
        let amounts = stmt
            .query_map(params![chain.to_string()], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;
        let total: u128 = amounts.into_iter().map(|a| a.max(0) as u128).sum();
        Ok(u64::try_from(total).unwrap_or(u64::MAX))
    }

    /// Persist a reconciliation snapshot (#825 drift history).
    pub fn insert_reserve_snapshot(&self, snapshot: &ReserveSnapshot) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO reserve_snapshots (
                taken_at, locked_reserve, eth_supply, sol_supply,
                drift, in_tolerance, peg_healthy, reserve_balance_checked
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                snapshot.taken_at,
                snapshot.locked_reserve as i64,
                snapshot.eth_supply.map(|s| s as i64),
                snapshot.sol_supply.map(|s| s as i64),
                snapshot.drift,
                snapshot.in_tolerance as i64,
                snapshot.peg_healthy as i64,
                snapshot.reserve_balance_checked as i64,
            ],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(())
    }

    /// Delete reconciliation snapshots older than `retain_secs`, always
    /// keeping the most recent row (#846: the tables are otherwise
    /// unbounded — one row per pass, ~525k rows/yr at the default 60s
    /// cadence). Returns the number of pruned rows.
    pub fn prune_reserve_snapshots(&self, retain_secs: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let cutoff = Utc::now().timestamp() - retain_secs;

        conn.execute(
            r#"
            DELETE FROM reserve_snapshots
            WHERE taken_at < ?1
              AND id != (SELECT MAX(id) FROM reserve_snapshots)
            "#,
            params![cutoff],
        )
        .map_err(|e| format!("Prune failed: {}", e))
    }

    /// Latest reconciliation snapshot, if any pass has run.
    pub fn latest_reserve_snapshot(&self) -> Result<Option<ReserveSnapshot>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT taken_at, locked_reserve, eth_supply, sol_supply,
                       drift, in_tolerance, peg_healthy,
                       COALESCE(reserve_balance_checked, 0)
                FROM reserve_snapshots ORDER BY id DESC LIMIT 1
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        stmt.query_row([], |row| {
            Ok(ReserveSnapshot {
                taken_at: row.get(0)?,
                locked_reserve: row.get::<_, i64>(1)? as u64,
                eth_supply: row.get::<_, Option<i64>>(2)?.map(|s| s as u64),
                sol_supply: row.get::<_, Option<i64>>(3)?.map(|s| s as u64),
                drift: row.get(4)?,
                in_tolerance: row.get::<_, i64>(5)? != 0,
                peg_healthy: row.get::<_, i64>(6)? != 0,
                reserve_balance_checked: row.get::<_, i64>(7)? != 0,
            })
        })
        .optional()
        .map_err(|e| format!("Query failed: {}", e))
    }

    // === Circuit breaker (bridge_state singleton) ===

    /// Whether the bridge is paused. Returns the pause reason when paused.
    pub fn is_paused(&self) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.query_row(
            "SELECT paused, paused_reason FROM bridge_state WHERE id = 1",
            [],
            |row| {
                let paused: i64 = row.get(0)?;
                let reason: Option<String> = row.get(1)?;
                Ok((paused != 0)
                    .then(|| reason.unwrap_or_else(|| "paused (no reason recorded)".to_string())))
            },
        )
        .map_err(|e| format!("Query failed: {}", e))
    }

    /// Set the global pause flag (the circuit breaker / kill switch).
    ///
    /// Returns whether the state CHANGED — callers audit-log the trip
    /// exactly once, so an already-tripped breaker is not re-audited every
    /// tick.
    pub fn set_paused(&self, paused: bool, reason: Option<&str>) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let changed = conn
            .execute(
                r#"
                UPDATE bridge_state
                SET paused = ?1, paused_reason = ?2, paused_at = ?3
                WHERE id = 1 AND paused != ?1
                "#,
                params![
                    paused as i64,
                    if paused { reason } else { None },
                    if paused {
                        Some(Utc::now().timestamp())
                    } else {
                        None
                    },
                ],
            )
            .map_err(|e| format!("Update failed: {}", e))?;

        Ok(changed > 0)
    }

    // === Rate limits (per-order cap + daily rolling windows) ===

    /// Atomically check and reserve an order's volume against the limits,
    /// exactly once per order.
    ///
    /// In a single `BEGIN IMMEDIATE` transaction: an existing reservation
    /// for `order_id` short-circuits to [`LimitCheck::AlreadyReserved`]
    /// (a crashed or replayed tick never double-counts); otherwise the
    /// per-order cap, the address's daily window, and the global daily
    /// window are checked and — only if all pass — the volume is reserved
    /// against the current UTC day bucket. A limit of 0 disables that
    /// check. `Rejected` reserves nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn check_and_reserve_limits(
        &self,
        order_id: &Uuid,
        address: &str,
        amount: u64,
        max_order_amount: u64,
        daily_limit_per_address: u64,
        global_daily_limit: u64,
        now: i64,
    ) -> Result<LimitCheck, String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("Transaction failed: {}", e))?;

        let existing: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM limit_reservations WHERE order_id = ?1",
                params![order_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        if existing > 0 {
            return Ok(LimitCheck::AlreadyReserved);
        }

        if max_order_amount > 0 && amount > max_order_amount {
            return Ok(LimitCheck::Rejected(LimitViolation::PerOrderCap));
        }

        let day_bucket = now.div_euclid(86_400);

        if daily_limit_per_address > 0 {
            let addr_volume: i64 = tx
                .query_row(
                    r#"
                    SELECT COALESCE(SUM(amount), 0) FROM limit_reservations
                    WHERE address = ?1 AND day_bucket = ?2
                    "#,
                    params![address, day_bucket],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Query failed: {}", e))?;
            if (addr_volume.max(0) as u128).saturating_add(amount as u128)
                > daily_limit_per_address as u128
            {
                return Ok(LimitCheck::Rejected(LimitViolation::AddressDailyCap));
            }
        }

        if global_daily_limit > 0 {
            let global_volume: i64 = tx
                .query_row(
                    r#"
                    SELECT COALESCE(SUM(amount), 0) FROM limit_reservations
                    WHERE day_bucket = ?1
                    "#,
                    params![day_bucket],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Query failed: {}", e))?;
            if (global_volume.max(0) as u128).saturating_add(amount as u128)
                > global_daily_limit as u128
            {
                return Ok(LimitCheck::Rejected(LimitViolation::GlobalDailyCap));
            }
        }

        tx.execute(
            r#"
            INSERT INTO limit_reservations (order_id, address, amount, day_bucket, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                order_id.to_string(),
                address,
                amount as i64,
                day_bucket,
                now
            ],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        tx.commit().map_err(|e| format!("Commit failed: {}", e))?;
        Ok(LimitCheck::Reserved)
    }

    /// Total volume reserved against the daily window containing `now`
    /// (monitoring / anomaly detection).
    pub fn daily_reserved_volume(&self, now: i64) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let volume: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(amount), 0) FROM limit_reservations WHERE day_bucket = ?1",
                params![now.div_euclid(86_400)],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(volume.max(0) as u64)
    }

    // === Monitoring queries (#827) ===

    /// Order counts grouped by status (`failed: <reason>` variants are
    /// folded into a single `failed` bucket).
    pub fn order_status_counts(&self) -> Result<Vec<(String, i64)>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT CASE WHEN status LIKE 'failed%' THEN 'failed' ELSE status END AS bucket,
                       COUNT(*)
                FROM bridge_orders GROUP BY bucket ORDER BY bucket
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;
        Ok(rows)
    }

    /// Number of orders the engine still has to act on (the backlog the
    /// circuit breaker trips on).
    pub fn actionable_backlog(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let count: i64 = conn
            .query_row(
                r#"
                SELECT COUNT(*) FROM bridge_orders
                WHERE status IN ('deposit_confirmed', 'mint_pending',
                                 'burn_confirmed', 'release_pending')
                "#,
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(count.max(0) as u64)
    }

    /// Orders past the settlement stages that have not advanced within
    /// `older_than_secs` — stuck orders needing operator attention. Covers
    /// every non-terminal status past `awaiting_deposit` (which expires
    /// instead: no funds have moved yet).
    pub fn stuck_orders(&self, older_than_secs: i64) -> Result<Vec<BridgeOrder>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let cutoff = Utc::now().timestamp() - older_than_secs;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, order_type, source_chain, dest_chain, amount, fee,
                       source_tx, dest_tx, source_address, dest_address,
                       status, error_message, memo, mint_authorization,
                       dest_confirmed_at, created_at, updated_at
                FROM bridge_orders
                WHERE status IN ('deposit_detected', 'deposit_confirmed', 'mint_pending',
                                 'burn_detected', 'burn_confirmed', 'release_pending')
                  AND updated_at < ?1
                ORDER BY updated_at ASC
                "#,
            )
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let rows = stmt
            .query_map(params![cutoff], Self::row_to_order)
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;
        Ok(rows)
    }

    /// Whether an audit entry with this action exists for the order
    /// (used to emit alerts exactly once per order).
    pub fn has_audit_for_order(&self, order_id: &Uuid, action: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE order_id = ?1 AND action = ?2",
                params![order_id.to_string(), action],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(count > 0)
    }

    // === Component health (signer / minter / releaser availability) ===

    /// Record a component's availability (engine startup + runtime checks).
    pub fn set_component_health(
        &self,
        component: &str,
        healthy: bool,
        detail: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO component_health (component, healthy, detail, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![component, healthy as i64, detail, Utc::now().timestamp()],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(())
    }

    /// All recorded component health rows: `(component, healthy, detail)`.
    pub fn component_health(&self) -> Result<Vec<(String, bool, String)>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare("SELECT component, healthy, detail FROM component_health ORDER BY component")
            .map_err(|e| format!("Prepare failed: {}", e))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            })
            .map_err(|e| format!("Query failed: {}", e))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Collect failed: {}", e))?;
        Ok(rows)
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

        // Update status along a legal edge.
        db.update_order_status(&order.id, &OrderStatus::DepositDetected, None)
            .unwrap();

        let updated = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(updated.status, OrderStatus::DepositDetected);
    }

    #[test]
    fn test_update_order_status_enforces_state_machine() {
        // #839: the DB layer itself rejects illegal transitions, so no
        // future writer can bypass the state machine.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        db.insert_order(&order).unwrap();

        // Illegal jump: AwaitingDeposit -> Completed.
        let err = db
            .update_order_status(&order.id, &OrderStatus::Completed, None)
            .unwrap_err();
        assert!(err.contains("illegal order status transition"), "{}", err);
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::AwaitingDeposit
        );

        // Terminal states are frozen even through the raw update path.
        db.update_order_status(&order.id, &OrderStatus::Expired, None)
            .unwrap();
        assert!(db
            .update_order_status(&order.id, &OrderStatus::DepositDetected, None)
            .is_err());
        assert!(db
            .update_order_status(
                &order.id,
                &OrderStatus::Failed {
                    reason: "cannot clobber terminal".to_string()
                },
                None
            )
            .is_err());
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::Expired
        );

        // Unknown order id is an error, not a silent no-op.
        assert!(db
            .update_order_status(&Uuid::new_v4(), &OrderStatus::Expired, None)
            .is_err());
    }

    #[test]
    fn test_update_order_status_same_status_is_idempotent_refresh() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db);

        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xtx"))
            .unwrap();
        // A replayed tick re-asserting the same status must not error.
        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xtx"))
            .unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::MintPending
        );
    }

    #[test]
    fn test_update_order_status_terminal_same_status_preserves_dest_tx() {
        // #855 defense-in-depth: a `Completed -> Completed` write with a
        // different tx_hash must NOT overwrite the settled dest_tx.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db); // DepositConfirmed

        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xtx"))
            .unwrap();
        db.update_order_status(&order.id, &OrderStatus::Completed, Some("A"))
            .unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().dest_tx.as_deref(),
            Some("A")
        );

        // Same-status write on a terminal order with a different hash must be
        // a no-op for dest_tx (the tx-hash refresh branch is gated off).
        db.update_order_status(&order.id, &OrderStatus::Completed, Some("B"))
            .unwrap();
        let after = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(after.status, OrderStatus::Completed);
        assert_eq!(
            after.dest_tx.as_deref(),
            Some("A"),
            "terminal same-status write must not clobber the settled dest_tx"
        );
    }

    #[test]
    fn test_update_order_status_nonterminal_same_status_refreshes_dest_tx() {
        // The terminal guard must not regress the legitimate non-terminal
        // idempotent refresh: MintPending -> MintPending updates dest_tx.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_mint_order(&db);

        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("A"))
            .unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().dest_tx.as_deref(),
            Some("A")
        );
        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("B"))
            .unwrap();
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().dest_tx.as_deref(),
            Some("B"),
            "non-terminal same-status refresh must still update dest_tx"
        );
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

    fn setup_burn_order(db: &Database) -> BridgeOrder {
        let mut order = BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_user_stealth_addr".to_string(),
            "0xburntx".to_string(),
            0,
        );
        order.set_status(OrderStatus::BurnConfirmed);
        db.insert_order(&order).unwrap();
        order
    }

    #[test]
    fn test_release_claim_exactly_once() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_burn_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        // First claim: fresh, no tx recorded.
        let first = db.try_claim_release(&order.id, &hash).unwrap();
        assert!(first.release_tx_hash.is_none());
        assert!(first.submitted_at.is_none());
        assert!(first.confirmed_at.is_none());

        // Record the signed tx (before broadcast).
        let recorded = db
            .record_release_tx(&order.id, "bth_tx_one", &[0xde, 0xad])
            .unwrap();
        assert_eq!(recorded.release_tx_hash.as_deref(), Some("bth_tx_one"));
        assert_eq!(recorded.release_tx_raw.as_deref(), Some(&[0xde, 0xad][..]));
        assert!(recorded.submitted_at.is_some());

        // A re-claim (concurrent tick / post-restart) returns the SAME
        // recorded tx — the caller must reuse it, never sign a second one.
        let reclaim = db.try_claim_release(&order.id, &hash).unwrap();
        assert_eq!(reclaim.release_tx_hash.as_deref(), Some("bth_tx_one"));

        // A duplicate record with a DIFFERENT tx must not overwrite:
        // exactly-once.
        let second = db
            .record_release_tx(&order.id, "bth_tx_two", &[0xbe, 0xef])
            .unwrap();
        assert_eq!(
            second.release_tx_hash.as_deref(),
            Some("bth_tx_one"),
            "a recorded release tx must never be replaced"
        );
        assert_eq!(second.release_tx_raw.as_deref(), Some(&[0xde, 0xad][..]));
    }

    #[test]
    fn test_release_confirm_flow() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_burn_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        db.try_claim_release(&order.id, &hash).unwrap();
        db.record_release_tx(&order.id, "bth_tx", &[1]).unwrap();
        db.update_order_status(&order.id, &OrderStatus::ReleasePending, Some("bth_tx"))
            .unwrap();

        db.mark_release_confirmed(&order.id).unwrap();

        let claim = db.get_release_by_order(&order.id).unwrap().unwrap();
        assert!(claim.confirmed_at.is_some());

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::Released);
        assert!(stored.dest_confirmed_at.is_some());
    }

    #[test]
    fn test_release_rollback_clears_unconfirmed() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_burn_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        db.try_claim_release(&order.id, &hash).unwrap();
        db.record_release_tx(&order.id, "bth_dropped", &[1])
            .unwrap();
        db.update_order_status(&order.id, &OrderStatus::ReleasePending, Some("bth_dropped"))
            .unwrap();

        // Provably-dead tx: unwind to BurnConfirmed, claim gone.
        db.rollback_release(&order.id).unwrap();

        assert!(db.get_release_by_order(&order.id).unwrap().is_none());
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::BurnConfirmed);
        assert!(stored.dest_tx.is_none());

        // Re-entry after rollback takes a fresh claim.
        let fresh = db.try_claim_release(&order.id, &hash).unwrap();
        assert!(fresh.release_tx_hash.is_none());
    }

    #[test]
    fn test_release_rollback_never_touches_confirmed() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let order = setup_burn_order(&db);
        let hash = hex::encode(order.order_id_bytes());

        db.try_claim_release(&order.id, &hash).unwrap();
        db.record_release_tx(&order.id, "bth_tx", &[1]).unwrap();
        db.update_order_status(&order.id, &OrderStatus::ReleasePending, Some("bth_tx"))
            .unwrap();
        db.mark_release_confirmed(&order.id).unwrap();

        // A late rollback attempt must be a no-op on a confirmed release.
        db.rollback_release(&order.id).unwrap();

        assert!(
            db.get_release_by_order(&order.id).unwrap().is_some(),
            "confirmed release claim must survive"
        );
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::Released);
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
            safe_nonce: Some(7),
        });
        db.insert_order(&order).unwrap();

        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.mint_authorization, order.mint_authorization);
        assert!(stored.dest_confirmed_at.is_none());
    }

    #[test]
    fn test_watcher_cursor_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        assert!(db.get_cursor(Chain::Ethereum).unwrap().is_none());

        db.set_cursor(Chain::Ethereum, 100, Some("0xaa")).unwrap();
        let cursor = db.get_cursor(Chain::Ethereum).unwrap().unwrap();
        assert_eq!(cursor.last_height, 100);
        assert_eq!(cursor.last_block_hash.as_deref(), Some("0xaa"));

        // Upsert replaces; per-chain rows are independent.
        db.set_cursor(Chain::Ethereum, 101, Some("0xbb")).unwrap();
        db.set_cursor(Chain::Bth, 7, None).unwrap();
        let eth = db.get_cursor(Chain::Ethereum).unwrap().unwrap();
        assert_eq!(eth.last_height, 101);
        assert_eq!(eth.last_block_hash.as_deref(), Some("0xbb"));
        let bth = db.get_cursor(Chain::Bth).unwrap().unwrap();
        assert_eq!(bth.last_height, 7);
        assert!(bth.last_block_hash.is_none());
    }

    fn burn_order(source_tx: &str) -> BridgeOrder {
        BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            0,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_stealth_addr".to_string(),
            source_tx.to_string(),
            0,
        )
    }

    #[test]
    fn test_insert_burn_order_exactly_once_by_source_key() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let first = burn_order("0xburn");
        assert!(db
            .insert_burn_order(&first, "0xburn#0", 50, Some("0xblock50"))
            .unwrap());

        // Replay with the SAME source key (cursor rewind / reorg re-add):
        // the second insert is a no-op. Burn ids are deterministic over the
        // source tuple (#1050), so `dup` derives the SAME id as `first`; the
        // idempotency guard must skip it rather than collide on the order id.
        let dup = burn_order("0xburn");
        assert_eq!(dup.id, first.id, "same burn tuple must derive the same id");
        assert!(!db
            .insert_burn_order(&dup, "0xburn#0", 51, Some("0xblock51"))
            .unwrap());
        // Exactly one order row exists, unchanged from the first insert (the
        // replay did not overwrite block 50 with block 51).
        assert!(db.get_order(&first.id).unwrap().is_some());

        let rec = db.get_burn_by_source("0xburn#0").unwrap().unwrap();
        assert_eq!(rec.order_id, first.id);
        assert_eq!(rec.block_number, 50);
        assert!(!rec.orphaned);
        assert_eq!(
            db.get_burn_by_order(&first.id).unwrap().unwrap().source_key,
            "0xburn#0"
        );
    }

    #[test]
    fn test_burn_orphan_and_relocate() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let order = burn_order("0xburn");
        db.insert_burn_order(&order, "0xburn#0", 50, Some("0xold"))
            .unwrap();

        // Orphan flag is set exactly once.
        assert!(db.mark_burn_orphaned("0xburn#0").unwrap());
        assert!(!db.mark_burn_orphaned("0xburn#0").unwrap());
        assert!(db.get_burn_by_source("0xburn#0").unwrap().unwrap().orphaned);

        // Re-inclusion relocates the record and clears the flag.
        db.update_burn_location("0xburn#0", 52, Some("0xnew"))
            .unwrap();
        let rec = db.get_burn_by_source("0xburn#0").unwrap().unwrap();
        assert_eq!(rec.block_number, 52);
        assert_eq!(rec.block_hash.as_deref(), Some("0xnew"));
        assert!(!rec.orphaned);
    }

    #[test]
    fn test_record_deposit_detected_guarded_and_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bridge_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        db.insert_order(&order).unwrap();

        // First detection records the revealed amount + source tx.
        assert!(db
            .record_deposit_detected(&order.id, "0xdeposit", 999_000_000_000)
            .unwrap());
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.status, OrderStatus::DepositDetected);
        assert_eq!(stored.source_tx.as_deref(), Some("0xdeposit"));
        assert_eq!(stored.amount, 999_000_000_000);

        // Replay is a no-op (status guard).
        assert!(!db.record_deposit_detected(&order.id, "0xother", 1).unwrap());
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(stored.source_tx.as_deref(), Some("0xdeposit"));
        assert_eq!(stored.amount, 999_000_000_000);
    }

    // === Reserve ledger (#825) ===

    fn reserve_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    #[test]
    fn test_record_locked_output_idempotent() {
        let db = reserve_db();
        let order = Uuid::new_v4();

        assert!(db
            .record_locked_output("dep:a", Chain::Ethereum, 1_000, &order)
            .unwrap());
        // Replay (same output id, even a different amount) is a no-op.
        assert!(!db
            .record_locked_output("dep:a", Chain::Ethereum, 9_999, &order)
            .unwrap());

        assert_eq!(db.locked_reserve_total().unwrap(), 1_000);
        let output = db.get_reserve_output("dep:a").unwrap().unwrap();
        assert!(output.locked);
        assert_eq!(output.amount, 1_000);
        assert_eq!(output.order_id, order);
    }

    #[test]
    fn test_mark_output_spent_moves_out_of_total_once() {
        let db = reserve_db();
        let mint = Uuid::new_v4();
        let spender = Uuid::new_v4();

        db.record_locked_output("dep:a", Chain::Ethereum, 700, &mint)
            .unwrap();
        db.record_locked_output("dep:b", Chain::Ethereum, 300, &mint)
            .unwrap();
        assert_eq!(db.locked_reserve_total().unwrap(), 1_000);

        assert!(db.mark_output_spent("dep:a", &spender).unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 300);

        // Double-spend rejected: already unlocked.
        assert!(!db.mark_output_spent("dep:a", &spender).unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 300);

        let output = db.get_reserve_output("dep:a").unwrap().unwrap();
        assert!(!output.locked);
        assert_eq!(output.spent_order_id, Some(spender));
    }

    #[test]
    fn test_apply_release_spend_fifo_with_change() {
        let db = reserve_db();
        let m1 = Uuid::new_v4();
        let m2 = Uuid::new_v4();
        let release = Uuid::new_v4();

        db.record_locked_output("dep:1", Chain::Ethereum, 600, &m1)
            .unwrap();
        db.record_locked_output("dep:2", Chain::Ethereum, 500, &m2)
            .unwrap();
        // A Solana-backed output must not be touched by an Ethereum spend.
        db.record_locked_output("dep:sol", Chain::Solana, 400, &m1)
            .unwrap();

        // Spend 900: consumes dep:1 (600) + 300 of dep:2, change 200.
        assert!(db
            .apply_release_spend(&release, Chain::Ethereum, 900)
            .unwrap());
        assert_eq!(db.locked_reserve_by_chain(Chain::Ethereum).unwrap(), 200);
        assert_eq!(db.locked_reserve_by_chain(Chain::Solana).unwrap(), 400);
        assert_eq!(db.locked_reserve_total().unwrap(), 600);

        let change = db
            .get_reserve_output(&format!("chg:{}", release))
            .unwrap()
            .unwrap();
        assert!(change.locked);
        assert_eq!(change.amount, 200);
        assert_eq!(change.order_id, release);

        // Replay of the same release is a no-op (exactly-once).
        assert!(!db
            .apply_release_spend(&release, Chain::Ethereum, 900)
            .unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 600);
    }

    #[test]
    fn test_apply_release_spend_insufficient_rolls_back() {
        let db = reserve_db();
        let mint = Uuid::new_v4();
        let release = Uuid::new_v4();

        db.record_locked_output("dep:1", Chain::Ethereum, 100, &mint)
            .unwrap();

        // 250 > 100 locked: the whole spend must fail and roll back.
        let err = db
            .apply_release_spend(&release, Chain::Ethereum, 250)
            .unwrap_err();
        assert!(err.contains("insufficient locked reserve"), "{}", err);

        // Nothing was mutated: the output is still locked, no change row.
        assert_eq!(db.locked_reserve_total().unwrap(), 100);
        assert!(db.get_reserve_output("dep:1").unwrap().unwrap().locked);
        assert!(db
            .get_reserve_output(&format!("chg:{}", release))
            .unwrap()
            .is_none());

        // A later, coverable spend succeeds (the failed attempt left no
        // idempotency residue).
        assert!(db
            .apply_release_spend(&release, Chain::Ethereum, 100)
            .unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 0);
    }

    #[test]
    fn test_unlock_backing_for_failed_mint() {
        let db = reserve_db();
        let mint = Uuid::new_v4();

        db.record_locked_output(&format!("dep:{}", mint), Chain::Ethereum, 500, &mint)
            .unwrap();
        assert_eq!(db.locked_reserve_total().unwrap(), 500);

        assert!(db
            .unlock_backing_for_order(&mint, Chain::Ethereum, 500)
            .unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 0);
        // Idempotent.
        assert!(!db
            .unlock_backing_for_order(&mint, Chain::Ethereum, 500)
            .unwrap());
    }

    #[test]
    fn test_unlock_backing_by_value_after_fifo_consumed_dep_output() {
        // #846 item 1: a release's FIFO spend consumed the failed mint's
        // dep: output (its residual value lives in a chg: output). The
        // unlock must still remove the failed mint's net amount from the
        // locked ledger — by value — or the ledger overcounts forever.
        let db = reserve_db();
        let failed_mint = Uuid::new_v4();
        let other_mint = Uuid::new_v4();
        let release = Uuid::new_v4();

        // FIFO order: the failed mint's output first (600), then another
        // deposit (1_000).
        db.record_locked_output(
            &format!("dep:{}", failed_mint),
            Chain::Ethereum,
            600,
            &failed_mint,
        )
        .unwrap();
        db.record_locked_output(
            &format!("dep:{}", other_mint),
            Chain::Ethereum,
            1_000,
            &other_mint,
        )
        .unwrap();

        // A release spends 700: consumes the failed mint's 600 entirely +
        // 100 of the other output, change 900 attributed to the release.
        assert!(db
            .apply_release_spend(&release, Chain::Ethereum, 700)
            .unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 900);

        // Now the mint fails. Its dep: output is gone (locked = 0), so an
        // id-based unlock would find nothing. The value-based unlock
        // removes 600 from the remaining locked outputs FIFO.
        assert!(db
            .unlock_backing_for_order(&failed_mint, Chain::Ethereum, 600)
            .unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 300);

        // Change semantics: the 900 change output was consumed, and a new
        // 300 change output attributed to the failed mint remains locked.
        let change = db
            .get_reserve_output(&format!("chg:{}", failed_mint))
            .unwrap()
            .unwrap();
        assert!(change.locked);
        assert_eq!(change.amount, 300);

        // Replay is a no-op.
        assert!(!db
            .unlock_backing_for_order(&failed_mint, Chain::Ethereum, 600)
            .unwrap());
        assert_eq!(db.locked_reserve_total().unwrap(), 300);
    }

    #[test]
    fn test_unlock_backing_insufficient_rolls_back() {
        let db = reserve_db();
        let mint = Uuid::new_v4();
        db.record_locked_output("dep:other", Chain::Ethereum, 100, &Uuid::new_v4())
            .unwrap();

        // 500 > 100 locked: the whole unlock must fail and roll back.
        let err = db
            .unlock_backing_for_order(&mint, Chain::Ethereum, 500)
            .unwrap_err();
        assert!(err.contains("insufficient locked reserve"), "{}", err);
        assert_eq!(db.locked_reserve_total().unwrap(), 100);
    }

    // === Circuit breaker + rate limits (#827) ===

    #[test]
    fn test_pause_state_roundtrip_and_change_detection() {
        let db = reserve_db();

        assert!(db.is_paused().unwrap().is_none());

        // Trip: state changes exactly once.
        assert!(db.set_paused(true, Some("drift alert")).unwrap());
        assert_eq!(db.is_paused().unwrap().as_deref(), Some("drift alert"));
        assert!(!db.set_paused(true, Some("drift alert")).unwrap());

        // Resume clears the reason.
        assert!(db.set_paused(false, None).unwrap());
        assert!(db.is_paused().unwrap().is_none());
        assert!(!db.set_paused(false, None).unwrap());
    }

    #[test]
    fn test_limits_per_order_cap() {
        let db = reserve_db();
        let order = setup_mint_order(&db);

        let check = db
            .check_and_reserve_limits(&order.id, "0xuser", 1_001, 1_000, 0, 0, 1_000_000)
            .unwrap();
        assert_eq!(check, LimitCheck::Rejected(LimitViolation::PerOrderCap));
        // Nothing was reserved.
        assert_eq!(db.daily_reserved_volume(1_000_000).unwrap(), 0);
    }

    #[test]
    fn test_limits_daily_windows_and_day_boundary() {
        let db = reserve_db();
        let a = setup_mint_order(&db);
        let b = setup_mint_order(&db);
        let c = setup_mint_order(&db);
        let now = 86_400 * 100 + 10; // some day bucket

        // Address cap 1_000: first 700 passes...
        assert_eq!(
            db.check_and_reserve_limits(&a.id, "0xuser", 700, 0, 1_000, 10_000, now)
                .unwrap(),
            LimitCheck::Reserved
        );
        // ... a second 700 for the same address exceeds the window ...
        assert_eq!(
            db.check_and_reserve_limits(&b.id, "0xuser", 700, 0, 1_000, 10_000, now)
                .unwrap(),
            LimitCheck::Rejected(LimitViolation::AddressDailyCap)
        );
        // ... but passes at the next day boundary.
        assert_eq!(
            db.check_and_reserve_limits(&b.id, "0xuser", 700, 0, 1_000, 10_000, now + 86_400)
                .unwrap(),
            LimitCheck::Reserved
        );

        // Global cap: a different address is refused once the bridge-wide
        // window is exhausted.
        assert_eq!(
            db.check_and_reserve_limits(&c.id, "0xother", 9_500, 0, 10_000, 10_000, now + 86_400)
                .unwrap(),
            LimitCheck::Rejected(LimitViolation::GlobalDailyCap)
        );
    }

    #[test]
    fn test_limits_reservation_is_exactly_once_per_order() {
        let db = reserve_db();
        let order = setup_mint_order(&db);
        let now = 86_400 * 100;

        assert_eq!(
            db.check_and_reserve_limits(&order.id, "0xuser", 600, 0, 1_000, 0, now)
                .unwrap(),
            LimitCheck::Reserved
        );
        // A replayed tick (crash between reservation and mint) must not
        // double-count the volume.
        assert_eq!(
            db.check_and_reserve_limits(&order.id, "0xuser", 600, 0, 1_000, 0, now)
                .unwrap(),
            LimitCheck::AlreadyReserved
        );
        assert_eq!(db.daily_reserved_volume(now).unwrap(), 600);
    }

    #[test]
    fn test_monitoring_counts_backlog_stuck_and_health() {
        let db = reserve_db();

        let confirmed = setup_mint_order(&db); // deposit_confirmed
        let mut stale = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000,
            0,
            "bth".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        stale.set_status(OrderStatus::MintPending);
        stale.updated_at = Utc::now() - chrono::Duration::hours(3);
        db.insert_order(&stale).unwrap();

        let counts = db.order_status_counts().unwrap();
        assert!(counts.contains(&("deposit_confirmed".to_string(), 1)));
        assert!(counts.contains(&("mint_pending".to_string(), 1)));
        assert_eq!(db.actionable_backlog().unwrap(), 2);

        // Only the stale order is stuck past a 1-hour threshold.
        let stuck = db.stuck_orders(3_600).unwrap();
        assert_eq!(stuck.len(), 1);
        assert_eq!(stuck[0].id, stale.id);
        let _ = confirmed;

        // Component health roundtrip.
        db.set_component_health("attestation", false, "federation misconfigured")
            .unwrap();
        db.set_component_health("releaser:bth", true, "").unwrap();
        let health = db.component_health().unwrap();
        assert_eq!(
            health,
            vec![
                (
                    "attestation".to_string(),
                    false,
                    "federation misconfigured".to_string()
                ),
                ("releaser:bth".to_string(), true, String::new()),
            ]
        );
    }

    #[test]
    fn test_prune_reserve_snapshots_keeps_latest() {
        let db = reserve_db();
        let old = ReserveSnapshot {
            taken_at: 1_000, // far in the past
            locked_reserve: 1,
            eth_supply: None,
            sol_supply: None,
            drift: 0,
            in_tolerance: true,
            peg_healthy: true,
            reserve_balance_checked: false,
        };
        db.insert_reserve_snapshot(&old).unwrap();
        db.insert_reserve_snapshot(&ReserveSnapshot {
            taken_at: 2_000,
            ..old.clone()
        })
        .unwrap();

        // Both are ancient, but the latest row always survives pruning.
        let pruned = db.prune_reserve_snapshots(86_400).unwrap();
        assert_eq!(pruned, 1);
        let latest = db.latest_reserve_snapshot().unwrap().unwrap();
        assert_eq!(latest.taken_at, 2_000);
        assert_eq!(db.prune_reserve_snapshots(86_400).unwrap(), 0);
    }

    #[test]
    fn test_pending_allowance_sums_by_status_and_chain() {
        let db = reserve_db();

        let mut confirmed = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000,
            100,
            "bth".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        confirmed.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&confirmed).unwrap();

        let mut completed = BridgeOrder::new_mint(
            Chain::Ethereum,
            2_000,
            0,
            "bth".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        completed.set_status(OrderStatus::Completed);
        db.insert_order(&completed).unwrap();

        let mut burning = BridgeOrder::new_burn(
            Chain::Ethereum,
            700,
            0,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_dest".to_string(),
            "0xburn".to_string(),
            0,
        );
        burning.set_status(OrderStatus::BurnConfirmed);
        db.insert_order(&burning).unwrap();

        // Only the in-flight mint counts, net of fee; Completed does not.
        assert_eq!(db.pending_mint_backing(Chain::Ethereum).unwrap(), 900);
        assert_eq!(db.pending_mint_backing(Chain::Solana).unwrap(), 0);
        // The unreleased burn counts gross.
        assert_eq!(db.pending_burn_amount(Chain::Ethereum).unwrap(), 700);
        assert_eq!(db.pending_burn_amount(Chain::Solana).unwrap(), 0);
    }

    #[test]
    fn test_reserve_snapshot_roundtrip_and_latest() {
        let db = reserve_db();
        assert!(db.latest_reserve_snapshot().unwrap().is_none());

        let first = ReserveSnapshot {
            taken_at: 1_000,
            locked_reserve: 5_000,
            eth_supply: Some(5_000),
            sol_supply: None,
            drift: 0,
            in_tolerance: true,
            peg_healthy: true,
            reserve_balance_checked: false,
        };
        db.insert_reserve_snapshot(&first).unwrap();

        let second = ReserveSnapshot {
            taken_at: 2_000,
            locked_reserve: 5_000,
            eth_supply: Some(6_000),
            sol_supply: None,
            drift: 1_000,
            in_tolerance: false,
            peg_healthy: false,
            reserve_balance_checked: true,
        };
        db.insert_reserve_snapshot(&second).unwrap();

        let latest = db.latest_reserve_snapshot().unwrap().unwrap();
        assert_eq!(latest, second);
    }

    #[test]
    fn test_count_and_prune_expired_mint_orders() {
        // #1042: expired, deposit-less mint orders are countable (global
        // create ceiling) and prunable (bounded DB growth); anything that
        // saw funds move is never pruned.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_reserve_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        db.insert_order(&order).unwrap();
        assert_eq!(db.count_awaiting_deposit_mint_orders().unwrap(), 1);

        // A burn order does not count toward the mint-create ceiling.
        let burn = BridgeOrder::new_burn(
            Chain::Ethereum,
            500,
            0,
            "0xsource".to_string(),
            "bth_dest".to_string(),
            "0xburntx".to_string(),
            0,
        );
        db.insert_order(&burn).unwrap();
        assert_eq!(db.count_awaiting_deposit_mint_orders().unwrap(), 1);

        // Expire the mint order; the count drops, the row still exists.
        db.update_order_status(&order.id, &OrderStatus::Expired, None)
            .unwrap();
        assert_eq!(db.count_awaiting_deposit_mint_orders().unwrap(), 0);
        assert!(db.get_order(&order.id).unwrap().is_some());

        // Within the retention window nothing is pruned (updated_at is
        // "now"; a large retain_secs puts the cutoff in the past).
        assert_eq!(db.prune_expired_mint_orders(3600).unwrap(), 0);
        assert!(db.get_order(&order.id).unwrap().is_some());

        // Past the retention window (negative retention pushes the cutoff
        // into the future) the expired residue is deleted...
        assert_eq!(db.prune_expired_mint_orders(-1).unwrap(), 1);
        assert!(db.get_order(&order.id).unwrap().is_none());
        // ...but the burn order (funds moved) is untouched.
        assert!(db.get_order(&burn.id).unwrap().is_some());
    }

    #[test]
    fn test_count_and_prune_expired_release_intents() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let now = Utc::now().timestamp();
        let live = ReleaseIntent {
            id: Uuid::new_v4(),
            source_chain: Chain::Ethereum,
            bth_address: "bth_addr_live".to_string(),
            amount: 100,
            fee: 1,
            token_address: "0xtoken".to_string(),
            created_at: now,
            expires_at: now + 3_600,
        };
        let stale = ReleaseIntent {
            id: Uuid::new_v4(),
            source_chain: Chain::Ethereum,
            bth_address: "bth_addr_stale".to_string(),
            amount: 200,
            fee: 1,
            token_address: "0xtoken".to_string(),
            created_at: now - 10_000,
            expires_at: now - 5_000,
        };
        db.insert_release_intent(&live).unwrap();
        db.insert_release_intent(&stale).unwrap();

        // Only the unexpired intent counts toward the global ceiling.
        assert_eq!(db.count_active_release_intents(now).unwrap(), 1);

        // Retention keeps recently-expired intents pollable...
        assert_eq!(db.prune_expired_release_intents(10_000).unwrap(), 0);
        // ...and prunes them once the window elapses; the live intent stays.
        assert_eq!(db.prune_expired_release_intents(1_000).unwrap(), 1);
        assert!(db.get_release_intent(&stale.id).unwrap().is_none());
        assert!(db.get_release_intent(&live.id).unwrap().is_some());
    }

    /// Build a mint order with a forced status + creation time (bypassing
    /// the state machine — these rows exercise the aggregation SQL only).
    fn mint_at(amount: u64, status: OrderStatus, created: i64) -> BridgeOrder {
        let mut o = BridgeOrder::new_mint(
            Chain::Ethereum,
            amount,
            1,
            "bth_reserve".to_string(),
            "0xdest".to_string(),
        );
        o.status = status;
        o.created_at = Utc.timestamp_opt(created, 0).unwrap();
        o
    }

    /// Build a burn order with a forced status + creation time.
    fn burn_at(amount: u64, status: OrderStatus, created: i64) -> BridgeOrder {
        let mut o = BridgeOrder::new_burn(
            Chain::Ethereum,
            amount,
            1,
            "0xsource".to_string(),
            "bth_dest".to_string(),
            format!("0xburntx-{}", Uuid::new_v4()),
            0,
        );
        o.status = status;
        o.created_at = Utc.timestamp_opt(created, 0).unwrap();
        o
    }

    #[test]
    fn test_aggregate_order_activity_buckets_mint_orders() {
        // #1054: the wrap-side aggregation buckets by outcome and applies
        // the window edge INCLUSIVELY on created_at.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let now = Utc::now().timestamp();
        let cutoff = now - 86_400;

        // In-window: one of each bucket.
        db.insert_order(&mint_at(100, OrderStatus::Completed, now - 10))
            .unwrap();
        db.insert_order(&mint_at(200, OrderStatus::AwaitingDeposit, now - 10))
            .unwrap();
        db.insert_order(&mint_at(300, OrderStatus::MintPending, now - 10))
            .unwrap();
        db.insert_order(&mint_at(400, OrderStatus::Expired, now - 10))
            .unwrap();
        db.insert_order(&mint_at(
            500,
            OrderStatus::Failed {
                reason: "boom".to_string(),
            },
            now - 10,
        ))
        .unwrap();
        // Window edge: created exactly AT the cutoff is IN the window...
        db.insert_order(&mint_at(1_000, OrderStatus::Completed, cutoff))
            .unwrap();
        // ...one second earlier is all-time only.
        db.insert_order(&mint_at(10_000, OrderStatus::Completed, cutoff - 1))
            .unwrap();

        let day = db
            .aggregate_order_activity(OrderType::Mint, Some(cutoff))
            .unwrap();
        assert_eq!(day.completed.count, 2, "in-window + exact-cutoff order");
        assert_eq!(day.completed.volume, 1_100);
        assert_eq!(day.pending.count, 2, "awaiting_deposit + mint_pending");
        assert_eq!(day.pending.volume, 500);
        assert_eq!(day.expired.count, 1);
        assert_eq!(day.expired.volume, 400);
        assert_eq!(day.failed.count, 1);
        assert_eq!(day.failed.volume, 500);

        let all = db.aggregate_order_activity(OrderType::Mint, None).unwrap();
        assert_eq!(all.completed.count, 3, "pre-cutoff order counts all-time");
        assert_eq!(all.completed.volume, 11_100);
        assert_eq!(all.pending, day.pending);
        assert_eq!(all.expired, day.expired);
        assert_eq!(all.failed, day.failed);
    }

    #[test]
    fn test_aggregate_order_activity_buckets_burn_orders_separately() {
        // #1054: unwraps aggregate over BURN orders only — `released` is
        // their settled bucket — and never bleed into the mint side.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let now = Utc::now().timestamp();
        let cutoff = now - 86_400;

        db.insert_order(&burn_at(700, OrderStatus::Released, now - 10))
            .unwrap();
        db.insert_order(&burn_at(800, OrderStatus::BurnConfirmed, now - 10))
            .unwrap();
        db.insert_order(&burn_at(900, OrderStatus::Released, cutoff - 1))
            .unwrap();
        // A completed MINT order must not appear in the burn aggregate.
        db.insert_order(&mint_at(100, OrderStatus::Completed, now - 10))
            .unwrap();

        let day = db
            .aggregate_order_activity(OrderType::Burn, Some(cutoff))
            .unwrap();
        assert_eq!(day.completed.count, 1);
        assert_eq!(day.completed.volume, 700);
        assert_eq!(day.pending.count, 1);
        assert_eq!(day.pending.volume, 800);
        assert_eq!(day.expired, ActivityAggregate::default());
        assert_eq!(day.failed, ActivityAggregate::default());

        let all = db.aggregate_order_activity(OrderType::Burn, None).unwrap();
        assert_eq!(all.completed.count, 2);
        assert_eq!(all.completed.volume, 1_600);

        // And the mint aggregate sees only the mint order.
        let mint_all = db.aggregate_order_activity(OrderType::Mint, None).unwrap();
        assert_eq!(mint_all.completed.count, 1);
        assert_eq!(mint_all.completed.volume, 100);
        assert_eq!(mint_all.pending, ActivityAggregate::default());
    }

    #[test]
    fn test_aggregate_order_activity_empty_db_is_all_zero() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let empty = db.aggregate_order_activity(OrderType::Mint, None).unwrap();
        assert_eq!(empty, ActivityBreakdown::default());
    }

    #[test]
    fn test_aggregate_order_activity_no_i64_overflow_past_i64_max() {
        // #1059: a single status bucket whose summed volume exceeds i64::MAX
        // (~9.22M BTH in picocredits) must NOT error. The old SQLite
        // `SUM(amount)` raised an "integer overflow" runtime error here and
        // the handler mapped it to a permanent 500.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let now = Utc::now().timestamp();
        // Each row carries the largest amount a single order can store
        // (amount is persisted as i64). Two of them sum past i64::MAX.
        let big = i64::MAX as u64;
        db.insert_order(&mint_at(big, OrderStatus::Completed, now - 20))
            .unwrap();
        db.insert_order(&mint_at(big, OrderStatus::Completed, now - 10))
            .unwrap();

        let all = db
            .aggregate_order_activity(OrderType::Mint, None)
            .expect("aggregation must not overflow past i64::MAX");
        assert_eq!(all.completed.count, 2);
        // Exact 2 * i64::MAX — larger than i64::MAX, still within u64.
        let expected = 2u128 * i64::MAX as u128;
        assert_eq!(all.completed.volume, expected);
        assert!(
            expected > i64::MAX as u128,
            "test must cross the i64 ceiling"
        );
    }

    #[test]
    fn test_aggregate_order_activity_no_u64_saturation_past_u64_max() {
        // #1059: the u128 widening must be genuinely exercised — a bucket whose
        // summed volume exceeds u64::MAX (~18.4M BTH) must return the EXACT
        // total, not a saturated `u64::MAX`. A u64 `saturating_add` accumulator
        // would have silently clamped here (wrong-but-plausible number).
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let now = Utc::now().timestamp();
        let big = i64::MAX as u64;
        // Three rows of i64::MAX sum to 3 * i64::MAX ≈ 2.77e19 > u64::MAX.
        db.insert_order(&mint_at(big, OrderStatus::Completed, now - 30))
            .unwrap();
        db.insert_order(&mint_at(big, OrderStatus::Completed, now - 20))
            .unwrap();
        db.insert_order(&mint_at(big, OrderStatus::Completed, now - 10))
            .unwrap();

        let all = db
            .aggregate_order_activity(OrderType::Mint, None)
            .expect("aggregation must not error");
        assert_eq!(all.completed.count, 3);
        let expected = 3u128 * i64::MAX as u128;
        assert_eq!(
            all.completed.volume, expected,
            "u128 sum must be exact, not saturated at u64::MAX"
        );
        assert!(
            expected > u64::MAX as u128,
            "test must cross the u64 ceiling to exercise the u128 widening"
        );
    }

    #[test]
    fn test_aggregate_order_activity_single_row_at_i64_max_boundary() {
        // #1059 edge case: exactly i64::MAX in a single row (the per-order
        // storage boundary) must round-trip through the aggregate unchanged —
        // not clamped, not errored.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();

        let now = Utc::now().timestamp();
        db.insert_order(&mint_at(i64::MAX as u64, OrderStatus::Completed, now - 10))
            .unwrap();

        let all = db.aggregate_order_activity(OrderType::Mint, None).unwrap();
        assert_eq!(all.completed.count, 1);
        assert_eq!(all.completed.volume, i64::MAX as u128);
    }
}
