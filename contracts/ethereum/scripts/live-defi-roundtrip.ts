// Live wBTH DeFi round trip on Sepolia (#866/#868/#869 — the mainnet
// liquidity-launch, run on testnet). Mirrors the fork-tested harness
// (bridge/service/src/{uniswap_fork_tests,defi_round_trip_tests}.rs) but drives
// the ALREADY-DEPLOYED, Etherscan-verified wBTH + the REAL 2-of-3 Gnosis Safe:
//
//   1. FUND   — deployer -> LP EOA (WETH side + gas)
//   2. MINT   — 100,000 wBTH to the LP via Safe.execTransaction(bridgeMint),
//               2-of-3 owner secp256k1 sigs, relayed by the role-less deployer
//               (ADR-0002 custody, exactly as bridge/service/src/mint/ethereum.rs)
//   3. POOL   — LP wraps ETH->WETH, creates the wBTH/WETH v3 pool (0.30% tier,
//               1:1 raw price), adds a full-range two-sided position
//   4. SWAP   — 0.01 WETH -> wBTH through SwapRouter02.exactInputSingle
//   5. BURN   — bridgeBurn the swap proceeds (Ethereum-side repatriation; the
//               native-BTH release leg is Layer 2, blocked on a live node)
//
// Idempotent: each step reads chain state first and skips work already done, so
// a re-run after a mid-way failure resumes rather than double-spends.
//
// Run:  npx hardhat run scripts/live-defi-roundtrip.ts --network sepolia
// Secrets: keys are read from ../../.secrets/bridge-testnet/*.key (git-ignored).

import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

// --- Fixed deployment (Sepolia) ---------------------------------------------
const WBTH = "0x49b985ec427ee771a601f11b18f7d4402fa2dd7b";
const SAFE = "0x61274F558f9027e2D402d3340dE89152FA3F3947";

// Canonical Uniswap v3 periphery on Sepolia (matches uniswap_fork_tests.rs).
const UNI = {
  factory: "0x0227628f3F023bb0B980b67D528571c95c6DaC1c",
  positionManager: "0x1238536071E1c677A632429e3655c799b22cDA52",
  swapRouter: "0x3bFA4769FB09eefC5a80d6E87c3B9C650f7Ae48E",
  weth: "0xfFf9976782d46CC05630D1f6eBAb18b2324d6B14",
};

// --- Amounts (match defi_round_trip_tests.rs) -------------------------------
const WBTH_LIQ = 100_000_000_000_000_000n; // 10^17 pico = 100,000 wBTH
const WETH_LIQ = 100_000_000_000_000_000n; // 10^17 wei  = 0.1 WETH
const SWAP_IN = 10_000_000_000_000_000n; //   10^16 wei  = 0.01 WETH
const FEE = 3000; // 0.30% tier

// Slippage bounds. Zero is safe HERE only because this script is Sepolia-only
// (chainId guard below) and seeds a fresh, empty pool where there is no MEV to
// sandwich. A mainnet port MUST set non-zero bounds (#1017): SLIPPAGE_BPS
// clamps the liquidity add against amount*Desired, and SWAP_MIN_OUT sets the
// swap's amountOutMinimum (best derived from a live Quoter quote off-chain).
const SLIPPAGE_BPS = BigInt(process.env.SLIPPAGE_BPS ?? "0"); // 0 = no bound
const SWAP_MIN_OUT = BigInt(process.env.SWAP_MIN_OUT ?? "0"); // wBTH base units
const minBound = (desired: bigint) => (desired * (10_000n - SLIPPAGE_BPS)) / 10_000n;
const TICK_SPACING = 60; // fee 3000 -> spacing 60
const LP_ETH_FUND = ethers.parseEther("0.3"); // WETH (0.11) + gas headroom
const WETH_WRAP = WETH_LIQ + SWAP_IN; // 0.11 WETH total the LP needs

// A stable, clearly-labelled order id for this governance liquidity mint
// (replay-proof: the token records processedOrders[orderId]).
const ORDER_ID = ethers.id("wbth-sepolia-liquidity-bootstrap-2026-07-16");

