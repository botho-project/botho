// Copyright (c) 2024 The Botho Foundation

//! Public, browser-reachable bridge order API (#1036, epic #1029).
//!
//! ─── Why this is a SEPARATE surface from `api.rs` ──────────────────────────
//! The operational API (`api::router`, bound via `reserve.api_listen`) co-hosts
//! `POST /api/breaker` — an UNAUTHENTICATED pause/RESUME kill switch —
//! alongside `/api/status`, `/metrics`, and `/api/reserve/proof`. Its only
//! defense is network placement (loopback by default). It must NEVER be exposed
//! to browsers, because anyone who can reach it can defeat a fail-closed
//! auto-trip during a peg incident.
//!
//! The wallet export/unwrap flows, however, need a browser-reachable surface to
//! open a mint order and poll its status. So this module stands up a SECOND,
//! independent router on its OWN listener (`public_api.listen`) that serves
//! ONLY the user-facing order endpoints and NOTHING operational:
//!
//!   - `POST /api/bridge/orders`              → open a mint order (BTH → wBTH)
//!   - `GET  /api/bridge/orders/{id}`         → mint order status
//!   - `POST /api/bridge/release-orders`      → register an unwrap intent
//!   - `GET  /api/bridge/release-orders/{id}` → release order status
//!   - `GET  /health`                         → static liveness (leaks nothing)
//!
//! There is deliberately no code path from this router to the breaker, the
//! ops status snapshot, the Prometheus surface, or reserve-proof control.
//! `test_public_router_does_not_serve_ops_endpoints` pins that invariant.
//!
//! ─── Threat model ─────────────────────────────────────────────────────────
//! This is a MINTING surface (i.e. money) reachable from the open internet, so:
//!   * Input validation — chain enum, per-chain address format, u64 amount,
//!     per-order cap, amount-above-fee — rejects malformed/abusive requests
//!     before any DB write. Order-create only ever produces an
//!     `AwaitingDeposit` order; no value moves until a real BTH deposit is
//!     confirmed by the watcher, and the federation-side daily caps + circuit
//!     breaker (enforced by the engine at mint time) still gate settlement.
//!   * CORS is an exact-match allow-list (no wildcards): a browser on an
//!     un-listed origin is refused the `Access-Control-Allow-Origin` header.
//!   * Per-IP rate limiting (a tight cap on order-create specifically) plus a
//!     request body-size cap blunt spam/flooding.
//!   * A GLOBAL order-create ceiling (`public_api.max_open_orders`, #1042)
//!     backstops the per-IP limits against distributed spam, and the engine
//!     loop prunes expired/abandoned create residue so the DB stays bounded.
//!   * The kill switch and ops detail are simply not routed here.
//!
//! ─── Information-exposure scope (#1042) ────────────────────────────────────
//! These endpoints are UNAUTHENTICATED, so everything they return must be
//! either (a) data the caller supplied, or (b) derivable from PUBLIC on-chain
//! state. Today that holds:
//!   * The mint GET returns only mint-order fields the creator supplied plus
//!     the reserve deposit address (published) and tx hashes (on-chain).
//!   * The release-intent GET echoes the caller-registered intent and
//!     correlates it to a watcher-created burn order by `(bthAddress, amount)`.
//!     Anyone can register an intent for an arbitrary pair and poll it to learn
//!     whether a matching burn exists and its status/tx hashes — ALL of which
//!     is already public on the counterparty chain and the BTH chain, so no
//!     confidentiality is lost.
//!
//! Any future field added to these responses must preserve that invariant:
//! never surface operator/ops state (pause reason, backlog, reserve detail),
//! internal error strings, or anything not reconstructible from public
//! chain data. Ops detail belongs on the loopback `api.rs` surface only.
//!
//! CORS + IP rate limiting assume the bind is either direct or behind a
//! trusted reverse proxy that preserves the peer address; behind an untrusted
//! proxy the per-IP limits collapse to per-proxy, which the operator must
//! account for (documented in `PublicApiSettings`).

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bth_bridge_core::{BridgeConfig, BridgeOrder, Chain, ChainAddress, OrderStatus, OrderType};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use crate::db::{Database, ReleaseIntent};

/// Runtime configuration snapshot for the public order API, derived from
/// [`BridgeConfig`] at startup. Held behind an `Arc` in [`PublicApiState`].
#[derive(Debug, Clone)]
pub struct PublicApiConfig {
    /// BTH reserve deposit address returned to the wallet for mint orders.
    /// `None` disables order-create (the API cannot tell users where to send
    /// the deposit).
    deposit_address: Option<String>,
    /// wBTH ERC-20 contract address (Ethereum unwrap `tokenAddress`).
    eth_token_address: String,
    /// wBTH program/mint address (Solana unwrap `tokenAddress`).
    sol_token_address: String,
    /// Bridge fee, basis points.
    fee_bps: u32,
    /// Minimum bridge fee, picocredits.
    min_fee: u64,
    /// Per-order maximum amount, picocredits.
    max_order_amount: u64,
    /// Configured minimum order amount, picocredits (0 = "just above fee").
    min_order_amount: u64,
    /// Order expiry, minutes (drives `expiresAt`).
    order_expiry_minutes: i64,
    /// Exact-match CORS origin allow-list.
    cors_allowed_origins: Vec<String>,
    /// Per-IP order-create cap per 60s window (0 = unlimited).
    create_rate_limit_per_min: u32,
    /// Per-IP general request cap per 60s window (0 = unlimited).
    rate_limit_per_min: u32,
    /// Max request body size, bytes.
    max_body_bytes: usize,
    /// Global ceiling on outstanding order-create records (0 = unlimited).
    /// See [`bth_bridge_core::PublicApiSettings::max_open_orders`] (#1042).
    max_open_orders: u64,
}

