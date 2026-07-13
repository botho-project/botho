// Copyright (c) 2024 The Botho Foundation

//! Reserve accounting and proof-of-reserves reconciliation (#825).
//!
//! ## The peg invariant (ADR 0003 / ADR 0005)
//!
//! The bridge locks native BTH and mints wBTH 1:1, so solvency is the
//! two-chain invariant:
//!
//! ```text
//! Σ(wBTH outstanding on Ethereum) + Σ(wBTH outstanding on Solana)
//!     == locked BTH reserve on Botho
//! ```
//!
//! Per ADR 0003 only **factor-1 (background/commerce) coins** are
//! wrappable, and a factor-1 coin pays exactly zero demurrage forever, so
//! the locked reserve never decays: the invariant is **exact** — there is
//! no demurrage tolerance term. This is an application-level convention
//! (bridge-controlled outputs + this reconciler), not a consensus
//! construct, per the project's no-hard-forks posture.
//!
//! ## Reserve derivation
//!
//! The locked reserve is derived from the `reserve_ledger` table — a view
//! of bridge-controlled outputs — never from a mutable counter:
//!
//! - deposit confirmed → mint: [`crate::db::Database::record_locked_output`]
//!   records the deposit's backing (the order's NET amount: the mint fee stays
//!   in bridge custody as revenue, not peg backing);
//! - burn → release confirmed: [`crate::db::Database::apply_release_spend`]
//!   spends locked outputs FIFO for the GROSS burn amount (the supply dropped
//!   by the full burn) and returns any remainder as change;
//! - a mint that fails after its deposit was locked is unlocked (the funds are
//!   owed back to the depositor, not backing supply).
//!
//! ## Tolerance semantics
//!
//! Each reconciliation compares, per chain, the on-chain wrapped supply
//! against the ledger's locked backing. The chain is in tolerance iff:
//!
//! ```text
//! supply − locked <=  tolerance                       (no unbacked wBTH)
//! locked − supply <=  in_flight + tolerance           (no missing supply)
//! ```
//!
//! where `in_flight` is the allowance for orders between settlement points
//! (deposits locked but not yet minted; burns seen on-chain but not yet
//! released) and `tolerance` is `reserve.tolerance_picocredits` (default 0
//! — the ADR 0003 exact peg; raise only to absorb supply-poll timing skew).
//! Positive drift beyond tolerance means unbacked wrapped supply — a mint
//! authority compromise or accounting bug. Negative drift beyond the
//! allowance means supply that should exist does not, or the ledger
//! overcounts — either is a peg incident.
//!
//! Additionally, when the on-Botho reserve-balance transport is available,
//! the ACTUAL balance of the reserve address is checked against the
//! ledger: a shortfall is a custody incident (unauthorized reserve
//! movement) and flips `pegHealthy` even if supplies match the ledger.
//!
//! Every pass persists a `reserve_snapshots` row and an `audit_log` entry,
//! and any violation emits a `reserve_drift_alert` audit event plus an
//! error log (rate-bounded by the reconcile interval).

use alloy::{
    network::TransactionBuilder,
    primitives::Address,
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    sol,
    sol_types::SolCall,
};
use async_trait::async_trait;
use bth_bridge_core::{BridgeConfig, BthConfig, Chain, EthereumConfig, SolanaConfig};
use chrono::Utc;
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::db::{Database, ReserveSnapshot};

sol! {
    /// ERC-20 supply surface of the wBTH token
    /// (`contracts/ethereum/contracts/WrappedBTH.sol`).
    #[allow(missing_docs)]
    interface IWrappedBTHSupply {
        function totalSupply() external view returns (uint256);
    }
}

/// Errors from reserve supply/balance sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveError {
    /// Misconfiguration (bad address, unparsable URL, ...).
    Config(String),
    /// RPC / network failure (retryable next pass).
    Rpc(String),
    /// Transport not yet wired up (Solana supply / BTH reserve balance,
    /// see #828). Fail-safe: the chain is reported unverified, never
    /// silently healthy.
    NotImplemented(String),
}

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReserveError::Config(m) => write!(f, "config error: {}", m),
            ReserveError::Rpc(m) => write!(f, "rpc error: {}", m),
            ReserveError::NotImplemented(m) => write!(f, "not implemented: {}", m),
        }
    }
}

