import { ethers } from "hardhat";

/**
 * Deploy ONE 2-of-3 Gnosis Safe for the wBTH bridge custody (#1011, #866).
 *
 * Per ADR 0002 the WrappedBTH admin/minter/pauser roles must be held by a
 * Gnosis Safe, not an EOA. The maintainer-ratified custody model for the
 * testnet bootstrap is a SINGLE 2-of-3 Safe used for all three roles, owned by
 * the three provisioned owner EOAs (BRIDGE_SAFE_OWNER_1/2/3, see
 * scripts/bridge-testnet-accounts.sh). The deployer key only pays gas and
 * receives NO Safe ownership.
 *
 * The Safe is created by calling the CANONICAL Safe v1.3.0 SafeProxyFactory
 * directly (no SDK / no network service dependency): factory
 * `createProxyWithNonce(singleton, initializer, saltNonce)` where `initializer`
 * is `Safe.setup(owners, threshold, 0x0, 0x, fallbackHandler, 0x0, 0, 0x0)`.
 *
 * Canonical Safe v1.3.0 addresses (identical on Ethereum mainnet and Sepolia;
 * pinned from https://github.com/safe-global/safe-deployments — v1.3.0
 * "canonical"). All four were verified live on Sepolia (chainId 11155111) to
 * have deployed bytecode via `cast code`. Override per-network via env.
 */
const SAFE_PROXY_FACTORY =
  process.env.SAFE_PROXY_FACTORY_ADDRESS ||
  "0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2"; // GnosisSafeProxyFactory v1.3.0
const SAFE_SINGLETON =
  process.env.SAFE_SINGLETON_ADDRESS ||
  "0xd9Db270c1B5E3Bd161E8c8503c55cEABeE709552"; // GnosisSafe (L1) v1.3.0
const SAFE_FALLBACK_HANDLER =
  process.env.SAFE_FALLBACK_HANDLER_ADDRESS ||
  "0xf48f2B2d2a534e402487b3ee7C18c33Aec0Fe5e4"; // CompatibilityFallbackHandler v1.3.0

const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

// Minimal ABIs — the canonical v1.3.0 contracts. In v1.3.0 the ProxyCreation
// event args are both NON-indexed (this changed to `indexed proxy` in v1.4.1).
const FACTORY_ABI = [
  "function createProxyWithNonce(address _singleton, bytes initializer, uint256 saltNonce) returns (address proxy)",
  "event ProxyCreation(address proxy, address singleton)",
];
const SAFE_ABI = [
  "function setup(address[] _owners, uint256 _threshold, address to, bytes data, address fallbackHandler, address paymentToken, uint256 payment, address paymentReceiver)",
  "function getOwners() view returns (address[])",
  "function getThreshold() view returns (uint256)",
];

function requireOwner(name: string): string {
  const v = process.env[name];
  if (!v || !ethers.isAddress(v)) {
    throw new Error(
      `${name} must be a valid EOA address (set the three BRIDGE_SAFE_OWNER_{1,2,3} owner addresses)`
    );
  }
  return ethers.getAddress(v);
}

async function assertDeployed(label: string, address: string) {
  const code = await ethers.provider.getCode(address);
  if (code === "0x" || code.length <= 2) {
    throw new Error(
      `${label} has no bytecode at ${address} on this network — wrong chain or ` +
        `wrong Safe release. Deploy against Sepolia (or a Sepolia fork), or ` +
        `override SAFE_PROXY_FACTORY_ADDRESS / SAFE_SINGLETON_ADDRESS / ` +
        `SAFE_FALLBACK_HANDLER_ADDRESS.`
    );
  }
}

/**
 * Deploy the 2-of-3 Safe and return its address. Idempotent-ish: if a
 * SAFE_ADDRESS env is already set, returns it without deploying. Exported so
 * deploy-all.ts can chain Safe → WrappedBTH in one process.
 */