impl PublicApiConfig {
    /// Build the public-API runtime config from the full bridge config.
    pub fn from_config(config: &BridgeConfig) -> Self {
        Self {
            deposit_address: config.bth.reserve_address.clone(),
            eth_token_address: config.ethereum.wbth_contract.clone(),
            sol_token_address: config.solana.wbth_program.clone(),
            fee_bps: config.bridge.fee_bps,
            min_fee: config.bridge.min_fee,
            max_order_amount: config.bridge.max_order_amount,
            min_order_amount: config.public_api.min_order_amount,
            order_expiry_minutes: config.bridge.order_expiry_minutes,
            cors_allowed_origins: config.public_api.cors_allowed_origins.clone(),
            create_rate_limit_per_min: config.public_api.create_rate_limit_per_min,
            rate_limit_per_min: config.public_api.rate_limit_per_min,
            max_body_bytes: config.public_api.max_body_bytes.max(1),
            max_open_orders: config.public_api.max_open_orders,
        }
    }

    /// Bridge fee for an amount (matches [`BridgeConfig::calculate_fee`]).
    fn calculate_fee(&self, amount: u64) -> u64 {
        let percentage_fee = (amount as u128 * self.fee_bps as u128 / 10_000) as u64;
        percentage_fee.max(self.min_fee)
    }

    /// The wBTH token/mint address for a wrapped chain.
    fn token_address(&self, chain: Chain) -> String {
        match chain {
            Chain::Ethereum => self.eth_token_address.clone(),
            Chain::Solana => self.sol_token_address.clone(),
            Chain::Bth => String::new(),
        }
    }
}

/// Shared state of the public API router.
#[derive(Clone)]
pub struct PublicApiState {
    /// Bridge database (orders + release intents).
    db: Database,
    /// Runtime config snapshot.
    cfg: Arc<PublicApiConfig>,
    /// Per-IP fixed-window rate limiter.
    limiter: Arc<RateLimiter>,
}

impl PublicApiState {
    /// Construct the public API state from a database handle and config.
    pub fn new(db: Database, cfg: PublicApiConfig) -> Self {
        Self {
            db,
            cfg: Arc::new(cfg),
            limiter: Arc::new(RateLimiter::new()),
        }
    }
}

// ────────────────────────────── Rate limiting ──────────────────────────────

/// A minimal per-key fixed-window counter (no external dependency). Keys are
/// `"<ip>|<bucket>"`, so order-create and status polls are capped separately.
struct RateLimiter {
    windows: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Record a hit for `key` and return `true` if it is within `limit` for the
    /// current 60-second window. `limit == 0` disables the cap.
    fn check(&self, key: &str, limit: u32) -> bool {
        if limit == 0 {
            return true;
        }
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut map = self.windows.lock().unwrap_or_else(PoisonError::into_inner);

        // Opportunistically prune stale entries so the map cannot grow without
        // bound under churn of distinct client IPs.
        if map.len() > 10_000 {
            map.retain(|_, (start, _)| now.duration_since(*start) < window);
        }

        let entry = map.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        if entry.1 >= limit {
            return false;
        }
        entry.1 += 1;
        true
    }
}

fn rl_key(peer: SocketAddr, bucket: &str) -> String {
    format!("{}|{}", peer.ip(), bucket)
}

// ─────────────────────────────── Wire types ────────────────────────────────

/// Destination chain a mint order can target (strict subset of [`Chain`]).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WrappedChain {
    Ethereum,
    Solana,
}

impl WrappedChain {
    fn to_chain(self) -> Chain {
        match self {
            WrappedChain::Ethereum => Chain::Ethereum,
            WrappedChain::Solana => Chain::Solana,
        }
    }
}

/// `POST /api/bridge/orders` request body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMintOrderRequest {
    /// Chain wBTH is minted on.
    dest_chain: WrappedChain,
    /// The user's OWN address on `dest_chain` — where wBTH lands.
    dest_address: String,
    /// Gross BTH to lock, picocredits, as a base-10 string (u64-safe).
    amount: String,
}

/// `POST /api/bridge/release-orders` request body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateReleaseOrderRequest {
    /// Chain the user burns wBTH on.
    source_chain: WrappedChain,
    /// Botho address the released BTH should land at.
    bth_address: String,
    /// Gross wBTH to burn, picocredits, as a base-10 string (u64-safe).
    amount: String,
}

/// A mint order as returned to the wallet. Field names + status strings match
/// `web/packages/features/src/bridge/types.ts` (`MintOrder`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MintOrderResponse {
    id: String,
    status: String,
    dest_chain: String,
    dest_address: String,
    amount: String,
    fee: String,
    deposit_address: String,
    memo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dest_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<String>,
}

