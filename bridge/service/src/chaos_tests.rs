// Copyright (c) 2024 The Botho Foundation

//! Chaos / restart tests for the bridge order engine (#827 DoD).
//!
//! Every order transition is a SQLite commit boundary, so "kill -9 after
//! commit K" is exactly equivalent to "perform the writes of commits
//! 1..=K, then drop the engine and the DB handle". These tests drive
//! orders through the FULL lifecycle over a file-backed database, crash
//! the engine at EVERY durable state-transition boundary (by performing
//! the partial writes of that boundary and reconstructing the engine +
//! `Database` over the same file, via the real startup path including
//! `recover_on_startup`), and assert exactly-once outcomes:
//!
//! - the destination chain minted exactly ONE transaction per order (the mock
//!   chain enforces the #826 contract-side order-id guard and counts violation
//!   attempts);
//! - the BTH chain saw exactly ONE landed release per order (BTH has no
//!   on-chain guard — a second landed tx IS the double-release bug);
//! - every order reaches its terminal state (no stuck unrecoverable state),
//!   with the reserve ledger arithmetically exact.
//!
//! The mock chains are `Arc`-shared across "restarts": they are the
//! external world, which survives an engine crash.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use bth_bridge_core::{
    BridgeConfig, BridgeOrder, Chain, MintAuthorization, OrderStatus, ReleaseAuthorization,
};
use uuid::Uuid;

use crate::{
    attestation::{AttestationProvider, StubAttestationProvider},
    db::Database,
    engine::OrderProcessor,
    mint::{ConfirmationStatus, MintError, Minter, PreparedMint},
    release::{PreparedRelease, ReleaseConfirmation, ReleaseError, Releaser},
};

/// The external world: destination-chain and BTH-chain state that
/// survives engine crashes.
struct ChainWorld {
    /// order_id_hash -> the ONE landed mint tx (the #826 contract-side
    /// duplicate-order guard).
    minted: Mutex<HashMap<String, String>>,
    /// order_id_hash -> every DISTINCT landed release tx. BTH has no
    /// on-chain order-id guard, so a second entry is a real double
    /// release — the violation these tests hunt.
    released: Mutex<HashMap<String, Vec<String>>>,
    /// mint tx id -> order hash (set at signing time).
    mint_tx_orders: Mutex<HashMap<String, String>>,
    /// release tx hash -> order hash (set at signing time).
    release_tx_orders: Mutex<HashMap<String, String>>,
    /// Rejected duplicate-mint broadcasts (contract reverts). Must stay 0:
    /// the service-side guards should never even ATTEMPT one.
    double_mint_attempts: AtomicU32,
    seq: AtomicU32,
}

