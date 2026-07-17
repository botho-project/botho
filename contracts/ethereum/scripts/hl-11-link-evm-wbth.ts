// #877 step 3/5: link the HyperCore WBTH spot token (hl-10) to the HyperEVM
// wBTH PeerToken (NTT spoke from #876/#1026) so Core <-> EVM transfers work.
//
// Two actions, both signed by 0x111018 (which is BOTH the Core spot deployer
// AND the EVM contract deployer):
//   1. requestEvmContract  — Core-side: propose the EVM address + decimal
//                            mapping (evmExtraWeiDecimals = 12 - 8 = 4).
//   2. finalizeEvmContract — EVM-deployer proof: the PeerToken was CREATE'd
//                            by 0x111018 at nonce 0 (verified via
//                            ethers.getCreateAddress), so input = create{0}.
//
// Decimal caveat (runbook): Core<->EVM transfer amounts must be round in the
// 4 extra EVM wei decimals or the remainder is burned — keep every amount
// 8-decimal-aligned end to end.
//
// Run: HL_TOKEN_INDEX=<idx from hl-10> npx ts-node --compiler-options
//   '{"module":"commonjs","target":"ES2020","esModuleInterop":true,
//   "skipLibCheck":true}' scripts/hl-11-link-evm-wbth.ts
import { ethers } from "ethers";
import {
  CORE_TOKEN_NAME, DEPLOYER, EVM_EXTRA_WEI_DECIMALS, HYPEREVM_RPC, info,
  loadWallet, PEER_TOKEN, PEER_TOKEN_CREATE_NONCE, sendL1Action,
} from "./hl-lib";

async function resolveTokenIndex(): Promise<number> {
  if (process.env.HL_TOKEN_INDEX) return parseInt(process.env.HL_TOKEN_INDEX, 10);
  // fall back to the deployer's completed deploys in spot meta
  const meta = await info({ type: "spotMeta" });
  const candidates = (meta.tokens ?? []).filter((t: any) => t.name === CORE_TOKEN_NAME);
  for (const t of candidates) {
    const det = await info({ type: "tokenDetails", tokenId: t.tokenId });
    if (det?.deployer?.toLowerCase() === DEPLOYER.toLowerCase()) return t.index;
  }
  throw new Error(`could not find a ${CORE_TOKEN_NAME} token deployed by ${DEPLOYER}; pass HL_TOKEN_INDEX`);
}

async function main() {
  const w = loadWallet();
  if (w.address !== DEPLOYER) throw new Error(`expected deployer ${DEPLOYER}, got ${w.address}`);
  const token = await resolveTokenIndex();
  console.log(`Core token index: ${token}  EVM PeerToken: ${PEER_TOKEN}`);

  // sanity: PeerToken decimals must match the 8 + 4 mapping, and the CREATE
  // nonce proof must actually derive the PeerToken address.
  const evm = new ethers.JsonRpcProvider(HYPEREVM_RPC);
  const erc20 = new ethers.Contract(PEER_TOKEN, ["function decimals() view returns (uint8)"], evm);
  const dec = Number(await erc20.decimals());
  if (dec !== 8 + EVM_EXTRA_WEI_DECIMALS) throw new Error(`PeerToken decimals ${dec} != ${8 + EVM_EXTRA_WEI_DECIMALS}`);
  const derived = ethers.getCreateAddress({ from: DEPLOYER, nonce: PEER_TOKEN_CREATE_NONCE });
  if (derived.toLowerCase() !== PEER_TOKEN.toLowerCase()) throw new Error(`CREATE(nonce ${PEER_TOKEN_CREATE_NONCE}) derives ${derived}, not the PeerToken`);

  console.log(`1. requestEvmContract (evmExtraWeiDecimals ${EVM_EXTRA_WEI_DECIMALS})...`);
  const r1 = await sendL1Action(w, {
    type: "requestEvmContract",
    token,
    address: PEER_TOKEN.toLowerCase(),
    evmExtraWeiDecimals: EVM_EXTRA_WEI_DECIMALS,
  });
  console.log("requestEvmContract:", JSON.stringify(r1));

  console.log(`2. finalizeEvmContract (create nonce ${PEER_TOKEN_CREATE_NONCE})...`);
  const r2 = await sendL1Action(w, {
    type: "finalizeEvmContract",
    token,
    input: { create: { nonce: PEER_TOKEN_CREATE_NONCE } },
  });
  console.log("finalizeEvmContract:", JSON.stringify(r2));

  // verify the link landed in spot meta
  const meta = await info({ type: "spotMeta" });
  const entry = (meta.tokens ?? []).find((t: any) => t.index === token);
  console.log("spotMeta evmContract:", JSON.stringify(entry?.evmContract ?? null));
  if (entry?.evmContract?.address?.toLowerCase() !== PEER_TOKEN.toLowerCase()) {
    throw new Error("link not reflected in spotMeta yet — re-check in a minute");
  }
  console.log("\n=== LINKED: HyperCore WBTH <-> HyperEVM wBTH PeerToken ===");
  console.log("next: hl-12-bridge-in-wbth.ts");
}
main().catch((e) => { console.error(e); process.exit(1); });
