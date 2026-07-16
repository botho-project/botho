// Prove the devnet wBTH/WSOL Orca pool trades: a small wBTH -> WSOL swap that
// stays within the seeded position's initialized tick arrays (#870). Separate
// from devnet-orca-pool.ts so re-running never opens a second position.

import * as anchor from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import { NATIVE_MINT } from "@solana/spl-token";
import {
  WhirlpoolContext, buildWhirlpoolClient, ORCA_WHIRLPOOL_PROGRAM_ID,
  PDAUtil, swapQuoteByInputToken,
} from "@orca-so/whirlpools-sdk";
import { Percentage, DecimalUtil } from "@orca-so/common-sdk";
import Decimal from "decimal.js";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://api.devnet.solana.com";
const DEVNET_CONFIG = new PublicKey("FcrweFY1G9HJAHG5inkGB6pKg1HZ6x9UC2WioAfWrGkR");
const WBTH_MINT = new PublicKey("F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX");
const TICK_SPACING = 64;
const WBTH_DECIMALS = 12;
const SECRETS = path.resolve(__dirname, "../../../.secrets/bridge-testnet");
const SWAP_WBTH = new Decimal("500"); // 500 wBTH -> WSOL: ~1% of in-range depth

function load(name: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(path.join(SECRETS, `${name}.json`), "utf8"))));
}

async function main() {
  const conn = new Connection(RPC, "confirmed");
  const lp = load("solana-lp");
  const ctx = WhirlpoolContext.from(conn, new anchor.Wallet(lp), ORCA_WHIRLPOOL_PROGRAM_ID);
  const client = buildWhirlpoolClient(ctx);

  const [mintA, mintB] = [NATIVE_MINT, WBTH_MINT].sort((a, b) =>
    Buffer.compare(a.toBuffer(), b.toBuffer())); // Orca canonical order
  const poolPda = PDAUtil.getWhirlpool(ORCA_WHIRLPOOL_PROGRAM_ID, DEVNET_CONFIG, mintA, mintB, TICK_SPACING);
  const pool = await client.getPool(poolPda.publicKey);
  const before = pool.getData();
  console.log("pool:", poolPda.publicKey.toBase58());
  console.log("tick before swap:", before.tickCurrentIndex);

  const amountIn = DecimalUtil.toBN(SWAP_WBTH, WBTH_DECIMALS);
  const quote = await swapQuoteByInputToken(
    pool, WBTH_MINT, amountIn,
    Percentage.fromFraction(Number(process.env.SLIPPAGE_BPS ?? "100"), 10_000), // #1017
    ORCA_WHIRLPOOL_PROGRAM_ID, ctx.fetcher, undefined,
  );
  console.log(`swap ${SWAP_WBTH} wBTH -> est ${quote.estimatedAmountOut.toString()} lamports WSOL`);
  const sig = await (await pool.swap(quote)).buildAndExecute();
  console.log("swap tx:", sig);

  const after = (await client.getPool(poolPda.publicKey)).getData();
  console.log("tick after swap:", after.tickCurrentIndex);
  console.log("\n=== SWAP CONFIRMED — pool trades ===");
  console.log("explorer:", `https://explorer.solana.com/address/${poolPda.publicKey.toBase58()}?cluster=devnet`);
}

main().catch((e) => { console.error(e); process.exit(1); });
