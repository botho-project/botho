// HL official-route step 2/3: swap ETH -> native USDC on Arbitrum via Uniswap v3
// SwapRouter02 (0.05% tier). Pays with native ETH (msg.value) — SwapRouter02
// wraps to WETH internally. Quotes first, sets a 1% min-out.
//
// Run: npx ts-node --compiler-options '{"module":"commonjs","target":"ES2020",
//   "esModuleInterop":true,"skipLibCheck":true}' scripts/hl-2-swap-eth-usdc-arbitrum.ts

import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const ARB_RPC = "https://arb1.arbitrum.io/rpc";
const ROUTER = "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45"; // SwapRouter02 (verified)
const QUOTER = "0x61fFE014bA17989E743c5F6cB21bF9697530B21e"; // QuoterV2
const WETH = "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1";
const USDC = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"; // native USDC (verified: 6 dec)
const FEE = 500; // 0.05%
const AMOUNT_IN = ethers.parseEther("0.0038"); // leave ~0.0002 ETH for gas on this + the deposit tx
const KEYFILE = path.resolve(__dirname, "../../../.secrets/bridge-mainnet/eth-botho.key");

const QUOTER_ABI = ["function quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96)) returns (uint256 amountOut,uint160,uint32,uint256)"];
const ROUTER_ABI = ["function exactInputSingle((address tokenIn,address tokenOut,uint24 fee,address recipient,uint256 amountIn,uint256 amountOutMinimum,uint160 sqrtPriceLimitX96)) payable returns (uint256 amountOut)"];
const USDC_ABI = ["function balanceOf(address) view returns (uint256)"];

async function main() {
  const provider = new ethers.JsonRpcProvider(ARB_RPC);
  const net = await provider.getNetwork();
  if (net.chainId !== 42161n) throw new Error(`expected Arbitrum One (42161), got ${net.chainId}`);
  const wallet = new ethers.Wallet(fs.readFileSync(KEYFILE, "utf8").trim(), provider);
  console.log("from:", wallet.address);

  const ethBal = await provider.getBalance(wallet.address);
  console.log(`Arbitrum ETH: ${ethers.formatEther(ethBal)}`);
  if (ethBal < AMOUNT_IN) throw new Error("insufficient Arbitrum ETH for the swap");

  const usdc = new ethers.Contract(USDC, USDC_ABI, provider);
  const usdc0 = await usdc.balanceOf(wallet.address);

  // Quote (staticCall — QuoterV2 is non-view but returns via revert-decode).
  const quoter = new ethers.Contract(QUOTER, QUOTER_ABI, provider);
  const q = await quoter.quoteExactInputSingle.staticCall({
    tokenIn: WETH, tokenOut: USDC, amountIn: AMOUNT_IN, fee: FEE, sqrtPriceLimitX96: 0n,
  });
  const quotedOut = q[0] as bigint;
  const minOut = (quotedOut * 99n) / 100n; // 1% slippage
  console.log(`quote: ${ethers.formatUnits(quotedOut, 6)} USDC for ${ethers.formatEther(AMOUNT_IN)} ETH; minOut ${ethers.formatUnits(minOut, 6)}`);
  if (quotedOut < 6_000_000n) throw new Error(`ABORT: quote ${ethers.formatUnits(quotedOut,6)} USDC < 6 (need >=6 to deposit)`);

  const router = new ethers.Contract(ROUTER, ROUTER_ABI, wallet);
  const tx = await router.exactInputSingle(
    { tokenIn: WETH, tokenOut: USDC, fee: FEE, recipient: wallet.address, amountIn: AMOUNT_IN, amountOutMinimum: minOut, sqrtPriceLimitX96: 0n },
    { value: AMOUNT_IN },
  );
  console.log("swap tx:", tx.hash, `\n  https://arbiscan.io/tx/${tx.hash}`);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("swap reverted");

  const usdc1 = await usdc.balanceOf(wallet.address);
  console.log(`USDC: ${ethers.formatUnits(usdc0,6)} -> ${ethers.formatUnits(usdc1,6)} (+${ethers.formatUnits(usdc1-usdc0,6)})`);
  console.log(`Arbitrum ETH left: ${ethers.formatEther(await provider.getBalance(wallet.address))}`);
  console.log("\nNext: deposit 6 USDC to Hyperliquid Bridge2.");
}

main().catch((e) => { console.error(e); process.exit(1); });