// --- ABIs (minimal) ---------------------------------------------------------
const WBTH_ABI = [
  "function balanceOf(address) view returns (uint256)",
  "function totalSupply() view returns (uint256)",
  "function paused() view returns (bool)",
  "function decimals() view returns (uint8)",
  "function processedOrders(bytes32) view returns (bool)",
  "function bridgeMint(address to, uint256 amount, bytes32 orderId)",
  "function bridgeBurn(uint256 amount, string bthAddress)",
  "function approve(address spender, uint256 amount) returns (bool)",
];
const SAFE_ABI = [
  "function nonce() view returns (uint256)",
  "function getThreshold() view returns (uint256)",
  "function getOwners() view returns (address[])",
  "function getTransactionHash(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 _nonce) view returns (bytes32)",
  "function execTransaction(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address payable refundReceiver,bytes signatures) payable returns (bool)",
];
const WETH_ABI = [
  "function balanceOf(address) view returns (uint256)",
  "function deposit() payable",
  "function approve(address spender, uint256 amount) returns (bool)",
];
const FACTORY_ABI = [
  "function getPool(address,address,uint24) view returns (address)",
];
const NPM_ABI = [
  "function createAndInitializePoolIfNecessary(address token0,address token1,uint24 fee,uint160 sqrtPriceX96) payable returns (address pool)",
  "function mint((address token0,address token1,uint24 fee,int24 tickLower,int24 tickUpper,uint256 amount0Desired,uint256 amount1Desired,uint256 amount0Min,uint256 amount1Min,address recipient,uint256 deadline)) payable returns (uint256 tokenId,uint128 liquidity,uint256 amount0,uint256 amount1)",
];
const ROUTER_ABI = [
  "function exactInputSingle((address tokenIn,address tokenOut,uint24 fee,address recipient,uint256 amountIn,uint256 amountOutMinimum,uint160 sqrtPriceLimitX96)) payable returns (uint256 amountOut)",
];

// ---------------------------------------------------------------------------
const SECRETS = path.resolve(__dirname, "../../../.secrets/bridge-testnet");
function loadKey(name: string): string {
  return fs.readFileSync(path.join(SECRETS, `${name}.key`), "utf8").trim();
}

function fmt(pico: bigint): string {
  return `${ethers.formatUnits(pico, 12)} wBTH (${pico} pico)`;
}

async function waitTx(label: string, txp: Promise<ethers.TransactionResponse>) {
  const tx = await txp;
  console.log(`   ${label}: ${tx.hash}`);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error(`${label} reverted (${tx.hash})`);
  console.log(`   ${label}: mined in block ${rc.blockNumber}`);
  return rc;
}