impl ChainWorld {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            minted: Mutex::new(HashMap::new()),
            released: Mutex::new(HashMap::new()),
            mint_tx_orders: Mutex::new(HashMap::new()),
            release_tx_orders: Mutex::new(HashMap::new()),
            double_mint_attempts: AtomicU32::new(0),
            seq: AtomicU32::new(0),
        })
    }

    fn mint_count(&self, order: &BridgeOrder) -> usize {
        let hash = hex::encode(order.order_id_bytes());
        self.minted.lock().unwrap().contains_key(&hash) as usize
    }

    fn release_count(&self, order: &BridgeOrder) -> usize {
        let hash = hex::encode(order.order_id_bytes());
        self.released
            .lock()
            .unwrap()
            .get(&hash)
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

/// Chaos minter: exactly-once at the chain level, auto-confirming once a
/// tx has landed.
///
/// `aggressive_drop` controls what an unlanded tx reports: the chaos
/// harness uses `true` (a never-broadcast tx is PROVABLY dropped in the
/// deterministic single-writer world, driving the unwind/resubmit path);
/// the concurrent load test uses `false` (`Pending`, mirroring the
/// production rule that an implementation must never report a
/// merely-unseen transaction as dropped — under concurrency another tick
/// may broadcast it a moment later).
struct ChaosMinter {
    world: Arc<ChainWorld>,
    prepare_calls: AtomicU32,
    aggressive_drop: bool,
}

impl ChaosMinter {
    async fn sign(&self, order: &BridgeOrder) -> PreparedMint {
        let auth = StubAttestationProvider.authorize_mint(order).await.unwrap();
        self.prepare_mint(order, &auth).await.unwrap()
    }
}

#[async_trait]
impl Minter for ChaosMinter {
    fn chain(&self) -> Chain {
        Chain::Ethereum
    }

    async fn prepare_mint(
        &self,
        order: &BridgeOrder,
        _auth: &MintAuthorization,
    ) -> Result<PreparedMint, MintError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        let tx_id = format!("0xtx{}", self.world.seq.fetch_add(1, Ordering::SeqCst));
        self.world
            .mint_tx_orders
            .lock()
            .unwrap()
            .insert(tx_id.clone(), hex::encode(order.order_id_bytes()));
        Ok(PreparedMint {
            tx_id,
            raw: vec![0xde],
        })
    }

    async fn broadcast(&self, prepared: &PreparedMint) -> Result<(), MintError> {
        let hash = self
            .world
            .mint_tx_orders
            .lock()
            .unwrap()
            .get(&prepared.tx_id)
            .cloned()
            .expect("broadcast of a never-signed tx");
        let mut minted = self.world.minted.lock().unwrap();
        match minted.get(&hash) {
            // Idempotent re-broadcast of the landed tx.
            Some(landed) if *landed == prepared.tx_id => Ok(()),
            // Contract-side order-id guard (#826): duplicate mint reverts.
            Some(_) => {
                self.world
                    .double_mint_attempts
                    .fetch_add(1, Ordering::SeqCst);
                Err(MintError::Rpc(
                    "execution reverted: order already minted".to_string(),
                ))
            }
            None => {
                minted.insert(hash, prepared.tx_id.clone());
                Ok(())
            }
        }
    }

    async fn check_confirmation(
        &self,
        order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ConfirmationStatus, MintError> {
        let hash = hex::encode(order.order_id_bytes());
        match self.world.minted.lock().unwrap().get(&hash) {
            Some(landed) if landed == dest_tx => Ok(ConfirmationStatus::Confirmed),
            _ if self.aggressive_drop => Ok(ConfirmationStatus::Reorged),
            _ => Ok(ConfirmationStatus::Pending { confirmations: 0 }),
        }
    }
}

/// Chaos releaser: every broadcast tx lands (BTH has NO on-chain guard),
/// auto-confirming a landed tx. See [`ChaosMinter`] for the
/// `aggressive_drop` semantics.
struct ChaosReleaser {
    world: Arc<ChainWorld>,
    prepare_calls: AtomicU32,
    aggressive_drop: bool,
}

impl ChaosReleaser {
    async fn sign(&self, order: &BridgeOrder) -> PreparedRelease {
        let auth = StubAttestationProvider
            .authorize_release(order)
            .await
            .unwrap();
        self.prepare_release(order, &auth).await.unwrap()
    }
}

#[async_trait]
impl Releaser for ChaosReleaser {
    async fn prepare_release(
        &self,
        order: &BridgeOrder,
        _auth: &ReleaseAuthorization,
    ) -> Result<PreparedRelease, ReleaseError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        let tx_hash = format!("bth_tx{}", self.world.seq.fetch_add(1, Ordering::SeqCst));
        self.world
            .release_tx_orders
            .lock()
            .unwrap()
            .insert(tx_hash.clone(), hex::encode(order.order_id_bytes()));
        Ok(PreparedRelease {
            tx_hash,
            raw: vec![0xca],
        })
    }

    async fn broadcast(&self, prepared: &PreparedRelease) -> Result<(), ReleaseError> {
        let hash = self
            .world
            .release_tx_orders
            .lock()
            .unwrap()
            .get(&prepared.tx_hash)
            .cloned()
            .expect("broadcast of a never-signed tx");
        let mut released = self.world.released.lock().unwrap();
        let landed = released.entry(hash).or_default();
        // Re-broadcast of the same tx is idempotent; a DIFFERENT tx for
        // the same order LANDS (no on-chain guard) — recording the double
        // release these tests assert never happens.
        if !landed.contains(&prepared.tx_hash) {
            landed.push(prepared.tx_hash.clone());
        }
        Ok(())
    }

    async fn check_confirmation(
        &self,
        order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ReleaseConfirmation, ReleaseError> {
        let hash = hex::encode(order.order_id_bytes());
        let landed = self
            .world
            .released
            .lock()
            .unwrap()
            .get(&hash)
            .map(|v| v.contains(&dest_tx.to_string()))
            .unwrap_or(false);
        if landed {
            Ok(ReleaseConfirmation::Confirmed)
        } else if self.aggressive_drop {
            // Never-broadcast tx: provably dead in the deterministic
            // single-writer world (its inputs were never committed
            // anywhere).
            Ok(ReleaseConfirmation::Dropped)
        } else {
            Ok(ReleaseConfirmation::Pending { confirmations: 0 })
        }
    }
}

/// One "deployment": a DB file plus the external chain world. `boot()`
/// simulates a process start (open DB, migrate, startup recovery);
/// dropping the returned handles simulates a crash at the last commit.
struct Harness {
    _dir: tempfile::TempDir,
    db_path: String,
    world: Arc<ChainWorld>,
    minter: Arc<ChaosMinter>,
    releaser: Arc<ChaosReleaser>,
    config: BridgeConfig,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir
            .path()
            .join("bridge-chaos.db")
            .to_string_lossy()
            .to_string();
        let world = ChainWorld::new();
        Self {
            db_path,
            minter: Arc::new(ChaosMinter {
                world: world.clone(),
                prepare_calls: AtomicU32::new(0),
                aggressive_drop: true,
            }),
            releaser: Arc::new(ChaosReleaser {
                world: world.clone(),
                prepare_calls: AtomicU32::new(0),
                aggressive_drop: true,
            }),
            world,
            config: BridgeConfig::default(),
            _dir: dir,
        }
    }

    fn boot(&self) -> (OrderProcessor, Database) {
        let db = Database::open(&self.db_path).unwrap();
        db.migrate().unwrap();
        let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
        minters.insert(Chain::Ethereum, self.minter.clone());
        let processor = OrderProcessor::new(
            self.config.clone(),
            db.clone(),
            minters,
            Some(self.releaser.clone()),
            Arc::new(StubAttestationProvider),
        );
        processor.recover_on_startup().unwrap();
        (processor, db)
    }
}