/// A release order as returned to the wallet. Field names + status strings
/// match `types.ts` (`ReleaseOrder`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseOrderResponse {
    id: String,
    status: String,
    source_chain: String,
    bth_address: String,
    amount: String,
    fee: String,
    token_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dest_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<String>,
}

/// Error envelope; the web client surfaces `error` verbatim.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ErrorBody { error: msg.into() })).into_response()
}

/// The bare, wire-stable status tag for an order.
///
/// This is the #1036 `failed`-status normalization: the core
/// `OrderStatus::Display` renders `Failed{reason}` as `"failed: <reason>"` and
/// serde renders it as the struct-variant `{"failed":{"reason":…}}` — neither
/// matches the TS contract, which models a bare `status: "failed"` plus a
/// separate `failureReason`. The public API therefore emits this bare tag and
/// carries the reason in `failureReason`.
fn status_tag(status: &OrderStatus) -> &'static str {
    match status {
        OrderStatus::AwaitingDeposit => "awaiting_deposit",
        OrderStatus::DepositDetected => "deposit_detected",
        OrderStatus::DepositConfirmed => "deposit_confirmed",
        OrderStatus::MintPending => "mint_pending",
        OrderStatus::Completed => "completed",
        OrderStatus::BurnDetected => "burn_detected",
        OrderStatus::BurnConfirmed => "burn_confirmed",
        OrderStatus::ReleasePending => "release_pending",
        OrderStatus::Released => "released",
        OrderStatus::Failed { .. } => "failed",
        OrderStatus::Expired => "expired",
    }
}

fn failure_reason(order: &BridgeOrder) -> Option<String> {
    match &order.status {
        OrderStatus::Failed { reason } => Some(reason.clone()),
        _ => None,
    }
}

fn mint_response(order: &BridgeOrder, expiry_minutes: i64) -> MintOrderResponse {
    let expires_at = order
        .created_at
        .timestamp()
        .saturating_add(expiry_minutes.max(0).saturating_mul(60));
    MintOrderResponse {
        id: order.id.to_string(),
        status: status_tag(&order.status).to_string(),
        dest_chain: order.dest_chain.to_string(),
        dest_address: order.dest_address.clone(),
        amount: order.amount.to_string(),
        fee: order.fee.to_string(),
        deposit_address: order.source_address.clone(),
        memo: order.memo.map(hex::encode).unwrap_or_default(),
        dest_tx: order.dest_tx.clone(),
        expires_at: Some(expires_at),
        failure_reason: failure_reason(order),
    }
}

fn release_response_from_intent(intent: &ReleaseIntent, status: &str) -> ReleaseOrderResponse {
    ReleaseOrderResponse {
        id: intent.id.to_string(),
        status: status.to_string(),
        source_chain: intent.source_chain.to_string(),
        bth_address: intent.bth_address.clone(),
        amount: intent.amount.to_string(),
        fee: intent.fee.to_string(),
        token_address: intent.token_address.clone(),
        source_tx: None,
        dest_tx: None,
        expires_at: Some(intent.expires_at),
        failure_reason: None,
    }
}

fn release_response_from_order(
    intent: &ReleaseIntent,
    order: &BridgeOrder,
) -> ReleaseOrderResponse {
    ReleaseOrderResponse {
        id: intent.id.to_string(),
        status: status_tag(&order.status).to_string(),
        source_chain: intent.source_chain.to_string(),
        bth_address: intent.bth_address.clone(),
        amount: intent.amount.to_string(),
        fee: intent.fee.to_string(),
        token_address: intent.token_address.clone(),
        // The burn tx is the order's SOURCE tx; the BTH release tx is its
        // DEST tx.
        source_tx: order.source_tx.clone(),
        dest_tx: order.dest_tx.clone(),
        expires_at: Some(intent.expires_at),
        failure_reason: failure_reason(order),
    }
}

// ──────────────────────────────── Handlers ─────────────────────────────────

/// Static liveness for load-balancer probes. Deliberately leaks nothing (no
/// pause state, no DB read) — unlike the ops `/health`.
async fn public_health() -> Response {
    (StatusCode::OK, "OK").into_response()
}

