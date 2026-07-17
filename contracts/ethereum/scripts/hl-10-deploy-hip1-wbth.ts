// #877 step 2/5: deploy the WBTH HIP-1 spot token on Hyperliquid testnet
// (HyperCore order book), with HIP-2 hyperliquidity opt-in.
//
// Five-step spotDeploy sequence (resumable — re-run after any failure and it
// continues from where the on-chain deploy state left off):
//   1. registerToken2      — pays the Dutch-auction gas (~500 spot HYPE!) and
//                            allocates the token index. IRREVERSIBLE SPEND:
//                            gated behind HL_CONFIRM=yes.
//   2. userGenesis         — genesis-mints 999,000 WBTH to the token's Core
//                            system (asset-bridge) address 0x2000..{index} so
//                            EVM->Core bridge-ins are backed (issue #877 spec).
//   3. genesis             — maxSupply 1,000,000 WBTH; the 1,000 WBTH residual
//                            becomes the HIP-2 hyperliquidity balance.
//   4. registerSpot        — deploys the WBTH/USDC spot pair.
//   5. registerHyperliquidity — HIP-2: anchor 1.0 USDC, 100 ask levels x 10
//                            WBTH, 2 bid levels seeded with deployer USDC
//                            (~20 USDC, moved from perp automatically).
// Then setDeployerTradingFeeShare 0% (all fees to the protocol; we take none).
//
// Decimals: szDecimals 2 + weiDecimals 8 (szDecimals+5 <= weiDecimals) and
// evmExtraWeiDecimals = 12 - 8 = 4 against the 12-decimal HyperEVM PeerToken.
//
// Run: HL_CONFIRM=yes npx ts-node --compiler-options '{"module":"commonjs",
//   "target":"ES2020","esModuleInterop":true,"skipLibCheck":true}' \
//   scripts/hl-10-deploy-hip1-wbth.ts
import {
  CORE_SZ_DECIMALS, CORE_TOKEN_NAME, CORE_WEI_DECIMALS, DEPLOYER, fmt, info,
  loadWallet, perpUsdc, sendL1Action, spotBalances, systemAddress,
  usdClassTransfer, wire,
} from "./hl-lib";

// supply plan (wei = 8 decimals)
const MAX_SUPPLY_WEI = "100000000000000"; // 1,000,000 WBTH
const SYSTEM_GENESIS_WEI = "99900000000000"; // 999,000 WBTH -> asset-bridge address
const HIP2_START_PX = "1"; // anchor: 1 WBTH ~ 1 USDC (testnet demo peg)
const HIP2_ORDER_SZ = "10"; // WBTH per level
const HIP2_N_ORDERS = 100; // 100 x 10 = the 1,000 WBTH hyperliquidity residual
const HIP2_SEEDED_LEVELS = 2; // 2 bid levels seeded with ~20 deployer USDC

async function deployState(): Promise<any | null> {
  const st = await info({ type: "spotDeployState", user: DEPLOYER });
  const s = (st.states ?? []).find((x: any) => x?.spec?.name === CORE_TOKEN_NAME);
  return { state: s ?? null, gasAuction: st.gasAuction };
}

async function findSpotPair(tokenIndex: number): Promise<number | null> {
  const meta = await info({ type: "spotMeta" });
  const pair = (meta.universe ?? []).find((u: any) => u.tokens?.[0] === tokenIndex && u.tokens?.[1] === 0);
  return pair ? pair.index : null;
}