/// Tick until the order reaches a terminal state (bounded).
async fn settle(processor: &OrderProcessor, db: &Database, order_id: &Uuid) -> OrderStatus {
    for _ in 0..12 {
        let status = db.get_order(order_id).unwrap().unwrap().status;
        if status.is_terminal() {
            return status;
        }
        processor.process_pending_orders().await.unwrap();
    }
    db.get_order(order_id).unwrap().unwrap().status
}

fn mint_order() -> BridgeOrder {
    let mut order = BridgeOrder::new_mint(
        Chain::Ethereum,
        1_000_000_000_000,
        1_000_000_000,
        "bth_bridge_addr".to_string(),
        "0x1234567890abcdef1234567890abcdef12345678".to_string(),
    );
    order.set_status(OrderStatus::DepositConfirmed);
    order
}

fn burn_order() -> BridgeOrder {
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
    order
}

// === Mint-path crash points ===

/// Every durable boundary of the mint submit/confirm pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MintCrash {
    /// Baseline: crash before any engine write.
    BeforeAnything,
    /// After the reserve backing was locked, before the mint record.
    AfterReserveLock,
    /// After the signed tx was persisted to `mints`, before the status
    /// update and before broadcast.
    AfterMintRecorded,
    /// After the order advanced to `MintPending`, before broadcast.
    AfterStatusMintPending,
    /// After the tx was broadcast, before confirmation was recorded.
    AfterBroadcast,
    /// After `mark_mint_confirmed` (terminal): restart must be a no-op.
    AfterConfirmed,
}

