// Copyright (c) 2024 The Botho Foundation

//! Uniswap-v3 fork harness (#1004): create a wBTH/WETH pool, add two-sided
//! liquidity, and swap WETH -> wBTH against the REAL Uniswap v3 periphery,
//! driven from Rust against a node **forking real Sepolia state**.
//!
//! This is Phase A of the mainnet-liquidity-bootstrap DeFi round trip (#865):
//! the reusable primitive that seeds wBTH liquidity on a DEX. Uniswap v3 is
//! already deployed on Sepolia, so an `anvil --fork-url <sepolia>` node
//! inherits the real Factory / NonfungiblePositionManager / SwapRouter02 /
//! WETH9 — we deploy only a throwaway `WrappedBTH` (exactly as
//! [`crate::fork_tests`] does) and drive the real Uniswap contracts. **No
//! funded account, no deployed contract, no secret.**
//!
//! ## What the harness does
//!
//! 1. Deploys a throwaway `WrappedBTH` (deployer holds `MINTER_ROLE`) and mints
//!    wBTH to the LP/user account.
//! 2. Funds the LP account with test ETH via the existing `*_setBalance` path
//!    ([`crate::fork_tests::fund_dev_accounts_if_requested`], gated on
//!    `BRIDGE_FORK_FUND_ACCOUNTS`) and wraps some to WETH via `WETH9.deposit`.
//! 3. Sorts wBTH/WETH into token0/token1, initializes the pool at a 1:1 raw
//!    price (`sqrtPriceX96 = 2^96`) via `createAndInitializePoolIfNecessary`,
//!    approves both tokens to the position manager, and `mint`s a
//!    **full-range** two-sided position.
//! 4. Swaps WETH -> wBTH through `SwapRouter02.exactInputSingle`, capturing
//!    before/after balances.
//! 5. Asserts: `factory.getPool(...)` is non-zero, the minted position has
//!    liquidity > 0, the swap increased wBTH and decreased WETH in the right
//!    direction, and — to prove the swapped wBTH is a normal ERC-20 balance the
//!    user can later repatriate (the `bridgeBurn` hook #1005 will wire) — a
//!    `bridgeBurn` of the swap proceeds succeeds and lowers the balance.
//!
//! ## Running
//!
//! `#[ignore]`d (needs a fork node + compiled artifacts) and **self-skips**
//! when no node is reachable — the same discipline as [`crate::fork_tests`]
//! (#992). Easiest:
//!
//! ```text
//! # start a local fork of Sepolia over any public RPC (no key needed for
//! # read-mostly public endpoints):
//! anvil --fork-url https://ethereum-sepolia-rpc.publicnode.com --port 8545 &
//!
//! # compile the throwaway token artifact once:
//! (cd contracts/ethereum && npm ci && npx hardhat compile)
//!
//! # run the harness against the fork:
//! BRIDGE_FORK_RPC_URL=http://127.0.0.1:8545 \
//! BRIDGE_FORK_EXPECTED_CHAIN_ID=11155111 \
//! BRIDGE_FORK_FUND_ACCOUNTS=1 \
//!   cargo test -p bth-bridge-service -- --ignored uniswap_fork_ --nocapture
//! ```
//!
//! ## Fork -> live flip (the mainnet liquidity-seeding reuse point)
//!
//! Every Uniswap address is env-gated with the canonical Sepolia value as its
//! default, so the SAME harness seeds a real pool by swapping env only:
//!
//! - `BRIDGE_UNISWAP_FACTORY`          — UniswapV3Factory
//! - `BRIDGE_UNISWAP_POSITION_MANAGER` — NonfungiblePositionManager
//! - `BRIDGE_UNISWAP_SWAP_ROUTER`      — SwapRouter02
//! - `BRIDGE_WETH_ADDRESS`             — WETH9
//!
//! To seed liquidity on live Sepolia/mainnet (#866/#869): point
//! `BRIDGE_FORK_RPC_URL` at a live RPC, set `BRIDGE_UNISWAP_*` /
//! `BRIDGE_WETH_ADDRESS` for that chain, leave `BRIDGE_FORK_FUND_ACCOUNTS`
//! **unset** (there is no `setBalance` on a real chain), and supply a
//! genuinely funded LP key instead of the dev key. Same harness, config-only.
//!
//! ## Canonical Uniswap v3 Sepolia addresses (defaults below)
//!
//! Verified against the official Uniswap deployment table
//! (docs.uniswap.org/contracts/v3/reference/deployments/ethereum-deployments
//! and the `Uniswap/deployments` data) at implementation time (#1004):
//!
//! | Contract                    | Sepolia address                              |
//! |-----------------------------|----------------------------------------------|
//! | UniswapV3Factory            | `0x0227628f3F023bb0B980b67D528571c95c6DaC1c` |
//! | NonfungiblePositionManager  | `0x1238536071E1c677A632429e3655c799b22cDA52` |
//! | SwapRouter02                | `0x3bFA4769FB09eefC5a80d6E87c3B9C650f7Ae48E` |
//! | WETH9 (Uniswap-canonical)   | `0xfFf9976782d46CC05630D1f6eBAb18b2324d6B14` |

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{
        aliases::{I24, U160, U24},
        Address, B256, U256,
    },
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    sol,
    sol_types::{SolCall, SolValue},
};