/// `POST /api/bridge/orders`: open a mint order (BTH → wBTH).
async fn create_mint_order(
    State(state): State<PublicApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    payload: Result<Json<CreateMintOrderRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !state
        .limiter
        .check(&rl_key(peer, "create"), state.cfg.create_rate_limit_per_min)
    {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded; retry shortly",
        );
    }

    let Json(req) = match payload {
        Ok(j) => j,
        Err(rej) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("invalid request body: {}", rej.body_text()),
            )
        }
    };

    let Some(deposit_address) = state.cfg.deposit_address.clone() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "bridge deposit address not configured; order creation unavailable",
        );
    };

    let chain = req.dest_chain.to_chain();
    if let Err(e) = ChainAddress::new(chain, req.dest_address.clone()).validate() {
        return err(
            StatusCode::BAD_REQUEST,
            format!("invalid destination address: {}", e),
        );
    }

    let amount = match req.amount.parse::<u64>() {
        Ok(a) => a,
        Err(_) => {
            return err(
                StatusCode::BAD_REQUEST,
                "amount must be a base-10 picocredit integer",
            )
        }
    };

    if amount > state.cfg.max_order_amount {
        return err(
            StatusCode::BAD_REQUEST,
            "amount exceeds the per-order maximum",
        );
    }

    let fee = state.cfg.calculate_fee(amount);
    let minimum = state.cfg.min_order_amount.max(fee.saturating_add(1));
    if amount < minimum {
        return err(
            StatusCode::BAD_REQUEST,
            "amount below the minimum (must exceed the bridge fee)",
        );
    }

    // GLOBAL create ceiling (#1042): the per-IP limiter bounds per-client
    // rate, but a distributed source could otherwise grow the DB without
    // bound. The engine loop expires and prunes abandoned orders, so a full
    // backlog drains on its own.
    if state.cfg.max_open_orders > 0 {
        match state.db.count_awaiting_deposit_mint_orders() {
            Ok(open) if open >= state.cfg.max_open_orders => {
                warn!(
                    "public api: mint order-create refused: {} open orders >= global cap {}",
                    open, state.cfg.max_open_orders
                );
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "order backlog is full; retry later",
                );
            }
            Ok(_) => {}
            Err(e) => {
                warn!("public api: open-order count failed: {}", e);
                return err(StatusCode::INTERNAL_SERVER_ERROR, "failed to create order");
            }
        }
    }

    let mut order = BridgeOrder::new_mint(
        chain,
        amount,
        fee,
        deposit_address,
        req.dest_address.clone(),
    );
    order.generate_memo();

    if let Err(e) = state.db.insert_order(&order) {
        warn!("public api: failed to persist mint order: {}", e);
        return err(StatusCode::INTERNAL_SERVER_ERROR, "failed to create order");
    }
    if let Err(e) = state.db.log_audit(
        Some(&order.id),
        "order_created",
        &format!("public API mint order: dest={} amount={}", chain, amount),
    ) {
        warn!("public api: audit log failed: {}", e);
    }

    (
        StatusCode::OK,
        Json(mint_response(&order, state.cfg.order_expiry_minutes)),
    )
        .into_response()
}

/// `GET /api/bridge/orders/{id}`: mint order status.
async fn get_mint_order(
    State(state): State<PublicApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> Response {
    if !state
        .limiter
        .check(&rl_key(peer, "get"), state.cfg.rate_limit_per_min)
    {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded; retry shortly",
        );
    }

    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => return err(StatusCode::BAD_REQUEST, "invalid order id"),
    };

    match state.db.get_order(&uuid) {
        // Only ever serve MINT orders here — burn orders are never leaked
        // through the mint endpoint.
        Ok(Some(order)) if order.order_type == OrderType::Mint => (
            StatusCode::OK,
            Json(mint_response(&order, state.cfg.order_expiry_minutes)),
        )
            .into_response(),
        Ok(_) => err(StatusCode::NOT_FOUND, "order not found"),
        Err(e) => {
            warn!("public api: get order failed: {}", e);
            err(StatusCode::INTERNAL_SERVER_ERROR, "order lookup failed")
        }
    }
}

/// `POST /api/bridge/release-orders`: register an unwrap (wBTH → BTH) intent.
///
/// The burn happens in the user's OWN counterparty wallet and is
/// self-describing (`bridgeBurn(amount, bthAddress)`); the release is driven by
/// the watcher regardless of this call. Registration only stores a
/// non-custodial tracking record so the wallet gets a pollable UUID that the
/// status endpoint later correlates to the watcher-created burn order.
async fn create_release_order(
    State(state): State<PublicApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    payload: Result<Json<CreateReleaseOrderRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !state
        .limiter
        .check(&rl_key(peer, "create"), state.cfg.create_rate_limit_per_min)
    {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded; retry shortly",
        );
    }

    let Json(req) = match payload {
        Ok(j) => j,
        Err(rej) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("invalid request body: {}", rej.body_text()),
            )
        }
    };

    let chain = req.source_chain.to_chain();
    if let Err(e) = ChainAddress::new(Chain::Bth, req.bth_address.clone()).validate() {
        return err(
            StatusCode::BAD_REQUEST,
            format!("invalid BTH release address: {}", e),
        );
    }

    let amount = match req.amount.parse::<u64>() {
        Ok(a) => a,
        Err(_) => {
            return err(
                StatusCode::BAD_REQUEST,
                "amount must be a base-10 picocredit integer",
            )
        }
    };

    if amount > state.cfg.max_order_amount {
        return err(
            StatusCode::BAD_REQUEST,
            "amount exceeds the per-order maximum",
        );
    }

    let fee = state.cfg.calculate_fee(amount);
    let minimum = state.cfg.min_order_amount.max(fee.saturating_add(1));
    if amount < minimum {
        return err(
            StatusCode::BAD_REQUEST,
            "amount below the minimum (must exceed the bridge fee)",
        );
    }

    let now = Utc::now().timestamp();

    // GLOBAL create ceiling (#1042), mirroring the mint path: unexpired
    // intents are counted; expired ones stop counting immediately and are
    // pruned by the engine loop after a retention window.
    if state.cfg.max_open_orders > 0 {
        match state.db.count_active_release_intents(now) {
            Ok(open) if open >= state.cfg.max_open_orders => {
                warn!(
                    "public api: release-intent create refused: {} active intents >= global cap {}",
                    open, state.cfg.max_open_orders
                );
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "release-order backlog is full; retry later",
                );
            }
            Ok(_) => {}
            Err(e) => {
                warn!("public api: active-intent count failed: {}", e);
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to register release order",
                );
            }
        }
    }

    let expires_at = now.saturating_add(state.cfg.order_expiry_minutes.max(0).saturating_mul(60));
    let intent = ReleaseIntent {
        id: Uuid::new_v4(),
        source_chain: chain,
        bth_address: req.bth_address.clone(),
        amount,
        fee,
        token_address: state.cfg.token_address(chain),
        created_at: now,
        expires_at,
    };

    if let Err(e) = state.db.insert_release_intent(&intent) {
        warn!("public api: failed to persist release intent: {}", e);
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to register release order",
        );
    }
    // Audit-log symmetry with mint create (#1042). Intents are not
    // `bridge_orders` rows, so the id travels in the details instead of the
    // (FK-referencing) order_id column.
    if let Err(e) = state.db.log_audit(
        None,
        "release_intent_created",
        &format!(
            "public API release intent {}: source={} amount={}",
            intent.id, chain, amount
        ),
    ) {
        warn!("public api: audit log failed: {}", e);
    }

    (
        StatusCode::OK,
        Json(release_response_from_intent(&intent, "awaiting_burn")),
    )
        .into_response()
}