async fn run_mint_chaos(point: MintCrash) {
    let harness = Harness::new();
    let order = mint_order();
    let hash = hex::encode(order.order_id_bytes());

    // Engine instance 1: perform the writes up to the crash point, in the
    // exact order the engine performs them, then crash.
    {
        let (_processor, db) = harness.boot();
        db.insert_order(&order).unwrap();

        if point >= MintCrash::AfterReserveLock {
            db.record_locked_output(
                &format!("dep:{}", order.id),
                order.dest_chain,
                order.net_amount(),
                &order.id,
            )
            .unwrap();
        }
        if point >= MintCrash::AfterMintRecorded {
            let prepared = harness.minter.sign(&order).await;
            db.record_mint_submitted(&order.id, &hash, order.dest_chain, &prepared.tx_id)
                .unwrap();
            if point >= MintCrash::AfterStatusMintPending {
                db.update_order_status(&order.id, &OrderStatus::MintPending, Some(&prepared.tx_id))
                    .unwrap();
            }
            if point >= MintCrash::AfterBroadcast {
                harness.minter.broadcast(&prepared).await.unwrap();
            }
            if point >= MintCrash::AfterConfirmed {
                db.mark_mint_confirmed(&order.id).unwrap();
            }
        }
        // Crash: processor + db handles dropped here.
    }

    // Engine instance 2: restart over the same file and settle.
    let (processor, db) = harness.boot();
    let status = settle(&processor, &db, &order.id).await;

    // Exactly-once outcomes, regardless of where the crash hit.
    assert_eq!(
        status,
        OrderStatus::Completed,
        "{:?}: order must settle after restart",
        point
    );
    assert_eq!(
        harness.world.mint_count(&order),
        1,
        "{:?}: the chain must see exactly one mint",
        point
    );
    assert_eq!(
        harness.world.double_mint_attempts.load(Ordering::SeqCst),
        0,
        "{:?}: no duplicate mint may even be attempted",
        point
    );
    let mint = db.get_mint_by_order(&order.id).unwrap().unwrap();
    assert!(mint.confirmed_at.is_some());
    assert_eq!(
        Some(mint.dest_tx),
        db.get_order(&order.id).unwrap().unwrap().dest_tx,
        "{:?}: recorded and order-visible tx must agree",
        point
    );
    // Reserve ledger exact: the completed mint's net backing is locked
    // exactly once.
    assert_eq!(
        db.locked_reserve_total().unwrap(),
        order.net_amount(),
        "{:?}: backing must be locked exactly once",
        point
    );
    // Further restarts + ticks change nothing (terminal is stable).
    drop((processor, db));
    let (processor, db) = harness.boot();
    processor.process_pending_orders().await.unwrap();
    assert_eq!(harness.world.mint_count(&order), 1);
    assert_eq!(
        db.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed
    );
}

#[tokio::test]
async fn chaos_mint_crash_at_every_transition_boundary() {
    for point in [
        MintCrash::BeforeAnything,
        MintCrash::AfterReserveLock,
        MintCrash::AfterMintRecorded,
        MintCrash::AfterStatusMintPending,
        MintCrash::AfterBroadcast,
        MintCrash::AfterConfirmed,
    ] {
        run_mint_chaos(point).await;
    }
}

// === Deposit-detection crash window (#843) ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DepositCrash {
    /// Between the detect write and the idempotency row (watcher step 1→2).
    AfterDetect,
    /// Between the idempotency row and the confirm write (step 2→4).
    AfterDetectMarked,
}

async fn run_deposit_chaos(point: DepositCrash) {
    let harness = Harness::new();
    let mut order = BridgeOrder::new_mint(
        Chain::Ethereum,
        1_000_000_000_000,
        1_000_000_000,
        "bth_bridge_addr".to_string(),
        "0x1234567890abcdef1234567890abcdef12345678".to_string(),
    );
    order.generate_memo();

    {
        let (_processor, db) = harness.boot();
        db.insert_order(&order).unwrap();
        assert!(db
            .record_deposit_detected(&order.id, "0xdeposit", order.amount)
            .unwrap());
        if point >= DepositCrash::AfterDetectMarked {
            db.mark_deposit_processed("0xdeposit", &order.id).unwrap();
        }
        // Crash before the confirm write: without recovery this order is
        // stranded at DepositDetected forever (#843).
    }

    let (processor, db) = harness.boot();
    // Startup recovery already ran inside boot(); both crash halves must
    // be closed.
    assert_eq!(
        db.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::DepositConfirmed,
        "{:?}: recovery must roll the stranded order forward",
        point
    );
    assert!(db.is_deposit_processed("0xdeposit").unwrap());

    let status = settle(&processor, &db, &order.id).await;
    assert_eq!(status, OrderStatus::Completed, "{:?}", point);
    assert_eq!(harness.world.mint_count(&order), 1, "{:?}", point);
    assert_eq!(db.count_audit_action("deposit_recovered").unwrap(), 1);
}

#[tokio::test]
async fn chaos_deposit_detected_crash_window_recovers() {
    run_deposit_chaos(DepositCrash::AfterDetect).await;
    run_deposit_chaos(DepositCrash::AfterDetectMarked).await;
}