use crate::fork_tests::{
    artifact_bytecode, call_u256, deploy, dev_signer, expected_chain_id,
    fund_dev_accounts_if_requested, rpc_url,
};

// ---------------------------------------------------------------------------
// Real Uniswap v3 periphery + token interfaces (forked in from Sepolia state)
// ---------------------------------------------------------------------------
sol! {
    #[allow(missing_docs)]
    interface IUniswapV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool);
    }

    #[allow(missing_docs)]
    interface INonfungiblePositionManager {
        struct MintParams {
            address token0;
            address token1;
            uint24 fee;
            int24 tickLower;
            int24 tickUpper;
            uint256 amount0Desired;
            uint256 amount1Desired;
            uint256 amount0Min;
            uint256 amount1Min;
            address recipient;
            uint256 deadline;
        }
        function createAndInitializePoolIfNecessary(
            address token0,
            address token1,
            uint24 fee,
            uint160 sqrtPriceX96
        ) external payable returns (address pool);
        function mint(MintParams calldata params)
            external
            payable
            returns (uint256 tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
    }

    #[allow(missing_docs)]
    interface ISwapRouter02 {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        function exactInputSingle(ExactInputSingleParams calldata params)
            external
            payable
            returns (uint256 amountOut);
    }

    #[allow(missing_docs)]
    interface IWETH9 {
        function deposit() external payable;
        function approve(address spender, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
    }

    #[allow(missing_docs)]
    interface IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
    }

    #[allow(missing_docs)]
    interface IWrappedBTHMint {
        function bridgeMint(address to, uint256 amount, bytes32 orderId) external;
        function bridgeBurn(uint256 amount, string calldata bthAddress) external;
    }
}

// ---------------------------------------------------------------------------
// Env-gated addresses: canonical Sepolia values are the DEFAULTS, so the same
// harness flips to a live pool by swapping env (the mainnet reuse point).
// ---------------------------------------------------------------------------
const DEFAULT_FACTORY: &str = "0x0227628f3F023bb0B980b67D528571c95c6DaC1c";
const DEFAULT_POSITION_MANAGER: &str = "0x1238536071E1c677A632429e3655c799b22cDA52";
const DEFAULT_SWAP_ROUTER: &str = "0x3bFA4769FB09eefC5a80d6E87c3B9C650f7Ae48E";
const DEFAULT_WETH: &str = "0xfFf9976782d46CC05630D1f6eBAb18b2324d6B14";

fn env_addr(var: &str, default: &str) -> Address {
    std::env::var(var)
        .unwrap_or_else(|_| default.to_string())
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{var} must be a valid 0x address"))
}

// ---------------------------------------------------------------------------
// Small tx helpers (mirroring fork_tests.rs raw-calldata style)
// ---------------------------------------------------------------------------

