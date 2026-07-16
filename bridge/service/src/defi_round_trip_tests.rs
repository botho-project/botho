// Copyright (c) 2024 The Botho Foundation

//! Full wBTH DeFi round-trip e2e (#1005) — the headline
//! **mint → wrap → fund → pool → swap → repatriate** loop, driven end to end
//! against a node **forking real Sepolia state** plus a local Botho node.
//!
//! This is **Phase A of the mainnet-liquidity-bootstrap DeFi round trip
//! (#865)**: the single orchestrated demonstration AND the rehearsal script for
//! the mainnet liquidity launch. It wires two already-landed pieces into one
//! continuous journey of a coin:
//!
//! - The full-loop wrap/repatriate legs ([`crate::e2e_full_loop_tests`], #993)
//!   drive the REAL bridge engine ([`OrderProcessor::process_pending_orders`]):
//!   a confirmed BTH deposit → t-of-n EIP-712 federation attestation →
//!   `Safe.execTransaction(bridgeMint)` (steps 1–2), and a `bridgeBurn` →
//!   t-of-n Ed25519 attestation → [`BthReleaser`] reserve spend to a fresh
//!   stealth output (step 6).
//! - The Uniswap-v3 fork harness ([`crate::uniswap_fork_tests`], #1004) seeds a
//!   real wBTH/WETH pool and swaps against it (steps 3–5), factored into the
//!   reusable [`crate::uniswap_fork_tests::create_pool_and_add_liquidity`] /
//!   [`crate::uniswap_fork_tests::swap_weth_for_wbth`] helpers this test
//!   shares.
//!
//! ## The journey of one coin
//!
//! 1. **Mint BTH** on a local `botho-testnet` node (a funded factor-1 reserve).
//! 2. **Wrap → wBTH**: the engine drives the mint order to `Completed`, minting
//!    wBTH to the user through the Safe. **Every wBTH in this test is a wrapped
//!    coin** — the token's only `MINTER` is the federation Safe, so there is no
//!    other mint path.
//! 3. **Fund gas** for the fork's dev accounts via `*_setBalance` and wrap ETH
//!    into WETH (the faucet + WETH stand-ins).
//! 4. **Seed the pool**: create the wBTH/WETH Uniswap v3 pool and add two-sided
//!    liquidity — the wBTH side is the coin that was just wrapped.
//! 5. **Purchase**: swap WETH → wBTH against the seeded pool (the market buys
//!    wBTH; the bought wBTH comes straight out of the pool).
//! 6. **Repatriate**: `bridgeBurn` exactly the swap proceeds and let the engine
//!    drive the burn order to `Released`, paying native BTH to a fresh stealth
//!    output the user scans back off the live node.
//!
//! So a coin genuinely travels **Botho BTH → wBTH → into a DEX pool → bought
//! via a swap → back to native BTH**.
//!
//! ## The round-trip assertions (the demonstration's proof)
//!
//! - **Peg on wrap** — `wbth.balanceOf(user) == wbth.totalSupply() == BTH
//!   locked` (ADR 0003 factor-1, exact); the reserve reconciler reports `drift
//!   == 0` after the mint.
//! - **Pool + swap** — `factory.getPool(...)` is non-zero, the position has
//!   `liquidity > 0`, and the swap moved WETH → wBTH in the right direction.
//! - **Provenance of the repatriated coin** — the burn amount **equals the swap
//!   output**, and the released BTH equals the burn amount (net of fees, zero
//!   here), delivered to a fresh stealth output the user's view key scans back
//!   (ADR 0004), on a tx unlinkable to the EVM burn.
//! - **Proof-of-reserves across the whole loop** — the reconciler reports
//!   `drift == 0` at the start (`0/0`), after the mint (`WRAP/WRAP`), and after
//!   the partial repatriation (`WRAP-swapOut / WRAP-swapOut`): only the backing
//!   for the burned coins is unlocked, the rest stays locked behind the wBTH
//!   still circulating in the pool.
//!
//! ## Running (the mainnet liquidity-launch rehearsal)
//!
//! `#[ignore]`d AND **self-skips** unless a Sepolia-fork RPC AND the BTH
//! reserve key material are both present — it never claims a live path it could
//! not exercise (the #992/#993 discipline). The hermetic driver boots both
//! nodes, funds via `*_setBalance` (zero external creds):
//!
//! ```text
//! ./scripts/bridge-e2e-defi-fork.sh https://ethereum-sepolia-rpc.publicnode.com
//! ```
//!
//! or manually, against an `anvil --fork-url <sepolia>` node + a local Botho
//! node with a funded factor-1 reserve:
//!
//! ```text
//! BRIDGE_FORK_RPC_URL=http://127.0.0.1:8545 \
//! BRIDGE_FORK_EXPECTED_CHAIN_ID=11155111 \
//! BRIDGE_FORK_FUND_ACCOUNTS=1 \
//! BRIDGE_BTH_RPC_URL=http://127.0.0.1:27200 \
//! BRIDGE_BTH_RESERVE_VIEW_KEY=/path/reserve.view.hex \
//! BRIDGE_BTH_RESERVE_SPEND_KEY=/path/reserve.spend.hex \
//! BRIDGE_BTH_RESERVE_ADDRESS=<reserve bth address> \
//! BRIDGE_BTH_USER_ADDRESS=<user bth address> \
//! BRIDGE_BTH_USER_VIEW_KEY=/path/user.view.hex \
//! BRIDGE_BTH_USER_SPEND_KEY=/path/user.spend.hex \
//!   cargo test -p bth-bridge-service -- --ignored defi_round_trip_ --nocapture
//! ```
//!
//! ## Fork → testnet → mainnet flip
//!
//! The SAME test seeds a live pool by swapping endpoints only (no test-logic
//! change): point `BRIDGE_FORK_RPC_URL` at a live Sepolia/mainnet RPC, set the
//! `BRIDGE_UNISWAP_*` / `BRIDGE_WETH_ADDRESS` for that chain, set
//! `BRIDGE_WBTH_ADDRESS` to the #866-deployed token instead of a throwaway
//! deploy, leave `BRIDGE_FORK_FUND_ACCOUNTS` **unset** (there is no
//! `setBalance` on a real chain), and supply genuinely funded relayer/LP keys.
//! That live-Sepolia execution is Phase B (#866/#868/#869) — this issue is the
//! fork rehearsal + the harness those reuse verbatim.

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    sol,
    sol_types::{SolCall, SolValue},
};
use bth_bridge_core::{
    attestation::{sign_attestation_ed25519, AttestationKind},
    BridgeConfig, BridgeOrder, BthConfig, Chain, GasPriceStrategy, OrderStatus,
};
use chrono::Utc;
use ed25519_dalek::SigningKey;
use std::{collections::HashMap, sync::Arc, time::Duration};
use uuid::Uuid;

