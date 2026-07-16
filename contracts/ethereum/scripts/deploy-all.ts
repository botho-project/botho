import { ethers } from "hardhat";
import { deploySafe } from "./deploy-safe";

/**
 * One-shot bridge custody + token bring-up (#1011, #866).
 *
 * Chains, in a single process:
 *   1. deploy the 2-of-3 Gnosis Safe (scripts/deploy-safe.ts) from the three
 *      BRIDGE_SAFE_OWNER_{1,2,3} owner EOAs,
 *   2. wire that ONE Safe as all three WrappedBTH roles (admin/minter/pauser)
 *      per the maintainer-ratified single-Safe custody model,
 *   3. deploy WrappedBTH with the Safe holding every role (the deployer gets
 *      none — ADR 0002),
 *   4. print the addresses + Etherscan links.
 *
 * This does NOT modify scripts/deploy.ts; it reuses the same constructor shape
 * (adminSafe, minterSafe, pauserSafe). If you prefer discrete steps, run
 * deploy-safe.ts, copy SAFE_ADDRESS into WBTH_ADMIN_SAFE/MINTER_SAFE/PAUSER_SAFE
 * in .env, then run deploy.ts — this script is just that sequence automated.
 */
async function main() {
  const safeAddress = await deploySafe();

  // Single-Safe custody: the one 2-of-3 Safe is admin, minter and pauser.
  const adminSafe = safeAddress;
  const minterSafe = safeAddress;
  const pauserSafe = safeAddress;

  const [deployer] = await ethers.getSigners();
  console.log("");
  console.log(`Deploying WrappedBTH (deployer ${deployer.address} receives NO roles)`);
  console.log(`  admin/minter/pauser Safe = ${safeAddress}`);

  const WrappedBTH = await ethers.getContractFactory("WrappedBTH");
  const wbth = await WrappedBTH.deploy(adminSafe, minterSafe, pauserSafe);
  await wbth.waitForDeployment();
  const wbthAddress = ethers.getAddress(await wbth.getAddress());

  const chainId = Number((await ethers.provider.getNetwork()).chainId);
  const explorer =
    chainId === 11155111
      ? "https://sepolia.etherscan.io/address/"
      : chainId === 1
        ? "https://etherscan.io/address/"
        : null;

  console.log("");
  console.log("=== Bridge custody deployed ===");
  console.log(`SAFE_ADDRESS=${safeAddress}`);
  console.log(`WBTH_ADDRESS=${wbthAddress}`);
  if (explorer) {
    console.log("");
    console.log(`Safe:       ${explorer}${safeAddress}`);
    console.log(`WrappedBTH: ${explorer}${wbthAddress}`);
  }
  console.log("");
  console.log("Record these in contracts/ethereum/README.md, then verify with:");
  console.log(
    `  npx hardhat verify --network sepolia ${wbthAddress} ${safeAddress} ${safeAddress} ${safeAddress}`
  );
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
