import { ethers } from "hardhat";

/**
 * Deploy WrappedBTH.
 *
 * Per ADR 0002 the three roles MUST be held by Gnosis Safes, not EOAs:
 *   WBTH_ADMIN_SAFE  – governance Safe (DEFAULT_ADMIN_ROLE: limits/roles)
 *   WBTH_MINTER_SAFE – validator Safe (MINTER_ROLE: t-of-n secp256k1)
 *   WBTH_PAUSER_SAFE – guardian Safe (PAUSER_ROLE: pause/unpause)
 *
 * The deployer key receives NO roles. Record the Safe addresses and their
 * thresholds in contracts/ethereum/README.md for every deployment.
 */
async function main() {
  const adminSafe = process.env.WBTH_ADMIN_SAFE;
  const minterSafe = process.env.WBTH_MINTER_SAFE;
  const pauserSafe = process.env.WBTH_PAUSER_SAFE;

  if (!adminSafe || !minterSafe || !pauserSafe) {
    throw new Error(
      "Set WBTH_ADMIN_SAFE, WBTH_MINTER_SAFE and WBTH_PAUSER_SAFE (Gnosis Safe addresses, per ADR 0002)"
    );
  }

  const [deployer] = await ethers.getSigners();
  console.log(`Deployer (receives NO roles): ${deployer.address}`);
  console.log(`Admin (governance) Safe:      ${adminSafe}`);
  console.log(`Minter (validator) Safe:      ${minterSafe}`);
  console.log(`Pauser (guardian) Safe:       ${pauserSafe}`);

  const WrappedBTH = await ethers.getContractFactory("WrappedBTH");
  const wbth = await WrappedBTH.deploy(adminSafe, minterSafe, pauserSafe);
  await wbth.waitForDeployment();

  console.log(`WrappedBTH deployed at: ${await wbth.getAddress()}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
