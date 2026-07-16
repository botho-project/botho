// #1026: rotate the WbthPeerToken minter from the deployer to the HyperEVM
// NttManager, so the manager can mint/burn the PeerToken on cross-chain
// transfers (burn-and-mint mode). Owner (deployer) calls setMinter.
import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://rpc.hyperliquid-testnet.xyz/evm";
const PEER_TOKEN = "0x230f154Ae33A53dcFFEDedB2d92cc1F32BcE7610";
const HYPEREVM_MANAGER = "0x07F159042E9F89484dfdA37D09057c871dbCB475";
const KEY = path.resolve(__dirname, "../../../.secrets/bridge-testnet/eth-deployer.key");
const ABI = [
  "function minter() view returns (address)",
  "function owner() view returns (address)",
  "function setMinter(address newMinter)",
];

async function main() {
  const p = new ethers.JsonRpcProvider(RPC);
  const w = new ethers.Wallet(fs.readFileSync(KEY, "utf8").trim(), p);
  const tok = new ethers.Contract(PEER_TOKEN, ABI, w);
  console.log("minter before:", await tok.minter());
  const tx = await tok.setMinter(HYPEREVM_MANAGER);
  console.log("setMinter tx:", tx.hash);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("setMinter reverted");
  const after = await tok.minter();
  console.log("minter after:", after);
  if (after.toLowerCase() !== HYPEREVM_MANAGER.toLowerCase()) throw new Error("minter mismatch");
  console.log("OK — NttManager is now the PeerToken minter.");
}
main().catch((e) => { console.error(e); process.exit(1); });
