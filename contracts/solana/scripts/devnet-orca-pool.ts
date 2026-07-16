// Seed a wBTH/WSOL Orca Whirlpool on devnet (#870) — the Solana analogue of the
// Ethereum Uniswap-v3 bootstrap. Creates the pool, opens a full-ish-range
// position funded from the bridge-minted wBTH + wrapped SOL, then swaps to
// prove the pool trades. Uses @orca-so/whirlpools-sdk 0.13 (anchor 0.29 line).
//
// Run: npx ts-node --compiler-options '{"module":"commonjs","target":"ES2020",
//        "esModuleInterop":true,"skipLibCheck":true}' scripts/devnet-orca-pool.ts

import * as anchor from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import { NATIVE_MINT } from "@solana/spl-token";
import {
  WhirlpoolContext, buildWhirlpoolClient, ORCA_WHIRLPOOL_PROGRAM_ID,
  PDAUtil, PriceMath, PoolUtil, TickUtil,
  increaseLiquidityQuoteByInputTokenWithParams, swapQuoteByInputToken,
  NO_TOKEN_EXTENSION_CONTEXT,
} from "@orca-so/whirlpools-sdk";
import { Percentage, DecimalUtil } from "@orca-so/common-sdk";
import Decimal from "decimal.js";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://api.devnet.solana.com";
const DEVNET_CONFIG = new PublicKey("FcrweFY1G9HJAHG5inkGB6pKg1HZ6x9UC2WioAfWrGkR");
const WBTH_MINT = new PublicKey("F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX");
const TICK_SPACING = 64; // 0.30% fee tier
const WBTH_DECIMALS = 12;
const SOL_DECIMALS = 9;
// Demo price: 1e-6 WSOL per wBTH (decimal-adjusted) so a ~0.05 SOL side pairs
// with tens of thousands of wBTH from the 100k we minted.
const INIT_PRICE = new Decimal("0.000001");
const SECRETS = path.resolve(__dirname, "../../../.secrets/bridge-testnet");
// Slippage bound for liquidity add + swap (#1017). Default 1% for devnet; a
// mainnet run should tune this and derive tight min-outs from the live quote.
const SLIPPAGE = Percentage.fromFraction(Number(process.env.SLIPPAGE_BPS ?? "100"), 10_000);

function load(name: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(path.join(SECRETS, `${name}.json`), "utf8"))));
}