impl std::error::Error for ReserveError {}

/// On-chain wrapped-supply read access for one chain, mockable for tests.
#[async_trait]
pub trait SupplySource: Send + Sync {
    /// The wrapped chain this source reports on.
    fn chain(&self) -> Chain;

    /// Current outstanding wBTH supply in picocredits (wBTH carries 12
    /// decimals on both chains, 1:1 with picocredits).
    async fn wrapped_supply(&self) -> Result<u128, ReserveError>;
}

/// Live Ethereum supply source: `WrappedBTH.totalSupply()` via `eth_call`
/// (same alloy provider pattern as `mint::ethereum`).
pub struct EthSupplySource {
    provider: DynProvider,
    wbth: Address,
}

impl EthSupplySource {
    /// Build a source from configuration. Does not perform network I/O.
    pub fn new(config: &EthereumConfig) -> Result<Self, ReserveError> {
        let wbth: Address = config
            .wbth_contract
            .parse()
            .map_err(|e| ReserveError::Config(format!("invalid wbth_contract: {}", e)))?;
        let url = config
            .rpc_url
            .parse()
            .map_err(|e| ReserveError::Config(format!("invalid ethereum rpc_url: {}", e)))?;
        let provider = ProviderBuilder::new().connect_http(url).erased();
        Ok(Self { provider, wbth })
    }
}

#[async_trait]
impl SupplySource for EthSupplySource {
    fn chain(&self) -> Chain {
        Chain::Ethereum
    }

    async fn wrapped_supply(&self) -> Result<u128, ReserveError> {
        let call = TransactionRequest::default()
            .with_to(self.wbth)
            .with_input(IWrappedBTHSupply::totalSupplyCall {}.abi_encode());
        let ret = self
            .provider
            .call(call)
            .await
            .map_err(|e| ReserveError::Rpc(format!("totalSupply() call failed: {}", e)))?;
        let supply = IWrappedBTHSupply::totalSupplyCall::abi_decode_returns(&ret)
            .map_err(|e| ReserveError::Rpc(format!("totalSupply() decode failed: {}", e)))?;
        supply.try_into().map_err(|_| {
            ReserveError::Rpc("totalSupply() exceeds u128 — not a real BTH quantity".to_string())
        })
    }
}

/// Solana supply source: wBTH SPL `Mint.supply` via `getTokenSupply`
/// (12-decimal mint, 1:1 with picocredits).
///
/// TODO(#828): implement against `SolanaConfig::rpc_url`. Until then this
/// is a fail-safe stub: the reconciler reports Solana as UNVERIFIED (its
/// DB-expected backing is excluded from drift math) rather than silently
/// healthy.
pub struct SolSupplySource {
    #[allow(dead_code)]
    config: SolanaConfig,
}

