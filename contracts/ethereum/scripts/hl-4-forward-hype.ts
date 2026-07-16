// HL official-route step 4/4: forward native HYPE on HyperEVM from 0x8E90 to the
// NTT deployer 0x111018 (which holds the Sepolia ETH), making it the single
// deployer for both EVM chains. Keeps 0.1 HYPE on 0x8E90 for future use.
import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://rpc.hyperliquid-testnet.xyz/evm";
const TO = "0x111018cfe4523097B7f651f3A06fA9a2956CF155";
const KEY = path.resolve(__dirname, "../../../.secrets/bridge-mainnet/eth-botho.key");

async function main() {
  const p = new ethers.JsonRpcProvider(RPC);
  const net = await p.getNetwork();
  if (net.chainId !== 998n) throw new Error(`expected HyperEVM 998, got ${net.chainId}`);
  const w = new ethers.Wallet(fs.readFileSync(KEY, "utf8").trim(), p);
  const bal = await p.getBalance(w.address);
  console.log("from", w.address, "HYPE:", ethers.formatEther(bal));
  const gp = (await p.getFeeData()).gasPrice ?? 1_000_000_000n;
  const gasReserve = 21000n * gp * 3n;
  const send = bal - gasReserve - ethers.parseEther("0.1");
  if (send <= 0n) throw new Error("nothing to forward");
  console.log(`forwarding ${ethers.formatEther(send)} HYPE -> ${TO}`);
  const tx = await w.sendTransaction({ to: TO, value: send });
  console.log("tx:", tx.hash);
  const rc = await tx.wait();
  if (!rc || rc.status !== 1) throw new Error("forward reverted");
  console.log("0x111018 HYPE now:", ethers.formatEther(await p.getBalance(TO)));
  console.log("0x8E90 HYPE left:", ethers.formatEther(await p.getBalance(w.address)));
}
main().catch((e) => { console.error(e); process.exit(1); });