// === Release-path crash points ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReleaseCrash {
    /// Baseline: crash before any engine write.
    BeforeAnything,
    /// After the durable claim, before signing.
    AfterClaim,
    /// After the signed tx (hash + raw bytes) was persisted, before the
    /// status update and before broadcast.
    AfterTxRecorded,
    /// After the order advanced to `ReleasePending`, before broadcast.
    AfterStatusReleasePending,
    /// After the tx was broadcast, before confirmation was recorded.
    AfterBroadcast,
    /// After the reserve spend was applied, before `mark_release_confirmed`.
    AfterReserveSpend,
}

/// Reserve seeded well above the burn amount.
const RELEASE_SEED: u64 = 5_000_000_000_000;

async fn run_release_chaos(point: ReleaseCrash) {
    let harness = Harness::new();
    let order = burn_order();
    let hash = hex::encode(order.order_id_bytes());

    {
        let (_processor, db) = harness.boot();
        db.record_locked_output("dep:seed", Chain::Ethereum, RELEASE_SEED, &Uuid::new_v4())
            .unwrap();
        db.insert_order(&order).unwrap();

        if point >= ReleaseCrash::AfterClaim {
            db.try_claim_release(&order.id, &hash).unwrap();
        }
        if point >= ReleaseCrash::AfterTxRecorded {
            let prepared = harness.releaser.sign(&order).await;
            db.record_release_tx(&order.id, &prepared.tx_hash, &prepared.raw)
                .unwrap();
            if point >= ReleaseCrash::AfterStatusReleasePending {
                db.update_order_status(
                    &order.id,
                    &OrderStatus::ReleasePending,
                    Some(&prepared.tx_hash),
                )
                .unwrap();
            }
            if point >= ReleaseCrash::AfterBroadcast {
                harness.releaser.broadcast(&prepared).await.unwrap();
            }
            if point >= ReleaseCrash::AfterReserveSpend {
                assert!(db
                    .apply_release_spend(&order.id, order.source_chain, order.amount)
                    .unwrap());
            }
        }
        // Crash.
    }

    let (processor, db) = harness.boot();
    let status = settle(&processor, &db, &order.id).await;

    assert_eq!(
        status,
        OrderStatus::Released,
        "{:?}: burn must settle after restart",
        point
    );
    assert_eq!(
        harness.world.release_count(&order),
        1,
        "{:?}: exactly one release may land on BTH (no on-chain guard!)",
        point
    );
    let claim = db.get_release_by_order(&order.id).unwrap().unwrap();
    assert!(claim.confirmed_at.is_some());
    assert_eq!(
        claim.release_tx_hash,
        db.get_order(&order.id).unwrap().unwrap().dest_tx,
        "{:?}: recorded and order-visible tx must agree",
        point
    );
    // Reserve ledger exact: the GROSS burn left the ledger exactly once.
    assert_eq!(
        db.locked_reserve_total().unwrap(),
        RELEASE_SEED - order.amount,
        "{:?}: reserve must be spent exactly once",
        point
    );

    // Further restarts + ticks change nothing.
    drop((processor, db));
    let (processor, db) = harness.boot();
    processor.process_pending_orders().await.unwrap();
    assert_eq!(harness.world.release_count(&order), 1);
    assert_eq!(
        db.locked_reserve_total().unwrap(),
        RELEASE_SEED - order.amount
    );
    assert_eq!(
        db.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Released
    );
}

#[tokio::test]
async fn chaos_release_crash_at_every_transition_boundary() {
    for point in [
        ReleaseCrash::BeforeAnything,
        ReleaseCrash::AfterClaim,
        ReleaseCrash::AfterTxRecorded,
        ReleaseCrash::AfterStatusReleasePending,
        ReleaseCrash::AfterBroadcast,
        ReleaseCrash::AfterReserveSpend,
    ] {
        run_release_chaos(point).await;
    }
}

