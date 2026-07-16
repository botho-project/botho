// HL official-route step 3/3: deposit USDC to Hyperliquid mainnet by transferring
// native USDC to the Bridge2 contract on Arbitrum. This establishes the
// "deposited on mainnet with the same address" fact that unlocks the TESTNET
// faucet for 0x8E90. Min deposit 5 USDC (below = lost forever); we send 6.
//
// Run: npx ts-node --compiler-options '{"module":"commonjs","target":"ES2020",
//   "esModuleInterop":true,"skipLibCheck":true}' scripts/hl-3-deposit-hyperliquid.ts

import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const ARB_RPC = "https://arb1.arbitrum.io/rpc";
const USDC = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"; // native USDC (verified)
const BRIDGE2 = "0x2Df1c51E09aECF9cacB7bc98cB1742757f163dF7"; // Hyperliquid Bridge2 (verified: 38.8KB code, official docs + Arbiscan)
const DEPOSIT = 6_000_000n; // 6 USDC (6 decimals) — above the 5 USDC minimum
const KEYFILE = path.resolve(__dirname, "../../../.secrets/bridge-mainnet/eth-botho.key");

const USDC_ABI = [
  "function balanceOf(address) view returns (uint256)",
  "function transfer(address to, uint256 amount) returns (bool)",
  "function decimals() view returns (uint8)",
  "function symbol() view returns (string)",
];

async function main() {
  const provider = new ethers.JsonRpcProvider(ARB_RPC);
  if ((await provider.getNetwork()).chainId !== 42161n) throw new Error("not Arbitrum One");
  const wallet = new ethers.Wallet(fs.readFileSync(KEYFILE, "utf8").trim(), provider);
  console.log("from:", wallet.address);

  const usdc = new ethers.Contract(USDC, USDC_ABI, wallet);
  // Re-verify the token before sending real value.
  if ((await usdc.symbol()) !== "USDC" || Number(await usdc.decimals()) !== 6)
    throw new Error("USDC token sanity check failed");
  const bal = await usdc.balanceOf(wallet.address);
  console.log(`USDC balance: ${ethers.formatUnits(bal, 6)}`);
  if (bal < DEPOSIT) throw new Error(`insufficient USDC: have ${ethers.formatUnits(bal,6)}, need ${ethers.formatUnits(DEPOSIT,6)}`);
  if (DEPOSIT < 5_000_000n) throw new Error("ABORT: below Hyperliquid 5 USDC minimum");

  // Confirm the bridge contract exists (guard against a typo'd address = lost funds).
  if ((await provider.getCode(BRIDGE2)) === "0x") throw new Error("ABORT: Bridge2 has no code");

  console.log(`depositing ${ethers.formatUnits(DEPOSIT, 6)} USDC -> Hyperliquid Bridge2 ${BRIDGE2}`);
  const tx = await usdc.transfer(BRIDGE2, DEPOSIT);
  console.log("deposit tx:", tx.hash, `\n  https://arbiscan.io/tx/${tx.hash}`);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("deposit transfer reverted");
  console.log("mined in block", rc.blockNumber);

  console.log(`USDC left on Arbitrum: ${ethers.formatUnits(await usdc.balanceOf(wallet.address), 6)}`);
  console.log("\n=== DEPOSITED ON HYPERLIQUID MAINNET ===");
  console.log("Credits to the HL account for", wallet.address, "in a few min (Arbitrum finality).");
  console.log("This unlocks the TESTNET faucet for this address. Next: claim testnet USDC,");
  console.log("sell for HYPE, bridge HYPE HyperCore->HyperEVM, forward to 0x111018 (ntt deployer).");
}

main().catch((e) => { console.error(e); process.exit(1); });
