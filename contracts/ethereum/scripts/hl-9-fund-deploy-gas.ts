// #877 step 1/5: acquire the HIP-1 deploy gas on Hyperliquid testnet.
//
// FINDING (2026-07-16): the spot deploy Dutch auction is denominated in HYPE,
// not USDC ("Gas is deducted from the deployer's spot HYPE balance", floor
// 500 HYPE), and testnet HYPE trades at ~$32-52 mock-USDC on the HYPE/USDC
// book — so the "free testnet deploy" assumption in #877 is wrong: deploy gas
// is ~16k-26k mock USDC worth of HYPE. Getting there needs operator help (see
// the runbook); this script converts whatever the deployer DOES have into
// spot HYPE and reports the remaining shortfall:
//   1. bridges the deployer's HyperEVM native HYPE -> HyperCore spot HYPE
//      (send to system address 0x2222...2222), keeping a little for EVM gas
//   2. moves perp USDC -> spot USDC (usdClassTransfer)
//   3. IOC-buys HYPE with spare spot USDC (keeps USDC_RESERVE for HIP-2
//      seeding + the hl-13 swap demo)
//
// Optional funder mode: HL_FUNDER_KEYFILE=<key> HL_FUND_USDC=<amt> sends perp
// USDC from a (mainnet-activated, drip-eligible) funder account to the
// deployer first. claimDrip is UNSIGNED (plain info request) and is attempted
// for the funder automatically. Run every drip cooldown until funded.
//
// Run: npx ts-node --compiler-options '{"module":"commonjs","target":"ES2020",
//   "esModuleInterop":true,"skipLibCheck":true}' scripts/hl-9-fund-deploy-gas.ts
import { ethers } from "ethers";
import * as fs from "fs";
import {
  DEPLOYER, HYPEREVM_RPC, fmt, info, loadWallet, perpUsdc, sendL1Action,
  spotBalances, usdClassTransfer, usdSend, wire,
} from "./hl-lib";

const HYPE_SYSTEM = "0x2222222222222222222222222222222222222222"; // EVM->Core HYPE bridge
const HYPE_PAIR = "@1035"; // HYPE/USDC on testnet (tokens [1105, 0])
const HYPE_ASSET = 10000 + 1035;
const TARGET_HYPE = parseFloat(process.env.HL_TARGET_HYPE ?? "505"); // auction floor 500 + buffer
const USDC_RESERVE = parseFloat(process.env.HL_USDC_RESERVE ?? "40"); // HIP-2 seed (~20) + swap demo (~12) + fees
const EVM_GAS_KEEP = ethers.parseEther("0.3"); // HYPE kept on the EVM side for hl-12

async function bestAsk(): Promise<number | null> {
  const book = await info({ type: "l2Book", coin: HYPE_PAIR });
  const ask = book?.levels?.[1]?.[0];
  return ask ? parseFloat(ask.px) : null;
}

async function main() {
  const w = loadWallet();
  if (w.address !== DEPLOYER) throw new Error(`expected deployer ${DEPLOYER}, got ${w.address}`);

  // optional funder leg (operator provides the keyfile; NOT run by automation)
  const funderKeyfile = process.env.HL_FUNDER_KEYFILE;
  if (funderKeyfile) {
    const funder = new ethers.Wallet(fs.readFileSync(funderKeyfile, "utf8").trim());
    console.log("funder:", funder.address);
    const drip = await info({ type: "claimDrip", user: funder.address }); // unsigned faucet claim
    console.log("claimDrip:", JSON.stringify(drip)); // null = claimed 1000 mock USDC; else the error string
    const amt = process.env.HL_FUND_USDC;
    if (amt) {
      console.log(`usdSend ${amt} USDC funder -> deployer (1 USDC fee)...`);
      console.log(JSON.stringify(await usdSend(funder, DEPLOYER, wire(amt))));
    }
  }

  // status
  const evm = new ethers.JsonRpcProvider(HYPEREVM_RPC);
  const evmHype = await evm.getBalance(DEPLOYER);
  const spot = await spotBalances(DEPLOYER);
  const perp = await perpUsdc(DEPLOYER);
  console.log(`deployer EVM HYPE: ${ethers.formatEther(evmHype)}  Core spot HYPE: ${spot.get("HYPE") ?? 0}`);
  console.log(`deployer perp USDC: ${perp}  spot USDC: ${spot.get("USDC") ?? 0}`);
  const auction = (await info({ type: "spotDeployState", user: DEPLOYER })).gasAuction;
  console.log(`gas auction: current ${auction.currentGas} HYPE (start ${auction.startGas}, floor 500)`);

  // 1. bridge EVM HYPE -> Core spot HYPE
  if (evmHype > EVM_GAS_KEEP + ethers.parseEther("0.1")) {
    const send = evmHype - EVM_GAS_KEEP;
    console.log(`bridging ${ethers.formatEther(send)} HYPE EVM -> Core (to ${HYPE_SYSTEM})...`);
    const tx = await w.connect(evm).sendTransaction({ to: HYPE_SYSTEM, value: send });
    console.log("tx:", tx.hash);
    await tx.wait();
  }

  // 2. perp USDC -> spot USDC (keep $1 in perp)
  if (perp > 1.5) {
    const amt = wire(Math.floor((perp - 1) * 100) / 100);
    console.log(`usdClassTransfer ${amt} USDC perp -> spot...`);
    console.log(JSON.stringify(await usdClassTransfer(w, amt, false)));
  }

  // 3. buy HYPE with spare spot USDC (leave USDC_RESERVE)
  const spot2 = await spotBalances(DEPLOYER);
  const usdcSpare = (spot2.get("USDC") ?? 0) - USDC_RESERVE;
  const haveHype = spot2.get("HYPE") ?? 0;
  const needHype = TARGET_HYPE - haveHype;
  if (needHype > 0 && usdcSpare > 10.5) { // 10 USDC exchange minimum order value
    const ask = await bestAsk();
    if (!ask) throw new Error("no HYPE ask on the book");
    const px = wire(Math.ceil(ask * 1.02 * 100) / 100);
    const sz = wire(Math.min(needHype, Math.floor((usdcSpare / (ask * 1.02)) * 100) / 100));
    console.log(`IOC buy ${sz} HYPE @ <= ${px} (~${fmt(parseFloat(sz) * parseFloat(px))} USDC)...`);
    const r = await sendL1Action(w, {
      type: "order",
      orders: [{ a: HYPE_ASSET, b: true, p: px, s: sz, r: false, t: { limit: { tif: "Ioc" } } }],
      grouping: "na",
    });
    console.log(JSON.stringify(r?.response?.data?.statuses ?? r));
  }

  const finalSpot = await spotBalances(DEPLOYER);
  const finalHype = finalSpot.get("HYPE") ?? 0;
  console.log(`\nspot HYPE: ${finalHype} / ${TARGET_HYPE} target  |  spot USDC: ${finalSpot.get("USDC") ?? 0}`);
  if (finalHype >= parseFloat(auction.currentGas)) {
    console.log("=== DEPLOY GAS FUNDED — run hl-10-deploy-hip1-wbth.ts ===");
  } else {
    console.log(`SHORTFALL: need ~${fmt(parseFloat(auction.currentGas) - finalHype)} more spot HYPE`);
    console.log("(operator: drip-fund a mainnet-activated account, usdSend USDC here, re-run; see runbook)");
  }
}
main().catch((e) => { console.error(e); process.exit(1); });