/// Restart the engine after EVERY tick of a normal lifecycle (a rolling
/// crash-loop deployment) — both flows must still settle exactly once.
#[tokio::test]
async fn chaos_restart_after_every_tick() {
    let harness = Harness::new();
    let mint = mint_order();
    let burn = burn_order();

    {
        let (_processor, db) = harness.boot();
        db.record_locked_output("dep:seed", Chain::Ethereum, RELEASE_SEED, &Uuid::new_v4())
            .unwrap();
        db.insert_order(&mint).unwrap();
        db.insert_order(&burn).unwrap();
    }

    for _ in 0..8 {
        let (processor, db) = harness.boot();
        processor.process_pending_orders().await.unwrap();
        let m = db.get_order(&mint.id).unwrap().unwrap().status;
        let b = db.get_order(&burn.id).unwrap().unwrap().status;
        if m.is_terminal() && b.is_terminal() {
            break;
        }
        // Crash after every single tick.
    }

    let (_processor, db) = harness.boot();
    assert_eq!(
        db.get_order(&mint.id).unwrap().unwrap().status,
        OrderStatus::Completed
    );
    assert_eq!(
        db.get_order(&burn.id).unwrap().unwrap().status,
        OrderStatus::Released
    );
    assert_eq!(harness.world.mint_count(&mint), 1);
    assert_eq!(harness.world.release_count(&burn), 1);
    assert_eq!(harness.world.double_mint_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(
        db.locked_reserve_total().unwrap(),
        RELEASE_SEED + mint.net_amount() - burn.amount
    );
}

// === Load test: high-volume concurrent pipeline ===

/// Hundreds of concurrent orders through the mock pipeline with several
/// engine ticks running CONCURRENTLY over the shared connection: no
/// deadlocks, no lost orders, chain-level exactly-once throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_test_concurrent_orders_exactly_once() {
    const MINTS: usize = 120;
    const BURNS: usize = 120;
    const AMOUNT: u64 = 1_000_000_000_000; // 1 BTH

    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let world = ChainWorld::new();
    let minter = Arc::new(ChaosMinter {
        world: world.clone(),
        prepare_calls: AtomicU32::new(0),
        aggressive_drop: false,
    });
    let releaser = Arc::new(ChaosReleaser {
        world: world.clone(),
        prepare_calls: AtomicU32::new(0),
        aggressive_drop: false,
    });
    let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
    minters.insert(Chain::Ethereum, minter.clone());
    let processor = Arc::new(OrderProcessor::new(
        BridgeConfig::default(),
        db.clone(),
        minters,
        Some(releaser.clone()),
        Arc::new(StubAttestationProvider),
    ));

    // Seed reserve to cover every burn.
    db.record_locked_output(
        "dep:seed",
        Chain::Ethereum,
        AMOUNT * BURNS as u64,
        &Uuid::new_v4(),
    )
    .unwrap();

    let mut mint_ids = Vec::with_capacity(MINTS);
    for i in 0..MINTS {
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            AMOUNT,
            0,
            "bth_bridge_addr".to_string(),
            format!("0xuser{:040}", i), // distinct per-address windows
        );
        order.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&order).unwrap();
        mint_ids.push(order.id);
    }
    let mut burn_ids = Vec::with_capacity(BURNS);
    for i in 0..BURNS {
        let mut order = BridgeOrder::new_burn(
            Chain::Ethereum,
            AMOUNT,
            0,
            format!("0xburner{:038}", i),
            "bth_user_stealth_addr".to_string(),
            format!("0xburntx{}", i),
            0,
        );
        order.set_status(OrderStatus::BurnConfirmed);
        db.insert_order(&order).unwrap();
        burn_ids.push(order.id);
    }

    // 4 concurrent tick loops hammer the shared connection.
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let p = processor.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..6 {
                p.process_pending_orders().await.unwrap();
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    // Drain any stragglers single-threaded (bounded).
    for _ in 0..10 {
        if db.actionable_backlog().unwrap() == 0 {
            break;
        }
        processor.process_pending_orders().await.unwrap();
    }

    // No lost orders: every order reached its terminal state.
    for id in &mint_ids {
        let order = db.get_order(id).unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Completed, "mint {} lost", id);
        assert_eq!(world.mint_count(&order), 1, "mint {} not exactly-once", id);
    }
    for id in &burn_ids {
        let order = db.get_order(id).unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Released, "burn {} lost", id);
        assert_eq!(
            world.release_count(&order),
            1,
            "burn {} released a wrong number of times",
            id
        );
    }
    assert_eq!(
        world.double_mint_attempts.load(Ordering::SeqCst),
        0,
        "no duplicate mint may even be attempted under concurrency"
    );

    // Reserve ledger arithmetic holds under the full concurrent volume.
    assert_eq!(
        db.locked_reserve_total().unwrap(),
        AMOUNT * BURNS as u64 + AMOUNT * MINTS as u64 - AMOUNT * BURNS as u64,
        "locked reserve must equal seeded + minted-net - burned-gross"
    );
    assert_eq!(db.actionable_backlog().unwrap(), 0);
}