/// Send a state-changing tx and assert it mined without reverting.
async fn send_tx(provider: &DynProvider, to: Address, input: Vec<u8>) {
    let receipt = provider
        .send_transaction(TransactionRequest::default().with_to(to).with_input(input))
        .await
        .expect("tx accepted")
        .get_receipt()
        .await
        .expect("tx mined");
    assert!(receipt.status(), "tx reverted");
}

/// Send a payable tx carrying `value` wei and assert it mined.
async fn send_tx_value(provider: &DynProvider, to: Address, input: Vec<u8>, value: U256) {
    let receipt = provider
        .send_transaction(
            TransactionRequest::default()
                .with_to(to)
                .with_input(input)
                .with_value(value),
        )
        .await
        .expect("payable tx accepted")
        .get_receipt()
        .await
        .expect("payable tx mined");
    assert!(receipt.status(), "payable tx reverted");
}

/// Tick spacing for the fee tier (v3): 500 -> 10, 3000 -> 60, 10000 -> 200.
fn tick_spacing(fee: u32) -> i32 {
    match fee {
        500 => 10,
        3000 => 60,
        10000 => 200,
        other => panic!("unsupported fee tier {other}"),
    }
}

/// Full-range ticks for a fee tier: MIN_TICK/MAX_TICK snapped inward to a
/// multiple of the tick spacing (an unaligned tick makes `mint` revert).
fn full_range_ticks(fee: u32) -> (i32, i32) {
    const MIN_TICK: i32 = -887272;
    const MAX_TICK: i32 = 887272;
    let spacing = tick_spacing(fee);
    // Rust integer division truncates toward zero, which snaps both bounds
    // inward (>= MIN_TICK, <= MAX_TICK) as Uniswap requires.
    let lower = (MIN_TICK / spacing) * spacing;
    let upper = (MAX_TICK / spacing) * spacing;
    (lower, upper)
}

