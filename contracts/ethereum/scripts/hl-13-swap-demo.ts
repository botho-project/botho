// #877 step 5/5: order-book swap demo — buy and sell WBTH against spot USDC
// on the HyperCore order book (against the HIP-2 hyperliquidity quotes),
// capturing order ids, fills, and before/after balances for the runbook.
//
// Order sizes respect the 10 USDC exchange minimum order value (testnet
// enforces it too — live-verified) and szDecimals 2.
//
// Run: HL_TOKEN_INDEX=<idx> npx ts-node --compiler-options
//   '{"module":"commonjs","target":"ES2020","esModuleInterop":true,
//   "skipLibCheck":true}' scripts/hl-13-swap-demo.ts
import {
  CORE_TOKEN_NAME, DEPLOYER, info, loadWallet, perpUsdc, sendL1Action,
  spotBalances, usdClassTransfer, wire,
} from "./hl-lib";

const DEMO_SZ_WBTH = parseFloat(process.env.HL_DEMO_SZ ?? "11"); // ~$11 notional at the 1.0 anchor

async function findPair(tokenIndex: number): Promise<number> {
  const meta = await info({ type: "spotMeta" });
  const pair = (meta.universe ?? []).find((u: any) => u.tokens?.[0] === tokenIndex && u.tokens?.[1] === 0);
  if (!pair) throw new Error("WBTH/USDC pair not found — run hl-10 first");
  return pair.index;
}

async function printBalances(label: string) {
  const spot = await spotBalances(DEPLOYER);
  console.log(`${label}: spot USDC ${spot.get("USDC") ?? 0} | spot ${CORE_TOKEN_NAME} ${spot.get(CORE_TOKEN_NAME) ?? 0}`);
}

async function book(pair: number) {
  const b = await info({ type: "l2Book", coin: `@${pair}` });
  const [bids, asks] = b.levels;
  console.log("book:", `bid ${bids[0]?.px ?? "-"} x ${bids[0]?.sz ?? "-"}`, "|", `ask ${asks[0]?.px ?? "-"} x ${asks[0]?.sz ?? "-"}`);
  return { bestBid: bids[0] ? parseFloat(bids[0].px) : null, bestAsk: asks[0] ? parseFloat(asks[0].px) : null };
}

async function ioc(w: any, asset: number, isBuy: boolean, px: string, sz: string) {
  const r = await sendL1Action(w, {
    type: "order",
    orders: [{ a: asset, b: isBuy, p: px, s: sz, r: false, t: { limit: { tif: "Ioc" } } }],
    grouping: "na",
  });
  const status = r?.response?.data?.statuses?.[0];
  console.log(`${isBuy ? "BUY" : "SELL"} IOC ${sz} @ ${px}:`, JSON.stringify(status));
  return status;
}

async function main() {
  const w = loadWallet();
  if (w.address !== DEPLOYER) throw new Error(`expected deployer ${DEPLOYER}, got ${w.address}`);
  const tokenIndex = parseInt(process.env.HL_TOKEN_INDEX ?? "", 10);
  if (!Number.isInteger(tokenIndex)) throw new Error("set HL_TOKEN_INDEX to the hl-10 token index");
  const pair = await findPair(tokenIndex);
  const asset = 10000 + pair;
  console.log(`pair @${pair} (asset ${asset})`);

  // make sure the buy leg is funded in SPOT USDC (>= min notional + fees)
  const need = DEMO_SZ_WBTH * 1.1 + 1;
  const spot = await spotBalances(DEPLOYER);
  if ((spot.get("USDC") ?? 0) < need && (await perpUsdc(DEPLOYER)) > need + 1) {
    console.log(`moving ${wire(need)} USDC perp -> spot...`);
    await usdClassTransfer(w, wire(need), false);
  }

  await printBalances("before");
  let { bestAsk, bestBid } = await book(pair);

  // leg 1: BUY WBTH from the HIP-2 asks
  if (bestAsk === null) throw new Error("no asks on the book — check registerHyperliquidity");
  const buy = await ioc(w, asset, true, wire(Math.ceil(bestAsk * 1.05 * 1000) / 1000), wire(DEMO_SZ_WBTH));
  if (!buy?.filled) throw new Error("buy leg did not fill");
  await printBalances("after buy ");

  // leg 2: SELL the same size into the bids (HIP-2 seeded levels)
  ({ bestAsk, bestBid } = await book(pair));
  if (bestBid === null) throw new Error("no bids on the book");
  const sell = await ioc(w, asset, false, wire(Math.floor(bestBid * 0.95 * 1000) / 1000), wire(DEMO_SZ_WBTH));
  if (!sell?.filled) throw new Error("sell leg did not fill (partial fills print above)");
  await printBalances("after sell");

  // pull the authoritative fill records (tx hashes, fees, crossing side)
  const fills = (await info({ type: "userFills", user: DEPLOYER }))
    .filter((f: any) => f.coin === `@${pair}`)
    .slice(0, 4);
  console.log("\nfills (newest first):");
  for (const f of fills) {
    console.log(`  ${f.side === "B" ? "BUY " : "SELL"} ${f.sz} @ ${f.px} oid ${f.oid} tid ${f.tid} hash ${f.hash} fee ${f.fee} ${f.feeToken}`);
  }
  console.log(`\n=== SWAP DEMO COMPLETE: WBTH traded on the Hyperliquid testnet order book ===`);
  console.log(`UI: https://app.hyperliquid-testnet.xyz/trade/@${pair}`);
}
main().catch((e) => { console.error(e); process.exit(1); });