use crate::{
    attestation::{sign_attestation_secp256k1, AttestationProvider, FederationAttestationProvider},
    db::Database,
    engine::OrderProcessor,
    fork_tests::{
        artifact_bytecode, call_u256, deploy, dev_signer, expected_chain_id,
        fund_dev_accounts_if_requested, rpc_url,
    },
    mint::{ethereum::EthMinter, Minter},
    release::{bth::BthReleaser, Releaser},
    reserve::Reconciler,
    uniswap_fork_tests::{
        create_pool_and_add_liquidity, swap_weth_for_wbth, uniswap_periphery_present,
        uniswap_v3_env, wrap_weth,
    },
    watchers::{
        bth::{BthChainClient, NodeBthClient},
        ethereum::{with_tx_ordinals, AlloyEthClient, EthChainClient},
    },
};

sol! {
    #[allow(missing_docs)]
    interface IWrappedBTHRoundTrip {
        function balanceOf(address account) external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function bridgeBurn(uint256 amount, string calldata bthAddress) external;
    }
}

/// Well-known dev accounts of `anvil` / `npx hardhat node` (mnemonic
/// "test test test test test test test test test test test junk"). Test ETH
/// only — not secrets. Mirrors `fork_tests.rs` / `e2e_full_loop_tests.rs`.
const DEV_KEYS: [&str; 4] = [
    // 0: contract deployer + relayer EOA (pays gas, holds no authority)
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    // 1: Safe owner 1 (local mint attestation signer)
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    // 2: Safe owner 2 (peer federation member, envelope injected)
    "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    // 3: the bridging user — LP + swapping "market" + repatriating burner
    "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
];

