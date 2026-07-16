// Dry-run the 2-of-3 Safe bridgeMint against LIVE Sepolia state via eth_call —
// no tx sent, no funds spent. Proves the custody path (the only novel code in
// live-defi-roundtrip.ts) will not revert before we broadcast:
//   1. build bridgeMint calldata + read the real Safe nonce
//   2. get the real SafeTx digest, sign with 2 owners, RECOVER -> assert owners
//   3. eth_call execTransaction(bridgeMint) from the deployer -> expect success
//
// Run: npx hardhat run scripts/validate-mint-sim.ts --network sepolia

import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

const WBTH = "0x49b985ec427ee771a601f11b18f7d4402fa2dd7b";
const SAFE = "0x61274F558f9027e2D402d3340dE89152FA3F3947";
const WBTH_LIQ = 100_000_000_000_000_000n; // 10^17 pico = 100,000 wBTH
const ORDER_ID = ethers.id("wbth-sepolia-liquidity-bootstrap-2026-07-16");

const WBTH_ABI = [
  "function bridgeMint(address to, uint256 amount, bytes32 orderId)",
  "function processedOrders(bytes32) view returns (bool)",
];
const SAFE_ABI = [
  "function nonce() view returns (uint256)",
  "function getThreshold() view returns (uint256)",
  "function getOwners() view returns (address[])",
  "function getTransactionHash(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 _nonce) view returns (bytes32)",
  "function execTransaction(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address payable refundReceiver,bytes signatures) payable returns (bool)",
];

const SECRETS = path.resolve(__dirname, "../../../.secrets/bridge-testnet");
const key = (n: string) => fs.readFileSync(path.join(SECRETS, `${n}.key`), "utf8").trim();

async function main() {
  const rpc = process.env.SEPOLIA_RPC_URL!;
  const provider = new ethers.JsonRpcProvider(rpc);
  const deployer = new ethers.Wallet(key("eth-deployer"), provider);
  const owners = [1, 2, 3].map((i) => new ethers.Wallet(key(`eth-safe-owner-${i}`), provider));

  const wbth = new ethers.Contract(WBTH, WBTH_ABI, provider);
  const safe = new ethers.Contract(SAFE, SAFE_ABI, provider);

  const threshold = Number(await safe.getThreshold());
  const onchainOwners: string[] = (await safe.getOwners()).map((a: string) => a.toLowerCase());
  console.log("threshold:", threshold, "owners:", onchainOwners);
  console.log("orderId processed already?", await wbth.processedOrders(ORDER_ID));

  const data = wbth.interface.encodeFunctionData("bridgeMint", [deployer.address, WBTH_LIQ, ORDER_ID]);
  const nonce = await safe.nonce();
  const Z = ethers.ZeroAddress;
  const digest: string = await safe.getTransactionHash(WBTH, 0n, data, 0, 0n, 0n, 0n, Z, Z, nonce);
  console.log("SafeTx nonce:", nonce, "digest:", digest);

  // Sign with `threshold` owners; recover to prove the sig format.
  const parts = owners.slice(0, threshold).map((w) => {
    const sig = ethers.Signature.from(w.signingKey.sign(digest));
    const recovered = ethers.recoverAddress(digest, sig).toLowerCase();
    if (recovered !== w.address.toLowerCase())
      throw new Error(`recover mismatch: ${recovered} != ${w.address}`);
    if (!onchainOwners.includes(recovered))
      throw new Error(`signer ${recovered} is not a Safe owner`);
    console.log(`  signer ${w.address} recovers OK, is owner: yes`);
    return { addr: w.address.toLowerCase(), sig: sig.serialized };
  });
  parts.sort((a, b) => (a.addr < b.addr ? -1 : 1));
  const sigBlob = "0x" + parts.map((p) => p.sig.slice(2)).join("");

  // eth_call the execTransaction against live state (mint to deployer here, a
  // throwaway target for the sim — the real run mints to the LP).
  const iface = new ethers.Interface(SAFE_ABI);
  const callData = iface.encodeFunctionData("execTransaction", [
    WBTH, 0n, data, 0, 0n, 0n, 0n, Z, Z, sigBlob,
  ]);
  try {
    const ret = await provider.call({ from: deployer.address, to: SAFE, data: callData });
    const [ok] = iface.decodeFunctionResult("execTransaction", ret);
    console.log("eth_call execTransaction ->", ok ? "SUCCESS (would mint)" : "returned false");
    if (!ok) throw new Error("execTransaction simulated to false");
  } catch (e: any) {
    console.error("SIMULATION REVERTED:", e.shortMessage || e.message);
    process.exit(1);
  }
  console.log("\nVALIDATION PASSED — custody mint path is sound on live state.");
}

main().catch((e) => { console.error(e); process.exit(1); });