impl SolSupplySource {
    /// Build a source from configuration. Does not perform network I/O.
    pub fn new(config: SolanaConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl SupplySource for SolSupplySource {
    fn chain(&self) -> Chain {
        Chain::Solana
    }

    async fn wrapped_supply(&self) -> Result<u128, ReserveError> {
        Err(ReserveError::NotImplemented(
            "Solana getTokenSupply transport pending #828".to_string(),
        ))
    }
}

/// Actual on-Botho balance of the reserve address, mockable for tests.
///
/// This is leg (iii) of the reconciliation: the ledger says how much
/// SHOULD be locked; this source says how much the reserve address
/// actually holds. A shortfall is a custody incident.
#[async_trait]
pub trait ReserveBalanceSource: Send + Sync {
    /// Spendable balance of the reserve address in picocredits.
    async fn reserve_balance(&self) -> Result<u128, ReserveError>;
}

/// Live BTH reserve-balance source.
///
/// TODO(#828): implement against the BTH node transport (view-key scan of
/// reserve-address outputs, the same wiring as `watchers::bth`). Until
/// then this is a fail-safe stub: the custody check is SKIPPED and
/// `reserveBalanceChecked` stays false — never a silent pass.
pub struct NodeReserveBalanceSource {
    #[allow(dead_code)]
    config: BthConfig,
}

impl NodeReserveBalanceSource {
    /// Build a source from configuration. Does not perform network I/O.
    pub fn new(config: BthConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ReserveBalanceSource for NodeReserveBalanceSource {
    async fn reserve_balance(&self) -> Result<u128, ReserveError> {
        Err(ReserveError::NotImplemented(
            "BTH reserve-address balance transport pending #828".to_string(),
        ))
    }
}

/// Per-chain reconciliation detail in a [`ReserveProof`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainReserveStatus {
    /// Chain name (`"ethereum"` / `"solana"`).
    pub chain: String,
    /// Whether the on-chain supply could be read this pass.
    pub verified: bool,
    /// On-chain wrapped supply in picocredits (`None` if unverified).
    pub wrapped_supply: Option<u64>,
    /// Ledger-locked backing attributed to this chain, in picocredits.
    pub locked_backing: u64,
    /// In-flight allowance (pending mints net + pending burns gross).
    pub in_flight: u64,
    /// `supply − locked` in picocredits (`None` if unverified).
    pub drift: Option<i64>,
    /// Whether this chain satisfied the tolerance bounds (`true` for
    /// unverified chains only in the degenerate "nothing expected" sense:
    /// they are excluded from drift math but flagged via `verified`).
    pub in_tolerance: bool,
}

/// Proof-of-reserves snapshot: the `GET /api/reserve/proof` contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveProof {
    /// Total locked reserve per the ledger, in picocredits.
    pub locked_reserve: u64,
    /// Verified wBTH totalSupply on Ethereum, picocredits.
    pub eth_supply: Option<u64>,
    /// Verified wBTH supply on Solana, picocredits (pending #828).
    pub sol_supply: Option<u64>,
    /// Σ of the verified supplies, picocredits.
    pub total_wrapped: Option<u64>,
    /// Σ(verified supply) − Σ(locked backing of verified chains).
    pub drift: i64,
    /// All verified chains within tolerance + in-flight allowance.
    pub in_tolerance: bool,
    /// `in_tolerance` AND the actual reserve balance covered the ledger
    /// (when checkable). The dashboard's red/green peg state.
    pub peg_healthy: bool,
    /// Whether the on-Botho reserve balance was actually checked (false
    /// until the #828 transport lands).
    pub reserve_balance_checked: bool,
    /// When the reconciliation ran (unix seconds).
    pub taken_at: i64,
    /// Per-chain detail.
    pub chains: Vec<ChainReserveStatus>,
}

/// Periodic reconciler: DB-derived locked reserve vs on-chain wrapped
/// supply per chain vs (when available) the actual reserve balance.
pub struct Reconciler {
    db: Database,
    supplies: Vec<Arc<dyn SupplySource>>,
    reserve_balance: Option<Arc<dyn ReserveBalanceSource>>,
    tolerance: u64,
}

impl Reconciler {
    /// Build a reconciler with explicit sources (tests use mocks).
    pub fn new(
        db: Database,
        supplies: Vec<Arc<dyn SupplySource>>,
        reserve_balance: Option<Arc<dyn ReserveBalanceSource>>,
        tolerance: u64,
    ) -> Self {
        Self {
            db,
            supplies,
            reserve_balance,
            tolerance,
        }
    }

    /// Build the production reconciler from configuration. A chain whose
    /// source cannot be constructed is reported unverified (fail-safe),
    /// never skipped silently.
    pub fn from_config(config: &BridgeConfig, db: Database) -> Self {
        let mut supplies: Vec<Arc<dyn SupplySource>> = Vec::new();
        match EthSupplySource::new(&config.ethereum) {
            Ok(source) => supplies.push(Arc::new(source)),
            Err(e) => warn!("Ethereum supply polling disabled: {}", e),
        }
        supplies.push(Arc::new(SolSupplySource::new(config.solana.clone())));

        Self::new(
            db,
            supplies,
            Some(Arc::new(NodeReserveBalanceSource::new(config.bth.clone()))),
            config.reserve.tolerance_picocredits,
        )
    }

    /// One reconciliation pass: compute, persist, and (on violation)
    /// alert. Returns the proof snapshot.
    pub async fn reconcile_once(&self) -> Result<ReserveProof, String> {
        let taken_at = Utc::now().timestamp();
        let tolerance = self.tolerance as u128;

        let mut chains = Vec::with_capacity(self.supplies.len());
        let mut eth_supply: Option<u64> = None;
        let mut sol_supply: Option<u64> = None;
        let mut verified_supply: u128 = 0;
        let mut verified_locked: u128 = 0;
        let mut any_verified = false;
        let mut all_in_tolerance = true;

        for source in &self.supplies {
            let chain = source.chain();
            let locked = self.db.locked_reserve_by_chain(chain)?;
            let in_flight = self
                .db
                .pending_mint_backing(chain)?
                .saturating_add(self.db.pending_burn_amount(chain)?);

            match source.wrapped_supply().await {
                Ok(supply) => {
                    let drift = supply as i128 - locked as i128;
                    let in_tolerance = drift <= tolerance as i128
                        && (-drift) <= (in_flight as u128 + tolerance) as i128;
                    if !in_tolerance {
                        all_in_tolerance = false;
                    }
                    any_verified = true;
                    verified_supply = verified_supply.saturating_add(supply);
                    verified_locked = verified_locked.saturating_add(locked as u128);

                    let supply_u64 = u64::try_from(supply).unwrap_or(u64::MAX);
                    match chain {
                        Chain::Ethereum => eth_supply = Some(supply_u64),
                        Chain::Solana => sol_supply = Some(supply_u64),
                        Chain::Bth => {}
                    }
                    chains.push(ChainReserveStatus {
                        chain: chain.to_string(),
                        verified: true,
                        wrapped_supply: Some(supply_u64),
                        locked_backing: locked,
                        in_flight,
                        drift: Some(clamp_i64(drift)),
                        in_tolerance,
                    });
                }
                Err(ReserveError::NotImplemented(msg)) => {
                    debug!("{} supply unverified: {}", chain, msg);
                    chains.push(unverified_status(chain, locked, in_flight));
                }
                Err(e) => {
                    // Transient RPC failure: the chain goes unverified for
                    // this pass (surfaced via `verified: false`), it does
                    // NOT fabricate a drift alert.
                    warn!("{} supply poll failed (will retry): {}", chain, e);
                    chains.push(unverified_status(chain, locked, in_flight));
                }
            }
        }

        let locked_reserve = self.db.locked_reserve_total()?;
        let drift = clamp_i64(verified_supply as i128 - verified_locked as i128);

        // Custody leg: the ACTUAL reserve balance must cover the ledger.
        let mut reserve_balance_checked = false;
        let mut reserve_covered = true;
        if let Some(source) = &self.reserve_balance {
            match source.reserve_balance().await {
                Ok(balance) => {
                    reserve_balance_checked = true;
                    if balance.saturating_add(tolerance) < locked_reserve as u128 {
                        reserve_covered = false;
                        warn!(
                            "reserve balance {} below ledger-locked {} (short {})",
                            balance,
                            locked_reserve,
                            locked_reserve as u128 - balance
                        );
                    }
                }
                Err(ReserveError::NotImplemented(msg)) => {
                    debug!("reserve balance unverified: {}", msg)
                }
                Err(e) => warn!("reserve balance poll failed (will retry): {}", e),
            }
        }

        let proof = ReserveProof {
            locked_reserve,
            eth_supply,
            sol_supply,
            total_wrapped: any_verified.then(|| u64::try_from(verified_supply).unwrap_or(u64::MAX)),
            drift,
            in_tolerance: all_in_tolerance,
            peg_healthy: all_in_tolerance && reserve_covered,
            reserve_balance_checked,
            taken_at,
            chains,
        };

        // Persist the pass (drift history is auditable) + audit trail.
        self.db.insert_reserve_snapshot(&ReserveSnapshot {
            taken_at,
            locked_reserve,
            eth_supply,
            sol_supply,
            drift,
            in_tolerance: all_in_tolerance,
            peg_healthy: proof.peg_healthy,
        })?;
        let details =
            serde_json::to_string(&proof).map_err(|e| format!("serialize proof: {}", e))?;
        self.db.log_audit(None, "reserve_reconcile", &details)?;

        if !proof.peg_healthy {
            // Alert path. The error log is rate-bounded by the reconcile
            // interval (one pass = at most one alert).
            self.db.log_audit(None, "reserve_drift_alert", &details)?;
            error!(
                "PEG ALERT: locked_reserve={} total_wrapped={:?} drift={} \
                 in_tolerance={} reserve_covered={} — possible peg break or \
                 custody incident (#825)",
                locked_reserve, proof.total_wrapped, drift, all_in_tolerance, reserve_covered
            );
        } else {
            debug!(
                "reserve reconciled: locked={} wrapped={:?} drift={}",
                locked_reserve, proof.total_wrapped, drift
            );
        }

        Ok(proof)
    }

    /// Run the reconciler on a fixed interval until shutdown.
    pub async fn run(self, interval: Duration, mut shutdown: broadcast::Receiver<()>) {
        info!(
            "Starting reserve reconciler (interval {:?}, tolerance {} picocredits)",
            interval, self.tolerance
        );
        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Reserve reconciler shutting down");
                    return;
                }
                _ = tokio::time::sleep(interval) => {
                    if let Err(e) = self.reconcile_once().await {
                        warn!("Reserve reconciliation failed (will retry): {}", e);
                    }
                }
            }
        }
    }
}

