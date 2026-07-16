// #1026: mint a small amount of wBTH to the deployer via the real 2-of-3 Safe
// (ADR-0002 custody path), to fund the NTT demo transfer Sepolia -> HyperEVM.
import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const WBTH = "0x49b985ec427ee771a601f11b18f7d4402fa2dd7b";
const SAFE = "0x61274F558f9027e2D402d3340dE89152FA3F3947";
const MINT_TO = "0x111018cfe4523097B7f651f3A06fA9a2956CF155"; // deployer
const AMOUNT = 100_000_000_000_000n; // 100 wBTH (10^14 pico)
const ORDER_ID = ethers.id("wbth-ntt-demo-2026-07-16");
const SECRETS = path.resolve(__dirname, "../../../.secrets/bridge-testnet");
const key = (n: string) => fs.readFileSync(path.join(SECRETS, `${n}.key`), "utf8").trim();

const WBTH_ABI = ["function bridgeMint(address to, uint256 amount, bytes32 orderId)", "function balanceOf(address) view returns (uint256)", "function processedOrders(bytes32) view returns (bool)"];
const SAFE_ABI = [
  "function nonce() view returns (uint256)",
  "function getThreshold() view returns (uint256)",
  "function getTransactionHash(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 _nonce) view returns (bytes32)",
  "function execTransaction(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address payable refundReceiver,bytes signatures) payable returns (bool)",
];

async function main() {
  const provider = new ethers.JsonRpcProvider(process.env.SEPOLIA_RPC_URL || "https://ethereum-sepolia-rpc.publicnode.com");
  const deployer = new ethers.Wallet(key("eth-deployer"), provider);
  const owners = [1, 2, 3].map((i) => new ethers.Wallet(key(`eth-safe-owner-${i}`), provider));
  const wbth = new ethers.Contract(WBTH, WBTH_ABI, provider);
  const safe = new ethers.Contract(SAFE, SAFE_ABI, provider);

  if (await wbth.processedOrders(ORDER_ID)) { console.log("order already minted; balance:", ethers.formatUnits(await wbth.balanceOf(MINT_TO), 12)); return; }
  const data = wbth.interface.encodeFunctionData("bridgeMint", [MINT_TO, AMOUNT, ORDER_ID]);
  const nonce = await safe.nonce();
  const Z = ethers.ZeroAddress;
  const digest: string = await safe.getTransactionHash(WBTH, 0n, data, 0, 0n, 0n, 0n, Z, Z, nonce);
  const threshold = Number(await safe.getThreshold());
  const parts = owners.slice(0, threshold).map((w) => ({ addr: w.address.toLowerCase(), sig: ethers.Signature.from(w.signingKey.sign(digest)).serialized }))
    .sort((a, b) => (a.addr < b.addr ? -1 : 1));
  const blob = "0x" + parts.map((p) => p.sig.slice(2)).join("");
  console.log(`minting ${ethers.formatUnits(AMOUNT, 12)} wBTH -> ${MINT_TO} via 2-of-3 Safe (nonce ${nonce})`);
  const tx = await (safe.connect(deployer) as ethers.Contract).execTransaction(WBTH, 0n, data, 0, 0n, 0n, 0n, Z, Z, blob);
  console.log("execTransaction:", tx.hash);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("mint reverted");
  console.log("deployer wBTH balance:", ethers.formatUnits(await wbth.balanceOf(MINT_TO), 12));
}
main().catch((e) => { console.error(e); process.exit(1); });