async function main() {
  const rpc = process.env.SEPOLIA_RPC_URL;
  if (!rpc) throw new Error("SEPOLIA_RPC_URL not set (contracts/ethereum/.env)");
  const provider = new ethers.JsonRpcProvider(rpc);
  const net = await provider.getNetwork();
  if (net.chainId !== 11155111n)
    throw new Error(`expected Sepolia (11155111), got ${net.chainId}`);

  const deployer = new ethers.Wallet(loadKey("eth-deployer"), provider);
  const lp = new ethers.Wallet(loadKey("eth-lp"), provider);
  const owners = [
    new ethers.Wallet(loadKey("eth-safe-owner-1"), provider),
    new ethers.Wallet(loadKey("eth-safe-owner-2"), provider),
    new ethers.Wallet(loadKey("eth-safe-owner-3"), provider),
  ];

  const wbth = new ethers.Contract(WBTH, WBTH_ABI, provider);
  const safe = new ethers.Contract(SAFE, SAFE_ABI, provider);
  const weth = new ethers.Contract(UNI.weth, WETH_ABI, provider);
  const factory = new ethers.Contract(UNI.factory, FACTORY_ABI, provider);

  console.log("=== wBTH live DeFi round trip (Sepolia) ===");
  console.log("deployer :", deployer.address);
  console.log("LP/user  :", lp.address);
  console.log("wBTH     :", WBTH, "| Safe:", SAFE);

  // ---- Preflight ----------------------------------------------------------
  if (await wbth.paused()) throw new Error("wBTH is PAUSED — aborting");
  const threshold = await safe.getThreshold();
  console.log(`Safe threshold: ${threshold} of ${(await safe.getOwners()).length}`);

  // ---- Step 1: FUND LP ----------------------------------------------------
  console.log("\n[1/5] Fund LP EOA");
  const lpBal = await provider.getBalance(lp.address);
  if (lpBal < LP_ETH_FUND) {
    await waitTx(
      "fund",
      deployer.sendTransaction({ to: lp.address, value: LP_ETH_FUND - lpBal }),
    );
  } else {
    console.log(`   LP already holds ${ethers.formatEther(lpBal)} ETH — skip`);
  }

  // ---- Step 2: MINT via 2-of-3 Safe --------------------------------------
  console.log("\n[2/5] Mint wBTH via 2-of-3 Safe");
  const already = await wbth.processedOrders(ORDER_ID);
  const lpWbth0 = await wbth.balanceOf(lp.address);
  if (already) {
    console.log(`   orderId already processed; LP holds ${fmt(lpWbth0)} — skip mint`);
  } else {
    const data = wbth.interface.encodeFunctionData("bridgeMint", [
      lp.address,
      WBTH_LIQ,
      ORDER_ID,
    ]);
    const safeNonce = await safe.nonce();
    const Z = ethers.ZeroAddress;
    const safeTxHash: string = await safe.getTransactionHash(
      WBTH, 0n, data, 0, 0n, 0n, 0n, Z, Z, safeNonce,
    );
    // Sign the raw 32-byte SafeTx hash with 2 owners; Gnosis Safe wants the
    // 65-byte {r,s,v} blob, owners sorted ascending by address.
    const signers = owners.slice(0, Number(threshold));
    const parts = signers
      .map((w) => ({
        addr: w.address.toLowerCase(),
        sig: ethers.Signature.from(w.signingKey.sign(safeTxHash)).serialized,
      }))
      .sort((a, b) => (a.addr < b.addr ? -1 : 1));
    const sigBlob = "0x" + parts.map((p) => p.sig.slice(2)).join("");
    console.log(`   SafeTx nonce ${safeNonce}, hash ${safeTxHash}`);
    console.log(`   signed by ${signers.map((s) => s.address).join(", ")}`);
    await waitTx(
      "execTransaction(bridgeMint)",
      (safe.connect(deployer) as ethers.Contract).execTransaction(
        WBTH, 0n, data, 0, 0n, 0n, 0n, Z, Z, sigBlob,
      ),
    );
    const lpWbth1 = await wbth.balanceOf(lp.address);
    console.log(`   LP wBTH: ${fmt(lpWbth0)} -> ${fmt(lpWbth1)}`);
    if (lpWbth1 - lpWbth0 !== WBTH_LIQ)
      throw new Error(`mint delta mismatch: ${lpWbth1 - lpWbth0} != ${WBTH_LIQ}`);
  }

  // ---- Step 3: POOL + LIQUIDITY ------------------------------------------
  console.log("\n[3/5] Wrap WETH + create pool + add liquidity");
  const wethBal = await weth.balanceOf(lp.address);
  if (wethBal < WETH_WRAP) {
    await waitTx(
      "WETH.deposit",
      (weth.connect(lp) as ethers.Contract).deposit({ value: WETH_WRAP - wethBal }),
    );
  } else {
    console.log(`   LP already holds ${ethers.formatEther(wethBal)} WETH — skip wrap`);
  }

  // Sort tokens: wBTH (0x49..) < WETH (0xff..) => token0 = wBTH.
  const wbthLt = WBTH.toLowerCase() < UNI.weth.toLowerCase();
  const token0 = wbthLt ? WBTH : UNI.weth;
  const token1 = wbthLt ? UNI.weth : WBTH;
  const amount0 = wbthLt ? WBTH_LIQ : WETH_LIQ;
  const amount1 = wbthLt ? WETH_LIQ : WBTH_LIQ;
  const sqrtPriceX96 = 1n << 96n; // 1:1 raw price

  const npm = new ethers.Contract(UNI.positionManager, NPM_ABI, lp);
  let pool: string = await factory.getPool(token0, token1, FEE);
  if (pool === ethers.ZeroAddress) {
    await waitTx(
      "createAndInitializePoolIfNecessary",
      npm.createAndInitializePoolIfNecessary(token0, token1, FEE, sqrtPriceX96),
    );
    pool = await factory.getPool(token0, token1, FEE);
  } else {
    console.log(`   pool already exists at ${pool} — skip create`);
  }
  if (pool === ethers.ZeroAddress) throw new Error("factory.getPool == 0 after create");
  console.log(`   pool: ${pool}`);

  // Approve both tokens to the position manager.
  await waitTx("approve token0->NPM",
    (new ethers.Contract(token0, WBTH_ABI, lp)).approve(UNI.positionManager, ethers.MaxUint256));
  await waitTx("approve token1->NPM",
    (new ethers.Contract(token1, WBTH_ABI, lp)).approve(UNI.positionManager, ethers.MaxUint256));

  // Full-range ticks snapped to spacing.
  const tickLower = Math.ceil(-887272 / TICK_SPACING) * TICK_SPACING;
  const tickUpper = Math.floor(887272 / TICK_SPACING) * TICK_SPACING;
  const mintParams = {
    token0, token1, fee: FEE, tickLower, tickUpper,
    amount0Desired: amount0, amount1Desired: amount1,
    amount0Min: minBound(amount0), amount1Min: minBound(amount1),
    recipient: lp.address, deadline: BigInt(2n ** 63n),
  };
  const sim = await npm.mint.staticCall(mintParams);
  console.log(`   mint (sim): liquidity=${sim[1]} amount0=${sim[2]} amount1=${sim[3]}`);
  await waitTx("mint(position)", npm.mint(mintParams));
  if (sim[1] === 0n) throw new Error("minted liquidity is zero");

  // ---- Step 4: SWAP -------------------------------------------------------
  console.log("\n[4/5] Swap WETH -> wBTH");
  await waitTx("approve WETH->router",
    (weth.connect(lp) as ethers.Contract).approve(UNI.swapRouter, ethers.MaxUint256));
  const router = new ethers.Contract(UNI.swapRouter, ROUTER_ABI, lp);
  const wbthBeforeSwap = await wbth.balanceOf(lp.address);
  const swapParams = {
    tokenIn: UNI.weth, tokenOut: WBTH, fee: FEE, recipient: lp.address,
    amountIn: SWAP_IN, amountOutMinimum: SWAP_MIN_OUT, sqrtPriceLimitX96: 0n,
  };
  await waitTx("exactInputSingle", router.exactInputSingle(swapParams));
  const wbthAfterSwap = await wbth.balanceOf(lp.address);
  const wbthOut = wbthAfterSwap - wbthBeforeSwap;
  console.log(`   swap produced ${fmt(wbthOut)} for ${ethers.formatEther(SWAP_IN)} WETH`);
  if (wbthOut <= 0n) throw new Error("swap produced no wBTH");

  // ---- Step 5: BURN (repatriate) -----------------------------------------
  console.log("\n[5/5] bridgeBurn swap proceeds (Ethereum-side repatriation)");
  const supplyBefore = await wbth.totalSupply();
  await waitTx("bridgeBurn",
    (wbth.connect(lp) as ethers.Contract).bridgeBurn(wbthOut, "bth-testnet-repatriation-demo"));
  const supplyAfter = await wbth.totalSupply();
  console.log(`   totalSupply: ${fmt(supplyBefore)} -> ${fmt(supplyAfter)}`);
  if (supplyBefore - supplyAfter !== wbthOut)
    throw new Error("burn did not reduce supply by the swap output");

  console.log("\n=== ROUND TRIP COMPLETE ===");
  console.log(`pool: https://sepolia.etherscan.io/address/${pool}`);
  console.log(`wBTH: https://sepolia.etherscan.io/address/${WBTH}`);
  console.log("NOTE: native-BTH release leg (Layer 2) is separate — needs a live");
  console.log("Botho node + watcher (#866/#868); this proves the Ethereum side.");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