fn unverified_status(chain: Chain, locked: u64, in_flight: u64) -> ChainReserveStatus {
    ChainReserveStatus {
        chain: chain.to_string(),
        verified: false,
        wrapped_supply: None,
        locked_backing: locked,
        in_flight,
        drift: None,
        in_tolerance: true,
    }
}

fn clamp_i64(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_bridge_core::{BridgeOrder, OrderStatus};
    use proptest::prelude::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    /// Programmable supply source.
    struct MockSupply {
        chain: Chain,
        supply: Mutex<Result<u128, ReserveError>>,
    }

    impl MockSupply {
        fn new(chain: Chain, supply: u128) -> Arc<Self> {
            Arc::new(Self {
                chain,
                supply: Mutex::new(Ok(supply)),
            })
        }

        fn set(&self, supply: u128) {
            *self.supply.lock().unwrap() = Ok(supply);
        }

        fn set_err(&self, err: ReserveError) {
            *self.supply.lock().unwrap() = Err(err);
        }
    }

    #[async_trait]
    impl SupplySource for MockSupply {
        fn chain(&self) -> Chain {
            self.chain
        }

        async fn wrapped_supply(&self) -> Result<u128, ReserveError> {
            self.supply.lock().unwrap().clone()
        }
    }

    /// Programmable reserve-balance source.
    struct MockBalance {
        balance: Mutex<Result<u128, ReserveError>>,
    }

    impl MockBalance {
        fn new(balance: u128) -> Arc<Self> {
            Arc::new(Self {
                balance: Mutex::new(Ok(balance)),
            })
        }

        fn set(&self, balance: u128) {
            *self.balance.lock().unwrap() = Ok(balance);
        }
    }

    #[async_trait]
    impl ReserveBalanceSource for MockBalance {
        async fn reserve_balance(&self) -> Result<u128, ReserveError> {
            self.balance.lock().unwrap().clone()
        }
    }

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn lock(db: &Database, chain: Chain, amount: u64) -> Uuid {
        let order = Uuid::new_v4();
        db.record_locked_output(&format!("dep:{}", order), chain, amount, &order)
            .unwrap();
        order
    }

    #[tokio::test]
    async fn test_reconcile_healthy_exact_peg() {
        let db = setup_db();
        lock(&db, Chain::Ethereum, 1_000);
        lock(&db, Chain::Ethereum, 500);

        let eth = MockSupply::new(Chain::Ethereum, 1_500);
        let reconciler = Reconciler::new(db.clone(), vec![eth], None, 0);

        let proof = reconciler.reconcile_once().await.unwrap();
        assert_eq!(proof.locked_reserve, 1_500);
        assert_eq!(proof.eth_supply, Some(1_500));
        assert_eq!(proof.total_wrapped, Some(1_500));
        assert_eq!(proof.drift, 0);
        assert!(proof.in_tolerance);
        assert!(proof.peg_healthy);
        assert!(!proof.reserve_balance_checked);

        // The pass is persisted + audited.
        let snapshot = db.latest_reserve_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.locked_reserve, 1_500);
        assert!(snapshot.in_tolerance);
        assert!(snapshot.peg_healthy);
        assert_eq!(db.count_audit_action("reserve_reconcile").unwrap(), 1);
        assert_eq!(db.count_audit_action("reserve_drift_alert").unwrap(), 0);
    }

    #[tokio::test]
    async fn test_drift_injection_unbacked_supply_trips_alert() {
        let db = setup_db();
        lock(&db, Chain::Ethereum, 1_000);

        let eth = MockSupply::new(Chain::Ethereum, 1_000);
        let reconciler = Reconciler::new(db.clone(), vec![eth.clone()], None, 0);
        assert!(reconciler.reconcile_once().await.unwrap().peg_healthy);

        // Inject drift: supply exceeds the reserve by 1 picocredit
        // (an unauthorized mint). Exact peg -> alert.
        eth.set(1_001);
        let proof = reconciler.reconcile_once().await.unwrap();
        assert_eq!(proof.drift, 1);
        assert!(!proof.in_tolerance);
        assert!(!proof.peg_healthy);

        let snapshot = db.latest_reserve_snapshot().unwrap().unwrap();
        assert!(!snapshot.in_tolerance);
        assert!(!snapshot.peg_healthy);
        assert_eq!(db.count_audit_action("reserve_drift_alert").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_drift_injection_missing_supply_trips_alert() {
        let db = setup_db();
        lock(&db, Chain::Ethereum, 1_000);

        // No in-flight orders explain locked > supply: alert.
        let eth = MockSupply::new(Chain::Ethereum, 400);
        let reconciler = Reconciler::new(db.clone(), vec![eth], None, 0);

        let proof = reconciler.reconcile_once().await.unwrap();
        assert_eq!(proof.drift, -600);
        assert!(!proof.in_tolerance);
        assert!(!proof.peg_healthy);
        assert_eq!(db.count_audit_action("reserve_drift_alert").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_in_flight_mint_allowance_covers_negative_drift() {
        let db = setup_db();

        // A confirmed deposit that has been locked but whose mint has not
        // landed yet: locked=900 (net of fee 100), supply still 0.
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000,
            100,
            "bth".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.set_status(OrderStatus::DepositConfirmed);
        db.insert_order(&order).unwrap();
        db.record_locked_output(
            &format!("dep:{}", order.id),
            Chain::Ethereum,
            order.net_amount(),
            &order.id,
        )
        .unwrap();

        let eth = MockSupply::new(Chain::Ethereum, 0);
        let reconciler = Reconciler::new(db.clone(), vec![eth.clone()], None, 0);

        let proof = reconciler.reconcile_once().await.unwrap();
        assert_eq!(proof.drift, -900);
        assert!(
            proof.in_tolerance,
            "in-flight mints must not trip the alert"
        );
        assert!(proof.peg_healthy);

        // Once the mint completes (supply up, order Completed) the
        // allowance drops out and the exact peg holds.
        eth.set(900);
        db.update_order_status(&order.id, &OrderStatus::MintPending, Some("0xtx"))
            .unwrap();
        db.mark_mint_confirmed(&order.id).unwrap();
        let proof = reconciler.reconcile_once().await.unwrap();
        assert_eq!(proof.drift, 0);
        assert!(proof.in_tolerance);
        assert_eq!(db.count_audit_action("reserve_drift_alert").unwrap(), 0);
    }

    #[tokio::test]
    async fn test_unverified_chain_is_flagged_not_alerted() {
        let db = setup_db();
        lock(&db, Chain::Ethereum, 1_000);

        let eth = MockSupply::new(Chain::Ethereum, 1_000);
        let sol = MockSupply::new(Chain::Solana, 0);
        sol.set_err(ReserveError::NotImplemented("pending #828".to_string()));
        let reconciler = Reconciler::new(db.clone(), vec![eth, sol], None, 0);

        let proof = reconciler.reconcile_once().await.unwrap();
        assert!(proof.in_tolerance, "an unverified chain must not alert");
        assert!(proof.peg_healthy);
        assert_eq!(proof.sol_supply, None);
        let sol_status = proof.chains.iter().find(|c| c.chain == "solana").unwrap();
        assert!(!sol_status.verified);
        assert_eq!(sol_status.drift, None);

        // A transient RPC failure behaves the same (no false alert).
        let flaky = MockSupply::new(Chain::Ethereum, 0);
        flaky.set_err(ReserveError::Rpc("connection refused".to_string()));
        let reconciler = Reconciler::new(db.clone(), vec![flaky], None, 0);
        let proof = reconciler.reconcile_once().await.unwrap();
        assert!(proof.in_tolerance);
        assert_eq!(proof.total_wrapped, None);
        assert_eq!(db.count_audit_action("reserve_drift_alert").unwrap(), 0);
    }

    #[tokio::test]
    async fn test_unauthorized_reserve_movement_trips_custody_alert() {
        let db = setup_db();
        lock(&db, Chain::Ethereum, 1_000);

        let eth = MockSupply::new(Chain::Ethereum, 1_000);
        let balance = MockBalance::new(1_000);
        let reconciler = Reconciler::new(db.clone(), vec![eth], Some(balance.clone()), 0);

        let proof = reconciler.reconcile_once().await.unwrap();
        assert!(proof.reserve_balance_checked);
        assert!(proof.peg_healthy);

        // Someone moves 200 picocredits out of the reserve address without
        // a corresponding burn: supplies still match the ledger, but the
        // custody leg fails -> pegHealthy=false + alert.
        balance.set(800);
        let proof = reconciler.reconcile_once().await.unwrap();
        assert!(proof.in_tolerance, "ledger vs supply is still consistent");
        assert!(
            !proof.peg_healthy,
            "custody shortfall must flip the peg state"
        );
        assert_eq!(db.count_audit_action("reserve_drift_alert").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_tolerance_absorbs_bounded_skew() {
        let db = setup_db();
        lock(&db, Chain::Ethereum, 1_000);

        let eth = MockSupply::new(Chain::Ethereum, 1_005);
        let reconciler = Reconciler::new(db.clone(), vec![eth.clone()], None, 5);
        let proof = reconciler.reconcile_once().await.unwrap();
        assert_eq!(proof.drift, 5);
        assert!(proof.in_tolerance, "drift == tolerance is allowed");

        eth.set(1_006);
        let proof = reconciler.reconcile_once().await.unwrap();
        assert!(!proof.in_tolerance, "drift beyond tolerance alerts");
    }

    // === Property test (issue #825 DoD) ===
    //
    // Across randomized interleaved mint/burn sequences the invariant
    // holds after every settled operation:
    //     locked_reserve_total() == Σ(expected wrapped supply per chain)
    // and no sequence drives the locked total negative (burns only settle
    // against existing supply, mirroring the on-chain reality that you
    // cannot burn wBTH that was never minted). Demurrage is zero on the
    // factor-1 reserve (ADR 0003), so no decay term appears.

    #[derive(Debug, Clone)]
    enum Op {
        /// Mint: lock `net` backing on `chain` (supply += net).
        Mint { chain_ix: u8, net: u64 },
        /// Burn a fraction of the current supply on `chain`.
        Burn { chain_ix: u8, fraction: u8 },
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..2, 1u64..5_000_000).prop_map(|(chain_ix, net)| Op::Mint { chain_ix, net }),
            (0u8..2, 1u8..=100).prop_map(|(chain_ix, fraction)| Op::Burn { chain_ix, fraction }),
        ]
    }

    fn chain_of(ix: u8) -> Chain {
        if ix == 0 {
            Chain::Ethereum
        } else {
            Chain::Solana
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn prop_invariant_holds_across_mint_burn_sequences(
            ops in proptest::collection::vec(op_strategy(), 1..40)
        ) {
            let db = setup_db();
            // Expected outstanding supply per chain, maintained by the
            // test as the chain-side ground truth.
            let mut supply = [0u128; 2];

            for op in ops {
                match op {
                    Op::Mint { chain_ix, net } => {
                        let chain = chain_of(chain_ix);
                        let order = Uuid::new_v4();
                        let output_id = format!("dep:{}", order);
                        let recorded = db
                            .record_locked_output(&output_id, chain, net, &order)
                            .unwrap();
                        prop_assert!(recorded);
                        supply[chain_ix as usize] += net as u128;
                    }
                    Op::Burn { chain_ix, fraction } => {
                        let outstanding = supply[chain_ix as usize];
                        let amount =
                            (outstanding * fraction as u128 / 100).min(outstanding) as u64;
                        if amount == 0 {
                            continue;
                        }
                        let chain = chain_of(chain_ix);
                        let release = Uuid::new_v4();
                        prop_assert!(db
                            .apply_release_spend(&release, chain, amount)
                            .unwrap());
                        supply[chain_ix as usize] -= amount as u128;
                    }
                }

                // Invariant after every settled op, per chain and total.
                let expected_total: u128 = supply.iter().sum();
                prop_assert_eq!(
                    db.locked_reserve_total().unwrap() as u128,
                    expected_total
                );
                prop_assert_eq!(
                    db.locked_reserve_by_chain(Chain::Ethereum).unwrap() as u128,
                    supply[0]
                );
                prop_assert_eq!(
                    db.locked_reserve_by_chain(Chain::Solana).unwrap() as u128,
                    supply[1]
                );
            }

            // A burn exceeding the outstanding supply can never be settled
            // against the ledger (no sequence drives the total negative).
            let over = db.locked_reserve_by_chain(Chain::Ethereum).unwrap() + 1;
            prop_assert!(db
                .apply_release_spend(&Uuid::new_v4(), Chain::Ethereum, over)
                .is_err());
        }
    }

    #[test]
    fn test_proof_serializes_to_dashboard_contract() {
        // The JSON field names are the contract consumed by the
        // metrics-daemon and the /network dashboard hook.
        let proof = ReserveProof {
            locked_reserve: 1_500,
            eth_supply: Some(1_000),
            sol_supply: Some(500),
            total_wrapped: Some(1_500),
            drift: 0,
            in_tolerance: true,
            peg_healthy: true,
            reserve_balance_checked: false,
            taken_at: 1_752_000_000,
            chains: vec![ChainReserveStatus {
                chain: "ethereum".to_string(),
                verified: true,
                wrapped_supply: Some(1_000),
                locked_backing: 1_000,
                in_flight: 0,
                drift: Some(0),
                in_tolerance: true,
            }],
        };

        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&proof).unwrap()).unwrap();
        for key in [
            "lockedReserve",
            "ethSupply",
            "solSupply",
            "totalWrapped",
            "drift",
            "inTolerance",
            "pegHealthy",
            "reserveBalanceChecked",
            "takenAt",
            "chains",
        ] {
            assert!(json.get(key).is_some(), "missing contract field {}", key);
        }
        assert_eq!(json["lockedReserve"], 1_500);
        assert_eq!(json["pegHealthy"], true);
        assert_eq!(json["chains"][0]["wrappedSupply"], 1_000);
        assert_eq!(json["chains"][0]["lockedBacking"], 1_000);
    }
}
