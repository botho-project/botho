// Self-test for hl-lib.ts signing: verifies the vendored msgpack encoder,
// action hashing, L1-action signatures, and user-signed-action signatures
// against golden vectors generated with hyperliquid-python-sdk 0.x
// (utils/signing.py) using the well-known throwaway hardhat key #0.
// Run (no network, no secrets):
//   npx ts-node --compiler-options '{"module":"commonjs","target":"ES2020",
//     "esModuleInterop":true,"skipLibCheck":true}' scripts/hl-lib-selftest.ts
import { ethers } from "ethers";
import {
  actionHash, signL1Action, signUserSignedAction,
  USD_CLASS_TRANSFER_SIGN_TYPES, USD_SEND_SIGN_TYPES, systemAddress, wire,
} from "./hl-lib";

const w = new ethers.Wallet("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"); // hardhat #0, throwaway
const NONCE = 1784260000000;

// [action, expected actionHash, expected r] from the python SDK (testnet source "b")
const VECTORS: Array<[any, string, string]> = [
  [{ type: "spotDeploy", registerToken2: { spec: { name: "WBTH", szDecimals: 2, weiDecimals: 8 }, maxGas: 510000000, fullName: "Wrapped Botho (testnet)" } },
    "0x3d8d233f268d5cc91656b8aec76e950e89081fbcc79c8eaf75bdaea4fc74defc", "0xa4fd2358703b030eee0a489b96b067d679ba9558b8fda490eeef16c0058fcd81"],
  [{ type: "spotDeploy", userGenesis: { token: 1234, userAndWei: [["0x20000000000000000000000000000000000004d2", "99900000000000"]], existingTokenAndWei: [] } },
    "0x0386997aff0f60faedfd91388672e43fb47b5f4a2e13811d8fc7f1e62c8a9779", "0xf35a2258997791fc28b6c568c96a7f48b0c6d58fa6547bc8ac0b732294dc5647"],
  [{ type: "spotDeploy", genesis: { token: 1234, maxSupply: "100000000000000", noHyperliquidity: false } },
    "0xb59f49834d31ba6582bfb5741cc380c24f220d6395f98713c18db403c5d909b5", "0x8017c2ca75ef957cd217918eb44b89827e5ddb4e66726d669be93a3681f53a26"],
  [{ type: "spotDeploy", registerSpot: { tokens: [1234, 0] } },
    "0x487e89262e54c3830ad76ed437781674e35df7e7521eb3c9b25d7407116856a5", "0xb435f405283ec5b42d287e2c9560f38479ab231f8d0e61ce86283cf7b1db97ff"],
  [{ type: "spotDeploy", registerHyperliquidity: { spot: 5678, startPx: "1.0", orderSz: "10.0", nOrders: 100, nSeededLevels: 2 } },
    "0xa2bc8083792a5a277be25b8b8522b0263b30787ff73a66ff5ea6ca8351b6dde7", "0xe3e2b38a3cc0822ee5066682d8f92b7d7daccd306a7d7dc7361b22a68836403b"],
  [{ type: "requestEvmContract", token: 1234, address: "0x230f154ae33a53dcffededb2d92cc1f32bce7610", evmExtraWeiDecimals: 4 },
    "0xa036cd702d359ab99452c5cab4dacfd8701ede01d08545193e25d0b700ae1489", "0x8670a8b07a932ceab6953eb1e8508e25aedc091900709c7fc4b4b8c72533536a"],
  [{ type: "finalizeEvmContract", token: 1234, input: { create: { nonce: 0 } } },
    "0xa9dff8d22abc2167468a8710f8d70a264be953ddf780eec7c81d12e0ba9af7c9", "0x95948fc4efb0133b88d24073299ead1a2d666555413e019a6e2089d25fb0ed1c"],
  [{ type: "order", orders: [{ a: 11234, b: true, p: "1.05", s: "2.0", r: false, t: { limit: { tif: "Ioc" } } }], grouping: "na" },
    "0xe513219cad7e28f90740d086fdd227c117e4b7d71bbfc23c6f3ef7ce948a42d5", "0x54fd3d44b87d128bae81db9882e874b91fe2cec18c31e65037a614765524e7cf"],
];

async function main() {
  let fail = 0;
  for (const [action, expHash, expR] of VECTORS) {
    const h = actionHash(action, null, NONCE);
    const sig = await signL1Action(w, action, NONCE);
    const ok = h === expHash && sig.r === expR;
    if (!ok) fail++;
    console.log(`${ok ? "PASS" : "FAIL"} ${action.type}${action.type === "spotDeploy" ? ":" + Object.keys(action)[1] : ""}${ok ? "" : ` hash=${h} r=${sig.r}`}`);
  }
  const uct: any = { type: "usdClassTransfer", amount: "2.0", toPerp: false, nonce: NONCE };
  const s1 = await signUserSignedAction(w, uct, USD_CLASS_TRANSFER_SIGN_TYPES, "HyperliquidTransaction:UsdClassTransfer");
  const ok1 = s1.r === "0xe94cb7c2e8ac567d649e2167048ec247066036785adb24207baaa5d5e5e5a2d4";
  console.log(`${ok1 ? "PASS" : "FAIL"} usdClassTransfer${ok1 ? "" : " r=" + s1.r}`);
  if (!ok1) fail++;
  const us: any = { destination: "0x111018cfe4523097B7f651f3A06fA9a2956CF155", amount: "510.0", time: NONCE, type: "usdSend" };
  const s2 = await signUserSignedAction(w, us, USD_SEND_SIGN_TYPES, "HyperliquidTransaction:UsdSend");
  const ok2 = s2.r === "0xca4f72969152e8628bef15c9bd963e771670035c9b41cecccc95545207e1d1f1";
  console.log(`${ok2 ? "PASS" : "FAIL"} usdSend${ok2 ? "" : " r=" + s2.r}`);
  if (!ok2) fail++;
  // wire(): the exchange re-canonicalizes float strings before signature
  // verification — live-confirmed: "53.0" breaks recovery, "53" verifies.
  const wireCases: Array<[number | string, string]> = [
    [53.0, "53"], ["0.02", "0.02"], [1.0, "1"], [10.0, "10"], [0.31, "0.31"], [2.5, "2.5"],
  ];
  for (const [input, expected] of wireCases) {
    const got = wire(input);
    const ok = got === expected;
    console.log(`${ok ? "PASS" : "FAIL"} wire(${JSON.stringify(input)}) = ${got}`);
    if (!ok) fail++;
  }
  const sys = systemAddress(200);
  const ok3 = sys.toLowerCase() === "0x20000000000000000000000000000000000000c8"; // docs example
  console.log(`${ok3 ? "PASS" : "FAIL"} systemAddress(200) = ${sys}`);
  if (!ok3) fail++;
  if (fail) { console.error(`${fail} FAILURES`); process.exit(1); }
  console.log("all signing self-tests passed");
}
main().catch((e) => { console.error(e); process.exit(1); });
