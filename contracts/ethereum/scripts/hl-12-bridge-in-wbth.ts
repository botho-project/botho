// #877 step 4/5: bridge wBTH from HyperEVM into HyperCore spot.
//
// After hl-11's link, an ERC-20 transfer of the PeerToken to the token's Core
// system address (0x2000..{index}) credits the SENDER's HyperCore spot
// balance. The deployer holds 10 wBTH on HyperEVM (minted by the #1026 NTT
// round trip, backed 1:1 by wBTH locked on Sepolia).
//
// Amounts MUST be multiples of 1e4 PeerToken wei (evmExtraWeiDecimals = 4) or
// the non-round remainder is burned by the Core credit.
//
// Run: HL_TOKEN_INDEX=<idx> [HL_BRIDGE_WBTH=10] npx ts-node
//   --compiler-options '{"module":"commonjs","target":"ES2020",
//   "esModuleInterop":true,"skipLibCheck":true}' scripts/hl-12-bridge-in-wbth.ts
import { ethers } from "ethers";
import {
  CORE_TOKEN_NAME, DEPLOYER, HYPEREVM_RPC, loadWallet, PEER_TOKEN,
  spotBalances, systemAddress,
} from "./hl-lib";

const AMOUNT_WBTH = process.env.HL_BRIDGE_WBTH ?? "10"; // default: everything from the #1026 round trip

async function main() {
  const w = loadWallet();
  if (w.address !== DEPLOYER) throw new Error(`expected deployer ${DEPLOYER}, got ${w.address}`);
  const tokenIndex = parseInt(process.env.HL_TOKEN_INDEX ?? "", 10);
  if (!Number.isInteger(tokenIndex)) throw new Error("set HL_TOKEN_INDEX to the hl-10 token index");
  const sysAddr = systemAddress(tokenIndex);

  const amount = ethers.parseUnits(AMOUNT_WBTH, 12);
  if (amount % 10000n !== 0n) throw new Error("amount must be a multiple of 1e-8 wBTH (evmExtraWeiDecimals rounding burns the rest)");

  const evm = new ethers.JsonRpcProvider(HYPEREVM_RPC);
  const erc20 = new ethers.Contract(PEER_TOKEN, [
    "function balanceOf(address) view returns (uint256)",
    "function transfer(address,uint256) returns (bool)",
  ], w.connect(evm));

  const evmBefore = await erc20.balanceOf(DEPLOYER);
  const coreBefore = (await spotBalances(DEPLOYER)).get(CORE_TOKEN_NAME) ?? 0;
  console.log(`EVM wBTH: ${ethers.formatUnits(evmBefore, 12)}  Core ${CORE_TOKEN_NAME}: ${coreBefore}`);
  if (evmBefore < amount) throw new Error("insufficient EVM wBTH");

  console.log(`transferring ${AMOUNT_WBTH} wBTH -> Core system address ${sysAddr}...`);
  const tx = await erc20.transfer(sysAddr, amount);
  console.log("EVM tx:", tx.hash, `\n  https://testnet.purrsec.com/tx/${tx.hash}`);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("transfer reverted");

  // Core credit arrives via a system transaction shortly after the EVM block
  process.stdout.write("waiting for HyperCore credit");
  for (let i = 0; i < 30; i++) {
    await new Promise((r) => setTimeout(r, 4000));
    const coreNow = (await spotBalances(DEPLOYER)).get(CORE_TOKEN_NAME) ?? 0;
    process.stdout.write(".");
    if (coreNow > coreBefore) {
      console.log(`\nCore ${CORE_TOKEN_NAME} balance: ${coreBefore} -> ${coreNow}`);
      console.log(`EVM wBTH left: ${ethers.formatUnits(await erc20.balanceOf(DEPLOYER), 12)}`);
      console.log("\n=== wBTH BRIDGED ONTO HYPERCORE SPOT — run hl-13-swap-demo.ts ===");
      return;
    }
  }
  throw new Error("\nCore credit not observed within 2 min — check the link (hl-11) and system address");
}
main().catch((e) => { console.error(e); process.exit(1); });
