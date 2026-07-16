// HL official-route step 1/3: bridge ETH from Ethereum mainnet -> Arbitrum via
// the CANONICAL Arbitrum Delayed Inbox (depositEth). Real mainnet value.
// Threads the 0.08 Chainstack gate: moves only 0.004 ETH so 0x8E90 stays >=0.08.
//
// Run: npx ts-node --compiler-options '{"module":"commonjs","target":"ES2020",
//   "esModuleInterop":true,"skipLibCheck":true}' scripts/hl-1-bridge-eth-to-arbitrum.ts

import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const ETH_RPC = "https://ethereum-rpc.publicnode.com";
const INBOX = "0x4Dbd4fc535Ac27206064B68FfCf827b0A60BAB3f"; // Arbitrum One Delayed Inbox (verified: has code)
const BRIDGE_ETH = ethers.parseEther("0.004"); // ~$7.5 — enough for a 6 USDC deposit + Arbitrum gas
const GATE = ethers.parseEther("0.08"); // Chainstack faucet balance gate — must stay >= this
const KEYFILE = path.resolve(__dirname, "../../../.secrets/bridge-mainnet/eth-botho.key");

const INBOX_ABI = ["function depositEth() payable returns (uint256)"];

async function main() {
  const provider = new ethers.JsonRpcProvider(ETH_RPC);
  const net = await provider.getNetwork();
  if (net.chainId !== 1n) throw new Error(`expected Ethereum mainnet (1), got ${net.chainId}`);
  const wallet = new ethers.Wallet(fs.readFileSync(KEYFILE, "utf8").trim(), provider);
  console.log("from:", wallet.address);
  if (wallet.address.toLowerCase() !== "0x8e9043051a39bc87d969c060d5a2fa5f577844f3")
    throw new Error("unexpected signer address");

  const bal0 = await provider.getBalance(wallet.address);
  console.log(`balance: ${ethers.formatEther(bal0)} ETH`);

  const inbox = new ethers.Contract(INBOX, INBOX_ABI, wallet);
  const data = inbox.interface.encodeFunctionData("depositEth", []);
  console.log("Inbox:", INBOX, "| calldata:", data, "(expect 0x439370b1)");
  if (data !== "0x439370b1") throw new Error(`unexpected depositEth selector ${data}`);

  // Gas + safety: after this tx, balance MUST remain >= 0.08 (Chainstack gate).
  const feeData = await provider.getFeeData();
  const gasEst = await provider.estimateGas({ to: INBOX, value: BRIDGE_ETH, data, from: wallet.address });
  const maxGasCost = gasEst * (feeData.maxFeePerGas ?? feeData.gasPrice ?? 0n);
  const after = bal0 - BRIDGE_ETH - maxGasCost;
  console.log(`bridging ${ethers.formatEther(BRIDGE_ETH)} ETH; gasEst ${gasEst}, maxGasCost ~${ethers.formatEther(maxGasCost)} ETH`);
  console.log(`projected balance after: ~${ethers.formatEther(after)} ETH (gate ${ethers.formatEther(GATE)})`);
  if (after < GATE) throw new Error(`ABORT: would drop below the 0.08 Chainstack gate (${ethers.formatEther(after)} < 0.08)`);

  const tx = await inbox.depositEth({ value: BRIDGE_ETH });
  console.log("bridge tx:", tx.hash, `\n  https://etherscan.io/tx/${tx.hash}`);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("bridge tx reverted");
  console.log("mined in block", rc.blockNumber);

  const bal1 = await provider.getBalance(wallet.address);
  console.log(`balance now: ${ethers.formatEther(bal1)} ETH (gate ${ethers.formatEther(GATE)}: ${bal1 >= GATE ? "SAFE" : "BELOW!"})`);
  console.log("\nBridged ETH credits on Arbitrum in ~10-15 min. Next: poll Arbitrum balance, then swap ETH->USDC.");
}

main().catch((e) => { console.error(e); process.exit(1); });