async function main() {
  const conn = new Connection(RPC, "confirmed");
  const lp = load("solana-lp");
  const wallet = new anchor.Wallet(lp);
  const ctx = WhirlpoolContext.from(conn, wallet, ORCA_WHIRLPOOL_PROGRAM_ID);
  const client = buildWhirlpoolClient(ctx);
  console.log("LP wallet:", lp.publicKey.toBase58());

  // Order the mints (Orca requires tokenMintA < tokenMintB).
  const [mintA, mintB] = PoolUtil.orderMints(WBTH_MINT, NATIVE_MINT).map((m) => new PublicKey(m.toString()));
  const wbthIsA = mintA.equals(WBTH_MINT);
  const decA = wbthIsA ? WBTH_DECIMALS : SOL_DECIMALS;
  const decB = wbthIsA ? SOL_DECIMALS : WBTH_DECIMALS;
  // Price is tokenB per tokenA. Our INIT_PRICE is WSOL/wBTH; flip if wBTH is B.
  const priceAB = wbthIsA ? INIT_PRICE : new Decimal(1).div(INIT_PRICE);
  console.log(`token A: ${mintA.toBase58()} (dec ${decA})`);
  console.log(`token B: ${mintB.toBase58()} (dec ${decB})`);

  // ---- Create pool (idempotent: reuse if it already exists) ----
  const poolPda = PDAUtil.getWhirlpool(ORCA_WHIRLPOOL_PROGRAM_ID, DEVNET_CONFIG, mintA, mintB, TICK_SPACING);
  console.log("pool PDA:", poolPda.publicKey.toBase58());
  const existing = await conn.getAccountInfo(poolPda.publicKey);
  if (!existing) {
    const initTick = PriceMath.priceToInitializableTickIndex(priceAB, decA, decB, TICK_SPACING);
    console.log(`[1] createPool at tick ${initTick} (price ${priceAB.toString()} B/A)`);
    const { poolKey, tx } = await client.createPool(
      DEVNET_CONFIG, mintA, mintB, TICK_SPACING, initTick, lp.publicKey,
    );
    const sig = await tx.buildAndExecute();
    console.log("   createPool:", sig, "->", poolKey.toBase58());
  } else {
    console.log("[1] pool already exists — reuse");
  }

  const pool = await client.getPool(poolPda.publicKey);
  const data = pool.getData();
  const curTick = data.tickCurrentIndex;
  console.log(`   current tick ${curTick}, sqrtPrice ${data.sqrtPrice.toString()}`);

  // ---- Open a wide position + add liquidity (input = 0.05 SOL side) ----
  // Range: ~one order of magnitude in price each way, aligned to spacing.
  const lower = TickUtil.getInitializableTickIndex(curTick - 44 * TICK_SPACING, TICK_SPACING);
  const upper = TickUtil.getInitializableTickIndex(curTick + 44 * TICK_SPACING, TICK_SPACING);

  // Orca does NOT auto-create tick arrays. Initialize the arrays containing the
  // position ticks AND the current tick (swap traversal) or the whirlpool's
  // Account<TickArray> constraint fails with AccountOwnedByWrongProgram (0xbbf).
  const initTa = await pool.initTickArrayForTicks([lower, upper, curTick]);
  if (initTa) {
    const taSig = await initTa.buildAndExecute();
    console.log("   initTickArrays:", taSig);
  } else {
    console.log("   tick arrays already initialized");
  }

  const solInput = DecimalUtil.toBN(new Decimal("0.05"), SOL_DECIMALS); // 0.05 WSOL
  console.log(`[2] open position ticks [${lower}, ${upper}], input 0.05 WSOL`);
  const quote = increaseLiquidityQuoteByInputTokenWithParams({
    inputTokenMint: NATIVE_MINT,
    inputTokenAmount: solInput,
    tokenMintA: mintA,
    tokenMintB: mintB,
    tickCurrentIndex: curTick,
    sqrtPrice: data.sqrtPrice,
    tickLowerIndex: lower,
    tickUpperIndex: upper,
    slippageTolerance: SLIPPAGE,
    tokenExtensionCtx: NO_TOKEN_EXTENSION_CONTEXT,
  });
  console.log(`   quote: tokenA max ${quote.tokenMaxA.toString()}, tokenB max ${quote.tokenMaxB.toString()}`);
  const { positionMint, tx: openTx } = await pool.openPosition(lower, upper, quote);
  const openSig = await openTx.buildAndExecute();
  console.log("   openPosition:", openSig, "positionMint", positionMint.toBase58());

  // ---- Swap to prove the pool trades: 0.005 WSOL -> wBTH ----
  console.log("[3] swap 0.005 WSOL -> wBTH");
  const refreshed = await client.getPool(poolPda.publicKey);
  const swapIn = DecimalUtil.toBN(new Decimal("0.005"), SOL_DECIMALS);
  const swapQuote = await swapQuoteByInputToken(
    refreshed, NATIVE_MINT, swapIn, SLIPPAGE,
    ORCA_WHIRLPOOL_PROGRAM_ID, ctx.fetcher, undefined,
  );
  console.log(`   estimated out ${swapQuote.estimatedAmountOut.toString()} (of wBTH mint)`);
  const swapTx = await refreshed.swap(swapQuote);
  const swapSig = await swapTx.buildAndExecute();
  console.log("   swap:", swapSig);

  console.log("\n=== ORCA POOL LIVE (devnet) ===");
  console.log("pool:", poolPda.publicKey.toBase58());
  console.log("explorer:", `https://explorer.solana.com/address/${poolPda.publicKey.toBase58()}?cluster=devnet`);
}

main().catch((e) => { console.error(e); process.exit(1); });