/// The Uniswap-v3 fork harness: pool create + two-sided liquidity + swap
/// against the real forked Sepolia periphery. Requires a fork node — see the
/// module docs. Self-skips cleanly when no node is reachable.
#[tokio::test]
#[ignore = "requires a Sepolia-fork node (anvil --fork-url) and compiled artifacts; see module docs / scripts/bridge-e2e-fork.sh"]
async fn uniswap_fork_pool_create_add_liquidity_and_swap() {
    let url: alloy::transports::http::reqwest::Url = rpc_url().parse().expect("valid RPC url");

    let deployer = dev_signer(0);
    let lp = dev_signer(3); // LP + swapping user (same account holds the wBTH)
    let lp_addr = lp.address();

    let deploy_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(deployer.clone()))
        .connect_http(url.clone())
        .erased();

    // ---- Self-skip when no fork node is reachable (#992 discipline) ------
    let chain_id = match deploy_provider.get_chain_id().await {
        Ok(id) => id,
        Err(e) => {
            eprintln!(
                "SKIP uniswap_fork_pool_create_add_liquidity_and_swap: no node at {} ({e}). \
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

    // On a fresh fork the dev keys are not pre-funded — mint them test ETH via
    // *_setBalance so the deploy + LP + swap can pay gas with no real funded
    // account. No-op on local 31337 (accounts already funded).
    fund_dev_accounts_if_requested(&deploy_provider).await;

    // ---- Resolve the real (forked) Uniswap periphery + WETH -------------
    let factory = env_addr("BRIDGE_UNISWAP_FACTORY", DEFAULT_FACTORY);
    let position_manager = env_addr("BRIDGE_UNISWAP_POSITION_MANAGER", DEFAULT_POSITION_MANAGER);
    let swap_router = env_addr("BRIDGE_UNISWAP_SWAP_ROUTER", DEFAULT_SWAP_ROUTER);
    let weth = env_addr("BRIDGE_WETH_ADDRESS", DEFAULT_WETH);

    // ---- Deploy a throwaway wBTH; deployer holds MINTER_ROLE ------------
    // (admin / minter / pauser) — deployer as minter so it can mint directly.
    let wbth = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/WrappedBTH.sol/WrappedBTH.json"),
        (deployer.address(), deployer.address(), deployer.address()).abi_encode_params(),
    )
    .await;

    // Mint wBTH to the LP (in picocredits, 12 decimals). 200_000 BTH — well
    // under maxMintPerTx (1M BTH) and the daily/auto-pause limits (10M BTH).
    let wbth_mint: U256 = U256::from(200_000u64) * U256::from(1_000_000_000_000u64);
    send_tx(
        &deploy_provider,
        wbth,
        IWrappedBTHMint::bridgeMintCall {
            to: lp_addr,
            amount: wbth_mint,
            orderId: B256::from([0x11u8; 32]),
        }
        .abi_encode(),
    )
    .await;

    // ---- LP-signed provider for deposit / approve / mint / swap / burn ---
    let lp_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(lp.clone()))
        .connect_http(url.clone())
        .erased();

    // Wrap 1 ETH -> WETH (the LP was funded above / already funded locally).
    let one_eth = U256::from(1_000_000_000_000_000_000u64);
    send_tx_value(
        &lp_provider,
        weth,
        IWETH9::depositCall {}.abi_encode(),
        one_eth,
    )
    .await;

    // ---- Sort into token0/token1 and pick a fee tier --------------------
    let (token0, token1) = if wbth < weth {
        (wbth, weth)
    } else {
        (weth, wbth)
    };
    let fee: u32 = 3000; // 0.30% tier, tick spacing 60
    let (tick_lower, tick_upper) = full_range_ticks(fee);

    // Initialize at a 1:1 raw price: sqrtPriceX96 = sqrt(1) * 2^96 = 2^96.
    // Full-range liquidity means the exact price is not load-bearing; a 1:1
    // interior price simply guarantees both sides are consumed two-sided.
    let sqrt_price_x96: U160 = U160::from(1u8) << 96;
    send_tx(
        &lp_provider,
        position_manager,
        INonfungiblePositionManager::createAndInitializePoolIfNecessaryCall {
            token0,
            token1,
            fee: U24::from(fee),
            sqrtPriceX96: sqrt_price_x96,
        }
        .abi_encode(),
    )
    .await;

    // Pool must now exist in the factory.
    let pool_ret = lp_provider
        .call(
            TransactionRequest::default().with_to(factory).with_input(
                IUniswapV3Factory::getPoolCall {
                    tokenA: token0,
                    tokenB: token1,
                    fee: U24::from(fee),
                }
                .abi_encode(),
            ),
        )
        .await
        .expect("getPool call");
    let pool = IUniswapV3Factory::getPoolCall::abi_decode_returns(&pool_ret).expect("pool address");
    assert_ne!(
        pool,
        Address::ZERO,
        "factory.getPool returned the zero address"
    );

    // ---- Approve both tokens to the position manager, then mint ---------
    send_tx(
        &lp_provider,
        token0,
        IERC20::approveCall {
            spender: position_manager,
            amount: U256::MAX,
        }
        .abi_encode(),
    )
    .await;
    send_tx(
        &lp_provider,
        token1,
        IERC20::approveCall {
            spender: position_manager,
            amount: U256::MAX,
        }
        .abi_encode(),
    )
    .await;

    // Provide an equal raw amount of each token (1:1 price -> both consumed).
    // 10^17 base units: 10^17 pico wBTH (of the 2*10^17 minted) and 0.1 WETH
    // (of the 1 wrapped) — both sides funded, so the position is two-sided.
    let liq_amount = U256::from(100_000_000_000_000_000u64);
    let mint_params = INonfungiblePositionManager::MintParams {
        token0,
        token1,
        fee: U24::from(fee),
        tickLower: I24::try_from(tick_lower).expect("tickLower fits i24"),
        tickUpper: I24::try_from(tick_upper).expect("tickUpper fits i24"),
        amount0Desired: liq_amount,
        amount1Desired: liq_amount,
        amount0Min: U256::ZERO,
        amount1Min: U256::ZERO,
        recipient: lp_addr,
        deadline: U256::from(u64::MAX),
    };
    let mint_input = INonfungiblePositionManager::mintCall {
        params: mint_params,
    }
    .abi_encode();

    // Simulate the mint to read the minted liquidity (a state-changing call
    // returns nothing usable from the receipt), then apply it on-chain.
    let mint_ret = lp_provider
        .call(
            TransactionRequest::default()
                .with_from(lp_addr)
                .with_to(position_manager)
                .with_input(mint_input.clone()),
        )
        .await
        .expect("mint simulates");
    let minted =
        INonfungiblePositionManager::mintCall::abi_decode_returns(&mint_ret).expect("mint return");
    assert!(minted.liquidity > 0, "minted position has zero liquidity");
    assert!(minted.amount0 > U256::ZERO, "token0 side not funded");
    assert!(minted.amount1 > U256::ZERO, "token1 side not funded");
    send_tx(&lp_provider, position_manager, mint_input).await;

    // ---- Swap WETH -> wBTH through the real router ----------------------
    send_tx(
        &lp_provider,
        weth,
        IWETH9::approveCall {
            spender: swap_router,
            amount: U256::MAX,
        }
        .abi_encode(),
    )
    .await;

    let wbth_before = call_u256(
        &lp_provider,
        wbth,
        IERC20::balanceOfCall { account: lp_addr }.abi_encode(),
    )
    .await;
    let weth_before = call_u256(
        &lp_provider,
        weth,
        IWETH9::balanceOfCall { account: lp_addr }.abi_encode(),
    )
    .await;

    // 0.01 WETH in (leaves plenty after the 0.1 WETH provided as liquidity).
    let amount_in = U256::from(10_000_000_000_000_000u64);
    send_tx(
        &lp_provider,
        swap_router,
        ISwapRouter02::exactInputSingleCall {
            params: ISwapRouter02::ExactInputSingleParams {
                tokenIn: weth,
                tokenOut: wbth,
                fee: U24::from(fee),
                recipient: lp_addr,
                amountIn: amount_in,
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: U160::ZERO,
            },
        }
        .abi_encode(),
    )
    .await;

    let wbth_after = call_u256(
        &lp_provider,
        wbth,
        IERC20::balanceOfCall { account: lp_addr }.abi_encode(),
    )
    .await;
    let weth_after = call_u256(
        &lp_provider,
        weth,
        IWETH9::balanceOfCall { account: lp_addr }.abi_encode(),
    )
    .await;

    assert!(
        wbth_after > wbth_before,
        "swap did not increase wBTH ({wbth_before} -> {wbth_after})"
    );
    assert!(
        weth_after < weth_before,
        "swap did not decrease WETH ({weth_before} -> {weth_after})"
    );
    assert_eq!(
        weth_before - weth_after,
        amount_in,
        "exactInputSingle spent exactly amountIn WETH"
    );
    let swapped_wbth = wbth_after - wbth_before;
    eprintln!(
        "uniswap fork: pool {pool:#x}, minted liquidity {}, swapped in {amount_in} WETH -> {swapped_wbth} wBTH",
        minted.liquidity
    );

    // ---- The swapped wBTH is a normal ERC-20 balance the user can redeem -
    // Prove it: bridgeBurn the swap proceeds (the repatriation hook #1005 will
    // wire) and confirm the balance drops — closing the DeFi round trip's
    // "purchase wBTH, then send it home" half.
    send_tx(
        &lp_provider,
        wbth,
        IWrappedBTHMint::bridgeBurnCall {
            amount: swapped_wbth,
            bthAddress: "bth_repatriation_destination_stealth_addr".to_string(),
        }
        .abi_encode(),
    )
    .await;
    let wbth_final = call_u256(
        &lp_provider,
        wbth,
        IERC20::balanceOfCall { account: lp_addr }.abi_encode(),
    )
    .await;
    assert_eq!(
        wbth_final,
        wbth_after - swapped_wbth,
        "bridgeBurn of swap proceeds did not reduce the wBTH balance"
    );
}
