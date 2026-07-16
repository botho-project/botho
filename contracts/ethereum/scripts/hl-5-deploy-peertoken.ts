// #1026: deploy the WbthPeerToken (HyperEVM-side wBTH, 12 decimals) to HyperEVM
// testnet. minter/owner = deployer initially; minter is rotated to the NttManager
// after `ntt add-chain HyperEVM --mode burning --token <this>` deploys it.
import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://rpc.hyperliquid-testnet.xyz/evm";
const KEY = path.resolve(__dirname, "../../../.secrets/bridge-testnet/eth-deployer.key");
const ART = path.resolve(__dirname, "../artifacts/contracts/WbthPeerToken.sol/WbthPeerToken.json");

async function main() {
  const p = new ethers.JsonRpcProvider(RPC);
  if ((await p.getNetwork()).chainId !== 998n) throw new Error("not HyperEVM 998");
  const w = new ethers.Wallet(fs.readFileSync(KEY, "utf8").trim(), p);
  console.log("deployer:", w.address, "HYPE:", ethers.formatEther(await p.getBalance(w.address)));

  const art = JSON.parse(fs.readFileSync(ART, "utf8"));
  const factory = new ethers.ContractFactory(art.abi, art.bytecode, w);
  console.log("deploying WbthPeerToken(Wrapped BTH, wBTH, minter=deployer, owner=deployer)...");
  const c = await factory.deploy("Wrapped BTH", "wBTH", w.address, w.address);
  console.log("deploy tx:", c.deploymentTransaction()?.hash);
  await c.waitForDeployment();
  const addr = await c.getAddress();
  console.log("WbthPeerToken:", addr);

  const tok = new ethers.Contract(addr, art.abi, p);
  console.log("  name:", await tok.name(), "symbol:", await tok.symbol(), "decimals:", await tok.decimals());
  console.log("  minter:", await tok.minter(), "owner:", await tok.owner());
  console.log("\nNext: ntt add-chain HyperEVM --mode burning --token", addr);
}
main().catch((e) => { console.error(e); process.exit(1); });
