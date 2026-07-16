// #1026: manually redeem the NTT transfer on HyperEVM (no Wormhole executor on
// testnet). Fetch the guardian-signed VAA from Wormholescan and submit it to the
// HyperEVM WormholeTransceiver.receiveMessage, which delivers to the NttManager
// (threshold 1) → mints the wBTH PeerToken to the recipient.
import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://rpc.hyperliquid-testnet.xyz/evm";
const TRANSCEIVER = "0xC5652d52fBE4c41c91a65Ecd18304B20e58Df491"; // HyperEVM WormholeTransceiver
const PEER_TOKEN = "0x230f154Ae33A53dcFFEDedB2d92cc1F32BcE7610";
const RECIPIENT = "0x111018cfe4523097B7f651f3A06fA9a2956CF155";
const VAA_URL = "https://api.testnet.wormholescan.io/api/v1/vaas/10002/000000000000000000000000bee886bcc887e96487c2103e46fda7ada6b89195/2";
const KEY = path.resolve(__dirname, "../../../.secrets/bridge-testnet/eth-deployer.key");

const XCVR_ABI = ["function receiveMessage(bytes encodedMessage)"];
const ERC20_ABI = ["function balanceOf(address) view returns (uint256)"];

async function main() {
  // 1. fetch VAA
  const res = await fetch(VAA_URL);
  const j: any = await res.json();
  const b64 = j?.data?.vaa ?? j?.vaa;
  if (!b64) throw new Error("no VAA in response");
  const vaa = "0x" + Buffer.from(b64, "base64").toString("hex");
  console.log("VAA bytes:", vaa.length / 2 - 1);

  const p = new ethers.JsonRpcProvider(RPC);
  const w = new ethers.Wallet(fs.readFileSync(KEY, "utf8").trim(), p);
  const token = new ethers.Contract(PEER_TOKEN, ERC20_ABI, p);
  const before = await token.balanceOf(RECIPIENT);
  console.log("recipient wBTH before:", ethers.formatUnits(before, 12));

  const xcvr = new ethers.Contract(TRANSCEIVER, XCVR_ABI, w);
  console.log("submitting receiveMessage on HyperEVM transceiver...");
  const tx = await xcvr.receiveMessage(vaa);
  console.log("redeem tx:", tx.hash);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("receiveMessage reverted");

  const after = await token.balanceOf(RECIPIENT);
  console.log("recipient wBTH after:", ethers.formatUnits(after, 12), `(+${ethers.formatUnits(after - before, 12)})`);
  console.log("\n=== NTT ROUND TRIP COMPLETE: wBTH minted on HyperEVM ===");
}
main().catch((e) => { console.error(e); process.exit(1); });