export async function deploySafe(): Promise<string> {
  // Idempotent-ish: if a Safe address is already recorded, do nothing. This
  // lets deploy-all.ts / a re-run skip a Safe that was already created.
  if (process.env.SAFE_ADDRESS && ethers.isAddress(process.env.SAFE_ADDRESS)) {
    const existing = ethers.getAddress(process.env.SAFE_ADDRESS);
    console.log(`SAFE_ADDRESS already set (${existing}); skipping Safe deploy.`);
    console.log(`SAFE_ADDRESS=${existing}`);
    return existing;
  }

  const owners = [
    requireOwner("BRIDGE_SAFE_OWNER_1"),
    requireOwner("BRIDGE_SAFE_OWNER_2"),
    requireOwner("BRIDGE_SAFE_OWNER_3"),
  ];
  const uniqueOwners = new Set(owners.map((o) => o.toLowerCase()));
  if (uniqueOwners.size !== owners.length) {
    throw new Error("BRIDGE_SAFE_OWNER_{1,2,3} must be three DISTINCT addresses");
  }

  const threshold = Number(process.env.SAFE_THRESHOLD || "2");
  if (!Number.isInteger(threshold) || threshold < 1 || threshold > owners.length) {
    throw new Error(
      `SAFE_THRESHOLD must be an integer in [1, ${owners.length}] (default 2)`
    );
  }

  const [deployer] = await ethers.getSigners();
  console.log(`Network:            ${(await ethers.provider.getNetwork()).chainId}`);
  console.log(`Deployer (gas only): ${deployer.address}`);
  console.log(`Safe owners:        ${owners.join(", ")}`);
  console.log(`Safe threshold:     ${threshold}-of-${owners.length}`);
  console.log(`ProxyFactory:       ${SAFE_PROXY_FACTORY}`);
  console.log(`Safe singleton:     ${SAFE_SINGLETON}`);
  console.log(`Fallback handler:   ${SAFE_FALLBACK_HANDLER}`);

  // Fail loudly + early if the canonical contracts are not on this network.
  await assertDeployed("SafeProxyFactory", SAFE_PROXY_FACTORY);
  await assertDeployed("Safe singleton", SAFE_SINGLETON);
  await assertDeployed("Fallback handler", SAFE_FALLBACK_HANDLER);

  const safeInterface = new ethers.Interface(SAFE_ABI);
  const initializer = safeInterface.encodeFunctionData("setup", [
    owners,
    threshold,
    ZERO_ADDRESS, // to (no module/delegatecall setup)
    "0x", // data
    SAFE_FALLBACK_HANDLER,
    ZERO_ADDRESS, // paymentToken
    0, // payment
    ZERO_ADDRESS, // paymentReceiver
  ]);

  // A fresh salt per run avoids CREATE2 collisions with a previously deployed
  // Safe that had identical owners/threshold. Override SAFE_SALT_NONCE for a
  // deterministic address if you need one.
  const saltNonce = process.env.SAFE_SALT_NONCE || Date.now().toString();

  const factory = new ethers.Contract(SAFE_PROXY_FACTORY, FACTORY_ABI, deployer);
  console.log(`Deploying Safe (saltNonce=${saltNonce}) ...`);
  const tx = await factory.createProxyWithNonce(SAFE_SINGLETON, initializer, saltNonce);
  const receipt = await tx.wait();

  let safeAddress: string | undefined;
  for (const log of receipt!.logs) {
    try {
      const parsed = factory.interface.parseLog({
        topics: [...log.topics],
        data: log.data,
      });
      if (parsed && parsed.name === "ProxyCreation") {
        safeAddress = parsed.args.proxy as string;
        break;
      }
    } catch {
      // not a ProxyCreation log
    }
  }
  if (!safeAddress) {
    throw new Error("ProxyCreation event not found in receipt — Safe deploy failed");
  }
  safeAddress = ethers.getAddress(safeAddress);

  // Verify the deployed Safe really is the 2-of-3 we asked for.
  const safe = new ethers.Contract(safeAddress, SAFE_ABI, deployer);
  const deployedOwners: string[] = (await safe.getOwners()).map((o: string) =>
    ethers.getAddress(o)
  );
  const deployedThreshold = Number(await safe.getThreshold());

  const ownersMatch =
    deployedOwners.length === owners.length &&
    owners.every((o) => deployedOwners.map((d) => d.toLowerCase()).includes(o.toLowerCase()));
  if (!ownersMatch) {
    throw new Error(
      `Deployed Safe owners ${deployedOwners.join(",")} != requested ${owners.join(",")}`
    );
  }
  if (deployedThreshold !== threshold) {
    throw new Error(
      `Deployed Safe threshold ${deployedThreshold} != requested ${threshold}`
    );
  }

  console.log("");
  console.log(`Safe deployed and verified:`);
  console.log(`  getThreshold() = ${deployedThreshold}`);
  console.log(`  getOwners()    = ${deployedOwners.join(", ")}`);
  console.log(`  tx hash        = ${tx.hash}`);
  console.log("");
  // Machine-parseable line for deploy-all.ts / the runbook.
  console.log(`SAFE_ADDRESS=${safeAddress}`);
  return safeAddress;
}

// Only run as a standalone script when invoked directly (not when imported by
// deploy-all.ts). Hardhat runs scripts via `require.main`.
if (require.main === module) {
  deploySafe().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