async function main() {
  const w = loadWallet();
  if (w.address !== DEPLOYER) throw new Error(`expected deployer ${DEPLOYER}, got ${w.address}`);

  let { state, gasAuction } = await deployState();
  console.log("gas auction:", JSON.stringify(gasAuction));
  console.log("deploy state:", JSON.stringify(state));

  // step 1: registerToken2 (pays auction gas in spot HYPE)
  if (!state) {
    const spot = await spotBalances(DEPLOYER);
    const haveHype = spot.get("HYPE") ?? 0;
    const gas = parseFloat(gasAuction.currentGas);
    console.log(`\nstep 1 registerToken2: auction ${gas} HYPE, deployer spot HYPE ${haveHype}`);
    if (haveHype < gas) throw new Error(`insufficient spot HYPE (${haveHype} < ${gas}) — run hl-9 / fund first`);
    if (process.env.HL_CONFIRM !== "yes") throw new Error("IRREVERSIBLE: spends the auction gas. Re-run with HL_CONFIRM=yes");
    const maxGas = Math.round(Math.min(gas * 1.05 + 5, haveHype) * 1e8); // HYPE wei (8 dec) cap
    const r = await sendL1Action(w, {
      type: "spotDeploy",
      registerToken2: {
        spec: { name: CORE_TOKEN_NAME, szDecimals: CORE_SZ_DECIMALS, weiDecimals: CORE_WEI_DECIMALS },
        maxGas,
        fullName: "Wrapped Botho (testnet)",
      },
    });
    console.log("registerToken2:", JSON.stringify(r));
    ({ state } = await deployState());
    if (!state) throw new Error("no deploy state after registerToken2 — inspect manually");
  }
  const token: number = state.token;
  const sysAddr = systemAddress(token).toLowerCase();
  console.log(`\ntoken index: ${token}  system (asset-bridge) address: ${sysAddr}`);

  // step 2: userGenesis -> system address (skip if already recorded)
  const genesisBalances: Array<[string, string]> = state.userGenesisBalances ?? [];
  if (!genesisBalances.some(([u]) => u.toLowerCase() === sysAddr)) {
    console.log(`step 2 userGenesis: ${fmt(Number(SYSTEM_GENESIS_WEI) / 1e8)} WBTH -> ${sysAddr}`);
    const r = await sendL1Action(w, {
      type: "spotDeploy",
      userGenesis: { token, userAndWei: [[sysAddr, SYSTEM_GENESIS_WEI]], existingTokenAndWei: [] },
    });
    console.log("userGenesis:", JSON.stringify(r));
  } else console.log("step 2 userGenesis: already done");

  // step 3: genesis (sets max supply; residual 1,000 WBTH -> hyperliquidity)
  if (!state.maxSupply) {
    console.log(`step 3 genesis: maxSupply ${fmt(Number(MAX_SUPPLY_WEI) / 1e8)} WBTH, hyperliquidity residual enabled`);
    const r = await sendL1Action(w, {
      type: "spotDeploy",
      genesis: { token, maxSupply: MAX_SUPPLY_WEI, noHyperliquidity: false },
    });
    console.log("genesis:", JSON.stringify(r));
  } else console.log("step 3 genesis: already done");

  // step 4: registerSpot (WBTH/USDC)
  let pair = await findSpotPair(token);
  if (pair === null) {
    console.log("step 4 registerSpot: [WBTH, USDC]");
    const r = await sendL1Action(w, { type: "spotDeploy", registerSpot: { tokens: [token, 0] } });
    console.log("registerSpot:", JSON.stringify(r));
    pair = await findSpotPair(token);
  }
  console.log(`spot pair index: ${pair} (order-book asset id ${pair !== null ? 10000 + pair : "?"})`);
  if (pair === null) throw new Error("spot pair not found after registerSpot");

  // step 5: registerHyperliquidity (HIP-2) — needs ~20 spot USDC for seeded bids
  const seedUsdc = HIP2_SEEDED_LEVELS * parseFloat(HIP2_ORDER_SZ) * parseFloat(HIP2_START_PX);
  const spotNow = await spotBalances(DEPLOYER);
  if ((spotNow.get("USDC") ?? 0) < seedUsdc && (await perpUsdc(DEPLOYER)) > seedUsdc + 1) {
    console.log(`moving ${seedUsdc} USDC perp -> spot for HIP-2 seeding...`);
    await usdClassTransfer(w, wire(seedUsdc), false);
  }
  console.log(`step 5 registerHyperliquidity: px ${HIP2_START_PX}, ${HIP2_N_ORDERS} x ${HIP2_ORDER_SZ} WBTH asks, ${HIP2_SEEDED_LEVELS} USDC-seeded bid levels`);
  const r5 = await sendL1Action(w, {
    type: "spotDeploy",
    registerHyperliquidity: {
      spot: pair, startPx: wire(HIP2_START_PX), orderSz: wire(HIP2_ORDER_SZ),
      nOrders: HIP2_N_ORDERS, nSeededLevels: HIP2_SEEDED_LEVELS,
    },
  });
  console.log("registerHyperliquidity:", JSON.stringify(r5));

  // fee share: keep nothing for the deployer
  const r6 = await sendL1Action(w, { type: "spotDeploy", setDeployerTradingFeeShare: { token, share: "0%" } });
  console.log("setDeployerTradingFeeShare 0%:", JSON.stringify(r6));

  console.log(`\n=== WBTH LIVE ON HYPERCORE: token ${token}, pair @${pair} ===`);
  console.log("record the token index in the runbook, then run hl-11-link-evm-wbth.ts");
}
main().catch((e) => { console.error(e); process.exit(1); });