/// `GET /api/bridge/release-orders/{id}`: release order status.
///
/// Information-exposure scope (#1042): this endpoint returns ONLY
///   * the caller-registered intent fields echoed back (chain, bthAddress,
///     amount, fee, token address, expiry), and
///   * when a burn order correlates by `(bthAddress, amount)`: its status tag,
///     burn tx hash (`sourceTx`), release tx hash (`destTx`), and failure
///     reason — all reconstructible from public on-chain data.
///
/// Because intents are unauthenticated, anyone can register an arbitrary
/// `(bthAddress, amount)` pair and poll this endpoint; that reveals nothing
/// non-public (burns and releases are public on both chains). Keep it that
/// way: never add operator/ops state, internal errors, or per-user data
/// here — see the module-level "Information-exposure scope" section.
async fn get_release_order(
    State(state): State<PublicApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> Response {
    if !state
        .limiter
        .check(&rl_key(peer, "get"), state.cfg.rate_limit_per_min)
    {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded; retry shortly",
        );
    }

    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => return err(StatusCode::BAD_REQUEST, "invalid order id"),
    };

    let intent = match state.db.get_release_intent(&uuid) {
        Ok(Some(i)) => i,
        Ok(None) => return err(StatusCode::NOT_FOUND, "release order not found"),
        Err(e) => {
            warn!("public api: get release intent failed: {}", e);
            return err(StatusCode::INTERNAL_SERVER_ERROR, "release lookup failed");
        }
    };

    match state
        .db
        .find_burn_order_for_release(&intent.bth_address, intent.amount)
    {
        Ok(Some(order)) => (
            StatusCode::OK,
            Json(release_response_from_order(&intent, &order)),
        )
            .into_response(),
        Ok(None) => {
            // No matching burn detected yet: still awaiting the user's burn,
            // or expired if the tracking window elapsed.
            let status = if Utc::now().timestamp() >= intent.expires_at {
                "expired"
            } else {
                "awaiting_burn"
            };
            (
                StatusCode::OK,
                Json(release_response_from_intent(&intent, status)),
            )
                .into_response()
        }
        Err(e) => {
            warn!("public api: release correlation failed: {}", e);
            err(StatusCode::INTERNAL_SERVER_ERROR, "release lookup failed")
        }
    }
}

// ─────────────────────────────────── CORS ──────────────────────────────────

/// Exact-match CORS middleware. Reflects the request `Origin` back only when it
/// is on the configured allow-list; unlisted origins receive no
/// `Access-Control-Allow-Origin`, so browsers block the cross-origin read.
async fn cors_mw(State(cfg): State<Arc<PublicApiConfig>>, req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let allowed = origin.filter(|o| cfg.cors_allowed_origins.iter().any(|a| a == o));

    if req.method() == Method::OPTIONS {
        // Preflight: answer directly, never dispatch to a handler.
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::NO_CONTENT;
        apply_cors(resp.headers_mut(), allowed.as_deref());
        return resp;
    }

    let mut resp = next.run(req).await;
    apply_cors(resp.headers_mut(), allowed.as_deref());
    resp
}

fn apply_cors(headers: &mut HeaderMap, allowed_origin: Option<&str>) {
    // Vary by Origin so a shared cache never serves one origin's CORS decision
    // to another.
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = allowed_origin {
        if let Ok(value) = HeaderValue::from_str(origin) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, POST, OPTIONS"),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("Content-Type"),
            );
            headers.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("600"),
            );
        }
    }
}

// ──────────────────────────────── Router/serve ─────────────────────────────