/// The live BTH-node environment for the round trip. `None` when any piece is
/// absent — the test then self-skips (never a false green). Mirrors the
/// full-loop `BthEnv`, but defaults `amount` to a pool-viable 200,000 BTH so
/// the wrapped wBTH can seed real Uniswap liquidity.
struct BthEnv {
    rpc_url: String,
    reserve_view_key: String,
    reserve_spend_key: String,
    reserve_address: String,
    user_address: String,
    user_view_key: String,
    user_spend_key: String,
    /// Amount to wrap (picocredits). The driver must fund the reserve with at
    /// least this much spendable factor-1 balance. Default 200,000 BTH — under
    /// the wBTH `maxMintPerTx` (1M BTH) and ample to seed the pool.
    amount: u64,
}

fn non_empty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}

fn bth_env() -> Option<BthEnv> {
    Some(BthEnv {
        rpc_url: non_empty("BRIDGE_BTH_RPC_URL")?,
        reserve_view_key: non_empty("BRIDGE_BTH_RESERVE_VIEW_KEY")?,
        reserve_spend_key: non_empty("BRIDGE_BTH_RESERVE_SPEND_KEY")?,
        reserve_address: non_empty("BRIDGE_BTH_RESERVE_ADDRESS")?,
        user_address: non_empty("BRIDGE_BTH_USER_ADDRESS")?,
        user_view_key: non_empty("BRIDGE_BTH_USER_VIEW_KEY")?,
        user_spend_key: non_empty("BRIDGE_BTH_USER_SPEND_KEY")?,
        amount: non_empty("BRIDGE_BTH_AMOUNT")
            .and_then(|s| s.parse().ok())
            .unwrap_or(200_000_000_000_000_000),
    })
}

/// Drive `process_pending_orders` up to `ticks` times, stopping as soon as the
/// order reaches `target`. Returns the final observed status. (Same shape as
/// the full-loop driver — duplicated to keep this module additive.)
async fn drive_until(
    processor: &OrderProcessor,
    db: &Database,
    order_id: &Uuid,
    target: OrderStatus,
    ticks: usize,
) -> OrderStatus {
    let mut last = OrderStatus::AwaitingDeposit;
    for _ in 0..ticks {
        processor
            .process_pending_orders()
            .await
            .expect("processor tick");
        last = db
            .get_order(order_id)
            .expect("get_order")
            .expect("order present")
            .status;
        if last == target {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    last
}

/// The full six-step DeFi round trip through the real engine + real Uniswap.
#[tokio::test]
#[ignore = "requires a Sepolia-fork node (anvil --fork-url) + a local Botho node with a funded reserve; run scripts/bridge-e2e-defi-fork.sh"]
async fn defi_round_trip_mint_wrap_pool_swap_repatriate() {
    // ---- Gate 1: BTH reserve key material (self-skip until #999) ----------
    let Some(bth) = bth_env() else {
        eprintln!(
            "SKIP defi_round_trip: set BRIDGE_BTH_RPC_URL, \
             BRIDGE_BTH_RESERVE_VIEW_KEY, BRIDGE_BTH_RESERVE_SPEND_KEY, \
             BRIDGE_BTH_RESERVE_ADDRESS, BRIDGE_BTH_USER_ADDRESS, \
             BRIDGE_BTH_USER_VIEW_KEY, BRIDGE_BTH_USER_SPEND_KEY to run the \
             round trip (a funded factor-1 reserve on a live Botho node is \
             required)"
        );
        return;
    };

    // Liquidity sizing: seed the pool with 10^17 pico wBTH (100,000 BTH) — the
    // proven #1004 scale — matched by 10^17 wei WETH (0.1 WETH), and swap in
    // 10^16 wei WETH (0.01 WETH). The wBTH side is drawn from the wrap, so the
    // wrapped amount must cover it.
    let wbth_liq = U256::from(100_000_000_000_000_000u64); // 10^17 pico wBTH
    let weth_liq = U256::from(100_000_000_000_000_000u64); // 10^17 wei WETH
    let swap_in = U256::from(10_000_000_000_000_000u64); //  10^16 wei WETH
    let fee: u32 = 3000; // 0.30% tier
    assert!(
        U256::from(bth.amount) >= wbth_liq,
        "BRIDGE_BTH_AMOUNT ({}) must be >= the wBTH liquidity side ({wbth_liq}); \
         raise it or lower the liquidity target",
        bth.amount
    );

    let url: alloy::transports::http::reqwest::Url = rpc_url().parse().expect("valid RPC url");
    let deployer = dev_signer(0);
    let owner1 = dev_signer(1);
    let owner2 = dev_signer(2);
    let user = dev_signer(3); // wraps, LPs, swaps, and repatriates
    let user_addr = user.address();

    let deploy_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(deployer.clone()))
        .connect_http(url.clone())
        .erased();

    // ---- Gate 2: an Ethereum node is reachable ----------------------------
    let chain_id = match deploy_provider.get_chain_id().await {
        Ok(id) => id,
        Err(e) => {
            eprintln!(
                "SKIP defi_round_trip: no Ethereum node reachable at {} ({e}). \
                 Start one with `anvil --fork-url <sepolia-rpc>` — see module docs.",
                rpc_url()
            );
            return;
        }
    };
    if let Some(expected) = expected_chain_id() {
        assert_eq!(
            chain_id, expected,
            "node reports chain id {chain_id}, expected {expected}"
        );
    }

    // ---- Gate 3: the real Uniswap v3 periphery is present -----------------
    // On a plain local dev chain the canonical Sepolia periphery has no code;
    // this test needs a fork (or a live chain). Self-skip cleanly otherwise.
    let uni = uniswap_v3_env();
    if !uniswap_periphery_present(&deploy_provider, &uni).await {
        eprintln!(
            "SKIP defi_round_trip: no Uniswap v3 periphery code at the configured \
             addresses on chain {chain_id}. Point BRIDGE_FORK_RPC_URL at an \
             `anvil --fork-url <sepolia>` node (or set BRIDGE_UNISWAP_* for the \
             target chain)."
        );
        return;
    }

    // Fund the dev accounts on the fork (test ETH via *_setBalance; no real
    // funded account). No-op on local 31337.
    fund_dev_accounts_if_requested(&deploy_provider).await;

    // ---- Deploy the 2-of-2 validator Safe and the wBTH token -------------
    // The Safe is the token's ONLY MINTER — so every wBTH is a wrapped coin.
    let mut owners = vec![owner1.address(), owner2.address()];
    owners.sort();
    let safe_addr = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/test/SafeStub.sol/SafeStub.json"),
        (owners.clone(), U256::from(2u8)).abi_encode_params(),
    )
    .await;
    let wbth_addr = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/WrappedBTH.sol/WrappedBTH.json"),
        // admin / MINTER (the Safe!) / pauser
        (deployer.address(), safe_addr, deployer.address()).abi_encode_params(),
    )
    .await;

    // ---- Federation keys: secp256k1 (mint) + Ed25519 (release) -----------
    let ed_local = SigningKey::from_bytes(&[0x11u8; 32]); // this node
    let ed_peer = SigningKey::from_bytes(&[0x22u8; 32]); // peer validator

    let dir = tempfile::tempdir().expect("tempdir");
    let relayer_key_path = dir.path().join("relayer.hex");
    std::fs::write(&relayer_key_path, DEV_KEYS[0]).unwrap();
    let owner1_key_path = dir.path().join("owner1.hex");
    std::fs::write(&owner1_key_path, DEV_KEYS[1]).unwrap();
    let ed_local_key_path = dir.path().join("ed_local.hex");
    std::fs::write(&ed_local_key_path, hex::encode(ed_local.to_bytes())).unwrap();

    // ---- One BridgeConfig wiring BOTH chains -----------------------------
    let mut config = BridgeConfig::default();
    config.ethereum.rpc_url = rpc_url();
    config.ethereum.chain_id = chain_id;
    config.ethereum.wbth_contract = format!("{:#x}", wbth_addr);
    config.ethereum.safe_address = Some(format!("{:#x}", safe_addr));
    config.ethereum.private_key_file = Some(relayer_key_path.to_string_lossy().into_owned());
    config.ethereum.confirmations_required = 1;
    config.ethereum.gas_price_strategy = GasPriceStrategy::Fixed(3);
    config.ethereum.mint_signers = vec![
        format!("{:#x}", owner1.address()),
        format!("{:#x}", owner2.address()),
    ];
    config.ethereum.mint_threshold = 2;

    config.bth = BthConfig {
        rpc_url: bth.rpc_url.clone(),
        ws_url: String::new(),
        view_key_file: Some(bth.reserve_view_key.clone()),
        spend_key_file: Some(bth.reserve_spend_key.clone()),
        pq_seed_file: None,
        confirmations_required: 0,
        reserve_address: Some(bth.reserve_address.clone()),
        release_signers: vec![
            hex::encode(ed_local.verifying_key().to_bytes()),
            hex::encode(ed_peer.verifying_key().to_bytes()),
        ],
        release_threshold: 2,
        release_confirmations_required: 0,
    };

    config.bridge.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
    config.bridge.attestation_secp256k1_key_file =
        Some(owner1_key_path.to_string_lossy().into_owned());
    config.bridge.attestation_ed25519_key_file =
        Some(ed_local_key_path.to_string_lossy().into_owned());
    config.reserve.api_listen = String::new();

    // ---- Real engine wiring: minter, releaser, attestation, reconciler ---
    let db = Database::open_in_memory().expect("db");
    db.migrate().expect("migrate");

    let eth_minter = EthMinter::new(config.ethereum.clone()).expect("eth minter builds");
    let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
    minters.insert(Chain::Ethereum, Arc::new(eth_minter));

    let releaser = BthReleaser::new(config.bth.clone()).expect("bth releaser builds");

    let provider = Arc::new(
        FederationAttestationProvider::from_config(&config)
            .expect("valid federation config")
            .expect("federation configured (both eth mint + bth release)"),
    );

    let processor = OrderProcessor::new(
        config.clone(),
        db.clone(),
        minters,
        Some(Arc::new(releaser) as Arc<dyn Releaser>),
        provider.clone() as Arc<dyn AttestationProvider>,
    );

    let reconciler = Reconciler::from_config(&config, db.clone());

    // Assertion 0 — proof-of-reserves starts pristine: nothing wrapped, nothing
    // locked, zero drift.
    let proof_start = reconciler
        .reconcile_once()
        .await
        .expect("reconcile (start)");
    assert_eq!(proof_start.eth_supply, Some(0), "no wBTH before the wrap");
    assert_eq!(proof_start.locked_reserve, 0, "no reserve locked yet");
    assert_eq!(proof_start.drift, 0, "peg starts flat");

    // =====================================================================
    // STEP 1+2 — MINT BTH (reserve) then WRAP → wBTH through the real engine.
    // =====================================================================
    let mut mint_order = BridgeOrder::new_mint(
        Chain::Ethereum,
        bth.amount,
        0, // keep the peg exact (fee accounting is covered by unit tests)
        bth.reserve_address.clone(),
        format!("{:#x}", user_addr),
    );
    mint_order.source_tx = Some("bth_deposit_confirmed_by_watcher".to_string());
    mint_order.set_status(OrderStatus::DepositConfirmed);
    db.insert_order(&mint_order).expect("insert mint order");

    // Fail-safe: a single Safe-owner signature must NOT authorize a mint.
    processor
        .process_pending_orders()
        .await
        .expect("tick (below-threshold mint)");
    assert_eq!(
        db.get_order(&mint_order.id).unwrap().unwrap().status,
        OrderStatus::DepositConfirmed,
        "single signer must not authorize a mint (fail-safe)"
    );
    let pre_mint_supply = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHRoundTrip::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(pre_mint_supply, U256::ZERO, "no wBTH before the threshold");

    // Peer (owner 2) submits its EIP-712 envelope, bound to the fresh Safe
    // nonce (0).
    let now = Utc::now().timestamp().max(0) as u64;
    let mint_kind = AttestationKind::MintWbth {
        dest_chain: Chain::Ethereum,
        dest_address: mint_order.dest_address.clone(),
        amount: mint_order.net_amount(),
        order_id: mint_order.id,
        source_tx: mint_order.source_tx.clone().unwrap(),
        safe_nonce: Some(0),
    };
    let mint_envelope = sign_attestation_secp256k1(
        &mint_kind,
        &owner2,
        chain_id,
        safe_addr,
        wbth_addr,
        &Uuid::new_v4().simple().to_string(),
        now,
        now + 300,
    )
    .expect("peer mint envelope signs");
    assert!(
        provider
            .submit_attestation(&mint_envelope, &mint_order)
            .accepted,
        "peer mint attestation must be accepted"
    );

    // At threshold (2/2) the engine mints through the Safe.
    let final_mint = drive_until(&processor, &db, &mint_order.id, OrderStatus::Completed, 40).await;
    assert_eq!(
        final_mint,
        OrderStatus::Completed,
        "engine must drive the mint to Completed"
    );

    // Assertion (peg on wrap) — wBTH minted == BTH locked (factor-1, ADR 0003).
    let wrap_amount = mint_order.net_amount();
    let minted_balance = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHRoundTrip::balanceOfCall { account: user_addr }.abi_encode(),
    )
    .await;
    assert_eq!(
        minted_balance,
        U256::from(wrap_amount),
        "wBTH balance == locked BTH (factor-1)"
    );
    let supply_after_mint = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHRoundTrip::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(
        supply_after_mint,
        U256::from(wrap_amount),
        "supply == locked"
    );

    // Proof-of-reserves after mint: Σ(wBTH) == locked, zero drift, custody
    // leg actually consulted.
    let proof_after_mint = reconciler.reconcile_once().await.expect("reconcile (mint)");
    assert_eq!(proof_after_mint.eth_supply, Some(wrap_amount));
    assert_eq!(proof_after_mint.locked_reserve, wrap_amount);
    assert_eq!(proof_after_mint.drift, 0, "Σ wBTH == locked reserve");
    assert!(proof_after_mint.in_tolerance, "peg within tolerance");
    assert!(
        proof_after_mint.reserve_balance_checked,
        "the live BTH reserve balance must be checked (custody leg)"
    );

    // =====================================================================
    // STEP 3 — FUND gas + WETH (the faucet + WETH stand-ins).
    // =====================================================================
    let user_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(user.clone()))
        .connect_http(url.clone())
        .erased();
    // Wrap 1 ETH into WETH: ample for the 0.1 WETH liquidity + 0.01 WETH swap.
    wrap_weth(
        &user_provider,
        uni.weth,
        U256::from(1_000_000_000_000_000_000u64),
    )
    .await;

    // =====================================================================
    // STEP 4 — SEED the pool: create wBTH/WETH pool + add liquidity. The wBTH
    // side is the coin just wrapped.
    // =====================================================================
    let liq = create_pool_and_add_liquidity(
        &user_provider,
        &uni,
        wbth_addr,
        user_addr,
        wbth_liq,
        weth_liq,
        fee,
    )
    .await;
    assert_ne!(liq.pool, Address::ZERO, "pool created");
    assert!(liq.liquidity > 0, "position has liquidity > 0");
    assert!(liq.amount0 > U256::ZERO, "token0 side funded");
    assert!(liq.amount1 > U256::ZERO, "token1 side funded");

    // Supply is unchanged by seeding + (soon) swapping — no mint/burn — so the
    // peg still reconciles flat while the wBTH sits in the pool.
    let proof_after_seed = reconciler.reconcile_once().await.expect("reconcile (seed)");
    assert_eq!(proof_after_seed.eth_supply, Some(wrap_amount));
    assert_eq!(
        proof_after_seed.drift, 0,
        "peg flat while liquidity is live"
    );

    // =====================================================================
    // STEP 5 — PURCHASE: swap WETH → wBTH against the seeded pool.
    // =====================================================================
    let swap = swap_weth_for_wbth(&user_provider, &uni, wbth_addr, user_addr, swap_in, fee).await;
    assert!(swap.wbth_out > U256::ZERO, "swap increased wBTH");
    assert_eq!(
        swap.weth_spent, swap_in,
        "exactInputSingle spent exactly amountIn WETH"
    );
    let swapped_wbth = swap.wbth_out;
    let swapped_pc = u64::try_from(swapped_wbth).expect("swap output fits u64 picocredits");
    eprintln!(
        "defi round trip: pool {:#x}, liquidity {}, bought {swapped_pc} pico wBTH \
         with {swap_in} wei WETH",
        liq.pool, liq.liquidity
    );

    // =====================================================================
    // STEP 6 — REPATRIATE: bridgeBurn exactly the swap proceeds → engine drives
    // the reserve release to a fresh stealth output.
    // =====================================================================
    let burn_receipt = user_provider
        .send_transaction(
            TransactionRequest::default().with_to(wbth_addr).with_input(
                IWrappedBTHRoundTrip::bridgeBurnCall {
                    amount: swapped_wbth,
                    bthAddress: bth.user_address.clone(),
                }
                .abi_encode(),
            ),
        )
        .await
        .expect("burn tx accepted")
        .get_receipt()
        .await
        .expect("burn tx mined");
    assert!(burn_receipt.status(), "bridgeBurn reverted");

    // Detect the burn with the SAME transport the Ethereum watcher runs.
    let eth_client = AlloyEthClient::new(&config.ethereum).expect("watcher client");
    let tip = eth_client.latest_block().await.expect("eth tip");
    let events = eth_client.burn_events(0, tip).await.expect("burn scan");
    let ordered = with_tx_ordinals(events);
    let (event, _ordinal) = ordered
        .iter()
        .find(|(e, _)| e.tx_hash == format!("{:#x}", burn_receipt.transaction_hash))
        .expect("watcher sees the burn");

    // Provenance: the burn amount EQUALS the swap output.
    assert_eq!(
        event.amount, swapped_pc,
        "burn amount == swap output (the repatriated coin is exactly what the market bought)"
    );
    assert_eq!(event.bth_address, bth.user_address);

    let mut burn_order = BridgeOrder::new_burn(
        Chain::Ethereum,
        event.amount,
        0,
        event.from.clone(),
        event.bth_address.clone(),
        event.tx_hash.clone(),
    );
    burn_order.set_status(OrderStatus::BurnConfirmed);
    db.insert_order(&burn_order).expect("insert burn order");

    // Fail-safe: a single Ed25519 signature must NOT authorize a release.
    processor
        .process_pending_orders()
        .await
        .expect("tick (below-threshold release)");
    assert_eq!(
        db.get_order(&burn_order.id).unwrap().unwrap().status,
        OrderStatus::BurnConfirmed,
        "single signer must not authorize a release (fail-safe)"
    );

    // Peer validator submits its Ed25519 release envelope.
    let release_kind = FederationAttestationProvider::release_kind_for_test(&burn_order);
    let release_envelope = sign_attestation_ed25519(
        &release_kind,
        &ed_peer,
        &Uuid::new_v4().simple().to_string(),
        now,
        now + 300,
    )
    .expect("peer release envelope signs");
    assert!(
        provider
            .submit_attestation(&release_envelope, &burn_order)
            .accepted,
        "peer release attestation must be accepted"
    );

    // At threshold (2/2) the engine releases through the live BthReleaser.
    let final_release =
        drive_until(&processor, &db, &burn_order.id, OrderStatus::Released, 60).await;
    assert_eq!(
        final_release,
        OrderStatus::Released,
        "engine must drive the release to Released (requires a funded factor-1 reserve)"
    );

    // Assertion (provenance + ADR 0004) — the released BTH equals the burned
    // (== swapped) amount, delivered to a FRESH stealth output the USER's own
    // view key scans back, distinct from the EVM burn tx.
    let user_client = NodeBthClient::new(BthConfig {
        rpc_url: bth.rpc_url.clone(),
        ws_url: String::new(),
        view_key_file: Some(bth.user_view_key.clone()),
        spend_key_file: Some(bth.user_spend_key.clone()),
        pq_seed_file: None,
        confirmations_required: 0,
        reserve_address: Some(bth.user_address.clone()),
        release_signers: Vec::new(),
        release_threshold: 0,
        release_confirmations_required: 0,
    })
    .expect("user scan client");
    let user_tip = user_client.tip_height().await.expect("bth tip");
    let scan_start = user_tip.saturating_sub(200);
    let mut released_output = None;
    for height in scan_start..=user_tip {
        if let Some(block) = user_client.block_at(height).await.expect("scan block") {
            if let Some(dep) = block.deposits.iter().find(|d| d.amount == swapped_pc) {
                released_output = Some(dep.clone());
                break;
            }
        }
    }
    let released_output =
        released_output.expect("user must scan back the fresh released stealth output");
    assert_eq!(
        released_output.amount, swapped_pc,
        "released BTH == burned == swap output (net of fees)"
    );
    assert_ne!(
        released_output.tx_hash,
        burn_order.source_tx.clone().unwrap(),
        "the BTH release output is a fresh on-chain tx, unlinkable to the EVM burn"
    );

    // Assertion (proof-of-reserves across the loop) — only the backing for the
    // repatriated coins is unlocked; the rest stays locked behind the wBTH
    // still circulating in the pool. Supply and locked both drop by exactly the
    // swap output, drift stays zero.
    let expected_supply = wrap_amount - swapped_pc;
    let final_supply = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHRoundTrip::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(
        final_supply,
        U256::from(expected_supply),
        "supply dropped by exactly the burned swap output"
    );

    let proof_after_release = reconciler
        .reconcile_once()
        .await
        .expect("reconcile (release)");
    assert_eq!(
        proof_after_release.eth_supply,
        Some(expected_supply),
        "wBTH supply == wrapped − repatriated"
    );
    assert_eq!(
        proof_after_release.drift, 0,
        "peg exact after the partial repatriation (Σ wBTH == locked reserve)"
    );
    assert!(
        proof_after_release.in_tolerance,
        "peg within tolerance after the round trip"
    );

    eprintln!(
        "defi round trip OK: wrapped {wrap_amount} pc → seeded wBTH/WETH pool → \
         market bought {swapped_pc} pc wBTH → repatriated to a fresh stealth \
         output; peg exact (supply {expected_supply}, drift 0)"
    );
}