/// Build the PUBLIC order router. Contains ONLY user-facing order endpoints —
/// never the breaker, ops status, metrics, or reserve-proof control.
pub fn public_router(state: PublicApiState) -> Router {
    let cors_cfg = state.cfg.clone();
    let max_body = state.cfg.max_body_bytes;
    Router::new()
        .route("/health", get(public_health))
        .route("/api/bridge/orders", post(create_mint_order))
        .route("/api/bridge/orders/:id", get(get_mint_order))
        .route("/api/bridge/release-orders", post(create_release_order))
        .route("/api/bridge/release-orders/:id", get(get_release_order))
        .layer(DefaultBodyLimit::max(max_body))
        .layer(middleware::from_fn_with_state(cors_cfg, cors_mw))
        .with_state(state)
}

/// Serve the public order API until shutdown.
pub async fn serve_public(
    addr: String,
    state: PublicApiState,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {} failed: {}", addr, e))?;
    info!(
        "Bridge PUBLIC order API listening on {} (CORS allow-list: {:?})",
        addr, state.cfg.cors_allowed_origins
    );
    if state.cfg.cors_allowed_origins.is_empty() {
        warn!(
            "Public order API has an EMPTY CORS allow-list — browsers on any web \
             origin will be blocked from reading responses. Set \
             public_api.cors_allowed_origins to the wallet's origin(s)."
        );
    }

    // ConnectInfo is required so per-IP rate limiting sees the peer address.
    let app = public_router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
            info!("Bridge PUBLIC order API shutting down");
        })
        .await
        .map_err(|e| format!("public API server error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_bridge_core::OrderStatus;

    fn test_config() -> PublicApiConfig {
        PublicApiConfig {
            deposit_address: Some("bth_reserve_deposit_addr".to_string()),
            eth_token_address: "0x49b985ec00000000000000000000000000000000".to_string(),
            sol_token_address: "F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX".to_string(),
            fee_bps: 10,
            min_fee: 100_000_000,
            max_order_amount: 1_000_000_000_000_000,
            min_order_amount: 0,
            order_expiry_minutes: 60,
            cors_allowed_origins: vec!["https://botho.io".to_string()],
            // Generous by default so multi-request tests aren't throttled;
            // the rate-limit test builds its own tight-capped state.
            create_rate_limit_per_min: 100,
            rate_limit_per_min: 1_000,
            max_body_bytes: 8 * 1024,
            max_open_orders: 10_000,
        }
    }

    /// A structurally valid bare BTH address (base58 of 64 bytes, the
    /// legacy `view32 || spend32` layout) — #1042 tightened
    /// `ChainAddress::validate`, so tests must use decodable addresses.
    fn test_bth_address() -> String {
        bs58::encode([7u8; 64]).into_string()
    }

    fn test_state() -> (PublicApiState, Database) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        (PublicApiState::new(db.clone(), test_config()), db)
    }

    #[test]
    fn test_status_tag_normalizes_failed() {
        // #1036: the wire tag is a bare "failed", never "failed: <reason>".
        assert_eq!(
            status_tag(&OrderStatus::Failed {
                reason: "boom".to_string()
            }),
            "failed"
        );
        assert_eq!(
            status_tag(&OrderStatus::AwaitingDeposit),
            "awaiting_deposit"
        );
        assert_eq!(status_tag(&OrderStatus::Completed), "completed");
        assert_eq!(status_tag(&OrderStatus::Released), "released");
    }

    #[test]
    fn test_calculate_fee_matches_config() {
        let cfg = test_config();
        // 0.1% of 1 BTH = 0.001 BTH; small amounts floor at min_fee.
        assert_eq!(cfg.calculate_fee(1_000_000_000_000), 1_000_000_000);
        assert_eq!(cfg.calculate_fee(1_000_000), cfg.min_fee);
    }

    async fn spawn_public_server(
        state: PublicApiState,
    ) -> (
        SocketAddr,
        broadcast::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown_tx, _) = broadcast::channel(1);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = public_router(state).into_make_service_with_connect_info::<SocketAddr>();
        let mut shutdown_rx = shutdown_tx.subscribe();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                })
                .await
                .unwrap();
        });
        (addr, shutdown_tx, server)
    }

    #[tokio::test]
    async fn test_create_and_get_mint_order() {
        let (state, _db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        // Valid create.
        let body = r#"{"destChain":"ethereum","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"1000000000000"}"#;
        let (status, resp) =
            http_request(addr, "POST", "/api/bridge/orders", Some(body), None).await;
        assert_eq!(status, 200, "{}", resp);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["status"], "awaiting_deposit");
        assert_eq!(json["destChain"], "ethereum");
        assert_eq!(json["amount"], "1000000000000");
        assert_eq!(json["depositAddress"], "bth_reserve_deposit_addr");
        assert!(
            json["memo"].as_str().unwrap().len() == 128,
            "64-byte hex memo"
        );
        assert!(json.get("expiresAt").is_some());
        let id = json["id"].as_str().unwrap().to_string();

        // Status lookup round-trips.
        let (status, resp) = http_request(
            addr,
            "GET",
            &format!("/api/bridge/orders/{}", id),
            None,
            None,
        )
        .await;
        assert_eq!(status, 200, "{}", resp);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["id"], id);
        assert_eq!(json["status"], "awaiting_deposit");

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_create_mint_order_rejects_invalid_input() {
        let (state, _db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        // Bad chain enum.
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/orders",
            Some(r#"{"destChain":"bitcoin","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"1000000000000"}"#),
            None,
        )
        .await;
        assert_eq!(status, 400);

        // Bad ETH address.
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/orders",
            Some(r#"{"destChain":"ethereum","destAddress":"nope","amount":"1000000000000"}"#),
            None,
        )
        .await;
        assert_eq!(status, 400);

        // Amount not a number.
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/orders",
            Some(r#"{"destChain":"ethereum","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"lots"}"#),
            None,
        )
        .await;
        assert_eq!(status, 400);

        // Amount below the fee floor (min_fee = 1e8, so 1000 < fee).
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/orders",
            Some(r#"{"destChain":"ethereum","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"1000"}"#),
            None,
        )
        .await;
        assert_eq!(status, 400);

        // Amount above the per-order cap.
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/orders",
            Some(r#"{"destChain":"ethereum","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"9999999999999999"}"#),
            None,
        )
        .await;
        assert_eq!(status, 400);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_get_unknown_order_is_404_and_bad_id_is_400() {
        let (state, _db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        let (status, _) = http_request(
            addr,
            "GET",
            "/api/bridge/orders/00000000-0000-0000-0000-000000000000",
            None,
            None,
        )
        .await;
        assert_eq!(status, 404);

        let (status, _) =
            http_request(addr, "GET", "/api/bridge/orders/not-a-uuid", None, None).await;
        assert_eq!(status, 400);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_rate_limit_on_create() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let mut cfg = test_config();
        cfg.create_rate_limit_per_min = 3;
        let state = PublicApiState::new(db, cfg);
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        let body = r#"{"destChain":"ethereum","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"1000000000000"}"#;
        // create_rate_limit_per_min = 3 in the test config.
        for _ in 0..3 {
            let (status, _) =
                http_request(addr, "POST", "/api/bridge/orders", Some(body), None).await;
            assert_eq!(status, 200);
        }
        let (status, _) = http_request(addr, "POST", "/api/bridge/orders", Some(body), None).await;
        assert_eq!(status, 429, "4th create in the window must be throttled");

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_public_router_does_not_serve_ops_endpoints() {
        // The whole point of #1036: the public surface must NOT expose the
        // kill switch or any operational control.
        let (state, _db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        for (method, path, body) in [
            ("POST", "/api/breaker", Some(r#"{"paused":true}"#)),
            ("GET", "/api/status", None),
            ("GET", "/metrics", None),
            ("GET", "/api/reserve/proof", None),
        ] {
            let (status, _) = http_request(addr, method, path, body, None).await;
            assert_eq!(
                status, 404,
                "public surface must NOT route {} {}",
                method, path
            );
        }

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_cors_reflects_only_allowed_origin() {
        let (state, _db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        // Allowed origin is reflected.
        let (status, _, headers) = http_request_with_headers(
            addr,
            "GET",
            "/health",
            None,
            Some(("Origin", "https://botho.io")),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .map(String::as_str),
            Some("https://botho.io")
        );

        // Un-listed origin gets NO allow-origin header.
        let (_, _, headers) = http_request_with_headers(
            addr,
            "GET",
            "/health",
            None,
            Some(("Origin", "https://evil.example")),
        )
        .await;
        assert!(!headers.contains_key("access-control-allow-origin"));

        // Preflight OPTIONS for an allowed origin returns 204 with CORS.
        let (status, _, headers) = http_request_with_headers(
            addr,
            "OPTIONS",
            "/api/bridge/orders",
            None,
            Some(("Origin", "https://botho.io")),
        )
        .await;
        assert_eq!(status, 204);
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .map(String::as_str),
            Some("https://botho.io")
        );

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_release_order_register_and_track() {
        let (state, db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        // Register a release intent.
        let bth_addr = test_bth_address();
        let body = format!(
            r#"{{"sourceChain":"ethereum","bthAddress":"{}","amount":"500000000000"}}"#,
            bth_addr
        );
        let (status, resp) = http_request(
            addr,
            "POST",
            "/api/bridge/release-orders",
            Some(body.as_str()),
            None,
        )
        .await;
        assert_eq!(status, 200, "{}", resp);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["status"], "awaiting_burn");
        assert_eq!(json["sourceChain"], "ethereum");
        assert_eq!(
            json["tokenAddress"],
            "0x49b985ec00000000000000000000000000000000"
        );
        let id = json["id"].as_str().unwrap().to_string();

        // Before any burn: still awaiting_burn.
        let (status, resp) = http_request(
            addr,
            "GET",
            &format!("/api/bridge/release-orders/{}", id),
            None,
            None,
        )
        .await;
        assert_eq!(status, 200, "{}", resp);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["status"], "awaiting_burn");

        // Simulate the watcher creating the burn order for this (address,
        // amount): the status endpoint now correlates + tracks it.
        let mut burn = BridgeOrder::new_burn(
            Chain::Ethereum,
            500_000_000_000,
            0,
            "0xsource".to_string(),
            bth_addr,
            "0xburntx".to_string(),
        );
        burn.set_status(OrderStatus::BurnConfirmed);
        db.insert_order(&burn).unwrap();

        let (status, resp) = http_request(
            addr,
            "GET",
            &format!("/api/bridge/release-orders/{}", id),
            None,
            None,
        )
        .await;
        assert_eq!(status, 200, "{}", resp);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["status"], "burn_confirmed");
        assert_eq!(json["sourceTx"], "0xburntx");

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_failed_order_wire_shape() {
        // A failed order surfaces `status:"failed"` + `failureReason`, never
        // the Display form "failed: <reason>" (#1036).
        let (state, db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_reserve_deposit_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        order.generate_memo();
        order.fail("deposit never arrived");
        db.insert_order(&order).unwrap();

        let (status, resp) = http_request(
            addr,
            "GET",
            &format!("/api/bridge/orders/{}", order.id),
            None,
            None,
        )
        .await;
        assert_eq!(status, 200, "{}", resp);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["failureReason"], "deposit never arrived");

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_global_create_cap_on_mint_orders() {
        // #1042: the GLOBAL ceiling refuses order-create once the number of
        // awaiting-deposit mint orders reaches the cap, independent of
        // client IP (the per-IP limiter is generous here).
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let mut cfg = test_config();
        cfg.max_open_orders = 2;
        let state = PublicApiState::new(db, cfg);
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        let body = r#"{"destChain":"ethereum","destAddress":"0x1234567890abcdef1234567890abcdef12345678","amount":"1000000000000"}"#;
        for _ in 0..2 {
            let (status, _) =
                http_request(addr, "POST", "/api/bridge/orders", Some(body), None).await;
            assert_eq!(status, 200);
        }
        let (status, resp) =
            http_request(addr, "POST", "/api/bridge/orders", Some(body), None).await;
        assert_eq!(status, 503, "create past the global cap must be refused");
        assert!(resp.contains("backlog"), "{}", resp);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_global_create_cap_on_release_intents() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let mut cfg = test_config();
        cfg.max_open_orders = 1;
        let state = PublicApiState::new(db, cfg);
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        let body = format!(
            r#"{{"sourceChain":"ethereum","bthAddress":"{}","amount":"500000000000"}}"#,
            test_bth_address()
        );
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/release-orders",
            Some(body.as_str()),
            None,
        )
        .await;
        assert_eq!(status, 200);

        let (status, resp) = http_request(
            addr,
            "POST",
            "/api/bridge/release-orders",
            Some(body.as_str()),
            None,
        )
        .await;
        assert_eq!(status, 503, "intent create past the global cap refused");
        assert!(resp.contains("backlog"), "{}", resp);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_create_release_order_rejects_invalid_bth_address() {
        // #1042: the strengthened BTH validator rejects junk destinations at
        // order-create (the old validator only rejected empty strings).
        let (state, _db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        for bad in ["bth_user_receive_addr", "not base58!", "botho://1/abc"] {
            let body = format!(
                r#"{{"sourceChain":"ethereum","bthAddress":"{}","amount":"500000000000"}}"#,
                bad
            );
            let (status, _) = http_request(
                addr,
                "POST",
                "/api/bridge/release-orders",
                Some(body.as_str()),
                None,
            )
            .await;
            assert_eq!(status, 400, "junk BTH address {:?} must be rejected", bad);
        }

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_release_intent_create_writes_audit_row() {
        // #1042 audit symmetry: release-intent create logs an audit line
        // like mint create does.
        let (state, db) = test_state();
        let (addr, shutdown_tx, server) = spawn_public_server(state).await;

        let body = format!(
            r#"{{"sourceChain":"ethereum","bthAddress":"{}","amount":"500000000000"}}"#,
            test_bth_address()
        );
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/bridge/release-orders",
            Some(body.as_str()),
            None,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(db.count_audit_action("release_intent_created").unwrap(), 1);

        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }

    /// Minimal HTTP/1.1 request over a raw socket. Returns (status, body).
    async fn http_request(
        addr: SocketAddr,
        method: &str,
        path: &str,
        json_body: Option<&str>,
        _unused: Option<()>,
    ) -> (u16, String) {
        let (status, body, _headers) =
            http_request_with_headers(addr, method, path, json_body, None).await;
        (status, body)
    }

    /// Like `http_request` but also returns lower-cased response headers and
    /// lets the caller set one request header (used for `Origin`).
    async fn http_request_with_headers(
        addr: SocketAddr,
        method: &str,
        path: &str,
        json_body: Option<&str>,
        extra_header: Option<(&str, &str)>,
    ) -> (u16, String, HashMap<String, String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let extra = extra_header
            .map(|(k, v)| format!("{}: {}\r\n", k, v))
            .unwrap_or_default();
        let request = match json_body {
            Some(body) => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n{}Content-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                method,
                path,
                addr,
                extra,
                body.len(),
                body
            ),
            None => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
                method, path, addr, extra
            ),
        };
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response).to_string();

        let status: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .map(|(h, b)| (h.to_string(), b.to_string()))
            .unwrap_or_default();

        let mut headers = HashMap::new();
        for line in head.lines().skip(1) {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }
        (status, body, headers)
    }
}
